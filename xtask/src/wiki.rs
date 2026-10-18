//! `cargo xtask wiki <dir>`: writes the repository's documents in `docs/` as
//! GitHub wiki pages into `<dir>`, a checkout of the wiki. `docs/` stays the
//! source; the wiki is a copy that `.github/workflows/wiki.yml` refreshes on
//! every push to main that changes it.
//!
//! The wiki is flat, so every document becomes a page named in [`page_name`],
//! and links between documents become links between pages. A link to any
//! other file in the repository becomes a link to it on GitHub. The command
//! reference is too long for one wiki page, so its commands and its types
//! each get a page of their own. Only tracked files are read.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::Command;

/// Where links to files outside `docs/` point.
const REPO_URL: &str = "https://github.com/loombridge/beatlayer";

/// The command reference's sections that get a page of their own, with the
/// page's name.
const COMMAND_PAGES: [(&str, &str); 2] = [
    ("## Commands", "Command-reference-commands"),
    ("## Types", "Command-reference-types"),
];

/// The planning documents and the other pages outside the specs, in the
/// sidebar's order.
const NAMED: [(&str, &str); 12] = [
    ("docs/01-roadmap.md", "Roadmap"),
    ("docs/02-architecture.md", "Architecture"),
    ("docs/03-build-plan.md", "Build-plan"),
    ("docs/04-parity-matrix.md", "Parity-matrix"),
    ("docs/05-improvements.md", "Improvements-over-Live"),
    ("docs/progress.md", "Progress"),
    ("docs/commands.md", "Command-reference"),
    ("docs/security.md", "Security"),
    ("docs/ui-parity/README.md", "UI-parity-reference"),
    ("docs/specs/README.md", "Behaviour-specs"),
    ("docs/specs/TEMPLATE.md", "Spec-template"),
    ("docs/README.md", "Docs"),
];

/// A wiki page: its name (the file name without `.md`) and its Markdown.
#[derive(Debug, PartialEq, Eq)]
pub struct Page {
    pub name: String,
    pub text: String,
}

/// Writes the wiki into `out`, replacing every page already there.
pub fn write(root: &Path, out: &Path) -> Result<(), String> {
    let tracked = git(root, &["ls-files", "docs", "."])?;
    let tracked: BTreeSet<String> = tracked.lines().map(str::to_string).collect();
    let mut docs = BTreeMap::new();
    for path in tracked
        .iter()
        .filter(|p| p.starts_with("docs/") && p.ends_with(".md"))
    {
        let text = std::fs::read_to_string(root.join(path))
            .map_err(|e| format!("couldn't read {path}: {e}"))?;
        docs.insert(path.clone(), text);
    }
    let commit = git(root, &["rev-parse", "HEAD"])?.trim().to_string();
    let pages = pages(&docs, &tracked, &commit)?;

    std::fs::create_dir_all(out).map_err(|e| format!("couldn't create {}: {e}", out.display()))?;
    let names: BTreeSet<String> = pages.iter().map(|p| format!("{}.md", p.name)).collect();
    let existing =
        std::fs::read_dir(out).map_err(|e| format!("couldn't read {}: {e}", out.display()))?;
    for entry in existing {
        let entry = entry.map_err(|e| e.to_string())?;
        let file = entry.file_name().to_string_lossy().to_string();
        if file.ends_with(".md") && !names.contains(&file) {
            std::fs::remove_file(entry.path())
                .map_err(|e| format!("couldn't remove {file}: {e}"))?;
        }
    }
    for page in &pages {
        let path = out.join(format!("{}.md", page.name));
        std::fs::write(&path, &page.text)
            .map_err(|e| format!("couldn't write {}: {e}", path.display()))?;
    }
    println!("wrote {} wiki pages to {}", pages.len(), out.display());
    Ok(())
}

/// Every page of the wiki, from the documents (path to text) and the
/// repository's tracked files, at `commit`.
pub fn pages(
    docs: &BTreeMap<String, String>,
    tracked: &BTreeSet<String>,
    commit: &str,
) -> Result<Vec<Page>, String> {
    let mut names = BTreeMap::new();
    let mut taken = BTreeMap::new();
    for (path, text) in docs {
        let name = page_name(path, text);
        if let Some(other) = taken.insert(name.clone(), path.clone()) {
            return Err(format!(
                "{other} and {path} would both be the wiki page {name}"
            ));
        }
        names.insert(path.clone(), name);
    }

    // The pages before their links are rewritten: (source path, name, text).
    let mut drafts = Vec::new();
    for (path, text) in docs {
        let name = &names[path];
        let text = drop_title(text, name, path);
        if path == "docs/commands.md" {
            drafts.extend(
                split_commands(&text)
                    .into_iter()
                    .map(|(n, t)| (path.clone(), n, t)),
            );
        } else {
            drafts.push((path.clone(), name.clone(), text));
        }
    }
    let anchors: BTreeMap<String, BTreeSet<String>> = drafts
        .iter()
        .map(|(_, name, text)| (name.clone(), anchors(text)))
        .collect();
    // The pages a document's anchors can be on, its own page first.
    let pages_of = |path: &str| -> Vec<String> {
        let mut found: Vec<String> = names.get(path).into_iter().cloned().collect();
        if path == "docs/commands.md" {
            found.extend(COMMAND_PAGES.iter().map(|(_, n)| n.to_string()));
        }
        found
    };
    let resolve = |path: &str, anchor: &str| -> Option<String> {
        let candidates = pages_of(path);
        let first = candidates.first()?.clone();
        if anchor.is_empty() {
            return Some(first);
        }
        candidates
            .into_iter()
            .find(|n| anchors.get(n).is_some_and(|a| a.contains(anchor)))
            .or(Some(first))
    };

    let mut pages = Vec::new();
    for (path, name, text) in &drafts {
        let dir = path.rsplit_once('/').map_or("", |(d, _)| d);
        let text = rewrite_links(text, |target| {
            let (file, anchor) = target.split_once('#').unwrap_or((target, ""));
            if file.is_empty() {
                // A link within the document, which may now be on a sibling page.
                let page = resolve(path, anchor)?;
                return (page != *name).then(|| format!("{page}#{anchor}"));
            }
            if file.contains("://") || file.starts_with("mailto:") || file.starts_with('/') {
                return None;
            }
            let full = normalise(&format!("{dir}/{file}"))?;
            if docs.contains_key(&full) {
                let page = resolve(&full, anchor)?;
                return Some(if anchor.is_empty() {
                    page
                } else {
                    format!("{page}#{anchor}")
                });
            }
            let fragment = if anchor.is_empty() {
                String::new()
            } else {
                format!("#{anchor}")
            };
            if tracked.contains(&full) {
                Some(format!("{REPO_URL}/blob/main/{full}{fragment}"))
            } else if tracked.iter().any(|t| t.starts_with(&format!("{full}/"))) {
                Some(format!("{REPO_URL}/tree/main/{full}{fragment}"))
            } else {
                None
            }
        });
        pages.push(Page {
            name: name.clone(),
            text,
        });
    }

    pages.push(Page {
        name: "Home".into(),
        text: home(docs, &names),
    });
    pages.push(Page {
        name: "_Sidebar".into(),
        text: sidebar(docs, &names),
    });
    pages.push(Page {
        name: "_Footer".into(),
        text: format!(
            "Copied from [`docs/`]({REPO_URL}/tree/main/docs) at [`{short}`]({REPO_URL}/commit/{commit}). \
             Change a document by pull request in the repository: edits made here are overwritten.\n",
            short = &commit[..commit.len().min(7)],
        ),
    });
    Ok(pages)
}

/// The wiki page a document becomes: a fixed name for the documents in
/// [`NAMED`], `UI-parity-<file>` for the rest of the UI parity reference, and
/// a spec's file name, first letter capitalised, with any word its title
/// writes in capitals written that way (`clap-hosting.md` is `CLAP-hosting`).
fn page_name(path: &str, text: &str) -> String {
    if let Some((_, name)) = NAMED.iter().find(|(p, _)| *p == path) {
        return name.to_string();
    }
    let file = path
        .rsplit('/')
        .next()
        .unwrap_or(path)
        .trim_end_matches(".md");
    if path.starts_with("docs/ui-parity/") {
        return format!("UI-parity-{file}");
    }
    let title = title(text).unwrap_or("");
    let cased: BTreeMap<String, &str> = title
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().skip(1).any(char::is_uppercase))
        .map(|w| (w.to_lowercase(), w))
        .collect();
    let words: Vec<String> = file
        .split('-')
        .map(|w| {
            cased
                .get(w)
                .map_or_else(|| w.to_string(), |c| c.to_string())
        })
        .collect();
    capitalise(&words.join("-"))
}

/// A document's first-level title.
fn title(text: &str) -> Option<&str> {
    text.lines()
        .find_map(|l| l.strip_prefix("# "))
        .map(str::trim)
}

/// The document without its title where the page's name already says it: the
/// wiki shows the name above the page. A spec whose title says more than its
/// name keeps it.
fn drop_title(text: &str, name: &str, path: &str) -> String {
    let Some(title) = title(text) else {
        return text.to_string();
    };
    let spec = path.starts_with("docs/specs/") && !NAMED.iter().any(|(p, _)| *p == path);
    let plain = |s: &str| s.to_lowercase().replace(['-', ' '], "");
    let bare = title.strip_prefix("The ").unwrap_or(title);
    if spec && plain(bare) != plain(name) {
        return text.to_string();
    }
    let line = format!("# {title}");
    let mut lines = text.lines();
    let mut out = String::new();
    let mut dropped = false;
    for l in lines.by_ref() {
        if !dropped && l.trim_end() == line {
            dropped = true;
            continue;
        }
        if dropped && out.is_empty() && l.trim().is_empty() {
            continue;
        }
        out.push_str(l);
        out.push('\n');
    }
    out
}

/// The command reference as three pages: its commands, its types, and the
/// rest, which links to them where they were.
fn split_commands(text: &str) -> Vec<(String, String)> {
    let mut main = String::new();
    let mut parts: Vec<(String, String)> = COMMAND_PAGES
        .iter()
        .map(|(_, n)| (n.to_string(), String::new()))
        .collect();
    let mut current: Option<usize> = None;
    let mut fence = false;
    for line in text.lines() {
        if is_fence(line) {
            fence = !fence;
        }
        if !fence && line.starts_with("## ") {
            current = COMMAND_PAGES
                .iter()
                .position(|(h, _)| line.trim_end() == *h);
            if let Some(i) = current {
                let (heading, name) = COMMAND_PAGES[i];
                let label = heading.trim_start_matches("## ");
                main.push_str(&format!(
                    "{heading}\n\nOn a page of its own: [{label}]({name}).\n\n"
                ));
            }
        }
        match current {
            Some(i) => {
                parts[i].1.push_str(line);
                parts[i].1.push('\n');
            }
            None => {
                main.push_str(line);
                main.push('\n');
            }
        }
    }
    let mut pages = vec![("Command-reference".to_string(), main)];
    pages.extend(parts);
    pages
}

fn is_fence(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("```") || t.starts_with("~~~")
}

/// The anchors GitHub gives a page's headings, and its HTML anchors.
fn anchors(text: &str) -> BTreeSet<String> {
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    let mut out = BTreeSet::new();
    let mut fence = false;
    for line in text.lines() {
        if is_fence(line) {
            fence = !fence;
            continue;
        }
        if fence {
            continue;
        }
        for marker in ["<a id=\"", "<a name=\""] {
            let mut rest = line;
            while let Some(at) = rest.find(marker) {
                rest = &rest[at + marker.len()..];
                if let Some(end) = rest.find('"') {
                    out.insert(rest[..end].to_string());
                }
            }
        }
        let hashes = line.chars().take_while(|c| *c == '#').count();
        if !(1..=6).contains(&hashes) || !line[hashes..].starts_with(' ') {
            continue;
        }
        let slug = slug(&line[hashes..]);
        let count = seen.entry(slug.clone()).or_insert(0);
        out.insert(if *count == 0 {
            slug.clone()
        } else {
            format!("{slug}-{count}")
        });
        *count += 1;
    }
    out
}

/// GitHub's anchor for a heading: its text lower-cased, without punctuation
/// other than hyphens and underscores, with spaces as hyphens.
fn slug(heading: &str) -> String {
    let text = rewrite_links(heading, |_| None);
    // A link's text stands for the link.
    let mut plain = String::new();
    let mut rest = text.as_str();
    while let Some(open) = rest.find('[') {
        plain.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        match after
            .find("](")
            .and_then(|mid| after[mid..].find(')').map(|end| (mid, mid + end)))
        {
            Some((mid, end)) => {
                plain.push_str(&after[..mid]);
                rest = &after[end + 1..];
            }
            None => {
                plain.push('[');
                rest = after;
            }
        }
    }
    plain.push_str(rest);
    plain
        .trim()
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | ' '))
        .map(|c| if c == ' ' { '-' } else { c })
        .collect()
}

/// Applies `map` to the target of every inline link outside code, keeping
/// the target where `map` gives `None`.
fn rewrite_links(text: &str, mut map: impl FnMut(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut fence = false;
    for line in text.split_inclusive('\n') {
        if is_fence(line) {
            fence = !fence;
        }
        if fence || is_fence(line) {
            out.push_str(line);
            continue;
        }
        let bytes = line.as_bytes();
        let mut i = 0;
        while i < line.len() {
            if bytes[i] == b'`' {
                // Skip a code span: up to the next run of as many backticks.
                let run = line[i..].bytes().take_while(|b| *b == b'`').count();
                let ticks = &line[i..i + run];
                match line[i + run..].find(ticks) {
                    Some(end) => {
                        let stop = i + run + end + run;
                        out.push_str(&line[i..stop]);
                        i = stop;
                    }
                    None => {
                        out.push_str(ticks);
                        i += run;
                    }
                }
                continue;
            }
            if line[i..].starts_with("](") {
                let start = i + 2;
                let mut depth = 0;
                let mut end = None;
                for (j, c) in line[start..].char_indices() {
                    match c {
                        '(' => depth += 1,
                        ')' if depth == 0 => {
                            end = Some(start + j);
                            break;
                        }
                        ')' => depth -= 1,
                        ' ' => break,
                        _ => {}
                    }
                }
                if let Some(end) = end {
                    let target = &line[start..end];
                    out.push_str("](");
                    out.push_str(&map(target).unwrap_or_else(|| target.to_string()));
                    out.push(')');
                    i = end + 1;
                    continue;
                }
            }
            let c = line[i..].chars().next().unwrap_or(' ');
            out.push(c);
            i += c.len_utf8();
        }
    }
    out
}

/// A repository path with its `.` and `..` parts resolved. A `..` at the
/// root stays there, as it does on GitHub.
fn normalise(path: &str) -> Option<String> {
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            _ => parts.push(part),
        }
    }
    Some(parts.join("/"))
}

/// The specs, as (page name, title), by name.
fn specs<'a>(
    docs: &'a BTreeMap<String, String>,
    names: &BTreeMap<String, String>,
) -> Vec<(String, &'a str)> {
    let mut specs: Vec<(String, &str)> = docs
        .iter()
        .filter(|(p, _)| p.starts_with("docs/specs/") && !NAMED.iter().any(|(n, _)| n == p))
        .map(|(p, t)| (names[p].clone(), title(t).unwrap_or("")))
        .collect();
    specs.sort_by_key(|(n, _)| n.to_lowercase());
    specs
}

/// The planning documents and references with their page names, in order.
fn listed<'a>(names: &'a BTreeMap<String, String>, prefix: &str) -> Vec<(&'a str, &'a str)> {
    let mut out: Vec<(&str, &str)> = NAMED
        .iter()
        .filter(|(p, _)| p.starts_with(prefix) && names.contains_key(*p))
        .map(|(p, n)| (*p, *n))
        .filter(|(p, _)| prefix != "docs/" || !p["docs/".len()..].contains('/'))
        .map(|(p, n)| (p, names.get(p).map_or(n, String::as_str)))
        .collect();
    if prefix == "docs/ui-parity/" {
        let extra = names
            .iter()
            .filter(|(p, _)| p.starts_with(prefix) && !NAMED.iter().any(|(n, _)| n == p));
        out.extend(extra.map(|(p, n)| (p.as_str(), n.as_str())));
    }
    out
}

/// A page's name as its title: hyphens as spaces.
fn label(name: &str) -> String {
    name.replace('-', " ")
}

fn home(docs: &BTreeMap<String, String>, names: &BTreeMap<String, String>) -> String {
    let mut out = String::from(
        "These pages are a copy of the documents in the repository's \
         [`docs/`](https://github.com/loombridge/beatlayer/tree/main/docs) folder, \
         updated each time they change on main.\n\n## Planning\n\n",
    );
    for (_, name) in listed(names, "docs/") {
        out.push_str(&format!("- [{}]({name})\n", label(name)));
    }
    out.push_str("\n## UI parity reference\n\n");
    for (_, name) in listed(names, "docs/ui-parity/") {
        let short = name.strip_prefix("UI-parity-").unwrap_or(name);
        out.push_str(&format!("- [{}]({name})\n", label(&capitalise(short))));
    }
    out.push_str("\n## Behaviour specs\n\n[How specs are written](Behaviour-specs).\n\n| Spec | What it covers |\n|---|---|\n");
    for (name, title) in specs(docs, names) {
        out.push_str(&format!(
            "| [{}]({name}) | {} |\n",
            label(&name),
            title.replace('|', "\\|")
        ));
    }
    out
}

fn sidebar(docs: &BTreeMap<String, String>, names: &BTreeMap<String, String>) -> String {
    let mut out = String::from("**[Home](Home)**\n\n**Planning**\n\n");
    for (_, name) in listed(names, "docs/") {
        out.push_str(&format!("- [{}]({name})\n", label(name)));
    }
    out.push_str("\n**UI parity**\n\n");
    for (_, name) in listed(names, "docs/ui-parity/") {
        let short = name.strip_prefix("UI-parity-").unwrap_or(name);
        out.push_str(&format!("- [{}]({name})\n", label(&capitalise(short))));
    }
    let specs = specs(docs, names);
    out.push_str(&format!(
        "\n**[Behaviour specs](Behaviour-specs)**\n\n<details>\n<summary>All {} specs</summary>\n\n",
        specs.len()
    ));
    for (name, _) in specs {
        out.push_str(&format!("- [{}]({name})\n", label(&name)));
    }
    out.push_str("\n</details>\n");
    out
}

fn capitalise(s: &str) -> String {
    let mut chars = s.chars();
    chars
        .next()
        .map_or_else(String::new, |f| f.to_uppercase().chain(chars).collect())
}

fn git(root: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .map_err(|e| format!("couldn't run git: {e}"))?;
    if output.status.success() {
        String::from_utf8(output.stdout).map_err(|e| e.to_string())
    } else {
        Err(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn docs(files: &[(&str, &str)]) -> BTreeMap<String, String> {
        files
            .iter()
            .map(|(p, t)| (p.to_string(), t.to_string()))
            .collect()
    }

    fn page<'a>(pages: &'a [Page], name: &str) -> &'a str {
        &pages
            .iter()
            .find(|p| p.name == name)
            .unwrap_or_else(|| panic!("no page {name}"))
            .text
    }

    #[test]
    fn links_between_documents_become_links_between_pages() {
        let docs = docs(&[
            (
                "docs/01-roadmap.md",
                "# 01. Roadmap\n\nSee [the plan](03-build-plan.md#how) and [transport](specs/transport.md).\n",
            ),
            ("docs/03-build-plan.md", "# 03. Build plan\n\n## How\n"),
            (
                "docs/specs/transport.md",
                "# The Control Bar's transport: Play and Stop\n\nBack to [the plan](../03-build-plan.md), [code](../../crates/engine/src/lib.rs), [web](https://example.com/x.md).\n",
            ),
        ]);
        let tracked = ["crates/engine/src/lib.rs"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let pages = pages(&docs, &tracked, "0123456789abcdef").unwrap();
        assert_eq!(
            page(&pages, "Roadmap"),
            "See [the plan](Build-plan#how) and [transport](Transport).\n"
        );
        assert_eq!(
            page(&pages, "Transport"),
            "# The Control Bar's transport: Play and Stop\n\nBack to [the plan](Build-plan), \
             [code](https://github.com/loombridge/beatlayer/blob/main/crates/engine/src/lib.rs), [web](https://example.com/x.md).\n"
        );
        assert!(page(&pages, "_Footer").contains(
            "[`0123456`](https://github.com/loombridge/beatlayer/commit/0123456789abcdef)"
        ));
    }

    #[test]
    fn code_keeps_its_brackets() {
        let text = "`[a](b.md)` and\n```\n[c](d.md)\n```\n[e](f.md)\n";
        let out = rewrite_links(text, |_| Some("X".into()));
        assert_eq!(out, "`[a](b.md)` and\n```\n[c](d.md)\n```\n[e](X)\n");
    }

    #[test]
    fn the_command_reference_is_split_and_its_anchors_follow() {
        let docs = docs(&[
            (
                "docs/commands.md",
                "# Command reference\n\nUses [Types](#types) and [`foo`](#foo).\n\n## Summary\n\n## Commands\n\n### `foo`\n\nTakes [`Bar`](#bar).\n\n## Types\n\n### `Bar`\n",
            ),
            (
                "docs/02-architecture.md",
                "# 02. Architecture\n\n[bar](commands.md#bar)\n",
            ),
        ]);
        let pages = pages(&docs, &BTreeSet::new(), "abc").unwrap();
        let main = page(&pages, "Command-reference");
        assert!(
            main.starts_with("Uses [Types](#types) and [`foo`](Command-reference-commands#foo)."),
            "{main}"
        );
        assert!(main.contains(
            "## Commands\n\nOn a page of its own: [Commands](Command-reference-commands)."
        ));
        assert!(!main.contains("### `foo`"));
        assert!(
            page(&pages, "Command-reference-commands")
                .contains("Takes [`Bar`](Command-reference-types#bar).")
        );
        assert_eq!(
            page(&pages, "Architecture"),
            "[bar](Command-reference-types#bar)\n"
        );
    }

    #[test]
    fn anchors_are_githubs() {
        let a = anchors(
            "# A b\n## `stems.open`: Open\n## A b\n```\n# not\n```\n## [Link](x.md) here\n",
        );
        assert_eq!(
            a,
            ["a-b", "a-b-1", "stemsopen-open", "link-here"]
                .iter()
                .map(|s| s.to_string())
                .collect()
        );
    }

    #[test]
    fn spec_pages_are_named_by_their_files() {
        assert_eq!(
            page_name("docs/specs/clap-hosting.md", "# CLAP hosting\n"),
            "CLAP-hosting"
        );
        assert_eq!(
            page_name("docs/specs/app-shell.md", "# The app shell: start-up\n"),
            "App-shell"
        );
        assert_eq!(
            page_name("docs/ui-parity/menus.md", "# Menus\n"),
            "UI-parity-menus"
        );
    }

    /// The real documents make a wiki: no two share a page, and every link
    /// between them lands on a page that has its anchor.
    #[test]
    fn the_documents_make_a_wiki() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let Ok(tracked) = git(root, &["ls-files"]) else {
            return;
        };
        let tracked: BTreeSet<String> = tracked.lines().map(str::to_string).collect();
        let docs: BTreeMap<String, String> = tracked
            .iter()
            .filter(|p| p.starts_with("docs/") && p.ends_with(".md"))
            .filter_map(|p| Some((p.clone(), std::fs::read_to_string(root.join(p)).ok()?)))
            .collect();
        if docs.is_empty() {
            return;
        }
        let pages = pages(&docs, &tracked, "HEAD").unwrap();
        let anchors: BTreeMap<&str, BTreeSet<String>> = pages
            .iter()
            .map(|p| (p.name.as_str(), anchors(&p.text)))
            .collect();
        let mut broken = Vec::new();
        for p in &pages {
            rewrite_links(&p.text, |target| {
                // Web links, and rustdoc paths the command reference keeps from doc comments.
                if target.contains("://") || target.contains("::") || target.starts_with("mailto:")
                {
                    return None;
                }
                let (page, anchor) = target.split_once('#').unwrap_or((target, ""));
                let page = if page.is_empty() {
                    p.name.as_str()
                } else {
                    page
                };
                match anchors.get(page) {
                    None => broken.push(format!("{}: {target}", p.name)),
                    Some(a) if !anchor.is_empty() && !a.contains(anchor) => {
                        broken.push(format!("{}: {target}", p.name));
                    }
                    Some(_) => {}
                }
                None
            });
        }
        assert!(
            broken.is_empty(),
            "{} links land nowhere:\n{}",
            broken.len(),
            broken.join("\n")
        );
    }
}

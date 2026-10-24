//! The command reference (WP-0.14, decisions D16 and D17).
//!
//! The command reference is generated from the command registry: every crate
//! that implements commands registers them, and this module builds the same
//! registry the app does, checks it (every command has a schema, a
//! description, a permission class, contexts, and bindings that name real
//! shortcuts in fitting contexts), and renders the reference.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use bl_commands::readback::{self, READBACK};
use bl_commands::{RegisterError, Registry, reference};

use crate::{ui_parity, wiki};

/// The generated reference, relative to the workspace root: an index and one
/// file per command namespace, so a command added to a namespace changes that
/// namespace's file alone.
pub const DIR: &str = "docs/commands";

/// The index's file name in [`DIR`].
const INDEX: &str = "README.md";

/// A crate's function that adds its commands to a registry.
type Register = fn(&mut Registry) -> Result<(), RegisterError>;

/// Each crate's function that registers its commands. Add a crate here when
/// it starts registering commands, so they're checked and listed.
const PROVIDERS: &[(&str, Register)] = &[
    ("bl-model", bl_model::registry::register),
    ("bl-project", bl_project::registry::register),
    ("bl-engine", bl_engine::transport::register),
    ("bl-engine", bl_engine::session::register),
    ("bl-shell-spec", bl_shell_spec::register),
];

/// Regenerates the command reference, after checking the registry, and
/// removes the files of namespaces it no longer has.
pub fn generate(root: &Path) -> Result<(), String> {
    let (registry, files) = build(root)?;
    let dir = root.join(DIR);
    std::fs::create_dir_all(&dir).map_err(|e| format!("couldn't create {DIR}: {e}"))?;
    for name in extra_files(&dir, &files)? {
        std::fs::remove_file(dir.join(&name))
            .map_err(|e| format!("couldn't remove {DIR}/{name}: {e}"))?;
    }
    for (name, text) in &files {
        std::fs::write(dir.join(name), text)
            .map_err(|e| format!("couldn't write {DIR}/{name}: {e}"))?;
        eprintln!("wrote {DIR}/{name}: {} lines", text.lines().count());
    }
    eprintln!("{} commands in {} files", registry.len(), files.len());
    Ok(())
}

/// Fails if the registry has a problem or a file of the command reference is
/// out of date, missing or extra.
pub fn check(root: &Path) -> Result<(), String> {
    let (registry, files) = build(root)?;
    let dir = root.join(DIR);
    let mut problems = Vec::new();
    for (name, expected) in &files {
        match std::fs::read_to_string(dir.join(name)) {
            Ok(actual) if actual.replace("\r\n", "\n") == *expected => {}
            Ok(_) => problems.push(format!("{DIR}/{name} is out of date")),
            Err(_) => problems.push(format!("{DIR}/{name} is missing")),
        }
    }
    for name in extra_files(&dir, &files)? {
        problems.push(format!("{DIR}/{name} is not a file of the reference"));
    }
    if !problems.is_empty() {
        return Err(format!(
            "the command reference differs from the command registry; regenerate it with \
             `cargo xtask commands`:\n  {}",
            problems.join("\n  ")
        ));
    }
    eprintln!(
        "{} commands, each with a schema, a description, and a permission class; their \
         bindings match the shortcut reference, and the command reference is in sync",
        registry.len()
    );
    Ok(())
}

/// The Markdown files in `dir` that the reference doesn't have.
fn extra_files(dir: &Path, files: &BTreeMap<String, String>) -> Result<Vec<String>, String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Ok(Vec::new());
    };
    let mut extra = Vec::new();
    for entry in entries {
        let name = entry
            .map_err(|e| format!("couldn't read {DIR}: {e}"))?
            .file_name()
            .to_string_lossy()
            .into_owned();
        if name.ends_with(".md") && !files.contains_key(&name) {
            extra.push(name);
        }
    }
    extra.sort();
    Ok(extra)
}

/// Checks only what needs no planning documents: every command has a schema,
/// a description, a permission class, and contexts. Its bindings and the
/// generated reference need the UI parity reference.
pub fn check_registry() -> Result<(), String> {
    let registry = catalogue()?;
    let problems = registry_problems(&registry);
    if !problems.is_empty() {
        return Err(format!(
            "the command registry has {} problem(s):\n  {}",
            problems.len(),
            problems.join("\n  ")
        ));
    }
    eprintln!(
        "{} commands, each with a schema, a description, and a permission class",
        registry.len()
    );
    eprintln!(
        "their bindings and the command reference: {}",
        ui_parity::ABSENT
    );
    Ok(())
}

/// The registry, checked, and the reference rendered from it: the file names
/// in [`DIR`] and their text.
fn build(root: &Path) -> Result<(Registry, BTreeMap<String, String>), String> {
    let registry = catalogue()?;
    let shortcuts = ui_parity::shortcut_table(root)?;
    let mut problems = registry_problems(&registry);
    problems.extend(
        registry
            .binding_problems(&shortcuts)
            .iter()
            .map(ToString::to_string),
    );
    if !problems.is_empty() {
        return Err(format!(
            "the command registry has {} problem(s):\n  {}",
            problems.len(),
            problems.join("\n  ")
        ));
    }
    let rendered = reference::markdown(&registry, &shortcuts)?;
    let files = split(&registry, &rendered)?;
    Ok((registry, files))
}

/// `text` cut at its `## ` headings outside code fences: each part's heading
/// (empty for what comes before the first) and its lines, heading included.
fn sections(text: &str) -> Vec<(&str, Vec<&str>)> {
    let mut parts = vec![("", Vec::new())];
    let mut fence = false;
    for line in text.lines() {
        if wiki::is_fence(line) {
            fence = !fence;
        }
        if !fence && line.starts_with("## ") {
            parts.push((line, Vec::new()));
        }
        if let Some((_, lines)) = parts.last_mut() {
            lines.push(line);
        }
    }
    parts
}

/// The generated reference cut into files: the index, with the introduction,
/// the permission classes, the namespaces and the types, and a file for each
/// namespace with its summary, its commands and its readback rows. Links
/// between the files are rewritten to point where their targets now are.
fn split(registry: &Registry, rendered: &str) -> Result<BTreeMap<String, String>, String> {
    let parts = sections(rendered);
    let part = |heading: &str| -> Result<&Vec<&str>, String> {
        parts
            .iter()
            .find(|(h, _)| *h == heading)
            .map(|(_, lines)| lines)
            .ok_or_else(|| format!("the generated reference has no `{heading}` section"))
    };
    let block = |lines: &[&str]| lines.join("\n").trim().to_string();
    let namespaces: BTreeSet<&str> = registry.iter().map(|spec| spec.area()).collect();

    // The summary's rows and the commands' text, by namespace.
    let summary = part("## Summary")?;
    let header = block(&summary[2..4]);
    let mut rows: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for line in summary.iter().filter(|l| l.starts_with("| [`")) {
        let id = line.split('`').nth(1).unwrap_or_default();
        rows.entry(id.split('.').next().unwrap_or_default())
            .or_default()
            .push(line);
    }
    let mut commands: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    let (mut area, mut fence) = ("", false);
    for line in part("## Commands")?.iter().skip(1) {
        if wiki::is_fence(line) {
            fence = !fence;
        }
        if !fence && let Some(name) = line.strip_prefix("### Area `") {
            area = name.trim_end_matches('`');
        } else if !fence && line.starts_with("#### `") {
            commands
                .entry(area)
                .or_default()
                .push(line[1..].to_string());
        } else {
            commands.entry(area).or_default().push((*line).to_string());
        }
    }

    // The introduction without its count, which would change with every
    // command added.
    let opening: Vec<&str> = part("")?
        .iter()
        .copied()
        .filter(|l| !(l.ends_with(" areas.") && l.starts_with(|c: char| c.is_ascii_digit())))
        .collect();
    let index = namespaces
        .iter()
        .map(|ns| format!("- [`{ns}`]({ns}.md)"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut texts = BTreeMap::new();
    texts.insert(
        INDEX.to_string(),
        [
            block(&opening),
            block(part("## Permission classes")?),
            format!(
                "## Namespaces\n\nThe commands, by the part of their ID before its first dot:\n\n{index}"
            ),
            readback::INTRO.trim().to_string(),
            block(part("## Types")?),
        ]
        .join("\n\n"),
    );
    for ns in &namespaces {
        let tables = readback::markdown(registry, READBACK, ns);
        let mut file = vec![
            format!(
                "# `{ns}` commands\n\nPart of [the command reference]({INDEX}), generated by \
                 `cargo xtask commands`. Don't edit it by hand."
            ),
            format!(
                "## Summary\n\n{header}\n{}",
                rows.get(ns).map(|r| r.join("\n")).unwrap_or_default()
            ),
            format!(
                "## Commands\n\n{}",
                block(&commands[ns].iter().map(String::as_str).collect::<Vec<_>>())
            ),
        ];
        if !tables.is_empty() {
            file.push(format!("## Readback\n\n{}", tables.trim()));
        }
        texts.insert(format!("{ns}.md"), file.join("\n\n"));
    }

    // A link to another file's heading names that file.
    let index_anchors = wiki::anchors(&texts[INDEX]);
    let homes: BTreeMap<String, &str> = registry
        .iter()
        .map(|spec| (wiki::slug(&format!("`{}`", spec.id)), spec.area()))
        .collect();
    let mut files = BTreeMap::new();
    for (name, text) in &texts {
        let own = wiki::anchors(text);
        let text = wiki::rewrite_links(text, |target| {
            let anchor = target.strip_prefix('#')?;
            if own.contains(anchor) {
                None
            } else if index_anchors.contains(anchor) {
                Some(format!("{INDEX}#{anchor}"))
            } else {
                homes.get(anchor).map(|ns| format!("{ns}.md#{anchor}"))
            }
        });
        files.insert(name.clone(), format!("{}\n", text.trim_end()));
    }
    Ok(files)
}

/// What is wrong with the registry itself: a part missing from a command, and
/// a command that changes something without saying where it reads back.
fn registry_problems(registry: &Registry) -> Vec<String> {
    let mut problems: Vec<String> = registry
        .problems()
        .iter()
        .map(ToString::to_string)
        .collect();
    problems.extend(readback::problems(registry, READBACK, |object| {
        bl_model::saved_schema(object.path()).map(|schema| schema.as_value().clone())
    }));
    problems
}

/// Every registered command, from every provider.
pub fn catalogue() -> Result<Registry, String> {
    let mut registry = Registry::new();
    for (provider, register) in PROVIDERS {
        register(&mut registry).map_err(|e| format!("{provider}: {e}"))?;
    }
    Ok(registry)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every command that changes something names where its effect reads
    /// back, and the place exists.
    #[test]
    fn every_command_that_changes_something_names_where_it_reads_back() {
        let registry = catalogue().expect("the providers register");
        assert_eq!(registry_problems(&registry), Vec::<String>::new());
    }

    /// The saved forms the readback table is checked against hold the fields
    /// the set saves, and only those.
    #[test]
    fn the_saved_forms_are_checked_by_their_fields() {
        let track = bl_model::saved_schema("track").expect("a track has a saved form");
        assert!(readback::has_field(track.as_value(), "routing.output"));
        assert!(readback::has_field(track.as_value(), "automation[].points"));
        assert!(!readback::has_field(track.as_value(), "routing.nothing"));
        let song = bl_model::saved_schema("song").expect("the song has a saved form");
        assert!(readback::has_field(song.as_value(), "settings.punch_in"));
        assert!(readback::has_field(song.as_value(), "scenes[].name"));
    }

    /// The real registry must be complete and its reference must render, with
    /// every command in its namespace's file and every link landing on a
    /// heading.
    #[test]
    fn the_repository_reference_renders() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("xtask sits in the workspace root");
        if !ui_parity::reference_present(root) {
            check_registry().expect("the command registry is complete");
            return;
        }
        let (registry, files) =
            build(root).expect("the command registry is complete and its reference renders");
        for spec in &registry {
            let heading = format!("\n### `{}`\n", spec.id);
            let found: Vec<&String> = files
                .iter()
                .filter(|(_, text)| text.contains(&heading))
                .map(|(name, _)| name)
                .collect();
            assert_eq!(found, [&format!("{}.md", spec.area())], "{}", spec.id);
        }
        let anchors: BTreeMap<&str, BTreeSet<String>> = files
            .iter()
            .map(|(name, text)| (name.as_str(), wiki::anchors(text)))
            .collect();
        let mut broken = Vec::new();
        for (name, text) in &files {
            wiki::rewrite_links(text, |target| {
                let (file, anchor) = target.split_once('#').unwrap_or((target, ""));
                // Doc-comment paths the types keep, and links out of the reference.
                if target.contains("::") || target.contains("://") || file.contains('/') {
                    return None;
                }
                let file = if file.is_empty() { name.as_str() } else { file };
                let found = anchors
                    .get(file)
                    .is_some_and(|a| anchor.is_empty() || a.contains(anchor));
                if !found {
                    broken.push(format!("{name}: {target}"));
                }
                None
            });
        }
        assert!(
            broken.is_empty(),
            "links land nowhere:\n{}",
            broken.join("\n")
        );
    }
}

//! `cargo xtask check --affected`: which packages' tests a change needs.
//!
//! The changed files are those that differ between the merge base of the base
//! ref and `HEAD`, and the working tree, staged or not, plus untracked files.
//! Each maps to the workspace package whose directory holds it, and the tests
//! run for those packages and every package that depends on them, through
//! normal, dev, or build dependencies, transitively.
//!
//! Some changes reach every package, so they test everything: the workspace
//! manifest and lockfile, the toolchain, Cargo's and the tools'
//! configuration, xtask, CI, the shared test fixtures and assets, and any
//! other file outside a package. Prose outside the packages (Markdown, the
//! licence, the issue templates) tests nothing. Markdown inside a package
//! counts as its source, since a crate can compile it in (`bl-agent` serves
//! its guide that way). Whatever this can't place tests everything.
//!
//! In a checkout with the planning documents, `docs/` isn't tracked here: a
//! change to it, or not being able to tell, tests the packages whose code
//! reads it (found by looking for the path in their Rust sources) and their
//! dependents.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::Command;

use serde_json::Value;

/// Files at the workspace root that every package builds or is checked with.
const ROOT_FILES: &[&str] = &[
    "Cargo.toml",
    "Cargo.lock",
    "rust-toolchain",
    "rust-toolchain.toml",
    "deny.toml",
    "clippy.toml",
    ".clippy.toml",
    "rustfmt.toml",
    ".rustfmt.toml",
];

/// Folders at the workspace root whose changes reach every package, and why.
const ROOT_DIRS: &[(&str, &str)] = &[
    (".cargo/", "Cargo's configuration"),
    (".config/", "the tools' configuration"),
    (".github/", "CI"),
    ("xtask/", "xtask, which runs the checks"),
    ("tests/", "the shared test fixtures and golden references"),
    ("assets/", "the shared assets"),
];

/// What the tests step runs.
#[derive(Debug, PartialEq, Eq)]
pub enum Tests {
    /// Every package, and why.
    All(String),
    /// The packages changed, and the packages tested: those and their
    /// dependents. Both are empty if only prose changed.
    Some {
        changed: BTreeSet<String>,
        tested: BTreeSet<String>,
    },
}

/// A workspace package, as the selection sees it.
#[derive(Debug)]
pub struct Package {
    pub name: String,
    /// Its directory relative to the workspace root, with `/` separators and
    /// no trailing one.
    pub dir: String,
}

/// The workspace's packages and the dependencies between them.
pub struct Workspace {
    pub packages: Vec<Package>,
    /// For each package, the workspace packages it depends on, of any kind.
    deps: BTreeMap<String, BTreeSet<String>>,
}

impl Workspace {
    pub fn from_metadata(metadata: &Value) -> Result<Self, String> {
        let malformed = |what: &str| format!("unexpected `cargo metadata` output: no {what}");
        let root = Path::new(
            metadata["workspace_root"]
                .as_str()
                .ok_or_else(|| malformed("workspace_root"))?,
        );
        let members: BTreeSet<&str> = metadata["workspace_members"]
            .as_array()
            .ok_or_else(|| malformed("workspace_members"))?
            .iter()
            .filter_map(Value::as_str)
            .collect();
        let mut names = BTreeMap::new();
        let mut packages = Vec::new();
        for package in metadata["packages"]
            .as_array()
            .ok_or_else(|| malformed("packages"))?
        {
            let (Some(id), Some(name), Some(manifest)) = (
                package["id"].as_str(),
                package["name"].as_str(),
                package["manifest_path"].as_str(),
            ) else {
                continue;
            };
            if !members.contains(id) {
                continue;
            }
            let dir = Path::new(manifest)
                .parent()
                .and_then(|dir| dir.strip_prefix(root).ok())
                .ok_or_else(|| format!("{name} isn't inside the workspace root"))?;
            let dir: Vec<String> = dir
                .components()
                .map(|part| part.as_os_str().to_string_lossy().into_owned())
                .collect();
            names.insert(id.to_owned(), name.to_owned());
            packages.push(Package {
                name: name.to_owned(),
                dir: dir.join("/"),
            });
        }
        let mut deps: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for node in metadata["resolve"]["nodes"]
            .as_array()
            .ok_or_else(|| malformed("resolve.nodes"))?
        {
            let Some(name) = node["id"].as_str().and_then(|id| names.get(id)) else {
                continue;
            };
            let on = node["deps"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|dep| names.get(dep["pkg"].as_str()?))
                .cloned()
                .collect();
            deps.insert(name.clone(), on);
        }
        Ok(Self { packages, deps })
    }

    /// `changed` and every workspace package that depends on one of them,
    /// directly or not.
    pub fn with_dependents(&self, changed: &BTreeSet<String>) -> BTreeSet<String> {
        let mut tested = changed.clone();
        loop {
            let more: Vec<&String> = self
                .deps
                .iter()
                .filter(|(name, on)| !tested.contains(*name) && !on.is_disjoint(&tested))
                .map(|(name, _)| name)
                .collect();
            if more.is_empty() {
                return tested;
            }
            tested.extend(more.into_iter().cloned());
        }
    }

    /// The package whose directory holds `path` (relative, with `/`), if any:
    /// the innermost, should one package sit inside another.
    fn owner(&self, path: &str) -> Option<&Package> {
        self.packages
            .iter()
            .filter(|package| {
                path.strip_prefix(&package.dir)
                    .is_some_and(|rest| rest.starts_with('/'))
            })
            .max_by_key(|package| package.dir.len())
    }

    /// The packages whose Rust sources name the `docs/` folder.
    fn docs_readers(&self, root: &Path) -> BTreeSet<String> {
        self.packages
            .iter()
            .filter(|package| names_docs(&root.join(&package.dir)))
            .map(|package| package.name.clone())
            .collect()
    }
}

/// Where a changed file leads.
#[derive(Debug, PartialEq, Eq)]
enum Reach {
    /// Nothing: prose.
    Nothing,
    /// The planning documents.
    Docs,
    /// One package.
    Package(String),
    /// Everything, and why.
    Everything(String),
}

fn reach(workspace: &Workspace, path: &str) -> Reach {
    if path == "LICENSE" || path.starts_with(".github/ISSUE_TEMPLATE/") {
        return Reach::Nothing;
    }
    if path == "docs" || path.starts_with("docs/") {
        return Reach::Docs;
    }
    let owner = workspace.owner(path);
    if path.ends_with(".md") && owner.is_none() {
        return Reach::Nothing;
    }
    if ROOT_FILES.contains(&path) {
        return Reach::Everything(format!("{path} changed"));
    }
    if let Some((dir, what)) = ROOT_DIRS.iter().find(|(dir, _)| path.starts_with(dir)) {
        return Reach::Everything(format!("{dir} ({what}) changed: {path}"));
    }
    match owner {
        Some(package) => Reach::Package(package.name.clone()),
        None => Reach::Everything(format!("{path} is outside every package")),
    }
}

/// Works out what `cargo xtask check --affected` tests, printing why.
pub fn select(root: &Path, workspace: &Workspace, base: &str) -> Tests {
    let files = match changed_files(root, base) {
        Ok(files) => files,
        Err(why) => return Tests::All(why),
    };
    let mut docs_changed = false;
    let mut changed = BTreeMap::<String, Vec<&str>>::new();
    for path in &files {
        match reach(workspace, path) {
            Reach::Nothing => {}
            Reach::Docs => docs_changed = true,
            Reach::Package(name) => changed.entry(name).or_default().push(path),
            Reach::Everything(why) => return Tests::All(why),
        }
    }
    eprintln!(
        "{} changed file(s) since the merge base with {base}",
        files.len()
    );
    for (name, paths) in &changed {
        eprintln!("  {name}: {}{}", paths[0], more(paths.len()));
    }

    // The planning documents, when this checkout has them through a link.
    if !docs_changed && root.join("docs").exists() && !docs_tracked(root) {
        match docs_changes(&root.join("docs")) {
            Ok(paths) if paths.is_empty() => {}
            Ok(paths) => {
                eprintln!("  docs/: {}{}", paths[0], more(paths.len()));
                docs_changed = true;
            }
            Err(why) => {
                eprintln!("  docs/: {why}, so it counts as changed");
                docs_changed = true;
            }
        }
    }
    let mut changed: BTreeSet<String> = changed.into_keys().collect();
    if docs_changed {
        let readers = workspace.docs_readers(root);
        eprintln!(
            "  docs/ changed, which {} read",
            readers.iter().cloned().collect::<Vec<_>>().join(", ")
        );
        changed.extend(readers);
    }
    let tested = workspace.with_dependents(&changed);
    Tests::Some { changed, tested }
}

fn more(n: usize) -> String {
    match n {
        0 | 1 => String::new(),
        n => format!(" and {} more", n - 1),
    }
}

/// The files that differ from the merge base of `base` and `HEAD`, committed
/// or not, plus untracked ones, relative to the root with `/` separators.
fn changed_files(root: &Path, base: &str) -> Result<BTreeSet<String>, String> {
    let merge_base = git(root, &["merge-base", base, "HEAD"])
        .map_err(|e| format!("couldn't find the merge base of {base} and HEAD ({e})"))?;
    let merge_base = merge_base.trim();
    eprintln!("affected: comparing with {merge_base}, where HEAD left {base}");
    let mut files = paths(&git(
        root,
        &["diff", "--name-only", "--no-renames", "-z", merge_base],
    )?);
    files.extend(paths(&git(
        root,
        &["ls-files", "--others", "--exclude-standard", "-z"],
    )?));
    Ok(files)
}

/// Whether `docs/` is part of this repository rather than a link to another
/// checkout; if it is, its changes are among the changed files already.
fn docs_tracked(root: &Path) -> bool {
    git(root, &["ls-files", "--error-unmatch", "--", "docs"]).is_ok()
}

/// The changes to a linked `docs/` in its own checkout: against the merge base
/// of its `origin/main`, committed or not, plus untracked files.
fn docs_changes(docs: &Path) -> Result<Vec<String>, String> {
    let merge_base = git(docs, &["merge-base", "origin/main", "HEAD"])
        .map_err(|_| "its checkout has no merge base with origin/main".to_owned())?;
    let mut files = paths(&git(
        docs,
        &[
            "diff",
            "--name-only",
            "--no-renames",
            "--relative",
            "-z",
            merge_base.trim(),
        ],
    )?);
    files.extend(paths(&git(
        docs,
        &["ls-files", "--others", "--exclude-standard", "-z"],
    )?));
    Ok(files.into_iter().collect())
}

fn paths(output: &str) -> BTreeSet<String> {
    output
        .split('\0')
        .filter(|path| !path.is_empty())
        .map(str::to_owned)
        .collect()
}

fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .map_err(|e| format!("couldn't run git: {e}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

/// Whether any Rust source under `dir` names the `docs/` folder as a path.
fn names_docs(dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    entries.flatten().any(|entry| {
        let path = entry.path();
        if path.is_dir() {
            entry.file_name() != "target" && names_docs(&path)
        } else {
            path.extension().is_some_and(|ext| ext == "rs")
                && std::fs::read_to_string(&path).is_ok_and(|text| {
                    ["\"docs\"", "\"docs/", "/docs\""]
                        .iter()
                        .any(|pattern| text.contains(pattern))
                })
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace() -> Workspace {
        let package = |name: &str, dir: &str| Package {
            name: name.to_owned(),
            dir: dir.to_owned(),
        };
        let deps = |names: &[&str]| names.iter().map(|&n| n.to_owned()).collect();
        Workspace {
            packages: vec![
                package("bl-core", "crates/core"),
                package("bl-model", "crates/model"),
                package("bl-ui", "crates/ui"),
                package("beatlayer", "crates/app"),
                package("xtask", "xtask"),
            ],
            deps: BTreeMap::from([
                ("bl-core".to_owned(), deps(&[])),
                ("bl-model".to_owned(), deps(&["bl-core"])),
                ("bl-ui".to_owned(), deps(&["bl-model"])),
                ("beatlayer".to_owned(), deps(&["bl-ui", "bl-model"])),
                ("xtask".to_owned(), deps(&["bl-model"])),
            ]),
        }
    }

    fn set(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|&n| n.to_owned()).collect()
    }

    #[test]
    fn files_reach_their_package_or_everything_or_nothing() {
        let ws = workspace();
        let package = |name: &str| Reach::Package(name.to_owned());
        let everything = |path: &str| matches!(reach(&ws, path), Reach::Everything(_));
        assert_eq!(reach(&ws, "crates/ui/src/lib.rs"), package("bl-ui"));
        // A file beside a package's sources reaches that package, such as
        // the keymap's data file (crates/shell-spec/keymaps/live12.toml).
        assert_eq!(reach(&ws, "crates/ui/themes/dark.toml"), package("bl-ui"));
        assert_eq!(reach(&ws, "crates/app/Cargo.toml"), package("beatlayer"));
        // Markdown in a package can be compiled in.
        assert_eq!(reach(&ws, "crates/core/guide.md"), package("bl-core"));
        // A directory that only starts with a package's name isn't it.
        assert!(everything("crates/uikit/src/lib.rs"));
        for prose in [
            "README.md",
            "AGENTS.md",
            "tests/golden/README.md",
            ".github/pull_request_template.md",
            ".github/ISSUE_TEMPLATE/bug.yml",
            "LICENSE",
        ] {
            assert_eq!(reach(&ws, prose), Reach::Nothing, "{prose}");
        }
        for path in [
            "Cargo.toml",
            "Cargo.lock",
            "rust-toolchain.toml",
            "deny.toml",
            "rustfmt.toml",
            "clippy.toml",
            ".cargo/config.toml",
            ".config/nextest.toml",
            ".github/workflows/ci.yml",
            "xtask/src/main.rs",
            "tests/golden/manifest.toml",
            "assets/beatlayer.png",
            "rtsan/src/lib.rs",
            ".gitignore",
        ] {
            assert!(everything(path), "{path}");
        }
        assert_eq!(reach(&ws, "docs/ui-parity/layout.md"), Reach::Docs);
    }

    #[test]
    fn dependents_are_found_transitively() {
        let ws = workspace();
        assert_eq!(
            ws.with_dependents(&set(&["bl-ui"])),
            set(&["bl-ui", "beatlayer"])
        );
        assert_eq!(
            ws.with_dependents(&set(&["bl-core"])),
            set(&["bl-core", "bl-model", "bl-ui", "beatlayer", "xtask"])
        );
        assert_eq!(ws.with_dependents(&set(&["xtask"])), set(&["xtask"]));
        assert_eq!(ws.with_dependents(&BTreeSet::new()), BTreeSet::new());
    }

    #[test]
    fn the_packages_that_read_the_planning_documents_are_found() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("xtask sits in the workspace root");
        let metadata = crate::cargo_metadata(root).unwrap();
        let readers = Workspace::from_metadata(&metadata)
            .unwrap()
            .docs_readers(root);
        for reader in ["xtask", "bl-ui", "bl-shell", "bl-shell-spec"] {
            assert!(readers.contains(reader), "{readers:?}");
        }
        assert!(!readers.contains("bl-core"), "{readers:?}");
    }
}

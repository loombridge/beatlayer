//! The one-test-binary rule, which `cargo xtask check` runs.
//!
//! A crate with several integration test files builds them as one binary,
//! `tests/main.rs`, with `autotests = false` in its manifest: one binary per
//! file linked the whole crate graph into each, about 1,700 binaries and
//! 170 GB a worktree (23 October 2026). With `autotests` off, a test file
//! that `tests/main.rs` doesn't name as a module would never build or run, so
//! this fails on one, and on a crate that has gone back to a binary per file.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

/// Every crate's integration test files are in its one test binary.
pub fn check(root: &Path) -> Result<(), String> {
    let mut problems = Vec::new();
    let crates = fs::read_dir(root.join("crates")).map_err(|e| format!("crates/: {e}"))?;
    let mut dirs: Vec<_> = crates.filter_map(Result::ok).map(|e| e.path()).collect();
    dirs.sort();
    for dir in dirs {
        let tests = dir.join("tests");
        let Ok(manifest) = fs::read_to_string(dir.join("Cargo.toml")) else {
            continue;
        };
        if !tests.is_dir() {
            continue;
        }
        let own_main = own_main_tests(&manifest);
        let mut files = BTreeSet::new();
        for entry in fs::read_dir(&tests).map_err(|e| format!("{}: {e}", tests.display()))? {
            let path = entry.map_err(|e| e.to_string())?.path();
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            if path.extension().is_some_and(|e| e == "rs")
                && stem != "main"
                && !own_main.contains(stem)
            {
                files.insert(stem.to_owned());
            }
        }
        let name = dir.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        let combined = tests.join("main.rs");
        if !combined.exists() {
            if files.len() > 1 {
                problems.push(format!(
                    "crates/{name}: {} test files but no tests/main.rs; list them there as modules \
                     and set autotests = false",
                    files.len()
                ));
            }
            continue;
        }
        if !manifest.lines().any(|l| l.trim() == "autotests = false") {
            problems.push(format!(
                "crates/{name}: tests/main.rs but autotests isn't false"
            ));
        }
        let listed =
            fs::read_to_string(&combined).map_err(|e| format!("{}: {e}", combined.display()))?;
        let modules: BTreeSet<_> = listed
            .lines()
            .filter_map(|l| {
                l.trim()
                    .strip_prefix("mod ")?
                    .strip_suffix(';')
                    .map(str::to_owned)
            })
            .collect();
        for file in files.difference(&modules) {
            problems.push(format!(
                "crates/{name}/tests/{file}.rs isn't in tests/main.rs, so it would never run: add `mod {file};`"
            ));
        }
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems.join("\n"))
    }
}

/// Tests the manifest declares as binaries of their own: any `[[test]]`
/// whose path isn't `tests/main.rs` (one with its own `main`, or one that
/// needs its own global allocator).
fn own_main_tests(manifest: &str) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for block in manifest.split("[[test]]").skip(1) {
        let body = block.split("\n[").next().unwrap_or_default();
        let field = |key: &str| {
            body.lines().find_map(|l| {
                l.trim()
                    .strip_prefix(key)?
                    .trim()
                    .strip_prefix('=')
                    .map(|v| v.trim().trim_matches('"').to_owned())
            })
        };
        if field("path").as_deref() != Some("tests/main.rs")
            && let Some(name) = field("name")
        {
            names.insert(name);
        }
    }
    names
}

#[cfg(test)]
mod tests {
    use super::own_main_tests;

    #[test]
    fn declared_tests_other_than_the_combined_one_keep_their_own_binary() {
        let manifest = "[package]\nname = \"x\"\n\n[[test]]\nname = \"au_editor\"\nharness = false\n\n\
                        [[test]]\nname = \"x_tests\"\npath = \"tests/main.rs\"\n\n[lints]\n";
        assert_eq!(
            own_main_tests(manifest).into_iter().collect::<Vec<_>>(),
            ["au_editor"]
        );
    }
}

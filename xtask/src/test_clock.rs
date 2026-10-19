//! The test-clock guard behind `cargo xtask test-clock`, which
//! `cargo xtask check` runs too (T199).
//!
//! A test that waits for the null device with `thread::sleep` or `Instant::now`
//! races the machine: under load a phrase splits or a deadline passes. Tests
//! drive time themselves instead, through the null device's manual pacing and
//! the test clock (`bl_shell::testing`).
//!
//! The guard fails on either call in a file under `crates/*/tests/`, or in a
//! `#[cfg(test)]` module, unless the file is in
//! [`ALLOWLIST`](self::ALLOWLIST) with a reason. Entries are
//! `path # reason`; a path with no wall-clock wait fails as stale, so the
//! list shrinks as tests move (T200).

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::Command;

/// Where the allowlisted files and their reasons live.
pub const ALLOWLIST: &str = "xtask/test-clock-allowlist.txt";

/// The calls a test must not make to wait for a result.
const WALL_CLOCK: [&str; 2] = ["thread::sleep", "Instant::now"];

/// Checks every test file against the allowlist.
pub fn check(root: &Path) -> Result<(), String> {
    let allowed = read_allowlist(root)?;
    let mut found = Vec::new();
    let mut used = BTreeSet::new();
    for path in test_files(root)? {
        let text = match std::fs::read_to_string(root.join(&path)) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(format!("couldn't read {path}: {error}")),
        };
        for (line, call) in wall_clock_lines(&path, &text) {
            if allowed.contains_key(&path) {
                used.insert(path.clone());
            } else {
                found.push(format!("{path}:{line}: `{call}`"));
            }
        }
    }

    let stale: Vec<&String> = allowed
        .keys()
        .filter(|path| !used.contains(*path))
        .collect();
    if found.is_empty() && stale.is_empty() {
        eprintln!(
            "{} test file(s) call no wall clock; {} allowlisted with a reason",
            test_files(root)?.len() - used.len(),
            allowed.len()
        );
        return Ok(());
    }

    let mut message = String::new();
    if !found.is_empty() {
        message.push_str("a test waits on the wall clock instead of driving time (T199):\n  ");
        message.push_str(&found.join("\n  "));
        message.push_str(
            "\n\nUse the null device's manual pacing and the test clock \
             (`bl_shell::testing::advance`, `wait_for`, `run_blocks`), or add the \
             file to ",
        );
        message.push_str(ALLOWLIST);
        message.push_str(" with the reason it needs the wall clock.");
    }
    if !stale.is_empty() {
        if !message.is_empty() {
            message.push_str("\n\n");
        }
        message.push_str("the allowlist names files with no wall-clock wait; remove them:\n  ");
        message.push_str(
            &stale
                .iter()
                .map(|path| path.as_str())
                .collect::<Vec<_>>()
                .join("\n  "),
        );
    }
    Err(message)
}

/// The lines of a test file that call the wall clock, as `(line, call)`.
///
/// A file under `crates/*/tests/` is all test code; another file is scanned
/// from its `#[cfg(test)]` module.
fn wall_clock_lines(path: &str, text: &str) -> Vec<(usize, &'static str)> {
    let whole_file = is_test_file(path);
    let mut in_tests = whole_file;
    let mut found = Vec::new();
    for (index, line) in text.lines().enumerate() {
        if !whole_file && line.trim_start().starts_with("#[cfg(test)]") {
            in_tests = true;
        }
        if !in_tests || line.trim_start().starts_with("//") {
            continue;
        }
        for call in WALL_CLOCK {
            if line.contains(call) {
                found.push((index + 1, call));
            }
        }
    }
    found
}

fn is_test_file(path: &str) -> bool {
    path.split('/').nth(2) == Some("tests")
}

/// Every `.rs` file under `crates/*/src` or `crates/*/tests`, tracked or
/// untracked but not ignored: a planted file is caught before it's committed.
fn test_files(root: &Path) -> Result<Vec<String>, String> {
    let output = Command::new("git")
        .args([
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
            "--",
            "crates",
        ])
        .current_dir(root)
        .output()
        .map_err(|e| format!("couldn't list the repository's files: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "couldn't list the repository's files: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(output
        .stdout
        .split(|byte| *byte == 0)
        .filter_map(|path| std::str::from_utf8(path).ok())
        .filter(|path| {
            path.ends_with(".rs")
                && matches!(path.split('/').next(), Some("crates"))
                && matches!(path.split('/').nth(2), Some("src" | "tests"))
        })
        .map(str::to_owned)
        .collect())
}

fn read_allowlist(root: &Path) -> Result<BTreeMap<String, String>, String> {
    let text = std::fs::read_to_string(root.join(ALLOWLIST))
        .map_err(|e| format!("couldn't read {ALLOWLIST}: {e}"))?;
    let mut allowed = BTreeMap::new();
    for (index, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((path, reason)) = line.split_once('#') else {
            return Err(format!(
                "{ALLOWLIST}:{}: needs a `path # reason` entry",
                index + 1
            ));
        };
        let reason = reason.trim();
        if reason.is_empty() {
            return Err(format!(
                "{ALLOWLIST}:{}: the entry needs a reason after `#`",
                index + 1
            ));
        }
        allowed.insert(path.trim().to_owned(), reason.to_owned());
    }
    Ok(allowed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_planted_wait_in_a_test_file_is_found() {
        let text = "fn test() {\n    thread::sleep(Duration::from_millis(5));\n}\n";
        assert_eq!(
            wall_clock_lines("crates/shell/tests/planted.rs", text),
            [(2, "thread::sleep")]
        );
    }

    #[test]
    fn a_cfg_test_module_is_scanned_and_production_is_not() {
        let text = "fn play() { let now = Instant::now(); }\n\
                    #[cfg(test)]\nmod tests {\n    fn t() { let _ = Instant::now(); }\n}\n";
        assert_eq!(
            wall_clock_lines("crates/audio-io/src/planted.rs", text),
            [(4, "Instant::now")]
        );
    }

    #[test]
    fn comments_about_the_calls_are_not_calls() {
        let text = "// thread::sleep is banned\nfn t() {}\n";
        assert!(wall_clock_lines("crates/shell/tests/documented.rs", text).is_empty());
    }
}

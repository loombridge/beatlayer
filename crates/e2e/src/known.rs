//! The known-failures ratchet: `tests/known-failures.toml` lists failures
//! the suite has found in Beatlayer that aren't fixed yet, so a sweep can
//! go on past them while every new failure still fails the run.
//!
//! Each entry names its `suite` (the test family, such as `commands`), a
//! `key` (what was checked: a command, a device and parameter, a scenario's
//! step), the `property` that doesn't hold, and a plain-words
//! `description` of the bug:
//!
//! ```toml
//! [[failure]]
//! suite = "devices"
//! key = "beatlayer.grit/Drive"
//! property = "level bounded"
//! description = "At its maximum the output rises 30 dB above the default setting."
//! ```
//!
//! A [`Ratchet`] for one suite records each check's outcome and, at
//! [`Ratchet::finish`], fails on any failure not listed and on any listed
//! entry whose check now passes, so a fix deletes its entry in the same
//! change. Every entry is a real, reproducible bug in Beatlayer, never a
//! gap in the harness: a harness bug is fixed in the harness.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::Deserialize;

/// One listed failure.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    /// The test family that finds it, such as `commands` or `devices`.
    pub suite: String,
    /// What was checked: a command, a device and parameter, a step.
    pub key: String,
    /// The property that doesn't hold.
    pub property: String,
    /// The bug, in plain words.
    pub description: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    #[serde(default)]
    failure: Vec<Entry>,
}

/// The known failures, as `tests/known-failures.toml` lists them.
#[derive(Clone, Debug, Default)]
pub struct Known {
    entries: Vec<Entry>,
}

impl Known {
    /// Where the list lives.
    #[must_use]
    pub fn path() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/known-failures.toml")
    }

    /// Reads the list.
    ///
    /// # Panics
    ///
    /// If the file can't be read or parsed, or an entry is incomplete or
    /// listed twice.
    #[must_use]
    pub fn load() -> Self {
        let path = Self::path();
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("couldn't read {}: {error}", path.display()));
        Self::parse(&text).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
    }

    /// Parses a list in the file's format.
    ///
    /// # Errors
    ///
    /// Why the text isn't a valid list: bad TOML, a missing or empty field,
    /// or the same suite, key and property listed twice.
    pub fn parse(text: &str) -> Result<Self, String> {
        let file: File = toml::from_str(text).map_err(|error| error.to_string())?;
        let mut seen = BTreeSet::new();
        for entry in &file.failure {
            for (field, value) in [
                ("suite", &entry.suite),
                ("key", &entry.key),
                ("property", &entry.property),
                ("description", &entry.description),
            ] {
                if value.trim().is_empty() {
                    return Err(format!("an entry has an empty `{field}`: {entry:?}"));
                }
            }
            if !seen.insert((&entry.suite, &entry.key, &entry.property)) {
                return Err(format!(
                    "listed twice: suite `{}`, key `{}`, property `{}`",
                    entry.suite, entry.key, entry.property
                ));
            }
        }
        Ok(Self {
            entries: file.failure,
        })
    }

    /// Every entry.
    #[must_use]
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// The entry for this check, if it's listed.
    #[must_use]
    pub fn find(&self, suite: &str, key: &str, property: &str) -> Option<&Entry> {
        self.entries
            .iter()
            .find(|e| e.suite == suite && e.key == key && e.property == property)
    }

    /// A ratchet for `suite`'s checks.
    pub fn ratchet(&self, suite: &str) -> Ratchet {
        Ratchet {
            suite: suite.to_owned(),
            listed: self
                .entries
                .iter()
                .filter(|e| e.suite == suite)
                .cloned()
                .collect(),
            checked: 0,
            failed: Vec::new(),
            known: Vec::new(),
            fixed: Vec::new(),
        }
    }
}

/// One suite's checks against the list. Record every check with
/// [`check`](Self::check), then call [`finish`](Self::finish).
#[derive(Debug)]
#[must_use = "a ratchet reports only when it's finished"]
pub struct Ratchet {
    suite: String,
    listed: Vec<Entry>,
    checked: usize,
    /// Failures not listed: `(key, property, detail)`.
    failed: Vec<(String, String, String)>,
    /// Listed failures that failed again.
    known: Vec<(String, String)>,
    /// Listed failures whose check passed.
    fixed: Vec<(String, String)>,
}

impl Ratchet {
    /// Records one check of `property` for `key`: `Ok` if it held, or what
    /// went wrong.
    pub fn check(&mut self, key: &str, property: &str, outcome: Result<(), String>) {
        self.checked += 1;
        let listed = self
            .listed
            .iter()
            .any(|e| e.key == key && e.property == property);
        match (outcome, listed) {
            (Ok(()), false) => {}
            (Ok(()), true) => self.fixed.push((key.to_owned(), property.to_owned())),
            (Err(_), true) => self.known.push((key.to_owned(), property.to_owned())),
            (Err(detail), false) => {
                self.failed
                    .push((key.to_owned(), property.to_owned(), detail));
            }
        }
    }

    /// Records a failure of `property` for `key`.
    pub fn violation(&mut self, key: &str, property: &str, detail: impl Into<String>) {
        self.check(key, property, Err(detail.into()));
    }

    /// Whether `key`'s `property` is listed as failing.
    #[must_use]
    pub fn is_listed(&self, key: &str, property: &str) -> bool {
        self.listed
            .iter()
            .any(|e| e.key == key && e.property == property)
    }

    /// How many checks were recorded.
    #[must_use]
    pub fn checked(&self) -> usize {
        self.checked
    }

    /// Prints a summary, then panics if any failure isn't listed or any
    /// listed failure's check passed. Listed entries this run didn't check
    /// (a quick run checks less than a full one) are printed, not failed.
    ///
    /// # Panics
    ///
    /// As it says, with every such failure and fixed entry.
    pub fn finish(self) {
        let unchecked: Vec<&Entry> = self
            .listed
            .iter()
            .filter(|e| {
                let seen = |list: &[(String, String)]| {
                    list.iter().any(|(k, p)| *k == e.key && *p == e.property)
                };
                !seen(&self.known) && !seen(&self.fixed)
            })
            .collect();
        eprintln!(
            "{}: {} checks; {} new failures, {} known failures, {} listed failures now pass, \
             {} listed not checked this run",
            self.suite,
            self.checked,
            self.failed.len(),
            self.known.len(),
            self.fixed.len(),
            unchecked.len()
        );
        for entry in &unchecked {
            eprintln!(
                "  not checked: `{}` / `{}`: {}",
                entry.key, entry.property, entry.description
            );
        }
        let mut report = String::new();
        if !self.failed.is_empty() {
            report.push_str(&format!(
                "{} failure(s) not in {}:\n",
                self.failed.len(),
                Known::path().display()
            ));
            for (key, property, detail) in &self.failed {
                report.push_str(&format!("  `{key}` / `{property}`: {detail}\n"));
            }
        }
        if !self.fixed.is_empty() {
            report.push_str(&format!(
                "{} listed failure(s) now pass; delete their entries:\n",
                self.fixed.len()
            ));
            for (key, property) in &self.fixed {
                report.push_str(&format!("  `{key}` / `{property}`\n"));
            }
        }
        assert!(report.is_empty(), "suite `{}`:\n{report}", self.suite);
    }
}

//! The keymap against the UI parity reference: every shortcut is bound or marked with the milestone that binds it.

use bl_commands::Registry;
use bl_ui::keymap::{Keymap, Platform, Status, parse_binding};

use crate::keymap::helpers::*;

/// A shortcut from the shortcut reference.
#[derive(Debug, serde::Deserialize)]
struct Reference {
    id: String,
    context: String,
    kind: String,
    trigger: String,
    milestone: String,
    mac: Vec<String>,
    win: Vec<String>,
}

#[derive(serde::Deserialize)]
struct ReferenceFile {
    shortcut: Vec<Reference>,
}

/// The reference's shortcuts, or `None` in a checkout without the planning
/// documents (they aren't published yet), where the test skips.
fn reference() -> Option<Vec<Reference>> {
    let docs = repo().join("docs");
    if !docs.exists() {
        eprintln!(
            "skipped: the UI parity reference isn't published yet, and this checkout doesn't have it"
        );
        return None;
    }
    let text = std::fs::read_to_string(docs.join("ui-parity/shortcuts.toml")).unwrap();
    let file: ReferenceFile = toml::from_str(&text).unwrap();
    Some(file.shortcut)
}

/// Each work package's status in the build plan's status table, by its
/// `WP-` name: the rows that read `| 6.70 | todo | ... |`.
fn package_statuses() -> std::collections::HashMap<String, String> {
    let text = std::fs::read_to_string(repo().join("docs/03-build-plan.md")).unwrap();
    text.lines()
        .filter_map(|line| {
            let mut cells = line.split('|').skip(1).map(str::trim);
            let (id, status) = (cells.next()?, cells.next()?);
            let status = matches!(
                status,
                "done" | "todo" | "in progress" | "review" | "blocked"
            )
            .then(|| status.to_owned())?;
            Some((format!("WP-{id}"), status))
        })
        .collect()
}

/// The shortcuts of the views and windows the shell (WP-1.7) builds, which
/// must be bound.
const SHELL: &[&str] = &[
    "S-FILE-001",
    "S-FILE-002",
    "S-FILE-003",
    "S-FILE-004",
    "S-FILE-005",
    "S-FILE-006",
    "S-FILE-009",
    "S-VIEW-001",
    "S-VIEW-002",
    "S-VIEW-003",
    "S-VIEW-010",
    "S-BRW-002",
    "S-VIEW-019",
    "S-VIEW-020",
    "S-VIEW-021",
    "S-VIEW-004",
    "S-VIEW-005",
    "S-VIEW-015",
    "S-VIEW-016",
    "S-VIEW-017",
    "S-VIEW-022",
    "S-NOTE-030",
    "S-FOCUS-004",
    "S-FOCUS-005",
    "S-FOCUS-007",
    "S-FOCUS-001",
    "S-FOCUS-003",
    "S-FOCUS-011",
    "S-FOCUS-012",
    "S-ZOOM-001",
    "S-ZOOM-002",
    "S-ENGINE-001",
    "S-SET-001",
    "S-SET-002",
    "S-SET-005",
    "S-SET-006",
    "S-SET-007",
    "S-EDIT-006",
    "S-EDIT-007",
    "S-TRACK-001",
];

/// Everything wrong with a keymap against the reference and the registry.
fn problems(keymap: &Keymap, reference: &[Reference], registry: &Registry) -> Vec<String> {
    let package_statuses = package_statuses();
    let mut problems = Vec::new();
    let entries = keymap.entries();
    if entries.len() != reference.len() {
        problems.push(format!(
            "the keymap has {} shortcuts and the reference {}",
            entries.len(),
            reference.len()
        ));
    }
    for (entry, source) in entries.iter().zip(reference) {
        let id = &source.id;
        if entry.id != *id
            || entry.context != source.context
            || entry.kind != source.kind
            || entry.milestone != source.milestone
            || entry.mac != source.mac
            || entry.win != source.win
            || format!("{:?}", entry.trigger).to_lowercase() != source.trigger
        {
            problems.push(format!("{id}: differs from the reference"));
            continue;
        }
        match &entry.status {
            Status::Bound => {
                for command in &entry.commands {
                    let Some(spec) = registry.get(&command.id) else {
                        problems.push(format!("{id}: runs {}, which isn't registered", command.id));
                        continue;
                    };
                    if !spec.bindings.iter().any(|b| b.shortcut == *id) {
                        problems.push(format!("{id}: {} doesn't bind it", command.id));
                    }
                    if command.contexts.is_empty() {
                        problems.push(format!("{id}: {} runs in no context", command.id));
                    }
                }
                if entry.kind == "key" {
                    for (platform, bindings) in
                        [(Platform::Mac, &entry.mac), (Platform::Other, &entry.win)]
                    {
                        for binding in bindings {
                            if parse_binding(binding, platform).is_none() {
                                problems.push(format!("{id}: `{binding}` can't be matched"));
                            }
                        }
                    }
                }
            }
            Status::System(how) | Status::Widget(how) => {
                if how.is_empty() {
                    problems.push(format!("{id}: says nothing about who handles it"));
                }
            }
            Status::Pending { milestone, package } => {
                if *milestone != source.milestone {
                    problems.push(format!(
                        "{id}: pending for {milestone}, not {}",
                        source.milestone
                    ));
                }
                // Spec: ui-parity README, "How UI packages use it". A pending
                // shortcut of M1 to M3 names a package that isn't done.
                if matches!(source.milestone.as_str(), "M1" | "M2" | "M3") {
                    match package.as_deref() {
                        None => problems.push(format!(
                            "{id}: an {} shortcut with no package",
                            source.milestone
                        )),
                        Some("WP-1.7") => {
                            problems.push(format!("{id}: the shell's own, but unbound"));
                        }
                        Some(package) => match package_statuses.get(package) {
                            None => problems.push(format!("{id}: {package} isn't in the plan")),
                            Some(status) if status == "done" => problems
                                .push(format!("{id}: {package} is done, but left it unbound")),
                            Some(_) => {}
                        },
                    }
                }
            }
        }
    }
    for id in SHELL {
        if keymap
            .entry_for(id)
            .is_none_or(|e| e.status != Status::Bound)
        {
            problems.push(format!(
                "{id}: the shell builds its view, but it isn't bound"
            ));
        }
    }
    problems
}

#[test]
fn every_shortcut_is_bound_or_marked_with_the_milestone_that_binds_it() {
    let keymap = Keymap::live12();
    let Some(reference) = reference() else {
        return;
    };
    let problems = problems(&keymap, &reference, &registry());
    assert!(problems.is_empty(), "{problems:#?}");
    let bound = keymap
        .entries()
        .iter()
        .filter(|e| e.status == Status::Bound)
        .count();
    assert!(bound >= SHELL.len(), "{bound} bound");
}

#[test]
fn the_test_fails_when_a_shortcut_is_neither_bound_nor_marked() {
    // Unbind Save and take away its package: the test must notice.
    let text = bl_ui::keymap::LIVE12.replace(
        "status = \"bound\"\ncommands = [\n  { id = \"set.save\", contexts = [\"global\"] },\n]",
        "status = \"pending\"",
    );
    assert_ne!(text, bl_ui::keymap::LIVE12, "the replacement applied");
    let keymap = Keymap::parse(&text).unwrap();
    let Some(reference) = reference() else {
        return;
    };
    let problems = problems(&keymap, &reference, &registry());
    assert!(
        problems
            .iter()
            .any(|p| p.contains("S-FILE-003: an M1 shortcut with no package")),
        "{problems:#?}"
    );
    assert!(
        problems
            .iter()
            .any(|p| p.contains("S-FILE-003: the shell builds"))
    );
}

#[test]
fn the_test_fails_when_a_pending_shortcut_of_m1_to_m3_has_no_package_or_a_done_one() {
    // S-FOCUS-009 is an M2 shortcut that WP-6.72 binds.
    let pending = "package = \"WP-6.72\"\n";
    assert!(
        bl_ui::keymap::LIVE12.contains(pending),
        "S-FOCUS-009's package"
    );
    let Some(reference) = reference() else {
        return;
    };
    let problems_of = |text: String| {
        let keymap = Keymap::parse(&text).unwrap();
        problems(&keymap, &reference, &registry())
    };
    let none = problems_of(bl_ui::keymap::LIVE12.replace(pending, ""));
    assert!(
        none.iter()
            .any(|p| p.contains("S-FOCUS-009: an M2 shortcut with no package")),
        "{none:#?}"
    );
    let done = problems_of(bl_ui::keymap::LIVE12.replace(pending, "package = \"WP-1.8b\"\n"));
    assert!(
        done.iter()
            .any(|p| p.contains("S-FOCUS-009: WP-1.8b is done, but left it unbound")),
        "{done:#?}"
    );
    let unknown = problems_of(bl_ui::keymap::LIVE12.replace(pending, "package = \"WP-99.9\"\n"));
    assert!(
        unknown
            .iter()
            .any(|p| p.contains("S-FOCUS-009: WP-99.9 isn't in the plan")),
        "{unknown:#?}"
    );
}

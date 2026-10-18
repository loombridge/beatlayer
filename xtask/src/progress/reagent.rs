//! Reagent's progress, read from its plan, `docs/reagent/01-plan.md`:
//!
//! - its phases from the "## Phases" table (`| Phase | What you can do at its
//!   end | Packages |`), with each phase's package estimate;
//! - its planned packages from the `### R0: …` and `### R1: …` tables
//!   (`| WP | Package | Size | Depends on |`), and the later phases' epics;
//! - each package's status from the "## Status" table (`| WP | Status | Date |
//!   PR | Notes |`), where a PR cell may link pull requests in several
//!   repositories.
//!
//! Anything the status table names that the plan doesn't have, a status
//! outside `todo`, `in progress`, `review` and `done`, and a planned package
//! with no status row are recorded as problems, which the tests reject.

use std::collections::BTreeMap;

use serde_json::{Value, json};

use super::{Status, cells, plain};

/// Where Reagent's plan is, relative to the repository root.
pub const PATH: &str = "docs/reagent/01-plan.md";

/// A pull request link from a status row, such as `plugin-kit#2`.
#[derive(Debug, PartialEq, Eq)]
struct Link {
    label: String,
    url: String,
}

#[derive(Debug)]
struct Package {
    id: String,
    title: String,
    size: Option<String>,
    depends: Option<String>,
    status: Status,
    /// Whether the status table has a row for it.
    tracked: bool,
    date: Option<String>,
    prs: Vec<Link>,
    note: Option<String>,
}

#[derive(Debug)]
struct Phase {
    /// `R0`, `R1`, and so on.
    id: String,
    title: String,
    goal: Option<String>,
    estimate: Option<u32>,
    epics: Option<String>,
    exit: Option<String>,
    packages: Vec<Package>,
}

/// Reagent's plan, parsed.
pub struct Reagent {
    phases: Vec<Phase>,
    problems: Vec<String>,
}

/// `R0` or `R12`.
fn phase_id(text: &str) -> Option<&str> {
    let digits = text.strip_prefix('R')?;
    (!digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit())).then_some(text)
}

/// `R0.1` or `R1.17`, with the phase it belongs to.
fn package_id(text: &str) -> Option<(&str, &str)> {
    let (phase, number) = text.split_once('.')?;
    phase_id(phase)?;
    (!number.is_empty() && number.chars().all(|c| c.is_ascii_digit())).then_some((text, phase))
}

/// The order of package IDs: `R0.2` before `R0.10`.
fn package_order(id: &str) -> (u32, u32) {
    let (phase, number) = id.split_once('.').unwrap_or((id, "0"));
    (
        phase.trim_start_matches('R').parse().unwrap_or(u32::MAX),
        number.parse().unwrap_or(u32::MAX),
    )
}

/// Every Markdown link in a cell: `[plugin-kit#2](url), [beatlayer#31](url)`.
fn links(cell: &str) -> Vec<Link> {
    let mut found = Vec::new();
    let mut rest = cell;
    while let Some(open) = rest.find('[') {
        let after = &rest[open + 1..];
        let Some(close) = after.find("](") else { break };
        let Some(end) = after[close + 2..].find(')') else {
            break;
        };
        found.push(Link {
            label: after[..close].trim().to_string(),
            url: after[close + 2..close + 2 + end].trim().to_string(),
        });
        rest = &after[close + 2 + end + 1..];
    }
    found
}

/// The status table's wording, which must be exact.
fn status(text: &str) -> Option<Status> {
    match text {
        "todo" => Some(Status::Todo),
        "in progress" => Some(Status::InProgress),
        "review" => Some(Status::Review),
        "done" => Some(Status::Done),
        _ => None,
    }
}

fn non_empty(text: &str) -> Option<String> {
    let text = plain(text);
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

impl Reagent {
    pub fn parse(plan: &str) -> Self {
        let mut phases: Vec<Phase> = Vec::new();
        let mut problems = Vec::new();
        let mut statuses: Vec<Vec<String>> = Vec::new();
        let mut section = String::new();
        let mut subsection = String::new();
        for line in plan.lines() {
            if let Some(heading) = line.strip_prefix("## ") {
                section = heading.trim().to_string();
                subsection.clear();
                continue;
            }
            if let Some(heading) = line.strip_prefix("### ") {
                subsection = heading.trim().to_string();
                continue;
            }
            // **R0 exit demo:** generate a new instrument ...
            if let Some(rest) = line.strip_prefix("**R")
                && let Some((head, demo)) = rest.split_once(" exit demo:**")
                && let Some(phase) = phases.iter_mut().find(|p| p.id[1..] == *head)
            {
                phase.exit = non_empty(demo);
                continue;
            }
            let Some(row) = cells(line) else { continue };
            if section == "Phases" && subsection.is_empty() {
                // | R1 Sound engine core | What you can do | 17 |
                let Some((id, title)) = row[0].split_once(' ') else {
                    continue;
                };
                if phase_id(id).is_none() || row.len() < 3 {
                    continue;
                }
                phases.push(Phase {
                    id: id.to_string(),
                    title: title.trim().to_string(),
                    goal: non_empty(&row[1]),
                    estimate: row[2].trim().parse().ok(),
                    epics: None,
                    exit: None,
                    packages: Vec::new(),
                });
            } else if section == "Phases" && subsection.contains("epics") {
                // | R2 Interface | Page structure; ... |
                let Some((id, _)) = row[0].split_once(' ') else {
                    continue;
                };
                if let Some(phase) = phases.iter_mut().find(|p| p.id == id)
                    && row.len() >= 2
                {
                    phase.epics = non_empty(&row[1]);
                }
            } else if section == "Phases" {
                // | R0.1 | The two public repositories ... | M | RD1 to RD4 |
                let Some((id, phase_id)) = package_id(&row[0]) else {
                    continue;
                };
                if row.len() < 3 {
                    continue;
                }
                let Some(phase) = phases.iter_mut().find(|p| p.id == phase_id) else {
                    problems.push(format!(
                        "{id} is planned in a phase the phases table doesn't list"
                    ));
                    continue;
                };
                phase.packages.push(Package {
                    id: id.to_string(),
                    title: plain(&row[1]),
                    size: non_empty(&row[2]),
                    depends: row.get(3).and_then(|d| non_empty(d)),
                    status: Status::Todo,
                    tracked: false,
                    date: None,
                    prs: Vec::new(),
                    note: None,
                });
            } else if section == "Status" && row.first().is_some_and(|c| c != "WP") {
                statuses.push(row);
            }
        }

        for row in &statuses {
            let id = row[0].as_str();
            let Some(package) = phases
                .iter_mut()
                .flat_map(|p| &mut p.packages)
                .find(|p| p.id == id)
            else {
                problems.push(format!(
                    "the status table names {id}, which the plan doesn't have"
                ));
                continue;
            };
            let cell = row.get(1).map(String::as_str).unwrap_or("");
            match status(cell) {
                Some(status) => package.status = status,
                None => problems.push(format!(
                    "{id}'s status is \"{cell}\", not todo, in progress, review or done"
                )),
            }
            if package.tracked {
                problems.push(format!("the status table lists {id} twice"));
            }
            package.tracked = true;
            package.date = row.get(2).and_then(|d| non_empty(d));
            package.prs = row.get(3).map(|c| links(c)).unwrap_or_default();
            package.note = row.get(4).and_then(|n| non_empty(n));
        }
        for phase in &mut phases {
            phase.packages.sort_by_key(|p| package_order(&p.id));
            for package in &phase.packages {
                if !package.tracked {
                    problems.push(format!("{} has no row in the status table", package.id));
                }
            }
        }
        Self { phases, problems }
    }

    /// The plan as JSON, shaped like Beatlayer's phases so the page draws
    /// both the same way.
    pub fn to_json(&self) -> Value {
        let mut by_status: BTreeMap<&str, usize> = BTreeMap::new();
        let mut total = 0;
        let phases: Vec<Value> = self
            .phases
            .iter()
            .map(|phase| {
                if phase.packages.is_empty() {
                    let estimate = phase.estimate.unwrap_or(0) as usize;
                    *by_status.entry(Status::Planned.as_str()).or_default() += estimate;
                    total += estimate;
                } else {
                    for package in &phase.packages {
                        *by_status.entry(package.status.as_str()).or_default() += 1;
                    }
                    total += phase.packages.len();
                }
                json!({
                    "number": phase.id,
                    "title": phase.title,
                    "goal": phase.goal,
                    "estimate": phase.estimate,
                    "epics": phase.epics,
                    "exit": phase.exit,
                    "packages": phase.packages.iter().map(|p| json!({
                        "id": p.id,
                        "title": p.title,
                        "size": p.size,
                        "depends": p.depends,
                        "status": p.status.as_str(),
                        "date": p.date,
                        "prs": p.prs.iter().map(|l| json!({"label": l.label, "url": l.url})).collect::<Vec<_>>(),
                        "note": p.note,
                    })).collect::<Vec<_>>(),
                })
            })
            .collect();
        json!({
            "phases": phases,
            "totals": { "packages": { "total": total, "by_status": by_status } },
            "problems": self.problems,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reagent's real plan, or `None` in a checkout without it, where the
    /// test skips: the plan stays with the maintainers, so only their
    /// checkouts have it.
    fn real_plan() -> Option<String> {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        if !root.join(PATH).exists() {
            eprintln!("skipped: this checkout doesn't have Reagent's plan");
            return None;
        }
        Some(std::fs::read_to_string(root.join(PATH)).expect("Reagent's plan is readable"))
    }

    #[test]
    fn reagents_plan_parses_into_every_phase() {
        let Some(plan) = real_plan() else { return };
        let reagent = Reagent::parse(&plan);
        let ids: Vec<&str> = reagent.phases.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(
            ids,
            ["R0", "R1", "R2", "R3", "R4", "R5", "R6", "R7", "R8", "R9"]
        );
        for phase in &reagent.phases {
            assert!(!phase.title.is_empty(), "{} has a name", phase.id);
            assert!(
                !phase.packages.is_empty() || phase.estimate.is_some_and(|n| n > 0),
                "{} has packages or an estimate",
                phase.id
            );
        }
        for phase in &reagent.phases[..2] {
            assert!(!phase.packages.is_empty(), "{} has packages", phase.id);
        }
        assert!(reagent.phases[0].packages.iter().any(|p| p.id == "R0.1"));
    }

    #[test]
    fn reagents_status_table_matches_its_plan() {
        let Some(plan) = real_plan() else { return };
        let reagent = Reagent::parse(&plan);
        assert!(
            reagent.problems.is_empty(),
            "Reagent's status table doesn't match its plan:\n{}",
            reagent.problems.join("\n")
        );
    }

    #[test]
    fn statuses_links_and_unknown_rows() {
        let plan = "\
## Phases

| Phase | What you can do at its end | Packages |
|---|---|---|
| R0 Foundations | A test instrument plays | 3 |
| R1 Sound engine core | Play it headless | 2 |
| R2 Interface | Its own interface | 12 |

### R0: Foundations

| WP | Package | Size | Depends on |
|---|---|---|---|
| R0.1 | The repositories | M | RD1 |
| R0.2 | Move `bl-rt` | M | R0.1 |
| R0.10 | The UI | L | R0.1 |

**R0 exit demo:** an agent plays it.

### R1: Sound engine core

| WP | Package | Size | Depends on |
|---|---|---|---|
| R1.1 | Inventory pass | M | R0.1 |

### R2 to R9: epics

| Phase | Epics |
|---|---|
| R2 Interface | Pages and editors |

## Status

| WP | Status | Date | PR | Notes |
|---|---|---|---|---|
| R0.1 | done | 2026-10-12 | [plugin-kit#1](https://example.org/k/1), [reagent#1](https://example.org/r/1) | merged |
| R0.10 | in progress | | | |
| R0.2 | blocked | | | |
| R1.9 | todo | | | |
";
        let reagent = Reagent::parse(plan);
        let r0 = &reagent.phases[0];
        let ids: Vec<&str> = r0.packages.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, ["R0.1", "R0.2", "R0.10"]);
        assert_eq!(r0.packages[0].status, Status::Done);
        assert_eq!(
            r0.packages[0].prs,
            [
                Link {
                    label: "plugin-kit#1".into(),
                    url: "https://example.org/k/1".into()
                },
                Link {
                    label: "reagent#1".into(),
                    url: "https://example.org/r/1".into()
                },
            ]
        );
        assert_eq!(r0.packages[0].note.as_deref(), Some("merged"));
        assert_eq!(r0.packages[2].status, Status::InProgress);
        assert_eq!(r0.exit.as_deref(), Some("an agent plays it."));
        assert_eq!(
            reagent.phases[2].epics.as_deref(),
            Some("Pages and editors")
        );
        assert_eq!(
            reagent.problems,
            [
                "R0.2's status is \"blocked\", not todo, in progress, review or done",
                "the status table names R1.9, which the plan doesn't have",
                "R1.1 has no row in the status table",
            ]
        );

        let json = reagent.to_json();
        assert_eq!(json["totals"]["packages"]["total"], 3 + 1 + 12);
        assert_eq!(json["totals"]["packages"]["by_status"]["planned"], 12);
        assert_eq!(json["totals"]["packages"]["by_status"]["done"], 1);
    }
}

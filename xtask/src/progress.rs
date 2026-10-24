//! The progress model behind `cargo xtask progress` and `cargo xtask
//! dashboard`, read from the planning documents:
//!
//! - the milestones from the roadmap;
//! - the phases and their work packages from the build plan, with each
//!   package's status from its status table;
//! - every Live feature from the parity matrix, split into one part per
//!   milestone it names, with each part's status taken from the work packages
//!   its row names ("Built in WP-1.8a, WP-1.8b");
//! - the improvements over Live;
//! - Reagent's plan, when the checkout has it ([`reagent`]).
//!
//! The tests at the bottom read the real documents, so `cargo xtask check`
//! fails when a row names a work package the build plan doesn't have.

mod reagent;

use std::collections::BTreeMap;

use serde_json::{Value, json};

use reagent::Reagent;

/// The planning documents, as text.
pub struct Docs {
    pub roadmap: String,
    pub plan: String,
    pub parity: String,
    pub improvements: String,
    /// Reagent's plan, absent where the checkout doesn't have it.
    pub reagent: Option<String>,
}

/// The paths of the planning documents, relative to the repository root.
pub const PATHS: [&str; 4] = [
    "docs/01-roadmap.md",
    "docs/03-build-plan.md",
    "docs/04-parity-matrix.md",
    "docs/05-improvements.md",
];

impl Docs {
    /// Reads the documents with `read`, which takes a path from [`PATHS`] or
    /// Reagent's plan and gives `None` for a file that doesn't exist. Only
    /// Reagent's plan may be missing.
    pub fn read(
        mut read: impl FnMut(&str) -> Result<Option<String>, String>,
    ) -> Result<Self, String> {
        let mut required = |path: &str| read(path)?.ok_or_else(|| format!("{path} doesn't exist"));
        Ok(Self {
            roadmap: required(PATHS[0])?,
            plan: required(PATHS[1])?,
            parity: required(PATHS[2])?,
            improvements: required(PATHS[3])?,
            reagent: read(reagent::PATH)?,
        })
    }
}

/// Where a package, a feature, or an improvement stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Status {
    Done,
    Review,
    InProgress,
    /// A package not started, or a feature whose packages haven't started.
    Todo,
    /// A feature due in a phase that's broken into packages, but that no
    /// package builds: a gap in the plan.
    Unassigned,
    /// Due in a phase that isn't broken into packages yet.
    Planned,
    /// Out of scope for 1.0.
    Out,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Done => "done",
            Status::Review => "review",
            Status::InProgress => "in progress",
            Status::Todo => "todo",
            Status::Unassigned => "unassigned",
            Status::Planned => "planned",
            Status::Out => "out",
        }
    }

    /// Reads the status table's wording.
    fn parse(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "done" => Some(Status::Done),
            "review" | "in review" => Some(Status::Review),
            "in progress" | "building" => Some(Status::InProgress),
            "todo" | "to do" | "blocked" => Some(Status::Todo),
            _ => None,
        }
    }

    /// The status of something built by several packages.
    fn combine(statuses: &[Status]) -> Status {
        if statuses.is_empty() {
            Status::Todo
        } else if statuses.iter().all(|s| *s == Status::Done) {
            Status::Done
        } else if statuses
            .iter()
            .all(|s| matches!(s, Status::Done | Status::Review))
        {
            Status::Review
        } else if statuses
            .iter()
            .any(|s| matches!(s, Status::Done | Status::Review | Status::InProgress))
        {
            Status::InProgress
        } else {
            Status::Todo
        }
    }
}

/// A work package ID such as `1.8a`, ordered as the build plan lists them.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct WpId {
    phase: u32,
    number: u32,
    suffix: String,
}

impl WpId {
    fn parse(text: &str) -> Option<Self> {
        let (phase, rest) = text.trim().split_once('.')?;
        let digits = rest
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(rest.len());
        let (number, suffix) = rest.split_at(digits);
        if suffix.chars().any(|c| !c.is_ascii_lowercase()) {
            return None;
        }
        Some(Self {
            phase: phase.parse().ok()?,
            number: number.parse().ok()?,
            suffix: suffix.to_string(),
        })
    }

    fn text(&self) -> String {
        format!("{}.{}{}", self.phase, self.number, self.suffix)
    }
}

#[derive(Debug)]
struct Package {
    id: WpId,
    title: String,
    size: Option<String>,
    status: Status,
    date: Option<String>,
    pr: Option<(u32, String)>,
    note: Option<String>,
}

#[derive(Debug)]
struct Phase {
    number: u32,
    title: String,
    goal: Option<String>,
    packages: Vec<Package>,
    epics: Option<String>,
    estimate: Option<u32>,
}

#[derive(Debug)]
struct Milestone {
    number: u32,
    name: String,
    release: String,
    promise: String,
    exit: String,
}

#[derive(Debug)]
struct Part {
    milestone: Option<u32>,
    label: Option<String>,
    status: Status,
    packages: Vec<String>,
}

#[derive(Debug)]
struct Feature {
    area: String,
    name: String,
    beatlayer_only: bool,
    notes: String,
    parts: Vec<Part>,
}

#[derive(Debug)]
struct Improvement {
    id: String,
    title: String,
    built_in: String,
    decision: String,
    phase: Option<u32>,
    status: Status,
    packages: Vec<String>,
}

/// The whole model.
pub struct Progress {
    milestones: Vec<Milestone>,
    phases: Vec<Phase>,
    features: Vec<Feature>,
    improvements: Vec<Improvement>,
    /// `WP-` references that match no package, with where they were found.
    unknown_refs: Vec<String>,
    reagent: Option<Reagent>,
}

impl Progress {
    pub fn parse(docs: &Docs) -> Self {
        let milestones = parse_milestones(&docs.roadmap);
        let phases = parse_phases(&docs.plan);
        let mut unknown_refs = Vec::new();
        let features = parse_features(&docs.parity, &phases, &mut unknown_refs);
        let improvements = parse_improvements(&docs.improvements, &phases, &mut unknown_refs);
        Self {
            milestones,
            phases,
            features,
            improvements,
            unknown_refs,
            reagent: docs.reagent.as_deref().map(Reagent::parse),
        }
    }

    /// The model as JSON, for the dashboard and for agents.
    pub fn to_json(&self) -> Value {
        let mut package_counts: BTreeMap<&str, usize> = BTreeMap::new();
        let mut packages_total = 0;
        let phases: Vec<Value> = self
            .phases
            .iter()
            .map(|phase| {
                let milestone = self.milestones.iter().find(|m| m.number == phase.number);
                for package in &phase.packages {
                    *package_counts.entry(package.status.as_str()).or_default() += 1;
                }
                let squares = if phase.packages.is_empty() {
                    let estimate = phase.estimate.unwrap_or(0) as usize;
                    *package_counts.entry(Status::Planned.as_str()).or_default() += estimate;
                    estimate
                } else {
                    phase.packages.len()
                };
                packages_total += squares;
                json!({
                    "number": phase.number,
                    "title": phase.title,
                    "goal": phase.goal,
                    "milestone": milestone.map(|m| json!({
                        "number": m.number,
                        "name": m.name,
                        "release": m.release,
                        "promise": m.promise,
                        "exit": m.exit,
                    })),
                    "epics": phase.epics,
                    "estimate": phase.estimate,
                    "packages": phase.packages.iter().map(|p| json!({
                        "id": p.id.text(),
                        "title": p.title,
                        "size": p.size,
                        "status": p.status.as_str(),
                        "date": p.date,
                        "pr": p.pr.as_ref().map(|(number, url)| json!({"number": number, "url": url})),
                        "note": p.note,
                    })).collect::<Vec<_>>(),
                })
            })
            .collect();

        let mut part_counts: BTreeMap<&str, usize> = BTreeMap::new();
        for part in self.features.iter().flat_map(|f| &f.parts) {
            *part_counts.entry(part.status.as_str()).or_default() += 1;
        }
        let features: Vec<Value> = self
            .features
            .iter()
            .map(|f| {
                json!({
                    "area": f.area,
                    "name": f.name,
                    "beatlayer_only": f.beatlayer_only,
                    "notes": f.notes,
                    "parts": f.parts.iter().map(|p| json!({
                        "milestone": p.milestone,
                        "label": p.label,
                        "status": p.status.as_str(),
                        "packages": p.packages,
                    })).collect::<Vec<_>>(),
                })
            })
            .collect();
        let improvements: Vec<Value> = self
            .improvements
            .iter()
            .map(|i| {
                json!({
                    "id": i.id,
                    "title": i.title,
                    "built_in": i.built_in,
                    "decision": i.decision,
                    "phase": i.phase,
                    "status": i.status.as_str(),
                    "packages": i.packages,
                })
            })
            .collect();

        json!({
            "milestones": self.milestones.iter().map(|m| json!({
                "number": m.number,
                "name": m.name,
                "release": m.release,
            })).collect::<Vec<_>>(),
            "phases": phases,
            "features": features,
            "improvements": improvements,
            "totals": {
                "packages": { "total": packages_total, "by_status": package_counts },
                "parity": {
                    "total": self.features.iter().flat_map(|f| &f.parts).filter(|p| p.status != Status::Out).count(),
                    "by_status": part_counts,
                },
            },
            "unknown_refs": self.unknown_refs,
            "reagent": self.reagent.as_ref().map(Reagent::to_json),
        })
    }
}

/// The cells of a Markdown table row, trimmed, or `None` for a line that
/// isn't a row or is the header's separator.
fn cells(line: &str) -> Option<Vec<String>> {
    let line = line.trim();
    let inner = line.strip_prefix('|')?.strip_suffix('|')?;
    let cells: Vec<String> = inner.split('|').map(|c| c.trim().to_string()).collect();
    if cells
        .iter()
        .all(|c| !c.is_empty() && c.chars().all(|ch| matches!(ch, '-' | ':')))
    {
        return None;
    }
    Some(cells)
}

/// Removes Markdown emphasis and links, keeping the text.
fn plain(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(open) = rest.find('[') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        match after.find("](").and_then(|close| {
            let url_end = after[close + 2..].find(')')?;
            Some((close, close + 2 + url_end + 1))
        }) {
            Some((close, end)) => {
                out.push_str(&after[..close]);
                rest = &after[end..];
            }
            None => {
                out.push('[');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out.replace("**", "").replace('`', "")
}

/// Every `WP-` reference in a text, such as `WP-1.8a` or `WP-2.18`.
fn wp_refs(text: &str) -> Vec<String> {
    let mut refs = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find("WP-") {
        let after = &rest[at + 3..];
        let end = after
            .find(|c: char| !(c.is_ascii_digit() || c == '.' || c.is_ascii_lowercase()))
            .unwrap_or(after.len());
        let id = after[..end].trim_end_matches('.');
        if WpId::parse(id).is_some() && !refs.iter().any(|r| r == id) {
            refs.push(id.to_string());
        }
        rest = &after[end..];
    }
    refs
}

/// The packages a reference names: `2.18` names 2.18a and 2.18b when the
/// plan has no 2.18 of its own.
fn resolve<'a>(reference: &str, phases: &'a [Phase]) -> Vec<&'a Package> {
    let all = || phases.iter().flat_map(|p| &p.packages);
    if let Some(exact) = all().find(|p| p.id.text() == reference) {
        return vec![exact];
    }
    all()
        .filter(|p| {
            p.id.text().strip_prefix(reference).is_some_and(|rest| {
                !rest.is_empty() && rest.chars().all(|c| c.is_ascii_lowercase())
            })
        })
        .collect()
}

fn parse_milestones(roadmap: &str) -> Vec<Milestone> {
    let mut milestones = Vec::new();
    for row in roadmap.lines().filter_map(cells) {
        let Some(first) = row.first() else { continue };
        let Some(rest) = first.strip_prefix('M') else {
            continue;
        };
        let Some((number, name)) = rest.split_once(' ') else {
            continue;
        };
        let Ok(number) = number.parse() else { continue };
        if row.len() < 4 {
            continue;
        }
        milestones.push(Milestone {
            number,
            name: name.to_string(),
            release: plain(&row[1]),
            promise: plain(&row[2]),
            exit: plain(&row[3]),
        });
    }
    milestones
}

fn parse_phases(plan: &str) -> Vec<Phase> {
    let mut phases: Vec<Phase> = Vec::new();
    let mut section = String::new();
    let mut statuses: Vec<Vec<String>> = Vec::new();
    for line in plan.lines() {
        if let Some(heading) = line.strip_prefix("## ") {
            section = heading.to_string();
            if let Some(rest) = heading.strip_prefix("Phase ")
                && let Some((number, title)) = rest.split_once(": ")
                && let Ok(number) = number.parse::<u32>()
            {
                let title = match title.find(" (") {
                    Some(at) => &title[..at],
                    None => title,
                };
                phases.push(Phase {
                    number,
                    title: title.to_string(),
                    goal: None,
                    packages: Vec::new(),
                    epics: None,
                    estimate: None,
                });
            }
            continue;
        }
        if let Some(goal) = line.strip_prefix("Goal: ")
            && let Some(phase) = phases.last_mut()
            && section.starts_with("Phase ")
        {
            phase.goal = Some(plain(goal));
            continue;
        }
        // Phase 0 lists its packages as headings: "### WP-0.8a GPUI spike (L),
        // WP-0.8b egui spike (L), and WP-0.8c decision (S)".
        if let Some(heading) = line.strip_prefix("### ")
            && section.starts_with("Phase ")
            && let Some(phase) = phases.last_mut()
        {
            for piece in heading.split("WP-").skip(1) {
                let id_end = piece.find(' ').unwrap_or(piece.len());
                let Some(id) = WpId::parse(&piece[..id_end]) else {
                    continue;
                };
                let rest = piece[id_end..].trim();
                let (title, size) = match rest.find(" (") {
                    Some(at) => {
                        let size = rest[at + 2..].split(')').next().unwrap_or("").to_string();
                        (rest[..at].to_string(), Some(size))
                    }
                    None => (rest.trim_end_matches([',', ' ']).to_string(), None),
                };
                phase.packages.push(Package {
                    id,
                    title,
                    size,
                    status: Status::Todo,
                    date: None,
                    pr: None,
                    note: None,
                });
            }
            continue;
        }
        let Some(row) = cells(line) else { continue };
        if section.starts_with("Phases ") && section.contains("epics") {
            // | 3 Session view | M3 | epics | 20 |
            let Some((number, title)) = row[0].split_once(' ') else {
                continue;
            };
            let Ok(number) = number.parse::<u32>() else {
                continue;
            };
            if row.len() < 4 {
                continue;
            }
            phases.push(Phase {
                number,
                title: title.to_string(),
                goal: None,
                packages: Vec::new(),
                epics: Some(plain(&row[2])),
                estimate: row[3].parse().ok(),
            });
        } else if section.starts_with("Phase ") {
            // | 1.8a | Arrangement canvas: ... | L | ... |
            let Some(id) = WpId::parse(&row[0]) else {
                continue;
            };
            if let Some(phase) = phases.last_mut()
                && row.len() >= 3
            {
                phase.packages.push(Package {
                    id,
                    title: plain(&row[1]),
                    size: Some(row[2].clone()),
                    status: Status::Todo,
                    date: None,
                    pr: None,
                    note: None,
                });
            }
        } else if section.starts_with("Status table") && row.len() >= 2 {
            statuses.push(row);
        }
    }
    phases.sort_by_key(|p| p.number);
    apply_statuses(&mut phases, &statuses);
    phases
}

/// Applies the status table: ranges ("1.8a to 1.10") first, then single
/// rows, so a single row overrides a range. A package the table lists but
/// no phase does (such as 0.13b) joins its phase after its neighbours.
fn apply_statuses(phases: &mut [Phase], rows: &[Vec<String>]) {
    let (ranges, singles): (Vec<_>, Vec<_>) = rows.iter().partition(|r| r[0].contains(" to "));
    for row in ranges {
        let Some(status) = Status::parse(&row[1]) else {
            continue;
        };
        let Some((from, to)) = row[0].split_once(" to ") else {
            continue;
        };
        let (Some(from), Some(to)) = (WpId::parse(from), WpId::parse(to)) else {
            continue;
        };
        for package in phases.iter_mut().flat_map(|p| &mut p.packages) {
            if package.id >= from && package.id <= to {
                package.status = status;
            }
        }
    }
    for row in singles {
        let Some(id) = WpId::parse(&row[0]) else {
            continue;
        };
        let Some(status) = Status::parse(&row[1]) else {
            continue;
        };
        let date = row.get(2).filter(|d| !d.is_empty()).cloned();
        let (pr, note) = row
            .get(3)
            .map(|cell| pull_request(cell))
            .unwrap_or((None, None));
        let Some(phase) = phases.iter_mut().find(|p| p.number == id.phase) else {
            continue;
        };
        let package = match phase.packages.iter().position(|p| p.id == id) {
            Some(at) => &mut phase.packages[at],
            None => {
                let at = phase
                    .packages
                    .iter()
                    .position(|p| p.id > id)
                    .unwrap_or(phase.packages.len());
                phase.packages.insert(
                    at,
                    Package {
                        id: id.clone(),
                        title: note.clone().unwrap_or_else(|| format!("WP-{}", id.text())),
                        size: None,
                        status,
                        date: None,
                        pr: None,
                        note: None,
                    },
                );
                &mut phase.packages[at]
            }
        };
        package.status = status;
        package.date = date;
        package.pr = pr;
        package.note = note;
    }
}

/// Splits a status table's PR cell, "[#19](url), fixes in progress", into
/// the pull request and the note after it.
fn pull_request(cell: &str) -> (Option<(u32, String)>, Option<String>) {
    let pr = cell.strip_prefix("[#").and_then(|rest| {
        let (number, rest) = rest.split_once("](")?;
        let (url, _) = rest.split_once(')')?;
        Some((number.parse().ok()?, url.to_string()))
    });
    let note = match &pr {
        Some(_) => cell
            .split_once(')')
            .map(|(_, rest)| rest.trim_start_matches([',', ':', ' ']).to_string()),
        None => Some(plain(cell)),
    }
    .filter(|n| !n.is_empty());
    (pr, note)
}

/// The milestones a parity row names: "M1 (WAV, AIFF, FLAC), M2 (MP3)" is two
/// parts; "M1 onwards" is one.
fn milestone_parts(text: &str) -> Vec<(u32, Option<String>)> {
    let mut parts = Vec::new();
    let bytes = text.as_bytes();
    let mut at = 0;
    while at < bytes.len() {
        let starts_word = at == 0 || !bytes[at - 1].is_ascii_alphanumeric();
        if bytes[at] == b'M' && starts_word {
            let digits = text[at + 1..]
                .find(|c: char| !c.is_ascii_digit())
                .unwrap_or(text.len() - at - 1);
            if digits > 0
                && let Ok(number) = text[at + 1..at + 1 + digits].parse()
            {
                at += 1 + digits;
                let rest = &text[at..];
                let label = rest
                    .strip_prefix(" (")
                    .and_then(|r| r.split_once(')'))
                    .map(|(label, _)| label.to_string());
                if let Some(label) = &label {
                    at += label.len() + 3;
                }
                parts.push((number, label));
                continue;
            }
        }
        at += 1;
    }
    parts
}

/// Packages that exist in a phase, and whether all of them are done.
fn phase_state(phases: &[Phase], number: u32) -> Option<bool> {
    let phase = phases.iter().find(|p| p.number == number)?;
    if phase.packages.is_empty() {
        None
    } else {
        Some(phase.packages.iter().all(|p| p.status == Status::Done))
    }
}

/// The status of a part due in `milestone` and built by `refs`.
fn part_status(milestone: u32, refs: &[String], phases: &[Phase]) -> Status {
    if refs.is_empty() {
        return match phase_state(phases, milestone) {
            Some(true) => Status::Done,
            Some(false) => Status::Unassigned,
            None => Status::Planned,
        };
    }
    let statuses: Vec<Status> = refs
        .iter()
        .flat_map(|r| resolve(r, phases))
        .map(|p| p.status)
        .collect();
    Status::combine(&statuses)
}

fn parse_features(parity: &str, phases: &[Phase], unknown: &mut Vec<String>) -> Vec<Feature> {
    let mut features = Vec::new();
    let mut area = String::new();
    let mut in_table = false;
    for line in parity.lines() {
        if let Some(heading) = line.strip_prefix("## ") {
            area = heading.trim().to_string();
            in_table = false;
            continue;
        }
        let Some(row) = cells(line) else {
            if !line.trim_start().starts_with('|') {
                in_table = false;
            }
            continue;
        };
        if row.get(1).is_some_and(|c| c == "Milestone") {
            in_table = true;
            continue;
        }
        if !in_table || row.len() < 3 {
            continue;
        }
        let (name, beatlayer_only) = match row[0].strip_prefix("(Beatlayer only) ") {
            Some(name) => (name.to_string(), true),
            None => (row[0].clone(), false),
        };
        let refs = wp_refs(&row.join(" "));
        for r in &refs {
            if resolve(r, phases).is_empty() {
                unknown.push(format!("WP-{r} in the parity matrix, \"{name}\""));
            }
        }
        let milestone_cell = row[1].trim();
        let parts =
            if milestone_cell.eq_ignore_ascii_case("out") || milestone_cell.starts_with("Out") {
                vec![Part {
                    milestone: None,
                    label: None,
                    status: Status::Out,
                    packages: Vec::new(),
                }]
            } else {
                let named = milestone_parts(milestone_cell);
                let earliest = named.iter().map(|(n, _)| *n).min();
                let single = named.len() <= 1;
                named
                    .into_iter()
                    .map(|(milestone, label)| {
                        // In a row split across milestones, a package counts for
                        // the part in its own phase; earlier phases' packages
                        // count for the earliest part.
                        let packages: Vec<String> = refs
                            .iter()
                            .filter(|r| {
                                let phase = WpId::parse(r).map(|id| id.phase).unwrap_or(u32::MAX);
                                single
                                    || phase == milestone
                                    || (Some(milestone) == earliest && phase < milestone)
                            })
                            .cloned()
                            .collect();
                        Part {
                            milestone: Some(milestone),
                            label,
                            status: part_status(milestone, &packages, phases),
                            packages,
                        }
                    })
                    .collect()
            };
        features.push(Feature {
            area: area.clone(),
            name: plain(&name),
            beatlayer_only,
            notes: plain(row.get(2).map(String::as_str).unwrap_or("")),
            parts,
        });
    }
    features
}

fn parse_improvements(text: &str, phases: &[Phase], unknown: &mut Vec<String>) -> Vec<Improvement> {
    let mut improvements = Vec::new();
    for row in text.lines().filter_map(cells) {
        if !row[0].starts_with("IMP-") || row.len() < 6 {
            continue;
        }
        let built_in = plain(&row[4]);
        let refs = wp_refs(&built_in);
        for r in &refs {
            if resolve(r, phases).is_empty() {
                unknown.push(format!("WP-{r} in the improvements, {}", row[0]));
            }
        }
        // "Phase 6" or "Phase 9" with no package yet.
        let later: Vec<u32> = built_in
            .split("Phase ")
            .skip(1)
            .filter_map(|rest| {
                rest.split(|c: char| !c.is_ascii_digit())
                    .next()?
                    .parse()
                    .ok()
            })
            .collect();
        let phase = refs
            .iter()
            .filter_map(|r| WpId::parse(r).map(|id| id.phase))
            .chain(later.iter().copied())
            .min();
        let decision = plain(&row[5]);
        let status = if refs.is_empty() {
            // Built outside the phases' packages (IMP-11, ahead of Phase 9):
            // the status column says so, and a later phase may still owe a
            // part.
            match decision.split_whitespace().next() {
                Some("Built") if later.is_empty() => Status::Done,
                Some("Built") => Status::InProgress,
                _ => Status::Planned,
            }
        } else {
            let statuses: Vec<Status> = refs
                .iter()
                .flat_map(|r| resolve(r, phases))
                .map(|p| p.status)
                .collect();
            match Status::combine(&statuses) {
                // Done in its packages, but a later phase still owes a part.
                Status::Done | Status::Review if !later.is_empty() => Status::InProgress,
                status => status,
            }
        };
        improvements.push(Improvement {
            id: row[0].clone(),
            title: plain(&row[1]),
            built_in,
            decision,
            phase,
            status,
            packages: refs,
        });
    }
    improvements
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The planning documents, or `None` in a checkout without them (they
    /// aren't published yet), where the test skips.
    fn real_docs() -> Option<Docs> {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        if !crate::ui_parity::reference_present(&root) {
            eprintln!(
                "skipped: the planning documents aren't published yet, and this checkout doesn't have them"
            );
            return None;
        }
        Some(
            Docs::read(|path| match std::fs::read_to_string(root.join(path)) {
                Ok(text) => Ok(Some(text)),
                // Reagent's plan stays with the maintainers, so a published
                // checkout doesn't have it.
                Err(e) if e.kind() == std::io::ErrorKind::NotFound && path == reagent::PATH => {
                    Ok(None)
                }
                Err(e) => Err(e.to_string()),
            })
            .expect("the planning documents are readable"),
        )
    }

    #[test]
    fn the_planning_documents_parse_into_every_phase() {
        let Some(docs) = real_docs() else {
            return;
        };
        let progress = Progress::parse(&docs);
        let numbers: Vec<u32> = progress.phases.iter().map(|p| p.number).collect();
        assert_eq!(numbers, (0..=10).collect::<Vec<_>>());
        let milestones: Vec<u32> = progress.milestones.iter().map(|m| m.number).collect();
        assert_eq!(milestones, (0..=10).collect::<Vec<_>>());
        for phase in &progress.phases[..8] {
            assert!(
                !phase.packages.is_empty(),
                "phase {} has packages",
                phase.number
            );
        }
        for phase in &progress.phases[8..] {
            assert!(
                phase.estimate.is_some_and(|n| n > 0),
                "phase {} has an estimate",
                phase.number
            );
        }
        let phase1: Vec<String> = progress.phases[1]
            .packages
            .iter()
            .map(|p| p.id.text())
            .collect();
        for id in ["1.1", "1.8a", "1.8b", "1.16"] {
            assert!(phase1.contains(&id.to_string()), "phase 1 lists {id}");
        }
        assert!(
            progress.phases[0]
                .packages
                .iter()
                .any(|p| p.id.text() == "0.13b"),
            "a package only the status table lists joins its phase"
        );
    }

    #[test]
    fn every_work_package_the_documents_name_exists() {
        let Some(docs) = real_docs() else {
            return;
        };
        let progress = Progress::parse(&docs);
        assert!(
            progress.unknown_refs.is_empty(),
            "these name work packages the build plan doesn't have:\n{}",
            progress.unknown_refs.join("\n")
        );
    }

    #[test]
    fn every_parity_row_has_a_milestone_or_is_out() {
        let Some(docs) = real_docs() else {
            return;
        };
        let progress = Progress::parse(&docs);
        assert!(
            progress.features.len() > 150,
            "{} rows",
            progress.features.len()
        );
        for feature in &progress.features {
            assert!(
                !feature.parts.is_empty(),
                "\"{}\" names no milestone",
                feature.name
            );
        }
        assert!(progress.improvements.len() >= 12);
    }

    #[test]
    fn only_reagents_plan_may_be_missing() {
        let docs = Docs::read(|path| Ok((path != reagent::PATH).then(String::new)))
            .expect("Reagent's plan may be missing");
        assert!(docs.reagent.is_none());
        assert!(Progress::parse(&docs).to_json()["reagent"].is_null());
        let missing = Docs::read(|path| Ok((path != PATHS[1]).then(String::new)));
        assert!(missing.is_err(), "the build plan may not be missing");
    }

    #[test]
    fn ranges_apply_first_and_single_rows_override_them() {
        let plan = "\
## Phase 1: Playback core (M1)

| WP | Package | Size | Depends on |
|---|---|---|---|
| 1.8a | Canvas | L | |
| 1.8b | Editing | L | |
| 1.9 | Mixer view | M | |
| 1.10 | Browser | M | |
| 1.11 | Autosave | M | |

## Status table

| WP | Status | Date | PR |
|---|---|---|---|
| 1.8a to 1.10 | in progress | | |
| 1.9 | review | 10 October 2026 | [#21](https://example.org/21), with a note |
";
        let phases = parse_phases(plan);
        let status: Vec<(String, Status)> = phases[0]
            .packages
            .iter()
            .map(|p| (p.id.text(), p.status))
            .collect();
        assert_eq!(
            status,
            [
                ("1.8a".into(), Status::InProgress),
                ("1.8b".into(), Status::InProgress),
                ("1.9".into(), Status::Review),
                ("1.10".into(), Status::InProgress),
                ("1.11".into(), Status::Todo),
            ]
        );
        let review = &phases[0].packages[2];
        assert_eq!(review.pr, Some((21, "https://example.org/21".into())));
        assert_eq!(review.note.as_deref(), Some("with a note"));
    }

    #[test]
    fn split_rows_give_each_part_its_own_phase_packages() {
        assert_eq!(
            milestone_parts("M1 (WAV, AIFF, FLAC), M2 (MP3)"),
            [(1, Some("WAV, AIFF, FLAC".into())), (2, Some("MP3".into()))]
        );
        assert_eq!(milestone_parts("M1 onwards"), [(1, None)]);
        assert_eq!(milestone_parts("M1, then every milestone"), [(1, None)]);

        let plan = "\
## Phase 1: Playback core (M1)

| WP | Package | Size |
|---|---|---|
| 1.6 | Mixer graph | M |
| 1.12 | Export | M |

## Phase 2: Make a track (M2, public alpha 0.1)

| WP | Package | Size |
|---|---|---|
| 2.1 | MIDI clips | M |

## Status table

| WP | Status | Date | PR |
|---|---|---|---|
| 1.6 | done | | |
";
        let phases = parse_phases(plan);
        let parity = "\
## Mixing

| Live feature | Milestone | Notes |
|---|---|---|
| Audio, MIDI, and Main tracks | M1 (audio, Main), M2 (MIDI) | Built in WP-1.6, WP-2.1 |
| Info View | M2 | |
| Export formats | M1 (WAV), M2 (MP3) | Built in WP-1.12 |
| Video | M9 | |
| Max for Live | Out | Ableton's product |
| Sidechain | M2 | Built in WP-2.99 |
";
        let mut unknown = Vec::new();
        let features = parse_features(parity, &phases, &mut unknown);
        let parts: Vec<Vec<(Option<u32>, Status)>> = features
            .iter()
            .map(|f| f.parts.iter().map(|p| (p.milestone, p.status)).collect())
            .collect();
        assert_eq!(
            parts,
            [
                vec![(Some(1), Status::Done), (Some(2), Status::Todo)],
                vec![(Some(2), Status::Unassigned)],
                vec![(Some(1), Status::Todo), (Some(2), Status::Unassigned)],
                vec![(Some(9), Status::Planned)],
                vec![(None, Status::Out)],
                vec![(Some(2), Status::Todo)],
            ]
        );
        assert_eq!(unknown.len(), 1, "{unknown:?}");
    }

    #[test]
    fn an_improvement_built_outside_the_phases_reads_its_status_column() {
        let text = "| IMP-90 | A | B | C | Built ahead of Phase 9 in #1; the rest in Phase 9 | Built in part |\n\
                    | IMP-91 | A | B | C | Built in #2 | Built |\n\
                    | IMP-92 | A | B | C | Phase 9 | Planned |\n";
        let mut unknown = Vec::new();
        let list = parse_improvements(text, &[], &mut unknown);
        let status: Vec<Status> = list.iter().map(|i| i.status).collect();
        assert_eq!(status, [Status::InProgress, Status::Done, Status::Planned]);
        assert!(unknown.is_empty());
        // It joins no phase's packages.
        assert!(list.iter().all(|i| i.packages.is_empty()));
    }

    #[test]
    fn statuses_combine_towards_the_least_finished() {
        use Status::*;
        assert_eq!(Status::combine(&[Done, Done]), Done);
        assert_eq!(Status::combine(&[Done, Review]), Review);
        assert_eq!(Status::combine(&[Done, Todo]), InProgress);
        assert_eq!(Status::combine(&[Todo, Todo]), Todo);
        assert_eq!(
            wp_refs("Core effects in WP-2.18. Built in WP-2.18a, WP-1.10."),
            ["2.18", "2.18a", "1.10"]
        );
    }
}

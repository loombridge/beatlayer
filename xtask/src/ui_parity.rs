//! The shortcut map in the UI parity reference (WP-0.13, decision D16).
//!
//! `docs/ui-parity/shortcuts.toml` is the source of truth for Live 12's
//! keyboard shortcuts, and `docs/ui-parity/shortcuts.md` is generated from it.
//! This module validates the TOML (unique IDs, known contexts and milestones,
//! well-formed bindings, no unexplained binding conflict within a context)
//! and renders the Markdown.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::Path;

use toml::{Table, Value};

/// The source of truth, relative to the workspace root.
pub const TOML_PATH: &str = "docs/ui-parity/shortcuts.toml";
/// The generated table, relative to the workspace root.
pub const MD_PATH: &str = "docs/ui-parity/shortcuts.md";

const MILESTONES: &[&str] = &[
    "M1", "M2", "M3", "M4", "M5", "M6", "M7", "M8", "M9", "M10", "Out",
];
const KINDS: &[&str] = &["key", "mouse", "modifier", "sequence"];
const TRIGGERS: &[&str] = &["press", "held", "latch"];

/// Named keys a binding may use, besides single letters and digits.
const NAMED_KEYS: &[&str] = &[
    "Tab",
    "Enter",
    "Esc",
    "Space",
    "Delete",
    "Backspace",
    "Home",
    "End",
    "PageUp",
    "PageDown",
    "Up",
    "Down",
    "Left",
    "Right",
    "Plus",
    "Minus",
    "Comma",
    "Period",
    "Slash",
    "LeftBracket",
    "RightBracket",
    "Menu",
];

/// Regenerates `shortcuts.md` from `shortcuts.toml`, after validating it.
pub fn generate(root: &Path) -> Result<(), String> {
    let map = load(root)?;
    let rendered = render(&map);
    let path = root.join(MD_PATH);
    std::fs::write(&path, rendered).map_err(|e| format!("couldn't write {MD_PATH}: {e}"))?;
    eprintln!(
        "wrote {MD_PATH}: {} shortcuts in {} contexts",
        map.shortcuts.len(),
        map.contexts.len()
    );
    Ok(())
}

/// Fails if `shortcuts.toml` is invalid or `shortcuts.md` is out of date.
pub fn check(root: &Path) -> Result<(), String> {
    let map = load(root)?;
    let expected = render(&map);
    let actual = std::fs::read_to_string(root.join(MD_PATH))
        .map_err(|e| format!("couldn't read {MD_PATH}: {e}"))?;
    if actual.replace("\r\n", "\n") != expected {
        return Err(format!(
            "{MD_PATH} is out of date with {TOML_PATH}; regenerate it with `cargo xtask ui-parity`"
        ));
    }
    eprintln!(
        "{TOML_PATH} is valid ({} shortcuts, no unexplained conflicts) and {MD_PATH} is in sync",
        map.shortcuts.len()
    );
    Ok(())
}

/// The validated shortcut map as the command registry needs it: contexts,
/// and each shortcut's context and keys (WP-0.14).
pub fn shortcut_table(root: &Path) -> Result<bl_commands::ShortcutTable, String> {
    let map = load(root)?;
    Ok(bl_commands::ShortcutTable::new(
        map.contexts.into_iter().map(|context| context.id),
        map.shortcuts
            .into_iter()
            .map(|shortcut| bl_commands::Shortcut {
                id: shortcut.id,
                action: shortcut.action,
                context: shortcut.context,
                mac: shortcut.mac,
                win: shortcut.win,
                milestone: shortcut.milestone,
            }),
    ))
}

/// A shortcut as the keymap needs it (WP-1.7).
pub struct KeySource {
    pub id: String,
    pub action: String,
    pub context: String,
    pub kind: String,
    pub trigger: String,
    pub milestone: String,
    pub mac: Vec<String>,
    pub win: Vec<String>,
}

/// The validated shortcut map as the keymap generator needs it: the Live
/// version observed, and every shortcut in the reference's order.
pub fn key_sources(root: &Path) -> Result<(String, Vec<KeySource>), String> {
    let map = load(root)?;
    let sources = map
        .shortcuts
        .into_iter()
        .map(|s| KeySource {
            id: s.id,
            action: s.action,
            context: s.context,
            kind: s.kind,
            trigger: s.trigger,
            milestone: s.milestone,
            mac: s.mac,
            win: s.win,
        })
        .collect();
    Ok((map.live_version, sources))
}

fn load(root: &Path) -> Result<Map, String> {
    let text = std::fs::read_to_string(root.join(TOML_PATH))
        .map_err(|e| format!("couldn't read {TOML_PATH}: {e}"))?;
    let map = parse(&text).map_err(|errors| report(&errors))?;
    let mut errors = validate(&map);
    errors.extend(cross_references(root, &map)?);
    if errors.is_empty() {
        Ok(map)
    } else {
        Err(report(&errors))
    }
}

fn report(errors: &[String]) -> String {
    format!("docs/ui-parity has problems:\n  {}", errors.join("\n  "))
}

/// The hand-written pages of the reference, whose row IDs are checked.
const PAGES: &[&str] = &[
    "docs/ui-parity/README.md",
    "docs/ui-parity/layout.md",
    "docs/ui-parity/menus.md",
    "docs/ui-parity/interactions.md",
];

/// Checks the row IDs the pages define and cite: no ID is defined twice, and
/// every cited ID (S- in shortcuts.toml; L-, M-, C-, I- in the pages' table
/// rows) exists, so a renamed or deleted row can't leave a dangling reference.
fn cross_references(root: &Path, map: &Map) -> Result<Vec<String>, String> {
    let mut pages = Vec::new();
    for page in PAGES {
        let text = std::fs::read_to_string(root.join(page))
            .map_err(|e| format!("couldn't read {page}: {e}"))?;
        pages.push((*page, text));
    }
    let shortcut_ids = map.shortcuts.iter().map(|s| s.id.as_str());
    Ok(check_pages(&pages, shortcut_ids))
}

fn check_pages<'a>(
    pages: &[(&str, String)],
    shortcut_ids: impl IntoIterator<Item = &'a str>,
) -> Vec<String> {
    let mut errors = Vec::new();
    let mut defined: BTreeSet<&str> = shortcut_ids.into_iter().collect();
    for (page, text) in pages {
        for line in text.lines() {
            let Some(first) = line
                .strip_prefix('|')
                .and_then(|rest| rest.split('|').next())
            else {
                continue;
            };
            let id = first.trim();
            if is_row_id(id) && !id.starts_with("S-") && !defined.insert(id) {
                errors.push(format!("{page}: row ID {id} is defined twice"));
            }
        }
    }
    for (page, text) in pages {
        let mut missing = BTreeSet::new();
        for token in text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-')) {
            if is_row_id(token) && !defined.contains(token) {
                missing.insert(token);
            }
        }
        for id in missing {
            errors.push(format!("{page}: cites {id}, which no page defines"));
        }
    }
    errors
}

/// A row ID such as `L-ARR-012`, `M-EDIT-013a`, `C-CLIP-SES-004`, or
/// `S-VIEW-001`.
fn is_row_id(token: &str) -> bool {
    let parts: Vec<&str> = token.split('-').collect();
    let (Some(prefix), Some(last)) = (parts.first(), parts.last()) else {
        return false;
    };
    let number = last.trim_end_matches(|c: char| c.is_ascii_lowercase());
    parts.len() >= 3
        && ["S", "L", "M", "C", "I"].contains(prefix)
        && parts[1..parts.len() - 1].iter().all(|area| {
            !area.is_empty()
                && area
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
        })
        && number.len() == 3
        && number.chars().all(|c| c.is_ascii_digit())
        && last.len() - number.len() <= 1
}

/// The whole shortcut map.
#[derive(Debug)]
struct Map {
    live_version: String,
    manual: String,
    contexts: Vec<Context>,
    shortcuts: Vec<Shortcut>,
}

#[derive(Debug)]
struct Context {
    id: String,
    description: String,
}

#[derive(Debug)]
struct Shortcut {
    id: String,
    action: String,
    context: String,
    mac: Vec<String>,
    win: Vec<String>,
    kind: String,
    trigger: String,
    milestone: String,
    source: String,
    same_as: Option<String>,
    allow_conflict: Vec<String>,
    confirmed: bool,
    notes: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Platform {
    Mac,
    Win,
}

impl Platform {
    fn name(self) -> &'static str {
        match self {
            Platform::Mac => "macOS",
            Platform::Win => "Windows",
        }
    }

    /// Modifiers in the order a canonical chord lists them.
    fn modifiers(self) -> &'static [&'static str] {
        match self {
            Platform::Mac => &["Fn", "Ctrl", "Option", "Shift", "Cmd"],
            Platform::Win => &["Ctrl", "Alt", "Shift"],
        }
    }
}

/// Reads the TOML into a [`Map`], collecting every structural error.
fn parse(text: &str) -> Result<Map, Vec<String>> {
    let table: Table = text
        .parse()
        .map_err(|e| vec![format!("not valid TOML: {e}")])?;
    let mut errors = Vec::new();

    let allowed = ["schema", "live_version", "manual", "context", "shortcut"];
    for key in table.keys() {
        if !allowed.contains(&key.as_str()) {
            errors.push(format!("unknown top-level key `{key}`"));
        }
    }
    match table.get("schema").and_then(Value::as_integer) {
        Some(1) => {}
        Some(other) => errors.push(format!("schema {other} isn't supported (expected 1)")),
        None => errors.push("missing `schema = 1`".into()),
    }
    let live_version = top_string(&table, "live_version", &mut errors);
    let manual = top_string(&table, "manual", &mut errors);

    let mut contexts = Vec::new();
    for (i, entry) in array_of_tables(&table, "context", &mut errors)
        .into_iter()
        .enumerate()
    {
        let mut fields = Fields::new(entry, format!("context #{}", i + 1));
        let context = Context {
            id: fields.string("id"),
            description: fields.string("description"),
        };
        errors.extend(fields.finish());
        contexts.push(context);
    }

    let mut shortcuts = Vec::new();
    for (i, entry) in array_of_tables(&table, "shortcut", &mut errors)
        .into_iter()
        .enumerate()
    {
        let label = entry
            .get("id")
            .and_then(Value::as_str)
            .map_or_else(|| format!("shortcut #{}", i + 1), str::to_owned);
        let mut fields = Fields::new(entry, label);
        let shortcut = Shortcut {
            id: fields.string("id"),
            action: fields.string("action"),
            context: fields.string("context"),
            mac: fields.strings("mac"),
            win: fields.strings("win"),
            kind: fields.string("kind"),
            trigger: fields.string("trigger"),
            milestone: fields.string("milestone"),
            source: fields.string("source"),
            same_as: fields.optional_string("same_as"),
            allow_conflict: fields.optional_strings("allow_conflict"),
            confirmed: fields.optional_bool("confirmed").unwrap_or(true),
            notes: fields.optional_string("notes"),
        };
        errors.extend(fields.finish());
        shortcuts.push(shortcut);
    }

    if errors.is_empty() {
        Ok(Map {
            live_version,
            manual,
            contexts,
            shortcuts,
        })
    } else {
        Err(errors)
    }
}

fn top_string(table: &Table, key: &str, errors: &mut Vec<String>) -> String {
    match table.get(key).and_then(Value::as_str) {
        Some(value) => value.to_owned(),
        None => {
            errors.push(format!("missing top-level string `{key}`"));
            String::new()
        }
    }
}

fn array_of_tables<'a>(table: &'a Table, key: &str, errors: &mut Vec<String>) -> Vec<&'a Table> {
    match table.get(key) {
        None => {
            errors.push(format!("no [[{key}]] entries"));
            Vec::new()
        }
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|item| {
                let table = item.as_table();
                if table.is_none() {
                    errors.push(format!("`{key}` must be an array of tables"));
                }
                table
            })
            .collect(),
        Some(_) => {
            errors.push(format!("`{key}` must be an array of tables"));
            Vec::new()
        }
    }
}

/// Typed access to one entry's fields that records every problem and
/// rejects fields it wasn't asked for, so a misspelt field is caught.
struct Fields<'a> {
    table: &'a Table,
    label: String,
    seen: BTreeSet<&'static str>,
    errors: Vec<String>,
}

impl<'a> Fields<'a> {
    fn new(table: &'a Table, label: String) -> Self {
        Self {
            table,
            label,
            seen: BTreeSet::new(),
            errors: Vec::new(),
        }
    }

    fn string(&mut self, key: &'static str) -> String {
        self.optional_string(key).unwrap_or_else(|| {
            self.errors
                .push(format!("{}: missing string `{key}`", self.label));
            String::new()
        })
    }

    fn optional_string(&mut self, key: &'static str) -> Option<String> {
        self.seen.insert(key);
        match self.table.get(key) {
            None => None,
            Some(Value::String(value)) if value.trim().is_empty() => {
                self.errors
                    .push(format!("{}: `{key}` is empty", self.label));
                None
            }
            Some(Value::String(value)) => Some(value.clone()),
            Some(_) => {
                self.errors
                    .push(format!("{}: `{key}` must be a string", self.label));
                None
            }
        }
    }

    fn strings(&mut self, key: &'static str) -> Vec<String> {
        if !self.table.contains_key(key) {
            self.errors
                .push(format!("{}: missing array `{key}`", self.label));
        }
        self.optional_strings(key)
    }

    fn optional_strings(&mut self, key: &'static str) -> Vec<String> {
        self.seen.insert(key);
        match self.table.get(key) {
            None => Vec::new(),
            Some(Value::Array(items)) => items
                .iter()
                .filter_map(|item| match item.as_str() {
                    Some(value) if !value.trim().is_empty() => Some(value.to_owned()),
                    _ => {
                        self.errors.push(format!(
                            "{}: `{key}` must hold non-empty strings",
                            self.label
                        ));
                        None
                    }
                })
                .collect(),
            Some(_) => {
                self.errors
                    .push(format!("{}: `{key}` must be an array", self.label));
                Vec::new()
            }
        }
    }

    fn optional_bool(&mut self, key: &'static str) -> Option<bool> {
        self.seen.insert(key);
        match self.table.get(key) {
            None => None,
            Some(Value::Boolean(value)) => Some(*value),
            Some(_) => {
                self.errors
                    .push(format!("{}: `{key}` must be true or false", self.label));
                None
            }
        }
    }

    fn finish(mut self) -> Vec<String> {
        for key in self.table.keys() {
            if !self.seen.contains(key.as_str()) {
                self.errors
                    .push(format!("{}: unknown field `{key}`", self.label));
            }
        }
        self.errors
    }
}

/// Checks everything that needs the whole map: IDs, references, contexts,
/// enumerations, binding syntax, and conflicts.
fn validate(map: &Map) -> Vec<String> {
    let mut errors = Vec::new();

    let mut context_ids = BTreeSet::new();
    for context in &map.contexts {
        if !is_context_id(&context.id) {
            errors.push(format!(
                "context `{}`: IDs are lower-case words joined by hyphens",
                context.id
            ));
        }
        if !context_ids.insert(context.id.as_str()) {
            errors.push(format!("context `{}` is defined twice", context.id));
        }
    }

    let mut ids = BTreeSet::new();
    for shortcut in &map.shortcuts {
        if !is_shortcut_id(&shortcut.id) {
            errors.push(format!(
                "{}: IDs look like S-AREA-001 (upper-case area, three digits)",
                shortcut.id
            ));
        }
        if !ids.insert(shortcut.id.as_str()) {
            errors.push(format!("{}: duplicate ID", shortcut.id));
        }
    }

    for shortcut in &map.shortcuts {
        let id = &shortcut.id;
        if !context_ids.contains(shortcut.context.as_str()) {
            errors.push(format!("{id}: unknown context `{}`", shortcut.context));
        }
        if !KINDS.contains(&shortcut.kind.as_str()) {
            errors.push(format!(
                "{id}: kind `{}` isn't one of {}",
                shortcut.kind,
                KINDS.join(", ")
            ));
        }
        if !TRIGGERS.contains(&shortcut.trigger.as_str()) {
            errors.push(format!(
                "{id}: trigger `{}` isn't one of {}",
                shortcut.trigger,
                TRIGGERS.join(", ")
            ));
        }
        if !MILESTONES.contains(&shortcut.milestone.as_str()) {
            errors.push(format!(
                "{id}: milestone `{}` isn't one of {}",
                shortcut.milestone,
                MILESTONES.join(", ")
            ));
        }
        if shortcut.mac.is_empty() && shortcut.win.is_empty() {
            errors.push(format!("{id}: has no binding on either platform"));
        }
        for reference in shortcut.same_as.iter().chain(&shortcut.allow_conflict) {
            if reference == id {
                errors.push(format!("{id}: refers to itself"));
            } else if !ids.contains(reference.as_str()) {
                errors.push(format!("{id}: refers to unknown shortcut `{reference}`"));
            }
        }
        if shortcut.kind == "key" {
            for (platform, bindings) in [
                (Platform::Mac, &shortcut.mac),
                (Platform::Win, &shortcut.win),
            ] {
                for binding in bindings {
                    if let Err(message) = chords(binding, platform) {
                        errors.push(format!(
                            "{id}: {} binding `{binding}`: {message}",
                            platform.name()
                        ));
                    }
                }
            }
        }
    }

    errors.extend(conflicts(map));
    errors
}

fn is_context_id(id: &str) -> bool {
    !id.is_empty()
        && !id.starts_with('-')
        && !id.ends_with('-')
        && id.chars().all(|c| c.is_ascii_lowercase() || c == '-')
}

fn is_shortcut_id(id: &str) -> bool {
    let mut parts = id.split('-');
    let (Some("S"), Some(area), Some(number), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return false;
    };
    !area.is_empty()
        && area
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
        && number.len() == 3
        && number.chars().all(|c| c.is_ascii_digit())
}

/// Finds key chords bound twice in one context on one platform, unless the
/// two entries say they're the same command (`same_as`) or that the overlap
/// is known (`allow_conflict`).
fn conflicts(map: &Map) -> Vec<String> {
    let mut owners: BTreeMap<(&str, Platform, String), Vec<&Shortcut>> = BTreeMap::new();
    for shortcut in map.shortcuts.iter().filter(|s| s.kind == "key") {
        for (platform, bindings) in [
            (Platform::Mac, &shortcut.mac),
            (Platform::Win, &shortcut.win),
        ] {
            let mut mine = BTreeSet::new();
            for binding in bindings {
                // Malformed bindings are reported by `validate`.
                mine.extend(chords(binding, platform).unwrap_or_default());
            }
            for chord in mine {
                owners
                    .entry((shortcut.context.as_str(), platform, chord))
                    .or_default()
                    .push(shortcut);
            }
        }
    }

    let mut errors = Vec::new();
    for ((context, platform, chord), shortcuts) in &owners {
        for (i, a) in shortcuts.iter().enumerate() {
            for b in &shortcuts[i + 1..] {
                if !explained(a, b) {
                    errors.push(format!(
                        "{} and {} both bind {chord} on {} in context `{context}`; \
                         mark one `same_as` the other, or list it in `allow_conflict` with a note",
                        a.id,
                        b.id,
                        platform.name()
                    ));
                }
            }
        }
    }
    errors
}

fn explained(a: &Shortcut, b: &Shortcut) -> bool {
    let names = |s: &Shortcut, other: &Shortcut| {
        s.same_as.as_deref() == Some(other.id.as_str()) || s.allow_conflict.contains(&other.id)
    };
    let same_command = |a: &Shortcut, b: &Shortcut| {
        // Two entries marked the same as a third are the same command too.
        a.same_as.is_some() && a.same_as == b.same_as
    };
    names(a, b) || names(b, a) || same_command(a, b)
}

/// Parses one binding into canonical chords, expanding ranges such as
/// `F1..F8`. Modifiers are put in a fixed order, so `Shift+Cmd+S` and
/// `Cmd+Shift+S` are the same chord.
fn chords(binding: &str, platform: Platform) -> Result<Vec<String>, String> {
    let parts: Vec<&str> = binding.split('+').map(str::trim).collect();
    let (key, modifiers) = parts.split_last().ok_or("empty binding")?;

    let order = platform.modifiers();
    let mut held = BTreeSet::new();
    for modifier in modifiers {
        let Some(rank) = order.iter().position(|m| m == modifier) else {
            return Err(format!(
                "`{modifier}` isn't a {} modifier (use {})",
                platform.name(),
                order.join(", ")
            ));
        };
        if !held.insert(rank) {
            return Err(format!("`{modifier}` appears twice"));
        }
    }
    let prefix: String = held
        .iter()
        .map(|&rank| format!("{}+", order[rank]))
        .collect();

    Ok(keys(key)?
        .into_iter()
        .map(|key| format!("{prefix}{key}"))
        .collect())
}

/// One key name, or a range of letters, digits, or function keys.
fn keys(key: &str) -> Result<Vec<String>, String> {
    if let Some((first, last)) = key.split_once("..") {
        let (Some((prefix_a, a)), Some((prefix_b, b))) = (split_number(first), split_number(last))
        else {
            return Err(format!("`{key}` isn't a range of digits or function keys"));
        };
        if prefix_a != prefix_b || a >= b || !matches!(prefix_a, "" | "F") {
            return Err(format!("`{key}` isn't an increasing range"));
        }
        let range: Vec<String> = (a..=b).map(|n| format!("{prefix_a}{n}")).collect();
        for k in &range {
            check_key(k)?;
        }
        return Ok(range);
    }
    check_key(key)?;
    Ok(vec![key.to_owned()])
}

fn split_number(key: &str) -> Option<(&str, u32)> {
    let digits = key.trim_start_matches(|c: char| !c.is_ascii_digit());
    let prefix = &key[..key.len() - digits.len()];
    Some((prefix, digits.parse().ok()?))
}

fn check_key(key: &str) -> Result<(), String> {
    let single = key.len() == 1
        && key
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit());
    let function = key
        .strip_prefix('F')
        .and_then(|n| n.parse::<u32>().ok())
        .is_some_and(|n| (1..=12).contains(&n));
    if single || function || NAMED_KEYS.contains(&key) {
        Ok(())
    } else {
        Err(format!(
            "unknown key `{key}` (use A-Z, 0-9, F1-F12, or one of {})",
            NAMED_KEYS.join(", ")
        ))
    }
}

/// Renders the generated Markdown table.
fn render(map: &Map) -> String {
    let mut out = String::new();
    let count = |milestone: &str| {
        map.shortcuts
            .iter()
            .filter(|s| s.milestone == milestone)
            .count()
    };
    let flagged: Vec<&Shortcut> = map
        .shortcuts
        .iter()
        .filter(|s| {
            !s.confirmed || s.same_as.is_some() || !s.allow_conflict.is_empty() || s.notes.is_some()
        })
        .collect();
    let unconfirmed = map.shortcuts.iter().filter(|s| !s.confirmed).count();

    let _ = writeln!(out, "# Keyboard shortcuts");
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "<!-- Generated from shortcuts.toml by `cargo xtask ui-parity`. Don't edit this file by hand: \
         edit shortcuts.toml and regenerate. `cargo xtask check` fails if the two differ. -->"
    );
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "Live {}'s keyboard shortcuts, the default keymap Beatlayer binds (D16). Source: {}, \
         and observation of Live {} where the Source column says so. \
         {} shortcuts in {} contexts; {} need confirming (see the notes at the end). \
         How to read and change this table: [README](README.md).",
        map.live_version,
        map.manual,
        map.live_version,
        map.shortcuts.len(),
        map.contexts.len(),
        unconfirmed
    );
    let _ = writeln!(out);
    let _ = writeln!(out, "## By milestone");
    let _ = writeln!(out);
    let _ = writeln!(out, "| Milestone | Shortcuts |");
    let _ = writeln!(out, "|---|---|");
    for milestone in MILESTONES {
        let n = count(milestone);
        if n > 0 {
            let _ = writeln!(out, "| {milestone} | {n} |");
        }
    }
    let _ = writeln!(out);
    let _ = writeln!(out, "## Contexts");
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "A shortcut applies in its context. The same keys may mean different things in different \
         contexts; within one context they must not, unless the notes explain it."
    );
    let _ = writeln!(out);
    let _ = writeln!(out, "| Context | Applies when | Shortcuts |");
    let _ = writeln!(out, "|---|---|---|");
    for context in &map.contexts {
        let n = map
            .shortcuts
            .iter()
            .filter(|s| s.context == context.id)
            .count();
        let _ = writeln!(
            out,
            "| [`{}`](#{}) | {} | {n} |",
            context.id,
            context.id,
            cell(&context.description)
        );
    }

    for context in &map.contexts {
        let rows: Vec<&Shortcut> = map
            .shortcuts
            .iter()
            .filter(|s| s.context == context.id)
            .collect();
        if rows.is_empty() {
            continue;
        }
        let _ = writeln!(out);
        let _ = writeln!(out, "## {}", context.id);
        let _ = writeln!(out);
        let _ = writeln!(out, "{}", context.description);
        let _ = writeln!(out);
        let _ = writeln!(
            out,
            "| ID | Action | macOS | Windows | Kind | Trigger | Milestone | Source |"
        );
        let _ = writeln!(out, "|---|---|---|---|---|---|---|---|");
        for s in rows {
            let marker = if s.confirmed { "" } else { " (unconfirmed)" };
            let _ = writeln!(
                out,
                "| {} | {}{marker} | {} | {} | {} | {} | {} | {} |",
                s.id,
                cell(&s.action),
                bindings(&s.mac, &s.kind),
                bindings(&s.win, &s.kind),
                s.kind,
                s.trigger,
                s.milestone,
                cell(&s.source)
            );
        }
    }

    let _ = writeln!(out);
    let _ = writeln!(out, "## Notes, shared keys, and unconfirmed bindings");
    let _ = writeln!(out);
    let _ = writeln!(out, "| ID | Status | Note |");
    let _ = writeln!(out, "|---|---|---|");
    for s in flagged {
        let mut status = Vec::new();
        if !s.confirmed {
            status.push("unconfirmed".to_owned());
        }
        if let Some(other) = &s.same_as {
            status.push(format!("same command as {other}"));
        }
        if !s.allow_conflict.is_empty() {
            status.push(format!("shares keys with {}", s.allow_conflict.join(", ")));
        }
        if status.is_empty() {
            status.push("note".to_owned());
        }
        let _ = writeln!(
            out,
            "| {} | {} | {} |",
            s.id,
            cell(&status.join("; ")),
            cell(s.notes.as_deref().unwrap_or(""))
        );
    }
    out
}

fn bindings(bindings: &[String], kind: &str) -> String {
    if bindings.is_empty() {
        return "none".into();
    }
    bindings
        .iter()
        .map(|b| {
            if kind == "key" {
                format!("`{b}`")
            } else {
                cell(b)
            }
        })
        .collect::<Vec<_>>()
        .join(" or ")
}

/// Escapes text for a Markdown table cell.
fn cell(text: &str) -> String {
    text.replace('|', "\\|").replace('\n', " ")
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEADER: &str = r#"
schema = 1
live_version = "12.4.3"
manual = "test"

[[context]]
id = "global"
description = "Anywhere."

[[context]]
id = "session"
description = "Session View."
"#;

    fn entry(id: &str, context: &str, mac: &str, extra: &str) -> String {
        format!(
            r#"
[[shortcut]]
id = "{id}"
action = "Do something"
context = "{context}"
mac = ["{mac}"]
win = ["Ctrl+X"]
kind = "key"
trigger = "press"
milestone = "M1"
source = "manual 42.1"
{extra}
"#
        )
    }

    fn errors(text: &str) -> Vec<String> {
        match parse(text) {
            Ok(map) => validate(&map),
            Err(errors) => errors,
        }
    }

    #[test]
    fn modifiers_are_put_in_a_fixed_order() {
        assert_eq!(
            chords("Shift+Cmd+S", Platform::Mac),
            Ok(vec!["Shift+Cmd+S".to_owned()])
        );
        assert_eq!(
            chords("Cmd+Option+Ctrl+V", Platform::Mac),
            Ok(vec!["Ctrl+Option+Cmd+V".to_owned()])
        );
        assert_eq!(
            chords("Alt+Ctrl+V", Platform::Win),
            Ok(vec!["Ctrl+Alt+V".to_owned()])
        );
    }

    #[test]
    fn ranges_expand() {
        assert_eq!(
            chords("F1..F3", Platform::Win),
            Ok(vec!["F1".to_owned(), "F2".to_owned(), "F3".to_owned()])
        );
        assert_eq!(chords("Cmd+0..2", Platform::Mac).map(|c| c.len()), Ok(3));
        assert!(chords("F3..F1", Platform::Win).is_err());
    }

    #[test]
    fn platform_modifiers_are_enforced() {
        assert!(chords("Alt+X", Platform::Mac).is_err());
        assert!(chords("Cmd+X", Platform::Win).is_err());
        assert!(chords("Option+X", Platform::Win).is_err());
        assert!(chords("Cmd+Cmd+X", Platform::Mac).is_err());
    }

    #[test]
    fn unknown_keys_are_rejected() {
        assert!(chords("Cmd+Return", Platform::Mac).is_err());
        assert!(chords("Cmd+a", Platform::Mac).is_err());
        assert!(chords("F13", Platform::Win).is_err());
        assert!(chords("Cmd+PageUp", Platform::Mac).is_ok());
    }

    #[test]
    fn a_valid_map_has_no_errors() {
        let text = format!(
            "{HEADER}{}{}",
            entry("S-EDIT-001", "global", "Cmd+X", ""),
            entry("S-SES-001", "session", "Cmd+X", "")
        );
        assert_eq!(errors(&text), Vec::<String>::new());
    }

    #[test]
    fn duplicate_ids_are_rejected() {
        let text = format!(
            "{HEADER}{}{}",
            entry("S-EDIT-001", "global", "Cmd+X", ""),
            entry("S-EDIT-001", "session", "Cmd+C", "")
        );
        assert!(errors(&text).iter().any(|e| e.contains("duplicate ID")));
    }

    #[test]
    fn conflicts_in_one_context_are_rejected() {
        let text = format!(
            "{HEADER}{}{}",
            entry("S-EDIT-001", "global", "Cmd+X", ""),
            entry("S-EDIT-002", "global", "Cmd+X", "")
        );
        let errors = errors(&text);
        assert!(
            errors
                .iter()
                .any(|e| e.contains("both bind Cmd+X on macOS")),
            "{errors:?}"
        );
        assert!(
            errors
                .iter()
                .any(|e| e.contains("both bind Ctrl+X on Windows"))
        );
    }

    #[test]
    fn explained_conflicts_are_allowed() {
        let same = format!(
            "{HEADER}{}{}",
            entry("S-EDIT-001", "global", "Cmd+X", ""),
            entry("S-EDIT-002", "global", "Cmd+X", r#"same_as = "S-EDIT-001""#)
        );
        assert_eq!(errors(&same), Vec::<String>::new());
        let known = format!(
            "{HEADER}{}{}",
            entry(
                "S-EDIT-001",
                "global",
                "Cmd+X",
                r#"allow_conflict = ["S-EDIT-002"]"#
            ),
            entry("S-EDIT-002", "global", "Cmd+X", "")
        );
        assert_eq!(errors(&known), Vec::<String>::new());
    }

    #[test]
    fn unknown_fields_and_values_are_rejected() {
        let text = format!(
            "{HEADER}{}",
            entry("S-EDIT-001", "nowhere", "Cmd+X", r#"milstone = "M1""#)
        );
        let errors = errors(&text);
        assert!(
            errors
                .iter()
                .any(|e| e.contains("unknown field `milstone`"))
        );
        let text = format!("{HEADER}{}", entry("S-EDIT-001", "nowhere", "Cmd+X", ""));
        let errors = super::tests::errors(&text);
        assert!(errors.iter().any(|e| e.contains("unknown context")));
    }

    #[test]
    fn rendering_marks_the_file_as_generated() {
        let text = format!("{HEADER}{}", entry("S-EDIT-001", "global", "Cmd+X", ""));
        let map = parse(&text).expect("valid");
        let md = render(&map);
        assert!(md.contains("Generated from shortcuts.toml"));
        assert!(md.contains("| S-EDIT-001 | Do something | `Cmd+X` | `Ctrl+X` |"));
    }

    #[test]
    fn row_ids_are_recognised() {
        for id in [
            "L-ARR-012",
            "M-EDIT-013a",
            "C-CLIP-SES-004",
            "S-VIEW-001",
            "L-WIN2-001",
        ] {
            assert!(is_row_id(id), "{id}");
        }
        for id in [
            "L-AREA-NNN",
            "M-EDIT-13",
            "X-ABC-001",
            "M-EDIT-013ab",
            "L--001",
            "L-arr-001",
        ] {
            assert!(!is_row_id(id), "{id}");
        }
    }

    #[test]
    fn pages_must_define_what_they_cite() {
        let pages = [
            (
                "a.md",
                "| L-ARR-001 | thing |\n| L-ARR-002 | other, see L-ARR-001 and S-VIEW-001 |\n"
                    .to_owned(),
            ),
            (
                "b.md",
                "| M-FILE-001 | New | S-VIEW-002 | see L-ARR-003 |\n| L-ARR-001 | again |\n"
                    .to_owned(),
            ),
        ];
        let errors = check_pages(&pages, ["S-VIEW-001"]);
        assert!(
            errors.iter().any(|e| e.contains("b.md: cites L-ARR-003")),
            "{errors:?}"
        );
        assert!(
            errors.iter().any(|e| e.contains("b.md: cites S-VIEW-002")),
            "{errors:?}"
        );
        assert!(
            errors
                .iter()
                .any(|e| e.contains("L-ARR-001 is defined twice")),
            "{errors:?}"
        );
        assert_eq!(errors.len(), 3, "{errors:?}");
    }

    /// The real map must be valid and its table in sync, so `cargo test`
    /// catches drift as well as `cargo xtask check`.
    #[test]
    fn the_repository_map_is_valid_and_in_sync() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("xtask sits in the workspace root");
        check(root).expect("docs/ui-parity is valid and in sync");
    }
}

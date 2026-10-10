//! The default keymap (WP-1.7; the architecture, "The keymap").
//!
//! `crates/ui/keymaps/live12.toml` is generated from the UI parity
//! reference's shortcuts and the command registry: every shortcut in
//! `docs/ui-parity/shortcuts.toml`, in its order, with what it does in
//! Beatlayer. A shortcut is `bound` when a registered command binds it, and
//! the entry lists each command with the contexts it runs in. Otherwise it's
//! `system` or `widget` (the platform or the focused control handles it), or
//! `pending` with the milestone that binds it; an M1 shortcut that's pending
//! also names the package that binds it, from [`OWNERS`].
//!
//! `cargo xtask check` fails if the file is out of date, and generation
//! fails if an M1 shortcut is neither bound nor owned by a package, so the
//! keymap test in `bl-ui` (`tests/keymap.rs`) holds by construction.

use std::fmt::Write as _;
use std::path::Path;

use bl_commands::{Context, Registry};
use serde_json::Value;

use crate::commands;
use crate::ui_parity::{self, KeySource};

/// The generated keymap, relative to the workspace root.
pub const PATH: &str = "crates/ui/keymaps/live12.toml";

/// Shortcuts the operating system handles, with how.
const SYSTEM: &[(&str, &str)] = &[
    (
        "S-MENU-001",
        "Windows' native menu bar opens a menu with Alt and its title's underlined letter",
    ),
    ("S-MENU-002", "the platform's menus move between items"),
    ("S-MENU-003", "the platform's menus move between menus"),
    (
        "S-MENU-004",
        "the platform's menus choose the highlighted item",
    ),
    ("S-MENU-005", "macOS's Help menu searches the menus"),
];

/// Shortcuts the focused control handles, with which control.
const WIDGET: &[(&str, &str)] = &[
    (
        "S-SET-003",
        "the focused Settings chooser steps to the next or previous choice",
    ),
    ("S-SET-004", "the focused Settings switch toggles"),
];

/// The package that binds each M1 shortcut the shell doesn't: exact IDs
/// first, then ID prefixes. The keymap test fails if an M1 shortcut has
/// no command and no package here, or if this names WP-1.7 itself.
const OWNERS: &[(&str, &str)] = &[
    ("S-VIEW-011", "WP-1.8a"),
    ("S-VIEW-012", "WP-1.9"),
    ("S-VIEW-014", "WP-1.9"),
    ("S-FOCUS-006", "WP-1.10"),
    ("S-FOCUS-013", "WP-1.14"),
    ("S-FOCUS-014", "WP-1.14"),
    ("S-FILE-007", "WP-1.12"),
    ("S-EDIT-012", "WP-1.8a"),
    ("S-ENV-004", "WP-1.8b"),
    ("S-ZOOM-003", "WP-1.8a"),
    ("S-ZOOM-004", "WP-1.8a"),
    ("S-ZOOM-005", "WP-1.8a"),
    ("S-ZOOM-006", "WP-1.8a"),
    ("S-ZOOM-007", "WP-1.8a"),
    ("S-GRID-007", "WP-1.8b"),
    ("S-ARR-020", "WP-1.8a"),
    ("S-ARR-021", "WP-1.8a"),
    ("S-ARR-022", "WP-1.8a"),
    ("S-ARR-023", "WP-1.8a"),
    ("S-ARR-024", "WP-1.8a"),
    ("S-ARR-025", "WP-1.8a"),
    ("S-ARR-026", "WP-1.8a"),
    ("S-ARR-030", "WP-1.8a"),
    ("S-ARR-031", "WP-1.8a"),
    ("S-ARR-032", "WP-1.5"),
    ("S-ARR-033", "WP-1.5"),
    ("S-ARR-034", "WP-1.8a"),
    ("S-ARR-035", "WP-1.9"),
    ("S-TRACK-005", "WP-1.8a"),
    ("S-TRACK-008", "WP-1.8a"),
    ("S-TRACK-009", "WP-1.8a"),
    ("S-TRACK-010", "WP-1.8a"),
    ("S-A11Y-003", "WP-1.8a"),
    ("S-A11Y-015", "WP-1.10"),
    ("S-A11Y-016", "WP-1.10"),
    ("S-A11Y-017", "WP-1.10"),
    // Prefixes.
    ("S-EDIT-", "WP-1.8b"),
    ("S-VAL-", "WP-1.5"),
    ("S-LOOP-", "WP-1.8b"),
    ("S-ZOOM-", "WP-1.8b"),
    ("S-GRID-", "WP-1.8a"),
    ("S-ARR-", "WP-1.8b"),
    ("S-TRACK-", "WP-1.8b"),
    ("S-TRANS-", "WP-1.5"),
    ("S-BRW-", "WP-1.10"),
    ("S-CTX-", "WP-1.8b"),
    ("S-A11Y-", "WP-1.8b"),
];

/// The package that binds an M1 shortcut, from [`OWNERS`].
fn owner(id: &str) -> Option<&'static str> {
    OWNERS
        .iter()
        .find(|(key, _)| *key == id)
        .or_else(|| {
            OWNERS
                .iter()
                .find(|(key, _)| key.ends_with('-') && id.starts_with(key))
        })
        .map(|(_, package)| *package)
}

/// Regenerates the keymap.
pub fn generate(root: &Path) -> Result<(), String> {
    let (rendered, summary) = build(root)?;
    std::fs::write(root.join(PATH), rendered).map_err(|e| format!("couldn't write {PATH}: {e}"))?;
    eprintln!("wrote {PATH}: {summary}");
    Ok(())
}

/// Fails if the keymap is out of date with the shortcuts and the registry.
pub fn check(root: &Path) -> Result<(), String> {
    let (expected, summary) = build(root)?;
    let actual = std::fs::read_to_string(root.join(PATH))
        .map_err(|e| format!("couldn't read {PATH}: {e}"))?;
    if actual.replace("\r\n", "\n") != expected {
        return Err(format!(
            "{PATH} is out of date with docs/ui-parity/shortcuts.toml and the command registry; \
             regenerate it with `cargo xtask keymap`"
        ));
    }
    eprintln!("{PATH} is in sync: {summary}");
    Ok(())
}

fn build(root: &Path) -> Result<(String, String), String> {
    let (live_version, sources) = ui_parity::key_sources(root)?;
    let registry = commands::catalogue()?;
    render(&live_version, &sources, &registry)
}

/// A string as a TOML basic string.
fn quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn list(items: &[String]) -> String {
    let quoted: Vec<String> = items.iter().map(|s| quote(s)).collect();
    format!("[{}]", quoted.join(", "))
}

/// A JSON argument value as a TOML inline value.
fn toml_value(value: &Value) -> Result<String, String> {
    match value {
        Value::String(s) => Ok(quote(s)),
        Value::Bool(b) => Ok(b.to_string()),
        Value::Number(n) => Ok(n.to_string()),
        other => Err(format!(
            "a binding fixes {other}, which the keymap can't write"
        )),
    }
}

fn render(
    live_version: &str,
    sources: &[KeySource],
    registry: &Registry,
) -> Result<(String, String), String> {
    let mut out = String::new();
    out.push_str(
        "# Beatlayer's default keymap: Live 12's shortcuts (D16).\n\
         #\n\
         # Generated by `cargo xtask keymap` from docs/ui-parity/shortcuts.toml and the\n\
         # command registry. Don't edit it by hand: `cargo xtask check` fails if it\n\
         # differs from what they generate. Each shortcut's `status` says what it does:\n\
         # `bound` (the commands it runs, each in the contexts listed), `system` (the\n\
         # platform handles it), `widget` (the focused control does), or `pending`\n\
         # (nothing yet; the milestone binds it, and `package` names the package for M1).\n\n",
    );
    let _ = writeln!(out, "schema = 1\nlive_version = {}", quote(live_version));

    let (mut bound, mut system, mut widget, mut pending) = (0, 0, 0, 0);
    let mut problems = Vec::new();
    for source in sources {
        let _ = write!(
            out,
            "\n[[shortcut]]\nid = {}\naction = {}\ncontext = {}\nkind = {}\ntrigger = {}\n\
             milestone = {}\nmac = {}\nwin = {}\n",
            quote(&source.id),
            quote(&source.action),
            quote(&source.context),
            quote(&source.kind),
            quote(&source.trigger),
            quote(&source.milestone),
            list(&source.mac),
            list(&source.win),
        );
        let mut commands = Vec::new();
        for spec in registry {
            for binding in spec.bindings.iter().filter(|b| b.shortcut == source.id) {
                let contexts: Vec<String> = if source.context == Context::Global.id() {
                    spec.contexts.iter().map(|c| c.id().to_owned()).collect()
                } else {
                    vec![source.context.clone()]
                };
                let mut line = format!(
                    "{{ id = {}, contexts = {}",
                    quote(&spec.id),
                    list(&contexts)
                );
                if !binding.args.is_empty() {
                    let args = binding
                        .args
                        .iter()
                        .map(|(k, v)| Ok(format!("{k} = {}", toml_value(v)?)))
                        .collect::<Result<Vec<_>, String>>()?;
                    let _ = write!(line, ", args = {{ {} }}", args.join(", "));
                }
                if let Some(toggles) = &binding.toggles {
                    let _ = write!(line, ", toggles = {}", quote(toggles));
                }
                line.push_str(" }");
                commands.push(line);
            }
        }
        if !commands.is_empty() {
            bound += 1;
            out.push_str("status = \"bound\"\ncommands = [\n");
            for command in commands {
                let _ = writeln!(out, "  {command},");
            }
            out.push_str("]\n");
        } else if let Some((_, how)) = SYSTEM.iter().find(|(id, _)| *id == source.id) {
            system += 1;
            let _ = writeln!(out, "status = \"system\"\nhandler = {}", quote(how));
        } else if let Some((_, how)) = WIDGET.iter().find(|(id, _)| *id == source.id) {
            widget += 1;
            let _ = writeln!(out, "status = \"widget\"\nhandler = {}", quote(how));
        } else {
            pending += 1;
            out.push_str("status = \"pending\"\n");
            if source.milestone == "M1" {
                match owner(&source.id) {
                    Some(package) => {
                        let _ = writeln!(out, "package = {}", quote(package));
                    }
                    None => problems.push(format!(
                        "{} is an M1 shortcut that no command binds and no package owns; bind it \
                         or add it to OWNERS in xtask/src/keymap.rs",
                        source.id
                    )),
                }
            }
        }
    }
    if !problems.is_empty() {
        return Err(format!(
            "the keymap has {} problem(s):\n  {}",
            problems.len(),
            problems.join("\n  ")
        ));
    }
    let summary = format!(
        "{} shortcuts: {bound} bound to commands, {system} handled by the system, {widget} by the \
         focused control, {pending} pending",
        sources.len()
    );
    Ok((out, summary))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owners_resolve_exact_ids_before_prefixes() {
        assert_eq!(owner("S-ARR-030"), Some("WP-1.8a"));
        assert_eq!(owner("S-ARR-001"), Some("WP-1.8b"));
        assert_eq!(owner("S-BRW-007"), Some("WP-1.10"));
        assert_eq!(owner("S-NOPE-001"), None);
        assert!(OWNERS.iter().all(|(_, package)| *package != "WP-1.7"));
    }

    #[test]
    fn the_repository_keymap_is_in_sync() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("xtask sits in the workspace root");
        check(root).expect("crates/ui/keymaps/live12.toml is in sync");
    }
}

//! The default keymap (WP-1.7; the architecture, "The keymap").
//!
//! `crates/shell-spec/keymaps/live12.toml` is generated from the UI parity
//! reference's shortcuts and the command registry: every shortcut in
//! the shortcut reference, in its order, with what it does in
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
pub const PATH: &str = "crates/shell-spec/keymaps/live12.toml";

/// Shortcuts the operating system handles, with how: the platform's menus,
/// and items its standard menus provide with their keys.
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
    (
        "S-FILE-010",
        "macOS's standard Hide Others item in the application menu (M-APP-005)",
    ),
    (
        "S-MENU-006",
        "macOS's Emoji & Symbols item, which the system adds to the Edit menu (M-EDIT-040)",
    ),
];

/// Shortcuts the focused control handles, with which control. The
/// Arrangement View reads three itself because each has keys that act
/// differently, and the keymap gives a shortcut one command with one set of
/// arguments (WP-1.8a); each runs a registered command with the arguments
/// its key means.
const WIDGET: &[(&str, &str)] = &[
    (
        "S-EDIT-016",
        "the Arrangement's automation lane: the edit keys with Option (Alt on Windows) held \
         apply to the envelopes (the automation editing spec, R6)",
    ),
    (
        "S-ENV-002",
        "the drag on an automation lane moves the breakpoint's value in finer steps",
    ),
    (
        "S-ENV-006",
        "the drag on an automation lane ignores the grid while the key is held",
    ),
    (
        "S-SET-003",
        "the focused Settings chooser steps to the next or previous choice",
    ),
    ("S-SET-004", "the focused Settings switch toggles"),
    (
        "S-ARR-020",
        "the Arrangement View: with title bars selected, Left folds and Right unfolds them \
         (`arrangement.fold_tracks`); U is S-TRACK-010's binding",
    ),
    (
        "S-ARR-022",
        "the Arrangement View: Option-Plus (Alt-Plus) makes the selected tracks taller and \
         Option-Minus (Alt-Minus) shorter (`arrangement.set_track_height`)",
    ),
    (
        "S-A11Y-003",
        "the Arrangement View: Up and Down select the track above or below \
         (`arrangement.select_tracks`)",
    ),
    // The browser reads its arrow keys itself for the same reason (WP-1.10).
    (
        "S-BRW-001",
        "the browser's list: Up and Down select the item above or below (`browser.select`), \
         previewing it while Preview is on",
    ),
    (
        "S-BRW-003",
        "the browser's list: Right opens the selected folder and Left closes it \
         (`browser.open_folder`); Left inside a folder selects the folder",
    ),
    (
        "S-BRW-005",
        "the browser's list: Shift-Enter, and Right on a file, preview the selected file \
         (`browser.preview`)",
    ),
    (
        "S-BRW-007",
        "the browser's search field: Down and Enter move to the first result \
         (`focus.browser`, `browser.select`)",
    ),
    (
        "S-BRW-015",
        "the browser's Quick Tags: Cmd-E (Ctrl-E) moves the keyboard focus to the Add field",
    ),
    (
        "S-A11Y-015",
        "the browser's list: Shift-Up and Shift-Down extend the selection (`browser.select`)",
    ),
    (
        "S-A11Y-016",
        "the browser's sidebar: Up and Down choose the label above or below \
         (`browser.open_place`)",
    ),
    (
        "S-A11Y-017",
        "the browser: Right moves from the sidebar to the list, and Left at the list's top \
         level back (`focus.browser`)",
    ),
    // WP-2.2: while the computer MIDI keyboard is on, its letters play notes
    // and step its octave and velocity, with Shift (or Cmd, Ctrl, Option)
    // leaving them their shortcuts. The key dispatcher reads them itself
    // because a note goes down and up with its key; each runs a registered
    // command.
    (
        "S-MAP-004",
        "while the computer MIDI keyboard is on, Z and X move its octave \
         (`midi.computer_keyboard_octave`)",
    ),
    (
        "S-MAP-005",
        "while the computer MIDI keyboard is on, C and V step its velocity \
         (`midi.computer_keyboard_velocity`)",
    ),
    (
        "S-MAP-006",
        "while the computer MIDI keyboard is on, its note keys play and release notes \
         (`midi.computer_keyboard_key`)",
    ),
    (
        "S-MAP-007",
        "while the computer MIDI keyboard is on, a letter with Shift runs its shortcut as it \
         would with the keyboard off",
    ),
    // WP-1.8b: keys with two directions, and modifiers a drag reads as it
    // goes, each running a registered command.
    (
        "S-ARR-028",
        "the Arrangement View: Left and Right nudge the selected clips a grid step, or move \
         the insert marker with none selected (`arrangement.nudge`)",
    ),
    (
        "S-A11Y-001",
        "the Arrangement View: Option-Left and Option-Right (Alt) move the insert marker to \
         the previous or next clip edge or locator (`arrangement.set_insert_marker`)",
    ),
    (
        "S-A11Y-002",
        "the Arrangement View: Shift with Left or Right moves the time selection's end a grid \
         step, with Up or Down takes in the track above or below \
         (`arrangement.select_time`)",
    ),
    (
        "S-EDIT-013",
        "the Arrangement View: Cmd (Alt) held while dragging clips or their edges ignores \
         snapping, or snaps with it off (`arrangement.move_clips`, \
         `arrangement.resize_clips`)",
    ),
    (
        "S-GRID-007",
        "the Arrangement View: as S-EDIT-013, Cmd (Alt) held while dragging bypasses snapping",
    ),
    (
        "S-ARR-007",
        "the Arrangement View: Shift-Option-dragging (Shift-Alt) a clip's title bar selects \
         time inside the clip (`arrangement.select_time`)",
    ),
    // The value fields (WP-1.18): the Control Bar's transport section today,
    // each change running the field's registered command
    // (the control bar transport spec).
    (
        "S-VAL-001",
        "the focused value field steps its value (the Control Bar's position, loop, tempo, \
         and time-signature fields run `transport.locate`, `transport.set_loop`, \
         `song.set_tempo`, `song.set_time_signatures`; the mixers' and the track headers' \
         volume and pan run `mixer.set_volume` and `mixer.set_pan`)",
    ),
    (
        "S-VAL-002",
        "the focused value field steps its value in fine steps",
    ),
    (
        "S-VAL-003",
        "the value field being dragged steps in fine steps",
    ),
    (
        "S-VAL-004",
        "the focused value field returns to its default (a mixer's volume to 0 dB, its pan to \
         the centre)",
    ),
    ("S-VAL-005", "the focused value field takes a typed value"),
    (
        "S-DEV-006",
        "the focused device control returns to its parameter's default",
    ),
    (
        "S-VAL-006",
        "the focused bar-beat-sixteenth field moves on to its next field",
    ),
    ("S-VAL-007", "the focused value field drops a typed value"),
    ("S-VAL-008", "the focused value field sets a typed value"),
    // WP-1.14: the context menus from the keyboard, each item running what
    // a right-click's does.
    (
        "S-CTX-002",
        "the focused view opens its selection's context menu: the Arrangement's clip or \
         time-selection menu (its grid choices run `arrangement.set_grid`), or the chosen \
         place's in the browser's sidebar (`browser.remove_folder`)",
    ),
    // WP-2.3: the Clip View's note selection reads the transpose keys itself,
    // each running `note.move` (part 3).
    (
        "S-A11Y-008",
        "the Clip View's note editor: Up and Down transpose the selected notes by a semitone \
         (`note.move`)",
    ),
    (
        "S-A11Y-009",
        "the Clip View's note editor: Shift-Up and Shift-Down transpose the selected notes by \
         an octave (`note.move`)",
    ),
    // WP-6.53: the note editor reads the keys that move its insert marker,
    // select time and select notes itself, because each of them does
    // something different depending on what is selected.
    (
        "S-NOTE-002",
        "the Clip View's note editor: Option-dragging a note copies it (`note.move` with `copy`)",
    ),
    (
        "S-NOTE-017",
        "the Clip View's note editor: Home moves the insert marker to the clip's start \
         (`clip_view.set_insert_marker`)",
    ),
    (
        "S-NOTE-018",
        "the Clip View's note editor: End moves the insert marker to the clip's end \
         (`clip_view.set_insert_marker`)",
    ),
    (
        "S-NOTE-019",
        "the Clip View's note editor: Option-Down and Option-Up select the note nearest the \
         insert marker, and the previous or next note after that (`clip_view.select_notes`)",
    ),
    (
        "S-NOTE-020",
        "the Clip View's note editor: Option-Left and Option-Right select the previous or next \
         note of the same pitch (`clip_view.select_notes`)",
    ),
    (
        "S-NOTE-042",
        "the Clip View's note editor: Esc deselects the notes and clears the time selection \
         (`clip_view.select_notes`, `clip_view.select_time`)",
    ),
    (
        "S-NOTE-043",
        "the Clip View's note editor: Option-Shift-Down and Option-Shift-Up add the previous \
         or next note to the selection (`clip_view.select_notes`)",
    ),
    (
        "S-NOTE-045",
        "the Clip View's note editor: Left and Right move the insert marker a grid step, or \
         the selected notes (`clip_view.set_insert_marker`, `note.move`)",
    ),
    (
        "S-NOTE-046",
        "the Clip View's note editor: Option-Shift-Left and Option-Shift-Right move the time \
         selection's end to the next or previous note edge (`clip_view.select_time`)",
    ),
    (
        "S-A11Y-004",
        "the Clip View's note editor: Shift-Left and Shift-Right select time, a grid step at a \
         time (`clip_view.select_time`)",
    ),
    (
        "S-A11Y-005",
        "the Clip View's note editor: Enter selects the notes that start inside the time \
         selection (`clip_view.select_notes`)",
    ),
    (
        "S-A11Y-006",
        "the Clip View's note editor: Option-Left and Option-Right (Ctrl on Windows) move the \
         insert marker to the previous or next note edge (`clip_view.set_insert_marker`)",
    ),
    (
        "S-A11Y-007",
        "the Clip View's note editor: Shift with the note-jump keys adds the note to the \
         selection (`clip_view.select_notes`)",
    ),
    (
        "S-A11Y-010",
        "the Clip View's note editor: Shift-Left and Shift-Right lengthen or shorten the \
         selected notes by the grid (`note.resize`)",
    ),
    (
        "S-A11Y-011",
        "the Clip View's note editor: Left and Right move the selected notes by the grid \
         (`note.move`)",
    ),
    // WP-4.10: the Sample Editor reads its arrow keys itself, because each
    // shortcut has two keys that act differently and the keymap gives a
    // shortcut one command with one set of arguments; each runs a
    // registered command.
    (
        "S-SAMPLE-003",
        "the Clip View's Sample Editor: Left and Right move the selected warp markers by one \
         step of the editing grid (`clip.move_warp_markers`)",
    ),
    (
        "S-SAMPLE-004",
        "the Clip View's Sample Editor: Cmd-Left and Cmd-Right select the previous or next \
         warp marker (`clip_view.select_warp_markers`)",
    ),
    (
        "S-SAMPLE-006",
        "the Clip View's Sample Editor: with the Start marker selected, Shift-Left and \
         Shift-Right move the clip's region by one step of the editing grid \
         (`clip.set_region`)",
    ),
    (
        "S-A11Y-012",
        "the Clip View's Sample Editor: Option-Left and Option-Right (Ctrl on Windows) move \
         the insert marker by one step of the editing grid (`clip_view.set_insert_marker`)",
    ),
    (
        "S-A11Y-013",
        "the Clip View's Sample Editor: with nothing selected, Shift-Left and Shift-Right \
         start or extend a time selection by one step of the editing grid \
         (`clip_view.select_time`)",
    ),
    // WP-3.7: the Session View grid reads its arrows, Page keys, Home and
    // End, and Esc itself, because each shortcut has keys that act
    // differently and the keymap gives a shortcut one command with one set of
    // arguments.
    (
        "S-SES-002",
        "the Session View: the arrows select the neighbouring slot, across the tracks and \
         down the scenes (`session.select`)",
    ),
    (
        "S-SES-010",
        "the Session View: Up and Down select the scene above or below (`session.select`)",
    ),
    (
        "S-SES-011",
        "the Session View: Page Up and Page Down move eight scenes (`session.select`)",
    ),
    (
        "S-SES-019",
        "the Session View: Esc goes to the highlighted track's title bar (`session.select`)",
    ),
];

/// The package that binds each M1 shortcut the shell doesn't: exact IDs
/// first, then ID prefixes. The keymap test fails if an M1 shortcut has
/// no command and no package here, or if this names WP-1.7 itself.
const OWNERS: &[(&str, &str)] = &[
    ("S-VIEW-011", "WP-1.8a"),
    ("S-EDIT-012", "WP-1.8b"),
    ("S-ENV-004", "WP-6.51"),
    ("S-ARR-008", "WP-6.51"),
    ("S-ARR-009", "WP-6.51"),
    ("S-ARR-010", "WP-6.51"),
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
    ("S-TRACK-005", "WP-1.8b"),
    ("S-TRACK-008", "WP-1.8a"),
    ("S-TRACK-009", "WP-1.8a"),
    ("S-TRACK-010", "WP-1.8a"),
    ("S-A11Y-003", "WP-1.8a"),
    // Playing a time selection needs WP-1.8b's selections (transport.md).
    ("S-TRANS-003", "WP-1.8b"),
    ("S-TRANS-004", "WP-1.8b"),
    // Prefixes.
    ("S-EDIT-", "WP-1.8b"),
    ("S-LOOP-", "WP-1.8b"),
    ("S-ZOOM-", "WP-1.8b"),
    ("S-GRID-", "WP-1.8a"),
    ("S-ARR-", "WP-1.8b"),
    ("S-TRACK-", "WP-1.8b"),
    ("S-TRANS-", "WP-1.5"),
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
            "{PATH} is out of date with the shortcut reference and the command registry; \
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
         # Generated by `cargo xtask keymap` from the shortcut reference and the\n\
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
        assert_eq!(owner("S-TRANS-001"), Some("WP-1.5"));
        assert_eq!(owner("S-NOPE-001"), None);
        assert!(OWNERS.iter().all(|(_, package)| *package != "WP-1.7"));
    }

    #[test]
    fn the_repository_keymap_is_in_sync() {
        // Spec: export R16
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("xtask sits in the workspace root");
        if !ui_parity::reference_present(root) {
            eprintln!("{}", ui_parity::ABSENT);
            return;
        }
        check(root).expect("crates/shell-spec/keymaps/live12.toml is in sync");
    }
}

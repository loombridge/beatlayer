//! The walk of the whole command registry: every command has a key, a menu
//! item or a path listed here, with its reason (Spec: accessibility R8).

mod paths;
pub(crate) mod setups;

use crate::accessibility::helpers::*;
use crate::accessibility::settings::{open_settings, tab_to};
use crate::common::{Harness, Window};
use Reason::{Equivalent, LaterView, Live, Unobserved};
use bl_shell::menus::{Action, MenuPlatform, menu_bar};
use bl_ui::keymap::{Keymap, Status};
use bl_ui::main_window::UiEvent;
use egui::{Key, Modifiers};
use std::collections::BTreeMap;

/// How a command is reached without a pointer, when it has no key and no
/// menu item.
pub(crate) enum Path {
    /// A control the keyboard reaches: the test opens `window`, runs
    /// `setup`, moves the focus where a person starts (`start`), presses
    /// Tab until the control is focused, then `key`, and checks the command
    /// ran.
    Control {
        window: Window,
        page: &'static str,
        setup: fn(&mut Harness),
        start: fn(&mut Harness),
        label: &'static str,
        key: Key,
    },
    /// No keyboard path of its own, with the reason (the accessibility
    /// spec, R8, lists each).
    Exception(Reason),
}

/// Why a command has no keyboard path of its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Reason {
    /// A query for agents: a person reads the same on screen.
    Query,
    /// For agents only, with no person's form and no Live feature behind
    /// it, named in the text.
    AgentOnly(&'static str),
    /// An agent's form of a pointer gesture or dialog, whose keyboard path
    /// is another command or the system's dialog, named in the text.
    Equivalent(&'static str),
    /// Its view comes in a later milestone; until then agents reach it.
    LaterView(&'static str),
    /// Live itself has no keyboard path for it.
    Live(&'static str),
    /// Live's own keyboard path for it isn't observed yet, named in the
    /// text: its place in the mixers' Tab order and its key, which the next
    /// observation session records.
    Unobserved(&'static str),
    /// A gap in M1: what's missing, and what closes it.
    Gap(&'static str),
}

pub(crate) fn nothing(_: &mut Harness) {}

pub(crate) const CLIP_VIEW: Reason = LaterView("the Clip View (milestone M2)");

pub(crate) const TUNING_SECTION: Reason =
    LaterView("the browser's Tunings label and the Tuning section (WP-6.10)");

pub(crate) const GROOVE_POOL: Reason = LaterView(
    "the Groove Pool, the Clip View's groove controls and the browser's Grooves label (WP-6.20)",
);

// The Clip View's note editing (WP-2.3): the pointer reaches each of
// these — a click, a marquee, and drags in the note editor — and their
// keys (shortcuts.md, `note-editor` and `note-selection`) come with a
// later package.
pub(crate) const NOTE_EDITOR: Reason =
    LaterView("the note editor's keys (the note-editor shortcuts, milestone M2)");

// The Clip View's MPE tab (WP-6.18): the pointer edits a note's breakpoints,
// and the keyboard reaches the toggles and the triangle and reads each
// envelope as a list (the MPE spec, R34); moving breakpoints by key waits
// for the envelope keys' extension to this tab.
pub(crate) const MPE_BREAKPOINTS: Reason = LaterView(
    "moving, adding and deleting a note's expression breakpoints from the keyboard in the MPE tab",
);

pub(crate) const NOTE_FOOTER: Reason =
    LaterView("the note editor's footer and Lane Selector in the Tab order (WP-6.1's follow-up)");

pub(crate) const CLIP_LAUNCH: Reason = LaterView("the Clip View's Launch panel (WP-3.9)");

pub(crate) const AUDIO_PANEL: Reason = LaterView("the Audio Utilities panel's controls (WP-4.10b)");

pub(crate) const CLIP_TITLE_BAR: Reason =
    LaterView("the audio clip's title bar Save button (WP-4.10b)");

pub(crate) const AUTO_WARP: Reason = LaterView(
    "the Sample Editor's context menu's Set 1.1.1 Here and Warp items, drawn disabled \
         until they run auto-warp's commands",
);

// The Envelopes tab (WP-3.16): the pointer draws, moves, curves and
// deletes breakpoints, and the keys for the breakpoints come with a
// later package; its choosers and Linked switch are reached by Tab.
pub(crate) const CLIP_ENVELOPES: Reason = Equivalent(
    "the Envelope Editor's keys (automation-editing R22 to R26, clip-envelopes R14): Enter \
         makes and selects a breakpoint (S-A11Y-019), Left, Right, Up, Down and a typed number \
         move it and change its value (S-A11Y-023, S-A11Y-030), Delete deletes it (S-A11Y-025) \
         and Cmd-Delete clears the envelope (S-ENV-005)",
);

// The Envelope Editor's selections, edit commands, shapes, stretch and skew
// (WP-6.25c): the Edit keys act on its time selection, and Simplify Envelope
// is in the Edit menu and the selection's menu.
pub(crate) const CLIP_ENVELOPE_EDIT: Reason = Equivalent(
    "the Envelope Editor: Cut, Copy, Paste and Duplicate (S-EDIT-001 to S-EDIT-004) act on \
         its time selection, Delete (S-A11Y-025) deletes it while no breakpoint is selected, \
         and the arrows move a selected breakpoint (clip-envelopes R12, R14)",
);

pub(crate) const CLIP_ENVELOPE_SHAPES: Reason = Equivalent(
    "the buttons of the time selection's context menu in the Envelope Editor, and Edit \
         menu's Simplify Envelope (M-EDIT-039), which runs while the editor has the \
         focus (clip-envelopes R13)",
);

pub(crate) const CLIP_HANDLES: Reason = Live(
    "the handles of a time selection in the Envelope Editor are dragged; the manual gives \
         them no keys (manual 25.5.3 and 27.5.4; clip-envelopes R13)",
);

pub(crate) const CLIP_SELECTING: Reason = Unobserved(
    "selecting time or a segment in the Envelope Editor from the keyboard: Live's manual \
         (41.5) names no key for it, and whether Live has one isn't observed",
);

pub(crate) const CURVING: Reason = Unobserved(
    "curving a segment from the keyboard: Live's manual (41.5) names no key for it, and \
         whether Live has one isn't observed",
);

pub(crate) const DEVICE_PARAMS: Reason = Equivalent(
    "a described device's parameter controls in the Device View: Tab to one, then Up and \
         Down step it and Delete sets its default (the device framework spec, R17)",
);

pub(crate) const DEVICE_MENU: Reason = Equivalent(
    "the device's context menu, from its Show Options button (Enter, then the arrows), as \
         in Live",
);

pub(crate) const TRACK_MENU: Reason = Equivalent(
    "the track's context menu, opened from its title bar with the context menu key \
         (S-CTX-002), then the arrows and Enter, as in Live",
);

pub(crate) const METRONOME_MENU: Reason = Equivalent(
    "the Metronome settings menu, from its pop-up arrow or context menu, as in Live \
         (manual 19.4.1)",
);

pub(crate) const RECORDING: Reason =
    LaterView("the Record button and transport recording (milestone M2, S-TRANS-006)");

pub(crate) const EDITING: Reason = Equivalent(
    "the Arrangement's editing keys run it: paste, Delete, Left and Right, and Enter then \
         Left or Right (arrangement-editing R12)",
);

// The Arrangement's automation lanes (WP-6.25): the edit keys with
// Option (Alt on Windows) apply to the envelopes (S-EDIT-016), and the
// lane's own gestures reach the rest.
pub(crate) const ENVELOPE_EDIT: Reason = Equivalent(
    "the Arrangement's automation lane: the edit keys with Option (Alt on Windows) apply to \
         the envelopes (S-EDIT-016), and the lane's gestures move and select their breakpoints \
         (automation-editing R2 to R4)",
);

// The shapes are buttons in the time selection's context menu, which
// the keyboard opens (S-CTX-002); the handles that stretch and skew a
// selection are dragged, and the manual gives them no keys.
pub(crate) const ENVELOPE_SHAPES: Reason = Equivalent(
    "the buttons of the time selection's context menu in an automation lane, which the \
         context-menu keys open (S-CTX-002; automation-editing R11)",
);

pub(crate) const HANDLES: Reason = Live(
    "the handles of a time selection in an automation lane are dragged; the manual gives \
         them no keys (manual 25.5.3; automation-editing R14 to R17)",
);

// The automation lane that has the keyboard (WP-6.26): Enter makes and
// selects a breakpoint at the insert marker, and the arrows, a typed
// number, Delete and Cmd-Delete edit it.
pub(crate) const LANE_KEYS: Reason = Equivalent(
    "the automation lane's keys (automation-editing R22 to R26): Enter makes and selects a \
         breakpoint (S-A11Y-019), Left, Right, Up, Down and a typed number move it and change \
         its value (S-A11Y-023, S-A11Y-030), Delete deletes it (S-A11Y-025) and Cmd-Delete \
         deletes the shown parameter's automation (S-ENV-005)",
);

// Selecting a breakpoint is view state: Enter and Option-click do it.
pub(crate) const SELECTING: Reason = Equivalent(
    "Enter at the insert marker selects the breakpoint there (S-A11Y-019), Tab and \
         Shift-Tab the next or previous (S-A11Y-022), and Option-click (Alt-click) selects \
         one with the pointer (S-A11Y-021)",
);

// The browser's tags (browser-tags R10 to R16): the Tag Editor and
// Quick Tags reach them.
pub(crate) const TAG_EDITOR: Reason = Equivalent(
    "the browser's Tag Editor: its check boxes, its Add Tag\u{2026} and Add Group\u{2026} \
         rows, its context menu's Rename and Delete items, its list's context menu's Reset \
         Order item, and dragging a tag or a group (the browser tags spec, R10 to R12)",
);

pub(crate) const QUICK_TAGS: Reason = Equivalent(
    "the browser's Quick Tags: its Add\u{2026} field, its X, and its Create New Tag\u{2026} \
         item (the browser tags spec, R13 and R14)",
);

// The browser's Filter View, saved labels, sidebar and columns
// (browser-tags R18 to R28): the Tab order reaches the Filter View's tag
// buttons, the Filter View Menu, the Results bar's buttons, the
// sidebar and the header row's Content Options button.
pub(crate) const FILTERS: Reason = Equivalent(
    "the browser's Filter View: its tag buttons (the arrow keys, Enter and Cmd-Enter), the \
         Filter View Menu's group items, the Results bar's Clear and Add Label buttons, Esc and \
         Cmd-F in the search field (the browser tags spec, R20 to R23)",
);

pub(crate) const LABELS: Reason = Equivalent(
    "the browser's sidebar: the Library section's Edit mode and its check boxes, a label's \
         context menu (Rename, also Cmd-R, Choose Icon, Remove), the Library heading's Reset \
         Label Order item, and dragging a label (the browser tags spec, R24 to R26)",
);

pub(crate) const COLUMNS: Reason = Equivalent(
    "the content pane's header row: the Content Options menu, a column's name sorting by it, \
         and dragging a column's name (the browser tags spec, R27 and R28)",
);

/// Every registered command, from every crate that registers them.
fn registry() -> bl_commands::Registry {
    let mut registry = bl_commands::Registry::new();
    bl_model::registry::register(&mut registry).unwrap();
    bl_project::registry::register(&mut registry).unwrap();
    bl_engine::transport::register(&mut registry).unwrap();
    bl_engine::session::register(&mut registry).unwrap();
    bl_shell::register(&mut registry).unwrap();
    registry
}

/// The commands a key runs (a bound key or key sequence), on either
/// platform.
fn keyed(keymap: &Keymap) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for entry in keymap.entries() {
        if !matches!(entry.kind.as_str(), "key" | "sequence") {
            continue;
        }
        match &entry.status {
            Status::Bound => {
                for command in &entry.commands {
                    out.entry(command.id.clone())
                        .or_insert_with(|| entry.id.clone());
                }
            }
            // The focused control's keys name the commands they run.
            Status::Widget(handler) => {
                for id in handler.split('`').skip(1).step_by(2) {
                    out.entry(id.to_owned()).or_insert_with(|| entry.id.clone());
                }
            }
            _ => {}
        }
    }
    out
}

/// The commands a menu item runs, on either platform: directly, or through
/// a bound shortcut.
fn on_menus(keymap: &Keymap) -> BTreeMap<String, String> {
    let every_context = bl_commands::Context::ALL;
    let mut out = BTreeMap::new();
    for platform in [MenuPlatform::Mac, MenuPlatform::Other] {
        for menu in menu_bar(platform) {
            for item in menu.walk().filter(|item| item.is_shown()) {
                let ids: Vec<String> = match item.action {
                    Action::Command(id) | Action::Choice(id, _) => vec![id.to_owned()],
                    Action::Shortcut(shortcuts) => shortcuts
                        .iter()
                        .filter_map(|s| keymap.command_for(s, &every_context))
                        .map(|c| c.id.clone())
                        .collect(),
                    _ => Vec::new(),
                };
                for id in ids {
                    out.entry(id).or_insert_with(|| item.row.to_owned());
                }
            }
        }
    }
    out
}

/// Opens the window a control is in, gets there with the keyboard, and
/// presses its key: true if the command ran.
fn reach(command_id: &str, path: &Path) -> Result<(), String> {
    let Path::Control {
        window,
        page,
        setup,
        start,
        label,
        key,
    } = path
    else {
        return Ok(());
    };
    let mut h = match window {
        Window::Main => {
            let mut h = Harness::with_title_bar(Window::Main);
            h.settle();
            h
        }
        Window::Settings => open_settings(page),
        Window::Device(_) | Window::Assistant | Window::Second => {
            unreachable!("no keyboard path starts in a floating window")
        }
    };
    setup(&mut h);
    if *window == Window::Main {
        tab_navigation_on(&mut h);
    }
    h.settings.refresh();
    h.settle();
    start(&mut h);
    if !tab_to(&mut h, label) {
        return Err(format!("{command_id}: Tab never reached {label:?}"));
    }
    h.take_events();
    if *key == Key::K {
        // Typing into a text field.
        h.frame(vec![egui::Event::Text("k".to_owned())]);
    } else {
        h.key(*key, Modifiers::NONE);
    }
    h.settle();
    let ran = h
        .take_events()
        .iter()
        .any(|event| matches!(event, UiEvent::Invoke(i) if i.id == command_id));
    if ran {
        Ok(())
    } else {
        Err(format!("{command_id}: {label:?} didn't run it"))
    }
}

// Spec: accessibility R8.
#[test]
fn every_m1_command_is_reachable_from_the_keyboard() {
    let registry = registry();
    let keymap = Keymap::live12();
    let keyed = keyed(&keymap);
    let menus = on_menus(&keymap);
    let paths = paths();
    let mut problems = Vec::new();
    let mut table = Vec::new();
    for spec in registry.iter() {
        let id = spec.id.as_ref();
        let key = keyed.get(id);
        let menu = menus.get(id);
        match (key, menu, paths.get(id)) {
            (Some(shortcut), _, Some(_)) | (None, Some(shortcut), Some(Path::Exception(_))) => {
                problems.push(format!(
                    "{id} is listed with its own path, but {shortcut} reaches it: take it out of \
                     the list"
                ));
            }
            (Some(shortcut), _, None) => table.push((id, format!("key, {shortcut}"))),
            (None, Some(row), None) => table.push((id, format!("menu, {row}"))),
            (None, menu, Some(path @ Path::Control { label, .. })) => match reach(id, path) {
                Ok(()) => table.push((
                    id,
                    format!(
                        "control, {label}{}",
                        menu.map(|row| format!("; menu, {row}")).unwrap_or_default()
                    ),
                )),
                Err(problem) => problems.push(problem),
            },
            (None, None, Some(Path::Exception(reason))) => {
                table.push((id, format!("{reason:?}")));
            }
            (None, None, None) => problems.push(format!(
                "{id} has no key, no menu item, and no keyboard-reachable control: give it one, \
                 or list it in `paths` with the reason (the accessibility spec, R8)"
            )),
        }
    }
    for id in paths.keys() {
        if registry.get(id).is_none() {
            problems.push(format!("{id} is listed but isn't a registered command"));
        }
    }
    for (id, how) in &table {
        println!("| `{id}` | {how} |");
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
    let gaps: Vec<_> = paths
        .iter()
        .filter(|(_, p)| matches!(p, Path::Exception(Reason::Gap(_))))
        .map(|(id, _)| *id)
        .collect();
    assert_eq!(
        gaps,
        [
            "chain.save_preset",
            "clip.save_file",
            "device.add_layers",
            "device.add_slice",
            "device.audition_sample",
            "device.autosample_open",
            "device.convert",
            "device.crop_sample",
            "device.delete_layers",
            "device.delete_slice",
            "device.distribute_layers",
            "device.duplicate_layers",
            "device.edit_sample",
            "device.load_sample",
            "device.move_layers",
            "device.move_slice",
            "device.normalize_layers",
            "device.rename_layer",
            "device.reverse_sample",
            "device.set_layer_zone",
            "device.snap_layer_markers",
            "device.sort_layers",
            "set.export_clips",
            "set.export_tracks",
            "track.move",
        ],
        "the gaps the accessibility spec lists"
    );
}

// Spec: accessibility R8.
#[test]
fn the_reachability_test_fails_for_a_command_with_no_keyboard_path() {
    let mut registry = registry();
    registry
        .register(
            bl_commands::CommandSpec::new("test.unreachable", "Unreachable")
                .description("A command nothing reaches, to show the test fails.")
                .permission(bl_commands::Permission::Read)
                .context(bl_commands::Context::Global)
                .args::<bl_commands::NoArgs>()
                .result::<()>(),
        )
        .unwrap();
    let keymap = Keymap::live12();
    let keyed = keyed(&keymap);
    let menus = on_menus(&keymap);
    let paths = paths();
    let unreachable: Vec<&str> = registry
        .iter()
        .map(|spec| spec.id.as_ref())
        .filter(|id| {
            !keyed.contains_key(*id) && !menus.contains_key(*id) && !paths.contains_key(id)
        })
        .collect();
    assert_eq!(unreachable, ["test.unreachable"]);
}

/// A control in the main window that the keyboard reaches.
pub(crate) fn main(
    setup: fn(&mut Harness),
    start: fn(&mut Harness),
    label: &'static str,
    key: Key,
) -> Path {
    Path::Control {
        window: Window::Main,
        page: "",
        setup,
        start,
        label,
        key,
    }
}

/// A control on a Settings page that the keyboard reaches.
pub(crate) fn settings(
    page: &'static str,
    setup: fn(&mut Harness),
    label: &'static str,
    key: Key,
) -> Path {
    Path::Control {
        window: Window::Settings,
        page,
        setup,
        start: nothing,
        label,
        key,
    }
}

/// Every command without a key or a menu item, and how it's reached.
fn paths() -> BTreeMap<&'static str, Path> {
    [
        paths::arrangement::entries(),
        paths::browser::entries(),
        paths::clips::entries(),
        paths::control_bar::entries(),
        paths::devices::entries(),
        paths::files::entries(),
        paths::mixer::entries(),
        paths::session::entries(),
        paths::settings::entries(),
    ]
    .into_iter()
    .flatten()
    .collect()
}

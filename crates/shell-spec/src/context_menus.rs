//! The context menus, as data: the menu a clip opens and the menu a time
//! selection opens in the Arrangement (WP-1.8b), the menu a device's
//! title bar and Show Options button open (WP-2.4), and the menu a track's
//! title bar opens (WP-4.22), in Live's order from
//! the menu reference (C-CLIP-ARR, C-TIME, C-DEVICE, C-TRACK), each item tied to its
//! shortcut as the menu bar's are ([`Action::Shortcut`]), so it runs the
//! command the keymap gives that shortcut in the Arrangement, or shows
//! disabled until a package binds it.
//!
//! The grid's Adaptive Grid and Fixed Grid rows are rows of choices rather
//! than single items: they carry `arrangement.set_grid`, and the UI lays
//! out a choice for each setting.

use crate::arrangement::SET_GRID;
use crate::audio_editing::CONSOLIDATE_TO_SCENE;
use crate::devices::{
    COMPARE_COPY, COMPARE_SWITCH, SAVE_DEFAULT_PRESET, SET_FLOATING, SET_ROLE, UNGROUP,
};
use crate::menus::{Action, IMP_12, MenuItem, Row, SEPARATOR, items};
use crate::notes::{CHOP as CHOP_NOTES, SPLIT as SPLIT_NOTES};
use crate::slice::OPEN as SLICE_TO_MIDI;
use crate::stems::OPEN as SEPARATE_STEMS;
use crate::tracks::{DUPLICATE_WITHOUT_CLIPS, SET_DEVICES_ENABLED};

use Action::{Choice, Command as Cmd, Pending, Shortcut as Key, Submenu};

/// The model's command that colours a chain.
const SET_CHAIN_COLOUR: &str = "chain.set_colour";

/// The rows that hold the zoom-adaptive grid's settings.
pub const ADAPTIVE_GRID_ROWS: [&str; 2] = ["C-CLIP-ARR-020", "C-TIME-013"];

/// The rows that hold the fixed grid's spacings.
pub const FIXED_GRID_ROWS: [&str; 2] = ["C-CLIP-ARR-021", "C-TIME-014"];

const CLIP: &[Row] = &[
    ("C-CLIP-ARR-001", "Cut", Key(&["S-EDIT-001"])),
    ("C-CLIP-ARR-002", "Copy", Key(&["S-EDIT-002"])),
    ("C-CLIP-ARR-003", "Duplicate", Key(&["S-EDIT-004"])),
    ("C-CLIP-ARR-004", "Delete", Key(&["S-EDIT-005"])),
    SEPARATOR,
    (
        "C-CLIP-ARR-005",
        "Zoom to Time Selection",
        Key(&["S-ARR-030"]),
    ),
    (
        "C-CLIP-ARR-006",
        "Zoom Back from Time Selection",
        Key(&["S-ARR-031"]),
    ),
    SEPARATOR,
    ("C-CLIP-ARR-007", "Rename", Key(&["S-EDIT-008"])),
    ("C-CLIP-ARR-008", "Edit Info Text", Pending(None)),
    ("C-CLIP-ARR-009", "Extract Groove(s)", Pending(None)),
    ("C-CLIP-ARR-010", "Deactivate Clip", Key(&["S-ARR-027"])),
    ("C-CLIP-ARR-011", "Export MIDI Clip...", Pending(None)),
    SEPARATOR,
    ("C-CLIP-ARR-012", "Loop Selection", Key(&["S-ARR-011"])),
    SEPARATOR,
    ("C-CLIP-ARR-013", "Freeze Track", Key(&["S-TRACK-017"])),
    (
        "C-CLIP-ARR-014",
        "Bounce Track in Place",
        Cmd("track.bounce_in_place"),
    ),
    (
        "C-CLIP-ARR-015",
        "Bounce to New Track",
        Key(&["S-BOUNCE-001"]),
    ),
    (
        "C-CLIP-ARR-016",
        "Paste Bounced Audio",
        Key(&["S-BOUNCE-002"]),
    ),
    SEPARATOR,
    ("C-CLIP-ARR-017", "Crop Clip", Key(&["S-ARR-003"])),
    ("C-CLIP-ARR-018", "Reverse Clip(s)", Key(&["S-ARR-029"])),
    SEPARATOR,
    ("C-CLIP-ARR-019", "Snap to Grid", Key(&["S-GRID-005"])),
    ("C-CLIP-ARR-020", "Adaptive Grid", Cmd(SET_GRID)),
    ("C-CLIP-ARR-021", "Fixed Grid", Cmd(SET_GRID)),
    SEPARATOR,
    ("C-CLIP-ARR-022", "Narrow Grid", Key(&["S-GRID-002"])),
    ("C-CLIP-ARR-023", "Widen Grid", Key(&["S-GRID-003"])),
    SEPARATOR,
    ("C-CLIP-ARR-024", "Triplet Grid", Key(&["S-GRID-004"])),
];

const TIME: &[Row] = &[
    ("C-TIME-001", "Cut", Key(&["S-EDIT-001"])),
    ("C-TIME-002", "Copy", Key(&["S-EDIT-002"])),
    ("C-TIME-003", "Duplicate", Key(&["S-EDIT-004"])),
    SEPARATOR,
    ("C-TIME-004", "Zoom to Time Selection", Key(&["S-ARR-030"])),
    (
        "C-TIME-005",
        "Zoom Back from Time Selection",
        Key(&["S-ARR-031"]),
    ),
    SEPARATOR,
    ("C-TIME-006", "Insert Empty MIDI Clip", Key(&["S-ARR-037"])),
    SEPARATOR,
    ("C-TIME-007", "Loop Selection", Key(&["S-ARR-011"])),
    SEPARATOR,
    ("C-TIME-008", "Freeze Track", Key(&["S-TRACK-017"])),
    (
        "C-TIME-009",
        "Bounce Track in Place",
        Cmd("track.bounce_in_place"),
    ),
    ("C-TIME-010", "Bounce to New Track", Key(&["S-BOUNCE-001"])),
    ("C-TIME-011", "Paste Bounced Audio", Key(&["S-BOUNCE-002"])),
    SEPARATOR,
    ("C-TIME-012", "Snap to Grid", Key(&["S-GRID-005"])),
    ("C-TIME-013", "Adaptive Grid", Cmd(SET_GRID)),
    ("C-TIME-014", "Fixed Grid", Cmd(SET_GRID)),
    SEPARATOR,
    ("C-TIME-015", "Narrow Grid", Key(&["S-GRID-002"])),
    ("C-TIME-016", "Widen Grid", Key(&["S-GRID-003"])),
    SEPARATOR,
    ("C-TIME-017", "Triplet Grid", Key(&["S-GRID-004"])),
];

/// The scrub area's menu (C-TIME-022).
const SCRUB: &[Row] = &[
    ("C-TIME-022a", "Add Locator", Cmd(crate::locator::ADD)),
    ("C-TIME-022b", "Insert Time Signature Change", Pending(None)),
];

/// A locator's menu (C-TIME-023 and C-TIME-025).
const LOCATOR: &[Row] = &[
    (
        "C-TIME-023a",
        "Loop to Next Locator",
        Cmd(crate::locator::LOOP_TO_NEXT),
    ),
    (
        "C-TIME-023b",
        "Set Song Start Time Here",
        Cmd(crate::locator::SET_START_TIME),
    ),
    SEPARATOR,
    ("C-TIME-025a", "Edit Info Text", Pending(None)),
];

/// The loop brace's menu (C-TIME-023).
const BRACE: &[Row] = &[(
    "C-TIME-023c",
    "Set Song Start Time Here",
    Choice(crate::locator::SET_START_TIME, "{\"loop_brace\":true}"),
)];

/// The row of Beatlayer's own Float (IMP-14, WP-2.5b), after Fold: an
/// improvement, so not a row of menus.md, which records Live's menus. It's
/// ticked while the selected device floats, and choosing it again docks it.
pub const IMP_14: &str = "IMP-14";

/// The row of Beatlayer's own Use as MIDI Effect (IMP-24), after Float: an
/// improvement, so not a row of menus.md. It places a plug-in that sends
/// notes on in the MIDI-effect position of a MIDI track's chain, before the
/// instrument, and puts one already there back where its type goes; its
/// label follows the selected device (`Shell::dynamic_label`).
pub const IMP_24: &str = "IMP-24";

/// The row of Beatlayer's own Deactivate All Devices / Activate All Devices
/// in a track's menu (IMP-25), just before Freeze Track: an improvement, so
/// not a row of menus.md.
pub const IMP_25: &str = "IMP-25";

/// Whether a context menu's row is one of Live's, in menus.md, rather than
/// one of Beatlayer's improvements.
#[must_use]
pub fn is_lives(row: &str) -> bool {
    row.is_empty() || row.starts_with("C-")
}

const DEVICE: &[Row] = &[
    ("C-DEVICE-001", "Cut", Key(&["S-EDIT-001"])),
    ("C-DEVICE-002", "Copy", Key(&["S-EDIT-002"])),
    ("C-DEVICE-003", "Duplicate", Key(&["S-EDIT-004"])),
    SEPARATOR,
    ("C-DEVICE-004", "Rename", Key(&["S-EDIT-008"])),
    ("C-DEVICE-005", "Edit Info Text", Pending(None)),
    SEPARATOR,
    ("C-DEVICE-006", "Delete", Key(&["S-EDIT-005"])),
    ("C-DEVICE-007", "Group", Key(&["S-DEV-001"])),
    ("C-DEVICE-008", "Fold", Key(&["S-DEV-014"])),
    (IMP_14, "Float", Cmd(SET_FLOATING)),
    (
        "C-DEVICE-009",
        "Show Preset Name",
        Cmd("device.set_show_preset_name"),
    ),
    SEPARATOR,
    ("C-DEVICE-010", "Compare: Switch to B", Cmd(COMPARE_SWITCH)),
    ("C-DEVICE-011", "Compare: Copy A to B", Cmd(COMPARE_COPY)),
    SEPARATOR,
    (
        "C-DEVICE-012",
        "Save as Default Preset",
        Cmd(SAVE_DEFAULT_PRESET),
    ),
    (IMP_24, "Use as MIDI Effect", Cmd(SET_ROLE)),
];

/// The menu a device's title bar opens, by a right-click or its Show
/// Options button (C-DEVICE). A device's own items, which Live lists after
/// these, follow them in the menu: the device's switches that its type marks
/// as menu options ([`DeviceFrame::menu_options`](crate::devices::DeviceFrame::menu_options),
/// C-DEVICE-015).
#[must_use]
pub fn device() -> Vec<MenuItem> {
    items(DEVICE)
}

/// The row of Group to Drum Rack (C-DEVICE-013), which a device's menu
/// offers straight after Group wherever the selection could sit in one
/// chain of a drum rack (the drum racks spec, R25).
pub const GROUP_TO_DRUM_RACK_ROW: &str = "C-DEVICE-013";

/// The Group to Drum Rack item, which the UI adds after Group while the
/// command can run.
#[must_use]
pub fn group_to_drum_rack() -> MenuItem {
    items(&[(
        GROUP_TO_DRUM_RACK_ROW,
        "Group to Drum Rack",
        Cmd(crate::pads::GROUP_TO_DRUM_RACK),
    )])
    .remove(0)
}

/// The row of Beatlayer's own Ungroup in a rack's menu (racks spec, R26),
/// straight after Group: not a row of menus.md.
pub const R26_UNGROUP: &str = "R26-ungroup";

/// The rows of a rack's menu that A/B compare doesn't apply to (racks spec,
/// R26): the device menu's Compare items, disabled for a rack.
pub const RACK_COMPARE_ROWS: [&str; 2] = ["C-DEVICE-010", "C-DEVICE-011"];

/// The menu a rack's title bar opens: a device's (C-DEVICE) with Group
/// working on the selection, Ungroup straight after it (Beatlayer's own
/// place), and the two Compare items disabled, because A/B compare doesn't
/// apply to racks (racks spec, R26).
#[must_use]
pub fn rack() -> Vec<MenuItem> {
    let mut rows = Vec::with_capacity(DEVICE.len() + 1);
    for &row in DEVICE {
        rows.push(row);
        if row.0 == "C-DEVICE-007" {
            rows.push((R26_UNGROUP, "Ungroup", Cmd(UNGROUP)));
        }
    }
    items(&rows)
}

/// The row of a chain's menu that holds the colour palette, 5 rows of 14
/// swatches (racks spec, R26; C-TRACK-015's palette, drawn by the UI).
pub const CHAIN_COLOUR_ROW: &str = "R26-colour";

const CHAIN: &[Row] = &[
    ("R26-rename", "Rename", Key(&["S-EDIT-008"])),
    ("R26-info", "Edit Info Text", Pending(None)),
    SEPARATOR,
    (CHAIN_COLOUR_ROW, "Colour", Cmd(SET_CHAIN_COLOUR)),
];

/// The menu a chain's row in a rack's chain list opens: Rename, Edit Info
/// Text and the colour palette (racks spec, R26; Beatlayer's own rows, as
/// menus.md records none for chains).
#[must_use]
pub fn chain() -> Vec<MenuItem> {
    items(CHAIN)
}

const TRACK: &[Row] = &[
    ("C-TRACK-001", "Cut", Key(&["S-EDIT-001"])),
    ("C-TRACK-002", "Copy", Key(&["S-EDIT-002"])),
    ("C-TRACK-003", "Duplicate", Key(&["S-EDIT-004"])),
    // IMP-12 places it straight after Duplicate.
    (
        IMP_12,
        "Duplicate Track Without Clips",
        Cmd(DUPLICATE_WITHOUT_CLIPS),
    ),
    ("C-TRACK-004", "Delete", Key(&["S-TRACK-018"])),
    SEPARATOR,
    ("C-TRACK-005", "Rename", Key(&["S-TRACK-004"])),
    ("C-TRACK-006", "Edit Info Text", Pending(None)),
    SEPARATOR,
    ("C-TRACK-007", "Freeze Track", Key(&["S-TRACK-017"])),
    (
        "C-TRACK-008",
        "Bounce Track in Place",
        Cmd("track.bounce_in_place"),
    ),
    SEPARATOR,
    ("C-TRACK-009", "Insert Audio Track", Key(&["S-TRACK-001"])),
    ("C-TRACK-010", "Insert MIDI Track", Key(&["S-TRACK-002"])),
    ("C-TRACK-011", "Insert Return Track", Key(&["S-TRACK-003"])),
    ("C-TRACK-012", "Group Tracks", Key(&["S-TRACK-006"])),
    SEPARATOR,
    ("C-TRACK-013", "Select Track Content", Pending(None)),
    SEPARATOR,
    ("C-TRACK-014", "Save as Default MIDI Track", Pending(None)),
    SEPARATOR,
    ("C-TRACK-015", "Colour palette", Pending(None)),
    SEPARATOR,
    ("C-TRACK-016", "Assign Track Color to Clips", Pending(None)),
];

/// What makes a track's menu differ from the base one, C-TRACK-001 to
/// C-TRACK-016 (C-TRACK-017 to C-TRACK-020).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TrackMenu {
    /// The track is a group (C-TRACK-017).
    pub group: bool,
    /// Several tracks are selected (C-TRACK-018).
    pub several: bool,
    /// The menu opens in the Arrangement (C-TRACK-019).
    pub arrangement: bool,
    /// The Arrangement has a time selection on the track (C-TRACK-020).
    pub time_selection: bool,
    /// Every top-level device on the selected tracks is already off, so the
    /// Deactivate All Devices item reads Activate All Devices (IMP-25).
    pub devices_off: bool,
}

/// The rows the other states add after row `after`. menus.md doesn't
/// record where Live puts them: they sit by the items they go with.
fn track_extras(after: &str, state: TrackMenu) -> Vec<Row> {
    let mut rows = Vec::new();
    match after {
        "C-TRACK-008" => {
            if state.group {
                rows.push((
                    "C-TRACK-017c",
                    "Bounce Group to New Track",
                    Cmd("track.bounce_to_new_track"),
                ));
            }
            if state.time_selection {
                rows.push(("C-TRACK-020", "Crop Clip", Key(&["S-ARR-003"])));
            }
        }
        "C-TRACK-012" => {
            if state.group {
                rows.push(("C-TRACK-017a", "Ungroup Tracks", Key(&["S-TRACK-007"])));
            }
            if state.several {
                rows.push(("C-TRACK-018a", "Link Tracks", Pending(None)));
                rows.push(("C-TRACK-018b", "Unlink Track(s)", Pending(None)));
            }
            if state.arrangement {
                rows.push(SEPARATOR);
                rows.push(("C-TRACK-019a", "Show Take Lanes", Pending(None)));
                rows.push((
                    "C-TRACK-019b",
                    "Insert Take Lane",
                    Pending(Some("S-COMP-004")),
                ));
                rows.push(("C-TRACK-019c", "Delete All Take Lanes", Pending(None)));
            }
        }
        "C-TRACK-016" if state.group => rows.push((
            "C-TRACK-017b",
            "Assign Track Color to Grouped Tracks and Clips",
            Pending(None),
        )),
        _ => {}
    }
    rows
}

/// The menu a track's title bar opens, by a right-click in the Arrangement's
/// headers, the Session View, or a mixer (C-TRACK), with the items the
/// track's `state` adds.
#[must_use]
pub fn track(state: TrackMenu) -> Vec<MenuItem> {
    let mut rows = Vec::with_capacity(TRACK.len() + 9);
    for &row in TRACK {
        // IMP-25 places Deactivate All Devices just before Freeze Track, in
        // the same group; when every top-level device on the selection is
        // already off it reads Activate All Devices.
        if row.0 == "C-TRACK-007" {
            rows.push((
                IMP_25,
                if state.devices_off {
                    "Activate All Devices"
                } else {
                    "Deactivate All Devices"
                },
                Cmd(SET_DEVICES_ENABLED),
            ));
        }
        rows.push(row);
        rows.extend(track_extras(row.0, state));
    }
    items(&rows)
}

/// The menu a note or the empty grid in the MIDI Note Editor opens
/// (C-NOTE), as the piano roll spec's R30 says. Live's order and
/// separators weren't recorded, so the rows are the manual's and the release
/// notes' in the order menus.md lists them.
const NOTE: &[Row] = &[
    ("C-NOTE-001", "Cut", Key(&["S-EDIT-001"])),
    ("C-NOTE-002", "Copy", Key(&["S-EDIT-002"])),
    ("C-NOTE-003", "Paste", Key(&["S-EDIT-003"])),
    ("C-NOTE-004", "Duplicate", Key(&["S-EDIT-004"])),
    ("C-NOTE-005", "Delete", Key(&["S-EDIT-005"])),
    (
        "C-NOTE-006",
        "Invert Selection",
        Pending(Some("S-NOTE-034")),
    ),
    ("C-NOTE-007", "Deactivate Note(s)", Key(&["S-NOTE-041"])),
    ("C-NOTE-008", "Group Notes", Key(&["S-NOTE-031"])),
    ("C-NOTE-009", "Ungroup Notes", Key(&["S-NOTE-032"])),
    ("C-NOTE-010", "Split Note(s)", Cmd(SPLIT_NOTES)),
    ("C-NOTE-011", "Chop Note(s) on Grid", Cmd(CHOP_NOTES)),
    ("C-NOTE-012", "Join Notes", Key(&["S-NOTE-007"])),
    ("C-NOTE-013", "Fit to Time Range", Key(&["S-NOTE-008"])),
    ("C-NOTE-014", "Clip Markers", Submenu),
    (
        "C-NOTE-014a",
        "Set Start Marker",
        Pending(Some("S-LOOP-001")),
    ),
    ("C-NOTE-014b", "Set Loop Start", Pending(Some("S-LOOP-002"))),
    ("C-NOTE-014c", "Set Loop End", Pending(Some("S-LOOP-003"))),
    ("C-NOTE-014d", "Set End Marker", Pending(Some("S-LOOP-004"))),
    ("C-NOTE-015", "Split Clip", Key(&["S-NOTE-036"])),
    (
        "C-NOTE-016",
        "Highlight Scales",
        Pending(Some("S-NOTE-035")),
    ),
];

/// The menu a key in the MIDI Note Editor's piano ruler opens (C-KEYS).
const KEYS: &[Row] = &[
    ("C-KEYS-001", "Deactivate Note(s)", Key(&["S-NOTE-041"])),
    ("C-KEYS-002", "Note spelling", Pending(None)),
];

/// The menu a loop bar above the note editor opens while it shows several
/// clips (C-LOOPBAR).
const LOOP_BAR: &[Row] = &[("C-LOOPBAR-001", "Duplicate", Key(&["S-LOOP-012"]))];

/// The menu a loop bar opens (C-LOOPBAR).
#[must_use]
pub fn loop_bar() -> Vec<MenuItem> {
    items(LOOP_BAR)
}

/// The menu a note or the empty grid in the MIDI Note Editor opens
/// (C-NOTE).
#[must_use]
pub fn note() -> Vec<MenuItem> {
    items(NOTE)
}

/// The menu a key in the MIDI Note Editor's piano ruler opens (C-KEYS).
#[must_use]
pub fn keys() -> Vec<MenuItem> {
    items(KEYS)
}

/// The package or milestone that builds a context-menu item shown disabled
/// for now, for the item to name.
#[must_use]
pub fn pending_package(row: &str) -> Option<&'static str> {
    match row {
        "C-DEVICE-005" | "C-CLIP-ARR-008" | "C-TRACK-006" | "R26-info" => {
            Some("WP-2.24 (the Info View)")
        }
        "C-TRACK-018a" | "C-TRACK-018b" => Some("Phase 6 (linked-track editing)"),
        "C-TRACK-019a" | "C-TRACK-019b" | "C-TRACK-019c" => {
            Some("Phase 6 (take lanes and comping)")
        }
        "C-TRACK-015" | "C-TRACK-016" | "C-TRACK-017b" => {
            Some("a package not planned yet (track colours)")
        }
        "C-TRACK-013" | "C-TRACK-014" => Some("a package not planned yet"),
        "C-CLIP-ARR-027a" | "C-CLIP-ARR-027b" | "C-CLIP-ARR-027c" => {
            Some("Phase 9 (audio to MIDI, IMP-06)")
        }
        // The MIDI Note Editor's own items (piano-roll R30).
        "C-NOTE-006" => Some("WP-6.4 (Invert Selection and the note filters)"),
        "C-NOTE-014" | "C-NOTE-014a" | "C-NOTE-014b" | "C-NOTE-014c" | "C-NOTE-014d" => {
            Some("WP-6.54 (the clip markers from the keys)")
        }
        "C-NOTE-016" | "C-KEYS-002" => Some("WP-6.7b (the scales in the Clip View)"),
        _ => None,
    }
}

/// What a clip's or a time selection's menu adds to its base rows
/// (C-CLIP-ARR-025, C-CLIP-ARR-026, C-TIME-019, C-TIME-020): a selection
/// that holds clips brings Split, Consolidate, and Crop Clip, and a time
/// selection that holds clips also brings Consolidate Time to New Scene.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SelectionMenu {
    /// The selection holds clips.
    pub clips: bool,
}

/// The rows a selection that holds clips adds to a clip's menu.
const CLIP_SELECTION: &[Row] = &[
    SEPARATOR,
    ("C-CLIP-ARR-025", "Split", Key(&["S-ARR-001"])),
    ("C-CLIP-ARR-026", "Consolidate", Key(&["S-ARR-002"])),
];

/// The rows a time selection that holds clips adds to its menu.
const TIME_SELECTION: &[Row] = &[
    SEPARATOR,
    ("C-TIME-019a", "Split", Key(&["S-ARR-001"])),
    ("C-TIME-019b", "Consolidate", Key(&["S-ARR-002"])),
    ("C-TIME-019c", "Crop Clip", Key(&["S-ARR-003"])),
    (
        "C-TIME-020",
        "Consolidate Time to New Scene",
        Cmd(CONSOLIDATE_TO_SCENE),
    ),
];

/// The menu a clip in the Arrangement opens (C-CLIP-ARR), without the rows a
/// selection adds.
#[must_use]
pub fn arrangement_clip() -> Vec<MenuItem> {
    arrangement_clip_in(SelectionMenu::default())
}

/// The menu a clip in the Arrangement opens, with the rows `state` adds.
#[must_use]
pub fn arrangement_clip_in(state: SelectionMenu) -> Vec<MenuItem> {
    items(&with_selection(
        CLIP,
        CLIP_SELECTION,
        "C-CLIP-ARR-004",
        state,
    ))
}

/// The rows an audio clip's menu adds after Reverse Clip(s) (C-CLIP-ARR-027):
/// the three Convert commands and Separate Stems to New Audio Tracks, in the
/// order observed in Live 12 on macOS (18 October 2026, from a screenshot of
/// an audio clip's menu).
const AUDIO_CLIP: &[Row] = &[
    SEPARATOR,
    (
        "C-CLIP-ARR-027e",
        "Slice to New MIDI Track",
        Cmd(SLICE_TO_MIDI),
    ),
    (
        "C-CLIP-ARR-027a",
        "Convert Harmony to New MIDI Track",
        Pending(None),
    ),
    (
        "C-CLIP-ARR-027b",
        "Convert Melody to New MIDI Track",
        Pending(None),
    ),
    (
        "C-CLIP-ARR-027c",
        "Convert Drums to New MIDI Track",
        Pending(None),
    ),
    (
        "C-CLIP-ARR-027d",
        "Separate Stems to New Audio Tracks",
        Cmd(SEPARATE_STEMS),
    ),
];

/// The menu an audio clip in the Arrangement opens: the clip's menu with
/// the rows only audio clips have (C-CLIP-ARR-027).
#[must_use]
pub fn arrangement_audio_clip() -> Vec<MenuItem> {
    arrangement_audio_clip_in(SelectionMenu::default())
}

/// The menu an audio clip in the Arrangement opens, with the rows `state`
/// adds.
#[must_use]
pub fn arrangement_audio_clip_in(state: SelectionMenu) -> Vec<MenuItem> {
    let mut rows = Vec::with_capacity(CLIP.len() + AUDIO_CLIP.len() + CLIP_SELECTION.len());
    let selection = with_selection(CLIP, CLIP_SELECTION, "C-CLIP-ARR-004", state);
    for &row in &selection {
        rows.push(row);
        if row.0 == "C-CLIP-ARR-018" {
            rows.extend_from_slice(AUDIO_CLIP);
        }
    }
    items(&rows)
}

/// The menu a time selection in the Arrangement opens (C-TIME), without the
/// rows a selection adds.
#[must_use]
pub fn arrangement_time() -> Vec<MenuItem> {
    arrangement_time_in(SelectionMenu::default())
}

/// The menu a time selection in the Arrangement opens, with the rows `state`
/// adds.
#[must_use]
pub fn arrangement_time_in(state: SelectionMenu) -> Vec<MenuItem> {
    items(&with_selection(TIME, TIME_SELECTION, "C-TIME-007", state))
}

/// The scrub area's menu (C-TIME-022).
#[must_use]
pub fn arrangement_scrub_in() -> Vec<MenuItem> {
    items(SCRUB)
}

/// A locator's menu (C-TIME-023 and C-TIME-025).
#[must_use]
pub fn arrangement_locator_in() -> Vec<MenuItem> {
    items(LOCATOR)
}

/// The loop brace's menu (C-TIME-023).
#[must_use]
pub fn arrangement_brace_in() -> Vec<MenuItem> {
    items(BRACE)
}

/// `base` with `extra`, which a selection that holds clips brings, inserted
/// after the row `after`.
fn with_selection(base: &[Row], extra: &[Row], after: &str, state: SelectionMenu) -> Vec<Row> {
    if !state.clips {
        return base.to_vec();
    }
    let mut rows = Vec::with_capacity(base.len() + extra.len());
    for &row in base {
        rows.push(row);
        if row.0 == after {
            rows.extend_from_slice(extra);
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    /// The rows of one context-menu section of the menu reference, up to
    /// its "Other states" table: each row's ID and Live's label, or `None`
    /// for a separator. `None` in a checkout without the planning documents
    /// (they aren't published yet), where the test skips.
    fn reference(heading: &str) -> Option<Vec<Option<(String, String)>>> {
        let docs = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs");
        if !docs.exists() {
            eprintln!(
                "skipped: the UI parity reference isn't published yet, and this checkout doesn't have it"
            );
            return None;
        }
        let text = std::fs::read_to_string(docs.join("ui-parity").join("menus.md")).unwrap();
        let mut rows = Vec::new();
        let mut inside = false;
        for line in text.lines() {
            if line.starts_with("### ") {
                inside = line.contains(heading);
                continue;
            }
            if !inside {
                continue;
            }
            if line.starts_with("Other states") {
                break;
            }
            let cells: Vec<&str> = line.split('|').map(str::trim).collect();
            if cells.len() > 3 && cells[1].starts_with("C-") {
                rows.push(Some((cells[1].to_owned(), cells[2].to_owned())));
            } else if cells.len() > 3 && cells[1].is_empty() && cells[2] == "—" {
                rows.push(None);
            }
        }
        Some(rows)
    }

    const EVERY_STATE: TrackMenu = TrackMenu {
        group: true,
        several: true,
        arrangement: true,
        time_selection: true,
        devices_off: false,
    };

    #[test]
    fn the_device_and_track_menus_pending_items_name_their_package() {
        for item in device().into_iter().chain(track(EVERY_STATE)) {
            if matches!(item.action, Pending(_)) {
                assert!(pending_package(item.row).is_some(), "{}", item.row);
            }
        }
    }

    // Spec: racks R26
    #[test]
    fn a_racks_menu_adds_ungroup_and_disables_compare() {
        let rack = rack();
        let rows: Vec<&str> = rack.iter().map(|i| i.row).collect();
        let group = rows.iter().position(|r| *r == "C-DEVICE-007").unwrap();
        assert_eq!(rows[group + 1], R26_UNGROUP, "straight after Group");
        let device = device();
        assert_eq!(
            rack.iter().filter(|i| i.row != R26_UNGROUP).count(),
            device.len(),
            "the rest is the device menu"
        );
        // The Compare commands refuse a rack, so the shell shows them
        // disabled (device compare R6).
        for (row, command) in RACK_COMPARE_ROWS
            .into_iter()
            .zip([COMPARE_SWITCH, COMPARE_COPY])
        {
            let item = rack.iter().find(|i| i.row == row).unwrap();
            assert_eq!(item.command(), Some(command), "{row}");
        }
        let ungroup = rack.iter().find(|i| i.row == R26_UNGROUP).unwrap();
        assert_eq!(ungroup.command(), Some(UNGROUP));
    }

    // Spec: racks R26
    #[test]
    fn a_chains_menu_holds_rename_info_text_and_the_colours() {
        let labels: Vec<&str> = chain()
            .iter()
            .filter(|i| !i.is_separator())
            .map(|i| i.label)
            .collect();
        assert_eq!(labels, ["Rename", "Edit Info Text", "Colour"]);
        assert!(chain().iter().all(|i| is_lives(i.row) == i.row.is_empty()));
        for item in chain() {
            if matches!(item.action, Pending(_)) {
                assert!(pending_package(item.row).is_some(), "{}", item.row);
            }
        }
    }

    #[test]
    fn the_track_menus_other_states_add_the_items_menus_md_lists() {
        let base = track(TrackMenu::default());
        let all = track(EVERY_STATE);
        let added: Vec<_> = all
            .iter()
            .filter(|i| !i.is_separator() && !base.iter().any(|b| b.row == i.row))
            .collect();
        assert_eq!(added.len(), 9);
        let Some(text) = std::fs::read_to_string(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/ui-parity/menus.md"),
        )
        .ok() else {
            return;
        };
        for item in added {
            let id = &item.row[..11];
            let line = text
                .lines()
                .find(|l| l.starts_with(&format!("| {id} |")))
                .unwrap_or_else(|| panic!("{id} isn't in menus.md"));
            assert!(line.contains(item.label), "{}: {line}", item.label);
        }
    }

    #[test]
    fn an_audio_clips_menu_adds_the_convert_and_stems_rows_after_reverse() {
        let base = arrangement_clip();
        let audio = arrangement_audio_clip();
        let added: Vec<_> = audio
            .iter()
            .filter(|i| !i.is_separator() && !base.iter().any(|b| b.row == i.row))
            .collect();
        assert_eq!(added.len(), 5);
        let reverse = audio
            .iter()
            .position(|i| i.row == "C-CLIP-ARR-018")
            .unwrap();
        assert!(audio[reverse + 1].is_separator());
        assert_eq!(audio[reverse + 2].label, "Slice to New MIDI Track");
        assert_eq!(
            audio[reverse + 6].label,
            "Separate Stems to New Audio Tracks"
        );
        let Some(text) = std::fs::read_to_string(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/ui-parity/menus.md"),
        )
        .ok() else {
            return;
        };
        for item in added {
            let line = text
                .lines()
                .find(|l| l.starts_with(&format!("| {} |", item.row)))
                .unwrap_or_else(|| panic!("{} isn't in menus.md", item.row));
            assert!(line.contains(item.label), "{}: {line}", item.label);
        }
    }

    #[test]
    fn the_menus_follow_lives_rows_and_separators_in_order() {
        for (menu, heading) in [
            (arrangement_clip(), "(C-CLIP-ARR)"),
            (arrangement_time(), "(C-TIME)"),
            (device(), "(C-DEVICE)"),
            (track(TrackMenu::default()), "(C-TRACK)"),
            (note(), "(C-NOTE)"),
            (keys(), "(C-KEYS)"),
            (loop_bar(), "(C-LOOPBAR)"),
        ] {
            let Some(theirs) = reference(heading) else {
                return;
            };
            assert!(!theirs.is_empty(), "{heading}");
            let menu: Vec<_> = menu.into_iter().filter(|i| is_lives(i.row)).collect();
            assert_eq!(menu.len(), theirs.len(), "{heading}");
            for (item, row) in menu.iter().zip(&theirs) {
                match row {
                    None => assert!(item.is_separator(), "{heading}: {}", item.row),
                    Some((id, label)) => {
                        assert_eq!(item.row, id);
                        // A row of choices is named for its kind, and the
                        // reference names the choices after the colon.
                        let choices = ADAPTIVE_GRID_ROWS.contains(&id.as_str())
                            || FIXED_GRID_ROWS.contains(&id.as_str())
                            || id == "C-KEYS-002";
                        let label = if choices {
                            label.split(':').next().unwrap()
                        } else {
                            label
                        };
                        let label = label.trim_end_matches(" choices");
                        // The note editor's rows name their shortcut.
                        let label = label.split(" (S-").next().unwrap();
                        // A submenu's label ends with the arrow menus.md
                        // writes, and what follows it.
                        let label = label.split(" ▸").next().unwrap();
                        assert_eq!(item.label, label, "{id}");
                    }
                }
            }
        }
    }

    // Spec: piano-roll R30
    #[test]
    fn the_note_editor_menus_name_their_later_packages() {
        for menu in [note(), keys()] {
            for item in menu {
                if matches!(item.action, Pending(_)) {
                    assert!(pending_package(item.row).is_some(), "{}", item.row);
                }
            }
        }
        // The Clip Markers submenu's items wait for their package too.
        let markers = note().into_iter().find(|i| i.row == "C-NOTE-014").unwrap();
        assert_eq!(markers.items.len(), 4);
        for item in markers.items {
            assert!(matches!(item.action, Pending(_)), "{}", item.row);
            assert!(pending_package(item.row).is_some(), "{}", item.row);
        }
    }
}

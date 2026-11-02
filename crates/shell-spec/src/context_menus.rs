//! The context menus, as data: the menu a clip opens and the menu a time
//! selection opens in the Arrangement (WP-1.8b), the menu a device's
//! title bar and Show Options button open (WP-2.4), the menu a track's
//! title bar opens (WP-4.22), and the Session View's slot, clip and scene
//! menus and the Clip View title bar's (WP-6.56), in Live's order from
//! the menu reference (C-CLIP-ARR, C-TIME, C-DEVICE, C-TRACK, C-SLOT,
//! C-CLIP-SES, C-SCENE, C-CLIPTITLE), each item tied to its
//! shortcut as the menu bar's are ([`Action::Shortcut`]), so it runs the
//! command the keymap gives that shortcut in the Arrangement, or shows
//! disabled until a package binds it.
//!
//! The grid's Adaptive Grid and Fixed Grid rows are rows of choices rather
//! than single items: they carry `arrangement.set_grid`, and the UI lays
//! out a choice for each setting.

use crate::arrangement::{SET_GRID, SHOW_TAKE_LANES};
use crate::audio_editing::CONSOLIDATE_TO_SCENE;
use crate::automation::{INSERT_SHAPE, SIMPLIFY};
use crate::clip_properties::{ARRANGE_PANELS, FOLD_PANELS};
use crate::commands::{VIEW_HIGHLIGHT_SCALES, VIEW_NOTE_SPELLING};
use crate::comping::COPY_TO_MAIN_LANE;
use crate::devices::{
    COMPARE_COPY, COMPARE_SWITCH, SAVE_DEFAULT_PRESET, SET_FLOATING, SET_ROLE, UNGROUP,
};
use crate::menus::{Action, IMP_12, IMP_33_FOLLOW, MenuItem, Row, SEPARATOR, items};
use crate::midi_files::EXPORT as EXPORT_MIDI_CLIP;
use crate::notes::{CHOP as CHOP_NOTES, SPLIT as SPLIT_NOTES};
use crate::slice::OPEN as SLICE_TO_MIDI;
use crate::stems::OPEN as SEPARATE_STEMS;
use crate::tracks::{DUPLICATE_WITHOUT_CLIPS, LINK, SET_DEVICES_ENABLED, UNLINK};

use Action::{Choice, Command as Cmd, Pending, Shortcut as Key, Submenu};

/// The model's command that colours a chain.
const SET_CHAIN_COLOUR: &str = "chain.set_colour";

/// The model's command that colours the selected tracks, which the palette
/// at the end of a track's menu runs.
const SET_TRACK_COLOUR: &str = "track.set_colour";

/// The rows that hold the zoom-adaptive grid's settings.
pub const ADAPTIVE_GRID_ROWS: [&str; 2] = ["C-CLIP-ARR-020", "C-TIME-013"];

/// The rows that hold the automation shapes, as rows of buttons: the
/// waveforms, then the ramps and the ADSR (C-TIME-021; the automation
/// editing spec, R11).
pub const SHAPE_ROWS: [&str; 2] = ["C-TIME-021a", "C-TIME-021b"];

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
    (
        "C-CLIP-ARR-008",
        "Edit Info Text",
        Cmd("clip.set_info_text"),
    ),
    ("C-CLIP-ARR-009", "Extract Groove(s)", Cmd(EXTRACT_GROOVE)),
    ("C-CLIP-ARR-010", "Deactivate Clip", Key(&["S-ARR-027"])),
    (
        "C-CLIP-ARR-011",
        "Export MIDI Clip...",
        Cmd(EXPORT_MIDI_CLIP),
    ),
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
    (
        "C-TIME-022b",
        "Insert Time Signature Change",
        Cmd(crate::locator::INSERT_SIGNATURE),
    ),
];

/// A fragmentary bar's menu (C-TIME-024).
const FRAGMENTARY: &[Row] = &[
    (
        "C-TIME-024a",
        "Delete Fragmentary Bar Time",
        Cmd(crate::editing::DELETE_FRAGMENTARY_BAR_TIME),
    ),
    (
        "C-TIME-024b",
        "Complete Fragmentary Bar",
        Cmd(crate::editing::COMPLETE_FRAGMENTARY_BAR),
    ),
];

/// A time-signature marker's menu (C-TIME-027).
const SIGNATURE: &[Row] = &[(
    "C-TIME-027",
    "Edit Time Signature",
    Cmd(crate::locator::EDIT_SIGNATURE),
)];

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
    (
        "C-TIME-025a",
        "Edit Info Text",
        Cmd(crate::locator::SET_INFO_TEXT),
    ),
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
    (
        "C-DEVICE-005",
        "Edit Info Text",
        Cmd("device.set_info_text"),
    ),
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
    ("R26-info", "Edit Info Text", Cmd("chain.set_info")),
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
    ("C-TRACK-006", "Edit Info Text", Cmd("track.set_info_text")),
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
    ("C-TRACK-015", "Colour palette", Cmd(SET_TRACK_COLOUR)),
    SEPARATOR,
    (
        "C-TRACK-016",
        "Assign Track Color to Clips",
        Cmd("track.assign_colour_to_clips"),
    ),
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
    /// The track is a MIDI track, which has Follow Chord Lane (IMP-33).
    pub midi: bool,
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
            if state.several || state.group {
                rows.push(("C-TRACK-018a", "Link Tracks", Cmd(LINK)));
                rows.push(("C-TRACK-018b", "Unlink Track(s)", Cmd(UNLINK)));
            }
            if state.arrangement {
                rows.push(SEPARATOR);
                // Spec: track-context-menu R10.
                rows.push(("C-TRACK-019a", "Show Take Lanes", Cmd(SHOW_TAKE_LANES)));
                rows.push(("C-TRACK-019b", "Insert Take Lane", Key(&["S-COMP-004"])));
                rows.push((
                    "C-TRACK-019c",
                    "Delete All Take Lanes",
                    Cmd("take_lane.delete_all"),
                ));
                rows.push((
                    "C-TRACK-019d",
                    "Delete All Unused Take Lanes",
                    Cmd("take_lane.delete_unused"),
                ));
            }
        }
        // Spec: chord-lane R5. A MIDI track's own switch, in a group of its own.
        "C-TRACK-014" if state.midi => {
            rows.push(SEPARATOR);
            rows.push((
                IMP_33_FOLLOW,
                "Follow Chord Lane",
                Cmd("track.set_follow_chord_lane"),
            ));
        }
        "C-TRACK-016" if state.group => rows.push((
            "C-TRACK-017b",
            "Assign Track Color to Grouped Tracks and Clips",
            Cmd("track.assign_colour_to_grouped"),
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

/// The model's command that colours the selected clips, which the palette
/// at the end of a Session clip's menu and the Clip View title bar's menu
/// runs.
const SET_CLIP_COLOUR: &str = "clip.set_colour";

/// The model's command that colours scenes, which the palette at the end of
/// a scene's menu runs.
const SET_SCENE_COLOUR: &str = "scene.set_colour";

/// The row of a Session clip's Paste, straight after Copy (R2; Beatlayer's
/// place, as menus.md records no id for it).
pub const CLIP_SES_PASTE: &str = "C-CLIP-SES-002a";

const SLOT: &[Row] = &[
    ("C-SLOT-001", "Freeze Track", Key(&["S-TRACK-017"])),
    (
        "C-SLOT-002",
        "Bounce Track in Place",
        Cmd("track.bounce_in_place"),
    ),
    ("C-SLOT-003", "Bounce to New Track", Key(&["S-BOUNCE-001"])),
    ("C-SLOT-004", "Paste Bounced Audio", Key(&["S-BOUNCE-002"])),
    SEPARATOR,
    ("C-SLOT-005", "Crop Clip", Key(&["S-SES-028"])),
    SEPARATOR,
    ("C-SLOT-006", "Remove Stop Button", Key(&["S-SES-005"])),
    ("C-SLOT-007", "Insert Empty MIDI Clip", Key(&["S-SES-007"])),
];

/// What makes an empty slot's menu differ from the base one, C-SLOT-001 to
/// C-SLOT-007 as Live shows them on a MIDI track (C-SLOT-008, C-SLOT-009).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SlotMenu {
    /// Something to paste is on the clipboard (C-SLOT-008).
    pub paste: bool,
    /// The track is a MIDI track, whose slot offers Insert Empty MIDI Clip.
    pub midi: bool,
    /// The track is a group (C-SLOT-009).
    pub group: bool,
    /// The slot's stop button was removed, so its item reads Add Stop
    /// Button.
    pub stop_removed: bool,
}

impl Default for SlotMenu {
    fn default() -> Self {
        Self {
            paste: false,
            midi: true,
            group: false,
            stop_removed: false,
        }
    }
}

/// The label of the stop button's item (C-SLOT-006, C-CLIP-SES-015).
const fn stop_button(removed: bool) -> &'static str {
    if removed {
        "Add Stop Button"
    } else {
        "Remove Stop Button"
    }
}

/// The menu an empty slot opens, by a right-click or the keys (C-SLOT), with
/// the items `state` adds: Paste first, followed by a separator, and Bounce
/// Group to New Track after Bounce to New Track (the session context menus
/// spec, R1).
#[must_use]
pub fn slot(state: SlotMenu) -> Vec<MenuItem> {
    let mut rows = Vec::with_capacity(SLOT.len() + 3);
    if state.paste {
        rows.push(("C-SLOT-008", "Paste", Key(&["S-EDIT-003"])));
        rows.push(SEPARATOR);
    }
    for &row in SLOT {
        match row.0 {
            "C-SLOT-006" => rows.push((row.0, stop_button(state.stop_removed), row.2)),
            "C-SLOT-007" if !state.midi => {}
            _ => rows.push(row),
        }
        if row.0 == "C-SLOT-003" && state.group {
            rows.push((
                "C-SLOT-009",
                "Bounce Group to New Track",
                Cmd("track.bounce_to_new_track"),
            ));
        }
    }
    items(&rows)
}

const CLIP_SES: &[Row] = &[
    ("C-CLIP-SES-001", "Cut", Key(&["S-EDIT-001"])),
    ("C-CLIP-SES-002", "Copy", Key(&["S-EDIT-002"])),
    ("C-CLIP-SES-003", "Duplicate", Key(&["S-EDIT-004"])),
    ("C-CLIP-SES-004", "Delete", Key(&["S-EDIT-005"])),
    SEPARATOR,
    ("C-CLIP-SES-005", "Rename", Key(&["S-EDIT-008"])),
    (
        "C-CLIP-SES-006",
        "Edit Info Text",
        Cmd("clip.set_info_text"),
    ),
    ("C-CLIP-SES-007", "Extract Groove(s)", Cmd(EXTRACT_GROOVE)),
    ("C-CLIP-SES-008", "Deactivate Clip", Key(&["S-SES-018"])),
    (
        "C-CLIP-SES-009",
        "Export MIDI Clip...",
        Cmd(EXPORT_MIDI_CLIP),
    ),
    SEPARATOR,
    ("C-CLIP-SES-010", "Freeze Track", Key(&["S-TRACK-017"])),
    (
        "C-CLIP-SES-011",
        "Bounce Track in Place",
        Cmd("track.bounce_in_place"),
    ),
    (
        "C-CLIP-SES-012",
        "Bounce to New Track",
        Key(&["S-BOUNCE-001"]),
    ),
    (
        "C-CLIP-SES-013",
        "Paste Bounced Audio",
        Key(&["S-BOUNCE-002"]),
    ),
    SEPARATOR,
    ("C-CLIP-SES-014", "Crop Clip", Key(&["S-SES-028"])),
    SEPARATOR,
    ("C-CLIP-SES-015", "Remove Stop Button", Key(&["S-SES-005"])),
    (
        "C-CLIP-SES-016",
        "Create Follow Action Chain",
        Key(&["S-SES-014"]),
    ),
    SEPARATOR,
    (
        "C-CLIP-SES-017",
        "Select on Launch (Pref)",
        Cmd("settings.set_select_on_launch"),
    ),
    SEPARATOR,
    ("C-CLIP-SES-018", "Colour palette", Cmd(SET_CLIP_COLOUR)),
];

/// The rows an audio clip's Session menu adds after Crop Clip and a
/// separator, in the Arrangement's audio clip menu's order (C-CLIP-SES-019,
/// C-CLIP-SES-020; the session context menus spec, R2).
const CLIP_SES_AUDIO: &[Row] = &[
    SEPARATOR,
    (
        "C-CLIP-SES-019a",
        "Slice to New MIDI Track",
        Cmd(SLICE_TO_MIDI),
    ),
    (
        "C-CLIP-SES-019b",
        "Convert Harmony to New MIDI Track",
        Pending(None),
    ),
    (
        "C-CLIP-SES-019c",
        "Convert Melody to New MIDI Track",
        Pending(None),
    ),
    (
        "C-CLIP-SES-019d",
        "Convert Drums to New MIDI Track",
        Pending(None),
    ),
    (
        "C-CLIP-SES-020",
        "Separate Stems to New Audio Tracks",
        Cmd(SEPARATE_STEMS),
    ),
];

/// What makes a Session clip's menu differ from the base one, C-CLIP-SES-001
/// to C-CLIP-SES-018 as Live shows them on a MIDI clip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionClipMenu {
    /// Something to paste is on the clipboard: Paste comes straight after
    /// Copy.
    pub paste: bool,
    /// The clip is a MIDI clip, whose menu has Export MIDI Clip....
    pub midi: bool,
    /// The clip is an audio clip, whose menu adds the Slice, Convert and
    /// Separate Stems items after Crop Clip (C-CLIP-SES-019, C-CLIP-SES-020).
    pub audio: bool,
    /// The slot's stop button was removed, so its item reads Add Stop
    /// Button.
    pub stop_removed: bool,
}

impl Default for SessionClipMenu {
    fn default() -> Self {
        Self {
            paste: false,
            midi: true,
            audio: false,
            stop_removed: false,
        }
    }
}

/// The menu a clip in a Session slot opens (C-CLIP-SES), with the items
/// `state` adds.
#[must_use]
pub fn session_clip(state: SessionClipMenu) -> Vec<MenuItem> {
    let mut rows = Vec::with_capacity(CLIP_SES.len() + CLIP_SES_AUDIO.len() + 1);
    for &row in CLIP_SES {
        match row.0 {
            "C-CLIP-SES-009" if !state.midi => continue,
            "C-CLIP-SES-015" => rows.push((row.0, stop_button(state.stop_removed), row.2)),
            _ => rows.push(row),
        }
        match row.0 {
            "C-CLIP-SES-002" if state.paste => {
                rows.push((CLIP_SES_PASTE, "Paste", Key(&["S-EDIT-003"])));
            }
            "C-CLIP-SES-014" if state.audio => rows.extend_from_slice(CLIP_SES_AUDIO),
            _ => {}
        }
    }
    items(&rows)
}

const SCENE: &[Row] = &[
    ("C-SCENE-001", "Cut", Key(&["S-EDIT-001"])),
    ("C-SCENE-002", "Copy", Key(&["S-EDIT-002"])),
    ("C-SCENE-003", "Duplicate", Key(&["S-EDIT-004"])),
    ("C-SCENE-004", "Delete", Key(&["S-EDIT-005"])),
    SEPARATOR,
    ("C-SCENE-005", "Rename", Key(&["S-EDIT-008"])),
    ("C-SCENE-006", "Edit Info Text", Cmd("scene.set_info_text")),
    SEPARATOR,
    ("C-SCENE-007", "Insert Scene", Key(&["S-SES-008"])),
    (
        "C-SCENE-008",
        "Capture and Insert Scene",
        Key(&["S-SES-009"]),
    ),
    SEPARATOR,
    (
        "C-SCENE-009",
        "Select All Slots in Scene",
        Cmd(SELECT_SCENE_SLOTS),
    ),
    SEPARATOR,
    (
        "C-SCENE-010",
        "Cancel Scene Launch",
        Cmd(CANCEL_SCENE_LAUNCH),
    ),
    SEPARATOR,
    (
        "C-SCENE-011",
        "Select Next Scene on Launch (Pref)",
        Cmd("settings.set_select_next_scene_on_launch"),
    ),
    (
        "C-SCENE-012",
        "Trigger Recording on Launch (Pref)",
        Cmd("settings.set_start_recording_on_scene_launch"),
    ),
    SEPARATOR,
    (
        "C-SCENE-013",
        "Scene Tempo and Time Signature",
        Cmd("mixer_view.scene_tempo"),
    ),
    SEPARATOR,
    ("C-SCENE-014", "No Color", Cmd(SET_SCENE_COLOUR)),
];

/// The scene menu's item that selects every slot of the selected scenes
/// (C-SCENE-009). It runs `session.select` once for each slot, so it has no
/// command of its own: the UI draws it.
pub const SELECT_SCENE_SLOTS: &str = "session.select";

/// The scene menu's Cancel Scene Launch (C-SCENE-010).
pub const CANCEL_SCENE_LAUNCH: &str = "session.cancel_scene_launch";

/// The model's command that makes a groove of each clip.
const EXTRACT_GROOVE: &str = "groove.extract";

/// What makes a scene's menu differ from the base one, C-SCENE-001 to
/// C-SCENE-014.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SceneMenu {
    /// Something to paste is on the clipboard: Paste (C-SCENE-015) comes
    /// straight after Copy.
    pub paste: bool,
}

/// The menu a scene opens (C-SCENE), from the Main track's column or the
/// Scene View's title bar, with the items `state` adds.
#[must_use]
pub fn scene(state: SceneMenu) -> Vec<MenuItem> {
    let mut rows = Vec::with_capacity(SCENE.len() + 1);
    for &row in SCENE {
        rows.push(row);
        if row.0 == "C-SCENE-002" && state.paste {
            rows.push(("C-SCENE-015", "Paste", Key(&["S-EDIT-003"])));
        }
    }
    items(&rows)
}

/// The row of a scene field's one item, Return to Default (C-SCENE-016): it
/// runs `scene.set_tempo` on the tempo field and `scene.set_time_signature`
/// on the signature's, each with no value.
#[must_use]
pub fn scene_field(tempo: bool) -> Vec<MenuItem> {
    items(&[(
        "C-SCENE-016",
        "Return to Default",
        Cmd(if tempo {
            "scene.set_tempo"
        } else {
            "scene.set_time_signature"
        }),
    )])
}

/// The row of the Clip View title bar's Rename (C-CLIPTITLE-001).
pub const CLIP_TITLE_RENAME: &str = "C-CLIPTITLE-001";

/// The row of the Clip View title bar's colour palette (C-CLIPTITLE-003).
pub const CLIP_TITLE_COLOUR: &str = "C-CLIPTITLE-003";

const CLIP_TITLE: &[Row] = &[
    (CLIP_TITLE_RENAME, "Rename", Key(&["S-EDIT-008"])),
    (
        "C-CLIPTITLE-002",
        "Edit Info Text",
        Cmd("clip.set_info_text"),
    ),
    SEPARATOR,
    (CLIP_TITLE_COLOUR, "Colour palette", Cmd(SET_CLIP_COLOUR)),
    SEPARATOR,
    ("C-CLIPTITLE-004", "Fold", Cmd(FOLD_PANELS)),
    SEPARATOR,
    (
        "C-CLIPTITLE-005",
        "Arrange Clip View Panels Automatically",
        Choice(ARRANGE_PANELS, r#"{"arrangement":"automatic"}"#),
    ),
    (
        "C-CLIPTITLE-006",
        "Arrange Clip View Panels Horizontally",
        Choice(ARRANGE_PANELS, r#"{"arrangement":"horizontal"}"#),
    ),
    (
        "C-CLIPTITLE-007",
        "Arrange Clip View Panels Vertically",
        Choice(ARRANGE_PANELS, r#"{"arrangement":"vertical"}"#),
    ),
];

/// The menu the Clip View's title bar opens (C-CLIPTITLE), in Beatlayer's own
/// order (the session context menus spec, R4).
#[must_use]
pub fn clip_title() -> Vec<MenuItem> {
    items(CLIP_TITLE)
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
    ("C-NOTE-006", "Invert Selection", Key(&["S-NOTE-034"])),
    ("C-NOTE-007", "Deactivate Note(s)", Key(&["S-NOTE-041"])),
    ("C-NOTE-008", "Group Notes", Key(&["S-NOTE-031"])),
    ("C-NOTE-009", "Ungroup Notes", Key(&["S-NOTE-032"])),
    ("C-NOTE-010", "Split Note(s)", Cmd(SPLIT_NOTES)),
    ("C-NOTE-011", "Chop Note(s) on Grid", Cmd(CHOP_NOTES)),
    ("C-NOTE-012", "Join Notes", Key(&["S-NOTE-007"])),
    ("C-NOTE-013", "Fit to Time Range", Key(&["S-NOTE-008"])),
    ("C-NOTE-014", "Clip Markers", Submenu),
    ("C-NOTE-014a", "Set Start Marker", Key(&["S-LOOP-001"])),
    ("C-NOTE-014b", "Set Loop Start", Key(&["S-LOOP-002"])),
    ("C-NOTE-014c", "Set Loop End", Key(&["S-LOOP-003"])),
    ("C-NOTE-014d", "Set End Marker", Key(&["S-LOOP-004"])),
    ("C-NOTE-015", "Split Clip", Key(&["S-NOTE-036"])),
    ("C-NOTE-016", "Highlight Scales", Cmd(VIEW_HIGHLIGHT_SCALES)),
];

/// The menu a key in the MIDI Note Editor's piano ruler opens (C-KEYS).
const KEYS: &[Row] = &[
    ("C-KEYS-001", "Deactivate Note(s)", Key(&["S-NOTE-041"])),
    ("C-KEYS-002", "Note spelling", Submenu),
    (
        "C-KEYS-002a",
        "Sharps",
        Choice(VIEW_NOTE_SPELLING, r#"{"spelling":"sharps"}"#),
    ),
    (
        "C-KEYS-002b",
        "Flats",
        Choice(VIEW_NOTE_SPELLING, r#"{"spelling":"flats"}"#),
    ),
    (
        "C-KEYS-002c",
        "Sharps and Flats",
        Choice(VIEW_NOTE_SPELLING, r#"{"spelling":"sharps_and_flats"}"#),
    ),
    (
        "C-KEYS-002d",
        "Auto",
        Choice(VIEW_NOTE_SPELLING, r#"{"spelling":"auto"}"#),
    ),
    (
        "C-KEYS-002e",
        "MIDI Note Numbers",
        Choice(VIEW_NOTE_SPELLING, r#"{"spelling":"note_numbers"}"#),
    ),
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
        "C-CLIP-SES-019b" | "C-CLIP-SES-019c" | "C-CLIP-SES-019d" => {
            Some("Phase 9 (audio to MIDI, IMP-06)")
        }
        "C-TRACK-013" | "C-TRACK-014" => Some("a package not planned yet"),
        "C-CLIP-ARR-027a" | "C-CLIP-ARR-027b" | "C-CLIP-ARR-027c" => {
            Some("Phase 9 (audio to MIDI, IMP-06)")
        }
        // The MIDI Note Editor's own items (piano-roll R30).
        "C-NOTE-014" | "C-NOTE-014a" | "C-NOTE-014b" | "C-NOTE-014c" | "C-NOTE-014d" => {
            Some("WP-6.54 (the clip markers from the keys)")
        }
        _ => None,
    }
}

/// What a clip's or a time selection's menu adds to its base rows
/// (C-CLIP-ARR-025, C-CLIP-ARR-026, C-TIME-019, C-TIME-020): a selection
/// that holds clips brings Split, Consolidate, and Crop Clip, and a time
/// selection that holds clips also brings Consolidate Time to New Scene; a
/// selection on a take lane brings Copy Selection to Main Lane (the comping
/// spec, R29).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SelectionMenu {
    /// The selection holds clips.
    pub clips: bool,
    /// A time selection's menu brings the …Time commands (C-TIME-018).
    pub time_commands: bool,
    /// The time selection covers an automation lane, which brings the
    /// shapes and Simplify Envelope (C-TIME-021).
    pub automation: bool,
    /// The selection is on a take lane.
    pub take_lane: bool,
    /// A time selection holds clips on audio tracks, which brings Separate
    /// Stems to New Audio Tracks after Crop Clip (stem separation R22).
    pub audio: bool,
}

/// The row of Beatlayer's own Copy Selection to Main Lane in a take-lane
/// clip's menu, after Paste Bounced Audio (the comping spec, R29).
pub const R29_COPY_CLIP: &str = "R29-copy-to-main-lane-clip";

/// The row of Beatlayer's own Copy Selection to Main Lane in the time
/// selection's menu on a take lane, after Paste Bounced Audio (R29).
pub const R29_COPY_TIME: &str = "R29-copy-to-main-lane-time";

/// The menu a take lane's header opens, C-TAKE-001 to C-TAKE-008 of menus.md
/// in its order (the comping spec, R29). Live's separators weren't recorded.
const TAKE_LANE: &[Row] = &[
    (
        "C-TAKE-001",
        "Copy Selection to Main Lane",
        Cmd(COPY_TO_MAIN_LANE),
    ),
    ("C-TAKE-002", "Insert Take Lane", Key(&["S-COMP-004"])),
    ("C-TAKE-003", "Duplicate", Key(&["S-COMP-005"])),
    ("C-TAKE-004", "Rename", Key(&["S-EDIT-008"])),
    ("C-TAKE-005", "Delete", Key(&["S-EDIT-005"])),
    (
        "C-TAKE-006",
        "Delete All Take Lanes",
        Cmd("take_lane.delete_all"),
    ),
    (
        "C-TAKE-007",
        "Delete All Unused Take Lanes",
        Cmd("take_lane.delete_unused"),
    ),
    ("C-TAKE-008", "Paste Bounced Audio", Key(&["S-BOUNCE-002"])),
];

/// The menu a take lane's header opens (C-TAKE).
#[must_use]
pub fn take_lane() -> Vec<MenuItem> {
    items(TAKE_LANE)
}

/// The rows a time selection in an automation lane adds to the top of its
/// menu: the shapes as two rows of buttons, and Simplify Envelope. Where in
/// the menu they sit isn't recorded in the UI parity reference.
const TIME_AUTOMATION: &[Row] = &[
    ("C-TIME-021a", "Waveforms", Cmd(INSERT_SHAPE)),
    ("C-TIME-021b", "Ramps and ADSR", Cmd(INSERT_SHAPE)),
    ("C-TIME-021c", "Simplify Envelope", Cmd(SIMPLIFY)),
    SEPARATOR,
];

/// The rows a selection that holds clips adds to a clip's menu.
const CLIP_SELECTION: &[Row] = &[
    SEPARATOR,
    ("C-CLIP-ARR-025", "Split", Key(&["S-ARR-001"])),
    ("C-CLIP-ARR-026", "Consolidate", Key(&["S-ARR-002"])),
];

/// The …Time commands a time selection's menu adds after Duplicate
/// (C-TIME-018; time editing R22). menus.md doesn't place them in the
/// observed menu, so [`SelectionMenu::time_commands`] brings them and the
/// base menu stays as observed.
const TIME_COMMANDS: &[Row] = &[
    SEPARATOR,
    ("C-TIME-018a", "Cut Time", Key(&["S-ARR-015"])),
    ("C-TIME-018b", "Copy Time", Key(&["S-ARR-016"])),
    ("C-TIME-018c", "Paste Time", Key(&["S-ARR-017"])),
    ("C-TIME-018d", "Duplicate Time", Key(&["S-ARR-018"])),
    ("C-TIME-018e", "Delete Time", Key(&["S-ARR-019"])),
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

/// The row a time selection that holds clips on audio tracks adds after Crop
/// Clip (C-TIME-019c), as C-TIME-028: Beatlayer's own place (stem separation R22).
const TIME_STEMS: &[Row] = &[(
    "C-TIME-028",
    "Separate Stems to New Audio Tracks",
    Cmd(SEPARATE_STEMS),
)];

/// The menu a clip in the Arrangement opens (C-CLIP-ARR), without the rows a
/// selection adds.
#[must_use]
pub fn arrangement_clip() -> Vec<MenuItem> {
    arrangement_clip_in(SelectionMenu::default())
}

/// The menu a clip in the Arrangement opens, with the rows `state` adds.
#[must_use]
pub fn arrangement_clip_in(state: SelectionMenu) -> Vec<MenuItem> {
    items(&with_clip_colours(
        with_take_lane(
            with_selection(CLIP, CLIP_SELECTION, "C-CLIP-ARR-004", state),
            "C-CLIP-ARR-016",
            (
                R29_COPY_CLIP,
                "Copy Selection to Main Lane",
                Cmd(COPY_TO_MAIN_LANE),
            ),
            state,
        ),
        state,
    ))
}

/// The row of an Arrangement clip's menu that holds the colour palette,
/// last in the menu after a separator (C-CLIP-ARR-028; the themes spec,
/// R16), drawn by the UI.
pub const CLIP_COLOUR_ROW: &str = "C-CLIP-ARR-028";

/// `rows` with the clip colours after them (C-CLIP-ARR-028) when the menu
/// acts on clips, as a clip's menu does once the clip is selected.
fn with_clip_colours(mut rows: Vec<Row>, state: SelectionMenu) -> Vec<Row> {
    if state.clips {
        rows.push(SEPARATOR);
        rows.push((CLIP_COLOUR_ROW, "Colour palette", Cmd("clip.set_colour")));
    }
    rows
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
    let selection = with_take_lane(
        with_selection(CLIP, CLIP_SELECTION, "C-CLIP-ARR-004", state),
        "C-CLIP-ARR-016",
        (
            R29_COPY_CLIP,
            "Copy Selection to Main Lane",
            Cmd(COPY_TO_MAIN_LANE),
        ),
        state,
    );
    for &row in &selection {
        rows.push(row);
        if row.0 == "C-CLIP-ARR-018" {
            rows.extend_from_slice(AUDIO_CLIP);
        }
    }
    items(&with_clip_colours(rows, state))
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
    let mut rows = Vec::new();
    if state.automation {
        rows.extend_from_slice(TIME_AUTOMATION);
    }
    let mut time = with_selection(TIME, TIME_SELECTION, "C-TIME-007", state);
    if state.clips && state.audio {
        time = insert_after(&time, TIME_STEMS, "C-TIME-019c");
    }
    if state.time_commands {
        time = insert_after(&time, TIME_COMMANDS, "C-TIME-003");
    }
    rows.extend(with_take_lane(
        time,
        "C-TIME-011",
        (
            R29_COPY_TIME,
            "Copy Selection to Main Lane",
            Cmd(COPY_TO_MAIN_LANE),
        ),
        state,
    ));
    items(&rows)
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

/// A fragmentary bar's menu (C-TIME-024).
#[must_use]
pub fn arrangement_fragmentary_bar_in() -> Vec<MenuItem> {
    items(FRAGMENTARY)
}

/// A time-signature marker's menu (C-TIME-027).
#[must_use]
pub fn arrangement_signature_in() -> Vec<MenuItem> {
    items(SIGNATURE)
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
    insert_after(base, extra, after)
}

/// `base` with `extra` inserted after the row `after`.
fn insert_after(base: &[Row], extra: &[Row], after: &str) -> Vec<Row> {
    let mut rows = Vec::with_capacity(base.len() + extra.len());
    for &row in base {
        rows.push(row);
        if row.0 == after {
            rows.extend_from_slice(extra);
        }
    }
    rows
}

/// `rows` with Copy Selection to Main Lane, `copy`, inserted after the row
/// `after` while the selection is on a take lane.
fn with_take_lane(rows: Vec<Row>, after: &str, copy: Row, state: SelectionMenu) -> Vec<Row> {
    if !state.take_lane {
        return rows;
    }
    let mut out = Vec::with_capacity(rows.len() + 1);
    for row in rows {
        out.push(row);
        if row.0 == after {
            out.push(copy);
        }
    }
    out
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
        midi: true,
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
        // IMP-33's Follow Chord Lane is Beatlayer's own, so menus.md's Live rows
        // don't list it by a C-TRACK ID; its own test follows.
        let added: Vec<_> = all
            .iter()
            .filter(|i| {
                !i.is_separator() && i.row != IMP_33_FOLLOW && !base.iter().any(|b| b.row == i.row)
            })
            .collect();
        assert_eq!(added.len(), 10);
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

    // Spec: chord-lane R5
    #[test]
    fn a_midi_tracks_menu_adds_follow_chord_lane_after_save_as_default_midi_track() {
        let rows = |state| -> Vec<&'static str> { track(state).iter().map(|i| i.row).collect() };
        assert!(!rows(TrackMenu::default()).contains(&IMP_33_FOLLOW));
        let midi = rows(TrackMenu {
            midi: true,
            ..TrackMenu::default()
        });
        let default = midi.iter().position(|r| *r == "C-TRACK-014").unwrap();
        assert_eq!(midi[default + 1], "", "in a group of its own");
        assert_eq!(midi[default + 2], IMP_33_FOLLOW);
        let item = track(TrackMenu {
            midi: true,
            ..TrackMenu::default()
        })
        .into_iter()
        .find(|i| i.row == IMP_33_FOLLOW)
        .unwrap();
        assert_eq!(item.command(), Some("track.set_follow_chord_lane"));
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

    // Spec: time-editing R22
    #[test]
    fn a_time_selections_menu_brings_the_time_commands_after_duplicate() {
        let base = arrangement_time();
        assert!(!base.iter().any(|i| i.row.starts_with("C-TIME-018")));
        let with = arrangement_time_in(SelectionMenu {
            time_commands: true,
            ..SelectionMenu::default()
        });
        let labels: Vec<_> = with.iter().map(|i| i.label).collect();
        let duplicate = labels.iter().position(|l| *l == "Duplicate").unwrap();
        assert_eq!(
            labels[duplicate + 2..duplicate + 7],
            [
                "Cut Time",
                "Copy Time",
                "Paste Time",
                "Duplicate Time",
                "Delete Time"
            ]
        );
        assert!(with[duplicate + 1].is_separator() && with[duplicate + 7].is_separator());
    }

    #[test]
    fn the_menus_follow_lives_rows_and_separators_in_order() {
        for (menu, heading) in [
            (arrangement_clip(), "(C-CLIP-ARR)"),
            (arrangement_time(), "(C-TIME)"),
            (device(), "(C-DEVICE)"),
            (track(TrackMenu::default()), "(C-TRACK)"),
            (take_lane(), "(C-TAKE)"),
            (note(), "(C-NOTE)"),
            (keys(), "(C-KEYS)"),
            (loop_bar(), "(C-LOOPBAR)"),
            (slot(SlotMenu::default()), "(C-SLOT)"),
            (session_clip(SessionClipMenu::default()), "(C-CLIP-SES)"),
            (scene(SceneMenu::default()), "(C-SCENE)"),
            (clip_title(), "(C-CLIPTITLE)"),
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
                        // The Clip View title bar's rows describe their item
                        // after its name.
                        if id.starts_with("C-CLIPTITLE") {
                            assert!(label.starts_with(item.label), "{id}: {label}");
                            continue;
                        }
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
                        // The scene's last row is No Color and the palette.
                        let label = label.split(", then").next().unwrap();
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

    // Spec: session-context-menus R1, R13
    #[test]
    fn a_slots_menu_adds_paste_and_the_group_bounce_where_the_spec_puts_them() {
        let rows = |state| -> Vec<&'static str> { slot(state).iter().map(|i| i.row).collect() };
        let base = rows(SlotMenu::default());
        assert_eq!(base[0], "C-SLOT-001");
        let paste = rows(SlotMenu {
            paste: true,
            ..SlotMenu::default()
        });
        assert_eq!(&paste[..3], ["C-SLOT-008", "", "C-SLOT-001"]);
        let group = rows(SlotMenu {
            group: true,
            ..SlotMenu::default()
        });
        let at = group.iter().position(|r| *r == "C-SLOT-003").unwrap();
        assert_eq!(group[at + 1], "C-SLOT-009");
        let audio = rows(SlotMenu {
            midi: false,
            ..SlotMenu::default()
        });
        assert!(!audio.contains(&"C-SLOT-007"));
        // Add Stop Button reads for a slot without one.
        let removed = slot(SlotMenu {
            stop_removed: true,
            ..SlotMenu::default()
        });
        let stop = removed.iter().find(|i| i.row == "C-SLOT-006").unwrap();
        assert_eq!(stop.label, "Add Stop Button");
        assert_eq!(stop.action, Key(&["S-SES-005"]));
    }

    // Spec: session-context-menus R2, R13
    #[test]
    fn a_session_clips_menu_adds_paste_the_audio_rows_and_leaves_out_the_midi_export() {
        let menu = |state| session_clip(state);
        let rows = |state| -> Vec<&'static str> { menu(state).iter().map(|i| i.row).collect() };
        let paste = rows(SessionClipMenu {
            paste: true,
            ..SessionClipMenu::default()
        });
        assert_eq!(
            &paste[..3],
            ["C-CLIP-SES-001", "C-CLIP-SES-002", CLIP_SES_PASTE]
        );
        let audio = menu(SessionClipMenu {
            midi: false,
            audio: true,
            ..SessionClipMenu::default()
        });
        assert!(!audio.iter().any(|i| i.row == "C-CLIP-SES-009"));
        let crop = audio
            .iter()
            .position(|i| i.row == "C-CLIP-SES-014")
            .unwrap();
        assert!(audio[crop + 1].is_separator());
        let labels: Vec<&str> = audio[crop + 2..crop + 7].iter().map(|i| i.label).collect();
        assert_eq!(
            labels,
            [
                "Slice to New MIDI Track",
                "Convert Harmony to New MIDI Track",
                "Convert Melody to New MIDI Track",
                "Convert Drums to New MIDI Track",
                "Separate Stems to New Audio Tracks"
            ]
        );
        // The rest of the audio rows are the same as the Arrangement's.
        assert_eq!(audio[crop + 2].command(), Some(SLICE_TO_MIDI));
    }

    // Spec: session-context-menus R3
    #[test]
    fn a_scenes_menu_adds_paste_after_copy_and_a_field_offers_return_to_default() {
        let paste = scene(SceneMenu { paste: true });
        let copy = paste.iter().position(|i| i.row == "C-SCENE-002").unwrap();
        assert_eq!(paste[copy + 1].row, "C-SCENE-015");
        assert_eq!(paste[copy + 1].label, "Paste");
        assert_eq!(scene_field(true)[0].command(), Some("scene.set_tempo"),);
        assert_eq!(
            scene_field(false)[0].command(),
            Some("scene.set_time_signature"),
        );
        assert_eq!(scene_field(true)[0].row, "C-SCENE-016");
    }

    // Spec: session-context-menus R13
    #[test]
    fn the_session_menus_pending_items_name_their_package() {
        let every = [
            slot(SlotMenu {
                paste: true,
                group: true,
                ..SlotMenu::default()
            }),
            session_clip(SessionClipMenu {
                paste: true,
                audio: true,
                ..SessionClipMenu::default()
            }),
            scene(SceneMenu { paste: true }),
            clip_title(),
        ];
        let mut pending = Vec::new();
        for item in every.into_iter().flatten() {
            if matches!(item.action, Pending(_)) {
                assert!(pending_package(item.row).is_some(), "{}", item.row);
                pending.push(item.row);
            }
        }
        // Only the items whose commands still wait for a package.
        assert_eq!(
            pending,
            ["C-CLIP-SES-019b", "C-CLIP-SES-019c", "C-CLIP-SES-019d",]
        );
    }

    // Spec: stem-separation R22
    #[test]
    fn separate_stems_is_in_the_session_clips_and_the_time_selections_menus() {
        let time = |audio| {
            arrangement_time_in(SelectionMenu {
                clips: true,
                audio,
                ..SelectionMenu::default()
            })
        };
        let rows: Vec<_> = time(true).iter().map(|i| i.row).collect();
        let crop = rows.iter().position(|r| *r == "C-TIME-019c").unwrap();
        assert_eq!(rows[crop + 1], "C-TIME-028");
        assert!(!time(false).iter().any(|i| i.row == "C-TIME-028"));
        let session = session_clip(SessionClipMenu {
            audio: true,
            ..SessionClipMenu::default()
        });
        let stems = session.iter().find(|i| i.row == "C-CLIP-SES-020").unwrap();
        assert_eq!(stems.command(), Some("stems.open"));
    }

    // Spec: session-context-menus R16
    #[test]
    fn the_arrangement_clips_export_runs_the_midi_export() {
        let export = arrangement_clip()
            .into_iter()
            .find(|i| i.row == "C-CLIP-ARR-011")
            .unwrap();
        assert_eq!(export.command(), Some("midi.export_clip"));
    }

    // Spec: track-context-menu R10, comping R29
    #[test]
    fn the_track_menu_in_the_arrangement_holds_the_take_lane_items_after_group_tracks() {
        let menu = track(TrackMenu {
            arrangement: true,
            ..TrackMenu::default()
        });
        let rows: Vec<&str> = menu.iter().map(|i| i.row).collect();
        let group = rows.iter().position(|r| *r == "C-TRACK-012").unwrap();
        assert_eq!(
            &rows[group + 1..group + 6],
            [
                "",
                "C-TRACK-019a",
                "C-TRACK-019b",
                "C-TRACK-019c",
                "C-TRACK-019d"
            ]
        );
        let command = |row: &str| menu.iter().find(|i| i.row == row).unwrap().action;
        assert_eq!(command("C-TRACK-019a"), Cmd(SHOW_TAKE_LANES));
        assert_eq!(command("C-TRACK-019b"), Key(&["S-COMP-004"]));
        assert_eq!(command("C-TRACK-019c"), Cmd("take_lane.delete_all"));
        assert_eq!(command("C-TRACK-019d"), Cmd("take_lane.delete_unused"));
    }

    // Spec: comping R29
    #[test]
    fn a_selection_on_a_take_lane_adds_copy_selection_to_main_lane_after_paste_bounced_audio() {
        let on = SelectionMenu {
            clips: true,
            time_commands: false,
            automation: false,
            take_lane: true,
            audio: false,
        };
        for (menu, bounced, own) in [
            (arrangement_clip_in(on), "C-CLIP-ARR-016", R29_COPY_CLIP),
            (arrangement_time_in(on), "C-TIME-011", R29_COPY_TIME),
        ] {
            let at = menu.iter().position(|i| i.row == bounced).unwrap();
            assert_eq!(menu[at].label, "Paste Bounced Audio");
            assert_eq!(menu[at + 1].row, own);
            assert_eq!(menu[at + 1].label, "Copy Selection to Main Lane");
            assert_eq!(menu[at + 1].command(), Some(COPY_TO_MAIN_LANE));
        }
        assert!(
            !arrangement_clip_in(SelectionMenu::default())
                .iter()
                .any(|i| i.row == R29_COPY_CLIP)
        );
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
        // The Clip Markers submenu's items run the shortcuts' commands
        // (piano-roll R37).
        let markers = note().into_iter().find(|i| i.row == "C-NOTE-014").unwrap();
        assert_eq!(markers.items.len(), 4);
        for item in markers.items {
            assert!(matches!(item.action, Key(_)), "{}", item.row);
        }
    }
}

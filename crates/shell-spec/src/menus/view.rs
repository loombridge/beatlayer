//! The View menu.

use super::Action::{Choice, Command as Cmd, Omitted, Shortcut as Key, Submenu};
use super::{Action, D24, IMP_01, IMP_25, IMP_25_CLOSE_WINDOWS, Row, SEPARATOR};
use crate::arrangement::{SHOW_IN_OUT, SHOW_RETURN_TRACKS, SHOW_VOLUME};
use crate::assistant::VIEW_TOGGLE_ASSISTANT;
use crate::clip_properties::ARRANGE_PANELS;
use crate::commands::{
    CLOSE_PLUG_IN_WINDOWS, VIEW_EXPAND_CLIP_VIEW, VIEW_FULL_SCREEN, VIEW_SWITCH_DETAIL_VIEW,
    VIEW_TOGGLE_BROWSER, VIEW_TOGGLE_CLIP_VIEW, VIEW_TOGGLE_DETAIL_AREA, VIEW_TOGGLE_DEVICE_VIEW,
    VIEW_TOGGLE_FULL_HEIGHT_BROWSER, VIEW_TOGGLE_INFO_VIEW, VIEW_TOGGLE_PLUG_IN_WINDOWS,
    VIEW_TOGGLE_SECOND_WINDOW, VIEW_TOGGLE_SESSION_ARRANGEMENT, VIEW_TOGGLE_UNDO_HISTORY,
    VIEW_TUNING_SECTION, VIEW_ZOOM_IN, VIEW_ZOOM_OUT, VIEW_ZOOM_RESET,
};
use crate::file_manager::VIEW_TOGGLE_FILE_MANAGER;
use crate::groove::VIEW_TOGGLE_GROOVE_POOL;
use crate::mixer_view::{
    CROSSFADER as MIXER_CROSSFADER, DEVICES as MIXER_DEVICES, IN_OUT as MIXER_IN_OUT,
    PERFORMANCE_IMPACT as MIXER_PERFORMANCE_IMPACT, RETURN_TRACKS as MIXER_RETURN_TRACKS,
    RETURN_TRACKS_SCROLL as MIXER_RETURN_TRACKS_SCROLL, SCENE_TEMPO as MIXER_SCENE_TEMPO,
    SENDS as MIXER_SENDS, TOGGLE as MIXER_TOGGLE, TRACK_OPTIONS as MIXER_TRACK_OPTIONS,
    VOLUME as MIXER_VOLUME,
};

pub(super) const VIEW: &[Row] = &[
    ("M-VIEW-001", "Full Screen", Cmd(VIEW_FULL_SCREEN)),
    (
        "M-VIEW-002",
        "Second Window",
        Cmd(VIEW_TOGGLE_SECOND_WINDOW),
    ),
    ("M-VIEW-003", "Zoom", Submenu),
    ("M-VIEW-003a", "Zoom In", Cmd(VIEW_ZOOM_IN)),
    ("M-VIEW-003b", "Zoom Out", Cmd(VIEW_ZOOM_OUT)),
    ("M-VIEW-003c", "Reset Zoom to 100%", Cmd(VIEW_ZOOM_RESET)),
    SEPARATOR,
    ("M-VIEW-004", "Info", Cmd(VIEW_TOGGLE_INFO_VIEW)),
    ("M-VIEW-005", "Learn View", Key(&["S-VIEW-018"])),
    (
        "M-VIEW-006",
        "Pack Overview",
        Omitted("Ableton's pack service (D21)"),
    ),
    ("M-VIEW-007", "File Manager", Cmd(VIEW_TOGGLE_FILE_MANAGER)),
    ("M-VIEW-008", "Undo History", Cmd(VIEW_TOGGLE_UNDO_HISTORY)),
    (D24, "Assistant", Cmd(VIEW_TOGGLE_ASSISTANT)),
    SEPARATOR,
    (
        "M-VIEW-009",
        "Toggle Arrangement/Session View",
        Cmd(VIEW_TOGGLE_SESSION_ARRANGEMENT),
    ),
    SEPARATOR,
    ("M-VIEW-010", "Clip View", Cmd(VIEW_TOGGLE_CLIP_VIEW)),
    ("M-VIEW-011", "Device View", Cmd(VIEW_TOGGLE_DEVICE_VIEW)),
    (
        "M-VIEW-012",
        "Toggle Clip/Device View",
        Cmd(VIEW_SWITCH_DETAIL_VIEW),
    ),
    ("M-VIEW-013", "Expand Clip View", Cmd(VIEW_EXPAND_CLIP_VIEW)),
    (
        "M-VIEW-014",
        "Hide Device View",
        Cmd(VIEW_TOGGLE_DETAIL_AREA),
    ),
    SEPARATOR,
    (
        "M-VIEW-015",
        "Arrange Clip View Panels Automatically",
        Choice(ARRANGE_PANELS, r#"{"arrangement":"automatic"}"#),
    ),
    (
        "M-VIEW-016",
        "Arrange Clip View Panels Horizontally",
        Choice(ARRANGE_PANELS, r#"{"arrangement":"horizontal"}"#),
    ),
    (
        "M-VIEW-017",
        "Arrange Clip View Panels Vertically",
        Choice(ARRANGE_PANELS, r#"{"arrangement":"vertical"}"#),
    ),
    SEPARATOR,
    ("M-VIEW-018", "Content Editor", Key(&["S-CLIPMODE-003"])),
    ("M-VIEW-019", "Envelope Editor", Key(&["S-CLIPMODE-004"])),
    ("M-VIEW-020", "Expression Editor", Key(&["S-CLIPMODE-005"])),
    SEPARATOR,
    ("M-VIEW-021", "Browser", Cmd(VIEW_TOGGLE_BROWSER)),
    ("M-VIEW-022", "Groove Pool", Cmd(VIEW_TOGGLE_GROOVE_POOL)),
    ("M-VIEW-023", "Tuning", Cmd(VIEW_TUNING_SECTION)),
    (
        "M-VIEW-024",
        "Full-Height Browser",
        Cmd(VIEW_TOGGLE_FULL_HEIGHT_BROWSER),
    ),
    SEPARATOR,
    ("M-VIEW-025", "Search in Browser", Key(&["S-BRW-006"])),
    ("M-VIEW-026", "Show Similar Files", Key(&["S-BRW-010"])),
    ("M-VIEW-027", "Show Filter View", Key(&["S-BRW-013"])),
    SEPARATOR,
    (
        "M-VIEW-028",
        "Plug-In Windows",
        Cmd(VIEW_TOGGLE_PLUG_IN_WINDOWS),
    ),
    // IMP-25 places it straight after Plug-In Windows, with no key.
    (
        IMP_25_CLOSE_WINDOWS,
        "Close All Plug-in Windows",
        Cmd(CLOSE_PLUG_IN_WINDOWS),
    ),
    ("M-VIEW-029", "Video Window", Key(&["S-VIEW-009"])),
    SEPARATOR,
    (
        "M-VIEW-030",
        "Show the Max for Live Window",
        Omitted(
            "Max for Live is another Ableton product (D21); Beatlayer's extension host may take this place (M9)",
        ),
    ),
    SEPARATOR,
    ("M-VIEW-031", "Mixer", Cmd(MIXER_TOGGLE)),
    ("M-VIEW-032", "Mixer Controls", Submenu),
    ("M-VIEW-032a", "In/Out", Cmd(MIXER_IN_OUT)),
    ("M-VIEW-032b", "Sends", Cmd(MIXER_SENDS)),
    ("M-VIEW-032c", "Volume", Cmd(MIXER_VOLUME)),
    ("M-VIEW-032-sep1", "", Action::Separator),
    ("M-VIEW-032d", "Track Options", Cmd(MIXER_TRACK_OPTIONS)),
    ("M-VIEW-032e", "Crossfader", Cmd(MIXER_CROSSFADER)),
    (
        "M-VIEW-032f",
        "Performance Impact",
        Cmd(MIXER_PERFORMANCE_IMPACT),
    ),
    (IMP_25, "Devices", Cmd(MIXER_DEVICES)),
    ("M-VIEW-032-sep2", "", Action::Separator),
    ("M-VIEW-032g", "Return Tracks", Cmd(MIXER_RETURN_TRACKS)),
    // IMP-01's switch, straight after Return Tracks, with no key.
    (
        IMP_01,
        "Return Tracks Scroll with Tracks",
        Cmd(MIXER_RETURN_TRACKS_SCROLL),
    ),
    ("M-VIEW-033", "Arrangement Track Controls", Submenu),
    ("M-VIEW-033a", "In/Out", Cmd(SHOW_IN_OUT)),
    ("M-VIEW-033b", "Volume", Cmd(SHOW_VOLUME)),
    ("M-VIEW-033c", "Track Options", Cmd(MIXER_TRACK_OPTIONS)),
    ("M-VIEW-033-sep1", "", Action::Separator),
    ("M-VIEW-033d", "Return Tracks", Cmd(SHOW_RETURN_TRACKS)),
    (
        IMP_01,
        "Return Tracks Scroll with Tracks",
        Cmd(MIXER_RETURN_TRACKS_SCROLL),
    ),
    SEPARATOR,
    ("M-VIEW-034", "Overview", Key(&["S-VIEW-011"])),
    (
        "M-VIEW-035",
        "Scene Tempo and Time Signature",
        Cmd(MIXER_SCENE_TEMPO),
    ),
    SEPARATOR,
    ("M-VIEW-036", "Automation Mode", Key(&["S-ENV-001"])),
    ("M-VIEW-037", "Show Grouped Tracks", Key(&["S-ARR-021"])),
    (
        "M-VIEW-038",
        "Show Selected Grouped Track",
        Key(&["S-TRACK-010"]),
    ),
    SEPARATOR,
    ("M-VIEW-039", "Fold to Notes", Key(&["S-NOTE-038"])),
    ("M-VIEW-040", "Fold to Scale", Key(&["S-NOTE-039"])),
    SEPARATOR,
    ("M-VIEW-041", "Show Chains", Key(&["S-DEV-012"])),
    ("M-VIEW-042", "Hide Chains", Key(&["S-DEV-013"])),
    ("M-VIEW-043", "Zoom to Time Selection", Key(&["S-ARR-030"])),
    (
        "M-VIEW-044",
        "Zoom Back from Time Selection",
        Key(&["S-ARR-031"]),
    ),
];

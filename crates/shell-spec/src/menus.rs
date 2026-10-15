//! The menu bar, as data: every menu, item, and separator of
//! the menu reference in Live's order (D16), each item tied to the
//! command it runs.
//!
//! An item reaches its command in one of two ways: directly
//! ([`Action::Command`]), for the shell's own commands and items without a
//! shortcut, or through its shortcut ([`Action::Shortcut`]), as menus.md
//! records it, in which case the keymap names the command for the focused
//! context. Items whose command isn't built yet are kept in place and shown
//! disabled ([`Action::Pending`]). The app turns this into native menus on
//! macOS and Windows and draws it on Linux.
//!
//! Labels are Live's words, with Beatlayer's name where Live names its own
//! product ("New Beatlayer Set", "About Beatlayer", "Read the Beatlayer
//! Manual..."); items for Ableton's services and other products (its
//! website, account, store, pack service, and Max for Live) are left out
//! ([`Action::Omitted`]). Decision D21 records the rule.

use crate::browser::{ADD_FOLDER as BROWSER_ADD_FOLDER, SET_PREVIEW as BROWSER_PREVIEW};
use crate::commands::{
    APP_ABOUT, APP_HIDE, APP_QUIT, APP_SETTINGS, AUDIO_TOGGLE_ENGINE, FOCUS_ARRANGEMENT,
    FOCUS_CONTROL_BAR, FOCUS_NEXT, FOCUS_NEXT_NEIGHBOR, FOCUS_PREVIOUS, FOCUS_PREVIOUS_NEIGHBOR,
    FOCUS_TOGGLE_TAB_NAVIGATION, FOCUS_TOGGLE_WRAP_NAVIGATION, SET_COLLECT_ALL, SET_NEW, SET_OPEN,
    SET_SAVE, SET_SAVE_AS, SET_SAVE_AS_DEFAULT, SET_SAVE_AS_TEMPLATE, VIEW_DRAW_MODE,
    VIEW_FULL_SCREEN, VIEW_TOGGLE_BROWSER, VIEW_TOGGLE_CLIP_VIEW, VIEW_TOGGLE_INFO_VIEW,
    VIEW_TOGGLE_PLUG_IN_WINDOWS, VIEW_TOGGLE_SESSION_ARRANGEMENT, VIEW_ZOOM_IN, VIEW_ZOOM_OUT,
    VIEW_ZOOM_RESET, WINDOW_CLOSE,
};
use crate::mixer_view::{
    FOCUS as MIXER_FOCUS, IN_OUT as MIXER_IN_OUT, RETURN_TRACKS as MIXER_RETURN_TRACKS,
    SENDS as MIXER_SENDS, TOGGLE as MIXER_TOGGLE, VOLUME as MIXER_VOLUME,
};

/// The platform a menu bar is for: macOS has an application menu; Windows
/// and Linux put Settings in Options, Quit in File, and About in Help.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuPlatform {
    /// macOS.
    Mac,
    /// Windows and Linux.
    Other,
}

impl MenuPlatform {
    /// The platform this build runs on.
    #[must_use]
    pub const fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::Mac
        } else {
            Self::Other
        }
    }
}

/// An item the operating system supplies on macOS.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SystemItem {
    /// The Services submenu.
    Services,
    /// Hide Others, which handles its own key (S-FILE-010).
    HideOthers,
    /// Show All.
    ShowAll,
    /// Emoji & Symbols, which macOS adds to the Edit menu by itself, with
    /// its key (S-MENU-006).
    AddedByMacOs,
}

/// What a menu item does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Runs a command.
    Command(&'static str),
    /// Runs whatever the first of these shortcuts runs in the focused
    /// context; disabled if none runs anything there.
    Shortcut(&'static [&'static str]),
    /// Its command isn't built yet: shown, disabled. It may name the
    /// shortcut menus.md records for it, which the item displays.
    Pending(Option<&'static str>),
    /// A submenu: the rows after it whose IDs extend its own (`M-EDIT-013a`
    /// after `M-EDIT-013`).
    Submenu,
    /// Supplied by the operating system.
    System(SystemItem),
    /// Left out: the row names an Ableton service or another Ableton
    /// product (D21). The reason says what Beatlayer does instead.
    Omitted(&'static str),
    /// A line between groups of items, where Live draws one.
    Separator,
}

/// The row of Beatlayer's own Duplicate Track Without Clips (IMP-12),
/// next to Duplicate: an improvement, so not a row of menus.md, which
/// records Live's menus.
pub const IMP_12: &str = "IMP-12";

/// Whether a row is one of Live's, in menus.md, rather than one of
/// Beatlayer's improvements.
#[must_use]
pub fn is_lives(row: &str) -> bool {
    row.is_empty() || row.starts_with("M-")
}

/// One row of menus.md, or of an improvement, as Beatlayer shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuItem {
    /// The row's ID in menus.md, such as `M-FILE-009`; empty for a
    /// separator.
    pub row: &'static str,
    /// The label Beatlayer shows.
    pub label: &'static str,
    /// What choosing it does.
    pub action: Action,
    /// A submenu's items.
    pub items: Vec<MenuItem>,
}

impl MenuItem {
    /// The command this item runs directly, if it does.
    #[must_use]
    pub fn command(&self) -> Option<&'static str> {
        match self.action {
            Action::Command(id) => Some(id),
            _ => None,
        }
    }

    /// Whether the item appears in the menu. A separator appears unless
    /// [`shown`] drops it.
    #[must_use]
    pub fn is_shown(&self) -> bool {
        !matches!(
            self.action,
            Action::Omitted(_) | Action::System(SystemItem::AddedByMacOs)
        )
    }

    /// Whether the item is a separator.
    #[must_use]
    pub fn is_separator(&self) -> bool {
        self.action == Action::Separator
    }
}

/// The items a menu shows, in order: hidden items left out, and with them
/// any separator that would then open or close the menu or follow another.
#[must_use]
pub fn shown(items: &[MenuItem]) -> Vec<&MenuItem> {
    let mut out: Vec<&MenuItem> = Vec::with_capacity(items.len());
    for item in items.iter().filter(|item| item.is_shown()) {
        if item.is_separator() && out.last().is_none_or(|last| last.is_separator()) {
            continue;
        }
        out.push(item);
    }
    while out.last().is_some_and(|last| last.is_separator()) {
        out.pop();
    }
    out
}

/// A menu-bar menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Menu {
    /// Its title, such as `File`.
    pub title: &'static str,
    /// The letter that opens it with Alt on Windows (S-MENU-001), if any.
    pub mnemonic: Option<char>,
    /// Its items in order, submenus nested, separators included.
    pub items: Vec<MenuItem>,
}

impl Menu {
    /// Every item, submenu items and separators included, in order.
    pub fn walk(&self) -> impl Iterator<Item = &MenuItem> {
        fn push<'a>(items: &'a [MenuItem], out: &mut Vec<&'a MenuItem>) {
            for item in items {
                out.push(item);
                push(&item.items, out);
            }
        }
        let mut out = Vec::new();
        push(&self.items, &mut out);
        out.into_iter()
    }
}

use Action::{Command as Cmd, Omitted, Pending, Shortcut as Key, Submenu, System};

pub(crate) type Row = (&'static str, &'static str, Action);

/// A separator row.
pub(crate) const SEPARATOR: Row = ("", "", Action::Separator);

const APP: &[Row] = &[
    ("M-APP-001", "About Beatlayer", Cmd(APP_ABOUT)),
    SEPARATOR,
    ("M-APP-002", "Settings...", Cmd(APP_SETTINGS)),
    SEPARATOR,
    ("M-APP-003", "Services", System(SystemItem::Services)),
    SEPARATOR,
    ("M-APP-004", "Hide Beatlayer", Cmd(APP_HIDE)),
    ("M-APP-005", "Hide Others", System(SystemItem::HideOthers)),
    ("M-APP-006", "Show All", System(SystemItem::ShowAll)),
    SEPARATOR,
    ("M-APP-007", "Quit Beatlayer", Cmd(APP_QUIT)),
];

const FILE: &[Row] = &[
    ("M-FILE-001", "New Beatlayer Set", Cmd(SET_NEW)),
    ("M-FILE-002", "Open Beatlayer Set...", Cmd(SET_OPEN)),
    ("M-FILE-003", "Open Recent Set", Pending(None)),
    ("M-FILE-004", "Close Window", Cmd(WINDOW_CLOSE)),
    SEPARATOR,
    ("M-FILE-005", "Install Pack...", Pending(None)),
    (
        "M-FILE-006",
        "Add Folder to Browser...",
        Cmd(BROWSER_ADD_FOLDER),
    ),
    SEPARATOR,
    ("M-FILE-007", "Manage Files", Pending(None)),
    ("M-FILE-008", "Open Beatlayer Set Backups", Pending(None)),
    SEPARATOR,
    ("M-FILE-009", "Save Beatlayer Set", Cmd(SET_SAVE)),
    ("M-FILE-010", "Save Beatlayer Set As...", Cmd(SET_SAVE_AS)),
    ("M-FILE-011", "Save a Copy...", Pending(None)),
    ("M-FILE-012", "Collect All and Save", Cmd(SET_COLLECT_ALL)),
    (
        "M-FILE-013",
        "Save Beatlayer Set As Template...",
        Cmd(SET_SAVE_AS_TEMPLATE),
    ),
    (
        "M-FILE-014",
        "Save Beatlayer Set As Default Set...",
        Cmd(SET_SAVE_AS_DEFAULT),
    ),
    SEPARATOR,
    (
        "M-FILE-015",
        "Export Audio/Video...",
        Pending(Some("S-FILE-007")),
    ),
    (
        "M-FILE-016",
        "Export MIDI Clip...",
        Pending(Some("S-FILE-008")),
    ),
];

const EDIT: &[Row] = &[
    ("M-EDIT-001", "Undo", Key(&["S-EDIT-006"])),
    ("M-EDIT-002", "Redo", Key(&["S-EDIT-007"])),
    SEPARATOR,
    ("M-EDIT-003", "Cut", Key(&["S-EDIT-001"])),
    ("M-EDIT-004", "Copy", Key(&["S-EDIT-002"])),
    ("M-EDIT-005", "Paste", Key(&["S-EDIT-003"])),
    ("M-EDIT-006", "Duplicate", Key(&["S-EDIT-004"])),
    (
        IMP_12,
        "Duplicate Track Without Clips",
        Cmd(crate::tracks::DUPLICATE_WITHOUT_CLIPS),
    ),
    ("M-EDIT-007", "Delete", Key(&["S-EDIT-005"])),
    SEPARATOR,
    (
        "M-EDIT-008",
        "Deactivate Track",
        Key(&["S-TRACK-016", "S-SES-018", "S-ARR-027"]),
    ),
    SEPARATOR,
    ("M-EDIT-009", "Select All", Key(&["S-EDIT-009"])),
    ("M-EDIT-010", "Invert Selection", Key(&["S-NOTE-034"])),
    ("M-EDIT-011", "Select Loop", Key(&["S-ARR-013"])),
    ("M-EDIT-012", "Loop Selection", Key(&["S-ARR-011"])),
    ("M-EDIT-013", "Clip Markers", Submenu),
    ("M-EDIT-013a", "Set Start Marker", Key(&["S-LOOP-001"])),
    ("M-EDIT-013b", "Set Loop Start", Key(&["S-LOOP-002"])),
    ("M-EDIT-013c", "Set Loop End", Key(&["S-LOOP-003"])),
    ("M-EDIT-013d", "Set End Marker", Key(&["S-LOOP-004"])),
    SEPARATOR,
    (
        "M-EDIT-014",
        "Group Tracks",
        Key(&["S-TRACK-006", "S-DEV-001", "S-NOTE-031"]),
    ),
    (
        "M-EDIT-015",
        "Ungroup Tracks",
        Key(&["S-TRACK-007", "S-DEV-002", "S-NOTE-032"]),
    ),
    SEPARATOR,
    ("M-EDIT-016", "Compare: Switch to B", Key(&["S-DEV-011"])),
    ("M-EDIT-017", "Compare: Copy A to B", Pending(None)),
    SEPARATOR,
    ("M-EDIT-018", "Cut Time", Key(&["S-ARR-015"])),
    ("M-EDIT-019", "Copy Time", Key(&["S-ARR-016"])),
    ("M-EDIT-020", "Paste Clip(s)", Key(&["S-ARR-017"])),
    ("M-EDIT-021", "Duplicate Time", Key(&["S-ARR-018"])),
    ("M-EDIT-022", "Delete Time", Key(&["S-ARR-019"])),
    SEPARATOR,
    (
        "M-EDIT-023",
        "Apply Transformation/Generator",
        Key(&["S-NOTE-033"]),
    ),
    SEPARATOR,
    ("M-EDIT-024", "Rename", Key(&["S-EDIT-008"])),
    ("M-EDIT-025", "Edit Info Text", Pending(None)),
    SEPARATOR,
    ("M-EDIT-026", "Solo Track", Key(&["S-TRACK-014"])),
    ("M-EDIT-027", "Arm Track", Key(&["S-TRACK-013"])),
    SEPARATOR,
    ("M-EDIT-028", "Freeze Track", Key(&["S-TRACK-017"])),
    ("M-EDIT-029", "Bounce Track in Place", Pending(None)),
    ("M-EDIT-030", "Bounce to New Track", Key(&["S-BOUNCE-001"])),
    ("M-EDIT-031", "Paste Bounced Audio", Key(&["S-BOUNCE-002"])),
    SEPARATOR,
    ("M-EDIT-032", "Remove Stop Button", Key(&["S-SES-005"])),
    ("M-EDIT-033", "Consolidate", Key(&["S-ARR-002"])),
    ("M-EDIT-034", "Fit to Time Range", Key(&["S-NOTE-008"])),
    SEPARATOR,
    (
        "M-EDIT-035",
        "Quantize",
        Key(&["S-NOTE-011", "S-SAMPLE-001"]),
    ),
    (
        "M-EDIT-036",
        "Quantize Settings...",
        Key(&["S-NOTE-012", "S-SAMPLE-002"]),
    ),
    ("M-EDIT-037", "Extract Groove(s)", Pending(None)),
    ("M-EDIT-038", "Record Quantization", Submenu),
    ("M-EDIT-038a", "No Quantization", Pending(None)),
    ("M-EDIT-038b", "Quarter-Note Quantization", Pending(None)),
    ("M-EDIT-038c", "Eighth-Note Quantization", Pending(None)),
    (
        "M-EDIT-038d",
        "Eighth-Note Triplets Quantization",
        Pending(None),
    ),
    (
        "M-EDIT-038e",
        "Eighth-Note and Triplets Quantization",
        Pending(None),
    ),
    ("M-EDIT-038f", "Sixteenth-Note Quantization", Pending(None)),
    (
        "M-EDIT-038g",
        "Sixteenth-Note Triplets Quantization",
        Pending(None),
    ),
    (
        "M-EDIT-038h",
        "Sixteenth-Note and Triplets Quantization",
        Pending(None),
    ),
    (
        "M-EDIT-038i",
        "Thirty-Second Note Quantization",
        Pending(None),
    ),
    SEPARATOR,
    ("M-EDIT-039", "Simplify Envelope", Pending(None)),
    SEPARATOR,
    (
        "M-EDIT-040",
        "Emoji & Symbols",
        System(SystemItem::AddedByMacOs),
    ),
];

const CREATE: &[Row] = &[
    ("M-CREATE-001", "Insert Audio Track", Key(&["S-TRACK-001"])),
    ("M-CREATE-002", "Insert MIDI Track", Key(&["S-TRACK-002"])),
    ("M-CREATE-003", "Insert Return Track", Key(&["S-TRACK-003"])),
    ("M-CREATE-004", "Insert Take Lane", Key(&["S-COMP-004"])),
    SEPARATOR,
    ("M-CREATE-005", "Insert Scene", Key(&["S-SES-008"])),
    (
        "M-CREATE-006",
        "Capture and Insert Scene",
        Key(&["S-SES-009"]),
    ),
    (
        "M-CREATE-007",
        "Consolidate Time to New Scene",
        Pending(None),
    ),
    SEPARATOR,
    (
        "M-CREATE-008",
        "Insert Empty MIDI Clip",
        Key(&["S-SES-007", "S-ARR-037"]),
    ),
    ("M-CREATE-009", "Capture MIDI", Pending(None)),
    SEPARATOR,
    ("M-CREATE-010", "Create Fade", Key(&["S-ARR-008"])),
    SEPARATOR,
    ("M-CREATE-011", "Slice to New MIDI Track", Pending(None)),
    (
        "M-CREATE-012",
        "Convert Harmony to New MIDI Track",
        Pending(None),
    ),
    (
        "M-CREATE-013",
        "Convert Melody to New MIDI Track",
        Pending(None),
    ),
    (
        "M-CREATE-014",
        "Convert Drums to New MIDI Track",
        Pending(None),
    ),
    (
        "M-CREATE-015",
        "Separate Stems to New Audio Tracks",
        Pending(None),
    ),
    SEPARATOR,
    ("M-CREATE-016", "Add Locator", Pending(None)),
    ("M-CREATE-017", "Add Time Signature Change", Pending(None)),
    SEPARATOR,
    ("M-CREATE-018", "Import Audio/MIDI File...", Pending(None)),
];

const PLAYBACK: &[Row] = &[
    ("M-PLAY-001", "Play", Key(&["S-TRANS-001"])),
    ("M-PLAY-002", "Continue Playback", Key(&["S-TRANS-002"])),
    ("M-PLAY-003", "Play From Insert Marker", Key(&["S-ARR-032"])),
    (
        "M-PLAY-004",
        "Return Play Position to 1.1.1",
        Cmd(crate::transport::RETURN_TO_START),
    ),
    (
        "M-PLAY-005",
        "Move Insert Marker To Playhead",
        Key(&["S-ARR-033"]),
    ),
    ("M-PLAY-006", "Options", Submenu),
    ("M-PLAY-006a", "Follow", Key(&["S-ZOOM-005"])),
    ("M-PLAY-006b", "Start Transport With Record", Pending(None)),
    (
        "M-PLAY-006c",
        "Start Transport With Tap Tempo",
        Pending(None),
    ),
    ("M-PLAY-006d", "Link Start Stop Sync", Pending(None)),
    SEPARATOR,
    ("M-PLAY-007", "Record to Arrangement", Key(&["S-TRANS-006"])),
    (
        "M-PLAY-008",
        "Arm Recording to Arrangement",
        Key(&["S-TRANS-007"]),
    ),
    ("M-PLAY-009", "Record to Session", Key(&["S-TRANS-008"])),
    SEPARATOR,
    ("M-PLAY-010", "Metronome", Key(&["S-TRANS-011"])),
    (
        "M-PLAY-011",
        "Hear Metronome Only While Recording",
        Cmd(crate::transport::SET_METRONOME_ONLY_WHILE_RECORDING),
    ),
    SEPARATOR,
    ("M-PLAY-012", "Back to Arrangement", Key(&["S-TRANS-009"])),
];

const VIEW: &[Row] = &[
    ("M-VIEW-001", "Full Screen", Cmd(VIEW_FULL_SCREEN)),
    ("M-VIEW-002", "Second Window", Key(&["S-VIEW-002"])),
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
    ("M-VIEW-007", "File Manager", Pending(None)),
    ("M-VIEW-008", "Undo History", Key(&["S-VIEW-021"])),
    SEPARATOR,
    (
        "M-VIEW-009",
        "Toggle Arrangement/Session View",
        Cmd(VIEW_TOGGLE_SESSION_ARRANGEMENT),
    ),
    SEPARATOR,
    ("M-VIEW-010", "Clip View", Cmd(VIEW_TOGGLE_CLIP_VIEW)),
    ("M-VIEW-011", "Device View", Key(&["S-VIEW-016"])),
    (
        "M-VIEW-012",
        "Toggle Clip/Device View",
        Key(&["S-VIEW-004"]),
    ),
    ("M-VIEW-013", "Expand Clip View", Key(&["S-NOTE-030"])),
    ("M-VIEW-014", "Hide Device View", Key(&["S-VIEW-022"])),
    SEPARATOR,
    (
        "M-VIEW-015",
        "Arrange Clip View Panels Automatically",
        Pending(None),
    ),
    (
        "M-VIEW-016",
        "Arrange Clip View Panels Horizontally",
        Pending(None),
    ),
    (
        "M-VIEW-017",
        "Arrange Clip View Panels Vertically",
        Pending(None),
    ),
    SEPARATOR,
    ("M-VIEW-018", "Content Editor", Key(&["S-CLIPMODE-003"])),
    ("M-VIEW-019", "Envelope Editor", Key(&["S-CLIPMODE-004"])),
    ("M-VIEW-020", "Expression Editor", Key(&["S-CLIPMODE-005"])),
    SEPARATOR,
    ("M-VIEW-021", "Browser", Cmd(VIEW_TOGGLE_BROWSER)),
    ("M-VIEW-022", "Groove Pool", Key(&["S-VIEW-017"])),
    ("M-VIEW-023", "Tuning", Pending(None)),
    ("M-VIEW-024", "Full-Height Browser", Pending(None)),
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
    ("M-VIEW-032d", "Track Options", Pending(None)),
    ("M-VIEW-032e", "Crossfader", Pending(None)),
    ("M-VIEW-032f", "Performance Impact", Pending(None)),
    ("M-VIEW-032-sep2", "", Action::Separator),
    ("M-VIEW-032g", "Return Tracks", Cmd(MIXER_RETURN_TRACKS)),
    ("M-VIEW-033", "Arrangement Track Controls", Submenu),
    ("M-VIEW-033a", "In/Out", Pending(None)),
    ("M-VIEW-033b", "Volume", Pending(None)),
    ("M-VIEW-033c", "Track Options", Pending(None)),
    ("M-VIEW-033-sep1", "", Action::Separator),
    ("M-VIEW-033d", "Return Tracks", Pending(None)),
    SEPARATOR,
    ("M-VIEW-034", "Overview", Key(&["S-VIEW-011"])),
    (
        "M-VIEW-035",
        "Scene Tempo and Time Signature",
        Pending(None),
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

const NAVIGATE: &[Row] = &[
    ("M-NAV-001", "Control Bar", Cmd(FOCUS_CONTROL_BAR)),
    ("M-NAV-002", "Session View", Key(&["S-FOCUS-002"])),
    ("M-NAV-003", "Arrangement View", Cmd(FOCUS_ARRANGEMENT)),
    ("M-NAV-004", "Clip View", Key(&["S-FOCUS-004"])),
    ("M-NAV-005", "Device View", Key(&["S-FOCUS-005"])),
    ("M-NAV-006", "Browser", Key(&["S-FOCUS-006"])),
    ("M-NAV-007", "Groove Pool", Key(&["S-FOCUS-007"])),
    ("M-NAV-008", "Learn View", Key(&["S-FOCUS-008"])),
    ("M-NAV-009", "Tuning", Pending(None)),
    SEPARATOR,
    ("M-NAV-010", "Mixer", Cmd(MIXER_FOCUS)),
    ("M-NAV-011", "Clip Panels", Key(&["S-FOCUS-010"])),
    SEPARATOR,
    ("M-NAV-012", "Previous", Cmd(FOCUS_PREVIOUS)),
    ("M-NAV-013", "Next", Cmd(FOCUS_NEXT)),
    // Enabled whenever there's a neighbour, whatever the Tab setting
    // (observed in Live 12.4.6).
    (
        "M-NAV-014",
        "Previous Neighbor",
        Cmd(FOCUS_PREVIOUS_NEIGHBOR),
    ),
    ("M-NAV-015", "Next Neighbor", Cmd(FOCUS_NEXT_NEIGHBOR)),
    SEPARATOR,
    ("M-NAV-016", "Browser History Back", Key(&["S-BRW-011"])),
    ("M-NAV-017", "Browser History Forward", Key(&["S-BRW-012"])),
    SEPARATOR,
    (
        "M-NAV-018",
        "Use Tab Key to Move Focus",
        Cmd(FOCUS_TOGGLE_TAB_NAVIGATION),
    ),
    (
        "M-NAV-019",
        "Wrap Keyboard Navigation",
        Cmd(FOCUS_TOGGLE_WRAP_NAVIGATION),
    ),
];

const OPTIONS: &[Row] = &[
    ("M-OPT-001", "Accessibility", Submenu),
    ("M-OPT-001a", "Speak Menu Commands", Pending(None)),
    (
        "M-OPT-001b",
        "Speak Minimum and Maximum Sliders Values",
        Pending(None),
    ),
    ("M-OPT-001c", "Speak Time in Seconds", Pending(None)),
    ("M-OPT-001d", "Speak Parameter States", Pending(None)),
    (
        "M-OPT-001e",
        "Speak Extended MIDI Note Description",
        Pending(None),
    ),
    SEPARATOR,
    ("M-OPT-002", "Edit MIDI Map", Key(&["S-MAP-001"])),
    ("M-OPT-003", "Edit Key Map", Key(&["S-MAP-002"])),
    ("M-OPT-004", "Computer MIDI Keyboard", Key(&["S-MAP-003"])),
    ("M-OPT-005", "MIDI Note Editor Preview", Pending(None)),
    ("M-OPT-006", "Browser File Preview", Cmd(BROWSER_PREVIEW)),
    SEPARATOR,
    ("M-OPT-007", "External Sync", Pending(None)),
    ("M-OPT-008", "Delay Compensation", Pending(None)),
    (
        "M-OPT-009",
        "Reduced Latency When Monitoring",
        Pending(None),
    ),
    SEPARATOR,
    ("M-OPT-010", "Time Ruler Format", Submenu),
    ("M-OPT-010a", "Time", Pending(None)),
    ("M-OPT-010b", "24 fps (Film)", Pending(None)),
    ("M-OPT-010c", "25 fps (PAL)", Pending(None)),
    ("M-OPT-010d", "30 fps", Pending(None)),
    (
        "M-OPT-010e",
        "29.97 fps (Drop Frame / NTSC-TV)",
        Pending(None),
    ),
    ("M-OPT-010f", "29.97 fps (Non-Drop / NTSC)", Pending(None)),
    SEPARATOR,
    (
        "M-OPT-011",
        "Smaller Launch Quantization",
        Key(&["S-SES-024", "S-GRID-002"]),
    ),
    (
        "M-OPT-012",
        "Larger Launch Quantization",
        Key(&["S-SES-025", "S-GRID-003"]),
    ),
    (
        "M-OPT-013",
        "Triplet Launch Quantization",
        Key(&["S-SES-026", "S-GRID-004"]),
    ),
    (
        "M-OPT-014",
        "Launch Quantization Enabled",
        Key(&["S-SES-027", "S-GRID-005"]),
    ),
    ("M-OPT-015", "Snap Automation to Grid", Pending(None)),
    ("M-OPT-016", "Fixed Grid", Key(&["S-GRID-006"])),
    SEPARATOR,
    ("M-OPT-017", "Draw Mode", Cmd(VIEW_DRAW_MODE)),
    ("M-OPT-018", "Step Input Mode", Key(&["S-NOTE-044"])),
    SEPARATOR,
    ("M-OPT-019", "Highlight Scales", Key(&["S-NOTE-035"])),
    ("M-OPT-020", "Chase MIDI Notes", Pending(None)),
    ("M-OPT-021", "MIDI Envelope Auto-Reset", Pending(None)),
    (
        "M-OPT-022",
        "Retune Set On Loading Tuning Systems",
        Pending(None),
    ),
    SEPARATOR,
    ("M-OPT-023", "Solo Switches", Pending(None)),
    ("M-OPT-024", "Cue Switches", Pending(None)),
    // `bl-model`'s switch, which the shell turns to the other state.
    ("M-OPT-025", "Solo in Place", Cmd("mixer.set_solo_in_place")),
    SEPARATOR,
    ("M-OPT-026", "Lock Envelopes", Pending(None)),
    ("M-OPT-027", "MIDI Arrangement Overdub", Pending(None)),
    SEPARATOR,
    (
        "M-OPT-028",
        "Max for Live Developer Mode",
        Omitted("Max for Live is another Ableton product (D21); see M-VIEW-030"),
    ),
    SEPARATOR,
    ("M-OPT-029", "Audio Engine On", Cmd(AUDIO_TOGGLE_ENGINE)),
];

const HELP: &[Row] = &[
    ("M-HELP-001", "Learn View", Key(&["S-VIEW-018"])),
    (
        "M-HELP-002",
        "Pack Overview",
        Omitted("Ableton's pack service (D21); see M-VIEW-006"),
    ),
    SEPARATOR,
    ("M-HELP-003", "Load Demo Set", Pending(None)),
    ("M-HELP-004", "Read the Beatlayer Manual...", Pending(None)),
    (
        "M-HELP-005",
        "Learn Live at ableton.com...",
        Omitted("Ableton's website (D21); Beatlayer's own site goes in M-HELP-006 and M-HELP-007"),
    ),
    SEPARATOR,
    ("M-HELP-006", "Join the User Forum...", Pending(None)),
    ("M-HELP-007", "Get Support", Pending(None)),
    (
        "M-HELP-008",
        "User Account and Licenses...",
        Omitted("Ableton's account (D21); Beatlayer is open source and has no licence keys"),
    ),
    (
        "M-HELP-009",
        "Purchase License...",
        Omitted("Ableton's store (D21); Beatlayer is free"),
    ),
    SEPARATOR,
    ("M-HELP-010", "Check for Updates...", Pending(None)),
];

/// Builds items from rows, nesting each submenu's rows under it.
pub(crate) fn items(rows: &[Row]) -> Vec<MenuItem> {
    let mut out: Vec<MenuItem> = Vec::new();
    for &(row, label, action) in rows {
        let item = MenuItem {
            row,
            label,
            action,
            items: Vec::new(),
        };
        let parent = out
            .last_mut()
            .filter(|last| last.action == Submenu && !row.is_empty() && row.starts_with(last.row));
        match parent {
            Some(parent) => parent.items.push(item),
            None => out.push(item),
        }
    }
    out
}

/// The row of `rows` with this ID, as an item.
fn take(rows: &[Row], id: &str) -> MenuItem {
    let &(row, label, action) = rows
        .iter()
        .find(|(row, ..)| *row == id)
        .expect("the row exists");
    MenuItem {
        row,
        label,
        action,
        items: Vec::new(),
    }
}

/// The menu bar for a platform, in Live's order: on macOS the application
/// menu first; on Windows and Linux, Settings at the end of Options, Quit
/// at the end of File, and About at the end of Help, where Live's manual
/// puts the first two (their exact place on Windows, and the separators
/// there, are still to observe).
#[must_use]
pub fn menu_bar(platform: MenuPlatform) -> Vec<Menu> {
    let menu = |title, mnemonic, rows: &[Row]| Menu {
        title,
        mnemonic,
        items: items(rows),
    };
    let mut menus = vec![
        menu("File", Some('F'), FILE),
        menu("Edit", Some('E'), EDIT),
        menu("Create", Some('C'), CREATE),
        menu("Playback", Some('P'), PLAYBACK),
        menu("View", Some('V'), VIEW),
        menu("Navigate", Some('N'), NAVIGATE),
        menu("Options", Some('O'), OPTIONS),
        menu("Help", Some('H'), HELP),
    ];
    match platform {
        MenuPlatform::Mac => menus.insert(0, menu("Beatlayer", None, APP)),
        MenuPlatform::Other => {
            menus[0].items.push(take(APP, "M-APP-007"));
            menus[6].items.push(take(APP, "M-APP-002"));
            menus[7].items.push(take(APP, "M-APP-001"));
            for menu in &mut menus {
                menu.items
                    .retain(|item| !matches!(item.action, System(SystemItem::AddedByMacOs)));
            }
        }
    }
    menus
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    /// A menus.md row: its ID and Live's label, or `None` for a separator.
    type Reference = Option<(String, String)>;

    /// Each menu-bar section of menus.md, by heading, with its rows and
    /// separators in order; `None` in a checkout without the planning
    /// documents (they aren't published yet), where the test skips.
    fn reference_menus() -> Option<Vec<(String, Vec<Reference>)>> {
        let docs = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs");
        if !docs.exists() {
            eprintln!(
                "skipped: the UI parity reference isn't published yet, and this checkout doesn't have it"
            );
            return None;
        }
        let text = std::fs::read_to_string(docs.join("ui-parity/menus.md")).unwrap();
        let mut menus: Vec<(String, Vec<Reference>)> = Vec::new();
        for line in text.lines() {
            if let Some(heading) = line.strip_prefix("## ") {
                if heading.starts_with("Context menus") {
                    break;
                }
                menus.push((heading.to_owned(), Vec::new()));
                continue;
            }
            let cells: Vec<&str> = line.split('|').map(str::trim).collect();
            let Some((_, rows)) = menus.last_mut() else {
                continue;
            };
            if cells.len() > 3 && cells[1].starts_with("M-") {
                let label = cells[2].trim_end_matches(" ▸").to_owned();
                rows.push(Some((cells[1].to_owned(), label)));
            } else if cells.len() > 3 && cells[1].is_empty() && cells[2] == "—" {
                rows.push(None);
            }
        }
        menus.retain(|(_, rows)| !rows.is_empty());
        Some(menus)
    }

    #[test]
    fn every_menu_row_and_separator_is_in_the_model_in_order() {
        // Spec: app-shell R15.
        let Some(reference) = reference_menus() else {
            return;
        };
        let bar = menu_bar(MenuPlatform::Mac);
        assert_eq!(bar.len(), reference.len(), "nine menus");
        assert!(
            reference[0].0.starts_with("Live"),
            "the application menu comes first on macOS"
        );
        let mut separators = 0;
        for (menu, (heading, rows)) in bar.iter().zip(&reference) {
            let ours: Vec<&str> = menu
                .walk()
                .filter(|item| is_lives(item.row))
                .map(|item| if item.is_separator() { "—" } else { item.row })
                .collect();
            let theirs: Vec<&str> = rows
                .iter()
                .map(|row| row.as_ref().map_or("—", |(id, _)| id.as_str()))
                .collect();
            assert_eq!(
                ours, theirs,
                "menu {} against menus.md's {heading}",
                menu.title
            );
            separators += ours.iter().filter(|row| **row == "—").count();
        }
        assert_eq!(separators, 65, "README: 65 separators in the menu bar");
    }

    #[test]
    fn labels_are_lives_with_beatlayers_name_and_no_ableton_services() {
        // Spec: app-shell R15.
        // D21: Beatlayer's name where Live names its own product; items for
        // Ableton's services and other products left out; everything else
        // in Live's words.
        let Some(menus) = reference_menus() else {
            return;
        };
        let reference: Vec<(String, String)> = menus
            .into_iter()
            .flat_map(|(_, rows)| rows)
            .flatten()
            .collect();
        let bar = menu_bar(MenuPlatform::Mac);
        let items: Vec<&MenuItem> = bar
            .iter()
            .flat_map(Menu::walk)
            .filter(|i| !i.is_separator() && is_lives(i.row))
            .collect();
        assert_eq!(items.len(), reference.len());
        let mut omitted = Vec::new();
        for (item, (id, live)) in items.iter().zip(&reference) {
            assert_eq!(item.row, id);
            if let Action::Omitted(reason) = item.action {
                assert!(reason.contains("D21"), "{id}: {reason}");
                omitted.push(item.row);
                continue;
            }
            let expected = live
                .split(' ')
                .map(|word| if word == "Live" { "Beatlayer" } else { word })
                .collect::<Vec<_>>()
                .join(" ");
            assert_eq!(item.label, expected, "{id}");
            for word in ["Live", "Ableton", "ableton.com", "Max for"] {
                assert!(!item.label.contains(word), "{id}: {}", item.label);
            }
        }
        assert_eq!(
            omitted,
            [
                "M-VIEW-006",
                "M-VIEW-030",
                "M-OPT-028",
                "M-HELP-002",
                "M-HELP-005",
                "M-HELP-008",
                "M-HELP-009"
            ]
        );
        for (id, live) in &reference {
            if live.contains("Max for Live") || live.contains("ableton.com") {
                assert!(omitted.contains(&id.as_str()), "{id}");
            }
        }
    }

    #[test]
    fn submenus_nest_their_rows() {
        let bar = menu_bar(MenuPlatform::Mac);
        let edit = bar.iter().find(|m| m.title == "Edit").unwrap();
        let markers = edit.items.iter().find(|i| i.row == "M-EDIT-013").unwrap();
        let rows: Vec<&str> = markers.items.iter().map(|i| i.row).collect();
        assert_eq!(
            rows,
            ["M-EDIT-013a", "M-EDIT-013b", "M-EDIT-013c", "M-EDIT-013d"]
        );
        assert!(edit.items.iter().all(|i| i.row != "M-EDIT-013a"));
    }

    #[test]
    fn hidden_items_take_their_spare_separators_with_them() {
        // Spec: app-shell R15.
        for platform in [MenuPlatform::Mac, MenuPlatform::Other] {
            for menu in menu_bar(platform) {
                let items = shown(&menu.items);
                assert!(!items.first().unwrap().is_separator(), "{}", menu.title);
                assert!(!items.last().unwrap().is_separator(), "{}", menu.title);
                for pair in items.windows(2) {
                    assert!(
                        !(pair[0].is_separator() && pair[1].is_separator()),
                        "{}",
                        menu.title
                    );
                }
                assert!(items.iter().all(|item| item.is_shown()));
            }
        }
        // Help loses three of Live's items and keeps its groups.
        let bar = menu_bar(MenuPlatform::Mac);
        let help = bar.iter().find(|m| m.title == "Help").unwrap();
        let rows: Vec<&str> = shown(&help.items)
            .iter()
            .map(|i| if i.is_separator() { "—" } else { i.row })
            .collect();
        assert!(!rows.contains(&"M-HELP-005"));
        assert_eq!(rows.last(), Some(&"M-HELP-010"));
    }

    #[test]
    fn other_platforms_move_the_application_menu_items() {
        let bar = menu_bar(MenuPlatform::Other);
        assert_eq!(bar[0].title, "File");
        assert_eq!(bar[0].items.last().unwrap().command(), Some(APP_QUIT));
        assert_eq!(bar[6].items.last().unwrap().command(), Some(APP_SETTINGS));
        assert_eq!(bar[7].items.last().unwrap().command(), Some(APP_ABOUT));
        assert!(bar.iter().all(|m| m.walk().all(|i| i.row != "M-APP-003")));
        assert!(bar.iter().all(|m| m.walk().all(|i| i.row != "M-EDIT-040")));
    }
}

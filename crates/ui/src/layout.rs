//! The layout layer: the UI parity reference's rows as rectangles
//! (ADR-001, "Layout parity is hand-placed"; D16).
//!
//! Every placed element names the row of the layout reference it
//! implements, and its position comes from that row's measurement in Live
//! 12.4.3 at 1600 × 1000 points. Panes and controls that keep their size in
//! Live keep it here; the rest stretch with the window the way the rows
//! say. Nothing else in the UI decides where a pane or a control goes.
//!
//! Coordinates are points in the content area below the title bar. Live's
//! measurements are from the top of the window frame, title bar included,
//! so they're moved up by [`TITLE_BAR`]. On macOS the window's content
//! covers the whole frame and Beatlayer draws Live's 23-point title bar
//! itself (D22), so the windows lay this out [`TITLE_BAR`] points down and
//! every row lands at layout.md's window coordinates; on Windows and Linux
//! the system's title bar sits above the content area instead.
//!
//! The interface zoom (WP-1.17, spec interface-zoom) scales all of it. The
//! layout is computed in *layout points*, Live's points at 100%, and egui
//! draws a layout point as `zoom` window points through its points-per-pixel
//! ([`apply_zoom`]), so text is rasterised at the zoomed size and stays
//! crisp. A window `zoom` times as large as another shows exactly what the
//! other shows at 100%, every rectangle `zoom` times as large
//! ([`layout_size`], [`ShellLayout::scale`]); its smallest size grows with
//! the zoom ([`minimum_window`]), counting the drawn menu bar where menus
//! are drawn in the window ([`drawn_menu_bar_height`]). Only the drawn
//! title bar keeps its 23 window points, as the system's buttons in it
//! don't scale ([`title_bar_height`]).

use bl_shell::commands::{
    BROWSER_MIN_WIDTH, BROWSER_WIDTH, CLIP_VIEW_HEIGHT, CLIP_VIEW_MIN_HEIGHT, DETAIL_GAP,
    DEVICE_VIEW_HEIGHT, MAIN_VIEW_MIN_HEIGHT, MAIN_VIEW_MIN_WIDTH,
};
use egui::{Pos2, Rect, Vec2, pos2, vec2};

pub mod mixer;

/// Live's title bar height at the reference size (L-WIN-002). layout.md
/// measures from the window's top edge; the content area starts below it.
pub const TITLE_BAR: f32 = 23.0;

/// Inner margin of the drawn menu bar panel (top and bottom).
pub const DRAWN_MENU_MARGIN: f32 = 2.0;

/// The reference window: 1600 × 1000 points with its title bar, so the
/// content area is 1600 × 977.
pub const REFERENCE: Vec2 = vec2(1600.0, 1000.0 - TITLE_BAR);

/// The smallest content area the shell lays out without overlapping, in
/// layout points ([`minimum_window`] scales it by the zoom): the Control
/// Bar's stretching gaps (260 points at the reference width, 20 fewer than
/// Live's since the assistant's toggle takes the Control Bar's far right,
/// D24) close up at 1340 points.
pub const MINIMUM: Vec2 = vec2(1340.0, 640.0);

/// The second window's content area when it first opens, in points with its
/// title bar (L-WIN2-001).
pub const SECOND_WINDOW: Vec2 = vec2(990.0, 703.0);

/// The smallest content area the second window lays out, in layout points
/// ([`minimum_second_window`] scales it by the zoom). Its Control Bar is cut
/// off at its right edge rather than setting a smallest width
/// (interface-zoom R15).
pub const SECOND_MINIMUM: Vec2 = vec2(640.0, 400.0);

/// The margin between panes and at the window's left and right edges
/// (L-WIN-009).
pub const MARGIN: f32 = 9.0;

/// The Undo History panel's width in layout points: the Learn View
/// column's width (L-WIN-008), which the panel takes the place of
/// (L-OTH-001).
pub const UNDO_HISTORY_WIDTH: f32 = 295.0;

/// The Settings window's size with its title bar (L-SET-001).
pub const SETTINGS_WINDOW: Vec2 = vec2(562.0, 617.0);

/// The drawn menu bar's height in layout points, derived from egui's style:
/// its interactive button height ([`egui::Spacing::interact_size`]), its top
/// and bottom inner margins ([`DRAWN_MENU_MARGIN`]), and its panel separator
/// line stroke.
#[must_use]
pub fn drawn_menu_bar_height(style: &egui::Style) -> f32 {
    style.spacing.interact_size.y
        + 2.0 * DRAWN_MENU_MARGIN
        + style.visuals.widgets.noninteractive.bg_stroke.width.round()
}

/// A y position measured in Live's window, as a content-area position.
const fn y(window_y: f32) -> f32 {
    window_y - TITLE_BAR
}

/// The size in layout points of a content area `content` window points
/// across at `zoom` (1 is 100%): what the layout layer lays out.
#[must_use]
pub fn layout_size(content: Vec2, zoom: f32) -> Vec2 {
    content / zoom
}

/// The main window's smallest inner size in window points at `zoom`:
/// [`MINIMUM`] scaled by the zoom, with Live's title bar where Beatlayer
/// draws it (`title_bar`, macOS), which keeps its height at every zoom
/// (interface-zoom R7, R9), and the drawn menu bar's height scaled by the
/// zoom where menus are drawn in the window (`menu_bar`, Linux).
#[must_use]
pub fn minimum_window(zoom: f32, title_bar: bool, menu_bar: bool) -> Vec2 {
    let menu_bar_height = if menu_bar {
        drawn_menu_bar_height(&egui::Style::default()) * zoom
    } else {
        0.0
    };
    MINIMUM * zoom
        + vec2(
            0.0,
            if title_bar { TITLE_BAR } else { 0.0 } + menu_bar_height,
        )
}

/// The second window's smallest inner size in window points at `zoom`:
/// [`SECOND_MINIMUM`] scaled by the zoom, with Live's title bar where
/// Beatlayer draws it (interface-zoom R15).
#[must_use]
pub fn minimum_second_window(zoom: f32, title_bar: bool) -> Vec2 {
    SECOND_MINIMUM * zoom + vec2(0.0, if title_bar { TITLE_BAR } else { 0.0 })
}

/// The second window's minimum and its new inner size when the zoom becomes
/// `zoom`, as [`fit_main_window`] gives the main window's.
#[must_use]
pub fn fit_second_window(
    current: Vec2,
    zoom: f32,
    title_bar: bool,
    screen: Option<Vec2>,
) -> (Vec2, Vec2) {
    let minimum = minimum_second_window(zoom, title_bar);
    let minimum = screen.map_or(minimum, |screen| minimum.min(screen));
    (minimum, current.max(minimum))
}

/// The main window's minimum and its new inner size, in window points, when
/// the zoom becomes `zoom` with the window `current` points inside, on a
/// screen that allows at most `screen` (if it's known): the minimum at that
/// zoom brought within the screen, and the window grown to it where it's
/// smaller, never shrunk (interface-zoom R7). A screen too small for the
/// zoom's minimum gets a window as big as it allows, and the layout keeps
/// its minimum size, its far right and bottom out of view.
#[must_use]
pub fn fit_main_window(
    current: Vec2,
    zoom: f32,
    title_bar: bool,
    menu_bar: bool,
    screen: Option<Vec2>,
) -> (Vec2, Vec2) {
    let minimum = minimum_window(zoom, title_bar, menu_bar);
    let minimum = screen.map_or(minimum, |screen| minimum.min(screen));
    (minimum, current.max(minimum))
}

/// The Settings window's inner size in window points at `zoom`: Live's 562
/// × 617 points at 100% (L-SET-001), its content scaled by the zoom, with
/// Live's title bar where Beatlayer draws it (interface-zoom R8).
#[must_use]
pub fn settings_window(zoom: f32, title_bar: bool) -> Vec2 {
    (SETTINGS_WINDOW - vec2(0.0, TITLE_BAR)) * zoom
        + vec2(0.0, if title_bar { TITLE_BAR } else { 0.0 })
}

/// The drawn title bar's height in layout points at `zoom`: always
/// [`TITLE_BAR`] window points, since the system's window buttons in it
/// don't scale (interface-zoom R9).
#[must_use]
pub fn title_bar_height(zoom: f32) -> f32 {
    TITLE_BAR / zoom
}

/// Sets a window's interface zoom: egui draws each layout point as `zoom`
/// times the screen's points (its points-per-pixel), so text and lines are
/// drawn at the zoomed size rather than stretched. It also turns off egui's
/// own zoom keys: the keymap's View menu keys zoom (S-ZOOM-001, 002), and
/// Cmd-0 is Live's key for launch quantisation (S-GQ-005), so egui's
/// Cmd-plus, Cmd-minus, and Cmd-0 would put the window out of step with the
/// setting (interface-zoom R10).
pub fn apply_zoom(ctx: &egui::Context, zoom: f32) {
    ctx.options_mut(|options| options.zoom_with_keyboard = false);
    ctx.set_zoom_factor(zoom);
}

/// Brings a window's zoom to `zoom` unless a mouse button is down in it,
/// and returns the zoom the window's next frame draws at. The Settings
/// window takes a new zoom this way, so dragging its Zoom setting doesn't
/// rescale the window under the pointer: Live's Settings window changes
/// when the mouse button is released (interface-zoom R4).
pub fn follow_zoom(ctx: &egui::Context, applied: f32, zoom: f32) -> f32 {
    if (applied - zoom).abs() < 1e-6 || ctx.input(|i| i.pointer.any_down()) {
        return applied;
    }
    apply_zoom(ctx, zoom);
    zoom
}

/// One placed row of layout.md: its ID, the part of it this rectangle is,
/// and the rectangle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Row {
    /// The layout.md row, such as `L-CTRL-014`.
    pub id: &'static str,
    /// What the rectangle is, such as `Play`.
    pub name: &'static str,
    /// Where it is.
    pub rect: Rect,
}

impl Row {
    /// The row scaled by `factor` about the content area's corner: at a zoom
    /// of `factor`, in window points.
    #[must_use]
    pub fn scale(self, factor: f32) -> Self {
        Self {
            rect: scale_rect(self.rect, factor),
            ..self
        }
    }
}

const fn row(id: &'static str, name: &'static str, rect: Rect) -> Row {
    Row { id, name, rect }
}

/// A rectangle scaled about the origin.
fn scale_rect(rect: Rect, factor: f32) -> Rect {
    Rect::from_min_max(
        (rect.min.to_vec2() * factor).to_pos2(),
        (rect.max.to_vec2() * factor).to_pos2(),
    )
}

/// What kind of control an element is: how it's drawn and what it tells
/// assistive technology.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A button that acts once.
    Button,
    /// A switch that's on or off.
    Switch,
    /// A number the person can change.
    Value,
    /// A choice from a list.
    Chooser,
    /// A small arrow that opens a menu.
    MenuArrow,
    /// Text that shows a state.
    Display,
    /// A light that shows activity.
    Indicator,
    /// A level, such as the CPU meter.
    Meter,
}

/// The strip an element sits in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Strip {
    /// The Control Bar along the top (L-WIN-003).
    ControlBar,
    /// The Status Bar along the bottom (L-WIN-007).
    StatusBar,
}

/// How an element moves when the window is wider or narrower than the
/// reference.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Anchor {
    /// Stays put: the groups at the left.
    Left,
    /// Moves by a share of the extra width: the Control Bar's middle groups,
    /// whose gaps "stretch with the window" (layout.md, Control Bar).
    Middle,
    /// Moves with the right edge.
    Right,
}

/// Every element the shell places in the Control Bar and the Status Bar.
/// Each is described by its row in [`ELEMENTS`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Element {
    BrowserToggle,
    BrowserMenu,
    ExternalSync,
    SyncInput,
    SyncOutput,
    ResyncExternalHardware,
    LinkToggle,
    LinkMenu,
    TempoFollower,
    TapTempo,
    Tempo,
    NudgeDown,
    NudgeUp,
    SignatureNumerator,
    SignatureDenominator,
    GlobalAmount,
    Metronome,
    MetronomeMenu,
    Quantization,
    ScaleMode,
    RootNote,
    ScaleName,
    Follow,
    PositionBars,
    PositionBeats,
    PositionSixteenths,
    Play,
    Stop,
    Record,
    Overdub,
    AutomationArm,
    ReEnableAutomation,
    CaptureMidi,
    New,
    SessionRecord,
    LoopStartBars,
    LoopStartBeats,
    LoopStartSixteenths,
    PunchIn,
    Loop,
    PunchOut,
    LoopLengthBars,
    LoopLengthBeats,
    LoopLengthSixteenths,
    DrawMode,
    ComputerKeyboard,
    KeyMap,
    KeyIndicators,
    MidiMap,
    SampleRate,
    Cpu,
    CpuMenu,
    Overload,
    RemoteIndicators,
    MidiIndicators,
    SessionView,
    ArrangementView,
    AssistantToggle,
    InfoToggle,
    Message,
    ClipViewToggle,
    ClipSelector,
    DeviceViewToggle,
    DeviceSelector,
    MixerToggle,
    MixerMenu,
}

/// Where an element goes and what it is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    /// The element.
    pub element: Element,
    /// The layout.md row it implements, such as `L-CTRL-014`.
    pub row: &'static str,
    /// Its accessible name, in Live's words.
    pub name: &'static str,
    /// Its strip.
    pub strip: Strip,
    /// Its left edge at the reference width, from layout.md.
    pub x: f32,
    /// Its width, which doesn't change with the window.
    pub width: f32,
    /// How it moves with the window.
    pub anchor: Anchor,
    /// What it is.
    pub kind: Kind,
    /// The package or milestone that brings it to life, if the shell only
    /// draws it. Such an element is disabled and says so to assistive
    /// technology.
    pub pending: Option<&'static str>,
}

#[expect(
    clippy::too_many_arguments,
    reason = "one row of the placement table, field by field"
)]
const fn p(
    element: Element,
    row: &'static str,
    name: &'static str,
    strip: Strip,
    x: f32,
    width: f32,
    anchor: Anchor,
    kind: Kind,
    pending: Option<&'static str>,
) -> Placement {
    Placement {
        element,
        row,
        name,
        strip,
        x,
        width,
        anchor,
        kind,
        pending,
    }
}

use Anchor::{Left, Middle, Right};
use Element as E;
use Kind::{Button, Chooser, Display, Indicator, MenuArrow, Meter, Switch, Value};
use Strip::{ControlBar as CB, StatusBar as SB};

/// The row of Beatlayer's own assistant: the Control Bar's toggle and the
/// pane (D24). Live has no assistant, so these are no rows of layout.md.
pub const D24: &str = "D24";

/// Every element, in on-screen order, with its row and its measurement in
/// Live at 1600 × 1000 points (layout.md, Control Bar and Status Bar), except
/// that the assistant's toggle (D24) takes the Control Bar's far right, so
/// the right group, the view selector included, sits 20 points (the toggle's
/// width and a gap) left of Live's measurements. The
/// Tempo Follower toggle and the Global Groove Amount (L-CTRL-003, 008) are
/// hidden in Live's default state and show when their settings say so.
/// Widths not measured in layout.md run to the
/// next element's left edge, less a 2-point gap. The transport section
/// (Tempo, the time signature, the Arrangement Position, Play, Stop, the
/// loop fields, and the Loop switch) works from WP-1.18
/// (the control bar transport spec), and so does Tap Tempo (WP-7.31).
#[rustfmt::skip]
pub const ELEMENTS: &[Placement] = &[
    p(E::BrowserToggle, "L-CTRL-001", "Show/Hide Browser", CB, 9.0, 18.0, Left, Switch, None),
    p(E::BrowserMenu, "L-CTRL-001", "Browser Config Menu", CB, 27.0, 8.0, Left, MenuArrow, None),
    p(E::ExternalSync, "L-CTRL-028", "External Sync", CB, 54.0, EXTERNAL_SYNC_WIDTH, Left, Switch, None),
    p(E::SyncInput, "L-CTRL-028", "Sync Input", CB, 54.0, SYNC_LAMPS_WIDTH, Left, Indicator, None),
    p(E::SyncOutput, "L-CTRL-028", "Sync Output", CB, 54.0, SYNC_LAMPS_WIDTH, Left, Indicator, None),
    p(E::ResyncExternalHardware, "L-CTRL-029", "Resync External Hardware", CB, 54.0, RESYNC_WIDTH, Left, Button, None),
    p(E::LinkToggle, "L-CTRL-002", "Link", CB, 54.0, LINK_WIDTH, Left, Switch, None),
    p(E::LinkMenu, "L-CTRL-002", "Link Options", CB, 54.0, LINK_MENU_WIDTH, Left, MenuArrow, None),
    p(E::TempoFollower, "L-CTRL-003", "Tempo Follower", CB, 54.0, FOLLOW_WIDTH, Left, Switch, None),
    p(E::TapTempo, "L-CTRL-004", "Tap Tempo", CB, 54.0, 28.0, Left, Button, None),
    p(E::Tempo, "L-CTRL-005", "Tempo", CB, 82.0, 44.0, Left, Value, None),
    p(E::NudgeDown, "L-CTRL-006", "Phase Nudge Down", CB, 126.0, 18.0, Left, Switch, None),
    p(E::NudgeUp, "L-CTRL-006", "Phase Nudge Up", CB, 145.0, 18.0, Left, Switch, None),
    p(E::SignatureNumerator, "L-CTRL-007", "Time Signature Numerator", CB, 167.0, 20.0, Left, Value, None),
    p(E::SignatureDenominator, "L-CTRL-007", "Time Signature Denominator", CB, 189.0, 18.0, Left, Value, None),
    p(E::GlobalAmount, "L-CTRL-008", "Global Amount", CB, 209.0, GLOBAL_AMOUNT_WIDTH, Left, Value, None),
    p(E::Metronome, "L-CTRL-009", "Metronome", CB, 209.0, 26.0, Left, Switch, None),
    p(E::MetronomeMenu, "L-CTRL-009", "Metronome Settings", CB, 237.0, 10.0, Left, MenuArrow, None),
    p(E::Quantization, "L-CTRL-010", "Quantization Menu", CB, 250.0, 45.0, Left, Chooser, Some("M3")),
    p(E::ScaleMode, "L-CTRL-011", "Scale Mode", CB, 314.0, 18.0, Left, Switch, None),
    p(E::RootNote, "L-CTRL-011", "Root Note", CB, 335.0, 44.0, Left, Chooser, None),
    p(E::ScaleName, "L-CTRL-011", "Scale Name", CB, 381.0, 71.0, Left, Chooser, None),
    p(E::Follow, "L-CTRL-012", "Follow", CB, 555.0, 20.0, Middle, Switch, None),
    p(E::PositionBars, "L-CTRL-013", "Arrangement Position Bars", CB, 582.0, 26.0, Middle, Value, None),
    p(E::PositionBeats, "L-CTRL-013", "Arrangement Position Beats", CB, 608.0, 25.0, Middle, Value, None),
    p(E::PositionSixteenths, "L-CTRL-013", "Arrangement Position Sixteenths", CB, 633.0, 26.0, Middle, Value, None),
    p(E::Play, "L-CTRL-014", "Play", CB, 678.0, 18.0, Middle, Button, None),
    p(E::Stop, "L-CTRL-014", "Stop", CB, 701.0, 18.0, Middle, Button, None),
    p(E::Record, "L-CTRL-014", "Arrangement Record", CB, 725.0, 18.0, Middle, Switch, Some("WP-2.13")),
    p(E::Overdub, "L-CTRL-015", "MIDI Arrangement Overdub", CB, 758.0, 18.0, Middle, Switch, Some("WP-2.14")),
    p(E::AutomationArm, "L-CTRL-015", "Automation Arm", CB, 778.0, 16.0, Middle, Switch, None),
    p(E::ReEnableAutomation, "L-CTRL-015", "Re-Enable Automation", CB, 796.0, 18.0, Middle, Button, None),
    p(E::CaptureMidi, "L-CTRL-015", "Capture MIDI", CB, 816.0, 16.0, Middle, Button, None),
    p(E::New, "L-CTRL-031", "New", CB, 834.0, NEW_WIDTH, Middle, Button, None),
    p(E::SessionRecord, "L-CTRL-015", "Session Record", CB, 834.0, 18.0, Middle, Switch, Some("M3")),
    p(E::LoopStartBars, "L-CTRL-016", "Loop Start Bars", CB, 875.0, 26.0, Middle, Value, None),
    p(E::LoopStartBeats, "L-CTRL-016", "Loop Start Beats", CB, 901.0, 26.0, Middle, Value, None),
    p(E::LoopStartSixteenths, "L-CTRL-016", "Loop Start Sixteenths", CB, 927.0, 26.0, Middle, Value, None),
    p(E::PunchIn, "L-CTRL-017", "Punch-In", CB, 956.0, 21.0, Middle, Switch, None),
    p(E::Loop, "L-CTRL-017", "Arrangement Loop", CB, 979.0, 21.0, Middle, Switch, None),
    p(E::PunchOut, "L-CTRL-017", "Punch-Out", CB, 1002.0, 21.0, Middle, Switch, None),
    p(E::LoopLengthBars, "L-CTRL-018", "Loop Length Bars", CB, 1029.0, 26.0, Middle, Value, None),
    p(E::LoopLengthBeats, "L-CTRL-018", "Loop Length Beats", CB, 1055.0, 25.0, Middle, Value, None),
    p(E::LoopLengthSixteenths, "L-CTRL-018", "Loop Length Sixteenths", CB, 1080.0, 26.0, Middle, Value, None),
    p(E::DrawMode, "L-CTRL-019", "Draw Mode", CB, 1263.0, 17.0, Right, Switch, None),
    p(E::ComputerKeyboard, "L-CTRL-020", "Computer MIDI Keyboard", CB, 1282.0, 17.0, Right, Switch, None),
    p(E::KeyMap, "L-CTRL-021", "Key Map Mode", CB, 1301.0, 27.0, Right, Switch, None),
    p(E::KeyIndicators, "L-CTRL-021", "Key Input and Output Indicators", CB, 1330.0, 7.0, Right, Indicator, None),
    p(E::MidiMap, "L-CTRL-022", "MIDI Map Mode", CB, 1339.0, 28.0, Right, Switch, None),
    p(E::SampleRate, "L-CTRL-023", "Sample Rate", CB, 1369.0, 50.0, Right, Display, None),
    p(E::Cpu, "L-CTRL-024", "CPU Load", CB, 1419.0, 49.0, Right, Meter, None),
    p(E::CpuMenu, "L-CTRL-024", "CPU Meter Options", CB, 1468.0, 8.0, Right, MenuArrow, Some("M5")),
    p(E::Overload, "L-CTRL-025", "Disk Overload", CB, 1478.0, 18.0, Right, Indicator, Some("WP-1.3")),
    p(E::RemoteIndicators, "L-CTRL-026", "Remote Input and Output Indicators", CB, 1498.0, 7.0, Right, Indicator, None),
    p(E::MidiIndicators, "L-CTRL-026", "MIDI Track Input and Output Indicators", CB, 1507.0, 14.0, Right, Indicator, None),
    p(E::SessionView, "L-CTRL-027", "Session View", CB, 1529.0, 20.0, Right, Switch, None),
    p(E::ArrangementView, "L-CTRL-027", "Arrangement View", CB, 1550.0, 20.0, Right, Switch, None),
    p(E::AssistantToggle, D24, "Show/Hide Assistant", CB, 1572.0, 18.0, Right, Switch, None),
    p(E::InfoToggle, "L-STAT-001", "Show/Hide Info View", SB, 17.0, 16.0, Left, Switch, None),
    p(E::Message, "L-STAT-002", "Status", SB, 44.0, 1196.0, Left, Display, None),
    p(E::ClipViewToggle, "L-STAT-004", "Show/Hide Clip View", SB, 1259.0, 10.0, Right, Switch, None),
    p(E::ClipSelector, "L-STAT-005", "Clip View Selector", SB, 1270.0, 198.0, Right, Display, None),
    p(E::DeviceViewToggle, "L-STAT-006", "Show/Hide Device View", SB, 1486.0, 10.0, Right, Switch, None),
    p(E::DeviceSelector, "L-STAT-007", "Device View Selector", SB, 1497.0, 41.0, Right, Display, None),
    p(E::MixerToggle, "L-STAT-008", "Show/Hide Mixer", SB, 1557.0, 24.0, Right, Switch, None),
    p(E::MixerMenu, "L-STAT-008", "Mixer Config Menu", SB, 1581.0, 10.0, Right, MenuArrow, None),
];

/// The placement of an element.
#[must_use]
pub fn placement(element: Element) -> &'static Placement {
    ELEMENTS
        .iter()
        .find(|p| p.element == element)
        .expect("every element is placed")
}

/// The Global Groove Amount field's width, and how far it moves everything to
/// its right while it shows (L-CTRL-008).
pub const GLOBAL_AMOUNT_WIDTH: f32 = 40.0;

/// The width of the EXT switch (L-CTRL-028), when it shows.
pub const EXTERNAL_SYNC_WIDTH: f32 = 26.0;

/// The width of the sync lamp pair (L-CTRL-028), when it shows.
pub const SYNC_LAMPS_WIDTH: f32 = 7.0;

/// The width of the Resync External Hardware button (L-CTRL-029), when it
/// shows.
pub const RESYNC_WIDTH: f32 = 40.0;

/// The width of the Link toggle (L-CTRL-002), when it shows: Beatlayer's own,
/// as Live's isn't measured. It holds "2 Links".
pub const LINK_WIDTH: f32 = 44.0;

/// The width of the Link menu's arrow (L-CTRL-002), when it shows: the
/// Metronome Settings arrow's.
pub const LINK_MENU_WIDTH: f32 = 10.0;

/// The width of the Tempo Follower toggle (L-CTRL-003), when it shows:
/// Beatlayer's own, as Live's isn't measured. It holds "Follow".
pub const FOLLOW_WIDTH: f32 = 36.0;

/// The width of the New button (L-CTRL-031), when it shows.
pub const NEW_WIDTH: f32 = 28.0;

/// What the New button takes, with the gap after it, from the Control Bar's
/// middle group, which moves what follows it right by this much.
const NEW_SPACE: f32 = NEW_WIDTH + 2.0;

/// The gap after each of those, in points.
const SYNC_GAP: f32 = 2.0;

/// The share of extra width that goes to the gap left of the Control Bar's
/// middle groups: Live's two stretching gaps are 103 points (after the scale
/// controls) and 177 points (before Draw Mode) at the reference width; the
/// second is 157 points here, the assistant's toggle having taken 20 (D24).
const MIDDLE_SHARE: f32 = 103.0 / (103.0 + 157.0);

/// Which of the detail area's views show and how tall the Clip View is,
/// which decide where the detail area, the main view and the browser go
/// (detail-views R7 to R12). A window holds one of its own.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DetailViews {
    /// Whether the Clip View shows.
    pub clip: bool,
    /// Whether the Device View shows.
    pub device: bool,
    /// The Clip View's height, in layout points, as it was set.
    pub clip_height: f32,
    /// Whether the Clip View takes its largest height.
    pub expanded: bool,
    /// Whether the browser runs down to the Status Bar, the detail area
    /// beside it (Full-Height Browser, browser R26).
    pub full_height_browser: bool,
    /// The browser's width as it was set, in layout points (browser R29).
    pub browser_width: f32,
    /// Whether the Control Bar shows the Global Groove Amount field, which
    /// moves what is to its right 40 points right (L-CTRL-008, the grooves
    /// spec R30).
    pub global_amount: bool,
    /// Whether the Control Bar shows the EXT switch with its sync lamp pair
    /// (L-CTRL-028), and the Resync External Hardware button (L-CTRL-029):
    /// they sit at the left of the tempo group and move what is to their
    /// right right by their widths (the MIDI clock spec, R10, R15).
    pub sync: (bool, bool),
    /// Whether the Control Bar shows the Link toggle with its menu's arrow
    /// (L-CTRL-002, the Link spec R17), which sit after the sync items and
    /// move what is to their right right by their widths. The window shows
    /// it by default; a layout built without it has Live's measured
    /// positions, as with the other optional items.
    pub link: bool,
    /// Whether the Control Bar shows the Tempo Follower toggle (L-CTRL-003,
    /// the Tempo Follower spec R2), which sits after the Link toggle, before
    /// Tap, and moves what is to its right right by its width. The window
    /// shows it while Show Tempo Follower Toggle is Show.
    pub follow: bool,
    /// Whether the Control Bar shows the New button (L-CTRL-031), which a
    /// map mode shows just before Session Record and which moves it, and
    /// what follows in the middle group, right (map modes R20).
    pub new_button: bool,
}

impl DetailViews {
    /// What the shell's detail area shows.
    #[must_use]
    pub fn of(detail: &bl_shell::detail::DetailArea) -> Self {
        Self {
            clip: detail.clip_shown(),
            device: detail.device_shown(),
            clip_height: detail.clip_height() as f32,
            expanded: detail.expanded(),
            full_height_browser: false,
            browser_width: BROWSER_WIDTH,
            global_amount: false,
            sync: (false, false),
            link: false,
            follow: false,
            new_button: false,
        }
    }

    /// The same, with the sync lamp pair and the Resync External Hardware
    /// button showing or not.
    #[must_use]
    pub const fn with_sync(mut self, lamps: bool, resync: bool) -> Self {
        self.sync = (lamps, resync);
        self
    }

    /// The same, with the Link toggle showing or not.
    #[must_use]
    pub const fn with_link(mut self, on: bool) -> Self {
        self.link = on;
        self
    }

    /// The same, with the Tempo Follower toggle showing or not.
    #[must_use]
    pub const fn with_follow(mut self, on: bool) -> Self {
        self.follow = on;
        self
    }

    /// The same, with the Global Groove Amount field showing or not.
    #[must_use]
    pub const fn with_global_amount(mut self, on: bool) -> Self {
        self.global_amount = on;
        self
    }

    /// The same, with the New button showing or not.
    #[must_use]
    pub const fn with_new_button(mut self, on: bool) -> Self {
        self.new_button = on;
        self
    }

    /// The same, with the browser running down to the Status Bar or not.
    #[must_use]
    pub const fn with_full_height_browser(mut self, on: bool) -> Self {
        self.full_height_browser = on;
        self
    }

    /// The same, with the browser `width` points wide as it was set.
    #[must_use]
    pub const fn with_browser_width(mut self, width: f32) -> Self {
        self.browser_width = width;
        self
    }
}

impl Default for DetailViews {
    /// A new session: the Device View alone, the Clip View at its own
    /// height.
    fn default() -> Self {
        Self {
            clip: false,
            device: true,
            clip_height: CLIP_VIEW_HEIGHT,
            expanded: false,
            full_height_browser: false,
            browser_width: BROWSER_WIDTH,
            global_amount: false,
            sync: (false, false),
            link: false,
            follow: false,
            new_button: false,
        }
    }
}

/// The rectangles of the main window's panes and controls for a content
/// area of a given size.
#[derive(Clone, Debug, PartialEq)]
pub struct ShellLayout {
    /// The whole content area.
    pub content: Rect,
    /// The Control Bar strip (L-WIN-003).
    pub control_bar: Rect,
    /// The browser column, while it shows (L-WIN-004).
    pub browser: Option<Rect>,
    /// The main view (L-WIN-005).
    pub main_view: Rect,
    /// The detail area (L-WIN-006): the Clip View and the Device View,
    /// stacked when both show, without the Info View. It has no height
    /// while neither shows.
    pub detail: Rect,
    /// The Info View, at the detail area's left, while it shows
    /// (L-INFO-001), as tall as the detail area, or as the Device View
    /// while neither shows (info-view R7).
    pub info_view: Option<Rect>,
    /// The Clip View, right of the Info View (L-CLIP-001), or taking the
    /// whole detail area's width while the Info View hides: alone, or above
    /// the Device View.
    pub clip_view: Option<Rect>,
    /// The Device View, right of the Info View (L-DEV-001), or taking the
    /// whole detail area's width while the Info View hides, at the
    /// detail area's bottom.
    pub device_view: Option<Rect>,
    /// The tallest the Clip View can be in this window, in layout points:
    /// the height that leaves the main view its smallest (R10).
    pub largest_clip_view: f32,
    /// The widest the browser can be in this window, in layout points: the
    /// width that leaves the main view its least (browser R29).
    pub largest_browser: f32,
    /// The Status Bar strip (L-WIN-007).
    pub status_bar: Rect,
    /// The assistant's pane, at the right edge, while it's docked and open
    /// (D24). Like Live's Learn View (L-WIN-008) it runs from the Control
    /// Bar's bottom to the window's bottom, and the main view, the detail
    /// area and the Status Bar stop at its left edge.
    pub assistant: Option<Rect>,
    /// The Undo History panel, in the Learn View's column at the right
    /// edge (L-OTH-001), while it shows. It sits immediately left of the
    /// assistant's pane when both show, and the main view, the detail area
    /// and the Status Bar stop at its left edge.
    pub undo_history: Option<Rect>,
    /// Every element's rectangle, in [`ELEMENTS`] order.
    pub elements: Vec<(Element, Rect)>,
}

impl ShellLayout {
    /// Lays out a content area of `size` points, with or without the
    /// browser, and with or without the Info View. Sizes below [`MINIMUM`]
    /// are laid out as [`MINIMUM`].
    #[must_use]
    pub fn compute(size: Vec2, browser: bool, info_view: bool) -> Self {
        Self::compute_with_assistant(size, browser, info_view, None)
    }

    /// The same, with the assistant's pane docked at the right edge when
    /// `assistant` gives its width in points.
    #[must_use]
    pub fn compute_with_assistant(
        size: Vec2,
        browser: bool,
        info_view: bool,
        assistant: Option<f32>,
    ) -> Self {
        Self::compute_with_panes(size, browser, info_view, assistant, None)
    }

    /// The same, with the Undo History panel in the Learn View's column
    /// when `undo_history` gives its width in points. The panel sits
    /// immediately left of the assistant's pane when both show (L-OTH-001).
    #[must_use]
    pub fn compute_with_panes(
        size: Vec2,
        browser: bool,
        info_view: bool,
        assistant: Option<f32>,
        undo_history: Option<f32>,
    ) -> Self {
        Self::compute_with_detail(
            size,
            browser,
            info_view,
            assistant,
            undo_history,
            DetailViews::default(),
        )
    }

    /// The same, with the detail area's views as `detail` has them.
    #[must_use]
    pub fn compute_with_detail(
        size: Vec2,
        browser: bool,
        info_view: bool,
        assistant: Option<f32>,
        undo_history: Option<f32>,
        detail_views: DetailViews,
    ) -> Self {
        Self::lay_out(
            size,
            false,
            browser,
            info_view,
            assistant,
            undo_history,
            detail_views,
        )
    }

    /// The second window's layout (second-window R2, R17): no Info View, no
    /// assistant and no Undo History, which belong to the main window, and
    /// a Control Bar measured from the window's left edge and cut off at its
    /// right edge, as it is laid out in the main window's smallest width.
    #[must_use]
    pub fn compute_second(size: Vec2, browser: bool, detail_views: DetailViews) -> Self {
        Self::lay_out(size, true, browser, false, None, None, detail_views)
    }

    fn lay_out(
        size: Vec2,
        second: bool,
        browser: bool,
        info_view: bool,
        assistant: Option<f32>,
        undo_history: Option<f32>,
        detail_views: DetailViews,
    ) -> Self {
        let size = size.max(if second { SECOND_MINIMUM } else { MINIMUM });
        let (w, h) = (size.x, size.y);
        let extra = w - REFERENCE.x;
        // The second window's Control Bar is laid out as the main window's
        // is at its smallest width.
        let controls_extra = if second { w.max(MINIMUM.x) } else { w } - REFERENCE.x;
        // The panes at the right edge, from the window's margin inwards:
        // the assistant's, then the Undo History's, 9 points apart.
        let edge = w - MARGIN;
        let assistant_left = assistant.map(|width| edge - width);
        let panel_right = match assistant_left {
            Some(left) => left - MARGIN,
            None => edge,
        };
        let undo_history = undo_history.map(|width| {
            Rect::from_min_max(pos2(panel_right - width, y(61.0)), pos2(panel_right, h))
        });
        // The main view and the detail area stop 9 points left of the
        // leftmost pane, and the Status Bar at its left edge (L-WIN-007,
        // L-WIN-009).
        let (right, status_right) = match (&undo_history, assistant_left) {
            (Some(panel), _) => (panel.left() - MARGIN, panel.left()),
            (None, Some(left)) => (left - MARGIN, left),
            (None, None) => (edge, w),
        };
        let taken = w - status_right;

        // Rows measured from the top keep their place; the detail area and
        // the Status Bar keep their distance from the bottom.
        let control_bar = Rect::from_min_max(Pos2::ZERO, pos2(w, y(61.0)));
        let status_bottom = h - (REFERENCE.y - y(993.0));
        let status_bar = Rect::from_min_max(
            pos2(0.0, status_bottom - 25.0),
            pos2(w - taken, status_bottom),
        );
        let assistant = assistant.map(|width| {
            Rect::from_min_max(
                pos2(w - MARGIN - width, y(61.0)),
                pos2(w - MARGIN, h - (REFERENCE.y - y(988.0))),
            )
        });
        let detail_bottom = status_bar.top() - (y(968.0) - y(962.0));
        // The Device View keeps its height at the bottom and the Clip View
        // sits above it; a window too short for the Clip View's height
        // gives it less, down to its smallest, and below that the main view
        // less (detail-views R8, R10, R12).
        let device_height = if detail_views.device {
            DEVICE_VIEW_HEIGHT
        } else {
            0.0
        };
        let gap = if detail_views.clip && detail_views.device {
            DETAIL_GAP
        } else {
            0.0
        };
        let largest_clip_view =
            (detail_bottom - y(61.0) - MAIN_VIEW_MIN_HEIGHT - DETAIL_GAP - device_height - gap)
                .max(CLIP_VIEW_MIN_HEIGHT);
        let clip_height = match (detail_views.clip, detail_views.expanded) {
            (false, _) => 0.0,
            (true, true) => largest_clip_view,
            (true, false) => detail_views
                .clip_height
                .clamp(CLIP_VIEW_MIN_HEIGHT, largest_clip_view),
        };
        let height = clip_height + gap + device_height;
        // Spec: browser R26. With the Full-Height Browser the detail area
        // starts to the right of the browser's column.
        let full_height = browser && detail_views.full_height_browser;
        // Spec: browser R29. The browser keeps its width as it was set, less
        // in a window too narrow for it, where the main view keeps its
        // least width; never narrower than its narrowest.
        let largest_browser = (right - MAIN_VIEW_MIN_WIDTH - 2.0 * MARGIN).max(BROWSER_MIN_WIDTH);
        let browser_right = MARGIN
            + detail_views
                .browser_width
                .clamp(BROWSER_MIN_WIDTH, largest_browser);
        let detail_left = if full_height {
            browser_right + MARGIN
        } else {
            MARGIN
        };
        let detail = Rect::from_min_max(
            pos2(detail_left, detail_bottom - height),
            pos2(right, detail_bottom),
        );
        // The Info View is as tall as the detail area, or as tall as the
        // Device View while neither view shows (info-view R7).
        let info_view = info_view.then(|| {
            let height = if height > 0.0 {
                height
            } else {
                DEVICE_VIEW_HEIGHT
            };
            Rect::from_min_max(
                pos2(detail.left(), detail_bottom - height),
                pos2(detail.left() + (240.0 - MARGIN), detail_bottom),
            )
        });
        // The main view runs down to the detail area, 8 points above it, or
        // to its bottom edge while neither view shows; the browser stops
        // above the Info View as well (R9).
        let views_bottom = if height > 0.0 {
            detail.top() - DETAIL_GAP
        } else {
            detail_bottom
        };
        let browser_bottom = if full_height {
            detail_bottom
        } else {
            info_view.map_or(views_bottom, |info| info.top() - DETAIL_GAP)
        };
        let browser_rect = browser.then(|| {
            Rect::from_min_max(pos2(MARGIN, y(61.0)), pos2(browser_right, browser_bottom))
        });
        let main_left = browser_rect.map_or(MARGIN, |b| b.right() + MARGIN);
        let main_view = Rect::from_min_max(pos2(main_left, y(61.0)), pos2(right, views_bottom));
        // With the Info View hidden the views take the detail area's whole
        // width.
        let views_left = info_view.map_or(detail.left(), |info| info.right() + 9.0);
        let clip_view = detail_views.clip.then(|| {
            Rect::from_min_max(
                pos2(views_left, detail.top()),
                pos2(right, detail.top() + clip_height),
            )
        });
        let device_view = detail_views.device.then(|| {
            Rect::from_min_max(pos2(views_left, detail_bottom - device_height), detail.max)
        });

        // Spec: grooves R30. With the Global Groove Amount shown, the Control
        // Bar's left group from the Metronome on moves 40 points right, out of
        // the gaps that stretch; the middle group is pushed right by what the
        // first gap is short of. The gaps hold the 40 points only from
        // `MINIMUM` plus 40 up, so a narrower window, and the second
        // window's Control Bar, which is laid out at the minimum, hide the
        // field and keep the Control Bar's order.
        let groove_at = placement(Element::GlobalAmount).x;
        let groove_shown =
            detail_views.global_amount && !second && w >= MINIMUM.x + GLOBAL_AMOUNT_WIDTH;
        let groove_shift = if groove_shown {
            GLOBAL_AMOUNT_WIDTH
        } else {
            0.0
        };
        // Spec: midi-clock R10, R15. The EXT switch with its sync lamp pair
        // and the Resync External Hardware button open the tempo group, and
        // what follows
        // moves right by their widths, in the room the first gap has, as the
        // Global Amount does; a narrower window, and the second window, hide
        // them.
        let (want_lamps, want_resync) = detail_views.sync;
        let ext_width = EXTERNAL_SYNC_WIDTH + SYNC_GAP;
        let lamps_width = ext_width + SYNC_LAMPS_WIDTH + SYNC_GAP;
        let resync_width = RESYNC_WIDTH + SYNC_GAP;
        let wanted = f32::from(u8::from(want_lamps)) * lamps_width
            + f32::from(u8::from(want_resync)) * resync_width;
        let room = w >= MINIMUM.x + groove_shift + wanted;
        let (lamps_shown, resync_shown) = (
            want_lamps && !second && room,
            want_resync && !second && room,
        );
        let sync_shift = f32::from(u8::from(lamps_shown)) * lamps_width
            + f32::from(u8::from(resync_shown)) * resync_width;
        // Spec: link R17. The Link toggle with its arrow follows them, before
        // Tap, in the room the first gap has; a narrower window, and the
        // second window, hide it.
        let link_width = LINK_WIDTH + SYNC_GAP + LINK_MENU_WIDTH + SYNC_GAP;
        let link_shown =
            detail_views.link && !second && w >= MINIMUM.x + groove_shift + wanted + link_width;
        let link_shift = if link_shown { link_width } else { 0.0 };
        // Spec: tempo-follower R2, control-bar-transport R24. The Tempo
        // Follower toggle follows the Link toggle, before Tap, in the same
        // room; a narrower window, and the second window, hide it.
        let follow_width = FOLLOW_WIDTH + SYNC_GAP;
        let follow_shown = detail_views.follow
            && !second
            && w >= MINIMUM.x + groove_shift + wanted + link_shift + follow_width;
        let follow_shift = if follow_shown { follow_width } else { 0.0 };
        let sync_at = placement(Element::TapTempo).x;
        // Spec: map-modes R20. The New button shows in a map mode, before
        // Session Record, out of the room the middle group's gap has; a
        // narrower window, and the second window, hide it.
        let new_shown =
            detail_views.new_button && !second && w >= MINIMUM.x + NEW_SPACE / (1.0 - MIDDLE_SHARE);
        let new_shift = if new_shown { NEW_SPACE } else { 0.0 };
        let new_at = placement(Element::SessionRecord).x;
        let middle_push = {
            let left_end = placement(Element::ScaleName);
            let middle = placement(Element::Follow);
            let gap = (middle.x + controls_extra * MIDDLE_SHARE)
                - (left_end.x
                    + left_end.width
                    + groove_shift
                    + sync_shift
                    + link_shift
                    + follow_shift);
            (-gap).clamp(0.0, groove_shift + sync_shift + link_shift + follow_shift)
        };
        let elements = ELEMENTS
            .iter()
            .map(|placement| {
                let extra = match placement.strip {
                    Strip::ControlBar => controls_extra,
                    Strip::StatusBar => extra,
                };
                let control_bar = placement.strip == Strip::ControlBar;
                let own_sync = matches!(
                    placement.element,
                    Element::ExternalSync
                        | Element::SyncInput
                        | Element::SyncOutput
                        | Element::ResyncExternalHardware
                        | Element::LinkToggle
                        | Element::LinkMenu
                        | Element::TempoFollower
                );
                let shift = match placement.anchor {
                    Left if control_bar && !own_sync => {
                        // The Global Amount field makes the room the groove
                        // shift gives, so only the sync items push it.
                        let groove = if placement.x >= groove_at
                            && placement.element != Element::GlobalAmount
                        {
                            groove_shift
                        } else {
                            0.0
                        };
                        let sync = if placement.x >= sync_at {
                            sync_shift + link_shift + follow_shift
                        } else {
                            0.0
                        };
                        groove + sync
                    }
                    Left => 0.0,
                    Middle if control_bar => {
                        let new = if placement.x >= new_at && placement.element != Element::New {
                            new_shift
                        } else {
                            0.0
                        };
                        extra * MIDDLE_SHARE + middle_push + new
                    }
                    Middle => extra * MIDDLE_SHARE,
                    // The Status Bar's right edge is the pane's left edge.
                    Right if placement.strip == Strip::StatusBar => extra - taken,
                    Right => extra,
                };
                let mut width = placement.width;
                if placement.element == Element::GlobalAmount && !groove_shown {
                    // Hidden while the pool holds no groove, or the Control
                    // Bar has no room for it.
                    width = 0.0;
                }
                if (matches!(
                    placement.element,
                    Element::ExternalSync | Element::SyncInput | Element::SyncOutput
                ) && !lamps_shown)
                    || (placement.element == Element::ResyncExternalHardware && !resync_shown)
                {
                    width = 0.0;
                }
                if matches!(placement.element, Element::LinkToggle | Element::LinkMenu)
                    && !link_shown
                {
                    width = 0.0;
                }
                if placement.element == Element::TempoFollower && !follow_shown {
                    width = 0.0;
                }
                if placement.element == Element::New && !new_shown {
                    width = 0.0;
                }
                if placement.element == Element::Message {
                    // The message area stretches to the Clip View toggle.
                    width += extra - taken;
                }
                let mut x = placement.x + shift;
                if placement.element == Element::ResyncExternalHardware && lamps_shown {
                    x += lamps_width;
                }
                // The Link toggle follows the sync items, its arrow the toggle.
                if matches!(placement.element, Element::LinkToggle | Element::LinkMenu) {
                    x += sync_shift;
                }
                if placement.element == Element::LinkMenu && link_shown {
                    x += LINK_WIDTH + SYNC_GAP;
                }
                // The Tempo Follower toggle follows the Link toggle and its
                // arrow.
                if placement.element == Element::TempoFollower {
                    x += sync_shift + link_shift;
                }
                // The lamps sit beside the EXT switch, one above the other.
                if matches!(placement.element, Element::SyncInput | Element::SyncOutput)
                    && lamps_shown
                {
                    x += ext_width;
                }
                let rect = match placement.strip {
                    Strip::ControlBar
                        if matches!(
                            placement.element,
                            Element::SyncInput | Element::SyncOutput
                        ) =>
                    {
                        let top = if placement.element == Element::SyncInput {
                            28.0
                        } else {
                            38.0
                        };
                        Rect::from_min_size(pos2(x, y(top)), vec2(width, 8.0))
                    }
                    Strip::ControlBar => Rect::from_min_size(pos2(x, y(28.0)), vec2(width, 18.0)),
                    Strip::StatusBar => Rect::from_min_size(
                        pos2(x, status_bar.top() + 4.0),
                        vec2(width, status_bar.height() - 8.0),
                    ),
                };
                (placement.element, rect)
            })
            .collect();

        Self {
            content: Rect::from_min_size(Pos2::ZERO, size),
            control_bar,
            browser: browser_rect,
            main_view,
            detail,
            info_view,
            clip_view,
            device_view,
            largest_clip_view,
            largest_browser,
            status_bar,
            assistant,
            undo_history,
            elements,
        }
    }

    /// Where the detail area's views go, right of the Info View: both
    /// views and the gap between them, or where the Device View would be
    /// while neither shows. The File Manager and the list of the set's
    /// plug-ins take it.
    #[must_use]
    pub fn views(&self) -> Rect {
        let left = self
            .info_view
            .map_or(self.detail.left(), |info| info.right() + MARGIN);
        let top = if self.detail.height() > 0.0 {
            self.detail.top()
        } else {
            self.detail.bottom() - DEVICE_VIEW_HEIGHT
        };
        Rect::from_min_max(pos2(left, top), self.detail.max)
    }

    /// The same layout moved by `offset`, for a content area that doesn't
    /// start at the window's corner (below a drawn menu bar).
    #[must_use]
    pub fn translate(mut self, offset: Vec2) -> Self {
        let move_rect = |rect: &mut Rect| *rect = rect.translate(offset);
        for rect in [
            &mut self.content,
            &mut self.control_bar,
            &mut self.main_view,
            &mut self.detail,
            &mut self.status_bar,
        ] {
            move_rect(rect);
        }
        for rect in [&mut self.clip_view, &mut self.device_view]
            .into_iter()
            .flatten()
        {
            move_rect(rect);
        }
        if let Some(browser) = &mut self.browser {
            move_rect(browser);
        }
        if let Some(assistant) = &mut self.assistant {
            move_rect(assistant);
        }
        if let Some(undo_history) = &mut self.undo_history {
            move_rect(undo_history);
        }
        if let Some(info_view) = &mut self.info_view {
            move_rect(info_view);
        }
        for (_, rect) in &mut self.elements {
            move_rect(rect);
        }
        self
    }

    /// The same layout scaled by `factor` about the content area's corner:
    /// at a zoom of `factor`, where each rectangle is in window points.
    #[must_use]
    pub fn scale(mut self, factor: f32) -> Self {
        let scale = |rect: &mut Rect| *rect = scale_rect(*rect, factor);
        for rect in [
            &mut self.content,
            &mut self.control_bar,
            &mut self.main_view,
            &mut self.detail,
            &mut self.status_bar,
        ] {
            scale(rect);
        }
        for rect in [&mut self.clip_view, &mut self.device_view]
            .into_iter()
            .flatten()
        {
            scale(rect);
        }
        self.largest_clip_view *= factor;
        self.largest_browser *= factor;
        if let Some(browser) = &mut self.browser {
            scale(browser);
        }
        if let Some(assistant) = &mut self.assistant {
            scale(assistant);
        }
        if let Some(undo_history) = &mut self.undo_history {
            scale(undo_history);
        }
        if let Some(info_view) = &mut self.info_view {
            scale(info_view);
        }
        for (_, rect) in &mut self.elements {
            scale(rect);
        }
        self
    }

    /// Every layout.md row this layout places, panes first, then every
    /// element in [`ELEMENTS`] order.
    #[must_use]
    pub fn rows(&self) -> Vec<Row> {
        let mut rows = vec![
            row("L-WIN-003", "Control Bar", self.control_bar),
            row("L-WIN-005", "Main view", self.main_view),
            row("L-WIN-006", "Detail area", self.detail),
            row("L-WIN-007", "Status Bar", self.status_bar),
        ];
        let views = [
            ("L-INFO-001", "Info View", self.info_view),
            ("L-CLIP-001", "Clip View", self.clip_view),
            ("L-DEV-001", "Device View", self.device_view),
        ];
        let mut at = 3;
        for (id, name, rect) in views {
            if let Some(rect) = rect {
                rows.insert(at, row(id, name, rect));
                at += 1;
            }
        }
        if let Some(browser) = self.browser {
            rows.insert(1, row("L-WIN-004", "Browser", browser));
        }
        if let Some(assistant) = self.assistant {
            rows.push(row(D24, "Assistant", assistant));
        }
        if let Some(undo_history) = self.undo_history {
            rows.push(row("L-OTH-001", "Undo History", undo_history));
        }
        rows.extend(
            ELEMENTS
                .iter()
                .zip(&self.elements)
                .map(|(placement, (_, rect))| row(placement.row, placement.name, *rect)),
        );
        rows
    }

    /// An element's rectangle.
    #[must_use]
    pub fn rect(&self, element: Element) -> Rect {
        self.elements
            .iter()
            .find(|(e, _)| *e == element)
            .map(|(_, rect)| *rect)
            .expect("every element is laid out")
    }
}

/// A row's height in the browser's sidebar and content pane. Not measured
/// in Live (browser, "To observe in Live").
pub const BROWSER_ROW: f32 = 17.0;

/// The content pane's least width, which a narrowed browser keeps before
/// its sidebar narrows, as observed in Live 12.4.6 (browser R29).
pub const CONTENT_MIN_WIDTH: f32 = 57.0;

/// The sidebar's narrowest width, which the browser's narrowest leaves it
/// (browser R29).
pub const SIDEBAR_MIN_WIDTH: f32 = BROWSER_MIN_WIDTH - CONTENT_MIN_WIDTH - 1.0;

/// How high the Quick Tags section is, when it shows (L-BRW-012). Live's
/// isn't measured; this is Beatlayer's own.
pub const QUICK_TAGS_HEIGHT: f32 = 62.0;

/// A row of the Tuning section: its name row, each of its five controls
/// and its buttons (L-BRW-014; the tunings spec, R28; the sizes are
/// Beatlayer's own).
pub const TUNING_ROW: f32 = 20.0;

/// How wide the Tag Editor is, when it opens beside the content pane
/// (L-BRW-013). Live's isn't measured; this is Beatlayer's own.
pub const TAG_EDITOR_WIDTH: f32 = 190.0;

/// How high the Filter View is, when it shows: from y 84 to y 204 in the
/// default state (L-BRW-007).
pub const FILTER_VIEW_HEIGHT: f32 = 120.0;

/// The gap under the Filter View: the column header row sits at y 208
/// (L-BRW-009).
pub const FILTER_VIEW_GAP: f32 = 4.0;

/// How high the Results bar is (L-BRW-008). Live's isn't measured; this is
/// Beatlayer's own.
pub const RESULTS_BAR_HEIGHT: f32 = 18.0;

/// Which of the content pane's optional parts show, which decide where the
/// column header row and the list go.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BrowserParts {
    /// Quick Tags, at the pane's bottom (L-BRW-012).
    pub quick_tags: bool,
    /// The Filter View, at the pane's top (L-BRW-007).
    pub filter_view: bool,
    /// The Results bar, under the Filter View, while a search or a filter is
    /// active (L-BRW-008).
    pub results_bar: bool,
    /// The Tuning section, at the pane's bottom (L-BRW-014; the tunings
    /// spec, R28), as many rows of [`TUNING_ROW`] points as it has; none
    /// while it hides.
    pub tuning_rows: u8,
}

/// The browser's parts inside its column (L-BRW rows), from layout.md's
/// measurements in Live 12.4.3 at 1600 × 1000 points. The top row and the
/// sidebar keep their size; the search field and the content pane stretch
/// with the column, which keeps Live's 302 points until it can be widened
/// (I-DRAG-027). With the Filter View shown (R18) the column header row sits
/// under it, at y 208; hidden, at the content pane's top.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BrowserLayout {
    /// The browser column (L-WIN-004).
    pub column: Rect,
    /// Browse Back (L-BRW-001).
    pub back: Rect,
    /// Browse Forward (L-BRW-001).
    pub forward: Rect,
    /// The search field (L-BRW-001).
    pub search: Rect,
    /// The Show Filter View toggle (L-BRW-001).
    pub filter_toggle: Rect,
    /// The Filter View Menu (L-BRW-001).
    pub filter_menu: Rect,
    /// The sidebar (L-BRW-002).
    pub sidebar: Rect,
    /// The content pane (L-BRW-006).
    pub content: Rect,
    /// The Filter View (L-BRW-007), as high as the pane's top part it fills;
    /// no height while it is hidden.
    pub filter_view: Rect,
    /// The Results bar (L-BRW-008); no height while there is no search or
    /// filter.
    pub results_bar: Rect,
    /// The column header row, with Name (L-BRW-009).
    pub header: Rect,
    /// The Content Options menu's button, at the header row's right.
    pub column_menu: Rect,
    /// The item list (L-BRW-010).
    pub list: Rect,
    /// The Quick Tags section, at the content pane's bottom, above the
    /// preview strip (L-BRW-012).
    pub quick_tags: Rect,
    /// The Tuning section, above Quick Tags (L-BRW-014); no height while it
    /// hides.
    pub tuning: Rect,
    /// The preview strip at the content pane's bottom (L-BRW-011).
    pub strip: Rect,
    /// The Preview switch, at the strip's left (L-BRW-011).
    pub preview_toggle: Rect,
    /// The Raw switch, at the strip's right (WP-4.18; spec: browser R18).
    pub raw_toggle: Rect,
    /// The waveform of the file previewed, beside the switch.
    pub preview_wave: Rect,
}

impl BrowserLayout {
    /// Lays the browser out in its column, with Quick Tags hidden.
    #[must_use]
    pub fn compute(column: Rect) -> Self {
        Self::compute_with(column, false)
    }

    /// Lays the browser out in its column, with Quick Tags showing or not
    /// and the Filter View hidden.
    #[must_use]
    pub fn compute_with(column: Rect, quick_tags_shown: bool) -> Self {
        Self::compute_parts(
            column,
            BrowserParts {
                quick_tags: quick_tags_shown,
                ..BrowserParts::default()
            },
        )
    }

    /// Lays the browser out in its column, with these parts showing.
    #[must_use]
    pub fn compute_parts(column: Rect, parts: BrowserParts) -> Self {
        let quick_tags_shown = parts.quick_tags;
        // Offsets from layout.md's window coordinates: the column spans x 9
        // to 311 and y 61 to 766 at the reference size.
        let x = |window_x: f32| column.left() + window_x - 9.0;
        let top = |window_y: f32| column.top() + window_y - 61.0;
        let row = |x0: f32, x1: f32| Rect::from_min_max(pos2(x0, top(62.0)), pos2(x1, top(80.0)));
        let right = column.right();
        let body_top = top(84.0);
        // Spec: browser R29. A narrowed browser narrows its content pane to
        // its least width, then the sidebar, down to its narrowest.
        let sidebar_right = (right - CONTENT_MIN_WIDTH - 1.0)
            .min(x(109.0))
            .max(column.left() + SIDEBAR_MIN_WIDTH);
        let sidebar = Rect::from_min_max(
            pos2(column.left(), body_top),
            pos2(sidebar_right, column.bottom()),
        );
        let content = Rect::from_min_max(
            pos2(sidebar_right + 1.0, body_top),
            pos2(right, column.bottom()),
        );
        // The Filter View fills the pane's top, the Results bar follows, and
        // the column header row and the list move down under them.
        let band = |top: f32, height: f32| {
            Rect::from_min_max(pos2(content.left(), top), pos2(right, top + height))
        };
        let mut below = content.top();
        let filter_view = band(
            below,
            if parts.filter_view {
                FILTER_VIEW_HEIGHT
            } else {
                0.0
            },
        );
        if parts.filter_view {
            below += FILTER_VIEW_HEIGHT + FILTER_VIEW_GAP;
        }
        let results_bar = band(
            below,
            if parts.results_bar {
                RESULTS_BAR_HEIGHT
            } else {
                0.0
            },
        );
        if parts.results_bar {
            below += RESULTS_BAR_HEIGHT;
        }
        let header = Rect::from_min_max(pos2(content.left(), below), pos2(right, below + 19.0));
        let column_menu = Rect::from_min_max(pos2(right - 16.0, header.top()), header.max);
        // The strip from y 744 to the column's bottom (766).
        let strip = Rect::from_min_max(pos2(content.left(), column.bottom() - 22.0), content.max);
        // Quick Tags sits at the content pane's bottom, above the preview
        // strip (L-BRW-012).
        let quick_tags = Rect::from_min_max(
            pos2(content.left(), strip.top() - QUICK_TAGS_HEIGHT),
            pos2(right, strip.top()),
        );
        let above_strip = if quick_tags_shown {
            quick_tags.top()
        } else {
            strip.top()
        };
        let tuning_height = f32::from(parts.tuning_rows) * TUNING_ROW;
        let tuning = Rect::from_min_max(
            pos2(content.left(), above_strip - tuning_height),
            pos2(right, above_strip),
        );
        let list_bottom = tuning.top();
        let list = Rect::from_min_max(
            pos2(content.left(), header.bottom()),
            pos2(right, list_bottom),
        );
        let preview_toggle = Rect::from_min_size(
            pos2(content.left() + 4.0, strip.top() + 4.0),
            vec2(14.0, 14.0),
        );
        let raw_toggle =
            Rect::from_min_size(pos2(right - 30.0, strip.top() + 4.0), vec2(26.0, 14.0));
        let preview_wave = Rect::from_min_max(
            pos2(preview_toggle.right() + 4.0, strip.top() + 2.0),
            pos2(raw_toggle.left() - 4.0, strip.bottom() - 2.0),
        );
        Self {
            column,
            back: row(x(9.0), x(27.0)),
            forward: row(x(29.0), x(47.0)),
            // The search field ends 41 points short of the column's right,
            // and the filter controls keep their place from the right.
            search: row(x(50.0), (right - 41.0).max(x(50.0))),
            filter_toggle: row(right - 36.0, right - 18.0),
            filter_menu: row(right - 14.0, right - 2.0),
            sidebar,
            content,
            filter_view,
            results_bar,
            header,
            column_menu,
            list,
            quick_tags,
            tuning,
            strip,
            preview_toggle,
            raw_toggle,
            preview_wave,
        }
    }
}

/// Where the Tag Editor opens, beside the browser's content pane
/// (L-BRW-013).
#[must_use]
pub fn tag_editor_rect(browser: Rect) -> Rect {
    Rect::from_min_size(
        pos2(browser.right() + 6.0, browser.top()),
        vec2(TAG_EDITOR_WIDTH, browser.height()),
    )
}

/// The search field's width (command palette R2; Beatlayer's own size).
pub const SEARCH_WIDTH: f32 = 480.0;

/// The height of the search field's top row: the scope buttons, the text and
/// Show Only Matches.
pub const SEARCH_HEADER: f32 = 30.0;

/// A row of the search field's list.
pub const SEARCH_ROW: f32 = 22.0;

/// The most rows the search field's list shows before it scrolls.
pub const SEARCH_ROWS: usize = 12;

/// The gap between the main view's top edge and the search field.
const SEARCH_GAP: f32 = 4.0;

/// Where the search field sits (command palette R2, R14; IMP-16 in
/// layout.md): centred at the main view's top edge, 480 points wide, its
/// list under its top row, which `rows` rows of at most 12 fill.
#[must_use]
pub fn search_field_rect(main_view: Rect, rows: usize) -> Rect {
    let list = rows.min(SEARCH_ROWS) as f32 * SEARCH_ROW;
    let width = SEARCH_WIDTH.min(main_view.width());
    Rect::from_min_size(
        pos2(
            main_view.center().x - width / 2.0,
            main_view.top() + SEARCH_GAP,
        ),
        vec2(width, SEARCH_HEADER + list),
    )
}

/// The Arrangement View's track header column (L-ARR-006): 294 points at
/// the right of the main view, whatever the window's width; the lanes take
/// the rest.
pub const ARRANGEMENT_HEADERS: f32 = 294.0;

/// The gap between the track lanes and the header column (L-ARR-005, 006).
pub const ARRANGEMENT_HEADER_GAP: f32 = 2.0;

/// A pinned row's height: each return track and Main under the lanes
/// (L-ARR-014).
pub const PINNED_ROW: f32 = 18.0;

/// The track header column's sub-columns, as offsets from its left edge
/// (L-ARR-006 to 009): the title (fold button and name), In/Out, the mixer
/// controls, and the meter.
pub mod header_columns {
    /// The title: fold button, then the name, in the track's colour.
    pub const TITLE: (f32, f32) = (0.0, 103.0);
    /// In/Out: input type and channel, monitoring, output type and channel.
    pub const IN_OUT: (f32, f32) = (106.0, 179.0);
    /// The mixer: activator, Solo, Arm; volume, pan; sends.
    pub const MIXER: (f32, f32) = (182.0, 280.0);
    /// The track's meter.
    pub const METER: (f32, f32) = (283.0, 294.0);

    /// The title sub-column's width: with the In/Out column hidden, the
    /// title takes its width (arrangement-view R21).
    #[must_use]
    pub const fn title_width(in_out: bool) -> f32 {
        if in_out { TITLE.1 } else { IN_OUT.1 }
    }
}

/// How tall the time-signature lane is (L-ARR-004).
pub const SIGNATURE_LANE: f32 = 11.0;

/// How tall the chord lane is (L-ARR-024).
pub const CHORD_LANE: f32 = 18.0;

/// How tall the bar numbers are, above the time-signature lane inside the
/// beat-time ruler.
const BAR_NUMBERS: f32 = 12.0;

/// The Arrangement View's parts inside the main view, from layout.md's
/// L-ARR rows measured in Live 12.4.3 at 1600 × 1000 points. Rows from the
/// top keep their distance from the main view's top, rows from the bottom
/// from its bottom; the lanes and rulers stretch with the window and the
/// header column keeps its width.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ArrangementLayout {
    /// The whole view: the main view's rectangle.
    pub view: Rect,
    /// The Overview, while it shows (L-ARR-001).
    pub overview: Option<Rect>,
    /// The beat-time ruler, with the loop brace (L-ARR-002).
    pub beat_ruler: Rect,
    /// The scrub area, with the locators (L-ARR-003).
    pub scrub: Rect,
    /// The time-signature marker lane, inside the beat-time ruler between
    /// its bar numbers and the loop brace, while the set has signature
    /// changes (L-ARR-004).
    pub signature_lane: Option<Rect>,
    /// The chord lane, across the lanes' width above the first track, while
    /// it shows (L-ARR-024), and its header in the header column.
    pub chord_lane: Option<Rect>,
    /// The chord lane's header (L-ARR-024).
    pub chord_header: Option<Rect>,
    /// Set Locator (L-ARR-010, first row).
    pub set_locator: Rect,
    /// Automation Mode (L-ARR-010, first row).
    pub automation_mode: Rect,
    /// Lock Envelopes (L-ARR-010, first row).
    pub lock_envelopes: Rect,
    /// Previous Locator, under Set Locator (L-ARR-010, second row).
    pub previous_locator: Rect,
    /// Next Locator, under Set Locator (L-ARR-010, second row).
    pub next_locator: Rect,
    /// The scrolling track lanes (L-ARR-005).
    pub lanes: Rect,
    /// The scrolling track headers beside them (L-ARR-006).
    pub headers: Rect,
    /// The return tracks' and Main's lanes, pinned under the others
    /// (L-ARR-014).
    pub pinned_lanes: Rect,
    /// Their headers.
    pub pinned_headers: Rect,
    /// The grid spacing indicator (L-ARR-015).
    pub grid_indicator: Rect,
    /// The time ruler (L-ARR-016).
    pub time_ruler: Rect,
    /// The waveform zoom toggle (L-ARR-017).
    pub waveform_zoom: Rect,
    /// The Waveform Vertical Zoom Level field (L-ARR-017).
    pub waveform_zoom_level: Rect,
    /// Optimize Height, H (L-ARR-017).
    pub optimize_height: Rect,
    /// Optimize Width, W (L-ARR-017).
    pub optimize_width: Rect,
    /// The Arrangement's mixer, while it shows (L-ARR-018, WP-1.9).
    pub mixer: Option<Rect>,
    /// The bottom the parts fit above: the view's, or the mixer's top.
    fit_bottom: f32,
}

impl ArrangementLayout {
    /// Lays the Arrangement View out in the main view's rectangle `view`,
    /// with or without the Overview, and with `pinned_rows` rows (the
    /// return tracks and Main) pinned under the lanes.
    #[must_use]
    pub fn compute(view: Rect, overview: bool, pinned_rows: usize) -> Self {
        // Offsets from layout.md's window coordinates: the main view starts
        // at y 61 and ends at 766 at the reference size.
        let top = |window_y: f32| view.top() + window_y - 61.0;
        let bottom = |window_y: f32| view.bottom() - (766.0 - window_y);
        // Without the Overview the rulers move up into its place.
        let shift = if overview { 0.0 } else { -20.0 };
        let lanes_right = view.right() - ARRANGEMENT_HEADERS - ARRANGEMENT_HEADER_GAP;
        let headers_left = view.right() - ARRANGEMENT_HEADERS;
        let header_x = |window_x: f32| headers_left + window_x - 1297.0;
        let row = |x0: f32, x1: f32, window_y: f32| {
            Rect::from_min_max(
                pos2(header_x(x0), top(window_y) + shift),
                pos2(header_x(x1), top(window_y) + shift + 16.0),
            )
        };

        let pinned_rows = pinned_rows.max(1);
        // Main sits a point below the returns (B at 706, Main at 725).
        let pinned_bottom = bottom(743.0);
        let gap = if pinned_rows > 1 { 1.0 } else { 0.0 };
        let pinned_top = pinned_bottom - PINNED_ROW * pinned_rows as f32 - gap;
        let lanes_top = top(119.0) + shift;
        let lanes = Rect::from_min_max(
            pos2(view.left(), lanes_top),
            pos2(lanes_right, pinned_top.max(lanes_top)),
        );
        Self {
            view,
            overview: overview.then(|| {
                Rect::from_min_max(pos2(view.left(), top(61.0)), pos2(lanes_right, top(74.0)))
            }),
            beat_ruler: Rect::from_min_max(
                pos2(view.left(), top(81.0) + shift),
                pos2(lanes_right, top(99.0) + shift),
            ),
            scrub: Rect::from_min_max(
                pos2(view.left(), top(99.0) + shift),
                pos2(lanes_right, top(115.0) + shift),
            ),
            signature_lane: None,
            chord_lane: None,
            chord_header: None,
            set_locator: row(1299.0, 1335.0, 82.0),
            automation_mode: row(1340.0, 1356.0, 82.0),
            lock_envelopes: row(1360.0, 1376.0, 82.0),
            previous_locator: row(1299.0, 1316.0, 100.0),
            next_locator: row(1318.0, 1335.0, 100.0),
            lanes,
            headers: Rect::from_min_max(
                pos2(headers_left, lanes.top()),
                pos2(view.right(), lanes.bottom()),
            ),
            pinned_lanes: Rect::from_min_max(
                pos2(view.left(), pinned_top),
                pos2(lanes_right, pinned_bottom),
            ),
            pinned_headers: Rect::from_min_max(
                pos2(headers_left, pinned_top),
                pos2(view.right(), pinned_bottom),
            ),
            grid_indicator: Rect::from_min_max(
                pos2(lanes_right - 30.0, bottom(728.0)),
                pos2(lanes_right - 2.0, bottom(740.0)),
            ),
            time_ruler: Rect::from_min_max(
                pos2(view.left(), bottom(746.0)),
                pos2(lanes_right, bottom(760.0)),
            ),
            waveform_zoom: Rect::from_min_max(
                pos2(header_x(1303.0), bottom(748.0)),
                pos2(header_x(1313.0), bottom(760.0)),
            ),
            waveform_zoom_level: Rect::from_min_max(
                pos2(header_x(1314.0), bottom(748.0)),
                pos2(header_x(1351.0), bottom(760.0)),
            ),
            optimize_height: Rect::from_min_max(
                pos2(header_x(1372.0), bottom(747.0)),
                pos2(header_x(1386.0), bottom(761.0)),
            ),
            optimize_width: Rect::from_min_max(
                pos2(header_x(1390.0), bottom(747.0)),
                pos2(header_x(1404.0), bottom(761.0)),
            ),
            mixer: None,
            fit_bottom: view.bottom(),
        }
        .fitted()
    }

    /// Squeezes every part into the view's height. A view shorter than the
    /// rows above and below the lanes (the mixer's sections take the rest)
    /// would otherwise put them over the mixer and the Control Bar, where
    /// they'd still take the clicks.
    fn fitted(mut self) -> Self {
        let (top, bottom) = (self.view.top(), self.fit_bottom);
        for rect in [
            &mut self.beat_ruler,
            &mut self.scrub,
            &mut self.set_locator,
            &mut self.automation_mode,
            &mut self.lock_envelopes,
            &mut self.previous_locator,
            &mut self.next_locator,
            &mut self.lanes,
            &mut self.headers,
            &mut self.pinned_lanes,
            &mut self.pinned_headers,
            &mut self.grid_indicator,
            &mut self.time_ruler,
            &mut self.waveform_zoom,
            &mut self.waveform_zoom_level,
            &mut self.optimize_height,
            &mut self.optimize_width,
        ]
        .into_iter()
        .chain(self.overview.as_mut())
        .chain(self.signature_lane.as_mut())
        .chain(self.chord_lane.as_mut())
        .chain(self.chord_header.as_mut())
        {
            rect.min.y = rect.min.y.clamp(top, bottom);
            rect.max.y = rect.max.y.clamp(top, bottom);
            rect.min.y = rect.min.y.min(rect.max.y);
        }
        self
    }

    /// The same layout with the time-signature lane showing (L-ARR-004): the
    /// beat-time ruler grows by the lane's height, and the scrub area and the
    /// track lanes move down by it.
    #[must_use]
    pub fn with_signature_lane(mut self) -> Self {
        let top = self.beat_ruler.top() + BAR_NUMBERS;
        self.signature_lane = Some(Rect::from_min_max(
            pos2(self.beat_ruler.left(), top),
            pos2(self.beat_ruler.right(), top + SIGNATURE_LANE),
        ));
        self.beat_ruler.max.y += SIGNATURE_LANE;
        self.scrub = self.scrub.translate(vec2(0.0, SIGNATURE_LANE));
        self.lanes.min.y += SIGNATURE_LANE;
        self.headers.min.y += SIGNATURE_LANE;
        self.fitted()
    }

    /// The same layout with the chord lane showing (L-ARR-024): a row of its
    /// own at the top of the track lanes, under the rulers, the scrub area
    /// and the time-signature lane, which the track lanes and their headers
    /// start under.
    #[must_use]
    pub fn with_chord_lane(mut self) -> Self {
        let top = self.lanes.top();
        self.chord_lane = Some(Rect::from_min_max(
            pos2(self.lanes.left(), top),
            pos2(self.lanes.right(), top + CHORD_LANE),
        ));
        self.chord_header = Some(Rect::from_min_max(
            pos2(self.headers.left(), top),
            pos2(self.headers.right(), top + CHORD_LANE),
        ));
        self.lanes.min.y += CHORD_LANE;
        self.headers.min.y += CHORD_LANE;
        self.fitted()
    }

    /// The same view with its mixer showing (L-ARR-018; the mixer view
    /// spec, R2): the mixer, as tall as its title row and the `sections`
    /// that show, takes the main view's bottom, and the rest of the view
    /// (the rulers, the lanes, the pinned returns and Main, the grid
    /// indicator, the time ruler, and the buttons under the header column)
    /// is laid out above it as without one.
    #[must_use]
    pub fn compute_with_mixer(
        view: Rect,
        overview: bool,
        pinned_rows: usize,
        sections: mixer::Sections,
    ) -> Self {
        let mixer = mixer::arrangement_mixer(view, sections);
        let mut layout = Self::compute(view.with_max_y(mixer.top()), overview, pinned_rows);
        layout.view = view;
        layout.mixer = Some(mixer);
        layout
    }

    /// Every layout.md row this layout places, top to bottom.
    #[must_use]
    pub fn rows(&self) -> Vec<Row> {
        let mut rows = Vec::with_capacity(18);
        if let Some(overview) = self.overview {
            rows.push(row("L-ARR-001", "Overview", overview));
        }
        rows.extend([
            row("L-ARR-002", "Beat-Time Ruler", self.beat_ruler),
            row("L-ARR-003", "Scrub Area", self.scrub),
            row("L-ARR-010", "Set Locator", self.set_locator),
            row("L-ARR-010", "Automation Mode", self.automation_mode),
            row("L-ARR-010", "Lock Envelopes", self.lock_envelopes),
            row("L-ARR-010", "Previous Locator", self.previous_locator),
            row("L-ARR-010", "Next Locator", self.next_locator),
            row("L-ARR-005", "Track Lanes", self.lanes),
            row("L-ARR-006", "Track Headers", self.headers),
            row("L-ARR-014", "Return and Main Lanes", self.pinned_lanes),
            row("L-ARR-014", "Return and Main Headers", self.pinned_headers),
            row("L-ARR-015", "Grid", self.grid_indicator),
            row("L-ARR-016", "Time Ruler", self.time_ruler),
            row("L-ARR-017", "Waveform Zoom", self.waveform_zoom),
            row(
                "L-ARR-017",
                "Waveform Vertical Zoom Level",
                self.waveform_zoom_level,
            ),
            row("L-ARR-017", "Optimize Height", self.optimize_height),
            row("L-ARR-017", "Optimize Width", self.optimize_width),
        ]);
        if let Some(lane) = self.signature_lane {
            rows.push(row("L-ARR-004", "Time Signature Lane", lane));
        }
        if let Some(lane) = self.chord_lane {
            rows.push(row("L-ARR-024", "Chord Lane", lane));
        }
        if let Some(mixer) = self.mixer {
            rows.push(row("L-ARR-018", "Arrangement Mixer", mixer));
        }
        rows
    }

    /// The pinned row at `index` of `count`, lanes and header together: the
    /// returns, then Main a point lower.
    #[must_use]
    pub fn pinned_row(&self, index: usize, count: usize) -> Rect {
        let gap = if count > 1 && index + 1 == count {
            1.0
        } else {
            0.0
        };
        let y = self.pinned_lanes.top() + PINNED_ROW * index as f32 + gap;
        Rect::from_min_max(
            pos2(self.pinned_lanes.left(), y),
            pos2(self.pinned_headers.right(), y + PINNED_ROW),
        )
    }
}

/// The Settings window's layout (L-SET-002, L-SET-003), in its content area.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SettingsLayout {
    /// The page chooser column.
    pub chooser: Rect,
    /// The first page row; the others follow every [`SettingsLayout::ROW`]
    /// points.
    pub first_page: Rect,
    /// Where option labels start.
    pub label_x: f32,
    /// Where option controls start.
    pub control_x: f32,
    /// The page's area, right of the chooser.
    pub page: Rect,
}

impl SettingsLayout {
    /// The height of a page row in the chooser.
    pub const ROW: f32 = 25.0;
    /// The distance between option rows on a page.
    pub const OPTION_ROW: f32 = 22.0;
    /// A switch's width.
    pub const SWITCH: f32 = 54.0;
    /// A disclosure triangle (a port's sync options, midi-clock R1).
    pub const TRIANGLE: f32 = 18.0;
    /// A chooser's width.
    pub const CHOOSER: f32 = 108.0;
    /// An audio device chooser's width.
    pub const DEVICE_CHOOSER: f32 = 193.0;
    /// A number box's width, such as Tone Volume's. Not measured in Live
    /// (layout.md, L-SET-012): a switch's width until it is.
    pub const NUMBER: f32 = 54.0;

    /// Lays out a Settings window content area of `size` points.
    #[must_use]
    pub fn compute(size: Vec2) -> Self {
        let chooser = Rect::from_min_max(Pos2::ZERO, pos2(140.0, size.y));
        Self {
            chooser,
            first_page: Rect::from_min_size(pos2(0.0, y(34.0)), vec2(140.0, Self::ROW)),
            label_x: 154.0,
            control_x: 354.0,
            page: Rect::from_min_max(pos2(140.0, 0.0), size.to_pos2()),
        }
    }

    /// The same layout moved by `offset`, for a content area that doesn't
    /// start at the window's corner (below a drawn title bar).
    #[must_use]
    pub fn translate(self, offset: Vec2) -> Self {
        Self {
            chooser: self.chooser.translate(offset),
            first_page: self.first_page.translate(offset),
            label_x: self.label_x + offset.x,
            control_x: self.control_x + offset.x,
            page: self.page.translate(offset),
        }
    }

    /// The chooser row of the page at `index`.
    #[must_use]
    pub fn page_row(&self, index: usize) -> Rect {
        self.first_page
            .translate(vec2(0.0, Self::ROW * index as f32))
    }

    /// Every layout.md row this layout places: the page chooser and its
    /// `pages` rows (L-SET-002), and the page beside it (L-SET-003).
    #[must_use]
    pub fn rows(&self, pages: usize) -> Vec<Row> {
        let mut rows = vec![row("L-SET-002", "Page chooser", self.chooser)];
        rows.extend((0..pages).map(|index| row("L-SET-002", "Page row", self.page_row(index))));
        rows.push(row("L-SET-003", "Page", self.page));
        rows
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elements_are_in_order_and_do_not_overlap_at_the_reference_or_minimum() {
        for size in [REFERENCE, MINIMUM, vec2(2400.0, 1400.0)] {
            let layout = ShellLayout::compute(size, true, true);
            for strip in [Strip::ControlBar, Strip::StatusBar] {
                let rects: Vec<Rect> = ELEMENTS
                    .iter()
                    .zip(&layout.elements)
                    // The sync lamps sit one above the other, the lower
                    // sharing the upper's place along the bar.
                    .filter(|(p, _)| p.strip == strip && p.element != Element::SyncOutput)
                    .map(|(_, (_, r))| *r)
                    .collect();
                for pair in rects.windows(2) {
                    assert!(
                        pair[0].right() <= pair[1].left() + 0.01,
                        "{size:?}: {:?} then {:?}",
                        pair[0],
                        pair[1]
                    );
                }
                assert!(rects.last().unwrap().right() <= size.x.max(MINIMUM.x) - MARGIN + 0.01);
            }
        }
    }

    // Spec: browser R1.
    #[test]
    fn the_browser_layout_follows_layout_md() {
        let shell = ShellLayout::compute(REFERENCE, true, true).translate(vec2(0.0, TITLE_BAR));
        let column = shell.browser.unwrap();
        // L-WIN-004: x 9 to 311, y 61 to 766.
        assert_eq!(
            (column.left(), column.right(), column.top(), column.bottom()),
            (9.0, 311.0, 61.0, 766.0)
        );
        let browser = BrowserLayout::compute(column);
        // L-BRW-001: Back at 9, Forward at 29, search 50 to 270, the filter
        // toggle at 275 and its menu at 297.
        assert_eq!(browser.back.left(), 9.0);
        assert_eq!(browser.forward.left(), 29.0);
        assert_eq!(
            (browser.search.left(), browser.search.right()),
            (50.0, 270.0)
        );
        assert_eq!(browser.filter_toggle.left(), 275.0);
        assert_eq!(browser.filter_menu.left(), 297.0);
        assert!(browser.search.top() >= 61.0 && browser.search.bottom() <= 82.0);
        // L-BRW-002 and 006: the sidebar 9 to 109 and the content pane 110
        // to 311, from y 84.
        assert_eq!(
            (browser.sidebar.left(), browser.sidebar.right()),
            (9.0, 109.0)
        );
        assert_eq!(
            (browser.content.left(), browser.content.right()),
            (110.0, 311.0)
        );
        assert_eq!(browser.sidebar.top(), 84.0);
        // L-BRW-010 and 011: the list to y 744, the Preview switch at x 114,
        // y 748, the Raw switch at x 281, y 748.
        assert_eq!(browser.list.bottom(), 744.0);
        assert_eq!(browser.preview_toggle.min, pos2(114.0, 748.0));
        assert_eq!(browser.raw_toggle.min, pos2(281.0, 748.0));
    }

    #[test]
    fn hiding_the_browser_widens_the_main_view() {
        let with = ShellLayout::compute(REFERENCE, true, true);
        let without = ShellLayout::compute(REFERENCE, false, true);
        assert_eq!(without.browser, None);
        assert_eq!(without.main_view.left(), MARGIN);
        assert_eq!(without.main_view.right(), with.main_view.right());
    }

    // Spec: info-view R1, R3.
    #[test]
    fn the_info_view_shows_at_the_detail_areas_left() {
        let layout = ShellLayout::compute(REFERENCE, true, true);
        let info = layout.info_view.unwrap();
        // L-INFO-001: the detail area's left, 231 points wide.
        assert_eq!(info.min, layout.detail.min);
        assert_eq!(info.right(), 240.0);
        assert_eq!(info.bottom(), layout.detail.bottom());
        assert_eq!(layout.device_view.unwrap().left(), 249.0);
        assert!(layout.rows().iter().any(|r| r.id == "L-INFO-001"));
    }

    // Spec: info-view R2.
    #[test]
    fn hiding_the_info_view_widens_the_device_view() {
        let shown = ShellLayout::compute(REFERENCE, true, true);
        let hidden = ShellLayout::compute(REFERENCE, true, false);
        assert_eq!(hidden.info_view, None);
        assert_eq!(hidden.device_view.unwrap().left(), hidden.detail.left());
        assert_eq!(
            hidden.device_view.unwrap().right(),
            shown.device_view.unwrap().right()
        );
        assert!(!hidden.rows().iter().any(|r| r.id == "L-INFO-001"));
    }

    fn detail(clip: bool, device: bool) -> DetailViews {
        DetailViews {
            clip,
            device,
            ..DetailViews::default()
        }
    }

    fn with(size: Vec2, browser: bool, info: bool, views: DetailViews) -> ShellLayout {
        ShellLayout::compute_with_detail(size, browser, info, None, None, views)
    }

    /// Whether two rectangles share any area.
    fn overlap(a: Rect, b: Rect) -> bool {
        let both = a.intersect(b);
        both.width() > 0.01 && both.height() > 0.01
    }

    // Spec: detail-views R7.
    #[test]
    fn one_view_alone_sits_at_the_detail_areas_bottom() {
        let device = with(REFERENCE, true, true, detail(false, true));
        assert_eq!(device.clip_view, None);
        assert_eq!(device.detail.height(), 188.0);
        assert_eq!(device.device_view.unwrap().height(), 188.0);
        assert_eq!(device.main_view.bottom(), device.detail.top() - 8.0);
        assert_eq!(device.browser.unwrap().bottom(), device.detail.top() - 8.0);

        let clip = with(REFERENCE, true, true, detail(true, false));
        assert_eq!(clip.device_view, None);
        assert_eq!(clip.detail.height(), 225.0);
        let view = clip.clip_view.unwrap();
        assert_eq!(view.height(), 225.0);
        assert_eq!(view.bottom(), clip.detail.bottom());
        assert_eq!(view.left(), 249.0, "right of the Info View");
        assert_eq!(clip.main_view.bottom(), clip.detail.top() - 8.0);
        assert!(clip.rows().iter().any(|r| r.id == "L-CLIP-001"));
        assert!(!clip.rows().iter().any(|r| r.id == "L-DEV-001"));
    }

    // Spec: detail-views R8, info-view R7.
    #[test]
    fn stacked_views_keep_the_device_view_at_the_bottom() {
        let both = with(REFERENCE, true, true, detail(true, true));
        let (clip, device) = (both.clip_view.unwrap(), both.device_view.unwrap());
        assert_eq!(device.height(), 188.0);
        assert_eq!(device.bottom(), both.detail.bottom());
        assert_eq!(clip.height(), 225.0);
        assert_eq!(clip.bottom() + 8.0, device.top(), "8 points between them");
        assert_eq!(both.detail.height(), 225.0 + 8.0 + 188.0);
        assert_eq!(both.main_view.bottom(), clip.top() - 8.0);
        let info = both.info_view.unwrap();
        assert_eq!(info.top(), clip.top(), "as tall as the detail area");
        assert_eq!(info.bottom(), device.bottom());
    }

    // Spec: detail-views R9, info-view R7.
    #[test]
    fn with_neither_view_the_main_view_runs_to_the_detail_areas_bottom() {
        let none = with(REFERENCE, true, true, detail(false, false));
        assert_eq!((none.clip_view, none.device_view), (None, None));
        assert_eq!(none.detail.height(), 0.0);
        assert_eq!(none.main_view.bottom(), none.detail.bottom());
        let info = none.info_view.unwrap();
        assert_eq!(info.height(), 188.0, "the Device View's height");
        assert_eq!(none.browser.unwrap().bottom(), info.top() - 8.0);
        let no_info = with(REFERENCE, true, false, detail(false, false));
        assert_eq!(no_info.browser.unwrap().bottom(), no_info.detail.bottom());
    }

    // Spec: detail-views R11.
    #[test]
    fn expanding_gives_the_clip_view_the_room_the_main_view_can_spare() {
        let mut views = detail(true, false);
        views.expanded = true;
        let alone = with(REFERENCE, true, true, views);
        assert_eq!(alone.main_view.height(), 80.0);
        assert_eq!(alone.clip_view.unwrap().height(), alone.largest_clip_view);
        views.device = true;
        let stacked = with(REFERENCE, true, true, views);
        assert_eq!(stacked.main_view.height(), 80.0);
        assert_eq!(stacked.device_view.unwrap().height(), 188.0);
        assert!(stacked.largest_clip_view < alone.largest_clip_view);
    }

    // Spec: detail-views R10, R12.
    #[test]
    fn the_clip_views_height_is_squeezed_in_a_short_window() {
        let mut views = detail(true, true);
        views.clip_height = 2000.0;
        let tall = with(REFERENCE, true, true, views);
        assert_eq!(tall.main_view.height(), 80.0);
        let short = with(MINIMUM, true, true, views);
        let clip = short.clip_view.unwrap().height();
        assert!(clip >= 100.0);
        assert!(clip == 100.0 || short.main_view.height() == 80.0);
        views.clip_height = 10.0;
        let small = with(REFERENCE, true, true, views);
        assert_eq!(small.clip_view.unwrap().height(), 100.0);
    }

    // Spec: detail-views R7 to R9, R12.
    #[test]
    fn no_pane_overlaps_another_at_any_zoom_or_size() {
        for (clip, device) in [(true, false), (false, true), (true, true), (false, false)] {
            for expanded in [false, true] {
                for zoom in [0.5_f32, 1.0, 2.0] {
                    for window in [REFERENCE, MINIMUM] {
                        let views = DetailViews {
                            expanded,
                            ..detail(clip, device)
                        };
                        let layout = with(window, true, true, views).scale(zoom);
                        let panes: Vec<Rect> = [
                            Some(layout.main_view),
                            layout.browser,
                            layout.info_view,
                            layout.clip_view,
                            layout.device_view,
                            Some(layout.status_bar),
                        ]
                        .into_iter()
                        .flatten()
                        .collect();
                        for (at, one) in panes.iter().enumerate() {
                            for other in &panes[at + 1..] {
                                assert!(
                                    !overlap(*one, *other),
                                    "{one:?} overlaps {other:?}: {views:?} at {zoom}"
                                );
                            }
                        }
                        // The heights scale with the zoom (R12).
                        if let Some(device) = layout.device_view {
                            assert!((device.height() - 188.0 * zoom).abs() < 0.01);
                        }
                    }
                }
            }
        }
    }
}

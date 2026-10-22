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
    TapTempo,
    Tempo,
    NudgeDown,
    NudgeUp,
    SignatureNumerator,
    SignatureDenominator,
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
/// Link toggle, the Tempo Follower toggle, and the Global Groove Amount
/// (L-CTRL-002, 003, 008) are hidden in Live's default state and come with
/// the packages that build them. Widths not measured in layout.md run to the
/// next element's left edge, less a 2-point gap. The transport section
/// (Tempo, the time signature, the Arrangement Position, Play, Stop, the
/// loop fields, and the Loop switch) works from WP-1.18
/// (the control bar transport spec); Tap Tempo waits for M2.
#[rustfmt::skip]
pub const ELEMENTS: &[Placement] = &[
    p(E::BrowserToggle, "L-CTRL-001", "Show/Hide Browser", CB, 9.0, 18.0, Left, Switch, None),
    p(E::BrowserMenu, "L-CTRL-001", "Browser Config Menu", CB, 27.0, 8.0, Left, MenuArrow, Some("M6")),
    p(E::TapTempo, "L-CTRL-004", "Tap Tempo", CB, 54.0, 28.0, Left, Button, Some("M2")),
    p(E::Tempo, "L-CTRL-005", "Tempo", CB, 82.0, 44.0, Left, Value, None),
    p(E::NudgeDown, "L-CTRL-006", "Phase Nudge Down", CB, 126.0, 18.0, Left, Button, Some("M7")),
    p(E::NudgeUp, "L-CTRL-006", "Phase Nudge Up", CB, 145.0, 18.0, Left, Button, Some("M7")),
    p(E::SignatureNumerator, "L-CTRL-007", "Time Signature Numerator", CB, 167.0, 20.0, Left, Value, None),
    p(E::SignatureDenominator, "L-CTRL-007", "Time Signature Denominator", CB, 189.0, 18.0, Left, Value, None),
    p(E::Metronome, "L-CTRL-009", "Metronome", CB, 209.0, 26.0, Left, Switch, None),
    p(E::MetronomeMenu, "L-CTRL-009", "Metronome Settings", CB, 237.0, 10.0, Left, MenuArrow, None),
    p(E::Quantization, "L-CTRL-010", "Quantization Menu", CB, 250.0, 45.0, Left, Chooser, Some("M3")),
    p(E::ScaleMode, "L-CTRL-011", "Scale Mode", CB, 314.0, 18.0, Left, Switch, Some("M6")),
    p(E::RootNote, "L-CTRL-011", "Root Note", CB, 335.0, 44.0, Left, Chooser, Some("M6")),
    p(E::ScaleName, "L-CTRL-011", "Scale Name", CB, 381.0, 71.0, Left, Chooser, Some("M6")),
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
    p(E::KeyMap, "L-CTRL-021", "Key Map Mode", CB, 1301.0, 27.0, Right, Switch, Some("M7")),
    p(E::KeyIndicators, "L-CTRL-021", "Key Input and Output Indicators", CB, 1330.0, 7.0, Right, Indicator, Some("M7")),
    p(E::MidiMap, "L-CTRL-022", "MIDI Map Mode", CB, 1339.0, 28.0, Right, Switch, Some("M7")),
    p(E::SampleRate, "L-CTRL-023", "Sample Rate", CB, 1369.0, 50.0, Right, Display, None),
    p(E::Cpu, "L-CTRL-024", "CPU Load", CB, 1419.0, 49.0, Right, Meter, None),
    p(E::CpuMenu, "L-CTRL-024", "CPU Meter Options", CB, 1468.0, 8.0, Right, MenuArrow, Some("M5")),
    p(E::Overload, "L-CTRL-025", "Disk Overload", CB, 1478.0, 18.0, Right, Indicator, Some("WP-1.3")),
    p(E::MidiIndicators, "L-CTRL-026", "MIDI Track Input and Output Indicators", CB, 1507.0, 14.0, Right, Indicator, None),
    p(E::SessionView, "L-CTRL-027", "Session View", CB, 1529.0, 20.0, Right, Switch, None),
    p(E::ArrangementView, "L-CTRL-027", "Arrangement View", CB, 1550.0, 20.0, Right, Switch, None),
    p(E::AssistantToggle, D24, "Show/Hide Assistant", CB, 1572.0, 18.0, Right, Switch, None),
    p(E::InfoToggle, "L-STAT-001", "Show/Hide Info View", SB, 17.0, 16.0, Left, Switch, None),
    p(E::Message, "L-STAT-002", "Status", SB, 44.0, 1196.0, Left, Display, None),
    p(E::ClipViewToggle, "L-STAT-004", "Show/Hide Clip View", SB, 1259.0, 10.0, Right, Switch, None),
    p(E::ClipSelector, "L-STAT-005", "Clip View Selector", SB, 1270.0, 198.0, Right, Display, Some("M2")),
    p(E::DeviceViewToggle, "L-STAT-006", "Show/Hide Device View", SB, 1486.0, 10.0, Right, Switch, Some("M2")),
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

/// The share of extra width that goes to the gap left of the Control Bar's
/// middle groups: Live's two stretching gaps are 103 points (after the scale
/// controls) and 177 points (before Draw Mode) at the reference width; the
/// second is 157 points here, the assistant's toggle having taken 20 (D24).
const MIDDLE_SHARE: f32 = 103.0 / (103.0 + 157.0);

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
    /// The detail area (L-WIN-006): the Info View and the Clip or Device
    /// View.
    pub detail: Rect,
    /// The Info View, at the detail area's left, while it shows
    /// (L-INFO-001).
    pub info_view: Option<Rect>,
    /// The Device View, right of the Info View (L-DEV-001), or taking the
    /// whole detail area while the Info View hides.
    pub device_view: Rect,
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
        let size = size.max(MINIMUM);
        let (w, h) = (size.x, size.y);
        let extra = w - REFERENCE.x;
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
        let detail = Rect::from_min_max(
            pos2(MARGIN, detail_bottom - 188.0),
            pos2(right, detail_bottom),
        );
        let views_bottom = detail.top() - 8.0;
        let browser_rect =
            browser.then(|| Rect::from_min_max(pos2(MARGIN, y(61.0)), pos2(311.0, views_bottom)));
        let main_left = browser_rect.map_or(MARGIN, |b| b.right() + MARGIN);
        let main_view = Rect::from_min_max(pos2(main_left, y(61.0)), pos2(right, views_bottom));
        let info_view =
            info_view.then(|| Rect::from_min_max(detail.min, pos2(240.0, detail.bottom())));
        // With the Info View hidden the Device View takes the detail area's
        // whole width.
        let device_left = info_view.map_or(detail.min.x, |info| info.right() + 9.0);
        let device_view = Rect::from_min_max(pos2(device_left, detail.top()), detail.max);

        let elements = ELEMENTS
            .iter()
            .map(|placement| {
                let shift = match placement.anchor {
                    Left => 0.0,
                    Middle => extra * MIDDLE_SHARE,
                    // The Status Bar's right edge is the pane's left edge.
                    Right if placement.strip == Strip::StatusBar => extra - taken,
                    Right => extra,
                };
                let mut width = placement.width;
                if placement.element == Element::Message {
                    // The message area stretches to the Clip View toggle.
                    width += extra - taken;
                }
                let x = placement.x + shift;
                let rect = match placement.strip {
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
            device_view,
            status_bar,
            assistant,
            undo_history,
            elements,
        }
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
            &mut self.device_view,
            &mut self.status_bar,
        ] {
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
            &mut self.device_view,
            &mut self.status_bar,
        ] {
            scale(rect);
        }
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
            row("L-DEV-001", "Device View", self.device_view),
            row("L-WIN-007", "Status Bar", self.status_bar),
        ];
        if let Some(info_view) = self.info_view {
            rows.insert(3, row("L-INFO-001", "Info View", info_view));
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

/// The browser's parts inside its column (L-BRW rows), from layout.md's
/// measurements in Live 12.4.3 at 1600 × 1000 points. The top row and the
/// sidebar keep their size; the search field and the content pane stretch
/// with the column, which keeps Live's 302 points until it can be widened
/// (I-DRAG-027). The Filter View (L-BRW-007) is an M6 feature and hidden, so
/// the column header row sits at the content pane's top.
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
    /// The column header row, with Name (L-BRW-009).
    pub header: Rect,
    /// The item list (L-BRW-010).
    pub list: Rect,
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
    /// Lays the browser out in its column.
    #[must_use]
    pub fn compute(column: Rect) -> Self {
        // Offsets from layout.md's window coordinates: the column spans x 9
        // to 311 and y 61 to 766 at the reference size.
        let x = |window_x: f32| column.left() + window_x - 9.0;
        let top = |window_y: f32| column.top() + window_y - 61.0;
        let row = |x0: f32, x1: f32| Rect::from_min_max(pos2(x0, top(62.0)), pos2(x1, top(80.0)));
        let right = column.right();
        let body_top = top(84.0);
        let sidebar = Rect::from_min_max(
            pos2(column.left(), body_top),
            pos2(x(109.0), column.bottom()),
        );
        let content = Rect::from_min_max(pos2(x(110.0), body_top), pos2(right, column.bottom()));
        let header = Rect::from_min_max(content.min, pos2(right, content.top() + 19.0));
        // The strip from y 744 to the column's bottom (766).
        let strip = Rect::from_min_max(pos2(content.left(), column.bottom() - 22.0), content.max);
        let list = Rect::from_min_max(
            pos2(content.left(), header.bottom()),
            pos2(right, strip.top()),
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
            search: row(x(50.0), right - 41.0),
            filter_toggle: row(right - 36.0, right - 18.0),
            filter_menu: row(right - 14.0, right - 2.0),
            sidebar,
            content,
            header,
            list,
            strip,
            preview_toggle,
            raw_toggle,
            preview_wave,
        }
    }
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
        }
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
                    .filter(|(p, _)| p.strip == strip)
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
        assert_eq!(layout.device_view.left(), 249.0);
        assert!(layout.rows().iter().any(|r| r.id == "L-INFO-001"));
    }

    // Spec: info-view R2.
    #[test]
    fn hiding_the_info_view_widens_the_device_view() {
        let shown = ShellLayout::compute(REFERENCE, true, true);
        let hidden = ShellLayout::compute(REFERENCE, true, false);
        assert_eq!(hidden.info_view, None);
        assert_eq!(hidden.device_view.left(), hidden.detail.left());
        assert_eq!(hidden.device_view.right(), shown.device_view.right());
        assert!(!hidden.rows().iter().any(|r| r.id == "L-INFO-001"));
    }
}

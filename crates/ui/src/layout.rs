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

use egui::{Pos2, Rect, Vec2, pos2, vec2};

/// Live's title bar height at the reference size (L-WIN-002). layout.md
/// measures from the window's top edge; the content area starts below it.
pub const TITLE_BAR: f32 = 23.0;

/// The reference window: 1600 × 1000 points with its title bar, so the
/// content area is 1600 × 977.
pub const REFERENCE: Vec2 = vec2(1600.0, 1000.0 - TITLE_BAR);

/// The smallest content area the shell lays out without overlapping
/// controls: the Control Bar's stretching gaps (280 points at the reference
/// width) close up at 1320 points.
pub const MINIMUM: Vec2 = vec2(1320.0, 640.0);

/// The margin between panes and at the window's left and right edges
/// (L-WIN-009).
pub const MARGIN: f32 = 9.0;

/// The Settings window's size with its title bar (L-SET-001).
pub const SETTINGS_WINDOW: Vec2 = vec2(562.0, 617.0);

/// A y position measured in Live's window, as a content-area position.
const fn y(window_y: f32) -> f32 {
    window_y - TITLE_BAR
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

/// Every element, in on-screen order, with its row and its measurement in
/// Live at 1600 × 1000 points (layout.md, Control Bar and Status Bar). The
/// Link toggle, the Tempo Follower toggle, and the Global Groove Amount
/// (L-CTRL-002, 003, 008) are hidden in Live's default state and come with
/// the packages that build them. Widths not measured in layout.md run to the
/// next element's left edge, less a 2-point gap.
#[rustfmt::skip]
pub const ELEMENTS: &[Placement] = &[
    p(E::BrowserToggle, "L-CTRL-001", "Show/Hide Browser", CB, 9.0, 18.0, Left, Switch, None),
    p(E::BrowserMenu, "L-CTRL-001", "Browser Config Menu", CB, 27.0, 8.0, Left, MenuArrow, Some("WP-1.10")),
    p(E::TapTempo, "L-CTRL-004", "Tap Tempo", CB, 54.0, 28.0, Left, Button, Some("WP-1.5")),
    p(E::Tempo, "L-CTRL-005", "Tempo", CB, 82.0, 44.0, Left, Value, Some("WP-1.5")),
    p(E::NudgeDown, "L-CTRL-006", "Phase Nudge Down", CB, 126.0, 18.0, Left, Button, Some("M7")),
    p(E::NudgeUp, "L-CTRL-006", "Phase Nudge Up", CB, 145.0, 18.0, Left, Button, Some("M7")),
    p(E::SignatureNumerator, "L-CTRL-007", "Time Signature Numerator", CB, 167.0, 20.0, Left, Value, Some("WP-2.20")),
    p(E::SignatureDenominator, "L-CTRL-007", "Time Signature Denominator", CB, 189.0, 18.0, Left, Value, Some("WP-2.20")),
    p(E::Metronome, "L-CTRL-009", "Metronome", CB, 209.0, 26.0, Left, Switch, Some("WP-2.15")),
    p(E::MetronomeMenu, "L-CTRL-009", "Metronome Settings", CB, 237.0, 10.0, Left, MenuArrow, Some("WP-2.15")),
    p(E::Quantization, "L-CTRL-010", "Quantization Menu", CB, 250.0, 45.0, Left, Chooser, Some("M3")),
    p(E::ScaleMode, "L-CTRL-011", "Scale Mode", CB, 314.0, 18.0, Left, Switch, Some("M6")),
    p(E::RootNote, "L-CTRL-011", "Root Note", CB, 335.0, 44.0, Left, Chooser, Some("M6")),
    p(E::ScaleName, "L-CTRL-011", "Scale Name", CB, 381.0, 71.0, Left, Chooser, Some("M6")),
    p(E::Follow, "L-CTRL-012", "Follow", CB, 555.0, 20.0, Middle, Switch, None),
    p(E::PositionBars, "L-CTRL-013", "Arrangement Position Bars", CB, 582.0, 26.0, Middle, Value, Some("WP-1.5")),
    p(E::PositionBeats, "L-CTRL-013", "Arrangement Position Beats", CB, 608.0, 25.0, Middle, Value, Some("WP-1.5")),
    p(E::PositionSixteenths, "L-CTRL-013", "Arrangement Position Sixteenths", CB, 633.0, 26.0, Middle, Value, Some("WP-1.5")),
    p(E::Play, "L-CTRL-014", "Play", CB, 678.0, 18.0, Middle, Button, Some("WP-1.5")),
    p(E::Stop, "L-CTRL-014", "Stop", CB, 701.0, 18.0, Middle, Button, Some("WP-1.5")),
    p(E::Record, "L-CTRL-014", "Arrangement Record", CB, 725.0, 18.0, Middle, Switch, Some("WP-2.13")),
    p(E::Overdub, "L-CTRL-015", "MIDI Arrangement Overdub", CB, 758.0, 18.0, Middle, Switch, Some("WP-2.14")),
    p(E::AutomationArm, "L-CTRL-015", "Automation Arm", CB, 778.0, 16.0, Middle, Switch, Some("WP-2.12")),
    p(E::ReEnableAutomation, "L-CTRL-015", "Re-Enable Automation", CB, 796.0, 18.0, Middle, Button, Some("WP-2.12")),
    p(E::CaptureMidi, "L-CTRL-015", "Capture MIDI", CB, 816.0, 16.0, Middle, Button, Some("M3")),
    p(E::SessionRecord, "L-CTRL-015", "Session Record", CB, 834.0, 18.0, Middle, Switch, Some("M3")),
    p(E::LoopStartBars, "L-CTRL-016", "Loop Start Bars", CB, 875.0, 26.0, Middle, Value, Some("WP-1.5")),
    p(E::LoopStartBeats, "L-CTRL-016", "Loop Start Beats", CB, 901.0, 26.0, Middle, Value, Some("WP-1.5")),
    p(E::LoopStartSixteenths, "L-CTRL-016", "Loop Start Sixteenths", CB, 927.0, 26.0, Middle, Value, Some("WP-1.5")),
    p(E::PunchIn, "L-CTRL-017", "Punch-In", CB, 956.0, 21.0, Middle, Switch, Some("WP-2.13")),
    p(E::Loop, "L-CTRL-017", "Arrangement Loop", CB, 979.0, 21.0, Middle, Switch, Some("WP-1.5")),
    p(E::PunchOut, "L-CTRL-017", "Punch-Out", CB, 1002.0, 21.0, Middle, Switch, Some("WP-2.13")),
    p(E::LoopLengthBars, "L-CTRL-018", "Loop Length Bars", CB, 1029.0, 26.0, Middle, Value, Some("WP-1.5")),
    p(E::LoopLengthBeats, "L-CTRL-018", "Loop Length Beats", CB, 1055.0, 25.0, Middle, Value, Some("WP-1.5")),
    p(E::LoopLengthSixteenths, "L-CTRL-018", "Loop Length Sixteenths", CB, 1080.0, 26.0, Middle, Value, Some("WP-1.5")),
    p(E::DrawMode, "L-CTRL-019", "Draw Mode", CB, 1283.0, 17.0, Right, Switch, Some("WP-1.8b")),
    p(E::ComputerKeyboard, "L-CTRL-020", "Computer MIDI Keyboard", CB, 1302.0, 17.0, Right, Switch, Some("WP-2.2")),
    p(E::KeyMap, "L-CTRL-021", "Key Map Mode", CB, 1321.0, 27.0, Right, Switch, Some("M7")),
    p(E::KeyIndicators, "L-CTRL-021", "Key Input and Output Indicators", CB, 1350.0, 7.0, Right, Indicator, Some("M7")),
    p(E::MidiMap, "L-CTRL-022", "MIDI Map Mode", CB, 1359.0, 28.0, Right, Switch, Some("M7")),
    p(E::SampleRate, "L-CTRL-023", "Sample Rate", CB, 1389.0, 50.0, Right, Display, None),
    p(E::Cpu, "L-CTRL-024", "CPU Load", CB, 1439.0, 49.0, Right, Meter, None),
    p(E::CpuMenu, "L-CTRL-024", "CPU Meter Options", CB, 1488.0, 8.0, Right, MenuArrow, Some("M5")),
    p(E::Overload, "L-CTRL-025", "Disk Overload", CB, 1498.0, 18.0, Right, Indicator, Some("WP-1.3")),
    p(E::MidiIndicators, "L-CTRL-026", "MIDI Track Input and Output Indicators", CB, 1527.0, 14.0, Right, Indicator, Some("WP-2.2")),
    p(E::SessionView, "L-CTRL-027", "Session View", CB, 1549.0, 20.0, Right, Switch, None),
    p(E::ArrangementView, "L-CTRL-027", "Arrangement View", CB, 1570.0, 20.0, Right, Switch, None),
    p(E::InfoToggle, "L-STAT-001", "Show/Hide Info View", SB, 17.0, 16.0, Left, Switch, Some("M2")),
    p(E::Message, "L-STAT-002", "Status", SB, 44.0, 1196.0, Left, Display, None),
    p(E::ClipViewToggle, "L-STAT-004", "Show/Hide Clip View", SB, 1259.0, 10.0, Right, Switch, Some("M2")),
    p(E::ClipSelector, "L-STAT-005", "Clip View Selector", SB, 1270.0, 198.0, Right, Display, Some("M2")),
    p(E::DeviceViewToggle, "L-STAT-006", "Show/Hide Device View", SB, 1486.0, 10.0, Right, Switch, Some("M2")),
    p(E::DeviceSelector, "L-STAT-007", "Device View Selector", SB, 1497.0, 41.0, Right, Display, Some("M2")),
    p(E::MixerToggle, "L-STAT-008", "Show/Hide Mixer", SB, 1557.0, 24.0, Right, Switch, Some("WP-1.9")),
    p(E::MixerMenu, "L-STAT-008", "Mixer Config Menu", SB, 1581.0, 10.0, Right, MenuArrow, Some("WP-1.9")),
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
/// controls) and 177 points (before Draw Mode) at the reference width.
const MIDDLE_SHARE: f32 = 103.0 / (103.0 + 177.0);

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
    /// The Info View, at the detail area's left (L-INFO-001).
    pub info_view: Rect,
    /// The Device View, right of the Info View (L-DEV-001).
    pub device_view: Rect,
    /// The Status Bar strip (L-WIN-007).
    pub status_bar: Rect,
    /// Every element's rectangle, in [`ELEMENTS`] order.
    pub elements: Vec<(Element, Rect)>,
}

impl ShellLayout {
    /// Lays out a content area of `size` points, with or without the
    /// browser. Sizes below [`MINIMUM`] are laid out as [`MINIMUM`].
    #[must_use]
    pub fn compute(size: Vec2, browser: bool) -> Self {
        let size = size.max(MINIMUM);
        let (w, h) = (size.x, size.y);
        let extra = w - REFERENCE.x;
        let right = w - MARGIN;

        // Rows measured from the top keep their place; the detail area and
        // the Status Bar keep their distance from the bottom.
        let control_bar = Rect::from_min_max(Pos2::ZERO, pos2(w, y(61.0)));
        let status_bottom = h - (REFERENCE.y - y(993.0));
        let status_bar =
            Rect::from_min_max(pos2(0.0, status_bottom - 25.0), pos2(w, status_bottom));
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
        let info_view = Rect::from_min_max(detail.min, pos2(240.0, detail.bottom()));
        let device_view = Rect::from_min_max(pos2(249.0, detail.top()), detail.max);

        let elements = ELEMENTS
            .iter()
            .map(|placement| {
                let shift = match placement.anchor {
                    Left => 0.0,
                    Middle => extra * MIDDLE_SHARE,
                    Right => extra,
                };
                let mut width = placement.width;
                if placement.element == Element::Message {
                    // The message area stretches to the Clip View toggle.
                    width += extra;
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
            &mut self.info_view,
            &mut self.device_view,
            &mut self.status_bar,
        ] {
            move_rect(rect);
        }
        if let Some(browser) = &mut self.browser {
            move_rect(browser);
        }
        for (_, rect) in &mut self.elements {
            move_rect(rect);
        }
        self
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
        }
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elements_are_in_order_and_do_not_overlap_at_the_reference_or_minimum() {
        for size in [REFERENCE, MINIMUM, vec2(2400.0, 1400.0)] {
            let layout = ShellLayout::compute(size, true);
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

    #[test]
    fn hiding_the_browser_widens_the_main_view() {
        let with = ShellLayout::compute(REFERENCE, true);
        let without = ShellLayout::compute(REFERENCE, false);
        assert_eq!(without.browser, None);
        assert_eq!(without.main_view.left(), MARGIN);
        assert_eq!(without.main_view.right(), with.main_view.right());
    }
}

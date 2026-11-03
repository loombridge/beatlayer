//! MPE Control, the MPE-shaping MIDI effect (WP-5.32; the MPE shaper spec):
//! it shapes the per-note expressions that reach it, note by note, with a
//! linear or S-shaped curve, a range and smoothing, and can convert each of
//! the three sources to a channel message for an instrument that doesn't
//! read MPE.
//!
//! The three sources are pressure, slide and per-note pitch bend. Each is
//! switched on or off, has its own curve and smoothing, and each conversion
//! is its own switch: Press to AT sends channel pressure, Slide to Mod sends
//! CC 1, and NotePB to PB sends a channel pitch bend. Swapping Press to Slide
//! sends what pressure carried as slide instead.
//!
//! What the manual leaves open — the exact curve laws, the smoothing's time
//! constants, and what Default does to a note that carries no expression — is
//! Beatlayer's own, written into the spec. Its panel is laid out as Live
//! draws it (layout L-DEV-072): a column of tabs for the three sources with
//! the shown source's curve knobs under them, a transfer curve whose foot
//! holds the curve type, Min, Max, X and Y, and, in the expanded part, the
//! shown source's settings.

use bl_engine::{DeviceType, NoteEvent, NoteExpression, NoteKind, Registry};
use bl_model::device_layout::{
    BODY_HEIGHT, Control, ControlBand, DisplayCell, Glyph, LayoutNode, Page, PageStyle, PagesNode,
    TitlePart, Widget,
};
use bl_model::{DeviceDescription, DeviceLayout, DeviceRole, ParamDescriptor, ParamId, ParamUnit};

use crate::framework::{Audio, NativeDevice, ParamBank, add};
use crate::midi_fx::value;

/// The MPE Control device type's id.
pub const MPE_CONTROL: &str = "beatlayer.mpe_control";

/// How many expression sources there are: Press, Slide and NotePB.
pub const SOURCES: u32 = 3;
/// The parameters each source has.
pub const PER_SOURCE: u32 = 12;

/// Source Press.
pub const PRESS: usize = 0;
/// Source Slide.
pub const SLIDE: usize = 1;
/// Source NotePB.
pub const NOTE_PB: usize = 2;

/// Whether source `s` acts.
#[must_use]
pub fn source_on(s: u32) -> ParamId {
    ParamId(s * PER_SOURCE)
}
/// Source `s`'s curve type: Linear or S-shaped.
#[must_use]
pub fn source_curve(s: u32) -> ParamId {
    ParamId(s * PER_SOURCE + 1)
}
/// Source `s`'s Curve A dial.
#[must_use]
pub fn source_curve_a(s: u32) -> ParamId {
    ParamId(s * PER_SOURCE + 2)
}
/// Source `s`'s Curve B dial.
#[must_use]
pub fn source_curve_b(s: u32) -> ParamId {
    ParamId(s * PER_SOURCE + 3)
}
/// Whether source `s`'s two S-shaped segments move together.
#[must_use]
pub fn source_link(s: u32) -> ParamId {
    ParamId(s * PER_SOURCE + 4)
}
/// Source `s`'s lowest output value.
#[must_use]
pub fn source_min(s: u32) -> ParamId {
    ParamId(s * PER_SOURCE + 5)
}
/// Source `s`'s highest output value.
#[must_use]
pub fn source_max(s: u32) -> ParamId {
    ParamId(s * PER_SOURCE + 6)
}
/// Source `s`'s S-shaped breakpoint's place along the input.
#[must_use]
pub fn source_x(s: u32) -> ParamId {
    ParamId(s * PER_SOURCE + 7)
}
/// Source `s`'s S-shaped breakpoint's output.
#[must_use]
pub fn source_y(s: u32) -> ParamId {
    ParamId(s * PER_SOURCE + 8)
}
/// Whether source `s` smooths its output.
#[must_use]
pub fn source_smooth(s: u32) -> ParamId {
    ParamId(s * PER_SOURCE + 9)
}
/// Source `s`'s rise time, in milliseconds.
#[must_use]
pub fn source_rise(s: u32) -> ParamId {
    ParamId(s * PER_SOURCE + 10)
}
/// Source `s`'s fall time, in milliseconds.
#[must_use]
pub fn source_fall(s: u32) -> ParamId {
    ParamId(s * PER_SOURCE + 11)
}

/// Press's Default switch.
pub const PRESS_DEFAULT: ParamId = ParamId(36);
/// Press's default value.
pub const PRESS_DEFAULT_VALUE: ParamId = ParamId(37);
/// Swap Press to Slide.
pub const SWAP_TO_SLIDE: ParamId = ParamId(38);
/// Press to AT.
pub const PRESS_TO_AT: ParamId = ParamId(39);
/// Slide's Default switch.
pub const SLIDE_DEFAULT: ParamId = ParamId(40);
/// Slide's default value.
pub const SLIDE_DEFAULT_VALUE: ParamId = ParamId(41);
/// Slide's Centered switch.
pub const SLIDE_CENTERED: ParamId = ParamId(42);
/// Slide's mode: Abs, Rel or Ons.
pub const SLIDE_MODE: ParamId = ParamId(43);
/// Slide to Mod.
pub const SLIDE_TO_MOD: ParamId = ParamId(44);
/// NotePB's pitch range multiplier.
pub const PITCH_RANGE: ParamId = ParamId(45);
/// NotePB to PB.
pub const NOTEPB_TO_PB: ParamId = ParamId(46);

/// The curve type choices.
pub const CURVES: [&str; 2] = ["Linear", "S-shaped"];
/// The Pitch Range choices: the factors the bend is scaled by.
pub const PITCH_RANGES: [&str; 4] = ["1x", "2x", "4x", "8x"];
/// The slide mode choices.
pub const SLIDE_MODES: [&str; 3] = ["Abs", "Rel", "Ons"];

fn source_params(s: u32, label: &str) -> Vec<ParamDescriptor> {
    vec![
        ParamDescriptor::toggle(source_on(s), label, true).describe(&format!(
            "Whether {label} is shaped. Off, its data passes through."
        )),
        ParamDescriptor::choice(
            source_curve(s),
            &format!("{label} Curve"),
            &CURVES,
            // NotePB starts with the S-shaped curve, whose breakpoint is
            // its centre.
            usize::from(s == NOTE_PB as u32),
        )
        .describe(&format!(
            "{label}'s curve: a linear ramp or an S-shaped one."
        )),
        ParamDescriptor::continuous(
            source_curve_a(s),
            &format!("{label} Curve A"),
            (-1.0, 1.0),
            0.0,
            ParamUnit::None,
        )
        .decimals(2)
        .describe(&format!("{label}'s curve bend, 0 leaving it straight.")),
        ParamDescriptor::continuous(
            source_curve_b(s),
            &format!("{label} Curve B"),
            (-1.0, 1.0),
            0.0,
            ParamUnit::None,
        )
        .decimals(2)
        .describe(&format!(
            "{label}'s second S-shaped segment's bend, 0 leaving it straight, when the \
             segments aren't linked."
        )),
        ParamDescriptor::toggle(source_link(s), &format!("{label} Curve Link"), true).describe(
            &format!("Whether {label}'s two S-shaped segments move together."),
        ),
        ParamDescriptor::integer(
            source_min(s),
            &format!("{label} Min"),
            (0, 127),
            0,
            ParamUnit::None,
        )
        .describe(&format!("The lowest value {label} comes out as, of 127.")),
        ParamDescriptor::integer(
            source_max(s),
            &format!("{label} Max"),
            (0, 127),
            127,
            ParamUnit::None,
        )
        .describe(&format!("The highest value {label} comes out as, of 127.")),
        ParamDescriptor::integer(
            source_x(s),
            &format!("{label} X"),
            (0, 127),
            64,
            ParamUnit::None,
        )
        .describe(&format!(
            "{label}'s S-shaped breakpoint along the input, of 127."
        )),
        ParamDescriptor::integer(
            source_y(s),
            &format!("{label} Y"),
            (0, 127),
            64,
            ParamUnit::None,
        )
        .describe(&format!("{label}'s S-shaped breakpoint's output, of 127.")),
        ParamDescriptor::toggle(source_smooth(s), &format!("{label} Smooth"), false)
            .describe(&format!("Whether {label}'s output is smoothed.")),
        ParamDescriptor::continuous(
            source_rise(s),
            &format!("{label} Rise"),
            (0.0, 1000.0),
            15.0,
            ParamUnit::Milliseconds,
        )
        .decimals(1)
        .describe(&format!("How long {label} takes to rise to a new value.")),
        ParamDescriptor::continuous(
            source_fall(s),
            &format!("{label} Fall"),
            (0.0, 1000.0),
            15.0,
            ParamUnit::Milliseconds,
        )
        .decimals(1)
        .describe(&format!("How long {label} takes to fall to a new value.")),
    ]
}

/// The name of the pages node whose tabs choose the source.
pub const SOURCE_PAGES: &str = "Source";

/// The source tabs' names, in order.
pub const SOURCE_NAMES: [&str; 3] = ["Press", "Slide", "NotePB"];

/// The name of the pages node of the settings, which follows the source tabs.
pub const SETTINGS_PAGES: &str = "Source Settings";

/// The width of the settings part, in points.
const SETTINGS_WIDTH: f32 = 88.0;

/// MPE Control's panel as Live draws it: the source tabs with the shown
/// source's curve knobs under them, the transfer curve with its curve type,
/// Min, Max, X and Y at its foot, and, in the expanded part, the settings of
/// the shown source.
fn layout() -> DeviceLayout {
    let at = |param, widget, x, y| Control::new(param, widget, x, y);
    let pages = (0..SOURCES)
        .map(|s| {
            let foot = ControlBand::free(
                160.0,
                25.0,
                vec![
                    at(source_curve(s), Widget::IconSegments(2), 2.0, 11.0),
                    at(source_min(s), Widget::Number, 52.0, 2.0).sized(24.0, 21.0),
                    at(source_max(s), Widget::Number, 78.0, 2.0).sized(26.0, 21.0),
                    at(source_x(s), Widget::Number, 106.0, 2.0).sized(24.0, 21.0),
                    at(source_y(s), Widget::Number, 132.0, 2.0).sized(24.0, 21.0),
                ],
            );
            // The second knob and the link between the two show with the
            // S-shaped curve, whose two segments they bend.
            let s_only = |control: Control| control.hidden_while(source_curve(s), 0);
            let knobs = ControlBand::free(
                52.0,
                BODY_HEIGHT - 48.0,
                vec![
                    at(source_curve_a(s), Widget::SmallKnob, 10.0, 8.0).unnamed(),
                    s_only(at(source_link(s), Widget::IconToggle, 17.0, 52.0))
                        .with_glyph(Glyph::Link)
                        .sized(18.0, 12.0),
                    s_only(at(source_curve_b(s), Widget::SmallKnob, 10.0, 70.0).unnamed()),
                ],
            );
            Page::new(
                SOURCE_NAMES[s as usize],
                LayoutNode::Cell(
                    DisplayCell::new("Transfer Curve", 160.0, BODY_HEIGHT, 1.0).with_bottom(foot),
                ),
            )
            .with_section(LayoutNode::Band(knobs))
        })
        .collect();
    DeviceLayout::new(LayoutNode::Pages(PagesNode::new(
        SOURCE_PAGES,
        PageStyle::Column,
        pages,
    )))
    .with_expanded(LayoutNode::Pages(PagesNode::following(
        SETTINGS_PAGES,
        SOURCE_PAGES,
        (0..SOURCES)
            .map(|s| Page::new(SOURCE_NAMES[s as usize], LayoutNode::Band(settings(s))))
            .collect(),
    )))
    .with_triangle_right_when_shown()
    .with_title(vec![TitlePart::Mpe])
}

/// What the settings show for source `s`: Smooth, Rise and Fall, then what
/// the source has of its own, then its conversion.
fn settings(s: u32) -> ControlBand {
    let at = |param, widget, y| Control::new(param, widget, 6.0, y).sized(76.0, 12.0);
    let tick = |param, y| at(param, Widget::Tick, y);
    let mut controls = vec![
        tick(source_smooth(s), 3.0),
        at(source_rise(s), Widget::Inline, 18.0),
        at(source_fall(s), Widget::Inline, 33.0),
    ];
    let own = |param, widget, y, height| Control::new(param, widget, 6.0, y).sized(76.0, height);
    match s as usize {
        PRESS => controls.extend([
            tick(PRESS_DEFAULT, 62.0).sized(50.0, 12.0),
            Control::new(PRESS_DEFAULT_VALUE, Widget::Bare, 58.0, 62.0).sized(26.0, 12.0),
            tick(SWAP_TO_SLIDE, 79.0),
            tick(PRESS_TO_AT, 130.0),
        ]),
        SLIDE => controls.extend([
            tick(SLIDE_DEFAULT, 62.0).sized(50.0, 12.0),
            Control::new(SLIDE_DEFAULT_VALUE, Widget::Bare, 58.0, 62.0).sized(26.0, 12.0),
            tick(SLIDE_CENTERED, 79.0),
            own(SLIDE_MODE, Widget::Segments(3), 98.0, 21.0).titled(),
            tick(SLIDE_TO_MOD, 130.0),
        ]),
        _ => controls.extend([
            own(PITCH_RANGE, Widget::Segments(4), 65.0, 21.0).titled(),
            tick(NOTEPB_TO_PB, 130.0),
        ]),
    }
    ControlBand::free(SETTINGS_WIDTH, BODY_HEIGHT, controls)
}

/// MPE Control's device type.
#[must_use]
pub fn mpe_control_type() -> DeviceType {
    let mut params = source_params(0, "Press");
    params.extend(source_params(1, "Slide"));
    params.extend(source_params(2, "NotePB"));
    params.extend([
        ParamDescriptor::toggle(PRESS_DEFAULT, "Press Default", false)
            .describe("Gives a note that carries no pressure the Press default value."),
        ParamDescriptor::integer(
            PRESS_DEFAULT_VALUE,
            "Press Default Value",
            (0, 127),
            0,
            ParamUnit::None,
        )
        .describe("The pressure a note without one gets, while Press Default is on."),
        ParamDescriptor::toggle(SWAP_TO_SLIDE, "Swap to Slide", false)
            .describe("Sends what pressure carried as slide instead."),
        ParamDescriptor::toggle(PRESS_TO_AT, "Press to AT", false)
            .describe("Converts per-note pressure to channel pressure."),
        ParamDescriptor::toggle(SLIDE_DEFAULT, "Slide Default", false)
            .describe("Gives a note that carries no slide the Slide default value."),
        ParamDescriptor::integer(
            SLIDE_DEFAULT_VALUE,
            "Slide Default Value",
            (0, 127),
            0,
            ParamUnit::None,
        )
        .describe("The slide a note without one gets, while Slide Default is on."),
        ParamDescriptor::toggle(SLIDE_CENTERED, "Slide Centered", false).describe(
            "Reads slide as distance from the middle: the centre is zero and the ends are one.",
        ),
        ParamDescriptor::choice(SLIDE_MODE, "Slide Mode", &SLIDE_MODES, 0).describe(
            "Abs reads slide as it comes, Rel starts each note in the middle, and Ons \
             updates slide only when a note starts.",
        ),
        ParamDescriptor::toggle(SLIDE_TO_MOD, "Slide to Mod", false)
            .describe("Converts slide to the mod wheel, CC 1."),
        ParamDescriptor::choice(PITCH_RANGE, "Pitch Range", &PITCH_RANGES, 0).describe(
            "Scales per-note pitch bend by 1, 2, 4 or 8, for a controller and instrument whose \
             ranges differ.",
        ),
        ParamDescriptor::toggle(NOTEPB_TO_PB, "NotePB to PB", false)
            .describe("Converts per-note pitch bend to a channel pitch bend."),
    ]);
    DeviceType::native(MPE_CONTROL, "MPE Control", DeviceRole::MidiEffect).with_description(
        DeviceDescription::new(params)
            .with_menu_options(&[source_on(0), source_on(1), source_on(2)])
            .with_layout(layout())
            .with_banks(&[
                (
                    "Pressure",
                    &[
                        "Press Min",
                        "Press Max",
                        "Press Smooth",
                        "Press Rise",
                        "Press Fall",
                        "Press Default",
                        "Press Default Value",
                    ],
                ),
                (
                    "Pressure Curve",
                    &[
                        "Press Curve",
                        "Press Curve A",
                        "Press Curve B",
                        "Press Curve Link",
                        "Press X",
                        "Press Y",
                    ],
                ),
                (
                    "Slide",
                    &[
                        "Slide Min",
                        "Slide Max",
                        "Slide Smooth",
                        "Slide Rise",
                        "Slide Fall",
                        "Slide Default",
                        "Slide Default Value",
                    ],
                ),
                (
                    "Slide Curve",
                    &[
                        "Slide Curve",
                        "Slide Curve A",
                        "Slide Curve B",
                        "Slide Curve Link",
                        "Slide X",
                        "Slide Y",
                        "Slide Centered",
                        "Slide Mode",
                    ],
                ),
                (
                    "Note Pitch Bend",
                    &[
                        "NotePB Min",
                        "NotePB Max",
                        "NotePB Smooth",
                        "NotePB Rise",
                        "NotePB Fall",
                        "Pitch Range",
                        "NotePB to PB",
                    ],
                ),
                (
                    "Note PB Curve",
                    &[
                        "NotePB Curve",
                        "NotePB Curve A",
                        "NotePB Curve B",
                        "NotePB Curve Link",
                        "NotePB X",
                        "NotePB Y",
                    ],
                ),
                ("Routing", &["Swap to Slide", "Press to AT", "Slide to Mod"]),
            ]),
    )
}

/// Adds MPE Control to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, mpe_control_type(), |_, _| MpeControl {
        clock: 0,
        notes: Vec::with_capacity(ROOM),
    });
}

/// How many notes the device shapes at once.
const ROOM: usize = 1024;

/// A note the device is shaping: where a relative slide started, each
/// source's last value and when, and whether each source has a value yet.
#[derive(Clone, Copy)]
struct Held {
    channel: u8,
    key: u8,
    raw0: f64,
    value: [f64; SOURCES as usize],
    time: [i64; SOURCES as usize],
    seeded: [bool; SOURCES as usize],
    /// Whether Ons mode has sent this note its one slide (the MPE shaper
    /// spec, R13).
    ons_sent: bool,
}

/// One value's move along a time constant: the fraction of the way from
/// where it was to where it is going, in `dt_frames` frames at `rate`,
/// over `time_constant_ms` milliseconds.
fn approach(time_constant_ms: f64, dt_frames: f64, rate: f64) -> f64 {
    if time_constant_ms <= 0.0 {
        return 1.0;
    }
    let tau = time_constant_ms / 1000.0 * rate;
    (1.0 - (-dt_frames / tau).exp()).clamp(0.0, 1.0)
}

/// A curve's output for an input `t` (0 to 1): a linear bend, or the two
/// segments of an S-shaped curve joined at (X, Y). Each segment is bent by
/// `a`, and by `b` when the segments aren't linked. It is the spec's law
/// (the MPE shaper spec): Beatlayer's own, since the manual states none.
#[must_use]
pub fn shape(t: f64, curved: bool, a: f64, b: f64, link: bool, x: f64, y: f64) -> f64 {
    let bend = |t: f64, c: f64| {
        let exponent = 2f64.powf((0.5 - c) * 4.0);
        t.clamp(0.0, 1.0).powf(exponent)
    };
    let t = t.clamp(0.0, 1.0);
    if !curved {
        return bend(t, a);
    }
    let x = x.clamp(0.001, 0.999);
    let y = y.clamp(0.0, 1.0);
    if t <= x {
        y * bend(t / x, a)
    } else {
        y + (1.0 - y) * bend((t - x) / (1.0 - x), if link { a } else { b })
    }
}

/// One source's settings for a block.
struct Source {
    on: bool,
    curved: bool,
    a: f64,
    b: f64,
    link: bool,
    min: f64,
    max: f64,
    x: f64,
    y: f64,
    smooth: bool,
    rise: f64,
    fall: f64,
}

impl Source {
    fn read(params: &ParamBank, s: u32) -> Self {
        Self {
            on: value(params, source_on(s)) >= 0.5,
            curved: value(params, source_curve(s)) >= 0.5,
            // The curve laws take a bend from 0 to 1, 0.5 straight.
            a: (value(params, source_curve_a(s)) + 1.0) / 2.0,
            b: (value(params, source_curve_b(s)) + 1.0) / 2.0,
            link: value(params, source_link(s)) >= 0.5,
            min: value(params, source_min(s)) / 127.0,
            max: value(params, source_max(s)) / 127.0,
            x: value(params, source_x(s)) / 127.0,
            y: value(params, source_y(s)) / 127.0,
            smooth: value(params, source_smooth(s)) >= 0.5,
            rise: value(params, source_rise(s)),
            fall: value(params, source_fall(s)),
        }
    }

    /// The shaped value for an input 0 to 1.
    fn apply(&self, t: f64) -> f64 {
        let f = shape(t, self.curved, self.a, self.b, self.link, self.x, self.y);
        self.min + f * (self.max - self.min)
    }
}

struct MpeControl {
    /// Frames the device has processed: the clock smoothing runs on.
    clock: i64,
    notes: Vec<Held>,
}

impl MpeControl {
    fn blank(channel: u8, key: u8) -> Held {
        Held {
            channel,
            key,
            raw0: 0.5,
            value: [0.0; SOURCES as usize],
            time: [0; SOURCES as usize],
            seeded: [false; SOURCES as usize],
            ons_sent: false,
        }
    }

    /// The entry for a note on `channel` and `key`, making one if it fits.
    fn entry(&mut self, channel: u8, key: u8) -> usize {
        if let Some(at) = self
            .notes
            .iter()
            .position(|held| held.channel == channel && held.key == key)
        {
            return at;
        }
        if self.notes.len() == ROOM {
            return 0;
        }
        self.notes.push(Self::blank(channel, key));
        self.notes.len() - 1
    }

    fn end(&mut self, channel: u8, key: u8) {
        self.notes
            .retain(|held| !(held.channel == channel && held.key == key));
    }
}

impl NativeDevice for MpeControl {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        let rate = audio.sample_rate().as_f64();
        let sources = [
            Source::read(params, 0),
            Source::read(params, 1),
            Source::read(params, 2),
        ];
        let centered = value(params, SLIDE_CENTERED) >= 0.5;
        let slide_mode = value(params, SLIDE_MODE) as usize;
        let swap = value(params, SWAP_TO_SLIDE) >= 0.5;
        let press_to_at = value(params, PRESS_TO_AT) >= 0.5;
        let slide_to_mod = value(params, SLIDE_TO_MOD) >= 0.5;
        let notepb_to_pb = value(params, NOTEPB_TO_PB) >= 0.5;
        let range = 2f64.powf(value(params, PITCH_RANGE).round());
        let press_default = value(params, PRESS_DEFAULT) >= 0.5;
        let slide_default_on = value(params, SLIDE_DEFAULT) >= 0.5;
        let default_value = [
            value(params, PRESS_DEFAULT_VALUE) / 127.0,
            value(params, SLIDE_DEFAULT_VALUE) / 127.0,
        ];
        for note in audio.notes() {
            let (channel, key) = (note.channel, note.key);
            match note.kind {
                NoteKind::On => {
                    let at = self.entry(channel, key);
                    self.notes[at] = Self::blank(channel, key);
                    // Defaults: a note that carries no expression gets one.
                    let pressed = [
                        press_default.then(|| sources[PRESS].apply(default_value[0])),
                        slide_default_on.then(|| sources[SLIDE].apply(default_value[1])),
                    ];
                    audio.send(note);
                    for (s, value) in pressed.into_iter().enumerate() {
                        if let Some(value) = value {
                            self.notes[at].value[s] = value;
                            self.notes[at].time[s] = self.clock + i64::from(note.offset);
                            self.notes[at].seeded[s] = true;
                            audio.send(NoteEvent {
                                kind: NoteKind::Expression {
                                    expression: if s == SLIDE {
                                        NoteExpression::Slide
                                    } else {
                                        NoteExpression::Pressure
                                    },
                                    value: value as f32,
                                },
                                ..note
                            });
                        }
                    }
                }
                NoteKind::Off | NoteKind::Choke => {
                    audio.send(note);
                    self.end(channel, key);
                }
                NoteKind::Expression { expression, value } => {
                    let s = match expression {
                        NoteExpression::Pressure => PRESS,
                        NoteExpression::Slide => SLIDE,
                        NoteExpression::Bend => NOTE_PB,
                    };
                    if !sources[s].on {
                        audio.send(note);
                        continue;
                    }
                    let at = self.entry(channel, key);
                    // Spec: mpe-shaper R13. In Ons mode a note takes the
                    // first slide that reaches it and keeps it: the later
                    // ones are ignored.
                    if s == SLIDE && slide_mode == 2 {
                        if self.notes[at].ons_sent {
                            continue;
                        }
                        self.notes[at].ons_sent = true;
                    }
                    let raw = f64::from(value);
                    let time = self.clock + i64::from(note.offset);
                    let relative = s == SLIDE && slide_mode == 1;
                    if relative && !self.notes[at].seeded[SLIDE] {
                        self.notes[at].raw0 = raw;
                    }
                    let t = if s == NOTE_PB {
                        (raw + 1.0) * 0.5
                    } else if relative {
                        (0.5 + (raw - self.notes[at].raw0)).clamp(0.0, 1.0)
                    } else if s == SLIDE && centered {
                        (raw - 0.5).abs() * 2.0
                    } else {
                        raw
                    };
                    let target = sources[s].apply(t);
                    // Spec: mpe-shaper R13. Ons mode deactivates smoothing.
                    let smooths =
                        sources[s].smooth && !relative && !(s == SLIDE && slide_mode == 2);
                    let shaped = if smooths {
                        let last = self.notes[at].value[s];
                        let dt = (time - self.notes[at].time[s]).max(0) as f64;
                        let constant = if target >= last {
                            sources[s].rise
                        } else {
                            sources[s].fall
                        };
                        if self.notes[at].seeded[s] {
                            last + (target - last) * approach(constant, dt, rate)
                        } else {
                            target
                        }
                    } else {
                        target
                    };
                    self.notes[at].value[s] = shaped;
                    self.notes[at].time[s] = time;
                    self.notes[at].seeded[s] = true;
                    let out_kind = match s {
                        PRESS if press_to_at => NoteKind::ChannelPressure {
                            value: shaped.clamp(0.0, 1.0) as f32,
                        },
                        PRESS => NoteKind::Expression {
                            expression: if swap {
                                NoteExpression::Slide
                            } else {
                                NoteExpression::Pressure
                            },
                            value: shaped.clamp(0.0, 1.0) as f32,
                        },
                        SLIDE if slide_to_mod => NoteKind::Control {
                            controller: 1,
                            value: shaped.clamp(0.0, 1.0) as f32,
                        },
                        SLIDE => NoteKind::Expression {
                            expression: NoteExpression::Slide,
                            value: shaped.clamp(0.0, 1.0) as f32,
                        },
                        _ => {
                            let bend = ((shaped * 2.0 - 1.0) * range).clamp(-1.0, 1.0) as f32;
                            if notepb_to_pb {
                                NoteKind::PitchBend { value: bend }
                            } else {
                                NoteKind::Expression {
                                    expression: NoteExpression::Bend,
                                    value: bend,
                                }
                            }
                        }
                    };
                    audio.send(NoteEvent {
                        kind: out_kind,
                        ..note
                    });
                }
                // Spec: device-framework R39. A channel message passes through.
                _ => audio.send(note),
            }
        }
        self.clock += audio.frames() as i64;
    }
}

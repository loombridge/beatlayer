//! The Expression Control (WP-5.11; the modulation spec, R27): Beatlayer's equivalent of
//! an Expression Control device. A MIDI effect that passes its notes and MIDI
//! through untouched and turns what they say into control signals for the
//! parameters it maps: five source tabs, each a source, a curve, smoothing
//! and a slot of its own.
//!
//! The laws are Beatlayer's own. A source is a share from 0 to 1: Velocity
//! and Keytrack (the key's number over 127) are those of the latest note,
//! Modwheel, Expression and Sustain are controllers 1, 11 and 64, Pitchbend
//! the wheel from 0 at its bottom to 1 at its top (½ at the centre),
//! Pressure the channel's pressure or the latest held note's own, Slide the
//! latest held note's own, Random a draw at each note spread about the middle
//! by Random Amount, and Increment the step the notes have reached, from 0
//! at its first to 1 at its last, going round. Before anything is played a
//! source reads 0 (Pitchbend and Random ½). The curve turns the share into
//! another, and Min and Max put it between two others (Min above Max flips
//! it). Smoothing follows it at the Rise and Fall times: Linear takes that
//! long for the whole range, Logarithmic is a one-pole smoother with that time
//! constant.

use bl_dsp::Random;
use bl_engine::{DeviceType, NoteEvent, NoteExpression, NoteKind, Registry};
use bl_model::automation::curve_value;
use bl_model::device_layout::{
    BODY_HEIGHT, BandLabel, Control, ControlBand, DeviceLayout, DisplayCell, LayoutNode, Page,
    PageStyle, PagesNode, TitlePart, Widget,
};
use bl_model::modulator::{self, MODE, PLAY_MOD, PLAY_SLOTS, POLARITY};
use bl_model::param::{ParamLabel, Relabel};
use bl_model::{
    DeviceDescription, DeviceDisplay, DeviceId, DeviceRole, DisplayReading, DisplaySeries, OrdMap,
    ParamDescriptor, ParamId, ParamUnit,
};

use crate::framework::{Audio, Modulation, NativeDevice, ParamBank, add};
use crate::midi_fx::seed;
use crate::modulator::mapping_params_with;
use crate::signal_mod::{Held, Indices, SignalOut};

/// The tabs.
pub const TABS: usize = PLAY_SLOTS as usize;
/// The source choices, in the order of the chooser (R27).
pub const SOURCES: [&str; 10] = [
    "Velocity",
    "Modwheel",
    "Pitchbend",
    "Pressure",
    "Keytrack",
    "Expression",
    "Random",
    "Increment",
    "Slide",
    "Sustain",
];
/// What each tab starts on: Velocity, Pressure, Slide, Random and Increment,
/// as the tabs' names say (layout L-DEV-021).
const DEFAULT_SOURCES: [f64; TABS] = [0.0, 3.0, 8.0, 6.0, 7.0];
/// The tabs' names, and each one's page of the panel.
const TAB_NAMES: [&str; TABS] = ["Vel", "Press", "Slide", "Rand", "Incr"];
/// The units of the curve's Min, Max, X and Y: the range of a controller.
const RANGE: f64 = 127.0;
/// The curve choices.
pub const CURVES: [&str; 2] = ["Linear", "S"];
/// The smoothing choices.
pub const SMOOTHINGS: [&str; 2] = ["Lin", "Log"];

/// The first parameter id of tab `tab`; each tab has [`FIELDS`] ids.
const TAB_BASE: u32 = 100;
/// The ids each tab owns.
const FIELDS: u32 = 16;

/// A tab's parameter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    /// The source.
    Source = 0,
    /// Random Amount.
    RandomAmount = 1,
    /// Increment Steps.
    IncrementSteps = 2,
    /// The curve's kind.
    Curve = 3,
    /// Curve A: the bend of the upper segment, or of a Linear curve.
    CurveA = 4,
    /// Curve B: the bend of the lower segment.
    CurveB = 5,
    /// Curve Link: B follows A, opposite.
    CurveLink = 6,
    /// The S curve's middle breakpoint, X.
    CurveX = 7,
    /// The S curve's middle breakpoint, Y.
    CurveY = 8,
    /// The curve's output at its bottom.
    Min = 9,
    /// The curve's output at its top.
    Max = 10,
    /// The smoothing's kind.
    Smoothing = 11,
    /// Rise.
    Rise = 12,
    /// Fall.
    Fall = 13,
}

/// Tab `tab`'s (0 to 4) parameter `field`.
#[must_use]
pub const fn param(tab: usize, field: Field) -> ParamId {
    ParamId(TAB_BASE + tab as u32 * FIELDS + field as u32)
}

/// The Expression Control's type.
#[must_use]
pub fn play_mod_type() -> DeviceType {
    let mut params = Vec::new();
    for (tab, &default_source) in DEFAULT_SOURCES.iter().enumerate() {
        let n = tab + 1;
        let name = |what: &str| format!("{what} {n}");
        let percent = |field, what: &str, range: (f64, f64), default, text: &str| {
            ParamDescriptor::continuous(
                param(tab, field),
                &name(what),
                range,
                default,
                ParamUnit::Percent,
            )
            .decimals(0)
            .describe(text)
        };
        let range = |field, what: &str, range: (f64, f64), default, text: &str| {
            ParamDescriptor::integer(
                param(tab, field),
                &name(what),
                (range.0 as i64, range.1 as i64),
                default as i64,
                ParamUnit::None,
            )
            .describe(text)
        };
        let time = |field, what: &str, text: &str| {
            ParamDescriptor::continuous(
                param(tab, field),
                &name(what),
                (0.0, 1_000.0),
                0.0,
                ParamUnit::Milliseconds,
            )
            .decimals(0)
            .describe(text)
        };
        params.extend([
            ParamDescriptor::choice(
                param(tab, Field::Source),
                &name("Source"),
                &SOURCES,
                default_source as usize,
            )
            .describe("What the tab follows: Velocity, Modwheel, Pitchbend, Pressure, Keytrack, Expression, Random, Increment, Slide or Sustain."),
            percent(Field::RandomAmount, "Random Amount", (0.0, 100.0), 50.0, "How far each note's random value strays from the middle."),
            ParamDescriptor::integer(
                param(tab, Field::IncrementSteps),
                &name("Increment Steps"),
                (1, 32),
                8,
                ParamUnit::None,
            )
            .describe("How many steps Increment goes through before it starts again."),
            ParamDescriptor::choice(param(tab, Field::Curve), &name("Curve"), &CURVES, 0)
                .describe("Linear: two breakpoints bent by Curve A. S: three, Curve A bending the upper segment and Curve B the lower."),
            percent(Field::CurveA, "Curve A", (-100.0, 100.0), 0.0, "Bends the curve, or its upper segment."),
            percent(Field::CurveB, "Curve B", (-100.0, 100.0), 0.0, "Bends the S curve's lower segment."),
            ParamDescriptor::toggle(param(tab, Field::CurveLink), &name("Curve Link"), false)
                .describe("Bends both segments of the S curve in opposite directions, by Curve A."),
            range(Field::CurveX, "Curve X", (1.0, 126.0), 64.0, "Where the S curve's middle breakpoint sits across, from 0 to 127."),
            range(Field::CurveY, "Curve Y", (0.0, 127.0), 64.0, "Where the S curve's middle breakpoint sits up, from 0 to 127."),
            range(Field::Min, "Min", (0.0, 127.0), 0.0, "The curve's output where its input is at the bottom, from 0 to 127."),
            range(Field::Max, "Max", (0.0, 127.0), 127.0, "The curve's output where its input is at the top, from 0 to 127."),
            ParamDescriptor::choice(param(tab, Field::Smoothing), &name("Smoothing"), &SMOOTHINGS, 1)
                .describe("Linear takes the Rise or Fall time for the whole range; Logarithmic follows with that time constant."),
            time(Field::Rise, "Rise", "How slowly the output follows a rising value."),
            time(Field::Fall, "Fall", "How slowly the output follows a falling value."),
        ]);
    }
    // Live's Output Range is each tab's own Amount (or Min and Max) and the
    // Polarity: there is no Depth, and the Mod switch reads Modulate or Remote,
    // Unipolar to begin with (layout L-DEV-021).
    params.extend(mapping_params_with(
        PLAY_SLOTS,
        None,
        (["Modulate", "Remote"], 1),
    ));
    DeviceType::native(PLAY_MOD, "Expression Control", DeviceRole::MidiEffect).with_description(
        DeviceDescription::new(params)
            .with_relabels(relabels())
            .with_layout(layout())
            .with_banks(&[
                ("Mapping", &["Mod", "Polarity"]),
                // Each tab's curve and each slot's mapping have a Min
                // and a Max of the same name: they are given by number.
                (
                    "Targets 1-2",
                    &["Amount 1", "#68", "#69", "Amount 2", "#71", "#72"],
                ),
                (
                    "Targets 3-4",
                    &["Amount 3", "#74", "#75", "Amount 4", "#77", "#78"],
                ),
                ("Target 5", &["Amount 5", "#80", "#81"]),
                (
                    "Source 1",
                    &[
                        "Source 1",
                        "Random Amount 1",
                        "Increment Steps 1",
                        "Smoothing 1",
                        "Rise 1",
                        "Fall 1",
                    ],
                ),
                (
                    "Curve 1",
                    &[
                        "Curve 1",
                        "Curve A 1",
                        "Curve B 1",
                        "Curve Link 1",
                        "Curve X 1",
                        "Curve Y 1",
                        "#109",
                        "#110",
                    ],
                ),
                (
                    "Source 2",
                    &[
                        "Source 2",
                        "Random Amount 2",
                        "Increment Steps 2",
                        "Smoothing 2",
                        "Rise 2",
                        "Fall 2",
                    ],
                ),
                (
                    "Curve 2",
                    &[
                        "Curve 2",
                        "Curve A 2",
                        "Curve B 2",
                        "Curve Link 2",
                        "Curve X 2",
                        "Curve Y 2",
                        "#125",
                        "#126",
                    ],
                ),
                (
                    "Source 3",
                    &[
                        "Source 3",
                        "Random Amount 3",
                        "Increment Steps 3",
                        "Smoothing 3",
                        "Rise 3",
                        "Fall 3",
                    ],
                ),
                (
                    "Curve 3",
                    &[
                        "Curve 3",
                        "Curve A 3",
                        "Curve B 3",
                        "Curve Link 3",
                        "Curve X 3",
                        "Curve Y 3",
                        "#141",
                        "#142",
                    ],
                ),
                (
                    "Source 4",
                    &[
                        "Source 4",
                        "Random Amount 4",
                        "Increment Steps 4",
                        "Smoothing 4",
                        "Rise 4",
                        "Fall 4",
                    ],
                ),
                (
                    "Curve 4",
                    &[
                        "Curve 4",
                        "Curve A 4",
                        "Curve B 4",
                        "Curve Link 4",
                        "Curve X 4",
                        "Curve Y 4",
                        "#157",
                        "#158",
                    ],
                ),
                (
                    "Source 5",
                    &[
                        "Source 5",
                        "Random Amount 5",
                        "Increment Steps 5",
                        "Smoothing 5",
                        "Rise 5",
                        "Fall 5",
                    ],
                ),
                (
                    "Curve 5",
                    &[
                        "Curve 5",
                        "Curve A 5",
                        "Curve B 5",
                        "Curve Link 5",
                        "Curve X 5",
                        "Curve Y 5",
                        "#173",
                        "#174",
                    ],
                ),
            ]),
    )
}

/// The names Live's panel gives each tab's controls, which the tab's number
/// makes unique for agents: each reads under Live's name (layout L-DEV-021).
fn relabels() -> Vec<Relabel> {
    let names = [
        (Field::Source, "Source"),
        (Field::RandomAmount, "Random"),
        (Field::IncrementSteps, "Steps"),
        (Field::Curve, "Curve Type"),
        (Field::CurveA, "Curve"),
        (Field::CurveB, "Curve"),
        (Field::CurveLink, "Link"),
        (Field::CurveX, "X"),
        (Field::CurveY, "Y"),
        (Field::Min, "Min"),
        (Field::Max, "Max"),
        (Field::Smoothing, "Smooth"),
        (Field::Rise, "Rise"),
        (Field::Fall, "Fall"),
    ];
    (0..TABS)
        .flat_map(|tab| {
            names.iter().map(move |&(field, name)| Relabel {
                param: param(tab, field),
                by: MODE,
                labels: vec![ParamLabel::named(name)],
            })
        })
        .collect()
}

/// One tab's page (layout L-DEV-021): Source, Target, Output Range and Curve
/// down the left; the curve's display with Min, Max, the curve's type, X and Y
/// along its foot; Modulate, Steps, Random, Smooth and Rise | Fall down the
/// right.
fn tab_page(tab: usize) -> LayoutNode {
    let at = Control::new;
    let p = |field| param(tab, field);
    let slot = tab as u8;
    // The device-wide Mod and Polarity are the first tab's controls, which
    // the others' repeat.
    let again = |control: Control| {
        if tab == 0 {
            control
        } else {
            control.repeating()
        }
    };
    let left = ControlBand::free(
        88.0,
        BODY_HEIGHT,
        vec![
            at(p(Field::Source), Widget::Chooser, 4.0, 4.0).sized(80.0, 21.0),
            at(modulator::amount(slot), Widget::Map(slot), 4.0, 42.0).sized(80.0, 12.0),
            again(
                at(POLARITY, Widget::Cycle, 4.0, 71.0)
                    .sized(44.0, 12.0)
                    .only_while(MODE, 0),
            ),
            at(modulator::amount(slot), Widget::Bare, 52.0, 71.0)
                .sized(30.0, 12.0)
                .only_while(MODE, 0),
            at(modulator::min(slot), Widget::Bare, 4.0, 71.0)
                .sized(38.0, 12.0)
                .only_while(MODE, 1),
            at(modulator::max(slot), Widget::Bare, 44.0, 71.0)
                .sized(38.0, 12.0)
                .only_while(MODE, 1),
            at(p(Field::CurveA), Widget::SmallKnob, 0.0, 92.0),
            at(p(Field::CurveLink), Widget::Round, 35.0, 112.0).only_while(p(Field::Curve), 1),
            at(p(Field::CurveB), Widget::SmallKnob, 48.0, 92.0).only_while(p(Field::Curve), 1),
        ],
    )
    .with_labels(vec![
        BandLabel::new("Target", 4.0, 31.0, 80.0),
        BandLabel::new("Output Range", 4.0, 60.0, 80.0),
    ]);
    let curve = DisplayCell::new(&format!("Curve {}", tab + 1), 134.0, 150.0, 1.0)
        .with_inset(4.0)
        .with_bottom(ControlBand::free(
            134.0,
            35.0,
            vec![
                at(p(Field::Min), Widget::Inline, 4.0, 4.0).sized(50.0, 12.0),
                at(p(Field::Max), Widget::Inline, 4.0, 19.0).sized(50.0, 12.0),
                at(p(Field::Curve), Widget::IconSegments(2), 58.0, 4.0).sized(27.0, 12.0),
                at(p(Field::CurveX), Widget::Inline, 88.0, 4.0)
                    .sized(44.0, 12.0)
                    .greyed_while(p(Field::Curve), 0),
                at(p(Field::CurveY), Widget::Inline, 88.0, 19.0)
                    .sized(44.0, 12.0)
                    .greyed_while(p(Field::Curve), 0),
            ],
        ));
    let right = ControlBand::free(
        60.0,
        BODY_HEIGHT,
        vec![
            again(at(MODE, Widget::Cycle, 4.0, 4.0).sized(52.0, 12.0)),
            at(p(Field::IncrementSteps), Widget::Bare, 4.0, 31.0).sized(52.0, 12.0),
            at(p(Field::RandomAmount), Widget::Bare, 4.0, 57.0).sized(52.0, 12.0),
            at(p(Field::Smoothing), Widget::Cycle, 4.0, 83.0).sized(52.0, 12.0),
            at(p(Field::Rise), Widget::Bare, 4.0, 109.0).sized(52.0, 12.0),
            at(p(Field::Fall), Widget::Bare, 4.0, 123.0).sized(52.0, 12.0),
        ],
    )
    .with_labels(vec![
        BandLabel::new("Steps", 4.0, 22.0, 52.0),
        BandLabel::new("Random", 4.0, 48.0, 52.0),
        BandLabel::new("Smooth", 4.0, 74.0, 52.0),
        BandLabel::new("Rise | Fall", 4.0, 100.0, 52.0),
    ]);
    LayoutNode::Row(vec![
        LayoutNode::Band(left),
        LayoutNode::Cell(curve),
        LayoutNode::Band(right),
    ])
}

/// The panel: the five source tabs down its left edge, each its own page.
fn layout() -> DeviceLayout {
    DeviceLayout::new(LayoutNode::Pages(PagesNode::new(
        "Tabs",
        PageStyle::Rail,
        (0..TABS)
            .map(|tab| Page::new(TAB_NAMES[tab], tab_page(tab)))
            .collect(),
    )))
    .with_title(vec![TitlePart::Mpe])
}

/// Adds the Expression Control to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, play_mod_type(), |device, config| {
        PlayMod::new(config.sample_rate.as_f64(), seed(device))
    });
}

/// The shares the notes and MIDI have given each source. The CV tools read
/// them the same way (`docs/specs/cv-tools.md`, R13).
#[derive(Clone, Copy)]
pub(crate) struct Sources {
    pub(crate) velocity: f64,
    pub(crate) modwheel: f64,
    pub(crate) pitchbend: f64,
    pub(crate) pressure: f64,
    pub(crate) keytrack: f64,
    pub(crate) expression: f64,
    pub(crate) slide: f64,
    pub(crate) sustain: f64,
}

impl Sources {
    /// What a note or message says to the sources, with `held` the keys
    /// down after it: a note on gives Velocity and Keytrack and starts Slide
    /// at 0, and the per-note sources follow the latest key held.
    pub(crate) fn take(&mut self, note: &NoteEvent, held: &Held) {
        match note.kind {
            NoteKind::On => {
                self.velocity = f64::from(note.velocity);
                self.keytrack = f64::from(note.key) / 127.0;
                self.slide = 0.0;
            }
            NoteKind::Off | NoteKind::Choke => {}
            NoteKind::Control { controller, value } => match controller {
                1 => self.modwheel = f64::from(value),
                11 => self.expression = f64::from(value),
                64 => self.sustain = f64::from(value),
                _ => {}
            },
            NoteKind::PitchBend { value } => self.pitchbend = f64::from(value + 1.0) / 2.0,
            NoteKind::ChannelPressure { value } => self.pressure = f64::from(value),
            NoteKind::Expression { expression, value } => {
                // Per-note sources follow the most recent note held.
                let latest = held.last().is_some_and(|key| key.names(note));
                if latest {
                    match expression {
                        NoteExpression::Pressure => self.pressure = f64::from(value),
                        NoteExpression::Slide => self.slide = f64::from(value),
                        NoteExpression::Bend => {}
                    }
                }
            }
        }
    }

    /// The share of source `kind`, an index into [`SOURCES`], except Random
    /// and Increment, which a tab keeps.
    pub(crate) fn share(&self, kind: usize) -> f64 {
        match kind {
            0 => self.velocity,
            1 => self.modwheel,
            2 => self.pitchbend,
            3 => self.pressure,
            4 => self.keytrack,
            5 => self.expression,
            8 => self.slide,
            _ => self.sustain,
        }
    }

    pub(crate) const START: Self = Self {
        velocity: 0.0,
        modwheel: 0.0,
        pitchbend: 0.5,
        pressure: 0.0,
        keytrack: 0.0,
        expression: 0.0,
        slide: 0.0,
        sustain: 0.0,
    };
}

struct PlayMod {
    out: SignalOut,
    rate: f64,
    seed: u64,
    random: Random,
    held: Held,
    sources: Sources,
    /// Each tab's random value, and the step its Increment has reached.
    drawn: [f64; TABS],
    step: [Option<u32>; TABS],
    /// Each tab's output after smoothing.
    smoothed: [f64; TABS],
    was_playing: bool,
    /// Each tab's output as the display last read it.
    shown: [f64; TABS],
}

impl PlayMod {
    fn new(rate: f64, seed: u64) -> Self {
        Self {
            out: SignalOut::new(),
            rate,
            seed,
            random: Random::seeded(seed),
            held: Held::new(),
            sources: Sources::START,
            drawn: [0.5; TABS],
            step: [None; TABS],
            smoothed: [0.0; TABS],
            was_playing: false,
            shown: [0.0; TABS],
        }
    }

    /// Takes a note or message.
    fn take(&mut self, note: &NoteEvent, params: &ParamBank, f: usize) {
        let at = |tab: usize, field: Field| {
            f64::from(
                params.ramp(
                    params
                        .index(param(tab, field))
                        .expect("the description holds it"),
                )[f],
            )
        };
        match note.kind {
            NoteKind::On => {
                self.held.on(note);
                self.sources.take(note, &self.held);
                for tab in 0..TABS {
                    // Spec: modulation R27, R21. A draw at each note, whether
                    // or not a tab follows Random, so the stream is the same
                    // whatever the choosers say.
                    let draw = self.random.next_bipolar();
                    self.drawn[tab] = 0.5 + 0.5 * at(tab, Field::RandomAmount) / 100.0 * draw;
                    let steps = at(tab, Field::IncrementSteps).max(1.0) as u32;
                    self.step[tab] = Some(self.step[tab].map_or(0, |s| s + 1) % steps);
                }
            }
            NoteKind::Off | NoteKind::Choke => {
                self.held.off(note);
            }
            _ => self.sources.take(note, &self.held),
        }
    }

    /// Tab `tab`'s source now, a share from 0 to 1.
    fn source(&self, tab: usize, kind: usize, params: &ParamBank, f: usize) -> f64 {
        match kind {
            6 => self.drawn[tab],
            7 => {
                let steps = f64::from(
                    params.ramp(
                        params
                            .index(param(tab, Field::IncrementSteps))
                            .expect("the description holds it"),
                    )[f],
                )
                .max(1.0);
                match self.step[tab] {
                    Some(step) if steps > 1.0 => (f64::from(step) / (steps - 1.0)).min(1.0),
                    _ => 0.0,
                }
            }
            _ => self.sources.share(kind),
        }
    }
}

/// The curve of a tab (R27): the share `x` from 0 to 1 turned into another,
/// before Min and Max. Linear is a line from the bottom to the top bent by
/// A; S runs through the middle breakpoint (`mx`, `my`), the lower segment
/// bent by B (or A's opposite with Link) and the upper by A.
fn curve(s: bool, a: f64, b: f64, link: bool, middle: (f64, f64), x: f64) -> f64 {
    if !s {
        return curve_value(0.0, 1.0, x, a);
    }
    let b = if link { -a } else { b };
    let (mx, my) = middle;
    if x < mx {
        curve_value(0.0, my, x / mx, b)
    } else {
        curve_value(my, 1.0, (x - mx) / (1.0 - mx), a)
    }
}

impl NativeDevice for PlayMod {
    fn is_signal_modulator(&self) -> bool {
        true
    }

    fn lag(&mut self, modulation: &mut Modulation<'_, '_>) {
        self.out.lag(modulation);
    }

    fn release(&mut self, _: &mut bl_engine::NoteBuffer) {
        // Spec: device-framework R32. Switched off, it forgets the keys.
        self.held.clear();
        self.out.off();
    }

    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        let frames = audio.frames();
        let playing = audio.playing();
        if self.out.begin(audio) {
            // Spec: modulation R20, R21. It begins as a render from here
            // does: Random starts again with the transport, and what the
            // last note said is forgotten unless a key is already down.
            self.random = Random::seeded(self.seed);
            self.drawn = [0.5; TABS];
            self.step = [None; TABS];
            self.smoothed = [0.0; TABS];
            if self.held.is_empty() {
                self.sources.velocity = 0.0;
                self.sources.keytrack = 0.0;
                self.sources.slide = 0.0;
            }
        }
        if self.was_playing && !playing {
            // Spec: modulation R27. Increment starts again when the
            // transport stops.
            self.step = [None; TABS];
        }
        self.was_playing = playing;
        let ix = Indices::new(params, PLAY_SLOTS);
        let index = |tab: usize, field: Field| {
            params
                .index(param(tab, field))
                .expect("the description holds it")
        };
        let ramps: [[&[f32]; 14]; TABS] = std::array::from_fn(|tab| {
            std::array::from_fn(|field| params.ramp(index(tab, FIELD_LIST[field])))
        });
        let mut notes = audio.notes().peekable();
        #[expect(
            clippy::needless_range_loop,
            reason = "the frame indexes every tab's ramps, and is what the rows are written at"
        )]
        for f in 0..frames {
            while let Some(note) = notes.next_if(|n| n.offset as usize == f) {
                audio.send(note);
                self.take(&note, params, f);
            }
            let mut levels = [0.0; TABS];
            for (tab, level) in levels.iter_mut().enumerate() {
                let at = |field: Field| f64::from(ramps[tab][field as usize][f]);
                let share = self.source(tab, at(Field::Source) as usize, params, f);
                let s = at(Field::Curve) >= 0.5;
                let (a, b) = (at(Field::CurveA) / 100.0, at(Field::CurveB) / 100.0);
                let middle = (at(Field::CurveX) / RANGE, at(Field::CurveY) / RANGE);
                let y = curve(
                    s,
                    a,
                    b,
                    at(Field::CurveLink) >= 0.5,
                    middle,
                    share.clamp(0.0, 1.0),
                );
                let (low, high) = (at(Field::Min) / RANGE, at(Field::Max) / RANGE);
                let target = low + y * (high - low);
                // Spec: modulation R27. Smoothing, Rise going up, Fall down.
                let rising = target > self.smoothed[tab];
                let ms = at(if rising { Field::Rise } else { Field::Fall });
                let current = self.smoothed[tab];
                self.smoothed[tab] = if ms <= 0.0 {
                    target
                } else if at(Field::Smoothing) >= 0.5 {
                    current + (target - current) * (1.0 - (-1000.0 / (ms * self.rate)).exp())
                } else {
                    let step = 1000.0 / (ms * self.rate);
                    if rising {
                        (current + step).min(target)
                    } else {
                        (current - step).max(target)
                    }
                };
                *level = self.smoothed[tab];
            }
            self.shown = levels;
            self.out.frame(audio, &ix, params, f, &levels);
        }
    }

    fn display_len(&self) -> usize {
        TABS
    }

    fn display(&mut self, out: &mut [f32]) {
        for (value, shown) in out.iter_mut().zip(self.shown) {
            *value = shown as f32;
        }
    }
}

/// The fields in the order of [`Field`]'s numbers, which `ramps` indexes by.
const FIELD_LIST: [Field; 14] = [
    Field::Source,
    Field::RandomAmount,
    Field::IncrementSteps,
    Field::Curve,
    Field::CurveA,
    Field::CurveB,
    Field::CurveLink,
    Field::CurveX,
    Field::CurveY,
    Field::Min,
    Field::Max,
    Field::Smoothing,
    Field::Rise,
    Field::Fall,
];

/// The Expression Control's display (R27): each tab's curve as its controls set it,
/// and each tab's output now from what its node published.
#[must_use]
pub fn display(
    device: DeviceId,
    description: &DeviceDescription,
    values: &OrdMap<ParamId, f64>,
    data: Option<&[f32]>,
) -> DeviceDisplay {
    const POINTS: usize = 48;
    let value = |id: ParamId| description.value(values, id).unwrap_or(0.0);
    let mut out = DeviceDisplay {
        spectrograms: Vec::new(),
        device,
        live: data.is_some(),
        view: None,
        series: Vec::new(),
        markers: Vec::new(),
        readings: Vec::new(),
    };
    for tab in 0..TABS {
        let at = |field: Field| value(param(tab, field));
        let (low, high) = (at(Field::Min) / RANGE, at(Field::Max) / RANGE);
        let samples = (0..=POINTS)
            .map(|i| {
                let x = i as f64 / POINTS as f64;
                let y = curve(
                    at(Field::Curve) >= 0.5,
                    at(Field::CurveA) / 100.0,
                    at(Field::CurveB) / 100.0,
                    at(Field::CurveLink) >= 0.5,
                    (at(Field::CurveX) / RANGE, at(Field::CurveY) / RANGE),
                    x,
                );
                (low + y * (high - low)).clamp(0.0, 1.0) as f32
            })
            .collect();
        out.series.push(DisplaySeries {
            name: format!("Curve {}", tab + 1),
            unit: String::new(),
            x_unit: SOURCES[(at(Field::Source) as usize).min(SOURCES.len() - 1)].to_owned(),
            x_range: [0.0, 1.0],
            log_x: false,
            y_range: [0.0, 1.0],
            detail: false,
            values: samples,
        });
    }
    if let Some(levels) = data.filter(|d| d.len() == TABS) {
        out.readings = levels
            .iter()
            .enumerate()
            .map(|(tab, &level)| DisplayReading {
                name: format!("Output {}", tab + 1),
                value: level * 100.0,
                unit: "%".to_owned(),
                text: format!("{:.0} %", level * 100.0),
            })
            .collect();
    }
    out
}

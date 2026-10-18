//! The Play Mod (WP-5.11; the modulation spec, R27): Beatlayer's equivalent of
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
    BODY_HEIGHT, ControlBand, DeviceLayout, DisplayCell, LayoutNode, Widget,
};
use bl_model::modulator::{self, PLAY_MOD, PLAY_SLOTS};
use bl_model::{
    DeviceDescription, DeviceDisplay, DeviceId, DeviceRole, DisplayReading, DisplaySeries, OrdMap,
    ParamDescriptor, ParamId, ParamUnit,
};

use crate::framework::{Audio, Modulation, NativeDevice, ParamBank, add};
use crate::midi_fx::seed;
use crate::modulator::mapping_params;
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
/// What each tab starts on.
const DEFAULT_SOURCES: [f64; TABS] = [0.0, 1.0, 2.0, 3.0, 4.0];
/// The curve choices.
pub const CURVES: [&str; 2] = ["Linear", "S"];
/// The smoothing choices.
pub const SMOOTHINGS: [&str; 2] = ["Linear", "Logarithmic"];

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

/// The Play Mod's type.
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
            percent(Field::RandomAmount, "Random Amount", (0.0, 100.0), 100.0, "How far each note's random value strays from the middle."),
            ParamDescriptor::integer(
                param(tab, Field::IncrementSteps),
                &name("Increment Steps"),
                (1, 32),
                4,
                ParamUnit::None,
            )
            .describe("How many steps Increment goes through before it starts again."),
            ParamDescriptor::choice(param(tab, Field::Curve), &name("Curve"), &CURVES, 0)
                .describe("Linear: two breakpoints bent by Curve A. S: three, Curve A bending the upper segment and Curve B the lower."),
            percent(Field::CurveA, "Curve A", (-100.0, 100.0), 0.0, "Bends the curve, or its upper segment."),
            percent(Field::CurveB, "Curve B", (-100.0, 100.0), 0.0, "Bends the S curve's lower segment."),
            ParamDescriptor::toggle(param(tab, Field::CurveLink), &name("Curve Link"), false)
                .describe("Bends both segments of the S curve in opposite directions, by Curve A."),
            percent(Field::CurveX, "Curve X", (1.0, 99.0), 50.0, "Where the S curve's middle breakpoint sits across."),
            percent(Field::CurveY, "Curve Y", (0.0, 100.0), 50.0, "Where the S curve's middle breakpoint sits up."),
            percent(Field::Min, "Min", (0.0, 100.0), 0.0, "The curve's output where its input is at the bottom."),
            percent(Field::Max, "Max", (0.0, 100.0), 100.0, "The curve's output where its input is at the top."),
            ParamDescriptor::choice(param(tab, Field::Smoothing), &name("Smoothing"), &SMOOTHINGS, 0)
                .describe("Linear takes the Rise or Fall time for the whole range; Logarithmic follows with that time constant."),
            time(Field::Rise, "Rise", "How slowly the output follows a rising value."),
            time(Field::Fall, "Fall", "How slowly the output follows a falling value."),
        ]);
    }
    params.extend(mapping_params(
        PLAY_SLOTS,
        (
            "Output Range Max",
            "In Modulation mode, how far the modulator reaches: 0 % none, 100 % full.",
        ),
    ));
    DeviceType::native(PLAY_MOD, "Play Mod", DeviceRole::MidiEffect)
        .with_description(DeviceDescription::new(params).with_layout(layout()))
}

/// One tab: its controls, its curve's display and its slot's mapping.
fn tab_layout(tab: usize) -> LayoutNode {
    let controls = ControlBand::stacked(
        BODY_HEIGHT,
        &[
            (param(tab, Field::Source), Widget::Chooser),
            (param(tab, Field::RandomAmount), Widget::Knob),
            (param(tab, Field::IncrementSteps), Widget::Number),
            (param(tab, Field::Smoothing), Widget::Chooser),
            (param(tab, Field::Rise), Widget::Knob),
            (param(tab, Field::Fall), Widget::Knob),
        ],
    );
    let curve = ControlBand::stacked(
        BODY_HEIGHT,
        &[
            (param(tab, Field::Curve), Widget::Chooser),
            (param(tab, Field::CurveA), Widget::Knob),
            (param(tab, Field::CurveB), Widget::Knob),
            (param(tab, Field::CurveLink), Widget::Toggle),
            (param(tab, Field::CurveX), Widget::Knob),
            (param(tab, Field::CurveY), Widget::Knob),
            (param(tab, Field::Min), Widget::Knob),
            (param(tab, Field::Max), Widget::Knob),
        ],
    );
    let slot = tab as u8;
    let mapping = DisplayCell::new(&format!("Mapping {}", tab + 1), 180.0, BODY_HEIGHT, 1.0)
        .with_bottom(ControlBand::stacked(
            62.0,
            &[
                (modulator::amount(slot), Widget::Knob),
                (modulator::min(slot), Widget::Knob),
                (modulator::max(slot), Widget::Knob),
            ],
        ));
    LayoutNode::Row(vec![
        LayoutNode::Band(controls),
        LayoutNode::Cell(DisplayCell::new(
            &format!("Curve {}", tab + 1),
            120.0,
            BODY_HEIGHT,
            1.0,
        )),
        LayoutNode::Band(curve),
        LayoutNode::Cell(mapping),
    ])
}

fn layout() -> DeviceLayout {
    let output = ControlBand::stacked(
        BODY_HEIGHT,
        &[
            (modulator::MODE, Widget::Chooser),
            (modulator::POLARITY, Widget::Chooser),
            (modulator::DEPTH, Widget::Knob),
        ],
    );
    DeviceLayout::new(LayoutNode::Row(vec![
        tab_layout(0),
        LayoutNode::Band(output),
    ]))
    .with_expanded(LayoutNode::Row((1..TABS).map(tab_layout).collect()))
}

/// Adds the Play Mod to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, play_mod_type(), |device, config| {
        PlayMod::new(config.sample_rate.as_f64(), seed(device))
    });
}

/// The shares the notes and MIDI have given each source.
#[derive(Clone, Copy)]
struct Sources {
    velocity: f64,
    modwheel: f64,
    pitchbend: f64,
    pressure: f64,
    keytrack: f64,
    expression: f64,
    slide: f64,
    sustain: f64,
}

impl Sources {
    const START: Self = Self {
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
                self.sources.velocity = f64::from(note.velocity);
                self.sources.keytrack = f64::from(note.key) / 127.0;
                self.sources.slide = 0.0;
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
            NoteKind::Control { controller, value } => match controller {
                1 => self.sources.modwheel = f64::from(value),
                11 => self.sources.expression = f64::from(value),
                64 => self.sources.sustain = f64::from(value),
                _ => {}
            },
            NoteKind::PitchBend { value } => self.sources.pitchbend = f64::from(value + 1.0) / 2.0,
            NoteKind::ChannelPressure { value } => self.sources.pressure = f64::from(value),
            NoteKind::Expression { expression, value } => {
                // Per-note sources follow the most recent note held.
                let latest = self.held.last().is_some_and(|key| key.names(note));
                if latest {
                    match expression {
                        NoteExpression::Pressure => self.sources.pressure = f64::from(value),
                        NoteExpression::Slide => self.sources.slide = f64::from(value),
                        NoteExpression::Bend => {}
                    }
                }
            }
        }
    }

    /// Tab `tab`'s source now, a share from 0 to 1.
    fn source(&self, tab: usize, kind: usize, params: &ParamBank, f: usize) -> f64 {
        match kind {
            0 => self.sources.velocity,
            1 => self.sources.modwheel,
            2 => self.sources.pitchbend,
            3 => self.sources.pressure,
            4 => self.sources.keytrack,
            5 => self.sources.expression,
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
            8 => self.sources.slide,
            _ => self.sources.sustain,
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
                let middle = (at(Field::CurveX) / 100.0, at(Field::CurveY) / 100.0);
                let y = curve(
                    s,
                    a,
                    b,
                    at(Field::CurveLink) >= 0.5,
                    middle,
                    share.clamp(0.0, 1.0),
                );
                let (low, high) = (at(Field::Min) / 100.0, at(Field::Max) / 100.0);
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

/// The Play Mod's display (R27): each tab's curve as its controls set it,
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
        device,
        live: data.is_some(),
        view: None,
        series: Vec::new(),
        markers: Vec::new(),
        readings: Vec::new(),
    };
    for tab in 0..TABS {
        let at = |field: Field| value(param(tab, field));
        let (low, high) = (at(Field::Min) / 100.0, at(Field::Max) / 100.0);
        let samples = (0..=POINTS)
            .map(|i| {
                let x = i as f64 / POINTS as f64;
                let y = curve(
                    at(Field::Curve) >= 0.5,
                    at(Field::CurveA) / 100.0,
                    at(Field::CurveB) / 100.0,
                    at(Field::CurveLink) >= 0.5,
                    (at(Field::CurveX) / 100.0, at(Field::CurveY) / 100.0),
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

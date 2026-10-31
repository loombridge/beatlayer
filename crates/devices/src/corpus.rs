//! Corpus, the native resonating-body effect (WP-8.44; the Corpus spec): it
//! rings its input through two physical resonators of a chosen type, a beam,
//! a marimba bar, a string, a membrane, a plate, a pipe or a tube, with an
//! LFO moving their tuning, a band-pass on the input, Bleed, a limited Gain,
//! and a MIDI sidechain that sets the tuning from a track's notes.
//!
//! The device holds no model maths: both resonators are voices of one
//! `bl_dsp::physical` bank (a [`ModalBank`] for the bars, membranes and plate,
//! a [`WaveguideBank`] for the string, the pipe and the tube), which it sets
//! from its controls. The controls reach the bank on a grid of
//! [`CONTROL`] frames counted from the node's start, and on the frame of each
//! note, so a render is the same at every block size.
//!
//! The manual records no ranges, defaults, mappings or limiter, so those are
//! Beatlayer's own (see `docs/specs/corpus.md`).

use bl_dsp::physical::{
    CONTROL, Kind, ModalBank, ModalParams, Quality, Shape, WaveguideBank, WaveguideParams,
};
use bl_engine::{DeviceType, KeyPitches, NodeConfig, NodeData, NoteEvent, NoteKind, Registry};
use bl_model::device_layout::{BODY_HEIGHT, ControlBand, DisplayCell, LayoutNode, Widget};
use bl_model::{
    Device, DeviceDescription, DeviceDisplay, DeviceId, DeviceLayout, DeviceRole, DisplayReading,
    ParamDescriptor, ParamId, ParamUnit,
};

use crate::framework::{Audio, NativeDevice, ParamBank, add};
use crate::lfo::{Lfo, Rate, Shape as LfoShape, Stereo, Wave};
use crate::phaser_flanger::DIVISIONS;
use crate::sections::{Biquad, Coeffs};

/// Corpus's device type.
pub const CORPUS: &str = "beatlayer.corpus";

/// Type: one of [`TYPES`].
pub const TYPE: ParamId = ParamId(0);
/// Quality: Eco or High, the modal resonators' partial counts.
pub const QUALITY: ParamId = ParamId(1);
/// Decay, in per cent: the fundamental's ring from 50 ms to 30 s.
pub const DECAY: ParamId = ParamId(2);
/// Material: how long the highs ring against the lows, in per cent.
pub const MATERIAL: ParamId = ParamId(3);
/// Radius of a pipe or a tube, in per cent.
pub const RADIUS: ParamId = ParamId(4);
/// Bright: the upper partials' weight, in per cent.
pub const BRIGHT: ParamId = ParamId(5);
/// Inharm: how far the partials are stretched or squeezed, in per cent.
pub const INHARM: ParamId = ParamId(6);
/// Opening of a pipe's far end, in per cent.
pub const OPENING: ParamId = ParamId(7);
/// Ratio of a membrane's or a plate's sides, in per cent.
pub const RATIO: ParamId = ParamId(8);
/// Hit: where the body is excited, from its centre to its edge.
pub const HIT: ParamId = ParamId(9);
/// Pos. L: where the left resonator is listened to.
pub const POS_L: ParamId = ParamId(10);
/// Pos. R: where the right resonator is listened to.
pub const POS_R: ParamId = ParamId(11);
/// Tune: the left resonator's fundamental, in Hz.
pub const TUNE: ParamId = ParamId(12);
/// Fine: cents on top of the tuning.
pub const FINE: ParamId = ParamId(13);
/// Spread: how far the resonators are detuned from each other, in per cent.
pub const SPREAD: ParamId = ParamId(14);
/// Width: how far apart the resonators are heard, in per cent.
pub const WIDTH: ParamId = ParamId(15);
/// LFO Wave: one of [`WAVES`].
pub const LFO_WAVE: ParamId = ParamId(16);
/// LFO Mode: whether the rate is in Hz or synced to the tempo.
pub const LFO_MODE: ParamId = ParamId(17);
/// LFO Amount: how far the LFO moves the tuning, in per cent.
pub const LFO_AMOUNT: ParamId = ParamId(18);
/// LFO Rate in Hz.
pub const LFO_HZ: ParamId = ParamId(19);
/// LFO Beats: the synced division, one of [`DIVISIONS`].
pub const LFO_DIVISION: ParamId = ParamId(20);
/// LFO Offset: degrees against the beat grid, in the synced mode.
pub const LFO_OFFSET: ParamId = ParamId(21);
/// LFO Stereo: Phase or Spin.
pub const LFO_STEREO: ParamId = ParamId(22);
/// LFO Phase: degrees the right LFO is ahead.
pub const LFO_PHASE: ParamId = ParamId(23);
/// LFO Spin: per cent the right LFO's rate is above the left's.
pub const LFO_SPIN: ParamId = ParamId(24);
/// Filter: whether the input passes the band-pass.
pub const FILTER_ON: ParamId = ParamId(25);
/// Filter Freq: the band-pass's centre, in Hz.
pub const FILTER_FREQ: ParamId = ParamId(26);
/// Filter Bdwidth: the band-pass's width, in octaves.
pub const FILTER_WIDTH: ParamId = ParamId(27);
/// Bleed: how much of the unprocessed input joins the resonators' output.
pub const BLEED: ParamId = ParamId(28);
/// Gain: the resonators' level into the limiter, in dB.
pub const GAIN: ParamId = ParamId(29);
/// Dry/Wet, in per cent.
pub const DRY_WET: ParamId = ParamId(30);
/// Frequency: whether the MIDI source's notes set the tuning.
pub const FREQUENCY: ParamId = ParamId(31);
/// Tune Semitones: Tune in the Frequency mode, in semitones.
pub const TUNE_SEMITONES: ParamId = ParamId(32);
/// Off Decay: whether a note-off damps the resonators.
pub const OFF_DECAY: ParamId = ParamId(33);
/// Off Decay Amount, in per cent.
pub const OFF_AMOUNT: ParamId = ParamId(34);
/// Last/Low: which held note sets the tuning.
pub const PRIORITY: ParamId = ParamId(35);
/// PB Range: the pitch bend's range, in semitones.
pub const PB_RANGE: ParamId = ParamId(36);

/// The resonator types, in the chooser's order.
pub const TYPES: [&str; 7] = [
    "Beam", "Marimba", "String", "Membrane", "Plate", "Pipe", "Tube",
];

/// The qualities, in the chooser's order.
pub const QUALITIES: [&str; 2] = ["Eco", "High"];

/// The LFO's waves, in the chooser's order, under the manual's names.
pub const WAVES: [&str; 7] = [
    "Sine",
    "Square",
    "Triangle",
    "Sawtooth Up",
    "Sawtooth Down",
    "Noise (stepped)",
    "Noise (smooth)",
];

/// The effects LFO's waves the chooser's [`WAVES`] play, in order.
const WAVE_OF: [Wave; 7] = [
    Wave::Sine,
    Wave::Rectangle,
    Wave::Triangle,
    Wave::SawUp,
    Wave::SawDown,
    Wave::SampleHold,
    Wave::Random,
];

/// The first noise wave in [`WAVES`]: the stepped one and the smooth one
/// follow, and no stereo mode moves them apart.
const FIRST_NOISE: usize = 5;

/// The LFO's rate modes.
pub const RATE_MODES: [&str; 2] = ["Hz", "Sync"];

/// The LFO's stereo modes.
pub const STEREO_MODES: [&str; 2] = ["Phase", "Spin"];

/// Which held note sets the tuning.
pub const PRIORITIES: [&str; 2] = ["Last", "Low"];

/// The division a new device's LFO starts on: 1/4.
const DEFAULT_DIVISION: usize = 4;

/// The fundamental's ring at Decay 0 and at 100 %, in seconds.
const DECAY_SECONDS: (f64, f64) = (0.05, 30.0);

/// How far a full LFO moves the tuning at an Amount of 100 %, in cents.
const LFO_CENTS: f64 = 2_400.0;

/// How far Spread detunes each resonator at 100 %, in cents.
const SPREAD_CENTS: f64 = 50.0;

/// How long Off Decay at 100 % takes to stop the resonators, in seconds.
const OFF_SECONDS: f64 = 0.05;

/// The key whose note is A3 = 440 Hz, and its pitch.
const A3: f64 = 69.0;
const A3_HZ: f64 = 440.0;

/// The lowest fundamental either bank plays, in Hz: the LFO and Spread can
/// carry Tune's 20 Hz below it, and the waveguides' lines are made for it.
const LOWEST_HZ: f64 = 5.0;

/// The keys of notes held at once that the sidechain remembers.
const HELD_KEYS: usize = 16;

/// The most notes and messages one stretch of a block takes.
const EVENT_ROOM: usize = 4_096;

/// The limiter's release, in seconds.
const RELEASE_SECONDS: f64 = 0.1;

/// Ratio takes a whole number of per cent, so the plate's and membrane's
/// mode shapes are worked out at most once for each.
const RATIO_STEPS: f64 = 100.0;

/// Corpus's device type.
#[must_use]
pub fn corpus_type() -> DeviceType {
    let divisions: Vec<&str> = DIVISIONS.iter().map(|d| d.0).collect();
    let params = vec![
        ParamDescriptor::choice(TYPE, "Type", &TYPES, 0)
            .describe("The body that rings: a beam, a marimba bar, a string, a membrane, a plate, a pipe or a tube."),
        ParamDescriptor::choice(QUALITY, "Quality", &QUALITIES, 1)
            .describe("Eco rings 8 partials and High 64, in the bodies made of partials."),
        ParamDescriptor::continuous(DECAY, "Decay", (0.0, 100.0), 50.0, ParamUnit::Percent)
            .describe("How long the fundamental rings, from 50 ms to 30 s."),
        ParamDescriptor::continuous(MATERIAL, "Material", (-100.0, 100.0), 0.0, ParamUnit::Percent)
            .describe("How long the highs ring against the lows: damped like wood at the bottom, ringing like metal at the top."),
        ParamDescriptor::continuous(RADIUS, "Radius", (0.0, 100.0), 0.0, ParamUnit::Percent)
            .describe("A pipe's or a tube's radius: a larger one rings longer and keeps its highs."),
        ParamDescriptor::continuous(BRIGHT, "Bright", (-100.0, 100.0), 0.0, ParamUnit::Percent)
            .describe("Lifts the upper partials above 0 and cuts them below."),
        ParamDescriptor::continuous(INHARM, "Inharm", (-100.0, 100.0), 0.0, ParamUnit::Percent)
            .describe("Pushes the partials up above 0 and brings them down below."),
        ParamDescriptor::continuous(OPENING, "Opening", (0.0, 100.0), 100.0, ParamUnit::Percent)
            .describe("A pipe's far end: closed at 0, open at both ends at 100."),
        ParamDescriptor::continuous(RATIO, "Ratio", (0.0, 100.0), 50.0, ParamUnit::Percent)
            .describe("The size of a membrane or plate along one axis against the other."),
        ParamDescriptor::continuous(HIT, "Hit", (0.0, 100.0), 30.0, ParamUnit::Percent)
            .describe("Where the body is excited, from its centre to its edge."),
        ParamDescriptor::continuous(POS_L, "Pos. L", (0.0, 100.0), 30.0, ParamUnit::Percent)
            .describe("Where the left resonator is listened to, from its centre to its edge."),
        ParamDescriptor::continuous(POS_R, "Pos. R", (0.0, 100.0), 70.0, ParamUnit::Percent)
            .describe("Where the right resonator is listened to, from its centre to its edge."),
        ParamDescriptor::continuous(TUNE, "Tune", (20.0, 5_000.0), 440.0, ParamUnit::Hertz)
            .logarithmic()
            .describe("The left resonator's fundamental, in Hz."),
        ParamDescriptor::continuous(FINE, "Fine", (-50.0, 50.0), 0.0, ParamUnit::Cents)
            .describe("The tuning in cents."),
        ParamDescriptor::continuous(SPREAD, "Spread", (-100.0, 100.0), 0.0, ParamUnit::Percent)
            .describe("Detunes the right resonator up and the left down, by up to 50 cents each."),
        ParamDescriptor::continuous(WIDTH, "Width", (0.0, 100.0), 100.0, ParamUnit::Percent)
            .describe("Both resonators in the centre at 0, each on its own side at 100."),
        ParamDescriptor::choice(LFO_WAVE, "LFO Wave", &WAVES, 0).describe("The LFO's waveform."),
        ParamDescriptor::choice(LFO_MODE, "LFO Mode", &RATE_MODES, 0)
            .describe("Whether the LFO's rate is in Hz or a beat division."),
        ParamDescriptor::continuous(LFO_AMOUNT, "LFO Amount", (0.0, 100.0), 0.0, ParamUnit::Percent)
            .describe("How far the LFO moves the tuning: at 100 %, up to two octaves."),
        ParamDescriptor::continuous(LFO_HZ, "LFO Rate", (0.01, 40.0), 1.0, ParamUnit::Hertz)
            .logarithmic()
            .describe("The LFO's rate in Hz."),
        ParamDescriptor::choice(LFO_DIVISION, "LFO Beats", &divisions, DEFAULT_DIVISION)
            .describe("The LFO's cycle as a beat division, synced to the tempo."),
        ParamDescriptor::continuous(LFO_OFFSET, "LFO Offset", (-180.0, 180.0), 0.0, ParamUnit::None)
            .decimals(0)
            .describe("Moves the LFO against the beat grid, in degrees, in the synced mode."),
        ParamDescriptor::choice(LFO_STEREO, "LFO Stereo", &STEREO_MODES, 0)
            .describe("Whether the two resonators' LFOs differ in phase or in rate. The noise waves ignore it."),
        ParamDescriptor::continuous(LFO_PHASE, "LFO Phase", (0.0, 360.0), 0.0, ParamUnit::None)
            .decimals(0)
            .describe("How many degrees the right LFO is ahead."),
        ParamDescriptor::continuous(LFO_SPIN, "LFO Spin", (0.0, 50.0), 0.0, ParamUnit::Percent)
            .describe("How much faster the right LFO runs than the left, in the Hz mode."),
        ParamDescriptor::toggle(FILTER_ON, "Filter", false)
            .describe("Passes the input through a band-pass before the resonators."),
        ParamDescriptor::continuous(FILTER_FREQ, "Filter Freq", (50.0, 18_000.0), 1_000.0, ParamUnit::Hertz)
            .logarithmic()
            .describe("The band-pass's centre."),
        ParamDescriptor::continuous(FILTER_WIDTH, "Bdwidth", (0.1, 4.0), 2.0, ParamUnit::None)
            .describe("The band-pass's width, in octaves."),
        ParamDescriptor::continuous(BLEED, "Bleed", (0.0, 100.0), 0.0, ParamUnit::Percent)
            .describe("Mixes some of the unprocessed input into the resonators' output."),
        ParamDescriptor::gain(GAIN, "Gain", 24.0, 0.0)
            .describe("The resonators' level, into a limiter at 0 dBFS that keeps them from clipping."),
        ParamDescriptor::continuous(DRY_WET, "Dry/Wet", (0.0, 100.0), 100.0, ParamUnit::Percent)
            .describe("The balance between the dry signal and the resonators."),
        ParamDescriptor::toggle(FREQUENCY, "Frequency", false)
            .describe("The notes of the MIDI source set the tuning, Tune then reading in semitones."),
        ParamDescriptor::integer(TUNE_SEMITONES, "Tune Semitones", (-48, 48), 0, ParamUnit::Semitones)
            .describe("Tune with Frequency on: the notes' pitch moved by this many semitones."),
        ParamDescriptor::toggle(OFF_DECAY, "Off Decay", false)
            .describe("A note-off with no other note held damps the resonators."),
        ParamDescriptor::continuous(OFF_AMOUNT, "Off Decay Amount", (0.0, 100.0), 50.0, ParamUnit::Percent)
            .describe("How far a note-off damps the decay: 0 leaves it, 100 stops the resonators within 50 ms."),
        ParamDescriptor::choice(PRIORITY, "Last/Low", &PRIORITIES, 0)
            .describe("Which held note sets the tuning: the newest or the lowest."),
        ParamDescriptor::integer(PB_RANGE, "PB Range", (0, 24), 2, ParamUnit::Semitones)
            .describe("How far a full pitch bend of the MIDI source moves the tuning."),
    ];
    DeviceType::native(CORPUS, "Corpus", DeviceRole::AudioEffect)
        .with_midi_sidechain_tap()
        .with_description(
            DeviceDescription::new(params)
                .with_layout(layout())
                .with_banks(&[
                    (
                        "Main",
                        &[
                            "Type", "Decay", "Material", "Bright", "Tune", "Spread", "Gain",
                            "Dry/Wet",
                        ],
                    ),
                    (
                        "Body",
                        &[
                            "Radius", "Inharm", "Opening", "Ratio", "Hit", "Pos. L", "Pos. R",
                            "Quality",
                        ],
                    ),
                    (
                        "LFO",
                        &[
                            "LFO Amount",
                            "LFO Wave",
                            "LFO Mode",
                            "LFO Rate",
                            "LFO Beats",
                            "LFO Stereo",
                            "LFO Phase",
                            "LFO Spin",
                        ],
                    ),
                    ("LFO Shape", &["LFO Offset"]),
                    ("Filter", &["Filter", "Filter Freq", "Bdwidth"]),
                    (
                        "Sidechain",
                        &[
                            "Frequency",
                            "Off Decay",
                            "Off Decay Amount",
                            "Last/Low",
                            "PB Range",
                            "Width",
                            "Bleed",
                            "Fine",
                        ],
                    ),
                    ("Sidechain Tune", &["Tune Semitones"]),
                ]),
        )
}

/// The device's layout: the LFO, the filter, the resonator and the global
/// sections around the X–Y display; the sidechain's own controls in the
/// expanded part.
fn layout() -> DeviceLayout {
    use Widget::{Chooser, Knob, Number, Toggle};
    let band = |controls: &[(ParamId, Widget)]| {
        LayoutNode::Band(ControlBand::stacked(BODY_HEIGHT, controls))
    };
    DeviceLayout::new(LayoutNode::Row(vec![
        band(&[
            (LFO_WAVE, Chooser),
            (LFO_MODE, Chooser),
            (LFO_AMOUNT, Knob),
            (LFO_HZ, Knob),
            (LFO_DIVISION, Chooser),
            (LFO_OFFSET, Knob),
            (LFO_STEREO, Chooser),
            (LFO_PHASE, Knob),
            (LFO_SPIN, Knob),
        ]),
        band(&[
            (FILTER_ON, Toggle),
            (FILTER_FREQ, Knob),
            (FILTER_WIDTH, Knob),
        ]),
        band(&[
            (TYPE, Chooser),
            (QUALITY, Chooser),
            (DECAY, Knob),
            (MATERIAL, Knob),
            (RADIUS, Knob),
            (BRIGHT, Knob),
            (INHARM, Knob),
            (OPENING, Knob),
            (RATIO, Knob),
            (HIT, Knob),
            (POS_L, Knob),
            (POS_R, Knob),
            (TUNE, Knob),
            (FINE, Knob),
            (SPREAD, Knob),
            (WIDTH, Knob),
        ]),
        LayoutNode::Cell(DisplayCell::new(XY_CELL, 140.0, BODY_HEIGHT, 1.0)),
        band(&[(BLEED, Knob), (GAIN, Knob), (DRY_WET, Knob)]),
    ]))
    .with_expanded(band(&[
        (FREQUENCY, Toggle),
        (TUNE_SEMITONES, Number),
        (OFF_DECAY, Toggle),
        (OFF_AMOUNT, Knob),
        (PRIORITY, Chooser),
        (PB_RANGE, Number),
    ]))
}

/// The name of the display cell that is the X–Y control.
pub const XY_CELL: &str = "X-Y";

/// What the type `kind` (an index of [`TYPES`]) doesn't use: its controls
/// are dimmed and keep their values (the Corpus spec, R2). A string has no
/// partials to count or weight, so Quality and Bright dim for it as well.
#[must_use]
pub fn unused(kind: usize) -> &'static [ParamId] {
    match kind {
        0 | 1 => &[RADIUS, OPENING, RATIO],
        3 | 4 => &[RADIUS, OPENING],
        2 => &[QUALITY, BRIGHT, RADIUS, OPENING, RATIO],
        5 => &[
            QUALITY, MATERIAL, BRIGHT, INHARM, HIT, POS_L, POS_R, BLEED, RATIO,
        ],
        _ => &[
            QUALITY, MATERIAL, BRIGHT, INHARM, HIT, POS_L, POS_R, BLEED, OPENING, RATIO,
        ],
    }
}

/// The parameter an X–Y drag's vertical axis moves for the type `kind`:
/// Material, or Radius for a pipe and a tube.
#[must_use]
pub fn xy_vertical(kind: usize) -> ParamId {
    if kind >= 5 { RADIUS } else { MATERIAL }
}

/// Adds the Corpus device to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, corpus_type(), Corpus::new);
}

/// What a type rings with.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Body {
    Modal(Shape),
    Guide(Kind),
}

impl Body {
    fn is_modal(self) -> bool {
        matches!(self, Self::Modal(_))
    }
}

fn body(kind: usize) -> Body {
    match kind {
        0 => Body::Modal(Shape::Beam),
        1 => Body::Modal(Shape::Marimba),
        2 => Body::Guide(Kind::String),
        3 => Body::Modal(Shape::Membrane),
        4 => Body::Modal(Shape::Plate),
        5 => Body::Guide(Kind::Pipe),
        _ => Body::Guide(Kind::Tube),
    }
}

/// The gain that brings a body's level for noise to about the input's, at
/// 0 dB of Gain: the models scale what enters them by their poles' distance
/// from the unit circle, so a long ring is a narrow band and a quiet one.
/// The level of a partial rung by white noise goes as the square root of
/// that distance: with a ring of `decay` seconds at `rate`, 1 − pole is
/// ln 1000 / (rate × decay).
#[must_use]
pub fn makeup(rate: f64, decay: f64) -> f64 {
    (2.0 * rate * decay / LN_1000).sqrt() / 4.0
}

/// ln 1000: 60 dB in amplitude.
const LN_1000: f64 = 6.907_755_278_982_137;

/// The seconds the fundamental rings at a Decay of `percent`.
fn decay_seconds(percent: f64) -> f64 {
    let (from, to) = DECAY_SECONDS;
    from * (to / from).powf(percent.clamp(0.0, 100.0) / 100.0)
}

/// The band-pass's coefficients: unity gain at `hz`, `octaves` wide.
fn band_pass(hz: f64, octaves: f64, rate: f64) -> Coeffs {
    let w0 = core::f64::consts::TAU * (hz / rate).clamp(1.0e-5, 0.49);
    let (sin, cos) = w0.sin_cos();
    let alpha = sin * (core::f64::consts::LN_2 / 2.0 * octaves * w0 / sin).sinh();
    let a0 = 1.0 + alpha;
    Coeffs {
        b0: (alpha / a0) as f32,
        b1: 0.0,
        b2: (-alpha / a0) as f32,
        a1: (-2.0 * cos / a0) as f32,
        a2: ((1.0 - alpha) / a0) as f32,
    }
}

/// The keys held and the pitch they set: the newest or the lowest held, the
/// last one kept after the release, and the pitch bend.
#[derive(Clone, Debug)]
struct Played {
    held: [u8; HELD_KEYS],
    len: usize,
    /// The key that sets the tuning now, if any note has played.
    key: Option<u8>,
    /// The channel's pitch bend, from −1 to 1.
    bend: f64,
    /// Whether a note-off damped the body and no note has come since.
    damped: bool,
}

impl Played {
    fn new() -> Self {
        Self {
            held: [0; HELD_KEYS],
            len: 0,
            key: None,
            bend: 0.0,
            damped: false,
        }
    }

    /// Takes a note or message of the source; `playable` is whether the
    /// set's tuning plays the key, and `off_decay` whether Off Decay is on.
    fn take(&mut self, note: &NoteEvent, playable: bool, off_decay: bool) {
        match note.kind {
            NoteKind::On if playable => {
                if let Some(at) = self.held[..self.len].iter().position(|&k| k == note.key) {
                    self.held.copy_within(at + 1..self.len, at);
                    self.len -= 1;
                }
                if self.len == HELD_KEYS {
                    self.held.copy_within(1.., 0);
                    self.len -= 1;
                }
                self.held[self.len] = note.key;
                self.len += 1;
                self.damped = false;
            }
            NoteKind::Off | NoteKind::Choke => {
                let Some(at) = self.held[..self.len].iter().position(|&k| k == note.key) else {
                    return;
                };
                self.held.copy_within(at + 1..self.len, at);
                self.len -= 1;
                self.damped = self.len == 0 && off_decay;
            }
            NoteKind::PitchBend { value } => self.bend = f64::from(value).clamp(-1.0, 1.0),
            _ => {}
        }
    }

    /// The key that sets the tuning: the lowest or the newest held, or the
    /// last one that did.
    fn current(&mut self, low: bool) -> Option<u8> {
        let held = &self.held[..self.len];
        let key = if low {
            held.iter().copied().min()
        } else {
            held.last().copied()
        };
        self.key = key.or(self.key);
        self.key
    }
}

/// What the controls ask of the two resonators at one frame.
struct Settings {
    /// Each resonator's fundamental, in Hz.
    hz: [f64; 2],
    decay: f64,
    material: f64,
    bright: f64,
    inharm: f64,
    opening: f64,
    radius: f64,
    ratio: f64,
    hit: f64,
    pos: [f64; 2],
}

/// Corpus's audio-thread half.
struct Corpus {
    rate: f64,
    stride: usize,
    modal: ModalBank,
    guide: WaveguideBank,
    /// Which bank rings, and what it was last made for.
    body: Option<Body>,
    table: Option<(Shape, Quality, u32, i32)>,
    lfo: Lfo,
    played: Played,
    keys: KeyPitches,
    filters: [Biquad; 2],
    filter_key: Option<(u64, u64)>,
    /// Gain's make-up ([`makeup`]) and the Decay it was worked out for,
    /// followed frame by frame so it moves with Decay's glide at any block
    /// size.
    makeup: (f32, f64),
    /// The limiter's gain now, and how far it moves toward 1 each frame.
    limit: f64,
    release: f64,
    /// The most the last block took off, in dB.
    reduction: f32,
    events: Vec<NoteEvent>,
    /// The frames the node has run: the control grid counts them.
    counter: u64,
    /// The block's input, its filtered copy, and the resonators' output, per
    /// channel.
    input: [Vec<f32>; 2],
    wet: [Vec<f32>; 2],
    /// A chunk of excitation and of output between two control updates.
    excite: Vec<f32>,
    out: Vec<f32>,
}

impl Corpus {
    fn new(device: &Device, config: &NodeConfig) -> Self {
        let rate = config.sample_rate.as_f64();
        let frames = config.max_frames.max(1);
        let modal = ModalBank::new(rate as f32, 2);
        let stride = modal.stride();
        let buffer = || vec![0.0; frames];
        Self {
            rate,
            stride,
            modal,
            guide: WaveguideBank::new(rate as f32, 2, Kind::String, LOWEST_HZ as f32),
            body: None,
            table: None,
            lfo: Lfo::new(config.sample_rate, device.id.0.get()),
            played: Played::new(),
            keys: KeyPitches::twelve(),
            filters: [Biquad::default(); 2],
            filter_key: None,
            makeup: (f32::NAN, 1.0),
            limit: 1.0,
            release: 1.0 - (-1.0 / (RELEASE_SECONDS * rate)).exp(),
            reduction: 0.0,
            events: Vec::with_capacity(EVENT_ROOM),
            counter: 0,
            input: [buffer(), buffer()],
            wet: [buffer(), buffer()],
            excite: vec![0.0; CONTROL * stride],
            out: vec![0.0; 2 * CONTROL * stride],
        }
    }

    /// Sets both resonators to `settings`, at the frame the controls
    /// changed on.
    fn set(&mut self, kind: usize, quality: usize, s: &Settings) {
        let body = body(kind);
        let fresh = self.body.map(Body::is_modal) != Some(body.is_modal());
        match body {
            Body::Modal(shape) => {
                let quality = if quality == 0 {
                    Quality::Eco
                } else {
                    Quality::High
                };
                // Ratio and Inharm take whole per cent: the mode shapes are
                // worked out again only when Ratio, the shape or the
                // quality moves.
                let step = (s.ratio * RATIO_STEPS).round() as u32;
                let inharm = (s.inharm * 100.0).round() as i32;
                let want = (shape, quality, step, inharm);
                if self.table != Some(want) {
                    self.table = Some(want);
                    let aspect = 0.25 * 16.0_f64.powf(f64::from(step) / RATIO_STEPS);
                    self.modal
                        .rebuild_table(shape, quality, aspect as f32, inharm as f32 / 100.0);
                }
                for v in 0..2 {
                    let params = ModalParams {
                        hz: s.hz[v] as f32,
                        decay: s.decay as f32,
                        material: s.material as f32,
                        brightness: s.bright as f32,
                        hit: s.hit as f32,
                        pos_l: s.pos[v] as f32,
                        pos_r: s.pos[v] as f32,
                    };
                    if fresh {
                        self.modal.reset_voice(v, &params);
                    } else {
                        self.modal.set_params(v, &params);
                    }
                }
            }
            Body::Guide(kind) => {
                if fresh || self.body != Some(body) {
                    self.guide.set_kind(kind);
                }
                let plain = matches!(kind, Kind::Pipe | Kind::Tube);
                for v in 0..2 {
                    let mut params = WaveguideParams {
                        hz: s.hz[v] as f32,
                        decay: s.decay as f32,
                        opening: s.opening as f32,
                        radius: s.radius as f32,
                        ..WaveguideParams::default()
                    };
                    if !plain {
                        // A string's ends are where it is hit and listened
                        // to: 0 at an end and 0.5 at its middle, where the
                        // controls run from the centre to the edge.
                        params.material = s.material as f32;
                        params.inharm = s.inharm.max(0.0) as f32;
                        params.excite = (0.5 * (1.0 - s.hit)) as f32;
                        params.pickup_l = (0.5 * (1.0 - s.pos[v])) as f32;
                        params.pickup_r = params.pickup_l;
                    }
                    if fresh {
                        self.guide.reset_voice(v, &params);
                    } else {
                        self.guide.set_params(v, &params);
                    }
                }
            }
        }
        self.body = Some(body);
    }

    /// Runs the excitation of frames `start..end` of the block through the
    /// active bank and keeps the output.
    fn run(&mut self, start: usize, end: usize) {
        let n = end - start;
        if n == 0 {
            return;
        }
        let stride = self.stride;
        let (excite, out) = (&self.excite[..n * stride], &mut self.out[..2 * n * stride]);
        match self.body {
            Some(Body::Guide(_)) => self.guide.process(excite, out),
            _ => self.modal.process(excite, out),
        }
        for f in 0..n {
            for v in 0..2 {
                self.wet[v][start + f] = out[f * 2 * stride + v];
            }
        }
    }
}

impl NativeDevice for Corpus {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        let frames = audio.frames();
        let channels = audio.channels().min(2);
        if frames == 0 || channels == 0 {
            return;
        }
        let at = |id: ParamId| params.index(id).expect("the description holds it");
        let on = |id: ParamId| params.value(at(id)) >= 0.5;
        let choice = |id: ParamId| params.value(at(id)) as usize;
        let target = |id: ParamId| params.target(at(id));
        let ramp = |id: ParamId| params.ramp(at(id));
        let (rate, stride) = (self.rate, self.stride);

        // 1. The LFO, and this stretch's notes.
        let wave = choice(LFO_WAVE).min(WAVES.len() - 1);
        let synced = choice(LFO_MODE) == 1;
        let lfo_rate = if synced {
            Rate::Beats(DIVISIONS[choice(LFO_DIVISION).min(DIVISIONS.len() - 1)].1)
        } else {
            Rate::Hertz(target(LFO_HZ))
        };
        // Neither Phase nor Spin moves the noise waves apart (R6), and Spin
        // is for the Hz mode.
        let stereo = if wave >= FIRST_NOISE {
            Stereo::Phase(0.0)
        } else if choice(LFO_STEREO) == 1 && !synced {
            Stereo::Spin(target(LFO_SPIN))
        } else {
            Stereo::Phase(target(LFO_PHASE))
        };
        self.lfo.set(audio, lfo_rate, stereo, target(LFO_OFFSET));
        let shape = LfoShape::of(WAVE_OF[wave]);
        let tempo = audio.tempo();
        self.events.clear();
        for note in audio.notes() {
            if self.events.len() < self.events.capacity() {
                self.events.push(note);
            }
        }
        for ch in 0..2 {
            let from = ch.min(channels - 1);
            self.input[ch][..frames].copy_from_slice(audio.channel_mut(from));
        }

        // 2. The controls the grid and the notes move.
        let kind = choice(TYPE).min(TYPES.len() - 1);
        let (quality, low) = (choice(QUALITY), choice(PRIORITY) == 1);
        let (frequency, off_decay) = (on(FREQUENCY), on(OFF_DECAY));
        let filter_on = on(FILTER_ON);
        let (tune, fine, spread) = (ramp(TUNE), ramp(FINE), ramp(SPREAD));
        let (decay, material, bright, inharm) =
            (ramp(DECAY), ramp(MATERIAL), ramp(BRIGHT), ramp(INHARM));
        let (opening, radius, ratio, hit) = (ramp(OPENING), ramp(RADIUS), ramp(RATIO), ramp(HIT));
        let (pos_l, pos_r) = (ramp(POS_L), ramp(POS_R));
        let (amount, off_amount) = (ramp(LFO_AMOUNT), ramp(OFF_AMOUNT));
        let (filter_freq, filter_width) = (ramp(FILTER_FREQ), ramp(FILTER_WIDTH));
        let semitones = target(TUNE_SEMITONES);
        let range = target(PB_RANGE);
        let unit = |v: f32| f64::from(v) / 100.0;

        // 3. Each frame: the notes up to it, the LFO, the control update on
        // the grid and on a note's frame, and each channel's excitation.
        let mut next = 0;
        let mut chunk = 0;
        for i in 0..frames {
            let mut moved = false;
            while let Some(note) = self.events.get(next).filter(|n| n.offset as usize <= i) {
                let playable = self.keys.pitch(note.key).is_some();
                self.played.take(note, playable, off_decay);
                next += 1;
                moved = true;
            }
            let swing = self.lfo.step(tempo, i, shape);
            if moved || self.counter.is_multiple_of(CONTROL as u64) {
                self.run(chunk, i);
                chunk = i;
                let key = self.played.current(low);
                let base = match key {
                    Some(key) if frequency => {
                        let steps = self.played.bend * range;
                        let pitch = self.keys.bent(key, steps).unwrap_or(f64::from(key));
                        A3_HZ * ((pitch - A3 + semitones) / 12.0).exp2()
                    }
                    _ => f64::from(tune[i]),
                };
                let moved_cents = |side: usize| {
                    let spread =
                        if side == 0 { -1.0 } else { 1.0 } * unit(spread[i]) * SPREAD_CENTS;
                    f64::from(fine[i]) + spread + LFO_CENTS * unit(amount[i]) * swing[side]
                };
                let settings = Settings {
                    hz: std::array::from_fn(|side| {
                        (base * (moved_cents(side) / 1_200.0).exp2()).clamp(LOWEST_HZ, rate / 4.0)
                    }),
                    decay: {
                        let ring = decay_seconds(f64::from(decay[i]));
                        if self.played.damped {
                            let share = unit(off_amount[i]);
                            ring.powf(1.0 - share) * OFF_SECONDS.powf(share)
                        } else {
                            ring
                        }
                    },
                    material: (unit(material[i]) + 1.0) / 2.0,
                    bright: unit(bright[i]),
                    inharm: unit(inharm[i]),
                    opening: unit(opening[i]),
                    radius: unit(radius[i]),
                    ratio: unit(ratio[i]),
                    hit: unit(hit[i]),
                    pos: [unit(pos_l[i]), unit(pos_r[i])],
                };
                self.set(kind, quality, &settings);
                // The band-pass follows the controls on the grid too.
                let want = (
                    f64::from(filter_freq[i]).to_bits(),
                    f64::from(filter_width[i]).to_bits(),
                );
                if self.filter_key != Some(want) {
                    self.filter_key = Some(want);
                    let c = band_pass(f64::from(filter_freq[i]), f64::from(filter_width[i]), rate);
                    for filter in &mut self.filters {
                        filter.c = c;
                    }
                }
            }
            for ch in 0..2 {
                let x = self.input[ch][i];
                self.excite[(i - chunk) * stride + ch] = if filter_on {
                    self.filters[ch].process(x)
                } else {
                    x
                };
            }
            self.counter += 1;
        }
        self.run(chunk, frames);

        // 4. The mix: Width, Bleed, Gain into the limiter, and Dry/Wet.
        let (width, bleed) = (ramp(WIDTH), ramp(BLEED));
        let (gain, wet) = (ramp(GAIN), ramp(DRY_WET));
        let bleeds = !matches!(body(kind), Body::Guide(Kind::Pipe | Kind::Tube));
        let mut least = 1.0f64;
        for i in 0..frames {
            let w = unit(width[i]);
            let (a, b) = ((1.0 + w) / 2.0, (1.0 - w) / 2.0);
            let (y0, y1) = (f64::from(self.wet[0][i]), f64::from(self.wet[1][i]));
            if self.makeup.0.to_bits() != decay[i].to_bits() {
                self.makeup = (decay[i], makeup(rate, decay_seconds(f64::from(decay[i]))));
            }
            let gain = f64::from(gain[i]) * self.makeup.1;
            let bleed = if bleeds { unit(bleed[i]) } else { 0.0 };
            let raw = [f64::from(self.input[0][i]), f64::from(self.input[1][i])];
            let sum = [
                (a * y0 + b * y1) * gain + bleed * raw[0],
                (a * y1 + b * y0) * gain + bleed * raw[1],
            ];
            // A limiter with no look-ahead: it takes the gain down to hold
            // the peak at 0 dBFS the frame it passes it, and lets it back
            // up over its release.
            let peak = sum[0].abs().max(sum[1].abs());
            let need = if peak > 1.0 { 1.0 / peak } else { 1.0 };
            self.limit = if need < self.limit {
                need
            } else {
                (self.limit + (1.0 - self.limit) * self.release).min(need)
            };
            least = least.min(self.limit);
            let mix = unit(wet[i]);
            for ch in 0..channels {
                let limited = (sum[ch] * self.limit).clamp(-1.0, 1.0);
                audio.channel_mut(ch)[i] = (raw[ch] * (1.0 - mix) + limited * mix) as f32;
            }
        }
        self.reduction = (20.0 * least.log10()) as f32;
    }

    fn receive_data(&mut self, data: NodeData) {
        if let Some(keys) = data.downcast_ref::<KeyPitches>() {
            self.keys = *keys;
        }
    }

    fn gain_reduction(&self) -> f32 {
        self.reduction
    }

    fn display_len(&self) -> usize {
        1
    }

    fn display(&mut self, out: &mut [f32]) {
        out[0] = self.reduction;
    }
}

/// The display's reading of the limiter: its gain reduction in dB, from the
/// data the node published (the lamp lights while it is below 0).
#[must_use]
pub fn display(device: DeviceId, data: Option<&[f32]>) -> DeviceDisplay {
    let readings = data
        .and_then(|d| d.first())
        .map(|&db| {
            vec![DisplayReading {
                name: LIMITER.to_owned(),
                value: db,
                unit: "dB".to_owned(),
                text: if db < LAMP_DB {
                    format!("Limiter {db:.1} dB")
                } else {
                    "Limiter off".to_owned()
                },
            }]
        })
        .unwrap_or_default();
    DeviceDisplay {
        device,
        live: data.is_some(),
        view: None,
        series: Vec::new(),
        markers: Vec::new(),
        readings,
    }
}

/// The reading the limiter's lamp shows.
pub const LIMITER: &str = "Limiter";

/// The gain reduction at which the lamp is lit, in dB.
pub const LAMP_DB: f32 = -0.01;

#[cfg(test)]
mod tests {
    use super::*;

    fn note(kind: NoteKind, key: u8) -> NoteEvent {
        NoteEvent {
            offset: 0,
            kind,
            channel: 0,
            key,
            velocity: 1.0,
            note_id: None,
        }
    }

    // Spec: corpus R9, R10
    #[test]
    fn last_plays_the_newest_held_note_and_low_the_lowest() {
        let mut played = Played::new();
        for key in [64, 60, 67] {
            played.take(&note(NoteKind::On, key), true, false);
        }
        assert_eq!(played.current(false), Some(67));
        assert_eq!(played.current(true), Some(60));
        played.take(&note(NoteKind::Off, 67), true, false);
        assert_eq!(played.current(false), Some(60), "the newest still held");
        played.take(&note(NoteKind::Off, 60), true, false);
        assert_eq!(played.current(false), Some(64));
        played.take(&note(NoteKind::Off, 64), true, false);
        assert_eq!(played.current(false), Some(64), "the last pitch stays");
        assert!(!played.damped, "Off Decay was off");
    }

    // Spec: corpus R10
    #[test]
    fn off_decay_damps_when_the_last_held_note_ends_and_a_note_undoes_it() {
        let mut played = Played::new();
        played.take(&note(NoteKind::On, 60), true, true);
        played.take(&note(NoteKind::On, 64), true, true);
        played.take(&note(NoteKind::Off, 64), true, true);
        assert!(!played.damped, "60 is still held");
        played.take(&note(NoteKind::Off, 60), true, true);
        assert!(played.damped);
        played.take(&note(NoteKind::On, 62), true, true);
        assert!(!played.damped);
    }

    // Spec: corpus R3
    #[test]
    fn decay_runs_from_fifty_milliseconds_to_thirty_seconds_logarithmically() {
        assert!((decay_seconds(0.0) - 0.05).abs() < 1e-12);
        assert!((decay_seconds(100.0) - 30.0).abs() < 1e-9);
        assert!((decay_seconds(50.0) - (0.05f64 * 30.0).sqrt()).abs() < 1e-9);
    }

    /// |H| of the biquad `c` at `hz`, in dB.
    fn response_db(c: Coeffs, hz: f64, rate: f64) -> f64 {
        let w = core::f64::consts::TAU * hz / rate;
        let (z1, z2) = ((w).cos().mul_add(1.0, 0.0), (2.0 * w).cos());
        let (s1, s2) = ((-w).sin(), (-2.0 * w).sin());
        let (b0, b1, b2) = (f64::from(c.b0), f64::from(c.b1), f64::from(c.b2));
        let (a1, a2) = (f64::from(c.a1), f64::from(c.a2));
        let num = (b0 + b1 * z1 + b2 * z2).hypot(b1 * s1 + b2 * s2);
        let den = (1.0 + a1 * z1 + a2 * z2).hypot(a1 * s1 + a2 * s2);
        20.0 * (num / den).log10()
    }

    // Spec: corpus R7
    #[test]
    fn the_band_pass_measures_within_half_a_db_of_its_design_and_is_3_db_down_at_its_width() {
        for rate in [48_000.0, 96_000.0] {
            for (centre, octaves) in [
                (1_000.0, 2.0),
                (50.0, 1.0),
                (4_000.0, 0.5),
                (2_000.0, 1.0),
                (500.0, 4.0),
                (12_000.0, 4.0),
            ] {
                let design = band_pass(centre, octaves, rate);
                let mut filter = Biquad::default();
                filter.set(design);
                let mut measure = |hz: f64| {
                    filter = Biquad::default();
                    filter.set(design);
                    let n = (rate as usize) * 2;
                    let mut sum = 0.0;
                    for i in 0..n {
                        let x = (core::f64::consts::TAU * hz * i as f64 / rate).sin() as f32;
                        let y = filter.process(x);
                        if i >= n / 2 {
                            sum += f64::from(y).powi(2);
                        }
                    }
                    20.0 * ((sum / (n / 2) as f64).sqrt() * 2f64.sqrt()).log10()
                };
                for hz in [
                    centre / 4.0,
                    centre / 2.0,
                    centre,
                    centre * 1.5,
                    centre * 3.0,
                ] {
                    if hz < rate / 2.2 {
                        let (measured, wanted) = (measure(hz), response_db(design, hz, rate));
                        assert!(
                            (measured - wanted).abs() < 0.5,
                            "{centre} Hz, {octaves} oct at {rate}: {measured} dB at {hz}, design {wanted}"
                        );
                    }
                }
                // Unity at the centre; −3 dB where the width ends, away from
                // the warp near Nyquist.
                assert!(response_db(design, centre, rate).abs() < 0.1);
                if centre * (octaves / 2.0).exp2() < rate / 8.0 {
                    for hz in [
                        centre * (-octaves / 2.0).exp2(),
                        centre * (octaves / 2.0).exp2(),
                    ] {
                        let edge = response_db(design, hz, rate);
                        assert!(
                            (edge + 3.0).abs() < 0.5,
                            "{centre} Hz, {octaves} oct: {edge} dB at {hz}"
                        );
                    }
                }
            }
        }
    }
}

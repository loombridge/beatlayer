//! Drum Sampler, the one-shot drum sampler (WP-5.18; the one-shot drum sampler
//! spec): one sample played from a start and a length through an
//! attack-hold-decay envelope, with a playback-effect section of nine
//! effects, a filter, and a global section whose one modulation route moves
//! a destination from the note's velocity or slide.
//!
//! The sample is the decoded audio the media crate gives the engine, sent to
//! this node as node data (as the sampler's is): the voices read it at the
//! positions they need, so nothing here reads a file or allocates. Every
//! value that changes within a block is read from the parameter bank's
//! ramps, frame by frame, and every voice counts its own frames, so a render
//! is the same at any block size.

use std::f64::consts::TAU;
use std::sync::Arc;

use bl_engine::{
    DeviceType, KeyPitches, NodeConfig, NodeData, NoteEvent, NoteExpression, NoteKind, Registry,
    SlotSample,
};
use bl_media::SampleData;
use bl_model::device_layout::{BODY_HEIGHT, ControlBand, DisplayCell, LayoutNode, Widget};
use bl_model::{DeviceDescription, DeviceLayout, DeviceRole, ParamDescriptor, ParamId, ParamUnit};

use crate::eq::{Type, coeffs};
use crate::framework::{Audio, NativeDevice, ParamBank, add};
use crate::sampler::{frames_of, read};
use crate::sections::Biquad;

/// The drum sampler device type's id.
pub const DRUM_SAMPLER: &str = "beatlayer.strike";

/// Where the played region starts, as a share of the sample.
pub const START: ParamId = ParamId(0);
/// How long the played region is, as a share of the sample.
pub const LENGTH: ParamId = ParamId(1);
/// The sample's own gain stage.
pub const SAMPLE_GAIN: ParamId = ParamId(2);
/// The envelope's attack time.
pub const ATTACK: ParamId = ParamId(3);
/// How long the envelope holds its peak (Trigger mode).
pub const HOLD: ParamId = ParamId(4);
/// Whether Hold is infinite: the sample plays to its end (Trigger mode).
pub const HOLD_INF: ParamId = ParamId(5);
/// The envelope's decay time.
pub const DECAY: ParamId = ParamId(6);
/// Trigger (the note's end is ignored) or Gate (the note's end starts the
/// decay).
pub const ENV_MODE: ParamId = ParamId(7);
/// Transposes the sample, in whole semitones.
pub const TRANSPOSE: ParamId = ParamId(8);
/// Fine-tunes the sample, in cents.
pub const DETUNE: ParamId = ParamId(9);
/// Whether the playback effect runs.
pub const FX_ON: ParamId = ParamId(10);
/// Which playback effect runs.
pub const FX_TYPE: ParamId = ParamId(11);
/// The effect's first parameter, from 0 to 100 % of its range.
pub const FX1: ParamId = ParamId(12);
/// The effect's second parameter, from 0 to 100 % of its range.
pub const FX2: ParamId = ParamId(13);
/// Whether the sound runs through the filter.
pub const FILTER_ON: ParamId = ParamId(14);
/// The filter's type.
pub const FILTER_TYPE: ParamId = ParamId(15);
/// The filter's frequency.
pub const FILTER_FREQ: ParamId = ParamId(16);
/// The low-pass and high-pass types' resonance.
pub const FILTER_RES: ParamId = ParamId(17);
/// The peak type's boost or cut.
pub const FILTER_GAIN: ParamId = ParamId(18);
/// The output level.
pub const VOLUME: ParamId = ParamId(19);
/// Where the sound sits in the stereo field.
pub const PAN: ParamId = ParamId(20);
/// How much note velocity scales the volume.
pub const VELOCITY_TO_VOLUME: ParamId = ParamId(21);
/// The modulation route's source: velocity or slide.
pub const MOD_SOURCE: ParamId = ParamId(22);
/// The modulation route's destination.
pub const MOD_DEST: ParamId = ParamId(23);
/// How far the source moves the destination.
pub const MOD_AMOUNT: ParamId = ParamId(24);
/// Whether a per-note pitch bend moves its note.
pub const PER_NOTE_BEND: ParamId = ParamId(25);
/// Whether the envelope's times follow the sample's pitch.
pub const ENV_FOLLOWS_PITCH: ParamId = ParamId(26);

/// The envelope modes.
pub const ENV_MODES: [&str; 2] = ["Trigger", "Gate"];

/// The playback effects, in the chooser's order.
pub const EFFECTS: [&str; 9] = [
    "Stretch",
    "Loop",
    "Pitch Env",
    "Punch",
    "8-Bit",
    "FM",
    "Ring Mod",
    "Sub Osc",
    "Noise Osc",
];

/// The filter types.
pub const FILTER_TYPES: [&str; 4] = ["Low-pass 12", "Low-pass 24", "High-pass 24", "Peak"];

/// The modulation sources.
pub const MOD_SOURCES: [&str; 2] = ["Velocity", "Slide"];

/// The modulation destinations; FX1 and FX2 are the effect's two
/// parameters.
pub const MOD_DESTS: [&str; 6] = ["Filter", "Attack", "Hold", "Decay", "FX1", "FX2"];

/// How many notes sound at once.
const POLYPHONY: usize = 8;

/// The slots a voice can sound in: the sounding voices and room for the
/// ones fading out after a retrigger or a steal.
const SLOTS: usize = 12;

/// How long a stolen or retriggered voice fades out, in milliseconds
/// (Beatlayer's own).
const STEAL_FADE_MS: f64 = 5.0;

/// The envelope's longest Attack and Decay, and Hold's, in milliseconds.
const ENVELOPE_MAX_MS: f64 = 10_000.0;

/// How far a per-note bend of 1 moves a note, in semitones (Beatlayer's
/// own, the instrument's pitch-bend range; the polysynth's is the same).
const BEND_SEMITONES: f64 = 48.0;

/// How far the modulation route moves the filter's frequency at full, in
/// semitones each way (Beatlayer's own).
const MOD_FILTER_SEMITONES: f64 = 48.0;

/// How far the modulation route stretches the envelope's times at full, in
/// octaves each way: a factor of 8 (Beatlayer's own).
const MOD_TIME_OCTAVES: f64 = 3.0;

/// The filter's coefficients are redesigned every this many frames of a
/// voice's life.
const CONTROL_INTERVAL: u32 = 16;

/// A low-pass or high-pass section's Q at no resonance (Butterworth) and
/// how much more full resonance adds (Beatlayer's own).
const Q_BASE: f64 = std::f64::consts::FRAC_1_SQRT_2;
const Q_RESONANCE: f64 = 11.3;

/// The peak filter's Q.
const PEAK_Q: f64 = 1.0;

/// Stretch: the factor runs 0.25 to 4 times the sample's length across FX1,
/// and the grain 10 to 200 ms across FX2.
const STRETCH_MIN: f64 = 0.25;
const STRETCH_RATIO: f64 = 16.0;
const GRAIN_MIN_MS: f64 = 10.0;
const GRAIN_RATIO: f64 = 20.0;

/// Loop: the loop's length runs 5 ms to 2 s across FX2; FX1 places its
/// start across the rest of the region.
const LOOP_MIN_MS: f64 = 5.0;
const LOOP_RATIO: f64 = 400.0;

/// Pitch Env: full amount moves the pitch this many semitones; the return
/// takes 10 ms to 2 s across FX2.
const PITCH_ENV_SEMITONES: f64 = 24.0;
const PITCH_ENV_MIN_MS: f64 = 10.0;
const PITCH_ENV_RATIO: f64 = 200.0;

/// Punch: the first few milliseconds pass untouched, then the gain drops by
/// up to this many decibels and returns over 20 ms to 2 s.
const PUNCH_MAX_DB: f64 = 24.0;
const PUNCH_TRANSIENT_MS: f64 = 5.0;
const PUNCH_DUCK_IN_MS: f64 = 1.0;
const PUNCH_MIN_MS: f64 = 20.0;
const PUNCH_RATIO: f64 = 100.0;

/// 8-Bit: the sample rate runs 500 Hz to 44 kHz across FX1; the built-in
/// low-pass sweeps from 16 kHz down to a quarter of that rate over 20 ms to
/// 2 s across FX2.
const BITS_MIN_HZ: f64 = 500.0;
const BITS_RATIO: f64 = 88.0;
const BITS_LP_START_HZ: f64 = 16_000.0;
const BITS_LP_MIN_HZ: f64 = 500.0;
const BITS_DECAY_MIN_MS: f64 = 20.0;
const BITS_DECAY_RATIO: f64 = 100.0;

/// FM and Ring Mod: the modulator runs 20 Hz to 5 kHz across FX2 (at C3,
/// following the pitch); FM's depth at full is this many milliseconds of
/// the sample.
const MOD_MIN_HZ: f64 = 20.0;
const MOD_RATIO: f64 = 250.0;
const FM_DEPTH_MS: f64 = 1.0;

/// Sub Osc: the frequency runs 30 to 120 Hz across FX2.
const SUB_MIN_HZ: f64 = 30.0;
const SUB_SPAN_HZ: f64 = 90.0;

/// Noise Osc: the colour runs 200 Hz to 20 kHz across FX2; full amount is
/// this peak level.
const NOISE_MIN_HZ: f64 = 200.0;
const NOISE_RATIO: f64 = 100.0;
const NOISE_PEAK: f32 = 0.5;

/// The Drum Sampler device type.
#[must_use]
pub fn drum_sampler_type() -> DeviceType {
    let time = |id, name: &str, default: f64| {
        ParamDescriptor::continuous(
            id,
            name,
            (0.0, ENVELOPE_MAX_MS),
            default,
            ParamUnit::Milliseconds,
        )
    };
    let percent = |id, name: &str, default: f64| {
        ParamDescriptor::continuous(id, name, (0.0, 100.0), default, ParamUnit::Percent)
    };
    let params = vec![
        percent(START, "Start", 0.0)
            .describe("Where in the sample the played region starts, as a share of it."),
        percent(LENGTH, "Length", 100.0)
            .describe("How long the played region is, as a share of the sample from Start."),
        ParamDescriptor::gain(SAMPLE_GAIN, "Sample Gain", 24.0, 0.0).describe(
            "The sample's own gain stage, before the effect and the filter; reaches silence at \
             its bottom.",
        ),
        time(ATTACK, "Attack", 0.0)
            .describe("The time the level takes to rise to its peak after a note starts."),
        time(HOLD, "Hold", 300.0).describe(
            "Trigger mode: how long the level stays at its peak. Ignored in Gate mode and while \
             Hold Inf is on.",
        ),
        ParamDescriptor::toggle(HOLD_INF, "Hold Inf", true).describe(
            "Trigger mode: holds the peak until the sample's end, so the whole sample plays.",
        ),
        time(DECAY, "Decay", 200.0).describe(
            "The time the level takes to fall to silence after Hold in Trigger mode, or after \
             the note ends in Gate mode. Also times the FM and Ring Mod effects' fade.",
        ),
        ParamDescriptor::choice(ENV_MODE, "Env Mode", &ENV_MODES, 0).describe(
            "Trigger plays the sample through whatever the note does; Gate starts the decay when \
             the note ends.",
        ),
        ParamDescriptor::integer(TRANSPOSE, "Transpose", (-48, 48), 0, ParamUnit::Semitones)
            .describe("Transposes the sample in whole semitones; C3 plays it at its own pitch."),
        ParamDescriptor::continuous(DETUNE, "Detune", (-50.0, 50.0), 0.0, ParamUnit::Cents)
            .describe("Fine-tunes the sample in cents."),
        ParamDescriptor::toggle(FX_ON, "Effect", false)
            .describe("Turns the playback effect section on."),
        ParamDescriptor::choice(FX_TYPE, "Effect Type", &EFFECTS, 0).describe(
            "Which playback effect runs: Stretch, Loop, Pitch Env, Punch, 8-Bit, FM, Ring Mod, \
             Sub Osc or Noise Osc.",
        ),
        percent(FX1, "FX1", 50.0).describe(
            "The effect's first parameter over its whole range: Stretch's factor, Loop's offset, \
             Pitch Env's amount, Punch's amount, 8-Bit's sample rate, FM's amount, Ring Mod's \
             amount, Sub Osc's level or Noise Osc's level.",
        ),
        percent(FX2, "FX2", 50.0).describe(
            "The effect's second parameter over its whole range: Stretch's grain size, Loop's \
             length, Pitch Env's decay, Punch's release, 8-Bit's decay time, FM's or Ring Mod's \
             frequency, Sub Osc's frequency or Noise Osc's colour.",
        ),
        ParamDescriptor::toggle(FILTER_ON, "Filter", false)
            .describe("Runs the sound through the filter; off bypasses it."),
        ParamDescriptor::choice(FILTER_TYPE, "Filter Type", &FILTER_TYPES, 0).describe(
            "A low-pass at 12 or 24 dB per octave, a high-pass at 24 dB per octave, or a peak \
             that boosts or cuts a band.",
        ),
        ParamDescriptor::continuous(
            FILTER_FREQ,
            "Frequency",
            (20.0, 20_000.0),
            1_000.0,
            ParamUnit::Hertz,
        )
        .logarithmic()
        .describe("The filter's cutoff, or the peak's centre."),
        percent(FILTER_RES, "Resonance", 0.0)
            .describe("Raises the low-pass and high-pass types' resonance at the cutoff."),
        ParamDescriptor::continuous(
            FILTER_GAIN,
            "Peak Gain",
            (-24.0, 24.0),
            0.0,
            ParamUnit::Decibels,
        )
        .describe("The peak type's boost or cut at its centre."),
        ParamDescriptor::gain(VOLUME, "Volume", 36.0, 0.0)
            .describe("The device's output level; reaches silence at its bottom."),
        ParamDescriptor::continuous(PAN, "Pan", (-50.0, 50.0), 0.0, ParamUnit::None)
            .decimals(0)
            .describe("Where the sound sits in the stereo field."),
        percent(VELOCITY_TO_VOLUME, "Velocity to Volume", 100.0).describe(
            "How much a note's velocity scales its volume: at 0 it doesn't, at 100 % the level \
             follows it.",
        ),
        ParamDescriptor::choice(MOD_SOURCE, "Mod Source", &MOD_SOURCES, 0)
            .describe("What moves the modulation destination: the note's velocity or its slide."),
        ParamDescriptor::choice(MOD_DEST, "Mod Destination", &MOD_DESTS, 0).describe(
            "What the modulation moves: the filter's frequency, the envelope's Attack, Hold or \
             Decay, or the effect's FX1 or FX2.",
        ),
        ParamDescriptor::continuous(
            MOD_AMOUNT,
            "Mod Amount",
            (-100.0, 100.0),
            0.0,
            ParamUnit::Percent,
        )
        .describe("How far the source moves the destination."),
        ParamDescriptor::toggle(PER_NOTE_BEND, "Enable Per-Note Pitch Bend", true)
            .describe("Lets a per-note pitch bend move its note."),
        ParamDescriptor::toggle(ENV_FOLLOWS_PITCH, "Envelope Follows Pitch", false).describe(
            "Scales the envelope's times with the sample's pitch, so a transposed sample is \
             shaped over the same part of it.",
        ),
    ];
    DeviceType::native(DRUM_SAMPLER, "Drum Sampler", DeviceRole::Instrument).with_description(
        DeviceDescription::new(params)
            .with_layout(layout())
            .with_menu_options(&[PER_NOTE_BEND, ENV_FOLLOWS_PITCH])
            .with_banks(&[
                (
                    "Sample",
                    &[
                        "Start",
                        "Length",
                        "Sample Gain",
                        "Transpose",
                        "Detune",
                        "Volume",
                        "Pan",
                        "Velocity to Volume",
                    ],
                ),
                (
                    "Envelope",
                    &["Attack", "Hold", "Hold Inf", "Decay", "Env Mode"],
                ),
                ("Effect", &["Effect", "Effect Type", "FX1", "FX2"]),
                (
                    "Filter",
                    &[
                        "Filter",
                        "Filter Type",
                        "Frequency",
                        "Resonance",
                        "Peak Gain",
                    ],
                ),
                (
                    "Modulation",
                    &["Mod Source", "Mod Destination", "Mod Amount"],
                ),
            ]),
    )
}

/// A layout of fixed control bands and the sample's display, which grows.
fn layout() -> DeviceLayout {
    let sample = ControlBand::stacked(
        BODY_HEIGHT,
        &[
            (START, Widget::Knob),
            (LENGTH, Widget::Knob),
            (SAMPLE_GAIN, Widget::Knob),
            (TRANSPOSE, Widget::Number),
            (DETUNE, Widget::Knob),
        ],
    );
    let envelope = ControlBand::stacked(
        BODY_HEIGHT,
        &[
            (ENV_MODE, Widget::Chooser),
            (ATTACK, Widget::Knob),
            (HOLD, Widget::Knob),
            (HOLD_INF, Widget::Toggle),
            (DECAY, Widget::Knob),
        ],
    );
    let effect = ControlBand::stacked(
        BODY_HEIGHT,
        &[
            (FX_ON, Widget::Toggle),
            (FX_TYPE, Widget::Chooser),
            (FX1, Widget::Knob),
            (FX2, Widget::Knob),
        ],
    );
    let filter = ControlBand::stacked(
        BODY_HEIGHT,
        &[
            (FILTER_ON, Widget::Toggle),
            (FILTER_TYPE, Widget::Chooser),
            (FILTER_FREQ, Widget::Knob),
            (FILTER_RES, Widget::Knob),
            (FILTER_GAIN, Widget::Knob),
        ],
    );
    let global = ControlBand::stacked(
        BODY_HEIGHT,
        &[
            (VOLUME, Widget::Knob),
            (PAN, Widget::Knob),
            (VELOCITY_TO_VOLUME, Widget::Knob),
            (MOD_SOURCE, Widget::Chooser),
            (MOD_DEST, Widget::Chooser),
            (MOD_AMOUNT, Widget::Knob),
        ],
    );
    // The sample's waveform and the effect's X-Y pad have no drawing yet:
    // the display cell is left for them.
    let display = DisplayCell::new("Sample", 160.0, BODY_HEIGHT, 1.0);
    DeviceLayout::new(LayoutNode::Row(vec![
        LayoutNode::Cell(display),
        LayoutNode::Band(sample),
        LayoutNode::Band(envelope),
        LayoutNode::Band(effect),
        LayoutNode::Band(filter),
        LayoutNode::Band(global),
    ]))
}

/// Adds Drum Sampler to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, drum_sampler_type(), |_, config| {
        DrumSampler::new(config)
    });
    registry.swap_samples_in_place(DRUM_SAMPLER);
}

/// The playback effects, by their chooser index.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Effect {
    None,
    Stretch,
    Loop,
    PitchEnv,
    Punch,
    EightBit,
    Fm,
    RingMod,
    SubOsc,
    NoiseOsc,
}

impl Effect {
    fn of(on: bool, index: usize) -> Self {
        if !on {
            return Self::None;
        }
        match index {
            0 => Self::Stretch,
            1 => Self::Loop,
            2 => Self::PitchEnv,
            3 => Self::Punch,
            4 => Self::EightBit,
            5 => Self::Fm,
            6 => Self::RingMod,
            7 => Self::SubOsc,
            _ => Self::NoiseOsc,
        }
    }

    /// Whether it makes sound of its own after the sample ends.
    fn makes_sound(self) -> bool {
        matches!(self, Self::SubOsc | Self::NoiseOsc)
    }
}

/// What the modulation route moves.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Destination {
    Filter,
    Attack,
    Hold,
    Decay,
    Fx1,
    Fx2,
}

/// The values that don't glide, for a stretch of a block: a stepped
/// parameter changes at once, on the frame that starts a stretch.
#[derive(Clone, Copy)]
struct Settings {
    rate: f64,
    gate: bool,
    hold_inf: bool,
    transpose: f64,
    effect: Effect,
    filter_on: bool,
    filter_type: usize,
    slide_source: bool,
    destination: Destination,
    bend: bool,
    follows_pitch: bool,
    /// The pitch of every key under the set's tuning (the tunings spec,
    /// R12); filled in by the caller.
    keys: KeyPitches,
}

impl Settings {
    fn read(params: &ParamBank, rate: f64) -> Self {
        let at = |id: ParamId| params.value(id.0 as usize);
        Self {
            rate,
            gate: at(ENV_MODE) >= 0.5,
            hold_inf: at(HOLD_INF) >= 0.5,
            transpose: at(TRANSPOSE),
            effect: Effect::of(at(FX_ON) >= 0.5, at(FX_TYPE) as usize),
            filter_on: at(FILTER_ON) >= 0.5,
            filter_type: (at(FILTER_TYPE) as usize).min(FILTER_TYPES.len() - 1),
            slide_source: at(MOD_SOURCE) >= 0.5,
            destination: match at(MOD_DEST) as usize {
                1 => Destination::Attack,
                2 => Destination::Hold,
                3 => Destination::Decay,
                4 => Destination::Fx1,
                5 => Destination::Fx2,
                _ => Destination::Filter,
            },
            bend: at(PER_NOTE_BEND) >= 0.5,
            follows_pitch: at(ENV_FOLLOWS_PITCH) >= 0.5,
            keys: KeyPitches::twelve(),
        }
    }
}

/// The parameters that glide, as the bank's ramps give them frame by frame.
struct Ramps<'p> {
    start: &'p [f32],
    length: &'p [f32],
    gain: &'p [f32],
    attack: &'p [f32],
    hold: &'p [f32],
    decay: &'p [f32],
    detune: &'p [f32],
    fx1: &'p [f32],
    fx2: &'p [f32],
    freq: &'p [f32],
    res: &'p [f32],
    peak_gain: &'p [f32],
    volume: &'p [f32],
    pan: &'p [f32],
    velocity: &'p [f32],
    amount: &'p [f32],
}

impl<'p> Ramps<'p> {
    fn new(params: &'p ParamBank) -> Self {
        let ramp = |id: ParamId| params.ramp(id.0 as usize);
        Self {
            start: ramp(START),
            length: ramp(LENGTH),
            gain: ramp(SAMPLE_GAIN),
            attack: ramp(ATTACK),
            hold: ramp(HOLD),
            decay: ramp(DECAY),
            detune: ramp(DETUNE),
            fx1: ramp(FX1),
            fx2: ramp(FX2),
            freq: ramp(FILTER_FREQ),
            res: ramp(FILTER_RES),
            peak_gain: ramp(FILTER_GAIN),
            volume: ramp(VOLUME),
            pan: ramp(PAN),
            velocity: ramp(VELOCITY_TO_VOLUME),
            amount: ramp(MOD_AMOUNT),
        }
    }

    fn live(&self, frame: usize) -> Live {
        let at = |ramp: &[f32]| f64::from(ramp[frame]);
        Live {
            start: at(self.start) / 100.0,
            length: at(self.length) / 100.0,
            gain: self.gain[frame],
            attack_ms: at(self.attack),
            hold_ms: at(self.hold),
            decay_ms: at(self.decay),
            detune: at(self.detune) / 100.0,
            fx1: at(self.fx1) / 100.0,
            fx2: at(self.fx2) / 100.0,
            freq: at(self.freq),
            res: at(self.res) / 100.0,
            peak_gain: at(self.peak_gain),
            velocity_to_volume: self.velocity[frame] / 100.0,
            amount: at(self.amount) / 100.0,
        }
    }
}

/// The gliding parameters' values on one frame.
#[derive(Clone, Copy)]
struct Live {
    start: f64,
    length: f64,
    gain: f32,
    attack_ms: f64,
    hold_ms: f64,
    decay_ms: f64,
    /// In semitones.
    detune: f64,
    fx1: f64,
    fx2: f64,
    freq: f64,
    res: f64,
    peak_gain: f64,
    velocity_to_volume: f32,
    amount: f64,
}

/// Where an envelope is.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    Idle,
    Attack,
    Hold,
    /// At the peak until the note ends (Gate) or the sample does (Hold Inf).
    Sustain,
    Decay,
}

/// A voice's envelope: a level moving in a straight line through its
/// stages.
#[derive(Clone, Copy)]
struct Env {
    stage: Stage,
    level: f32,
    from: f32,
    to: f32,
    left: u32,
    total: u32,
}

impl Env {
    const fn idle() -> Self {
        Self {
            stage: Stage::Idle,
            level: 0.0,
            from: 0.0,
            to: 0.0,
            left: 0,
            total: 1,
        }
    }

    fn enter(&mut self, stage: Stage, to: f32, frames: u32) {
        self.stage = stage;
        self.from = self.level;
        self.to = to;
        self.left = frames;
        self.total = frames.max(1);
        if frames == 0 {
            self.level = to;
        }
    }
}

/// How long a stage lasts, in frames, once the route has had its say.
#[derive(Clone, Copy)]
struct Times {
    attack: u32,
    hold: u32,
    decay: u32,
}

/// One grain of Stretch.
#[derive(Clone, Copy)]
struct Grain {
    start: f64,
    age: u32,
    len: u32,
}

/// One sounding note.
#[derive(Clone, Copy)]
struct Voice {
    active: bool,
    fading: bool,
    key: u8,
    /// Its key's pitch under the set's tuning, in semitones: C3's
    /// twelve-step pitch plays the sample at its own pitch (R25).
    tuned: f64,
    note_id: Option<u32>,
    /// When it started: the oldest voice is stolen first.
    counter: u64,
    velocity: f32,
    slide: f32,
    /// A per-note bend, from -1 to 1.
    bend: f32,
    region_start: f64,
    region_end: f64,
    /// Where it reads the sample, in frames (Stretch's time cursor).
    position: f64,
    /// Frames since the note started.
    age: u32,
    /// Hold Inf with Loop: how many frames the whole region takes.
    life: f64,
    /// The envelope's times are scaled by this (Envelope Follows Pitch).
    time_scale: f64,
    /// The sample has played out.
    done: bool,
    env: Env,
    fade_left: u32,
    fade_total: u32,
    grains: [Grain; 2],
    hop_left: u32,
    next_grain: usize,
    mod_phase: f64,
    sub_phase: f64,
    hold_phase: f64,
    held: (f32, f32),
    low_pass: (f32, f32),
    noise: u32,
    noise_low_pass: f32,
    filter: [[Biquad; 2]; 2],
    filter_kind: usize,
    /// Whether it plays the sample that was replaced while it sounded,
    /// fading out (sound similarity R29).
    old: bool,
}

impl Voice {
    fn idle() -> Self {
        // Finished grains, so a Stretch switched on mid-note has none to
        // play until it spawns its own.
        let grain = Grain {
            start: 0.0,
            age: 2,
            len: 2,
        };
        Self {
            active: false,
            fading: false,
            key: 0,
            tuned: 0.0,
            note_id: None,
            counter: 0,
            velocity: 1.0,
            slide: 0.0,
            bend: 0.0,
            region_start: 0.0,
            region_end: 0.0,
            position: 0.0,
            age: 0,
            life: 0.0,
            time_scale: 1.0,
            done: false,
            env: Env::idle(),
            fade_left: 0,
            fade_total: 1,
            grains: [grain; 2],
            hop_left: 0,
            next_grain: 0,
            mod_phase: 0.0,
            sub_phase: 0.0,
            hold_phase: 0.0,
            held: (0.0, 0.0),
            low_pass: (0.0, 0.0),
            noise: 1,
            noise_low_pass: 0.0,
            filter: [[Biquad::default(); 2]; 2],
            filter_kind: usize::MAX,
            old: false,
        }
    }

    /// Fades it out over `frames`, keeping it playing meanwhile.
    fn fade(&mut self, frames: u32) {
        self.fading = true;
        self.fade_left = frames.max(1);
        self.fade_total = frames.max(1);
    }

    /// The semitones from C3 the note and the pitch controls give it,
    /// without the effect's own movement.
    fn semitones(&self, s: &Settings, live: &Live) -> f64 {
        // Spec: one-shot drum sampler R25. Under a tuning a per-note bend
        // moves the note by steps of the tuning.
        let steps = f64::from(self.bend) * BEND_SEMITONES;
        let bend = if !s.bend {
            0.0
        } else if s.keys.is_tuned() && steps != 0.0 {
            s.keys
                .bent(self.key, steps)
                .zip(s.keys.pitch(self.key))
                .map_or(0.0, |(bent, at)| bent - at)
        } else {
            steps
        };
        self.tuned - 60.0 + s.transpose + live.detune + bend
    }

    /// The modulation route's value, from -1 to 1.
    fn route(&self, s: &Settings, live: &Live) -> f64 {
        let source = if s.slide_source {
            self.slide
        } else {
            self.velocity
        };
        live.amount * f64::from(source)
    }
}

/// The next white-noise value, from -1 to 1.
fn white(state: &mut u32) -> f32 {
    *state ^= *state << 13;
    *state ^= *state >> 17;
    *state ^= *state << 5;
    (*state as f32 / u32::MAX as f32) * 2.0 - 1.0
}

/// The one-pole low-pass coefficient for a cutoff of `hz`.
fn one_pole(hz: f64, rate: f64) -> f32 {
    (1.0 - (-TAU * (hz / rate).min(0.49)).exp()) as f32
}

/// Drum Sampler's state between blocks: the sample and its voices.
pub struct DrumSampler {
    data: Option<NodeData>,
    /// The pitch of every key under the set's tuning.
    keys: KeyPitches,
    voices: [Voice; SLOTS],
    /// How many notes have been played.
    counter: u64,
    /// The sample the voices that were sounding when another arrived go on
    /// reading while they fade out (sound similarity R29), until the last of
    /// them has gone.
    retiring: Option<NodeData>,
    /// Whether those voices are yet to be told to fade out, which the next
    /// block does, when it knows the rate.
    retire: bool,
}

impl DrumSampler {
    /// Makes a drum sampler: everything it needs is fixed in size.
    fn new(_config: &NodeConfig) -> Self {
        Self {
            data: None,
            keys: KeyPitches::twelve(),
            voices: [Voice::idle(); SLOTS],
            counter: 0,
            retiring: None,
            retire: false,
        }
    }

    fn play(&mut self, note: NoteEvent, s: &Settings, live: &Live, sample: &SampleData) {
        match note.kind {
            NoteKind::On => self.note_on(note, s, live, sample),
            NoteKind::Off => {
                if s.gate {
                    for voice in self.matching(&note) {
                        if matches!(voice.env.stage, Stage::Attack | Stage::Sustain) {
                            let frames = times(s, live, voice).decay;
                            voice.env.enter(Stage::Decay, 0.0, frames);
                        }
                    }
                }
            }
            NoteKind::Choke => {
                let fade = frames_of(STEAL_FADE_MS, s.rate).max(1);
                for voice in self.matching(&note) {
                    voice.fade(fade);
                }
            }
            NoteKind::Expression { expression, value } => {
                for voice in self.matching(&note) {
                    match expression {
                        NoteExpression::Bend => voice.bend = value.clamp(-1.0, 1.0),
                        NoteExpression::Slide => voice.slide = value.clamp(0.0, 1.0),
                        NoteExpression::Pressure => {}
                    }
                }
            }
            // It doesn't answer a channel message yet, as Simpler doesn't.
            NoteKind::Control { .. }
            | NoteKind::PitchBend { .. }
            | NoteKind::ChannelPressure { .. } => {}
        }
    }

    /// The sounding voices a note event names: by its note ID when it has
    /// one, else by its key.
    fn matching<'a>(&'a mut self, note: &NoteEvent) -> impl Iterator<Item = &'a mut Voice> {
        let (key, id) = (note.key, note.note_id);
        self.voices.iter_mut().filter(move |v| {
            v.active
                && !v.fading
                && match id {
                    Some(id) => v.note_id == Some(id),
                    None => v.key == key,
                }
        })
    }

    fn note_on(&mut self, note: NoteEvent, s: &Settings, live: &Live, sample: &SampleData) {
        // A key past the tuning's range plays nothing.
        let Some(tuned) = s.keys.pitch(note.key) else {
            return;
        };
        let frames = sample.frames() as f64;
        let region_start = (live.start * frames).floor();
        let region_end = ((live.start + live.length).min(1.0) * frames).floor();
        if region_end <= region_start {
            return;
        }
        let fade = frames_of(STEAL_FADE_MS, s.rate).max(1);
        // A repeated key cuts its earlier note off, and past the polyphony
        // the oldest sounding note goes.
        for voice in &mut self.voices {
            if voice.active && !voice.fading && voice.key == note.key {
                voice.fade(fade);
            }
        }
        while self.voices.iter().filter(|v| v.active && !v.fading).count() >= POLYPHONY {
            let Some(oldest) = self
                .voices
                .iter_mut()
                .filter(|v| v.active && !v.fading)
                .min_by_key(|v| v.counter)
            else {
                break;
            };
            oldest.fade(fade);
        }
        let slot = self.voices.iter().position(|v| !v.active).or_else(|| {
            // Every slot is busy: take the oldest of the fading ones,
            // which is closest to silent.
            self.voices
                .iter()
                .enumerate()
                .filter(|(_, v)| v.fading)
                .min_by_key(|(_, v)| v.counter)
                .map(|(i, _)| i)
        });
        let Some(slot) = slot else {
            return;
        };
        self.counter += 1;
        let mut voice = Voice::idle();
        voice.active = true;
        voice.key = note.key;
        voice.tuned = tuned;
        voice.note_id = note.note_id;
        voice.counter = self.counter;
        voice.velocity = note.velocity.clamp(0.0, 1.0);
        voice.region_start = region_start;
        voice.region_end = region_end;
        voice.position = region_start;
        voice.noise = 0x9E37_79B9u32.wrapping_mul(self.counter as u32 | 1) | 1;
        let ratio = 2f64.powf(voice.semitones(s, live) / 12.0);
        voice.life = (region_end - region_start) / ratio;
        voice.time_scale = if s.follows_pitch { 1.0 / ratio } else { 1.0 };
        let times = times(s, live, &voice);
        voice.env.enter(Stage::Attack, 1.0, times.attack);
        self.voices[slot] = voice;
    }

    /// One frame of every voice, mixed to a pair, before the volume and
    /// pan.
    fn frame(
        &mut self,
        s: &Settings,
        live: &Live,
        (sample, old): (&SampleData, Option<&SampleData>),
    ) -> (f32, f32) {
        let (mut left, mut right) = (0.0f32, 0.0f32);
        for voice in &mut self.voices {
            if !voice.active {
                continue;
            }
            let sample = if voice.old {
                old.unwrap_or(sample)
            } else {
                sample
            };
            let (l, r) = voice_frame(voice, s, live, sample);
            left += l;
            right += r;
        }
        (left, right)
    }
}

/// `frames` scaled by an envelope-follows-pitch factor.
fn scaled(frames: u32, scale: f64) -> u32 {
    (f64::from(frames) * scale).round() as u32
}

/// A voice's stage times now: the controls', stretched by the route when
/// it moves one of them, and scaled by Envelope Follows Pitch.
fn times(s: &Settings, live: &Live, voice: &Voice) -> Times {
    let route = voice.route(s, live);
    let time = |ms: f64, destination| {
        let ms = if s.destination == destination {
            ms * 2f64.powf(MOD_TIME_OCTAVES * route)
        } else {
            ms
        };
        scaled(frames_of(ms, s.rate), voice.time_scale)
    };
    Times {
        attack: time(live.attack_ms, Destination::Attack),
        hold: time(live.hold_ms, Destination::Hold),
        decay: time(live.decay_ms, Destination::Decay),
    }
}

/// The sample at `position` inside the voice's region, else silence.
fn tap(sample: &SampleData, region: (f64, f64), position: f64) -> (f32, f32) {
    if position < region.0 || position >= region.1 {
        return (0.0, 0.0);
    }
    read(sample, position)
}

/// Moves a voice's envelope on by one frame.
fn step_env(voice: &mut Voice, s: &Settings, live: &Live) {
    let env = &mut voice.env;
    if env.stage == Stage::Idle {
        return;
    }
    if env.left > 0 {
        env.left -= 1;
        let place = 1.0 - env.left as f32 / env.total as f32;
        env.level = env.from + (env.to - env.from) * place;
    }
    while voice.env.left == 0 {
        match voice.env.stage {
            Stage::Attack => {
                if s.gate || s.hold_inf {
                    voice.env.enter(Stage::Sustain, 1.0, 0);
                } else {
                    let frames = times(s, live, voice).hold;
                    voice.env.enter(Stage::Hold, 1.0, frames);
                }
            }
            Stage::Hold => {
                let frames = times(s, live, voice).decay;
                voice.env.enter(Stage::Decay, 0.0, frames);
            }
            Stage::Decay => {
                voice.env.stage = Stage::Idle;
                voice.env.level = 0.0;
                break;
            }
            Stage::Sustain | Stage::Idle => break,
        }
    }
}

/// Runs a voice for one frame: its sound before the volume and pan.
fn voice_frame(voice: &mut Voice, s: &Settings, live: &Live, sample: &SampleData) -> (f32, f32) {
    step_env(voice, s, live);
    if voice.env.stage == Stage::Idle {
        voice.active = false;
        return (0.0, 0.0);
    }
    let rate = s.rate;
    let route = voice.route(s, live);
    let nudge = |destination| {
        if s.destination == destination {
            route
        } else {
            0.0
        }
    };
    let fx1 = (live.fx1 + nudge(Destination::Fx1)).clamp(0.0, 1.0);
    let fx2 = (live.fx2 + nudge(Destination::Fx2)).clamp(0.0, 1.0);
    let age = f64::from(voice.age);
    let note_semitones = voice.semitones(s, live);
    let mut semitones = note_semitones;
    if s.effect == Effect::PitchEnv {
        let span = PITCH_ENV_MIN_MS * PITCH_ENV_RATIO.powf(fx2) / 1000.0 * rate;
        semitones += (fx1 * 2.0 - 1.0) * PITCH_ENV_SEMITONES * (1.0 - age / span).max(0.0);
    }
    let note_ratio = 2f64.powf(note_semitones / 12.0);
    let ratio = 2f64.powf(semitones / 12.0);

    // The sample, through the effect that works on it.
    let gain = live.gain;
    let region = (voice.region_start, voice.region_end);
    let mut sound = (0.0f32, 0.0f32);
    match s.effect {
        Effect::Stretch => {
            let factor = STRETCH_MIN * STRETCH_RATIO.powf(fx1);
            if voice.hop_left == 0 {
                let len =
                    ((GRAIN_MIN_MS * GRAIN_RATIO.powf(fx2) / 1000.0 * rate) as u32).max(8) & !1;
                let start = voice.position;
                if voice.age == 0 {
                    // The first grain starts at its peak, so the sample's
                    // attack isn't faded in.
                    voice.grains[0] = Grain {
                        start: start - f64::from(len / 2) * ratio,
                        age: len / 2,
                        len,
                    };
                    voice.grains[1] = Grain { start, age: 0, len };
                    voice.next_grain = 0;
                } else {
                    voice.grains[voice.next_grain] = Grain { start, age: 0, len };
                    voice.next_grain ^= 1;
                }
                voice.hop_left = len / 2;
            }
            voice.hop_left -= 1;
            for grain in &mut voice.grains {
                if grain.age < grain.len {
                    let window =
                        0.5 - 0.5 * (TAU * f64::from(grain.age) / f64::from(grain.len)).cos();
                    let (l, r) = tap(sample, region, grain.start + f64::from(grain.age) * ratio);
                    sound.0 += l * window as f32;
                    sound.1 += r * window as f32;
                    grain.age += 1;
                }
            }
            voice.position += ratio / factor;
        }
        Effect::Fm => {
            let decay = f64::from(scaled(frames_of(live.decay_ms, rate), voice.time_scale));
            let fade = if decay > 0.0 {
                (1.0 - age / decay).max(0.0)
            } else {
                0.0
            };
            let depth = fx1 * FM_DEPTH_MS / 1000.0 * rate * fade;
            let hz = MOD_MIN_HZ * MOD_RATIO.powf(fx2) * note_ratio;
            voice.mod_phase = (voice.mod_phase + hz / rate).fract();
            // The bend never reads before the region, where `tap` is silent.
            let bent = (voice.position + depth * (TAU * voice.mod_phase).sin()).max(region.0);
            sound = tap(sample, region, bent);
            voice.position += ratio;
        }
        Effect::RingMod => {
            let decay = f64::from(scaled(frames_of(live.decay_ms, rate), voice.time_scale));
            let fade = if decay > 0.0 {
                (1.0 - age / decay).max(0.0)
            } else {
                0.0
            };
            let hz = MOD_MIN_HZ * MOD_RATIO.powf(fx2) * note_ratio;
            voice.mod_phase = (voice.mod_phase + hz / rate).fract();
            let mix = (fx1 * fade) as f32;
            let carrier = (TAU * voice.mod_phase).sin() as f32;
            let (l, r) = tap(sample, region, voice.position);
            let scale = 1.0 - mix + mix * carrier;
            sound = (l * scale, r * scale);
            voice.position += ratio;
        }
        Effect::EightBit => {
            let hz = BITS_MIN_HZ * BITS_RATIO.powf(fx1);
            voice.hold_phase += hz / rate;
            if voice.hold_phase >= 1.0 || voice.age == 0 {
                voice.hold_phase = voice.hold_phase.fract();
                let (l, r) = tap(sample, region, voice.position);
                let crush = |x: f32| (x * 127.0).round() / 127.0;
                voice.held = (crush(l), crush(r));
            }
            let span = BITS_DECAY_MIN_MS * BITS_DECAY_RATIO.powf(fx2) / 1000.0 * rate;
            let place = (age / span).min(1.0);
            let cutoff =
                BITS_LP_START_HZ * ((hz / 4.0).max(BITS_LP_MIN_HZ) / BITS_LP_START_HZ).powf(place);
            let a = one_pole(cutoff, rate);
            voice.low_pass.0 += a * (voice.held.0 - voice.low_pass.0);
            voice.low_pass.1 += a * (voice.held.1 - voice.low_pass.1);
            sound = voice.low_pass;
            voice.position += ratio;
        }
        _ => {
            sound = tap(sample, region, voice.position);
            voice.position += ratio;
            if s.effect == Effect::Loop {
                let loop_len = LOOP_MIN_MS * LOOP_RATIO.powf(fx2) / 1000.0 * rate;
                let region = voice.region_end - voice.region_start;
                let loop_len = loop_len.min(region);
                let loop_start = voice.region_start + fx1 * (region - loop_len);
                let loop_end = loop_start + loop_len;
                if voice.position >= loop_end {
                    voice.position = loop_start + (voice.position - loop_end) % loop_len;
                }
            }
        }
    }
    if voice.position >= voice.region_end && s.effect != Effect::Loop {
        voice.done = true;
    }
    sound.0 *= gain;
    sound.1 *= gain;

    // The oscillators add to it.
    match s.effect {
        Effect::SubOsc => {
            let hz = (SUB_MIN_HZ + SUB_SPAN_HZ * fx2) * note_ratio;
            voice.sub_phase = (voice.sub_phase + hz / rate).fract();
            let level = (fx1 * (TAU * voice.sub_phase).sin()) as f32;
            sound.0 += level;
            sound.1 += level;
        }
        Effect::NoiseOsc => {
            let a = one_pole(NOISE_MIN_HZ * NOISE_RATIO.powf(fx2) * note_ratio, rate);
            voice.noise_low_pass += a * (white(&mut voice.noise) - voice.noise_low_pass);
            let level = voice.noise_low_pass * fx1 as f32 * NOISE_PEAK;
            sound.0 += level;
            sound.1 += level;
        }
        Effect::Punch => {
            let transient = PUNCH_TRANSIENT_MS / 1000.0 * rate;
            let release = PUNCH_MIN_MS * PUNCH_RATIO.powf(fx2) / 1000.0 * rate;
            let into = age - transient;
            if into >= 0.0 && into < release {
                let duck_in = (into / (PUNCH_DUCK_IN_MS / 1000.0 * rate)).min(1.0);
                let db = fx1 * PUNCH_MAX_DB * (1.0 - into / release) * duck_in;
                let scale = 10f32.powf(-db as f32 / 20.0);
                sound.0 *= scale;
                sound.1 *= scale;
            }
        }
        _ => {}
    }

    // The filter, redesigned every few frames.
    if s.filter_on {
        // A new type, or a filter switched on mid-note, starts from silence
        // and is designed at once, not at the next tick.
        let changed = voice.filter_kind != s.filter_type;
        if changed {
            voice.filter = [[Biquad::default(); 2]; 2];
            voice.filter_kind = s.filter_type;
        }
        if changed || voice.age.is_multiple_of(CONTROL_INTERVAL) {
            let hz =
                live.freq * 2f64.powf(MOD_FILTER_SEMITONES / 12.0 * nudge(Destination::Filter));
            let q = Q_BASE + Q_RESONANCE * live.res;
            let design = |kind, q| coeffs(kind, hz, live.peak_gain, q, rate);
            let (first, second) = match s.filter_type {
                0 => (design(Type::HighCut12, q), None),
                1 => (
                    design(Type::HighCut12, q),
                    Some(design(Type::HighCut12, Q_BASE)),
                ),
                2 => (
                    design(Type::LowCut12, q),
                    Some(design(Type::LowCut12, Q_BASE)),
                ),
                _ => (design(Type::Peak, PEAK_Q), None),
            };
            for channel in &mut voice.filter {
                channel[0].c = first;
                if let Some(second) = second {
                    channel[1].c = second;
                }
            }
        }
        let two_stages = matches!(s.filter_type, 1 | 2);
        let mut run = |x: f32, channel: usize| {
            let y = voice.filter[channel][0].process(x);
            if two_stages {
                voice.filter[channel][1].process(y)
            } else {
                y
            }
        };
        sound = (run(sound.0, 0), run(sound.1, 1));
    }

    // Level: the envelope, the velocity, and any fade out.
    let velocity = 1.0 - live.velocity_to_volume * (1.0 - voice.velocity);
    let mut level = voice.env.level * velocity;
    if voice.fading {
        level *= voice.fade_left as f32 / voice.fade_total as f32;
        voice.fade_left -= 1;
        if voice.fade_left == 0 {
            voice.active = false;
        }
    }
    voice.age = voice.age.saturating_add(1);
    // The voice ends when nothing is left to play: the sample is out and no
    // oscillator carries on, or Hold Inf has played the whole region.
    let carries_on = s.effect.makes_sound() && (s.gate || !s.hold_inf);
    if voice.done && !carries_on {
        voice.active = false;
    }
    if s.effect == Effect::Loop && s.hold_inf && !s.gate && f64::from(voice.age) >= voice.life {
        voice.active = false;
    }
    (sound.0 * level, sound.1 * level)
}

impl NativeDevice for DrumSampler {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        let frames = audio.frames();
        if frames == 0 {
            return;
        }
        // The decoded sample, held aside for the stretch; cloning the
        // `Arc` only moves a reference, so nothing allocates.
        let sample = match self
            .data
            .as_ref()
            .and_then(|data| data.downcast_ref::<SlotSample>())
        {
            Some(slot) if slot.sample.frames() > 0 => Arc::clone(&slot.sample),
            _ => {
                for voice in &mut self.voices {
                    voice.active = false;
                }
                return;
            }
        };
        let mut settings = Settings::read(params, audio.sample_rate().as_f64());
        settings.keys = self.keys;
        // The voices of the sample that was replaced fade out over the steal
        // fade, reading it still (sound similarity R29).
        if core::mem::take(&mut self.retire) {
            let fade = frames_of(STEAL_FADE_MS, settings.rate).max(1);
            for voice in &mut self.voices {
                if voice.active && voice.old {
                    voice.fade(fade);
                }
            }
        }
        let old = self
            .retiring
            .as_ref()
            .and_then(|data| data.downcast_ref::<SlotSample>())
            .map(|slot| Arc::clone(&slot.sample));
        let ramps = Ramps::new(params);
        let mut notes = audio.notes().peekable();
        let mut channels = audio.channels_mut();
        let Some(left) = channels.next() else {
            return;
        };
        let mut right = channels.next();
        for frame in 0..frames {
            let live = ramps.live(frame);
            while notes
                .peek()
                .is_some_and(|note| note.offset as usize <= frame)
            {
                let note = notes.next().expect("peeked");
                self.play(note, &settings, &live, &sample);
            }
            let (l, r) = self.frame(&settings, &live, (&sample, old.as_deref()));
            let volume = ramps.volume[frame];
            let b = (ramps.pan[frame] / 50.0).clamp(-1.0, 1.0);
            let (left_pan, right_pan) = (1.0 - b.max(0.0), 1.0 + b.min(0.0));
            left[frame] = l * volume * left_pan;
            if let Some(right) = &mut right {
                right[frame] = r * volume * right_pan;
            }
        }
        // Let go of the old sample before the node data that holds it: once
        // that is dropped, the drop thread may free it, and this clone
        // must not be the last reference on the audio thread.
        drop(old);
        if self.retiring.is_some() && !self.voices.iter().any(|v| v.active && v.old) {
            self.retiring = None;
        }
    }

    fn receive_data(&mut self, data: NodeData) {
        if let Some(keys) = data.downcast_ref::<KeyPitches>() {
            self.keys = *keys;
            return;
        }
        // Drum Sampler has one sample slot.
        if data
            .downcast_ref::<SlotSample>()
            .is_some_and(|slot| slot.slot == 0)
        {
            // Voices sounding now go on with the sample they started on and
            // fade out (sound similarity R29); a second sample arriving
            // within the fade cuts the first one's.
            if self.voices.iter().any(|voice| voice.active) {
                if self.retiring.is_some() {
                    for voice in self.voices.iter_mut().filter(|voice| voice.old) {
                        voice.active = false;
                    }
                }
                for voice in self.voices.iter_mut().filter(|voice| voice.active) {
                    voice.old = true;
                }
                self.retiring = self.data.take();
                self.retire = true;
            }
            self.data = Some(data);
        }
    }
}

//! Drift, the native polysynth (WP-2.17): two band-limited oscillators
//! with their shapes, a noise source, a zero-delay-feedback low-pass and
//! high-pass filter with drive and key tracking, the amplifier and
//! modulation envelopes, an LFO with its shapes and time modes, the
//! modulation routes the manual's Mod section and its dedicated Shape,
//! Pitch and Freq slots name, and MPE's per-note pitch bend, pressure and
//! slide, played through the plug-in kit's polyphonic voice manager.
//!
//! Its behaviour, parameters, units and names follow Live 12's compact
//! polyphonic synthesizer as its manual describes it (see
//! `docs/specs/polysynth.md`); the manual records few ranges or defaults,
//! so those are Beatlayer's own and are listed there to observe. The
//! waveforms, the shape morphing and the saturation curves are Beatlayer's
//! own too: the manual names the waveforms and says the Shape knob changes
//! the harmonic content, but doesn't draw any of them.
//!
//! Part 1 plays polyphonically. The LFO, the modulation routes it and the
//! envelopes drive, and MPE's per-note pitch bend, pressure and slide
//! arrived with the work package's second part; the voice modes, the
//! Drift-style per-voice variation, glide and Envelope 2's cycling mode
//! are still to come.

use core::f64::consts::TAU;

use bl_engine::{DeviceType, NodeConfig, NodeData, NodeEvent, Registry};
use bl_model::device_layout::{BODY_HEIGHT, ControlBand, DisplayCell, LayoutNode, Widget};
use bl_model::param::gain_to_db;
use bl_model::{DeviceDescription, DeviceLayout, DeviceRole, ParamDescriptor, ParamId, ParamUnit};
use lspk_dsp::envelope::{EnvelopeBank, EnvelopeParams, StealRestart, TimeUnit, Timing, Trigger};
use lspk_dsp::filter::{FilterBank, FilterType, VoiceParams, pitch_of};
use lspk_voice::{MAX_VOICES, Start, VoiceBlock, VoiceEvent, VoiceEventKind};
use lspk_wavetable::osc::{
    Inputs, Layout as OscLayout, Onset as OscOnset, Oscillator, Params as OscParams, PhaseMode,
};
use lspk_wavetable::{Cache, FRAME_SIZE, SharedTable, Wavetable};

use crate::framework::{Audio, NativeDevice, ParamBank, add};
use crate::instrument::matrix::{
    DestDef, Kind, Matrix, MatrixLayout, Pick, Range, Rate, Scope, SlotDef, SourceDef,
};
use crate::instrument::{self, Instrument, NoteRange, Parts, Section, Sources, State};

/// Drift's device type.
pub const POLYSYNTH: &str = "beatlayer.cascade";

/// Oscillator 1's switch.
pub const OSC1: ParamId = ParamId(0);
/// Oscillator 1's waveform.
pub const OSC1_WAVE: ParamId = ParamId(1);
/// Oscillator 1's shape, in per cent.
pub const OSC1_SHAPE: ParamId = ParamId(2);
/// Oscillator 1's octave transposition.
pub const OSC1_OCT: ParamId = ParamId(3);
/// Oscillator 1's gain.
pub const OSC1_GAIN: ParamId = ParamId(4);
/// Oscillator 2's switch.
pub const OSC2: ParamId = ParamId(5);
/// Oscillator 2's waveform.
pub const OSC2_WAVE: ParamId = ParamId(6);
/// Oscillator 2's octave transposition.
pub const OSC2_OCT: ParamId = ParamId(7);
/// Oscillator 2's transposition in semitones.
pub const OSC2_DETUNE: ParamId = ParamId(8);
/// Oscillator 2's gain.
pub const OSC2_GAIN: ParamId = ParamId(9);
/// The noise generator's switch.
pub const NOISE: ParamId = ParamId(10);
/// The noise generator's gain.
pub const NOISE_GAIN: ParamId = ParamId(11);
/// The filter processing switch.
pub const FILTER: ParamId = ParamId(12);
/// The oscillator phase retrigger switch.
pub const RETRIGGER: ParamId = ParamId(13);
/// The low-pass cutoff, in Hz.
pub const FREQ: ParamId = ParamId(14);
/// The low-pass filter type.
pub const FILTER_TYPE: ParamId = ParamId(15);
/// The low-pass key tracking.
pub const KEY: ParamId = ParamId(16);
/// The low-pass resonance, in per cent.
pub const RES: ParamId = ParamId(17);
/// The high-pass cutoff, in Hz.
pub const HP: ParamId = ParamId(18);
/// Envelope 1's attack, in milliseconds.
pub const ENV1_ATTACK: ParamId = ParamId(19);
/// Envelope 1's decay, in milliseconds.
pub const ENV1_DECAY: ParamId = ParamId(20);
/// Envelope 1's sustain, in per cent.
pub const ENV1_SUSTAIN: ParamId = ParamId(21);
/// Envelope 1's release, in milliseconds.
pub const ENV1_RELEASE: ParamId = ParamId(22);
/// Envelope 2's attack, in milliseconds.
pub const ENV2_ATTACK: ParamId = ParamId(23);
/// Envelope 2's decay, in milliseconds.
pub const ENV2_DECAY: ParamId = ParamId(24);
/// Envelope 2's sustain, in per cent.
pub const ENV2_SUSTAIN: ParamId = ParamId(25);
/// Envelope 2's release, in milliseconds.
pub const ENV2_RELEASE: ParamId = ParamId(26);
/// The instrument's volume.
pub const VOLUME: ParamId = ParamId(27);
/// The most notes that sound at once.
pub const VOICES: ParamId = ParamId(28);
/// The instrument's global transposition, in semitones.
pub const TRANSPOSE: ParamId = ParamId(29);
/// How much the volume follows the notes' velocity, in per cent.
pub const VEL_VOL: ParamId = ParamId(30);
/// The LFO's time mode: Rate, Ratio, Time, or Sync.
pub const LFO_MODE: ParamId = ParamId(31);
/// The LFO's rate in hertz, in the Rate time mode.
pub const LFO_RATE: ParamId = ParamId(32);
/// The LFO's rate as a ratio of the played note's frequency, in the Ratio
/// time mode.
pub const LFO_RATIO: ParamId = ParamId(33);
/// The LFO's cycle in milliseconds, in the Time time mode.
pub const LFO_TIME: ParamId = ParamId(34);
/// The LFO's division, in the Sync time mode.
pub const LFO_SYNC: ParamId = ParamId(35);
/// The LFO's waveform.
pub const LFO_SHAPE: ParamId = ParamId(36);
/// The LFO's retrigger switch.
pub const LFO_RETRIGGER: ParamId = ParamId(37);
/// The LFO's overall intensity, in per cent.
pub const LFO_AMOUNT: ParamId = ParamId(38);
/// The modulation source for the LFO's own rate.
pub const LFO_MOD_SOURCE: ParamId = ParamId(39);
/// How much the LFO's own modulation source moves its rate, in per cent.
pub const LFO_MOD_AMOUNT: ParamId = ParamId(40);
/// The first Mod slot's source.
pub const MOD1_SOURCE: ParamId = ParamId(41);
/// The first Mod slot's destination.
pub const MOD1_DEST: ParamId = ParamId(42);
/// The first Mod slot's amount, in per cent of the destination's travel.
pub const MOD1_AMOUNT: ParamId = ParamId(43);
/// The second Mod slot's source.
pub const MOD2_SOURCE: ParamId = ParamId(44);
/// The second Mod slot's destination.
pub const MOD2_DEST: ParamId = ParamId(45);
/// The second Mod slot's amount, in per cent of the destination's travel.
pub const MOD2_AMOUNT: ParamId = ParamId(46);
/// The third Mod slot's source.
pub const MOD3_SOURCE: ParamId = ParamId(47);
/// The third Mod slot's destination.
pub const MOD3_DEST: ParamId = ParamId(48);
/// The third Mod slot's amount, in per cent of the destination's travel.
pub const MOD3_AMOUNT: ParamId = ParamId(49);
/// Oscillator 1's Shape Mod source.
pub const SHAPE_MOD_SOURCE: ParamId = ParamId(50);
/// How much Oscillator 1's Shape Mod source morphs the waveform, in per
/// cent.
pub const SHAPE_MOD_AMOUNT: ParamId = ParamId(51);
/// The first Pitch Mod source, which moves both oscillators.
pub const PITCH_MOD1_SOURCE: ParamId = ParamId(52);
/// The first Pitch Mod amount, in per cent.
pub const PITCH_MOD1_AMOUNT: ParamId = ParamId(53);
/// The second Pitch Mod source, which moves both oscillators.
pub const PITCH_MOD2_SOURCE: ParamId = ParamId(54);
/// The second Pitch Mod amount, in per cent.
pub const PITCH_MOD2_AMOUNT: ParamId = ParamId(55);
/// The first Freq Mod source, which moves the low-pass cutoff.
pub const FREQ_MOD1_SOURCE: ParamId = ParamId(56);
/// The first Freq Mod amount, in per cent of the cutoff's travel.
pub const FREQ_MOD1_AMOUNT: ParamId = ParamId(57);
/// The second Freq Mod source, which moves the low-pass cutoff.
pub const FREQ_MOD2_SOURCE: ParamId = ParamId(58);
/// The second Freq Mod amount, in per cent of the cutoff's travel.
pub const FREQ_MOD2_AMOUNT: ParamId = ParamId(59);
/// Per-note pitch bend on or off, for an MPE controller.
pub const NOTE_PB: ParamId = ParamId(60);
/// The instrument's pitch bend range, in semitones.
pub const PB_RANGE: ParamId = ParamId(61);

/// Oscillator 1's waveforms, in the manual's order.
pub const WAVES1: [&str; 7] = [
    "Sine",
    "Triangle",
    "Shark Tooth",
    "Saturated",
    "Saw",
    "Pulse",
    "Rectangle",
];
/// Oscillator 2's waveforms, in the manual's order.
pub const WAVES2: [&str; 5] = ["Sine", "Triangle", "Saturated", "Saw", "Rectangle"];
/// The filter types: the manual's Type I and Type II.
pub const FILTER_TYPES: [&str; 2] = ["I", "II"];
/// The LFO's waveforms, in the manual's order.
pub const LFO_SHAPES: [&str; 9] = [
    "Sine",
    "Triangle",
    "Saw Up",
    "Saw Down",
    "Square",
    "Sample & Hold",
    "Wander",
    "Linear Envelope",
    "Exponential Envelope",
];
/// The LFO's time modes, in the manual's order.
pub const LFO_MODES: [&str; 4] = ["Rate", "Ratio", "Time", "Sync"];
/// The Sync time mode's divisions; Beatlayer's own list, to observe.
pub const LFO_DIVISIONS: [&str; 7] = ["1/16", "1/8", "1/4", "1/2", "1 Bar", "2 Bars", "4 Bars"];
/// How many sixteenths each division holds: the Sync time mode's rates.
pub(crate) const DIVISION_SIXTEENTHS: [f64; 7] = [1.0, 2.0, 4.0, 8.0, 16.0, 32.0, 64.0];
/// The modulation sources every route chooser offers, in the manual's
/// order, after Off.
pub const MOD_SOURCES: [&str; 9] = [
    "Off",
    "Env 1",
    "Env 2 / Cyc",
    "LFO",
    "Key",
    "Velocity",
    "Modwheel",
    "Pressure",
    "Slide",
];
/// The Mod section's destinations, in the manual's order, after Off.
pub const MOD_DESTINATIONS: [&str; 11] = [
    "Off",
    "Osc 1 Gain",
    "Osc 1 Shape",
    "Osc 2 Gain",
    "Osc 2 Detune",
    "Noise Gain",
    "LP Frequency",
    "LP Resonance",
    "HP Frequency",
    "LFO Rate",
    "Main Volume",
];

/// Oscillator 1's waveforms by index.
const SINE: usize = 0;
const TRIANGLE: usize = 1;
const SHARK: usize = 2;
const SATURATED: usize = 3;
const SAW: usize = 4;
const PULSE: usize = 5;
const RECTANGLE: usize = 6;

/// The frequency A4 is tuned to.
const REFERENCE: f32 = 440.0;
/// The key the low-pass cutoff is quoted at: key tracking moves it from
/// this note, which Live calls C3.
const MIDDLE_C: f32 = 60.0;
/// The frames each shape's table holds: the Shape knob moves through them.
const SHAPE_FRAMES: usize = 17;
/// How narrow a shape's duty cycle gets at Shape 100 %.
const MIN_DUTY: f64 = 0.2;
/// The gains' default, in dB: the manual's own default for all three.
const DEFAULT_GAIN_DB: f64 = -6.0;
/// Where the first saturation point starts working, in dB, and where it
/// reaches full: the manual's own default and Beatlayer's own slope.
const DRIVE_START_DB: f64 = -6.0;
const DRIVE_FULL_DB: f64 = 18.0;
/// Where the second saturation point starts working, in dB: the manual's
/// own 0 dB, reaching full at the gains' top.
const POST_START_DB: f64 = 0.0;
/// The envelopes' time range, in milliseconds, and the curve of each
/// stage: Beatlayer's own, like the kit's `EnvelopeParams` defaults.
const MIN_TIME_MS: f64 = 0.1;
const MAX_TIME_MS: f64 = 20_000.0;
const ATTACK_CURVE: f32 = 0.5;
const DECAY_CURVE: f32 = 0.666;
const RELEASE_CURVE: f32 = 0.666;

/// How far a full Pitch Mod amount moves both oscillators: Beatlayer's own
/// reading, in semitones.
const PITCH_MOD_SEMITONES: f64 = 24.0;
/// How far a full LFO Rate route multiplies the LFO's speed: Beatlayer's
/// own reading, four octaves.
const LFO_RATE_OCTAVES: f64 = 4.0;
/// How fast the Exponential Envelope waveform's decay falls over its
/// cycle: Beatlayer's own reading.
const EXPONENTIAL_DECAY: f64 = 5.0;

/// The modulation sources by place in the matrix's list: one less than the
/// choosers' value, which counts Off as 0.
const SRC_ENV1: usize = 0;
const SRC_ENV2: usize = 1;
const SRC_LFO: usize = 2;
const SRC_KEY: usize = 3;
const SRC_VELOCITY: usize = 4;
const SRC_MODWHEEL: usize = 5;
const SRC_PRESSURE: usize = 6;
const SRC_SLIDE: usize = 7;
/// The Mod destinations by place in the matrix's list, as the choosers'
/// value less one.
const DEST_OSC1_GAIN: usize = 0;
const DEST_OSC1_SHAPE: usize = 1;
const DEST_OSC2_GAIN: usize = 2;
const DEST_OSC2_DETUNE: usize = 3;
const DEST_NOISE_GAIN: usize = 4;
const DEST_LP_FREQ: usize = 5;
const DEST_LP_RES: usize = 6;
const DEST_HP_FREQ: usize = 7;
const DEST_LFO_RATE: usize = 8;
const DEST_VOLUME: usize = 9;
/// Pitch is no Mod destination: the Pitch Mod slots carry it, and no chooser
/// reaches it.
const DEST_PITCH: usize = 10;
/// The LFO's waveforms by index.
const LFO_SINE: usize = 0;
const LFO_TRIANGLE: usize = 1;
const LFO_SAW_UP: usize = 2;
const LFO_SAW_DOWN: usize = 3;
const LFO_SQUARE: usize = 4;
const LFO_SAMPLE_HOLD: usize = 5;
const LFO_WANDER: usize = 6;
const LFO_LINEAR: usize = 7;
const LFO_EXPONENTIAL: usize = 8;
/// The LFO's time modes by index.
const MODE_RATE: usize = 0;
const MODE_RATIO: usize = 1;
const MODE_TIME: usize = 2;
const MODE_SYNC: usize = 3;

/// The Drift device type.
#[must_use]
pub fn polysynth_type() -> DeviceType {
    let time = |id, name: &str, default: f64| {
        ParamDescriptor::continuous(
            id,
            name,
            (MIN_TIME_MS, MAX_TIME_MS),
            default,
            ParamUnit::Milliseconds,
        )
        .logarithmic()
    };
    let params = vec![
        ParamDescriptor::toggle(OSC1, "Osc 1", true).describe("Turns Oscillator 1 on."),
        ParamDescriptor::choice(OSC1_WAVE, "Osc 1 Wave", &WAVES1, SAW)
            .describe("Oscillator 1's waveform."),
        ParamDescriptor::continuous(OSC1_SHAPE, "Osc 1 Shape", (0.0, 100.0), 0.0, ParamUnit::Percent)
            .describe("Morphs Oscillator 1's waveform, narrowing a pulse or skewing the others."),
        ParamDescriptor::integer(OSC1_OCT, "Osc 1 Oct", (-4, 4), 0, ParamUnit::None)
            .decimals(0)
            .describe("Transposes Oscillator 1 in octaves."),
        ParamDescriptor::gain(OSC1_GAIN, "Osc 1 Gain", DRIVE_FULL_DB, DEFAULT_GAIN_DB)
            .describe("Sets Oscillator 1's level, down to silence."),
        ParamDescriptor::toggle(OSC2, "Osc 2", true).describe("Turns Oscillator 2 on."),
        ParamDescriptor::choice(OSC2_WAVE, "Osc 2 Wave", &WAVES2, 3)
            .describe("Oscillator 2's waveform."),
        ParamDescriptor::integer(OSC2_OCT, "Osc 2 Oct", (-4, 4), 0, ParamUnit::None)
            .decimals(0)
            .describe("Transposes Oscillator 2 in octaves."),
        ParamDescriptor::integer(OSC2_DETUNE, "Osc 2 Detune", (-12, 12), 0, ParamUnit::Semitones)
            .decimals(0)
            .describe("Transposes Oscillator 2 in semitones."),
        ParamDescriptor::gain(OSC2_GAIN, "Osc 2 Gain", DRIVE_FULL_DB, DEFAULT_GAIN_DB)
            .describe("Sets Oscillator 2's level, down to silence."),
        ParamDescriptor::toggle(NOISE, "Noise", false).describe("Adds white noise."),
        ParamDescriptor::gain(NOISE_GAIN, "Noise Gain", DRIVE_FULL_DB, DEFAULT_GAIN_DB)
            .describe("Sets the noise generator's level, down to silence."),
        ParamDescriptor::toggle(FILTER, "Filter", true)
            .describe("Sends the oscillators and the noise through the filter; off bypasses it."),
        ParamDescriptor::toggle(RETRIGGER, "Retrigger", true)
            .describe("Restarts both oscillators at the same phase on every note; off runs them freely."),
        ParamDescriptor::continuous(FREQ, "Freq", (20.0, 20_000.0), 1_000.0, ParamUnit::Hertz)
            .logarithmic()
            .describe("Sets the low-pass cutoff."),
        ParamDescriptor::choice(FILTER_TYPE, "Type", &FILTER_TYPES, 0)
            .describe("Type I is 12 dB per octave; Type II is 24 and softens its resonance."),
        ParamDescriptor::continuous(KEY, "Key", (0.0, 1.0), 0.0, ParamUnit::None)
            .decimals(2)
            .describe("How much the played note moves the low-pass cutoff: one octave per octave at 1.00."),
        ParamDescriptor::continuous(RES, "Res", (0.0, 100.0), 0.0, ParamUnit::Percent)
            .describe("Raises the low-pass resonance."),
        ParamDescriptor::continuous(HP, "HP", (20.0, 20_000.0), 20.0, ParamUnit::Hertz)
            .logarithmic()
            .describe("Sets the high-pass cutoff."),
        time(ENV1_ATTACK, "Env 1 Attack", 1.0).describe("How long Envelope 1 takes to reach full."),
        time(ENV1_DECAY, "Env 1 Decay", 1_000.0)
            .describe("How long Envelope 1 takes to fall to its sustain level."),
        ParamDescriptor::continuous(ENV1_SUSTAIN, "Env 1 Sustain", (0.0, 100.0), 100.0, ParamUnit::Percent)
            .describe("The level Envelope 1 holds while a note lasts."),
        time(ENV1_RELEASE, "Env 1 Release", 15.0)
            .describe("How long Envelope 1 takes to fall to silence after a note ends."),
        time(ENV2_ATTACK, "Env 2 Attack", 0.5).describe("How long Envelope 2 takes to reach full."),
        time(ENV2_DECAY, "Env 2 Decay", 1_000.0)
            .describe("How long Envelope 2 takes to fall to its sustain level."),
        ParamDescriptor::continuous(ENV2_SUSTAIN, "Env 2 Sustain", (0.0, 100.0), 100.0, ParamUnit::Percent)
            .describe("The level Envelope 2 holds while a note lasts."),
        time(ENV2_RELEASE, "Env 2 Release", 15.0)
            .describe("How long Envelope 2 takes to fall to silence after a note ends."),
        ParamDescriptor::gain(VOLUME, "Volume", DRIVE_FULL_DB, 0.0)
            .describe("Sets the instrument's output level, down to silence."),
        instrument::voices(VOICES, MAX_VOICES as i64, 16),
        instrument::transpose(TRANSPOSE, 48),
        ParamDescriptor::continuous(VEL_VOL, "Vel > Vol", (0.0, 100.0), 0.0, ParamUnit::Percent)
            .describe("How much the notes' velocity moves the volume."),
        ParamDescriptor::choice(LFO_MODE, "LFO Mode", &LFO_MODES, MODE_SYNC)
            .describe("Sets the LFO's rate in hertz, as a ratio of the note, in milliseconds, or in beat divisions."),
        ParamDescriptor::continuous(LFO_RATE, "LFO Rate", (0.01, 100.0), 1.0, ParamUnit::Hertz)
            .logarithmic()
            .describe("The LFO's rate in hertz, in the Rate time mode."),
        ParamDescriptor::continuous(LFO_RATIO, "LFO Ratio", (0.0625, 64.0), 1.0, ParamUnit::Ratio)
            .logarithmic()
            .describe("The LFO's speed as a ratio of the played note's frequency, in the Ratio time mode: at 1.0 : 1 an audio-rate pitch modulation is an FM tone."),
        ParamDescriptor::continuous(
            LFO_TIME,
            "LFO Time",
            (1.0, 20_000.0),
            500.0,
            ParamUnit::Milliseconds,
        )
        .logarithmic()
        .describe("The LFO's cycle in milliseconds, in the Time time mode."),
        ParamDescriptor::choice(LFO_SYNC, "LFO Sync", &LFO_DIVISIONS, 2)
            .describe("The LFO's cycle as a division of the song's tempo, in the Sync time mode."),
        ParamDescriptor::choice(LFO_SHAPE, "LFO Shape", &LFO_SHAPES, LFO_SINE)
            .describe("The LFO's waveform."),
        ParamDescriptor::toggle(LFO_RETRIGGER, "LFO R", true)
            .describe("Restarts the LFO at the same position on every note; off runs it freely."),
        ParamDescriptor::continuous(LFO_AMOUNT, "LFO Amount", (0.0, 100.0), 100.0, ParamUnit::Percent)
            .describe("Sets the LFO's overall intensity."),
        ParamDescriptor::choice(LFO_MOD_SOURCE, "LFO Mod", &MOD_SOURCES, 0)
            .describe("The source that moves the LFO's own rate."),
        ParamDescriptor::continuous(
            LFO_MOD_AMOUNT,
            "LFO Mod Amount",
            (-100.0, 100.0),
            0.0,
            ParamUnit::Percent,
        )
        .describe("How much the LFO's own modulation source moves its rate."),
        ParamDescriptor::choice(MOD1_SOURCE, "Mod 1 Source", &MOD_SOURCES, 0)
            .describe("The first Mod slot's modulation source."),
        ParamDescriptor::choice(MOD1_DEST, "Mod 1 Dest", &MOD_DESTINATIONS, 0)
            .describe("The first Mod slot's destination."),
        ParamDescriptor::continuous(MOD1_AMOUNT, "Mod 1 Amount", (-100.0, 100.0), 0.0, ParamUnit::Percent)
            .describe("How far the first Mod slot's source moves its destination, in per cent of its travel."),
        ParamDescriptor::choice(MOD2_SOURCE, "Mod 2 Source", &MOD_SOURCES, 0)
            .describe("The second Mod slot's modulation source."),
        ParamDescriptor::choice(MOD2_DEST, "Mod 2 Dest", &MOD_DESTINATIONS, 0)
            .describe("The second Mod slot's destination."),
        ParamDescriptor::continuous(MOD2_AMOUNT, "Mod 2 Amount", (-100.0, 100.0), 0.0, ParamUnit::Percent)
            .describe("How far the second Mod slot's source moves its destination, in per cent of its travel."),
        ParamDescriptor::choice(MOD3_SOURCE, "Mod 3 Source", &MOD_SOURCES, 0)
            .describe("The third Mod slot's modulation source."),
        ParamDescriptor::choice(MOD3_DEST, "Mod 3 Dest", &MOD_DESTINATIONS, 0)
            .describe("The third Mod slot's destination."),
        ParamDescriptor::continuous(MOD3_AMOUNT, "Mod 3 Amount", (-100.0, 100.0), 0.0, ParamUnit::Percent)
            .describe("How far the third Mod slot's source moves its destination, in per cent of its travel."),
        ParamDescriptor::choice(SHAPE_MOD_SOURCE, "Shape Mod", &MOD_SOURCES, 0)
            .describe("The source for Oscillator 1's Shape Mod."),
        ParamDescriptor::continuous(
            SHAPE_MOD_AMOUNT,
            "Shape Mod Amount",
            (-100.0, 100.0),
            0.0,
            ParamUnit::Percent,
        )
        .describe("How far the Shape Mod source morphs Oscillator 1's waveform."),
        ParamDescriptor::choice(PITCH_MOD1_SOURCE, "Pitch Mod 1", &MOD_SOURCES, 0)
            .describe("The first Pitch Mod source, which moves both oscillators."),
        ParamDescriptor::continuous(
            PITCH_MOD1_AMOUNT,
            "Pitch Mod 1 Amount",
            (-100.0, 100.0),
            0.0,
            ParamUnit::Percent,
        )
        .describe("How far the first Pitch Mod source moves both oscillators."),
        ParamDescriptor::choice(PITCH_MOD2_SOURCE, "Pitch Mod 2", &MOD_SOURCES, 0)
            .describe("The second Pitch Mod source, which moves both oscillators."),
        ParamDescriptor::continuous(
            PITCH_MOD2_AMOUNT,
            "Pitch Mod 2 Amount",
            (-100.0, 100.0),
            0.0,
            ParamUnit::Percent,
        )
        .describe("How far the second Pitch Mod source moves both oscillators."),
        ParamDescriptor::choice(FREQ_MOD1_SOURCE, "Freq Mod 1", &MOD_SOURCES, 0)
            .describe("The first Freq Mod source, which moves the low-pass cutoff."),
        ParamDescriptor::continuous(
            FREQ_MOD1_AMOUNT,
            "Freq Mod 1 Amount",
            (-100.0, 100.0),
            0.0,
            ParamUnit::Percent,
        )
        .describe("How far the first Freq Mod source moves the low-pass cutoff, in per cent of its travel."),
        ParamDescriptor::choice(FREQ_MOD2_SOURCE, "Freq Mod 2", &MOD_SOURCES, 0)
            .describe("The second Freq Mod source, which moves the low-pass cutoff."),
        ParamDescriptor::continuous(
            FREQ_MOD2_AMOUNT,
            "Freq Mod 2 Amount",
            (-100.0, 100.0),
            0.0,
            ParamUnit::Percent,
        )
        .describe("How far the second Freq Mod source moves the low-pass cutoff, in per cent of its travel."),
        instrument::note_pb(NOTE_PB, true),
        instrument::pb_range(PB_RANGE, 96, 48),
    ];
    let layout = DeviceLayout::new(LayoutNode::Row(vec![
        LayoutNode::Band(ControlBand::stacked(
            BODY_HEIGHT,
            &[
                (OSC1, Widget::Toggle),
                (OSC1_WAVE, Widget::Chooser),
                (OSC1_SHAPE, Widget::Knob),
                (OSC1_OCT, Widget::Number),
                (OSC1_GAIN, Widget::Knob),
                (OSC2, Widget::Toggle),
                (OSC2_WAVE, Widget::Chooser),
                (OSC2_OCT, Widget::Number),
                (OSC2_DETUNE, Widget::Number),
                (OSC2_GAIN, Widget::Knob),
                (NOISE, Widget::Toggle),
                (NOISE_GAIN, Widget::Knob),
                (FILTER, Widget::Toggle),
                (RETRIGGER, Widget::Toggle),
                (SHAPE_MOD_SOURCE, Widget::Chooser),
                (SHAPE_MOD_AMOUNT, Widget::Knob),
                (PITCH_MOD1_SOURCE, Widget::Chooser),
                (PITCH_MOD1_AMOUNT, Widget::Knob),
                (PITCH_MOD2_SOURCE, Widget::Chooser),
                (PITCH_MOD2_AMOUNT, Widget::Knob),
            ],
        )),
        LayoutNode::Cell(DisplayCell::new("Waveform", 150.0, BODY_HEIGHT, 1.0)),
        LayoutNode::Band(ControlBand::stacked(
            BODY_HEIGHT,
            &[
                (FREQ, Widget::Knob),
                (FILTER_TYPE, Widget::Chooser),
                (KEY, Widget::Knob),
                (RES, Widget::Knob),
                (HP, Widget::Knob),
                (FREQ_MOD1_SOURCE, Widget::Chooser),
                (FREQ_MOD1_AMOUNT, Widget::Knob),
                (FREQ_MOD2_SOURCE, Widget::Chooser),
                (FREQ_MOD2_AMOUNT, Widget::Knob),
            ],
        )),
        LayoutNode::Band(ControlBand::stacked(
            BODY_HEIGHT,
            &[
                (ENV1_ATTACK, Widget::Knob),
                (ENV1_DECAY, Widget::Knob),
                (ENV1_SUSTAIN, Widget::Knob),
                (ENV1_RELEASE, Widget::Knob),
                (ENV2_ATTACK, Widget::Knob),
                (ENV2_DECAY, Widget::Knob),
                (ENV2_SUSTAIN, Widget::Knob),
                (ENV2_RELEASE, Widget::Knob),
            ],
        )),
        LayoutNode::Band(ControlBand::stacked(
            BODY_HEIGHT,
            &[
                (LFO_SHAPE, Widget::Chooser),
                (LFO_MODE, Widget::Chooser),
                (LFO_SYNC, Widget::Chooser),
                (LFO_RATE, Widget::Knob),
                (LFO_RATIO, Widget::Knob),
                (LFO_TIME, Widget::Knob),
                (LFO_RETRIGGER, Widget::Toggle),
                (LFO_AMOUNT, Widget::Knob),
                (LFO_MOD_SOURCE, Widget::Chooser),
                (LFO_MOD_AMOUNT, Widget::Knob),
            ],
        )),
        LayoutNode::Band(ControlBand::stacked(
            BODY_HEIGHT,
            &[
                (MOD1_SOURCE, Widget::Chooser),
                (MOD1_DEST, Widget::Chooser),
                (MOD1_AMOUNT, Widget::Knob),
                (MOD2_SOURCE, Widget::Chooser),
                (MOD2_DEST, Widget::Chooser),
                (MOD2_AMOUNT, Widget::Knob),
                (MOD3_SOURCE, Widget::Chooser),
                (MOD3_DEST, Widget::Chooser),
                (MOD3_AMOUNT, Widget::Knob),
            ],
        )),
        LayoutNode::Band(ControlBand::stacked(
            BODY_HEIGHT,
            &[
                (VOLUME, Widget::Knob),
                (VOICES, Widget::Number),
                (TRANSPOSE, Widget::Number),
                (VEL_VOL, Widget::Knob),
                (NOTE_PB, Widget::Toggle),
                (PB_RANGE, Widget::Number),
            ],
        )),
    ]));
    DeviceType::native(POLYSYNTH, "Drift", DeviceRole::Instrument)
        .with_description(
            DeviceDescription::new(params)
                .with_layout(layout)
                .with_banks(&[
                    (
                        "Oscillators",
                        &[
                            "Osc 1",
                            "Osc 1 Wave",
                            "Osc 1 Oct",
                            "Osc 1 Gain",
                            "Osc 2",
                            "Osc 2 Wave",
                            "Osc 2 Oct",
                            "Osc 2 Gain",
                        ],
                    ),
                    (
                        "Osc Detail",
                        &["Osc 1 Shape", "Osc 2 Detune", "Noise", "Noise Gain"],
                    ),
                    ("Filter", &["Filter", "Type", "Freq", "Res", "HP", "Key"]),
                    (
                        "Envelope 1",
                        &[
                            "Env 1 Attack",
                            "Env 1 Decay",
                            "Env 1 Sustain",
                            "Env 1 Release",
                        ],
                    ),
                    (
                        "Envelope 2",
                        &[
                            "Env 2 Attack",
                            "Env 2 Decay",
                            "Env 2 Sustain",
                            "Env 2 Release",
                        ],
                    ),
                    (
                        "Global",
                        &[
                            "Volume",
                            "Voices",
                            "Transpose",
                            "Vel > Vol",
                            "Retrigger",
                            "Note PB",
                            "PB Range",
                        ],
                    ),
                    (
                        "LFO",
                        &[
                            "LFO Mode",
                            "LFO Rate",
                            "LFO Ratio",
                            "LFO Time",
                            "LFO Sync",
                            "LFO Shape",
                            "LFO R",
                            "LFO Amount",
                        ],
                    ),
                    (
                        "LFO Mod",
                        &["LFO Mod", "LFO Mod Amount", "Shape Mod", "Shape Mod Amount"],
                    ),
                    ("Mod 1", &["Mod 1 Source", "Mod 1 Dest", "Mod 1 Amount"]),
                    ("Mod 2", &["Mod 2 Source", "Mod 2 Dest", "Mod 2 Amount"]),
                    ("Mod 3", &["Mod 3 Source", "Mod 3 Dest", "Mod 3 Amount"]),
                    (
                        "Pitch Mod",
                        &[
                            "Pitch Mod 1",
                            "Pitch Mod 1 Amount",
                            "Pitch Mod 2",
                            "Pitch Mod 2 Amount",
                        ],
                    ),
                    (
                        "Freq Mod",
                        &[
                            "Freq Mod 1",
                            "Freq Mod 1 Amount",
                            "Freq Mod 2",
                            "Freq Mod 2 Amount",
                        ],
                    ),
                ]),
        )
        .with_matrix(polysynth_matrix())
}

/// Spec: instrument modulation R17. Drift's matrix: the sources and
/// destinations the choosers list after Off (and Pitch, which only the Pitch
/// Mod slots reach), the three Mod slots as chosen slots and the dedicated
/// Shape, Pitch, Freq and LFO slots as fixed ones, in the order their routes
/// have always been summed in. Every destination is worked out every frame
/// (Drift has no Hi-Quality), per voice and additive.
#[must_use]
pub fn polysynth_matrix() -> MatrixLayout {
    let source = |which: usize, range, scope| SourceDef {
        name: MOD_SOURCES[which + 1],
        range,
        scope,
    };
    let dest = |which: usize, param| DestDef {
        name: MOD_DESTINATIONS[which + 1],
        param: Some(param),
        scope: Scope::Voice,
        rate: Rate::Audio,
        kind: Kind::Additive,
    };
    let chosen = |source, dest, amount| SlotDef {
        source: Pick::Chosen(source),
        dest: Pick::Chosen(dest),
        amount,
    };
    let fixed = |source, dest: usize, amount| SlotDef {
        source: Pick::Chosen(source),
        dest: Pick::Fixed(dest as u8),
        amount,
    };
    MatrixLayout {
        sources: vec![
            source(SRC_ENV1, Range::Unipolar, Scope::Voice),
            source(SRC_ENV2, Range::Unipolar, Scope::Voice),
            source(SRC_LFO, Range::Bipolar, Scope::Voice),
            source(SRC_KEY, Range::Bipolar, Scope::Voice),
            source(SRC_VELOCITY, Range::Unipolar, Scope::Voice),
            source(SRC_MODWHEEL, Range::Unipolar, Scope::Global),
            source(SRC_PRESSURE, Range::Unipolar, Scope::Voice),
            source(SRC_SLIDE, Range::Unipolar, Scope::Voice),
        ],
        dests: vec![
            dest(DEST_OSC1_GAIN, OSC1_GAIN),
            dest(DEST_OSC1_SHAPE, OSC1_SHAPE),
            dest(DEST_OSC2_GAIN, OSC2_GAIN),
            dest(DEST_OSC2_DETUNE, OSC2_DETUNE),
            dest(DEST_NOISE_GAIN, NOISE_GAIN),
            dest(DEST_LP_FREQ, FREQ),
            dest(DEST_LP_RES, RES),
            dest(DEST_HP_FREQ, HP),
            dest(DEST_LFO_RATE, LFO_RATE),
            dest(DEST_VOLUME, VOLUME),
            DestDef {
                name: "Pitch",
                param: None,
                scope: Scope::Voice,
                rate: Rate::Audio,
                kind: Kind::Additive,
            },
        ],
        slots: vec![
            chosen(MOD1_SOURCE, MOD1_DEST, MOD1_AMOUNT),
            chosen(MOD2_SOURCE, MOD2_DEST, MOD2_AMOUNT),
            chosen(MOD3_SOURCE, MOD3_DEST, MOD3_AMOUNT),
            fixed(SHAPE_MOD_SOURCE, DEST_OSC1_SHAPE, SHAPE_MOD_AMOUNT),
            fixed(PITCH_MOD1_SOURCE, DEST_PITCH, PITCH_MOD1_AMOUNT),
            fixed(PITCH_MOD2_SOURCE, DEST_PITCH, PITCH_MOD2_AMOUNT),
            fixed(FREQ_MOD1_SOURCE, DEST_LP_FREQ, FREQ_MOD1_AMOUNT),
            fixed(FREQ_MOD2_SOURCE, DEST_LP_FREQ, FREQ_MOD2_AMOUNT),
            fixed(LFO_MOD_SOURCE, DEST_LFO_RATE, LFO_MOD_AMOUNT),
        ],
        forbidden: Vec::new(),
    }
}

/// Adds Drift to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, polysynth_type(), |_, config| make(config));
}

/// Makes a Drift for `config`'s rate, channels, and block size. Every
/// buffer and table it needs is made here, off the audio thread.
#[must_use]
pub fn make(config: &NodeConfig) -> PolySynth {
    PolySynth::new(config)
}

/// The waveform at phase `p` (0 to 1) before the Shape knob narrows it.
/// Beatlayer's own readings of the manual's names; the manual draws none
/// of them.
fn base_wave(wave: usize, p: f64) -> f64 {
    match wave {
        SINE => (TAU * p).sin(),
        TRIANGLE => 1.0 - 4.0 * (p - 0.5).abs(),
        // A saw with a slower rise and a fast fall: the manual's shark
        // tooth, which it says follows a classic analogue shape.
        SHARK => {
            if p < 0.8 {
                -1.0 + 2.5 * p
            } else {
                1.0 - 10.0 * (p - 0.8)
            }
        }
        // A sine driven into a soft clipper, for the manual's bass sounds.
        SATURATED => (2.5 * (TAU * p).sin()).tanh() / 2.5f64.tanh(),
        SAW => 2.0 * p - 1.0,
        // A quarter-width pulse, next to the rectangle's half.
        PULSE => {
            if p < 0.25 {
                1.0
            } else {
                -1.0
            }
        }
        RECTANGLE => {
            if p < 0.5 {
                1.0
            } else {
                -1.0
            }
        }
        _ => 0.0,
    }
}

/// The waveform at phase `p` with the Shape knob at `shape` (0 to 1).
/// A rectangle or pulse narrows its duty cycle; the others compress their
/// rise and stretch their fall, so a saw moves towards a triangle. Both
/// are Beatlayer's own; the manual only says the harmonic content changes.
fn shaped_wave(wave: usize, p: f64, shape: f64) -> f64 {
    let shape = shape.clamp(0.0, 1.0);
    match wave {
        PULSE | RECTANGLE => {
            let base = if wave == PULSE { 0.25 } else { 0.5 };
            let duty = base + (MIN_DUTY - base) * shape;
            if p < duty { 1.0 } else { -1.0 }
        }
        _ => {
            let duty = 1.0 - (1.0 - MIN_DUTY) * shape;
            if p < duty {
                base_wave(wave, p / duty)
            } else {
                base_wave(wave, (p - duty) / (1.0 - duty))
            }
        }
    }
}

/// One waveform's table: `frames` frames of the shape, band-limited into
/// mipmaps by the kit's table. `shape_frames` is false for an oscillator
/// whose Shape knob doesn't apply, which gets one frame.
fn wave_table(wave: usize, shape_frames: bool) -> SharedTable {
    let frames = if shape_frames { SHAPE_FRAMES } else { 1 };
    let table = Wavetable::from_fn(frames, |frame, sample| {
        let p = sample as f64 / FRAME_SIZE as f64;
        let shape = if frames > 1 {
            frame as f64 / (frames - 1) as f64
        } else {
            0.0
        };
        shaped_wave(wave, p, shape) as f32
    })
    .expect("a table of whole frames");
    Cache::global().share(table)
}

/// Oscillator 2's waveforms in the shared shape numbering: its own
/// chooser's order.
const WAVES2_BASE: [usize; 5] = [SINE, TRIANGLE, SATURATED, SAW, RECTANGLE];

/// A soft clipper that leaves small signals alone and bends large ones
/// towards `limit`, `amount` from 0 (the identity) to 1 (fully bent).
fn soft_clip(x: f32, amount: f32) -> f32 {
    if amount <= 0.0 {
        return x;
    }
    let knee = 0.5;
    let magnitude = x.abs();
    if magnitude <= knee {
        return x;
    }
    let over = (magnitude - knee) / knee;
    let bent = knee + knee * over.tanh();
    let clipped = x.signum() * bent;
    x + (clipped - x) * amount
}

/// The waveform an LFO plays at `phase` (0 to 1): `held` is the value
/// drawn for this cycle and `drawn` the one being moved towards, which the
/// generators read. The envelope waveforms decay over each cycle, and
/// their curves are Beatlayer's own: the manual names them but draws none.
fn lfo_wave(shape: usize, phase: f64, held: f64, drawn: f64) -> f64 {
    match shape {
        LFO_SINE => (TAU * phase).sin(),
        LFO_TRIANGLE => 1.0 - 4.0 * (phase - 0.5).abs(),
        LFO_SAW_UP => 2.0 * phase - 1.0,
        LFO_SAW_DOWN => 1.0 - 2.0 * phase,
        LFO_SQUARE => {
            if phase < 0.5 {
                1.0
            } else {
                -1.0
            }
        }
        // A value drawn each cycle and held.
        LFO_SAMPLE_HOLD => held,
        // Sample and hold with an S-shape between the two values.
        LFO_WANDER => held + (drawn - held) * phase * phase * (3.0 - 2.0 * phase),
        // A one-shot decay envelope, over each cycle of the LFO's rate.
        LFO_LINEAR => 1.0 - phase,
        LFO_EXPONENTIAL => (-EXPONENTIAL_DECAY * phase).exp(),
        _ => 0.0,
    }
}

/// The stretch's modulation source lanes: the envelopes and the LFO, one
/// buffer `frames × stride` each.
struct SourceLanes<'a> {
    env1: &'a [f32],
    env2: &'a [f32],
    lfo: &'a [f32],
    stride: usize,
}

/// Voice `voice`'s value for source `which` at `frame` of a stretch,
/// in the source's units: the envelopes, velocity, the mod wheel, pressure
/// and slide run from 0 to 1; the LFO and Key run from −1 to 1.
fn source_value(
    which: usize,
    voice: usize,
    frame: usize,
    lanes: &SourceLanes<'_>,
    voices: &VoiceBlock,
) -> f64 {
    match which {
        SRC_ENV1 => f64::from(lanes.env1[frame * lanes.stride + voice]),
        SRC_ENV2 => f64::from(lanes.env2[frame * lanes.stride + voice]),
        SRC_LFO => f64::from(lanes.lfo[frame * lanes.stride + voice]),
        // Spec: instrument voices R15 (polysynth R21): the voice section's
        // sources.
        SRC_KEY => Sources::of(voices).key(voice),
        SRC_VELOCITY => Sources::of(voices).velocity(voice),
        SRC_MODWHEEL => Sources::of(voices).mod_wheel(),
        SRC_PRESSURE => Sources::of(voices).pressure(voice),
        SRC_SLIDE => Sources::of(voices).slide(voice),
        _ => 0.0,
    }
}

/// The LFO's settings for a stretch.
#[derive(Clone, Copy)]
struct Lfos {
    shape: usize,
    /// Its time mode, one of the `MODE_*` values.
    mode: usize,
    /// Whether a note restarts the LFO.
    retrigger: bool,
    /// Its overall intensity, 0 to 1.
    amount: f64,
    /// Its speed in cycles a second, in the Rate, Time and Sync modes.
    hz: f64,
    /// Its speed as a ratio of the note's frequency, in the Ratio mode.
    ratio: f64,
}

/// The LFO's state: a phase for every voice, and one more for a
/// free-running LFO, which every voice reads.
struct Lfo {
    phase: [f64; MAX_VOICES],
    held: [f64; MAX_VOICES],
    drawn: [f64; MAX_VOICES],
    rng: [u32; MAX_VOICES],
    free_phase: f64,
    free_held: f64,
    free_drawn: f64,
    free_rng: u32,
}

impl Lfo {
    fn new() -> Self {
        let mut lfo = Self {
            phase: [0.0; MAX_VOICES],
            held: [0.0; MAX_VOICES],
            drawn: [0.0; MAX_VOICES],
            rng: core::array::from_fn(|voice| 0x5EED_0F05u32.wrapping_mul(voice as u32 + 1) | 1),
            free_phase: 0.0,
            free_held: 0.0,
            free_drawn: 0.0,
            free_rng: 0x5EED_0F05,
        };
        for voice in 0..MAX_VOICES {
            lfo.held[voice] = lfo.random(voice);
            lfo.drawn[voice] = lfo.random(voice);
        }
        lfo.free_held = lfo.random_free();
        lfo.free_drawn = lfo.random_free();
        lfo
    }

    /// Starts voice `voice`'s cycle at its beginning, as a note does.
    fn restart(&mut self, voice: usize) {
        self.phase[voice] = 0.0;
        self.held[voice] = self.drawn[voice];
        self.drawn[voice] = self.random(voice);
    }

    /// A new value from −1 to 1 for voice `voice`'s generators.
    fn random(&mut self, voice: usize) -> f64 {
        let state = &mut self.rng[voice];
        *state ^= *state << 13;
        *state ^= *state >> 17;
        *state ^= *state << 5;
        f64::from(*state >> 8) / f64::from(1 << 24) * 2.0 - 1.0
    }

    /// A new value from −1 to 1 for the free-running LFO's generators.
    fn random_free(&mut self) -> f64 {
        let state = &mut self.free_rng;
        *state ^= *state << 13;
        *state ^= *state >> 17;
        *state ^= *state << 5;
        f64::from(*state >> 8) / f64::from(1 << 24) * 2.0 - 1.0
    }
}

/// Drift's audio-thread half.
pub struct PolySynth {
    section: Section,
    synth: Synth,
}

impl PolySynth {
    fn new(config: &NodeConfig) -> Self {
        let rate = config.sample_rate.as_f64();
        Self {
            // Spec: instrument voices R22. Drift's voices are the voice
            // section's, with the parts Drift has.
            section: Section::new(
                Parts {
                    voices: Some(VOICES),
                    transpose: Some(TRANSPOSE),
                    pb_range: Some(PB_RANGE),
                    note_pb: Some(NOTE_PB),
                    note_range: NoteRange::PbRange,
                    ..Parts::default()
                },
                MAX_VOICES,
                1,
                rate,
                0,
            ),
            synth: Synth::new(rate, config.max_frames),
        }
    }
}

impl NativeDevice for PolySynth {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        let frames = audio.frames();
        if frames == 0 || audio.channels() == 0 {
            return;
        }
        self.synth.snapshot(audio, params);
        self.section.process(audio, params, &mut self.synth);
        for channel in 0..audio.channels() {
            audio
                .channel_mut(channel)
                .copy_from_slice(&self.synth.out[..frames]);
        }
    }

    /// Spec: instrument modulation R12; modulation R31. What the newest
    /// voice plays for each parameter the matrix moves.
    fn matrix_played(&self, report: &mut dyn FnMut(ParamId, f64)) {
        let matrix = &self.synth.matrix;
        for (dest, def) in matrix.layout().dests.iter().enumerate() {
            if let (Some(param), Some(position)) = (def.param, matrix.shown(dest)) {
                report(param, self.synth.value_of(dest, position));
            }
        }
    }

    fn flush(&mut self, _events: &[NodeEvent], _params: &ParamBank) {
        // A switched-off instrument is silent and holds no notes, so it
        // doesn't carry one over to the moment it comes back on.
        self.section.reset();
        self.synth.silence();
    }

    /// The pitch of every key under the set's tuning (the tunings spec,
    /// R12, R18). Notes already sounding keep the pitch they started at.
    fn receive_data(&mut self, data: NodeData) {
        self.section.receive_data(&data);
    }
}

/// Everything but the voice section: the [`Instrument`] the voices play.
struct Synth {
    rate: f64,
    stride: usize,
    osc1: Oscillator,
    osc2: Oscillator,
    tables1: Vec<SharedTable>,
    tables2: Vec<SharedTable>,
    lp: FilterBank,
    hp: FilterBank,
    env1: EnvelopeBank,
    env2: EnvelopeBank,
    rng: [u32; MAX_VOICES],
    lfo: Lfo,
    /// The Mod destinations' descriptors, by destination index: what each
    /// one's travel means.
    destinations: Vec<ParamDescriptor>,
    /// The parameters of the stretch being played, frame by frame: the
    /// sources' gains, the volume, the cutoffs, the key tracking and the
    /// velocity amount.
    g1: Vec<f32>,
    g2: Vec<f32>,
    gn: Vec<f32>,
    volume: Vec<f32>,
    freq: Vec<f32>,
    hp_hz: Vec<f32>,
    key: Vec<f32>,
    res: Vec<f32>,
    detune: Vec<f32>,
    vel_vol: Vec<f32>,
    /// The stretch's per-voice buffers, `frames × stride` each.
    osc1_out: Vec<f32>,
    osc2_out: Vec<f32>,
    audio: Vec<f32>,
    cutoff: Vec<f32>,
    hp_cutoff: Vec<f32>,
    env1_out: Vec<f32>,
    env2_out: Vec<f32>,
    lfo_out: Vec<f32>,
    pitch_mod: Vec<f32>,
    pitch_mod2: Vec<f32>,
    frame_mod: Vec<f32>,
    /// The stretch's mixed output, one value a frame.
    out: Vec<f32>,
    /// The switches and choices of the stretch.
    osc1_on: bool,
    osc2_on: bool,
    noise_on: bool,
    filter_on: bool,
    retrigger: bool,
    wave1: usize,
    wave2: usize,
    filter_type: usize,
    /// The LFO's settings, and the modulation matrix whose routes the
    /// stretch reads.
    lfos: Lfos,
    matrix: Matrix,
    /// Oscillator 1's Shape as a position on its travel, for the control
    /// to show the matrix's movement of it.
    shape: f64,
    /// The filter's settings and the envelopes', kept so a change is
    /// applied once; and whether a route moved the resonance, so the
    /// voices come back to the base settings.
    filter_params: VoiceParams,
    env1_params: EnvelopeParams,
    env2_params: EnvelopeParams,
    filter_resonance_moved: bool,
    /// The saturation the sources' gains ask for, before and after the
    /// filter.
    pre: f32,
    post: f32,
}

impl Synth {
    fn new(rate: f64, max_frames: usize) -> Self {
        let max_frames = max_frames.max(1);
        let layout = OscLayout {
            voices: MAX_VOICES,
            lanes_per_voice: 1,
        };
        let tables1: Vec<SharedTable> = (0..WAVES1.len()).map(|w| wave_table(w, true)).collect();
        let tables2: Vec<SharedTable> = WAVES2_BASE
            .iter()
            .map(|&wave| wave_table(wave, false))
            .collect();
        let osc1 = Oscillator::new(
            tables1[SAW].clone(),
            layout,
            rate as f32,
            max_frames,
            0x0C45_CADE,
        );
        let osc2 = Oscillator::new(
            tables2[3].clone(),
            layout,
            rate as f32,
            max_frames,
            0x0C45_CADF,
        );
        let mut lp = FilterBank::new(rate as f32, MAX_VOICES, 1);
        lp.set_type(FilterType::Lp12);
        let mut hp = FilterBank::new(rate as f32, MAX_VOICES, 1);
        hp.set_type(FilterType::Hp12);
        let stride = MAX_VOICES.next_multiple_of(16);
        let buffer = || vec![0.0; max_frames * stride];
        let ramp = || vec![0.0; max_frames];
        let description = polysynth_type().description;
        let destination = |id: ParamId| {
            description
                .param(id)
                .expect("the description lists every destination")
                .clone()
        };
        let destinations = vec![
            destination(OSC1_GAIN),
            destination(OSC1_SHAPE),
            destination(OSC2_GAIN),
            destination(OSC2_DETUNE),
            destination(NOISE_GAIN),
            destination(FREQ),
            destination(RES),
            destination(HP),
            destination(LFO_RATE),
            destination(VOLUME),
        ];
        Self {
            rate,
            stride,
            osc1,
            osc2,
            tables1,
            tables2,
            lp,
            hp,
            env1: EnvelopeBank::new(MAX_VOICES, EnvelopeParams::AMP, Timing::new(rate)),
            env2: EnvelopeBank::new(MAX_VOICES, EnvelopeParams::MODULATION, Timing::new(rate)),
            rng: core::array::from_fn(|voice| 0x9E37_79B9u32.wrapping_mul(voice as u32 + 1) | 1),
            lfo: Lfo::new(),
            destinations,
            g1: ramp(),
            g2: ramp(),
            gn: ramp(),
            volume: ramp(),
            freq: ramp(),
            hp_hz: ramp(),
            key: ramp(),
            res: ramp(),
            detune: ramp(),
            vel_vol: ramp(),
            osc1_out: buffer(),
            osc2_out: buffer(),
            audio: buffer(),
            cutoff: buffer(),
            hp_cutoff: buffer(),
            env1_out: buffer(),
            env2_out: buffer(),
            lfo_out: buffer(),
            pitch_mod: buffer(),
            pitch_mod2: buffer(),
            frame_mod: buffer(),
            out: vec![0.0; max_frames],
            osc1_on: true,
            osc2_on: true,
            noise_on: false,
            filter_on: true,
            retrigger: true,
            wave1: SAW,
            wave2: 3,
            filter_type: 0,
            lfos: Lfos {
                shape: LFO_SINE,
                mode: MODE_SYNC,
                retrigger: true,
                amount: 1.0,
                hz: 1.0,
                ratio: 1.0,
            },
            shape: 0.0,
            matrix: Matrix::new(
                polysynth_type()
                    .matrix
                    .expect("Drift's type has its matrix"),
            ),
            filter_params: VoiceParams::default(),
            env1_params: EnvelopeParams::AMP,
            env2_params: EnvelopeParams::MODULATION,
            filter_resonance_moved: false,
            pre: 0.0,
            post: 0.0,
        }
    }

    /// Reads the stretch's parameters into the synth: the ramps into its
    /// buffers, the switches and choices into its fields, and everything
    /// the oscillators, the filters, the envelopes, the LFO and the
    /// modulation routes need.
    fn snapshot(&mut self, audio: &Audio<'_, '_>, params: &ParamBank) {
        let frames = audio.frames();
        fn ramp_into(to: &mut [f32], params: &ParamBank, id: ParamId) {
            let index = params.index(id).expect("the description holds it");
            to.copy_from_slice(params.ramp(index));
        }
        let at = |id: ParamId| params.index(id).expect("the description holds it");
        ramp_into(&mut self.g1[..frames], params, OSC1_GAIN);
        ramp_into(&mut self.g2[..frames], params, OSC2_GAIN);
        ramp_into(&mut self.gn[..frames], params, NOISE_GAIN);
        ramp_into(&mut self.volume[..frames], params, VOLUME);
        ramp_into(&mut self.freq[..frames], params, FREQ);
        ramp_into(&mut self.hp_hz[..frames], params, HP);
        ramp_into(&mut self.key[..frames], params, KEY);
        ramp_into(&mut self.res[..frames], params, RES);
        ramp_into(&mut self.detune[..frames], params, OSC2_DETUNE);
        ramp_into(&mut self.vel_vol[..frames], params, VEL_VOL);
        self.out[..frames].fill(0.0);

        self.osc1_on = params.value(at(OSC1)) >= 0.5;
        self.osc2_on = params.value(at(OSC2)) >= 0.5;
        self.noise_on = params.value(at(NOISE)) >= 0.5;
        self.filter_on = params.value(at(FILTER)) >= 0.5;
        self.retrigger = params.value(at(RETRIGGER)) >= 0.5;
        let wave1 = (params.value(at(OSC1_WAVE)) as usize).min(WAVES1.len() - 1);
        let wave2 = (params.value(at(OSC2_WAVE)) as usize).min(WAVES2.len() - 1);
        if wave1 != self.wave1 {
            self.wave1 = wave1;
            self.osc1.set_table(self.tables1[wave1].clone());
        }
        if wave2 != self.wave2 {
            self.wave2 = wave2;
            self.osc2.set_table(self.tables2[wave2].clone());
        }
        let transpose = params.value(at(TRANSPOSE)) as f32;
        let shape1 = params.value(at(OSC1_SHAPE)) as f32 / 100.0;
        self.shape = f64::from(shape1);
        let phase_mode = if self.retrigger {
            PhaseMode::NoteStart
        } else {
            PhaseMode::Continue
        };
        for (osc, oct, semitone, shape) in [
            (
                &mut self.osc1,
                params.value(at(OSC1_OCT)) as i32,
                0,
                Some(shape1),
            ),
            (
                &mut self.osc2,
                params.value(at(OSC2_OCT)) as i32,
                params.value(at(OSC2_DETUNE)) as i32,
                None,
            ),
        ] {
            let p: &mut OscParams = osc.params_mut();
            p.octave = oct;
            p.semitone = semitone;
            p.coarse = transpose;
            p.phase_mode = phase_mode;
            p.phase_random = 0.0;
            p.phase = 0.0;
            p.level = 100.0;
            p.reference = REFERENCE;
            if let Some(shape) = shape {
                p.frame = 1.0 + shape * (SHAPE_FRAMES - 1) as f32;
                p.smooth = true;
            }
        }

        let filter_type = (params.value(at(FILTER_TYPE)) as usize).min(FILTER_TYPES.len() - 1);
        if filter_type != self.filter_type {
            self.filter_type = filter_type;
            self.lp.set_type(if filter_type == 0 {
                FilterType::Lp12
            } else {
                FilterType::Lp24
            });
        }
        // The manual's two saturation points: the gains' headroom drives
        // the filter's clipper and the resonance's taming, and a second
        // clipper past the filter. Both slopes are Beatlayer's own.
        let headroom = self.source_headroom_db(params, at);
        let pre = ((headroom - DRIVE_START_DB) / (DRIVE_FULL_DB - DRIVE_START_DB)).clamp(0.0, 1.0);
        self.pre = pre as f32;
        self.post =
            ((headroom - POST_START_DB) / (DRIVE_FULL_DB - POST_START_DB)).clamp(0.0, 1.0) as f32;
        let filter_params = VoiceParams {
            resonance: (params.value(at(RES)) / 100.0).clamp(0.0, 1.0) as f32,
            drive: self.pre,
            saturation: self.pre,
            mix: 1.0,
            level: 1.0,
            ..VoiceParams::default()
        };
        if filter_params != self.filter_params || self.filter_resonance_moved {
            self.filter_params = filter_params;
            self.filter_resonance_moved = false;
            for voice in 0..MAX_VOICES {
                self.lp.set_params(voice, &filter_params);
                self.hp.set_params(voice, &filter_params);
            }
        }

        let env1 = envelope_params(
            params,
            at,
            ENV1_ATTACK,
            ENV1_DECAY,
            ENV1_SUSTAIN,
            ENV1_RELEASE,
        );
        if env1 != self.env1_params {
            self.env1_params = env1;
            self.env1.set_all_params(env1);
        }
        let env2 = EnvelopeParams {
            steal_restart: StealRestart::StartLevel,
            ..envelope_params(
                params,
                at,
                ENV2_ATTACK,
                ENV2_DECAY,
                ENV2_SUSTAIN,
                ENV2_RELEASE,
            )
        };
        if env2 != self.env2_params {
            self.env2_params = env2;
            self.env2.set_all_params(env2);
        }

        let mode = (params.value(at(LFO_MODE)) as usize).min(LFO_MODES.len() - 1);
        let division = (params.value(at(LFO_SYNC)) as usize).min(LFO_DIVISIONS.len() - 1);
        // The Sync mode's cycle comes from the tempo map's tempo at the
        // stretch, as the delay device's does.
        let bpm = audio.bpm();
        let sixteenth = self.rate * 60.0 / bpm / 4.0;
        self.lfos = Lfos {
            shape: (params.value(at(LFO_SHAPE)) as usize).min(LFO_SHAPES.len() - 1),
            mode,
            retrigger: params.value(at(LFO_RETRIGGER)) >= 0.5,
            amount: (params.value(at(LFO_AMOUNT)) / 100.0).clamp(0.0, 1.0),
            hz: match mode {
                MODE_RATE => params.value(at(LFO_RATE)).max(0.0),
                MODE_TIME => 1_000.0 / params.value(at(LFO_TIME)).max(0.001),
                MODE_SYNC => self.rate / (DIVISION_SIXTEENTHS[division] * sixteenth).max(1.0),
                _ => params.value(at(LFO_RATIO)),
            },
            ratio: params.value(at(LFO_RATIO)).max(0.0),
        };
        self.matrix.begin(params);
    }

    /// The loudest enabled source's gain, in dB: what the manual's two
    /// saturation points read.
    fn source_headroom_db(&self, params: &ParamBank, at: impl Fn(ParamId) -> usize) -> f64 {
        let mut headroom = f64::NEG_INFINITY;
        if self.osc1_on {
            headroom = headroom.max(gain_to_db(params.value(at(OSC1_GAIN))));
        }
        if self.osc2_on {
            headroom = headroom.max(gain_to_db(params.value(at(OSC2_GAIN))));
        }
        if self.noise_on {
            headroom = headroom.max(gain_to_db(params.value(at(NOISE_GAIN))));
        }
        headroom
    }

    /// Drops everything the synth holds: for a device switched off.
    fn silence(&mut self) {
        for voice in 0..MAX_VOICES {
            self.env1.reset(voice);
            self.env2.reset(voice);
        }
        self.lp.reset();
        self.hp.reset();
        self.out.fill(0.0);
    }

    /// Voice `voice`'s value for source `which` at `frame` of the stretch.
    fn source(&self, which: usize, voice: usize, frame: usize, voices: &VoiceBlock) -> f64 {
        let lanes = SourceLanes {
            env1: &self.env1_out,
            env2: &self.env2_out,
            lfo: &self.lfo_out,
            stride: self.stride,
        };
        source_value(which, voice, frame, &lanes, voices)
    }

    /// Voice `voice`'s position (0 to 1 of its travel) for the parameter
    /// behind destination `dest`, given the parameter's own `base`
    /// position, after every route into it at `frame`.
    fn modulated(
        &self,
        base: f64,
        dest: usize,
        voice: usize,
        frame: usize,
        voices: &VoiceBlock,
    ) -> f64 {
        self.matrix
            .position(dest, base, voice, frame, true, &|source, voice, frame| {
                self.source(source, voice, frame, voices)
            })
    }

    /// The travel position of destination `dest`'s parameter at `value`.
    fn position_of(&self, dest: usize, value: f64) -> f64 {
        self.destinations[dest].normalize(value)
    }

    /// The value of destination `dest`'s parameter at `position`.
    fn value_of(&self, dest: usize, position: f64) -> f64 {
        self.destinations[dest].denormalize(position)
    }

    /// Voice `voice`'s value for destination `dest` at `frame`: the
    /// parameter's own `base` position, every route into it, and the
    /// switch that turns its source off.
    fn destination(
        &self,
        dest: usize,
        value: f64,
        on: bool,
        voice: usize,
        frame: usize,
        voices: &VoiceBlock,
    ) -> f64 {
        if !on {
            return 0.0;
        }
        if !self.matrix.has(dest) {
            return value;
        }
        let position = self.modulated(self.position_of(dest, value), dest, voice, frame, voices);
        self.value_of(dest, position)
    }

    /// The LFO's speed in cycles a second for `voice` at `frame`, after
    /// every route into its rate: a full route is ±4 octaves, Beatlayer's
    /// own reading. In the Ratio mode the speed follows the note, and
    /// elsewhere it is the mode's own rate.
    fn lfo_rate(&self, voice: usize, frame: usize, voices: &VoiceBlock) -> f64 {
        let delta = self
            .matrix
            .add(DEST_LFO_RATE, 0.0, voice, frame, &|source, voice, frame| {
                self.source(source, voice, frame, voices)
            });
        let base = if self.lfos.mode == MODE_RATIO {
            let note = f64::from(voices.pitch()[voice] + voices.pitch_step()[voice] * frame as f32);
            self.lfos.ratio * 440.0 * ((note - 69.0) / 12.0).exp2()
        } else {
            self.lfos.hz
        };
        base * (delta * LFO_RATE_OCTAVES).exp2()
    }

    /// Renders the LFO's value for every active voice, frame by frame,
    /// into `lfo_out`: each voice's own phase when the LFO retriggers, or
    /// in the Ratio mode whose speed follows the note, and one free-running
    /// phase, which every voice reads, otherwise.
    fn render_lfo(&mut self, voices: &VoiceBlock, count: usize) {
        let stride = self.stride;
        let lfos = self.lfos;
        let shared = !lfos.retrigger && lfos.mode != MODE_RATIO;
        let mask = voices.active_mask();
        let first = if mask == 0 {
            0
        } else {
            mask.trailing_zeros() as usize
        };
        let rate = self.rate;
        // The speed is a per-voice value; the free-running LFO takes it
        // from the first sounding voice, which is Beatlayer's own reading
        // of a shared LFO driven by per-voice sources.
        for frame in 0..count {
            if shared {
                let cycles = self.lfo_rate(first, frame, voices) / rate;
                self.lfo.free_phase += cycles;
                if self.lfo.free_phase >= 1.0 {
                    self.lfo.free_phase -= self.lfo.free_phase.floor();
                    self.lfo.free_held = self.lfo.free_drawn;
                    self.lfo.free_drawn = self.lfo.random_free();
                }
                let value = (lfo_wave(
                    lfos.shape,
                    self.lfo.free_phase,
                    self.lfo.free_held,
                    self.lfo.free_drawn,
                ) * lfos.amount) as f32;
                let mut active = mask;
                while active != 0 {
                    let voice = active.trailing_zeros() as usize;
                    active &= active - 1;
                    self.lfo_out[frame * stride + voice] = value;
                }
            } else {
                let mut active = mask;
                while active != 0 {
                    let voice = active.trailing_zeros() as usize;
                    active &= active - 1;
                    let cycles = self.lfo_rate(voice, frame, voices) / rate;
                    self.lfo.phase[voice] += cycles;
                    if self.lfo.phase[voice] >= 1.0 {
                        self.lfo.phase[voice] -= self.lfo.phase[voice].floor();
                        self.lfo.held[voice] = self.lfo.drawn[voice];
                        self.lfo.drawn[voice] = self.lfo.random(voice);
                    }
                    let value = (lfo_wave(
                        lfos.shape,
                        self.lfo.phase[voice],
                        self.lfo.held[voice],
                        self.lfo.drawn[voice],
                    ) * lfos.amount) as f32;
                    self.lfo_out[frame * stride + voice] = value;
                }
            }
        }
    }

    /// One frame of voice `voice`'s white noise.
    fn noise(&mut self, voice: usize) -> f32 {
        let state = &mut self.rng[voice];
        *state ^= *state << 13;
        *state ^= *state >> 17;
        *state ^= *state << 5;
        (*state as f32 / u32::MAX as f32) * 2.0 - 1.0
    }
}

/// An envelope's parameters from the four knobs `params` holds.
fn envelope_params(
    params: &ParamBank,
    at: impl Fn(ParamId) -> usize,
    attack: ParamId,
    decay: ParamId,
    sustain: ParamId,
    release: ParamId,
) -> EnvelopeParams {
    EnvelopeParams {
        unit: TimeUnit::Milliseconds,
        delay: 0.0,
        attack: params.value(at(attack)) as f32,
        hold: 0.0,
        decay: params.value(at(decay)) as f32,
        sustain: (params.value(at(sustain)) / 100.0) as f32,
        release: params.value(at(release)) as f32,
        attack_curve: ATTACK_CURVE,
        decay_curve: DECAY_CURVE,
        release_curve: RELEASE_CURVE,
        start: 0.0,
        end: 0.0,
        legato_inverted: false,
        steal_restart: StealRestart::CurrentLevel,
    }
}

impl Instrument for Synth {
    fn voice_event(&mut self, voices: &mut VoiceBlock, _state: &State, event: VoiceEvent) {
        let voice = event.voice;
        let key = voices.pitch()[voice];
        match event.kind {
            VoiceEventKind::Start { start, .. } => {
                let onset = match start {
                    Start::Fresh => OscOnset::Fresh,
                    Start::Steal => OscOnset::Steal,
                    Start::Retrigger => OscOnset::Retrigger,
                    Start::Legato => OscOnset::Legato,
                    Start::Join { .. } => OscOnset::Join,
                };
                self.osc1.start(voice, key, onset);
                self.osc2.start(voice, key, onset);
                // Retrigger starts the LFO's cycle over, and so does the
                // Ratio mode, whose speed is the note's own.
                if matches!(start, Start::Fresh | Start::Steal | Start::Retrigger)
                    && (self.lfos.retrigger || self.lfos.mode == MODE_RATIO)
                {
                    self.lfo.restart(voice);
                }
                match start {
                    Start::Fresh => {
                        self.env1.trigger(voice, Trigger::New);
                        self.env2.trigger(voice, Trigger::New);
                        self.lp.reset_voice(voice, &self.filter_params);
                        self.hp.reset_voice(voice, &self.filter_params);
                    }
                    Start::Steal | Start::Retrigger => {
                        self.env1.trigger(voice, Trigger::Retrigger);
                        self.env2.trigger(voice, Trigger::Retrigger);
                    }
                    Start::Legato => {
                        self.env1.trigger(voice, Trigger::Legato);
                        self.env2.trigger(voice, Trigger::Legato);
                    }
                    Start::Join { leader } => {
                        self.env1.join(voice, leader);
                        self.env2.join(voice, leader);
                    }
                }
            }
            VoiceEventKind::Release => {
                self.env1.release(voice);
                self.env2.release(voice);
            }
            VoiceEventKind::End { .. } => {
                self.env1.reset(voice);
                self.env2.reset(voice);
            }
        }
    }

    fn render(&mut self, voices: &mut VoiceBlock, state: &State, frames: std::ops::Range<usize>) {
        let (from, to) = (frames.start, frames.end);
        let count = to - from;
        if count == 0 {
            return;
        }
        let stride = self.stride;
        let span = voices.span(16);
        if span == 0 {
            return;
        }
        // Spec: modulation R31. The controls show what the newest voice plays.
        let newest = state.newest(voices);
        self.matrix.set_newest(newest);

        // 1. The envelopes and the LFO: the modulation sources of the
        //    stretch, before the routes that read them.
        self.env1.process(&mut self.env1_out[..count * stride]);
        self.env2.process(&mut self.env2_out[..count * stride]);
        self.lfo_out[..count * stride].fill(0.0);
        if self.matrix.uses(SRC_LFO) {
            self.render_lfo(voices, count);
        }

        // 2. Each voice's low-pass cutoff per frame, in semitones: the
        //    knob's frequency, its Freq Mod and Mod routes, and the played
        //    note through Key. The high-pass takes its Mod routes and
        //    doesn't key-track.
        let freq_moved = self.matrix.has(DEST_LP_FREQ);
        let hp_moved = self.matrix.has(DEST_HP_FREQ);
        {
            let pitch = voices.pitch();
            let step = voices.pitch_step();
            for frame in 0..count {
                let plain_lp = pitch_of(self.freq[from + frame], REFERENCE);
                let plain_hp = pitch_of(self.hp_hz[from + frame], REFERENCE);
                let key = self.key[from + frame];
                for voice in 0..span {
                    // Spec: polysynth R24. Key follows the tuned pitch.
                    let note = pitch[voice] + state.tuned_by(voice) + step[voice] * frame as f32
                        - MIDDLE_C;
                    let lp = if freq_moved {
                        let base =
                            self.position_of(DEST_LP_FREQ, f64::from(self.freq[from + frame]));
                        let position = self.modulated(base, DEST_LP_FREQ, voice, frame, voices);
                        pitch_of(self.value_of(DEST_LP_FREQ, position) as f32, REFERENCE)
                    } else {
                        plain_lp
                    };
                    let hp = if hp_moved {
                        let base =
                            self.position_of(DEST_HP_FREQ, f64::from(self.hp_hz[from + frame]));
                        let position = self.modulated(base, DEST_HP_FREQ, voice, frame, voices);
                        pitch_of(self.value_of(DEST_HP_FREQ, position) as f32, REFERENCE)
                    } else {
                        plain_hp
                    };
                    self.cutoff[frame * stride + voice] = lp + key * note;
                    self.hp_cutoff[frame * stride + voice] = hp;
                }
            }
        }

        // 3. Pitch Mod, in semitones, and Shape Mod, in frames; both
        //    oscillators take Pitch and Oscillator 1 the Shape. Oscillator
        //    2's Detune is part of its own pitch.
        // Spec: polysynth R24. A tuned voice's offset from its key's pitch
        // rides on Pitch Mod, so the oscillators play the tuned pitch.
        let tuned = state.any_tuned();
        let pitch_moved = self.matrix.has(DEST_PITCH) || tuned;
        let shape_moved = self.matrix.has(DEST_OSC1_SHAPE);
        let detune_moved = self.matrix.has(DEST_OSC2_DETUNE);
        if pitch_moved {
            self.pitch_mod[..count * stride].fill(0.0);
            for frame in 0..count {
                for voice in 0..span {
                    self.pitch_mod[frame * stride + voice] =
                        (self.pitch_semitones(voice, frame, voices)
                            + f64::from(state.tuned_by(voice))) as f32;
                }
            }
        }
        if detune_moved {
            self.pitch_mod2[..count * stride].fill(0.0);
            for frame in 0..count {
                let base_value = f64::from(self.detune[from + frame]);
                for voice in 0..span {
                    let semitones = self.pitch_semitones(voice, frame, voices)
                        + f64::from(state.tuned_by(voice));
                    let base = self.position_of(DEST_OSC2_DETUNE, base_value);
                    let position = self.modulated(base, DEST_OSC2_DETUNE, voice, frame, voices);
                    let moved = self.value_of(DEST_OSC2_DETUNE, position);
                    self.pitch_mod2[frame * stride + voice] =
                        (semitones + moved - base_value) as f32;
                }
            }
        } else if pitch_moved {
            self.pitch_mod2[..count * stride].copy_from_slice(&self.pitch_mod[..count * stride]);
        }
        if shape_moved {
            self.frame_mod[..count * stride].fill(0.0);
            for frame in 0..count {
                for voice in 0..span {
                    let moved = self.matrix.add(
                        DEST_OSC1_SHAPE,
                        0.0,
                        voice,
                        frame,
                        &|source, voice, frame| self.source(source, voice, frame, voices),
                    );
                    self.frame_mod[frame * stride + voice] =
                        (moved * (SHAPE_FRAMES - 1) as f64) as f32;
                }
            }
        }

        // 4. The oscillators, band-limited, every voice in its lane.
        let inputs1 = Inputs {
            lanes: span,
            pitch: voices.pitch(),
            pitch_step: Some(voices.pitch_step()),
            pitch_mod: pitch_moved.then(|| &self.pitch_mod[..count * stride]),
            frame_mod: shape_moved.then(|| &self.frame_mod[..count * stride]),
        };
        self.osc1
            .process(count, &inputs1, &mut self.osc1_out[..count * stride]);
        let pitch_moved2 = pitch_moved || detune_moved;
        let inputs2 = Inputs {
            lanes: span,
            pitch: voices.pitch(),
            pitch_step: Some(voices.pitch_step()),
            pitch_mod: pitch_moved2.then(|| &self.pitch_mod2[..count * stride]),
            frame_mod: None,
        };
        self.osc2
            .process(count, &inputs2, &mut self.osc2_out[..count * stride]);

        // 5. The sources summed, with the first saturation point. The
        //    gains take their Mod routes, per voice.
        let pre = self.pre;
        let noise_on = self.noise_on;
        let gains_moved = self.matrix.has(DEST_OSC1_GAIN)
            || self.matrix.has(DEST_OSC2_GAIN)
            || self.matrix.has(DEST_NOISE_GAIN);
        for frame in 0..count {
            let at = from + frame;
            for voice in 0..span {
                let (g1, g2, gn) = if gains_moved {
                    (
                        self.destination(
                            DEST_OSC1_GAIN,
                            f64::from(self.g1[at]),
                            self.osc1_on,
                            voice,
                            frame,
                            voices,
                        ) as f32,
                        self.destination(
                            DEST_OSC2_GAIN,
                            f64::from(self.g2[at]),
                            self.osc2_on,
                            voice,
                            frame,
                            voices,
                        ) as f32,
                        self.destination(
                            DEST_NOISE_GAIN,
                            f64::from(self.gn[at]),
                            self.noise_on,
                            voice,
                            frame,
                            voices,
                        ) as f32,
                    )
                } else {
                    (
                        if self.osc1_on { self.g1[at] } else { 0.0 },
                        if self.osc2_on { self.g2[at] } else { 0.0 },
                        self.gn[at],
                    )
                };
                let a = self.osc1_out[frame * stride + voice] * g1;
                let b = self.osc2_out[frame * stride + voice] * g2;
                let mut sample = a + b;
                if noise_on && gn != 0.0 {
                    sample += self.noise(voice) * gn;
                }
                if self.filter_on {
                    sample = soft_clip(sample, pre);
                }
                self.audio[frame * stride + voice] = sample;
            }
        }

        // 6. The filters and the second saturation point. Resonance is
        //    set per voice for the stretch when a route reaches it, which
        //    the filter then glides to, as it does for automation.
        if self.filter_on {
            if self.matrix.has(DEST_LP_RES) {
                for voice in 0..span {
                    let value = self.destination(
                        DEST_LP_RES,
                        f64::from(self.res[from]),
                        true,
                        voice,
                        0,
                        voices,
                    );
                    let params = VoiceParams {
                        resonance: (value / 100.0).clamp(0.0, 1.0) as f32,
                        ..self.filter_params
                    };
                    self.lp.set_params(voice, &params);
                    self.hp.set_params(voice, &params);
                }
                self.filter_resonance_moved = true;
            }
            self.lp.process_voices(
                span,
                &self.cutoff[..count * stride],
                &mut self.audio[..count * stride],
            );
            self.hp.process_voices(
                span,
                &self.hp_cutoff[..count * stride],
                &mut self.audio[..count * stride],
            );
            let post = self.post;
            if post > 0.0 {
                for sample in &mut self.audio[..count * stride] {
                    *sample = soft_clip(*sample, post);
                }
            }
        }

        // 7. The voices mixed to one mono signal, under Envelope 1, the
        //    velocity, and the Main Volume and its Mod routes.
        let volume_moved = self.matrix.has(DEST_VOLUME);
        let mut mask = voices.active_mask();
        while mask != 0 {
            let voice = mask.trailing_zeros() as usize;
            mask &= mask - 1;
            let velocity = voices.velocity()[voice];
            for frame in 0..count {
                let level = self.env1_out[frame * stride + voice];
                if level == 0.0 {
                    continue;
                }
                // Spec: polysynth R15. The knob is in per cent.
                let vel_vol = self.vel_vol[from + frame] / 100.0;
                let scale = 1.0 - vel_vol + vel_vol * velocity;
                let volume = if volume_moved {
                    self.destination(
                        DEST_VOLUME,
                        f64::from(self.volume[from + frame]),
                        true,
                        voice,
                        frame,
                        voices,
                    ) as f32
                } else {
                    self.volume[from + frame]
                };
                let mixed = self.audio[frame * stride + voice] * level * scale * volume;
                self.out[from + frame] += mixed;
            }
        }

        // Spec: modulation R31. The controls of Shape and LFO Rate show the
        // newest voice's movement at the stretch's last frame, as the others
        // do (the rest are shown as they are worked out).
        if let Some(voice) = newest {
            let last = count - 1;
            if self.matrix.has(DEST_OSC1_SHAPE) {
                let moved = self.matrix.add(
                    DEST_OSC1_SHAPE,
                    self.shape,
                    voice,
                    last,
                    &|source, voice, frame| self.source(source, voice, frame, voices),
                );
                self.matrix.show(DEST_OSC1_SHAPE, moved.clamp(0.0, 1.0));
            }
            if self.matrix.has(DEST_LFO_RATE) && self.lfos.mode == MODE_RATE {
                let hz = self.lfo_rate(voice, last, voices);
                self.matrix.show(
                    DEST_LFO_RATE,
                    self.position_of(DEST_LFO_RATE, hz).clamp(0.0, 1.0),
                );
            }
        }

        // 8. A voice whose amplifier envelope has ended is silent.
        let mut mask = voices.active_mask();
        while mask != 0 {
            let voice = mask.trailing_zeros() as usize;
            mask &= mask - 1;
            if self.env1.is_idle(voice) {
                voices.end(voice);
            }
        }
    }
}

impl Synth {
    /// Voice `voice`'s Pitch Mod value at `frame`, in semitones.
    fn pitch_semitones(&self, voice: usize, frame: usize, voices: &VoiceBlock) -> f64 {
        let semitones = self
            .matrix
            .add(DEST_PITCH, 0.0, voice, frame, &|source, voice, frame| {
                self.source(source, voice, frame, voices)
            });
        semitones * PITCH_MOD_SEMITONES
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Spec: polysynth R19 (the LFO's waveforms)
    #[test]
    fn every_lfo_wave_stays_inside_its_range() {
        for shape in 0..LFO_SHAPES.len() {
            for step in 0..=64 {
                let phase = f64::from(step) / 64.0;
                let value = lfo_wave(shape, phase, 0.5, -0.5);
                assert!((-1.0..=1.0).contains(&value), "{shape} at {phase}: {value}");
            }
        }
        assert!(
            (lfo_wave(LFO_SINE, 0.25, 0.0, 0.0) - 1.0).abs() < 1e-9,
            "the sine"
        );
        assert!(
            (lfo_wave(LFO_SINE, 0.75, 0.0, 0.0) + 1.0).abs() < 1e-9,
            "the sine"
        );
        assert!(
            (lfo_wave(LFO_TRIANGLE, 0.5, 0.0, 0.0) - 1.0).abs() < 1e-9,
            "the triangle"
        );
        assert!(
            (lfo_wave(LFO_SAW_UP, 0.0, 0.0, 0.0) + 1.0).abs() < 1e-9,
            "up"
        );
        assert!(
            (lfo_wave(LFO_SAW_DOWN, 1.0, 0.0, 0.0) + 1.0).abs() < 1e-9,
            "down"
        );
        assert!(
            (lfo_wave(LFO_SQUARE, 0.6, 0.0, 0.0) + 1.0).abs() < 1e-9,
            "the square"
        );
        // Sample and hold holds; wander moves between the held pair.
        assert!((lfo_wave(LFO_SAMPLE_HOLD, 0.9, 0.25, -0.75) - 0.25).abs() < 1e-9);
        assert!((lfo_wave(LFO_WANDER, 0.0, 0.25, -0.75) - 0.25).abs() < 1e-9);
        assert!((lfo_wave(LFO_WANDER, 0.5, 0.25, -0.75) + 0.25).abs() < 1e-9);
        // The envelope waveforms decay over the cycle.
        assert!(
            (lfo_wave(LFO_LINEAR, 0.0, 0.0, 0.0) - 1.0).abs() < 1e-9,
            "linear"
        );
        assert!(lfo_wave(LFO_LINEAR, 0.5, 0.0, 0.0) < 0.5 + 1e-9, "linear");
        assert!(
            lfo_wave(LFO_EXPONENTIAL, 0.5, 0.0, 0.0) < lfo_wave(LFO_LINEAR, 0.5, 0.0, 0.0),
            "the exponential decays faster"
        );
    }

    // Spec: polysynth R20 (the routes); instrument modulation R2, R17.
    #[test]
    fn a_route_reads_its_choosers_and_its_amount() {
        let description = polysynth_type().description;
        let config = NodeConfig {
            sample_rate: bl_core::time::SampleRate::HZ_48000,
            max_frames: 64,
            channels: 2,
            round_trip_latency: 0,
        };
        let layout = polysynth_matrix();
        let routes = |params: &[(ParamId, f64)]| {
            let mut device =
                polysynth_type().device(bl_model::DeviceId(bl_core::id::Id::new(1).unwrap()));
            for &(id, value) in params {
                device.params.insert(id, value);
            }
            let bank = ParamBank::new(&description, &device, &config);
            let mut matrix = Matrix::new(std::sync::Arc::new(layout.clone()));
            matrix.begin(&bank);
            matrix
        };
        // The choosers count Off as 0, so the first source is 1.
        let (lfo, freq) = ((SRC_LFO + 1) as f64, (DEST_LP_FREQ + 1) as f64);
        // Off on either side, or a zero amount, leaves no route.
        for params in [
            [(MOD1_SOURCE, 0.0), (MOD1_DEST, freq), (MOD1_AMOUNT, 50.0)],
            [(MOD1_SOURCE, lfo), (MOD1_DEST, 0.0), (MOD1_AMOUNT, 50.0)],
            [(MOD1_SOURCE, lfo), (MOD1_DEST, freq), (MOD1_AMOUNT, 0.0)],
        ] {
            let matrix = routes(&params);
            assert!(
                !matrix.has(DEST_LP_FREQ) && !matrix.uses(SRC_LFO),
                "{params:?}"
            );
        }
        let one = routes(&[(MOD1_SOURCE, lfo), (MOD1_DEST, freq), (MOD1_AMOUNT, 50.0)]);
        // The amount is a fraction of the travel: 50 % of a source at 1.
        let read = |_: usize, _: usize, _: usize| 1.0;
        assert!((one.add(DEST_LP_FREQ, 0.0, 0, 0, &read) - 0.5).abs() < 1e-9);
        // A dedicated slot's destination is its own.
        let shape = routes(&[
            (SHAPE_MOD_SOURCE, (SRC_ENV1 + 1) as f64),
            (SHAPE_MOD_AMOUNT, -25.0),
        ]);
        assert!(shape.has(DEST_OSC1_SHAPE) && !shape.has(DEST_LP_FREQ));
        assert!((shape.add(DEST_OSC1_SHAPE, 0.0, 0, 0, &read) + 0.25).abs() < 1e-9);
        // And the routes are found by destination, the LFO's own included.
        let all = routes(&[
            (MOD1_SOURCE, lfo),
            (MOD1_DEST, freq),
            (MOD1_AMOUNT, 50.0),
            (SHAPE_MOD_SOURCE, (SRC_ENV1 + 1) as f64),
            (SHAPE_MOD_AMOUNT, -25.0),
            (LFO_MOD_SOURCE, (SRC_ENV2 + 1) as f64),
            (LFO_MOD_AMOUNT, 100.0),
        ]);
        assert!(all.has(DEST_LP_FREQ) && all.has(DEST_OSC1_SHAPE) && all.has(DEST_LFO_RATE));
        assert!(all.uses(SRC_LFO));
        assert!(!routes(&[]).has(DEST_LP_FREQ) && !routes(&[]).uses(SRC_LFO));
    }
}

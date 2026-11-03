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
//! arrived with the work package's second part. WP-8.7 completed it: the
//! voice modes (Poly, Mono, Stereo and Unison stack up to four copies of
//! a note, each in a lane of its own), the Drift control's slow random
//! walk, Legato and Glide, Envelope 2's cycling mode with its rate as a
//! Mod destination, and Hi-Quality.

use core::f64::consts::TAU;

use bl_dsp::random::Random;
use bl_engine::{DeviceType, NodeConfig, NodeData, NodeEvent, Registry};
use bl_model::device_layout::{BODY_HEIGHT, ControlBand, DisplayCell, LayoutNode, Widget};
use bl_model::param::gain_to_db;
use bl_model::{
    Device, DeviceDescription, DeviceLayout, DeviceRole, ParamDescriptor, ParamId, ParamUnit,
};
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
/// The voice mode: Poly, Mono, Stereo or Unison.
pub const VOICE_MODE: ParamId = ParamId(62);
/// Mono's extra copies' level, in per cent.
pub const THICKNESS: ParamId = ParamId(63);
/// Stereo's width, in per cent.
pub const SPREAD: ParamId = ParamId(64);
/// Unison's detune, in per cent.
pub const STRENGTH: ParamId = ParamId(65);
/// How far each voice's pitch and cutoff wander, in per cent.
pub const DRIFT: ParamId = ParamId(66);
/// Legato in Mono.
pub const LEGATO: ParamId = ParamId(67);
/// Glide Time.
pub const GLIDE: ParamId = ParamId(68);
/// Envelope 2's mode: ADSR or Cycling.
pub const ENV2_MODE: ParamId = ParamId(69);
/// The cycling envelope's tilt, in per cent.
pub const CYC_TILT: ParamId = ParamId(70);
/// How long the cycling envelope stays at its top, in per cent of the cycle.
pub const CYC_HOLD: ParamId = ParamId(71);
/// The cycling envelope's time mode: Rate, Ratio, Time, or Sync.
pub const CYC_MODE: ParamId = ParamId(72);
/// The cycling envelope's rate in hertz, in the Rate time mode: the Cyc Env
/// Rate destination.
pub const CYC_RATE: ParamId = ParamId(73);
/// The cycling envelope's rate as a ratio of the played note's frequency.
pub const CYC_RATIO: ParamId = ParamId(74);
/// The cycling envelope's cycle in milliseconds.
pub const CYC_TIME: ParamId = ParamId(75);
/// The cycling envelope's division of the song's tempo.
pub const CYC_SYNC: ParamId = ParamId(76);
/// Hi-Quality, a menu option.
pub const HI_QUALITY: ParamId = ParamId(77);

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
/// The Mod section's destinations, in the manual's order, after Off; Cyc Env
/// Rate is last so that a saved choice keeps its meaning (R32).
pub const MOD_DESTINATIONS: [&str; 12] = [
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
    "Cyc Env Rate",
];
/// Envelope 2's modes.
pub const ENV2_MODES: [&str; 2] = ["ADSR", "Cycling"];
/// The voice modes by index, as [`instrument::voice_modes`] lists them.
const MODE_MONO: usize = 1;
const MODE_STEREO: usize = 2;
const MODE_UNISON: usize = 3;

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

/// The most copies a note plays, each in a lane of its own, and the lanes
/// the oscillators hold.
const COPIES: usize = 4;
const LANES: usize = MAX_VOICES * COPIES;
/// Spec: polysynth R27, R28. Mono's copies' detune in cents, Stereo's at
/// 100 % Spread, Unison's at 100 % Strength, and the random offset each
/// Unison copy adds at 100 % Strength: Beatlayer's own.
const MONO_CENTS: [f32; COPIES] = [0.0, -10.0, 5.0, 10.0];
const STEREO_CENTS: f32 = 10.0;
const UNISON_CENTS: f32 = 25.0;
const UNISON_RANDOM_CENTS: f32 = 5.0;
/// Spec: polysynth R29. How far a full Drift moves a voice's pitch (cents)
/// and low-pass cutoff (semitones), and how often its walk picks a new
/// target: Beatlayer's own.
const DRIFT_CENTS: f64 = 25.0;
const DRIFT_SEMITONES: f64 = 2.0;
const DRIFT_HZ: f64 = 0.5;
/// Spec: polysynth R31. How close to either end Tilt bends the slope beside
/// it from straight towards exponential: a tenth of its travel.
const TILT_BEND: f64 = 0.1;

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
const DEST_CYC_RATE: usize = 10;
/// Pitch is no Mod destination: the Pitch Mod slots carry it, and no chooser
/// reaches it.
const DEST_PITCH: usize = 11;
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
        instrument::voice_modes(VOICE_MODE),
        ParamDescriptor::continuous(THICKNESS, "Thickness", (0.0, 100.0), 0.0, ParamUnit::Percent)
            .describe("In Mono, the level of the three extra copies; at 0 % only one sounds."),
        ParamDescriptor::continuous(SPREAD, "Spread", (0.0, 100.0), 50.0, ParamUnit::Percent)
            .describe("In Stereo, how far the two copies detune and pan apart."),
        ParamDescriptor::continuous(STRENGTH, "Strength", (0.0, 100.0), 50.0, ParamUnit::Percent)
            .describe("In Unison, how far the four copies detune."),
        ParamDescriptor::continuous(DRIFT, "Drift", (0.0, 100.0), 0.0, ParamUnit::Percent)
            .describe("How far each voice's pitch and filter frequency wander at random."),
        instrument::legato(LEGATO, false),
        instrument::glide_time(GLIDE, 5_000.0, 0.0),
        ParamDescriptor::choice(ENV2_MODE, "Env 2 Mode", &ENV2_MODES, 0)
            .describe("Envelope 2 as an ADSR, or as a cycle that restarts with each note."),
        ParamDescriptor::continuous(CYC_TILT, "Cyc Env Tilt", (0.0, 100.0), 50.0, ParamUnit::Percent)
            .describe("Moves the cycling envelope's top from the start of its cycle to its end."),
        ParamDescriptor::continuous(CYC_HOLD, "Cyc Env Hold", (0.0, 100.0), 0.0, ParamUnit::Percent)
            .describe("How long the cycling envelope stays at its top, in per cent of its cycle."),
        ParamDescriptor::choice(CYC_MODE, "Cyc Env Mode", &LFO_MODES, MODE_RATE)
            .describe("Sets the cycling envelope's rate in hertz, as a ratio of the note, in milliseconds, or in beat divisions."),
        ParamDescriptor::continuous(CYC_RATE, "Cyc Env Rate", (0.01, 100.0), 1.0, ParamUnit::Hertz)
            .logarithmic()
            .describe("The cycling envelope's rate in hertz, in the Rate time mode."),
        ParamDescriptor::continuous(CYC_RATIO, "Cyc Env Ratio", (0.0625, 64.0), 1.0, ParamUnit::Ratio)
            .logarithmic()
            .describe("The cycling envelope's speed as a ratio of the played note's frequency, in the Ratio time mode."),
        ParamDescriptor::continuous(
            CYC_TIME,
            "Cyc Env Time",
            (1.0, 20_000.0),
            500.0,
            ParamUnit::Milliseconds,
        )
        .logarithmic()
        .describe("The cycling envelope's cycle in milliseconds, in the Time time mode."),
        ParamDescriptor::choice(CYC_SYNC, "Cyc Env Sync", &LFO_DIVISIONS, 2)
            .describe("The cycling envelope's cycle as a division of the song's tempo, in the Sync time mode."),
        instrument::hi_quality(HI_QUALITY),
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
                (ENV2_MODE, Widget::Chooser),
                (CYC_TILT, Widget::Knob),
                (CYC_HOLD, Widget::Knob),
                (CYC_MODE, Widget::Chooser),
                (CYC_SYNC, Widget::Chooser),
                (CYC_RATE, Widget::Knob),
                (CYC_RATIO, Widget::Knob),
                (CYC_TIME, Widget::Knob),
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
                (VOICE_MODE, Widget::Chooser),
                (THICKNESS, Widget::Knob),
                (SPREAD, Widget::Knob),
                (STRENGTH, Widget::Knob),
                (DRIFT, Widget::Knob),
                (LEGATO, Widget::Toggle),
                (GLIDE, Widget::Knob),
            ],
        )),
    ]));
    DeviceType::native(POLYSYNTH, "Drift", DeviceRole::Instrument)
        .with_description(
            DeviceDescription::new(params)
                .with_layout(layout)
                .with_menu_options(&[HI_QUALITY])
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
                        "Cycling",
                        &[
                            "Env 2 Mode",
                            "Cyc Env Tilt",
                            "Cyc Env Hold",
                            "Cyc Env Mode",
                            "Cyc Env Rate",
                            "Cyc Env Ratio",
                            "Cyc Env Time",
                            "Cyc Env Sync",
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
                    (
                        "Voice Mode",
                        &[
                            "Voice Mode",
                            "Thickness",
                            "Spread",
                            "Strength",
                            "Drift",
                            "Legato",
                            "Glide Time",
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
/// have always been summed in. Every destination is worked out every frame,
/// per voice and additive, with or without Hi-Quality (R20 asks for a grid
/// only of control-rate routes, and Drift has none).
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
            dest(DEST_CYC_RATE, CYC_RATE),
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
    add(registry, polysynth_type(), make);
}

/// Makes a Drift for `config`'s rate, channels, and block size. Every
/// buffer and table it needs is made here, off the audio thread.
#[must_use]
pub fn make(device: &Device, config: &NodeConfig) -> PolySynth {
    PolySynth::new(device, config)
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

/// Spec: polysynth R31. Envelope 2's cycling shape at `phase` (0 to 1) of
/// its cycle, with Tilt and Hold as fractions: it rises to 1 over Tilt's share
/// of the cycle left after Hold, stays at 1 for Hold's share, and falls to 0
/// over the rest. Within a tenth of Tilt's either end the slope beside the
/// top bends from straight towards exponential; the curve is Beatlayer's own.
fn cycle_wave(phase: f64, tilt: f64, hold: f64) -> f64 {
    let rise = tilt * (1.0 - hold);
    let top = rise + hold;
    let floor = (-EXPONENTIAL_DECAY).exp();
    // The exponential decay from 1 at the start of a slope to 0 at its end.
    let decay = |x: f64| ((-EXPONENTIAL_DECAY * x).exp() - floor) / (1.0 - floor);
    if phase < rise {
        let x = phase / rise;
        let bend = ((tilt - (1.0 - TILT_BEND)) / TILT_BEND).clamp(0.0, 1.0);
        x + bend * (1.0 - decay(x) - x)
    } else if phase < top {
        1.0
    } else {
        let x = (phase - top) / (1.0 - top);
        let bend = ((TILT_BEND - tilt) / TILT_BEND).clamp(0.0, 1.0);
        1.0 - x + bend * (decay(x) - (1.0 - x))
    }
}

/// A time mode's settings, as the LFO and the cycling envelope read them
/// (R19, R31).
#[derive(Clone, Copy)]
struct TimeMode {
    /// One of the `MODE_*` values.
    mode: usize,
    /// Its speed in cycles a second, in the Rate, Time and Sync modes.
    hz: f64,
    /// Its speed as a ratio of the note's frequency, in the Ratio mode.
    ratio: f64,
}

impl TimeMode {
    /// Reads the mode, Rate, Ratio, Time and Sync parameters `ids` hold,
    /// the Sync mode's cycle coming from the tempo `bpm`.
    fn read(
        params: &ParamBank,
        at: impl Fn(ParamId) -> usize,
        rate: f64,
        bpm: f64,
        ids: [ParamId; 5],
    ) -> Self {
        let [mode, hz, ratio, time, sync] = ids;
        let mode = (params.value(at(mode)) as usize).min(LFO_MODES.len() - 1);
        let division = (params.value(at(sync)) as usize).min(LFO_DIVISIONS.len() - 1);
        let sixteenth = rate * 60.0 / bpm / 4.0;
        Self {
            mode,
            hz: match mode {
                MODE_RATE => params.value(at(hz)).max(0.0),
                MODE_TIME => 1_000.0 / params.value(at(time)).max(0.001),
                MODE_SYNC => rate / (DIVISION_SIXTEENTHS[division] * sixteenth).max(1.0),
                _ => params.value(at(ratio)),
            },
            ratio: params.value(at(ratio)).max(0.0),
        }
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
    /// Whether a note restarts the LFO.
    retrigger: bool,
    /// Its overall intensity, 0 to 1.
    amount: f64,
    /// Its time mode and speed.
    time: TimeMode,
}

/// Envelope 2's cycling settings for a stretch.
#[derive(Clone, Copy)]
struct Cycle {
    /// Whether Envelope 2 cycles rather than being an ADSR.
    on: bool,
    /// Tilt and Hold as fractions.
    tilt: f64,
    hold: f64,
    time: TimeMode,
}

/// Spec: polysynth R27, R28. What a stack of copies plays at a frame: each
/// copy's detune in cents and its gain into the left and right channels.
#[derive(Clone, Copy)]
struct CopyPlan {
    cents: [f32; COPIES],
    left: [f32; COPIES],
    right: [f32; COPIES],
}

/// Spec: polysynth R29. A slow random walk from −1 to 1: it moves from one
/// target to the next, smoothly, drawing a new one at `DRIFT_HZ`.
struct Walk {
    random: Random,
    phase: f64,
    from: f64,
    to: f64,
}

impl Walk {
    fn new(seed: u64) -> Self {
        let mut random = Random::seeded(seed);
        let (from, to) = (random.next_bipolar(), random.next_bipolar());
        Self {
            random,
            phase: 0.0,
            from,
            to,
        }
    }

    /// The next frame's value, at `rate` frames a second.
    fn next(&mut self, rate: f64) -> f64 {
        self.phase += DRIFT_HZ / rate;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
            self.from = self.to;
            self.to = self.random.next_bipolar();
        }
        let x = self.phase;
        self.from + (self.to - self.from) * x * x * (3.0 - 2.0 * x)
    }
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
    fn new(device: &Device, config: &NodeConfig) -> Self {
        let rate = config.sample_rate.as_f64();
        Self {
            // Spec: instrument voices R22, polysynth R26 to R30, R33. Drift's
            // voices are the voice section's, with the parts Drift has.
            section: Section::new(
                Parts {
                    voices: Some(VOICES),
                    voice_mode: Some(VOICE_MODE),
                    copy_modes: true,
                    legato: Some(LEGATO),
                    glide_time: Some(GLIDE),
                    glide_only_held: true,
                    poly_glide: true,
                    transpose: Some(TRANSPOSE),
                    pb_range: Some(PB_RANGE),
                    note_pb: Some(NOTE_PB),
                    note_range: NoteRange::PbRange,
                    hi_quality: Some(HI_QUALITY),
                    ..Parts::default()
                },
                MAX_VOICES,
                COPIES,
                rate,
                0,
            ),
            synth: Synth::new(rate, config.max_frames, u64::from(device.id.0)),
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
        // Stereo's two copies sit left and right; the other modes are mono.
        let stereo = self.synth.mode == MODE_STEREO;
        for channel in 0..audio.channels() {
            let out = if stereo && channel % 2 == 1 {
                &self.synth.out_r
            } else {
                &self.synth.out
            };
            audio.channel_mut(channel).copy_from_slice(&out[..frames]);
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
    /// The filters, left and then right: only Stereo runs the second.
    lp: [FilterBank; 2],
    hp: [FilterBank; 2],
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
    thickness: Vec<f32>,
    spread: Vec<f32>,
    strength: Vec<f32>,
    drift: Vec<f32>,
    /// The stretch's per-voice buffers, `frames × stride` each.
    audio: Vec<f32>,
    audio_r: Vec<f32>,
    cutoff: Vec<f32>,
    hp_cutoff: Vec<f32>,
    env1_out: Vec<f32>,
    env2_out: Vec<f32>,
    lfo_out: Vec<f32>,
    /// What each voice's pitch and cutoff drift by this stretch, in
    /// semitones.
    drift_pitch: Vec<f32>,
    drift_cutoff: Vec<f32>,
    /// The stretch's per-lane buffers, `frames × LANES` each: what the
    /// oscillators render and read.
    osc1_out: Vec<f32>,
    osc2_out: Vec<f32>,
    pitch_mod: Vec<f32>,
    pitch_mod2: Vec<f32>,
    frame_mod: Vec<f32>,
    /// The stretch's mixed output, one value a frame, left and right.
    out: Vec<f32>,
    out_r: Vec<f32>,
    /// The voice mode of the stretch, and each voice's drift walks (pitch,
    /// then cutoff), Unison's random offsets (each lane's, from −1 to 1),
    /// and Envelope 2's cycle phase.
    mode: usize,
    walks: Vec<[Walk; 2]>,
    unison_random: [f32; LANES],
    unison: Random,
    cycle: Cycle,
    cycle_phase: [f64; MAX_VOICES],
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
    fn new(rate: f64, max_frames: usize, seed: u64) -> Self {
        let max_frames = max_frames.max(1);
        let layout = OscLayout {
            voices: MAX_VOICES,
            lanes_per_voice: COPIES,
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
        let lp = [(); 2].map(|()| {
            let mut bank = FilterBank::new(rate as f32, MAX_VOICES, 1);
            bank.set_type(FilterType::Lp12);
            bank
        });
        let hp = [(); 2].map(|()| {
            let mut bank = FilterBank::new(rate as f32, MAX_VOICES, 1);
            bank.set_type(FilterType::Hp12);
            bank
        });
        let stride = MAX_VOICES.next_multiple_of(16);
        let buffer = || vec![0.0; max_frames * stride];
        let lane_buffer = || vec![0.0; max_frames * LANES];
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
            destination(CYC_RATE),
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
            thickness: ramp(),
            spread: ramp(),
            strength: ramp(),
            drift: ramp(),
            audio: buffer(),
            audio_r: buffer(),
            cutoff: buffer(),
            hp_cutoff: buffer(),
            env1_out: buffer(),
            env2_out: buffer(),
            lfo_out: buffer(),
            drift_pitch: buffer(),
            drift_cutoff: buffer(),
            osc1_out: lane_buffer(),
            osc2_out: lane_buffer(),
            pitch_mod: lane_buffer(),
            pitch_mod2: lane_buffer(),
            frame_mod: lane_buffer(),
            out: vec![0.0; max_frames],
            out_r: vec![0.0; max_frames],
            mode: 0,
            // Spec: polysynth R29. The device's own seed and the voice's
            // number.
            walks: (0..MAX_VOICES as u64)
                .map(|voice| {
                    [
                        Walk::new(seed.wrapping_mul(64).wrapping_add(2 * voice)),
                        Walk::new(seed.wrapping_mul(64).wrapping_add(2 * voice + 1)),
                    ]
                })
                .collect(),
            unison_random: [0.0; LANES],
            unison: Random::seeded(seed ^ 0x0C45_CADE_0000_0000),
            cycle: Cycle {
                on: false,
                tilt: 0.5,
                hold: 0.0,
                time: TimeMode {
                    mode: MODE_RATE,
                    hz: 1.0,
                    ratio: 1.0,
                },
            },
            cycle_phase: [0.0; MAX_VOICES],
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
                retrigger: true,
                amount: 1.0,
                time: TimeMode {
                    mode: MODE_SYNC,
                    hz: 1.0,
                    ratio: 1.0,
                },
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
        ramp_into(&mut self.thickness[..frames], params, THICKNESS);
        ramp_into(&mut self.spread[..frames], params, SPREAD);
        ramp_into(&mut self.strength[..frames], params, STRENGTH);
        ramp_into(&mut self.drift[..frames], params, DRIFT);
        self.out[..frames].fill(0.0);
        self.out_r[..frames].fill(0.0);
        self.mode = (params.value(at(VOICE_MODE)) as usize).min(instrument::MODE_COPIES.len() - 1);

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
            for lp in &mut self.lp {
                lp.set_type(if filter_type == 0 {
                    FilterType::Lp12
                } else {
                    FilterType::Lp24
                });
            }
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
                self.set_filter_params(voice, &filter_params);
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

        // The Sync mode's cycle comes from the tempo map's tempo at the
        // stretch, as the delay device's does.
        let bpm = audio.bpm();
        self.lfos = Lfos {
            shape: (params.value(at(LFO_SHAPE)) as usize).min(LFO_SHAPES.len() - 1),
            retrigger: params.value(at(LFO_RETRIGGER)) >= 0.5,
            amount: (params.value(at(LFO_AMOUNT)) / 100.0).clamp(0.0, 1.0),
            time: TimeMode::read(
                params,
                at,
                self.rate,
                bpm,
                [LFO_MODE, LFO_RATE, LFO_RATIO, LFO_TIME, LFO_SYNC],
            ),
        };
        self.cycle = Cycle {
            on: params.value(at(ENV2_MODE)) >= 0.5,
            tilt: (params.value(at(CYC_TILT)) / 100.0).clamp(0.0, 1.0),
            hold: (params.value(at(CYC_HOLD)) / 100.0).clamp(0.0, 1.0),
            time: TimeMode::read(
                params,
                at,
                self.rate,
                bpm,
                [CYC_MODE, CYC_RATE, CYC_RATIO, CYC_TIME, CYC_SYNC],
            ),
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
        for bank in self.lp.iter_mut().chain(&mut self.hp) {
            bank.reset();
        }
        self.out.fill(0.0);
        self.out_r.fill(0.0);
    }

    /// Gives voice `voice`'s filters, in both channels, `params`.
    fn set_filter_params(&mut self, voice: usize, params: &VoiceParams) {
        for bank in self.lp.iter_mut().chain(&mut self.hp) {
            bank.set_params(voice, params);
        }
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

    /// The speed in cycles a second of the LFO (`dest` is its rate) or of
    /// Envelope 2's cycle (Cyc Env Rate), with time mode `time`, for `voice`
    /// at `frame`, after every route into it: a full route is ±4 octaves,
    /// Beatlayer's own reading. In the Ratio mode the speed follows the
    /// note, and elsewhere it is the mode's own rate.
    fn speed(
        &self,
        dest: usize,
        time: &TimeMode,
        voice: usize,
        frame: usize,
        voices: &VoiceBlock,
    ) -> f64 {
        let delta = self
            .matrix
            .add(dest, 0.0, voice, frame, &|source, voice, frame| {
                self.source(source, voice, frame, voices)
            });
        let base = if time.mode == MODE_RATIO {
            let lane = voice * COPIES;
            let note = f64::from(voices.pitch()[lane] + voices.pitch_step()[lane] * frame as f32);
            time.ratio * 440.0 * ((note - 69.0) / 12.0).exp2()
        } else {
            time.hz
        };
        base * (delta * LFO_RATE_OCTAVES).exp2()
    }

    fn lfo_rate(&self, voice: usize, frame: usize, voices: &VoiceBlock) -> f64 {
        self.speed(DEST_LFO_RATE, &self.lfos.time, voice, frame, voices)
    }

    /// Spec: polysynth R31, R32. Renders Envelope 2's cycle for every active
    /// voice, frame by frame, into `env2_out`: from 0 at the voice's note,
    /// at its rate and the Cyc Env Rate routes'.
    fn render_cycle(&mut self, voices: &VoiceBlock, count: usize) {
        let stride = self.stride;
        let cycle = self.cycle;
        let mut active = voices.active_mask();
        self.env2_out[..count * stride].fill(0.0);
        while active != 0 {
            let voice = active.trailing_zeros() as usize;
            active &= active - 1;
            for frame in 0..count {
                let phase = self.cycle_phase[voice];
                self.env2_out[frame * stride + voice] =
                    cycle_wave(phase, cycle.tilt, cycle.hold) as f32;
                let cycles =
                    self.speed(DEST_CYC_RATE, &cycle.time, voice, frame, voices) / self.rate;
                self.cycle_phase[voice] = (phase + cycles).fract();
            }
        }
    }

    /// Renders the LFO's value for every active voice, frame by frame,
    /// into `lfo_out`: each voice's own phase when the LFO retriggers, or
    /// in the Ratio mode whose speed follows the note, and one free-running
    /// phase, which every voice reads, otherwise.
    fn render_lfo(&mut self, voices: &VoiceBlock, count: usize) {
        let stride = self.stride;
        let lfos = self.lfos;
        let shared = !lfos.retrigger && lfos.time.mode != MODE_RATIO;
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
        let key = voices.pitch()[voice * COPIES];
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
                if matches!(start, Start::Fresh | Start::Steal | Start::Retrigger) {
                    // Spec: polysynth R28, R31. The cycle starts at 0 with
                    // the note, and Unison draws each copy's offset.
                    self.cycle_phase[voice] = 0.0;
                    for random in &mut self.unison_random[voice * COPIES..(voice + 1) * COPIES] {
                        *random = self.unison.next_bipolar() as f32;
                    }
                    // Retrigger starts the LFO's cycle over, and so does the
                    // Ratio mode, whose speed is the note's own.
                    if self.lfos.retrigger || self.lfos.time.mode == MODE_RATIO {
                        self.lfo.restart(voice);
                    }
                }
                match start {
                    Start::Fresh => {
                        self.env1.trigger(voice, Trigger::New);
                        self.env2.trigger(voice, Trigger::New);
                        for bank in self.lp.iter_mut().chain(&mut self.hp) {
                            bank.reset_voice(voice, &self.filter_params);
                        }
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
        let lane_span = voices.span(16);
        if lane_span == 0 {
            return;
        }
        // The voices those lanes belong to, in the filters' sixteens, and
        // how many copies each plays.
        let span = (lane_span / COPIES).next_multiple_of(16).min(MAX_VOICES);
        let mode = self.mode;
        let copies = instrument::MODE_COPIES[mode];
        let stereo = mode == MODE_STEREO;
        // Spec: modulation R31. The controls show what the newest voice plays.
        let newest = state.newest(voices);
        self.matrix.set_newest(newest);

        // 1. The envelopes and the LFO: the modulation sources of the
        //    stretch, before the routes that read them; Envelope 2 is the
        //    cycle where it cycles.
        self.env1.process(&mut self.env1_out[..count * stride]);
        self.env2.process(&mut self.env2_out[..count * stride]);
        self.lfo_out[..count * stride].fill(0.0);
        if self.matrix.uses(SRC_LFO) {
            self.render_lfo(voices, count);
        }
        if self.cycle.on && self.matrix.uses(SRC_ENV2) {
            self.render_cycle(voices, count);
        }

        // Spec: polysynth R29. Each sounding voice's drift walks, moved a
        // frame at a time whether or not Drift is up, so that a render
        // doesn't depend on where its blocks fall.
        let drift_on = self.drift[from..to].iter().any(|&drift| drift > 0.0);
        let mut active = voices.active_mask();
        while active != 0 {
            let voice = active.trailing_zeros() as usize;
            active &= active - 1;
            let [pitch, cutoff] = &mut self.walks[voice];
            for frame in 0..count {
                let amount = f64::from(self.drift[from + frame]) / 100.0;
                self.drift_pitch[frame * stride + voice] =
                    (pitch.next(self.rate) * amount * DRIFT_CENTS / 100.0) as f32;
                self.drift_cutoff[frame * stride + voice] =
                    (cutoff.next(self.rate) * amount * DRIFT_SEMITONES) as f32;
            }
        }

        // 2. Each voice's low-pass cutoff per frame, in semitones: the
        //    knob's frequency, its Freq Mod and Mod routes, the played
        //    note through Key, and the drift. The high-pass takes its Mod
        //    routes and doesn't key-track.
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
                    let lane = voice * COPIES;
                    // Spec: polysynth R24. Key follows the tuned pitch.
                    let note =
                        pitch[lane] + state.tuned_by(voice) + step[lane] * frame as f32 - MIDDLE_C;
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
                    let mut cutoff = lp + key * note;
                    if drift_on {
                        cutoff += self.drift_cutoff[frame * stride + voice];
                    }
                    self.cutoff[frame * stride + voice] = cutoff;
                    self.hp_cutoff[frame * stride + voice] = hp;
                }
            }
        }

        // 3. Pitch Mod, in semitones, and Shape Mod, in frames, for every
        //    lane; both oscillators take Pitch and Oscillator 1 the Shape.
        //    Oscillator 2's Detune is part of its own pitch. A copy's detune
        //    and the voice's drift ride on Pitch Mod too.
        // Spec: polysynth R24. A tuned voice's offset from its key's pitch
        // rides on Pitch Mod, so the oscillators play the tuned pitch.
        let tuned = state.any_tuned();
        let pitch_moved = self.matrix.has(DEST_PITCH) || tuned || copies > 1 || drift_on;
        let shape_moved = self.matrix.has(DEST_OSC1_SHAPE);
        let detune_moved = self.matrix.has(DEST_OSC2_DETUNE);
        if pitch_moved || detune_moved {
            for frame in 0..count {
                let plan = self.plan(from + frame);
                let unison = if mode == MODE_UNISON {
                    self.strength[from + frame] / 100.0 * UNISON_RANDOM_CENTS / 100.0
                } else {
                    0.0
                };
                let base_value = f64::from(self.detune[from + frame]);
                for voice in 0..span {
                    let semitones = self.pitch_semitones(voice, frame, voices)
                        + f64::from(state.tuned_by(voice));
                    let mut first = semitones as f32;
                    let mut second = if detune_moved {
                        let base = self.position_of(DEST_OSC2_DETUNE, base_value);
                        let position = self.modulated(base, DEST_OSC2_DETUNE, voice, frame, voices);
                        let moved = self.value_of(DEST_OSC2_DETUNE, position);
                        (semitones + moved - base_value) as f32
                    } else {
                        first
                    };
                    if drift_on {
                        let drift = self.drift_pitch[frame * stride + voice];
                        first += drift;
                        second += drift;
                    }
                    for copy in 0..copies {
                        let lane = voice * COPIES + copy;
                        let extra = plan.cents[copy] / 100.0 + self.unison_random[lane] * unison;
                        self.pitch_mod[frame * LANES + lane] = first + extra;
                        self.pitch_mod2[frame * LANES + lane] = second + extra;
                    }
                }
            }
        }
        if shape_moved {
            self.frame_mod[..count * LANES].fill(0.0);
            for frame in 0..count {
                for voice in 0..span {
                    let moved = self.matrix.add(
                        DEST_OSC1_SHAPE,
                        0.0,
                        voice,
                        frame,
                        &|source, voice, frame| self.source(source, voice, frame, voices),
                    );
                    let frames = (moved * (SHAPE_FRAMES - 1) as f64) as f32;
                    let first = frame * LANES + voice * COPIES;
                    self.frame_mod[first..first + COPIES].fill(frames);
                }
            }
        }

        // 4. The oscillators, band-limited, every copy in its lane.
        let inputs1 = Inputs {
            lanes: lane_span,
            pitch: voices.pitch(),
            pitch_step: Some(voices.pitch_step()),
            pitch_mod: pitch_moved.then(|| &self.pitch_mod[..count * LANES]),
            frame_mod: shape_moved.then(|| &self.frame_mod[..count * LANES]),
        };
        self.osc1
            .process(count, &inputs1, &mut self.osc1_out[..count * LANES]);
        let pitch_moved2 = pitch_moved || detune_moved;
        let inputs2 = Inputs {
            lanes: lane_span,
            pitch: voices.pitch(),
            pitch_step: Some(voices.pitch_step()),
            pitch_mod: pitch_moved2.then(|| &self.pitch_mod2[..count * LANES]),
            frame_mod: None,
        };
        self.osc2
            .process(count, &inputs2, &mut self.osc2_out[..count * LANES]);

        // 5. The sources summed, with the first saturation point. The
        //    gains take their Mod routes, per voice, and a voice's copies
        //    their gains into the left and right.
        let pre = self.pre;
        let noise_on = self.noise_on;
        let gains_moved = self.matrix.has(DEST_OSC1_GAIN)
            || self.matrix.has(DEST_OSC2_GAIN)
            || self.matrix.has(DEST_NOISE_GAIN);
        for frame in 0..count {
            let at = from + frame;
            let plan = self.plan(at);
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
                let (mut a, mut b, mut a_right, mut b_right) = (0.0, 0.0, 0.0, 0.0);
                for copy in 0..copies {
                    let lane = frame * LANES + voice * COPIES + copy;
                    let (x, y) = (self.osc1_out[lane], self.osc2_out[lane]);
                    a += x * plan.left[copy];
                    b += y * plan.left[copy];
                    if stereo {
                        a_right += x * plan.right[copy];
                        b_right += y * plan.right[copy];
                    }
                }
                let mut sample = a * g1 + b * g2;
                let mut sample_right = a_right * g1 + b_right * g2;
                if noise_on && gn != 0.0 {
                    let noise = self.noise(voice) * gn;
                    sample += noise;
                    sample_right += noise;
                }
                if self.filter_on {
                    sample = soft_clip(sample, pre);
                    sample_right = soft_clip(sample_right, pre);
                }
                self.audio[frame * stride + voice] = sample;
                if stereo {
                    self.audio_r[frame * stride + voice] = sample_right;
                }
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
                    self.set_filter_params(voice, &params);
                }
                self.filter_resonance_moved = true;
            }
            let post = self.post;
            let channels = if stereo { 2 } else { 1 };
            for (channel, audio) in [&mut self.audio, &mut self.audio_r]
                .into_iter()
                .take(channels)
                .enumerate()
            {
                self.lp[channel].process_voices(
                    span,
                    &self.cutoff[..count * stride],
                    &mut audio[..count * stride],
                );
                self.hp[channel].process_voices(
                    span,
                    &self.hp_cutoff[..count * stride],
                    &mut audio[..count * stride],
                );
                if post > 0.0 {
                    for sample in &mut audio[..count * stride] {
                        *sample = soft_clip(*sample, post);
                    }
                }
            }
        }

        // 7. The voices mixed, under Envelope 1, the velocity, and the Main
        //    Volume and its Mod routes: to one mono signal, or in Stereo to
        //    a left and a right.
        let volume_moved = self.matrix.has(DEST_VOLUME);
        let mut mask = voices.active_mask();
        while mask != 0 {
            let voice = mask.trailing_zeros() as usize;
            mask &= mask - 1;
            let velocity = voices.velocity()[voice * COPIES];
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
                if stereo {
                    self.out_r[from + frame] +=
                        self.audio_r[frame * stride + voice] * level * scale * volume;
                }
            }
        }

        // Spec: modulation R31. The controls of Shape, LFO Rate and Cyc Env
        // Rate show the newest voice's movement at the stretch's last frame,
        // as the others do (the rest are shown as they are worked out).
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
            if self.matrix.has(DEST_LFO_RATE) && self.lfos.time.mode == MODE_RATE {
                let hz = self.lfo_rate(voice, last, voices);
                self.matrix.show(
                    DEST_LFO_RATE,
                    self.position_of(DEST_LFO_RATE, hz).clamp(0.0, 1.0),
                );
            }
            if self.matrix.has(DEST_CYC_RATE) && self.cycle.on && self.cycle.time.mode == MODE_RATE
            {
                let hz = self.speed(DEST_CYC_RATE, &self.cycle.time, voice, last, voices);
                self.matrix.show(
                    DEST_CYC_RATE,
                    self.position_of(DEST_CYC_RATE, hz).clamp(0.0, 1.0),
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
    /// Spec: polysynth R26 to R28. The copies the voice mode plays at frame
    /// `at` of the stretch: Poly one; Mono four at Thickness; Stereo two,
    /// detuned and panned by Spread; Unison four, detuned by Strength. A
    /// stack keeps the note's level for uncorrelated copies (instrument
    /// voices R13).
    fn plan(&self, at: usize) -> CopyPlan {
        match self.mode {
            MODE_MONO => {
                let thickness = self.thickness[at] / 100.0;
                let level = (1.0 + 3.0 * thickness * thickness).sqrt().recip();
                let gains = [
                    level,
                    thickness * level,
                    thickness * level,
                    thickness * level,
                ];
                CopyPlan {
                    cents: MONO_CENTS,
                    left: gains,
                    right: gains,
                }
            }
            MODE_STEREO => {
                let spread = self.spread[at] / 100.0;
                let (left0, right0) = instrument::copy_gains(0, 2, spread);
                let (left1, right1) = instrument::copy_gains(1, 2, spread);
                let cents = STEREO_CENTS * spread;
                CopyPlan {
                    cents: [
                        instrument::copy_cents(0, 2, cents),
                        instrument::copy_cents(1, 2, cents),
                        0.0,
                        0.0,
                    ],
                    left: [left0, left1, 0.0, 0.0],
                    right: [right0, right1, 0.0, 0.0],
                }
            }
            MODE_UNISON => {
                let cents = UNISON_CENTS * self.strength[at] / 100.0;
                let (gain, _) = instrument::copy_gains(0, COPIES, 0.0);
                CopyPlan {
                    cents: core::array::from_fn(|i| instrument::copy_cents(i, COPIES, cents)),
                    left: [gain; COPIES],
                    right: [gain; COPIES],
                }
            }
            _ => CopyPlan {
                cents: [0.0; COPIES],
                left: [1.0, 0.0, 0.0, 0.0],
                right: [1.0, 0.0, 0.0, 0.0],
            },
        }
    }

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

    // Spec: polysynth R31 (Envelope 2's cycle: Tilt, Hold and the bent slopes)
    #[test]
    fn the_cycle_rises_holds_and_falls_by_tilt_and_hold() {
        let near = |a: f64, b: f64| (a - b).abs() < 1e-9;
        // Tilt 50 %, no Hold: a triangle that peaks half way.
        assert!(near(cycle_wave(0.0, 0.5, 0.0), 0.0));
        assert!(near(cycle_wave(0.25, 0.5, 0.0), 0.5));
        assert!(near(cycle_wave(0.5, 0.5, 0.0), 1.0));
        assert!(near(cycle_wave(0.75, 0.5, 0.0), 0.5));
        // Hold 50 % keeps the top for half the cycle, between the slopes.
        assert!(near(cycle_wave(0.25, 0.5, 0.5), 1.0));
        assert!(near(cycle_wave(0.7, 0.5, 0.5), 1.0));
        assert!(near(cycle_wave(0.875, 0.5, 0.5), 0.5));
        // Tilt moves the top to either end of the cycle.
        assert!(near(cycle_wave(0.0, 0.0, 0.0), 1.0));
        assert!(cycle_wave(0.999, 1.0, 0.0) > 0.99);
        // Within a tenth of an end the slope beside the top bends towards an
        // exponential: it falls faster at first (Tilt 0) or rises faster
        // (Tilt 100 %) than a straight line, and is straight from 10 %.
        assert!(cycle_wave(0.5, 0.0, 0.0) < 0.1);
        assert!(cycle_wave(0.5, 1.0, 0.0) > 0.9);
        assert!(near(cycle_wave(0.5, 0.1, 0.0), 1.0 - (0.5 - 0.1) / 0.9));
        assert!(cycle_wave(0.5, 0.05, 0.0) < 1.0 - (0.5 - 0.05) / 0.95);
        // It never leaves 0 to 1.
        for tilt in [0.0, 0.03, 0.5, 0.97, 1.0] {
            for hold in [0.0, 0.3, 1.0] {
                for step in 0..1000 {
                    let value = cycle_wave(f64::from(step) / 1000.0, tilt, hold);
                    assert!(
                        (0.0..=1.0).contains(&value),
                        "{tilt} {hold} {step}: {value}"
                    );
                }
            }
        }
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

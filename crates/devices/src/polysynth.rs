//! Cascade, the native polysynth (WP-2.17, part 1): two band-limited
//! oscillators with their shapes, a noise source, a zero-delay-feedback
//! low-pass and high-pass filter with drive and key tracking, and the
//! amplifier and modulation envelopes, played through the plug-in kit's
//! polyphonic voice manager.
//!
//! Its behaviour, parameters, units and names follow Live 12's compact
//! polyphonic synthesizer as its manual describes it (see
//! `docs/specs/polysynth.md`); the manual records few ranges or defaults,
//! so those are Beatlayer's own and are listed there to observe. The
//! waveforms, the shape morphing and the saturation curves are Beatlayer's
//! own too: the manual names the waveforms and says the Shape knob changes
//! the harmonic content, but doesn't draw any of them.
//!
//! Part 1 plays polyphonically. The voice modes, the Drift-style per-voice
//! variation, the LFO, the modulation matrix and MPE arrive with WP-2.17's
//! second part.

use core::f64::consts::TAU;

use bl_engine::{DeviceType, NodeConfig, NodeEvent, NoteKind, Registry};
use bl_model::device_layout::{BODY_HEIGHT, ControlBand, DisplayCell, LayoutNode, Widget};
use bl_model::param::gain_to_db;
use bl_model::{DeviceDescription, DeviceLayout, DeviceRole, ParamDescriptor, ParamId, ParamUnit};
use lspk_dsp::envelope::{EnvelopeBank, EnvelopeParams, StealRestart, TimeUnit, Timing, Trigger};
use lspk_dsp::filter::{FilterBank, FilterType, VoiceParams, pitch_of};
use lspk_plugin::{Event, Note};
use lspk_voice::{
    Layout as VoiceLayout, MAX_VOICES, Start, VoiceBlock, VoiceEvent, VoiceEventKind, VoiceHandler,
    Voices,
};
use lspk_wavetable::osc::{
    Inputs, Layout as OscLayout, Onset as OscOnset, Oscillator, Params as OscParams, PhaseMode,
};
use lspk_wavetable::{Cache, FRAME_SIZE, SharedTable, Wavetable};

use crate::framework::{Audio, NativeDevice, ParamBank, add};

/// Cascade's device type.
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
/// The most notes a stretch plays; more are dropped.
const NOTES: usize = 512;
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

/// The Cascade device type.
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
        ParamDescriptor::integer(VOICES, "Voices", (1, MAX_VOICES as i64), 16, ParamUnit::None)
            .decimals(0)
            .describe("The most notes that sound at once."),
        ParamDescriptor::integer(TRANSPOSE, "Transpose", (-48, 48), 0, ParamUnit::Semitones)
            .decimals(0)
            .describe("Transposes the instrument in semitones."),
        ParamDescriptor::continuous(VEL_VOL, "Vel > Vol", (0.0, 100.0), 0.0, ParamUnit::Percent)
            .describe("How much the notes' velocity moves the volume."),
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
                (VOLUME, Widget::Knob),
                (VOICES, Widget::Number),
                (TRANSPOSE, Widget::Number),
                (VEL_VOL, Widget::Knob),
            ],
        )),
    ]));
    DeviceType::native(POLYSYNTH, "Cascade", DeviceRole::Instrument)
        .with_description(DeviceDescription::new(params).with_layout(layout))
}

/// Adds Cascade to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, polysynth_type(), make);
}

/// Makes a Cascade for `config`'s rate, channels, and block size. Every
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

/// Cascade's audio-thread half.
pub struct PolySynth {
    voices: Voices,
    synth: Synth,
    events: Vec<Event>,
}

impl PolySynth {
    fn new(config: &NodeConfig) -> Self {
        let rate = config.sample_rate.as_f64() as f32;
        Self {
            voices: Voices::new(
                VoiceLayout {
                    voices: MAX_VOICES,
                    lanes_per_voice: 1,
                },
                rate,
            ),
            synth: Synth::new(rate, config.max_frames),
            events: Vec::with_capacity(NOTES),
        }
    }
}

impl NativeDevice for PolySynth {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        let frames = audio.frames();
        if frames == 0 || audio.channels() == 0 {
            return;
        }
        self.events.clear();
        for note in audio.notes() {
            if self.events.len() >= self.events.capacity() {
                break;
            }
            let which = Note {
                port: Some(0),
                channel: Some(note.channel),
                key: Some(note.key),
                id: note.note_id,
            };
            self.events.push(match note.kind {
                NoteKind::On => Event::NoteOn {
                    time: note.offset,
                    note: which,
                    velocity: note.velocity,
                },
                NoteKind::Off => Event::NoteOff {
                    time: note.offset,
                    note: which,
                    velocity: note.velocity,
                },
                NoteKind::Choke => Event::NoteChoke {
                    time: note.offset,
                    note: which,
                },
            });
        }
        self.synth.snapshot(params, frames);
        self.voices.settings_mut().voices = self.synth.polyphony;
        let (voices, synth) = (&mut self.voices, &mut self.synth);
        voices.process(self.events.iter().copied(), frames, synth);
        for channel in 0..audio.channels() {
            audio
                .channel_mut(channel)
                .copy_from_slice(&self.synth.out[..frames]);
        }
    }

    fn flush(&mut self, _events: &[NodeEvent], _params: &ParamBank) {
        // A switched-off instrument is silent and holds no notes, so it
        // doesn't carry one over to the moment it comes back on.
        self.voices.reset();
        self.synth.silence();
    }
}

/// Everything but the voice manager: the [`VoiceHandler`] the voices call.
struct Synth {
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
    vel_vol: Vec<f32>,
    /// The stretch's per-voice buffers, `frames × stride` each.
    osc1_out: Vec<f32>,
    osc2_out: Vec<f32>,
    audio: Vec<f32>,
    cutoff: Vec<f32>,
    hp_cutoff: Vec<f32>,
    env1_out: Vec<f32>,
    env2_out: Vec<f32>,
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
    /// The most notes at once.
    polyphony: usize,
    /// The filter's settings and the envelopes', kept so a change is
    /// applied once.
    filter_params: VoiceParams,
    env1_params: EnvelopeParams,
    env2_params: EnvelopeParams,
    /// The saturation the sources' gains ask for, before and after the
    /// filter.
    pre: f32,
    post: f32,
}

impl Synth {
    fn new(rate: f32, max_frames: usize) -> Self {
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
        let osc1 = Oscillator::new(tables1[SAW].clone(), layout, rate, max_frames, 0x0C45_CADE);
        let osc2 = Oscillator::new(tables2[3].clone(), layout, rate, max_frames, 0x0C45_CADF);
        let mut lp = FilterBank::new(rate, MAX_VOICES, 1);
        lp.set_type(FilterType::Lp12);
        let mut hp = FilterBank::new(rate, MAX_VOICES, 1);
        hp.set_type(FilterType::Hp12);
        let stride = MAX_VOICES.next_multiple_of(16);
        let buffer = || vec![0.0; max_frames * stride];
        let ramp = || vec![0.0; max_frames];
        Self {
            stride,
            osc1,
            osc2,
            tables1,
            tables2,
            lp,
            hp,
            env1: EnvelopeBank::new(
                MAX_VOICES,
                EnvelopeParams::AMP,
                Timing::new(f64::from(rate)),
            ),
            env2: EnvelopeBank::new(
                MAX_VOICES,
                EnvelopeParams::MODULATION,
                Timing::new(f64::from(rate)),
            ),
            rng: core::array::from_fn(|voice| 0x9E37_79B9u32.wrapping_mul(voice as u32 + 1) | 1),
            g1: ramp(),
            g2: ramp(),
            gn: ramp(),
            volume: ramp(),
            freq: ramp(),
            hp_hz: ramp(),
            key: ramp(),
            vel_vol: ramp(),
            osc1_out: buffer(),
            osc2_out: buffer(),
            audio: buffer(),
            cutoff: buffer(),
            hp_cutoff: buffer(),
            env1_out: buffer(),
            env2_out: buffer(),
            out: vec![0.0; max_frames],
            osc1_on: true,
            osc2_on: true,
            noise_on: false,
            filter_on: true,
            retrigger: true,
            wave1: SAW,
            wave2: 3,
            filter_type: 0,
            polyphony: 16,
            filter_params: VoiceParams::default(),
            env1_params: EnvelopeParams::AMP,
            env2_params: EnvelopeParams::MODULATION,
            pre: 0.0,
            post: 0.0,
        }
    }

    /// Reads the stretch's parameters into the synth: the ramps into its
    /// buffers, the switches and choices into its fields, and everything
    /// the oscillators, the filters and the envelopes need.
    fn snapshot(&mut self, params: &ParamBank, frames: usize) {
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
        ramp_into(&mut self.vel_vol[..frames], params, VEL_VOL);
        self.out[..frames].fill(0.0);

        self.osc1_on = params.value(at(OSC1)) >= 0.5;
        self.osc2_on = params.value(at(OSC2)) >= 0.5;
        self.noise_on = params.value(at(NOISE)) >= 0.5;
        self.filter_on = params.value(at(FILTER)) >= 0.5;
        self.retrigger = params.value(at(RETRIGGER)) >= 0.5;
        self.polyphony = (params.value(at(VOICES)) as usize).clamp(1, MAX_VOICES);
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
        if filter_params != self.filter_params {
            self.filter_params = filter_params;
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

impl VoiceHandler for Synth {
    fn voice_event(&mut self, voices: &mut VoiceBlock, event: VoiceEvent) {
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

    fn render(&mut self, voices: &mut VoiceBlock, frames: std::ops::Range<usize>) {
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

        // 1. Each voice's low-pass cutoff per frame, in semitones: the
        //    knob's frequency, moved by the played note through Key. The
        //    high-pass doesn't key-track.
        {
            let pitch = voices.pitch();
            let step = voices.pitch_step();
            for frame in 0..count {
                let base = pitch_of(self.freq[from + frame], REFERENCE);
                let key = self.key[from + frame];
                let hp = pitch_of(self.hp_hz[from + frame], REFERENCE);
                for voice in 0..span {
                    let note = pitch[voice] + step[voice] * frame as f32 - MIDDLE_C;
                    self.cutoff[frame * stride + voice] = base + key * note;
                    self.hp_cutoff[frame * stride + voice] = hp;
                }
            }
        }

        // 2. The oscillators, band-limited, every voice in its lane.
        let inputs = Inputs {
            lanes: span,
            pitch: voices.pitch(),
            pitch_step: Some(voices.pitch_step()),
            pitch_mod: None,
            frame_mod: None,
        };
        self.osc1
            .process(count, &inputs, &mut self.osc1_out[..count * stride]);
        self.osc2
            .process(count, &inputs, &mut self.osc2_out[..count * stride]);

        // 3. The sources summed, with the first saturation point.
        let pre = self.pre;
        let noise_on = self.noise_on;
        for frame in 0..count {
            let (mut g1, mut g2, gn) = (
                self.g1[from + frame],
                self.g2[from + frame],
                self.gn[from + frame],
            );
            if !self.osc1_on {
                g1 = 0.0;
            }
            if !self.osc2_on {
                g2 = 0.0;
            }
            for voice in 0..span {
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

        // 4. The filters and the second saturation point.
        if self.filter_on {
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

        // 5. The envelopes, and the voices mixed to one mono signal.
        self.env1.process(&mut self.env1_out[..count * stride]);
        self.env2.process(&mut self.env2_out[..count * stride]);
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
                let vel_vol = self.vel_vol[from + frame];
                let scale = 1.0 - vel_vol + vel_vol * velocity;
                self.out[from + frame] +=
                    self.audio[frame * stride + voice] * level * scale * self.volume[from + frame];
            }
        }

        // 6. A voice whose amplifier envelope has ended is silent.
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

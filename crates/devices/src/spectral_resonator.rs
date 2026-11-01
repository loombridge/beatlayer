//! Spectral Resonator (WP-8.48; the Spectral Resonator spec): the input's
//! spectrum, analysed by `bl_dsp::stft`, drives a comb of partials at a
//! fundamental that a control or the notes of a MIDI track set, each partial
//! ringing on with its own decay. A modulation section moves or gates the
//! partials, Unison duplicates them, and a spectrogram shows the result.
//!
//! The manual gives the controls but no laws or ranges, so the partials' law,
//! the decay and the damps, the modulation modes, Unison and the spectrogram
//! are Beatlayer's own (`docs/specs/spectral-resonator.md`).
//!
//! A partial is a sine oscillator at exactly its frequency, whose amplitude
//! is the larger of what the analysis brings at that frequency and what
//! rings on from before, falling 60 dB in the partial's decay time. The
//! oscillators run per sample; everything else (the pitches, the partials'
//! frequencies and decays, the modulation, the amplitudes) is worked out
//! every [`TICK`] frames counted from the node's start, and again on the
//! frame of each note or scale change, so a render doesn't depend on how the
//! host cuts its blocks.

use core::f64::consts::{FRAC_PI_4, TAU};

use bl_dsp::Random;
use bl_dsp::stft::{self, Complex32, Stft, Window};
use bl_engine::{DeviceType, KeyPitches, NodeConfig, NodeData, NoteEvent, NoteKind, Registry};
use bl_model::device_layout::{BODY_HEIGHT, ControlBand, DisplayCell, LayoutNode, Widget};
use bl_model::{
    Device, DeviceDescription, DeviceDisplay, DeviceId, DeviceLayout, DeviceRole, DisplayMarker,
    DisplayReading, DisplaySeries, DisplaySpectrogram, ParamDescriptor, ParamId, ParamUnit, Scale,
    SpectrogramGrid,
};

use crate::framework::{Audio, DelayLine, NativeDevice, ParamBank, add};
use crate::midi_fx::use_current_scale;

/// Spectral Resonator's device type.
pub const SPECTRAL_RESONATOR: &str = "beatlayer.spectral_resonator";

/// Pitch Mode: Internal or MIDI, one of [`PITCH_MODES`].
pub const PITCH_MODE: ParamId = ParamId(0);
/// Freq: the fundamental in Internal mode, in Hz (R5).
pub const FREQ: ParamId = ParamId(1);
/// SD Shift: the fundamental in Internal mode as a degree of the current
/// scale, which Freq becomes with Use Current Scale on (R5).
pub const SD_SHIFT: ParamId = ParamId(2);
/// Transp.: moves the fundamental by semitones, or by degrees with Use
/// Current Scale on.
pub const TRANSP: ParamId = ParamId(3);
/// Reads SD Shift and Transp. in degrees of the current scale; the title bar
/// draws it.
pub const USE_SCALE: ParamId = ParamId(4);
/// Stretch: spreads the partials apart (above 0) or squeezes them together
/// (below), and at +100 % leaves the odd ones (R2).
pub const STRETCH: ParamId = ParamId(5);
/// Shift: moves the input's spectrum by semitones before the resonances (R4).
pub const SHIFT: ParamId = ParamId(6);
/// Decay: how long a partial at 1 kHz takes to fall 60 dB (R3).
pub const DECAY: ParamId = ParamId(7);
/// HF Damp: shortens the decay above 1 kHz.
pub const HF_DAMP: ParamId = ParamId(8);
/// LF Damp: shortens the decay below 1 kHz.
pub const LF_DAMP: ParamId = ParamId(9);
/// Harmonics: how many partials sound.
pub const HARMONICS: ParamId = ParamId(10);
/// Modulation: one of [`MODULATIONS`] (R9).
pub const MODULATION: ParamId = ParamId(11);
/// Mod Rate, in Hz.
pub const MOD_RATE: ParamId = ParamId(12);
/// Pch. Mod: how far the modulation moves a partial's pitch, in semitones.
pub const PCH_MOD: ParamId = ParamId(13);
/// Unison: how many copies of each partial sound (R10).
pub const UNISON: ParamId = ParamId(14);
/// Uni. Amt: how far the copies are detuned, in per cent of 50 cents.
pub const UNI_AMT: ParamId = ParamId(15);
/// Quantize: moves each partial to the nearest note of the scale or tuning
/// (R7).
pub const QUANTIZE: ParamId = ParamId(16);
/// Input Send: the gain of the input into the analysis, before its limiter.
pub const INPUT_SEND: ParamId = ParamId(17);
/// Dry/Wet: the balance between the dry signal and the resonances.
pub const DRY_WET: ParamId = ParamId(18);
/// Mono/Poly: one of [`VOICE_MODES`], MIDI mode's voices (R6).
pub const MONO_POLY: ParamId = ParamId(19);
/// Polyphony: the voices of Poly.
pub const POLYPHONY: ParamId = ParamId(20);
/// MIDI Gate: whether the resonances sound only while a note is held, in
/// Mono.
pub const MIDI_GATE: ParamId = ParamId(21);
/// Glide: the time a Mono note takes to slide to the next, in milliseconds.
pub const GLIDE: ParamId = ParamId(22);
/// PB: the pitch bend range, in semitones.
pub const PB: ParamId = ParamId(23);

/// How many parameters the device has.
const PARAMS: usize = 24;

/// The Pitch Mode choices.
pub const PITCH_MODES: [&str; 2] = ["Internal", "MIDI"];
/// The Modulation choices.
pub const MODULATIONS: [&str; 4] = ["None", "Chorus", "Wander", "Granular"];
/// The Mono/Poly choices.
pub const VOICE_MODES: [&str; 2] = ["Mono", "Poly"];

/// The voices in the pool, made with the node (R12).
pub const VOICES: usize = 16;
/// The most partials a voice has: Harmonics' top.
pub const PARTIALS: usize = 64;
/// The most copies of a partial Unison makes.
pub const COPIES: usize = 8;

/// The spectrogram's name, and the name of the series its partial markers sit
/// on.
pub const SPECTROGRAM: &str = "Spectrogram";
pub const PARTIAL_SERIES: &str = "Partials";
/// The limiter lamp's reading.
pub const LIMITER: &str = "Limiter";

/// The spectrogram's grid: 128 columns over the last four seconds, 64 rows
/// from 40 Hz to 16 kHz, drawn between −90 and 0 dB (R15).
pub const GRID: SpectrogramGrid = SpectrogramGrid {
    columns: 128,
    rows: 64,
    frequencies: [40.0, 16_000.0],
    log_frequency: true,
    levels: [-90.0, 0.0],
};

/// The seconds the spectrogram covers.
const HISTORY_SECONDS: f64 = 4.0;

/// Frames between the control-rate updates.
const TICK: usize = 16;
/// The STFT's frame at 48 kHz, scaled with the rate, and its overlap (R2).
const FRAME_AT_48K: f64 = 2_048.0;
const OVERLAP: usize = 4;
/// The most notes and messages one stretch of a block takes.
const EVENT_ROOM: usize = 4_096;
/// The keys of notes held at once that Mono remembers.
const HELD_KEYS: usize = 16;
/// The sine table's size, a power of two no larger than 2^12, and a whole
/// cycle of an oscillator's phase.
const TABLE: usize = 4_096;
const PHASE_ONE: f64 = 4_294_967_296.0;
/// The key a fundamental in degrees counts from the root in the octave of,
/// C3, and the key a Mono voice sits on before any note.
const C3: i32 = 60;
/// The most degrees Transp. and SD Shift count.
const MOST_DEGREES: i32 = 28;
/// The most semitones Transp. counts.
const MOST_SEMITONES: i32 = 48;
/// How long the input limiter takes to let go, in seconds, and how long its
/// lamp stays lit after it acts.
const LIMITER_RELEASE: f64 = 0.05;
const LAMP_SECONDS: f64 = 0.1;
/// How long a granular gate takes to open or close, in seconds.
const GATE_SECONDS: f64 = 0.005;
/// A level below this is silence.
const FLOOR: f32 = 1.0e-9;
/// Where each partial of a Chorus sits in its cycle, per partial number: the
/// golden ratio, so no two line up.
const GOLDEN: f64 = 0.618_033_988_749_894_9;
/// How many times faster than Decay a partial at 20 Hz (LF Damp) or 20 kHz
/// (HF Damp) falls with its damp at 100 % (R3).
const DAMP_FACTOR: f64 = 8.0;
/// How long a partial's level takes to follow the analysis, in hops.
const FOLLOW_HOPS: f64 = 0.5;
/// How high above the display's floor the partial markers sit: the share of
/// the cell's height.
const MARKER_HEIGHT: f32 = 0.04;

/// What the display publishes: the grid, the limiter lamp, each voice's
/// fundamental and each voice's partials, in that order.
const DISPLAY_LEN: usize = GRID.columns * GRID.rows + 1 + VOICES + VOICES * PARTIALS;

/// Spectral Resonator's type.
#[must_use]
pub fn spectral_resonator_type() -> DeviceType {
    let params = vec![
        ParamDescriptor::choice(PITCH_MODE, "Pitch Mode", &PITCH_MODES, 0).describe(
            "Internal rings at Freq; MIDI rings at the notes of the device's MIDI source.",
        ),
        ParamDescriptor::continuous(FREQ, "Freq", (20.0, 5_000.0), 110.0, ParamUnit::Hertz)
            .logarithmic()
            .describe("The fundamental in Internal mode, in Hz."),
        ParamDescriptor::integer(SD_SHIFT, "SD Shift", (-28, 28), 0, ParamUnit::None).describe(
            "With Use Current Scale on, the fundamental in Internal mode as a degree of the \
             current scale, counted from its root in the octave of C3.",
        ),
        ParamDescriptor::integer(TRANSP, "Transp.", (-48, 48), 0, ParamUnit::Semitones).describe(
            "Moves the fundamental by semitones, or by degrees of the current scale with Use \
             Current Scale on.",
        ),
        ParamDescriptor::toggle(USE_SCALE, "Use Current Scale", false)
            .describe("Counts SD Shift and Transp. in degrees of the current scale."),
        ParamDescriptor::continuous(STRETCH, "Stretch", (-100.0, 100.0), 0.0, ParamUnit::Percent)
            .describe(
                "Below 0 the partials are squeezed together, above 0 spread apart; at 100 % only \
                 the odd ones sound.",
            ),
        ParamDescriptor::continuous(SHIFT, "Shift", (-48.0, 48.0), 0.0, ParamUnit::Semitones)
            .describe("Moves the input's spectrum before it reaches the resonances."),
        ParamDescriptor::continuous(DECAY, "Decay", (0.05, 20.0), 1.0, ParamUnit::Seconds)
            .logarithmic()
            .describe("How long a partial at 1 kHz takes to fall 60 dB."),
        ParamDescriptor::continuous(HF_DAMP, "HF Damp", (0.0, 100.0), 0.0, ParamUnit::Percent)
            .describe("Shortens the decay of the partials above 1 kHz, up to 8 times at 20 kHz."),
        ParamDescriptor::continuous(LF_DAMP, "LF Damp", (0.0, 100.0), 0.0, ParamUnit::Percent)
            .describe("Shortens the decay of the partials below 1 kHz, up to 8 times at 20 Hz."),
        ParamDescriptor::integer(HARMONICS, "Harmonics", (1, 64), 32, ParamUnit::None)
            .describe("How many partials sound: more is brighter."),
        ParamDescriptor::choice(MODULATION, "Modulation", &MODULATIONS, 0).describe(
            "Chorus moves the partials in a sine, Wander moves each along its own random line, \
             and Granular gates each on and off at random and scatters its pitch.",
        ),
        ParamDescriptor::continuous(MOD_RATE, "Mod Rate", (0.01, 20.0), 1.0, ParamUnit::Hertz)
            .logarithmic()
            .describe("How fast the modulation moves."),
        ParamDescriptor::continuous(PCH_MOD, "Pch. Mod", (0.0, 12.0), 0.2, ParamUnit::Semitones)
            .describe("How far the modulation moves a partial's pitch."),
        ParamDescriptor::integer(UNISON, "Unison", (1, 8), 1, ParamUnit::None).describe(
            "How many copies of each partial sound, detuned and spread between the channels.",
        ),
        ParamDescriptor::continuous(UNI_AMT, "Uni. Amt", (0.0, 100.0), 30.0, ParamUnit::Percent)
            .describe("How far Unison's copies are detuned: 100 % is 50 cents either way."),
        ParamDescriptor::toggle(QUANTIZE, "Quantize", false).describe(
            "Moves each partial to the nearest note of the current scale or tuning, or the \
             nearest semitone without one.",
        ),
        ParamDescriptor::gain(INPUT_SEND, "Input Send", 12.0, 0.0).describe(
            "The input's level into the resonances, before a limiter that keeps it below 0 dBFS.",
        ),
        ParamDescriptor::continuous(DRY_WET, "Dry/Wet", (0.0, 100.0), 100.0, ParamUnit::Percent)
            .describe("The balance between the dry signal and the resonances."),
        ParamDescriptor::choice(MONO_POLY, "Mono/Poly", &VOICE_MODES, 0).describe(
            "Mono rings at the newest held note; Poly rings each held note on a voice of its \
             own.",
        ),
        ParamDescriptor::integer(POLYPHONY, "Polyphony", (2, 16), 8, ParamUnit::None)
            .describe("The voices of Poly: a note past them takes the place of the oldest voice."),
        ParamDescriptor::toggle(MIDI_GATE, "MIDI Gate", true).describe(
            "In Mono, the resonances sound only while a note is held, ringing out after it; off, \
             they ring at the last note.",
        ),
        ParamDescriptor::continuous(GLIDE, "Glide", (0.0, 5_000.0), 0.0, ParamUnit::Milliseconds)
            .describe("How long a Mono note takes to slide the pitch to the next."),
        ParamDescriptor::integer(PB, "PB", (0, 24), 2, ParamUnit::Semitones)
            .describe("How far the full pitch bend of the MIDI source moves every voice."),
    ];
    DeviceType::native(
        SPECTRAL_RESONATOR,
        "Spectral Resonator",
        DeviceRole::AudioEffect,
    )
    .with_midi_sidechain_tap()
    .with_description(
        DeviceDescription::new(params)
            .with_layout(layout())
            .with_spectrogram(GRID)
            .with_title_switch(USE_SCALE)
            .with_banks(&[
                (
                    "Main",
                    &[
                        "Freq",
                        "Transp.",
                        "Decay",
                        "Stretch",
                        "Shift",
                        "Harmonics",
                        "Input Send",
                        "Dry/Wet",
                    ],
                ),
                (
                    "Damping and Modulation",
                    &[
                        "HF Damp",
                        "LF Damp",
                        "Modulation",
                        "Mod Rate",
                        "Pch. Mod",
                        "Unison",
                        "Uni. Amt",
                        "Quantize",
                    ],
                ),
                (
                    "MIDI",
                    &[
                        "Pitch Mode",
                        "Mono/Poly",
                        "Polyphony",
                        "MIDI Gate",
                        "Glide",
                        "PB",
                    ],
                ),
                ("Scale", &["SD Shift"]),
            ]),
    )
}

/// The display cell, then the pitch, the resonance, the modulation, Unison
/// and the output; the MIDI source's parameters in the expanded part.
fn layout() -> DeviceLayout {
    use Widget::{Chooser, Knob, Number, Toggle};
    let band = |controls: &[(ParamId, Widget)]| {
        LayoutNode::Band(ControlBand::stacked(BODY_HEIGHT, controls))
    };
    DeviceLayout::new(LayoutNode::Row(vec![
        LayoutNode::Cell(DisplayCell::new(SPECTROGRAM, 260.0, BODY_HEIGHT, 1.0)),
        band(&[
            (PITCH_MODE, Chooser),
            (FREQ, Knob),
            (SD_SHIFT, Number),
            (TRANSP, Number),
        ]),
        band(&[
            (DECAY, Knob),
            (STRETCH, Knob),
            (SHIFT, Knob),
            (HARMONICS, Number),
        ]),
        band(&[(HF_DAMP, Knob), (LF_DAMP, Knob), (QUANTIZE, Toggle)]),
        band(&[(MODULATION, Chooser), (MOD_RATE, Knob), (PCH_MOD, Knob)]),
        band(&[(UNISON, Number), (UNI_AMT, Knob)]),
        band(&[(INPUT_SEND, Knob), (DRY_WET, Knob)]),
    ]))
    .with_expanded(band(&[
        (MONO_POLY, Chooser),
        (POLYPHONY, Number),
        (MIDI_GATE, Toggle),
        (GLIDE, Knob),
        (PB, Number),
    ]))
}

/// Adds Spectral Resonator to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, spectral_resonator_type(), SpectralResonator::new);
}

/// The frame of the analysis at `rate` Hz: 2,048 at 48 kHz, a power of two.
fn frame_size(rate: f64) -> usize {
    let size = 1usize << (FRAME_AT_48K * rate / 48_000.0).log2().round().max(0.0) as u32;
    size.clamp(stft::MIN_SIZE, stft::MAX_SIZE)
}

/// The frequency of the pitch `semitones` from A3 = 440 Hz at 69.
fn hz_of(semitones: f64) -> f64 {
    440.0 * ((semitones - 69.0) / 12.0).exp2()
}

/// The pitch of `hz` in semitones from A3 = 440 Hz at 69.
fn semitones_of(hz: f64) -> f64 {
    69.0 + 12.0 * (hz / 440.0).log2()
}

/// How many times faster than Decay a partial at `hz` falls, with the damps
/// at `hf` and `lf` (0 to 1) (R3): 1 at 1 kHz, up to 8 at 20 kHz and 20 Hz.
fn speed(hz: f64, hf: f64, lf: f64) -> f64 {
    let (damp, reach) = if hz > 1_000.0 {
        (hf, (hz / 1_000.0).ln() / 20f64.ln())
    } else {
        (lf, (1_000.0 / hz).ln() / 50f64.ln())
    };
    1.0 + (DAMP_FACTOR - 1.0) * damp * reach.clamp(0.0, 1.0)
}

/// What the parameters are, over one stretch of a block: the ones the audio
/// thread reads each tick as per-frame values, and the choices.
struct Params<'a> {
    internal: bool,
    poly: bool,
    use_scale: bool,
    gate: bool,
    quantize: bool,
    modulation: usize,
    polyphony: usize,
    pb: f64,
    freq: &'a [f32],
    sd: &'a [f32],
    transp: &'a [f32],
    stretch: &'a [f32],
    shift: &'a [f32],
    decay: &'a [f32],
    hf: &'a [f32],
    lf: &'a [f32],
    harmonics: &'a [f32],
    mod_rate: &'a [f32],
    pch_mod: &'a [f32],
    unison: &'a [f32],
    uni_amt: &'a [f32],
    send: &'a [f32],
    wet: &'a [f32],
    glide: &'a [f32],
}

impl Params<'_> {
    /// Transp. at frame `i`, in semitones or in degrees.
    fn transp(&self, i: usize) -> i32 {
        let most = if self.use_scale {
            MOST_DEGREES
        } else {
            MOST_SEMITONES
        };
        (self.transp[i].round() as i32).clamp(-most, most)
    }
}

/// One partial of one voice.
#[derive(Clone, Copy)]
struct Partial {
    /// Its frequency before modulation and Unison, 0 where it sounds not.
    hz: f64,
    /// Where each copy of it is in its cycle, and how far it moves a frame,
    /// both in 2^-32 of a cycle.
    phase: [u32; COPIES],
    inc: [u32; COPIES],
    /// What the analysis brings at `hz`, what it came to once followed, and
    /// what rings on.
    drive: f32,
    seen: f32,
    ring: f32,
    /// A granular gate's state and where it is heading.
    gate: f32,
    gate_target: f32,
    /// How much of its level sounds (the odd partials at Stretch).
    weight: f32,
    /// The ring's fall over a tick, and what it was worked out for: the
    /// frequency, Decay and the two damps.
    decay: f32,
    decay_for: [f64; 4],
    /// The amplitude it sounds at, the one it reaches at the tick's end and
    /// the step between.
    amp: f32,
    target: f32,
    step: f32,
    /// Its own place in the modulation's cycle, from 0 to 1 (below 0 until
    /// it is drawn), the grain it holds the draws of (−1 for none), and the
    /// draws: the two ends of Wander's line, Granular's gate and its scatter.
    own: f64,
    grain: i64,
    draws: [f64; 4],
}

impl Partial {
    const SILENT: Self = Self {
        hz: 0.0,
        phase: [0; COPIES],
        inc: [0; COPIES],
        drive: 0.0,
        seen: 0.0,
        ring: 0.0,
        gate: 1.0,
        gate_target: 1.0,
        weight: 0.0,
        decay: 1.0,
        decay_for: [f64::NAN; 4],
        amp: 0.0,
        target: 0.0,
        step: 0.0,
        own: -1.0,
        grain: -1,
        draws: [0.0; 4],
    };

    /// Forgets what rings, but not where the oscillators are.
    fn clear(&mut self) {
        *self = Self {
            phase: self.phase,
            ..Self::SILENT
        };
    }
}

/// One voice of the pool.
#[derive(Clone, Copy)]
struct Voice {
    /// Poly's note on it, if it has had one, and whether it is held.
    key: Option<u8>,
    held: bool,
    /// When the note began, for the oldest to be taken.
    age: u64,
    /// Whether the input drives it now, and whether anything of it can sound.
    inject: bool,
    live: bool,
    /// The pitch it was last driven at, in semitones from A3 = 440 Hz at 69,
    /// which it rings out at once nothing drives it.
    pitch: Option<f64>,
    /// Its fundamental now, in Hz, for the display.
    f0: f64,
}

impl Voice {
    const IDLE: Self = Self {
        key: None,
        held: false,
        age: 0,
        inject: false,
        live: false,
        pitch: None,
        f0: 0.0,
    };
}

/// Mono's notes: the keys held, the newest last, and the glide to the
/// newest.
struct Mono {
    held: [u8; HELD_KEYS],
    len: usize,
    /// Whether a note has played since the node began.
    played: bool,
    /// The glide: from a key to a key from `start` (a frame count of the
    /// node) over `length` frames.
    from: f64,
    to: f64,
    start: u64,
    length: f64,
}

impl Mono {
    fn new() -> Self {
        Self {
            held: [0; HELD_KEYS],
            len: 0,
            played: false,
            from: f64::from(C3),
            to: f64::from(C3),
            start: 0,
            length: 0.0,
        }
    }

    /// The key it sits at, a fraction of a key included, at frame `at`.
    fn position(&self, at: u64) -> f64 {
        let t = at.saturating_sub(self.start) as f64;
        if self.length < 1.0 || t >= self.length {
            self.to
        } else {
            self.from + (self.to - self.from) * t / self.length
        }
    }

    /// Starts gliding to `key` at frame `at`, over `frames`.
    fn go(&mut self, key: u8, at: u64, frames: f64) {
        self.from = self.position(at);
        self.to = f64::from(key);
        self.start = at;
        self.length = frames.round().max(0.0);
    }

    fn press(&mut self, key: u8, at: u64, frames: f64) {
        if self.len == HELD_KEYS {
            self.held.copy_within(1.., 0);
            self.len -= 1;
        }
        self.held[self.len] = key;
        self.len += 1;
        // The first note has no earlier one to glide from.
        let frames = if self.played { frames } else { 0.0 };
        self.played = true;
        self.go(key, at, frames);
    }

    fn release(&mut self, key: u8, at: u64, frames: f64) {
        let Some(place) = self.held[..self.len].iter().rposition(|&k| k == key) else {
            return;
        };
        self.held.copy_within(place + 1..self.len, place);
        self.len -= 1;
        // The newest note still held takes over; with none, the pitch stays.
        if place == self.len && self.len > 0 {
            self.go(self.held[self.len - 1], at, frames);
        }
    }
}

/// Spectral Resonator's audio-thread half.
struct SpectralResonator {
    rate: f64,
    seed: u64,
    /// The analysis of the input, the magnitudes of its newest frame (scaled
    /// to the amplitude of a sine on a bin) and the factor that scales them.
    stft: Stft,
    mags: Vec<f32>,
    /// The analysis of the wet output for the spectrogram, the bins each row
    /// covers, and the columns: a ring whose next write is its oldest.
    wet_stft: Stft,
    rows: Vec<(usize, usize)>,
    grid: Vec<f32>,
    head: usize,
    column: Vec<f32>,
    column_left: i64,
    column_length: i64,
    /// The delay that lines the dry signal up with the analysis.
    dry: [DelayLine; 2],
    latency: usize,
    voices: [Voice; VOICES],
    partials: Vec<Partial>,
    mono: Mono,
    /// The channels' pitch bend, from −1 to 1.
    bend: f64,
    next_age: u64,
    keys: KeyPitches,
    keys_version: u64,
    /// The notes that are allowed for Quantize, as the pitches they have, and
    /// what they were last built from.
    allowed: [f64; 128],
    allowed_len: usize,
    built: Option<(Scale, u64)>,
    /// The scale of the last tick or note.
    scale_seen: Option<Scale>,
    /// The sine, one cycle with a closing point.
    table: Vec<f32>,
    /// k to the power 1 + s for each partial number, and the Stretch it was
    /// made for.
    law: [f64; PARTIALS],
    law_for: f64,
    /// What the two analyses scale a bin's magnitude by to read the amplitude
    /// of a sine on it.
    scale: f32,
    wet_scale: f32,
    /// Frames played since the node began, and the modulation's place in its
    /// cycles.
    pos: u64,
    mod_phase: f64,
    /// Unison's gains for the left and the right, per copy.
    pans: [(f32, f32); COPIES],
    copies: usize,
    /// What the spectrum is moved by, as a ratio.
    ratio: f64,
    follow: f32,
    gate_follow: f32,
    limit: f32,
    release: f32,
    lamp_left: u32,
    events: Vec<NoteEvent>,
}

impl SpectralResonator {
    fn new(device: &Device, config: &NodeConfig) -> Self {
        let rate = config.sample_rate.as_f64();
        let size = frame_size(rate);
        let new_stft = || Stft::new(size, OVERLAP, Window::Hann).expect("a frame the STFT offers");
        let stft = new_stft();
        let wet_stft = new_stft();
        let hop = stft.hop();
        let latency = stft.latency();
        let (scale, wet_scale) = (stft.amplitude_scale(), wet_stft.amplitude_scale());
        let column_length = (rate * HISTORY_SECONDS / GRID.columns as f64).round() as i64;
        Self {
            rate,
            seed: device.id.0.get(),
            mags: vec![0.0; stft.bins()],
            rows: stft::row_bins(
                GRID.rows,
                GRID.frequencies[0],
                GRID.frequencies[1],
                GRID.log_frequency,
                size,
                rate,
            ),
            grid: vec![GRID.levels[0]; GRID.columns * GRID.rows],
            head: 0,
            column: vec![GRID.levels[0]; GRID.rows],
            column_left: column_length,
            column_length,
            dry: [DelayLine::new(latency), DelayLine::new(latency)],
            latency,
            stft,
            wet_stft,
            voices: [Voice::IDLE; VOICES],
            partials: vec![Partial::SILENT; VOICES * PARTIALS],
            mono: Mono::new(),
            bend: 0.0,
            next_age: 1,
            keys: KeyPitches::twelve(),
            keys_version: 0,
            allowed: [0.0; 128],
            allowed_len: 0,
            built: None,
            scale_seen: None,
            table: (0..=TABLE)
                .map(|n| (TAU * n as f64 / TABLE as f64).sin() as f32)
                .collect(),
            law: [1.0; PARTIALS],
            law_for: f64::NAN,
            scale,
            wet_scale,
            pos: 0,
            mod_phase: 0.0,
            pans: [(0.0, 0.0); COPIES],
            copies: 1,
            ratio: 1.0,
            follow: (1.0 - (-(TICK as f64) / (hop as f64 * FOLLOW_HOPS)).exp()) as f32,
            gate_follow: (1.0 - (-(TICK as f64) / (GATE_SECONDS * rate)).exp()) as f32,
            limit: 1.0,
            release: (1.0 - (-1.0 / (LIMITER_RELEASE * rate)).exp()) as f32,
            lamp_left: 0,
            events: Vec::with_capacity(EVENT_ROOM),
        }
    }

    /// The pitch of the key `position` (a fraction of a key included) moved
    /// by Transp. and the bend, under the tuning, in semitones from A3 = 440
    /// Hz at 69 (R6, R8).
    fn key_pitch(&self, position: f64, p: &Params<'_>, scale: Scale, i: usize) -> Option<f64> {
        let key = position.floor();
        let frac = position - key;
        let key = key as i32;
        let moved = if p.use_scale {
            scale.move_off(key.clamp(0, 127) as u8, p.transp(i))
        } else {
            key + p.transp(i)
        }
        .clamp(0, 127);
        self.keys.bent(moved as u8, frac + self.bend * p.pb)
    }

    /// Internal mode's pitch (R5).
    fn internal_pitch(&self, p: &Params<'_>, scale: Scale, i: usize) -> Option<f64> {
        if p.use_scale {
            let root = C3 + i32::from(scale.root);
            let degrees = (p.sd[i].round() as i32).clamp(-MOST_DEGREES, MOST_DEGREES) + p.transp(i);
            let key = scale.move_off(root as u8, degrees).clamp(0, 127);
            self.keys.pitch(key as u8)
        } else {
            Some(semitones_of(f64::from(p.freq[i])) + f64::from(p.transp(i)))
        }
    }

    /// The pitch voice `v` is driven at, if something drives it.
    fn pitch_of(&self, v: usize, p: &Params<'_>, scale: Scale, i: usize) -> Option<f64> {
        if v == 0 && p.internal {
            self.internal_pitch(p, scale, i)
        } else if v == 0 && !p.poly {
            self.mono
                .played
                .then(|| self.key_pitch(self.mono.position(self.pos), p, scale, i))
                .flatten()
        } else {
            self.voices[v]
                .key
                .and_then(|key| self.key_pitch(f64::from(key), p, scale, i))
        }
    }

    /// Whether the input drives voice `v` now (R6).
    fn injects(&self, v: usize, p: &Params<'_>) -> bool {
        if p.internal {
            v == 0
        } else if p.poly {
            v < p.polyphony && self.voices[v].held
        } else {
            v == 0 && (self.mono.len > 0 || (!p.gate && self.mono.played))
        }
    }

    /// Takes a note or message of the source on the frame the node is at.
    fn note(&mut self, note: &NoteEvent, p: &Params<'_>, i: usize) {
        let glide = f64::from(p.glide[i]) / 1_000.0 * self.rate;
        match note.kind {
            NoteKind::On => {
                self.mono.press(note.key, self.pos, glide);
                self.take_voice(note.key, p.polyphony);
            }
            NoteKind::Off | NoteKind::Choke => {
                self.mono.release(note.key, self.pos, glide);
                let oldest = (0..VOICES)
                    .filter(|&v| self.voices[v].held && self.voices[v].key == Some(note.key))
                    .min_by_key(|&v| self.voices[v].age);
                if let Some(v) = oldest {
                    self.voices[v].held = false;
                }
            }
            NoteKind::PitchBend { value } => self.bend = f64::from(value).clamp(-1.0, 1.0),
            _ => {}
        }
    }

    /// Gives `key` a voice of the first `polyphony`: a silent one, else the
    /// oldest that is let go and rings, else the oldest held (R6).
    fn take_voice(&mut self, key: u8, polyphony: usize) {
        let voices = &self.voices;
        let chosen = (0..polyphony)
            .find(|&v| !voices[v].held && !voices[v].live)
            .or_else(|| {
                (0..polyphony)
                    .filter(|&v| !voices[v].held)
                    .min_by_key(|&v| voices[v].age)
            })
            .or_else(|| (0..polyphony).min_by_key(|&v| voices[v].age));
        let Some(v) = chosen else { return };
        for partial in &mut self.partials[v * PARTIALS..(v + 1) * PARTIALS] {
            partial.clear();
        }
        self.voices[v] = Voice {
            key: Some(key),
            held: true,
            age: self.next_age,
            live: true,
            ..self.voices[v]
        };
        self.next_age += 1;
    }

    /// Makes the pitches Quantize can move a partial to: the scale's notes
    /// under the tuning, or every note of the tuning (R7, R8).
    fn allow(&mut self, scale: Scale) {
        if self.built == Some((scale, self.keys_version)) {
            return;
        }
        self.built = Some((scale, self.keys_version));
        self.allowed_len = 0;
        for key in 0..128u8 {
            if let Some(pitch) = self.keys.pitch(key)
                && scale.holds(key)
            {
                self.allowed[self.allowed_len] = pitch;
                self.allowed_len += 1;
            }
        }
    }

    /// The allowed pitch nearest `pitch`, the lower of two as near; a pitch
    /// beyond the allowed ones stays.
    fn quantize(&self, pitch: f64) -> f64 {
        let allowed = &self.allowed[..self.allowed_len];
        let (Some(&first), Some(&last)) = (allowed.first(), allowed.last()) else {
            return pitch;
        };
        if pitch < first || pitch > last {
            return pitch;
        }
        let above = allowed
            .partition_point(|&a| a < pitch)
            .min(allowed.len() - 1);
        let below = above.saturating_sub(1);
        if pitch - allowed[below] <= allowed[above] - pitch {
            allowed[below]
        } else {
            allowed[above]
        }
    }

    /// Works out every sounding voice's pitch, its partials' frequencies,
    /// decays and weights, the modulation, Unison's detune and what the
    /// analysis brings, for the frame `i` the node is at (R2, R3, R5 to R10).
    fn tune(&mut self, p: &Params<'_>, scale: Scale, hidden: bool, i: usize) {
        let nyquist = self.rate / 2.0;
        let unison = (p.unison[i].round() as usize).clamp(1, COPIES);
        self.copies = unison;
        let spread = f64::from(p.uni_amt[i]) / 100.0 * 50.0;
        let place = |c: usize| {
            if unison == 1 {
                0.0
            } else {
                2.0 * c as f64 / (unison - 1) as f64 - 1.0
            }
        };
        let mut detune = [1.0f64; COPIES];
        for (c, factor) in detune.iter_mut().enumerate().take(unison) {
            let angle = (place(c) + 1.0) * FRAC_PI_4;
            let norm = 1.0 / (unison as f64).sqrt();
            self.pans[c] = ((angle.cos() * norm) as f32, (angle.sin() * norm) as f32);
            *factor = (place(c) * spread / 1_200.0).exp2();
        }
        let stretch = f64::from(p.stretch[i]) / 100.0;
        if self.law_for != stretch {
            self.law_for = stretch;
            for (k, law) in self.law.iter_mut().enumerate() {
                *law = ((k + 1) as f64).powf(1.0 + stretch);
            }
        }
        let count = (p.harmonics[i].round() as usize).clamp(1, PARTIALS);
        // Spec: spectral-resonator R2. Squeezing the partials together stacks
        // them on one another, so each is lowered by the square root of their
        // number as Stretch reaches -100 %, and the sum stays as loud.
        let squeeze = (count as f64).powf((stretch.min(0.0)) / 2.0) as f32;
        let (hf, lf) = (f64::from(p.hf[i]) / 100.0, f64::from(p.lf[i]) / 100.0);
        let decay = f64::from(p.decay[i]).max(1.0e-3);
        let depth = f64::from(p.pch_mod[i]);
        self.ratio = (f64::from(p.shift[i]) / 12.0).exp2();
        if p.quantize {
            self.allow(if hidden { Scale::default() } else { scale });
        }
        let top = detune[unison - 1].max(1.0);
        let (seed, mod_phase, rate) = (self.seed, self.mod_phase, self.rate);
        for v in 0..VOICES {
            let inject = self.injects(v, p);
            let pitch = self.pitch_of(v, p, scale, i);
            let mut voice = self.voices[v];
            voice.inject = inject;
            if pitch.is_some() {
                voice.pitch = pitch;
            }
            voice.live |= inject;
            let Some(pitch) = voice.pitch.filter(|_| voice.live) else {
                voice.live = false;
                self.voices[v] = voice;
                continue;
            };
            let f0 = hz_of(pitch);
            voice.f0 = f0;
            self.voices[v] = voice;
            for k in 0..PARTIALS {
                let at = v * PARTIALS + k;
                if k >= count {
                    self.partials[at].clear();
                    continue;
                }
                let mut hz = f0 * self.law[k];
                if p.quantize && hz > 0.0 {
                    hz = hz_of(self.quantize(semitones_of(hz)));
                }
                let partial = &mut self.partials[at];
                // Spec: spectral-resonator R9. Each partial's own place in
                // the modulation's cycle, and what it does there.
                let (semitones, gate) = match p.modulation {
                    1 => {
                        let cycle = mod_phase + (((k + 1) as f64) * GOLDEN).fract();
                        ((TAU * cycle).sin() * depth, 1.0)
                    }
                    2 | 3 => {
                        if partial.own < 0.0 {
                            partial.own = (draw(seed, v, k, 0, 0) + 1.0) / 2.0;
                        }
                        let cycle = mod_phase + partial.own;
                        let n = cycle.floor() as i64;
                        if partial.grain != n {
                            partial.grain = n;
                            partial.draws = [
                                draw(seed, v, k, n, 1),
                                draw(seed, v, k, n + 1, 1),
                                draw(seed, v, k, n, 2),
                                draw(seed, v, k, n, 3),
                            ];
                        }
                        let [a, b, on, scatter] = partial.draws;
                        if p.modulation == 2 {
                            ((a + (b - a) * (cycle - n as f64)) * depth, 1.0)
                        } else {
                            (scatter * depth, if on < 0.0 { 1.0 } else { 0.0 })
                        }
                    }
                    _ => (0.0, 1.0),
                };
                let moved = if semitones == 0.0 {
                    hz
                } else {
                    hz * (semitones / 12.0).exp2()
                };
                if hz <= 0.0 || !hz.is_finite() || moved * top >= nyquist {
                    partial.clear();
                    continue;
                }
                partial.hz = hz;
                // Spec: spectral-resonator R2. The even partials give way as
                // Stretch passes 0, and are gone at +100 %.
                partial.weight = if stretch > 0.0 && k % 2 == 1 {
                    (1.0 - stretch) as f32
                } else {
                    1.0
                } * squeeze;
                partial.gate_target = gate;
                let key = [hz, decay, hf, lf];
                if partial.decay_for != key {
                    partial.decay_for = key;
                    partial.decay =
                        10f64.powf(-3.0 * TICK as f64 / (decay / speed(hz, hf, lf) * rate)) as f32;
                }
                for (inc, factor) in partial.inc.iter_mut().zip(&detune).take(unison) {
                    *inc = (moved * factor / rate * PHASE_ONE) as u32;
                }
                partial.drive = if inject {
                    read(&self.mags, hz / self.ratio, rate)
                } else {
                    0.0
                };
            }
        }
    }

    /// Takes one tick of the levels: the partials follow the analysis, ring on
    /// and gate, and aim at the amplitude each reaches by the tick's end (R3).
    fn envelope(&mut self) {
        for v in 0..VOICES {
            let voice = &mut self.voices[v];
            if !voice.live {
                continue;
            }
            let mut sounds = voice.inject;
            for partial in &mut self.partials[v * PARTIALS..(v + 1) * PARTIALS] {
                let heard = if voice.inject { partial.drive } else { 0.0 };
                partial.seen += (heard - partial.seen) * self.follow;
                partial.ring = (partial.ring * partial.decay).max(partial.seen);
                if partial.ring < FLOOR {
                    partial.ring = 0.0;
                }
                partial.gate += (partial.gate_target - partial.gate) * self.gate_follow;
                partial.amp = partial.target;
                let target = partial.ring * partial.gate * partial.weight;
                partial.target = if target < FLOOR { 0.0 } else { target };
                partial.step = (partial.target - partial.amp) / TICK as f32;
                if partial.amp < FLOOR && partial.target == 0.0 {
                    partial.amp = 0.0;
                    partial.step = 0.0;
                }
                sounds |= partial.ring > 0.0 || partial.amp > 0.0 || partial.target > 0.0;
            }
            voice.live = sounds;
            if !sounds {
                voice.f0 = 0.0;
            }
        }
    }

    /// Plays `n` frames from frame `at` of the stretch: the input through the
    /// limiter into the analysis, the partials, the spectrogram's analysis
    /// of what they make, and the mix.
    fn render(&mut self, audio: &mut Audio<'_, '_>, p: &Params<'_>, at: usize, n: usize) {
        let channels = audio.channels().min(2);
        let mut dry = [[0.0f32; TICK]; 2];
        for s in 0..n {
            for (ch, line) in dry.iter_mut().enumerate().take(channels) {
                line[s] = audio.channel(ch)[at + s];
            }
            let heard = if channels > 1 {
                (dry[0][s] + dry[1][s]) / 2.0
            } else {
                dry[0][s]
            } * p.send[at + s];
            // The limiter acts at once and lets go slowly, so the analysis
            // never sees more than 0 dBFS.
            let need = if heard.abs() > 1.0 {
                1.0 / heard.abs()
            } else {
                1.0
            };
            if need < self.limit {
                self.limit = need;
                self.lamp_left = (LAMP_SECONDS * self.rate) as u32;
            } else {
                self.limit += (1.0 - self.limit) * self.release;
            }
            self.lamp_left = self.lamp_left.saturating_sub(1);
            let Self {
                stft, mags, scale, ..
            } = self;
            let scale = *scale;
            stft.process_sample(heard * self.limit, |spectrum: &mut [Complex32]| {
                for (mag, bin) in mags.iter_mut().zip(spectrum.iter()) {
                    *mag = bin.norm() * scale;
                }
            });
        }
        let (mut left, mut right) = ([0.0f32; TICK], [0.0f32; TICK]);
        for v in 0..VOICES {
            if !self.voices[v].live {
                continue;
            }
            for partial in &mut self.partials[v * PARTIALS..(v + 1) * PARTIALS] {
                if partial.amp == 0.0 && partial.step == 0.0 {
                    continue;
                }
                let mut amps = [0.0f32; TICK];
                let mut amp = partial.amp;
                for slot in amps.iter_mut().take(n) {
                    amp += partial.step;
                    *slot = amp;
                }
                for c in 0..self.copies {
                    let (to_left, to_right) = self.pans[c];
                    let (inc, mut phase) = (partial.inc[c], partial.phase[c]);
                    for (s, &level) in amps.iter().enumerate().take(n) {
                        // The top bits of the phase pick a place in the table,
                        // the rest a fraction of the way to the next.
                        phase = phase.wrapping_add(inc);
                        let whole = (phase >> (32 - TABLE.ilog2())) as usize;
                        let part = (phase << TABLE.ilog2()) as f32 / PHASE_ONE as f32;
                        let (a, b) = (self.table[whole], self.table[whole + 1]);
                        let y = (a + (b - a) * part) * level;
                        left[s] += y * to_left;
                        right[s] += y * to_right;
                    }
                    partial.phase[c] = phase;
                }
                partial.amp = amp;
            }
        }
        for s in 0..n {
            let Self {
                wet_stft,
                wet_scale,
                rows,
                column,
                column_left,
                column_length,
                grid,
                head,
                ..
            } = self;
            let (hop, wet_scale) = (wet_stft.hop() as i64, *wet_scale);
            wet_stft.process_sample(
                (left[s] + right[s]) / 2.0,
                |spectrum: &mut [Complex32]| {
                    for (level, &(from, to)) in column.iter_mut().zip(rows.iter()) {
                        let loudest = spectrum[from..=to]
                            .iter()
                            .map(|bin| bin.norm())
                            .fold(0.0, f32::max);
                        *level = level.max(20.0 * (loudest * wet_scale).max(FLOOR).log10());
                    }
                    *column_left -= hop;
                    if *column_left <= 0 {
                        *column_left += *column_length;
                        grid[*head * GRID.rows..][..GRID.rows].copy_from_slice(column);
                        *head = (*head + 1) % GRID.columns;
                        column.fill(GRID.levels[0]);
                    }
                },
            );
            let wet = f64::from(p.wet[at + s]) / 100.0;
            for (ch, input) in dry.iter().enumerate().take(channels) {
                let resonance = if channels == 1 {
                    left[s] + right[s]
                } else if ch == 0 {
                    left[s]
                } else {
                    right[s]
                };
                let line = &mut self.dry[ch];
                let delayed = line.read(self.latency);
                line.write(input[s]);
                audio.channel_mut(ch)[at + s] =
                    (f64::from(delayed) * (1.0 - wet) + f64::from(resonance) * wet) as f32;
            }
        }
    }
}

/// A random value from −1 to just below 1 for partial `k` of voice `v` in its
/// `n`th grain, for `salt`'s use: it follows the device's `seed` alone, so it
/// doesn't depend on the order partials are asked in.
fn draw(seed: u64, v: usize, k: usize, n: i64, salt: u64) -> f64 {
    let key = seed
        ^ ((v as u64) << 56)
        ^ ((k as u64) << 48)
        ^ (salt << 40)
        ^ (n as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    Random::seeded(key).next_bipolar()
}

/// What the analysis brings at `hz`: the larger of the two bins around it.
fn read(mags: &[f32], hz: f64, rate: f64) -> f32 {
    let size = (mags.len() - 1) * 2;
    let bin = hz * size as f64 / rate;
    if !(0.5..(mags.len() - 1) as f64).contains(&bin) {
        return 0.0;
    }
    let low = bin.floor() as usize;
    mags[low].max(mags[low + 1])
}

impl NativeDevice for SpectralResonator {
    fn receive_data(&mut self, data: NodeData) {
        if let Some(keys) = data.downcast_ref::<KeyPitches>() {
            self.keys = *keys;
            self.keys_version += 1;
        }
    }

    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        let frames = audio.frames();
        if frames == 0 || audio.channels() == 0 {
            return;
        }
        let ix: [usize; PARAMS] = core::array::from_fn(|id| {
            params
                .index(ParamId(id as u32))
                .expect("the description lists every parameter")
        });
        let ramp = |id: ParamId| params.ramp(ix[id.0 as usize]);
        let choice = |id: ParamId| params.value(ix[id.0 as usize]) as usize;
        let on = |id: ParamId| params.value(ix[id.0 as usize]) >= 0.5;
        let hidden = audio.scales_hidden();
        let p = Params {
            internal: choice(PITCH_MODE) == 0,
            poly: choice(MONO_POLY) == 1,
            use_scale: use_current_scale(audio, params, USE_SCALE),
            gate: on(MIDI_GATE),
            quantize: on(QUANTIZE),
            modulation: choice(MODULATION).min(MODULATIONS.len() - 1),
            polyphony: (params.value(ix[POLYPHONY.0 as usize]).round() as usize).clamp(2, VOICES),
            pb: params.target(ix[PB.0 as usize]),
            freq: ramp(FREQ),
            sd: ramp(SD_SHIFT),
            transp: ramp(TRANSP),
            stretch: ramp(STRETCH),
            shift: ramp(SHIFT),
            decay: ramp(DECAY),
            hf: ramp(HF_DAMP),
            lf: ramp(LF_DAMP),
            harmonics: ramp(HARMONICS),
            mod_rate: ramp(MOD_RATE),
            pch_mod: ramp(PCH_MOD),
            unison: ramp(UNISON),
            uni_amt: ramp(UNI_AMT),
            send: ramp(INPUT_SEND),
            wet: ramp(DRY_WET),
            glide: ramp(GLIDE),
        };
        self.events.clear();
        for note in audio.notes() {
            if self.events.len() < self.events.capacity() {
                self.events.push(note);
            }
        }
        let watches_scale = p.use_scale || p.quantize;
        let mut next = 0;
        let mut i = 0;
        while i < frames {
            let mut moved = false;
            while let Some(&note) = self.events.get(next).filter(|n| n.offset as usize <= i) {
                self.note(&note, &p, i);
                next += 1;
                moved = true;
            }
            let scale = audio.scale_at(i as u32);
            if watches_scale && self.scale_seen != Some(scale) {
                moved = true;
            }
            self.scale_seen = Some(scale);
            let boundary = self.pos.is_multiple_of(TICK as u64);
            if boundary {
                self.mod_phase += f64::from(p.mod_rate[i]) / self.rate * TICK as f64;
            }
            if boundary || moved {
                self.tune(&p, scale, hidden, i);
            }
            if boundary {
                self.envelope();
            }
            // The chunk ends at the tick's end, the next note, a change of
            // scale or the stretch's.
            let mut n = (TICK - (self.pos % TICK as u64) as usize).min(frames - i);
            if let Some(note) = self.events.get(next) {
                n = n.min((note.offset as usize).saturating_sub(i).max(1));
            }
            if watches_scale {
                n = (1..n)
                    .find(|&s| audio.scale_at((i + s) as u32) != scale)
                    .unwrap_or(n);
            }
            self.render(audio, &p, i, n);
            i += n;
            self.pos += n as u64;
        }
    }

    fn latency(&self) -> u32 {
        self.latency as u32
    }

    fn display_len(&self) -> usize {
        DISPLAY_LEN
    }

    fn display(&mut self, out: &mut [f32]) {
        let columns = GRID.columns * GRID.rows;
        // The spectrogram, oldest column first.
        let (newer, older) = self.grid.split_at(self.head * GRID.rows);
        out[..older.len()].copy_from_slice(older);
        out[older.len()..columns].copy_from_slice(newer);
        out[columns] = f32::from(u8::from(self.lamp_left > 0));
        let fundamentals = &mut out[columns + 1..columns + 1 + VOICES];
        for (slot, voice) in fundamentals.iter_mut().zip(&self.voices) {
            *slot = if voice.live { voice.f0 as f32 } else { 0.0 };
        }
        let marks = &mut out[columns + 1 + VOICES..];
        for (v, voice) in self.voices.iter().enumerate() {
            for (k, partial) in self.partials[v * PARTIALS..(v + 1) * PARTIALS]
                .iter()
                .enumerate()
            {
                marks[v * PARTIALS + k] = if voice.live { partial.hz as f32 } else { 0.0 };
            }
        }
    }
}

/// What the display shows, from the `data` its node published: the
/// spectrogram of the wet output, each sounding voice's partials marked, the
/// limiter's lamp and the voices' fundamentals (R14, R15).
#[must_use]
pub fn display(
    device: DeviceId,
    description: &DeviceDescription,
    data: Option<&[f32]>,
) -> DeviceDisplay {
    let grid = description.spectrogram.unwrap_or(GRID);
    let cells = grid.columns * grid.rows;
    let data = data.filter(|d| d.len() == DISPLAY_LEN);
    let spectrograms = data
        .and_then(|d| DisplaySpectrogram::from_data(SPECTROGRAM, grid, &d[..cells]))
        .into_iter()
        .collect();
    let mut readings = Vec::new();
    let mut markers = Vec::new();
    if let Some(data) = data {
        let lit = data[cells] >= 0.5;
        readings.push(DisplayReading {
            name: LIMITER.to_owned(),
            value: f32::from(u8::from(lit)),
            unit: String::new(),
            text: if lit { "on" } else { "off" }.to_owned(),
        });
        let fundamentals = &data[cells + 1..cells + 1 + VOICES];
        let partials = &data[cells + 1 + VOICES..];
        for (v, &hz) in fundamentals.iter().enumerate().filter(|(_, hz)| **hz > 0.0) {
            readings.push(DisplayReading {
                name: format!("Voice {} Fundamental", v + 1),
                value: hz,
                unit: "Hz".to_owned(),
                text: format!("{hz:.1} Hz"),
            });
            for (k, &at) in partials[v * PARTIALS..(v + 1) * PARTIALS]
                .iter()
                .enumerate()
                .filter(|(_, at)| **at > 0.0)
            {
                markers.push(DisplayMarker {
                    name: format!("Voice {} Partial {}", v + 1, k + 1),
                    series: PARTIAL_SERIES.to_owned(),
                    x: at,
                    y: MARKER_HEIGHT,
                    text: format!("Voice {} partial {} {at:.1} Hz", v + 1, k + 1),
                    handle: None,
                });
            }
        }
    }
    DeviceDisplay {
        device,
        live: data.is_some(),
        view: None,
        series: vec![DisplaySeries {
            name: PARTIAL_SERIES.to_owned(),
            unit: String::new(),
            x_unit: "Hz".to_owned(),
            x_range: grid.frequencies,
            log_x: grid.log_frequency,
            y_range: [0.0, 1.0],
            detail: false,
            values: Vec::new(),
        }],
        spectrograms,
        markers,
        readings,
    }
}

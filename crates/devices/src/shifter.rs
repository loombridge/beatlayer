//! Shifter, the native Shifter (WP-5.27; the pitch-and-frequency-shifter
//! spec): a pitch shifter, a frequency shifter and a ring modulator in one
//! device, with Spread and Wide for the stereo image, a feedback delay
//! (free in Hz or synced to the tempo) with a Tone filter in its feedback
//! path, an LFO and an envelope follower moving the shift, and a MIDI
//! sidechain that sets the pitch from the notes of a MIDI track.
//!
//! Pitch mode is a pair of read heads moving through a delay line, crossfaded
//! as each runs out of line; Freq mode is single-sideband modulation, with a
//! windowed FIR Hilbert transformer for the quadrature path; Ring mode
//! multiplies the input by a sine. All three delay the dry signal by the
//! same fixed latency, which the device reports, so Dry/Wet mixes signals
//! that line up. The shared parts are the framework's: [`Lfo`] and
//! [`EnvelopeFollower`].
//!
//! The manual records no ranges, defaults or algorithms, so those are
//! Beatlayer's own (see `docs/specs/pitch-and-frequency-shifter.md`).

use core::f64::consts::{PI, TAU};

use bl_dsp::EnvelopeFollower;
use bl_engine::{DeviceType, NodeConfig, NoteEvent, NoteKind, Registry};
use bl_model::device_layout::{BODY_HEIGHT, ControlBand, LayoutNode, Widget};
use bl_model::{
    Device, DeviceDescription, DeviceLayout, DeviceRole, ParamDescriptor, ParamId, ParamUnit,
};

use crate::auto_filter::DIVISIONS;
use crate::framework::{Audio, NativeDevice, ParamBank, add};
use crate::lfo::{Lfo, Rate, Shape, Stereo, Wave};

/// Shifter's device type.
pub const SHIFTER: &str = "beatlayer.veer";

/// Mode: Pitch, Freq or Ring, one of [`MODES`].
pub const MODE: ParamId = ParamId(0);
/// Coarse: Pitch mode's shift in whole semitones.
pub const COARSE: ParamId = ParamId(1);
/// Fine: Pitch mode's shift in cents, on top of Coarse.
pub const FINE: ParamId = ParamId(2);
/// Window: the length of Pitch mode's grains, in milliseconds.
pub const WINDOW: ParamId = ParamId(3);
/// Shift: Freq mode's shift, and Ring mode's modulation frequency, in Hz.
pub const SHIFT: ParamId = ParamId(4);
/// Drive: distortion on Ring mode's output, in per cent.
pub const DRIVE: ParamId = ParamId(5);
/// Spread: how far the channels are detuned from each other, in per cent.
pub const SPREAD: ParamId = ParamId(6);
/// Wide: inverts Spread for the right channel.
pub const WIDE: ParamId = ParamId(7);
/// Delay: whether the delay is on.
pub const DELAY_ON: ParamId = ParamId(8);
/// Delay Mode: whether its time is free, in Hz, or synced to the tempo.
pub const DELAY_MODE: ParamId = ParamId(9);
/// Delay Time: the free delay time, as a rate in Hz.
pub const DELAY_HZ: ParamId = ParamId(10);
/// Delay Beats: the synced delay time, one of [`DELAY_DIVISIONS`].
pub const DELAY_DIVISION: ParamId = ParamId(11);
/// Feedback: how much of the delay's output goes back into it, in per
/// cent.
pub const FEEDBACK: ParamId = ParamId(12);
/// Tone: how far the feedback path cuts the highs, in per cent.
pub const TONE: ParamId = ParamId(13);
/// Dry/Wet: the balance between the shifted and dry signals, in per cent.
pub const DRY_WET: ParamId = ParamId(14);

/// LFO Amount: how far the LFO moves the shift, in per cent, negative
/// turning the wave over.
pub const LFO_AMOUNT: ParamId = ParamId(15);
/// LFO Wave: one of [`WAVES`].
pub const LFO_WAVE: ParamId = ParamId(16);
/// LFO Mode: whether the rate is in Hz or synced to the tempo.
pub const LFO_MODE: ParamId = ParamId(17);
/// LFO Rate in Hz.
pub const LFO_HZ: ParamId = ParamId(18);
/// LFO Beats: the synced division, one of [`DIVISIONS`].
pub const LFO_DIVISION: ParamId = ParamId(19);
/// LFO Stereo: Phase or Spin.
pub const LFO_STEREO: ParamId = ParamId(20);
/// LFO Phase: degrees the right channel's LFO is ahead.
pub const LFO_PHASE: ParamId = ParamId(21);
/// LFO Spin: per cent the right channel's rate is above the left's.
pub const LFO_SPIN: ParamId = ParamId(22);
/// LFO Offset: degrees against the beat grid, in the synced mode.
pub const LFO_OFFSET: ParamId = ParamId(23);
/// LFO Duty Cycle: squeezes the wave toward the front or back of its
/// cycle, in per cent.
pub const LFO_DUTY: ParamId = ParamId(24);
/// LFO Width: how far apart the channels' Random S&H values are, in per
/// cent.
pub const LFO_WIDTH: ParamId = ParamId(25);

/// Env Fol: whether the envelope follower is on.
pub const ENV_ON: ParamId = ParamId(26);
/// Env Attack, in milliseconds.
pub const ENV_ATTACK: ParamId = ParamId(27);
/// Env Release, in milliseconds.
pub const ENV_RELEASE: ParamId = ParamId(28);
/// Env Amount in Pitch mode, in semitones at a full-scale input.
pub const ENV_SEMITONES: ParamId = ParamId(29);
/// Env Amount in Freq and Ring modes, in Hz at a full-scale input.
pub const ENV_HZ: ParamId = ParamId(30);

/// Pitch Source: Internal (Coarse and Fine) or MIDI, one of [`SOURCES`].
pub const PITCH_SOURCE: ParamId = ParamId(31);
/// Glide: the time a MIDI note takes to slide the pitch to the next, in
/// milliseconds.
pub const GLIDE: ParamId = ParamId(32);
/// PB: the pitch bend range, in semitones.
pub const BEND_RANGE: ParamId = ParamId(33);

/// The shift modes, in the chooser's order.
pub const MODES: [&str; 3] = ["Pitch", "Freq", "Ring"];

/// The delay's modes.
pub const DELAY_MODES: [&str; 2] = ["Hz", "Sync"];

/// The LFO's rate modes.
pub const RATE_MODES: [&str; 2] = ["Hz", "Sync"];

/// The pitch sources.
pub const SOURCES: [&str; 2] = ["Internal", "MIDI"];

/// The LFO's waves, in the chooser's order, under the manual's names.
pub const WAVES: [&str; 10] = [
    "Sine",
    "Triangle",
    "Triangle Analog",
    "Triangle 8",
    "Triangle 16",
    "Saw Up",
    "Saw Down",
    "Rectangle",
    "Random",
    "Random S&H",
];

/// The effects LFO's waves the chooser's [`WAVES`] play, in order.
const WAVE_OF: [Wave; 10] = [
    Wave::Sine,
    Wave::Triangle,
    Wave::TriangleAnalog,
    Wave::Triangle8,
    Wave::Triangle16,
    Wave::SawUp,
    Wave::SawDown,
    Wave::Rectangle,
    Wave::Random,
    Wave::SampleHold,
];

/// The index of Random in [`WAVES`], which uses Phase alone, and of Random
/// S&H, which uses Width alone.
const RANDOM: usize = 8;
const RANDOM_SH: usize = 9;

/// How many of [`DIVISIONS`] the delay offers: 1/32 up to a bar.
pub const DELAY_DIVISION_COUNT: usize = 14;

/// The delay's synced divisions, in the chooser's order.
pub const DELAY_DIVISIONS: &[(&str, f64)] = DIVISIONS.split_at(DELAY_DIVISION_COUNT).0;

/// The division a new device's delay starts on: 1/8.
const DEFAULT_DIVISION: usize = 5;

/// The division a new device's LFO starts on: 1/4.
const DEFAULT_LFO_DIVISION: usize = 8;

/// Where Pitch mode's MIDI source takes its notes from: the key that
/// shifts by nothing, C3.
pub const REFERENCE_KEY: u8 = 60;

/// The latency every mode reports, in seconds.
const LATENCY_SECONDS: f64 = 0.010;

/// How far a full-scale LFO moves the shift at an Amount of 100 %: in
/// semitones in Pitch mode, in Hz in Freq and Ring modes.
const LFO_SEMITONES: f64 = 12.0;
const LFO_HERTZ: f64 = 1_000.0;

/// How far Spread detunes each channel at 100 %: in cents in Pitch mode, in
/// Hz in Freq and Ring modes.
const SPREAD_CENTS: f64 = 50.0;
const SPREAD_HERTZ: f64 = 10.0;

/// The longest the delay can be, in seconds; a longer synced time is cut
/// to it.
const MAX_DELAY_SECONDS: f64 = 4.0;

/// The Kaiser window's shape for the Hilbert transformer.
const KAISER_BETA: f64 = 8.0;

/// How long the delay takes to move to a new time, in seconds.
const DELAY_GLIDE_SECONDS: f64 = 0.02;

/// How long Pitch mode takes to move between the shifted signal and the
/// plain delayed one, as the shift reaches or leaves nothing, in seconds.
const BYPASS_SECONDS: f64 = 0.010;

/// The keys of notes held at once that the MIDI source remembers.
const HELD_KEYS: usize = 16;

/// The most notes and messages one stretch of a block takes.
const EVENT_ROOM: usize = 4_096;

/// Shifter's device type.
#[must_use]
pub fn shifter_type() -> DeviceType {
    let divisions: Vec<&str> = DELAY_DIVISIONS.iter().map(|d| d.0).collect();
    let lfo_divisions: Vec<&str> = DIVISIONS.iter().map(|d| d.0).collect();
    let params = vec![
        ParamDescriptor::choice(MODE, "Mode", &MODES, 0).describe(
            "Pitch shifts the pitch by semitones and cents, Freq shifts every frequency by the \
             same number of Hz, and Ring multiplies the signal by a sine of that many Hz.",
        ),
        ParamDescriptor::integer(COARSE, "Coarse", (-48, 48), 0, ParamUnit::Semitones)
            .describe("Pitch mode's shift in semitones."),
        ParamDescriptor::continuous(FINE, "Fine", (-50.0, 50.0), 0.0, ParamUnit::Cents)
            .describe("Pitch mode's shift in cents, added to Coarse."),
        ParamDescriptor::continuous(WINDOW, "Window", (2.0, 20.0), 10.0, ParamUnit::Milliseconds)
            .describe(
                "Pitch mode's grain length. Longer suits low frequencies and shorter suits high \
             ones.",
            ),
        ParamDescriptor::continuous(SHIFT, "Shift", (-5_000.0, 5_000.0), 0.0, ParamUnit::Hertz)
            .describe(
                "Freq mode's shift, and Ring mode's modulation frequency, in Hz. Small shifts \
                 make phasing and tremolo, large ones dissonant, metallic tones.",
            ),
        ParamDescriptor::continuous(DRIVE, "Drive", (0.0, 100.0), 0.0, ParamUnit::Percent)
            .describe("Distorts Ring mode's output. The other modes ignore it."),
        ParamDescriptor::continuous(SPREAD, "Spread", (0.0, 100.0), 0.0, ParamUnit::Percent)
            .describe(
                "Detunes the channels from each other: the left up and the right down by up to \
                 50 cents in Pitch mode or 10 Hz in Freq and Ring modes.",
            ),
        ParamDescriptor::toggle(WIDE, "Wide", false)
            .describe("Inverts Spread for the right channel. It has no effect at 0 Spread."),
        ParamDescriptor::toggle(DELAY_ON, "Delay", false)
            .describe("Adds repeats of the shifted signal."),
        ParamDescriptor::choice(DELAY_MODE, "Delay Mode", &DELAY_MODES, 0)
            .describe("Whether the delay time is a rate in Hz or a beat division."),
        ParamDescriptor::continuous(
            DELAY_HZ,
            "Delay Time",
            (1.0, 1_000.0),
            5.0,
            ParamUnit::Hertz,
        )
        .logarithmic()
        .describe("The delay's rate in Hz: its time is the reciprocal."),
        ParamDescriptor::choice(DELAY_DIVISION, "Delay Beats", &divisions, DEFAULT_DIVISION)
            .describe("The delay time as a beat division, synced to the tempo."),
        ParamDescriptor::continuous(FEEDBACK, "Feedback", (0.0, 95.0), 0.0, ParamUnit::Percent)
            .describe("How much of the delay's output goes back into its input."),
        ParamDescriptor::continuous(TONE, "Tone", (0.0, 100.0), 0.0, ParamUnit::Percent)
            .describe("Darkens the delay's feedback path by cutting the highs."),
        ParamDescriptor::continuous(DRY_WET, "Dry/Wet", (0.0, 100.0), 100.0, ParamUnit::Percent)
            .describe("The balance between the dry and the shifted signal."),
        ParamDescriptor::continuous(
            LFO_AMOUNT,
            "LFO Amount",
            (-100.0, 100.0),
            0.0,
            ParamUnit::Percent,
        )
        .describe(
            "How far the LFO moves the shift: at 100 %, 12 semitones in Pitch mode or 1,000 Hz \
             in Freq and Ring modes.",
        ),
        ParamDescriptor::choice(LFO_WAVE, "LFO Wave", &WAVES, 0).describe("The LFO's waveform."),
        ParamDescriptor::choice(LFO_MODE, "LFO Mode", &RATE_MODES, 0)
            .describe("Whether the LFO's rate is in Hz or a beat division."),
        ParamDescriptor::continuous(LFO_HZ, "LFO Rate", (0.01, 40.0), 1.0, ParamUnit::Hertz)
            .logarithmic()
            .describe("The LFO's rate in Hz."),
        ParamDescriptor::choice(
            LFO_DIVISION,
            "LFO Beats",
            &lfo_divisions,
            DEFAULT_LFO_DIVISION,
        )
        .describe("The LFO's cycle as a beat division, synced to the tempo."),
        ParamDescriptor::choice(LFO_STEREO, "LFO Stereo", &["Phase", "Spin"], 0)
            .describe("Whether the two channels' LFOs differ in phase or in rate."),
        ParamDescriptor::continuous(LFO_PHASE, "LFO Phase", (0.0, 360.0), 0.0, ParamUnit::None)
            .decimals(0)
            .describe("How many degrees the right channel's LFO is ahead; 180 is opposite."),
        ParamDescriptor::continuous(LFO_SPIN, "LFO Spin", (-50.0, 50.0), 0.0, ParamUnit::Percent)
            .describe("How much faster the right channel's LFO runs than the left's."),
        ParamDescriptor::continuous(
            LFO_OFFSET,
            "LFO Offset",
            (-180.0, 180.0),
            0.0,
            ParamUnit::None,
        )
        .decimals(0)
        .describe("Moves both LFOs against the beat grid, in degrees, in the synced mode."),
        ParamDescriptor::continuous(
            LFO_DUTY,
            "LFO Duty Cycle",
            (-100.0, 100.0),
            0.0,
            ParamUnit::Percent,
        )
        .describe("Squeezes the LFO's wave toward the front or the back of its cycle."),
        ParamDescriptor::continuous(
            LFO_WIDTH,
            "LFO Width",
            (0.0, 100.0),
            0.0,
            ParamUnit::Percent,
        )
        .describe(
            "With Random S&H, how far apart the channels are: at 0 % they match, at 100 % \
                 the left's peaks are the right's valleys.",
        ),
        ParamDescriptor::toggle(ENV_ON, "Env Fol", false)
            .describe("Turns the envelope follower on: the input's level moves the shift."),
        ParamDescriptor::continuous(
            ENV_ATTACK,
            "Env Attack",
            (0.1, 1_000.0),
            10.0,
            ParamUnit::Milliseconds,
        )
        .logarithmic()
        .describe("How fast the envelope follower rises with the input."),
        ParamDescriptor::continuous(
            ENV_RELEASE,
            "Env Release",
            (1.0, 5_000.0),
            100.0,
            ParamUnit::Milliseconds,
        )
        .logarithmic()
        .describe("How fast the envelope follower falls with the input."),
        ParamDescriptor::continuous(
            ENV_SEMITONES,
            "Env Amount",
            (-48.0, 48.0),
            0.0,
            ParamUnit::Semitones,
        )
        .describe("Pitch mode: how many semitones a full-scale input moves the shift."),
        ParamDescriptor::continuous(
            ENV_HZ,
            "Env Amount Hz",
            (-5_000.0, 5_000.0),
            0.0,
            ParamUnit::Hertz,
        )
        .describe("Freq and Ring modes: how many Hz a full-scale input moves the shift."),
        ParamDescriptor::choice(PITCH_SOURCE, "Pitch Source", &SOURCES, 0).describe(
            "Internal shifts by Coarse and Fine. MIDI shifts by the interval of the notes of \
             the device's MIDI source from C3, in Pitch mode.",
        ),
        ParamDescriptor::continuous(GLIDE, "Glide", (0.0, 2_000.0), 0.0, ParamUnit::Milliseconds)
            .describe("How long a MIDI note takes to slide the pitch to its own."),
        ParamDescriptor::integer(BEND_RANGE, "PB", (0, 24), 2, ParamUnit::Semitones)
            .describe("How far the full pitch bend of the MIDI source moves the pitch."),
    ];
    DeviceType::native(SHIFTER, "Shifter", DeviceRole::AudioEffect)
        .with_midi_sidechain()
        .with_description(DeviceDescription::new(params).with_layout(layout()))
}

/// The device's layout: the shift, the delay, the LFO and the envelope
/// follower; the MIDI sidechain's parameters in the expanded part.
fn layout() -> DeviceLayout {
    use Widget::{Chooser, Knob, Toggle};
    let band = |controls: &[(ParamId, Widget)]| {
        LayoutNode::Band(ControlBand::stacked(BODY_HEIGHT, controls))
    };
    DeviceLayout::new(LayoutNode::Row(vec![
        band(&[
            (MODE, Chooser),
            (COARSE, Knob),
            (FINE, Knob),
            (WINDOW, Knob),
            (SHIFT, Knob),
            (DRIVE, Knob),
            (SPREAD, Knob),
            (WIDE, Toggle),
        ]),
        band(&[
            (DELAY_ON, Toggle),
            (DELAY_MODE, Chooser),
            (DELAY_HZ, Knob),
            (DELAY_DIVISION, Chooser),
            (FEEDBACK, Knob),
            (TONE, Knob),
        ]),
        band(&[
            (LFO_WAVE, Chooser),
            (LFO_MODE, Chooser),
            (LFO_AMOUNT, Knob),
            (LFO_HZ, Knob),
            (LFO_DIVISION, Chooser),
            (LFO_STEREO, Chooser),
            (LFO_PHASE, Knob),
            (LFO_SPIN, Knob),
            (LFO_OFFSET, Knob),
            (LFO_DUTY, Knob),
            (LFO_WIDTH, Knob),
        ]),
        band(&[
            (ENV_ON, Toggle),
            (ENV_ATTACK, Knob),
            (ENV_RELEASE, Knob),
            (ENV_SEMITONES, Knob),
            (ENV_HZ, Knob),
            (DRY_WET, Knob),
        ]),
    ]))
    .with_expanded(band(&[
        (PITCH_SOURCE, Chooser),
        (GLIDE, Knob),
        (BEND_RANGE, Knob),
    ]))
}

/// Adds the Shifter device to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, shifter_type(), Shifter::new);
}

/// The zeroth-order modified Bessel function of the first kind, for the
/// Kaiser window.
fn bessel_i0(x: f64) -> f64 {
    let (mut sum, mut term) = (1.0, 1.0);
    let quarter = x * x / 4.0;
    for k in 1..64 {
        term *= quarter / f64::from(k * k);
        sum += term;
        if term < 1e-12 * sum {
            break;
        }
    }
    sum
}

/// The Hilbert transformer's coefficients for a delay of `latency` frames:
/// the response at the odd offsets `1, 3, 5, …` from the centre, windowed.
/// The even offsets are zero and the response is odd-symmetric, so the
/// quadrature path takes half the multiplications.
fn hilbert(latency: usize) -> Vec<f32> {
    let norm = bessel_i0(KAISER_BETA);
    (0..latency.div_ceil(2))
        .map(|j| {
            let k = (2 * j + 1) as f64;
            let reach = k / latency as f64;
            let window = bessel_i0(KAISER_BETA * (1.0 - reach * reach).max(0.0).sqrt()) / norm;
            (2.0 / (PI * k) * window) as f32
        })
        .collect()
}

/// A MIDI source's pitch: the note held last, as semitones from
/// [`REFERENCE_KEY`], gliding to each new one over a time, and the pitch
/// bend.
#[derive(Clone, Debug)]
struct MidiPitch {
    /// The keys held, the newest last.
    held: [u8; HELD_KEYS],
    len: usize,
    target: f64,
    current: f64,
    step: f64,
    left: u32,
    /// The channel's pitch bend, from −1 to 1.
    bend: f64,
}

impl MidiPitch {
    fn new() -> Self {
        Self {
            held: [0; HELD_KEYS],
            len: 0,
            target: 0.0,
            current: 0.0,
            step: 0.0,
            left: 0,
            bend: 0.0,
        }
    }

    /// Takes a note or message of the source; `glide` is the glide time in
    /// frames.
    fn take(&mut self, note: &NoteEvent, glide: f64) {
        match note.kind {
            NoteKind::On => {
                if self.len == HELD_KEYS {
                    self.held.copy_within(1.., 0);
                    self.len -= 1;
                }
                self.held[self.len] = note.key;
                self.len += 1;
                self.go(f64::from(note.key) - f64::from(REFERENCE_KEY), glide);
            }
            NoteKind::Off | NoteKind::Choke => {
                let Some(at) = self.held[..self.len].iter().rposition(|&k| k == note.key) else {
                    return;
                };
                self.held.copy_within(at + 1..self.len, at);
                self.len -= 1;
                // The newest note still held takes over; with none, the
                // pitch stays where it is.
                if at == self.len && self.len > 0 {
                    let key = self.held[self.len - 1];
                    self.go(f64::from(key) - f64::from(REFERENCE_KEY), glide);
                }
            }
            NoteKind::PitchBend { value } => self.bend = f64::from(value).clamp(-1.0, 1.0),
            _ => {}
        }
    }

    fn go(&mut self, target: f64, glide: f64) {
        self.target = target;
        let frames = glide.round().max(0.0);
        if frames < 1.0 {
            self.current = target;
            self.left = 0;
        } else {
            self.left = frames as u32;
            self.step = (target - self.current) / frames;
        }
    }

    /// The pitch for the next frame, in semitones, with the bend over
    /// `range` semitones.
    fn next(&mut self, range: f64) -> f64 {
        if self.left > 0 {
            self.current += self.step;
            self.left -= 1;
            if self.left == 0 {
                self.current = self.target;
            }
        }
        self.current + self.bend * range
    }
}

/// Reads `history` (a ring of a power-of-two length) `delay` frames before
/// its newest sample, interpolating cubically between samples. `delay` is held to
/// 2 up to the ring's length less 4.
fn cubic(history: &[f32], newest: usize, delay: f64) -> f64 {
    let mask = history.len() - 1;
    // A modulated shift can carry a head's delay past the bounds its run
    // was set within; the line holds nothing beyond them.
    let delay = delay.clamp(2.0, (history.len() - 4) as f64);
    let whole = delay.floor();
    let t = delay - whole;
    let i = whole as usize;
    let at = |d: usize| f64::from(history[newest.wrapping_sub(d) & mask]);
    let (p0, p1, p2, p3) = (at(i - 1), at(i), at(i + 1), at(i + 2));
    let a = -0.5 * p0 + 1.5 * p1 - 1.5 * p2 + 0.5 * p3;
    let b = p0 - 2.5 * p1 + 2.0 * p2 - 0.5 * p3;
    let c = -0.5 * p0 + 0.5 * p2;
    ((a * t + b) * t + c) * t + p1
}

/// One of Pitch mode's read heads: how far through its run it is, from 0 to
/// 1, and how many frames before the newest the line is read.
#[derive(Clone, Copy, Debug)]
struct Head {
    life: f64,
    delay: f64,
}

impl Head {
    /// Where the heads wait while Pitch mode plays the plain delayed signal:
    /// the first at the middle of its run, at the latency, and the second
    /// not begun.
    fn parked(latency: usize) -> [Head; 2] {
        let delay = latency as f64;
        [Head { life: 0.5, delay }, Head { life: 0.0, delay }]
    }
}

/// One channel's state.
struct Chan {
    /// The input, a ring: the newest sample at `newest`.
    history: Vec<f32>,
    newest: usize,
    /// Pitch mode's two read heads, half a life apart.
    heads: [Head; 2],
    /// Whether the heads are parked for the plain delayed signal.
    parked: bool,
    /// How far Pitch mode has moved to the plain delayed signal, from 0 to
    /// 1.
    plain: f64,
    /// The carrier's phase in Freq and Ring modes, in cycles.
    carrier: f64,
    /// The delay's line, and where it writes next.
    echo: Vec<f32>,
    echo_at: usize,
    /// The feedback path's low-pass state.
    tone: f64,
    /// The delay time now, in frames; `None` before the first block.
    delay: Option<f64>,
    follower: EnvelopeFollower,
}

impl Chan {
    fn new(rate: f64, latency: usize, echo_len: usize) -> Self {
        Self {
            history: vec![0.0; (6 * latency).next_power_of_two()],
            newest: 0,
            heads: Head::parked(latency),
            parked: true,
            plain: 1.0,
            carrier: 0.0,
            echo: vec![0.0; echo_len],
            echo_at: 0,
            tone: 0.0,
            delay: None,
            follower: EnvelopeFollower::new(rate),
        }
    }

    fn push(&mut self, sample: f32) {
        self.newest = (self.newest + 1) & (self.history.len() - 1);
        self.history[self.newest] = sample;
    }

    /// The sample `delay` frames before the newest.
    fn at(&self, delay: usize) -> f32 {
        self.history[self.newest.wrapping_sub(delay) & (self.history.len() - 1)]
    }

    /// Pitch mode: the signal `semitones` away, from two read heads that
    /// each run through the line for `window` frames of the song, a half
    /// run apart, crossfaded as each ends. A head that begins is set where
    /// the other's recent output carries on into it, so a steady tone keeps
    /// its pitch through the changes of head. `latency` is where the heads
    /// centre, and `fade` how far Pitch mode moves each frame toward the
    /// plain delayed signal as the shift reaches nothing.
    fn pitch(&mut self, semitones: f64, window: f64, latency: usize, fade: f64) -> f64 {
        let centred = f64::from(self.at(latency));
        let nothing = semitones.abs() < 1e-9;
        // The shifted signal starts from the plain one, so it takes over at
        // once; going back, the two are crossfaded.
        self.plain = if nothing {
            (self.plain + fade).min(1.0)
        } else {
            0.0
        };
        if self.plain >= 1.0 {
            self.heads = Head::parked(latency);
            self.parked = true;
            return centred;
        }
        let ratio = (semitones / 12.0).exp2();
        // How far the delay moves each frame, and how much of its life a
        // head lives through.
        let slope = 1.0 - ratio;
        let window = window.max(2.0 * slope.abs());
        let step = slope.abs() / window;
        let bounds = |len: usize| (4.0, (3 * latency).min(len - 64) as f64);
        if self.parked {
            // Leaving the plain signal: the second head begins now.
            self.parked = false;
            self.begin(1, ratio, window, latency, bounds(self.history.len()));
        }
        let mut shifted = 0.0;
        for head in self.heads {
            let gain = (PI * head.life).sin().powi(2);
            shifted += gain * cubic(&self.history, self.newest, head.delay);
        }
        // Both heads move on before either begins afresh, so a new head is
        // set against where the other will be.
        for head in &mut self.heads {
            head.delay += slope;
            head.life += step;
        }
        for k in 0..2 {
            if self.heads[k].life >= 1.0 {
                self.heads[k].life -= 1.0;
                self.begin(k, ratio, window, latency, bounds(self.history.len()));
            }
        }
        shifted + (centred - shifted) * self.plain
    }

    /// Begins head `k` at the start of its run, at the delay near the
    /// nominal one where the other head's recent output carries on into it.
    /// The delay stays within `bounds`, for the whole run.
    fn begin(&mut self, k: usize, ratio: f64, window: f64, latency: usize, bounds: (f64, f64)) {
        let slope = 1.0 - ratio;
        let (lo, hi) = if slope > 0.0 {
            (bounds.0, bounds.1 - window)
        } else {
            (bounds.0 + window, bounds.1)
        };
        let nominal = latency as f64 - slope.signum() * window / 2.0;
        let nominal = nominal.clamp(lo, hi.max(lo));
        // How far the search reaches: half a window, and no more than a
        // few runs of the head, so a fast shift stays cheap.
        let reach = (window / 2.0).min(3.0 * window / slope.abs()).max(4.0);
        let other = self.heads[1 - k].delay;
        let delay = self.align(other, ratio, window, nominal, reach, (lo, hi.max(lo)));
        self.heads[k].delay = delay;
    }

    /// The delay within `reach` of `nominal` and `bounds` at which a head
    /// that reads the line at `ratio` times the rate had, in the last few
    /// milliseconds, the output a head at the delay `other` had: the best
    /// match of the two, to a fraction of a frame.
    fn align(
        &self,
        other: f64,
        ratio: f64,
        window: f64,
        nominal: f64,
        reach: f64,
        bounds: (f64, f64),
    ) -> f64 {
        const MAX: usize = 128;
        // How many frames of the line the match looks across: a longer
        // window, for lower tones, looks across more.
        let span = (0.75 * window).clamp(192.0, 640.0);
        let (from, to) = (
            (nominal - reach).max(bounds.0).ceil(),
            (nominal + reach).min(bounds.1).floor(),
        );
        if from > to {
            return nominal;
        }
        // A shift down gives many frames to look across, so every few are
        // taken.
        let stride = ((span / ratio) / MAX as f64).ceil().max(1.0);
        let count = ((span / ratio / stride).round() as usize).clamp(16, MAX);
        // The other head's output, j frames ago, and where in the line the
        // same frame of a head at a delay `d` read: `d + j * ratio`.
        let (mut wanted, mut whole, mut part) = ([0.0f32; MAX], [0usize; MAX], [0.0f32; MAX]);
        for j in 0..count {
            let back = j as f64 * stride * ratio;
            whole[j] = back.floor() as usize;
            part[j] = (back - back.floor()) as f32;
            let at = other + back;
            let i = at.floor();
            let t = (at - i) as f32;
            let (a, b) = (self.at(i as usize), self.at(i as usize + 1));
            wanted[j] = a + (b - a) * t;
        }
        let candidates = (to - from) as usize + 1;
        let mut scores = [0.0f32; 512];
        let candidates = candidates.min(scores.len());
        let (mut best, mut best_score) = (0usize, f32::NEG_INFINITY);
        // From the middle out, so ties keep the delay nearest the nominal.
        let middle = ((nominal - from).round().max(0.0) as usize).min(candidates - 1);
        for n in 0..candidates {
            let c = if n % 2 == 0 {
                middle.wrapping_add(n / 2)
            } else {
                middle.wrapping_sub(n.div_ceil(2))
            };
            if c >= candidates {
                continue;
            }
            let base = from as usize + c;
            let (mut dot, mut energy) = (0.0f32, 1e-9f32);
            for j in 0..count {
                let i = base + whole[j];
                let (a, b) = (self.at(i), self.at(i + 1));
                let z = a + (b - a) * part[j];
                dot += wanted[j] * z;
                energy += z * z;
            }
            let score = dot / energy.sqrt();
            scores[c] = score;
            if score > best_score * 1.000_001 + 1e-9 || n == 0 {
                best = c;
                best_score = score;
            }
        }
        // The peak to a fraction of a frame, from its neighbours'.
        let mut offset = 0.0;
        if best > 0 && best + 1 < candidates {
            let (a, b, c) = (scores[best - 1], scores[best], scores[best + 1]);
            let bend = a - 2.0 * b + c;
            if bend < 0.0 {
                offset = f64::from(0.5 * (a - c) / bend).clamp(-0.5, 0.5);
            }
        }
        from + best as f64 + offset
    }

    /// Freq mode: single-sideband modulation by `hertz`, from the input and
    /// its Hilbert transform, each `latency` frames late.
    fn frequency(&mut self, hertz: f64, rate: f64, latency: usize, taps: &[f32]) -> f64 {
        let mask = self.history.len() - 1;
        let centre = self.newest.wrapping_sub(latency);
        let mut quadrature = 0.0;
        for (j, &tap) in taps.iter().enumerate() {
            let k = 2 * j + 1;
            let before = self.history[centre.wrapping_sub(k) & mask];
            let after = self.history[centre.wrapping_add(k) & mask];
            quadrature += f64::from(tap) * f64::from(before - after);
        }
        let (sin, cos) = (TAU * self.carrier).sin_cos();
        self.carrier = (self.carrier + hertz / rate).rem_euclid(1.0);
        f64::from(self.at(latency)) * cos - quadrature * sin
    }

    /// Ring mode: the input, `latency` frames late, times a sine of
    /// `hertz`, distorted by `drive` (0 to 1).
    fn ring(&mut self, hertz: f64, rate: f64, latency: usize, drive: f64) -> f64 {
        let sin = (TAU * self.carrier).sin();
        self.carrier = (self.carrier + hertz / rate).rem_euclid(1.0);
        let ring = f64::from(self.at(latency)) * sin;
        if drive <= 0.0 {
            return ring;
        }
        let gain = 1.0 + 15.0 * drive;
        ring + drive * ((gain * ring).tanh() / gain.tanh() - ring)
    }

    /// The delay: `shifted` goes in with the feedback, and its repeats come
    /// out `time` frames later, the time moving toward a new one by `glide`
    /// of the way each frame. `tone` is the coefficient of the low-pass in
    /// the feedback path. Returns the repeats. With `on` off the repeats
    /// already in the line play out.
    fn repeats(
        &mut self,
        shifted: f64,
        time: f64,
        glide: f64,
        on: bool,
        feedback: f64,
        tone: f64,
    ) -> f64 {
        let len = self.echo.len();
        let delay = self.delay.get_or_insert(time);
        *delay += (time - *delay) * glide;
        let time = delay.clamp(1.0, (len - 2) as f64);
        let whole = time.floor();
        let fraction = time - whole;
        let back = |n: usize| f64::from(self.echo[(self.echo_at + len - n) % len]);
        let out = back(whole as usize) * (1.0 - fraction) + back(whole as usize + 1) * fraction;
        self.tone += tone * (out - self.tone);
        let fed = if on {
            shifted + feedback * self.tone
        } else {
            0.0
        };
        self.echo[self.echo_at] = fed as f32;
        self.echo_at = (self.echo_at + 1) % len;
        out
    }
}

/// Shifter's audio-thread half.
struct Shifter {
    rate: f64,
    /// The latency every mode reports, in frames.
    latency: usize,
    taps: Vec<f32>,
    channels: Vec<Chan>,
    lfo: Lfo,
    pitch: MidiPitch,
    /// The notes and messages of the stretch being processed.
    events: Vec<NoteEvent>,
    /// The block's input, per channel.
    input: [Vec<f32>; 2],
}

impl Shifter {
    fn new(device: &Device, config: &NodeConfig) -> Self {
        let rate = config.sample_rate.as_f64();
        let latency = (LATENCY_SECONDS * rate).round().max(8.0) as usize;
        let frames = config.max_frames.max(1);
        let echo_len = (MAX_DELAY_SECONDS * rate).ceil() as usize + 4;
        Self {
            rate,
            latency,
            taps: hilbert(latency),
            channels: (0..2).map(|_| Chan::new(rate, latency, echo_len)).collect(),
            lfo: Lfo::new(config.sample_rate, device.id.0.get()),
            pitch: MidiPitch::new(),
            events: Vec::with_capacity(EVENT_ROOM),
            input: [vec![0.0; frames], vec![0.0; frames]],
        }
    }
}

impl NativeDevice for Shifter {
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
        let rate = self.rate;
        let latency = self.latency;

        // 1. The LFO's rate and stereo mode, the followers, the delay's
        // time, and this stretch's notes and messages.
        let wave_index = choice(LFO_WAVE).min(WAVES.len() - 1);
        let lfo_rate = if choice(LFO_MODE) == 0 {
            Rate::Hertz(target(LFO_HZ))
        } else {
            Rate::Beats(DIVISIONS[choice(LFO_DIVISION).min(DIVISIONS.len() - 1)].1)
        };
        // Random uses Phase alone and Random S&H Width alone (the manual);
        // Spin has no effect on either.
        let stereo = match wave_index {
            RANDOM => Stereo::Phase(target(LFO_PHASE)),
            RANDOM_SH => Stereo::Phase(0.0),
            _ if choice(LFO_STEREO) == 1 => Stereo::Spin(target(LFO_SPIN)),
            _ => Stereo::Phase(target(LFO_PHASE)),
        };
        self.lfo.set(audio, lfo_rate, stereo, target(LFO_OFFSET));
        let shape = Shape {
            duty: if WAVE_OF[wave_index].is_random() {
                0.0
            } else {
                target(LFO_DUTY) / 100.0
            },
            ..Shape::of(WAVE_OF[wave_index])
        };
        let width = if wave_index == RANDOM_SH {
            target(LFO_WIDTH) / 100.0
        } else {
            0.0
        };
        let env_on = on(ENV_ON);
        for chan in &mut self.channels {
            chan.follower.set(
                target(ENV_ATTACK) / 1_000.0,
                target(ENV_RELEASE) / 1_000.0,
                false,
            );
        }
        let tempo = audio.tempo();
        let ticks = tempo.frames_to_ticks(audio.song_position(), audio.sample_rate());
        let bpm = tempo.bpm_at(ticks).get();
        let delay_time = if choice(DELAY_MODE) == 0 {
            rate / target(DELAY_HZ).max(1e-3)
        } else {
            DELAY_DIVISIONS[choice(DELAY_DIVISION).min(DELAY_DIVISION_COUNT - 1)].1 * 60.0 / bpm
                * rate
        }
        .min(MAX_DELAY_SECONDS * rate);
        let delay_glide = 1.0 - (-1.0 / (DELAY_GLIDE_SECONDS * rate)).exp();
        let tone = target(TONE) / 100.0;
        let tone_coefficient = if tone <= 0.0 {
            1.0
        } else {
            let cutoff = 20_000.0 * (500.0f64 / 20_000.0).powf(tone);
            1.0 - (-TAU * cutoff / rate).exp()
        };
        self.events.clear();
        for note in audio.notes() {
            if self.events.len() < self.events.capacity() {
                self.events.push(note);
            }
        }

        // 2. The mode's other settings, and the input.
        let (mode, midi) = (choice(MODE), choice(PITCH_SOURCE) == 1);
        let window = (target(WINDOW) / 1_000.0 * rate).clamp(8.0, (2 * latency - 8) as f64);
        let glide_frames = target(GLIDE) / 1_000.0 * rate;
        let range = target(BEND_RANGE);
        let fade = 1.0 / (BYPASS_SECONDS * rate);
        let (coarse, fine, shift) = (ramp(COARSE), ramp(FINE), ramp(SHIFT));
        let (spread, drive) = (ramp(SPREAD), ramp(DRIVE));
        let wide = on(WIDE);
        let (lfo_amount, env_semitones, env_hertz) =
            (ramp(LFO_AMOUNT), ramp(ENV_SEMITONES), ramp(ENV_HZ));
        let (feedback, wet) = (ramp(FEEDBACK), ramp(DRY_WET));
        let delay_on = on(DELAY_ON);
        for ch in 0..channels {
            self.input[ch][..frames].copy_from_slice(audio.channel_mut(ch));
        }

        // 3. Each frame: the notes up to it, the modulation, and each
        // channel's shifted signal, repeats and mix.
        let mut next = 0;
        for i in 0..frames {
            while let Some(note) = self.events.get(next).filter(|n| n.offset as usize <= i) {
                self.pitch.take(note, glide_frames);
                next += 1;
            }
            let pitch = self.pitch.next(range);
            let mut lfo = self.lfo.step(tempo, i, shape);
            lfo[1] *= 1.0 - 2.0 * width;
            let amount = f64::from(lfo_amount[i]) / 100.0;
            for (ch, &swing) in lfo.iter().enumerate().take(channels) {
                let x = self.input[ch][i];
                let chan = &mut self.channels[ch];
                chan.push(x);
                let level = if env_on {
                    chan.follower.process(x).min(1.0)
                } else {
                    0.0
                };
                // Spread: the left up, the right down, and Wide turns the
                // right over.
                let side = if ch == 0 || wide { 1.0 } else { -1.0 };
                let spread = side * f64::from(spread[i]) / 100.0;
                let shifted = match mode {
                    0 => {
                        let base = if midi {
                            pitch
                        } else {
                            f64::from(coarse[i]) + f64::from(fine[i]) / 100.0
                        };
                        let semitones = base
                            + spread * SPREAD_CENTS / 100.0
                            + amount * LFO_SEMITONES * swing
                            + f64::from(env_semitones[i]) * level;
                        chan.pitch(semitones, window, latency, fade)
                    }
                    _ => {
                        let hertz = f64::from(shift[i])
                            + spread * SPREAD_HERTZ
                            + amount * LFO_HERTZ * swing
                            + f64::from(env_hertz[i]) * level;
                        if mode == 1 {
                            chan.frequency(hertz, rate, latency, &self.taps)
                        } else {
                            chan.ring(hertz, rate, latency, f64::from(drive[i]) / 100.0)
                        }
                    }
                };
                let repeats = chan.repeats(
                    shifted,
                    delay_time,
                    delay_glide,
                    delay_on,
                    f64::from(feedback[i]) / 100.0,
                    tone_coefficient,
                );
                let dry = f64::from(chan.at(latency));
                let w = f64::from(wet[i]) / 100.0;
                self.input[ch][i] = (dry * (1.0 - w) + (shifted + repeats) * w) as f32;
            }
        }
        for ch in 0..channels {
            audio
                .channel_mut(ch)
                .copy_from_slice(&self.input[ch][..frames]);
        }
    }

    fn latency(&self) -> u32 {
        self.latency as u32
    }
}

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

    // Spec: pitch-and-frequency-shifter R11
    #[test]
    fn the_midi_pitch_follows_the_newest_held_note() {
        let mut pitch = MidiPitch::new();
        assert_eq!(pitch.next(2.0), 0.0, "no note, no shift");
        pitch.take(&note(NoteKind::On, 67), 0.0);
        assert_eq!(pitch.next(2.0), 7.0);
        pitch.take(&note(NoteKind::On, 55), 0.0);
        assert_eq!(pitch.next(2.0), -5.0, "the newest note sounds");
        // Releasing a note that isn't the newest changes nothing.
        pitch.take(&note(NoteKind::Off, 67), 0.0);
        assert_eq!(pitch.next(2.0), -5.0);
        pitch.take(&note(NoteKind::On, 72), 0.0);
        pitch.take(&note(NoteKind::Off, 72), 0.0);
        assert_eq!(pitch.next(2.0), -5.0, "back to the one held before");
        // The last note released leaves the pitch where it is.
        pitch.take(&note(NoteKind::Off, 55), 0.0);
        assert_eq!(pitch.next(2.0), -5.0);
        // The bend is a fraction of the range, at once.
        pitch.take(&note(NoteKind::PitchBend { value: -0.5 }, 0), 0.0);
        assert_eq!(pitch.next(12.0), -5.0 - 6.0);
        assert_eq!(pitch.next(0.0), -5.0, "a range of 0 turns the bend off");
    }

    // Spec: pitch-and-frequency-shifter R11
    #[test]
    fn glide_slides_in_a_whole_number_of_frames() {
        let mut pitch = MidiPitch::new();
        pitch.take(&note(NoteKind::On, 72), 100.0);
        let steps: Vec<f64> = (0..100).map(|_| pitch.next(0.0)).collect();
        assert!((steps[0] - 0.12).abs() < 1e-12, "{}", steps[0]);
        assert!((steps[49] - 6.0).abs() < 1e-9);
        assert_eq!(
            steps[99], 12.0,
            "the pitch arrives on the glide's last frame"
        );
        assert_eq!(pitch.next(0.0), 12.0);
    }

    // Spec: pitch-and-frequency-shifter R3
    #[test]
    fn the_hilbert_taps_start_at_the_ideal_response_and_window_off_to_nothing() {
        let taps = hilbert(480);
        assert_eq!(taps.len(), 240);
        // The ideal response is 2/(pi k); the window takes it to zero at the
        // far end.
        assert!((f64::from(taps[0]) - 2.0 / PI).abs() < 1e-3);
        assert!(taps[239].abs() < 1e-4);
    }
}

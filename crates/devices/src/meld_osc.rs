//! Meld's oscillators (spec: meld R4 to R11): the first ten types as laws
//! that turn a type, its two macros, the played key and the current scale
//! into a few lanes of the plug-in kit's wavetable oscillator, and the
//! band-limited tables those lanes read.
//!
//! A type's sound is a mix of lanes, each a table read at its own pitch
//! offset and frame; the tables carry the shape (Basic Shapes' morph and
//! duty cycle, Square Sync's ratios, Square 5th's pulse widths), so
//! every read is band-limited by the kit's mipmaps and nothing here
//! folds back. Everything is Beatlayer's own reading of the manual's
//! description of each type.

use std::sync::OnceLock;

use bl_model::Scale;
use lspk_dsp::simd::{F32s, U32s};
use lspk_wavetable::osc::{Bypass, Inputs, Layout, Onset, Oscillator, PhaseMode, PhaseStage};
use lspk_wavetable::{Cache, FRAME_SIZE, SharedTable, Wavetable};

/// The names of the twenty-five oscillator types, in the manual's order
/// (R4).
pub const TYPES: [&str; 25] = [
    "Basic Shapes",
    "Dual Basic Shapes",
    "Noisy Shapes",
    "Square Sync",
    "Square 5th",
    "Sub",
    "Swarm Sine",
    "Swarm Triangle",
    "Swarm Saw",
    "Swarm Square",
    "Harmonic FM",
    "Fold FM",
    "Squelch",
    "Simple FM",
    "Chip",
    "Shepard's Pi",
    "Tarp",
    "Extratone",
    "Noise Loop",
    "Filtered Noise",
    "Bitgrunge",
    "Crackle",
    "Rain",
    "Bubble",
    "Chord",
];

/// Each type's two macros' names, as the panel shows them (R5).
pub const MACROS: [(&str, &str); 25] = [
    ("Shape", "Tone"),
    ("Shape", "Detune"),
    ("Shape", "Rough"),
    ("Freq 1", "Freq 2"),
    ("5th Amt", "P Width"),
    ("Tone", "Aux"),
    ("Motion", "Spacing"),
    ("Motion", "Spacing"),
    ("Motion", "Spacing"),
    ("Motion", "Spacing"),
    ("Amount", "Ratio"),
    ("Amount", "Shape"),
    ("Amount", "Feedback"),
    ("Amount", "Ratio"),
    ("Tone", "Rate"),
    ("Rate", "Width"),
    ("Decay", "Tone"),
    ("Pitch", "Env Amt"),
    ("Rate", "Fade"),
    ("Freq", "Narrow"),
    ("Freq", "Mult"),
    ("Density", "Intensity"),
    ("Tone", "Rate"),
    ("Density", "Spread"),
    ("Shape", "Inversion"),
];

/// The most lanes a type reads for one voice: the swarms' copies.
pub const MAX_LANES: usize = 8;

/// The chunk of frames a bank renders at a time: the kit's control
/// interval, so controls change on a grid every block size shares.
pub const CHUNK: usize = 32;

/// The most voices one kit oscillator holds.
const OSC_VOICES: usize = lspk_wavetable::osc::MAX_VOICES;

/// Duty cycles Basic Shapes' Tone steps through, from 50 % to 5 %.
const TONES: usize = 24;
const DUTY_MIN: f64 = 0.05;

/// Square Sync's ratios: 1 to 8 in thirty-seconds.
const SYNC_FRAMES: usize = 7 * 32 + 1;

/// Square 5th's pulse widths: 5 % to 95 % in per cents.
const PW_FRAMES: usize = 91;

/// A fifth up, as a ratio of 3 to 2, in semitones.
const FIFTH: f32 = 7.019_55;

/// The swarms' chord, above the note, in semitones, and their unison
/// spread in cents (R11).
const CHORD: [f32; MAX_LANES] = [0.0, 7.0, 12.0, 19.0, 24.0, 31.0, 36.0, 43.0];
const SWARM_CENTS: f32 = 10.0;

/// The most a swarm copy drifts, in cents (R11).
pub const MOTION_CENTS: f32 = 30.0;

/// Which kit oscillator a type reads, by how many lanes a voice needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Basic,
    Dual,
    Sync,
    Fifth,
    Sub,
    Swarm,
}

impl Kind {
    /// The banks, in the order [`Banks`] holds them.
    pub const ALL: [Self; 6] = [
        Self::Basic,
        Self::Dual,
        Self::Sync,
        Self::Fifth,
        Self::Sub,
        Self::Swarm,
    ];

    /// Its place in [`Self::ALL`].
    #[must_use]
    pub const fn index(self) -> usize {
        self as usize
    }

    /// How many lanes a voice has in this bank.
    #[must_use]
    pub const fn lanes(self) -> usize {
        match self {
            Self::Basic | Self::Sync | Self::Fifth | Self::Sub => 2,
            Self::Dual => 4,
            Self::Swarm => MAX_LANES,
        }
    }

    /// The bank oscillator type `index` reads, if it is built.
    #[must_use]
    pub fn of(index: usize) -> Option<Self> {
        Some(match index {
            0 | 2 => Self::Basic,
            1 => Self::Dual,
            3 => Self::Sync,
            4 => Self::Fifth,
            5 => Self::Sub,
            6..=9 => Self::Swarm,
            _ => return None,
        })
    }
}

/// The band-limited tables the types read.
pub struct Tables {
    basic: SharedTable,
    sync: SharedTable,
    pulse: SharedTable,
    sub: SharedTable,
    /// Sine, triangle, saw and square, one frame each.
    shapes: [SharedTable; 4],
}

/// A wave of one cycle at `p` (0 to 1): sine, triangle, saw or square,
/// each rising through zero at the start of the cycle.
fn shape(kind: usize, p: f64) -> f64 {
    match kind {
        0 => (std::f64::consts::TAU * p).sin(),
        1 => (std::f64::consts::TAU * p).sin().asin() * 2.0 / std::f64::consts::PI,
        2 => 2.0 * ((p + 0.5).fract()) - 1.0,
        _ => {
            if p < 0.5 {
                1.0
            } else {
                -1.0
            }
        }
    }
}

/// A saw whose ramp takes the first `2 duty` of the cycle and then holds
/// at the top (R7): the plain saw at a duty cycle of 50 %.
fn narrow_saw(p: f64, duty: f64) -> f64 {
    (2.0 * (p / (2.0 * duty)).min(1.0)) - 1.0
}

/// A pulse high for the first `duty` of the cycle.
fn pulse(p: f64, duty: f64) -> f64 {
    if p < duty { 1.0 } else { -1.0 }
}

/// A table of `frames` frames whose frame `f` at cycle position `p` is
/// `wave(f, p)`, each frame brought to zero mean and a peak of one.
fn table(frames: usize, wave: impl Fn(usize, f64) -> f64) -> SharedTable {
    let drawn: Vec<Vec<f32>> = (0..frames)
        .map(|f| {
            let cycle: Vec<f64> = (0..FRAME_SIZE)
                .map(|n| wave(f, n as f64 / FRAME_SIZE as f64))
                .collect();
            let mean = cycle.iter().sum::<f64>() / FRAME_SIZE as f64;
            let peak = cycle.iter().map(|x| (x - mean).abs()).fold(1e-9, f64::max);
            cycle.iter().map(|x| ((x - mean) / peak) as f32).collect()
        })
        .collect();
    let drawn = Wavetable::new(&drawn).expect("a table of at most 256 frames of 2,048 samples");
    Cache::global().share(drawn)
}

fn duty_at(step: usize) -> f64 {
    0.5 - (0.5 - DUTY_MIN) * step as f64 / (TONES - 1) as f64
}

/// The tables, built once, off the audio thread.
#[must_use]
pub fn tables() -> &'static Tables {
    static TABLES: OnceLock<Tables> = OnceLock::new();
    TABLES.get_or_init(|| Tables {
        basic: table(2 + 2 * TONES, |f, p| match f {
            0 => shape(0, p),
            1 => shape(1, p),
            f if f < 2 + TONES => narrow_saw(p, duty_at(f - 2)),
            f => pulse(p, duty_at(f - 2 - TONES)),
        }),
        sync: table(SYNC_FRAMES, |f, p| {
            let ratio = 1.0 + f as f64 / 32.0;
            shape(3, (p * ratio).fract())
        }),
        pulse: table(PW_FRAMES, |f, p| pulse(p, 0.05 + 0.01 * f as f64)),
        sub: table(2, |f, p| shape(if f == 0 { 0 } else { 3 }, p)),
        shapes: [0, 1, 2, 3].map(|k| table(1, |_, p| shape(k, p))),
    })
}

/// What one lane of a voice plays: its pitch above the voice's, in
/// semitones, the frame it reads, and its gain.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Lane {
    pub semitones: f32,
    pub frame: f32,
    pub gain: f32,
}

/// A voice's lanes for one chunk.
#[derive(Clone, Copy, Debug, Default)]
pub struct Lanes {
    pub count: usize,
    pub lane: [Lane; MAX_LANES],
}

/// What a type reads besides its macros: the key and the scale its
/// intervals come from (Oscillator Scale Awareness off gives the chromatic
/// one), and each swarm copy's drift in cents.
pub struct Context<'a> {
    pub key: u8,
    pub scale: &'a Scale,
    pub drift: &'a [f32; MAX_LANES],
}

/// Semitones from `key` to the key `degrees` degrees of `scale` above it.
fn degrees(scale: &Scale, key: u8, degrees: i32) -> f32 {
    (scale.move_off(key, degrees) - i32::from(key)) as f32
}

/// The 0-based frame of Basic Shapes' wave `shape` (0 to 1) of anchor
/// `anchor` at Tone `tone` (0 to 1).
fn basic_frame(anchor: usize, tone: f32) -> f32 {
    let step = tone * (TONES - 1) as f32;
    match anchor {
        0 | 1 => anchor as f32,
        2 => 2.0 + step,
        _ => (2 + TONES) as f32 + step,
    }
}

/// Two lanes that crossfade the neighbouring waves of `shape` at `tone`,
/// from lane `at`, with an interval of `semitones` and a gain `gain`.
fn basic(lanes: &mut Lanes, at: usize, shape: f32, tone: f32, semitones: f32, gain: f32) {
    let place = shape.clamp(0.0, 1.0) * 3.0;
    let low = (place.floor() as usize).min(3);
    let high = (low + 1).min(3);
    let across = place - low as f32;
    lanes.lane[at] = Lane {
        semitones,
        frame: basic_frame(low, tone),
        gain: gain * (1.0 - across),
    };
    lanes.lane[at + 1] = Lane {
        semitones,
        frame: basic_frame(high, tone),
        gain: gain * across,
    };
}

/// Spec: meld R7 to R11. Type `index`'s lanes for macros `m1` and `m2`
/// (0 to 1).
#[must_use]
pub fn lanes(index: usize, m1: f32, m2: f32, cx: &Context<'_>) -> Lanes {
    let mut out = Lanes::default();
    let Some(kind) = Kind::of(index) else {
        return out;
    };
    out.count = kind.lanes();
    match index {
        // R7: Shape morphs the four waves; Tone narrows the saw and the
        // square. Noisy Shapes' Rough is phase noise, which the renderer
        // adds, and leaves Tone at zero.
        0 => basic(&mut out, 0, m1, m2, 0.0, 1.0),
        2 => basic(&mut out, 0, m1, 0.0, 0.0, 1.0),
        // R8: the second wave up to 50 cents, then up the intervals of an
        // octave.
        1 => {
            let up = if m2 <= 0.5 {
                m2
            } else {
                let octave = cx.scale.steps().len() as f32;
                degrees(cx.scale, cx.key, ((m2 - 0.5) * 2.0 * octave).round() as i32)
            };
            basic(&mut out, 0, m1, 0.0, 0.0, 0.5);
            basic(&mut out, 2, m1, 0.0, up, 0.5);
        }
        // R9: two squares hard-synced to the note at ratios 1 to 8, mixed
        // equally.
        3 => {
            for (i, m) in [m1, m2].into_iter().enumerate() {
                out.lane[i] = Lane {
                    semitones: 0.0,
                    frame: m.clamp(0.0, 1.0) * 7.0 * 32.0,
                    gain: 0.5,
                };
            }
        }
        // R9: a square at the note and one a fifth above it, at P Width.
        4 => {
            let width = m2.clamp(0.0, 1.0) * 90.0;
            let level = (1.0 + m1 * m1).sqrt().recip();
            out.lane[0] = Lane {
                semitones: 0.0,
                frame: width,
                gain: level,
            };
            out.lane[1] = Lane {
                semitones: FIFTH,
                frame: width,
                gain: level * m1,
            };
        }
        // R10: a sine into a square by Tone, and a sine an octave below.
        5 => {
            let level = (1.0 + m2 * m2).sqrt().recip();
            out.lane[0] = Lane {
                semitones: 0.0,
                frame: m1.clamp(0.0, 1.0),
                gain: level,
            };
            out.lane[1] = Lane {
                semitones: -12.0,
                frame: 0.0,
                gain: level * m2,
            };
        }
        // R11: eight copies, each a drifting unison copy fading into the
        // chord (or every second degree of the scale) as Spacing rises.
        _ => {
            let gain = (MAX_LANES as f32).sqrt().recip();
            for i in 0..MAX_LANES {
                let unison = (2.0 * i as f32 / (MAX_LANES - 1) as f32 - 1.0) * SWARM_CENTS / 100.0;
                let chord = if cx.scale.enabled {
                    degrees(cx.scale, cx.key, 2 * i as i32)
                } else {
                    CHORD[i]
                };
                out.lane[i] = Lane {
                    semitones: unison + (chord - unison) * m2 + cx.drift[i] * MOTION_CENTS / 100.0,
                    frame: 0.0,
                    gain,
                };
            }
        }
    }
    out
}

/// Which of the four shape tables a swarm type reads.
#[must_use]
pub fn swarm_shape(index: usize) -> usize {
    index.saturating_sub(6).min(3)
}

/// The kit oscillators for one kind of type, as many as hold `slots` voices
/// of the kind's lanes each, and the per-lane arrays one render of
/// [`CHUNK`] frames reads and writes.
pub struct Bank {
    oscs: Vec<Oscillator>,
    pitch: Vec<f32>,
    step: Vec<f32>,
    pitch_mod: Vec<f32>,
    phase_noise: Vec<u32>,
    out: Vec<f32>,
    per: usize,
    stride: usize,
}

impl Bank {
    /// A bank for `slots` voices at `rate`, with `seed` for the kit's
    /// random draws. Allocates.
    #[must_use]
    pub fn new(kind: Kind, slots: usize, rate: f32, seed: u64) -> Self {
        let tables = tables();
        let table = match kind {
            Kind::Basic | Kind::Dual => &tables.basic,
            Kind::Sync => &tables.sync,
            Kind::Fifth => &tables.pulse,
            Kind::Sub => &tables.sub,
            Kind::Swarm => &tables.shapes[2],
        };
        let per = kind.lanes();
        let oscs: Vec<Oscillator> = (0..slots.div_ceil(OSC_VOICES))
            .map(|i| {
                let mut osc = Oscillator::new(
                    table.clone(),
                    Layout {
                        voices: OSC_VOICES,
                        lanes_per_voice: per,
                    },
                    rate,
                    CHUNK,
                    seed.wrapping_add(i as u64),
                );
                let p = osc.params_mut();
                p.level = 100.0;
                p.frame = 1.0;
                p.smooth = true;
                p.phase = 0.0;
                p.phase_random = 0.0;
                p.key_track = true;
                osc
            })
            .collect();
        let stride = oscs[0].lane_count();
        Self {
            pitch: vec![0.0; stride * oscs.len()],
            step: vec![0.0; stride * oscs.len()],
            pitch_mod: vec![0.0; stride * oscs.len() * CHUNK],
            phase_noise: vec![0; stride * oscs.len() * CHUNK],
            out: vec![0.0; stride * oscs.len() * CHUNK],
            oscs,
            per,
            stride,
        }
    }

    /// Swaps the swarm's wave for shape `shape` (0 to 3).
    pub fn set_shape(&mut self, shape: usize) {
        let table = tables().shapes[shape.min(3)].clone();
        for osc in &mut self.oscs {
            osc.set_table(table.clone());
        }
    }

    fn at(&self, slot: usize, copy: usize) -> (usize, usize) {
        (slot / OSC_VOICES, (slot % OSC_VOICES) * self.per + copy)
    }

    /// Slot `slot` starts a note on `key`: its lanes restart at `degrees` of
    /// phase if `reset` and `onset` restarts, else carry on (R16).
    pub fn start(&mut self, slot: usize, key: f32, onset: Onset, reset: bool, degrees: f32) {
        let (osc, local) = self.at(slot, 0);
        let osc = &mut self.oscs[osc];
        let p = osc.params_mut();
        p.phase_mode = if reset {
            PhaseMode::NoteStart
        } else {
            PhaseMode::Continue
        };
        p.phase = degrees;
        osc.start(local / self.per, key, onset);
    }

    /// Sets lane `copy` of slot `slot` for the next render: its pitch at the
    /// first frame and per frame, and its frame in the table.
    pub fn set(&mut self, slot: usize, copy: usize, pitch: f32, step: f32, frame: f32) {
        let (osc, local) = self.at(slot, copy);
        self.pitch[osc * self.stride + local] = pitch;
        self.step[osc * self.stride + local] = step;
        self.oscs[osc].frame_offsets_mut()[local] = frame;
    }

    /// Pitch modulation in semitones for lane `copy` of slot `slot` over the
    /// chunk, `values` per frame: how a Glissando's steps reach the
    /// oscillator.
    pub fn set_mod(&mut self, slot: usize, copy: usize, values: &[f32]) {
        let (osc, local) = self.at(slot, copy);
        for (k, v) in values.iter().enumerate() {
            self.pitch_mod[(osc * CHUNK + k) * self.stride + local] = *v;
        }
    }

    /// Phase noise for lane `copy` of slot `slot` over the chunk, `values`
    /// per frame in cycles, added to the phase the table is read at.
    pub fn set_phase(&mut self, slot: usize, copy: usize, values: &[f32]) {
        let (osc, local) = self.at(slot, copy);
        for (k, v) in values.iter().enumerate() {
            let word = (f64::from(*v) * lspk_wavetable::CYCLE).round() as i64 as u32;
            self.phase_noise[(osc * CHUNK + k) * self.stride + local] = word;
        }
    }

    /// Renders `frames` frames of the first `slots` slots. `modulated` reads
    /// [`set_mod`](Self::set_mod)'s values and `noisy` [`set_phase`](Self::set_phase)'s,
    /// per lane and frame.
    pub fn render(&mut self, frames: usize, slots: usize, modulated: bool, noisy: bool) {
        for (i, osc) in self.oscs.iter_mut().enumerate() {
            let from = i * OSC_VOICES;
            if slots <= from {
                break;
            }
            let lanes = (slots - from).min(OSC_VOICES) * self.per;
            let base = i * self.stride;
            let span = i * CHUNK * self.stride..(i * CHUNK + frames) * self.stride;
            let inputs = Inputs {
                lanes,
                pitch: &self.pitch[base..base + self.stride],
                pitch_step: Some(&self.step[base..base + self.stride]),
                pitch_mod: modulated.then(|| &self.pitch_mod[span.clone()]),
                frame_mod: None,
            };
            let out = &mut self.out[span.clone()];
            if noisy {
                let mut stage = PhaseNoise {
                    offsets: &self.phase_noise[span],
                    lanes: self.stride,
                };
                osc.process_scalar_with(frames, &inputs, out, &mut stage, &mut Bypass);
            } else {
                osc.process(frames, &inputs, out);
            }
        }
    }

    /// Lane `copy` of slot `slot` at frame `frame` of the last render.
    #[must_use]
    pub fn out(&self, slot: usize, copy: usize, frame: usize) -> f32 {
        let (osc, local) = self.at(slot, copy);
        self.out[(osc * CHUNK + frame) * self.stride + local]
    }
}

/// Phase noise as a phase stage: each lane's read phase moves by its
/// offset at the frame.
struct PhaseNoise<'a> {
    offsets: &'a [u32],
    lanes: usize,
}

impl PhaseStage for PhaseNoise<'_> {
    #[inline(always)]
    fn shape<S: F32s>(
        &mut self,
        lanes: usize,
        frame: usize,
        phase: <S as F32s>::U32s,
        _: <S as F32s>::U32s,
    ) -> <S as F32s>::U32s {
        let at = frame * self.lanes + lanes;
        phase.wrapping_add(<S as F32s>::U32s::load(&self.offsets[at..]))
    }

    #[inline(always)]
    fn shape_lane(&mut self, lane: usize, frame: usize, phase: u32, _: u32) -> u32 {
        phase.wrapping_add(self.offsets[frame * self.lanes + lane])
    }

    /// The noise's slope is at most about twice the note's.
    #[inline(always)]
    fn stretch(&mut self, _: usize, _: std::ops::Range<usize>, _: u32) -> f32 {
        2.0
    }
}

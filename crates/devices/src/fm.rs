//! The FM core (WP-8.6; spec: operator R1 to R10): four phase-modulation
//! operators for every voice of the instruments' voice section, the eleven
//! algorithms, feedback, frequency by ratio or fixed, the built-in and user
//! waveforms as the plug-in kit's band-limited tables, and the aliasing
//! controls. Operator, the Drum Synths' FM and Meld's FM types build on it.
//!
//! A device owns a [`Core`], writes its [`Settings`] each stretch, calls
//! [`Core::begin`], plays its voices through [`Core::start`] and
//! [`Core::render`], and takes the stretch's stereo output from
//! [`Core::finish`].
//! Everything is made in [`Core::new`] and [`user_table`], off the audio
//! thread; nothing here allocates, locks or blocks on it.

use core::f32::consts::TAU;
use core::ops::Range;

use bl_dsp::{Oversampler, Random};
use bl_engine::NodeData;
use lspk_voice::{MAX_VOICES, Start, VoiceBlock};
use lspk_wavetable::{CYCLE, Cache, FRAME_SIZE, SharedTable, Wavetable, increment};

use crate::instrument::{Sources, State};

/// Operators A to D, as indexes 0 to 3.
pub const OPERATORS: usize = 4;

/// The built-in waveforms in the Wave chooser's order, then User (R6).
pub const WAVES: [&str; 23] = [
    "Sine",
    "Sine 4 Bit",
    "Sine 8 Bit",
    "Saw 3",
    "Saw 4",
    "Saw 6",
    "Saw 8",
    "Saw 16",
    "Saw 32",
    "Saw 64",
    "Saw D",
    "Square 3",
    "Square 4",
    "Square 6",
    "Square 8",
    "Square 16",
    "Square 32",
    "Square 64",
    "Square D",
    "Triangle",
    "Noise Looped",
    "Noise White",
    "User",
];

/// The waves played other than from a band-limited table, and User.
pub const SAW_D: usize = 10;
pub const SQUARE_D: usize = 18;
pub const NOISE_LOOPED: usize = 20;
pub const NOISE_WHITE: usize = 21;
pub const USER: usize = 22;

/// The harmonics of the saws (waves 3 to 9) and the squares (11 to 17).
const HARMONICS: [usize; 7] = [3, 4, 6, 8, 16, 32, 64];

/// The algorithms' graphs (R2): for each algorithm and operator, the
/// operators that modulate it, as a mask of operators A to D from bit 0.
/// Every modulator is a later letter than what it modulates, so playing D
/// first and A last gives each operator its modulators' current output.
const MODULATORS: [[u8; OPERATORS]; 11] = [
    [0b0010, 0b0100, 0b1000, 0b0000], // 1: D to C to B to A
    [0b0010, 0b1100, 0b0000, 0b0000], // 2: C and D to B to A
    [0b1010, 0b0100, 0b0000, 0b0000], // 3: C to B, B and D to A
    [0b0110, 0b1000, 0b1000, 0b0000], // 4: D to B and C, B and C to A
    [0b0100, 0b0100, 0b1000, 0b0000], // 5: D to C, C to A and B
    [0b0000, 0b0100, 0b1000, 0b0000], // 6: D to C to B, A alone
    [0b1110, 0b0000, 0b0000, 0b0000], // 7: B, C and D to A
    [0b0010, 0b0000, 0b1000, 0b0000], // 8: B to A, D to C
    [0b1000, 0b1000, 0b1000, 0b0000], // 9: D to A, B and C
    [0b0000, 0b0000, 0b1000, 0b0000], // 10: D to C, A and B alone
    [0b0000; OPERATORS],              // 11: all alone
];

/// The operators no other operator modulates in `algorithm` (0 to 10):
/// the carriers, as a mask from bit 0 for A.
#[must_use]
pub fn carriers(algorithm: usize) -> u8 {
    let modulators = MODULATORS[algorithm].iter().fold(0, |all, m| all | m);
    !modulators & 0b1111
}

/// Spec: operator R1. A modulator at amplitude `a` moves its carrier's
/// phase by up to `INDEX × a` radians.
const INDEX: f32 = 4.0;

/// Spec: operator R3. Feedback's reach in radians at 100 %.
const FEEDBACK: f32 = 1.5;

/// Spec: operator R4. A fixed frequency at or above this share of the
/// sample rate is silent.
const FIXED_TOP: f64 = 0.45;

/// A phase in radians as [`CYCLE`] units, wrapped.
const UNITS_PER_RADIAN: f32 = (4_294_967_296.0 / core::f64::consts::TAU) as f32;

/// The most partials a table holds: its frame's, below Nyquist.
const MAX_PARTIALS: usize = FRAME_SIZE / 2 - 1;

/// What one operator's controls say (R4, R5, R8).
#[derive(Clone, Copy, Debug)]
pub struct OperatorSettings {
    pub on: bool,
    /// Coarse as a ratio: 0.5, or 1 to 48.
    pub coarse: f32,
    /// Fine, in thousandths of a ratio.
    pub fine: f32,
    pub fixed: bool,
    /// Freq in hertz and Multi, whose product is the fixed frequency.
    pub freq: f32,
    pub multi: f32,
    /// Osc < Vel, from −1 to 1, and its Q switch.
    pub vel: f32,
    pub quantise: bool,
    /// Level as a linear amplitude.
    pub level: f32,
    /// An index of [`WAVES`].
    pub wave: usize,
    /// Feedback, from 0 to 1.
    pub feedback: f32,
    /// Phase, from 0 to 1 of a cycle.
    pub phase: f32,
    pub retrigger: bool,
}

impl OperatorSettings {
    const fn new(level: f32) -> Self {
        Self {
            on: true,
            coarse: 1.0,
            fine: 0.0,
            fixed: false,
            freq: 100.0,
            multi: 1.0,
            vel: 0.0,
            quantise: true,
            level,
            wave: 0,
            feedback: 0.0,
            phase: 0.0,
            retrigger: true,
        }
    }
}

/// What a device adds to one operator of one voice at one frame (operator
/// R16): its amplitude on top of Level, a move of its pitch in semitones
/// and an addition to its Feedback.
#[derive(Clone, Copy, Debug)]
pub struct Moves {
    pub gain: f32,
    pub semitones: f32,
    pub feedback: f32,
}

impl Moves {
    /// Only an amplitude: the operator plays at its pitch and Feedback.
    #[must_use]
    pub const fn gain(gain: f32) -> Self {
        Self {
            gain,
            semitones: 0.0,
            feedback: 0.0,
        }
    }
}

/// What the core reads from the device's parameters each stretch.
#[derive(Clone, Copy, Debug)]
pub struct Settings {
    /// Algorithm, 0 to 10 for the manual's 1 to 11.
    pub algorithm: usize,
    /// Antialias (R9).
    pub antialias: bool,
    pub interpolation: bool,
    /// Tone, from 0 to 1.
    pub tone: f32,
    pub ops: [OperatorSettings; OPERATORS],
}

impl Default for Settings {
    /// A new device's (R4, R5, R8, R9): A at 0 dB, B to D at −∞ dB.
    fn default() -> Self {
        Self {
            algorithm: 0,
            antialias: true,
            interpolation: true,
            tone: 0.7,
            ops: [
                OperatorSettings::new(1.0),
                OperatorSettings::new(0.0),
                OperatorSettings::new(0.0),
                OperatorSettings::new(0.0),
            ],
        }
    }
}

/// A user wave for operator `op`, as node data (R7): built off the audio
/// thread with [`user_table`], and swapped in by [`Core::receive_data`].
#[derive(Clone, Debug)]
pub struct UserWave {
    pub op: usize,
    pub table: SharedTable,
}

/// The sine, 2,048 samples of it.
fn sine_cycle() -> Vec<f32> {
    (0..FRAME_SIZE)
        .map(|n| (f64::from(TAU) * n as f64 / FRAME_SIZE as f64).sin() as f32)
        .collect()
}

/// The table of the partials with amplitudes `levels`, the first at
/// harmonic 1.
fn partials(levels: &[f32]) -> Wavetable {
    let sine = sine_cycle();
    Wavetable::from_fn(1, |_, n| {
        levels
            .iter()
            .enumerate()
            .map(|(h, level)| level * sine[((h + 1) * n) % FRAME_SIZE])
            .sum()
    })
    .expect("one frame is a table")
}

/// The sine in `steps` equal levels, from −1 to 1.
fn quantised(steps: usize) -> Wavetable {
    let sine = sine_cycle();
    let top = (steps - 1) as f32;
    Wavetable::from_fn(1, |_, n| {
        ((sine[n] * 0.5 + 0.5) * top).round() / top * 2.0 - 1.0
    })
    .expect("one frame is a table")
}

/// The built-in wave `wave` as a table, if it is played from one (R6).
fn built_in(wave: usize) -> Option<Wavetable> {
    // Each series is scaled so its limit's peak is 1.
    let series = |count: usize, term: &dyn Fn(usize) -> f32| {
        partials(&(1..=count).map(term).collect::<Vec<_>>())
    };
    let pi = core::f32::consts::PI;
    Some(match wave {
        0 => partials(&[1.0]),
        1 => quantised(16),
        2 => quantised(256),
        3..=9 => series(HARMONICS[wave - 3], &|h| {
            2.0 / pi / h as f32 * if h % 2 == 1 { 1.0 } else { -1.0 }
        }),
        11..=17 => series(HARMONICS[wave - 11], &|h| {
            if h % 2 == 1 { 4.0 / pi / h as f32 } else { 0.0 }
        }),
        19 => series(MAX_PARTIALS, &|h| match h % 4 {
            1 => 8.0 / (pi * pi) / (h * h) as f32,
            3 => -8.0 / (pi * pi) / (h * h) as f32,
            _ => 0.0,
        }),
        _ => return None,
    })
}

/// A user wave's table (R7): the levels of its first `partials.len()`
/// partials, each 0 to 1, and Repeat (0 to 1) or none. With Repeat the
/// drawn pattern goes on above its count, copy `k` from 1 scaled by
/// `(1 − repeat)^k`, up to the table's top partial. Builds a table: call
/// it off the audio thread, then send it as [`UserWave`].
///
/// # Panics
///
/// If `partials` is empty.
#[must_use]
pub fn user_table(partials: &[f32], repeat: Option<f32>) -> SharedTable {
    assert!(!partials.is_empty(), "a user wave has partials");
    let count = partials.len();
    let top = if repeat.is_some() {
        MAX_PARTIALS
    } else {
        count.min(MAX_PARTIALS)
    };
    let levels: Vec<f32> = (0..top)
        .map(|h| {
            let copy = (h / count) as i32;
            partials[h % count] * repeat.map_or(1.0, |r| (1.0 - r).powi(copy))
        })
        .collect();
    Cache::global().share(self::partials(&levels))
}

/// Voices' four operators and everything they share.
pub struct Core {
    /// What the stretch plays: written by the device before [`Core::begin`].
    pub settings: Settings,
    rate: f64,
    /// A table for each wave of [`WAVES`] played from one.
    tables: Vec<Option<SharedTable>>,
    user: [SharedTable; OPERATORS],
    /// Noise Looped's 1-second loop (R6).
    looped: Vec<f32>,
    noise: Random,
    /// Each voice's operators' phases and last two outputs.
    phase: [[u32; OPERATORS]; MAX_VOICES],
    history: [[[f32; 2]; OPERATORS]; MAX_VOICES],
    /// Each voice's operators' last table level, by the increment it was
    /// chosen for.
    levels: [[(u32, usize); OPERATORS]; MAX_VOICES],
    /// The stretch's mix, left and right, at the operators' rate.
    mix: [Vec<f32>; 2],
    oversampler: Oversampler,
    /// 1, or 2 with Antialias.
    factor: usize,
}

impl Core {
    /// A core for a device at `rate` that plays blocks of up to
    /// `max_frames`, with `seed` for its noise. Builds every table.
    #[must_use]
    pub fn new(rate: f64, max_frames: usize, seed: u64) -> Self {
        let cache = Cache::global();
        let tables: Vec<Option<SharedTable>> = (0..WAVES.len())
            .map(|wave| built_in(wave).map(|table| cache.share(table)))
            .collect();
        let sine = tables[0].clone().expect("Sine is a table");
        let mut random = Random::seeded(0x6C6F_6F70);
        Self {
            settings: Settings::default(),
            rate,
            tables,
            user: core::array::from_fn(|_| sine.clone()),
            looped: (0..rate as usize)
                .map(|_| random.next_bipolar() as f32)
                .collect(),
            noise: Random::seeded(seed),
            phase: [[0; OPERATORS]; MAX_VOICES],
            history: [[[0.0; 2]; OPERATORS]; MAX_VOICES],
            levels: [[(0, 0); OPERATORS]; MAX_VOICES],
            mix: [
                vec![0.0; max_frames.max(1) * 2],
                vec![0.0; max_frames.max(1) * 2],
            ],
            oversampler: Oversampler::new(1, 2, max_frames.max(1)),
            factor: 1,
        }
    }

    /// Frees every voice's state, as a switched-off device does.
    pub fn reset(&mut self) {
        self.phase = [[0; OPERATORS]; MAX_VOICES];
        self.history = [[[0.0; 2]; OPERATORS]; MAX_VOICES];
        self.oversampler.reset();
    }

    /// The frames the device delays its output by: Antialias's (R9).
    #[must_use]
    pub fn latency(&self) -> u32 {
        if self.settings.antialias {
            self.oversampler.latency()
        } else {
            0
        }
    }

    /// Takes a [`UserWave`] sent to the device; whether `data` was one.
    /// The wave it replaces is let go of, never freed here: the cache
    /// frees tables off the audio thread.
    pub fn receive_data(&mut self, data: &NodeData) -> bool {
        match data.downcast_ref::<UserWave>() {
            Some(wave) if wave.op < OPERATORS => {
                self.user[wave.op] = wave.table.clone();
                true
            }
            _ => false,
        }
    }

    /// Starts a stretch of `frames`.
    pub fn begin(&mut self, frames: usize) {
        let factor = if self.settings.antialias { 2 } else { 1 };
        if factor != self.factor {
            self.oversampler.reset();
            self.factor = factor;
        }
        for mix in &mut self.mix {
            mix[..frames * factor].fill(0.0);
        }
    }

    /// A voice's note starts (R8): a fresh voice's operators start at their
    /// Phase where they Retrigger. A stolen, retriggered or legato voice
    /// keeps its phases, so taking it never clicks.
    pub fn start(&mut self, voice: usize, start: Start) {
        if matches!(start, Start::Fresh) {
            for (op, settings) in self.settings.ops.iter().enumerate() {
                if settings.retrigger {
                    // The noise loop's phase is a position in its samples.
                    let cycle = if settings.wave == NOISE_LOOPED {
                        (self.looped.len() * self.factor) as f64
                    } else {
                        CYCLE
                    };
                    self.phase[voice][op] = (f64::from(settings.phase) * cycle) as u32;
                }
            }
            self.history[voice] = [[0.0; 2]; OPERATORS];
        }
    }

    /// Renders `frames` of the stretch for the active voices into the
    /// stretch's mix. `operator(voice, op, frame)` is what the device gives
    /// each operator on top of its settings (an envelope's amplitude, a
    /// pitch envelope's move), and `pan(voice, frame)` the left and right
    /// gains of the voice's place in the mix, for `frame` counted from the
    /// start of `frames`.
    ///
    /// Frames go first, then voices, so the noise waves draw the same
    /// values at any block size.
    pub fn render(
        &mut self,
        voices: &VoiceBlock,
        state: &State,
        frames: Range<usize>,
        operator: impl Fn(usize, usize, usize) -> Moves,
        pan: impl Fn(usize, usize) -> [f32; 2],
    ) {
        let settings = self.settings;
        let factor = self.factor;
        let internal = self.rate * factor as f64;
        let algorithm = settings.algorithm.min(MODULATORS.len() - 1);
        let normal = 1.0 / carriers(algorithm).count_ones() as f32;
        // Spec: operator R9. Each 25 % of Tone below 100 % halves the
        // partials a table plays: a table level chosen for a note that
        // many times higher.
        let darker = (f64::from(1.0 - settings.tone.clamp(0.0, 1.0)) * 4.0).exp2();
        let sources = Sources::of(voices);
        // Spec: operator R4. Each voice's ratios, from its velocity.
        let mut ratios = [[0.0f64; OPERATORS]; MAX_VOICES];
        for voice in voices.active_voices() {
            for (ratio, op) in ratios[voice].iter_mut().zip(&settings.ops) {
                let shift = (f64::from(op.vel) * sources.velocity(voice)).exp2();
                let mut coarse = f64::from(op.coarse) * shift;
                if op.quantise && op.vel != 0.0 {
                    coarse = if coarse < 0.75 {
                        0.5
                    } else {
                        coarse.round().min(48.0)
                    };
                }
                *ratio = coarse + f64::from(op.fine) / 1000.0;
            }
        }
        for i in 0..frames.len() {
            for voice in voices.active_voices() {
                let note = 440.0 * ((state.pitch(voices, voice, &frames, i) - 69.0) / 12.0).exp2();
                let mut steps = [0u32; OPERATORS];
                let mut amps = [0.0f32; OPERATORS];
                let mut feedbacks = [0.0f32; OPERATORS];
                for (op, s) in settings.ops.iter().enumerate() {
                    let moves = operator(voice, op, i);
                    let fixed = f64::from(s.freq) * f64::from(s.multi);
                    let mut hz = if s.fixed {
                        fixed
                    } else {
                        note * ratios[voice][op]
                    };
                    if moves.semitones != 0.0 {
                        hz *= (f64::from(moves.semitones) / 12.0).exp2();
                    }
                    steps[op] = increment(hz, internal);
                    let silent = s.fixed && hz >= FIXED_TOP * self.rate;
                    amps[op] = if s.on && !silent {
                        s.level * moves.gain
                    } else {
                        0.0
                    };
                    feedbacks[op] = (s.feedback + moves.feedback).clamp(0.0, 1.0);
                }
                let mut table_levels = [0usize; OPERATORS];
                for (op, level) in table_levels.iter_mut().enumerate() {
                    let scaled = (f64::from(steps[op]) * darker).min(f64::from(u32::MAX)) as u32;
                    let cached = &mut self.levels[voice][op];
                    if cached.0 != scaled {
                        let table = self.tables[0].as_ref().expect("Sine is a table");
                        *cached = (scaled, table.level_for(scaled));
                    }
                    *level = cached.1;
                }
                let place = pan(voice, i);
                for sub in 0..factor {
                    let out = self.step(voice, algorithm, &steps, &amps, &feedbacks, &table_levels);
                    let at = (frames.start + i) * factor + sub;
                    self.mix[0][at] += out * normal * place[0];
                    self.mix[1][at] += out * normal * place[1];
                }
            }
        }
    }

    /// One frame at the operators' rate for `voice`: D first and A last
    /// (R1, R2), each one's phase moved by its modulators' outputs and by
    /// its feedback if nothing modulates it (R3). Returns the carriers' sum.
    fn step(
        &mut self,
        voice: usize,
        algorithm: usize,
        steps: &[u32; OPERATORS],
        amps: &[f32; OPERATORS],
        feedbacks: &[f32; OPERATORS],
        levels: &[usize; OPERATORS],
    ) -> f32 {
        let carriers = carriers(algorithm);
        let mut outputs = [0.0f32; OPERATORS];
        let mut mix = 0.0;
        for op in (0..OPERATORS).rev() {
            let s = self.settings.ops[op];
            let phase = if s.wave == NOISE_LOOPED {
                // The loop's position, in samples at the operators' rate.
                (self.phase[voice][op] + 1) % (self.looped.len() * self.factor) as u32
            } else {
                self.phase[voice][op].wrapping_add(steps[op])
            };
            self.phase[voice][op] = phase;
            let modulators = MODULATORS[algorithm][op];
            let mut moved = 0.0;
            for (m, out) in outputs.iter().enumerate() {
                if modulators >> m & 1 == 1 {
                    moved += out;
                }
            }
            moved *= INDEX;
            let [last, before] = self.history[voice][op];
            if modulators == 0 {
                moved += feedbacks[op] * FEEDBACK * (last + before) * 0.5;
            }
            let out = if amps[op] == 0.0 {
                0.0
            } else {
                let at = phase.wrapping_add((moved * UNITS_PER_RADIAN) as i64 as u32);
                amps[op] * self.read(op, at, levels[op])
            };
            self.history[voice][op] = [out, last];
            outputs[op] = out;
            if carriers >> op & 1 == 1 {
                mix += out;
            }
        }
        mix
    }

    /// Operator `op`'s wave at `phase`: from its table at mipmap `level`,
    /// or the naive and noise waves (R6).
    fn read(&mut self, op: usize, phase: u32, level: usize) -> f32 {
        let s = &self.settings.ops[op];
        let interpolation = self.settings.interpolation;
        match s.wave {
            SAW_D => (phase as i32) as f32 / 2_147_483_648.0,
            SQUARE_D => {
                if (phase as i32) >= 0 {
                    1.0
                } else {
                    -1.0
                }
            }
            NOISE_WHITE => self.noise.next_bipolar() as f32,
            NOISE_LOOPED => {
                let (index, fraction) = (
                    phase as usize / self.factor,
                    (phase as usize % self.factor) as f32 / self.factor as f32,
                );
                let next = self.looped[(index + 1) % self.looped.len()];
                let this = self.looped[index];
                if interpolation {
                    this + (next - this) * fraction
                } else {
                    this
                }
            }
            wave => {
                let table = if wave == USER {
                    &self.user[op]
                } else {
                    self.tables[wave].as_ref().expect("a table wave")
                };
                let wave = table.wave(level, 0);
                if interpolation {
                    wave.read(phase)
                } else {
                    // The cycle's own sample at or below the phase: the
                    // phase of the next sample start, rounded up.
                    let size = wave.size() as u64;
                    let index = (u64::from(phase) * size) >> 32;
                    wave.read((index << 32).div_ceil(size) as u32)
                }
            }
        }
    }

    /// Ends a stretch of `frames`: the mix, decimated if it was made at
    /// twice the rate (R9), into `left` and `right`.
    pub fn finish(&mut self, frames: usize, left: &mut [f32], right: &mut [f32]) {
        let outs = [&mut left[..frames], &mut right[..frames]];
        if self.factor == 1 {
            for (out, mix) in outs.into_iter().zip(&self.mix) {
                out.copy_from_slice(&mix[..frames]);
            }
            return;
        }
        for (channel, out) in outs.into_iter().enumerate() {
            out.fill(0.0);
            let mix = &self.mix[channel];
            let mut at = 0;
            self.oversampler.process(channel, out, |up| {
                up.copy_from_slice(&mix[at..at + up.len()]);
                at += up.len();
            });
        }
    }
}

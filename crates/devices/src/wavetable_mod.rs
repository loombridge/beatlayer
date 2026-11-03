//! Wavetable's modulation sources (WP-8.10; the wavetable spec, R27, R28 and
//! R30): the three envelopes with their loop modes, the two LFOs' waves, and
//! the six unison modes' laws. Pure and allocation-free: the device runs one
//! [`Envelope`] and [`Lfo`] per voice and reads the laws per lane.

/// An envelope's loop modes (R27).
pub const LOOP_MODES: [&str; 3] = ["None", "Trigger", "Loop"];
/// An LFO's waves (R28).
pub const LFO_WAVES: [&str; 5] = ["Sine", "Saw", "Triangle", "Square", "Random"];
/// Unison's modes (R30).
pub const UNISON_MODES: [&str; 7] = [
    "None",
    "Classic",
    "Shimmer",
    "Noise",
    "Phase Sync",
    "Position Spread",
    "Random Note",
];
/// A synced LFO's divisions, 1/64 to 8 bars, and their length in quarter
/// notes (R28).
pub const DIVISIONS: [(&str, f64); 10] = [
    ("1/64", 0.0625),
    ("1/32", 0.125),
    ("1/16", 0.25),
    ("1/8", 0.5),
    ("1/4", 1.0),
    ("1/2", 2.0),
    ("1 bar", 4.0),
    ("2 bars", 8.0),
    ("4 bars", 16.0),
    ("8 bars", 32.0),
];

/// The unison modes by place in [`UNISON_MODES`].
pub const NO_UNISON: usize = 0;
pub const CLASSIC: usize = 1;
pub const SHIMMER: usize = 2;
pub const NOISE: usize = 3;
pub const PHASE_SYNC: usize = 4;
pub const POSITION_SPREAD: usize = 5;
pub const RANDOM_NOTE: usize = 6;

/// How an envelope loops (R27).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Loop {
    /// Holds Sustain until the note ends.
    None,
    /// Runs Attack, Decay and Release once from the note's start.
    Trigger,
    /// Runs Attack, Decay and Release to Final and starts again, until the
    /// note ends.
    Loop,
    /// Runs Attack and Decay and starts again from where Decay ended, while
    /// the note lasts, then releases (Meld's AD Loop).
    AdLoop,
}

impl Loop {
    /// The mode at `index` of [`LOOP_MODES`], or of Meld's four.
    #[must_use]
    pub fn of(index: usize) -> Self {
        match index {
            1 => Self::Trigger,
            2 => Self::Loop,
            3 => Self::AdLoop,
            _ => Self::None,
        }
    }
}

/// What an envelope reads for a frame: each segment's progress per frame and
/// shape exponent, Sustain as a share of Peak, and the levels it moves
/// between (R27).
#[derive(Clone, Copy, Debug)]
pub struct EnvParams {
    pub step: [f32; 3],
    pub power: [f32; 3],
    pub sustain: f32,
    pub initial: f32,
    pub peak: f32,
    pub fin: f32,
    pub mode: Loop,
}

impl EnvParams {
    /// The Amp envelope's levels: up from silence to 1, and down to silence.
    pub const AMP: Self = Self {
        step: [1.0; 3],
        power: [1.0; 3],
        sustain: 1.0,
        initial: 0.0,
        peak: 1.0,
        fin: 0.0,
        mode: Loop::None,
    };
}

/// A segment's shape exponent for slope `slope`: x^(2^(−2s)) of its share of
/// the way ([Operator](../../docs/specs/operator.md) R13).
#[must_use]
pub fn shape_power(slope: f32) -> f32 {
    2f32.powf(-2.0 * slope)
}

/// A segment's progress per frame for a time of `ms` at `rate`, `speed`
/// times as fast.
#[must_use]
pub fn segment_step(ms: f64, rate: f64, speed: f64) -> f32 {
    (speed / (ms * 1e-3 * rate).max(1.0)) as f32
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Stage {
    Idle,
    Attack,
    Decay,
    Sustain,
    Release,
}

/// One voice's envelope: Attack from Initial to Peak, Decay to Sustain of
/// Peak, Release to Final (R27).
#[derive(Clone, Copy, Debug)]
pub struct Envelope {
    stage: Stage,
    level: f32,
    /// How far through the segment, 0 to 1, and the level it started from.
    x: f32,
    from: f32,
    released: bool,
}

impl Envelope {
    pub const IDLE: Self = Self {
        stage: Stage::Idle,
        level: 0.0,
        x: 0.0,
        from: 0.0,
        released: false,
    };

    /// A note starts, rising from `from`: Initial for a fresh voice, the
    /// level a stolen voice had reached (instrument voices R4).
    pub fn start(&mut self, from: f32) {
        self.from = from;
        self.level = from;
        self.x = 0.0;
        self.released = false;
        self.stage = Stage::Attack;
    }

    /// The level now.
    #[must_use]
    pub fn level(&self) -> f32 {
        self.level
    }

    /// Whether the envelope has run out.
    #[must_use]
    pub fn is_idle(&self) -> bool {
        self.stage == Stage::Idle
    }

    /// The note ends: Release from where the level is, unless the mode runs
    /// Trigger's segments whatever the note's length.
    pub fn release(&mut self, mode: Loop) {
        if self.stage != Stage::Idle && mode != Loop::Trigger {
            self.released = true;
            self.begin_release();
        }
    }

    fn begin_release(&mut self) {
        self.from = self.level;
        self.x = 0.0;
        self.stage = Stage::Release;
    }

    /// The level of the next frame.
    pub fn next(&mut self, p: &EnvParams) -> f32 {
        match self.stage {
            Stage::Idle => {}
            Stage::Attack => {
                self.x += p.step[0];
                if self.x >= 1.0 {
                    (self.level, self.x, self.stage) = (p.peak, 0.0, Stage::Decay);
                    self.from = p.peak;
                } else {
                    self.level = self.from + (p.peak - self.from) * self.x.powf(p.power[0]);
                }
            }
            Stage::Decay => {
                let to = p.peak * p.sustain;
                self.x += p.step[1];
                if self.x >= 1.0 {
                    self.level = to;
                    match p.mode {
                        Loop::None => self.stage = Stage::Sustain,
                        Loop::AdLoop if !self.released => {
                            (self.from, self.x, self.stage) = (to, 0.0, Stage::Attack);
                        }
                        _ => self.begin_release(),
                    }
                } else {
                    self.level = p.peak + (to - p.peak) * self.x.powf(p.power[1]);
                }
            }
            Stage::Sustain => self.level = p.peak * p.sustain,
            Stage::Release => {
                self.x += p.step[2];
                if self.x >= 1.0 {
                    self.level = p.fin;
                    if p.mode == Loop::Loop && !self.released {
                        (self.from, self.x, self.stage) = (p.fin, 0.0, Stage::Attack);
                    } else {
                        self.stage = Stage::Idle;
                    }
                } else {
                    self.level = self.from + (p.fin - self.from) * self.x.powf(p.power[2]);
                }
            }
        }
        self.level
    }
}

/// What an LFO reads for a frame: its wave, Shape (−1 to 1) and Amount (0 to
/// 1) (R28).
#[derive(Clone, Copy, Debug)]
pub struct LfoParams {
    pub wave: usize,
    pub shape: f32,
    pub amount: f32,
}

/// One voice's LFO: a phase in cycles, the count of whole cycles it has run
/// (which a Random wave draws its values by), and the Attack's fade-in.
#[derive(Clone, Copy, Debug)]
pub struct Lfo {
    pub phase: f64,
    pub cycles: u64,
    /// The fade-in's progress, 0 to 1.
    pub attack: f32,
    /// What a Random wave's draws are seeded by, besides the cycle.
    pub salt: u64,
}

impl Lfo {
    pub const IDLE: Self = Self {
        phase: 0.0,
        cycles: 0,
        attack: 1.0,
        salt: 0,
    };

    /// A note starts the LFO at `phase` cycles (Retrigger on).
    pub fn start(&mut self, phase: f64, salt: u64) {
        self.phase = phase.rem_euclid(1.0);
        self.cycles = 0;
        self.attack = 0.0;
        self.salt = salt;
    }

    /// Runs on by `step` cycles.
    pub fn run(&mut self, step: f64) {
        self.phase += step;
        if self.phase >= 1.0 {
            let whole = self.phase.floor();
            self.phase -= whole;
            self.cycles = self.cycles.wrapping_add(whole as u64);
        }
    }

    /// Takes the song's place: `cycles` whole and part cycles from the
    /// song's start (an LFO following the transport, R28).
    pub fn follow(&mut self, cycles: f64) {
        let whole = cycles.floor();
        self.phase = cycles - whole;
        self.cycles = whole as i64 as u64;
    }

    /// The wave's value now, −1 to 1, before Amount and Attack.
    #[must_use]
    pub fn wave(&self, p: &LfoParams) -> f32 {
        let phase = self.phase as f32;
        let shape = p.shape.clamp(-1.0, 1.0);
        let warped = phase.powf(2f32.powf(-shape));
        match p.wave {
            0 => (core::f32::consts::TAU * warped).sin(),
            1 => 2.0 * warped - 1.0,
            2 => {
                let peak = ((1.0 + shape) / 2.0).clamp(0.001, 0.999);
                if phase < peak {
                    -1.0 + 2.0 * phase / peak
                } else {
                    1.0 - 2.0 * (phase - peak) / (1.0 - peak)
                }
            }
            3 => {
                let width = 0.5 + 0.45 * shape;
                if phase < width { 1.0 } else { -1.0 }
            }
            _ => {
                let draw = unit(self.salt ^ self.cycles.wrapping_mul(0x9E37_79B9_7F4A_7C15));
                draw.signum() * draw.abs().powf(2f32.powf(-2.0 * shape))
            }
        }
    }

    /// The LFO's output: its wave times Amount and the Attack's fade-in.
    #[must_use]
    pub fn value(&self, p: &LfoParams) -> f32 {
        self.wave(p) * p.amount * self.attack
    }
}

/// A uniform value from −1 to 1 drawn by `seed` (splitmix64).
#[must_use]
pub fn unit(seed: u64) -> f32 {
    let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    ((z >> 40) as f32 / (1u64 << 23) as f32) - 1.0
}

/// What one unison copy plays beyond Classic's pan: its pitch and its
/// position in the table, as a share of it (R30).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct UnisonCopy {
    pub cents: f32,
    pub position: f32,
}

/// Copy `i` of `n`'s even place from −1 to 1.
fn place(i: usize, n: usize) -> f32 {
    if n < 2 {
        0.0
    } else {
        2.0 * i as f32 / (n - 1) as f32 - 1.0
    }
}

/// Copy `i` of `n` at Amount `a` (0 to 1) in unison mode `mode` (R30).
/// `walk` is the copy's smoothed random pair, each from −1 to 1, and `note`
/// the pair drawn when its note started.
#[must_use]
pub fn copy(mode: usize, i: usize, n: usize, a: f32, walk: [f32; 2], note: [f32; 2]) -> UnisonCopy {
    let x = place(i, n);
    match mode {
        CLASSIC | PHASE_SYNC => UnisonCopy {
            cents: x * 25.0 * a,
            position: 0.0,
        },
        SHIMMER | NOISE => UnisonCopy {
            cents: walk[0] * 12.0 * a,
            position: walk[1] * 0.02 * a,
        },
        POSITION_SPREAD => UnisonCopy {
            cents: x * 5.0 * a,
            position: x * 0.25 * a,
        },
        RANDOM_NOTE => UnisonCopy {
            cents: note[0] * 25.0 * a,
            position: note[1] * 0.5 * a,
        },
        _ => UnisonCopy::default(),
    }
}

/// How often Shimmer's and Noise's walks draw a new target, in hertz.
#[must_use]
pub fn walk_rate(mode: usize) -> f64 {
    if mode == NOISE { 40.0 } else { 4.0 }
}

/// The walk's pair for lane `lane` at frame `clock` of the device: two
/// draws a cycle of the walk's rate apart, joined by a straight line, from
/// the device's `seed`. A function of the clock alone, so it is the same at
/// any block size.
#[must_use]
pub fn walk(seed: u64, lane: usize, clock: u64, rate: f64, mode: usize) -> [f32; 2] {
    let period = (rate / walk_rate(mode)).max(1.0);
    let at = clock as f64 / period;
    let (k, t) = (at.floor() as u64, (at - at.floor()) as f32);
    let draw = |k: u64, which: u64| {
        unit(seed ^ (lane as u64).wrapping_mul(0xD6E8_FEB8_6659_FD93) ^ (k << 1 | which) << 20)
    };
    let line = |which| draw(k, which) * (1.0 - t) + draw(k + 1, which) * t;
    [line(0), line(1)]
}

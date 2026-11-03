//! Operator's envelopes (WP-8.12; spec: operator R12 to R15): the stages,
//! slopes and loop modes of the seven envelopes, stepped a frame at a time
//! so each one is the same at any block size. An envelope doesn't know the
//! song: the device gives it the song's beats, counted at the tempo.

/// Spec: operator R12. Decay and Release fall exponentially: a segment
/// leaves `e^-EXPONENT` of its distance for its last frame, which it then
/// jumps (Beatlayer's own curve).
const EXPONENT: f32 = 8.0;

/// The loop modes' names, in the order of [`Loop`] (R14).
pub const LOOPS: [&str; 5] = ["None", "Loop", "Beat", "Sync", "Trigger"];

/// The Repeat values' names (R14), in the order of [`REPEAT_BEATS`].
pub const REPEATS: [&str; 8] = ["1/16", "1/8", "1/4", "1/2", "1", "2", "4", "8"];

/// What each Repeat is, in beats (Beatlayer's own list).
pub const REPEAT_BEATS: [f64; 8] = [0.0625, 0.125, 0.25, 0.5, 1.0, 2.0, 4.0, 8.0];

/// What an envelope does while its note is held and after (R14).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Loop {
    /// Attack, Decay, then Sustain until the note ends.
    #[default]
    None,
    /// At Sustain, back to Initial over Loop Time and Attack again.
    Loop,
    /// Starts again every Repeat after the note's start.
    Beat,
    /// As Beat, the first on the song's next sixteenth.
    Sync,
    /// Attack and Decay, then Release whatever the note's length.
    Trigger,
}

impl Loop {
    /// The loop mode at index `index` of [`LOOPS`].
    #[must_use]
    pub fn of(index: usize) -> Self {
        [
            Self::None,
            Self::Loop,
            Self::Beat,
            Self::Sync,
            Self::Trigger,
        ][index.min(4)]
    }
}

/// How an envelope's segments curve.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Curve {
    /// An oscillator's (R12): Attack straight in amplitude, Decay and
    /// Release exponential, Release to silence.
    Amplitude,
    /// The filter's, pitch's and LFO's (R13): the Attack, Decay and Release
    /// slopes, from −1 to 1.
    Slopes([f32; 3]),
}

/// One envelope's settings.
#[derive(Clone, Copy, Debug)]
pub struct Shape {
    pub initial: f32,
    pub peak: f32,
    pub sustain: f32,
    /// Where Release ends: 0 for an [`Curve::Amplitude`] envelope.
    pub end: f32,
    /// Attack, Decay and Release in seconds, before Time.
    pub attack: f32,
    pub decay: f32,
    pub release: f32,
    pub curve: Curve,
    pub mode: Loop,
    /// Loop Time in seconds.
    pub loop_secs: f32,
    /// Repeat in beats.
    pub repeat: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    Attack,
    Decay,
    Sustain,
    /// Loop's way back to Initial.
    Back,
    Release,
    Done,
}

/// A segment's share of the way after `progress` of its time, for slope
/// `slope` (R13): `x^(2^(−2 s))`.
fn bow(progress: f32, slope: f32) -> f32 {
    if slope == 0.0 {
        progress
    } else {
        progress.powf((-2.0 * slope).exp2())
    }
}

/// The share of the way left after `progress` of an exponential fall, from
/// 1 at the start to 0 at the end.
fn fall(progress: f32) -> f32 {
    let end = (-EXPONENT).exp();
    ((-EXPONENT * progress).exp() - end) / (1.0 - end)
}

/// One voice's envelope.
#[derive(Clone, Copy, Debug)]
pub struct State {
    stage: Stage,
    progress: f64,
    /// Where the segment under way began.
    from: f32,
    level: f32,
    /// The beat of the next restart, for Beat and Sync.
    next: f64,
}

impl State {
    /// An envelope that is finished: no note.
    pub const DONE: Self = Self {
        stage: Stage::Done,
        progress: 0.0,
        from: 0.0,
        level: 0.0,
        next: f64::INFINITY,
    };

    /// Its level now.
    #[must_use]
    pub fn level(&self) -> f32 {
        self.level
    }

    /// Whether it has finished its Release (or never began).
    #[must_use]
    pub fn is_done(&self) -> bool {
        self.stage == Stage::Done
    }

    /// Whether it has been released, or is on its way to release itself
    /// (Trigger).
    #[must_use]
    pub fn is_released(&self) -> bool {
        matches!(self.stage, Stage::Release | Stage::Done)
    }

    /// A note starts it from Initial at `beats`; `wait` is the beats to
    /// Sync's first restart, where the song's grid sets it, else a Repeat.
    pub fn start(&mut self, shape: &Shape, beats: f64, wait: Option<f64>) {
        self.stage = Stage::Attack;
        self.progress = 0.0;
        self.from = shape.initial;
        self.level = shape.initial;
        self.schedule(shape, beats, wait);
    }

    /// A note takes its voice (R4): the Attack begins again from the level
    /// it had reached.
    pub fn restart(&mut self, shape: &Shape, beats: f64, wait: Option<f64>) {
        self.begin_attack();
        self.schedule(shape, beats, wait);
    }

    fn schedule(&mut self, shape: &Shape, beats: f64, wait: Option<f64>) {
        self.next = match shape.mode {
            Loop::Beat => beats + shape.repeat,
            Loop::Sync => beats + wait.unwrap_or(shape.repeat),
            _ => f64::INFINITY,
        };
    }

    fn begin_attack(&mut self) {
        self.stage = Stage::Attack;
        self.progress = 0.0;
        self.from = self.level;
    }

    fn begin_release(&mut self) {
        self.stage = Stage::Release;
        self.progress = 0.0;
        self.from = self.level;
    }

    /// The note ends: Release from the level it has reached. Trigger
    /// ignores it (R14).
    pub fn release(&mut self, shape: &Shape) {
        if shape.mode != Loop::Trigger && !self.is_released() {
            self.begin_release();
        }
    }

    /// Advances one frame and gives the level. `tick` is the frame in
    /// seconds after Time and the note's sensitivities have scaled it, so
    /// that a segment of `t` seconds takes `t / tick` frames; `beats` is the
    /// song's beat at this frame, counted at the tempo.
    pub fn step(&mut self, shape: &Shape, tick: f32, beats: f64) -> f32 {
        if matches!(shape.mode, Loop::Beat | Loop::Sync)
            && !self.is_released()
            && beats >= self.next
        {
            self.begin_attack();
            while self.next <= beats {
                self.next += shape.repeat;
            }
        }
        let slopes = match shape.curve {
            Curve::Amplitude => [0.0; 3],
            Curve::Slopes(slopes) => slopes,
        };
        let amplitude = shape.curve == Curve::Amplitude;
        // A segment is done a millionth short of its end, so a run of small
        // steps that should add up to it does.
        let advance = |progress: &mut f64, seconds: f32| {
            *progress = if seconds <= tick {
                1.0
            } else {
                (*progress + f64::from(tick) / f64::from(seconds)).min(1.0)
            };
            *progress >= 1.0 - 1e-6
        };
        match self.stage {
            Stage::Attack => {
                let done = advance(&mut self.progress, shape.attack);
                self.level = if done {
                    shape.peak
                } else {
                    self.from + (shape.peak - self.from) * bow(self.progress as f32, slopes[0])
                };
                if done {
                    self.stage = Stage::Decay;
                    self.progress = 0.0;
                }
            }
            Stage::Decay => {
                let done = advance(&mut self.progress, shape.decay);
                self.level = if done {
                    shape.sustain
                } else if amplitude {
                    shape.sustain + (shape.peak - shape.sustain) * fall(self.progress as f32)
                } else {
                    shape.peak + (shape.sustain - shape.peak) * bow(self.progress as f32, slopes[1])
                };
                if done {
                    self.stage = Stage::Sustain;
                    self.progress = 0.0;
                }
            }
            Stage::Sustain => {
                self.level = shape.sustain;
                match shape.mode {
                    Loop::Trigger => self.begin_release(),
                    Loop::Loop => {
                        self.stage = Stage::Back;
                        self.progress = 0.0;
                        self.from = self.level;
                    }
                    _ => {}
                }
            }
            Stage::Back => {
                let done = advance(&mut self.progress, shape.loop_secs);
                self.level = self.from + (shape.initial - self.from) * self.progress as f32;
                if done {
                    self.level = shape.initial;
                    self.stage = Stage::Attack;
                    self.progress = 0.0;
                    self.from = shape.initial;
                }
            }
            Stage::Release => {
                let done = advance(&mut self.progress, shape.release);
                let end = if amplitude { 0.0 } else { shape.end };
                self.level = if done {
                    end
                } else if amplitude {
                    self.from * fall(self.progress as f32)
                } else {
                    self.from + (end - self.from) * bow(self.progress as f32, slopes[2])
                };
                if done {
                    self.stage = Stage::Done;
                }
            }
            Stage::Done => {}
        }
        self.level
    }
}

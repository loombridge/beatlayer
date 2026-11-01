//! The streaming pitch tracker: the Tuner's YIN detector (de Cheveigné and
//! Kawahara, "YIN, a fundamental frequency estimator for speech and music",
//! JASA 111(4), 2002) as a block-independent building block, with a
//! confidence, a voiced decision and an onset flag per measurement.

/// What a tracker is told about its input and what it listens for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Config {
    /// Samples per second.
    pub rate: f64,
    /// The lowest pitch looked for, in hertz.
    pub low: f64,
    /// The highest pitch looked for, in hertz.
    pub high: f64,
    /// Samples each measurement reads, at the newest end of the history.
    pub window: usize,
    /// New samples between one measurement's start and the next's.
    pub interval: usize,
    /// Whether a lag whose multiple fits the window far better is passed over
    /// for it, so a voice whose second harmonic is the loudest isn't read an
    /// octave up. Off, the first lag under the threshold is the period, as in
    /// the Tuner.
    pub octave_guard: bool,
}

impl Config {
    /// A window of one period of `low` and an interval of a quarter of it,
    /// for a voice between `low` and `high` hertz, with the range searched
    /// a twentieth wider at each end so the ends are read as well as the
    /// middle: a measurement reads the last `window + rate / low` samples
    /// and is ready one interval later, so a step in pitch is settled after
    /// about two periods of `low` and a little more.
    #[must_use]
    pub fn for_range(rate: f64, low: f64, high: f64) -> Self {
        let window = ((rate / low).ceil() as usize).max(128);
        Self {
            rate,
            low: low * 0.95,
            high: high * 1.05,
            window,
            interval: (window / 4).max(32),
            octave_guard: true,
        }
    }
}

/// One measurement.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pitch {
    /// The fundamental, in hertz; meaningful as a pitch only while `voiced`.
    pub frequency: f64,
    /// How periodic the window was, from 0 (not at all) to 1.
    pub confidence: f32,
    /// Whether the window was periodic enough to hold a pitch.
    pub voiced: bool,
    /// Whether a pitch began here: the first voiced measurement after
    /// silence, noise or a break, or one the loudness doubled at.
    pub onset: bool,
    /// How many samples the tracker had been given when the measured window
    /// ended; the window is the `window` samples before it.
    pub ended: u64,
}

/// The cumulative mean normalised difference under which a lag is a period.
const THRESHOLD: f32 = 0.15;
/// How much better, and how many times better, a longer lag has to repeat
/// the window than the first one found for the guard to take it.
const OCTAVE_MARGIN: f32 = 0.05;
const OCTAVE_RATIO: f32 = 0.3;
/// The window's RMS under which nothing is heard.
const GATE: f32 = 0.002;
/// How much louder than the last window one has to be to start a new pitch.
const SURGE: f32 = 2.0;

/// The sum of squared differences of two equal windows, in eight lanes so it
/// vectorises.
fn squared_difference(a: &[f32], b: &[f32]) -> f32 {
    let mut lanes = [0.0f32; 8];
    for (a, b) in a.as_chunks::<8>().0.iter().zip(b.as_chunks::<8>().0) {
        for k in 0..8 {
            let d = a[k] - b[k];
            lanes[k] += d * d;
        }
    }
    lanes.iter().sum()
}

/// A streaming monophonic pitch tracker.
///
/// [`push`](Self::push) the samples, then [`advance`](Self::advance) by the
/// number pushed: each `interval` samples a measurement starts on the last
/// `window`, and its work, the difference function over every lag, is spread
/// over the samples of the next interval, so no call costs more than its
/// share. A measurement ends within an interval of starting, and its result
/// is then waiting for [`pop`](Self::pop). Feeding the samples one at a time
/// ([`feed`](Self::feed)) makes the measurements fall on the same samples
/// however the caller cuts its blocks.
///
/// Everything is allocated in [`new`](Self::new).
pub struct Tracker {
    rate: f64,
    window: usize,
    interval: usize,
    octave_guard: bool,
    /// The lag range, in samples.
    min_tau: usize,
    max_tau: usize,
    ring: Vec<f32>,
    ring_pos: usize,
    heard: u64,
    since_analysis: usize,
    /// The ring in chronological order, taken when a measurement starts, so
    /// the difference function reads it without wrapping.
    scratch: Vec<f32>,
    analysing: bool,
    ended: u64,
    next_tau: usize,
    steps: usize,
    diff: Vec<f32>,
    cmnd: Vec<f32>,
    voiced_before: bool,
    rms_before: f32,
    rms: f32,
    /// Finished measurements not yet popped: a call finishes at most two.
    queue: [Option<Option<Pitch>>; 2],
    queued: usize,
}

impl Tracker {
    /// A tracker for `config`. Allocates.
    #[must_use]
    pub fn new(config: &Config) -> Self {
        let rate = config.rate;
        // The window plus the longest lag it is compared against, with one
        // lag to spare below `low`, so the lowest pitch is a whole period
        // inside the buffer at every rate.
        let longest = (rate / (config.low - 1.0)).ceil() as usize;
        let ring_len = config.window + longest + 2;
        Self {
            rate,
            window: config.window,
            interval: config.interval,
            octave_guard: config.octave_guard,
            min_tau: (rate / config.high).floor() as usize,
            max_tau: (rate / config.low).ceil() as usize,
            ring: vec![0.0; ring_len],
            ring_pos: 0,
            heard: 0,
            since_analysis: 0,
            scratch: vec![0.0; ring_len],
            analysing: false,
            ended: 0,
            next_tau: 0,
            steps: 0,
            diff: vec![0.0; longest + 4],
            cmnd: vec![1.0; longest + 4],
            voiced_before: false,
            rms_before: 0.0,
            rms: 0.0,
            queue: [None; 2],
            queued: 0,
        }
    }

    /// The longest period, in samples, a measurement can give.
    #[must_use]
    pub fn longest(&self) -> usize {
        self.max_tau
    }

    /// The samples of a measurement.
    #[must_use]
    pub fn window(&self) -> usize {
        self.window
    }

    /// The samples between measurements.
    #[must_use]
    pub fn interval(&self) -> usize {
        self.interval
    }

    /// Samples between the middle of a measured window and the moment its
    /// result is ready, at most: half a window and an interval.
    #[must_use]
    pub fn latency(&self) -> usize {
        self.window / 2 + self.interval
    }

    /// Difference-function steps taken since the last [`advance`](Self::advance)
    /// began.
    #[must_use]
    pub fn steps(&self) -> usize {
        self.steps
    }

    /// Adds `sample` to the history. Doesn't count time: call
    /// [`advance`](Self::advance) with the number pushed.
    pub fn push(&mut self, sample: f32) {
        self.ring[self.ring_pos] = sample;
        self.ring_pos = (self.ring_pos + 1) % self.ring.len();
        self.heard += 1;
    }

    /// `push` of one sample, then `advance` by one.
    pub fn feed(&mut self, sample: f32) {
        self.push(sample);
        self.advance(1);
    }

    /// Counts `frames` more samples pushed: starts a measurement each
    /// interval and does this call's share of the one under way.
    pub fn advance(&mut self, frames: usize) {
        self.steps = 0;
        self.since_analysis += frames;
        if self.since_analysis >= self.interval {
            self.since_analysis = 0;
            // shortcut: block sizes that vary can leave the last measurement
            // short of its share; it is finished here, in one go, then.
            self.analyse(self.interval);
            self.begin();
        }
        self.analyse(frames);
    }

    /// The oldest finished measurement not yet taken: `Some(None)` when the
    /// window was too quiet or held no lag to read a pitch from.
    pub fn pop(&mut self) -> Option<Option<Pitch>> {
        if self.queued == 0 {
            return None;
        }
        let first = self.queue[0];
        self.queue[0] = self.queue[1];
        self.queued -= 1;
        first
    }

    /// `period`, found at lag `tau`, measured again at the longest multiple of
    /// it that fits the window, up to eight (a longer lag would take longer to
    /// settle after a change of pitch): an interpolation's error in
    /// samples is the same however long the lag, so a short period is read
    /// that many times more finely.
    fn refine(&self, tau: usize, period: f64) -> f64 {
        // Eight lags of room keep the lags it reads inside what was computed.
        let multiple = (self.window.min(self.max_tau - 8) / tau).min(8);
        if multiple < 2 {
            return period;
        }
        let near = multiple * tau;
        let reach = multiple / 2 + 1;
        let lag = (near - reach..=near + reach)
            .min_by(|a, b| self.diff[*a].total_cmp(&self.diff[*b]))
            .unwrap_or(near);
        if lag == near - reach || lag == near + reach {
            return period;
        }
        let (alpha, beta, gamma) = (self.diff[lag - 1], self.diff[lag], self.diff[lag + 1]);
        let denom = 2.0 * (alpha - 2.0 * beta + gamma);
        let delta = if denom.abs() > 1e-12 {
            f64::from((alpha - gamma) / denom)
        } else {
            0.0
        };
        (lag as f64 + delta) / multiple as f64
    }

    /// The first lag the difference function is computed at. YIN's running
    /// mean runs from lag 1; the Tuner's starts at the shortest lag it reads.
    fn first_tau(&self) -> usize {
        if self.octave_guard {
            1
        } else {
            self.min_tau.saturating_sub(1)
        }
    }

    fn finish(&mut self, found: Option<Pitch>) {
        self.voiced_before = found.is_some_and(|p| p.voiced);
        if self.queued == self.queue.len() {
            // The oldest is lost if nobody took it.
            self.queue[0] = self.queue[1];
            self.queued -= 1;
        }
        self.queue[self.queued] = Some(found);
        self.queued += 1;
    }

    /// Snapshots the last window and starts measuring it; a quiet one
    /// finishes at once.
    fn begin(&mut self) {
        let len = self.ring.len();
        let (oldest, newest) = self.scratch.split_at_mut(len - self.ring_pos);
        oldest.copy_from_slice(&self.ring[self.ring_pos..]);
        newest.copy_from_slice(&self.ring[..self.ring_pos]);

        let window = &self.scratch[len - self.window..];
        self.rms = (window.iter().map(|s| s * s).sum::<f32>() / self.window as f32).sqrt();
        if self.rms < GATE {
            self.analysing = false;
            self.rms_before = 0.0;
            self.finish(None);
            return;
        }
        self.analysing = true;
        self.ended = self.heard;
        self.next_tau = self.first_tau();
    }

    /// Computes `frames` samples' share of the difference function, and ends
    /// the measurement after its last lag.
    fn analyse(&mut self, frames: usize) {
        if !self.analysing {
            return;
        }
        let first = self.first_tau();
        let last = (self.max_tau + 1).min(self.diff.len() - 1);
        let quota = ((last + 1 - first) * frames).div_ceil(self.interval);
        let stop = (self.next_tau + quota).min(last + 1);
        let len = self.scratch.len();
        let now = &self.scratch[len - self.window..];
        for tau in self.next_tau..stop {
            let back = &self.scratch[len - self.window - tau..len - tau];
            self.diff[tau] = squared_difference(back, now);
        }
        self.steps += (stop - self.next_tau) * self.window;
        self.next_tau = stop;
        if stop <= last {
            return;
        }
        self.analysing = false;
        let found = self.read();
        self.finish(found);
    }

    /// The pitch of the finished difference function.
    fn read(&mut self) -> Option<Pitch> {
        let (min_tau, max_tau) = (self.min_tau, self.max_tau);

        // Cumulative mean normalised difference.
        self.cmnd[0] = 1.0;
        let mut running = 0.0f32;
        for tau in 1..=max_tau {
            running += self.diff[tau];
            self.cmnd[tau] = if running > 1e-12 {
                self.diff[tau] * (tau as f32) / running
            } else {
                1.0
            };
        }

        // The first minimum under the threshold.
        let mut best = None;
        for tau in min_tau..max_tau {
            if self.cmnd[tau] < THRESHOLD
                && self.cmnd[tau] < self.cmnd[tau - 1]
                && self.cmnd[tau] <= self.cmnd[tau + 1]
            {
                best = Some(tau);
                break;
            }
        }
        let voiced = best.is_some();
        if let Some(tau) = best.as_mut().filter(|_| self.octave_guard) {
            // A lag a few times as long, at which the window repeats far
            // better, is the period, the shorter lag a harmonic of it: the
            // shortest such lag is it.
            let found = self.cmnd[*tau];
            for multiple in 2..=4 {
                let near = multiple * *tau;
                if near + 2 > max_tau {
                    break;
                }
                let (value, at) = (near - 2..=near + 2)
                    .map(|lag| (self.cmnd[lag], lag))
                    .fold((f32::MAX, 0), |a, b| if b.0 < a.0 { b } else { a });
                if found - value > OCTAVE_MARGIN && value < OCTAVE_RATIO * found {
                    *tau = at;
                    break;
                }
            }
        }

        // Failing that, the deepest minimum of the difference function.
        if best.is_none() {
            let mut deepest = f32::MAX;
            for tau in min_tau..max_tau {
                if self.diff[tau] < self.diff[tau - 1]
                    && self.diff[tau] <= self.diff[tau + 1]
                    && self.diff[tau] < deepest
                {
                    deepest = self.diff[tau];
                    best = Some(tau);
                }
            }
        }
        let tau = best?;

        // Parabolic interpolation on the difference function.
        let alpha = self.diff[tau - 1];
        let beta = self.diff[tau];
        let gamma = self.diff[tau + 1];
        let denom = 2.0 * (alpha - 2.0 * beta + gamma);
        let delta = if denom.abs() > 1e-12 {
            (alpha - gamma) / denom
        } else {
            0.0
        };
        let mut period = tau as f64 + f64::from(delta);
        if self.octave_guard {
            period = self.refine(tau, period);
        }
        let onset = voiced && (!self.voiced_before || self.rms > SURGE * self.rms_before);
        self.rms_before = self.rms;
        Some(Pitch {
            frequency: self.rate / period,
            confidence: (1.0 - self.cmnd[tau]).clamp(0.0, 1.0),
            voiced,
            onset,
            ended: self.ended,
        })
    }
}

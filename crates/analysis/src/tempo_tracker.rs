//! Real-time tempo tracking for Tempo Follower (WP-9.12, the Tempo Follower
//! spec's R11 to R18).
//!
//! Three parts, on two threads:
//!
//! 1. **The copy** ([`Copier`], R11) runs on the audio thread: it sums the
//!    chosen input channel (or pair) to mono into a lock-free ring, and drops
//!    and counts a block that doesn't fit. It allocates nothing and takes no
//!    lock.
//! 2. **The tracker** ([`Tracker`], R12) runs on a thread of its own
//!    ([`Tracker::spawn`]) and reads the ring into an [`Analyser`].
//! 3. **The analyser** is the method, with no threads or clocks so that
//!    tests feed it audio directly (R18). It downsamples to 11,025 Hz and
//!    takes frames of 128 samples every 110 (about 10 ms). Each frame's onset
//!    strength is its half-wave rectified spectral flux with a maximum filter
//!    across frequency (SuperFlux, Böck and Widmer 2013), compressed as in
//!    [`crate::onsets`]. Every quarter second a comb-filter bank scores
//!    each tempo from 60 to 200 BPM, in steps of 0.25 BPM, on the last 6
//!    seconds of onset strength: the correlation of the strength with itself
//!    one to four beats earlier, averaged, newer strength counting for more
//!    (after Ellis 2007 and Davies and Plumbley 2007). Its strongest peak, refined with a parabola, is the
//!    tempo, and its height is the confidence. The beat's phase is the
//!    alignment of a beat grid at that tempo with the last six beats of
//!    onset strength, corrected by a fraction of its error each time and
//!    advanced by the tempo in between (the causal beat tracker of Stark,
//!    Davies and Plumbley 2009).
//!
//! Between tempos whose evidence is close, a factor of 2 apart or not, the
//! analyser takes the one nearest the tempo it follows, and at the start the
//! one nearest the set's tempo; a move to a tempo more than 8% from the one
//! it follows needs 4 seconds of agreeing evidence (R13). Under
//! [`CONFIDENT`] it keeps its last confident tempo, and the tempo it gives
//! moves by at most 2% a second except when it first becomes confident
//! (R14).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use rtrb::{Consumer, Producer, RingBuffer};

use crate::stft::Stft;

/// The rate the analysis runs at, in Hz.
const RATE: f64 = 11_025.0;
/// Samples in a frame, and between two frames.
const FRAME: usize = 128;
const HOP: usize = 110;
/// Onset strength values a second.
const FRAME_RATE: f64 = RATE / HOP as f64;
/// Onset strength values kept: 6 seconds.
const HISTORY: usize = (6.0 * FRAME_RATE) as usize + 1;
/// Frames between two estimates: a quarter second.
const EVERY: usize = (0.25 * FRAME_RATE) as usize;
/// Frames of history before the first estimate: 3 seconds.
const WARM: usize = (3.0 * FRAME_RATE) as usize;
/// The fewest frames two copies of the strength must overlap by to be correlated.
const OVERLAP: usize = FRAME_RATE as usize;
/// The tempos scored, in BPM.
const SLOWEST: f64 = 60.0;
const FASTEST: f64 = 200.0;
const STEP: f64 = 0.25;
/// The time constant, in seconds, of how much less older strength counts, which
/// keeps the tempo from lagging far behind a ramp.
const RECENCY: f64 = 1.0;
/// The variance of the strength, in squared compressed units, under which it is flat.
const FLAT: f64 = 1e-3;
/// How many beats back each correlation looks.
const BEATS_BACK: usize = 4;
/// The beat grid's alignments tried, and how many beats it is laid over.
const PHASES: usize = 48;
const GRID_BEATS: usize = 6;
/// A peak is as good as the strongest when it reaches this share of its
/// height, or this share if it is in a ratio of 2 to the strongest, whose
/// evidence is ambiguous by nature.
const CLOSE: f64 = 0.9;
const CLOSE_OCTAVE: f64 = 0.75;
/// How far from 2 or 1/2 a ratio of tempos may be to count as one, as a fraction.
const OCTAVE_SLACK: f64 = 0.04;
/// A tempo this near (as a fraction) the one followed is a move to follow;
/// a tempo further off must be the estimate for [`AGREE_SECONDS`] first.
const NEAR: f64 = 0.08;
const AGREE_SECONDS: f64 = 4.0;
/// Confidence at or above which the tempo is followed (R14).
pub const CONFIDENT: f64 = 0.3;
/// The most the tempo moves in a second, as a fraction (R14).
const SLEW: f64 = 0.02;
/// How much of its change each estimate puts through to the confidence, to
/// the target tempo and to the phase's error.
const CONFIDENCE_GAIN: f64 = 0.5;
const TEMPO_GAIN: f64 = 0.5;
const PHASE_GAIN: f64 = 0.3;

/// The onset strength of a held chord's beating partials stays under this, in
/// the compressed units of [`COMPRESSION`], whatever its level; a hit's is far
/// above it. What is under it counts for nothing.
const FLUX_FLOOR: f64 = 2.5;
const COMPRESSION: f64 = 1_000.0;

/// What the tracker gives.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Estimate {
    /// Beats a minute: the last tempo the tracker was confident of, or the
    /// set's tempo before it ever was.
    pub bpm: f64,
    /// How far through its beat the audio is, from 0 (on the beat) to 1.
    pub phase: f64,
    /// How strongly the audio's onsets repeat at that tempo, from 0 to 1,
    /// smoothed. Under [`CONFIDENT`] `bpm` is being held.
    pub confidence: f64,
}

/// The tracker's method with no ring or thread: feed it audio, read what it
/// gives. The same audio from the same start gives the same estimates (R18).
pub struct Analyser {
    /// Input samples to an analysis sample.
    ratio: f64,
    /// How much of the analysis sample being averaged is in `sum`, in input samples.
    filled: f64,
    sum: f64,
    frame: [f64; FRAME],
    fill: usize,
    stft: Stft,
    norm: f64,
    previous: [f64; FRAME / 2 + 1],
    strength: Vec<f64>,
    /// The next slot of `strength`, and how many hold values.
    head: usize,
    held: usize,
    frames: usize,
    /// The strength in time order, less its mean.
    line: Vec<f64>,
    /// How much the strength `k` frames before the newest counts.
    recency: Vec<f64>,
    scores: Vec<f64>,
    confidence: f64,
    bpm: f64,
    /// Where the tempo is heading, before the slew.
    target: f64,
    /// Whether an estimate was ever confident.
    started: bool,
    /// A tempo a factor of 2 from the followed one, and the frame it was first seen.
    pending: Option<(f64, usize)>,
    phase: f64,
}

impl Analyser {
    /// An analyser of audio at `rate` Hz (mono), for a set at `set_bpm`.
    #[must_use]
    pub fn new(rate: u32, set_bpm: f64) -> Self {
        let stft = Stft::new(FRAME);
        let norm = 2.0 / stft.window_sum();
        let candidates = ((FASTEST - SLOWEST) / STEP) as usize + 1;
        Self {
            ratio: (f64::from(rate) / RATE).max(1.0),
            filled: 0.0,
            sum: 0.0,
            frame: [0.0; FRAME],
            fill: 0,
            stft,
            norm,
            previous: [0.0; FRAME / 2 + 1],
            strength: vec![0.0; HISTORY],
            head: 0,
            held: 0,
            frames: 0,
            line: vec![0.0; HISTORY],
            recency: (0..HISTORY)
                .map(|k| (-(k as f64) / (RECENCY * FRAME_RATE)).exp())
                .collect(),
            scores: vec![0.0; candidates],
            confidence: 0.0,
            bpm: set_bpm,
            target: set_bpm,
            started: false,
            pending: None,
            phase: 0.0,
        }
    }

    /// What the tracker gives now.
    #[must_use]
    pub fn estimate(&self) -> Estimate {
        Estimate {
            bpm: self.bpm,
            phase: self.phase,
            confidence: self.confidence,
        }
    }

    /// Takes the next `mono` samples of the input.
    pub fn push(&mut self, mono: &[f32]) {
        for &sample in mono {
            let x = f64::from(sample);
            let room = self.ratio - self.filled;
            if room > 1.0 + 1e-9 {
                self.sum += x;
                self.filled += 1.0;
            } else {
                // The box of input samples averaged into one analysis
                // sample ends inside this one.
                let analysis = (self.sum + x * room) / self.ratio;
                self.sum = x * (1.0 - room);
                self.filled = 1.0 - room;
                self.take(analysis);
            }
        }
    }

    fn take(&mut self, sample: f64) {
        self.frame[self.fill] = sample;
        self.fill += 1;
        if self.fill < FRAME {
            return;
        }
        self.frame_done();
        self.frame.copy_within(HOP.., 0);
        self.fill = FRAME - HOP;
    }

    /// The frame's onset strength; every [`EVERY`] frames, a new estimate.
    fn frame_done(&mut self) {
        let bins = self.stft.frame(&self.frame, 0);
        let mut level = [0.0; FRAME / 2 + 1];
        for (slot, bin) in level.iter_mut().zip(bins) {
            let m = bin.norm() * self.norm;
            *slot = (1.0 + COMPRESSION * (m - 1e-4).max(0.0)).log10();
        }
        let mut flux = 0.0;
        for (k, now) in level.iter().enumerate() {
            let before = self.previous[k.saturating_sub(1)..(k + 2).min(level.len())]
                .iter()
                .copied()
                .fold(0.0, f64::max);
            flux += (now - before).max(0.0);
        }
        let flux = (flux - FLUX_FLOOR).max(0.0);
        self.previous = level;
        self.strength[self.head] = flux;
        self.head = (self.head + 1) % HISTORY;
        self.held = (self.held + 1).min(HISTORY);
        self.frames += 1;
        self.phase = (self.phase + self.bpm / (60.0 * FRAME_RATE)).fract();
        if self.frames.is_multiple_of(EVERY) && self.held >= WARM {
            self.estimate_tempo();
        }
    }

    fn estimate_tempo(&mut self) {
        self.lay_line();
        // The confidence is the strongest peak's height.
        let top = self.score().unwrap_or(0.0);
        self.confidence += CONFIDENCE_GAIN * (top - self.confidence);
        if self.confidence < CONFIDENT {
            return;
        }
        let tempo = self.choose();
        if !self.started {
            self.started = true;
            self.bpm = tempo;
            self.target = tempo;
            self.pending = None;
            self.phase = self.align().unwrap_or(0.0);
            return;
        }
        if (tempo / self.target - 1.0).abs() < NEAR {
            self.pending = None;
            self.target += TEMPO_GAIN * (tempo - self.target);
        } else if self.agreed(tempo) {
            self.target = tempo;
        }
        let most = SLEW * EVERY as f64 / FRAME_RATE;
        self.bpm = self
            .target
            .clamp(self.bpm * (1.0 - most), self.bpm * (1.0 + most));
        if let Some(measured) = self.align() {
            let error = (measured - self.phase + 0.5).rem_euclid(1.0) - 0.5;
            self.phase = (self.phase + PHASE_GAIN * error).rem_euclid(1.0);
        }
    }

    /// Copies the strength out in time order, less its mean.
    fn lay_line(&mut self) {
        // `head` is the oldest value once the history is full.
        let (newer, older) = self.strength.split_at(self.head);
        self.line.clear();
        self.line
            .extend(older.iter().chain(newer).skip(HISTORY - self.held));
        let mean = self.line.iter().sum::<f64>() / self.line.len() as f64;
        for value in &mut self.line {
            *value -= mean;
        }
    }

    /// Scores every tempo and returns the strongest peak's score.
    fn score(&mut self) -> Option<f64> {
        let line = &self.line;
        let n = line.len();
        let recency = &self.recency;
        for (i, slot) in (0..).zip(self.scores.iter_mut()) {
            let bpm = SLOWEST + STEP * f64::from(i);
            let period = 60.0 * FRAME_RATE / bpm;
            let (mut total, mut used) = (0.0, 0);
            for beats in 1..=BEATS_BACK {
                let lag = period * beats as f64;
                if (n as f64) - lag < OVERLAP as f64 {
                    break;
                }
                let first = lag.ceil() as usize;
                // The recency-weighted correlation coefficient of the strength
                // with itself `lag` earlier.
                let (mut w, mut sx, mut sy, mut sxx, mut syy, mut sxy) =
                    (0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
                for t in first..n {
                    let (x, y) = (line[t], between(line, t as f64 - lag));
                    let weight = recency[n - 1 - t];
                    w += weight;
                    sx += weight * x;
                    sy += weight * y;
                    sxx += weight * x * x;
                    syy += weight * y * y;
                    sxy += weight * x * y;
                }
                let (a, b) = (sxx - sx * sx / w, syy - sy * sy / w);
                // A strength that hardly varies where it counts has no rhythm.
                if a > FLAT * w && b > FLAT * w {
                    total += (sxy - sx * sy / w) / (a * b).sqrt();
                }
                used += 1;
            }
            *slot = if used == 0 {
                0.0
            } else {
                (total / f64::from(used)).max(0.0)
            };
        }
        self.peaks().first().map(|peak| peak.1)
    }

    /// The local maxima of the scores as (BPM refined by a parabola, score),
    /// strongest first.
    fn peaks(&self) -> Vec<(f64, f64)> {
        let s = &self.scores;
        let last = s.len() - 1;
        let mut peaks: Vec<(f64, f64)> = (0..=last)
            .filter(|&i| {
                s[i] > 0.0 && (i == 0 || s[i] >= s[i - 1]) && (i == last || s[i] > s[i + 1])
            })
            .map(|i| {
                // A peak at the edge of the range has no curve to refine.
                let (before, after) = (s[i.saturating_sub(1)], s[(i + 1).min(last)]);
                let curve = before - 2.0 * s[i] + after;
                let shift = if curve < 0.0 && i > 0 && i < last {
                    (0.5 * (before - after) / curve).clamp(-0.5, 0.5)
                } else {
                    0.0
                };
                (SLOWEST + STEP * (i as f64 + shift), s[i])
            })
            .collect();
        peaks.sort_by(|a, b| b.1.total_cmp(&a.1));
        peaks
    }

    /// R13: of the peaks with nearly as much evidence as the strongest, the
    /// tempo nearest the one followed (the set's, at the start).
    fn choose(&self) -> f64 {
        let peaks = self.peaks();
        let height = peaks.first().map_or(0.0, |p| p.1);
        let distance = |bpm: f64| (bpm / self.target).log2().abs();
        let strongest = peaks.first().map_or(self.target, |p| p.0);
        peaks
            .iter()
            .filter(|&&(bpm, score)| {
                let ratio = bpm / strongest;
                let octave = (ratio / 2.0 - 1.0).abs() < OCTAVE_SLACK
                    || (ratio * 2.0 - 1.0).abs() < OCTAVE_SLACK;
                score >= if octave { CLOSE_OCTAVE } else { CLOSE } * height
            })
            .map(|p| p.0)
            .min_by(|&a, &b| distance(a).total_cmp(&distance(b)))
            .unwrap_or(self.target)
    }

    /// R13: whether `tempo`, which is not near the tempo followed, has been
    /// the estimate for 4 seconds.
    fn agreed(&mut self, tempo: f64) -> bool {
        let (seen, since) = match self.pending {
            Some((seen, since)) if (seen / tempo - 1.0).abs() < NEAR => (seen, since),
            _ => (tempo, self.frames),
        };
        self.pending = Some((seen, since));
        let agreed = (self.frames - since) as f64 >= AGREE_SECONDS * FRAME_RATE;
        if agreed {
            self.pending = None;
        }
        agreed
    }

    /// How far through its beat the newest frame is, by laying a grid of
    /// beats at the tempo over the last six: the alignment its onset
    /// strength sums highest at, refined by a parabola.
    fn align(&self) -> Option<f64> {
        let n = self.line.len();
        let period = 60.0 * FRAME_RATE / self.bpm;
        let last = (n - 1) as f64;
        let sums: Vec<f64> = (0..PHASES)
            .map(|p| {
                let first = last - period * p as f64 / PHASES as f64;
                (0..GRID_BEATS)
                    .map(|k| first - period * k as f64)
                    .filter(|&at| at >= 0.0 && at < last)
                    .map(|at| between(&self.line, at))
                    .sum()
            })
            .collect();
        let best = (0..PHASES).max_by(|&a, &b| sums[a].total_cmp(&sums[b]))?;
        let (l, c, r) = (
            sums[(best + PHASES - 1) % PHASES],
            sums[best],
            sums[(best + 1) % PHASES],
        );
        let curve = l - 2.0 * c + r;
        let shift = if curve < 0.0 {
            (0.5 * (l - r) / curve).clamp(-0.5, 0.5)
        } else {
            0.0
        };
        Some(((best as f64 + shift) / PHASES as f64).rem_euclid(1.0))
    }
}

/// The strength at `at` frames into `line`, between its values.
fn between(line: &[f64], at: f64) -> f64 {
    let i = at.floor();
    let fraction = at - i;
    let i = i as usize;
    line[i] * (1.0 - fraction) + line.get(i + 1).map_or(0.0, |next| next * fraction)
}

/// Which input channels the tracker listens to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Channel {
    /// One channel, counted from 0.
    Mono(usize),
    /// Two neighbouring channels, summed to mono: the first's number from 0.
    Pair(usize),
}

/// What the audio thread and the tracker thread share.
#[derive(Debug, Default)]
pub struct Shared {
    bpm: AtomicU64,
    phase: AtomicU64,
    confidence: AtomicU64,
    dropped: AtomicU64,
}

impl Shared {
    /// What the tracker last published.
    #[must_use]
    pub fn estimate(&self) -> Estimate {
        let get = |bits: &AtomicU64| f64::from_bits(bits.load(Ordering::Relaxed));
        Estimate {
            bpm: get(&self.bpm),
            phase: get(&self.phase),
            confidence: get(&self.confidence),
        }
    }

    /// Blocks the copy dropped because the ring was full.
    #[must_use]
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    fn publish(&self, estimate: Estimate) {
        let put = |bits: &AtomicU64, value: f64| bits.store(value.to_bits(), Ordering::Relaxed);
        put(&self.bpm, estimate.bpm);
        put(&self.phase, estimate.phase);
        put(&self.confidence, estimate.confidence);
    }
}

/// The audio thread's end of the ring (R11).
pub struct Copier {
    producer: Producer<f32>,
    shared: Arc<Shared>,
    channel: Channel,
}

impl Copier {
    /// Copies the chosen channel of `block` (interleaved in frames of
    /// `channels`), summed to mono for a pair, into the ring. A block that
    /// doesn't fit is dropped whole and counted; a block without the channel
    /// is ignored. Allocates nothing, takes no lock and doesn't block.
    pub fn copy(&mut self, block: &[f32], channels: usize) {
        let (first, second) = match self.channel {
            Channel::Mono(c) => (c, None),
            Channel::Pair(c) => (c, Some(c + 1)),
        };
        let frames = block.len().checked_div(channels).unwrap_or(0);
        if frames == 0 || first >= channels || second.is_some_and(|c| c >= channels) {
            return;
        }
        let Ok(chunk) = self.producer.write_chunk_uninit(frames) else {
            self.shared.dropped.fetch_add(1, Ordering::Relaxed);
            return;
        };
        chunk.fill_from_iter(block.chunks_exact(channels).map(|frame| match second {
            Some(second) => 0.5 * (frame[first] + frame[second]),
            None => frame[first],
        }));
    }
}

/// The tracker thread's end of the ring: reads what the audio thread copied
/// into an [`Analyser`] and publishes its estimate.
pub struct Tracker {
    analyser: Analyser,
    consumer: Consumer<f32>,
    shared: Arc<Shared>,
}

/// A ring from the audio callback to a tracker of audio at `rate` Hz, for a
/// set at `set_bpm`, listening to `channel`, holding a second of audio.
#[must_use]
pub fn ring(rate: u32, set_bpm: f64, channel: Channel) -> (Copier, Tracker) {
    let (producer, consumer) = RingBuffer::new(rate as usize);
    let shared = Arc::new(Shared::default());
    let analyser = Analyser::new(rate, set_bpm);
    shared.publish(analyser.estimate());
    (
        Copier {
            producer,
            shared: Arc::clone(&shared),
            channel,
        },
        Tracker {
            analyser,
            consumer,
            shared,
        },
    )
}

impl Tracker {
    /// What the copy and the tracker publish, for the engine to read.
    #[must_use]
    pub fn shared(&self) -> Arc<Shared> {
        Arc::clone(&self.shared)
    }

    /// Reads everything in the ring, analyses it and publishes the estimate.
    /// Returns how many samples it read.
    pub fn run(&mut self) -> usize {
        let count = self.consumer.slots();
        let Ok(chunk) = self.consumer.read_chunk(count) else {
            return 0;
        };
        let (first, second) = chunk.as_slices();
        self.analyser.push(first);
        self.analyser.push(second);
        chunk.commit_all();
        self.shared.publish(self.analyser.estimate());
        count
    }

    /// Runs the tracker on a thread of its own until the returned handle is
    /// dropped, which reads what is left in the ring first.
    #[must_use]
    pub fn spawn(mut self) -> Running {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let handle = thread::Builder::new()
            .name("tempo-tracker".to_owned())
            .spawn(move || {
                loop {
                    let stopping = flag.load(Ordering::Acquire);
                    if self.run() == 0 {
                        if stopping {
                            break;
                        }
                        thread::sleep(Duration::from_millis(5));
                    }
                }
            })
            .expect("a thread can start");
        Running {
            stop,
            handle: Some(handle),
        }
    }
}

/// A tracker on its thread; dropping it stops the thread.
pub struct Running {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl Drop for Running {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(handle) = self.handle.take() {
            // A panic in the tracker has nowhere to go from a drop.
            let _ = handle.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signals::{DrumLoop, click_track, drum_loop, pink_noise, progression};
    use std::sync::OnceLock;

    const RATE: u32 = 48_000;
    /// The estimates `track` makes are a tenth of a second apart.
    const TICK: f64 = 0.1;

    /// The estimate after each tenth of a second of `samples`.
    fn track(samples: &[f32], set_bpm: f64) -> Vec<Estimate> {
        let mut analyser = Analyser::new(RATE, set_bpm);
        samples
            .chunks(RATE as usize / 10)
            .map(|chunk| {
                analyser.push(chunk);
                analyser.estimate()
            })
            .collect()
    }

    fn after(estimates: &[Estimate], seconds: f64) -> &[Estimate] {
        &estimates[(seconds / TICK) as usize - 1..]
    }

    /// The loops R15 and R17 run on: 80 to 180 BPM, straight, lightly and
    /// fully swung, starting at once or a beat's fraction in.
    fn loops() -> impl Iterator<Item = DrumLoop> {
        (80..=180).step_by(10).flat_map(|bpm| {
            [(0.0, 0.0), (0.5, 0.13), (1.0, 0.31)]
                .into_iter()
                .map(move |(swing, offset)| DrumLoop {
                    swing,
                    offset,
                    seed: bpm as u64,
                    ..DrumLoop::new(f64::from(bpm))
                })
        })
    }

    /// The tempo a set might have when a drummer starts: 15% either way of
    /// the loop's, or its own.
    const SETS: [f64; 3] = [0.85, 1.0, 1.15];

    /// One run of the tracker on a generated loop.
    struct Run {
        spec: DrumLoop,
        set: f64,
        /// The tempo given at 8 seconds.
        at_8s: f64,
        /// How far from the loop's the tempo was at its worst from 8 seconds on.
        worst: f64,
        /// The tempo given at the end.
        last: f64,
    }

    /// Every loop from `loops` with each of `SETS`, run once for the tests of
    /// R15 and R17 together.
    fn runs() -> &'static [Run] {
        static RUNS: OnceLock<Vec<Run>> = OnceLock::new();
        RUNS.get_or_init(|| {
            let mut runs = Vec::new();
            for spec in loops() {
                let (audio, _) = drum_loop(&spec, 24.0, RATE);
                for factor in SETS {
                    let set = spec.bpm * factor;
                    let estimates = track(&audio, set);
                    let settled = after(&estimates, 8.0);
                    runs.push(Run {
                        spec,
                        set,
                        at_8s: settled[0].bpm,
                        worst: settled
                            .iter()
                            .map(|e| (e.bpm - spec.bpm).abs())
                            .fold(0.0, f64::max),
                        last: estimates.last().unwrap().bpm,
                    });
                }
            }
            runs
        })
    }

    // Spec: tempo-follower R15
    #[test]
    fn loops_from_80_to_180_bpm_settle_within_half_a_bpm_in_8_seconds() {
        eprintln!("loop        set    at 8 s   worst from 8 s to 24 s");
        for run in runs() {
            eprintln!(
                "{:3} swing {:.1} {:5.1} {:8.2} {:8.2}",
                run.spec.bpm, run.spec.swing, run.set, run.at_8s, run.worst
            );
            assert!(
                run.worst <= 0.5,
                "{} BPM, swing {}, set {}: off by {}",
                run.spec.bpm,
                run.spec.swing,
                run.set,
                run.worst
            );
        }
    }

    // Spec: tempo-follower R17
    #[test]
    fn fewer_than_5_percent_of_runs_settle_on_half_or_double() {
        let octaves = runs()
            .iter()
            .filter(|run| {
                let ratio = run.last / run.spec.bpm;
                (ratio - 0.5).abs() < 0.05 || (ratio - 2.0).abs() < 0.1
            })
            .count();
        eprintln!(
            "{octaves} of {} runs settled on half or double",
            runs().len()
        );
        assert!(octaves * 20 < runs().len(), "{octaves} of {}", runs().len());
    }

    // Spec: tempo-follower R16
    #[test]
    fn an_8_percent_ramp_over_16_bars_is_followed_within_1_5_bpm() {
        eprintln!("ramp from   worst lag");
        for bpm in [90.0, 120.0, 140.0, 160.0] {
            let spec = DrumLoop {
                ramp: 0.08,
                ..DrumLoop::new(bpm)
            };
            let (audio, _) = drum_loop(&spec, 60.0, RATE);
            let estimates = track(&audio, bpm);
            let lag = estimates
                .iter()
                .enumerate()
                .skip(80)
                .map(|(i, e)| (spec.tempo_at((i + 1) as f64 * TICK) - e.bpm).abs())
                .fold(0.0, f64::max);
            eprintln!("{bpm:9} {lag:10.2}");
            assert!(lag <= 1.5, "{bpm} BPM ramp: lags by {lag}");
        }
    }

    /// Silence, noise and a held chord, `seconds` long.
    fn unrhythmic() -> [(&'static str, Vec<f32>); 3] {
        [
            ("silence", vec![0.0; RATE as usize * 20]),
            ("noise", pink_noise(-20.0, 20.0, RATE, 5)),
            (
                "a held chord",
                progression(&[&[48, 60, 64, 67]], 20.0, RATE),
            ),
        ]
    }

    // Spec: tempo-follower R14
    #[test]
    fn silence_noise_and_a_held_chord_never_become_confident() {
        for (name, audio) in unrhythmic() {
            for e in track(&audio, 123.0) {
                assert!(e.confidence < CONFIDENT, "{name}: {e:?}");
                assert_eq!(e.bpm, 123.0, "{name}");
            }
        }
    }

    // Spec: tempo-follower R14
    #[test]
    fn silence_and_a_held_chord_after_a_loop_keep_its_tempo() {
        let (beat, _) = drum_loop(&DrumLoop::new(100.0), 12.0, RATE);
        for (name, after_it) in unrhythmic() {
            let audio: Vec<f32> = beat.iter().chain(&after_it).copied().collect();
            let estimates = track(&audio, 130.0);
            // The loop's ending thins what the tracker measures, and the
            // tempo it measures moves a little before it holds.
            for e in after(&estimates, 10.0) {
                assert!((e.bpm - 100.0).abs() < 1.0, "{name}: {e:?}");
            }
            // The loop is out of the last 6 seconds by now.
            let last = estimates.last().unwrap();
            assert!(last.confidence < CONFIDENT, "{name}: {last:?}");
        }
    }

    // Spec: tempo-follower R14
    #[test]
    fn the_tempo_moves_by_at_most_2_percent_a_second_once_confident() {
        let (slow, _) = drum_loop(&DrumLoop::new(100.0), 12.0, RATE);
        let (fast, _) = drum_loop(&DrumLoop::new(120.0), 40.0, RATE);
        let audio: Vec<f32> = slow.iter().chain(&fast).copied().collect();
        let estimates = track(&audio, 100.0);
        // Each estimate, a quarter second apart, moves it by at most 0.5%:
        // 2% a second.
        for pair in after(&estimates, 8.0).windows(2) {
            assert!(
                (pair[1].bpm / pair[0].bpm - 1.0).abs() <= 0.0051,
                "{pair:?}"
            );
        }
        assert!((estimates.last().unwrap().bpm - 120.0).abs() < 0.5);
    }

    // Spec: tempo-follower R13
    #[test]
    fn of_close_evidence_the_tempo_nearest_the_one_followed_is_kept() {
        let mut analyser = Analyser::new(RATE, 150.0);
        let index = |bpm: f64| ((bpm - SLOWEST) / STEP) as usize;
        for (bpm, height) in [(80.0, 0.8), (160.0, 0.78), (115.0, 0.5)] {
            analyser.scores[index(bpm)] = height;
        }
        // A tempo in a ratio of 2 with evidence this close is a tie, the
        // set's tempo breaks it, and so does the tempo followed later.
        assert!((analyser.choose() - 160.0).abs() < 0.5);
        analyser.target = 85.0;
        assert!((analyser.choose() - 80.0).abs() < 0.5);
        // With clearly less evidence it isn't.
        analyser.scores[index(80.0)] = 0.5;
        assert!((analyser.choose() - 160.0).abs() < 0.5);
        // Nor is a tempo not in a ratio of 2, with evidence 15% under.
        analyser.scores[index(80.0)] = 0.0;
        analyser.scores[index(115.0)] = 0.67;
        analyser.scores[index(160.0)] = 0.78;
        analyser.target = 115.0;
        assert!((analyser.choose() - 160.0).abs() < 0.5);
    }

    // Spec: tempo-follower R13
    #[test]
    fn a_far_tempo_needs_4_seconds_of_agreement() {
        let mut analyser = Analyser::new(RATE, 120.0);
        let four = (4.0 * FRAME_RATE).ceil() as usize;
        analyser.frames = 1000;
        assert!(!analyser.agreed(60.0));
        analyser.frames += four - 1;
        assert!(!analyser.agreed(60.5));
        // A different tempo starts the 4 seconds again.
        assert!(!analyser.agreed(90.0));
        analyser.frames += four - 1;
        assert!(!analyser.agreed(90.0));
        analyser.frames += 1;
        assert!(analyser.agreed(90.0));
    }

    // Spec: tempo-follower R13
    #[test]
    fn a_click_track_at_half_the_tempo_is_followed_after_4_seconds() {
        let (fast, _) = click_track(120.0, 0.1, 12.0, RATE);
        let (slow, _) = click_track(60.0, 0.1, 80.0, RATE);
        let audio: Vec<f32> = fast.iter().chain(&slow).copied().collect();
        let estimates = track(&audio, 120.0);
        let at = |seconds: f64| estimates[(seconds / TICK) as usize - 1].bpm;
        assert!((at(12.0) - 120.0).abs() < 0.5, "{}", at(12.0));
        // The slow clicks' evidence is clearly the greater from about 15
        // seconds, and the tempo waits 4 seconds more.
        assert!((at(18.0) - 120.0).abs() < 0.5, "{}", at(18.0));
        // Then it moves, at 2% a second.
        assert!(at(30.0) < 118.0 && at(30.0) > 90.0, "{}", at(30.0));
        assert!((at(92.0) - 60.0).abs() < 0.5, "{}", at(92.0));
    }

    // Spec: tempo-follower R18
    #[test]
    fn the_same_audio_gives_the_same_tempos_at_any_block_size() {
        let (audio, _) = drum_loop(&DrumLoop::new(133.0), 15.0, RATE);
        let run = |block: usize| -> Vec<Estimate> {
            let mut analyser = Analyser::new(RATE, 120.0);
            let mut out = Vec::new();
            let mut fed = 0;
            for chunk in audio.chunks(block) {
                analyser.push(chunk);
                fed += chunk.len();
                // The estimate after each whole second, whatever the blocks.
                if fed % RATE as usize == 0 {
                    out.push(analyser.estimate());
                }
            }
            out
        };
        // Each divides a second, so the estimates are of the same moments.
        let first = run(480);
        assert_eq!(first.len(), 15);
        assert_eq!(first, run(480));
        for block in [1_000, RATE as usize / 2, RATE as usize] {
            assert_eq!(first, run(block), "blocks of {block}");
        }
    }

    // Spec: tempo-follower R12
    #[test]
    fn the_phase_follows_the_beat() {
        let spec = DrumLoop {
            offset: 0.13,
            ..DrumLoop::new(120.0)
        };
        let (audio, _) = drum_loop(&spec, 24.0, RATE);
        let estimates = track(&audio, 120.0);
        let mut worst: f64 = 0.0;
        for (i, e) in estimates.iter().enumerate().skip(100) {
            let t = (i + 1) as f64 * TICK;
            let expected = ((t - spec.offset) * spec.bpm / 60.0).rem_euclid(1.0);
            let error = (e.phase - expected + 0.5).rem_euclid(1.0) - 0.5;
            worst = worst.max(error.abs());
        }
        eprintln!("the phase is within {worst:.3} of a beat");
        assert!(worst < 0.1, "{worst}");
    }

    /// A ring for audio at 1 kHz, which holds 1,000 samples.
    fn small(channel: Channel) -> (Copier, Tracker) {
        ring(1_000, 120.0, channel)
    }

    fn ring_holds(tracker: &mut Tracker) -> Vec<f32> {
        let chunk = tracker
            .consumer
            .read_chunk(tracker.consumer.slots())
            .unwrap();
        let held = chunk.as_slices().0.to_vec();
        chunk.commit_all();
        held
    }

    // Spec: tempo-follower R11
    #[test]
    fn the_copy_takes_the_chosen_channel_and_sums_a_pair_to_mono() {
        let block = [1.0, 2.0, 10.0, 3.0, 4.0, 20.0];
        for (channel, copied) in [
            (Channel::Mono(0), vec![1.0, 3.0]),
            (Channel::Mono(2), vec![10.0, 20.0]),
            (Channel::Pair(0), vec![1.5, 3.5]),
            (Channel::Pair(1), vec![6.0, 12.0]),
        ] {
            let (mut copier, mut tracker) = small(channel);
            copier.copy(&block, 3);
            assert_eq!(ring_holds(&mut tracker), copied, "{channel:?}");
        }
        // A channel the block hasn't is ignored, not dropped.
        for channel in [Channel::Mono(3), Channel::Pair(2)] {
            let (mut copier, mut tracker) = small(channel);
            copier.copy(&block, 3);
            assert!(ring_holds(&mut tracker).is_empty(), "{channel:?}");
            assert_eq!(tracker.shared().dropped(), 0);
        }
    }

    // Spec: tempo-follower R11
    #[test]
    fn a_block_that_doesnt_fit_is_dropped_whole_and_counted() {
        let (mut copier, mut tracker) = small(Channel::Mono(0));
        copier.copy(&vec![1.0; 600], 1);
        copier.copy(&vec![2.0; 600], 1);
        copier.copy(&vec![3.0; 400], 1);
        copier.copy(&[4.0], 1);
        let held = ring_holds(&mut tracker);
        assert_eq!(held.len(), 1_000);
        assert!(held[..600].iter().all(|&s| s == 1.0));
        assert!(held[600..].iter().all(|&s| s == 3.0));
        assert_eq!(tracker.shared().dropped(), 2);
    }

    // Spec: tempo-follower R11
    #[test]
    fn the_copy_allocates_nothing_and_takes_no_lock() {
        let (mut copier, mut tracker) = ring(RATE, 120.0, Channel::Pair(0));
        let block = vec![0.25f32; 2 * 512];
        bl_rt::rt_test!({
            // Past the ring's size, so the dropping path runs too.
            for _ in 0..120 {
                copier.copy(&block, 2);
            }
        });
        assert!(tracker.shared().dropped() > 0);
        assert!(tracker.run() > 0);
    }

    // Spec: tempo-follower R11, R12
    #[test]
    fn audio_through_the_ring_gives_the_same_tempo_as_audio_fed_directly() {
        let (mono, _) = drum_loop(&DrumLoop::new(128.0), 12.0, RATE);
        let stereo: Vec<f32> = mono.iter().flat_map(|&s| [s, -s]).collect();
        let noise: Vec<f32> = mono.iter().flat_map(|&s| [s, s]).collect();
        let (mut copier, mut tracker) = ring(RATE, 120.0, Channel::Pair(0));
        let shared = tracker.shared();
        // A pair in opposite phase sums to silence.
        for block in stereo.chunks(2 * 512) {
            copier.copy(block, 2);
            tracker.run();
        }
        assert!(shared.estimate().confidence < CONFIDENT);
        let (mut copier, mut tracker) = ring(RATE, 120.0, Channel::Pair(0));
        let shared = tracker.shared();
        for block in noise.chunks(2 * 512) {
            copier.copy(block, 2);
            tracker.run();
        }
        let direct = {
            let mut analyser = Analyser::new(RATE, 120.0);
            analyser.push(&mono);
            analyser.estimate()
        };
        assert_eq!(shared.estimate(), direct);
        assert!((direct.bpm - 128.0).abs() < 0.5, "{direct:?}");
        assert_eq!(shared.dropped(), 0);
    }

    // Spec: tempo-follower R12
    #[test]
    fn the_thread_reads_what_is_left_in_the_ring_when_it_stops() {
        let (mut copier, tracker) = ring(1_000, 120.0, Channel::Mono(0));
        let shared = tracker.shared();
        let running = tracker.spawn();
        copier.copy(&vec![0.5; 900], 1);
        drop(running);
        // The ring is empty again, so a block that filled it once fits.
        copier.copy(&vec![0.5; 900], 1);
        assert_eq!(shared.dropped(), 0);
    }
}

//! Oversampling (WP-2.5): runs a nonlinear process, such as a saturator, at
//! 2, 4, or 8 times the sample rate, so the harmonics it makes above the
//! original Nyquist frequency are filtered out instead of folding back as
//! aliases.
//!
//! Each doubling is a stage of a linear-phase low-pass FIR filter (65
//! taps, Kaiser window, cutoff at 0.21 of the doubled rate, so the original
//! band is flat to about 0.34 of the original rate, 16 kHz at 48 kHz, and
//! the band that would fold back is about 80 dB down). Going up, a stage
//! puts a zero between samples and filters, which it does as two shorter
//! filters (the even and the odd taps) over the samples themselves, so no
//! multiply is by an inserted zero; coming down, it filters and keeps every
//! other sample. Each stage delays the signal by 32 samples of
//! its own rate each way, so the whole trip's latency is a whole number of
//! frames at the original rate ([`Oversampler::latency`]), which a device
//! reports for delay compensation.
//!
//! Every buffer is made in [`Oversampler::new`], off the audio thread;
//! [`Oversampler::process`] allocates nothing.

/// Taps in each stage's filter. One less than a multiple of 8, so each
/// stage's delay is a whole number of frames at the original rate.
pub const TAPS: usize = 65;

/// Each stage's cutoff, as a fraction of its doubled rate.
const CUTOFF: f64 = 0.21;

/// The Kaiser window's shape: about 80 dB of stopband.
const KAISER_BETA: f64 = 8.0;

/// The most doublings an oversampler takes: 8 times.
pub const MAX_STAGES: u32 = 3;

/// One doubling's filter state for one channel: the recent inputs of its
/// way up and its way down.
#[derive(Clone, Debug)]
struct Stage {
    up: History,
    down: History,
}

/// A filter's recent inputs, stored twice over so the newest `len` are
/// always one unbroken slice.
#[derive(Clone, Debug)]
struct History {
    samples: Vec<f32>,
    at: usize,
    len: usize,
}

impl History {
    fn new(len: usize) -> Self {
        Self {
            samples: vec![0.0; 2 * len],
            at: 0,
            len,
        }
    }

    #[inline]
    fn push(&mut self, sample: f32) {
        self.samples[self.at] = sample;
        self.samples[self.at + self.len] = sample;
        self.at = (self.at + 1) % self.len;
    }

    /// The filter's output for the newest input: the newest `taps.len()`
    /// inputs against `taps`.
    #[inline]
    fn filter(&self, taps: &[f32]) -> f32 {
        // samples[at..at + len] runs oldest to newest.
        let end = self.at + self.len;
        dot(&self.samples[end - taps.len()..end], taps)
    }

    fn clear(&mut self) {
        self.samples.fill(0.0);
        self.at = 0;
    }
}

/// The dot product in eight independent lanes, which the compiler can turn
/// into vector multiply-adds; one running sum is a serial chain it can't.
#[inline]
fn dot(a: &[f32], b: &[f32]) -> f32 {
    let ((a, a_tail), (b, b_tail)) = (a.as_chunks::<8>(), b.as_chunks::<8>());
    let tail: f32 = a_tail.iter().zip(b_tail).map(|(x, y)| x * y).sum();
    let mut lanes = [0.0f32; 8];
    for (x, y) in a.iter().zip(b) {
        for ((lane, x), y) in lanes.iter_mut().zip(x).zip(y) {
            *lane += x * y;
        }
    }
    lanes.iter().sum::<f32>() + tail
}

/// Runs a process at a multiple of the sample rate, channel by channel.
#[derive(Clone, Debug)]
pub struct Oversampler {
    stages: u32,
    channels: usize,
    max_frames: usize,
    taps: Vec<f32>,
    /// The way up's two filters: the even taps and the odd taps, each
    /// doubled to make up for the zeros the polyphase form never inserts.
    even: Vec<f32>,
    odd: Vec<f32>,
    /// Per channel, per stage.
    state: Vec<Vec<Stage>>,
    /// Per stage, the audio at that stage's rate: `max_frames << (s + 1)`
    /// samples.
    work: Vec<Vec<f32>>,
    /// Multiplies the way up has made.
    #[cfg(test)]
    up_macs: u64,
}

impl Oversampler {
    /// An oversampler for `channels` channels that runs at `1 << stages`
    /// times the rate (0 stages runs at the rate itself), taking up to
    /// `max_frames` frames at a time at the original rate. Allocates; make
    /// it off the audio thread.
    ///
    /// # Panics
    ///
    /// If `stages` is more than [`MAX_STAGES`], or `max_frames` is 0.
    #[must_use]
    pub fn new(stages: u32, channels: usize, max_frames: usize) -> Self {
        assert!(stages <= MAX_STAGES, "at most {MAX_STAGES} stages");
        assert!(max_frames > 0, "max_frames must be above 0");
        let taps = design();
        let phase = |first| {
            taps.iter()
                .skip(first)
                .step_by(2)
                .map(|t| 2.0 * t)
                .collect()
        };
        Self {
            stages,
            channels,
            max_frames,
            even: phase(0),
            odd: phase(1),
            taps,
            state: (0..channels)
                .map(|_| {
                    (0..stages)
                        .map(|_| Stage {
                            up: History::new(TAPS.div_ceil(2)),
                            down: History::new(TAPS),
                        })
                        .collect()
                })
                .collect(),
            work: (0..stages)
                .map(|s| vec![0.0; max_frames << (s + 1)])
                .collect(),
            #[cfg(test)]
            up_macs: 0,
        }
    }

    /// How many times the rate the process runs at.
    #[must_use]
    pub fn factor(&self) -> usize {
        1 << self.stages
    }

    /// How many frames, at the original rate, the trip up and down delays
    /// the audio by.
    #[must_use]
    pub fn latency(&self) -> u32 {
        // Each stage delays by (TAPS - 1) / 2 samples of its rate going up
        // and again coming down.
        (1..=self.stages).map(|s| ((TAPS - 1) >> s) as u32).sum()
    }

    /// Clears the filters, as after a jump in the song.
    pub fn reset(&mut self) {
        for stage in self.state.iter_mut().flatten() {
            stage.up.clear();
            stage.down.clear();
        }
    }

    /// Runs `f` over channel `channel`'s `samples` at the higher rate, in
    /// place: up, `f`, and back down. Longer runs than `max_frames` go in
    /// pieces. With no stages, `f` runs on `samples` themselves.
    ///
    /// # Panics
    ///
    /// If `channel` isn't less than the channels it was made for.
    pub fn process(&mut self, channel: usize, samples: &mut [f32], mut f: impl FnMut(&mut [f32])) {
        assert!(channel < self.channels, "channel {channel} out of range");
        if self.stages == 0 {
            f(samples);
            return;
        }
        for piece in samples.chunks_mut(self.max_frames) {
            self.piece(channel, piece, &mut f);
        }
    }

    /// Runs `f` over channel `channel`'s `samples` at the higher rate, going
    /// up only: no way down, for a caller that only reads the high-rate
    /// audio, such as a peak detector. Longer runs than `max_frames` go in
    /// pieces, and `f` runs on each. With no stages, `f` runs on `samples`
    /// themselves.
    ///
    /// # Panics
    ///
    /// If `channel` isn't less than the channels it was made for.
    pub fn upsample(&mut self, channel: usize, samples: &[f32], mut f: impl FnMut(&[f32])) {
        assert!(channel < self.channels, "channel {channel} out of range");
        if self.stages == 0 {
            f(samples);
            return;
        }
        for piece in samples.chunks(self.max_frames) {
            self.up(channel, piece);
            f(&self.work[self.stages as usize - 1][..piece.len() << self.stages]);
        }
    }

    /// Fills the top stage's `work` with `samples` at the higher rate.
    fn up(&mut self, channel: usize, samples: &[f32]) {
        let n = samples.len();
        let state = &mut self.state[channel];
        for s in 0..self.stages as usize {
            let (before, after) = self.work.split_at_mut(s);
            let input: &[f32] = if s == 0 {
                samples
            } else {
                &before[s - 1][..n << s]
            };
            let output = &mut after[0][..n << (s + 1)];
            let history = &mut state[s].up;
            for (i, sample) in input.iter().enumerate() {
                // The taps are symmetric, so the newest 33 inputs against
                // the even taps are the stuffed filter's output for the
                // sample itself, and the newest 32 against the odd taps its
                // output for the zero after it.
                history.push(*sample);
                output[2 * i] = history.filter(&self.even);
                output[2 * i + 1] = history.filter(&self.odd);
            }
            #[cfg(test)]
            {
                self.up_macs += (input.len() * (self.even.len() + self.odd.len())) as u64;
            }
        }
    }

    fn piece(&mut self, channel: usize, samples: &mut [f32], f: &mut impl FnMut(&mut [f32])) {
        let n = samples.len();
        self.up(channel, samples);
        let taps = &self.taps;
        let state = &mut self.state[channel];
        let top = self.stages as usize - 1;
        f(&mut self.work[top][..n << self.stages]);
        // Down, stage by stage.
        for s in (0..self.stages as usize).rev() {
            let (before, after) = self.work.split_at_mut(s);
            let input = &after[0][..n << (s + 1)];
            let output: &mut [f32] = if s == 0 {
                samples
            } else {
                &mut before[s - 1][..n << s]
            };
            let history = &mut state[s].down;
            for (i, out) in output.iter_mut().enumerate() {
                history.push(input[2 * i]);
                *out = history.filter(taps);
                history.push(input[2 * i + 1]);
            }
        }
    }
}

/// The stages' low-pass filter: a windowed sinc, normalised to unity gain
/// at 0 Hz.
fn design() -> Vec<f32> {
    let middle = (TAPS - 1) as f64 / 2.0;
    let taps: Vec<f64> = (0..TAPS)
        .map(|i| {
            let x = i as f64 - middle;
            let sinc = if x == 0.0 {
                2.0 * CUTOFF
            } else {
                (2.0 * std::f64::consts::PI * CUTOFF * x).sin() / (std::f64::consts::PI * x)
            };
            let r = x / middle;
            let window =
                bessel_i0(KAISER_BETA * (1.0 - r * r).max(0.0).sqrt()) / bessel_i0(KAISER_BETA);
            sinc * window
        })
        .collect();
    let sum: f64 = taps.iter().sum();
    taps.iter().map(|t| (t / sum) as f32).collect()
}

/// The modified Bessel function of the first kind, order 0, by its series.
fn bessel_i0(x: f64) -> f64 {
    let mut sum = 1.0;
    let mut term = 1.0;
    let half = x / 2.0;
    for k in 1..50 {
        term *= (half / f64::from(k)) * (half / f64::from(k));
        sum += term;
        if term < sum * 1e-17 {
            break;
        }
    }
    sum
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::TAU;

    const RATE: f64 = 48_000.0;

    fn sine(hz: f64, frames: usize) -> Vec<f32> {
        (0..frames)
            .map(|i| (TAU * hz * i as f64 / RATE).sin() as f32)
            .collect()
    }

    /// The amplitude of `hz` in `samples`.
    fn amplitude(samples: &[f32], hz: f64) -> f64 {
        let (mut re, mut im) = (0.0, 0.0);
        for (i, s) in samples.iter().enumerate() {
            let phase = TAU * hz * i as f64 / RATE;
            re += f64::from(*s) * phase.cos();
            im += f64::from(*s) * phase.sin();
        }
        2.0 * (re * re + im * im).sqrt() / samples.len() as f64
    }

    // Spec: device-framework R8
    #[test]
    fn the_trip_up_and_down_passes_the_band_delayed_by_its_latency() {
        for stages in 0..=MAX_STAGES {
            let mut os = Oversampler::new(stages, 1, 256);
            let latency = os.latency() as usize;
            assert_eq!(latency, [0, 32, 48, 56][stages as usize]);
            let input = sine(1000.0, 4800);
            let mut audio = input.clone();
            // Pieces of uneven sizes, larger than max_frames too.
            let mut at = 0;
            for size in [100usize, 700, 3, 1997, 2000] {
                os.process(0, &mut audio[at..at + size], |high| {
                    assert_eq!(high.len() % (1 << stages), 0);
                    assert!(stages == 0 || high.len() <= 256 << stages);
                });
                at += size;
            }
            for i in 1000..4800 {
                let error = (audio[i] - input[i - latency]).abs();
                assert!(error < 2e-3, "{stages} stages, frame {i}: {error}");
            }
        }
    }

    // Spec: device-framework R8
    #[test]
    fn oversampling_keeps_a_nonlinearitys_harmonics_from_folding_back() {
        // Cubing a 15 kHz sine makes 45 kHz, which at 48 kHz folds back to
        // 3 kHz.
        let input = sine(15_000.0, 9600);
        let cube = |high: &mut [f32]| high.iter_mut().for_each(|s| *s = *s * *s * *s);
        let mut plain = input.clone();
        cube(&mut plain);
        let folded = amplitude(&plain[4800..], 3000.0);
        assert!((folded - 0.25).abs() < 0.01, "{folded}");
        for stages in 1..=MAX_STAGES {
            let mut os = Oversampler::new(stages, 1, 512);
            let mut audio = input.clone();
            os.process(0, &mut audio, cube);
            let alias = amplitude(&audio[4800..], 3000.0);
            assert!(alias < 0.25 / 1000.0, "{stages} stages: {alias}");
            // The 15 kHz itself (3/4 of the cube) comes through.
            let kept = amplitude(&audio[4800..], 15_000.0);
            assert!((kept - 0.75).abs() < 0.05, "{stages} stages: {kept}");
        }
    }

    /// The full-length filter of `x`, for the references below.
    fn convolve(x: &[f32], taps: &[f32]) -> Vec<f32> {
        (0..x.len())
            .map(|n| {
                (0..taps.len().min(n + 1))
                    .map(|k| f64::from(taps[k]) * f64::from(x[n - k]))
                    .sum::<f64>() as f32
            })
            .collect()
    }

    /// The way up with a zero between samples and the full filter, which
    /// the polyphase form replaces.
    fn reference_up(x: &[f32], taps: &[f32]) -> Vec<f32> {
        let stuffed: Vec<f32> = x.iter().flat_map(|s| [2.0 * s, 0.0]).collect();
        convolve(&stuffed, taps)
    }

    /// The whole trip, up, a per-sample `f` and down, over the whole signal
    /// at once: `f` acts on one sample at a time, so the pieces the real
    /// one runs in can't change the result.
    fn reference_process(input: &[f32], stages: u32, f: impl Fn(f32) -> f32) -> Vec<f32> {
        let taps = design();
        let mut audio = input.to_vec();
        for _ in 0..stages {
            audio = reference_up(&audio, &taps);
        }
        audio.iter_mut().for_each(|s| *s = f(*s));
        for _ in 0..stages {
            audio = convolve(&audio, &taps).into_iter().step_by(2).collect();
        }
        audio
    }

    /// Seeded white noise from a linear congruential generator.
    fn noise(frames: usize, seed: u32) -> Vec<f32> {
        let mut state = seed;
        (0..frames)
            .map(|_| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (state >> 8) as f32 / (1 << 23) as f32 - 1.0
            })
            .collect()
    }

    // Spec: device-framework R8
    #[test]
    fn the_polyphase_trip_matches_the_zero_stuffed_filter() {
        for stages in 1..=MAX_STAGES {
            let max_frames = 100;
            let sizes = [1, 7, 64, max_frames + 3];
            let tanh = |s: f32| (3.0 * s).tanh();
            let identity = |s: f32| s;
            let closures: [(&str, &dyn Fn(f32) -> f32); 2] =
                [("identity", &identity), ("tanh", &tanh)];
            for (name, f) in closures {
                let mut os = Oversampler::new(stages, 2, max_frames);
                assert_eq!(os.latency(), [0, 32, 48, 56][stages as usize]);
                for channel in 0..2 {
                    os.reset();
                    let input = noise(4096, 7 + channel as u32);
                    let want = reference_process(&input, stages, f);
                    let mut audio = input.clone();
                    let (mut at, mut i) = (0, 0);
                    while at < audio.len() {
                        let size = sizes[i % sizes.len()].min(audio.len() - at);
                        os.process(channel, &mut audio[at..at + size], |high| {
                            high.iter_mut().for_each(|s| *s = f(*s));
                        });
                        at += size;
                        i += 1;
                    }
                    for (n, (got, want)) in audio.iter().zip(&want).enumerate() {
                        assert!(
                            (got - want).abs() < 1e-6,
                            "{stages} stages, {name}, channel {channel}, frame {n}: {got} against {want}"
                        );
                    }
                }
            }
        }
    }

    // Spec: device-framework R8
    #[test]
    fn upsample_matches_process_closure_input() {
        for stages in 1..=MAX_STAGES {
            let input: Vec<f32> = (0..1000)
                .map(|i| (TAU * (100.0 + 20.0 * i as f64) * i as f64 / RATE / 2.0).sin() as f32)
                .collect();
            let mut with_process = Oversampler::new(stages, 1, 256);
            let mut with_upsample = Oversampler::new(stages, 1, 256);
            let (mut from_process, mut from_upsample) = (Vec::new(), Vec::new());
            let mut at = 0;
            for size in [7, 256, 300, 437] {
                let end = at + size;
                let mut copy = input[at..end].to_vec();
                with_process.process(0, &mut copy, |high| from_process.extend_from_slice(high));
                with_upsample.upsample(0, &input[at..end], |high| {
                    from_upsample.extend_from_slice(high)
                });
                at = end;
            }
            assert_eq!(from_process.len(), 1000 * (1 << stages));
            assert_eq!(from_process.len(), from_upsample.len());
            for (n, (a, b)) in from_process.iter().zip(&from_upsample).enumerate() {
                assert!(
                    (a - b).abs() < 1e-6,
                    "{stages} stages, sample {n}: {a} against {b}"
                );
            }
        }
    }

    // Spec: device-framework R8
    #[test]
    fn the_way_up_makes_at_most_half_the_stuffed_filters_multiplies() {
        let mut os = Oversampler::new(MAX_STAGES, 1, 256);
        os.process(0, &mut [0.0; 100], |_| {});
        // 65 multiplies for each pair of outputs: 1 + 2 + 4 pairs per input.
        assert!(os.up_macs <= 100 * 455, "{}", os.up_macs);
    }
}

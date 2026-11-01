//! Complex Pro's own formant correction (the warp modes spec, R22, R23, R35),
//! which finishes what the stretcher's does: on an octave the library puts the
//! first formant of a vowel a third too high.
//!
//! The correction is a short filter after the stretcher. Every [`HOP`] frames
//! it compares the spectral envelope of the last [`FRAME`] frames the
//! stretcher made with the envelope of the file around the audio they play,
//! both smoothed by cepstral liftering at the resolution Envelope sets, and
//! makes the minimum-phase filter that turns the one into the other (the
//! source's, moved by the formant shift Formants leaves). A minimum-phase
//! filter adds no latency to the clip, so the stretcher's timing (R24) and its
//! output with no pitch change are untouched: the correction runs only while
//! the pitch moves and Formants is above 0 %. The filter's gain is set so the
//! frame's energy is what it was, so the clip's level (R35) stays.
//!
//! The filter runs as a zero-latency partitioned convolution: its first
//! [`PART`] taps directly, the rest in blocks of [`PART`] frames by FFT, so
//! the cost per frame is a small fraction of the filter's length.
//!
//! Everything is made in [`Corrector::new`]; running allocates and locks
//! nothing (R5).

use std::f32::consts::PI;
use std::sync::Arc;

use realfft::num_complex::Complex;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};

/// The frames each envelope is read from.
pub(crate) const FRAME: usize = 2_048;
/// The frames between two filters.
const HOP: usize = 1_024;
const BINS: usize = FRAME / 2 + 1;
/// The filter's length.
const TAPS: usize = 1_024;
/// The frames a new filter takes to fade in over the last.
const FADE: usize = 256;
/// The frames in a block of the convolution, and the taps it runs directly:
/// the filter's later taps are convolved a block at a time, which they can
/// wait for, so the output is not delayed.
const PART: usize = 64;
/// The blocks the filter's later taps make.
const PARTS: usize = TAPS / PART - 1;
/// The bins of a block's spectrum, a transform of `2 * PART` frames.
const PBINS: usize = PART + 1;
/// The quietest a bin's power may be, against the loudest in its frame:
/// without it the valleys between harmonics, which no formant lives in, would
/// weigh on the envelope.
const FLOOR: f32 = 1e-6;
/// The most the filter may change a bin by, in the natural log of amplitude
/// (15 dB).
const LIMIT: f32 = 1.727;

/// What the correction is asked for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Tune {
    /// How far the formants move: the source's envelope is moved up by this
    /// factor (1 keeps the formants where they were).
    pub shift: f64,
    /// The quefrency the envelope is smoothed to, in frames: Envelope's
    /// fundamental's period (R23).
    pub cutoff: usize,
}

/// A channel's history and filters.
struct Channel {
    /// The stretcher's output, newest last, [`FRAME`] frames at least.
    history: Vec<f32>,
    len: usize,
    /// The filter in use and the one before it, last tap first so the sum
    /// runs along the history.
    now: Vec<f32>,
    before: Vec<f32>,
    /// The spectra of the two filters' taps from [`PART`] on, a block of taps
    /// each, scaled for the inverse transform.
    now_spectra: Vec<Complex<f32>>,
    before_spectra: Vec<Complex<f32>>,
    /// The spectra of the last [`PARTS`] blocks of input, a ring that
    /// `newest` marks the start of.
    blocks: Vec<Complex<f32>>,
    newest: usize,
    /// The output block's share from the taps of the filters, which is
    /// computed as the block begins.
    tail: Vec<f32>,
    tail_before: Vec<f32>,
    /// The frames seen since the stage began.
    count: usize,
}

impl Channel {
    fn new() -> Self {
        let mut channel = Self {
            history: vec![0.0; FRAME + HOP],
            len: FRAME,
            now: vec![0.0; TAPS],
            before: vec![0.0; TAPS],
            now_spectra: vec![Complex::new(0.0, 0.0); PARTS * PBINS],
            before_spectra: vec![Complex::new(0.0, 0.0); PARTS * PBINS],
            blocks: vec![Complex::new(0.0, 0.0); PARTS * PBINS],
            newest: 0,
            tail: vec![0.0; PART],
            tail_before: vec![0.0; PART],
            count: 0,
        };
        channel.reset();
        channel
    }

    fn reset(&mut self) {
        self.history.fill(0.0);
        self.len = FRAME;
        for filter in [&mut self.now, &mut self.before] {
            filter.fill(0.0);
            filter[TAPS - 1] = 1.0;
        }
        // The identity filter's one tap is in the direct part.
        for spectra in [
            &mut self.now_spectra,
            &mut self.before_spectra,
            &mut self.blocks,
        ] {
            spectra.fill(Complex::new(0.0, 0.0));
        }
        self.tail.fill(0.0);
        self.tail_before.fill(0.0);
        self.newest = 0;
        self.count = 0;
    }
}

/// The transforms and the room they work in.
struct Work {
    forward: Arc<dyn RealToComplex<f32>>,
    inverse: Arc<dyn ComplexToReal<f32>>,
    block_forward: Arc<dyn RealToComplex<f32>>,
    block_inverse: Arc<dyn ComplexToReal<f32>>,
    block_in: Vec<f32>,
    block_out: Vec<f32>,
    block_sum: Vec<Complex<f32>>,
    scratch_block_forward: Vec<Complex<f32>>,
    scratch_block_inverse: Vec<Complex<f32>>,
    window: Vec<f32>,
    frame: Vec<f32>,
    cepstrum: Vec<f32>,
    spectrum: Vec<Complex<f32>>,
    scratch_forward: Vec<Complex<f32>>,
    scratch_inverse: Vec<Complex<f32>>,
    source: Vec<Complex<f32>>,
    output: Vec<Complex<f32>>,
    power: Vec<f32>,
    log_source: Vec<f32>,
    log_output: Vec<f32>,
}

impl Work {
    fn new() -> Self {
        let mut planner = RealFftPlanner::<f32>::new();
        let forward = planner.plan_fft_forward(FRAME);
        let inverse = planner.plan_fft_inverse(FRAME);
        let block_forward = planner.plan_fft_forward(2 * PART);
        let block_inverse = planner.plan_fft_inverse(2 * PART);
        Self {
            block_in: vec![0.0; 2 * PART],
            block_out: vec![0.0; 2 * PART],
            block_sum: vec![Complex::new(0.0, 0.0); PBINS],
            scratch_block_forward: block_forward.make_scratch_vec(),
            scratch_block_inverse: block_inverse.make_scratch_vec(),
            block_forward,
            block_inverse,
            window: (0..FRAME)
                .map(|n| 0.5 - 0.5 * (2.0 * PI * n as f32 / FRAME as f32).cos())
                .collect(),
            frame: vec![0.0; FRAME],
            cepstrum: vec![0.0; FRAME],
            spectrum: vec![Complex::new(0.0, 0.0); BINS],
            scratch_forward: forward.make_scratch_vec(),
            scratch_inverse: inverse.make_scratch_vec(),
            source: vec![Complex::new(0.0, 0.0); BINS],
            output: vec![Complex::new(0.0, 0.0); BINS],
            power: vec![0.0; BINS],
            log_source: vec![0.0; BINS],
            log_output: vec![0.0; BINS],
            forward,
            inverse,
        }
    }

    /// The spectra of `taps` (last tap first) from tap [`PART`] on, a block
    /// of taps at a time, into `into`.
    fn tap_spectra(&mut self, taps: &[f32], into: &mut [Complex<f32>]) {
        let scale = 1.0 / (2 * PART) as f32;
        for (m, spectrum) in into.chunks_mut(PBINS).enumerate() {
            self.block_in.fill(0.0);
            for (i, frame) in self.block_in[..PART].iter_mut().enumerate() {
                *frame = taps[TAPS - 1 - (m + 1) * PART - i] * scale;
            }
            let _ = self.block_forward.process_with_scratch(
                &mut self.block_in,
                spectrum,
                &mut self.scratch_block_forward,
            );
            count(2 * PART * PART_LOG);
        }
    }

    /// The next [`PART`] frames of the output that the taps from [`PART`] on
    /// make, from the spectra of the last [`PARTS`] blocks of input in
    /// `blocks` (the newest at `newest`), into `tail`.
    fn multiply(
        &mut self,
        blocks: &[Complex<f32>],
        newest: usize,
        spectra: &[Complex<f32>],
        tail: &mut [f32],
    ) {
        self.block_sum.fill(Complex::new(0.0, 0.0));
        for (m, spectrum) in spectra.chunks(PBINS).enumerate() {
            let block = &blocks[(newest + m) % PARTS * PBINS..][..PBINS];
            for ((sum, &input), &tap) in self.block_sum.iter_mut().zip(block).zip(spectrum) {
                *sum += input * tap;
            }
        }
        count(4 * PARTS * PBINS + 2 * PART * PART_LOG);
        self.block_sum[0].im = 0.0;
        self.block_sum[PBINS - 1].im = 0.0;
        let _ = self.block_inverse.process_with_scratch(
            &mut self.block_sum,
            &mut self.block_out,
            &mut self.scratch_block_inverse,
        );
        tail.copy_from_slice(&self.block_out[PART..]);
    }

    /// The spectrum of the windowed `samples`, in `into`.
    fn analyse(&mut self, samples: &[f32], into: Target) {
        for ((frame, &sample), &window) in self.frame.iter_mut().zip(samples).zip(&self.window) {
            *frame = sample * window;
        }
        let spectrum = match into {
            Target::Source => &mut self.source,
            Target::Output => &mut self.output,
        };
        let _ =
            self.forward
                .process_with_scratch(&mut self.frame, spectrum, &mut self.scratch_forward);
    }

    /// The real cepstrum of the even real spectrum in `self.spectrum`, scaled
    /// and liftered to `cutoff`, in `self.cepstrum`.
    fn cepstrum_of_spectrum(&mut self, cutoff: usize) {
        self.spectrum[0].im = 0.0;
        self.spectrum[BINS - 1].im = 0.0;
        let _ = self.inverse.process_with_scratch(
            &mut self.spectrum,
            &mut self.cepstrum,
            &mut self.scratch_inverse,
        );
        bl_dsp::stft::lifter(&mut self.cepstrum, cutoff);
    }

    /// Smooths the log amplitude of the spectrum in `self.source` or
    /// `self.output` to `cutoff`, into the matching log.
    fn smooth(&mut self, from: Target, cutoff: usize) {
        let spectrum = match from {
            Target::Source => &self.source,
            Target::Output => &self.output,
        };
        let mut loudest = 0.0f32;
        for (power, bin) in self.power.iter_mut().zip(spectrum) {
            *power = bin.norm_sqr();
            loudest = loudest.max(*power);
        }
        let floor = loudest * FLOOR + 1e-30;
        for (bin, &power) in self.spectrum.iter_mut().zip(&self.power) {
            *bin = Complex::new(0.5 * (power + floor).ln(), 0.0);
        }
        self.cepstrum_of_spectrum(cutoff);
        let log = match from {
            Target::Source => &mut self.log_source,
            Target::Output => &mut self.log_output,
        };
        let _ = self.forward.process_with_scratch(
            &mut self.cepstrum,
            &mut self.spectrum,
            &mut self.scratch_forward,
        );
        for (log, bin) in log.iter_mut().zip(&self.spectrum) {
            *log = bin.re;
        }
    }

    /// The minimum-phase filter, last tap first, that gives the output's
    /// envelope the source's moved up by `tune.shift`, at the frame's energy.
    fn filter(&mut self, tune: &Tune, into: &mut [f32]) {
        let cutoff = tune.cutoff;
        // Last, so `self.power` is the output's, which weighs the energy.
        self.smooth(Target::Output, cutoff);
        let top = (BINS - 1) as f32;
        let mut weight = 0.0f32;
        let mut energy = 0.0f32;
        for k in 0..BINS {
            let at = (k as f32 / tune.shift as f32).min(top);
            let low = at.floor() as usize;
            let high = (low + 1).min(BINS - 1);
            let part = at - low as f32;
            let source = self.log_source[low] * (1.0 - part) + self.log_source[high] * part;
            let change = (source - self.log_output[k]).clamp(-LIMIT, LIMIT);
            self.spectrum[k] = Complex::new(change, 0.0);
            energy += self.power[k] * (2.0 * change).exp();
            weight += self.power[k];
        }
        // The frame keeps its energy.
        let level = if energy > 0.0 && weight > 0.0 {
            0.5 * (energy / weight).ln()
        } else {
            0.0
        };
        for bin in &mut self.spectrum {
            bin.re -= level;
        }
        self.cepstrum_of_spectrum(cutoff);
        // The minimum phase: the cepstrum folded onto the positive quefrencies.
        for (n, value) in self.cepstrum.iter_mut().enumerate() {
            if n > 0 && n < FRAME / 2 {
                *value *= 2.0;
            } else if n > FRAME / 2 {
                *value = 0.0;
            }
        }
        let _ = self.forward.process_with_scratch(
            &mut self.cepstrum,
            &mut self.spectrum,
            &mut self.scratch_forward,
        );
        for bin in &mut self.spectrum {
            *bin = Complex::from_polar(bin.re.exp(), bin.im);
        }
        self.spectrum[0].im = 0.0;
        self.spectrum[BINS - 1].im = 0.0;
        let _ = self.inverse.process_with_scratch(
            &mut self.spectrum,
            &mut self.cepstrum,
            &mut self.scratch_inverse,
        );
        let scale = 1.0 / FRAME as f32;
        let taper = TAPS * 3 / 4;
        for (n, tap) in into.iter_mut().rev().enumerate() {
            let fade = if n < taper {
                1.0
            } else {
                0.5 + 0.5 * (PI * (n - taper) as f32 / (TAPS - taper) as f32).cos()
            };
            *tap = self.cepstrum[n] * scale * fade;
        }
    }
}

#[derive(Clone, Copy)]
enum Target {
    Source,
    Output,
}

/// The log2 of the convolution's transform size.
const PART_LOG: usize = 7;

#[cfg(test)]
thread_local! {
    /// The multiply-adds the convolution has done on this thread.
    static MULTIPLY_ADDS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Counts `n` multiply-adds of the convolution, for the test that bounds its
/// cost: a complex one is 4, a real transform of `n` frames about `n log2 n`.
#[cfg(test)]
fn count(n: usize) {
    MULTIPLY_ADDS.with(|total| total.set(total.get() + n as u64));
}

#[cfg(not(test))]
fn count(_: usize) {}

/// The sum of the products of `a` and `b`, in eight lanes so it vectorises.
fn dot(a: &[f32], b: &[f32]) -> f32 {
    count(a.len());
    let mut lanes = [0.0f32; 8];
    let (a, b) = (a.as_chunks::<8>().0, b.as_chunks::<8>().0);
    for (a, b) in a.iter().zip(b) {
        for ((lane, a), b) in lanes.iter_mut().zip(a).zip(b) {
            *lane += a * b;
        }
    }
    lanes.iter().sum()
}

/// Complex Pro's formant correction, for every channel of one stretcher.
pub(crate) struct Corrector {
    work: Work,
    channels: Vec<Channel>,
    /// Whether the correction has run since the last reset.
    dirty: bool,
    /// Whether to convolve the filter's every tap in time, as a test's
    /// reference.
    #[cfg(test)]
    reference: bool,
}

impl Corrector {
    /// A corrector for `channels` channels. Called with a clip list, off the
    /// audio thread (R5).
    pub(crate) fn new(channels: usize) -> Self {
        Self {
            work: Work::new(),
            channels: (0..channels).map(|_| Channel::new()).collect(),
            dirty: false,
            #[cfg(test)]
            reference: false,
        }
    }

    /// Starts afresh, as at a locate or a launch.
    pub(crate) fn reset(&mut self) {
        if self.dirty {
            for channel in &mut self.channels {
                channel.reset();
            }
            self.dirty = false;
        }
    }

    /// Corrects `samples`, the next frames channel `channel` of the stretcher
    /// made. `source(i)` is the file frames to compare with the envelope of the
    /// frames up to `samples[i]`, centred where those frames play, if the
    /// window holds them.
    pub(crate) fn run<'a>(
        &mut self,
        channel: usize,
        samples: &mut [f32],
        tune: &Tune,
        source: impl Fn(usize) -> Option<&'a [f32]>,
    ) {
        self.dirty = true;
        let mut done = 0;
        while done < samples.len() {
            let state = &mut self.channels[channel];
            let at = state.count % HOP;
            let frames = (HOP - at).min(samples.len() - done);
            let part = &mut samples[done..done + frames];
            if at == 0 {
                // A hop begins: the filter made in it fades in over this one.
                state.before.copy_from_slice(&state.now);
                state.before_spectra.copy_from_slice(&state.now_spectra);
            }
            #[cfg(test)]
            if self.reference {
                convolve_reference(state, part, at);
            } else {
                convolve(&mut self.work, state, part, at);
            }
            #[cfg(not(test))]
            convolve(&mut self.work, state, part, at);
            done += frames;
            if state.count.is_multiple_of(HOP)
                && state.count >= FRAME
                && let Some(frame) = source(done - 1)
            {
                let heard = &state.history[state.len - FRAME..state.len];
                self.work.analyse(frame, Target::Source);
                self.work.analyse(heard, Target::Output);
                self.work.smooth(Target::Source, tune.cutoff);
                self.work.filter(tune, &mut state.now);
                self.work.tap_spectra(&state.now, &mut state.now_spectra);
            }
        }
    }
}

/// Filters `part`, the frames from `at` frames into a hop, through the
/// channel's filters, the new one fading in over the last. The filter's first
/// [`PART`] taps run on each frame; the rest run by FFT a block at a time, as
/// the block begins, from the input before it.
fn convolve(work: &mut Work, channel: &mut Channel, part: &mut [f32], at: usize) {
    make_room(channel, part.len());
    let first = channel.len;
    channel.history[first..first + part.len()].copy_from_slice(part);
    for (n, sample) in part.iter_mut().enumerate() {
        let place = at + n;
        let slot = place % PART;
        if slot == 0 {
            // FADE is a whole number of blocks, so a block fades or doesn't.
            begin_block(work, channel, first + n, place < FADE);
        }
        let end = first + n + 1;
        let recent = &channel.history[end - PART..end];
        let now = dot(&channel.now[TAPS - PART..], recent) + channel.tail[slot];
        *sample = if place < FADE {
            let share = (place + 1) as f32 / FADE as f32;
            let before = dot(&channel.before[TAPS - PART..], recent) + channel.tail_before[slot];
            before * (1.0 - share) + now * share
        } else {
            now
        };
    }
    channel.len += part.len();
    channel.count += part.len();
}

/// Makes room in the history for `frames` more, keeping the last [`FRAME`].
fn make_room(channel: &mut Channel, frames: usize) {
    if channel.len + frames > channel.history.len() {
        channel
            .history
            .copy_within(channel.len - FRAME..channel.len, 0);
        channel.len = FRAME;
    }
}

/// Starts a block of output at history frame `end`: takes in the block of
/// input before it, and works out what the filters' later taps make of it,
/// for the old filter too while `fading`.
fn begin_block(work: &mut Work, channel: &mut Channel, end: usize, fading: bool) {
    channel.newest = (channel.newest + PARTS - 1) % PARTS;
    work.block_in
        .copy_from_slice(&channel.history[end - 2 * PART..end]);
    let _ = work.block_forward.process_with_scratch(
        &mut work.block_in,
        &mut channel.blocks[channel.newest * PBINS..][..PBINS],
        &mut work.scratch_block_forward,
    );
    count(2 * PART * PART_LOG);
    let (blocks, newest) = (&channel.blocks, channel.newest);
    work.multiply(blocks, newest, &channel.now_spectra, &mut channel.tail);
    if fading {
        work.multiply(
            blocks,
            newest,
            &channel.before_spectra,
            &mut channel.tail_before,
        );
    }
}

/// [`convolve`] with every tap run in time on every frame: the filter's
/// definition, which the test of the FFT convolution compares with.
#[cfg(test)]
fn convolve_reference(channel: &mut Channel, part: &mut [f32], at: usize) {
    make_room(channel, part.len());
    let first = channel.len;
    channel.history[first..first + part.len()].copy_from_slice(part);
    for (n, sample) in part.iter_mut().enumerate() {
        let end = first + n + 1;
        let recent = &channel.history[end - TAPS..end];
        let now = dot(&channel.now, recent);
        let place = at + n;
        *sample = if place < FADE {
            let share = (place + 1) as f32 / FADE as f32;
            dot(&channel.before, recent) * (1.0 - share) + now * share
        } else {
            now
        };
    }
    channel.len += part.len();
    channel.count += part.len();
}

impl std::fmt::Debug for Corrector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Corrector")
            .field("channels", &self.channels.len())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `frames` of seeded white noise in -1..1.
    fn noise(seed: u64, frames: usize) -> Vec<f32> {
        let mut state = seed;
        (0..frames)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                (state >> 40) as f32 / (1u64 << 23) as f32 - 1.0
            })
            .collect()
    }

    const TUNE: Tune = Tune {
        shift: 1.12,
        cutoff: 64,
    };

    /// Runs `corrector` over `input` (a channel of noise per channel) in
    /// blocks of `blocks` frames in turn, comparing each with `source`.
    fn correct(
        corrector: &mut Corrector,
        input: &[Vec<f32>],
        source: &[Vec<f32>],
        blocks: &[usize],
    ) -> Vec<Vec<f32>> {
        let mut out = input.to_vec();
        let (mut at, mut next) = (0, 0);
        while at < out[0].len() {
            let n = blocks[next % blocks.len()].min(out[0].len() - at);
            next += 1;
            for (channel, samples) in out.iter_mut().enumerate() {
                let source = &source[channel][..FRAME];
                corrector.run(channel, &mut samples[at..at + n], &TUNE, |_| Some(source));
            }
            at += n;
        }
        out
    }

    // Spec: warp-modes R5, R35. The FFT convolution makes the samples the
    // filter's definition does, at any block size.
    #[test]
    fn the_fft_convolution_makes_the_samples_the_definition_does() {
        let input = [noise(1, 48_000), noise(2, 48_000)];
        let source = [noise(3, FRAME), noise(4, FRAME)];
        let blocks = [17, 512, 1_000, 64];
        let mut reference = Corrector::new(2);
        reference.reference = true;
        let expected = correct(&mut reference, &input, &source, &blocks);
        let got = correct(&mut Corrector::new(2), &input, &source, &blocks);
        let worst = expected
            .iter()
            .flatten()
            .zip(got.iter().flatten())
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        assert!(worst < 1e-5, "the largest difference is {worst}");
        assert!(
            got[0].iter().zip(&input[0]).any(|(a, b)| a != b),
            "the filter changed nothing"
        );
    }

    // Spec: warp-modes R5. The convolution's work per frame is a small part of
    // the filter's length: a second of stereo took 123 million multiply-adds
    // when every tap ran on every frame.
    #[test]
    fn a_second_of_stereo_takes_under_twenty_million_multiply_adds() {
        let input = [noise(5, 48_000), noise(6, 48_000)];
        let source = [noise(7, FRAME), noise(8, FRAME)];
        let mut corrector = Corrector::new(2);
        MULTIPLY_ADDS.with(|total| total.set(0));
        correct(&mut corrector, &input, &source, &[480]);
        let total = MULTIPLY_ADDS.with(std::cell::Cell::get);
        assert!(total < 20_000_000, "{total} multiply-adds");
    }

    // Spec: warp-modes R5. The fade from the old filter to the new is the
    // definition's too, with two different filters and a block that starts
    // anywhere in a hop.
    #[test]
    fn the_fft_convolution_fades_between_two_filters_as_the_definition_does() {
        let mut work = Work::new();
        let mut channels = [Channel::new(), Channel::new()];
        for channel in &mut channels {
            for (taps, seed) in [(&mut channel.before, 9), (&mut channel.now, 10)] {
                taps.copy_from_slice(
                    &noise(seed, TAPS)
                        .iter()
                        .map(|t| t * 0.1)
                        .collect::<Vec<_>>(),
                );
            }
            work.tap_spectra(&channel.now, &mut channel.now_spectra);
            work.tap_spectra(&channel.before, &mut channel.before_spectra);
        }
        let input = noise(11, 4 * HOP);
        let (mut a, mut b) = (input.clone(), input);
        let mut at = HOP - 100;
        let mut done = 0;
        for n in [100, 17, 300, 1, 63, 64, 500] {
            convolve(
                &mut work,
                &mut channels[0],
                &mut a[done..done + n],
                at % HOP,
            );
            convolve_reference(&mut channels[1], &mut b[done..done + n], at % HOP);
            (at, done) = (at + n, done + n);
        }
        let worst = a[..done]
            .iter()
            .zip(&b)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        assert!(worst < 1e-5, "the largest difference is {worst}");
    }
}

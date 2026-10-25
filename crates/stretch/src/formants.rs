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
        self.count = 0;
    }
}

/// The transforms and the room they work in.
struct Work {
    forward: Arc<dyn RealToComplex<f32>>,
    inverse: Arc<dyn ComplexToReal<f32>>,
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
        Self {
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
        lifter(&mut self.cepstrum, cutoff);
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

/// Scales `cepstrum` (of a real, even spectrum, so even itself) by `1 / FRAME`
/// and by a raised-cosine lifter that is 1 at quefrency 0 and 0 from `cutoff`.
fn lifter(cepstrum: &mut [f32], cutoff: usize) {
    let scale = 1.0 / FRAME as f32;
    for (n, value) in cepstrum.iter_mut().enumerate() {
        let quefrency = n.min(FRAME - n);
        *value *= if quefrency < cutoff {
            scale * (0.5 + 0.5 * (PI * quefrency as f32 / cutoff as f32).cos())
        } else {
            0.0
        };
    }
}

/// The sum of the products of `a` and `b`, in eight lanes so it vectorises.
fn dot(a: &[f32], b: &[f32]) -> f32 {
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
}

impl Corrector {
    /// A corrector for `channels` channels. Called with a clip list, off the
    /// audio thread (R5).
    pub(crate) fn new(channels: usize) -> Self {
        Self {
            work: Work::new(),
            channels: (0..channels).map(|_| Channel::new()).collect(),
            dirty: false,
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
            }
            convolve(state, part, at);
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
            }
        }
    }
}

/// Filters `part`, the frames from `at` frames into a hop, through the
/// channel's filters, the new one fading in over the last.
fn convolve(channel: &mut Channel, part: &mut [f32], at: usize) {
    if channel.len + part.len() > channel.history.len() {
        channel
            .history
            .copy_within(channel.len - FRAME..channel.len, 0);
        channel.len = FRAME;
    }
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

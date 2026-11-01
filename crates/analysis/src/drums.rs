//! Drum transcription (IMP-06; the audio-to-MIDI spec, R20 to R22): the kick,
//! snare and hi-hat hits of a region of audio, for Convert Drums.
//!
//! [`onsets`] finds onsets with the spectral flux of [`crate::onsets`] at 100
//! frames a second and describes each by [`FEATURES`] numbers. A small
//! network whose weights are `tools/drum-classifier/weights.txt`, written by
//! the script beside it, gives each class its own confidence, so one onset
//! can be several drums at once. [`drums`] keeps a class at
//! [`KEEP`] or more and makes a note of it.
//!
//! An onset is described by what it adds to the sound: the spectrum of the
//! 43 ms after it, less the spectrum of the 43 ms before, so the tail of an
//! earlier hit, a room's wash and steady noise are mostly taken away.
//!
//! Everything here runs on whatever thread calls it, never an audio thread:
//! it allocates freely.

use std::sync::{Arc, OnceLock};

use realfft::{RealFftPlanner, RealToComplex};

use crate::Audio;
use crate::onsets::{detect, onset_function_at};
use crate::stft::size_for;
use crate::transcribe::{Error, Grid, Note};

/// The pad notes of the Drums rack (R9): kick, snare and closed hi-hat.
pub const PADS: [u8; 3] = [36, 38, 42];

/// How many numbers describe an onset: four band energies, the centroid,
/// the flatness and the decay of the high end.
pub const FEATURES: usize = 7;

/// Onset function frames a second (R20).
const FRAMES_PER_SECOND: u32 = 100;

/// A class is kept at this confidence or more (R20).
const KEEP: f64 = 0.5;

/// Band edges in Hz: the four bands' bounds (R20).
const EDGES: [f64; 3] = [150.0, 1_000.0, 5_000.0];

/// The span of spectrum the flatness is taken over, in Hz.
const FLAT_FROM: f64 = 40.0;
const FLAT_TO: f64 = 20_000.0;

/// The 80 ms the decay is read over, in frames of 10 ms: the energy of the
/// signal's second difference, which weighs the highs, in the last four
/// frames against the first four.
const DECAY_FRAMES: usize = 8;

/// A snap to the grid needs this confidence and this closeness (R15).
const SNAP_CONFIDENCE: f64 = 0.8;
const SNAP_WITHIN: f64 = 0.020;

/// An onset: when, how strongly, and what it sounds like.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Onset {
    /// Seconds from the start of the audio.
    pub time: f64,
    /// The onset function's peak around it.
    pub strength: f64,
    /// [`FEATURES`] numbers: the log of each band's share of what the onset
    /// adds, the log of the centroid in kHz, the log flatness and the log of
    /// how much quieter the high end of the last 40 ms of the first 80 is
    /// than that of the first.
    pub features: [f64; FEATURES],
}

/// Describes onsets of audio at one rate.
pub struct Describer {
    rate: u32,
    size: usize,
    fft: Arc<dyn RealToComplex<f64>>,
}

impl Describer {
    /// A describer for audio at `rate`.
    #[must_use]
    pub fn new(rate: u32) -> Self {
        let size = size_for(0.043, rate);
        Self {
            rate,
            size,
            fft: RealFftPlanner::<f64>::new().plan_fft_forward(size),
        }
    }

    /// The power spectrum of the `size` samples of `mono` from `start`,
    /// under a half Hann window that is 1 at the end that touches the onset
    /// (the start if `falling`, else the end) and 0 at the other.
    fn power(&self, mono: &[f64], start: isize, falling: bool) -> Vec<f64> {
        let n = self.size;
        let mut input: Vec<f64> = (0..n)
            .map(|i| {
                let k = if falling { i } else { n - 1 - i };
                let w = (std::f64::consts::FRAC_PI_2 * k as f64 / n as f64).cos();
                let sample = usize::try_from(start + i as isize)
                    .ok()
                    .and_then(|at| mono.get(at))
                    .copied()
                    .unwrap_or(0.0);
                sample * w * w
            })
            .collect();
        let mut output = self.fft.make_output_vec();
        self.fft
            .process(&mut input, &mut output)
            .expect("the buffers are the plan's own");
        output.iter().map(|c| c.norm_sqr()).collect()
    }

    /// The features of the onset at `time` seconds in `mono`.
    #[must_use]
    pub fn describe(&self, mono: &[f64], time: f64) -> [f64; FEATURES] {
        let rate = f64::from(self.rate);
        let at = (time * rate).round() as isize - (0.001 * rate) as isize;
        let after = self.power(mono, at, true);
        let before = self.power(mono, at - self.size as isize, false);
        let added: Vec<f64> = after
            .iter()
            .zip(&before)
            .map(|(a, b)| (a - b).max(0.0))
            .collect();
        let hz = |k: usize| k as f64 * rate / self.size as f64;
        let mut bands = [0.0; 4];
        let (mut total, mut moment) = (0.0, 0.0);
        for (k, &p) in added.iter().enumerate() {
            let band = EDGES.iter().take_while(|&&edge| hz(k) >= edge).count();
            bands[band] += p;
            total += p;
            moment += p * hz(k);
        }
        let total = total.max(1e-12);
        let flat: Vec<f64> = added
            .iter()
            .enumerate()
            .filter(|&(k, _)| (FLAT_FROM..FLAT_TO).contains(&hz(k)))
            .map(|(_, &p)| p + 1e-6 * total / added.len() as f64)
            .collect();
        let mean = flat.iter().sum::<f64>() / flat.len() as f64;
        let log_mean = flat.iter().map(|p| p.ln()).sum::<f64>() / flat.len() as f64;
        let frame = (0.01 * rate) as usize;
        let from = usize::try_from(at + (0.001 * rate) as isize).unwrap_or(0);
        let energy = |j: usize| {
            let part = mono.get(from + j * frame..).unwrap_or(&[]);
            let part = &part[..frame.min(part.len())];
            let high: f64 = part
                .windows(3)
                .map(|w| (w[2] - 2.0 * w[1] + w[0]).powi(2))
                .sum();
            high / frame as f64
        };
        let half = DECAY_FRAMES / 2;
        let first: f64 = (0..half).map(energy).sum();
        let last: f64 = (half..DECAY_FRAMES).map(energy).sum();
        let decay = ((last + 1e-10) / (first + 1e-10)).ln().clamp(-14.0, 3.0);
        let share = |band: f64| (band / total + 1e-4).ln();
        [
            share(bands[0]),
            share(bands[1]),
            share(bands[2]),
            share(bands[3]),
            (moment / total / 1_000.0 + 1e-3).ln(),
            (log_mean - mean.ln()).max(-14.0),
            decay,
        ]
    }
}

/// The onsets of `mono` at `rate` (R20), each described. `progress` hears
/// how far it is, from 0 to 1, and stops it by returning `false`.
///
/// # Errors
///
/// [`Error::Cancelled`] if `progress` stopped it.
pub fn onsets(
    mono: &[f64],
    rate: u32,
    progress: &mut dyn FnMut(f32) -> bool,
) -> Result<Vec<Onset>, Error> {
    let function = onset_function_at(mono, rate, (rate / FRAMES_PER_SECOND) as usize);
    let times = detect(mono, &function);
    let describer = Describer::new(rate);
    let mut found = Vec::with_capacity(times.len());
    for (i, &time) in times.iter().enumerate() {
        let frame =
            ((time / function.hop_seconds()).round() as usize).min(function.values.len() - 1);
        let near =
            &function.values[frame.saturating_sub(2)..(frame + 3).min(function.values.len())];
        found.push(Onset {
            time,
            strength: near.iter().copied().fold(0.0, f64::max),
            features: describer.describe(mono, time),
        });
        if i % 32 == 31 && !progress((i + 1) as f32 / times.len() as f32) {
            return Err(Error::Cancelled);
        }
    }
    Ok(found)
}

/// The classifier's weights: each feature's mean and scale, a layer of tanh
/// units and a logistic output for each class, each a bias then a weight for
/// each input.
struct Model {
    mean: Vec<f64>,
    scale: Vec<f64>,
    hidden: Vec<Vec<f64>>,
    classes: Vec<Vec<f64>>,
}

impl Model {
    fn parse(text: &str) -> Option<Self> {
        let mut model = Self {
            mean: Vec::new(),
            scale: Vec::new(),
            hidden: Vec::new(),
            classes: Vec::new(),
        };
        for line in text.lines().filter(|l| !l.starts_with('#')) {
            let mut words = line.split_whitespace();
            let name = words.next()?;
            let row: Vec<f64> = words.map(str::parse).collect::<Result<_, _>>().ok()?;
            match name {
                "mean" => model.mean = row,
                "scale" => model.scale = row,
                "hidden" => model.hidden.push(row),
                _ => model.classes.push(row),
            }
        }
        let fits = model.mean.len() == FEATURES
            && model.scale.len() == FEATURES
            && model.hidden.iter().all(|r| r.len() == FEATURES + 1)
            && model.classes.len() == PADS.len()
            && model
                .classes
                .iter()
                .all(|r| r.len() == model.hidden.len() + 1);
        fits.then_some(model)
    }

    /// Each class's confidence for `features`.
    fn confidence(&self, features: &[f64; FEATURES]) -> [f64; 3] {
        let dot = |row: &[f64], inputs: &mut dyn Iterator<Item = f64>| {
            row[0] + row[1..].iter().zip(inputs).map(|(w, x)| w * x).sum::<f64>()
        };
        let scaled = |j: usize| (features[j] - self.mean[j]) / self.scale[j];
        let hidden: Vec<f64> = self
            .hidden
            .iter()
            .map(|row| dot(row, &mut (0..FEATURES).map(scaled)).tanh())
            .collect();
        let mut out = [0.0; 3];
        for (slot, row) in out.iter_mut().zip(&self.classes) {
            *slot = 1.0 / (1.0 + (-dot(row, &mut hidden.iter().copied())).exp());
        }
        out
    }
}

/// The weights `tools/drum-classifier/train.py` wrote, built in.
fn model() -> &'static Model {
    static MODEL: OnceLock<Model> = OnceLock::new();
    MODEL.get_or_init(|| {
        Model::parse(include_str!("../../../tools/drum-classifier/weights.txt"))
            .expect("the committed weights are well-formed")
    })
}

/// How confident the classifier is that an onset with `features` is a kick,
/// a snare and a hi-hat (R20).
#[must_use]
pub fn classify(features: &[f64; FEATURES]) -> [f64; 3] {
    model().confidence(features)
}

/// The drum notes in `audio` (R20 to R22): each onset's classes at
/// [`KEEP`] or more, a note of `sixteenth` seconds on the class's pad, its
/// velocity from the onset's strength by the pitch notes' law (R16), onsets
/// within 20 ms of a line of `grid` that are confident moved onto it. The
/// notes are in start order. `progress` is as for [`onsets`].
///
/// # Errors
///
/// [`Error::Cancelled`] if `progress` stopped it.
pub fn drums(
    audio: &Audio<'_>,
    sixteenth: f64,
    grid: Option<Grid>,
    progress: &mut dyn FnMut(f32) -> bool,
) -> Result<Vec<Note>, Error> {
    let found = onsets(&audio.mono(), audio.rate(), progress)?;
    let strongest = found.iter().map(|o| o.strength).fold(0.0, f64::max);
    let mut notes = Vec::new();
    for onset in &found {
        let confidence = classify(&onset.features);
        let ratio = onset.strength / strongest;
        let velocity = (127.0 + 63.0 * ratio.log10()).round().clamp(1.0, 127.0) as u8;
        for (pad, &sure) in PADS.iter().zip(&confidence) {
            if sure < KEEP {
                continue;
            }
            let mut start = onset.time;
            if let Some(line) = grid.map(|g| g.nearest(start))
                && sure >= SNAP_CONFIDENCE
                && (line - start).abs() <= SNAP_WITHIN
            {
                start = line;
            }
            notes.push(Note {
                start,
                end: start + sixteenth,
                pitch: *pad,
                velocity,
                confidence: sure as f32,
            });
        }
    }
    notes.sort_by(|a, b| a.start.total_cmp(&b.start).then(a.pitch.cmp(&b.pitch)));
    // A note ends where the next of its pad starts.
    for i in 0..notes.len() {
        if let Some(next) = notes[i + 1..].iter().find(|n| n.pitch == notes[i].pitch) {
            notes[i].end = notes[i].end.min(next.start);
        }
    }
    notes.retain(|n| n.end > n.start);
    Ok(notes)
}

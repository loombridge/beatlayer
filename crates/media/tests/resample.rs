//! Measures sample-rate conversion quality: aliasing and imaging in the
//! passband, and passband flatness.
//!
//! The passband is 0 Hz to 90% of the lower of the two Nyquist frequencies.
//! Every tone is placed on an exact bin of the output analysis window, so its
//! aliases and images land on exact bins too and a rectangular window shows
//! them with no leakage: any energy in a passband bin other than the tone's
//! own is a conversion error. Levels are relative to a full-scale sine.
//!
//! Run with `--nocapture` to print the measurements.

use bl_media::{SampleData, convert_sample_rate};
use realfft::RealFftPlanner;

/// The analysis window, in output frames.
const WINDOW: usize = 16_384;
/// Output frames skipped before the window, past the converter's start-up.
const SETTLE: usize = 8_192;
/// The acceptance limit for aliasing and imaging in the passband.
const LIMIT_DB: f64 = -100.0;
/// The largest passband deviation allowed for a tone, in dB.
const FLATNESS_DB: f64 = 0.1;

/// The rate pairs measured: the common conversions both ways, large ratios,
/// and pairs whose ratio doesn't reduce (the sinc fallback), both ways.
const PAIRS: &[(u32, u32)] = &[
    (44_100, 48_000),
    (48_000, 44_100),
    (88_200, 44_100),
    (96_000, 44_100),
    (96_000, 48_000),
    (192_000, 44_100),
    (44_100, 96_000),
    (48_000, 96_000),
    (22_050, 48_000),
    (44_100, 22_050),
    (32_000, 44_100),
    (44_100, 44_101),
    (48_000, 44_101),
];

fn passband_edge(from: u32, to: u32) -> f64 {
    0.45 * f64::from(from.min(to))
}

/// Full-scale sines at the given frequencies, summed, long enough for the
/// settle time and the window after conversion.
fn tones(rate: u32, to: u32, freqs: &[f64]) -> SampleData {
    let out_frames = SETTLE + WINDOW + 4_096;
    let frames = (out_frames as f64 * f64::from(rate) / f64::from(to)).ceil() as usize;
    let signal = (0..frames)
        .map(|n| {
            let t = n as f64 / f64::from(rate);
            freqs
                .iter()
                .map(|f| (2.0 * std::f64::consts::PI * f * t).sin())
                .sum::<f64>() as f32
        })
        .collect();
    SampleData::new(rate, vec![signal]).unwrap()
}

/// The level of each bin of the window, in dB relative to a full-scale sine.
fn spectrum_db(samples: &[f32]) -> Vec<f64> {
    let mut planner = RealFftPlanner::<f64>::new();
    let fft = planner.plan_fft_forward(samples.len());
    let mut input: Vec<f64> = samples.iter().map(|&s| f64::from(s)).collect();
    let mut output = fft.make_output_vec();
    fft.process(&mut input, &mut output).unwrap();
    let scale = 2.0 / samples.len() as f64;
    output
        .iter()
        .map(|c| 20.0 * (c.norm() * scale).max(1e-20).log10())
        .collect()
}

/// A frequency on an exact bin of the output window near `target`.
fn on_bin(target: f64, to: u32) -> (f64, usize) {
    let bin = (target * WINDOW as f64 / f64::from(to)).round().max(1.0) as usize;
    (bin as f64 * f64::from(to) / WINDOW as f64, bin)
}

struct Measurement {
    /// The worst error in a passband bin with passband tones in, in dB.
    passband_tones_db: f64,
    /// The worst error in a passband bin with tones above the output's
    /// Nyquist frequency in, in dB; `None` when converting up.
    high_tones_db: Option<f64>,
    /// The largest deviation from 0 dB of a passband tone.
    flatness_db: f64,
}

impl Measurement {
    fn worst_db(&self) -> f64 {
        self.passband_tones_db
            .max(self.high_tones_db.unwrap_or(f64::NEG_INFINITY))
    }
}

/// Measures a conversion function from one rate to another.
fn measure(from: u32, to: u32, convert: impl Fn(&SampleData) -> SampleData) -> Measurement {
    let edge = passband_edge(from, to);
    let edge_bin = (edge * WINDOW as f64 / f64::from(to)).floor() as usize;
    let mut result = Measurement {
        passband_tones_db: f64::NEG_INFINITY,
        high_tones_db: None,
        flatness_db: 0.0,
    };

    // Passband tones, one at a time: everything but the tone's own bin is
    // error, and the tone itself should come through at 0 dB.
    for i in 0..12 {
        let target = 50.0 + (edge - 50.0) * f64::from(i) / 11.0;
        let (freq, bin) = on_bin(target, to);
        if freq > edge {
            continue;
        }
        let out = convert(&tones(from, to, &[freq]));
        let levels = spectrum_db(&out.channel(0).unwrap()[SETTLE..SETTLE + WINDOW]);
        result.flatness_db = result.flatness_db.max(levels[bin].abs());
        for (b, &level) in levels.iter().enumerate().take(edge_bin + 1) {
            if b != bin {
                result.passband_tones_db = result.passband_tones_db.max(level);
            }
        }
    }

    // Tones above the output's Nyquist frequency, when there are any: the
    // converter must remove them, so every passband bin is error. They're
    // measured together, which only makes the test stricter.
    if from > to {
        let low = f64::from(to) / 2.0;
        let high = f64::from(from) / 2.0 * 0.995;
        let freqs: Vec<f64> = (0..16)
            .map(|i| on_bin(low + (high - low) * f64::from(i) / 15.0, to).0)
            .filter(|&f| f > low && f < high)
            .collect();
        let out = convert(&tones(from, to, &freqs));
        let levels = spectrum_db(&out.channel(0).unwrap()[SETTLE..SETTLE + WINDOW]);
        let worst = levels
            .iter()
            .take(edge_bin + 1)
            .copied()
            .fold(f64::NEG_INFINITY, f64::max);
        result.high_tones_db = Some(worst);
    }
    result
}

fn beatlayer(to: u32) -> impl Fn(&SampleData) -> SampleData {
    move |data| convert_sample_rate(data, to).unwrap()
}

#[test]
fn aliasing_and_imaging_stay_below_minus_100_db_in_the_passband() {
    // Spec: audio-import R6
    println!(
        "{:>7} -> {:>7}  {:>9}  {:>15}  {:>15}  {:>9}",
        "from Hz", "to Hz", "passband", "passband tones", "high tones", "flatness"
    );
    let mut failures = Vec::new();
    for &(from, to) in PAIRS {
        let m = measure(from, to, beatlayer(to));
        let high = m
            .high_tones_db
            .map_or_else(|| "-".to_owned(), |db| format!("{db:.1} dB"));
        println!(
            "{from:>7} -> {to:>7}  {:>6.0} Hz  {:>12.1} dB  {high:>15}  {:>6.4} dB",
            passband_edge(from, to),
            m.passband_tones_db,
            m.flatness_db,
        );
        if m.worst_db() > LIMIT_DB {
            failures.push(format!(
                "{from} -> {to}: {:.1} dB of error in the passband",
                m.worst_db()
            ));
        }
        if m.flatness_db > FLATNESS_DB {
            failures.push(format!(
                "{from} -> {to}: a passband tone is {:.3} dB off",
                m.flatness_db
            ));
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

#[test]
fn the_measurement_catches_a_converter_that_aliases() {
    // Spec: audio-import R6
    // A control: halving the rate by dropping every other sample, with no
    // filter, folds every tone above 24 kHz into the passband at full level.
    let decimate = |data: &SampleData| {
        let kept = data
            .channel(0)
            .unwrap()
            .iter()
            .step_by(2)
            .copied()
            .collect();
        SampleData::new(data.sample_rate() / 2, vec![kept]).unwrap()
    };
    let m = measure(96_000, 48_000, decimate);
    let high = m.high_tones_db.unwrap();
    assert!(high > -1.0, "the control measured only {high:.1} dB");
}

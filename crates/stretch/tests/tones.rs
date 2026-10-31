use bl_stretch::beats::Plan;
use bl_stretch::tones::{detect_period, diff_ops, grain_for_period};
use bl_stretch::{Request, Tones, WarpMode, Window};

pub fn sine(freq: f64, rate: f64, frames: usize) -> Vec<f32> {
    (0..frames)
        .map(|i| (std::f64::consts::TAU * freq * i as f64 / rate).sin() as f32)
        .collect()
}

static NO_BEATS: Plan = Plan::EMPTY;

fn request<'a>(positions: &'a [f64], first: i64, speed: f64) -> Request<'a> {
    Request {
        positions,
        first,
        speed,
        neutral: false,
        high_quality: false,
        beats: &NO_BEATS,
        transient_envelope: 100.0,
        pitch: 1.0,
        clip: 1,
        step: 1.0,
        song: f64::NAN,
    }
}

/// Renders 2 s of a 70 Hz sine at speed 0.8 through `mode` and returns how far
/// the output's pitch is from 70 Hz, in cents, and how far its level moves from
/// one stretch of 4 periods to the next, in dB.
pub fn bass_waveform(mode: &mut impl WarpMode, rate: f64) -> (f64, f64) {
    let (freq, speed) = (70.0, 0.8);
    let frames = (2.0 * rate) as usize;
    let source = sine(freq, rate, frames + 8192);
    let window = Window {
        data: &source,
        stride: source.len(),
        channels: 1,
        frames: source.len(),
        first: 0,
    };
    let positions: Vec<f64> = (0..frames).map(|i| i as f64 * speed).collect();
    let mut out = vec![0.0f32; frames];
    for first in (0..frames).step_by(1024) {
        let end = (first + 1024).min(frames);
        let req = request(&positions[first..end], first as i64, speed);
        mode.render(&req, &window, &mut out[first..end], end - first);
    }

    // The period, from the autocorrelation over a long stretch.
    let from = frames / 4;
    let part = &out[from..from + 16384];
    let lags = (rate / 120.0) as usize..(rate / 40.0) as usize;
    let corr: Vec<f64> = lags
        .clone()
        .map(|lag| {
            (0..part.len() - lag)
                .map(|j| f64::from(part[j]) * f64::from(part[j + lag]))
                .sum()
        })
        .collect();
    let best = (1..corr.len() - 1)
        .max_by(|&a, &b| corr[a].total_cmp(&corr[b]))
        .unwrap();
    let (a, b, c) = (corr[best - 1], corr[best], corr[best + 1]);
    let period = (lags.start + best) as f64 + 0.5 * (a - c) / (a - 2.0 * b + c);
    let cents = 1200.0 * (period * freq / rate).log2();

    let chunk = (4.0 * rate / freq).round() as usize;
    let db: Vec<f64> = out[from..frames - from]
        .chunks_exact(chunk)
        .map(|c| {
            10.0 * (c.iter().map(|&s| f64::from(s).powi(2)).sum::<f64>() / chunk as f64).log10()
        })
        .collect();
    let spread =
        db.iter().fold(f64::MIN, |m, &v| m.max(v)) - db.iter().fold(f64::MAX, |m, &v| m.min(v));
    (cents, spread)
}

#[test]
fn tones_grains_span_whole_periods_near_grain_size() {
    let sample_rate: f64 = 48_000.0;
    let freq: f64 = 440.0;
    let expected_period = (sample_rate / freq).round() as usize; // 109 samples

    // 440 Hz sine wave
    let frames = 2048;
    let sine: Vec<f32> = (0..frames)
        .map(|i| (2.0 * std::f32::consts::PI * freq as f32 * i as f32 / sample_rate as f32).sin())
        .collect();

    let detected = detect_period(&sine, sample_rate);
    assert_eq!(detected, Some(expected_period));

    // Nominal grain size 30 ms at 48 kHz = 1440 frames
    let nominal = (0.030 * sample_rate).round() as usize;
    let (len, hop) = grain_for_period(nominal, detected);

    // Grain spans an exact multiple of the detected period
    assert_eq!(len % expected_period, 0);
    assert_eq!(hop % expected_period, 0);

    // Grains span whole periods near grain_size
    let diff = (len as isize - nominal as isize).unsigned_abs();
    assert!(diff <= expected_period);
}

#[test]
fn unpitched_audio_takes_grain_size_grains() {
    let sample_rate = 48_000.0;
    // Flat or empty audio has no pitch
    let flat = vec![0.0f32; 2048];
    assert_eq!(detect_period(&flat, sample_rate), None);

    let nominal = (0.030 * sample_rate).round() as usize;
    let (len, hop) = grain_for_period(nominal, None);

    assert_eq!(len, nominal);
    assert_eq!(hop, nominal / 2);
}

#[test]
fn bass_pitch_is_detected_at_96_khz() {
    // 70 Hz is 1,371.4 frames a period at 96 kHz.
    let signal = sine(70.0, 96_000.0, 4096);
    let period = detect_period(&signal, 96_000.0).expect("a clear 70 Hz pitch");
    assert!(period.abs_diff(1371) <= 2, "period {period}");
}

#[test]
fn tones_keeps_a_bass_tones_waveform_at_96_khz() {
    for rate in [48_000.0, 96_000.0] {
        let (cents, spread) = bass_waveform(&mut Tones::new(1, rate), rate);
        assert!(cents.abs() < 5.0, "{rate} Hz: {cents} cents off");
        assert!(spread < 1.0, "{rate} Hz: level moves {spread} dB");
    }
}

#[test]
fn tones_pitch_search_work_is_bounded_in_every_block() {
    let rate = 48_000.0;
    let source = sine(220.0, rate, 4 * 48_000);
    let window = Window {
        data: &source,
        stride: source.len(),
        channels: 1,
        frames: source.len(),
        first: 0,
    };
    let positions: Vec<f64> = (0..48_000).map(f64::from).collect();
    let mut tones = Tones::new(1, rate);
    let mut out = [0.0f32; 64];
    let (start, mut worst) = (diff_ops(), 0);
    for first in (0..positions.len()).step_by(64) {
        let before = diff_ops();
        tones.render(
            &request(&positions[first..first + 64], first as i64, 1.0),
            &window,
            &mut out,
            64,
        );
        worst = worst.max(diff_ops() - before);
    }
    assert!(worst < 150_000, "one block did {worst} operations");
    assert!(
        diff_ops() - start < 5_000_000,
        "a second did {} operations",
        diff_ops() - start
    );
}

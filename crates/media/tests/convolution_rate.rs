//! A response made at another rate than the engine's is resampled with the
//! media resampler before `bl_dsp::convolution` prepares it (spec:
//! impulse-responses R5), and plays as one made at the engine's rate does.
//!
//! `convert_sample_rate` leaves a whole-buffer conversion from 44.1 kHz half
//! a source frame ahead of its input (its delay is removed in whole output
//! frames; measured with a 997 Hz sine, 0.544 output frames), so the response
//! made at 48 kHz is made that much ahead too.

use std::f64::consts::{PI, TAU};

use bl_dsp::convolution::{Convolver, prepare};
use bl_media::{SampleData, convert_sample_rate};

/// How far ahead of its input the converter leaves 44.1 kHz audio, in seconds.
const LEAD: f64 = 0.5 / 44_100.0;

/// A band-limited, decaying response of 1.5 seconds at `rate`, `ahead`
/// seconds early: damped sines below 9 kHz with a smooth onset, so it holds
/// nothing the 44.1 kHz rate can't carry.
fn response(rate: u32, channel: usize, ahead: f64) -> Vec<f32> {
    let frames = (f64::from(rate) * 1.5).round() as usize;
    (0..frames)
        .map(|n| {
            let t = n as f64 / f64::from(rate) + ahead;
            let onset = if t < 0.01 {
                (PI * t / 0.02).sin().powi(2)
            } else {
                1.0
            };
            let sum: f64 = (0..6)
                .map(|j| {
                    let f = 300.0 + 1450.0 * f64::from(j) + 83.0 * channel as f64;
                    (TAU * f * t + f64::from(j)).sin() * (-t * (2.0 + f64::from(j))).exp()
                })
                .sum();
            (onset * sum / 6.0) as f32
        })
        .collect()
}

/// A band-limited input at 48 kHz: sines below 10 kHz.
fn input(frames: usize) -> Vec<f32> {
    (0..frames)
        .map(|n| {
            let t = n as f64 / 48_000.0;
            [440.0, 1234.5, 3777.0, 8123.0]
                .iter()
                .enumerate()
                .map(|(i, f)| (TAU * f * t + i as f64).sin())
                .sum::<f64>() as f32
                / 4.0
        })
        .collect()
}

fn play(h: &[Vec<f32>], x: &[f32]) -> Vec<Vec<f32>> {
    let slices: Vec<&[f32]> = h.iter().map(Vec::as_slice).collect();
    let mut conv = Convolver::with_response(prepare(&slices, 48_000).unwrap());
    let (mut left, mut right) = (x.to_vec(), x.to_vec());
    conv.process(&mut left, &mut right);
    vec![left, right]
}

// Spec: impulse-responses R5; architecture, `bl_dsp::convolution`
#[test]
fn a_response_resampled_from_44100_plays_as_one_made_at_48000() {
    let x = input(60_000);
    let made: Vec<Vec<f32>> = (0..2).map(|c| response(48_000, c, LEAD)).collect();
    let file = SampleData::new(44_100, (0..2).map(|c| response(44_100, c, 0.0)).collect()).unwrap();
    let resampled = convert_sample_rate(&file, 48_000).unwrap();
    assert_eq!(resampled.frames(), made[0].len());
    let converted: Vec<Vec<f32>> = resampled.channels().map(<[f32]>::to_vec).collect();
    let (want, got) = (play(&made, &x), play(&converted, &x));
    let (mut err, mut level) = (0.0, 0.0);
    for (w, g) in want.iter().zip(&got) {
        // The first frames hold the resampler's own start-up.
        for (&w, &g) in w[2000..].iter().zip(&g[2000..]) {
            err += f64::from(w - g).powi(2);
            level += f64::from(w).powi(2);
        }
    }
    let db = 10.0 * (err / level).log10();
    assert!(db < -90.0, "{db:.1} dB");
}

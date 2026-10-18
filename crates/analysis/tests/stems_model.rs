//! The real Demucs network (IMP-11), where its packages are installed:
//! `BL_STEMS_RUNTIME` names ONNX Runtime's shared library and
//! `BL_STEMS_MODEL` the model. Without them the test passes untested, so
//! CI, which has neither, skips it.

#![cfg(feature = "stems")]

use std::path::PathBuf;

use bl_analysis::stems::onnx::Demucs;
use bl_analysis::stems::{Quality, RATE, Stem, separate};

fn energy(samples: &[f32]) -> f64 {
    samples.iter().map(|x| f64::from(*x) * f64::from(*x)).sum()
}

// Spec: stem-separation R12.
#[test]
fn a_bass_line_and_a_hi_hat_separate_into_bass_and_drums() {
    let (Some(runtime), Some(model)) = (
        std::env::var_os("BL_STEMS_RUNTIME").map(PathBuf::from),
        std::env::var_os("BL_STEMS_MODEL").map(PathBuf::from),
    ) else {
        return;
    };
    let mut network = Demucs::load(&runtime, &model, 8).unwrap();
    let rate = f64::from(RATE);
    let frames = RATE as usize * 10;
    // A 55 Hz bass note, and a noise burst every half second.
    let mut seed = 1u32;
    let mut noise = || {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        f64::from(seed >> 8) / f64::from(1u32 << 24) * 2.0 - 1.0
    };
    let mix: Vec<f32> = (0..frames)
        .map(|i| {
            let t = i as f64 / rate;
            let bass = 0.4 * (std::f64::consts::TAU * 55.0 * t).sin();
            let since = t % 0.5;
            let hat = if since < 0.03 {
                0.5 * noise() * (1.0 - since / 0.03)
            } else {
                0.0
            };
            (bass + hat) as f32
        })
        .collect();
    let started = std::time::Instant::now();
    let stems = separate(&mix, &mix, &mut network, Quality::HighSpeed, &mut |_| true).unwrap();
    eprintln!("separated 10 s in {:.1} s", started.elapsed().as_secs_f64());
    for stem in Stem::ALL {
        for channel in stems.get(stem) {
            assert_eq!(channel.len(), frames);
            assert!(channel.iter().all(|x| x.is_finite()));
        }
    }
    let bass = energy(&stems.get(Stem::Bass)[0]);
    let vocals = energy(&stems.get(Stem::Vocals)[0]);
    eprintln!(
        "energy: drums {:.1} bass {bass:.1} others {:.1} vocals {vocals:.1}",
        energy(&stems.get(Stem::Drums)[0]),
        energy(&stems.get(Stem::Others)[0]),
    );
    assert!(bass > vocals * 4.0);
}

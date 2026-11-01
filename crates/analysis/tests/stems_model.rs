//! The real Demucs network (IMP-11), where its packages are installed:
//! `BL_STEMS_RUNTIME` names ONNX Runtime's shared library and
//! `BL_STEMS_MODEL` the model, the split one for the GPU tests;
//! `BL_STEMS_WHOLE_MODEL` names the whole graph the split model is held to.
//! Without them the tests pass untested, so CI, which has none, skips them.

#![cfg(feature = "stems")]

use std::path::PathBuf;

use bl_analysis::stems::generated::{self, Generated};
#[cfg(target_os = "macos")]
use bl_analysis::stems::onnx::Provider;
use bl_analysis::stems::onnx::{Demucs, Options};
use bl_analysis::stems::{Quality, RATE, Stem, separate};

fn path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name).map(PathBuf::from)
}

/// The runtime and the model, if the packages are named.
fn packages(model: &str) -> Option<(PathBuf, PathBuf)> {
    Some((path("BL_STEMS_RUNTIME")?, path(model)?))
}

fn options(accelerate: bool) -> Options {
    Options {
        threads: 8,
        accelerate,
        cache: Some(std::env::temp_dir().join("beatlayer-stems-test-cache")),
    }
}

fn load(runtime: &std::path::Path, model: &std::path::Path, accelerate: bool) -> Demucs {
    Demucs::load(runtime, model, &options(accelerate), &mut |_| {}).unwrap()
}

fn energy(samples: &[f32]) -> f64 {
    samples.iter().map(|x| f64::from(*x) * f64::from(*x)).sum()
}

// Spec: stem-separation R7, R12, R13 and R33: on each provider the model
// has, a bass line and a hi-hat come apart, in both modes.
#[test]
fn a_bass_line_and_a_hi_hat_separate_into_bass_and_drums() {
    let Some((runtime, model)) = packages("BL_STEMS_MODEL") else {
        return;
    };
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
    for accelerate in [false, true] {
        let mut network = load(&runtime, &model, accelerate);
        eprintln!("provider: {}", network.report().describe());
        for quality in [Quality::HighSpeed, Quality::HighQuality] {
            let started = std::time::Instant::now();
            let stems = separate(&mix, &mix, &mut network, quality, &mut |_| true).unwrap();
            eprintln!(
                "separated 10 s, {quality:?}, in {:.1} s",
                started.elapsed().as_secs_f64()
            );
            for stem in Stem::ALL {
                for channel in stems.get(stem) {
                    assert_eq!(channel.len(), frames);
                    assert!(channel.iter().all(|x| x.is_finite()));
                }
            }
            let bass = energy(&stems.get(Stem::Bass)[0]);
            let vocals = energy(&stems.get(Stem::Vocals)[0]);
            assert!(bass > vocals * 4.0, "{accelerate} {quality:?}");
        }
    }
}

/// Each part's score against its true source, for `network` on `generated`.
fn scores(network: &mut Demucs, generated: &Generated, quality: Quality) -> [f64; 4] {
    let [left, right] = &generated.mix;
    let stems = separate(left, right, network, quality, &mut |_| true).unwrap();
    generated.scores(&stems)
}

// Spec: stem-separation R32. The split model, with Beatlayer's own
// transform, finds the sources of generated mixes no worse than the whole
// graph does, by more than 0.2 dB for any part, in both modes.
#[test]
fn the_split_model_is_within_tolerance_of_the_whole_graph() {
    let (Some((runtime, split)), Some(whole)) =
        (packages("BL_STEMS_MODEL"), path("BL_STEMS_WHOLE_MODEL"))
    else {
        return;
    };
    let mut whole = load(&runtime, &whole, false);
    let mut split = load(&runtime, &split, false);
    for seed in [1, 2] {
        let generated = generated::mix(12.0, seed);
        for quality in [Quality::HighSpeed, Quality::HighQuality] {
            let (want, got) = (
                scores(&mut whole, &generated, quality),
                scores(&mut split, &generated, quality),
            );
            eprintln!("seed {seed} {quality:?}: whole {want:.2?} split {got:.2?}");
            for (stem, (want, got)) in Stem::ALL.iter().zip(want.iter().zip(&got)) {
                assert!(got >= &(want - 0.2), "{stem:?}: split {got} whole {want}");
            }
        }
    }
}

// Spec: stem-separation R33 and R34. Where Core ML loads the model it runs
// it, and its stems are as good as the CPU's within the same tolerance; its
// compiled model is cached, so the second load compiles nothing.
#[test]
#[cfg(target_os = "macos")]
fn core_ml_gives_the_cpus_stems_within_tolerance_and_compiles_once() {
    let Some((runtime, model)) = packages("BL_STEMS_MODEL") else {
        return;
    };
    let generated = generated::mix(12.0, 3);
    let mut cpu = load(&runtime, &model, false);
    assert_eq!(cpu.report().provider, Provider::Cpu);
    assert_eq!(cpu.report().fallback, None);
    let mut gpu = load(&runtime, &model, true);
    let report = gpu.report().clone();
    assert_eq!(report.provider, Provider::CoreMl, "{}", report.describe());
    let again = load(&runtime, &model, true);
    assert_eq!(again.report().compiled, None);
    for quality in [Quality::HighSpeed, Quality::HighQuality] {
        let (want, got) = (
            scores(&mut cpu, &generated, quality),
            scores(&mut gpu, &generated, quality),
        );
        eprintln!("{quality:?}: cpu {want:.2?} core ml {got:.2?}");
        for (want, got) in want.iter().zip(&got) {
            assert!(got >= &(want - 0.2), "core ml {got} cpu {want}");
        }
    }
}

// Spec: stem-separation R33. A provider that refuses the model leaves the
// CPU to run it, and the report says why: here Core ML is refused by a
// cache folder that can't be made.
#[test]
#[cfg(target_os = "macos")]
fn a_refused_provider_falls_back_to_the_cpu_and_says_why() {
    let Some((runtime, model)) = packages("BL_STEMS_MODEL") else {
        return;
    };
    let blocker = std::env::temp_dir().join("beatlayer-stems-test-blocker");
    std::fs::write(&blocker, b"a file where the cache folder would go").unwrap();
    let options = Options {
        threads: 8,
        accelerate: true,
        cache: Some(blocker.join("cache")),
    };
    let mut network = Demucs::load(&runtime, &model, &options, &mut |_| {}).unwrap();
    let report = network.report().clone();
    assert_eq!(report.provider, Provider::Cpu);
    let why = report
        .fallback
        .as_deref()
        .expect("a reason for the fallback");
    assert!(why.contains("Core ML"), "{why}");
    assert!(report.describe().contains(why));
    let generated = generated::mix(8.0, 4);
    let [left, right] = &generated.mix;
    let stems = separate(left, right, &mut network, Quality::HighSpeed, &mut |_| true).unwrap();
    assert!(stems.get(Stem::Bass)[0].iter().all(|x| x.is_finite()));
    let _ = std::fs::remove_file(blocker);
}

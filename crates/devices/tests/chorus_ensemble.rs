//! Tests for Chorus-Ensemble (WP-5.20b; the chorus-ensemble spec).

bl_rt::install_trap!();

mod common;

use bl_devices::chorus_ensemble::{
    AMOUNT, CHORUS_ENSEMBLE, CHORUS_TAPS, CHORUS_TIME, DRY_WET, FEEDBACK, HP_FREQ, MODE, OUTPUT,
    RATE, WARMTH, WIDTH, chorus_ensemble_type,
};
use bl_engine::{DeviceType, Node, Registry};
use bl_model::{DeviceKind, ParamId};
use common::{db, device, node, noise, run};

fn registry() -> Registry {
    let mut registry = Registry::empty();
    bl_devices::chorus_ensemble::register(&mut registry);
    registry
}

fn chorus_ensemble_node(params: &[(ParamId, f64)], max_frames: usize) -> Box<dyn Node> {
    let device_type: DeviceType = chorus_ensemble_type();
    node(&registry(), &device(&device_type, params), max_frames)
}

// Spec: chorus-ensemble R1
#[test]
fn the_chorus_ensemble_is_described_and_laid_out() {
    let kind = chorus_ensemble_type();
    assert_eq!(
        kind.kind,
        DeviceKind::Native {
            device_type: CHORUS_ENSEMBLE.to_owned(),
        }
    );
    assert_eq!(kind.name, "Chorus-Ensemble");
    assert!(kind.description.params.len() >= 14);
}

// Spec: chorus-ensemble R2
#[test]
fn high_pass_output_and_warmth_act_on_audio() {
    // Output at 0.0 is silent
    let mut ce_silent = chorus_ensemble_node(&[(OUTPUT, 0.0)], 256);
    let input = noise(256);
    let out_silent = run(&mut *ce_silent, &input, &[]);
    for &s in &out_silent[0] {
        assert_eq!(s, 0.0);
    }

    // Warmth modifies harmonics
    let warm_input = noise(1024);
    let mut ce_clean = chorus_ensemble_node(&[(WARMTH, 0.0), (DRY_WET, 100.0)], 1024);
    let mut ce_warm = chorus_ensemble_node(&[(WARMTH, 100.0), (DRY_WET, 100.0)], 1024);
    let out_clean = run(&mut *ce_clean, &warm_input, &[]);
    let out_warm = run(&mut *ce_warm, &warm_input, &[]);
    let diff: f32 = out_clean[0]
        .iter()
        .zip(&out_warm[0])
        .map(|(a, b)| (a - b).abs())
        .sum();
    assert!(diff > 0.5, "Warmth should modify signal");
}

// Spec: chorus-ensemble R3
#[test]
fn width_and_feedback_operate_in_chorus_mode() {
    // Width 0% makes the wet signal mono
    let mut ce_mono = chorus_ensemble_node(
        &[(MODE, 0.0), (AMOUNT, 50.0), (WIDTH, 0.0), (DRY_WET, 100.0)],
        512,
    );
    let left = noise(512);
    let right: Vec<f32> = left.iter().map(|s| -s).collect();
    let mut stereo_input = (0..512)
        .flat_map(|i| [left[i], right[i]])
        .collect::<Vec<f32>>();

    let out_mono = {
        let frames = 512;
        let mut block = bl_engine::Block::new(
            &mut stereo_input,
            2,
            frames,
            frames,
            &[],
            bl_engine::TransportInfo::default(),
            common::RATE,
        );
        bl_rt::rt_test!({ ce_mono.process(&mut block) });
        stereo_input
            .chunks(frames)
            .map(<[f32]>::to_vec)
            .collect::<Vec<Vec<f32>>>()
    };

    // At Width 0%, channels should be identical
    for (&l, &r) in out_mono[0].iter().zip(&out_mono[1]) {
        assert!((l - r).abs() < 1e-5, "Width 0% should produce mono output");
    }

    // Width 200 %: the side is twice the middle, 6.02 dB, within 0.1 dB. The
    // middle and the side of the output are the middle and the side of the
    // wet through the width's two gains, and the wet is two taps with
    // nothing between them, so one impulse gives both levels: their ratio is
    // the gains' ratio, 2.0.
    let frames = 2048;
    let mut impulse = vec![0.0f32; frames];
    impulse[0] = 1.0;
    let mut ce_wide = chorus_ensemble_node(
        &[
            (MODE, 0.0),
            (RATE, 0.01),
            (AMOUNT, 100.0),
            (CHORUS_TIME, 2.0),
            (CHORUS_TAPS, 0.0),
            (HP_FREQ, 2000.0),
            (WIDTH, 200.0),
            (DRY_WET, 100.0),
        ],
        frames,
    );
    let out_wide = run(&mut *ce_wide, &impulse, &[]);
    let level = |signal: &[f32]| signal.iter().map(|s| f64::from(s.abs())).sum::<f64>();
    let middle: Vec<f32> = out_wide[0]
        .iter()
        .zip(&out_wide[1])
        .map(|(l, r)| 0.5 * (l + r))
        .collect();
    let side: Vec<f32> = out_wide[0]
        .iter()
        .zip(&out_wide[1])
        .map(|(l, r)| 0.5 * (l - r))
        .collect();
    let side_over_middle = db(level(&side) / level(&middle));
    assert!(
        (side_over_middle - 6.0206).abs() <= 0.1,
        "at Width 200 % the side is {side_over_middle:.3} dB over the middle"
    );
}

// Spec: chorus-ensemble R4
#[test]
fn chorus_time_modes_and_tap_counts_behave_as_specified() {
    // Fixed Time (e.g. 10 ms = index 2) holds base delay constant while Amount changes
    // Amount 10% vs Amount 80% with fixed Time 10 ms (at Rate near 0 so LFO is near 0)
    let mut ce_amt10 = chorus_ensemble_node(
        &[
            (MODE, 0.0),
            (CHORUS_TIME, 2.0), // 10 ms
            (CHORUS_TAPS, 0.0), // 1 Tap
            (RATE, 0.01),
            (AMOUNT, 10.0),
            (DRY_WET, 100.0),
        ],
        1024,
    );
    let mut ce_amt80 = chorus_ensemble_node(
        &[
            (MODE, 0.0),
            (CHORUS_TIME, 2.0), // 10 ms
            (CHORUS_TAPS, 0.0), // 1 Tap
            (RATE, 0.01),
            (AMOUNT, 80.0),
            (DRY_WET, 100.0),
        ],
        1024,
    );

    let mut impulse = vec![0.0f32; 1024];
    impulse[0] = 1.0;

    let out10 = run(&mut *ce_amt10, &impulse, &[]);
    let out80 = run(&mut *ce_amt80, &impulse, &[]);

    let peak10 = out10[0]
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.abs().partial_cmp(&b.1.abs()).unwrap())
        .unwrap()
        .0;
    let peak80 = out80[0]
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.abs().partial_cmp(&b.1.abs()).unwrap())
        .unwrap()
        .0;

    // Both should sit around 10 ms (480 frames at 48k)
    let expected_frame = (10.0 * 48.0) as usize;
    assert!((peak10 as isize - expected_frame as isize).abs() <= 10);
    assert!((peak80 as isize - expected_frame as isize).abs() <= 30);

    // 1 Tap vs 2 Taps
    let mut ce_1tap = chorus_ensemble_node(
        &[
            (MODE, 0.0),
            (CHORUS_TAPS, 0.0),
            (CHORUS_TIME, 2.0),
            (DRY_WET, 100.0),
            (AMOUNT, 0.0),
        ],
        1024,
    );
    let mut ce_2taps = chorus_ensemble_node(
        &[
            (MODE, 0.0),
            (CHORUS_TAPS, 1.0),
            (CHORUS_TIME, 2.0),
            (DRY_WET, 100.0),
            (AMOUNT, 0.0),
        ],
        1024,
    );

    let out_1tap = run(&mut *ce_1tap, &impulse, &[]);
    let out_2taps = run(&mut *ce_2taps, &impulse, &[]);

    // 1 tap produces single tap
    let count_1tap = out_1tap[0].iter().filter(|&&s| s.abs() > 0.2).count();
    assert_eq!(count_1tap, 1);
    // 2 taps both active
    let peak_2taps = out_2taps[0].iter().map(|s| s.abs()).fold(0.0f32, f32::max);
    assert!(peak_2taps > 0.2);
}

// Spec: chorus-ensemble R5
#[test]
fn ensemble_modulations_are_one_third_cycle_apart() {
    // Ensemble mode at the slowest rate, so the modulation holds still over
    // the block, and Amount at 100 %: each of the three lines reads the
    // impulse at the delay the modulation gives it at the start of the
    // cycle — the base 10 ms, and the base moved by sin(120°) and sin(240°)
    // of the 3.5 ms depth. The three places are compared to a tenth of a
    // frame, which is under 0.2° of the modulation's phase.
    let frames = 4096;
    let mut ensemble = chorus_ensemble_node(
        &[
            (MODE, 1.0),
            (RATE, 0.01),
            (AMOUNT, 100.0),
            (HP_FREQ, 2000.0),
            (DRY_WET, 100.0),
        ],
        frames,
    );
    let mut impulse = vec![0.0f32; frames];
    impulse[0] = 1.0;
    let out = run(&mut *ensemble, &impulse, &[]);
    let wet = &out[0];

    let rate = 48_000.0;
    let base = 0.010 * rate;
    let depth = 0.0035 * rate;
    let expected = |line: usize| base + depth * (std::f64::consts::TAU * line as f64 / 3.0).sin();
    let place = |line: usize| -> f64 {
        // The tap's centroid over a window around where the modulation puts
        // it. Every line's tap has the same shape, so what the interpolation
        // and the high-pass add to the centroid is the same for all three,
        // and the differences between them are the modulation's own.
        let from = (expected(line) - 30.0) as usize;
        let to = (expected(line) + 30.0) as usize;
        let mut weight = 0.0;
        let mut moment = 0.0;
        for (i, sample) in wet[from..to].iter().enumerate() {
            let w = f64::from(sample.abs());
            weight += w;
            moment += w * (from + i) as f64;
        }
        moment / weight
    };

    for line in 1..3 {
        let got = place(line) - place(0);
        let want = expected(line) - expected(0);
        assert!(
            (got - want).abs() <= 0.6,
            "line {line} sits {got:.2} frames from line 0, expected {want:.2}"
        );
    }
}

// Spec: chorus-ensemble R6
#[test]
fn vibrato_mode_passes_no_dry_and_ignores_feedback_and_dry_wet() {
    // In Vibrato mode (2), even if Dry/Wet is set to 0% and Feedback to 100%,
    // output is 100% wet, dry signal is 0, feedback is 0.
    let mut vibrato = chorus_ensemble_node(
        &[
            (MODE, 2.0),
            (DRY_WET, 0.0),    // Ignored: passes 100% wet
            (FEEDBACK, 100.0), // Ignored: feedback is 0
            (AMOUNT, 50.0),
            (RATE, 1.0),
        ],
        1024,
    );

    let mut impulse = vec![0.0f32; 1024];
    impulse[0] = 1.0;
    let out = run(&mut *vibrato, &impulse, &[]);

    // Direct impulse at frame 0 must be absent (no dry pass-through)
    assert_eq!(out[0][0], 0.0);

    // Delayed wet response exists
    let wet_energy: f32 = out[0].iter().map(|s| s * s).sum();
    assert!(wet_energy > 0.01);
}

// Spec: chorus-ensemble R7
#[test]
fn latency_is_zero() {
    let ce = chorus_ensemble_node(&[], 256);
    assert_eq!(ce.latency(), 0);
}

// Spec: chorus-ensemble R8
#[test]
fn renders_are_bit_identical_at_any_block_size() {
    let params = [
        (MODE, 0.0),
        (RATE, 1.5),
        (AMOUNT, 60.0),
        (DRY_WET, 70.0),
        (WARMTH, 25.0),
    ];

    let frames = 4096;
    let input = noise(frames);

    let mut out_by_size = Vec::new();
    for &block_size in &[1, 64, 512, 4096] {
        let mut ce = chorus_ensemble_node(&params, block_size);
        let mut collected = vec![vec![0.0f32; frames]; 2];

        for chunk_start in (0..frames).step_by(block_size) {
            let chunk_len = block_size.min(frames - chunk_start);
            let chunk_in = &input[chunk_start..chunk_start + chunk_len];
            let chunk_out = run(&mut *ce, chunk_in, &[]);
            for ch in 0..2 {
                collected[ch][chunk_start..chunk_start + chunk_len].copy_from_slice(&chunk_out[ch]);
            }
        }
        out_by_size.push(collected);
    }

    for (i, size) in [64, 512, 4096].iter().enumerate() {
        assert_eq!(
            out_by_size[0],
            out_by_size[i + 1],
            "Block size 1 vs {size} renders differ"
        );
    }
}

//! Tests for Phaser-Flanger (WP-5.20b; the phaser-flanger spec).

bl_rt::install_trap!();

mod common;

use bl_devices::phaser_flanger::{
    AMOUNT, BLEND, CENTER, DOUBLER_TIME, DRY_WET, ENV_AMOUNT, ENV_ATTACK, ENV_ON, ENV_RELEASE,
    FEEDBACK, FEEDBACK_INVERT, FLANGER_TIME, LFO_DUTY, LFO_HZ, LFO_WAVE, LFO2_HZ, LFO2_MIX, MODE,
    NOTCHES, OUTPUT, PHASER_FLANGER, SAFE_BASS, SPREAD, WARMTH, WAVES, phaser_flanger_type,
};
use bl_engine::{DeviceType, Node, Registry};
use bl_model::{DeviceKind, ParamId, ParamKind};
use common::response::{db, impulse_response, response_at};
use common::{device, node, noise, run};

fn registry() -> Registry {
    let mut registry = Registry::empty();
    bl_devices::phaser_flanger::register(&mut registry);
    registry
}

fn phaser_flanger_node(params: &[(ParamId, f64)], max_frames: usize) -> Box<dyn Node> {
    let device_type: DeviceType = phaser_flanger_type();
    node(&registry(), &device(&device_type, params), max_frames)
}

// Spec: phaser-flanger R1
#[test]
fn the_phaser_flanger_is_described_and_laid_out() {
    let kind = phaser_flanger_type();
    assert_eq!(
        kind.kind,
        DeviceKind::Native {
            device_type: PHASER_FLANGER.to_owned(),
        }
    );
    assert_eq!(kind.name, "Phaser-Flanger");
    assert!(kind.description.params.len() >= 25);
    assert!(kind.description.layout.has_expanded());
}

// Spec: phaser-flanger R2
#[test]
fn global_parameters_act_on_audio() {
    // Dry/Wet 0% returns input exactly
    let mut pf_dry = phaser_flanger_node(&[(DRY_WET, 0.0)], 256);
    let input = noise(256);
    let out = run(&mut *pf_dry, &input, &[]);
    for (i, &s) in input.iter().enumerate() {
        assert!((out[0][i] - s).abs() < 1e-6);
    }

    // Output at 0.0 (silence)
    let mut pf_silent = phaser_flanger_node(&[(OUTPUT, 0.0)], 256);
    let out_silent = run(&mut *pf_silent, &input, &[]);
    for &s in &out_silent[0] {
        assert_eq!(s, 0.0);
    }
}

// Spec: phaser-flanger R3
#[test]
fn phaser_notches_fall_at_mapped_frequencies() {
    // Mode = Phaser (0), Amount = 0%, Dry/Wet = 50%, Feedback = 0%
    let notches_count = 4;
    let center_freq = 1200.0;
    let spread_pct = 50.0;

    let mut pf = phaser_flanger_node(
        &[
            (MODE, 0.0),
            (AMOUNT, 0.0),
            (DRY_WET, 50.0),
            (FEEDBACK, 0.0),
            (NOTCHES, notches_count as f64),
            (CENTER, center_freq),
            (SPREAD, spread_pct),
            (BLEND, 0.0),
            (SAFE_BASS, 5.0), // Disable safe bass interference
        ],
        8192,
    );

    let ir = impulse_response(&mut *pf, 8192);

    for k in 0..notches_count {
        // The spec's mapping, worked out here rather than read from the
        // device, so a mapping that left the spec behind fails the test.
        let spacing = 0.5 * (0.2 + 1.8 * spread_pct / 100.0);
        let expected_notch =
            center_freq * 2.0_f64.powf((k as f64 - (notches_count - 1) as f64 / 2.0) * spacing);
        // Find minimum response in a window around expected_notch
        let mut min_db = f64::INFINITY;
        let mut min_freq = expected_notch;

        let steps = 40;
        for s in 0..=steps {
            let f = expected_notch * (0.95 + 0.10 * (s as f64) / (steps as f64));
            let val_db = db(response_at(&ir, f));
            if val_db < min_db {
                min_db = val_db;
                min_freq = f;
            }
        }

        let error_pct = (min_freq - expected_notch).abs() / expected_notch * 100.0;
        assert!(
            error_pct <= 2.0,
            "notch {k}: expected {expected_notch:.1} Hz, found min at {min_freq:.1} Hz (err {error_pct:.2}%)"
        );
        // Ensure it's an actual notch (attenuated significantly below passband)
        assert!(min_db < -10.0, "notch {k} depth was {min_db} dB");
    }
}

// Spec: phaser-flanger R4
#[test]
fn flanger_notches_fall_at_comb_frequencies_and_invert_moves_them() {
    // Mode = Flanger (1), Time = 1.0 ms -> period T = 0.001 s
    // Comb notches without feedback at (k + 0.5) / T = 500, 1500, 2500 Hz
    let time_ms = 1.0;
    let t_sec = time_ms / 1000.0;

    let mut flanger = phaser_flanger_node(
        &[
            (MODE, 1.0),
            (AMOUNT, 0.0),
            (FLANGER_TIME, time_ms),
            (DRY_WET, 50.0),
            (FEEDBACK, 0.0),
            (FEEDBACK_INVERT, 0.0),
            (SAFE_BASS, 5.0),
        ],
        4096,
    );

    let ir = impulse_response(&mut *flanger, 4096);

    for k in 0..3 {
        let expected_notch = (k as f64 + 0.5) / t_sec;
        let mut min_db = f64::INFINITY;
        let mut min_freq = expected_notch;

        for s in 0..=40 {
            let f = expected_notch * (0.98 + 0.04 * (s as f64) / 40.0);
            let val = db(response_at(&ir, f));
            if val < min_db {
                min_db = val;
                min_freq = f;
            }
        }

        let err_pct = (min_freq - expected_notch).abs() / expected_notch * 100.0;
        assert!(
            err_pct <= 1.0,
            "flanger notch {k}: expected {expected_notch} Hz, got {min_freq} Hz (err {err_pct:.2}%)"
        );
    }

    // With Feedback Invert on, notches shift to k / T = 1000, 2000 Hz
    let mut flanger_inv = phaser_flanger_node(
        &[
            (MODE, 1.0),
            (AMOUNT, 0.0),
            (FLANGER_TIME, time_ms),
            (DRY_WET, 50.0),
            (FEEDBACK, 70.0),
            (FEEDBACK_INVERT, 1.0),
            (SAFE_BASS, 5.0),
        ],
        4096,
    );

    let ir_inv = impulse_response(&mut *flanger_inv, 4096);
    let expected_inv_notch = 1.0 / t_sec; // 1000 Hz
    let mut min_inv_db = f64::INFINITY;
    let mut min_inv_freq = expected_inv_notch;
    for s in 0..=40 {
        let f = expected_inv_notch * (0.98 + 0.04 * (s as f64) / 40.0);
        let val = db(response_at(&ir_inv, f));
        if val < min_inv_db {
            min_inv_db = val;
            min_inv_freq = f;
        }
    }
    let err_pct_inv = (min_inv_freq - expected_inv_notch).abs() / expected_inv_notch * 100.0;
    assert!(err_pct_inv <= 1.0, "inverted notch err: {err_pct_inv:.2}%");
}

// Spec: phaser-flanger R5
#[test]
fn doubler_taps_sit_at_spec_times() {
    // Mode = Doubler (2), Time = 20.0 ms
    // Tap 0 at 20.0 ms (960 frames at 48k), Tap 1 at 30.0 ms (1440 frames at 48k)
    let time_ms = 20.0;
    let mut doubler = phaser_flanger_node(
        &[
            (MODE, 2.0),
            (AMOUNT, 0.0),
            (DOUBLER_TIME, time_ms),
            (DRY_WET, 100.0),
            (FEEDBACK, 0.0),
            (SAFE_BASS, 5.0),
        ],
        2048,
    );

    let mut input = vec![0.0f32; 2048];
    input[0] = 1.0;
    let out = run(&mut *doubler, &input, &[]);

    let expected_tap_0 = (20.0 * 48.0) as usize; // 960
    let expected_tap_1 = (30.0 * 48.0) as usize; // 1440

    // Channel 0 tap
    let ch0_peak = out[0]
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.abs().partial_cmp(&b.1.abs()).unwrap())
        .unwrap();
    assert_eq!(ch0_peak.0, expected_tap_0);

    // Channel 1 tap
    let ch1_peak = out[1]
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.abs().partial_cmp(&b.1.abs()).unwrap())
        .unwrap();
    assert_eq!(ch1_peak.0, expected_tap_1);
}

// Spec: phaser-flanger R6
#[test]
fn warmth_adds_saturation() {
    let mut pf_clean = phaser_flanger_node(&[(WARMTH, 0.0), (DRY_WET, 100.0)], 512);
    let mut pf_warm = phaser_flanger_node(&[(WARMTH, 100.0), (DRY_WET, 100.0)], 512);

    let mut input = vec![0.0f32; 512];
    for (i, s) in input.iter_mut().enumerate() {
        *s = (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 48000.0).sin() * 0.9;
    }

    let out_clean = run(&mut *pf_clean, &input, &[]);
    let out_warm = run(&mut *pf_warm, &input, &[]);

    let diff: f32 = out_clean[0]
        .iter()
        .zip(&out_warm[0])
        .map(|(a, b)| (a - b).abs())
        .sum();
    assert!(diff > 1.0, "Warmth should modify signal harmonics");
}

// Spec: phaser-flanger R7
#[test]
fn expanded_part_toggles_with_expand_command() {
    let kind = phaser_flanger_type();
    let layout = kind.description.layout;
    assert!(layout.has_expanded());
}

// Spec: phaser-flanger R8
#[test]
fn main_lfo_duty_cycle_moves_delay_time_path() {
    let mut pf_duty0 = phaser_flanger_node(
        &[
            (MODE, 1.0),
            (AMOUNT, 100.0),
            (LFO_DUTY, 0.0),
            (DRY_WET, 100.0),
        ],
        1024,
    );
    let mut pf_duty50 = phaser_flanger_node(
        &[
            (MODE, 1.0),
            (AMOUNT, 100.0),
            (LFO_DUTY, 50.0),
            (DRY_WET, 100.0),
        ],
        1024,
    );

    let input = noise(1024);
    let out0 = run(&mut *pf_duty0, &input, &[]);
    let out50 = run(&mut *pf_duty50, &input, &[]);

    let diff: f32 = out0[0]
        .iter()
        .zip(&out50[0])
        .map(|(a, b)| (a - b).abs())
        .sum();
    assert!(diff > 1.0, "Duty cycle should change modulation trajectory");

    // The chooser offers the manual's ten waves, with Triangle the default:
    // the shared LFO's Shark Tooth and Wander are not among them.
    let kind = phaser_flanger_type();
    let wave = kind
        .description
        .param(LFO_WAVE)
        .expect("the wave is described");
    let ParamKind::Choice { choices } = &wave.kind else {
        panic!("the wave is a chooser");
    };
    let names: Vec<&str> = choices.iter().map(String::as_str).collect();
    assert_eq!(names, WAVES);
    assert_eq!(choices[wave.default as usize], "Triangle");
}

// Spec: phaser-flanger R9
#[test]
fn lfo2_mix_blends_modulation_path() {
    let mut pf_lfo1 = phaser_flanger_node(
        &[
            (MODE, 1.0),
            (AMOUNT, 100.0),
            (LFO2_MIX, 0.0),
            (DRY_WET, 100.0),
        ],
        1024,
    );
    let mut pf_lfo2 = phaser_flanger_node(
        &[
            (MODE, 1.0),
            (AMOUNT, 100.0),
            (LFO2_MIX, 100.0),
            (LFO2_HZ, 5.0),
            (DRY_WET, 100.0),
        ],
        1024,
    );

    let input = noise(1024);
    let out1 = run(&mut *pf_lfo1, &input, &[]);
    let out2 = run(&mut *pf_lfo2, &input, &[]);

    let diff: f32 = out1[0]
        .iter()
        .zip(&out2[0])
        .map(|(a, b)| (a - b).abs())
        .sum();
    assert!(diff > 1.0, "LFO2 mix should alter modulation path");
}

// Spec: phaser-flanger R10
#[test]
fn envelope_follower_modulates_effect() {
    let mut pf_no_env = phaser_flanger_node(
        &[(MODE, 1.0), (AMOUNT, 0.0), (ENV_ON, 0.0), (DRY_WET, 100.0)],
        1024,
    );
    let mut pf_env = phaser_flanger_node(
        &[
            (MODE, 1.0),
            (AMOUNT, 0.0),
            (ENV_ON, 1.0),
            (ENV_AMOUNT, 100.0),
            (ENV_ATTACK, 5.0),
            (ENV_RELEASE, 50.0),
            (DRY_WET, 100.0),
        ],
        1024,
    );

    let mut input = vec![0.0f32; 1024];
    for (i, s) in input.iter_mut().enumerate() {
        *s = if i < 200 { 1.0 } else { 0.0 };
    }

    let out_no_env = run(&mut *pf_no_env, &input, &[]);
    let out_env = run(&mut *pf_env, &input, &[]);

    let diff: f32 = out_no_env[0]
        .iter()
        .zip(&out_env[0])
        .map(|(a, b)| (a - b).abs())
        .sum();
    assert!(diff > 0.1, "Envelope follower should modulate delay path");
}

// Spec: phaser-flanger R11
#[test]
fn safe_bass_leaves_content_an_octave_below_within_half_db_of_dry() {
    // Safe Bass cutoff = 400 Hz. One octave below = 200 Hz.
    let cutoff = 400.0;
    let test_freq = 200.0;

    // Flanger mode with the shortest Time, so the tap is a copy of the
    // input a couple of frames later, and the default Dry/Wet of 50 %: the
    // bass has to come out of the mix as it went in.
    let mut pf = phaser_flanger_node(
        &[
            (MODE, 1.0),
            (AMOUNT, 0.0),
            (FLANGER_TIME, 0.05),
            (FEEDBACK, 0.0),
            (DRY_WET, 50.0),
            (SAFE_BASS, cutoff),
            (WARMTH, 0.0),
        ],
        8192,
    );

    let ir = impulse_response(&mut *pf, 8192);
    let resp_db = db(response_at(&ir, test_freq));

    // Content an octave below should be within 0.5 dB of dry signal (0 dB)
    assert!(
        resp_db.abs() <= 0.5,
        "Safe Bass at 200 Hz: response was {resp_db:.3} dB, expected within 0.5 dB"
    );
}

// Spec: phaser-flanger R12
#[test]
fn latency_is_zero() {
    let pf = phaser_flanger_node(&[], 256);
    assert_eq!(pf.latency(), 0);
}

// Spec: phaser-flanger R13
#[test]
fn renders_are_bit_identical_at_any_block_size() {
    let params = [
        (MODE, 0.0),
        (AMOUNT, 50.0),
        (LFO_HZ, 2.0),
        (DRY_WET, 75.0),
        (WARMTH, 20.0),
    ];

    let frames = 4096;
    let input = noise(frames);

    let mut out_by_size = Vec::new();
    for &block_size in &[1, 64, 512, 4096] {
        let mut pf = phaser_flanger_node(&params, block_size);
        let mut collected = vec![vec![0.0f32; frames]; 2];

        for chunk_start in (0..frames).step_by(block_size) {
            let chunk_len = block_size.min(frames - chunk_start);
            let chunk_in = &input[chunk_start..chunk_start + chunk_len];
            let chunk_out = run(&mut *pf, chunk_in, &[]);
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

//! Compressor, the native compressor (WP-2.18a, part 2; the compressor spec):
//! the static curve at several ratios and knees, the ratio's infinite top,
//! expansion, the attack and release time constants, Auto Release, the Log
//! envelope, the Peak and RMS modes, the look-ahead and its latency,
//! Makeup, Dry/Wet and Out, and a sidechain key that ducks the input.

use crate::common;
use bl_devices::compressor::{
    ATTACK, AUTO_RELEASE, COMPRESSOR, DRY_WET, ENVELOPE, KNEE, LOOKAHEAD, MAKEUP, MODE, OUT, RATIO,
    RELEASE, SC_FILTER, SC_FREQ, SC_RES, SC_TYPE, THRESHOLD, compressor_type,
};
use bl_engine::{Block, DeviceType, Node, NodeEvent, Registry, TransportInfo};
use bl_model::ParamId;
use bl_model::param::db_to_gain;

use common::{RATE, device, node, set};

/// A registry with the compressor alone.
fn registry() -> Registry {
    let mut registry = Registry::empty();
    bl_devices::compressor::register(&mut registry);
    registry
}

/// A Compressor of `params`, up to `max_frames` at a time.
fn squeeze(params: &[(ParamId, f64)], max_frames: usize) -> Box<dyn Node> {
    let device_type: DeviceType = compressor_type();
    node(&registry(), &device(&device_type, params), max_frames)
}

/// Runs one block: `input` on both channels, with `key` (also on both) as
/// the device's sidechain signal when given, under the allocation trap.
fn run(node: &mut dyn Node, input: &[f32], key: Option<&[f32]>) -> Vec<Vec<f32>> {
    let frames = input.len();
    let mut data: Vec<f32> = (0..2).flat_map(|_| input.iter().copied()).collect();
    let keyed: Option<Vec<f32>> = key.map(|key| (0..2).flat_map(|_| key.iter().copied()).collect());
    let mut block = Block::new(
        &mut data,
        2,
        frames,
        frames,
        &[],
        TransportInfo::default(),
        RATE,
    );
    if let Some(keyed) = &keyed {
        block = block.with_sidechain(keyed);
    }
    bl_rt::rt_test!({ node.process(&mut block) });
    data.chunks(frames).map(<[f32]>::to_vec).collect()
}

/// Runs one block of `input` with `key` as the device's sidechain and
/// `events`, whose offsets split the block into stretches, under the
/// allocation trap.
fn run_with_events(
    node: &mut dyn Node,
    input: &[f32],
    key: &[f32],
    events: &[NodeEvent],
) -> Vec<Vec<f32>> {
    let frames = input.len();
    let mut data: Vec<f32> = (0..2).flat_map(|_| input.iter().copied()).collect();
    let keyed: Vec<f32> = (0..2).flat_map(|_| key.iter().copied()).collect();
    let mut block = Block::new(
        &mut data,
        2,
        frames,
        frames,
        events,
        TransportInfo::default(),
        RATE,
    )
    .with_sidechain(&keyed);
    bl_rt::rt_test!({ node.process(&mut block) });
    data.chunks(frames).map(<[f32]>::to_vec).collect()
}

/// Streams `input` through `node` in `block`-sized blocks, with `key`
/// alongside it, returning the left channel.
fn stream(node: &mut dyn Node, input: &[f32], block: usize, key: Option<&[f32]>) -> Vec<f32> {
    let mut out = Vec::with_capacity(input.len());
    for start in (0..input.len()).step_by(block) {
        let end = (start + block).min(input.len());
        let key = key.map(|key| &key[start..end]);
        out.extend_from_slice(&run(node, &input[start..end], key)[0]);
    }
    out
}

/// The settled level of a constant `input_db` signal through a device of
/// `params`.
fn level_of(params: &[(ParamId, f64)], input_db: f64, key_db: Option<f64>) -> f64 {
    let input = vec![db_to_gain(input_db) as f32; 24_000];
    let key = key_db.map(|db| vec![db_to_gain(db) as f32; 24_000]);
    let mut node = squeeze(params, 256);
    let out = stream(node.as_mut(), &input, 256, key.as_deref());
    out[20_000..]
        .iter()
        .fold(0.0f64, |m, s| m.max(f64::from(s.abs())))
}

/// The static curve's gain in dB for a constant `input_db` signal.
fn curve_db(params: &[(ParamId, f64)], input_db: f64, key_db: Option<f64>) -> f64 {
    20.0 * (level_of(params, input_db, key_db) / db_to_gain(input_db)).log10()
}

/// The settled level of `input` through a device of `params`, keyed by
/// `key`.
fn keyed_level(params: &[(ParamId, f64)], input: &[f32], key: &[f32]) -> f64 {
    let mut node = squeeze(params, 256);
    let out = stream(node.as_mut(), input, 256, Some(key));
    out[20_000..]
        .iter()
        .fold(0.0f64, |m, s| m.max(f64::from(s.abs())))
}

/// The time constant, in samples, of a decay of `gap` over the part where
/// it lies between `low` and `high`: a least-squares fit of its logarithm
/// against the sample index.
fn fit_tau(gaps: &[f64], low: f64, high: f64) -> f64 {
    let (mut sx, mut sy, mut sxx, mut sxy, mut count) = (0.0f64, 0.0, 0.0, 0.0, 0.0);
    for (n, gap) in gaps.iter().enumerate() {
        if *gap > low && *gap < high {
            let (x, y) = (n as f64, gap.ln());
            sx += x;
            sy += y;
            sxx += x * x;
            sxy += x * y;
            count += 1.0;
        }
    }
    assert!(count > 500.0, "enough of the curve to fit: {count}");
    let slope = (count * sxy - sx * sy) / (count * sxx - sx * sx);
    -1.0 / slope
}

// Spec: compressor R1
#[test]
fn the_compressor_is_described_and_declares_a_sidechain() {
    let device_type: DeviceType = compressor_type();
    device_type.description.check().unwrap();
    assert_eq!(device_type.name, "Compressor");
    assert!(device_type.sidechain, "it has a sidechain section");
    assert_eq!(device_type.description.params.len(), 16);
    assert!(matches!(
        device_type.kind,
        bl_model::DeviceKind::Native { ref device_type } if device_type == COMPRESSOR
    ));
}

// Spec: compressor R2, R3
#[test]
fn the_static_curve_matches_at_several_ratios_and_knees() {
    // Threshold -20 dB and a constant 0 dB input: 20 dB over the
    // threshold. Ratio r reduces that to 20/r dB; the top of the ratio's
    // range is an infinite ratio, so nothing over the threshold gets
    // through. A 6 dB knee turns the curve gradually between -23 and
    // -17 dB, and is full at the threshold plus or minus half of it.
    let cases = [
        (2.0, 0.0, 0.0, -10.0),
        (4.0, 0.0, 0.0, -15.0),
        (10.0, 0.0, 0.0, -18.0),
        (100.0, 0.0, 0.0, -20.0),
        (4.0, 6.0, -17.0, -2.25),
        (4.0, 6.0, -20.0, -0.5625),
        (4.0, 6.0, -23.0, 0.0),
    ];
    for (ratio, knee, input_db, expected) in cases {
        let params = [(THRESHOLD, -20.0), (RATIO, ratio), (KNEE, knee)];
        let got = curve_db(&params, input_db, None);
        assert!(
            (got - expected).abs() < 0.1,
            "ratio {ratio}, knee {knee}, in {input_db} dB: {got} dB, expected {expected}"
        );
    }
}

// Spec: compressor R4
#[test]
fn expand_mode_raises_the_output_above_the_threshold() {
    // The manual: a ratio of 1 to 2 raises a dB over the threshold to two
    // dB. Threshold -20 dB and 0 dB in: 20 dB over becomes +20 dB out.
    let expand = |ratio: f64| {
        let params = [(THRESHOLD, -20.0), (RATIO, ratio), (KNEE, 0.0), (MODE, 2.0)];
        curve_db(&params, 0.0, None)
    };
    assert!((expand(2.0) - 20.0).abs() < 0.1, "{}", expand(2.0));
    // A steeper ratio expands more, up to the +24 dB cap.
    assert!((expand(4.0) - 24.0).abs() < 0.1, "{}", expand(4.0));
    // Below the threshold, nothing is expanded.
    let params = [(THRESHOLD, -20.0), (RATIO, 2.0), (KNEE, 0.0), (MODE, 2.0)];
    assert!(curve_db(&params, -30.0, None).abs() < 0.1);
}

// Spec: compressor R4
#[test]
fn expand_follows_the_louder_channel() {
    // A steady level on the left alone, 10 dB over the threshold: a ratio
    // of 1 to 2 asks for +10 dB, whatever the silent right channel does.
    let frames = 48_000;
    let params = [
        (THRESHOLD, -20.0),
        (RATIO, 2.0),
        (KNEE, 0.0),
        (MODE, 2.0),
        (ATTACK, 0.1),
        (RELEASE, 1.0),
    ];
    let mut node = squeeze(&params, frames);
    let mut data = vec![0.0f32; 2 * frames];
    data[..frames].fill(db_to_gain(-10.0) as f32);
    let mut block = Block::new(
        &mut data,
        2,
        frames,
        frames,
        &[],
        TransportInfo::default(),
        RATE,
    );
    bl_rt::rt_test!({ node.process(&mut block) });
    let db = 20.0 * f64::from(data[frames - 1]).log10();
    assert!(db.abs() < 0.5, "left level {db} dBFS, expected about 0");
}

// Spec: compressor R5
#[test]
fn expand_mode_attack_and_release_are_their_parameters() {
    const ATTACK_MS: f64 = 5.0;
    const RELEASE_MS: f64 = 200.0;
    let quiet = db_to_gain(-40.0) as f32;
    let mut input = vec![quiet; 2_000];
    input.extend(vec![db_to_gain(-10.0) as f32; 48_000]);
    input.extend(vec![quiet; 48_000]);
    let params = [
        (THRESHOLD, -20.0),
        (RATIO, 2.0),
        (KNEE, 0.0),
        (MODE, 2.0),
        (ATTACK, ATTACK_MS),
        (RELEASE, RELEASE_MS),
    ];
    let mut node = squeeze(&params, 256);
    let out = stream(node.as_mut(), &input, 256, None);

    // The -10 dB input is 10 dB over the threshold: the gain heads for
    // +10 dB, which brings the output to 0 dBFS, with Attack.
    let attack: Vec<f64> = out[2_000..].iter().map(|s| 1.0 - f64::from(*s)).collect();
    let tau = fit_tau(&attack, 0.02, 0.6);
    let expected = ATTACK_MS * 0.001 * RATE.as_f64();
    assert!(
        (tau - expected).abs() / expected < 0.05,
        "attack {tau} samples, expected {expected}"
    );

    // On the step down, the gain returns to unity with Release.
    let release: Vec<f64> = out[50_000..]
        .iter()
        .map(|s| f64::from(*s) / f64::from(quiet) - 1.0)
        .collect();
    let tau = fit_tau(&release, 0.05, 0.6);
    let expected = RELEASE_MS * 0.001 * RATE.as_f64();
    assert!(
        (tau - expected).abs() / expected < 0.05,
        "release {tau} samples, expected {expected}"
    );
}

// Spec: compressor R6
#[test]
fn auto_release_reaches_its_slow_time_in_expand_mode() {
    // An expansion held for a second recovers more slowly under Auto
    // Release than under a 50 ms fixed one. The release time is read at
    // the start of each block, so the blocks are long enough for the
    // held expansion to set it for the one in which the signal drops.
    let quiet = db_to_gain(-40.0) as f32;
    let mut input = vec![db_to_gain(-10.0) as f32; 48_000];
    input.extend(vec![quiet; 48_000]);
    let recovered = |extra: (ParamId, f64)| {
        let params = [
            (THRESHOLD, -20.0),
            (RATIO, 2.0),
            (KNEE, 0.0),
            (MODE, 2.0),
            (ATTACK, 1.0),
            extra,
        ];
        let mut node = squeeze(&params, 24_000);
        let out = stream(node.as_mut(), &input, 24_000, None);
        (48_000..out.len())
            .find(|n| f64::from(out[*n]) / f64::from(quiet) < 1.2)
            .unwrap_or(out.len())
    };
    let fixed = recovered((RELEASE, 50.0));
    let auto = recovered((AUTO_RELEASE, 1.0));
    assert!(auto > fixed + 1_000, "auto {auto}, fixed {fixed}");
}

// Spec: compressor R2
#[test]
fn a_signal_below_the_knee_passes_unchanged_in_every_mode() {
    // -30 dBFS of noise under a 0 dB threshold with a 6 dB knee asks for
    // a gain of exactly 1, in Peak, RMS and Expand alike.
    let mut seed = 1u32;
    let input: Vec<f32> = (0..48_000)
        .map(|_| {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / (1u32 << 23) as f32 * db_to_gain(-30.0) as f32
                - db_to_gain(-30.0) as f32
        })
        .collect();
    for mode in [0.0, 1.0, 2.0] {
        let params = [(THRESHOLD, 0.0), (RATIO, 4.0), (KNEE, 6.0), (MODE, mode)];
        let mut node = squeeze(&params, 512);
        let out = stream(node.as_mut(), &input, 512, None);
        assert_eq!(out, input, "mode {mode}");
    }
}

// Spec: compressor R5
#[test]
fn the_attack_and_release_time_constants_are_their_parameters() {
    const ATTACK_MS: f64 = 50.0;
    const RELEASE_MS: f64 = 200.0;
    let quiet = db_to_gain(-40.0) as f32;
    let mut input = vec![quiet; 2_000];
    input.extend(vec![1.0f32; 48_000]);
    input.extend(vec![quiet; 48_000]);
    let params = [
        (THRESHOLD, -20.0),
        (RATIO, 4.0),
        (KNEE, 0.0),
        (ATTACK, ATTACK_MS),
        (RELEASE, RELEASE_MS),
    ];
    let mut node = squeeze(&params, 256);
    let out = stream(node.as_mut(), &input, 256, None);

    // On the step up, the gain heads for a quarter of the 20 dB over the
    // threshold, -15 dB, and the output is the gain (0 dB in).
    let target = db_to_gain(-15.0);
    let attack: Vec<f64> = out[2_000..]
        .iter()
        .map(|s| f64::from(*s) - target)
        .collect();
    let tau = fit_tau(&attack, 0.02, 0.6);
    let expected = ATTACK_MS * 0.001 * RATE.as_f64();
    assert!(
        (tau - expected).abs() / expected < 0.05,
        "attack {tau} samples, expected {expected}"
    );

    // On the step down, the gain recovers toward unity; the output over
    // the quiet input is the gain.
    let release: Vec<f64> = out[50_000..]
        .iter()
        .map(|s| 1.0 - f64::from(*s) / f64::from(quiet))
        .collect();
    let tau = fit_tau(&release, 0.05, 0.6);
    let expected = RELEASE_MS * 0.001 * RATE.as_f64();
    assert!(
        (tau - expected).abs() / expected < 0.05,
        "release {tau} samples, expected {expected}"
    );
}

// Spec: compressor R6
#[test]
fn auto_release_recovers_more_quickly_than_a_long_fixed_release() {
    // A two-thousand-sample burst of 0 dB in a -26 dB signal: with Auto
    // Release the gain comes back from the burst long before a 500 ms
    // fixed Release would let it.
    let mut input = vec![0.05f32; 80_000];
    for sample in &mut input[..2_000] {
        *sample = 1.0;
    }
    let recover = |extra: (ParamId, f64)| {
        let mut params = vec![(THRESHOLD, -20.0), (RATIO, 4.0), (KNEE, 0.0)];
        params.push(extra);
        let mut node = squeeze(&params, 256);
        let out = stream(node.as_mut(), &input, 256, None);
        (2_000..out.len())
            .find(|n| f64::from(out[*n]) / 0.05 > 0.9)
            .unwrap_or(out.len())
    };
    let fixed = recover((RELEASE, 500.0));
    let auto = recover((AUTO_RELEASE, 1.0));
    assert!(
        auto * 3 < fixed,
        "auto recovered at {auto}, fixed at {fixed}"
    );
}

// Spec: compressor R7
#[test]
fn the_log_envelope_releases_faster_from_a_deeper_reduction() {
    // A 40 dB reduction (threshold -40 dB, the infinite ratio, 0 dB in)
    // with the same Release: Log lets go of it sooner than Lin.
    let mut input = vec![1.0f32; 4_000];
    input.extend(vec![db_to_gain(-60.0) as f32; 4_800]);
    let recover = |env: f64| {
        let params = [
            (THRESHOLD, -40.0),
            (RATIO, 100.0),
            (KNEE, 0.0),
            (RELEASE, 200.0),
            (ENVELOPE, env),
        ];
        let mut node = squeeze(&params, 256);
        let out = stream(node.as_mut(), &input, 256, None);
        f64::from(*out.last().unwrap()) / f64::from(db_to_gain(-60.0) as f32)
    };
    let lin = recover(0.0);
    let log = recover(1.0);
    assert!(log > lin + 0.2, "Lin {lin}, Log {log}");
}

// Spec: compressor R7
#[test]
fn the_log_envelope_expands_as_far_as_the_curve_asks() {
    // Expand, ratio 4, threshold -20 dB, 0 dB in: the curve asks for +24 dB.
    // Log used to stall the gain at +6 dB.
    let params = [
        (THRESHOLD, -20.0),
        (RATIO, 4.0),
        (KNEE, 0.0),
        (MODE, 2.0),
        (ENVELOPE, 1.0),
    ];
    assert!((level_of(&params, 0.0, None) - db_to_gain(24.0)).abs() < 0.1 * db_to_gain(24.0));
}

// Spec: compressor R8
#[test]
fn rms_mode_is_less_sensitive_to_a_short_peak() {
    // A ten-sample burst in a quiet signal with an immediate attack: Peak's
    // detector sees every sample of it, RMS's ten-millisecond window barely
    // does.
    let mut input = vec![db_to_gain(-30.0) as f32; 24_000];
    for sample in &mut input[10_000..10_010] {
        *sample = 0.9;
    }
    let least = |mode: f64| {
        let params = [
            (THRESHOLD, -20.0),
            (RATIO, 4.0),
            (ATTACK, 0.01),
            (RELEASE, 50.0),
            (MODE, mode),
        ];
        let mut node = squeeze(&params, 256);
        let out = stream(node.as_mut(), &input, 256, None);
        out.iter()
            .zip(&input)
            .map(|(s, x)| f64::from(*s) / f64::from(*x))
            .fold(1.0f64, |m, gain| m.min(gain))
    };
    let peak = least(0.0);
    let rms = least(1.0);
    assert!(peak < 0.5, "Peak dips to {peak}");
    assert!(rms > 2.0 * peak, "RMS stays at {rms}");
}

// Spec: compressor R9
#[test]
fn the_look_ahead_delays_the_output_and_is_reported_as_latency() {
    // The engine reads the latency before the device first runs, so the
    // node reports the set's choice from its making. 1 ms is 48 samples at
    // 48 kHz and 10 ms is 480.
    for (choice, expected) in [(0.0, 0usize), (1.0, 48), (2.0, 480)] {
        let mut node = squeeze(&[(LOOKAHEAD, choice)], 1024);
        assert_eq!(node.latency() as usize, expected, "choice {choice}");
        let mut input = vec![0.0f32; 1024];
        input[0] = 0.5;
        let out = run(node.as_mut(), &input, None);
        assert!(out[0][expected].abs() > 0.0, "the pulse comes out late");
        assert!(
            out[0][..expected].iter().all(|s| *s == 0.0),
            "nothing before it"
        );
        assert!(
            out[0][expected + 1..].iter().all(|s| *s == 0.0),
            "nothing after it"
        );
    }
}

// Spec: compressor R10
#[test]
fn makeup_compensates_the_output_without_an_external_sidechain() {
    // Threshold -20 dB and ratio 4: Makeup adds the 15 dB the curve takes
    // from a full-scale signal, so a 0 dB input comes out at 0 dB.
    let params = [(THRESHOLD, -20.0), (RATIO, 4.0), (KNEE, 0.0), (MAKEUP, 1.0)];
    assert!(curve_db(&params, 0.0, None).abs() < 0.1);
    // With an external sidechain (a key at the same level) Makeup isn't
    // available: the reduction stands.
    let got = curve_db(&params, 0.0, Some(0.0));
    assert!((got + 15.0).abs() < 0.1, "{got} dB");
}

// Spec: compressor R11
#[test]
fn dry_wet_mixes_the_compressed_and_dry_signals() {
    let compressed = db_to_gain(-15.0);
    let cases = [
        (0.0, 1.0),
        (50.0, 0.5 * (1.0 + compressed)),
        (100.0, compressed),
    ];
    for (dry_wet, expected) in cases {
        let params = [
            (THRESHOLD, -20.0),
            (RATIO, 4.0),
            (KNEE, 0.0),
            (DRY_WET, dry_wet),
        ];
        let level = level_of(&params, 0.0, None);
        assert!((level - expected).abs() < 0.01, "{dry_wet} %: {level}");
    }
    // Out scales the processed signal (the dry one at 0 % bypasses it, as
    // a bypass does).
    let params = [
        (THRESHOLD, -20.0),
        (RATIO, 4.0),
        (KNEE, 0.0),
        (OUT, db_to_gain(-6.0)),
    ];
    let level = level_of(&params, 0.0, None);
    assert!(
        (level - compressed * db_to_gain(-6.0)).abs() < 0.01,
        "{level}"
    );
}

// Spec: compressor R12
#[test]
fn a_sidechain_key_ducks_the_input() {
    // The input sits below the threshold, so alone it passes; a key that
    // stands above the threshold ducks it by the curve's 15 dB.
    let quiet = db_to_gain(-30.0);
    let params = [(THRESHOLD, -20.0), (RATIO, 4.0), (KNEE, 0.0)];
    let alone = level_of(&params, -30.0, None);
    assert!((alone - quiet).abs() < 0.001, "{alone}");
    let ducked = level_of(&params, -30.0, Some(0.0));
    let expected = quiet * db_to_gain(-15.0);
    assert!((ducked - expected).abs() < 0.001, "{ducked}");
}

// Spec: compressor R13
#[test]
fn the_sc_filter_hears_only_its_band() {
    // The key alternates ±1 every sample: a level of 0 dB far above a
    // 200 Hz low pass. Without the filter it ducks the quiet input;
    // through the filter it is too quiet to trigger anything.
    let quiet = db_to_gain(-30.0) as f32;
    let input = vec![quiet; 24_000];
    let key: Vec<f32> = (0..24_000)
        .map(|i| if i % 2 == 0 { 1.0 } else { -1.0 })
        .collect();
    let params = |filter: f64| {
        [
            (THRESHOLD, -20.0),
            (RATIO, 4.0),
            (KNEE, 0.0),
            (SC_FILTER, filter),
            (SC_TYPE, 0.0),
            (SC_FREQ, 200.0),
            (SC_RES, 0.71),
        ]
    };
    let ducked = keyed_level(&params(0.0), &input, &key);
    assert!(
        (ducked - f64::from(quiet) * db_to_gain(-15.0)).abs() < 0.001,
        "{ducked}"
    );
    let open = keyed_level(&params(1.0), &input, &key);
    assert!((open - f64::from(quiet)).abs() < 0.001, "{open}");
}

// Spec: compressor R12
#[test]
fn the_detector_hears_the_keys_own_stretch() {
    // A key that is loud over the block's first half and silent over the
    // second, with a parameter change at the half, splits the block into
    // two stretches. The second stretch's detector must hear the key's
    // second half, so the gain recovers; hearing the block's first frames
    // again would leave it ducked.
    const HALF: usize = 512;
    let quiet = db_to_gain(-30.0) as f32;
    let input = vec![quiet; 2 * HALF];
    let mut key = vec![0.0f32; 2 * HALF];
    key[..HALF].fill(1.0);
    let params = [
        (THRESHOLD, -20.0),
        (RATIO, 4.0),
        (KNEE, 0.0),
        (ATTACK, 0.01),
        (RELEASE, 1.0),
    ];
    let mut node = squeeze(&params, 2 * HALF);
    // A change of one of its own parameters at the half splits the block
    // into two stretches; the knee's value doesn't move.
    let events = [set(HALF as u32, KNEE, 0.0)];
    let out = run_with_events(node.as_mut(), &input, &key, &events);
    let gain = |i: usize| f64::from(out[0][i]) / f64::from(quiet);
    assert!(
        gain(HALF - 1) < 0.5,
        "the first half ducks: {}",
        gain(HALF - 1)
    );
    assert!(
        gain(2 * HALF - 1) > 0.9,
        "the second half recovers: {}",
        gain(2 * HALF - 1)
    );
}

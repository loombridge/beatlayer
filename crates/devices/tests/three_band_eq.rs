//! Triad, the native three-band EQ (WP-5.21; the three-band-eq spec): the
//! bands summing flat at 24 dB and colouring at 48 dB, the crossovers'
//! slopes, a band at -inf or off leaving the others' response, the signal
//! lights, and the display.

bl_rt::install_trap!();

mod common;

use std::f64::consts::TAU;

use bl_devices::NativeNode;
use bl_devices::three_band_eq::{
    FREQ_HIGH, FREQ_LOW, GAIN_HIGH, GAIN_LOW, GAIN_MID, ON_HIGH, ON_LOW, ON_MID, SLOPE,
    THREE_BAND_EQ, Triad, three_band_eq_type,
};
use bl_engine::{DeviceType, Node, Registry};
use bl_model::ParamId;
use bl_model::param::db_to_gain;
use common::response::{Complex, add, db, impulse_response, response_at};
use common::{RATE, config, device, node, noise, run, set};

const FRAMES: usize = 16_384;

fn registry() -> Registry {
    let mut registry = Registry::empty();
    bl_devices::three_band_eq::register(&mut registry);
    registry
}

fn triad(params: &[(ParamId, f64)], max_frames: usize) -> Box<dyn Node> {
    let device_type: DeviceType = three_band_eq_type();
    node(&registry(), &device(&device_type, params), max_frames)
}

fn impulse(params: &[(ParamId, f64)]) -> Vec<f32> {
    impulse_response(triad(params, FRAMES).as_mut(), FRAMES)
}

/// `params` with every band but `band` (0, 1 or 2) switched off.
fn only(band: usize, params: &[(ParamId, f64)]) -> Vec<(ParamId, f64)> {
    let mut out = params.to_vec();
    for (i, on) in [ON_LOW, ON_MID, ON_HIGH].into_iter().enumerate() {
        if i != band {
            out.push((on, 0.0));
        }
    }
    out
}

/// The response in dB at 60 frequencies from 25 Hz to 18 kHz, evenly spaced
/// on a logarithmic axis.
fn sweep(ir: &[f32]) -> Vec<(f64, f64)> {
    (0..60)
        .map(|i| {
            let hz = 25.0 * (18_000.0f64 / 25.0).powf(f64::from(i) / 59.0);
            (hz, db(response_at(ir, hz)))
        })
        .collect()
}

// Spec: three-band-eq R1
#[test]
fn the_three_band_eq_is_described_with_its_ranges_and_defaults() {
    let description = three_band_eq_type().description;
    description.check().unwrap();
    for id in [GAIN_LOW, GAIN_MID, GAIN_HIGH] {
        let gain = description.param(id).unwrap();
        assert!(gain.is_gain());
        assert_eq!(gain.min(), 0.0, "down to silence");
        assert!((gain.max() - db_to_gain(6.0)).abs() < 1e-9, "up to +6 dB");
        assert_eq!(gain.default, 1.0);
    }
    for id in [ON_LOW, ON_MID, ON_HIGH] {
        assert_eq!(
            description.param(id).unwrap().default,
            1.0,
            "bands start on"
        );
    }
    assert_eq!(
        description.param(SLOPE).unwrap().default,
        0.0,
        "24 dB first"
    );
}

// Spec: three-band-eq R2
#[test]
fn at_24_db_the_bands_sum_flat_at_any_crossovers() {
    for (low, high) in [
        (250.0, 2_500.0),
        (500.0, 2_000.0),
        (500.0, 700.0),
        (60.0, 9_000.0),
    ] {
        let ir = impulse(&[(FREQ_LOW, low), (FREQ_HIGH, high)]);
        for (hz, level) in sweep(&ir) {
            assert!(
                level.abs() < 0.1,
                "{low}/{high} Hz at {hz:.0} Hz: {level} dB"
            );
        }
    }
}

// Spec: three-band-eq R3
#[test]
fn at_48_db_the_sum_is_coloured_where_the_crossovers_sit_close() {
    let worst = |low: f64, high: f64| {
        sweep(&impulse(&[
            (SLOPE, 1.0),
            (FREQ_LOW, low),
            (FREQ_HIGH, high),
        ]))
        .into_iter()
        .map(|(_, level)| level)
        .fold(0.0f64, |m, l| if l.abs() > m.abs() { l } else { m })
    };
    // Slight at the defaults, 3.3 octaves apart; 2 dB deep two octaves apart.
    let defaults = worst(250.0, 2_500.0);
    assert!(
        defaults.abs() > 0.05 && defaults.abs() < 0.5,
        "{defaults} dB"
    );
    let example = worst(500.0, 2_000.0);
    assert!((-3.0..-1.5).contains(&example), "{example} dB");
    // Far enough apart, nothing is left of it.
    assert!(worst(60.0, 9_000.0).abs() < 0.1);
}

// Spec: three-band-eq R4
#[test]
fn each_crossover_falls_at_its_slope() {
    // (steep, the two probe frequencies in crossover multiples) so that the
    // second stays well above the measurement's floor.
    for (steep, near, far, slope) in [(0.0, 2.0, 4.0, 24.0), (1.0, 1.5, 3.0, 48.0)] {
        let base = [(SLOPE, steep), (FREQ_LOW, 100.0), (FREQ_HIGH, 1_000.0)];
        // The low band falls above FreqLo...
        let ir = impulse(&only(0, &base));
        let fall = db(response_at(&ir, 100.0 * near)) - db(response_at(&ir, 100.0 * far));
        assert!((fall - slope).abs() < 1.0, "low band, {slope} dB: {fall}");
        // ... the mid band rises below FreqLo (set higher here, so the
        // probes stay clear of the float filters' low-frequency noise floor)
        // and falls above FreqHi...
        let wide = [(SLOPE, steep), (FREQ_LOW, 400.0), (FREQ_HIGH, 1_000.0)];
        let ir = impulse(&only(1, &wide));
        let rise = db(response_at(&ir, 400.0 / near)) - db(response_at(&ir, 400.0 / far));
        assert!(
            (rise - slope).abs() < 1.0,
            "mid band rising, {slope} dB: {rise}"
        );
        let fall = db(response_at(&ir, 1_000.0 * near)) - db(response_at(&ir, 1_000.0 * far));
        assert!(
            (fall - slope).abs() < 1.0,
            "mid band falling, {slope} dB: {fall}"
        );
        // ... and the high band rises below FreqHi.
        let ir = impulse(&only(2, &base));
        let rise = db(response_at(&ir, 1_000.0 / near)) - db(response_at(&ir, 1_000.0 / far));
        assert!((rise - slope).abs() < 1.0, "high band, {slope} dB: {rise}");
    }
}

/// Complex responses of three impulse responses, at `hz`.
fn at(irs: &[&Vec<f32>], hz: f64) -> Complex {
    irs.iter()
        .map(|ir| response_at(ir, hz))
        .fold((0.0, 0.0), add)
}

// Spec: three-band-eq R5
#[test]
fn a_band_at_minus_infinity_or_off_leaves_the_response_of_the_other_two() {
    for steep in [0.0, 1.0] {
        let base = [(SLOPE, steep)];
        let low = impulse(&only(0, &base));
        let mid = impulse(&only(1, &base));
        let high = impulse(&only(2, &base));
        let mut silenced = base.to_vec();
        silenced.push((GAIN_MID, 0.0));
        let at_minus_infinity = impulse(&silenced);
        let mut switched = base.to_vec();
        switched.push((ON_MID, 0.0));
        let switched_off = impulse(&switched);
        for (hz, _) in sweep(&low) {
            let want = db(at(&[&low, &high], hz));
            assert!(
                (db(response_at(&at_minus_infinity, hz)) - want).abs() < 0.1,
                "-inf at {hz:.0} Hz"
            );
            assert!(
                (db(response_at(&switched_off, hz)) - want).abs() < 0.1,
                "off at {hz:.0} Hz"
            );
        }
        // Each band's gain scales its own part: +6 dB doubles the mid band.
        let mut louder = only(1, &base);
        louder.push((GAIN_MID, db_to_gain(6.0)));
        let louder = impulse(&louder);
        let (plain, boosted) = (at(&[&mid], 1_000.0), at(&[&louder], 1_000.0));
        assert!((db(boosted) - db(plain) - 6.0).abs() < 0.01);
    }
}

/// How the lights read after `tone_db` at `hz` for a third of a second.
fn lights_after(hz: f64, tone_db: f64, extra: &[(ParamId, f64)]) -> [bool; 3] {
    let device_type = three_band_eq_type();
    let values = device(&device_type, extra);
    let config = config(4096);
    let mut node = NativeNode::new(
        Triad::new(&config),
        &device_type.description,
        &values,
        &config,
    );
    let amplitude = db_to_gain(tone_db);
    for block in 0..4 {
        let tone: Vec<f32> = (0..4096)
            .map(|n| {
                let n = (block * 4096 + n) as f64;
                (amplitude * (TAU * hz * n / RATE.as_f64()).sin()) as f32
            })
            .collect();
        run(&mut node, &tone, &[]);
    }
    node.device().lights()
}

// Spec: three-band-eq R6
#[test]
fn a_light_is_lit_while_its_band_carries_signal_above_minus_24_db() {
    assert_eq!(lights_after(60.0, -20.0, &[]), [true, false, false]);
    assert_eq!(lights_after(1_000.0, -20.0, &[]), [false, true, false]);
    assert_eq!(lights_after(9_000.0, -20.0, &[]), [false, false, true]);
    // Below the threshold nothing is lit.
    assert_eq!(lights_after(1_000.0, -30.0, &[]), [false; 3]);
    // The light works with the band switched off or at -inf.
    assert_eq!(
        lights_after(1_000.0, -20.0, &[(ON_MID, 0.0)]),
        [false, true, false]
    );
    assert_eq!(
        lights_after(1_000.0, -20.0, &[(GAIN_MID, 0.0)]),
        [false, true, false]
    );
}

// Spec: three-band-eq R6
#[test]
fn the_display_reads_the_lights() {
    let device_type = three_band_eq_type();
    let values = device(&device_type, &[]);
    let shown = bl_devices::display(
        THREE_BAND_EQ,
        values.id,
        &device_type.description,
        &values.params,
        Some(&[0.0, 1.0, 0.0]),
    )
    .expect("Triad has a display");
    let texts: Vec<_> = shown.readings.iter().map(|r| r.text.as_str()).collect();
    assert_eq!(texts, ["off", "lit", "off"]);
    assert_eq!(shown.readings[1].name, "Mid Signal");
}

// Spec: three-band-eq R7
#[test]
fn renders_are_bit_identical_at_any_block_size_and_add_no_latency() {
    let input = noise(8192);
    let mut renders = Vec::new();
    for block in [1usize, 64, 512, 4096] {
        let mut node = triad(&[], block);
        assert_eq!(node.latency(), 0);
        let mut out = Vec::new();
        for (n, chunk) in input.chunks(block).enumerate() {
            let events = if n == 0 {
                vec![
                    set(0, SLOPE, 1.0),
                    set(0, GAIN_LOW, 1.5),
                    set(0, FREQ_LOW, 400.0),
                ]
            } else if n * block >= 4096 && (n - 1) * block < 4096 {
                vec![set(0, ON_MID, 0.0)]
            } else {
                Vec::new()
            };
            out.extend_from_slice(&run(node.as_mut(), chunk, &events)[0]);
        }
        renders.push(out);
    }
    for render in &renders[1..] {
        assert_eq!(render, &renders[0]);
    }
}

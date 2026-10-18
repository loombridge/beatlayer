//! What the two modulators share (WP-5.10; the modulation spec, R12 to R16):
//! they leave audio alone, Depth scales their effect, Polarity and the
//! slot's Amount shape what they contribute, and Remote Control gives a
//! position to hold in place of a contribution. Every block runs as a
//! real-time section under the allocation trap.

bl_rt::install_trap!();

mod common;

use bl_core::time::{Frames, TempoMap};
use bl_devices::modulator::{RATE_HZ, WAVE, curve_mod_type, wave_mod_type};
use bl_engine::modbus::NOT_HELD;
use bl_engine::{Registry, TransportInfo};
use bl_model::ParamId;
use bl_model::modulator::{self, CURVE_MOD, DEPTH, MODE, POLARITY, WAVE_MOD};
use common::{control, device, node, noise, run};

fn registry() -> Registry {
    let mut registry = Registry::empty();
    bl_devices::modulator::register(&mut registry);
    registry
}

const PLAYING: TransportInfo = TransportInfo {
    playing: true,
    position: Frames(0),
};

/// A Wave Mod's node on the rising ramp at 10 Hz (a 4,800-frame cycle) with
/// `params` over those.
fn ramp(params: &[(ParamId, f64)]) -> Box<dyn bl_engine::Node> {
    let mut all = vec![(WAVE, 1.0), (RATE_HZ, 10.0)];
    all.extend_from_slice(params);
    node(&registry(), &device(&wave_mod_type(), &all), 4_800)
}

// Spec: modulation R12
#[test]
fn modulators_pass_their_audio_and_notes_untouched() {
    for (kind, device_type) in [(WAVE_MOD, wave_mod_type()), (CURVE_MOD, curve_mod_type())] {
        let mut node = node(&registry(), &device(&device_type, &[]), 512);
        let input = noise(512);
        let out = run(node.as_mut(), &input, &[]);
        assert!(
            out.iter().all(|channel| *channel == input),
            "{kind} left the audio as it was"
        );
    }
}

// Spec: modulation R13
#[test]
fn depth_scales_the_effect_from_none_to_full() {
    let reach = |depth: f64| {
        let mut node = ramp(&[
            (modulator::amount(0), 100.0),
            (POLARITY, 1.0),
            (DEPTH, depth),
        ]);
        let rows = control(
            node.as_mut(),
            4_800,
            512,
            &[],
            &TempoMap::default(),
            PLAYING,
        );
        rows.contribution(0).iter().copied().fold(0.0, f32::max)
    };
    assert_eq!(reach(0.0), 0.0, "0 % has no effect");
    let half = reach(50.0);
    let full = reach(100.0);
    assert!(full > 0.95, "{full}");
    assert!((half - full / 2.0).abs() < 1e-3, "{half} of {full}");
}

// Spec: modulation R14
#[test]
fn unipolar_adds_from_the_base_and_bipolar_swings_round_it() {
    let rows = |polarity: f64, amount: f64| {
        let mut node = ramp(&[
            (modulator::amount(0), amount),
            (POLARITY, polarity),
            (DEPTH, 100.0),
        ]);
        control(
            node.as_mut(),
            4_800,
            512,
            &[],
            &TempoMap::default(),
            PLAYING,
        )
        .contribution(0)
        .to_vec()
    };
    let up = rows(1.0, 100.0);
    assert!(up.iter().all(|&c| (0.0..=1.0).contains(&c)), "one way, up");
    let down = rows(1.0, -100.0);
    assert!(
        down.iter().zip(&up).all(|(d, u)| (d + u).abs() < 1e-6),
        "a negative amount goes the other way"
    );
    let swing = rows(0.0, 100.0);
    let (low, high) = swing
        .iter()
        .fold((0.0f32, 0.0f32), |(l, h), &c| (l.min(c), h.max(c)));
    assert!(
        low < -0.9 && high > 0.9,
        "both ways round the base: {low} {high}"
    );
    assert!(
        swing
            .iter()
            .zip(&up)
            .all(|(s, u)| (s - (2.0 * u - 1.0)).abs() < 1e-6),
        "the swing is 2u − 1"
    );
}

// Spec: modulation R16
#[test]
fn remote_control_holds_the_target_between_min_and_max() {
    let mut node = ramp(&[
        (MODE, 1.0),
        (modulator::min(0), 20.0),
        (modulator::max(0), 60.0),
        (modulator::amount(0), 100.0),
    ]);
    let rows = control(
        node.as_mut(),
        4_800,
        512,
        &[],
        &TempoMap::default(),
        PLAYING,
    );
    assert!(
        rows.contribution(0).iter().all(|&c| c == 0.0),
        "a held target gets no contribution"
    );
    let hold = rows.hold(0);
    assert!(
        hold.iter().all(|&h| (0.2 - 1e-6..=0.6 + 1e-6).contains(&h)),
        "between Min and Max"
    );
    let (low, high) = hold
        .iter()
        .fold((1.0f32, 0.0f32), |(l, h), &v| (l.min(v), h.max(v)));
    assert!(
        low < 0.21 && high > 0.59,
        "and it travels all of it: {low} {high}"
    );
    // Min above Max inverts.
    let mut node = ramp(&[
        (MODE, 1.0),
        (modulator::min(0), 60.0),
        (modulator::max(0), 20.0),
    ]);
    let rows = control(
        node.as_mut(),
        4_800,
        512,
        &[],
        &TempoMap::default(),
        PLAYING,
    );
    assert!(
        rows.hold(0)[100] > rows.hold(0)[4_000],
        "the ramp now falls"
    );
    // In Modulation mode nothing is held.
    let mut node = ramp(&[]);
    let rows = control(node.as_mut(), 512, 512, &[], &TempoMap::default(), PLAYING);
    assert!(rows.hold(0).iter().all(|&h| h == NOT_HELD));
}

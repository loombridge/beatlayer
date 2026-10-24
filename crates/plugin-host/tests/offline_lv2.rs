//! Offline renders play copies of the engine's LV2 plug-ins (WP-5.39; the
//! offline plug-in renders spec, as `offline.rs` has it for CLAP): an
//! instance has its rate fixed when it is made, and LV2 lets its state be
//! saved while it runs, so a render makes a copy at the render's own
//! settings from the state the plug-in has, and the engine's instance never
//! stops. The engine's node runs on a thread of its own here, as on an audio
//! device, and every block is a real-time section under the allocation trap.
//!
//! Spec: lv2-hosting R15, as clap-hosting R17.

use crate::common;
use bl_core::time::SampleRate;
use bl_engine::{NodeConfig, NodeFactory};
use bl_plugin_host::lv2::{Lv2Error, Lv2Host};
use bl_test_lv2::GAIN;
use common::{Engine, config, lv2_device, lv2_host, run};

/// A host with the test plug-in, and the engine running a device of it at
/// a gain of 0.5.
fn playing() -> Option<(Lv2Host, Engine)> {
    let host = lv2_host()?;
    let node = host
        .factory()
        .create(&lv2_device(1, &[(GAIN, 0.5)]), &config())
        .unwrap();
    let engine = Engine::start(node);
    engine.settles_at(0.5);
    Some((host, engine))
}

/// The render's settings: another rate and block size than the engine's.
fn render_config() -> NodeConfig {
    NodeConfig {
        round_trip_latency: 0,
        sample_rate: SampleRate::new(96_000).unwrap(),
        max_frames: 32,
        channels: 2,
    }
}

// Spec: offline-plugin-renders R3, R4, R5, R9
#[test]
fn a_lent_plugin_renders_at_the_renders_settings_and_the_engine_never_stops() {
    let Some((host, engine)) = playing() else {
        return;
    };
    let id = lv2_device(1, &[]).id;
    assert_eq!(host.status(id).unwrap().active_for, Some(config()));
    let state = host.save_state(id).unwrap();

    {
        let loan = host.lend().unwrap();
        assert_eq!(host.lend().unwrap_err(), Lv2Error::Lent);
        // The rendered set has the gain at 0.25.
        let mut node = loan
            .factory()
            .create(&lv2_device(1, &[(GAIN, 0.25)]), &render_config())
            .unwrap();
        assert!(loan.problems().is_empty());
        let status = host.status(id).unwrap();
        assert!(status.active);
        assert_eq!(status.active_for, Some(render_config()));
        // The engine's instance is its own, at its own gain.
        engine.settles_at(0.5);

        // The render runs its copy, at its values.
        let out = run(node.as_mut(), &[1.0; 32], &[], &[]);
        assert_eq!(out.channels[0][0], 0.25);
        drop(node);
    }

    // Back in the engine, at the engine's settings, with the state it had.
    engine.settles_at(0.5);
    host.idle();
    let status = host.status(id).unwrap();
    assert!(status.active);
    assert_eq!(status.active_for, Some(config()));
    assert_eq!(status.failure, None);
    assert_eq!(host.save_state(id).unwrap(), state);
    assert!(
        host.take_param_changes().is_empty(),
        "a render changes nothing"
    );
}

// Spec: offline-plugin-renders R4
#[test]
fn a_loan_dropped_before_the_renders_node_leaves_the_engine_playing() {
    let Some((host, engine)) = playing() else {
        return;
    };
    let id = lv2_device(1, &[]).id;
    let loan = host.lend().unwrap();
    let mut node = loan
        .factory()
        .create(&lv2_device(1, &[(GAIN, 0.5)]), &render_config())
        .unwrap();
    run(node.as_mut(), &[1.0; 32], &[], &[]);
    // A render abandoned with its node still standing: the copy goes when
    // the node lets go of it.
    drop(loan);
    engine.settles_at(0.5);
    drop(node);
    host.idle();
    host.idle();
    engine.settles_at(0.5);
    assert_eq!(host.status(id).unwrap().active_for, Some(config()));
    // A later render is allowed.
    drop(host.lend().unwrap());
}

// Spec: offline-plugin-renders R4
#[test]
fn a_render_node_that_never_ran_gives_its_copy_back_too() {
    let Some((host, engine)) = playing() else {
        return;
    };
    let id = lv2_device(1, &[]).id;
    {
        let loan = host.lend().unwrap();
        let node = loan
            .factory()
            .create(&lv2_device(1, &[(GAIN, 0.25)]), &render_config())
            .unwrap();
        // Cancelled before its first block.
        drop(node);
    }
    engine.settles_at(0.5);
    host.idle();
    assert_eq!(host.status(id).unwrap().active_for, Some(config()));
}

// Spec: offline-plugin-renders R3. A device the engine hasn't started yet
// renders from the state its set saved, with the rendered set's values over it.
#[test]
fn a_device_without_a_plugin_yet_gets_one_from_its_saved_state() {
    let Some(first) = lv2_host() else { return };
    let mut node = first
        .factory()
        .create(&lv2_device(1, &[(GAIN, 1.5)]), &config())
        .unwrap();
    run(node.as_mut(), &[1.0; 64], &[], &[]);
    let saved = first.save_state(lv2_device(1, &[]).id).unwrap();

    let host = lv2_host().unwrap();
    let id = lv2_device(2, &[]).id;
    let loan = host.lend().unwrap();
    let mut device = lv2_device(2, &[]);
    device.plugin = Some(saved.clone());
    let mut node = loan.factory().create(&device, &render_config()).unwrap();
    assert_eq!(run(node.as_mut(), &[1.0; 32], &[], &[]).channels[0][0], 1.5);
    drop(node);
    drop(loan);
    // The rendered set's values go over the state.
    let loan = host.lend().unwrap();
    let mut device = lv2_device(2, &[(GAIN, 0.75)]);
    device.plugin = Some(saved);
    let mut node = loan.factory().create(&device, &render_config()).unwrap();
    assert_eq!(run(node.as_mut(), &[1.0; 32], &[], &[]).channels[0][0], 0.75);
    drop(node);
    drop(loan);
    // The host keeps it for the engine, inactive until a node asks.
    let status = host.status(id).unwrap();
    assert!(!status.active);
    assert_eq!(status.failure, None);
}

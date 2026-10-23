//! Offline renders borrow the engine's VST3 plug-ins (WP-4.21; the offline
//! plug-in renders spec, as `offline.rs` has it for CLAP): a plug-in comes
//! back from the engine's node, runs in the render at the render's own
//! settings, and goes back to the engine active at the engine's settings
//! with the state it had. The engine's node runs on a thread of its own
//! here, as on an audio device, and every block is a real-time section
//! under the allocation trap.
//!
//! Spec: vst3-hosting R17, as clap-hosting R17.

use crate::common;
use bl_core::time::SampleRate;
use bl_engine::{NodeConfig, NodeFactory};
use bl_plugin_host::vst3::{Vst3Error, Vst3Host};
use bl_test_vst3::GAIN;
use common::{Engine, config, run, vst3_bundle, vst3_device};

/// A host with the test plug-in, and the engine running a device of it at
/// a gain of 0.5.
fn playing() -> (Vst3Host, Engine) {
    let host = Vst3Host::new();
    host.load(vst3_bundle()).unwrap();
    let node = host
        .factory()
        .create(&vst3_device(1, &[(GAIN, 0.5)]), &config())
        .unwrap();
    let engine = Engine::start(node);
    engine.settles_at(0.5);
    (host, engine)
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

// Spec: offline-plugin-renders R2, R3, R4, R5, R9
#[test]
fn a_lent_plugin_renders_at_the_renders_settings_and_goes_back_as_it_was() {
    let (host, engine) = playing();
    let id = vst3_device(1, &[]).id;
    assert_eq!(host.status(id).unwrap().active_for, Some(config()));
    let state = host.save_state(id).unwrap();

    {
        let loan = host.lend().unwrap();
        assert_eq!(host.lend().unwrap_err(), Vst3Error::Lent);
        // The rendered set has the gain at 0.25.
        let mut node = loan
            .factory()
            .create(&vst3_device(1, &[(GAIN, 0.25)]), &render_config())
            .unwrap();
        assert!(loan.problems().is_empty());
        let status = host.status(id).unwrap();
        assert!(status.active);
        assert_eq!(status.active_for, Some(render_config()));
        // Meanwhile the engine's node passes its input through.
        engine.settles_at(1.0);

        // The render runs it, at its values.
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
fn a_loan_dropped_before_the_renders_node_still_gives_the_plugin_back() {
    let (host, engine) = playing();
    let id = vst3_device(1, &[]).id;
    let loan = host.lend().unwrap();
    let mut node = loan
        .factory()
        .create(&vst3_device(1, &[(GAIN, 0.5)]), &render_config())
        .unwrap();
    run(node.as_mut(), &[1.0; 32], &[], &[]);
    // A render abandoned with its engine still standing: the plug-in goes
    // back once the node lets go of it.
    drop(loan);
    engine.settles_at(1.0);
    drop(node);
    host.idle();
    host.idle();
    engine.settles_at(0.5);
    assert_eq!(host.status(id).unwrap().active_for, Some(config()));
}

// Spec: offline-plugin-renders R4
#[test]
fn a_render_node_that_never_ran_gives_the_plugin_back_too() {
    let (host, engine) = playing();
    let id = vst3_device(1, &[]).id;
    {
        let loan = host.lend().unwrap();
        let node = loan
            .factory()
            .create(&vst3_device(1, &[(GAIN, 0.25)]), &render_config())
            .unwrap();
        // Cancelled before its first block: the component is still in the
        // node's queue when the render's engine goes.
        drop(node);
    }
    engine.settles_at(0.5);
    host.idle();
    assert_eq!(host.status(id).unwrap().active_for, Some(config()));
}

// Spec: offline-plugin-renders R6
#[test]
fn a_plugin_that_fails_while_rendering_goes_silent_there_and_in_the_engine() {
    let (host, engine) = playing();
    let id = vst3_device(1, &[]).id;
    {
        let loan = host.lend().unwrap();
        let mut node = loan
            .factory()
            .create(&vst3_device(1, &[(GAIN, 0.5)]), &render_config())
            .unwrap();
        assert_eq!(run(node.as_mut(), &[1.0; 32], &[], &[]).channels[0][0], 0.5);
        host.load_state(
            id,
            &bl_plugin_host::vst3::plugin_state(bl_test_vst3::FAIL_PROCESS, &[]),
        )
        .unwrap();
        for _ in 0..3 {
            let out = run(node.as_mut(), &[1.0; 32], &[], &[]);
            assert!(out.channels.iter().flatten().all(|s| *s == 0.0));
        }
        drop(node);
    }
    // The engine's node is silent from then on, and the host says why.
    engine.settles_at(0.0);
    host.idle();
    let status = host.status(id).unwrap();
    assert!(!status.active);
    assert!(status.failure.unwrap().contains("offline render"));

    // A later render finds it failed, and is silent too.
    let loan = host.lend().unwrap();
    let mut node = loan
        .factory()
        .create(&vst3_device(1, &[(GAIN, 0.5)]), &render_config())
        .unwrap();
    let out = run(node.as_mut(), &[1.0; 32], &[], &[]);
    assert!(out.channels.iter().flatten().all(|s| *s == 0.0));
}

// Spec: offline-plugin-renders R2
#[test]
fn a_plugin_the_engine_doesnt_hand_back_is_a_problem_for_the_render() {
    let host = Vst3Host::new();
    host.load(vst3_bundle()).unwrap();
    // A node the engine never runs: its plug-in never comes back.
    let _stuck = host
        .factory()
        .create(&vst3_device(1, &[]), &config())
        .unwrap();
    let loan = host.lend().unwrap();
    let mut node = loan
        .factory()
        .create(&vst3_device(1, &[(GAIN, 0.5)]), &render_config())
        .unwrap();
    let problems = loan.problems();
    assert_eq!(problems.len(), 1);
    assert!(problems[0].1.contains("in time"), "{problems:?}");
    let out = run(node.as_mut(), &[1.0; 32], &[], &[]);
    assert!(out.channels[0].iter().all(|s| *s == 1.0), "passes through");
}

// Spec: offline-plugin-renders R3
#[test]
fn a_device_without_a_plugin_yet_gets_one_from_its_saved_state() {
    let host = Vst3Host::new();
    host.load(vst3_bundle()).unwrap();
    let id = vst3_device(1, &[]).id;
    let loan = host.lend().unwrap();
    let mut node = loan
        .factory()
        .create(&vst3_device(1, &[(GAIN, 1.5)]), &render_config())
        .unwrap();
    assert_eq!(run(node.as_mut(), &[1.0; 32], &[], &[]).channels[0][0], 1.5);
    drop(node);
    drop(loan);
    // The host keeps it for the engine, inactive until a node asks.
    let status = host.status(id).unwrap();
    assert!(!status.active);
    assert_eq!(status.failure, None);
}

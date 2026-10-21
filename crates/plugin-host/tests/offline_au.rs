//! Offline renders borrow the engine's Audio Units (WP-4.21; the offline
//! plug-in renders spec, as `offline.rs` has it for CLAP): a unit comes
//! back from the engine's node, runs in the render at the render's own
//! settings, and goes back to the engine active at the engine's settings
//! with the state it had. The engine's node runs on a thread of its own
//! here, as on an audio device, and every block is a real-time section
//! under the allocation trap.
//!
//! Apple's AUDelay is the test unit. Fully wet at its longest delay, it
//! puts out silence for the first two seconds it runs, so a node running it
//! puts out 0 where a node passing its input through puts out 1.
//!
//! Spec: au-hosting R17, as clap-hosting R17.

#![cfg(target_os = "macos")]

bl_rt::install_trap!();

mod common;

use bl_core::id::Id;
use bl_core::time::SampleRate;
use bl_engine::{NodeConfig, NodeFactory};
use bl_model::{Device, DeviceId, DeviceKind, ParamId, PluginFormat};
use bl_plugin_host::au::{AuError, AuHost};
use common::{Engine, config, run};

const MIX: u32 = 0;
const DELAY_TIME: u32 = 1;
const FEEDBACK: u32 = 2;

/// An AUDelay device, fully wet at its longest delay with no feedback.
fn wet(id: u64) -> Device {
    let mut device = Device::new(
        DeviceId(Id::new(id).expect("an ID")),
        "Test AU",
        DeviceKind::Plugin {
            format: PluginFormat::AudioUnit,
            plugin_id: "aufx/dely/appl".to_owned(),
        },
    );
    for (param, value) in [(MIX, 100.0), (DELAY_TIME, 2.0), (FEEDBACK, 0.0)] {
        device.params.insert(ParamId(param), value);
    }
    device
}

/// A host with AUDelay, and the engine running a device of it.
fn playing() -> (AuHost, Engine) {
    let host = AuHost::new();
    host.load("aufx", "dely", "appl").unwrap();
    let node = host.factory().create(&wet(1), &config()).unwrap();
    let engine = Engine::start(node);
    engine.settles_at(0.0);
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
fn a_lent_unit_renders_at_the_renders_settings_and_goes_back_as_it_was() {
    let (host, engine) = playing();
    let id = wet(1).id;
    assert_eq!(host.status(id).unwrap().active_for, Some(config()));
    let state = host.save_state(id).unwrap();

    {
        let loan = host.lend().unwrap();
        assert_eq!(host.lend().unwrap_err(), AuError::Lent);
        let mut node = loan.factory().create(&wet(1), &render_config()).unwrap();
        assert!(loan.problems().is_empty());
        let status = host.status(id).unwrap();
        assert!(status.active);
        assert_eq!(status.active_for, Some(render_config()));
        // Meanwhile the engine's node passes its input through.
        engine.settles_at(1.0);

        // The render runs it: not passed through.
        let out = run(node.as_mut(), &[1.0; 32], &[], &[]);
        assert!(out.channels[0].iter().all(|s| *s == 0.0));
        drop(node);
    }

    // Back in the engine, at the engine's settings, with the state it had.
    engine.settles_at(0.0);
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
fn a_loan_dropped_before_the_renders_node_still_gives_the_unit_back() {
    let (host, engine) = playing();
    let id = wet(1).id;
    let loan = host.lend().unwrap();
    let mut node = loan.factory().create(&wet(1), &render_config()).unwrap();
    run(node.as_mut(), &[1.0; 32], &[], &[]);
    // A render abandoned with its engine still standing: the unit goes back
    // once the node lets go of it.
    drop(loan);
    engine.settles_at(1.0);
    drop(node);
    host.idle();
    host.idle();
    engine.settles_at(0.0);
    assert_eq!(host.status(id).unwrap().active_for, Some(config()));
}

// Spec: offline-plugin-renders R2
#[test]
fn a_unit_the_engine_doesnt_hand_back_is_a_problem_for_the_render() {
    let host = AuHost::new();
    host.load("aufx", "dely", "appl").unwrap();
    // A node the engine never runs: its unit never comes back.
    let _stuck = host.factory().create(&wet(1), &config()).unwrap();
    let loan = host.lend().unwrap();
    let mut node = loan.factory().create(&wet(1), &render_config()).unwrap();
    let problems = loan.problems();
    assert_eq!(problems.len(), 1);
    assert!(problems[0].1.contains("in time"), "{problems:?}");
    let out = run(node.as_mut(), &[1.0; 32], &[], &[]);
    assert!(out.channels[0].iter().all(|s| *s == 1.0), "passes through");
}

// Spec: offline-plugin-renders R3
#[test]
fn a_device_without_a_unit_yet_gets_one_from_its_saved_state() {
    let host = AuHost::new();
    host.load("aufx", "dely", "appl").unwrap();
    let id = wet(1).id;
    let loan = host.lend().unwrap();
    let mut node = loan.factory().create(&wet(1), &render_config()).unwrap();
    let out = run(node.as_mut(), &[1.0; 32], &[], &[]);
    assert!(
        out.channels[0].iter().all(|s| *s == 0.0),
        "not passed through"
    );
    drop(node);
    drop(loan);
    // The host keeps it for the engine, inactive until a node asks.
    let status = host.status(id).unwrap();
    assert!(!status.active);
    assert_eq!(status.failure, None);
}

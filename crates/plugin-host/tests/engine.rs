//! A CLAP plug-in in a track's device chain, through the engine (WP-2.6;
//! the CLAP hosting spec, R2, R3, R15): the engine's parameter changes reach
//! it, a chain edit keeps its instance, and a device deleted and brought
//! back gets the same plug-in. Every block runs under the allocation trap.
//!
//! Spec: clap-hosting R17.

bl_rt::install_trap!();

mod common;

use std::sync::Arc;

use bl_engine::testing::{self, Probe};
use bl_engine::{EngineConfig, Offline};
use bl_model::{Command, Editor, ParamId, TrackKind};
use bl_plugin_host::clap::ClapHost;
use common::{FRAMES, GAIN, RATE, bundle, device};

fn level(offline: &mut Offline) -> f32 {
    let mut last = Vec::new();
    offline.render(FRAMES as u64, |out| last = out.channel(0).to_vec());
    assert!(last.iter().all(|s| *s == last[0]), "{last:?}");
    last[0]
}

// Spec: clap-hosting R3, R15
#[test]
fn a_plugin_in_a_chain_keeps_its_instance_across_edits() {
    let host = ClapHost::new();
    host.load(bundle()).unwrap();
    let mut registry = testing::registry(&Arc::new(Probe::default()));
    registry.add(host.factory());
    let mut offline = Offline::new(EngineConfig::new(RATE, FRAMES), registry);

    let mut editor = Editor::new();
    let track = editor.new_id();
    editor
        .apply(Command::CreateTrack {
            id: track,
            kind: TrackKind::Audio,
            name: String::new(),
            parent: None,
            index: 0,
        })
        .unwrap();
    let dc = editor.new_id();
    let mut source = bl_model::Device::new(
        dc,
        "",
        bl_model::DeviceKind::Native {
            device_type: testing::DC.to_owned(),
        },
    );
    source.params.insert(testing::VALUE, 1.0);
    editor
        .apply(Command::InsertDevice {
            track,
            index: 0,
            device: source,
        })
        .unwrap();
    let id = editor.new_id();
    let mut plugin = device(1, &[(GAIN, 0.5)]);
    plugin.id = id;
    editor
        .apply(Command::InsertDevice {
            track,
            index: 1,
            device: plugin,
        })
        .unwrap();
    assert!(offline.sync(editor.document()).is_empty());
    assert_eq!(level(&mut offline), 0.5, "DC 1 through a gain of 0.5");

    // A parameter change reaches the running plug-in.
    editor
        .apply(Command::SetDeviceParam {
            device: id,
            param: ParamId(GAIN),
            value: 0.25,
        })
        .unwrap();
    offline.sync(editor.document());
    assert_eq!(level(&mut offline), 0.25);

    // A chain edit compiles a new program; the plug-in's node migrates
    // into it, still running.
    editor
        .apply(Command::SetDeviceEnabled {
            device: dc,
            enabled: false,
        })
        .unwrap();
    offline.sync(editor.document());
    assert_eq!(level(&mut offline), 0.0);
    editor.undo().unwrap();
    offline.sync(editor.document());
    assert_eq!(level(&mut offline), 0.25);
    assert!(host.status(id).unwrap().active);

    // Deleted, its node goes and the host deactivates the plug-in;
    // brought back by undo, the same plug-in runs again.
    editor.apply(Command::RemoveDevice { device: id }).unwrap();
    offline.sync(editor.document());
    assert_eq!(level(&mut offline), 1.0);
    level(&mut offline);
    host.idle();
    assert!(!host.status(id).unwrap().active);
    editor.undo().unwrap();
    offline.sync(editor.document());
    assert_eq!(level(&mut offline), 0.25);
    assert!(host.status(id).unwrap().active);
    assert_eq!(offline.heap_operations(), 0);
}

// Spec: mixer-view R40
#[test]
fn a_plugins_gain_adjustment_shows_on_its_tracks_meter() {
    let host = ClapHost::new();
    host.load(bundle()).unwrap();
    let mut registry = testing::registry(&Arc::new(Probe::default()));
    registry.add(host.factory());
    let mut offline = Offline::new(EngineConfig::new(RATE, FRAMES), registry);

    let mut editor = Editor::new();
    let track = editor.new_id();
    editor
        .apply(Command::CreateTrack {
            id: track,
            kind: TrackKind::Audio,
            name: String::new(),
            parent: None,
            index: 0,
        })
        .unwrap();
    let dc = editor.new_id();
    let mut source = bl_model::Device::new(
        dc,
        "",
        bl_model::DeviceKind::Native {
            device_type: testing::DC.to_owned(),
        },
    );
    source.params.insert(testing::VALUE, 1.0);
    editor
        .apply(Command::InsertDevice {
            track,
            index: 0,
            device: source,
        })
        .unwrap();
    let id = editor.new_id();
    let mut plugin = device(1, &[(GAIN, 0.5)]);
    plugin.id = id;
    editor
        .apply(Command::InsertDevice {
            track,
            index: 1,
            device: plugin,
        })
        .unwrap();
    assert!(offline.sync(editor.document()).is_empty());

    // The plug-in's own gain adjustment through CLAP's draft extension:
    // a gain of 0.5 is -6.02 dB, and it shows on the track's meter.
    offline.render((FRAMES * 8) as u64, |_| {});
    let reduction = offline.state().gain_reduction(track).unwrap();
    assert!(
        (reduction + 6.0206).abs() < 0.1,
        "the plug-in's adjustment reads as {reduction} dB"
    );

    // A gain of 1 reduces nothing.
    editor
        .apply(Command::SetDeviceParam {
            device: id,
            param: ParamId(GAIN),
            value: 1.0,
        })
        .unwrap();
    offline.sync(editor.document());
    offline.render((FRAMES * 8) as u64, |_| {});
    assert_eq!(offline.state().gain_reduction(track), Some(0.0));
}

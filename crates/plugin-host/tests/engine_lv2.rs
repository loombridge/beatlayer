//! An LV2 plug-in in a track's device chain, through the engine (WP-5.39;
//! the LV2 hosting spec, R2, R3, R10, R16): the engine's parameter changes
//! reach it, a chain edit keeps its instance, a device deleted and brought
//! back gets the same plug-in, and the latency it reports lines the tracks
//! up. Every block runs under the allocation trap.
//!
//! Spec: lv2-hosting R17.

use crate::common;
use std::sync::Arc;

use bl_engine::testing::{self, Probe};
use bl_engine::{EngineConfig, Offline};
use bl_model::{Command, Editor, ParamId, TrackId, TrackKind};
use bl_plugin_host::lv2::Lv2Host;
use bl_test_lv2::{GAIN, LATENCY};
use common::{FRAMES, RATE, lv2_device, lv2_host};

fn offline(host: &Lv2Host) -> Offline {
    let mut registry = testing::registry(&Arc::new(Probe::default()));
    registry.add(host.factory());
    Offline::new(EngineConfig::new(RATE, FRAMES), registry)
}

fn level(offline: &mut Offline) -> f32 {
    let mut last = Vec::new();
    offline.render(FRAMES as u64, |out| last = out.channel(0).to_vec());
    assert!(last.iter().all(|s| *s == last[0]), "{last:?}");
    last[0]
}

/// An audio track whose chain starts with a DC source of 1.
fn dc_track(editor: &mut Editor) -> (TrackId, bl_model::DeviceId) {
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
    (track, dc)
}

/// Puts an LV2 device with `gain` at the end of `track`'s chain.
fn plugin(editor: &mut Editor, track: TrackId, gain: f64) -> bl_model::DeviceId {
    let id = editor.new_id();
    let mut plugin = lv2_device(1, &[(GAIN, gain)]);
    plugin.id = id;
    let index = editor.document().track(track).unwrap().devices().len();
    editor
        .apply(Command::InsertDevice {
            track,
            index,
            device: plugin,
        })
        .unwrap();
    id
}

// Spec: lv2-hosting R3, R15
#[test]
fn a_plugin_in_a_chain_keeps_its_instance_across_edits() {
    let Some(host) = lv2_host() else { return };
    let mut offline = offline(&host);
    let mut editor = Editor::new();
    let (track, dc) = dc_track(&mut editor);
    let id = plugin(&mut editor, track, 0.5);
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

    // Deleted, its node goes and the instance goes back to the main
    // thread; brought back by undo, a new node gets a plug-in with the state
    // the old one had.
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

// Spec: lv2-hosting R10; delay-compensation R1, R6. The latency the plug-in
// reports after its first block is the node's, and a program compiled with
// it holds the other track back by as much.
#[test]
fn the_latency_a_plugin_reports_lines_the_tracks_up() {
    let Some(host) = lv2_host() else { return };
    let mut editor = Editor::new();
    let (plain, _) = dc_track(&mut editor);
    let (through, _) = dc_track(&mut editor);
    let id = plugin(&mut editor, through, 1.0);
    let _ = plain;

    // The first run learns the latency: the plug-in writes it in its first
    // block.
    {
        let mut first = offline(&host);
        first.sync(editor.document());
        first.render(FRAMES as u64, |_| {});
        host.idle();
        assert_eq!(host.status(id).unwrap().latency, LATENCY);
        assert_eq!(host.take_latency_changes(), [(id, LATENCY)]);
    }

    // A program compiled now (an engine restarted) holds the track that
    // doesn't go through the plug-in back by its latency, so the two line up
    // at the output: the plug-in delays nothing itself, so the sum is its
    // track alone until the held-back one arrives.
    let mut second = offline(&host);
    second.sync(editor.document());
    let mut out = Vec::new();
    second.render(FRAMES as u64, |o| out.extend_from_slice(o.channel(0)));
    let latency = LATENCY as usize;
    assert!(out[..latency].iter().all(|s| *s == 1.0), "{:?}", &out[..latency]);
    assert!(out[latency..].iter().all(|s| *s == 2.0), "{:?}", &out[latency..]);
    assert_eq!(second.heap_operations(), 0);
}

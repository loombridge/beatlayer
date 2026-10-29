//! A device's parameters under automation (WP-2.12): the framework reads the
//! envelope the compiler sends it, on the exact frame, and a hand move takes
//! the parameter back. Each block runs as a real-time section under the
//! allocation trap.

use crate::common;
use bl_core::time::{Frames, TempoMap, Ticks};
use bl_devices::testing::{self, UTILITY_GAIN};
use bl_engine::DeviceType;
use bl_engine::{Collector, Compiler, EngineConfig, NodeData, NodeKey, TransportInfo, Update};
use bl_model::{AutomationTarget, Command, DeviceId, Editor, TrackId, TrackKind};
use common::{RATE, node, run_at};

/// A test utility device with its gain at `gain`, for a node of its own.
fn utility_device(type_: &DeviceType, gain: f64) -> bl_model::Device {
    let mut device = type_.device(DeviceId(bl_core::id::Id::new(1).expect("1 is an ID")));
    device.params.insert(UTILITY_GAIN, gain);
    device
}

/// A track with the test utility on it, its gain at `gain`, and the
/// device's ID.
fn track_with_utility(editor: &mut Editor, type_: &DeviceType, gain: f64) -> (TrackId, DeviceId) {
    let track = editor.new_id();
    editor
        .apply(Command::CreateTrack {
            id: track,
            kind: TrackKind::Midi,
            name: String::new(),
            parent: None,
            index: 0,
        })
        .unwrap();
    let id = editor.new_id();
    let mut device = type_.device(id);
    device.params.insert(UTILITY_GAIN, gain);
    editor
        .apply(Command::InsertDevice {
            track,
            index: 0,
            device,
        })
        .unwrap();
    (track, id)
}

/// A compiler, so the test gets the automation the way a node does: placed
/// under the tempo map and sent as node data. A quarter note at 120 BPM is
/// 24,000 frames.
fn automation(editor: &Editor, device: bl_model::DeviceId) -> (TempoMap, NodeData) {
    let collector = Collector::new();
    let config = EngineConfig::new(RATE, 2048);
    let mut compiler = Compiler::new(config, testing::registry(), collector.handle());
    let update: Update = compiler.sync(editor.document());
    let data = update
        .data
        .into_iter()
        .find(|(key, _)| matches!(key, NodeKey::Device(id) if *id == device))
        .expect("the device's node is sent its automation")
        .1;
    (editor.document().settings().tempo.clone(), data)
}

// Spec: automation R2
#[test]
fn a_devices_parameter_follows_its_envelope_on_the_exact_frame() {
    let type_ = testing::utility_type();
    let mut editor = Editor::new();
    let (track, id) = track_with_utility(&mut editor, &type_, 1.0);
    let target = AutomationTarget::Device {
        device: id,
        param: UTILITY_GAIN,
    };
    // The gain falls from 1 at beat 2 to 0 at beat 3.
    editor
        .apply(Command::SetAutomationBreakpoint {
            track,
            target,
            at: Ticks::QUARTER,
            value: 1.0,
        })
        .unwrap();
    editor
        .apply(Command::SetAutomationBreakpoint {
            track,
            target,
            at: Ticks::QUARTER * 2,
            value: 0.0,
        })
        .unwrap();
    let (tempo, data) = automation(&editor, id);
    let mut utility = node(&testing::registry(), &utility_device(&type_, 1.0), 2048);
    utility.receive_data(data);

    let input = vec![vec![1.0f32; 2048]; 2];
    let run = |utility: &mut dyn bl_engine::Node, position: i64| {
        run_at(
            utility,
            &input,
            &[],
            &tempo,
            TransportInfo {
                playing: true,
                position: Frames(position),
                ..TransportInfo::default()
            },
        )
    };

    // A block from the middle of the ramp: the gain is the frame's own
    // value, to the bit.
    let out = run(utility.as_mut(), 36_000);
    for (i, &sample) in out[0].iter().enumerate() {
        let frame = 36_000 + i as i64;
        let want = (1.0 - (frame - 24_000) as f64 / 24_000.0) as f32;
        assert_eq!(sample, want, "frame {frame}");
    }

    // A block before the envelope's first point and one after its last hold
    // the nearest point's value; the ramp reaches its target on its exact
    // frame.
    assert!(run(utility.as_mut(), 0)[0].iter().all(|&s| s == 1.0));
    assert!(run(utility.as_mut(), 48_000)[0].iter().all(|&s| s == 0.0));
    assert!(run(utility.as_mut(), 60_000)[0].iter().all(|&s| s == 0.0));

    // While the song is stopped, the device holds the values it has.
    let stopped = run_at(
        utility.as_mut(),
        &input,
        &[],
        &tempo,
        TransportInfo::default(),
    );
    assert!(stopped[0].iter().all(|&s| s == 0.0));
}

// Spec: transport R24
#[test]
fn a_devices_envelope_is_read_at_the_song_frames_a_faster_song_plays() {
    let type_ = testing::utility_type();
    let mut editor = Editor::new();
    let (track, id) = track_with_utility(&mut editor, &type_, 1.0);
    let target = AutomationTarget::Device {
        device: id,
        param: UTILITY_GAIN,
    };
    // The gain falls from 1 at beat 2 to 0 at beat 3.
    for (at, value) in [(Ticks::QUARTER, 1.0), (Ticks::QUARTER * 2, 0.0)] {
        editor
            .apply(Command::SetAutomationBreakpoint {
                track,
                target,
                at,
                value,
            })
            .unwrap();
    }
    let (tempo, data) = automation(&editor, id);
    let mut utility = node(&testing::registry(), &utility_device(&type_, 1.0), 2048);
    utility.receive_data(data);

    // At twice the map's speed, frame i of a block from song frame 36,000
    // plays song frame 36,000 + 2i, and takes that frame's value.
    let input = vec![vec![1.0f32; 2048]; 2];
    let out = run_at(
        utility.as_mut(),
        &input,
        &[],
        &tempo,
        TransportInfo {
            playing: true,
            position: Frames(36_000),
            speed: 2.0,
            ..TransportInfo::default()
        },
    );
    for (i, &sample) in out[0].iter().enumerate() {
        let frame = 36_000 + 2 * i as i64;
        let want = (1.0 - (frame - 24_000) as f64 / 24_000.0) as f32;
        assert_eq!(sample, want, "frame {i}");
    }
}

// Spec: automation R3
#[test]
fn a_hand_move_takes_a_devices_parameter_back_from_its_envelope() {
    let type_ = testing::utility_type();
    let mut editor = Editor::new();
    let (track, id) = track_with_utility(&mut editor, &type_, 1.0);
    let target = AutomationTarget::Device {
        device: id,
        param: UTILITY_GAIN,
    };
    editor
        .apply(Command::SetAutomationBreakpoint {
            track,
            target,
            at: Ticks::ZERO,
            value: 0.5,
        })
        .unwrap();
    // Moving the parameter by hand overrides its envelope, so the compiler
    // sends the device no automation at all: the parameter follows its
    // control, which the node is made with.
    editor
        .apply(Command::SetDeviceParam {
            device: id,
            param: UTILITY_GAIN,
            value: 0.25,
        })
        .unwrap();
    let collector = Collector::new();
    let mut compiler = Compiler::new(
        EngineConfig::new(RATE, 2048),
        testing::registry(),
        collector.handle(),
    );
    let update: Update = compiler.sync(editor.document());
    assert!(
        !update
            .data
            .iter()
            .any(|(key, _)| matches!(key, NodeKey::Device(device) if *device == id)),
        "no envelope drives it"
    );
    let tempo = editor.document().settings().tempo.clone();
    let mut utility = node(&testing::registry(), &utility_device(&type_, 0.25), 2048);
    let input = vec![vec![1.0f32; 64]; 2];
    let transport = TransportInfo {
        playing: true,
        position: Frames(0),
        ..TransportInfo::default()
    };
    let out = run_at(utility.as_mut(), &input, &[], &tempo, transport);
    assert!(
        out[0].iter().all(|&s| s == 0.25),
        "the control drives it, not the envelope"
    );

    // Re-Enable Automation sends the envelope again, and the parameter
    // follows it from where the playhead is.
    editor.apply(Command::ReEnableAutomation {}).unwrap();
    let (tempo, data) = automation(&editor, id);
    utility.receive_data(data);
    let out = run_at(utility.as_mut(), &input, &[], &tempo, transport);
    assert!(
        out[0].iter().all(|&s| s == 0.5),
        "the envelope drives it again"
    );
}

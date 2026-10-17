//! A device's A and B states on the audio side (WP-5.12; the device compare
//! spec, R3 and R13): a switch reaches the running node as parameter
//! changes that glide, never as a new program, and a copy sends nothing.
//! Each block runs as a real-time section under the allocation trap.

bl_rt::install_trap!();

mod common;

use bl_devices::testing::{self, UTILITY_GAIN};
use bl_engine::{Collector, Compiler, EngineConfig, NodeEvent, NodeKey, ParamValue, Update};
use bl_model::{Command, DeviceId, Editor, TrackId, TrackKind};
use common::{RATE, node, run};

/// 20 ms at 48 kHz.
const GLIDE: usize = 960;

/// A set with the test utility on a track, its A state's Gain at 0.25 and
/// its B state's at the default 1, and a compiler that has sent the program
/// for it.
fn rig() -> (Editor, Compiler, Collector, DeviceId) {
    let mut editor = Editor::new();
    let track: TrackId = editor.new_id();
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
    editor
        .apply(Command::InsertDevice {
            track,
            index: 0,
            device: testing::utility_type().device(id),
        })
        .unwrap();
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
    assert!(compiler.sync(editor.document()).program.is_some());
    (editor, compiler, collector, id)
}

/// The events the update sends the device's node, at the start of a block.
fn events_for(update: &Update, device: DeviceId) -> Vec<NodeEvent> {
    update
        .params
        .iter()
        .filter(|event| event.key == NodeKey::Device(device))
        .map(|event| NodeEvent {
            offset: 0,
            param: event.param,
            value: event.value,
        })
        .collect()
}

// Spec: device-compare R3
#[test]
fn a_state_switch_glides_its_continuous_parameters() {
    let (mut editor, mut compiler, _collector, device) = rig();
    let mut utility = node(
        &testing::registry(),
        editor.document().device(device).unwrap(),
        2048,
    );
    assert_eq!(run(utility.as_mut(), &[1.0; 64], &[])[0][63], 0.25);

    editor.apply(Command::CompareSwitch { device }).unwrap();
    let update = compiler.sync(editor.document());
    let events = events_for(&update, device);
    assert_eq!(
        events.iter().map(|e| e.value).collect::<Vec<_>>(),
        [ParamValue::Default],
        "only the parameter whose value differs moves, to B's default"
    );
    let out = run(utility.as_mut(), &[1.0; 2048], &events);
    for channel in &out {
        assert!(
            (channel[0] - (0.25 + 0.75 / GLIDE as f32)).abs() < 1e-6,
            "the glide starts from A's value: {}",
            channel[0]
        );
        assert!(
            channel[..GLIDE].windows(2).all(|w| w[1] > w[0]),
            "a straight glide"
        );
        assert!(
            channel[GLIDE - 1..].iter().all(|s| *s == 1.0),
            "B's value once it lands"
        );
    }
}

// Spec: device-compare R13
#[test]
fn a_state_switch_sends_no_program() {
    let (mut editor, mut compiler, _collector, device) = rig();
    let mut utility = node(
        &testing::registry(),
        editor.document().device(device).unwrap(),
        2048,
    );
    for command in [
        Command::CompareSwitch { device },
        Command::CompareCopy { device },
        Command::CompareSwitch { device },
        Command::CompareSwitch { device },
    ] {
        editor.apply(command.clone()).unwrap();
        let update = compiler.sync(editor.document());
        assert!(update.program.is_none(), "{command:?} sent a program");
        run(utility.as_mut(), &[1.0; 512], &events_for(&update, device));
    }
}

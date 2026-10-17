//! VST3 hosting (WP-2.7; the VST3 hosting spec): the test plug-in, built
//! from source, loads by path as a device whose parameters the controller
//! gives, passes its input through until it's active, processes with each
//! parameter change on its sample offset and each note on its frame, saves
//! and loads its state, reports a latency change and restarts for it, and
//! reports the changes its controller makes itself. Every block runs as a
//! real-time section under the allocation trap.
//!
//! Spec: vst3-hosting R1 to R12, which mirrors clap-hosting's.

bl_rt::install_trap!();

mod common;

use bl_core::time::{Frames, Ticks};
use bl_engine::{AutomationList, Node, NodeData, NodeFactory, NoteEvent, NoteKind, TransportInfo};
use bl_model::modulation::Travel;
use bl_model::{DeviceKind, DeviceRole, ParamId, ParamKind, PluginFormat, PluginState};
use bl_plugin_host::vst3::{ParamChange, Vst3Error, Vst3Host, plugin_state};
use common::{
    CHANNELS, FRAMES, config, modulation_for, run, run_at, set, vst3_bundle, vst3_device,
};

fn note(offset: u32, kind: NoteKind, key: u8, velocity: f32) -> NoteEvent {
    NoteEvent {
        offset,
        kind,
        channel: 0,
        key,
        velocity,
        note_id: None,
    }
}

/// A host with the test plug-in loaded, and a running node for a device of
/// it with `params`.
fn running(params: &[(u32, f64)]) -> (Vst3Host, Box<dyn Node>) {
    let host = Vst3Host::new();
    host.load(vst3_bundle()).expect("the test plug-in loads");
    let node = host
        .factory()
        .create(&vst3_device(1, params), &config())
        .expect("a node for a VST3 device");
    (host, node)
}

// Spec: vst3-hosting R1, R5
#[test]
fn a_plugin_loads_by_path_with_its_parameters() {
    let host = Vst3Host::new();
    let types = host.load(vst3_bundle()).expect("the test plug-in loads");
    assert_eq!(types.len(), 1);
    let found = &types[0].device_type;
    assert_eq!(
        found.kind,
        DeviceKind::Plugin {
            format: PluginFormat::Vst3,
            plugin_id: bl_test_vst3::plugin_id()
        }
    );
    assert_eq!(found.name, bl_test_vst3::PLUGIN_NAME);
    assert_eq!(found.role, DeviceRole::AudioEffect);
    let params = &found.description.params;
    let names: Vec<_> = params.iter().map(|p| (p.id, p.name.as_str())).collect();
    assert_eq!(
        names,
        [
            (ParamId(bl_test_vst3::GAIN), "Gain"),
            (ParamId(bl_test_vst3::LATENCY), "Latency"),
            (ParamId(bl_test_vst3::LAST_NOTE), "Last Note")
        ]
    );
    assert!(params.iter().all(|p| p.automatable));
    assert!(matches!(
        params[0].kind,
        ParamKind::Continuous {
            min: 0.0,
            max: 2.0,
            ..
        }
    ));
    assert_eq!(params[0].default, 1.0);
    assert!(matches!(
        params[1].kind,
        ParamKind::Integer {
            min: 0,
            max: 4096,
            ..
        }
    ));
    assert!(found.description.check().is_ok());
    assert_eq!(host.factory().device_types(), vec![found.clone()]);

    let missing = host.load(&vst3_bundle().with_extension("missing"));
    assert!(matches!(missing, Err(Vst3Error::Load(_))), "{missing:?}");
}

// Spec: vst3-hosting R2
#[test]
fn a_node_passes_its_input_through_until_its_plugin_arrives() {
    let host = Vst3Host::new();
    host.load(vst3_bundle()).unwrap();
    // Made off the main thread, as the compiler thread makes it: the main
    // thread's idle gives it its plug-in.
    let factory = host.factory();
    let mut node = std::thread::scope(|s| {
        s.spawn(|| factory.create(&vst3_device(1, &[(bl_test_vst3::GAIN, 0.25)]), &config()))
            .join()
            .unwrap()
    })
    .expect("a node");
    let before = run(node.as_mut(), &[1.0; FRAMES], &[], &[]);
    assert!(before.channels[0].iter().all(|s| *s == 1.0));
    assert!(host.idle());
    let after = run(node.as_mut(), &[1.0; FRAMES], &[], &[]);
    assert!(
        after.channels[0].iter().all(|s| *s == 0.25),
        "{:?}",
        after.channels[0]
    );
}

// Spec: vst3-hosting R3
#[test]
fn audio_runs_through_the_plugin_and_a_change_lands_on_its_frame() {
    let (host, mut node) = running(&[(bl_test_vst3::GAIN, 0.5)]);
    let out = run(
        node.as_mut(),
        &[1.0; FRAMES],
        &[set(10, bl_test_vst3::GAIN, 2.0)],
        &[],
    );
    assert_eq!(out.channels.len(), CHANNELS);
    for channel in &out.channels {
        assert!(channel[..10].iter().all(|s| *s == 0.5), "{channel:?}");
        assert!(channel[10..].iter().all(|s| *s == 2.0), "{channel:?}");
    }
    let status = host.status(vst3_device(1, &[]).id).expect("a status");
    assert!(status.active);
    assert_eq!(status.failure, None);

    // The change reached the controller too, through the main thread: its
    // parameters are the device's (R5).
    host.idle();
    let status = host.status(vst3_device(1, &[]).id).unwrap();
    assert!(
        status.values.contains(&(ParamId(bl_test_vst3::GAIN), 2.0)),
        "{:?}",
        status.values
    );
}

// Spec: vst3-hosting R4
#[test]
fn notes_reach_the_plugin_and_come_back_out_on_their_frames() {
    let (_host, mut node) = running(&[(bl_test_vst3::GAIN, 0.0)]);
    let notes = [
        note(5, NoteKind::On, 60, 0.5),
        note(40, NoteKind::Off, 60, 0.0),
    ];
    let out = run(node.as_mut(), &[1.0; FRAMES], &[], &notes);
    for channel in &out.channels {
        // The plug-in adds the note's velocity on its frame.
        for (i, s) in channel.iter().enumerate() {
            assert_eq!(*s, if i == 5 { 0.5 } else { 0.0 }, "frame {i}");
        }
    }
    assert_eq!(
        out.notes,
        [
            note(5, NoteKind::On, 72, 0.5),
            note(40, NoteKind::Off, 72, 0.0)
        ],
        "echoed an octave up on the same frames"
    );
}

// Spec: vst3-hosting R8. A switched-off device's changes wait for the
// plug-in's next block, at its first frame.
#[test]
fn a_switched_off_plugin_keeps_changes_for_its_next_block() {
    let (host, mut node) = running(&[]);
    run(node.as_mut(), &[1.0; FRAMES], &[], &[]);
    node.flush(&[set(0, bl_test_vst3::GAIN, 0.75)]);
    let status = host.status(vst3_device(1, &[]).id).unwrap();
    assert!(status.active);
    let out = run(node.as_mut(), &[1.0; FRAMES], &[], &[]);
    assert!(out.channels[0].iter().all(|s| *s == 0.75));
}

// Spec: vst3-hosting R8, R9, R11 (as clap-hosting's R8, R9, R11). Both the
// component's and the controller's state save and load, and a state the
// plug-in refuses changes nothing.
#[test]
fn state_saves_and_loads_into_a_new_instance() {
    let (host, mut node) = running(&[]);
    let id = vst3_device(1, &[]).id;
    // A parameter change reaches the component; the controller hears it
    // from the main thread.
    run(
        node.as_mut(),
        &[1.0; FRAMES],
        &[set(0, bl_test_vst3::GAIN, 1.5)],
        &[],
    );
    // Its controller makes a gesture on Last Note, as its editor would.
    host.load_state(id, &plugin_state(&[], &bl_test_vst3::gesture_state(48, 64)))
        .expect("the gesture state loads");
    host.idle();
    let saved = host.save_state(id).expect("it saves");
    assert_eq!(saved.path, vst3_bundle().to_string_lossy());
    assert_eq!(&saved.data[..4], b"BLTV");

    // A set reopened: a new host, which knows nothing loaded, finds the
    // plug-in by the saved path and gives it its state.
    let reopened = Vst3Host::new();
    let mut restored = vst3_device(1, &[]);
    restored.plugin = Some(saved.clone());
    let mut node = reopened.factory().create(&restored, &config()).unwrap();
    let status = reopened.status(restored.id).unwrap();
    assert!(
        status.values.contains(&(ParamId(bl_test_vst3::GAIN), 1.5)),
        "{:?}",
        status.values
    );
    assert!(
        status
            .values
            .contains(&(ParamId(bl_test_vst3::LAST_NOTE), 64.0)),
        "{:?}",
        status.values
    );
    let out = run(node.as_mut(), &[1.0; FRAMES], &[], &[]);
    assert!(out.channels[0].iter().all(|s| *s == 1.5));
    assert_eq!(reopened.save_state(restored.id).unwrap(), saved);

    // A state it refuses changes nothing.
    let refused = reopened.load_state(restored.id, b"not a state");
    assert!(matches!(refused, Err(Vst3Error::Refused(_))), "{refused:?}");
    assert_eq!(reopened.save_state(restored.id).unwrap(), saved);
}

// Spec: vst3-hosting R10 (as clap-hosting's R10). A plug-in that asks for a
// restart is deactivated and activated again by the main thread, and its
// new latency is reported; the node passes its input through for the blocks
// between. The engine's delay compensation follows the report (WP-2.11,
// which the engine's own test proves).
#[test]
fn a_latency_change_restarts_the_plugin_and_is_reported() {
    let (host, mut node) = running(&[]);
    let id = vst3_device(1, &[]).id;
    run(node.as_mut(), &[1.0; FRAMES], &[], &[]);
    assert_eq!(node.latency(), 0);
    assert!(host.take_latency_changes().is_empty());

    // The latency goes up, as a person's change in the plug-in's editor:
    // the component takes the value, the controller hears it and asks the
    // host for a restart.
    run(
        node.as_mut(),
        &[1.0; FRAMES],
        &[set(0, bl_test_vst3::LATENCY, 256.0)],
        &[],
    );
    host.idle();
    run(node.as_mut(), &[1.0; FRAMES], &[], &[]);
    host.idle();
    let between = run(node.as_mut(), &[1.0; FRAMES], &[], &[]);
    assert!(
        between.channels[0].iter().all(|s| *s == 1.0),
        "passes through meanwhile"
    );
    host.idle();
    assert_eq!(node.latency(), 256);
    assert_eq!(host.status(id).unwrap().latency, 256);
    assert_eq!(host.take_latency_changes(), [(id, 256)]);
    assert!(host.status(id).unwrap().active);
    // Its delay line starts empty at the new latency: a constant input
    // comes out after 256 frames, as the plug-in's own delay says.
    let filling = run(node.as_mut(), &[1.0; FRAMES], &[], &[]);
    assert!(filling.channels[0].iter().all(|s| *s == 0.0));
    for _ in 0..3 {
        run(node.as_mut(), &[1.0; FRAMES], &[], &[]);
    }
    let after = run(node.as_mut(), &[1.0; FRAMES], &[], &[]);
    assert!(after.channels[0].iter().all(|s| *s == 1.0));
}

// Spec: vst3-hosting R6 (as clap-hosting's R6). A change the controller
// makes itself, inside a gesture, is reported once, with its final value,
// in plain units.
#[test]
fn a_gesture_from_the_controllers_editor_makes_one_change() {
    let (host, _node) = running(&[]);
    let id = vst3_device(1, &[]).id;
    assert!(host.take_param_changes().is_empty());
    host.load_state(id, &plugin_state(&[], &bl_test_vst3::gesture_state(48, 64)))
        .unwrap();
    assert!(
        host.take_param_changes().is_empty(),
        "nothing until the main thread runs"
    );
    host.idle();
    assert_eq!(
        host.take_param_changes(),
        [ParamChange {
            device: id,
            param: ParamId(bl_test_vst3::LAST_NOTE),
            value: 64.0
        }]
    );
    assert!(
        host.take_param_changes().is_empty(),
        "once, not once a frame"
    );
    let status = host.status(id).unwrap();
    assert!(
        status
            .values
            .contains(&(ParamId(bl_test_vst3::LAST_NOTE), 64.0)),
        "{:?}",
        status.values
    );
}

// Spec: vst3-hosting R12 (as clap-hosting's R12)
#[test]
fn a_plugin_that_fails_to_process_goes_silent_and_says_so() {
    let (host, mut node) = running(&[]);
    let id = vst3_device(1, &[]).id;
    host.load_state(id, &plugin_state(bl_test_vst3::FAIL_PROCESS, &[]))
        .unwrap();
    let out = run(node.as_mut(), &[1.0; FRAMES], &[], &[]);
    assert!(out.channels.iter().flatten().all(|s| *s == 0.0));
    // It's never called again, and the node keeps running.
    let again = run(
        node.as_mut(),
        &[1.0; FRAMES],
        &[set(3, bl_test_vst3::GAIN, 1.0)],
        &[],
    );
    assert!(again.channels.iter().flatten().all(|s| *s == 0.0));
    host.idle();
    let status = host.status(id).unwrap();
    assert!(status.failure.unwrap().contains("failed while processing"));
}

// Spec: vst3-hosting R12. One that refuses to activate leaves its device
// passing its input through.
#[test]
fn a_plugin_that_refuses_to_activate_passes_its_input_through() {
    let host = Vst3Host::new();
    host.load(vst3_bundle()).unwrap();
    let mut device = vst3_device(1, &[]);
    device.plugin = Some(PluginState {
        path: vst3_bundle().to_string_lossy().into_owned(),
        data: plugin_state(bl_test_vst3::FAIL_ACTIVATE, &[]),
    });
    let mut node = host.factory().create(&device, &config()).unwrap();
    let out = run(node.as_mut(), &[1.0; FRAMES], &[], &[]);
    assert!(out.channels.iter().flatten().all(|s| *s == 1.0));
    let status = host.status(device.id).unwrap();
    assert!(!status.active);
    assert!(
        status
            .failure
            .unwrap()
            .contains("refused to start processing")
    );
}

// Spec: vst3-hosting R1, R12. A device whose plug-in can't be found passes
// its input through, says so, and keeps its saved state: nothing can be
// saved over it.
#[test]
fn a_plugin_that_cant_be_found_leaves_its_device_passing_audio_through() {
    let host = Vst3Host::new();
    let mut lost = vst3_device(1, &[]);
    lost.plugin = Some(PluginState {
        path: "/nowhere/Gone.vst3".to_owned(),
        data: b"kept".to_vec(),
    });
    let mut node = host.factory().create(&lost, &config()).unwrap();
    let out = run(node.as_mut(), &[1.0; FRAMES], &[], &[]);
    assert!(out.channels.iter().flatten().all(|s| *s == 1.0));
    let status = host.status(lost.id).unwrap();
    assert!(!status.active);
    assert!(status.failure.is_some(), "{status:?}");
    assert!(matches!(
        host.save_state(lost.id),
        Err(Vst3Error::NoInstance(_))
    ));
    assert_eq!(lost.plugin.as_ref().unwrap().data, b"kept");
}

// Spec: vst3-hosting R9
#[test]
fn main_thread_calls_from_another_thread_are_refused() {
    let host = Vst3Host::new();
    std::thread::scope(|s| {
        s.spawn(|| {
            assert_eq!(host.load(vst3_bundle()), Err(Vst3Error::WrongThread));
        });
    });
}

/// Channel 0 of `blocks` blocks of `FRAMES / blocks` frames of 1.0 through
/// a node for a Test Gain Echo with a gain of 0.5 and the clip envelope
/// `data`, from song frame `from`.
fn gain_render(data: &NodeData, from: i64, blocks: usize) -> Vec<f32> {
    let (_host, mut node) = running(&[(bl_test_vst3::GAIN, 0.5)]);
    node.receive_data(data.clone());
    let each = FRAMES / blocks;
    (0..blocks)
        .flat_map(|i| {
            let transport = TransportInfo {
                playing: true,
                position: Frames(from + (i * each) as i64),
            };
            run_at(node.as_mut(), &vec![1.0; each], &[], &[], transport).channels[0].clone()
        })
        .collect()
}

// Spec: modulation R6, R10
#[test]
fn a_vst3_parameter_takes_the_played_value() {
    // A clip's envelope climbs from nothing to the whole travel over the
    // bar.
    let data = modulation_for(
        vst3_device(1, &[]),
        bl_test_vst3::GAIN,
        &[(Ticks::ZERO, 0.0), (Ticks::WHOLE, 1.0)],
    );
    let list = data.downcast_ref::<AutomationList>().expect("a list");
    let host = Vst3Host::new();
    let types = host.load(vst3_bundle()).unwrap();
    let gain = types[0]
        .device_type
        .description
        .param(ParamId(bl_test_vst3::GAIN))
        .unwrap();
    let id = ParamId(bl_test_vst3::GAIN);
    let played = |frame: i64| list.played(
            Travel::Param(gain),
            id,
            Some(frame),
            0.5,
            bl_engine::modbus::Bus::EMPTY,
            0,
        );

    // Every VST3 parameter is given the value it plays, as a parameter
    // change on the block's first frame and on each song frame that is a
    // multiple of 32.
    let from = 1000;
    let out = gain_render(&data, from, 1);
    for (i, sample) in out.iter().enumerate() {
        let frame = from + i as i64;
        // Before the first grid point the first frame's value holds.
        let want = played(if frame < 1024 { from } else { frame / 32 * 32 });
        assert_eq!(*sample, want as f32, "frame {frame}");
    }
    assert_ne!(played(from), 0.5, "the clip modulates the gain");

    // A render is the same at any block size.
    for blocks in [2, 4, 64] {
        assert_eq!(gain_render(&data, from, blocks), out, "{blocks} blocks");
    }

    // The song stops: the plug-in has its control's value again.
    let (_host, mut node) = running(&[(bl_test_vst3::GAIN, 0.5)]);
    node.receive_data(data);
    let transport = TransportInfo {
        playing: true,
        position: Frames(from),
    };
    run_at(node.as_mut(), &[1.0; FRAMES], &[], &[], transport);
    let stopped = run(node.as_mut(), &[1.0; FRAMES], &[], &[]);
    assert!(stopped.channels[0].iter().all(|s| *s == 0.5));
}

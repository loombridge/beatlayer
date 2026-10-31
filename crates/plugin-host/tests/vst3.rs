//! VST3 hosting (WP-2.7; the VST3 hosting spec): the test plug-in, built
//! from source, loads by path as a device whose parameters the controller
//! gives, passes its input through until it's active, processes with each
//! parameter change on its sample offset and each note on its frame, saves
//! and loads its state, reports a latency change and restarts for it, and
//! reports the changes its controller makes itself. Every block runs as a
//! real-time section under the allocation trap.
//!
//! Spec: vst3-hosting R1 to R12, which mirrors clap-hosting's.

use crate::common;
use bl_core::time::{Frames, Ticks};
use bl_engine::{AutomationList, Node, NodeData, NodeFactory, NoteEvent, NoteKind, TransportInfo};
use bl_model::modulation::Travel;
use bl_model::{DeviceKind, DeviceRole, ParamId, ParamKind, PluginFormat, PluginState};
use bl_plugin_host::vst3::{ParamChange, Vst3Error, Vst3Host, plugin_state};
use common::{
    CHANNELS, FRAMES, config, modulation_for, run, run_at, set, vst3_bundle, vst3_device,
    vst3_device_with_role,
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
            (ParamId(bl_test_vst3::LAST_NOTE), "Last Note"),
            (ParamId(bl_test_vst3::MODE), "Mode"),
            (ParamId(bl_test_vst3::TRIGGER), "Trigger")
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

// Spec: vst3-hosting R1, R4; IMP-24
#[test]
fn a_vst3_plugin_with_an_event_output_bus_has_a_note_output() {
    let host = Vst3Host::new();
    let types = host.load(vst3_bundle()).expect("the test plug-in loads");
    let found = &types[0].device_type;
    assert_eq!(found.role, DeviceRole::AudioEffect);
    assert!(
        found.note_output,
        "its event output bus lets it sit as a MIDI effect"
    );
}

// Spec: vst3-hosting R4; IMP-24
#[test]
fn a_vst3_plugin_placed_as_a_midi_effect_passes_notes_on_and_drops_its_audio() {
    let host = Vst3Host::new();
    host.load(vst3_bundle()).expect("the test plug-in loads");
    let node = host
        .factory()
        .create(
            &vst3_device_with_role(1, &[(bl_test_vst3::GAIN, 0.5)], DeviceRole::MidiEffect),
            &config(),
        )
        .expect("a node for a VST3 device");
    let mut node = node;
    let notes = [
        note(5, NoteKind::On, 60, 0.5),
        note(40, NoteKind::Off, 60, 0.0),
    ];
    let out = run(node.as_mut(), &[1.0; FRAMES], &[], &notes);
    for channel in &out.channels {
        // The audio it receives passes through: its own output is dropped.
        assert!(channel.iter().all(|s| *s == 1.0), "{channel:?}");
    }
    assert_eq!(
        out.notes,
        [
            note(5, NoteKind::On, 72, 0.5),
            note(40, NoteKind::Off, 72, 0.0)
        ],
        "its notes go on, an octave up on their frames"
    );
}

// Spec: vst3-hosting R18. The test plug-in's controller maps control change 1
// on channel 1 and the pitch bend on channel 3 to its gain: a message sets it
// on the message's frame; one the controller doesn't map does nothing.
#[test]
fn channel_messages_set_the_parameters_the_controller_maps_them_to() {
    let (_host, mut node) = running(&[(bl_test_vst3::GAIN, 0.5)]);
    let message = |offset, channel, kind| NoteEvent {
        offset,
        kind,
        channel,
        key: 0,
        velocity: 0.0,
        note_id: None,
    };
    let notes = [
        // Gain 2 (the top of its range) from frame 10.
        message(
            10,
            0,
            NoteKind::Control {
                controller: 1,
                value: 1.0,
            },
        ),
        // Not mapped: another channel, and another controller.
        message(
            15,
            1,
            NoteKind::Control {
                controller: 1,
                value: 0.0,
            },
        ),
        message(
            16,
            0,
            NoteKind::Control {
                controller: 7,
                value: 0.0,
            },
        ),
        // The wheel at its bottom: gain 0 from frame 30.
        message(30, 2, NoteKind::PitchBend { value: -1.0 }),
        // A bend at its centre: half of 2.
        message(50, 2, NoteKind::PitchBend { value: 0.0 }),
    ];
    let out = run(node.as_mut(), &[1.0; FRAMES], &[], &notes);
    let channel = &out.channels[0];
    assert!(channel[..10].iter().all(|s| *s == 0.5), "{channel:?}");
    assert!(channel[10..30].iter().all(|s| *s == 2.0), "{channel:?}");
    assert!(channel[30..50].iter().all(|s| *s == 0.0), "{channel:?}");
    // 8192 of 16383 is just short of the middle.
    let centre = 2.0 * 8192.0 / 16383.0;
    assert!(
        channel[50..]
            .iter()
            .all(|s| (f64::from(*s) - centre).abs() < 1e-6),
        "{channel:?}"
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

// Spec: vst3-hosting R9. A state whose controller part the plug-in refuses
// changes nothing, though its component took its own part.
#[test]
fn a_state_whose_controller_part_is_refused_changes_nothing() {
    let (host, _node) = running(&[]);
    let id = vst3_device(1, &[]).id;
    let before = host.save_state(id).unwrap();
    let mut component = bl_test_vst3::COMPONENT_MAGIC.to_vec();
    component.extend_from_slice(&0.5f64.to_le_bytes());
    component.extend_from_slice(&0u32.to_le_bytes());
    let bad = plugin_state(&component, b"junk controller");
    let refused = host.load_state(id, &bad);
    assert!(matches!(refused, Err(Vst3Error::Refused(_))), "{refused:?}");
    assert_eq!(host.save_state(id).unwrap(), before);
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

// Spec: vst3-hosting R6. A plug-in that changes its values itself and asks
// the host to read them again, as one that loads a preset in its own
// window, has each changed value come into the set once.
#[test]
fn a_values_restart_brings_the_changed_values_into_the_set() {
    let (host, _node) = running(&[(bl_test_vst3::GAIN, 0.5)]);
    let id = vst3_device(1, &[]).id;
    host.idle();
    assert!(host.take_param_changes().is_empty());
    host.load_state(id, &plugin_state(&[], &bl_test_vst3::quiet_gain_state(1.5)))
        .unwrap();
    host.idle();
    assert_eq!(
        host.take_param_changes(),
        [ParamChange {
            device: id,
            param: ParamId(bl_test_vst3::GAIN),
            value: 1.5
        }]
    );
    host.idle();
    assert!(host.take_param_changes().is_empty(), "once");
}

// Spec: vst3-hosting R10. A restart that asks only for the values and
// titles to be read again keeps the plug-in running: no block passes its
// input through.
#[test]
fn a_param_values_restart_ask_keeps_the_plugin_running() {
    let (host, mut node) = running(&[(bl_test_vst3::GAIN, 0.5)]);
    let id = vst3_device(1, &[]).id;
    let input = [1.0; FRAMES];
    let reference = run(node.as_mut(), &input, &[], &[]).channels;
    assert!(reference[0].iter().all(|s| *s == 0.5));
    let mut blocks = vec![
        run(
            node.as_mut(),
            &input,
            &[set(0, bl_test_vst3::TRIGGER, 1.0)],
            &[],
        )
        .channels,
    ];
    for _ in 0..4 {
        host.idle();
        assert!(host.status(id).unwrap().active);
        blocks.push(run(node.as_mut(), &input, &[], &[]).channels);
    }
    for block in blocks {
        assert_eq!(block, reference);
    }
}

// Spec: vst3-hosting R5, R6. A stepped parameter whose plain range is its
// normalized one is its step index: every step can be set, and the editor's
// gesture comes back as a step.
#[test]
fn an_sdk_stepped_parameter_with_a_unit_range_is_a_step_index() {
    let host = Vst3Host::new();
    let types = host.load(vst3_bundle()).unwrap();
    let mode = types[0]
        .device_type
        .description
        .params
        .iter()
        .find(|p| p.id == ParamId(bl_test_vst3::MODE))
        .expect("the mode parameter");
    assert_eq!(
        mode.kind,
        ParamKind::Integer {
            min: 0,
            max: 3,
            unit: bl_model::ParamUnit::None
        }
    );
    let id = vst3_device(1, &[]).id;
    let mut node = host
        .factory()
        .create(&vst3_device(1, &[(bl_test_vst3::MODE, 2.0)]), &config())
        .unwrap();
    let _ = run(node.as_mut(), &[1.0; FRAMES], &[], &[]);
    host.idle();
    let values = host.status(id).unwrap().values;
    assert!(
        values.contains(&(ParamId(bl_test_vst3::MODE), 2.0)),
        "{values:?}"
    );

    // The editor's switch moves to step 1.
    host.load_state(id, &plugin_state(&[], &bl_test_vst3::mode_gesture_state(1)))
        .unwrap();
    host.idle();
    assert_eq!(
        host.take_param_changes(),
        [ParamChange {
            device: id,
            param: ParamId(bl_test_vst3::MODE),
            value: 1.0
        }]
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
                ..TransportInfo::default()
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
    let played = |frame: i64| {
        list.played(
            Travel::Param(gain),
            id,
            Some(frame),
            0.5,
            bl_engine::modbus::Bus::EMPTY,
            0,
        )
    };

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
        ..TransportInfo::default()
    };
    run_at(node.as_mut(), &[1.0; FRAMES], &[], &[], transport);
    let stopped = run(node.as_mut(), &[1.0; FRAMES], &[], &[]);
    assert!(stopped.channels[0].iter().all(|s| *s == 0.5));
}

/// A fresh file for the test plug-in's log, which it reads from the
/// environment when it is first used: each test runs in a process of its own.
fn plugin_log(name: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("bl-t304-{name}-{}.log", std::process::id()));
    let _ = std::fs::remove_file(&path);
    // SAFETY: nothing reads the environment concurrently, and the plug-in
    // reads this when it first logs.
    unsafe { std::env::set_var(bl_test_vst3::LOG_VAR, &path) };
    path
}

fn logged(path: &std::path::Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

// Spec: vst3-hosting R2
#[test]
fn a_component_a_dropped_node_never_took_is_activated_for_the_next_node() {
    let host = Vst3Host::new();
    host.load(vst3_bundle()).unwrap();
    let factory = host.factory();
    let device = vst3_device(1, &[(bl_test_vst3::GAIN, 0.5)]);
    // The first node is activated, then dropped before it runs a block:
    // the component sits in its queue.
    let first = factory.create(&device, &config()).expect("a node");
    assert!(host.status(device.id).unwrap().active);
    drop(first);
    let mut second = factory.create(&device, &config()).expect("a node");
    let out = run(second.as_mut(), &[1.0; FRAMES], &[], &[]);
    assert!(
        out.channels[0].iter().all(|s| *s == 0.5),
        "{:?}",
        out.channels[0]
    );

    // And the device can still be let go of: its plug-in is torn down.
    drop(second);
    host.retain(&Default::default());
    host.idle();
    assert!(host.status(device.id).is_none());
}

// Spec: vst3-hosting R2. A plug-in whose node is gone isn't activated again,
// and it is deactivated before it is terminated.
#[test]
fn a_plugin_whose_node_is_dropped_is_not_activated_again() {
    let log = plugin_log("dropped-node");
    let (host, mut node) = running(&[(bl_test_vst3::GAIN, 0.5)]);
    let _ = run(node.as_mut(), &[1.0; FRAMES], &[], &[]);
    let activations = |lines: &[String]| {
        lines
            .iter()
            .filter(|l| l.starts_with("call=set-active ") && l.ends_with("value=1"))
            .count()
    };
    let before = activations(&logged(&log));
    assert!(before >= 1);
    // As the drop thread does when the device is deleted.
    drop(node);
    for _ in 0..10 {
        host.idle();
    }
    let lines = logged(&log);
    assert_eq!(
        activations(&lines),
        before,
        "activated again for a node that is gone"
    );
    assert!(
        lines
            .iter()
            .rfind(|l| l.starts_with("call=set-active "))
            .is_some_and(|l| l.ends_with("value=0")),
        "{lines:?}"
    );
    host.clear();
    assert!(
        logged(&log)
            .iter()
            .rfind(|l| l.starts_with("call=set-active "))
            .is_some_and(|l| l.ends_with("value=0"))
    );
}

// Spec: vst3-hosting R3
#[test]
fn a_value_reaches_the_plugin_as_its_controller_maps_it() {
    // SAFETY: the plug-in reads this when its parameters are described.
    unsafe { std::env::set_var(bl_test_vst3::SKEWED_VAR, "1") };
    // The gain is a square of its normalized value: 0.5 is a normalized 0.5
    // here, but a linear map would send 0.25.
    let (host, mut node) = running(&[(bl_test_vst3::GAIN, 0.5)]);
    let out = run(node.as_mut(), &[1.0; FRAMES], &[], &[]);
    assert!(
        out.channels[0].iter().all(|s| *s == 0.5),
        "{:?}",
        out.channels[0]
    );

    // So does an automated change, on its frame.
    let out = run(
        node.as_mut(),
        &[1.0; FRAMES],
        &[set(10, bl_test_vst3::GAIN, 1.28)],
        &[],
    );
    assert!(out.channels[0][..10].iter().all(|s| *s == 0.5));
    for sample in &out.channels[0][10..] {
        assert!((sample - 1.28).abs() < 1e-3, "{sample}");
    }

    // And the controller is told the same value, not a linear one.
    host.idle();
    let status = host.status(vst3_device(1, &[]).id).unwrap();
    let (_, gain) = status
        .values
        .iter()
        .find(|(id, _)| *id == ParamId(bl_test_vst3::GAIN))
        .unwrap();
    assert!((gain - 1.28).abs() < 1e-3, "{:?}", status.values);
}

// Spec: vst3-hosting R6
#[test]
fn the_component_and_the_controller_are_connected_both_ways_and_disconnected_both_ways() {
    let log = plugin_log("connections");
    let (host, node) = running(&[]);
    let lines = logged(&log);
    for call in ["connect:component", "connect:controller"] {
        assert!(
            lines
                .iter()
                .any(|l| l.starts_with(&format!("call={call} "))),
            "{lines:?}"
        );
    }
    drop(node);
    host.retain(&Default::default());
    host.idle();
    let lines = logged(&log);
    for call in ["disconnect:component", "disconnect:controller"] {
        assert!(
            lines
                .iter()
                .any(|l| l.starts_with(&format!("call={call} "))),
            "{lines:?}"
        );
    }
}

/// How many channels the test plug-in's input bus `bus` was given in the
/// blocks it processed, and whether the host activated it.
fn bus_lines(lines: &[String], bus: usize) -> (Vec<String>, Vec<String>) {
    let channels = |what: &str| {
        lines
            .iter()
            .filter(|l| l.contains(&format!("{what}:in{bus} ")))
            .map(|l| l.split("value=").nth(1).unwrap().to_owned())
            .collect()
    };
    (channels("bus"), channels("bus-active"))
}

// Spec: vst3-hosting R2
#[test]
fn every_bus_is_given_the_channels_it_has_or_is_left_inactive() {
    let log = plugin_log("aux-buses");
    // SAFETY: the plug-in reads this when it describes its buses.
    unsafe { std::env::set_var(bl_test_vst3::BUSES_VAR, "aux") };
    let (_host, mut node) = running(&[]);
    run(node.as_mut(), &[1.0; FRAMES], &[], &[]);
    let lines = logged(&log);
    // The main bus and three auxiliary ones fill the eight channels the
    // node feeds, each with all its channels. The fourth doesn't fit, so
    // it isn't activated, rather than active and fed none.
    for bus in 0..4 {
        assert_eq!(
            bus_lines(&lines, bus),
            (vec!["2".to_owned()], vec!["1".to_owned()]),
            "bus {bus}"
        );
    }
    assert_eq!(
        bus_lines(&lines, 4),
        (vec!["0".to_owned()], vec!["0".to_owned()]),
        "{lines:?}"
    );
}

// Spec: vst3-hosting R12
#[test]
fn a_plugin_whose_main_bus_has_more_channels_than_the_node_feeds_is_refused() {
    let log = plugin_log("wide-bus");
    // SAFETY: as above.
    unsafe { std::env::set_var(bl_test_vst3::BUSES_VAR, "wide") };
    let (host, mut node) = running(&[(bl_test_vst3::GAIN, 0.5)]);
    let out = run(node.as_mut(), &[1.0; FRAMES], &[], &[]);
    // It never gets a block with a short bus: its device passes its input
    // through and says why.
    assert!(out.channels[0].iter().all(|s| *s == 1.0));
    let status = host.status(vst3_device(1, &[]).id).unwrap();
    assert!(!status.active);
    assert!(
        status
            .failure
            .is_some_and(|why| why.contains("12 channels")),
        "refusal reported"
    );
    assert!(!logged(&log).iter().any(|l| l.contains("bus:in")));
}

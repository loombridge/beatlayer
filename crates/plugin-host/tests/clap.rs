//! CLAP hosting (WP-2.6; the CLAP hosting spec): the test plug-in, built
//! from source, loads by path, processes audio and notes on their frames,
//! takes and sends parameter changes, keeps its state, reports its
//! latency, opens its editor, and fails without taking the engine with it.
//! Every block runs as a real-time section under the allocation trap.
//!
//! Spec: clap-hosting R17.

bl_rt::install_trap!();

mod common;

use bl_engine::{Node, NodeFactory, NoteEvent, NoteKind};
use bl_model::{DeviceKind, DeviceRole, ParamId, ParamKind, PluginFormat};
use bl_plugin_host::clap::{ClapError, ClapHost, ParamChange};
use common::{
    CHANNELS, EDITOR_HEIGHT, EDITOR_SCALE, EDITOR_WIDTH, FRAMES, GAIN, LAST_NOTE, LATENCY,
    PLUGIN_ID, PLUGIN_NAME, bundle, config, device, run, set,
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
fn running(params: &[(u32, f64)]) -> (ClapHost, Box<dyn Node>) {
    let host = ClapHost::new();
    host.load(bundle()).expect("the test plug-in loads");
    let node = host
        .factory()
        .create(&device(1, params), &config())
        .expect("a node for a CLAP device");
    (host, node)
}

// Spec: clap-hosting R1, R5
#[test]
fn a_plugin_loads_by_path_with_its_parameters() {
    let host = ClapHost::new();
    let types = host.load(bundle()).expect("the test plug-in loads");
    assert_eq!(types.len(), 1);
    let found = &types[0].device_type;
    assert_eq!(
        found.kind,
        DeviceKind::Plugin {
            format: PluginFormat::Clap,
            plugin_id: PLUGIN_ID.to_owned()
        }
    );
    assert_eq!(found.name, PLUGIN_NAME);
    assert_eq!(found.role, DeviceRole::AudioEffect);
    let params = &found.description.params;
    let names: Vec<_> = params.iter().map(|p| (p.id, p.name.as_str())).collect();
    assert_eq!(
        names,
        [
            (ParamId(GAIN), "Gain"),
            (ParamId(LATENCY), "Latency"),
            (ParamId(LAST_NOTE), "Last Note"),
            (ParamId(EDITOR_SCALE), "Editor Scale"),
            (ParamId(EDITOR_WIDTH), "Editor Width"),
            (ParamId(EDITOR_HEIGHT), "Editor Height")
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

    let missing = host.load(&bundle().with_extension("missing"));
    assert!(matches!(missing, Err(ClapError::Load(_))), "{missing:?}");
}

// Spec: clap-hosting R2, R3
#[test]
fn audio_runs_through_the_plugin_and_a_change_lands_on_its_frame() {
    let (host, mut node) = running(&[(GAIN, 0.5)]);
    let out = run(node.as_mut(), &[1.0; FRAMES], &[set(10, GAIN, 2.0)], &[]);
    assert_eq!(out.channels.len(), CHANNELS);
    for channel in &out.channels {
        assert!(channel[..10].iter().all(|s| *s == 0.5), "{channel:?}");
        assert!(channel[10..].iter().all(|s| *s == 2.0), "{channel:?}");
    }
    let status = host.status(device(1, &[]).id).expect("a status");
    assert!(status.active);
    assert_eq!(status.failure, None);
}

// Spec: clap-hosting R2
#[test]
fn a_node_passes_its_input_through_until_its_plugin_arrives() {
    let host = ClapHost::new();
    host.load(bundle()).unwrap();
    // Made off the main thread, as the compiler thread makes it: the main
    // thread's idle gives it its plug-in.
    let factory = host.factory();
    let mut node = std::thread::scope(|s| {
        s.spawn(|| factory.create(&device(1, &[(GAIN, 0.25)]), &config()))
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

// Spec: clap-hosting R4
#[test]
fn notes_reach_the_plugin_and_come_back_out_on_their_frames() {
    let (host, mut node) = running(&[(GAIN, 0.0)]);
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

    // Spec: clap-hosting R6. It set its own Last Note, as an editor would,
    // in one gesture.
    host.idle();
    assert_eq!(
        host.take_param_changes(),
        [ParamChange {
            device: device(1, &[]).id,
            param: ParamId(LAST_NOTE),
            value: 60.0
        }]
    );
    assert!(host.take_param_changes().is_empty());
}

// Spec: clap-hosting R7
#[test]
fn a_switched_off_plugin_takes_changes_through_flush() {
    let (host, mut node) = running(&[]);
    run(node.as_mut(), &[1.0; FRAMES], &[], &[]);
    node.flush(&[set(0, GAIN, 0.75)]);
    let status = host.status(device(1, &[]).id).unwrap();
    assert!(
        status.values.contains(&(ParamId(GAIN), 0.75)),
        "{:?}",
        status.values
    );
    let out = run(node.as_mut(), &[1.0; FRAMES], &[], &[]);
    assert!(out.channels[0].iter().all(|s| *s == 0.75));
}

// Spec: clap-hosting R8, R9
#[test]
fn state_saves_and_loads_into_a_new_instance() {
    let (host, mut node) = running(&[]);
    run(
        node.as_mut(),
        &[1.0; FRAMES],
        &[set(0, GAIN, 1.5)],
        &[note(0, NoteKind::On, 64, 1.0)],
    );
    let saved = host.save_state(device(1, &[]).id).expect("it saves");
    assert_eq!(saved.path, bundle().to_string_lossy());
    assert_eq!(&saved.data[..4], b"BLTC");

    // A set reopened: a new host, which knows nothing loaded, finds the
    // plug-in by the saved path and gives it its state.
    let reopened = ClapHost::new();
    let mut restored = device(1, &[]);
    restored.plugin = Some(saved.clone());
    let mut node = reopened.factory().create(&restored, &config()).unwrap();
    let status = reopened.status(restored.id).unwrap();
    assert!(
        status.values.contains(&(ParamId(GAIN), 1.5)),
        "{:?}",
        status.values
    );
    assert!(status.values.contains(&(ParamId(LAST_NOTE), 64.0)));
    let out = run(node.as_mut(), &[1.0; FRAMES], &[], &[]);
    assert!(out.channels[0].iter().all(|s| *s == 1.5));
    assert_eq!(reopened.save_state(restored.id).unwrap(), saved);

    // A state it refuses changes nothing.
    let refused = reopened.load_state(restored.id, b"not a state");
    assert!(matches!(refused, Err(ClapError::Refused(_))), "{refused:?}");
    assert_eq!(reopened.save_state(restored.id).unwrap(), saved);
}

// Spec: clap-hosting R10
#[test]
fn a_latency_change_restarts_the_plugin_and_is_reported() {
    let (host, mut node) = running(&[]);
    run(node.as_mut(), &[1.0; FRAMES], &[], &[]);
    assert_eq!(node.latency(), 0);
    // The plug-in asks for a restart; the main thread asks for its
    // processor back, deactivates it, and activates it again.
    run(
        node.as_mut(),
        &[1.0; FRAMES],
        &[set(0, LATENCY, 256.0)],
        &[],
    );
    host.idle();
    let between = run(node.as_mut(), &[1.0; FRAMES], &[], &[]);
    assert!(
        between.channels[0].iter().all(|s| *s == 1.0),
        "passes through meanwhile"
    );
    host.idle();
    assert_eq!(node.latency(), 256);
    assert_eq!(host.status(device(1, &[]).id).unwrap().latency, 256);
    let after = run(node.as_mut(), &[1.0; FRAMES], &[], &[]);
    assert!(after.channels[0].iter().all(|s| *s == 1.0));
    assert!(host.status(device(1, &[]).id).unwrap().active);
}

// Spec: clap-hosting R12
#[test]
fn a_plugin_that_fails_to_process_goes_silent_and_says_so() {
    let (host, mut node) = running(&[]);
    host.load_state(device(1, &[]).id, bl_test_clap::FAIL_PROCESS)
        .unwrap();
    let out = run(node.as_mut(), &[1.0; FRAMES], &[], &[]);
    assert!(out.channels.iter().flatten().all(|s| *s == 0.0));
    // It's never called again, and the node keeps running.
    let again = run(node.as_mut(), &[1.0; FRAMES], &[set(3, GAIN, 1.0)], &[]);
    assert!(again.channels.iter().flatten().all(|s| *s == 0.0));
    host.idle();
    let status = host.status(device(1, &[]).id).unwrap();
    assert!(status.failure.unwrap().contains("failed while processing"));
}

// Spec: clap-hosting R12
#[test]
fn a_plugin_that_refuses_to_activate_passes_its_input_through() {
    let host = ClapHost::new();
    host.load(bundle()).unwrap();
    let saved = {
        let mut device = device(1, &[]);
        device.plugin = Some(bl_model::PluginState {
            path: bundle().to_string_lossy().into_owned(),
            data: bl_test_clap::FAIL_ACTIVATE.to_vec(),
        });
        device
    };
    let mut node = host.factory().create(&saved, &config()).unwrap();
    let out = run(node.as_mut(), &[1.0; FRAMES], &[], &[]);
    assert!(out.channels.iter().flatten().all(|s| *s == 1.0));
    let status = host.status(saved.id).unwrap();
    assert!(!status.active);
    assert!(status.failure.unwrap().contains("refused to activate"));
}

// Spec: clap-hosting R12
#[test]
fn a_plugin_that_cant_be_found_leaves_its_device_passing_audio_through() {
    let host = ClapHost::new();
    let mut lost = device(1, &[]);
    lost.plugin = Some(bl_model::PluginState {
        path: "/nowhere/Gone.clap".to_owned(),
        data: b"kept".to_vec(),
    });
    let mut node = host.factory().create(&lost, &config()).unwrap();
    let out = run(node.as_mut(), &[1.0; FRAMES], &[], &[]);
    assert!(out.channels.iter().flatten().all(|s| *s == 1.0));
    let status = host.status(lost.id).unwrap();
    assert!(status.failure.is_some());
    assert!(matches!(
        host.save_state(lost.id),
        Err(ClapError::NoInstance(_))
    ));
}

// Spec: clap-hosting R13
#[test]
fn the_editor_opens_and_closes_in_its_own_window() {
    let (host, _node) = running(&[]);
    let id = device(1, &[]).id;
    assert!(host.status(id).unwrap().has_editor);
    host.open_editor(id, "Test Gain Echo/1-Audio")
        .expect("it opens");
    assert!(host.status(id).unwrap().editor_open);
    host.open_editor(id, "again")
        .expect("opening it twice is fine");
    host.close_editor(id).unwrap();
    assert!(!host.status(id).unwrap().editor_open);
}

// Spec: device-chains R31, R33. Hiding is not closing: the editor stays
// open until it's closed, and a hidden editor shows again.
#[test]
fn the_editor_hides_and_shows_without_closing() {
    let (host, _node) = running(&[]);
    let id = device(1, &[]).id;
    assert!(host.open_editors().is_empty());
    host.open_editor(id, "Test Gain Echo/1-Audio")
        .expect("it opens");
    assert_eq!(host.open_editors(), [id]);

    host.set_editor_visible(id, false).unwrap();
    let hidden = host.status(id).unwrap();
    assert!(hidden.editor_open, "hiding is not closing");
    assert!(hidden.editor_hidden);
    // Hiding it again is a no-op.
    host.set_editor_visible(id, false).unwrap();

    host.set_editor_visible(id, true).unwrap();
    let shown = host.status(id).unwrap();
    assert!(shown.editor_open && !shown.editor_hidden);
    // Showing it again is a no-op.
    host.set_editor_visible(id, true).unwrap();

    // A closed editor isn't hidden, and showing it does nothing.
    host.close_editor(id).unwrap();
    assert!(!host.status(id).unwrap().editor_open);
    assert!(host.open_editors().is_empty());
    host.set_editor_visible(id, true).unwrap();
    assert!(!host.status(id).unwrap().editor_open);

    // A device without a running plug-in is refused.
    assert!(matches!(
        host.set_editor_visible(device(9, &[]).id, false),
        Err(ClapError::NoInstance(_))
    ));
}

// Spec: device-chains R31, R33. Asked to hide, and the plug-in refuses,
// it's asked once, not on every frame: the host remembers the visibility
// it asked for, and a changed wanted visibility still reaches the
// plug-in.
#[test]
fn a_plugin_that_refuses_to_hide_is_asked_once() {
    let (host, _node) = running(&[]);
    let id = device(1, &[]).id;
    host.open_editor(id, "Test Gain Echo/1-Audio")
        .expect("it opens");
    // Its next show or hide refuses, and it recovers: a second ask would
    // have hidden, so an editor that stays shown was asked once.
    host.load_state(id, bl_test_clap::FAIL_GUI).unwrap();

    let refused = host.set_editor_visible(id, false);
    assert!(matches!(refused, Err(ClapError::Refused(_))), "{refused:?}");
    assert!(
        !host.status(id).unwrap().editor_hidden,
        "it refused to hide"
    );

    // Asked again, as the shell asks on every frame: refused, without
    // asking the plug-in a second time.
    for _ in 0..60 {
        let again = host.set_editor_visible(id, false);
        assert!(matches!(again, Err(ClapError::Refused(_))), "{again:?}");
    }
    assert!(
        !host.status(id).unwrap().editor_hidden,
        "asked once; a second ask would have hidden"
    );

    // A changed wanted visibility still reaches the plug-in, which has
    // recovered by now.
    host.set_editor_visible(id, true).unwrap();
    host.set_editor_visible(id, false).unwrap();
    assert!(
        host.status(id).unwrap().editor_hidden,
        "asked again, it hides"
    );
}

// Spec: clap-hosting R14
#[test]
fn main_thread_calls_from_another_thread_are_refused() {
    let host = ClapHost::new();
    std::thread::scope(|s| {
        s.spawn(|| {
            assert_eq!(host.load(bundle()), Err(ClapError::WrongThread));
        });
    });
}

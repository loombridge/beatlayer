//! VST3 hosting, part 1 (WP-2.7; the VST3 hosting spec): the test plug-in,
//! built from source, loads by path as a device whose parameters the
//! controller gives, passes its input through until it's active, and then
//! processes with each parameter change on its sample offset and each note
//! on its frame. Every block runs as a real-time section under the
//! allocation trap.
//!
//! Spec: vst3-hosting R1 to R8.

bl_rt::install_trap!();

mod common;

use bl_engine::{Node, NodeFactory, NoteEvent, NoteKind};
use bl_model::{DeviceKind, DeviceRole, ParamId, ParamKind, PluginFormat};
use bl_plugin_host::vst3::{Vst3Error, Vst3Host};
use common::{CHANNELS, FRAMES, config, run, set, vst3_bundle, vst3_device};

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
    assert_eq!(names, [(ParamId(bl_test_vst3::GAIN), "Gain")]);
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

// Spec: vst3-hosting R1. A device whose plug-in can't be found passes its
// input through and says so.
#[test]
fn a_plugin_that_cant_be_found_leaves_its_device_passing_audio_through() {
    let host = Vst3Host::new();
    let mut lost = vst3_device(1, &[]);
    lost.plugin = Some(bl_model::PluginState {
        path: "/nowhere/Gone.vst3".to_owned(),
        data: Vec::new(),
    });
    let mut node = host.factory().create(&lost, &config()).unwrap();
    let out = run(node.as_mut(), &[1.0; FRAMES], &[], &[]);
    assert!(out.channels.iter().flatten().all(|s| *s == 1.0));
    let status = host.status(lost.id).unwrap();
    assert!(!status.active);
    assert!(status.failure.is_some(), "{status:?}");
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

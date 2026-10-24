//! LV2 hosting (WP-5.39; the LV2 hosting spec): the test plug-in, built from
//! source, loads by bundle as a device whose parameters its control ports
//! give, passes its input through until it's active, processes with each
//! parameter change on its frame and each note on its frame, saves and loads
//! its state, reports its latency, and is described from its metadata. Every
//! block runs as a real-time section under the allocation trap. They run
//! where lilv is: on Linux always, elsewhere when `BL_TEST_LILV` names a
//! lilv library.
//!
//! Spec: lv2-hosting R1 to R12, R14.

use crate::common;
use bl_core::time::{Frames, Ticks};
use bl_engine::{AutomationList, Node, NodeData, NodeFactory, NoteEvent, NoteKind, TransportInfo};
use bl_model::modulation::Travel;
use bl_model::param::Taper;
use bl_model::{DeviceKind, DeviceRole, ParamId, ParamKind, ParamUnit, PluginFormat, PluginState};
use bl_plugin_host::lv2::{Lv2Error, Lv2Host};
use bl_test_lv2::{
    BEAT_NOTE, CUTOFF, ECHO_TRANSPOSE, GAIN, LATENCY, MODE, PLUGIN_NAME, PLUGIN_URI, STEPS, TOGGLE,
};
use common::{CHANNELS, FRAMES, config, lv2_device, lv2_host, modulation_for, run, run_at, set};

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
fn running(params: &[(u32, f64)]) -> Option<(Lv2Host, Box<dyn Node>)> {
    let host = lv2_host()?;
    let node = host
        .factory()
        .create(&lv2_device(1, params), &config())
        .expect("a node for an LV2 device");
    Some((host, node))
}

/// The velocity a note of `velocity` comes back with: MIDI has seven bits.
fn midi_velocity(velocity: f32) -> f32 {
    (velocity * 127.0).round() / 127.0
}

// Spec: lv2-hosting R1, R5
#[test]
fn a_plugin_loads_by_bundle_with_its_parameters() {
    let Some(host) = lv2_host() else { return };
    let types = host.types();
    assert_eq!(types.len(), 1);
    let found = &types[0].device_type;
    assert_eq!(
        found.kind,
        DeviceKind::Plugin {
            format: PluginFormat::Lv2,
            plugin_id: PLUGIN_URI.to_owned()
        }
    );
    assert_eq!(found.name, PLUGIN_NAME);
    assert_eq!(found.role, DeviceRole::AudioEffect);
    assert!(
        found.note_output,
        "its MIDI output lets it sit as a MIDI effect"
    );
    let params = &found.description.params;
    let names: Vec<_> = params.iter().map(|p| (p.id, p.name.as_str())).collect();
    assert_eq!(
        names,
        [
            (ParamId(GAIN), "Gain"),
            (ParamId(TOGGLE), "Toggle"),
            (ParamId(MODE), "Mode"),
            (ParamId(CUTOFF), "Cutoff"),
            (ParamId(STEPS), "Steps"),
        ],
        "its control inputs, not its outputs"
    );
    assert!(params.iter().all(|p| p.automatable));
    assert!(matches!(
        params[0].kind,
        ParamKind::Continuous {
            min: 0.0,
            max: 2.0,
            taper: Taper::Linear,
            ..
        }
    ));
    assert_eq!(params[0].default, 1.0);
    assert_eq!(params[1].kind, ParamKind::Toggle);
    assert_eq!(
        params[2].kind,
        ParamKind::Choice {
            choices: vec!["Clean".into(), "Warm".into(), "Hot".into()]
        }
    );
    assert_eq!(params[2].default, 1.0);
    assert!(matches!(
        params[3].kind,
        ParamKind::Continuous {
            min: 20.0,
            max: 20_000.0,
            unit: ParamUnit::Hertz,
            taper: Taper::Logarithmic,
            ..
        }
    ));
    assert_eq!(params[3].default, 1000.0);
    assert!(matches!(
        params[4].kind,
        ParamKind::Integer { min: 1, max: 8, .. }
    ));
    assert_eq!(params[4].default, 4.0);
    assert!(found.description.check().is_ok());
    assert_eq!(host.factory().device_types(), vec![found.clone()]);

    let missing = host.load(&bl_test_lv2::bundle().with_extension("missing"));
    assert!(matches!(missing, Err(Lv2Error::Load(_))), "{missing:?}");
    let empty = host.load(&std::env::temp_dir());
    assert!(matches!(empty, Err(Lv2Error::Load(_))), "{empty:?}");
}

// Spec: lv2-hosting R2
#[test]
fn a_node_passes_its_input_through_until_its_plugin_arrives() {
    let Some(host) = lv2_host() else { return };
    // Made off the main thread, as the compiler thread makes it: the main
    // thread's idle gives it its plug-in.
    let factory = host.factory();
    let mut node = std::thread::scope(|s| {
        s.spawn(|| factory.create(&lv2_device(1, &[(GAIN, 0.25)]), &config()))
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

// Spec: lv2-hosting R3
#[test]
fn audio_runs_through_the_plugin_and_a_change_lands_on_its_frame() {
    let Some((host, mut node)) = running(&[(GAIN, 0.5)]) else {
        return;
    };
    let out = run(node.as_mut(), &[1.0; FRAMES], &[set(10, GAIN, 2.0)], &[]);
    assert_eq!(out.channels.len(), CHANNELS);
    for channel in &out.channels {
        assert!(channel[..10].iter().all(|s| *s == 0.5), "{channel:?}");
        assert!(channel[10..].iter().all(|s| *s == 2.0), "{channel:?}");
    }
    let status = host.status(lv2_device(1, &[]).id).expect("a status");
    assert!(status.active);
    assert_eq!(status.failure, None);
    assert!(
        status.values.contains(&(ParamId(GAIN), 2.0)),
        "{:?}",
        status.values
    );

    // Several changes in a block split it into parts, each at its value.
    let out = run(
        node.as_mut(),
        &[1.0; FRAMES],
        &[set(7, GAIN, 1.0), set(33, GAIN, 0.25), set(34, GAIN, 0.75)],
        &[],
    );
    let channel = &out.channels[0];
    assert!(channel[..7].iter().all(|s| *s == 2.0), "{channel:?}");
    assert!(channel[7..33].iter().all(|s| *s == 1.0), "{channel:?}");
    assert_eq!(channel[33], 0.25);
    assert!(channel[34..].iter().all(|s| *s == 0.75), "{channel:?}");
}

// Spec: lv2-hosting R2, R3, R4, R18. Blocks of 1, 100, 512 and 4,096 frames,
// each a real-time section under the allocation trap: the gain on every
// frame, a change on its frame, and a note on its frame.
#[test]
fn blocks_of_every_size_are_run_with_changes_and_notes_on_their_frames() {
    let Some(host) = lv2_host() else { return };
    let config = bl_engine::NodeConfig {
        max_frames: 4096,
        ..config()
    };
    let mut node = host
        .factory()
        .create(&lv2_device(1, &[(GAIN, 0.5)]), &config)
        .unwrap();
    for frames in [1usize, 100, 512, 4096] {
        let change = (frames / 2) as u32;
        let key = 40 + (frames % 40) as u8;
        let out = run(
            node.as_mut(),
            &vec![1.0; frames],
            &[set(change, GAIN, 0.25)],
            &[note(change, NoteKind::On, key, 1.0)],
        );
        for channel in &out.channels {
            assert!(
                channel[..change as usize].iter().all(|s| *s == 0.5),
                "{frames} frames"
            );
            assert!(
                channel[change as usize..].iter().all(|s| *s == 0.25),
                "{frames} frames"
            );
        }
        assert_eq!(
            out.notes,
            [note(change, NoteKind::On, key + ECHO_TRANSPOSE, 1.0)],
            "{frames} frames"
        );
        // Put the gain back for the next size.
        run(node.as_mut(), &[0.0; 1], &[set(0, GAIN, 0.5)], &[]);
    }
}

// Spec: lv2-hosting R4
#[test]
fn notes_reach_the_plugin_and_come_back_out_on_their_frames() {
    let Some((_host, mut node)) = running(&[]) else {
        return;
    };
    let notes = [
        note(5, NoteKind::On, 60, 0.5),
        note(40, NoteKind::Off, 60, 0.0),
    ];
    let out = run(node.as_mut(), &[0.0; FRAMES], &[], &notes);
    assert_eq!(
        out.notes,
        [
            note(5, NoteKind::On, 60 + ECHO_TRANSPOSE, midi_velocity(0.5)),
            note(40, NoteKind::Off, 60 + ECHO_TRANSPOSE, 0.0)
        ],
        "echoed an octave up on the same frames"
    );
    // A note that parts the block lands on its own frame in its part.
    let out = run(
        node.as_mut(),
        &[0.0; FRAMES],
        &[set(20, GAIN, 0.5)],
        &[note(30, NoteKind::On, 64, 1.0)],
    );
    assert_eq!(out.notes, [note(30, NoteKind::On, 76, 1.0)]);
}

// Spec: lv2-hosting R1, R4; IMP-24
#[test]
fn a_plugin_placed_as_a_midi_effect_passes_notes_on_and_drops_its_audio() {
    let Some(host) = lv2_host() else { return };
    let mut node = host
        .factory()
        .create(
            &lv2_device(1, &[(GAIN, 0.5)]).with_role(DeviceRole::MidiEffect),
            &config(),
        )
        .expect("a node for an LV2 device");
    let notes = [
        note(5, NoteKind::On, 60, 0.5),
        note(40, NoteKind::Off, 60, 0.0),
    ];
    let out = run(node.as_mut(), &[1.0; FRAMES], &[], &notes);
    for channel in &out.channels {
        // The audio it receives passes through: its own output is dropped.
        assert!(channel.iter().all(|s| *s == 1.0), "{channel:?}");
    }
    assert_eq!(out.notes.len(), 2, "its notes go on");
}

// Spec: lv2-hosting R4. A channel message reaches the plug-in as MIDI;
// the plug-in's echo of it is not a note, so it isn't sent on.
#[test]
fn channel_messages_reach_the_plugin_as_midi_but_are_not_sent_on() {
    let Some((_host, mut node)) = running(&[]) else {
        return;
    };
    let message = NoteEvent {
        offset: 3,
        kind: NoteKind::Control {
            controller: 1,
            value: 1.0,
        },
        channel: 0,
        key: 0,
        velocity: 0.0,
        note_id: None,
    };
    let out = run(node.as_mut(), &[0.0; FRAMES], &[], &[message]);
    assert!(out.notes.is_empty());
}

// Spec: lv2-hosting R3, R4. The song's position reaches a plug-in whose
// Atom port takes `time:Position`: while it plays the test plug-in sends a
// note for the whole beat.
#[test]
fn the_transport_reaches_the_plugin_as_a_position() {
    let Some((_host, mut node)) = running(&[]) else {
        return;
    };
    let bar = 96_000 / 2;
    let at = |frames: i64, playing| TransportInfo {
        playing,
        position: Frames(frames),
    };
    // 120 BPM at 48 kHz: a beat is 24,000 frames. Two and a half beats in.
    let out = run_at(
        node.as_mut(),
        &[0.0; FRAMES],
        &[],
        &[],
        at(2 * 24_000 + 12_000, true),
    );
    assert_eq!(out.notes.len(), 1, "{:?}", out.notes);
    assert_eq!(out.notes[0].key, BEAT_NOTE + 2);
    assert_eq!(out.notes[0].offset, 0);
    // The second bar's beat 1.
    let out = run_at(
        node.as_mut(),
        &[0.0; FRAMES],
        &[],
        &[],
        at(i64::from(bar * 2) + 24_000, true),
    );
    assert_eq!(out.notes[0].key, BEAT_NOTE + 1);
    // Stopped, it sends none.
    let out = run_at(node.as_mut(), &[0.0; FRAMES], &[], &[], at(0, false));
    assert!(out.notes.is_empty());
}

// Spec: lv2-hosting R7. A switched-off device's changes wait for the
// plug-in's next block, at its first frame.
#[test]
fn a_switched_off_plugin_keeps_changes_for_its_next_block() {
    let Some((host, mut node)) = running(&[]) else {
        return;
    };
    run(node.as_mut(), &[1.0; FRAMES], &[], &[]);
    node.flush(&[set(0, GAIN, 0.75)]);
    let status = host.status(lv2_device(1, &[]).id).unwrap();
    assert!(status.active);
    let out = run(node.as_mut(), &[1.0; FRAMES], &[], &[]);
    assert!(out.channels[0].iter().all(|s| *s == 0.75));
}

// Spec: lv2-hosting R8, R11 (as clap-hosting's R8, R9, R11). The state
// saves with the plug-in's own, a reopened set gives it back, and a state the
// plug-in refuses changes nothing.
#[test]
fn state_saves_and_loads_into_a_new_instance() {
    let Some((host, mut node)) = running(&[]) else {
        return;
    };
    let id = lv2_device(1, &[]).id;
    run(
        node.as_mut(),
        &[1.0; FRAMES],
        &[set(0, GAIN, 1.5)],
        &[note(0, NoteKind::On, 64, 0.5)],
    );
    let saved = host.save_state(id).expect("it saves");
    assert_eq!(saved.path, bl_test_lv2::bundle().to_string_lossy());
    let text = String::from_utf8(saved.data.clone()).expect("lilv writes text");
    assert!(text.contains("urn:beatlayer:test:last-key"), "{text}");

    // A set reopened: a new host, which knows nothing loaded, finds the
    // plug-in by its URI and gives it its state.
    let reopened = lv2_host().unwrap();
    let mut restored = lv2_device(1, &[]);
    restored.plugin = Some(saved.clone());
    let mut node = reopened.factory().create(&restored, &config()).unwrap();
    let status = reopened.status(restored.id).unwrap();
    assert!(
        status.values.contains(&(ParamId(GAIN), 1.5)),
        "{:?}",
        status.values
    );
    let out = run(node.as_mut(), &[1.0; FRAMES], &[], &[]);
    assert!(out.channels[0].iter().all(|s| *s == 1.5));
    assert_eq!(reopened.save_state(restored.id).unwrap(), saved);

    // The set's own values go over the state (R11).
    let mut changed = lv2_device(2, &[(GAIN, 0.5)]);
    changed.plugin = Some(saved.clone());
    let mut node = reopened.factory().create(&changed, &config()).unwrap();
    let out = run(node.as_mut(), &[1.0; FRAMES], &[], &[]);
    assert!(out.channels[0].iter().all(|s| *s == 0.5));

    // A state loaded into the running plug-in replaces it.
    let mut other = lv2_device(3, &[]);
    other.plugin = None;
    let mut node = reopened.factory().create(&other, &config()).unwrap();
    assert!(
        run(node.as_mut(), &[1.0; FRAMES], &[], &[]).channels[0]
            .iter()
            .all(|s| *s == 1.0)
    );
    reopened.load_state(other.id, &saved.data).unwrap();
    let out = run(node.as_mut(), &[1.0; FRAMES], &[], &[]);
    assert!(out.channels[0].iter().all(|s| *s == 1.5));

    // A state it refuses changes nothing.
    let refused = reopened.load_state(other.id, b"not a state");
    assert!(matches!(refused, Err(Lv2Error::Failed(_))), "{refused:?}");
    let out = run(node.as_mut(), &[1.0; FRAMES], &[], &[]);
    assert!(out.channels[0].iter().all(|s| *s == 1.5));
}

// Spec: lv2-hosting R11. A state the plug-in refuses leaves it at its
// defaults, and `plugin.info` says so; the set's values still go on it.
#[test]
fn a_state_the_plugin_refuses_leaves_it_at_its_defaults() {
    let Some(host) = lv2_host() else { return };
    let mut device = lv2_device(1, &[(GAIN, 0.75)]);
    device.plugin = Some(PluginState {
        path: bl_test_lv2::bundle().to_string_lossy().into_owned(),
        data: b"not a state".to_vec(),
    });
    let mut node = host.factory().create(&device, &config()).unwrap();
    let out = run(node.as_mut(), &[1.0; FRAMES], &[], &[]);
    assert!(out.channels[0].iter().all(|s| *s == 0.75));
    let status = host.status(device.id).unwrap();
    assert!(status.active);
    assert!(
        status.failure.is_some_and(|f| f.contains("defaults")),
        "it says it starts from its defaults"
    );
}

// Spec: lv2-hosting R10. The plug-in's latency port is the node's latency,
// and the engine is told when it changes.
#[test]
fn the_latency_port_is_the_nodes_latency_and_is_reported() {
    let Some((host, mut node)) = running(&[]) else {
        return;
    };
    let id = lv2_device(1, &[]).id;
    assert_eq!(node.latency(), 0, "before its first block");
    run(node.as_mut(), &[1.0; FRAMES], &[], &[]);
    assert_eq!(node.latency(), LATENCY);
    host.idle();
    assert_eq!(host.status(id).unwrap().latency, LATENCY);
    assert_eq!(host.take_latency_changes(), [(id, LATENCY)]);
    assert!(host.take_latency_changes().is_empty(), "once");

    // A node made later, after an engine restart, reads it at once.
    let later = host
        .factory()
        .create(&lv2_device(1, &[]), &config())
        .unwrap();
    assert_eq!(later.latency(), LATENCY);
}

// Spec: lv2-hosting R12 (as clap-hosting's R12). A device whose plug-in
// can't be found passes its input through, says so, and keeps its saved
// state: nothing can be saved over it.
#[test]
fn a_plugin_that_cant_be_found_leaves_its_device_passing_audio_through() {
    let Some(host) = lv2_host() else { return };
    let mut lost = lv2_device(9, &[]);
    lost.kind = DeviceKind::Plugin {
        format: PluginFormat::Lv2,
        plugin_id: "urn:nowhere:gone".to_owned(),
    };
    lost.plugin = Some(PluginState {
        path: "/nowhere/gone.lv2".to_owned(),
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
        Err(Lv2Error::NoInstance(_))
    ));
    assert_eq!(lost.plugin.as_ref().unwrap().data, b"kept");
}

// Spec: lv2-hosting R12, R19. Without lilv, the device passes its input
// through and the host says LV2 is unavailable.
#[cfg(not(target_os = "linux"))]
#[test]
fn without_lilv_lv2_is_unavailable_and_a_device_passes_audio_through() {
    let host = Lv2Host::new();
    assert_eq!(host.load(bl_test_lv2::bundle()), Err(Lv2Error::Unavailable));
    let mut node = host
        .factory()
        .create(&lv2_device(1, &[]), &config())
        .unwrap();
    let out = run(node.as_mut(), &[1.0; FRAMES], &[], &[]);
    assert!(out.channels.iter().flatten().all(|s| *s == 1.0));
    assert!(
        host.status(lv2_device(1, &[]).id)
            .unwrap()
            .failure
            .is_some()
    );
}

// Spec: lv2-hosting R14
#[test]
fn main_thread_calls_from_another_thread_are_refused() {
    let Some(host) = lv2_host() else { return };
    std::thread::scope(|s| {
        s.spawn(|| {
            assert_eq!(host.load(bl_test_lv2::bundle()), Err(Lv2Error::WrongThread));
        });
    });
}

/// Channel 0 of `blocks` blocks of `FRAMES / blocks` frames of 1.0 through
/// a node for a Test Gain Echo with a gain of 0.5 and the clip envelope
/// `data`, from song frame `from`.
fn gain_render(data: &NodeData, from: i64, blocks: usize) -> Vec<f32> {
    let (_host, mut node) = running(&[(GAIN, 0.5)]).expect("lilv");
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
fn an_lv2_parameter_takes_the_played_value() {
    let Some(host) = lv2_host() else { return };
    // A clip's envelope climbs from nothing to the whole travel over the
    // bar.
    let data = modulation_for(
        lv2_device(1, &[]),
        GAIN,
        &[(Ticks::ZERO, 0.0), (Ticks::WHOLE, 1.0)],
    );
    let list = data.downcast_ref::<AutomationList>().expect("a list");
    let types = host.types();
    let gain = types[0]
        .device_type
        .description
        .param(ParamId(GAIN))
        .unwrap();
    let id = ParamId(GAIN);
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

    // Every parameter is given the value it plays, on the block's first
    // frame and on each song frame that is a multiple of 32: the block runs
    // in parts between them.
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
    let (_host, mut node) = running(&[(GAIN, 0.5)]).unwrap();
    node.receive_data(data);
    let transport = TransportInfo {
        playing: true,
        position: Frames(from),
    };
    run_at(node.as_mut(), &[1.0; FRAMES], &[], &[], transport);
    let stopped = run(node.as_mut(), &[1.0; FRAMES], &[], &[]);
    assert!(stopped.channels[0].iter().all(|s| *s == 0.5));
}

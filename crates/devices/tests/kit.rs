//! The kit's plug-ins compiled in as native devices (the device framework spec,
//! R21 to R23): the kit's example plug-ins run in nodes, parameter changes
//! and notes land on their frames, commands run on the main thread and
//! report what they changed, and every block runs under the allocation
//! trap.

use crate::common;
use bl_core::time::{Frames, Ticks};
use bl_devices::KitHost;
use bl_engine::{
    Block, Collector, Compiler, EngineConfig, NodeKey, NoteBuffer, NoteEvent, NoteKind, Registry,
    TransportInfo,
};
use bl_model::{AutomationTarget, Command, DeviceKind, DeviceRole, Editor, TrackKind};
use common::{CHANNELS, RATE, config, device, run, run_at, set};
use lspk_example_gain::{GAIN, Gain};
use lspk_example_sine::Sine;
use serde_json::json;

fn host_with_examples() -> (KitHost, Registry) {
    let host = KitHost::new();
    host.add::<Gain>();
    host.add::<Sine>();
    let mut registry = Registry::empty();
    registry.add(host.factory());
    (host, registry)
}

// Spec: device-framework R21
#[test]
fn a_kit_plugin_is_a_native_device_type_described_by_its_own_descriptors() {
    let host = KitHost::new();
    let gain = host.add::<Gain>();
    let sine = host.add::<Sine>();
    assert_eq!(
        gain.kind,
        DeviceKind::Native {
            device_type: lspk_example_gain::ID.to_owned()
        }
    );
    assert_eq!(gain.name, "Example Gain");
    assert_eq!(gain.role, DeviceRole::AudioEffect);
    assert_eq!(sine.role, DeviceRole::Instrument);
    assert_eq!(gain.description.params, lspk_example_gain::params());
    assert_eq!(host.device_types(), vec![gain.clone(), sine]);
    assert!(host.handles(&gain.kind));
}

// Spec: device-framework R22, R9
#[test]
fn a_change_lands_on_its_frame_in_the_plugins_processor() {
    let (host, registry) = host_with_examples();
    let gain = host.device_types()[0].clone();
    let device = device(&gain, &[]);
    // Made on this thread, the main thread, so its processor arrives at once.
    let mut node = registry.create(&device, &config(256)).expect("a node");
    assert!(host.is_active(device.id));
    let out = run(node.as_mut(), &[1.0; 256], &[set(10, GAIN, 0.0)]);
    for channel in &out {
        assert!(channel[..10].iter().all(|s| *s == 1.0), "unchanged before");
        assert!(channel[10] < 1.0, "the glide starts on its frame");
        assert!(channel[10..].windows(2).all(|w| w[1] < w[0]), "and falls");
    }
}

// Spec: device-framework R22
#[test]
fn until_its_processor_arrives_an_effect_passes_its_input_through() {
    let (host, registry) = host_with_examples();
    let gain = host.device_types()[0].clone();
    let quiet = device(&gain, &[(GAIN, 0.25)]);
    // Made on another thread, as the engine's compiler makes nodes: the
    // processor waits for the main thread.
    let mut node = std::thread::scope(|s| {
        s.spawn(|| registry.create(&quiet, &config(64)))
            .join()
            .expect("the compiler thread")
    })
    .expect("a node");
    let out = run(node.as_mut(), &[0.5; 64], &[]);
    assert!(out[0].iter().all(|s| *s == 0.5), "passed through");
    host.idle();
    assert!(host.is_active(quiet.id));
    let out = run(node.as_mut(), &[0.5; 64], &[]);
    assert!(
        out[0].iter().all(|s| (*s - 0.125).abs() < 1e-6),
        "{}",
        out[0][0]
    );
    // Dropped, the node gives the processor back to the main thread.
    drop(node);
    host.idle();
    assert!(!host.is_active(quiet.id));
}

// Spec: device-framework R22, R9
#[test]
fn an_instrument_plays_each_note_from_its_frame() {
    let (host, registry) = host_with_examples();
    let sine = host.device_types()[1].clone();
    let device = device(&sine, &[]);
    let mut node = registry.create(&device, &config(128)).expect("a node");
    let notes = [NoteEvent {
        offset: 40,
        kind: NoteKind::On,
        channel: 0,
        key: 69,
        velocity: 1.0,
        note_id: None,
    }];
    // An instrument starts from silence, whatever its buffer held.
    let mut data = vec![0.3f32; 128 * CHANNELS];
    let mut out = NoteBuffer::with_capacity(8);
    {
        let mut block = Block::new(
            &mut data,
            CHANNELS,
            128,
            128,
            &[],
            TransportInfo::default(),
            RATE,
        )
        .with_notes(&notes, &mut out);
        bl_rt::rt_test!({ node.process(&mut block) });
    }
    let left = &data[..128];
    assert!(
        left[..40].iter().all(|s| *s == 0.0),
        "silent until the note"
    );
    assert!(left[41..].iter().any(|s| *s != 0.0), "sounding after it");
    assert_eq!(&data[128..], left, "both channels");
}

// Spec: device-framework R23
#[test]
fn a_command_runs_on_the_main_thread_and_reports_its_changes() {
    let (host, registry) = host_with_examples();
    let gain = host.device_types()[0].clone();
    let quiet = device(&gain, &[(GAIN, 0.25)]);
    let _node = registry.create(&quiet, &config(64)).expect("a node");
    let commands = host.commands(quiet.id).expect("its commands");
    let reset = commands.iter().find(|c| c.id == "gain.reset").unwrap();
    assert_eq!(reset.permission, Some(bl_commands::Permission::Edit));
    let run = host
        .run_command(quiet.id, "gain.reset", &json!({}))
        .expect("it runs");
    assert_eq!(run.result, json!(null));
    assert_eq!(run.changes, vec![(GAIN, 1.0)]);
    assert!(
        host.run_command(quiet.id, "gain.reset", &json!({ "to": 2 }))
            .unwrap_err()
            .contains("schema")
    );
    assert!(
        host.run_command(quiet.id, "gain.boost", &json!({}))
            .is_err()
    );
    // The set's state for it holds its values in the kit's layout.
    let state = host.save_state(quiet.id).expect("a state");
    assert_eq!(&state.data[..4], b"LSPK");
    let elsewhere = bl_model::DeviceId(bl_core::id::Id::new(99).expect("an ID"));
    assert!(host.commands(elsewhere).is_none());
}

// Spec: device-framework R22, automation R2
#[test]
fn an_envelope_drives_a_kit_plugins_parameter() {
    let (host, registry) = host_with_examples();
    let gain = host.device_types()[0].clone();
    let mut editor = Editor::new();
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
    editor
        .apply(Command::InsertDevice {
            track,
            index: 0,
            device: gain.device(id),
        })
        .unwrap();
    // The gain falls from 1 at beat 2 to 0 at beat 3: 24,000 frames a beat.
    let target = AutomationTarget::Device {
        device: id,
        param: GAIN,
    };
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
    let collector = Collector::new();
    let mut compiler = Compiler::new(EngineConfig::new(RATE, 2048), registry, collector.handle());
    let data = compiler
        .sync(editor.document())
        .data
        .into_iter()
        .find(|(key, _)| matches!(key, NodeKey::Device(device) if *device == id))
        .expect("the device's node is sent its automation")
        .1;
    host.idle();

    let (_, registry) = host_with_examples();
    let mut node = registry
        .create(&gain.device(id), &config(2048))
        .expect("a node");
    node.receive_data(data);
    let tempo = editor.document().settings().tempo.clone();
    let input = vec![vec![1.0f32; 2048]; 2];
    let mut last = 1.0;
    for block in 0..3 {
        let out = run_at(
            node.as_mut(),
            &input,
            &[],
            &tempo,
            TransportInfo {
                playing: true,
                position: Frames(36_000 + block * 2048),
                ..TransportInfo::default()
            },
        );
        last = out[0][2047];
    }
    // Halfway down the ramp, and nowhere near the control's own 1.
    assert!((0.1..0.6).contains(&last), "{last}");
}

// Spec: device-framework R22
#[test]
fn a_processor_still_in_the_queue_when_it_is_asked_for_goes_back() {
    let (host, registry) = host_with_examples();
    let gain = host.device_types()[0].clone();
    let quiet = device(&gain, &[(GAIN, 0.25)]);
    let mut node = std::thread::scope(|s| {
        s.spawn(|| registry.create(&quiet, &config(64)))
            .join()
            .expect("the compiler thread")
    })
    .expect("a node");
    // The processor is sent, and let go of before the node has taken it.
    host.idle();
    host.clear();
    for _ in 0..2 {
        let out = run(node.as_mut(), &[0.5; 64], &[]);
        assert!(out[0].iter().all(|s| *s == 0.5), "never run: {}", out[0][0]);
    }
}

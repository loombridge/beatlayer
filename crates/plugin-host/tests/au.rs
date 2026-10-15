//! Audio Unit hosting (WP-2.8; the AU hosting spec): Apple's built-in units
//! — AUDelay, the DLS synth, and AUNewTimePitch — load by component
//! description, render block by block with a parameter change on its exact
//! sample and a note on its frame, keep their state through the set, report
//! their latency, and fail without taking the engine with them. Every block
//! runs as a real-time section under the allocation trap.
//!
//! Spec: au-hosting R1–R4, R8–R10, R12, R17.

#![cfg(target_os = "macos")]

bl_rt::install_trap!();

use bl_core::id::Id;
use bl_core::time::SampleRate;
use bl_engine::{
    Block, Node, NodeConfig, NodeEvent, NodeFactory, NoteBuffer, NoteEvent, NoteKind, ParamValue,
    TransportInfo,
};
use bl_model::{Device, DeviceId, DeviceKind, DeviceRole, ParamId, PluginFormat, PluginState};
use bl_plugin_host::au::{AuError, AuHost};

/// 48 kHz, 512-frame blocks: a 1 ms delay at 48 kHz is 48 frames, so both
/// an impulse and its echo fit one block.
const RATE: SampleRate = SampleRate::HZ_48000;
const FRAMES: usize = 512;
const CHANNELS: usize = 2;

/// AUDelay's parameters, in the order its tree lists them.
const MIX: u32 = 0;
const DELAY_TIME: u32 = 1;
const FEEDBACK: u32 = 2;
const CUTOFF: u32 = 3;

fn config() -> NodeConfig {
    NodeConfig {
        sample_rate: RATE,
        max_frames: FRAMES,
        channels: CHANNELS,
    }
}

/// A device of the Audio Unit with `plugin_id`, with `params` set.
fn device(id: u64, plugin_id: &str, params: &[(u32, f64)]) -> Device {
    let mut device = Device::new(
        DeviceId(Id::new(id).expect("an ID")),
        "Test AU",
        DeviceKind::Plugin {
            format: PluginFormat::AudioUnit,
            plugin_id: plugin_id.to_owned(),
        },
    );
    for (param, value) in params {
        device.params.insert(ParamId(*param), *value);
    }
    device
}

fn set(offset: u32, param: u32, value: f64) -> NodeEvent {
    NodeEvent {
        offset,
        param: ParamId(param),
        value: ParamValue::Set(value),
    }
}

/// What a block put out.
struct Output {
    channels: Vec<Vec<f32>>,
}

/// Runs one block of `input` on every channel through `node`, with
/// `events` and `notes`, as a real-time section.
fn run(node: &mut dyn Node, input: &[f32], events: &[NodeEvent], notes: &[NoteEvent]) -> Output {
    let frames = input.len();
    let mut data: Vec<f32> = (0..CHANNELS).flat_map(|_| input.iter().copied()).collect();
    let mut out = NoteBuffer::with_capacity(64);
    {
        let mut block = Block::new(
            &mut data,
            CHANNELS,
            frames,
            frames,
            events,
            TransportInfo::default(),
            RATE,
        )
        .with_notes(notes, &mut out);
        bl_rt::rt_test!({ node.process(&mut block) });
    }
    Output {
        channels: data.chunks(frames).map(<[f32]>::to_vec).collect(),
    }
}

/// Runs one block of silence through `node`.
fn idle(node: &mut dyn Node) -> Output {
    run(node, &vec![0.0; FRAMES], &[], &[])
}

/// A host with Apple's AUDelay as a device's unit, activated for its node,
/// with `params` set on it.
fn running(params: &[(u32, f64)]) -> (AuHost, Box<dyn Node>) {
    let host = AuHost::new();
    host.load("aufx", "dely", "appl")
        .expect("AUDelay is installed");
    let device = device(1, "aufx/dely/appl", params);
    let node = host
        .factory()
        .create(&device, &config())
        .expect("a node for an AU device");
    // The node was made on this thread, the host's own: making it already
    // gave it its unit.
    assert!(host.status(device.id).is_some_and(|status| status.active));
    (host, node)
}

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

// Spec: au-hosting R1, R3
#[test]
fn an_audio_unit_loads_by_description_with_its_parameters() {
    let host = AuHost::new();
    let found = host.load("aufx", "dely", "appl").expect("AUDelay loads");
    assert_eq!(
        found.device_type.kind,
        DeviceKind::Plugin {
            format: PluginFormat::AudioUnit,
            plugin_id: "aufx/dely/appl".to_owned()
        }
    );
    assert_eq!(found.device_type.name, "AUDelay");
    assert_eq!(found.device_type.role, DeviceRole::AudioEffect);
    assert_eq!(found.vendor.as_deref(), Some("Apple"));
    let names: Vec<&str> = found
        .device_type
        .description
        .params
        .iter()
        .map(|p| p.name.as_str())
        .collect();
    assert_eq!(
        names,
        [
            "Dry/Wet Mix",
            "Delay Time",
            "Feedback",
            "Lowpass Cutoff Frequency"
        ]
    );
    assert!(found.device_type.description.check().is_ok());
    assert_eq!(
        host.factory().device_types(),
        vec![found.device_type.clone()]
    );

    // An instrument and a MIDI effect take their role from the type; a
    // description no unit has, or one that isn't a device, is refused.
    let synth = host.load("aumu", "dls ", "appl").expect("the DLS synth");
    assert_eq!(synth.device_type.role, DeviceRole::Instrument);
    assert_eq!(
        host.load("aumi", "zzzz", "appl"),
        Err(AuError::Load(
            "no Audio Unit aumi/zzzz/appl is installed".to_owned()
        ))
    );
    assert!(matches!(
        host.load("aufc", "conv", "appl"),
        Err(AuError::Load(_))
    ));
}

// Spec: au-hosting R2, R3
#[test]
fn a_parameter_change_lands_on_its_frame() {
    // Fully dry, a 1 ms delay, no feedback: the block's impulse sounds at
    // its own frame, and a change to fully wet at frame 244 is heard from
    // that exact frame (the wet path's echo lands about 48 frames later).
    let (host, mut node) = running(&[
        (MIX, 0.0),
        (DELAY_TIME, 0.001),
        (FEEDBACK, 0.0),
        (CUTOFF, 22050.0),
    ]);
    let mut input = vec![0.0; FRAMES];
    input[243] = 1.0;
    input[244] = 1.0;
    let out = run(node.as_mut(), &input, &[set(244, MIX, 100.0)], &[]);
    for channel in &out.channels {
        assert!(
            channel[..243].iter().all(|s| s.abs() < 1e-6),
            "silence before the impulse: {:?}",
            &channel[240..243]
        );
        assert!(
            (channel[243] - 1.0).abs() < 0.05,
            "the impulse at 243 is still dry: {}",
            channel[243]
        );
        assert!(
            channel[244].abs() < 0.05,
            "the change landed at 244, so no dry impulse there: {}",
            channel[244]
        );
        assert!(
            channel[245..280].iter().all(|s| s.abs() < 1e-5),
            "silence until the echo: {:?}",
            &channel[245..250]
        );
        let echo: f32 = channel[280..300].iter().map(|s| s.abs()).sum();
        assert!(echo > 0.5, "the wet delay sounds after the change: {echo}");
    }
    let status = host
        .status(device(1, "aufx/dely/appl", &[]).id)
        .expect("a status");
    assert!(status.active);
    assert_eq!(status.failure, None);
}

// Spec: au-hosting R2
#[test]
fn a_node_passes_its_input_through_until_its_unit_arrives() {
    let host = AuHost::new();
    host.load("aufx", "dely", "appl").unwrap();
    // Fully wet: once the unit runs, the dry impulse stops sounding.
    let device = device(1, "aufx/dely/appl", &[(MIX, 100.0), (DELAY_TIME, 0.001)]);
    // Made off the main thread, as the compiler thread makes it: the main
    // thread's idle gives it its unit.
    let factory = host.factory();
    let mut node = std::thread::scope(|scope| {
        scope
            .spawn(|| factory.create(&device, &config()))
            .join()
            .unwrap()
    })
    .expect("a node");
    let input: Vec<f32> = (0..FRAMES).map(|i| (i == 0) as u8 as f32).collect();
    let before = run(node.as_mut(), &input, &[], &[]);
    assert!(
        before.channels[0][0] == 1.0,
        "passes through: {:?}",
        &before.channels[0][..3]
    );
    assert!(host.idle());
    let after = run(node.as_mut(), &input, &[], &[]);
    assert!(
        after.channels[0][0].abs() < 1e-6,
        "the active unit is fully wet, so the dry impulse is gone: {}",
        after.channels[0][0]
    );
}

// Spec: au-hosting R4
#[test]
fn a_note_reaches_the_unit_on_its_frame() {
    let host = AuHost::new();
    host.load("aumu", "dls ", "appl").expect("the DLS synth");
    let device = device(1, "aumu/dls /appl", &[]);
    let mut node = host
        .factory()
        .create(&device, &config())
        .expect("a node for an instrument");
    assert!(host.status(device.id).is_some_and(|status| status.active));
    let notes = [
        note(100, NoteKind::On, 60, 0.8),
        note(400, NoteKind::Off, 60, 0.0),
    ];
    let out = run(node.as_mut(), &vec![0.0; FRAMES], &[], &notes);
    let channel = &out.channels[0];
    assert!(
        channel[..100].iter().all(|s| s.abs() < 1e-6),
        "nothing sounds before the note: {:?}",
        &channel[97..100]
    );
    let energy: f32 = channel[100..].iter().map(|s| s.abs()).sum();
    assert!(energy > 0.1, "the note sounds: {energy}");
}

// Spec: au-hosting R8, R9, R11
#[test]
fn a_set_round_trips_the_units_state() {
    let values = [
        (MIX, 25.0),
        (DELAY_TIME, 0.5),
        (FEEDBACK, 10.0),
        (CUTOFF, 5000.0),
    ];
    let host = AuHost::new();
    host.load("aufx", "dely", "appl").unwrap();
    let first = device(1, "aufx/dely/appl", &values);
    let id = first.id;
    let mut node = host.factory().create(&first, &config()).unwrap();
    assert!(host.status(id).is_some_and(|status| status.active));
    idle(node.as_mut());
    let state = host.save_state(id).expect("its state saves");
    assert!(!state.data.is_empty(), "the state has bytes");

    // A new host opens the state onto a fresh unit, with no parameter
    // values of its own: it comes back as it was saved.
    let reopened = AuHost::new();
    let mut second = device(2, "aufx/dely/appl", &[]);
    second.plugin = Some(PluginState {
        path: String::new(),
        data: state.data,
    });
    let id = second.id;
    let mut node = reopened.factory().create(&second, &config()).unwrap();
    assert!(reopened.status(id).is_some_and(|status| status.active));
    idle(node.as_mut());
    let status = reopened.status(id).expect("a status");
    assert_eq!(status.values, values.map(|(p, v)| (ParamId(p), v)));
    assert_eq!(status.failure, None);

    // A unit refuses a state that isn't its own: it keeps the state it
    // had, and starts from its defaults.
    let refused = AuHost::new();
    let mut third = device(3, "aufx/dely/appl", &[]);
    third.plugin = Some(PluginState {
        path: String::new(),
        data: vec![1, 2, 3, 4],
    });
    let id = third.id;
    let node = refused.factory().create(&third, &config()).unwrap();
    let _ = node;
    let status = refused.status(id).expect("a status");
    assert!(status.failure.is_some(), "{status:?}");
}

// Spec: au-hosting R10
#[test]
fn a_units_latency_is_reported() {
    let host = AuHost::new();
    host.load("aufx", "nutp", "appl").expect("AUNewTimePitch");
    let device = device(1, "aufx/nutp/appl", &[]);
    let id = device.id;
    let mut node = host.factory().create(&device, &config()).unwrap();
    let status = host.status(id).expect("a status");
    assert!(status.active);
    assert!(
        status.latency > 0,
        "AUNewTimePitch reports a latency: {status:?}"
    );
    assert_eq!(
        node.latency(),
        status.latency,
        "the node's latency is the unit's"
    );
    idle(node.as_mut());
    assert_eq!(host.status(id).unwrap().latency, status.latency);
}

// Spec: au-hosting R12
#[test]
fn a_unit_that_cant_be_found_leaves_its_device_passing_through() {
    let host = AuHost::new();
    let device = device(1, "aufx/zzzz/appl", &[]);
    let id = device.id;
    let mut node = host
        .factory()
        .create(&device, &config())
        .expect("a node for an unhosted AU device");
    let out = run(node.as_mut(), &vec![0.5; FRAMES], &[], &[]);
    assert!(out.channels[0].iter().all(|s| *s == 0.5), "passes through");
    let status = host.status(id).expect("a status");
    assert!(!status.active);
    assert!(status.failure.is_some(), "and says why: {status:?}");
    let again = run(node.as_mut(), &vec![0.5; FRAMES], &[], &[]);
    assert!(again.channels[0].iter().all(|s| *s == 0.5));
}

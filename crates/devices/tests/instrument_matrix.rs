//! The instruments' modulation matrix (WP-8.4; spec: instrument modulation
//! R1 to R17): routes on the framework's test instrument, whose Output can
//! read a destination's played position, and Drift's Mod section on the same
//! matrix.
//!
//! Every block runs as a real-time section under the allocation trap.

use std::sync::Arc;

use bl_core::time::{Frames, TempoMap, Ticks};
use bl_devices::instrument::matrix::{self, time_factor};
use bl_devices::polysynth::{
    FREQ_MOD1_AMOUNT, FREQ_MOD1_SOURCE, FREQ_MOD2_AMOUNT, FREQ_MOD2_SOURCE, LFO_MOD_AMOUNT,
    LFO_MOD_SOURCE, LFO_MODE, LFO_RATE, MOD_DESTINATIONS, MOD_SOURCES, MOD1_AMOUNT, MOD1_DEST,
    MOD1_SOURCE, MOD2_AMOUNT, MOD2_DEST, MOD2_SOURCE, MOD3_AMOUNT, MOD3_DEST, MOD3_SOURCE, NOISE,
    NOTE_PB, PB_RANGE, PITCH_MOD1_AMOUNT, PITCH_MOD1_SOURCE, PITCH_MOD2_AMOUNT, PITCH_MOD2_SOURCE,
    SHAPE_MOD_AMOUNT, SHAPE_MOD_SOURCE, VOICES,
};
use bl_devices::testing::{
    self, INSTRUMENT_CHOSEN_AMOUNT, INSTRUMENT_CHOSEN_DEST, INSTRUMENT_CHOSEN_SOURCE,
    INSTRUMENT_DESTINATIONS, INSTRUMENT_FIXED_AMOUNT, INSTRUMENT_FREE_AMOUNTS,
    INSTRUMENT_HI_QUALITY, INSTRUMENT_MATRIX_AMOUNT, INSTRUMENT_NOTE_LEVEL, INSTRUMENT_OUTPUT,
    INSTRUMENT_SOURCES, instrument_matrix, instrument_type,
};
use bl_engine::testing::{self as engine_testing, Probe};
use bl_engine::{
    Block, Collector, Compiler, EngineConfig, ModRoutes, Node, NodeKey, NoteBuffer, NoteEvent,
    NoteKind, Processor, TransportCommand, TransportInfo, channel,
};
use bl_model::matrix::SlotKind;
use bl_model::modulator::{self, ModTarget, POLARITY};
use bl_model::{
    AutomationTarget, Clip, ClipData, Command, Device, DeviceId, Editor, MidiData, ModRoute, Note,
    ParamId, TrackKind, Vector,
};

use crate::common::{CHANNELS, RATE, node};
use crate::instrument_voices::{
    BLOCKS, Setting, assert_same, digest, drift_type, expression, hz, message, off, on_at, peak,
    power_at, registry, render_node,
};

/// The test instrument's Output index for each destination's position.
const NOTE_LEVEL_OUT: f64 = 13.0;
const FINE_TUNE_OUT: f64 = 14.0;
const MASTER_OUT: f64 = 15.0;

/// A test instrument with `params` set and `routes` (slot, source,
/// destination) in its free slots.
fn routed(params: &[(ParamId, f64)], routes: &[(u8, &str, &str)]) -> Device {
    let mut device = instrument_type().device(DeviceId(bl_core::id::Id::new(1).unwrap()));
    for &(param, value) in params {
        device.params.insert(param, value);
    }
    device.routes = routes
        .iter()
        .map(|&(slot, source, destination)| ModRoute {
            slot,
            source: source.to_owned(),
            destination: destination.to_owned(),
        })
        .collect();
    device
}

fn render(device: &Device, block: usize, frames: usize, notes: &[NoteEvent]) -> Vec<f32> {
    let mut node = node(&testing::registry(), device, 4096);
    render_node(node.as_mut(), block, frames, notes, &Setting::default())[0].clone()
}

fn assert_all(signal: &[f32], want: f64, what: &str) {
    assert_within(signal, want, 1e-6, what);
}

fn assert_within(signal: &[f32], want: f64, tolerance: f64, what: &str) {
    for (frame, &got) in signal.iter().enumerate() {
        assert!(
            (f64::from(got) - want).abs() < tolerance,
            "{what}: frame {frame} is {got}, not {want}"
        );
    }
}

fn wheel(frame: usize, value: f32) -> NoteEvent {
    message(
        frame,
        NoteKind::Control {
            controller: 1,
            value,
        },
    )
}

fn bend(frame: usize, value: f32) -> NoteEvent {
    message(frame, NoteKind::PitchBend { value })
}

// Spec: instrument modulation R1, R2, R16
#[test]
fn a_devices_sources_destinations_and_slots_are_declared_once_with_fixed_descriptors() {
    let layout = instrument_matrix();
    let description = instrument_type().description;
    description.check().expect("every parameter is automatable");
    assert_eq!(layout.sources.len(), INSTRUMENT_SOURCES.len());
    assert_eq!(layout.dests.len(), INSTRUMENT_DESTINATIONS.len());
    let kinds: Vec<SlotKind> = layout.slots.iter().map(|slot| slot.kind()).collect();
    assert_eq!(
        kinds,
        [
            SlotKind::Free,
            SlotKind::Free,
            SlotKind::Free,
            SlotKind::Free,
            SlotKind::Chosen,
            SlotKind::Fixed
        ]
    );
    // Every amount is a parameter from -100 % to 100 %, 0 % by default.
    for slot in &layout.slots {
        let amount = description.param(slot.amount).expect("an amount parameter");
        assert_eq!(
            (amount.min(), amount.max(), amount.default),
            (-100.0, 100.0, 0.0)
        );
    }
    // Setting routes changes no descriptor.
    let with = routed(&[], &[(0, "Velocity", "Fine Tune")]);
    assert_eq!(
        instrument_type().description,
        description,
        "{:?}",
        with.routes
    );
}

// Spec: instrument modulation R3, R7
#[test]
fn a_velocity_route_moves_a_note_from_its_frame_at_any_block_size() {
    // A note at frame 100, off the 32-frame grid, at velocity 0.5.
    let device = routed(
        &[
            (OUTPUT, NOTE_LEVEL_OUT),
            (INSTRUMENT_FREE_AMOUNTS[0], -50.0),
        ],
        &[(0, "Velocity", "Note Level")],
    );
    let notes = [on_at(100, 60, 0.5)];
    let reference = render(&device, 4096, 2_048, &notes);
    // Unipolar: the position falls from 1 by the velocity times 50 %.
    assert_all(&reference[..100], 0.0, "before the note");
    assert_all(&reference[100..], 0.75, "from the note's own frame");
    for block in BLOCKS {
        assert_same(
            &render(&device, block, 2_048, &notes),
            &reference,
            &format!("block {block}"),
        );
    }
}

const OUTPUT: ParamId = INSTRUMENT_OUTPUT;

// Spec: instrument modulation R6, R13
#[test]
fn each_voice_is_moved_by_its_own_velocity() {
    // Two notes that sound together: the route moves each by its own velocity.
    let play = |routes: &[(u8, &str, &str)], amount: f64| {
        let device = routed(&[(INSTRUMENT_FREE_AMOUNTS[0], amount)], routes);
        let notes = [on_at(0, 60, 1.0), on_at(0, 72, 0.5)];
        let out = render(&device, 512, 12_000, &notes);
        let window = &out[6_000..11_000];
        (power_at(window, hz(60.0)), power_at(window, hz(72.0)))
    };
    let (plain_a, plain_b) = play(&[], 0.0);
    let (a, b) = play(&[(0, "Velocity", "Note Level")], -50.0);
    // Amplitude is velocity times level, 1 x 0.5 against 0.5 x 0.75.
    assert!((plain_a / plain_b - 4.0).abs() < 0.2, "{plain_a} {plain_b}");
    assert!((a / b - 16.0 / 9.0).abs() < 0.1, "{a} {b}");
}

// Spec: instrument modulation R3, R5
#[test]
fn bipolar_sources_swing_the_value_and_the_sum_is_clamped_to_the_travel() {
    // Key 84 reads 0.5, 4 octaves above... two octaves above C3: the swing is
    // 50 % of 0.5 around the middle of Fine Tune's travel.
    let key = routed(
        &[(OUTPUT, FINE_TUNE_OUT), (INSTRUMENT_FREE_AMOUNTS[0], 50.0)],
        &[(0, "Key", "Fine Tune")],
    );
    assert_all(
        &render(&key, 512, 1_024, &[on_at(0, 84, 1.0)])[100..],
        0.75,
        "key 84",
    );
    assert_all(
        &render(&key, 512, 1_024, &[on_at(0, 36, 1.0)])[100..],
        0.25,
        "key 36",
    );
    // A velocity of 1 at -100 % takes the position to -0.5: held at 0.
    let clamped = routed(
        &[
            (OUTPUT, FINE_TUNE_OUT),
            (INSTRUMENT_FREE_AMOUNTS[0], -100.0),
        ],
        &[(0, "Velocity", "Fine Tune")],
    );
    assert_all(
        &render(&clamped, 512, 1_024, &[on_at(0, 60, 1.0)])[100..],
        0.0,
        "clamped",
    );
}

// Spec: instrument modulation R4
#[test]
fn a_multiplicative_route_scales_the_value_and_at_0_percent_changes_nothing() {
    let master = |amount: f64, source: &str, notes: &[NoteEvent]| {
        let device = routed(
            &[(OUTPUT, MASTER_OUT), (INSTRUMENT_FREE_AMOUNTS[0], amount)],
            &[(0, source, "Master")],
        );
        render(&device, 512, 1_024, notes)[200..].to_vec()
    };
    let held = [on_at(0, 60, 1.0), wheel(10, 0.25)];
    // The wheel is a 7-bit controller: 0.25 is 32 of 127.
    let s = 32.0 / 127.0;
    // 100 % takes the source alone: s, and a negative amount 1 - s.
    assert_all(&master(100.0, "Mod Wheel", &held), s, "100 %");
    assert_all(&master(-100.0, "Mod Wheel", &held), 1.0 - s, "-100 %");
    assert_all(
        &master(50.0, "Mod Wheel", &held),
        1.0 - 0.5 * (1.0 - s),
        "50 %",
    );
    // A route at 0 % changes nothing: the played value to the bit.
    assert_all(&master(0.0, "Mod Wheel", &held), 1.0, "0 %");
    // A bipolar source reads (x + 1) / 2.
    let bent = [on_at(0, 60, 1.0), bend(10, 0.5)];
    // The wheel's 14 bits put it within a thousandth of 0.5.
    assert_within(&master(100.0, "Pitch Bend", &bent), 0.75, 1e-3, "bipolar");
}

// Spec: instrument modulation R6
#[test]
fn a_global_destination_reads_the_oldest_sounding_voice() {
    let device = routed(
        &[(OUTPUT, MASTER_OUT), (INSTRUMENT_FREE_AMOUNTS[0], 100.0)],
        &[(0, "Velocity", "Master")],
    );
    // The first note, at velocity 0.5, then a second at 1.0; the first ends.
    let notes = [on_at(0, 60, 0.5), on_at(100, 64, 1.0), off(2_000, 60)];
    let out = render(&device, 512, 12_000, &notes);
    assert_all(&out[1_000..2_000], 0.5, "the oldest voice's velocity");
    assert_all(&out[8_000..], 1.0, "the oldest left sounding");
}

// Spec: instrument modulation R7
#[test]
fn control_rate_destinations_follow_the_grid_unless_hi_quality_is_on() {
    // Pressure lands at frame 1,010 of a note that is sounding.
    let notes = [
        on_at(0, 60, 1.0),
        expression(1_010, 60, bl_engine::NoteExpression::Pressure, 1.0),
    ];
    let play = |hi_quality: f64, block: usize| {
        let device = routed(
            &[
                (OUTPUT, NOTE_LEVEL_OUT),
                (INSTRUMENT_HI_QUALITY, hi_quality),
                (INSTRUMENT_FREE_AMOUNTS[0], -50.0),
            ],
            &[(0, "Pressure", "Note Level")],
        );
        render(&device, block, 2_048, &notes)
    };
    for block in BLOCKS {
        let on = play(1.0, block);
        let off = play(0.0, block);
        // On: the position falls on the frame the pressure lands. Off: it
        // holds until the next multiple of 32, frame 1,024.
        assert_all(&on[1_000..1_010], 1.0, "before");
        assert_all(&on[1_010..1_100], 0.5, "hi-quality");
        assert_all(&off[1_010..1_024], 1.0, "held on the grid");
        assert_all(&off[1_024..1_100], 0.5, "the grid point");
    }
    let reference = play(0.0, 4_096);
    for block in BLOCKS {
        assert_same(&play(0.0, block), &reference, &format!("block {block}"));
    }
}

// Spec: instrument modulation R8, R10, R13
#[test]
fn a_route_changed_during_playback_takes_effect_and_allocates_nothing() {
    let layout = instrument_type().matrix.unwrap();
    let mut node = node(
        &testing::registry(),
        &routed(
            &[
                (OUTPUT, NOTE_LEVEL_OUT),
                (INSTRUMENT_FREE_AMOUNTS[0], -50.0),
            ],
            &[],
        ),
        512,
    );
    let collector = Collector::new();
    let notes = [on_at(0, 60, 1.0)];
    let first = render_node(node.as_mut(), 512, 1_024, &notes, &Setting::default())[0].clone();
    assert_all(&first[100..], 1.0, "no route");
    // A route set on the main thread reaches the playing node.
    let device = routed(&[], &[(0, "Velocity", "Note Level")]);
    node.receive_data(
        collector
            .handle()
            .data(ModRoutes(layout.free_routes(&device))),
    );
    let second = render_node(node.as_mut(), 512, 1_024, &[], &Setting::default())[0].clone();
    assert_all(&second[100..], 0.5, "the route");
    node.receive_data(
        collector
            .handle()
            .data(ModRoutes(layout.free_routes(&routed(&[], &[])))),
    );
    let third = render_node(node.as_mut(), 512, 1_024, &[], &Setting::default())[0].clone();
    assert_all(&third[100..], 1.0, "cleared");
}

// Spec: instrument modulation R2, R10
#[test]
fn a_chosen_slot_takes_its_source_and_destination_from_its_choosers() {
    let device = routed(
        &[
            (OUTPUT, NOTE_LEVEL_OUT),
            (INSTRUMENT_CHOSEN_SOURCE, 1.0),
            (INSTRUMENT_CHOSEN_DEST, 1.0),
            (INSTRUMENT_CHOSEN_AMOUNT, -50.0),
        ],
        &[],
    );
    assert_all(
        &render(&device, 512, 1_024, &[on_at(0, 60, 1.0)])[100..],
        0.5,
        "Velocity into Note Level",
    );
    // A fixed slot: Pressure into Fine Tune, whatever the choosers say.
    let fixed = routed(
        &[(OUTPUT, FINE_TUNE_OUT), (INSTRUMENT_FIXED_AMOUNT, 100.0)],
        &[],
    );
    let notes = [
        on_at(0, 60, 1.0),
        expression(10, 60, bl_engine::NoteExpression::Pressure, 0.5),
    ];
    assert_all(&render(&fixed, 512, 1_024, &notes)[200..], 1.0, "fixed");
}

// Spec: instrument modulation R15
#[test]
fn the_matrix_amount_scales_every_route_and_time_runs_a_factor_of_four_either_way() {
    let play = |amount: f64| {
        let device = routed(
            &[
                (OUTPUT, NOTE_LEVEL_OUT),
                (INSTRUMENT_FREE_AMOUNTS[0], -100.0),
                (INSTRUMENT_MATRIX_AMOUNT, amount),
            ],
            &[(0, "Velocity", "Note Level")],
        );
        render(&device, 512, 1_024, &[on_at(0, 60, 1.0)])[200..].to_vec()
    };
    assert_all(&play(100.0), 0.0, "Amount 100 %");
    assert_all(&play(50.0), 0.5, "Amount 50 %");
    assert_all(&play(0.0), 1.0, "Amount 0 %");
    assert_eq!(time_factor(0.0), 1.0);
    assert_eq!(time_factor(1.0), 0.25);
    assert_eq!(time_factor(-1.0), 4.0);
}

// Spec: instrument modulation R14
#[test]
fn a_route_has_no_curve_aux_source_or_scaler_of_its_own() {
    // The layout's slot has exactly a source, a destination and an amount.
    let slot = instrument_matrix().slots[0];
    let matrix::SlotDef {
        source: _,
        dest: _,
        amount: _,
    } = slot;
}

/// A song of the test instrument playing `notes`, on a track, with its
/// Note Level automated from 100 % at beat 2... returns the node, its
/// tempo map and its automation data.
fn song_with_automation(device: Device) -> (Box<dyn Node>, TempoMap, bl_engine::NodeData) {
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
    let mut device = device;
    device.id = id;
    editor
        .apply(Command::InsertDevice {
            track,
            index: 0,
            device,
        })
        .unwrap();
    let target = AutomationTarget::Device {
        device: id,
        param: INSTRUMENT_NOTE_LEVEL,
    };
    // Note Level falls from 100 % at beat 2 to 20 % at beat 3.
    for (at, value) in [(Ticks::QUARTER, 100.0), (Ticks::QUARTER * 2, 20.0)] {
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
    let mut compiler = Compiler::new(
        EngineConfig::new(RATE, 2048),
        testing::registry(),
        collector.handle(),
    );
    let update = compiler.sync(editor.document());
    let data = update
        .data
        .into_iter()
        .find(|(key, _)| matches!(key, NodeKey::Device(device) if *device == id))
        .expect("the device is sent its automation")
        .1;
    let device = editor.document().device(id).unwrap().clone();
    let node = node(&testing::registry(), &device, 2048);
    (node, editor.document().settings().tempo.clone(), data)
}

// Spec: instrument modulation R5; modulation R30
#[test]
fn a_route_adds_to_the_value_automation_plays_and_is_clamped_to_the_travel() {
    // The mod wheel at 0.5, 50 % into Note Level: +0.25 of its travel.
    let device = routed(
        &[(OUTPUT, NOTE_LEVEL_OUT), (INSTRUMENT_FREE_AMOUNTS[0], 50.0)],
        &[(0, "Mod Wheel", "Note Level")],
    );
    let (mut node, tempo, data) = song_with_automation(device);
    node.receive_data(data);
    let notes = [on_at(0, 60, 1.0), wheel(10, 0.5)];
    let frames = 36_000usize;
    let mut out = Vec::with_capacity(frames);
    let mut start = 0usize;
    while start < frames {
        let len = 512.min(frames - start);
        let block_notes: Vec<NoteEvent> = notes
            .iter()
            .filter(|n| (n.offset as usize) >= start && (n.offset as usize) < start + len)
            .map(|n| NoteEvent {
                offset: n.offset - start as u32,
                ..*n
            })
            .collect();
        let mut data = vec![0.0f32; CHANNELS * len];
        let mut sink = NoteBuffer::with_capacity(block_notes.len().max(1));
        {
            let mut block = Block::new(
                &mut data,
                CHANNELS,
                len,
                len,
                &[],
                TransportInfo {
                    playing: true,
                    position: Frames(start as i64),
                    ..TransportInfo::default()
                },
                RATE,
            )
            .with_tempo(&tempo)
            .with_notes(&block_notes, &mut sink);
            bl_rt::rt_test!({ node.process(&mut block) });
        }
        out.extend_from_slice(&data[..len]);
        start += len;
    }
    // Before beat 2 the envelope holds 100 %: the sum is clamped to 1. On the
    // ramp (24,000 to 48,000 frames) it is the envelope plus 0.25.
    assert_all(&out[1_000..20_000], 1.0, "clamped at the top");
    for frame in [26_048usize, 30_016, 34_016] {
        let envelope = 1.0 - 0.8 * (frame as f64 - 24_000.0) / 24_000.0;
        let want = (envelope + 0.25).min(1.0);
        assert!(
            (f64::from(out[frame]) - want).abs() < 2e-3,
            "frame {frame}: {} isn't {want}",
            out[frame]
        );
    }
}

/// A song on a rig with a Phase 5 LFO on an audio track mapped to the
/// instrument's Note Level, the instrument on a MIDI track playing a held
/// note at velocity 100: the instrument's output over `frames`.
fn lfo_rig(
    base: f64,
    routes: &[(u8, &str, &str)],
    amounts: &[(ParamId, f64)],
    frames: usize,
    block: usize,
) -> Vec<f32> {
    let config = EngineConfig::new(RATE, block);
    let collector = Collector::new();
    let mut registry = engine_testing::registry(&Arc::new(Probe::default()));
    bl_devices::testing::register(&mut registry);
    bl_devices::modulator::register(&mut registry);
    let mut compiler = Compiler::new(config, registry, collector.handle());
    let (mut sender, mut processor, _monitor): (_, Processor, _) = channel(&config);
    let mut editor = Editor::new();
    let mut sync = |editor: &Editor, compiler: &mut Compiler| {
        sender.send_update(compiler.sync(editor.document()));
        assert!(sender.flush(), "the queue is full");
    };
    let track = |editor: &mut Editor, kind| {
        let id = editor.new_id();
        editor
            .apply(Command::CreateTrack {
                id,
                kind,
                name: String::new(),
                parent: None,
                index: 0,
            })
            .unwrap();
        id
    };
    let source = track(&mut editor, TrackKind::Audio);
    let target = track(&mut editor, TrackKind::Midi);
    let id = editor.new_id();
    let mut params = vec![(OUTPUT, NOTE_LEVEL_OUT), (INSTRUMENT_NOTE_LEVEL, base)];
    params.extend_from_slice(amounts);
    let mut device = routed(&params, routes);
    device.id = id;
    editor
        .apply(Command::InsertDevice {
            track: target,
            index: 0,
            device,
        })
        .unwrap();
    let wave_id = editor.new_id();
    let mut wave = Device::new(
        wave_id,
        "",
        bl_model::DeviceKind::Native {
            device_type: modulator::WAVE_MOD.to_owned(),
        },
    );
    for (param, value) in [
        (bl_devices::modulator::WAVE, 1.0),
        (bl_devices::modulator::RATE_HZ, 10.0),
        (POLARITY, 1.0),
        (modulator::amount(0), 20.0),
    ] {
        wave.params.insert(param, value);
    }
    editor
        .apply(Command::InsertDevice {
            track: source,
            index: 0,
            device: wave,
        })
        .unwrap();
    editor
        .apply(Command::MapModulator {
            device: wave_id,
            slot: 0,
            target: ModTarget::Device {
                device: id,
                param: INSTRUMENT_NOTE_LEVEL,
            },
        })
        .unwrap();
    let clip = editor.new_id();
    editor
        .apply(Command::CreateClip {
            track: target,
            clip: Clip::new(
                clip,
                Ticks::ZERO,
                Ticks::WHOLE,
                ClipData::Midi(MidiData {
                    notes: Vector::from(vec![Note::new(60, Ticks::ZERO, Ticks::WHOLE, 127)]),
                    start: Ticks::ZERO,
                    end: None,
                    looping: false,
                    loop_start: Ticks::ZERO,
                    loop_end: None,
                    signature: None,
                    groups: Vec::new(),
                }),
            ),
        })
        .unwrap();
    sync(&editor, &mut compiler);
    sender.transport(TransportCommand::Play);
    let mut out = Vec::with_capacity(frames);
    while out.len() < frames {
        let n = block.min(frames - out.len());
        bl_rt::rt_test!({
            processor.process(n);
        });
        out.extend_from_slice(processor.output().channel(0));
    }
    out
}

// Spec: instrument modulation R5; modulation R30. A route adds to the value
// a Phase 5 LFO plays, and the sum is clamped.
#[test]
fn a_route_adds_to_the_value_an_lfo_plays_at_any_block_size() {
    let routes = [(0u8, "Velocity", "Note Level")];
    let amounts = [(INSTRUMENT_FREE_AMOUNTS[0], 30.0)];
    // The output through the track's mixer: calibrate it with the same
    // instrument reading a steady 1 (its Note Level at 100 %, no route).
    let steady = lfo_rig(100.0, &[], &[], 512, 64);
    let factor = f64::from(*steady.last().unwrap());
    assert!(factor > 0.0);
    let reference = lfo_rig(40.0, &routes, &amounts, 4_800, 64);
    for block in [1usize, 512] {
        assert_same(
            &lfo_rig(40.0, &routes, &amounts, 4_800, block),
            &reference,
            &format!("block {block}"),
        );
    }
    // The LFO rises 20 % over its 4,800-frame cycle: at each grid frame the
    // played value is 40 % plus its share, and the route adds velocity
    // (127/127 = 1) times 30 %.
    let velocity = 1.0;
    for step in [1usize, 40, 80, 120] {
        let share = 0.2 * step as f64 / 150.0;
        let want = 0.4 + share + 0.3 * velocity;
        let got = f64::from(reference[step * 32]) / factor;
        assert!((got - want).abs() < 2e-3, "step {step}: {got} isn't {want}");
    }
    // A base of 90 % and the same route go past the travel: clamped to 1.
    let clamped = lfo_rig(90.0, &routes, &amounts, 4_800, 64);
    for step in [1usize, 80, 149] {
        let got = f64::from(clamped[step * 32]) / factor;
        assert!((got - 1.0).abs() < 2e-3, "step {step}: {got}");
    }
}

/// Drift with a route in every slot, played through a phrase with the wheel,
/// pressure, slide and a bend, at 512 frames a block.
fn drift_with_every_route() -> Vec<f32> {
    let source = |name: &str| MOD_SOURCES.iter().position(|s| *s == name).unwrap() as f64;
    let destination = |name: &str| MOD_DESTINATIONS.iter().position(|d| *d == name).unwrap() as f64;
    let params = [
        (VOICES, 4.0),
        (NOTE_PB, 1.0),
        (PB_RANGE, 12.0),
        (NOISE, 1.0),
        (LFO_MODE, 0.0),
        (LFO_RATE, 6.0),
        (MOD1_SOURCE, source("LFO")),
        (MOD1_DEST, destination("LP Frequency")),
        (MOD1_AMOUNT, 40.0),
        (MOD2_SOURCE, source("Velocity")),
        (MOD2_DEST, destination("Osc 1 Gain")),
        (MOD2_AMOUNT, 30.0),
        (MOD3_SOURCE, source("Slide")),
        (MOD3_DEST, destination("Osc 2 Detune")),
        (MOD3_AMOUNT, 50.0),
        (SHAPE_MOD_SOURCE, source("Env 2 / Cyc")),
        (SHAPE_MOD_AMOUNT, 30.0),
        (PITCH_MOD1_SOURCE, source("Key")),
        (PITCH_MOD1_AMOUNT, 5.0),
        (PITCH_MOD2_SOURCE, source("Pressure")),
        (PITCH_MOD2_AMOUNT, 10.0),
        (FREQ_MOD1_SOURCE, source("Env 1")),
        (FREQ_MOD1_AMOUNT, 25.0),
        (FREQ_MOD2_SOURCE, source("Modwheel")),
        (FREQ_MOD2_AMOUNT, -20.0),
        (LFO_MOD_SOURCE, source("Env 1")),
        (LFO_MOD_AMOUNT, 30.0),
    ];
    let mut device = drift_type().device(DeviceId(bl_core::id::Id::new(1).unwrap()));
    for (param, value) in params {
        device.params.insert(param, value);
    }
    let mut node = node(&registry(), &device, 4096);
    let notes = [
        on_at(0, 60, 0.8),
        on_at(0, 64, 0.6),
        on_at(0, 67, 0.9),
        on_at(2_000, 76, 0.7),
        expression(4_000, 64, bl_engine::NoteExpression::Slide, 0.5),
        expression(5_000, 67, bl_engine::NoteExpression::Pressure, 0.6),
        wheel(8_000, 0.7),
        off(14_000, 60),
        on_at(16_000, 55, 0.5),
        off(30_000, 55),
    ];
    render_node(node.as_mut(), 512, 36_000, &notes, &Setting::default())[0].clone()
}

// Spec: instrument modulation R17; polysynth R20. The digest was taken from
// Drift before its Mod section moved onto the matrix.
#[test]
fn drift_with_a_route_in_every_slot_renders_as_it_did_sample_for_sample() {
    let signal = drift_with_every_route();
    assert!(peak(&signal) > 0.01, "the phrase sounds");
    // A vector unit of another architecture rounds some samples differently,
    // so only the one it was taken on compares it.
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    assert_eq!(digest(&signal), 0xaad4_879d_4520_20e3);
    let _ = digest(&signal);
}

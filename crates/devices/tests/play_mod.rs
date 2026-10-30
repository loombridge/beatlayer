//! The Expression Control (WP-5.11; the modulation spec, R27): each of its five tabs
//! follows one source of the notes and MIDI it hears, through a curve and
//! smoothing, to its own slot; Increment goes round its steps and starts
//! again when the transport stops; Random draws at each note from a
//! generator that starts again with the transport. The notes and MIDI pass
//! through untouched. Every block runs as a real-time section under the
//! allocation trap.

use crate::common;
use bl_core::time::{Frames, TempoMap};
use bl_devices::play_mod::{Field, param, play_mod_type};
use bl_engine::{Node, NoteEvent, NoteExpression, NoteKind, Registry, TransportInfo};
use bl_model::ParamId;
use bl_model::modulator::{self, POLARITY};
use common::{Signal, device, node, note_off, note_on, signal};

fn registry() -> Registry {
    let mut registry = Registry::empty();
    bl_devices::play_mod::register(&mut registry);
    registry
}

/// The sources, by their place in the chooser.
const VELOCITY: f64 = 0.0;
const MODWHEEL: f64 = 1.0;
const PITCHBEND: f64 = 2.0;
const PRESSURE: f64 = 3.0;
const KEYTRACK: f64 = 4.0;
const EXPRESSION: f64 = 5.0;
const RANDOM: f64 = 6.0;
const INCREMENT: f64 = 7.0;
const SLIDE: f64 = 8.0;
const SUSTAIN: f64 = 9.0;

fn playing(at: usize) -> TransportInfo {
    TransportInfo {
        playing: true,
        position: Frames(at as i64),
        ..TransportInfo::default()
    }
}

/// An Expression Control whose first tab follows `source` and gives it itself in
/// slot 1 (unipolar, full amount); `params` over those.
fn play(source: f64, params: &[(ParamId, f64)]) -> Box<dyn Node> {
    let mut all = vec![
        (modulator::amount(0), 100.0),
        (POLARITY, 1.0),
        (param(0, Field::Source), source),
    ];
    all.extend_from_slice(params);
    node(&registry(), &device(&play_mod_type(), &all), 512)
}

fn run_with(
    node: &mut dyn Node,
    frames: usize,
    block: usize,
    notes: &[NoteEvent],
    transport: impl Fn(usize) -> TransportInfo,
) -> Signal {
    signal(
        node,
        frames,
        block,
        &[],
        None,
        notes,
        &[],
        &TempoMap::default(),
        transport,
    )
}

fn run(node: &mut dyn Node, frames: usize, notes: &[NoteEvent]) -> Signal {
    run_with(node, frames, 512, notes, playing)
}

/// A message of kind `kind` on frame `offset`.
fn message(offset: u32, kind: NoteKind) -> NoteEvent {
    NoteEvent {
        offset,
        kind,
        channel: 0,
        key: 0,
        velocity: 0.0,
        note_id: None,
    }
}

fn control(offset: u32, controller: u8, value: f32) -> NoteEvent {
    message(offset, NoteKind::Control { controller, value })
}

/// The first tab's output at the end of `frames` frames.
fn last(out: &Signal) -> f32 {
    *out.rows.contribution(0).last().unwrap()
}

// Spec: modulation R27
#[test]
fn each_source_drives_its_tabs_target() {
    let close = |got: f32, want: f32, what: &str| {
        assert!((got - want).abs() < 1e-3, "{what}: {got} for {want}");
    };
    let end = 2_048;
    let one = |source, notes: &[NoteEvent], params: &[(ParamId, f64)]| {
        let mut node = play(source, params);
        last(&run(node.as_mut(), end, notes))
    };
    close(
        one(VELOCITY, &[note_on(100, 60, 0.5, None)], &[]),
        0.5,
        "Velocity",
    );
    close(one(MODWHEEL, &[control(100, 1, 0.7)], &[]), 0.7, "Modwheel");
    close(
        one(EXPRESSION, &[control(100, 11, 0.3)], &[]),
        0.3,
        "Expression",
    );
    close(one(SUSTAIN, &[control(100, 64, 1.0)], &[]), 1.0, "Sustain");
    close(one(PITCHBEND, &[], &[]), 0.5, "Pitchbend at the centre");
    close(
        one(
            PITCHBEND,
            &[message(100, NoteKind::PitchBend { value: 1.0 })],
            &[],
        ),
        1.0,
        "Pitchbend up",
    );
    close(
        one(
            PITCHBEND,
            &[message(100, NoteKind::PitchBend { value: -1.0 })],
            &[],
        ),
        0.0,
        "Pitchbend down",
    );
    close(
        one(
            PRESSURE,
            &[message(100, NoteKind::ChannelPressure { value: 0.4 })],
            &[],
        ),
        0.4,
        "Pressure from the channel",
    );
    close(
        one(KEYTRACK, &[note_on(100, 127, 1.0, None)], &[]),
        1.0,
        "Keytrack, top",
    );
    close(
        one(KEYTRACK, &[note_on(100, 0, 1.0, None)], &[]),
        0.0,
        "Keytrack, bottom",
    );
    close(
        one(KEYTRACK, &[note_on(100, 64, 1.0, None)], &[]),
        64.0 / 127.0,
        "Keytrack, middle",
    );
    close(one(VELOCITY, &[], &[]), 0.0, "nothing played reads 0");

    // Per-note sources follow the most recent note held.
    let first = note_on(100, 60, 1.0, Some(1));
    let second = note_on(200, 64, 1.0, Some(2));
    let expression = |note: &NoteEvent, expression, value| NoteEvent {
        offset: 300,
        kind: NoteKind::Expression { expression, value },
        ..*note
    };
    let notes = [
        first,
        second,
        expression(&second, NoteExpression::Slide, 0.8),
        expression(&first, NoteExpression::Slide, 0.2),
        expression(&second, NoteExpression::Pressure, 0.6),
    ];
    close(
        one(SLIDE, &notes, &[]),
        0.8,
        "Slide follows the latest note",
    );
    close(
        one(PRESSURE, &notes, &[]),
        0.6,
        "so does a note's own Pressure",
    );

    // Random: none of the draw's spread at 0 %, a new value at each note, the
    // same ones from the same start.
    close(
        one(
            RANDOM,
            &[note_on(100, 60, 1.0, None)],
            &[(param(0, Field::RandomAmount), 0.0)],
        ),
        0.5,
        "Random without spread",
    );
    let draws = |notes: &[NoteEvent]| {
        let mut node = play(RANDOM, &[]);
        let out = run(node.as_mut(), 3_000, notes);
        let level = out.rows.contribution(0);
        [800, 1_800, 2_900].map(|at| level[at])
    };
    let notes = [
        note_on(100, 60, 1.0, None),
        note_on(1_000, 61, 1.0, None),
        note_on(2_000, 62, 1.0, None),
    ];
    let a = draws(&notes);
    assert_eq!(a, draws(&notes), "a render repeats");
    assert!(a[0] != a[1] && a[1] != a[2], "a new value each note: {a:?}");
    assert!(a.iter().all(|v| (0.0..=1.0).contains(v)));
}

// Spec: modulation R27
#[test]
fn increment_steps_and_starts_again_at_stop() {
    let steps = [(param(0, Field::IncrementSteps), 4.0)];
    let notes: Vec<NoteEvent> = (0..8)
        .map(|i| note_on(1_000 * (i + 1), 60 + i as u8, 1.0, None))
        .collect();
    // Playing for the first five notes, stopped for the rest.
    let transport = |at: usize| {
        if at < 6_000 {
            playing(at)
        } else {
            TransportInfo {
                playing: false,
                position: Frames(0),
                ..TransportInfo::default()
            }
        }
    };
    let mut node = play(INCREMENT, &steps);
    let out = run_with(node.as_mut(), 9_000, 500, &notes, transport);
    let level = out.rows.contribution(0);
    let third = 1.0 / 3.0;
    for (note, want) in [0.0, third, 2.0 * third, 1.0, 0.0].into_iter().enumerate() {
        let got = level[1_000 * (note + 1) + 500];
        assert!((got - want).abs() < 1e-3, "note {note}: {got} for {want}");
    }
    // The transport stopped between the fifth note and the sixth, so the
    // sixth goes back to the first step, and the seventh goes on.
    assert!(
        level[6_500].abs() < 1e-3,
        "the first step again: {}",
        level[6_500]
    );
    assert!((level[7_500] - third).abs() < 1e-3, "{}", level[7_500]);

    // One step is always the first.
    let mut node = play(INCREMENT, &[(param(0, Field::IncrementSteps), 1.0)]);
    let out = run(node.as_mut(), 4_000, &notes[..3]);
    assert!(out.rows.contribution(0).iter().all(|&v| v == 0.0));
}

// Spec: modulation R21, R27
#[test]
fn random_starts_again_with_the_transport() {
    let notes = [note_on(100, 60, 1.0, None), note_on(1_000, 61, 1.0, None)];
    let mut node = play(RANDOM, &[]);
    let first = run(node.as_mut(), 2_000, &notes);
    // The transport stops, then starts: the draws begin again.
    let stopped = |_: usize| TransportInfo {
        playing: false,
        position: Frames(0),
        ..TransportInfo::default()
    };
    run_with(node.as_mut(), 512, 512, &[], stopped);
    let second = run(node.as_mut(), 2_000, &notes);
    assert!(
        first.rows.0 == second.rows.0,
        "the same draws and the same rows"
    );
}

// Spec: modulation R27
#[test]
fn the_curve_and_smoothing_shape_the_output() {
    let at = |source: f64, velocity: f32, params: &[(ParamId, f64)]| {
        let mut node = play(source, params);
        last(&run(
            node.as_mut(),
            2_048,
            &[note_on(100, 60, velocity, None)],
        ))
    };
    let close = |got: f32, want: f32, what: &str| {
        assert!((got - want).abs() < 2e-3, "{what}: {got} for {want}");
    };
    // Linear, bent by Curve A: the power curve of the automation spec.
    close(
        at(VELOCITY, 0.5, &[(param(0, Field::CurveA), 50.0)]),
        0.5f32.powf(2f32.powf(0.5)),
        "Curve A",
    );
    // S: through (½, 0.8) with the lower segment and the upper straight.
    let s = |velocity, more: &[(ParamId, f64)]| {
        let mut params = vec![
            (param(0, Field::Curve), 1.0),
            (param(0, Field::CurveX), 50.0),
            (param(0, Field::CurveY), 80.0),
        ];
        params.extend_from_slice(more);
        at(VELOCITY, velocity, &params)
    };
    close(s(0.25, &[]), 0.4, "S, lower");
    close(s(0.5, &[]), 0.8, "S, middle");
    close(s(0.75, &[]), 0.9, "S, upper");
    // Link bends the lower segment the other way from the upper.
    let bent = s(
        0.25,
        &[
            (param(0, Field::CurveA), 50.0),
            (param(0, Field::CurveLink), 1.0),
        ],
    );
    let unlinked = s(0.25, &[(param(0, Field::CurveA), 50.0)]);
    close(unlinked, 0.4, "A bends only the upper segment");
    assert!(bent > 0.4, "Link bends the lower the other way: {bent}");
    // Min and Max put the output between two others, and Min above Max
    // flips it.
    let range = |min, max| {
        at(
            VELOCITY,
            0.25,
            &[(param(0, Field::Min), min), (param(0, Field::Max), max)],
        )
    };
    close(range(20.0, 60.0), 0.3, "Min and Max");
    close(range(100.0, 0.0), 0.75, "flipped");

    // Smoothing: Rise 100 ms is 4,800 frames. Linear takes that for the whole
    // range, Logarithmic is a time constant.
    let rise = |smoothing: f64| {
        let mut node = play(
            VELOCITY,
            &[
                (param(0, Field::Smoothing), smoothing),
                (param(0, Field::Rise), 100.0),
                (param(0, Field::Fall), 200.0),
            ],
        );
        let notes = [note_on(0, 60, 1.0, None), note_on(12_000, 60, 0.0, None)];
        run(node.as_mut(), 30_000, &notes)
            .rows
            .contribution(0)
            .to_vec()
    };
    let linear = rise(0.0);
    assert!(
        (linear[2_400] - 0.5).abs() < 0.02,
        "Linear, half way: {}",
        linear[2_400]
    );
    assert!((linear[4_800 + 100] - 1.0).abs() < 0.02, "Linear, there");
    assert!(
        (linear[12_000 + 4_800] - 0.5).abs() < 0.02,
        "Linear fall, half way in 200 ms"
    );
    let log = rise(1.0);
    assert!(
        (log[4_800] - (1.0 - (-1.0f32).exp())).abs() < 0.02,
        "Logarithmic, a time constant: {}",
        log[4_800]
    );
}

// Spec: modulation R27
#[test]
fn the_notes_and_midi_pass_through_and_the_output_is_the_same_at_any_block_size() {
    let on = note_on(100, 60, 0.8, Some(1));
    let notes = [
        on,
        control(150, 1, 0.5),
        message(200, NoteKind::PitchBend { value: 0.3 }),
        note_off(900, &on, 0.5),
    ];
    let params = [
        (param(0, Field::Smoothing), 1.0),
        (param(0, Field::Rise), 20.0),
        (modulator::amount(1), 100.0),
        (param(1, Field::Source), MODWHEEL),
    ];
    let mut node = play(VELOCITY, &params);
    let reference = run(node.as_mut(), 4_000, &notes);
    assert_eq!(reference.notes, notes.to_vec());
    assert!(
        reference.rows.contribution(1).iter().any(|&v| v != 0.0),
        "the second tab has its own slot"
    );
    for block in [1, 32, 64] {
        let mut again = play(VELOCITY, &params);
        let out = run_with(again.as_mut(), 4_000, block, &notes, playing);
        assert_eq!(out.rows.0, reference.rows.0, "blocks of {block} frames");
    }
}

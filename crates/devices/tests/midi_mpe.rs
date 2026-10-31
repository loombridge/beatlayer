//! MPE Control (WP-5.32; the MPE shaper spec): each source's curve, range
//! and smoothing shape its per-note data, and each conversion turns it into
//! a channel message. Every block is a real-time section under the
//! allocation trap.

use crate::common;
use bl_devices::midi_mpe::{
    NOTEPB_TO_PB, PITCH_RANGE, PRESS, PRESS_DEFAULT, PRESS_DEFAULT_VALUE, PRESS_TO_AT,
    SLIDE_CENTERED, SLIDE_DEFAULT, SLIDE_DEFAULT_VALUE, SLIDE_MODE, SLIDE_TO_MOD, SWAP_TO_SLIDE,
    mpe_control_type, source_curve, source_curve_a, source_max, source_min, source_on, source_rise,
    source_smooth, source_x, source_y,
};
use bl_engine::{NoteEvent, NoteExpression, NoteKind};
use bl_model::ParamId;
use common::{device, node, note_on, notes_through, registry};

const FRAMES: usize = 256;

fn expression(offset: u32, expression: NoteExpression, value: f32) -> NoteEvent {
    NoteEvent {
        offset,
        kind: NoteKind::Expression { expression, value },
        channel: 0,
        key: 60,
        velocity: 0.0,
        note_id: None,
    }
}

/// Plays a note with `expressions` through a device with `params`, and
/// returns what it sent.
fn shape_run(params: &[(ParamId, f64)], expressions: &[NoteEvent]) -> Vec<NoteEvent> {
    let mut notes = vec![note_on(0, 60, 100.0 / 127.0, None)];
    notes.extend_from_slice(expressions);
    let mut node = node(&registry(), &device(&mpe_control_type(), params), FRAMES);
    notes_through(node.as_mut(), &notes, FRAMES)
}

fn pressure_value(events: &[NoteEvent]) -> Option<f32> {
    events.iter().find_map(|event| match event.kind {
        NoteKind::Expression {
            expression: NoteExpression::Pressure,
            value,
        } => Some(value),
        _ => None,
    })
}

// Spec: mpe-shaper R1. With no curve, pressure comes out as it went in.
#[test]
fn pressure_passes_the_identity_curve_unchanged() {
    let out = shape_run(&[], &[expression(4, NoteExpression::Pressure, 0.25)]);
    assert!(
        (pressure_value(&out).unwrap() - 0.25).abs() < 1e-6,
        "{out:?}"
    );
}

// Spec: mpe-shaper R2. Min and Max place the output range.
#[test]
fn min_and_max_place_the_output_range() {
    let params = &[
        (source_min(PRESS as u32), 0.2),
        (source_max(PRESS as u32), 0.8),
    ];
    let low = shape_run(params, &[expression(4, NoteExpression::Pressure, 0.0)]);
    let high = shape_run(params, &[expression(4, NoteExpression::Pressure, 1.0)]);
    assert!((pressure_value(&low).unwrap() - 0.2).abs() < 1e-6);
    assert!((pressure_value(&high).unwrap() - 0.8).abs() < 1e-6);
}

// Spec: mpe-shaper R3. Curve A bends the linear curve.
#[test]
fn curve_a_bends_the_linear_curve() {
    let out = shape_run(
        &[(source_curve_a(PRESS as u32), 1.0)],
        &[expression(4, NoteExpression::Pressure, 0.25)],
    );
    assert!(
        (pressure_value(&out).unwrap() - 0.25f64.powf(0.25) as f32).abs() < 1e-6,
        "{out:?}"
    );
}

// Spec: mpe-shaper R4. The S-shaped curve's breakpoint joins two segments.
#[test]
fn the_s_curve_joins_two_segments() {
    let params = &[
        (source_curve(PRESS as u32), 1.0),
        (source_x(PRESS as u32), 0.5),
        (source_y(PRESS as u32), 0.25),
    ];
    let out = shape_run(params, &[expression(4, NoteExpression::Pressure, 0.25)]);
    // The first segment runs from 0 to (X, Y): 0.25 of the way is half of Y.
    assert!(
        (pressure_value(&out).unwrap() - 0.125).abs() < 1e-6,
        "{out:?}"
    );
}

// Spec: mpe-shaper R5. Press to AT converts pressure to channel pressure.
#[test]
fn press_to_at_converts_to_channel_pressure() {
    let out = shape_run(
        &[(PRESS_TO_AT, 1.0)],
        &[expression(4, NoteExpression::Pressure, 0.5)],
    );
    let value = out.iter().find_map(|event| match event.kind {
        NoteKind::ChannelPressure { value } => Some(value),
        _ => None,
    });
    assert!((value.unwrap() - 0.5).abs() < 1e-6, "{out:?}");
}

// Spec: mpe-shaper R6. Swap to Slide sends pressure as slide.
#[test]
fn swap_to_slide_sends_pressure_as_slide() {
    let out = shape_run(
        &[(SWAP_TO_SLIDE, 1.0)],
        &[expression(4, NoteExpression::Pressure, 0.4)],
    );
    let value = out.iter().find_map(|event| match event.kind {
        NoteKind::Expression {
            expression: NoteExpression::Slide,
            value,
        } => Some(value),
        _ => None,
    });
    assert!((value.unwrap() - 0.4).abs() < 1e-6, "{out:?}");
}

// Spec: mpe-shaper R7. Slide to Mod converts slide to CC 1.
#[test]
fn slide_to_mod_converts_to_the_mod_wheel() {
    let out = shape_run(
        &[(SLIDE_TO_MOD, 1.0)],
        &[expression(4, NoteExpression::Slide, 0.6)],
    );
    let value = out.iter().find_map(|event| match event.kind {
        NoteKind::Control {
            controller: 1,
            value,
        } => Some(value),
        _ => None,
    });
    assert!((value.unwrap() - 0.6).abs() < 1e-6, "{out:?}");
}

// Spec: mpe-shaper R8. Slide's Centered switch reads distance from the
// middle.
#[test]
fn centered_slide_reads_distance_from_the_middle() {
    let out = shape_run(
        &[(SLIDE_CENTERED, 1.0)],
        &[expression(4, NoteExpression::Slide, 0.25)],
    );
    let value = out.iter().find_map(|event| match event.kind {
        NoteKind::Expression {
            expression: NoteExpression::Slide,
            value,
        } => Some(value),
        _ => None,
    });
    assert!((value.unwrap() - 0.5).abs() < 1e-6, "{out:?}");
}

// Spec: mpe-shaper R9. NotePB's pitch range scales the bend, and NotePB to
// PB converts it to a channel bend.
#[test]
fn notepb_scales_the_bend_and_converts_to_pitch_bend() {
    let out = shape_run(
        &[(PITCH_RANGE, 2.0), (NOTEPB_TO_PB, 1.0)],
        &[expression(4, NoteExpression::Bend, 0.5)],
    );
    let value = out.iter().find_map(|event| match event.kind {
        NoteKind::PitchBend { value } => Some(value),
        _ => None,
    });
    assert!((value.unwrap() - 1.0).abs() < 1e-6, "{out:?}");
    // A quarter bend at the default range comes out as a quarter bend.
    let quarter = shape_run(
        &[(NOTEPB_TO_PB, 1.0)],
        &[expression(4, NoteExpression::Bend, 0.25)],
    );
    let value = quarter.iter().find_map(|event| match event.kind {
        NoteKind::PitchBend { value } => Some(value),
        _ => None,
    });
    assert!((value.unwrap() - 0.25).abs() < 1e-6, "{quarter:?}");
}

// Spec: mpe-shaper R10. A source that's off passes its data through.
#[test]
fn a_source_that_is_off_passes_through() {
    let incoming = expression(4, NoteExpression::Pressure, 0.3);
    let out = shape_run(&[(source_on(PRESS as u32), 0.0)], &[incoming]);
    assert!(out.contains(&incoming), "{out:?}");
}

// Spec: mpe-shaper R11. Smoothing moves toward the new value over its rise
// time.
#[test]
fn smoothing_moves_over_the_rise_time() {
    let params = &[
        (source_smooth(PRESS as u32), 1.0),
        (source_rise(PRESS as u32), 100.0),
    ];
    let out = shape_run(
        params,
        &[
            expression(0, NoteExpression::Pressure, 0.0),
            expression(48, NoteExpression::Pressure, 1.0),
        ],
    );
    let values: Vec<f32> = out
        .iter()
        .filter_map(|event| match event.kind {
            NoteKind::Expression {
                expression: NoteExpression::Pressure,
                value,
            } => Some(value),
            _ => None,
        })
        .collect();
    assert_eq!(values.len(), 2, "{out:?}");
    assert!(values[0] < 1e-6, "the first sets the start");
    assert!(values[1] > 0.0 && values[1] < 0.1, "{values:?}");
}

// Spec: mpe-shaper R12. Press's Default gives a note without pressure one on
// its frame.
#[test]
fn press_default_gives_a_note_pressure() {
    let out = shape_run(&[(PRESS_DEFAULT, 1.0), (PRESS_DEFAULT_VALUE, 0.5)], &[]);
    assert!(
        (pressure_value(&out).unwrap() - 0.5).abs() < 1e-6,
        "{out:?}"
    );
}

// Spec: mpe-shaper R13. Slide's Ons mode updates slide only when a note
// starts, and Slide's Default gives a note slide.
#[test]
fn slide_defaults_on_note_on() {
    let out = shape_run(
        &[
            (SLIDE_MODE, 2.0),
            (SLIDE_DEFAULT, 1.0),
            (SLIDE_DEFAULT_VALUE, 0.75),
        ],
        &[],
    );
    let value = out.iter().find_map(|event| match event.kind {
        NoteKind::Expression {
            expression: NoteExpression::Slide,
            value,
        } => Some(value),
        _ => None,
    });
    assert!((value.unwrap() - 0.75).abs() < 1e-6, "{out:?}");
}

// Spec: mpe-shaper R13. Slide's Ons mode sends the first slide a note
// receives and ignores the later ones, and invents no slide of its own.
#[test]
fn slide_ons_sends_the_first_slide_once() {
    let out = shape_run(
        &[(SLIDE_MODE, 2.0)],
        &[
            expression(0, NoteExpression::Slide, 0.25),
            expression(48, NoteExpression::Slide, 0.75),
        ],
    );
    let values: Vec<f32> = out
        .iter()
        .filter_map(|event| match event.kind {
            NoteKind::Expression {
                expression: NoteExpression::Slide,
                value,
            } => Some(value),
            _ => None,
        })
        .collect();
    assert_eq!(values.len(), 1, "{out:?}");
    assert!((values[0] - 0.25).abs() < 1e-6, "{values:?}");
    // A note that receives no slide gets none.
    let none = shape_run(&[(SLIDE_MODE, 2.0)], &[]);
    assert!(
        !none.iter().any(|event| matches!(
            event.kind,
            NoteKind::Expression {
                expression: NoteExpression::Slide,
                ..
            }
        )),
        "{none:?}"
    );
}

// Spec: mpe-shaper R15. Slide's Rel mode starts each note in the middle and
// moves by how far the finger has swept.
#[test]
fn slide_rel_starts_in_the_middle() {
    let out = shape_run(
        &[(SLIDE_MODE, 1.0)],
        &[
            expression(0, NoteExpression::Slide, 0.25),
            expression(48, NoteExpression::Slide, 0.75),
        ],
    );
    let values: Vec<f32> = out
        .iter()
        .filter_map(|event| match event.kind {
            NoteKind::Expression {
                expression: NoteExpression::Slide,
                value,
            } => Some(value),
            _ => None,
        })
        .collect();
    assert_eq!(values.len(), 2, "{out:?}");
    assert!((values[0] - 0.5).abs() < 1e-6, "{values:?}");
    assert!((values[1] - 1.0).abs() < 1e-6, "{values:?}");
}

// Spec: mpe-shaper R14. A channel message passes through.
#[test]
fn a_channel_message_passes_through() {
    let message = NoteEvent {
        offset: 4,
        kind: NoteKind::Control {
            controller: 7,
            value: 0.5,
        },
        channel: 0,
        key: 0,
        velocity: 0.0,
        note_id: None,
    };
    let notes = vec![note_on(0, 60, 100.0 / 127.0, None), message];
    let mut node = node(&registry(), &device(&mpe_control_type(), &[]), FRAMES);
    let out = notes_through(node.as_mut(), &notes, FRAMES);
    assert!(out.contains(&message));
}

// Spec: mpe-shaper R1
#[test]
fn the_zero_to_one_parameters_read_and_parse_as_unitless_fractions() {
    let description = mpe_control_type().description;
    let param = |name: &str| {
        description
            .params
            .iter()
            .find(|d| d.name == name)
            .unwrap_or_else(|| panic!("{name} is described"))
    };
    for name in [
        "Press Default Value",
        "Slide Default Value",
        "Press Max",
        "Press Curve A",
    ] {
        let d = param(name);
        assert!(!d.format(d.default).ends_with('%'), "{name} has no unit");
        assert_eq!(d.parse("0.5"), Some(0.5), "{name} parses 0.5 as half");
    }
    let d = param("Press Default Value");
    assert_eq!(d.format(d.default), "0.50");
}

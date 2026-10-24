//! The tool interface (the MIDI Tools spec, R5, R8, R9, R13, R16, R20 and
//! R26), through the test tools.
#![cfg(feature = "testing")]

use bl_core::time::{Ticks, TimeSignature};
use bl_midi_tools::{
    DIVISION_NAMES, MAX_NOTES, ParamValue, Registry, ToolError, ToolInput, ToolKind, ToolNote,
    ToolPoint, ToolScale, division_ticks, fold_octaves, run,
};

fn input(notes: Vec<ToolNote>, range: (i64, i64)) -> ToolInput {
    ToolInput {
        notes,
        range: (Ticks(range.0), Ticks(range.1)),
        grid_step: Ticks::SIXTEENTH,
        signature: TimeSignature::FOUR_FOUR,
        scale: None,
        drum_keys: None,
        seed: 1,
    }
}

fn note(key: i16, start: i64, length: i64) -> ToolNote {
    ToolNote::new(key, Ticks(start), Ticks(length), 100)
}

fn registry_tool(id: &str) -> &'static dyn bl_midi_tools::MidiTool {
    Registry::native().find(id).expect("a test tool")
}

#[test]
fn r5_a_descriptor_checks_values_against_its_parameters() {
    let tool = registry_tool("test_transpose");
    let descriptor = tool.descriptor();
    assert_eq!(descriptor.kind, ToolKind::Transformation);
    let defaults = descriptor.defaults();
    assert_eq!(descriptor.check(&defaults), Ok(()));
    let mut wrong = defaults.clone();
    wrong[1] = ParamValue::Whole(9);
    assert!(descriptor.check(&wrong).is_err(), "pieces tops out at 4");
    wrong[1] = ParamValue::Switch(true);
    assert!(
        descriptor.check(&wrong).is_err(),
        "a switch isn't a whole number"
    );
    assert!(
        descriptor.check(&defaults[..2]).is_err(),
        "one value per parameter"
    );
    assert!(descriptor.parameters[0].scale_aware);
}

#[test]
fn r3_the_registry_lists_each_kind_apart() {
    let registry = Registry::native();
    let transformations: Vec<_> = registry
        .tools(ToolKind::Transformation)
        .map(|tool| tool.descriptor().id.clone())
        .collect();
    let generators: Vec<_> = registry
        .tools(ToolKind::Generator)
        .map(|tool| tool.descriptor().id.clone())
        .collect();
    assert_eq!(
        transformations,
        [
            "arpeggiate",
            "chop",
            "connect",
            "ornament",
            "quantize",
            "test_transpose"
        ],
        "the native tools in the selector's order, then the test tool"
    );
    assert_eq!(generators, ["test_fill"]);
    assert!(
        registry
            .get(ToolKind::Generator, "test_transpose")
            .is_none()
    );
}

#[test]
fn r1_note_divisions_are_whole_note_fractions_and_triplets() {
    assert_eq!(division_ticks(0), Ticks::WHOLE);
    assert_eq!(Ticks::WHOLE, Ticks(2_822_400));
    assert_eq!(DIVISION_NAMES[7], "1/16");
    assert_eq!(division_ticks(7), Ticks::SIXTEENTH);
    assert_eq!(division_ticks(2), Ticks(1_411_200 * 2 / 3), "1/2T");
}

#[test]
fn r8_a_tool_counts_scale_degrees_and_a_chromatic_scale_counts_semitones() {
    let tool = registry_tool("test_transpose");
    let mut with = input(vec![note(60, 0, 1000)], (0, 1000));
    let values = [
        ParamValue::Whole(1),
        ParamValue::Whole(1),
        ParamValue::Whole(0),
    ];
    let out = run(tool, &with, &values).expect("runs");
    assert_eq!(out.notes[0].key, 61, "no scale: a degree is a semitone");
    with.scale = Some(ToolScale {
        root: 0,
        steps: vec![0, 2, 4, 5, 7, 9, 11],
    });
    let out = run(tool, &with, &values).expect("runs");
    assert_eq!(out.notes[0].key, 62, "C major: one degree up from C is D");
}

#[test]
fn r9_drum_keys_take_the_scales_place() {
    let tool = registry_tool("test_transpose");
    let mut with = input(vec![note(36, 0, 1000)], (0, 1000));
    with.drum_keys = Some(vec![36, 38, 42]);
    let values = [
        ParamValue::Whole(1),
        ParamValue::Whole(1),
        ParamValue::Whole(0),
    ];
    assert_eq!(run(tool, &with, &values).expect("runs").notes[0].key, 38);
    assert!(with.allows(42) && !with.allows(40));
    assert_eq!(with.nearest_key(40), 38, "the lower of two equally near");
    assert_eq!(with.step_key(42, 1), None, "past the last key");
    // The generator keeps to the keys too.
    let fill = registry_tool("test_fill");
    let generated = [
        ParamValue::Pitch(41),
        ParamValue::Division(7),
        ParamValue::Whole(50),
        ParamValue::Whole(0),
    ];
    let out = run(fill, &input_with_drums(), &generated).expect("runs");
    assert!(out.notes.iter().all(|note| note.key == 42));
}

fn input_with_drums() -> ToolInput {
    let mut with = input(Vec::new(), (0, 4 * 176_400));
    with.drum_keys = Some(vec![36, 42]);
    with
}

#[test]
fn r13_keys_fold_by_octaves_and_a_generator_is_cut_at_the_ranges_end() {
    assert_eq!(fold_octaves(130), 118);
    assert_eq!(fold_octaves(-1), 11);
    assert_eq!(fold_octaves(-12), 0);
    assert_eq!(fold_octaves(60), 60);
    let tool = registry_tool("test_transpose");
    let with = input(vec![note(125, 0, 1000)], (0, 1000));
    let values = [
        ParamValue::Whole(7),
        ParamValue::Whole(1),
        ParamValue::Whole(0),
    ];
    // 125 + 7 semitones is past the keyboard (step_key says so), so the tool
    // writes 132 and Beatlayer folds it.
    assert_eq!(run(tool, &with, &values).expect("runs").notes[0].key, 120);
    // A generator's last note is cut at the range's end.
    let fill = registry_tool("test_fill");
    let generated = [
        ParamValue::Pitch(60),
        ParamValue::Division(7),
        ParamValue::Whole(100),
        ParamValue::Whole(0),
    ];
    let range_end = 176_400 + 100_000;
    let out = run(fill, &input(Vec::new(), (0, range_end)), &generated).expect("runs");
    assert_eq!(out.notes.len(), 2);
    assert_eq!(out.notes[1].length, Ticks(100_000));
}

#[test]
fn r14_pieces_take_the_stretch_of_each_envelope_over_their_span() {
    let point = |at: i64, value: f64| ToolPoint {
        at: Ticks(at),
        value,
        bend: 0.0,
    };
    let mut held = note(60, 0, 1000);
    held.pressure = vec![point(0, 0.0), point(600, 60.0)];
    held.pitch = vec![point(0, 2.0)];
    let part = held.part(Ticks(500), Ticks(1000));
    assert_eq!(part.start, Ticks(500));
    assert_eq!(
        part.pressure,
        [point(0, 50.0), point(100, 60.0), point(500, 60.0)],
        "a point at each end of the part holds the value there"
    );
    assert_eq!(part.pitch, [point(0, 2.0), point(500, 2.0)]);
    assert!(part.slide.is_empty(), "an empty envelope stays empty");
    let shorter = held.with_length(Ticks(500));
    assert_eq!(
        shorter.pressure,
        [point(0, 0.0), point(500, 50.0)],
        "the point past the new end goes and one holds the value there"
    );
    assert_eq!(held.with_length(Ticks(2000)).pressure, held.pressure);
}

#[test]
fn r16_the_same_input_settings_and_seed_give_the_same_notes() {
    let fill = registry_tool("test_fill");
    let values = [
        ParamValue::Pitch(60),
        ParamValue::Division(7),
        ParamValue::Whole(50),
        ParamValue::Whole(30),
    ];
    let mut with = input(Vec::new(), (0, 8 * 176_400));
    let one = run(fill, &with, &values).expect("runs");
    assert_eq!(one, run(fill, &with, &values).expect("runs"));
    with.seed = 2;
    assert_ne!(
        one,
        run(fill, &with, &values).expect("runs"),
        "another seed"
    );
}

#[test]
fn r16_a_run_of_more_than_the_limit_is_refused() {
    let fill = registry_tool("test_fill");
    let values = [
        ParamValue::Pitch(60),
        ParamValue::Division(12),
        ParamValue::Whole(50),
        ParamValue::Whole(0),
    ];
    // A 1/128 step is 22,050 ticks: this many steps are one past the limit.
    let steps = i64::try_from(MAX_NOTES).expect("fits") + 1;
    let with = input(Vec::new(), (0, 22_050 * steps));
    assert!(matches!(
        run(fill, &with, &values),
        Err(ToolError::TooManyNotes(_))
    ));
}

#[test]
fn r3_the_selectors_list_the_tools_in_the_manuals_order() {
    assert_eq!(
        bl_midi_tools::TRANSFORMATION_ORDER,
        [
            "arpeggiate",
            "chop",
            "connect",
            "glissando",
            "lfo",
            "ornament",
            "quantize",
            "recombine",
            "span",
            "strum",
            "time_warp",
            "velocity_shaper"
        ]
    );
    assert_eq!(
        bl_midi_tools::GENERATOR_ORDER,
        ["rhythm", "seed", "shape", "stacks", "euclidean"]
    );
}

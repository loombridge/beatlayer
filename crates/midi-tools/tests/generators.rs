//! Rhythm, Seed, Shape and Euclidean (the MIDI generators spec, R1 to R16),
//! on fixed seeds, in and out of Scale Mode and with drum keys.

use std::collections::HashSet;

use bl_core::time::{Ticks, TimeSignature};
use bl_midi_tools::{
    EUCLIDEAN_VOICES, ParamValue, Registry, ToolInput, ToolNote, ToolScale, division_ticks, euclid,
    key_name, parse_key, pattern_count, preset_levels, random_rotations, rhythm_steps, run,
};
use proptest::prelude::*;

const BAR: i64 = 2_822_400;
const SIXTEENTH: i64 = 176_400;
const EIGHTH: i64 = 352_800;

fn input(range: (i64, i64), seed: u32) -> ToolInput {
    ToolInput {
        notes: Vec::new(),
        range: (Ticks(range.0), Ticks(range.1)),
        grid_step: Ticks(SIXTEENTH),
        signature: TimeSignature::FOUR_FOUR,
        scale: None,
        drum_keys: None,
        seed,
    }
}

fn c_major(mut input: ToolInput) -> ToolInput {
    input.scale = Some(ToolScale {
        root: 0,
        steps: vec![0, 2, 4, 5, 7, 9, 11],
    });
    input
}

fn drums(mut input: ToolInput) -> ToolInput {
    input.drum_keys = Some(vec![36, 38, 42, 46]);
    input
}

fn whole(n: i64) -> ParamValue {
    ParamValue::Whole(n)
}

fn division(name: &str) -> ParamValue {
    ParamValue::Division(
        bl_midi_tools::DIVISION_NAMES
            .iter()
            .position(|n| *n == name)
            .expect("a division"),
    )
}

/// The tool's defaults with these parameters changed, as the shell does:
/// the tool settles them and the descriptor checks them.
fn change(id: &str, changes: &[(&str, ParamValue)]) -> Result<Vec<ParamValue>, String> {
    let tool = Registry::native().find(id).expect("a tool");
    let descriptor = tool.descriptor();
    let before = descriptor.defaults();
    let mut after = before.clone();
    let mut changed = Vec::new();
    for (name, value) in changes {
        let (at, spec) = descriptor.parameter(name).expect("a parameter");
        spec.check(value)?;
        after[at] = value.clone();
        changed.push(at);
    }
    tool.settle(&before, &mut after, &changed)?;
    descriptor.check(&after).map_err(|e| e.to_string())?;
    Ok(after)
}

fn generate(id: &str, input: &ToolInput, changes: &[(&str, ParamValue)]) -> Vec<ToolNote> {
    let tool = Registry::native().find(id).expect("a tool");
    run(tool, input, &change(id, changes).expect("settles"))
        .expect("runs")
        .notes
}

fn starts(notes: &[ToolNote]) -> Vec<i64> {
    notes.iter().map(|n| n.start.0).collect()
}

fn hits(steps: &[bool]) -> Vec<usize> {
    steps
        .iter()
        .enumerate()
        .filter(|(_, hit)| **hit)
        .map(|(at, _)| at)
        .collect()
}

#[test]
fn r1_a_pitch_is_written_on_the_allowed_key_nearest_it() {
    // 61 is as near C3 as D3 in C major: the lower wins. Drum keys 38 and 42
    // are as near 40: the lower wins.
    let notes = generate(
        "rhythm",
        &c_major(input((0, BAR), 1)),
        &[("pitch", ParamValue::Pitch(61))],
    );
    assert!(notes.iter().all(|n| n.key == 60 && n.velocity == 100));
    let notes = generate(
        "rhythm",
        &drums(input((0, BAR), 1)),
        &[("pitch", ParamValue::Pitch(40))],
    );
    assert!(notes.iter().all(|n| n.key == 38));
    let notes = generate(
        "rhythm",
        &input((0, BAR), 1),
        &[("pitch", ParamValue::Pitch(127))],
    );
    assert!(notes.iter().all(|n| n.key == 127), "any key from 0 to 127");
}

#[test]
fn r3_a_pitch_has_a_name_and_takes_a_typed_name_or_number() {
    assert_eq!(key_name(60), "C3");
    assert_eq!(key_name(0), "C-2");
    assert_eq!(key_name(127), "G8");
    assert_eq!(parse_key("C3"), Some(60));
    assert_eq!(parse_key("c#3"), Some(61));
    assert_eq!(parse_key("Db3"), Some(61));
    assert_eq!(parse_key("bb2"), Some(58));
    assert_eq!(parse_key("C-2"), Some(0));
    assert_eq!(parse_key("60"), Some(60));
    for text in ["", "H3", "C", "128", "G#8", "C#", "3C"] {
        assert_eq!(parse_key(text), None, "{text:?}");
    }
    for key in 0..=127 {
        assert_eq!(parse_key(&key_name(key)), Some(key));
    }
}

#[test]
fn r4_rhythms_parameters_have_the_ranges_and_defaults_of_the_table() {
    let descriptor = Registry::native().find("rhythm").unwrap().descriptor();
    let ids: Vec<&str> = descriptor
        .parameters
        .iter()
        .map(|p| p.id.as_str())
        .collect();
    assert_eq!(
        ids,
        [
            "pitch",
            "steps",
            "density",
            "pattern",
            "step_duration",
            "split",
            "shift",
            "velocity",
            "accent",
            "accent_frequency",
            "accent_offset"
        ]
    );
    assert_eq!(
        descriptor.defaults(),
        [
            ParamValue::Pitch(60),
            whole(16),
            whole(4),
            whole(0),
            division("1/16"),
            whole(0),
            whole(0),
            whole(100),
            whole(127),
            whole(0),
            whole(0)
        ]
    );
    // Density stops at Steps, Accent Frequency at Density, and Accent Offset
    // one before Density; a value another's change leaves outside moves in.
    let values = change(
        "rhythm",
        &[("steps", whole(4)), ("accent_frequency", whole(2))],
    )
    .unwrap();
    assert_eq!(values[2], whole(4));
    let values = change(
        "rhythm",
        &[("density", whole(2)), ("accent_offset", whole(1))],
    )
    .unwrap();
    assert_eq!(
        (values[9].clone(), values[10].clone()),
        (whole(0), whole(1))
    );
    assert!(change("rhythm", &[("steps", whole(4)), ("density", whole(5))]).is_err());
    assert!(change("rhythm", &[("accent_frequency", whole(5))]).is_err());
    assert!(change("rhythm", &[("shift", whole(16))]).is_err());
}

#[test]
fn r5_pattern_0_is_the_even_spread_and_the_others_follow_in_step_order() {
    // Eight steps, three notes: 56 patterns, the even spread first.
    assert_eq!(pattern_count(8, 3), 56);
    assert_eq!(hits(&rhythm_steps(8, 3, 0, 0)), [0, 3, 6]);
    assert_eq!(hits(&euclid(8, 3, 0)), [0, 3, 6]);
    assert_eq!(hits(&rhythm_steps(8, 3, 1, 0)), [0, 1, 2]);
    assert_eq!(hits(&rhythm_steps(8, 3, 2, 0)), [0, 1, 3]);
    assert_eq!(hits(&rhythm_steps(8, 3, 55, 0)), [5, 6, 7]);
    let all: HashSet<Vec<usize>> = (0..56).map(|p| hits(&rhythm_steps(8, 3, p, 0))).collect();
    assert_eq!(all.len(), 56, "every pattern is a different choice");
    assert!(all.iter().all(|p| p.len() == 3));
    // Shift goes round the steps.
    assert_eq!(hits(&rhythm_steps(8, 3, 0, 1)), [1, 4, 7]);
    assert_eq!(hits(&rhythm_steps(8, 3, 0, -1)), [2, 5, 7]);
    assert_eq!(hits(&rhythm_steps(8, 3, 0, 8 + 1)), [1, 4, 7]);
    // Density 0 and Density Steps have one pattern.
    assert_eq!(pattern_count(8, 0), 1);
    assert_eq!(pattern_count(8, 8), 1);
    assert_eq!(pattern_count(16, 8), 12_870);
    // The pattern is the one Euclidean writes for the same numbers.
    let rhythm = generate(
        "rhythm",
        &input((0, 2 * BAR), 1),
        &[("steps", whole(16)), ("density", whole(5))],
    );
    let euclidean = generate(
        "euclidean",
        &input((0, 2 * BAR), 1),
        &[("steps", whole(16)), ("density", whole(5))],
    );
    let drop_key = |notes: &[ToolNote]| starts(notes);
    assert_eq!(drop_key(&rhythm), drop_key(&euclidean));
}

#[test]
fn r6_the_pattern_repeats_across_the_range_and_a_split_halves_a_step() {
    // The manual's example: a one-bar range, 8 steps: once at 1/8, twice at
    // 1/16.
    let range = input((0, BAR), 1);
    let at_eighths = generate(
        "rhythm",
        &range,
        &[
            ("steps", whole(8)),
            ("density", whole(3)),
            ("step_duration", division("1/8")),
        ],
    );
    assert_eq!(starts(&at_eighths), [0, 3 * EIGHTH, 6 * EIGHTH]);
    assert!(at_eighths.iter().all(|n| n.length.0 == EIGHTH));
    let at_sixteenths = generate(
        "rhythm",
        &range,
        &[("steps", whole(8)), ("density", whole(3))],
    );
    assert_eq!(
        starts(&at_sixteenths),
        [
            0,
            3 * SIXTEENTH,
            6 * SIXTEENTH,
            8 * SIXTEENTH,
            11 * SIXTEENTH,
            14 * SIXTEENTH
        ]
    );
    // More steps than the range holds act as the steps it holds.
    let acting = generate(
        "rhythm",
        &range,
        &[
            ("steps", whole(16)),
            ("density", whole(4)),
            ("step_duration", division("1/8")),
        ],
    );
    let eight = generate(
        "rhythm",
        &range,
        &[
            ("steps", whole(8)),
            ("density", whole(4)),
            ("step_duration", division("1/8")),
        ],
    );
    assert_eq!(starts(&acting), starts(&eight));
    // A part pattern at the end: a bar and a half at 8 steps of 1/8.
    let part = generate(
        "rhythm",
        &input((0, BAR + BAR / 2), 1),
        &[
            ("steps", whole(8)),
            ("density", whole(2)),
            ("step_duration", division("1/8")),
        ],
    );
    // The second pattern's second note would start at the range's end.
    assert_eq!(starts(&part), [0, 4 * EIGHTH, BAR]);
    // A range shorter than a step still holds one step.
    assert_eq!(
        generate("rhythm", &input((0, SIXTEENTH / 2), 1), &[]).len(),
        1
    );
    // Split 100 %: every chosen step is two notes of half a step.
    let split = generate(
        "rhythm",
        &range,
        &[
            ("steps", whole(8)),
            ("density", whole(3)),
            ("split", whole(100)),
            ("step_duration", division("1/8")),
        ],
    );
    assert_eq!(
        starts(&split),
        [
            0,
            EIGHTH / 2,
            3 * EIGHTH,
            3 * EIGHTH + EIGHTH / 2,
            6 * EIGHTH,
            6 * EIGHTH + EIGHTH / 2
        ]
    );
    assert!(split.iter().all(|n| n.length.0 == EIGHTH / 2));
    // The same seed splits the same steps.
    let some = [("split", whole(50))];
    assert_eq!(
        generate("rhythm", &input((0, 4 * BAR), 5), &some),
        generate("rhythm", &input((0, 4 * BAR), 5), &some)
    );
    let with_split = generate("rhythm", &input((0, 4 * BAR), 5), &some).len();
    assert!(with_split > 16 && with_split < 32, "{with_split}");
}

#[test]
fn r7_accents_count_the_steps_that_write_from_the_ranges_start() {
    let velocities = |changes: &[(&str, ParamValue)]| -> Vec<u8> {
        generate("rhythm", &input((0, BAR), 1), changes)
            .iter()
            .map(|n| n.velocity)
            .collect()
    };
    let base = [
        ("steps", whole(8)),
        ("density", whole(4)),
        ("velocity", whole(60)),
        ("accent", whole(120)),
    ];
    let with = |extra: (&'static str, ParamValue)| -> Vec<(&'static str, ParamValue)> {
        base.iter().cloned().chain([extra]).collect()
    };
    assert!(velocities(&base).iter().all(|v| *v == 60), "no accents");
    // Frequency 1 accents every note.
    assert!(
        velocities(&with(("accent_frequency", whole(1))))
            .iter()
            .all(|v| *v == 120)
    );
    // Frequency 2: the first, third, fifth ... note, across both repeats of
    // a half-bar pattern.
    assert_eq!(
        velocities(&with(("accent_frequency", whole(2)))),
        [120, 60, 120, 60, 120, 60, 120, 60]
    );
    // The offset moves which notes take it.
    let mut offset = with(("accent_frequency", whole(2)));
    offset.push(("accent_offset", whole(1)));
    assert_eq!(velocities(&offset), [60, 120, 60, 120, 60, 120, 60, 120]);
    // Frequency 3 across the repeat: notes 0, 3 and 6.
    assert_eq!(
        velocities(&with(("accent_frequency", whole(3)))),
        [120, 60, 60, 120, 60, 60, 120, 60]
    );
    // A split step's two notes are accented together.
    let mut split = with(("accent_frequency", whole(2)));
    split.push(("split", whole(100)));
    let notes = generate("rhythm", &input((0, BAR), 1), &split);
    assert_eq!(
        notes.iter().map(|n| n.velocity).collect::<Vec<_>>(),
        [
            120, 120, 60, 60, 120, 120, 60, 60, 120, 120, 60, 60, 120, 120, 60, 60
        ]
    );
}

#[test]
fn r8_seeds_ranges_keep_their_lowest_below_their_highest() {
    let descriptor = Registry::native().find("seed").unwrap().descriptor();
    let defaults = descriptor.defaults();
    assert_eq!(defaults[0], ParamValue::PitchRange { low: 48, high: 72 });
    let get = |values: &[ParamValue], id: &str| values[descriptor.parameter(id).unwrap().0].clone();
    assert_eq!(get(&defaults, "duration_min"), whole(8));
    assert_eq!(get(&defaults, "duration_max"), whole(32));
    assert_eq!(get(&defaults, "velocity_min"), whole(64));
    assert_eq!(get(&defaults, "velocity_max"), whole(127));
    assert_eq!(get(&defaults, "voices"), whole(4));
    assert_eq!(get(&defaults, "density"), whole(50));
    // The end that was changed pushes the other.
    let values = change("seed", &[("duration_min", whole(64))]).unwrap();
    assert_eq!(
        (get(&values, "duration_min"), get(&values, "duration_max")),
        (whole(64), whole(64))
    );
    let values = change("seed", &[("velocity_max", whole(10))]).unwrap();
    assert_eq!(
        (get(&values, "velocity_min"), get(&values, "velocity_max")),
        (whole(10), whole(10))
    );
    // The handles may meet in one pitch.
    let one = ParamValue::PitchRange { low: 60, high: 60 };
    let notes = generate(
        "seed",
        &input((0, BAR), 3),
        &[("pitch_range", one), ("density", whole(100))],
    );
    assert!(!notes.is_empty() && notes.iter().all(|n| n.key == 60));
    assert!(
        change(
            "seed",
            &[("pitch_range", ParamValue::PitchRange { low: 70, high: 60 })]
        )
        .is_err()
    );
}

#[test]
fn r9_seed_makes_density_times_the_keys_for_each_whole_note() {
    // The notes are 1/128 long, so nothing is left out for overlapping.
    let tiny = [
        ("pitch_range", ParamValue::PitchRange { low: 60, high: 71 }),
        ("duration_min", whole(1)),
        ("duration_max", whole(1)),
        ("voices", whole(16)),
        ("density", whole(100)),
    ];
    assert_eq!(generate("seed", &input((0, BAR), 7), &tiny).len(), 12);
    assert_eq!(generate("seed", &input((0, 2 * BAR), 7), &tiny).len(), 24);
    let half: Vec<_> = tiny
        .iter()
        .filter(|(n, _)| *n != "density")
        .cloned()
        .chain([("density", whole(50))])
        .collect();
    assert_eq!(generate("seed", &input((0, BAR), 7), &half).len(), 6);
    // In C major, the octave 60 to 71 holds seven keys.
    assert_eq!(
        generate("seed", &c_major(input((0, BAR), 7)), &tiny).len(),
        7
    );
    // Density 0 writes nothing.
    assert!(generate("seed", &input((0, BAR), 7), &[("density", whole(0))]).is_empty());
    // The same seed, the same notes; another seed, others.
    let a = generate("seed", &input((0, BAR), 1), &[]);
    assert_eq!(a, generate("seed", &input((0, BAR), 1), &[]));
    assert_ne!(
        starts(&a),
        starts(&generate("seed", &input((0, BAR), 2), &[]))
    );
    // Starts fall on the grid; keys, lengths and velocities in their ranges.
    for n in generate("seed", &input((0, 4 * BAR), 11), &[("density", whole(100))]) {
        assert_eq!(n.start.0 % SIXTEENTH, 0);
        assert!((48..=72).contains(&n.key));
        assert!((64..=127).contains(&n.velocity));
        assert!(n.length.0 >= BAR / 128 || n.end().0 == 4 * BAR, "{n:?}");
    }
}

fn overlap(a: &ToolNote, b: &ToolNote) -> bool {
    a.start < b.end() && b.start < a.end()
}

fn most_at_once(notes: &[ToolNote]) -> usize {
    notes
        .iter()
        .map(|n| {
            notes
                .iter()
                .filter(|m| m.start <= n.start && n.start < m.end())
                .count()
        })
        .max()
        .unwrap_or(0)
}

#[test]
fn r10_seed_keeps_to_its_voices_and_never_overlaps_a_key_with_itself() {
    let dense = [
        ("pitch_range", ParamValue::PitchRange { low: 60, high: 63 }),
        ("duration_min", whole(32)),
        ("duration_max", whole(64)),
        ("density", whole(100)),
    ];
    for voices in [1, 2, 3] {
        let mut changes = dense.to_vec();
        changes.push(("voices", whole(voices)));
        let notes = generate("seed", &input((0, 2 * BAR), 4), &changes);
        assert!(!notes.is_empty());
        assert!(most_at_once(&notes) <= voices as usize, "{voices} voices");
        for (i, a) in notes.iter().enumerate() {
            for b in &notes[i + 1..] {
                assert!(a.key != b.key || !overlap(a, b), "{a:?} {b:?}");
            }
        }
    }
    // One voice, many wanted notes: some are left out after their redraws.
    let mut one = dense.to_vec();
    one.push(("voices", whole(1)));
    let wanted = 4 * 2; // four keys, two whole notes, Density 100 %
    assert!(generate("seed", &input((0, 2 * BAR), 4), &one).len() < wanted * 2);
    // A note that would run past the end is cut there.
    let long = [
        ("duration_min", whole(128)),
        ("duration_max", whole(128)),
        ("voices", whole(16)),
        ("density", whole(100)),
    ];
    let notes = generate("seed", &input((0, BAR), 9), &long);
    assert!(notes.iter().all(|n| n.end().0 <= BAR));
    assert!(notes.iter().any(|n| n.length.0 < BAR), "cut at the end");
}

#[test]
fn r11_shape_has_six_presets_of_sixteen_levels_and_drawing_reads_user() {
    let descriptor = Registry::native().find("shape").unwrap().descriptor();
    let (_, preset) = descriptor.parameter("preset").unwrap();
    let bl_midi_tools::ParamKind::Choice { options } = &preset.kind else {
        panic!("a choice")
    };
    let ids: Vec<&str> = options.iter().map(|o| o.id.as_str()).collect();
    assert_eq!(
        ids,
        ["rise", "fall", "arch", "valley", "wave", "steps", "user"]
    );
    let near = |a: f64, b: f64| (a - b).abs() < 1e-9;
    let rise = preset_levels(0);
    assert_eq!(rise.len(), 16);
    assert!(near(rise[0], 0.0) && near(rise[15], 1.0) && near(rise[3], 0.2));
    assert!(near(preset_levels(1)[3], 0.8));
    assert!(near(preset_levels(2)[0], 0.0) && near(preset_levels(2)[15], 0.0));
    assert!(near(preset_levels(3)[0], 1.0));
    assert!(near(preset_levels(4)[0], 0.5) && near(preset_levels(4)[4], 1.0));
    assert!(
        near(preset_levels(5)[3], 1.0)
            && near(preset_levels(5)[4], 0.0)
            && near(preset_levels(5)[5], 1.0 / 3.0)
    );
    // Choosing a preset writes its levels; drawing sets them and the preset
    // reads User, which keeps the levels it is chosen with.
    let values = change("shape", &[("preset", ParamValue::Choice(2))]).unwrap();
    assert_eq!(values[1], ParamValue::Levels(preset_levels(2)));
    let mut drawn = preset_levels(0);
    drawn[5] = 0.9;
    let values = change("shape", &[("levels", ParamValue::Levels(drawn.clone()))]).unwrap();
    assert_eq!(
        (values[0].clone(), values[1].clone()),
        (ParamValue::Choice(6), ParamValue::Levels(drawn.clone()))
    );
    let values = change("shape", &[("preset", ParamValue::Choice(6))]).unwrap();
    assert_eq!(values[1], ParamValue::Levels(preset_levels(0)));
    let both = change(
        "shape",
        &[
            ("preset", ParamValue::Choice(2)),
            ("levels", ParamValue::Levels(drawn.clone())),
        ],
    )
    .unwrap();
    assert_eq!(both[1], ParamValue::Levels(drawn));
    assert_eq!(
        descriptor.defaults()[2],
        ParamValue::PitchRange { low: 48, high: 72 }
    );
    assert!(change("shape", &[("levels", ParamValue::Levels(vec![0.0; 15]))]).is_err());
}

#[test]
fn r12_shape_gives_each_slot_the_level_of_its_place_in_the_range() {
    // Four slots of a quarter note over a bar, Rise from C2 to C4: the slots
    // sit 0, 1/4, 1/2 and 3/4 of the way, at levels 0, .25, .5 and .75.
    let rise = [("rate", division("1/4"))];
    let notes = generate("shape", &input((0, BAR), 1), &rise);
    assert_eq!(
        notes.iter().map(|n| n.key).collect::<Vec<_>>(),
        [48, 54, 60, 66]
    );
    assert!(
        notes
            .iter()
            .all(|n| n.velocity == 100 && n.length.0 == BAR / 4)
    );
    assert_eq!(starts(&notes), [0, BAR / 4, BAR / 2, 3 * BAR / 4]);
    let fall = [("rate", division("1/4")), ("preset", ParamValue::Choice(1))];
    let keys: Vec<i16> = generate("shape", &input((0, BAR), 1), &fall)
        .iter()
        .map(|n| n.key)
        .collect();
    assert_eq!(keys, [72, 66, 60, 54]);
    // Density picks that share of the slots, always the same ones for a seed.
    let sparse = [("density", whole(25))];
    let notes = generate("shape", &input((0, BAR), 6), &sparse);
    assert_eq!(notes.len(), 4);
    assert_eq!(notes, generate("shape", &input((0, BAR), 6), &sparse));
    assert!(generate("shape", &input((0, BAR), 6), &[("density", whole(0))]).is_empty());
    assert_eq!(generate("shape", &input((0, BAR), 6), &[]).len(), 16);
    assert_eq!(
        generate("shape", &input((0, BAR), 6), &[("rate", division("1/8"))]).len(),
        8
    );
    // The nearest key the generator may write.
    let notes = generate("shape", &c_major(input((0, BAR), 1)), &rise);
    assert_eq!(
        notes.iter().map(|n| n.key).collect::<Vec<_>>(),
        [48, 53, 60, 65]
    );
    let notes = generate("shape", &drums(input((0, BAR), 1)), &rise);
    assert!(
        notes
            .iter()
            .all(|n| [36, 38, 42, 46].contains(&(n.key as u8)))
    );
    // Jitter moves pitches but keeps them in the range.
    let jittery = [
        ("jitter", whole(100)),
        ("pitch_range", ParamValue::PitchRange { low: 60, high: 64 }),
    ];
    let notes = generate("shape", &input((0, 4 * BAR), 2), &jittery);
    assert!(notes.iter().all(|n| (60..=64).contains(&n.key)));
    let plain = generate(
        "shape",
        &input((0, 4 * BAR), 2),
        &[("pitch_range", ParamValue::PitchRange { low: 60, high: 64 })],
    );
    assert_ne!(notes, plain);
}

#[test]
fn r13_a_tie_lasts_into_the_next_slot_which_stays_empty() {
    let tied = [("rate", division("1/4")), ("tie", whole(100))];
    let notes = generate("shape", &input((0, BAR), 1), &tied);
    assert_eq!(starts(&notes), [0, BAR / 2]);
    assert!(notes.iter().all(|n| n.length.0 == BAR / 2));
    // The last slot has no next to tie into.
    let notes = generate("shape", &input((0, 3 * BAR / 4), 1), &tied);
    assert_eq!(starts(&notes), [0, BAR / 2]);
    assert_eq!(notes[1].length.0, BAR / 4);
    // No tie, no long notes.
    assert!(
        generate("shape", &input((0, BAR), 1), &[("tie", whole(0))])
            .iter()
            .all(|n| n.length.0 == SIXTEENTH)
    );
}

#[test]
fn r14_euclideans_voices_and_parameters_have_the_defaults_of_the_table() {
    let descriptor = Registry::native().find("euclidean").unwrap().descriptor();
    let get = |id: &str| descriptor.defaults()[descriptor.parameter(id).unwrap().0].clone();
    assert_eq!(
        (get("steps"), get("density"), get("division")),
        (whole(16), whole(4), division("1/16"))
    );
    for (voice, pitch) in [36u8, 38, 42, 46].into_iter().enumerate() {
        let n = voice + 1;
        assert_eq!(get(&format!("voice_{n}")), ParamValue::Switch(voice == 0));
        assert_eq!(get(&format!("pitch_{n}")), ParamValue::Pitch(pitch));
        assert_eq!(get(&format!("velocity_{n}")), whole(100));
        assert_eq!(get(&format!("rotation_{n}")), whole(0));
    }
    // Density stops at Steps, and a Rotation at the last step.
    assert!(change("euclidean", &[("steps", whole(4)), ("density", whole(5))]).is_err());
    let values = change(
        "euclidean",
        &[("steps", whole(4)), ("rotation_2", whole(3))],
    )
    .unwrap();
    assert_eq!(values[descriptor.parameter("density").unwrap().0], whole(4));
    let values = change("euclidean", &[("steps", whole(2))]).unwrap();
    assert_eq!(values[descriptor.parameter("density").unwrap().0], whole(2));
    assert!(
        change(
            "euclidean",
            &[("steps", whole(4)), ("rotation_1", whole(4))]
        )
        .is_err()
    );
}

#[test]
fn r15_density_hits_are_spread_as_evenly_as_they_go() {
    assert_eq!(hits(&euclid(16, 4, 0)), [0, 4, 8, 12]);
    assert_eq!(hits(&euclid(8, 3, 0)), [0, 3, 6]);
    assert_eq!(hits(&euclid(16, 5, 0)), [0, 4, 7, 10, 13]);
    assert_eq!(hits(&euclid(16, 4, 2)), [2, 6, 10, 14]);
    assert_eq!(hits(&euclid(8, 3, 1)), [1, 4, 7]);
    assert_eq!(hits(&euclid(8, 3, 7)), [2, 5, 7]);
    assert!(hits(&euclid(8, 0, 0)).is_empty());
    assert_eq!(hits(&euclid(8, 8, 3)).len(), 8);
    for steps in 1..=32 {
        for density in 0..=steps {
            assert_eq!(hits(&euclid(steps, density, 0)).len(), density);
            assert_eq!(hits(&euclid(steps, density, steps / 2)).len(), density);
        }
    }
}

#[test]
fn r16_each_switched_on_voice_repeats_its_pattern_from_the_ranges_start() {
    // Voice 1 alone: four notes a bar at 36, then the pattern again.
    let notes = generate("euclidean", &input((0, 2 * BAR), 1), &[]);
    assert_eq!(
        starts(&notes),
        [0, 4, 8, 12, 16, 20, 24, 28].map(|n| n * SIXTEENTH)
    );
    assert!(
        notes
            .iter()
            .all(|n| n.key == 36 && n.velocity == 100 && n.length.0 == SIXTEENTH)
    );
    // Four voices, each on its pitch, velocity and rotation.
    let all = [
        ("voice_2", ParamValue::Switch(true)),
        ("voice_3", ParamValue::Switch(true)),
        ("voice_4", ParamValue::Switch(true)),
        ("rotation_2", whole(1)),
        ("velocity_3", whole(50)),
        ("pitch_4", ParamValue::Pitch(49)),
    ];
    let notes = generate("euclidean", &input((0, BAR), 1), &all);
    assert_eq!(notes.len(), 16);
    let of = |key: i16| -> Vec<i64> {
        notes
            .iter()
            .filter(|n| n.key == key)
            .map(|n| n.start.0 / SIXTEENTH)
            .collect()
    };
    assert_eq!(
        (of(36), of(38), of(42), of(49)),
        (
            vec![0, 4, 8, 12],
            vec![1, 5, 9, 13],
            vec![0, 4, 8, 12],
            vec![0, 4, 8, 12]
        )
    );
    assert!(
        notes
            .iter()
            .filter(|n| n.key == 42)
            .all(|n| n.velocity == 50)
    );
    // A pattern of 8 steps of 1/8 and a part pattern at the end.
    let eights = [
        ("steps", whole(8)),
        ("density", whole(3)),
        ("division", division("1/8")),
    ];
    let notes = generate("euclidean", &input((0, BAR + BAR / 2), 1), &eights);
    assert_eq!(starts(&notes), [0, 3, 6, 8, 11].map(|n| n * EIGHTH));
    // Every hit lands in the range, and the Drum Rack's keys take the pitch.
    assert!(
        generate(
            "euclidean",
            &drums(input((0, BAR), 1)),
            &[("pitch_1", ParamValue::Pitch(37))]
        )
        .iter()
        .all(|n| n.key == 36)
    );
    assert!(
        generate(
            "euclidean",
            &c_major(input((0, BAR), 1)),
            &[("pitch_1", ParamValue::Pitch(37))]
        )
        .iter()
        .all(|n| n.key == 36)
    );
}

#[test]
fn r16_the_randomise_button_draws_every_rotation_from_the_seed() {
    let a = random_rotations(16, 7);
    assert_eq!(a, random_rotations(16, 7));
    assert_ne!(a, random_rotations(16, 8));
    assert_eq!(a.len(), EUCLIDEAN_VOICES);
    for seed in 0..200 {
        assert!(random_rotations(5, seed).iter().all(|r| (0..5).contains(r)));
    }
    assert_eq!(random_rotations(1, 3), [0; 4]);
}

#[test]
fn a_run_that_would_write_too_many_notes_is_refused() {
    let tool = Registry::native().find("euclidean").unwrap();
    let huge = input((0, 4000 * BAR), 1);
    let values = change(
        "euclidean",
        &[
            ("steps", whole(1)),
            ("density", whole(1)),
            ("division", division("1/128")),
        ],
    )
    .unwrap();
    assert!(run(tool, &huge, &values).is_err());
    assert_eq!(division_ticks(12).0, BAR / 128);
}

fn setup() -> impl Strategy<Value = (u32, i64, u8)> {
    (any::<u32>(), 1..5i64, 0..3u8)
}

fn world(seed: u32, bars: i64, kind: u8) -> ToolInput {
    let base = input((0, bars * BAR), seed);
    match kind {
        0 => base,
        1 => c_major(base),
        _ => drums(base),
    }
}

proptest! {
    #[test]
    fn every_generated_note_lies_in_its_range_and_its_allowed_keys(
        (seed, bars, kind) in setup(),
        low in 0..100u8,
        width in 0..28u8,
        voices in 1..16i64,
        density in 0..=100i64,
    ) {
        let world = world(seed, bars, kind);
        let range = ParamValue::PitchRange { low, high: low + width };
        let seeded = generate("seed", &world, &[("pitch_range", range.clone()), ("voices", whole(voices)), ("density", whole(density))]);
        for n in &seeded {
            prop_assert!(world.allows(n.key as u8));
            prop_assert!(n.key >= i16::from(low) && n.key <= i16::from(low + width));
            prop_assert!(n.start.0 >= 0 && n.end().0 <= bars * BAR);
        }
        prop_assert!(most_at_once(&seeded) <= voices as usize, "Seed never exceeds Voices");
        for (i, a) in seeded.iter().enumerate() {
            for b in &seeded[i + 1..] {
                prop_assert!(a.key != b.key || !overlap(a, b), "Seed overlaps a key with itself");
            }
        }
        // Shape, with the range's ends on keys it may write.
        let (lo, hi) = (world.nearest_key(low), world.nearest_key(low + width));
        let (lo, hi) = (lo.min(hi), lo.max(hi));
        let shaped = generate("shape", &world, &[("pitch_range", ParamValue::PitchRange { low: lo, high: hi }), ("jitter", whole(density)), ("tie", whole(density))]);
        for n in &shaped {
            prop_assert!(world.allows(n.key as u8));
            prop_assert!(n.key >= i16::from(lo) && n.key <= i16::from(hi), "{n:?} in {lo}..{hi}");
            prop_assert!(n.end().0 <= bars * BAR);
        }
    }

    #[test]
    fn euclidean_writes_exactly_density_hits_per_pattern_and_rhythm_density_notes(
        (seed, bars, kind) in setup(),
        steps in 1..=16i64,
        density_share in 0..=100i64,
        rotation in 0..16i64,
    ) {
        let world = world(seed, bars, kind);
        let density = steps * density_share / 100;
        let rotation = rotation % steps;
        // One pattern of 1/16 steps fits `bars * 16 / steps` times when
        // Steps divides 16; use a range of whole patterns.
        let range = input((0, steps * SIXTEENTH * 3), seed);
        let world = ToolInput { range: range.range, ..world };
        let notes = generate("euclidean", &world, &[("steps", whole(steps)), ("density", whole(density)), ("rotation_1", whole(rotation))]);
        prop_assert_eq!(notes.len() as i64, density * 3);
        prop_assert!(notes.iter().all(|n| world.allows(n.key as u8)));
        let notes = generate("rhythm", &world, &[("steps", whole(steps)), ("density", whole(density)), ("split", whole(0))]);
        prop_assert_eq!(notes.len() as i64, density * 3, "before splits");
        let notes = generate("rhythm", &world, &[("steps", whole(steps)), ("density", whole(density)), ("split", whole(100))]);
        prop_assert_eq!(notes.len() as i64, density * 6);
        prop_assert!(notes.iter().all(|n| world.allows(n.key as u8)));
    }

    #[test]
    fn every_rhythm_pattern_holds_density_steps(steps in 1..=12usize, share in 0..=100usize, pick in 0..100_000usize, shift in -20..20i64) {
        let density = steps * share / 100;
        let pattern = rhythm_steps(steps, density, pick, shift);
        prop_assert_eq!(pattern.len(), steps);
        prop_assert_eq!(hits(&pattern).len(), density);
    }
}

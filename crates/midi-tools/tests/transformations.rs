//! Arpeggiate, Chop, Connect, Ornament and Quantize (the MIDI
//! transformations spec, R1 to R19 and R43's Chop), on generated notes with
//! fixed seeds.

use bl_core::time::{Ticks, TimeSignature};
use bl_midi_tools::{
    ParamValue, Registry, ToolInput, ToolNote, ToolPoint, ToolScale, gaps_pattern, run,
    division_ticks,
};
use proptest::prelude::*;

const S: i64 = 176_400; // a 1/16 note
const BAR: i64 = 2_822_400;

fn input(notes: Vec<ToolNote>, range: (i64, i64)) -> ToolInput {
    ToolInput {
        notes,
        range: (Ticks(range.0), Ticks(range.1)),
        grid_step: Ticks(S),
        signature: TimeSignature::FOUR_FOUR,
        scale: None,
        drum_keys: None,
        seed: 1,
    }
}

fn note(key: i16, start: i64, length: i64) -> ToolNote {
    ToolNote::new(key, Ticks(start), Ticks(length), 100)
}

fn major() -> Option<ToolScale> {
    Some(ToolScale {
        root: 0,
        steps: vec![0, 2, 4, 5, 7, 9, 11],
    })
}

/// The tool's defaults with these parameters changed.
fn settings(id: &str, changes: &[(&str, ParamValue)]) -> Vec<ParamValue> {
    let descriptor = Registry::native().find(id).expect("a tool").descriptor();
    let mut values = descriptor.defaults();
    for (name, value) in changes {
        let (at, _) = descriptor.parameter(name).expect("a parameter");
        values[at] = value.clone();
    }
    values
}

fn apply(id: &str, input: &ToolInput, changes: &[(&str, ParamValue)]) -> Vec<ToolNote> {
    let tool = Registry::native().find(id).expect("a tool");
    run(tool, input, &settings(id, changes)).expect("runs").notes
}

fn placed(notes: &[ToolNote]) -> Vec<(i64, i16)> {
    notes.iter().map(|n| (n.start.0, n.key)).collect()
}

fn whole(n: i64) -> ParamValue {
    ParamValue::Whole(n)
}

fn style(name: &str) -> ParamValue {
    let descriptor = Registry::native().find("arpeggiate").unwrap().descriptor();
    let (_, spec) = descriptor.parameter("style").unwrap();
    let bl_midi_tools::ParamKind::Choice { options } = &spec.kind else {
        panic!("a choice")
    };
    ParamValue::Choice(options.iter().position(|o| o.id == name).expect("a style"))
}

#[test]
fn r2_ids_are_names_in_lower_case_and_a_length_keeps_a_tick() {
    let registry = Registry::native();
    let ids = |tool: &str| -> Vec<String> {
        registry
            .find(tool)
            .unwrap()
            .descriptor()
            .parameters
            .iter()
            .map(|p| p.id.clone())
            .collect()
    };
    assert_eq!(ids("arpeggiate"), ["style", "distance", "steps", "rate", "gate"]);
    assert_eq!(
        ids("chop"),
        ["parts", "gaps", "pattern", "emphasis", "stretch_chunks", "variation"]
    );
    assert_eq!(ids("connect"), ["spread", "density", "rate", "tie"]);
    assert_eq!(
        ids("ornament"),
        [
            "mode",
            "flam_position",
            "flam_velocity",
            "grace_notes_pitch",
            "grace_notes_position",
            "grace_notes_velocity",
            "grace_notes_chance",
            "grace_notes_amount"
        ]
    );
    assert_eq!(ids("quantize"), ["start", "end", "grid", "triplets", "amount"]);
    // A gate of 1 % of a 1/128 note rounds to a tick at least.
    let out = apply(
        "arpeggiate",
        &input(vec![note(60, 0, S)], (0, S)),
        &[("rate", ParamValue::Division(12)), ("gate", whole(1))],
    );
    assert!(out.iter().all(|n| n.length >= Ticks(1)));
}

#[test]
fn r3_arpeggiates_parameters_have_the_rules_ranges_and_defaults() {
    let d = Registry::native().find("arpeggiate").unwrap().descriptor();
    let defaults = d.defaults();
    assert_eq!(defaults[0], ParamValue::Choice(0), "Up");
    assert_eq!(defaults[1], whole(12));
    assert_eq!(defaults[2], whole(0));
    assert_eq!(defaults[3], ParamValue::Division(7), "1/16");
    assert_eq!(defaults[4], whole(100));
    assert!(d.parameters[1].scale_aware);
    assert!(d.check(&settings("arpeggiate", &[("distance", whole(48))])).is_ok());
    assert!(d.check(&settings("arpeggiate", &[("distance", whole(49))])).is_err());
    assert!(d.check(&settings("arpeggiate", &[("steps", whole(9))])).is_err());
    assert!(d.check(&settings("arpeggiate", &[("gate", whole(201))])).is_err());
    assert!(d.check(&settings("arpeggiate", &[("gate", whole(0))])).is_err());
    let bl_midi_tools::ParamKind::Choice { options } = &d.parameters[0].kind else {
        panic!("a choice")
    };
    assert_eq!(options.len(), 18, "Arpeggiator's styles");
}

#[test]
fn r4_overlapping_notes_make_a_group_and_a_step_with_nothing_held_writes_nothing() {
    // Two chords 8 steps apart: each plays its own arpeggio, and the steps
    // between them write nothing.
    let notes = vec![
        note(60, 0, 2 * S),
        note(64, 0, 2 * S),
        note(67, 8 * S, 2 * S),
    ];
    let out = apply("arpeggiate", &input(notes, (0, 10 * S)), &[]);
    assert_eq!(
        placed(&out),
        [(0, 60), (S, 64), (8 * S, 67), (9 * S, 67)],
        "the second group starts at its own earliest start"
    );
}

#[test]
fn r4_a_step_holds_only_the_notes_that_sound_then() {
    // C holds for 4 steps, E joins at step 2 and leaves at step 3.
    let notes = vec![note(60, 0, 4 * S), note(64, 2 * S, S)];
    let out = apply("arpeggiate", &input(notes, (0, 4 * S)), &[]);
    // Place 0 of [60], 1 of [60]: order [0] is length 1; at step 2 held
    // [60, 64], place 0 -> 60; step 3 held [60], place 0 -> 60.
    assert_eq!(placed(&out), [(0, 60), (S, 60), (2 * S, 60), (3 * S, 60)]);
    let out = apply(
        "arpeggiate",
        &input(vec![note(60, 0, 4 * S), note(64, 0, 4 * S)], (0, 4 * S)),
        &[],
    );
    assert_eq!(placed(&out), [(0, 60), (S, 64), (2 * S, 60), (3 * S, 64)]);
}

#[test]
fn r5_steps_raise_the_cycle_each_time_the_place_goes_round() {
    let chord = vec![note(60, 0, 6 * S), note(64, 0, 6 * S), note(67, 0, 6 * S)];
    let out = apply(
        "arpeggiate",
        &input(chord.clone(), (0, 6 * S)),
        &[("steps", whole(1)), ("distance", whole(12))],
    );
    let keys: Vec<i16> = out.iter().map(|n| n.key).collect();
    assert_eq!(keys, [60, 64, 67, 72, 76, 79]);
    // Back to cycle 0 after Steps.
    let out = apply(
        "arpeggiate",
        &input(
            vec![note(60, 0, 8 * S), note(64, 0, 8 * S), note(67, 0, 8 * S)],
            (0, 8 * S),
        ),
        &[("steps", whole(1))],
    );
    assert_eq!(
        out.iter().map(|n| n.key).collect::<Vec<_>>(),
        [60, 64, 67, 72, 76, 79, 60, 64]
    );
    // Distance counts degrees where the clip has a scale.
    let mut with = input(vec![note(60, 0, 4 * S)], (0, 4 * S));
    with.scale = major();
    let out = apply("arpeggiate", &with, &[("steps", whole(1)), ("distance", whole(1))]);
    assert_eq!(
        out.iter().map(|n| n.key).collect::<Vec<_>>(),
        [60, 62, 60, 62],
        "a degree of C major above C is D"
    );
}

#[test]
fn r5_the_styles_order_the_held_notes_as_arpeggiator_does() {
    let chord = |len| vec![note(60, 0, len), note(64, 0, len), note(67, 0, len), note(71, 0, len)];
    let keys = |name: &str, steps: i64| -> Vec<i16> {
        apply(
            "arpeggiate",
            &input(chord(steps * S), (0, steps * S)),
            &[("style", style(name))],
        )
        .iter()
        .map(|n| n.key)
        .collect()
    };
    assert_eq!(keys("up", 4), [60, 64, 67, 71]);
    assert_eq!(keys("down", 4), [71, 67, 64, 60]);
    assert_eq!(keys("updown", 6), [60, 64, 67, 71, 67, 64]);
    assert_eq!(keys("converge", 4), [60, 71, 64, 67]);
    assert_eq!(keys("thumb_up", 6), [60, 64, 60, 67, 60, 71]);
    assert_eq!(keys("pinky_up", 6), [71, 60, 71, 64, 71, 67]);
    // Play Order: earliest start first, then lowest key.
    let staggered = vec![note(67, 0, 4 * S), note(60, 0, 4 * S)];
    let out = apply(
        "arpeggiate",
        &input(staggered, (0, 2 * S)),
        &[("style", style("play_order"))],
    );
    assert_eq!(out.iter().map(|n| n.key).collect::<Vec<_>>(), [60, 67, 60, 67]);
}

#[test]
fn r5_chord_trigger_writes_every_held_note_and_the_random_styles_follow_the_seed() {
    let chord = vec![note(60, 0, 2 * S), note(64, 0, 2 * S)];
    let out = apply(
        "arpeggiate",
        &input(chord, (0, 2 * S)),
        &[("style", style("chord_trigger"))],
    );
    assert_eq!(placed(&out), [(0, 60), (0, 64), (S, 60), (S, 64)]);
    let many: Vec<ToolNote> = (0..5).map(|k| note(60 + 2 * k, 0, 40 * S)).collect();
    for name in ["random", "random_other", "random_once"] {
        let with = input(many.clone(), (0, 40 * S));
        let one = apply("arpeggiate", &with, &[("style", style(name))]);
        assert_eq!(one, apply("arpeggiate", &with, &[("style", style(name))]), "{name}");
        let mut other = with.clone();
        other.seed = 99;
        assert_ne!(
            one,
            apply("arpeggiate", &other, &[("style", style(name))]),
            "{name}: another seed"
        );
    }
    // Random Other never plays the note of the step before.
    let out = apply(
        "arpeggiate",
        &input(many.clone(), (0, 40 * S)),
        &[("style", style("random_other"))],
    );
    assert!(out.windows(2).all(|pair| pair[0].key != pair[1].key));
    // Random Once plays one order, whole, over and over.
    let out = apply(
        "arpeggiate",
        &input(many, (0, 40 * S)),
        &[("style", style("random_once"))],
    );
    let keys: Vec<i16> = out.iter().map(|n| n.key).collect();
    assert_eq!(keys[..5], keys[5..10]);
    let mut sorted = keys[..5].to_vec();
    sorted.sort_unstable();
    assert_eq!(sorted, [60, 62, 64, 66, 68]);
}

#[test]
fn r6_a_step_lasts_rate_times_gate_and_takes_its_held_notes_values() {
    let mut held = note(60, 0, 2 * S);
    held.velocity = 77;
    held.chance = 0.5;
    held.velocity_deviation = 9;
    held.release_velocity = 33;
    held.deactivated = true;
    let out = apply(
        "arpeggiate",
        &input(vec![held], (0, 2 * S)),
        &[("gate", whole(150))],
    );
    assert_eq!(out.len(), 2);
    assert_eq!(out[0].length, Ticks(S * 3 / 2), "overlaps the next step");
    assert_eq!(
        (out[0].velocity, out[0].chance, out[0].velocity_deviation, out[0].release_velocity, out[0].deactivated),
        (77, 0.5, 9, 33, true)
    );
    // Keys past the keyboard move back by octaves.
    let tool = Registry::native().find("arpeggiate").unwrap();
    let values = settings("arpeggiate", &[("steps", whole(1)), ("distance", whole(7))]);
    let high = input(vec![note(125, 0, 2 * S)], (0, 2 * S));
    let out = run(tool, &high, &values).unwrap().notes;
    assert_eq!(out.iter().map(|n| n.key).collect::<Vec<_>>(), [125, 120]);
}

#[test]
fn r6_a_step_carries_the_stretch_of_its_notes_envelopes() {
    let mut held = note(60, 0, 4 * S);
    held.pressure = vec![
        ToolPoint { at: Ticks(0), value: 0.0, bend: 0.0 },
        ToolPoint { at: Ticks(4 * S), value: 100.0, bend: 0.0 },
    ];
    let out = apply("arpeggiate", &input(vec![held], (0, 4 * S)), &[]);
    assert_eq!(out[1].pressure[0].value, 25.0, "the value at the step");
    assert_eq!(out[1].pressure.last().unwrap().value, 50.0, "and at its end");
}

#[test]
fn r5_and_r9_arpeggiate_stays_on_a_drum_racks_keys() {
    let mut with = input(
        vec![note(36, 0, 6 * S), note(38, 0, 6 * S), note(42, 0, 6 * S)],
        (0, 6 * S),
    );
    with.drum_keys = Some(vec![36, 38, 42, 46]);
    let out = apply("arpeggiate", &with, &[("steps", whole(1)), ("distance", whole(1))]);
    assert_eq!(
        out.iter().map(|n| n.key).collect::<Vec<_>>(),
        [36, 38, 42, 38, 42, 46],
        "a step counts through the keys"
    );
    assert!(out.iter().all(|n| with.allows(u8::try_from(n.key).unwrap())));
}

proptest! {
    #[test]
    fn r6_arpeggiates_notes_start_on_its_steps_and_stay_on_the_keyboard(
        keys in proptest::collection::vec(0i16..128, 1..6),
        len in 1i64..40,
        steps in 0i64..9,
        distance in -48i64..49,
        style_at in 0usize..18,
        rate in 3usize..13,
    ) {
        let notes: Vec<ToolNote> = keys.iter().map(|k| note(*k, 0, len * S / 4)).collect();
        let with = input(notes, (0, len * S));
        let tool = Registry::native().find("arpeggiate").unwrap();
        let values = settings("arpeggiate", &[
            ("style", ParamValue::Choice(style_at)),
            ("steps", whole(steps)),
            ("distance", whole(distance)),
            ("rate", ParamValue::Division(rate)),
        ]);
        let out = run(tool, &with, &values).unwrap().notes;
        let step = division_ticks(rate).0;
        for n in &out {
            prop_assert_eq!(n.start.0 % step, 0, "on a step");
            prop_assert!((0..=127).contains(&n.key));
            prop_assert!(n.length >= Ticks(1));
        }
    }
}

// Chop.

#[test]
fn r7_chops_parameters_have_the_rules_ranges_and_defaults() {
    let d = Registry::native().find("chop").unwrap().descriptor();
    let defaults = d.defaults();
    assert_eq!(defaults[0], whole(2));
    assert_eq!(defaults[1], whole(0));
    assert_eq!(defaults[2], ParamValue::Toggles(vec![true; 16]), "as Gaps writes it");
    assert_eq!(defaults[3], ParamValue::Toggles(vec![false; 16]));
    assert_eq!(defaults[4], whole(2));
    assert_eq!(defaults[5], whole(0));
    assert!(d.check(&settings("chop", &[("parts", whole(64))])).is_ok());
    assert!(d.check(&settings("chop", &[("parts", whole(65))])).is_err());
    assert!(d.check(&settings("chop", &[("stretch_chunks", whole(9))])).is_err());
}

#[test]
fn r8_gaps_write_the_pattern_and_a_toggle_leaves_gaps_without_a_number() {
    let on = |p: &[bool]| p.iter().take(6).copied().collect::<Vec<_>>();
    assert_eq!(on(&gaps_pattern(0)), [true; 6]);
    assert_eq!(on(&gaps_pattern(2)), [true, true, false, true, true, false], "g parts before each gap");
    assert_eq!(on(&gaps_pattern(-2)), [true, false, false, true, false, false], "a part, then g gaps");
    let tool = Registry::native().find("chop").unwrap();
    let before = settings("chop", &[("parts", whole(6))]);
    // Setting Gaps writes the pattern.
    let mut after = before.clone();
    after[1] = whole(2);
    tool.settle(&before, &mut after, &[1]).unwrap();
    assert_eq!(after[2], ParamValue::Toggles(gaps_pattern(2)));
    // A pattern set in the same change stands.
    let mut both = before.clone();
    both[1] = whole(2);
    both[2] = ParamValue::Toggles(vec![false, true, true, true, true, true, true, true, true, true, true, true, true, true, true, true]);
    let want = both[2].clone();
    tool.settle(&before, &mut both, &[1, 2]).unwrap();
    assert_eq!(both[2], want);
    // Gaps beyond what Parts allows are refused.
    let mut too_many = before.clone();
    too_many[0] = whole(2);
    too_many[1] = whole(3);
    assert!(tool.settle(&before, &mut too_many, &[1]).is_err());
    // The last part can't be switched to a gap.
    let mut none = before.clone();
    none[2] = ParamValue::Toggles(vec![false; 16]);
    assert!(tool.settle(&before, &mut none, &[2]).is_err());
    // An emphasised element keeps its Pattern state when Gaps writes.
    let mut kept = before.clone();
    kept[2] = ParamValue::Toggles({
        let mut p = vec![true; 16];
        p[1] = false;
        p
    });
    kept[3] = ParamValue::Toggles({
        let mut e = vec![false; 16];
        e[1] = true;
        e
    });
    let before_kept = kept.clone();
    kept[1] = whole(0);
    tool.settle(&before_kept, &mut kept, &[1]).unwrap();
    assert_eq!(kept[2], before_kept[2], "element 1 is emphasised");
}

#[test]
fn r9_pieces_come_in_proportion_to_their_weights_and_gaps_write_nothing() {
    let with = input(vec![note(60, 1000, 4000)], (0, 10_000));
    let out = apply("chop", &with, &[("parts", whole(4))]);
    assert_eq!(
        out.iter().map(|n| (n.start.0, n.length.0)).collect::<Vec<_>>(),
        [(1000, 1000), (2000, 1000), (3000, 1000), (4000, 1000)]
    );
    // Parts 4 with Gaps 1: part, gap, part, gap.
    let pattern = ParamValue::Toggles(gaps_pattern(1));
    let out = apply("chop", &with, &[("parts", whole(4)), ("gaps", whole(1)), ("pattern", pattern)]);
    assert_eq!(placed(&out), [(1000, 60), (3000, 60)]);
    // Emphasis stretches: weights 1 and 3.
    let mut emphasis = vec![false; 16];
    emphasis[1] = true;
    let out = apply(
        "chop",
        &with,
        &[("parts", whole(2)), ("emphasis", ParamValue::Toggles(emphasis)), ("stretch_chunks", whole(3))],
    );
    assert_eq!(
        out.iter().map(|n| (n.start.0, n.length.0)).collect::<Vec<_>>(),
        [(1000, 1000), (2000, 3000)]
    );
    // The pattern repeats past 16 parts; 32 parts of a gap-every-other pattern.
    let long = input(vec![note(60, 0, 3200)], (0, 3200));
    let out = apply(
        "chop",
        &long,
        &[("parts", whole(32)), ("gaps", whole(1)), ("pattern", ParamValue::Toggles(gaps_pattern(1)))],
    );
    assert_eq!(out.len(), 16);
    assert_eq!(out[15].start, Ticks(3000));
    // A piece shorter than a tick is left out.
    let tiny = input(vec![note(60, 0, 3)], (0, 3));
    assert_eq!(apply("chop", &tiny, &[("parts", whole(8))]).len(), 3);
    // Values of the input note are kept.
    let mut loud = note(60, 0, 100);
    loud.velocity = 42;
    loud.chance = 0.25;
    let out = apply("chop", &input(vec![loud], (0, 100)), &[]);
    assert!(out.iter().all(|n| n.velocity == 42 && n.chance == 0.25));
}

#[test]
fn r10_variation_moves_starts_and_ends_within_the_note() {
    let with = input(vec![note(60, 1000, 4000)], (0, 10_000));
    let straight = apply("chop", &with, &[("parts", whole(8))]);
    let varied = apply("chop", &with, &[("parts", whole(8)), ("variation", whole(100))]);
    assert_ne!(straight, varied);
    assert_eq!(varied, apply("chop", &with, &[("parts", whole(8)), ("variation", whole(100))]));
    for n in &varied {
        assert!(n.start >= Ticks(1000) && n.end() <= Ticks(5000) && n.length >= Ticks(1));
    }
    // Each end moves by at most v x half its piece (250 ticks of a 500-tick piece).
    for (a, b) in straight.iter().zip(&varied) {
        assert!((a.start.0 - b.start.0).abs() <= 250 && (a.end().0 - b.end().0).abs() <= 250);
    }
}

proptest! {
    #[test]
    fn r9_chops_pieces_tile_each_note_exactly(
        length in 1i64..2_000_000,
        parts in 2i64..65,
        stretch in 2i64..9,
        emphasis in proptest::collection::vec(any::<bool>(), 16),
    ) {
        let with = input(vec![note(60, 5000, length)], (0, 10_000_000));
        let out = apply("chop", &with, &[
            ("parts", whole(parts)),
            ("stretch_chunks", whole(stretch)),
            ("emphasis", ParamValue::Toggles(emphasis)),
        ]);
        // Every piece of a note with no gaps is a note: they meet end to start and fill it.
        let mut at = 5000;
        for n in &out {
            prop_assert_eq!(n.start.0, at);
            at = n.end().0;
        }
        prop_assert_eq!(at, 5000 + length);
    }
}

// Connect.

#[test]
fn r11_connects_parameters_have_the_rules_ranges_and_defaults() {
    let d = Registry::native().find("connect").unwrap().descriptor();
    assert_eq!(d.defaults(), [whole(0), whole(100), ParamValue::Division(7), whole(0)]);
    assert!(d.parameters[0].scale_aware);
    assert!(d.check(&settings("connect", &[("spread", whole(13))])).is_err());
}

#[test]
fn r12_and_r13_a_gap_gets_slots_whose_keys_follow_the_line_between_the_notes() {
    let with = input(vec![note(60, 0, S), note(64, 4 * S, S)], (0, 5 * S));
    let out = apply("connect", &with, &[]);
    assert_eq!(out.len(), 5, "the input is kept and three slots are filled");
    let added = &out[2..];
    assert_eq!(
        added.iter().map(|n| (n.start.0, n.key, n.length.0)).collect::<Vec<_>>(),
        [(S, 62, S), (2 * S, 63, S), (3 * S, 64, S)]
    );
    // Density picks that share of the slots; 0 picks none.
    assert_eq!(apply("connect", &with, &[("density", whole(0))]).len(), 2);
    assert_eq!(apply("connect", &with, &[("density", whole(34))]).len(), 3, "one of three");
    // Tie: the first filled note lasts to the next start and the rest stay empty.
    let out = apply("connect", &with, &[("tie", whole(100))]);
    assert_eq!(out.len(), 3);
    assert_eq!(out[2].length, Ticks(3 * S));
    // Velocity of the highest note starting at s, and its other values.
    let mut loud = note(60, 0, S);
    loud.velocity = 88;
    loud.chance = 0.5;
    let with = input(vec![loud, note(64, 4 * S, S)], (0, 5 * S));
    let out = apply("connect", &with, &[]);
    assert!(out[2..5].iter().all(|n| n.velocity == 88 && n.chance == 0.5));
    // A spread moves keys by up to that many steps and follows the seed.
    let spread = apply("connect", &with, &[("spread", whole(3))]);
    assert_eq!(spread, apply("connect", &with, &[("spread", whole(3))]));
    assert!(spread[2..5].iter().all(|n| (n.key - 63).abs() <= 3 + 1));
    // A gap with no time in it gets nothing.
    let touching = input(vec![note(60, 0, S), note(64, S, S)], (0, 2 * S));
    assert_eq!(apply("connect", &touching, &[]).len(), 2);
}

#[test]
fn r13_connect_counts_steps_in_a_scale_and_on_a_drum_rack() {
    let mut with = input(vec![note(60, 0, S), note(67, 8 * S, S)], (0, 9 * S));
    with.scale = major();
    // C to G is four degrees; seven slots, the key at each middle in degrees.
    let out = apply("connect", &with, &[]);
    for n in &out {
        assert!(with.allows(u8::try_from(n.key).unwrap()), "{}", n.key);
    }
    let mut drums = input(vec![note(36, 0, S), note(46, 8 * S, S)], (0, 9 * S));
    drums.drum_keys = Some(vec![36, 38, 42, 46]);
    let out = apply("connect", &drums, &[("spread", whole(2))]);
    assert!(out.len() > 2);
    assert!(out.iter().all(|n| drums.allows(u8::try_from(n.key).unwrap())));
}

proptest! {
    #[test]
    fn r12_connect_never_adds_a_note_outside_a_gap(
        starts in proptest::collection::vec((0i64..40, 1i64..6, 0i16..128), 1..8),
        density in 0i64..101,
        tie in 0i64..101,
        rate in 3usize..11,
        seed in any::<u32>(),
    ) {
        let notes: Vec<ToolNote> = starts.iter().map(|(s, l, k)| note(*k, s * S / 2, l * S / 2)).collect();
        let mut with = input(notes.clone(), (0, 100 * S));
        with.seed = seed;
        let out = apply("connect", &with, &[("density", whole(density)), ("tie", whole(tie)), ("rate", ParamValue::Division(rate))]);
        prop_assert_eq!(&out[..notes.len()], &notes[..]);
        for added in &out[notes.len()..] {
            // Inside the gap after some start: after the latest end of the notes starting there and before the next start.
            let before: Vec<&ToolNote> = notes.iter().filter(|n| n.start <= added.start).collect();
            let s = before.iter().map(|n| n.start).max().expect("a start before");
            let end = before.iter().filter(|n| n.start == s).map(|n| n.end()).max().unwrap();
            let next = notes.iter().map(|n| n.start).filter(|t| *t > s).min().expect("a start after");
            prop_assert!(added.start >= end && added.end() <= next);
        }
    }
}

// Ornament.

#[test]
fn r14_ornaments_parameters_have_the_rules_ranges_and_defaults() {
    let d = Registry::native().find("ornament").unwrap().descriptor();
    assert_eq!(
        d.defaults(),
        [
            ParamValue::Choice(0),
            whole(-50),
            whole(50),
            ParamValue::Choice(0),
            whole(-50),
            whole(50),
            whole(100),
            whole(2)
        ]
    );
    assert!(d.check(&settings("ornament", &[("flam_position", whole(-101))])).is_err());
    assert!(d.check(&settings("ornament", &[("flam_velocity", whole(0))])).is_err());
    assert!(d.check(&settings("ornament", &[("grace_notes_amount", whole(9))])).is_err());
}

#[test]
fn r15_a_flam_ends_where_the_note_starts_or_takes_over_its_start() {
    let with = input(vec![note(60, 4 * S, 2 * S)], (0, 8 * S));
    let out = apply("ornament", &with, &[]);
    let flam = &out[0];
    assert_eq!((flam.start.0, flam.length.0, flam.key, flam.velocity), (4 * S - S / 2, S / 2, 60, 50));
    assert_eq!((out[1].start.0, out[1].length.0), (4 * S, 2 * S), "the note stays");
    // Positive: the flam starts where the note did and the note starts later.
    let out = apply("ornament", &with, &[("flam_position", whole(100))]);
    assert_eq!((out[0].start.0, out[0].length.0), (4 * S, S));
    assert_eq!((out[1].start.0, out[1].end().0), (5 * S, 6 * S), "its end stays put");
    // At most the note's length less a tick.
    let short = input(vec![note(60, 4 * S, 100)], (0, 8 * S));
    let out = apply("ornament", &short, &[("flam_position", whole(100))]);
    assert_eq!((out[0].length.0, out[1].length.0), (99, 1));
    // 0 % adds nothing.
    assert_eq!(apply("ornament", &with, &[("flam_position", whole(0))]), with.notes);
    // A flam before tick 0 is cut at 0.
    let early = input(vec![note(60, S / 4, S)], (0, 4 * S));
    let out = apply("ornament", &early, &[]);
    assert_eq!((out[0].start.0, out[0].length.0), (0, S / 4));
}

#[test]
fn r16_grace_notes_fill_the_span_and_alternate_the_note_and_its_step() {
    let with = input(vec![note(60, 4 * S, 2 * S)], (0, 8 * S));
    let mode = ("mode", ParamValue::Choice(1));
    let out = apply("ornament", &with, &[mode.clone(), ("grace_notes_position", whole(-100)), ("grace_notes_amount", whole(3))]);
    // Three grace notes of a third of a step... filling one step, then the note.
    assert_eq!(out.len(), 4);
    assert_eq!(out[0].start.0, 3 * S);
    assert_eq!(out[2].end().0, 4 * S);
    // Counting back: next to the note a step above, then its key, then a step above.
    assert_eq!(out.iter().map(|n| n.key).collect::<Vec<_>>(), [61, 60, 61, 60]);
    let low = apply("ornament", &with, &[mode.clone(), ("grace_notes_pitch", ParamValue::Choice(1)), ("grace_notes_position", whole(-100))]);
    assert_eq!(low.iter().map(|n| n.key).collect::<Vec<_>>(), [60, 59, 60]);
    let same = apply("ornament", &with, &[mode.clone(), ("grace_notes_pitch", ParamValue::Choice(2)), ("grace_notes_position", whole(-100))]);
    assert!(same.iter().all(|n| n.key == 60));
    // Velocity and chance relative to the note's.
    let mut n = note(60, 4 * S, 2 * S);
    n.velocity = 80;
    n.chance = 0.8;
    let out = apply(
        "ornament",
        &input(vec![n], (0, 8 * S)),
        &[mode, ("grace_notes_velocity", whole(25)), ("grace_notes_chance", whole(50))],
    );
    assert_eq!(out[0].velocity, 20);
    assert!((out[0].chance - 0.4).abs() < 1e-9);
    // In C major a step above C is D.
    let mut scaled = input(vec![note(60, 4 * S, 2 * S)], (0, 8 * S));
    scaled.scale = major();
    let out = apply("ornament", &scaled, &[("mode", ParamValue::Choice(1))]);
    assert_eq!(out[1].key, 62);
}

#[test]
fn r17_applying_ornament_again_ornaments_the_notes_as_they_are() {
    let with = input(vec![note(60, 4 * S, 2 * S)], (0, 8 * S));
    let once = apply("ornament", &with, &[]);
    assert_eq!(once.len(), 2);
    let twice = apply("ornament", &input(once, (0, 8 * S)), &[]);
    assert_eq!(twice.len(), 4, "the flam gets a flam too");
}

// Quantize.

#[test]
fn r18_quantizes_parameters_have_the_rules_defaults() {
    let d = Registry::native().find("quantize").unwrap().descriptor();
    assert_eq!(
        d.defaults(),
        [ParamValue::Switch(true), ParamValue::Switch(false), ParamValue::Choice(0), ParamValue::Switch(false), whole(100)]
    );
    assert!(d.check(&settings("quantize", &[("amount", whole(101))])).is_err());
}

#[test]
fn r19_quantize_moves_starts_and_ends_toward_the_lines() {
    let with = input(vec![note(60, S + 1000, S - 3000)], (0, 8 * S));
    // The editor's grid (a 1/16) by default: the start lands on a line, the length stays.
    let out = apply("quantize", &with, &[]);
    assert_eq!((out[0].start.0, out[0].length.0), (S, S - 3000));
    // Amount moves part of the way.
    let out = apply("quantize", &with, &[("amount", whole(50))]);
    assert_eq!(out[0].start.0, S + 500);
    // Ends: the start stays put and the end lands on its line.
    let out = apply("quantize", &with, &[("start", ParamValue::Switch(false)), ("end", ParamValue::Switch(true))]);
    assert_eq!((out[0].start.0, out[0].end().0), (S + 1000, 2 * S));
    // Both: the start, then the end.
    let out = apply("quantize", &with, &[("end", ParamValue::Switch(true))]);
    assert_eq!((out[0].start.0, out[0].end().0), (S, 2 * S));
    // Both off changes nothing.
    let off = apply("quantize", &with, &[("start", ParamValue::Switch(false))]);
    assert_eq!(off, with.notes);
    // A named grid and triplets: lines a 1/8 triplet apart, laid out from the bar.
    let third = BAR / 8 * 2 / 3;
    let out = apply("quantize", &input(vec![note(60, third + 100, 1000)], (0, BAR)), &[("grid", ParamValue::Choice(4)), ("triplets", ParamValue::Switch(true))]);
    assert_eq!(out[0].start.0, third);
    // The current grid ignores Triplets.
    assert_eq!(
        apply("quantize", &with, &[("triplets", ParamValue::Switch(true))]),
        apply("quantize", &with, &[])
    );
    // Whole bars count from the contents' start.
    let bars = apply("quantize", &input(vec![note(60, BAR + 100, 1000)], (0, 4 * BAR)), &[("grid", ParamValue::Choice(1))]);
    assert_eq!(bars[0].start.0, BAR);
}

#[test]
fn r19_an_end_whose_line_is_at_or_before_the_start_goes_to_the_first_line_after_it() {
    let with = input(vec![note(60, 10, 20)], (0, 8 * S));
    let out = apply("quantize", &with, &[("start", ParamValue::Switch(false)), ("end", ParamValue::Switch(true))]);
    assert_eq!((out[0].start.0, out[0].end().0), (10, S));
    // A shortened note ends its envelopes at its new end.
    let mut long = note(60, 0, 2 * S - 500);
    long.pressure = vec![ToolPoint { at: Ticks(2 * S - 500), value: 9.0, bend: 0.0 }];
    let out = apply("quantize", &input(vec![long], (0, 8 * S)), &[("start", ParamValue::Switch(false)), ("end", ParamValue::Switch(true))]);
    assert_eq!(out[0].length.0, 2 * S);
}

proptest! {
    #[test]
    fn r19_at_100_percent_every_start_is_on_a_line(
        starts in proptest::collection::vec((0i64..20_000_000, 1i64..2_000_000), 1..10),
        grid in 0usize..7,
        triplets in any::<bool>(),
    ) {
        let notes: Vec<ToolNote> = starts.iter().map(|(s, l)| note(60, *s, *l)).collect();
        let out = apply("quantize", &input(notes, (0, 30_000_000)), &[("grid", ParamValue::Choice(grid)), ("triplets", ParamValue::Switch(triplets))]);
        let step = bl_midi_tools::grid_ticks(grid, triplets && grid > 0, Ticks(S)).0;
        for n in &out {
            // Lines are laid out from each bar's start, and a bar's end is a line too.
            let into = n.start.0.rem_euclid(BAR);
            prop_assert!(into % step == 0 || into == 0, "start {} step {}", n.start.0, step);
        }
    }
}

#[test]
fn r2_a_note_keeps_a_tick_and_ids_name_the_tools() {
    let registry = Registry::native();
    for id in ["arpeggiate", "chop", "connect", "ornament", "quantize"] {
        assert_eq!(registry.find(id).unwrap().descriptor().id, id);
    }
}

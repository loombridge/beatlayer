//! Glissando and LFO (the MIDI transformations spec, R36 to R42), on
//! generated notes with fixed seeds.

use bl_core::time::{Ticks, TimeSignature};
use bl_midi_tools::{ParamValue, Registry, Rng, ToolError, ToolInput, ToolNote, ToolPoint, run};

/// A 1/8 note, LFO's default period.
const EIGHTH: i64 = 352_800;

fn input(notes: Vec<ToolNote>) -> ToolInput {
    ToolInput {
        notes,
        range: (Ticks::ZERO, Ticks(10_000_000)),
        grid_step: Ticks(176_400),
        signature: TimeSignature::FOUR_FOUR,
        scale: None,
        drum_keys: None,
        seed: 1,
    }
}

fn note(key: i16, start: i64, length: i64) -> ToolNote {
    ToolNote::new(key, Ticks(start), Ticks(length), 100)
}

fn settings(id: &str, changes: &[(&str, ParamValue)]) -> Vec<ParamValue> {
    let descriptor = Registry::native().find(id).expect("a tool").descriptor();
    let mut values = descriptor.defaults();
    for (name, value) in changes {
        let (at, spec) = descriptor.parameter(name).expect("a parameter");
        values[at] = match (&spec.kind, value) {
            (bl_midi_tools::ParamKind::Choice { options }, ParamValue::Structured(id)) => {
                ParamValue::Choice(options.iter().position(|o| &o.id == id).expect("an option"))
            }
            _ => value.clone(),
        };
    }
    values
}

fn try_apply(
    id: &str,
    input: &ToolInput,
    changes: &[(&str, ParamValue)],
) -> Result<Vec<ToolNote>, ToolError> {
    let tool = Registry::native().find(id).expect("a tool");
    run(tool, input, &settings(id, changes)).map(|out| out.notes)
}

fn apply(id: &str, input: &ToolInput, changes: &[(&str, ParamValue)]) -> Vec<ToolNote> {
    try_apply(id, input, changes).expect("runs")
}

fn whole(n: i64) -> ParamValue {
    ParamValue::Whole(n)
}

fn number(n: f64) -> ParamValue {
    ParamValue::Number(n)
}

/// A choice by its ID.
fn pick(id: &str) -> ParamValue {
    ParamValue::Structured(id.to_owned())
}

fn points(envelope: &[ToolPoint]) -> Vec<(i64, f64)> {
    envelope.iter().map(|p| (p.at.0, p.value)).collect()
}

fn close(got: &[(i64, f64)], want: &[(i64, f64)]) {
    assert_eq!(got.len(), want.len(), "{got:?} against {want:?}");
    for (got, want) in got.iter().zip(want) {
        assert_eq!(got.0, want.0, "{got:?} against {want:?}");
        assert!((got.1 - want.1).abs() < 1e-9, "{got:?} against {want:?}");
    }
}

#[test]
fn r36_glissandos_parameters_have_the_rules_ranges_and_defaults() {
    let d = Registry::native().find("glissando").unwrap().descriptor();
    let ids: Vec<&str> = d.parameters.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(ids, ["start", "curve"]);
    assert_eq!(d.defaults(), [whole(50), whole(0)]);
    assert!(d.check(&[whole(100), whole(-100)]).is_ok());
    assert!(d.check(&[whole(101), whole(0)]).is_err());
    assert!(d.check(&[whole(0), whole(101)]).is_err());
    assert!(d.check(&[whole(0), whole(-101)]).is_err());
}

#[test]
fn r37_glissando_needs_two_notes() {
    let error = try_apply("glissando", &input(vec![note(60, 0, 1000)]), &[]).unwrap_err();
    assert!(matches!(error, ToolError::Input(_)), "{error}");
    assert!(try_apply("glissando", &input(Vec::new()), &[]).is_err());
}

#[test]
fn r37_chords_pair_by_rank_and_extra_notes_pair_with_the_highest() {
    // A chord of three (listed out of order), a chord of two, a lone note.
    let notes = vec![
        note(67, 0, 1000),
        note(60, 0, 1000),
        note(64, 0, 1000),
        note(72, 2000, 1000),
        note(65, 2000, 1000),
        note(50, 4000, 1000),
    ];
    let out = apply("glissando", &input(notes.clone()), &[]);
    let end = |note: &ToolNote| note.pitch.last().map(|p| p.value);
    // 60 -> 65, 64 -> 72 and 67 (the extra) -> 72; then 65 -> 50, and 72
    // (the extra) -> 50, the next chord's only note.
    assert_eq!(
        out.iter().map(end).collect::<Vec<_>>(),
        [
            Some(5.0),
            Some(5.0),
            Some(8.0),
            Some(-22.0),
            Some(-15.0),
            None
        ]
    );
    // Keys, starts and lengths don't change, and the order stays.
    for (got, was) in out.iter().zip(&notes) {
        assert_eq!(
            (got.key, got.start, got.length),
            (was.key, was.start, was.length)
        );
    }
}

#[test]
fn r37_the_last_chords_notes_keep_the_envelope_they_had() {
    let mut last = note(50, 4000, 1000);
    last.pitch = vec![ToolPoint {
        at: Ticks::ZERO,
        value: 3.0,
        bend: 0.0,
    }];
    let out = apply(
        "glissando",
        &input(vec![note(60, 0, 1000), last.clone()]),
        &[],
    );
    assert_eq!(out[1], last);
}

#[test]
fn r38_the_bend_follows_the_curve_law_point_by_point() {
    // A note of 160 ticks: Start at 50 % is tick 80, and 16 points 5 ticks apart.
    let pair = || input(vec![note(60, 0, 160), note(72, 1000, 160)]);
    let curved = |curve: i64| {
        let out = apply("glissando", &pair(), &[("curve", whole(curve))]);
        points(&out[0].pitch)
    };
    let law = |power: f64| -> Vec<(i64, f64)> {
        let mut want = vec![(0, 0.0), (80, 0.0)];
        want.extend((1..=16).map(|k| (80 + 5 * k, 12.0 * (k as f64 / 16.0).powf(power))));
        want
    };
    close(&curved(0), &law(1.0));
    // A positive Curve gets there sooner: x to the power of 2^(-2c).
    close(&curved(100), &law(0.25));
    close(&curved(50), &law(0.5));
    close(&curved(-100), &law(4.0));
    assert!(curved(100)[8].1 > curved(0)[8].1);
    // The last point is the whole difference, at the note's end.
    assert_eq!(curved(-100).last(), Some(&(160, 12.0)));
}

#[test]
fn r38_start_places_the_bend_and_a_start_at_the_end_leaves_the_bend_one_point() {
    let pair = |start: i64| {
        let out = apply(
            "glissando",
            &input(vec![note(60, 0, 160), note(48, 1000, 160)]),
            &[("start", whole(start))],
        );
        points(&out[0].pitch)
    };
    let from_zero = pair(0);
    assert_eq!(
        from_zero.len(),
        17,
        "Start 0 % starts the bend with the note"
    );
    assert_eq!(from_zero[0], (0, 0.0));
    assert_eq!(from_zero[16], (160, -12.0));
    // At 100 % the points land on one tick and the later one stays.
    assert_eq!(pair(100), [(0, 0.0), (160, -12.0)]);
}

#[test]
fn r38_a_bend_beyond_48_semitones_is_held() {
    let out = apply(
        "glissando",
        &input(vec![note(20, 0, 160), note(100, 1000, 160)]),
        &[("curve", whole(100))],
    );
    let bend = points(&out[0].pitch);
    assert!(bend.iter().all(|p| p.1 <= 48.0));
    assert_eq!(bend.last(), Some(&(160, 48.0)));
    let down = apply(
        "glissando",
        &input(vec![note(100, 0, 160), note(20, 1000, 160)]),
        &[],
    );
    assert_eq!(points(&down[0].pitch).last(), Some(&(160, -48.0)));
}

#[test]
fn r38_the_bend_replaces_the_envelope_the_note_had() {
    let mut first = note(60, 0, 160);
    first.pitch = vec![ToolPoint {
        at: Ticks(40),
        value: 7.0,
        bend: 0.5,
    }];
    first.slide = vec![ToolPoint {
        at: Ticks::ZERO,
        value: 30.0,
        bend: 0.0,
    }];
    let out = apply(
        "glissando",
        &input(vec![first.clone(), note(62, 1000, 160)]),
        &[],
    );
    assert_eq!(out[0].pitch[0].at, Ticks::ZERO);
    assert_eq!(out[0].pitch.len(), 18);
    assert_eq!(out[0].slide, first.slide, "the other envelopes stay");
}

#[test]
fn r39_lfos_parameters_have_the_rules_ranges_and_defaults() {
    let d = Registry::native().find("lfo").unwrap().descriptor();
    let ids: Vec<&str> = d.parameters.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "target",
            "shape_type",
            "shape",
            "rate",
            "time_shift",
            "amount",
            "amplitude_base",
            "attack",
            "decay"
        ]
    );
    assert_eq!(
        d.defaults(),
        [
            ParamValue::Choice(0),
            ParamValue::Choice(0),
            whole(0),
            ParamValue::Choice(3),
            whole(0),
            number(0.0),
            number(0.0),
            whole(0),
            whole(0)
        ]
    );
    let names = |id: &str| -> Vec<String> {
        let (_, spec) = d.parameter(id).unwrap();
        let bl_midi_tools::ParamKind::Choice { options } = &spec.kind else {
            panic!("a choice")
        };
        options.iter().map(|o| o.name.clone()).collect()
    };
    assert_eq!(names("target"), ["Pitch Bend", "Slide", "Pressure"]);
    assert_eq!(
        names("shape_type"),
        ["Sine", "Square", "Triangle", "Random"]
    );
    assert_eq!(
        names("rate"),
        ["1", "1/2", "1/4", "1/8", "1/16", "1/32", "1/64", "1/128"]
    );
    // The Amount of the Pitch Bend target reaches 96 semitones and no further.
    let note = input(vec![note(60, 0, EIGHTH)]);
    assert!(try_apply("lfo", &note, &[("amount", number(96.0))]).is_ok());
    assert!(try_apply("lfo", &note, &[("amount", number(96.5))]).is_err());
    assert!(try_apply("lfo", &note, &[("amplitude_base", number(48.5))]).is_err());
    assert!(
        try_apply(
            "lfo",
            &note,
            &[("target", pick("slide")), ("amount", number(127.0))]
        )
        .is_ok()
    );
    assert!(try_apply("lfo", &note, &[("amount", number(127.5))]).is_err());
}

#[test]
fn r39_attack_and_decay_together_are_at_most_100_percent() {
    let tool = Registry::native().find("lfo").unwrap();
    let d = tool.descriptor();
    let at = |id: &str| d.parameter(id).unwrap().0;
    let settle = |before: Vec<ParamValue>, set: &[(&str, ParamValue)]| {
        let mut after = before.clone();
        let mut changed = Vec::new();
        for (id, value) in set {
            after[at(id)] = value.clone();
            changed.push(at(id));
        }
        tool.settle(&before, &mut after, &changed).map(|()| after)
    };
    let mut start = d.defaults();
    start[at("attack")] = whole(60);
    start[at("decay")] = whole(30);
    // Setting one past what the other leaves lowers the other.
    let after = settle(start.clone(), &[("attack", whole(80))]).unwrap();
    assert_eq!(
        (after[at("attack")].clone(), after[at("decay")].clone()),
        (whole(80), whole(20))
    );
    let after = settle(start.clone(), &[("decay", whole(100))]).unwrap();
    assert_eq!(
        (after[at("attack")].clone(), after[at("decay")].clone()),
        (whole(0), whole(100))
    );
    // Within the 100 %, nothing moves.
    let after = settle(start.clone(), &[("attack", whole(70))]).unwrap();
    assert_eq!(after[at("decay")], whole(30));
    // Two set at once that go past it are refused.
    assert!(settle(start, &[("attack", whole(90)), ("decay", whole(90))]).is_err());
}

#[test]
fn r39_switching_target_keeps_the_numbers_within_the_new_ranges() {
    let tool = Registry::native().find("lfo").unwrap();
    let d = tool.descriptor();
    let at = |id: &str| d.parameter(id).unwrap().0;
    let mut slide = d.defaults();
    slide[at("target")] = ParamValue::Choice(1);
    slide[at("amount")] = number(120.0);
    slide[at("amplitude_base")] = number(-100.0);
    let before = slide.clone();
    let mut after = slide;
    after[at("target")] = ParamValue::Choice(0);
    tool.settle(&before, &mut after, &[at("target")]).unwrap();
    assert_eq!(
        (
            after[at("amount")].clone(),
            after[at("amplitude_base")].clone()
        ),
        (number(96.0), number(-48.0))
    );
    // Back again, the held numbers stay.
    let before = after.clone();
    after[at("target")] = ParamValue::Choice(2);
    tool.settle(&before, &mut after, &[at("target")]).unwrap();
    assert_eq!(
        (
            after[at("amount")].clone(),
            after[at("amplitude_base")].clone()
        ),
        (number(96.0), number(-48.0))
    );
    // A number out of the target's range, with the target unchanged, is refused.
    let before = after.clone();
    after[at("target")] = ParamValue::Choice(0);
    after[at("amount")] = number(100.0);
    assert!(tool.settle(&before, &mut after, &[at("amount")]).is_err());
}

#[test]
fn r40_to_r42_a_sine_is_written_32_points_a_period_with_the_base_and_amount() {
    // A note of one period, Amount 12 around a base of 2.
    let out = apply(
        "lfo",
        &input(vec![note(60, 5000, EIGHTH)]),
        &[("amount", number(12.0)), ("amplitude_base", number(2.0))],
    );
    let want: Vec<(i64, f64)> = (0..=32)
        .map(|k| {
            (
                EIGHTH * k / 32,
                2.0 + 12.0 * (std::f64::consts::TAU * k as f64 / 32.0).sin(),
            )
        })
        .collect();
    close(&points(&out[0].pitch), &want);
    assert!(out[0].slide.is_empty() && out[0].pressure.is_empty());
    assert_eq!(
        (out[0].key, out[0].start.0, out[0].length.0),
        (60, 5000, EIGHTH)
    );
}

#[test]
fn r40_the_target_names_the_envelope_written_and_replaces_the_one_it_had() {
    let mut with_all = note(60, 0, EIGHTH);
    let old = vec![ToolPoint {
        at: Ticks(10),
        value: 5.0,
        bend: 0.2,
    }];
    with_all.pitch = old.clone();
    with_all.slide = old.clone();
    with_all.pressure = old.clone();
    let run_to = |target: &str| {
        apply(
            "lfo",
            &input(vec![with_all.clone()]),
            &[
                ("target", pick(target)),
                ("amount", number(40.0)),
                ("amplitude_base", number(60.0)),
            ],
        )
        .remove(0)
    };
    let slide = run_to("slide");
    assert_eq!(
        (slide.pitch.clone(), slide.pressure.clone()),
        (old.clone(), old.clone())
    );
    assert_eq!(slide.slide.len(), 33);
    let pressure = run_to("pressure");
    assert_eq!(
        (pressure.pitch.clone(), pressure.slide.clone()),
        (old.clone(), old)
    );
    assert_eq!(points(&pressure.pressure), points(&slide.slide));
    // 60 + 40 sin: held within 0 to 127 for Slide and Pressure.
    assert!(slide.slide.iter().all(|p| (0.0..=127.0).contains(&p.value)));
    assert_eq!(
        slide.slide[8].value, 100.0,
        "the peak, a quarter way through"
    );
    assert!(slide.slide.iter().all(|p| p.bend == 0.0));
}

#[test]
fn r40_values_are_held_within_the_targets_range() {
    let out = apply(
        "lfo",
        &input(vec![note(60, 0, EIGHTH)]),
        &[("amount", number(90.0)), ("amplitude_base", number(40.0))],
    );
    let bend = points(&out[0].pitch);
    assert!(
        bend.iter().all(|p| (-48.0..=48.0).contains(&p.1)),
        "{bend:?}"
    );
    assert_eq!(bend[8].1, 48.0, "130 held at +48");
    let out = apply(
        "lfo",
        &input(vec![note(60, 0, EIGHTH)]),
        &[
            ("target", pick("pressure")),
            ("amount", number(100.0)),
            ("amplitude_base", number(10.0)),
        ],
    );
    assert_eq!(out[0].pressure[24].value, 0.0, "-90 held at 0");
}

#[test]
fn r40_attack_and_decay_fade_the_wave_in_and_out() {
    // A square of Amount 10 over a note of 4 periods, rising over its first
    // 25 % and falling over its last 50 %.
    let length = 4 * EIGHTH;
    let out = apply(
        "lfo",
        &input(vec![note(60, 0, length)]),
        &[
            ("shape_type", pick("square")),
            ("amount", number(10.0)),
            ("attack", whole(25)),
            ("decay", whole(50)),
        ],
    );
    let bend = points(&out[0].pitch);
    assert_eq!(bend.len(), 129);
    let at = |tick: i64| bend.iter().find(|p| p.0 == tick).unwrap().1;
    assert_eq!(at(0), 0.0);
    // A quarter of the way through the attack (a period in), the square
    // is at +1 times a quarter of Amount.
    assert!((at(length / 16) - 2.5).abs() < 1e-9);
    assert!(
        (at(length / 4) - 10.0).abs() < 1e-9,
        "full at the attack's end"
    );
    // Full until the decay begins at half the note.
    assert!((at(length / 2 - length / 64).abs() - 10.0).abs() < 1e-9);
    // Three quarters through is halfway down the decay.
    assert!((at(length * 3 / 4) - 5.0).abs() < 1e-9);
    assert_eq!(at(length), 0.0, "nothing at the note's end");
}

#[test]
fn r40_time_shift_holds_the_wave_back_or_starts_it_part_way_through() {
    let sine = |shift: i64| {
        points(
            &apply(
                "lfo",
                &input(vec![note(60, 0, EIGHTH)]),
                &[("time_shift", whole(shift)), ("amount", number(10.0))],
            )[0]
            .pitch,
        )
    };
    // 25 %: zero until a quarter of a period (point 8), then a cycle from there.
    let delayed = sine(25);
    assert!(delayed[..=8].iter().all(|p| p.1 == 0.0));
    let tau = std::f64::consts::TAU;
    for (k, p) in delayed.iter().enumerate().skip(9) {
        let want = 10.0 * (tau * (k - 8) as f64 / 32.0).sin();
        assert!((p.1 - want).abs() < 1e-9, "{k}");
    }
    // -25 %: starts a quarter of the way through, at the sine's peak.
    let early = sine(-25);
    assert!((early[0].1 - 10.0).abs() < 1e-9);
    for (k, p) in early.iter().enumerate() {
        let want = 10.0 * (tau * (k + 8) as f64 / 32.0).sin();
        assert!((p.1 - want).abs() < 1e-9, "{k}");
    }
}

#[test]
fn r41_square_triangle_and_shape_follow_the_laws() {
    let wave = |kind: &str, shape: i64| {
        points(
            &apply(
                "lfo",
                &input(vec![note(60, 0, EIGHTH)]),
                &[
                    ("shape_type", pick(kind)),
                    ("shape", whole(shape)),
                    ("amount", number(10.0)),
                ],
            )[0]
            .pitch,
        )
    };
    // Square at Shape 0: m = 1/2, +1 for the first half of the cycle, then -1.
    let square = wave("square", 0);
    assert!(square[..16].iter().all(|p| p.1 == 10.0));
    assert!(square[16..32].iter().all(|p| p.1 == -10.0));
    // Shape +50 %: m = 0.75.
    let square = wave("square", 50);
    assert!(square[..24].iter().all(|p| p.1 == 10.0));
    assert!(square[24..32].iter().all(|p| p.1 == -10.0));
    // Shape beyond the ends holds m to 0.05 and 0.95.
    assert_eq!(
        wave("square", 100)[30].1,
        10.0,
        "m = 0.95: 30/32 is below it"
    );
    assert_eq!(wave("square", 100)[31].1, -10.0);
    assert_eq!(
        wave("square", -100)[1].1,
        10.0,
        "m = 0.05: 1/32 is below it"
    );
    assert_eq!(wave("square", -100)[2].1, -10.0);
    // Triangle at Shape 0: -1 at the start, +1 half way, -1 at the end.
    let triangle = wave("triangle", 0);
    for (k, p) in triangle.iter().enumerate() {
        let y = k as f64 / 32.0;
        let want = 10.0
            * if y <= 0.5 {
                4.0 * y - 1.0
            } else {
                3.0 - 4.0 * y
            };
        assert!((p.1 - want).abs() < 1e-9, "{k}");
    }
    // Shape -50 %: m = 0.25, the peak a quarter way through.
    let skewed = wave("triangle", -50);
    assert!((skewed[8].1 - 10.0).abs() < 1e-9);
    assert!((skewed[0].1 + 10.0).abs() < 1e-9 && (skewed[32].1 + 10.0).abs() < 1e-9);
    // Sine at Shape +50 %: m = 0.75, so the first 0.75 of the cycle is the
    // sine's first half.
    let sine = wave("sine", 50);
    assert!((sine[12].1 - 10.0 * (std::f64::consts::TAU * 0.25).sin()).abs() < 1e-9);
    assert!(sine[24].1.abs() < 1e-9, "the half-way zero is at m");
}

#[test]
fn r41_random_holds_a_value_a_cycle_then_slides_to_the_next() {
    // Three periods at Shape 0: m = 1/2, so each cycle holds then moves
    // over its second half.
    let length = 3 * EIGHTH;
    let mut seeded = input(vec![note(60, 0, length)]);
    seeded.seed = 7;
    let out = apply(
        "lfo",
        &seeded,
        &[("shape_type", pick("random")), ("amount", number(10.0))],
    );
    let bend = points(&out[0].pitch);
    assert_eq!(bend.len(), 97);
    // The values the application's seed draws, from -1 to 1, one a cycle
    // and one past the last it reaches.
    let mut rng = Rng::new(7);
    let draws: Vec<f64> = (0..5).map(|_| rng.unit() * 2.0 - 1.0).collect();
    for cycle in 0..3 {
        for k in 0..32 {
            let x = k as f64 / 32.0;
            let want = if x < 0.5 {
                draws[cycle]
            } else {
                draws[cycle] + (draws[cycle + 1] - draws[cycle]) * (x - 0.5) / 0.5
            };
            let got = bend[cycle * 32 + k].1;
            assert!((got - 10.0 * want).abs() < 1e-9, "cycle {cycle} point {k}");
        }
    }
    // The same seed gives the same breakpoints, and another seed other ones.
    assert_eq!(
        apply(
            "lfo",
            &seeded,
            &[("shape_type", pick("random")), ("amount", number(10.0))]
        ),
        out
    );
    seeded.seed = 8;
    assert_ne!(
        apply(
            "lfo",
            &seeded,
            &[("shape_type", pick("random")), ("amount", number(10.0))]
        ),
        out
    );
}

#[test]
fn r42_a_long_note_gets_at_most_512_breakpoints_from_its_start_to_its_end() {
    // 100 periods would be 3,200 points.
    let length = 100 * EIGHTH;
    let out = apply(
        "lfo",
        &input(vec![note(60, 0, length)]),
        &[("amount", number(10.0))],
    );
    let bend = &out[0].pitch;
    assert_eq!(bend.len(), 512);
    assert_eq!((bend[0].at, bend[511].at), (Ticks::ZERO, Ticks(length)));
    assert!(bend.windows(2).all(|pair| pair[0].at < pair[1].at));
    // A note of a period and a half has 48 segments and so 49 points.
    let out = apply("lfo", &input(vec![note(60, 0, EIGHTH * 3 / 2)]), &[]);
    assert_eq!(out[0].pitch.len(), 49);
}

#[test]
fn r42_a_note_of_a_few_ticks_gets_only_its_two_ends_and_every_rate_is_a_period() {
    let out = apply(
        "lfo",
        &input(vec![note(60, 0, 5)]),
        &[("amount", number(10.0))],
    );
    assert_eq!(
        out[0].pitch.iter().map(|p| p.at.0).collect::<Vec<_>>(),
        [0, 5]
    );
    // Rate 1/4 is a period of 705,600 ticks: a note that long has 33 points.
    let out = apply(
        "lfo",
        &input(vec![note(60, 0, 2 * EIGHTH)]),
        &[("rate", pick("1_4"))],
    );
    assert_eq!(out[0].pitch.len(), 33);
    let out = apply("lfo", &input(vec![note(60, 0, 2 * EIGHTH)]), &[]);
    assert_eq!(out[0].pitch.len(), 65);
}

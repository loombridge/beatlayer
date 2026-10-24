//! Recombine, Span, Strum, Time Warp and Velocity Shaper (the MIDI
//! transformations spec, R23 to R35), each as a function of its input, its
//! settings and its seed.

use bl_core::time::{Ticks, TimeSignature};
use bl_midi_tools::{
    Breakpoint, ParamValue, Registry, Rng, ToolError, ToolInput, ToolNote, ToolPoint, run,
    strum_position,
};

const SIXTEENTH: i64 = Ticks::SIXTEENTH.0;
const EIGHTH: i64 = 2 * SIXTEENTH;
const BAR: i64 = 16 * SIXTEENTH;

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

/// The tool's defaults with these parameters set.
fn settings(id: &str, set: &[(&str, ParamValue)]) -> Vec<ParamValue> {
    let descriptor = Registry::native().find(id).unwrap().descriptor();
    let mut values = descriptor.defaults();
    for (name, value) in set {
        let (at, _) = descriptor.parameter(name).unwrap();
        values[at] = value.clone();
    }
    values
}

fn apply(
    id: &str,
    input: &ToolInput,
    set: &[(&str, ParamValue)],
) -> Result<Vec<ToolNote>, ToolError> {
    let tool = Registry::native().find(id).unwrap();
    run(tool, input, &settings(id, set)).map(|out| out.notes)
}

fn applied(id: &str, input: &ToolInput, set: &[(&str, ParamValue)]) -> Vec<ToolNote> {
    apply(id, input, set).expect("runs")
}

/// A random phrase: notes at random places, with random keys, lengths and
/// velocities, in the clip's order.
fn phrase(seed: u32, count: usize) -> Vec<ToolNote> {
    let mut rng = Rng::new(seed);
    let mut notes: Vec<ToolNote> = (0..count)
        .map(|_| {
            let mut note = note(
                36 + rng.below(48) as i16,
                i64::from(rng.below(16)) * SIXTEENTH,
                (1 + i64::from(rng.below(8))) * SIXTEENTH,
            );
            note.velocity = 1 + rng.below(127) as u8;
            note
        })
        .collect();
    notes.sort_by_key(|note| (note.start, note.key));
    notes
}

fn whole(n: i64) -> ParamValue {
    ParamValue::Whole(n)
}

fn on(yes: bool) -> ParamValue {
    ParamValue::Switch(yes)
}

fn choice(n: usize) -> ParamValue {
    ParamValue::Choice(n)
}

fn keys(notes: &[ToolNote]) -> Vec<i16> {
    notes.iter().map(|note| note.key).collect()
}

fn starts(notes: &[ToolNote]) -> Vec<i64> {
    notes.iter().map(|note| note.start.0).collect()
}

// Recombine.

fn cdef() -> Vec<ToolNote> {
    [60, 62, 64, 65]
        .into_iter()
        .enumerate()
        .map(|(at, key)| note(key, at as i64 * EIGHTH, EIGHTH))
        .collect()
}

#[test]
fn r24_rotation_moves_values_later_and_mirror_reverses_them() {
    let notes = input(cdef(), (0, BAR));
    let pitch = ("dimension", choice(1));
    assert_eq!(
        keys(&applied(
            "recombine",
            &notes,
            &[pitch.clone(), ("rotation_steps", whole(1))]
        )),
        [65, 60, 62, 64],
        "each value moves a place later, the last going round to the first"
    );
    assert_eq!(
        keys(&applied(
            "recombine",
            &notes,
            &[pitch.clone(), ("rotation_steps", whole(-1))]
        )),
        [62, 64, 65, 60]
    );
    assert_eq!(
        keys(&applied(
            "recombine",
            &notes,
            &[pitch.clone(), ("mirror", on(true))]
        )),
        [65, 64, 62, 60]
    );
    // Mirror comes before rotation.
    assert_eq!(
        keys(&applied(
            "recombine",
            &notes,
            &[pitch, ("mirror", on(true)), ("rotation_steps", whole(1))]
        )),
        [60, 65, 64, 62]
    );
}

#[test]
fn r24_each_dimension_moves_its_own_value_and_nothing_else() {
    let mut given = cdef();
    given[1].length = Ticks(3 * EIGHTH);
    given[3].velocity = 30;
    let notes = input(given.clone(), (0, 2 * BAR));
    let rotate = ("rotation_steps", whole(1));
    let out = applied(
        "recombine",
        &notes,
        &[("dimension", choice(0)), rotate.clone()],
    );
    assert_eq!(starts(&out), [3 * EIGHTH, 0, EIGHTH, 2 * EIGHTH]);
    assert_eq!(keys(&out), keys(&given), "a note keeps its other values");
    let out = applied(
        "recombine",
        &notes,
        &[("dimension", choice(2)), rotate.clone()],
    );
    assert_eq!(
        out.iter().map(|n| n.length.0).collect::<Vec<_>>(),
        [EIGHTH, EIGHTH, 3 * EIGHTH, EIGHTH]
    );
    assert_eq!(starts(&out), starts(&given));
    let out = applied("recombine", &notes, &[("dimension", choice(3)), rotate]);
    assert_eq!(
        out.iter().map(|n| n.velocity).collect::<Vec<_>>(),
        [30, 100, 100, 100]
    );
}

#[test]
fn r24_shuffle_is_the_seeds_and_leaves_the_multiset_of_values_unchanged() {
    for seed in 0..64 {
        let mut notes = input(phrase(seed, 12), (0, BAR));
        notes.seed = seed;
        for (dimension, value) in [
            (0, (|n: &ToolNote| n.start.0) as fn(&ToolNote) -> i64),
            (1, |n| i64::from(n.key)),
            (2, |n| n.length.0),
            (3, |n| i64::from(n.velocity)),
        ] {
            let set = [
                ("dimension", choice(dimension)),
                ("shuffle", on(true)),
                ("mirror", on(seed % 2 == 0)),
                ("rotation_steps", whole(i64::from(seed) % 7 - 3)),
            ];
            let out = applied("recombine", &notes, &set);
            let sorted = |notes: &[ToolNote]| {
                let mut values: Vec<i64> = notes.iter().map(value).collect();
                values.sort_unstable();
                values
            };
            assert_eq!(sorted(&out), sorted(&notes.notes), "seed {seed}");
            assert_eq!(out, applied("recombine", &notes, &set), "the same seed");
        }
    }
    let mut a = input(phrase(5, 12), (0, BAR));
    let mut b = a.clone();
    (a.seed, b.seed) = (1, 2);
    let set = [("dimension", choice(1)), ("shuffle", on(true))];
    assert_ne!(
        keys(&applied("recombine", &a, &set)),
        keys(&applied("recombine", &b, &set)),
        "a different seed shuffles differently"
    );
}

#[test]
fn r25_rotate_on_grid_moves_starts_by_grid_steps_and_wraps_round_the_range() {
    let mut notes = input(
        vec![
            note(60, 0, EIGHTH),
            note(62, EIGHTH, EIGHTH),
            note(64, 7 * EIGHTH, EIGHTH),
        ],
        (0, BAR),
    );
    notes.grid_step = Ticks(EIGHTH);
    let on_grid = |steps: i64| {
        applied(
            "recombine",
            &notes,
            &[
                ("rotate_on_grid", on(true)),
                ("rotation_steps", whole(steps)),
            ],
        )
    };
    assert_eq!(
        starts(&on_grid(1)),
        [EIGHTH, 2 * EIGHTH, 0],
        "the last eighth goes round to the start"
    );
    assert_eq!(starts(&on_grid(-1)), [7 * EIGHTH, 0, 6 * EIGHTH]);
    // Only Position rotates on the grid.
    let pitch = applied(
        "recombine",
        &notes,
        &[
            ("dimension", choice(1)),
            ("rotate_on_grid", on(true)),
            ("rotation_steps", whole(1)),
        ],
    );
    assert_eq!(keys(&pitch), [64, 60, 62]);
    // Shuffle and Mirror rearrange the starts first.
    let mirrored = applied(
        "recombine",
        &notes,
        &[
            ("rotate_on_grid", on(true)),
            ("mirror", on(true)),
            ("rotation_steps", whole(1)),
        ],
    );
    assert_eq!(starts(&mirrored), [0, 2 * EIGHTH, EIGHTH]);
}

// Span.

#[test]
fn r27_legato_lasts_each_note_to_the_next_start_and_the_last_to_the_ranges_end() {
    let notes = input(
        vec![
            note(60, 0, SIXTEENTH),
            note(62, 4 * SIXTEENTH, SIXTEENTH),
            note(64, 6 * SIXTEENTH, SIXTEENTH),
            note(65, 6 * SIXTEENTH, 3 * SIXTEENTH),
        ],
        (0, BAR),
    );
    let out = applied("span", &notes, &[]);
    let lengths: Vec<i64> = out.iter().map(|n| n.length.0).collect();
    assert_eq!(
        lengths,
        [4 * SIXTEENTH, 2 * SIXTEENTH, 10 * SIXTEENTH, 10 * SIXTEENTH]
    );
    // A note starting at or after the range's end keeps its length.
    let late = input(
        vec![note(60, 0, SIXTEENTH), note(62, BAR, 3 * SIXTEENTH)],
        (0, BAR),
    );
    assert_eq!(applied("span", &late, &[])[1].length, Ticks(3 * SIXTEENTH));
}

#[test]
fn r27_tenuto_keeps_lengths_and_staccato_halves_the_smallest_gap() {
    let notes = input(
        vec![
            note(60, 0, 2 * SIXTEENTH),
            note(62, 4 * SIXTEENTH, SIXTEENTH),
            note(64, 4 * SIXTEENTH, 5 * SIXTEENTH),
            note(65, 6 * SIXTEENTH, SIXTEENTH),
        ],
        (0, BAR),
    );
    let lengths = |mode: usize, extra: &[(&str, ParamValue)]| -> Vec<i64> {
        let mut set = vec![("mode", choice(mode))];
        set.extend_from_slice(extra);
        applied("span", &notes, &set)
            .iter()
            .map(|n| n.length.0)
            .collect()
    };
    assert_eq!(
        lengths(1, &[]),
        [2 * SIXTEENTH, SIXTEENTH, 5 * SIXTEENTH, SIXTEENTH]
    );
    // The different starts are 0, 4 and 6 sixteenths: the smallest gap is
    // two, so every note lasts one.
    assert_eq!(lengths(2, &[]), [SIXTEENTH; 4]);
    // Offset moves every end by a share of the grid step.
    assert_eq!(
        lengths(1, &[("offset", whole(50))]),
        [
            2 * SIXTEENTH + SIXTEENTH / 2,
            SIXTEENTH + SIXTEENTH / 2,
            5 * SIXTEENTH + SIXTEENTH / 2,
            SIXTEENTH + SIXTEENTH / 2
        ]
    );
    // With one start there is no gap, and the lengths stay.
    let chord = input(
        vec![note(60, 0, SIXTEENTH), note(64, 0, 2 * SIXTEENTH)],
        (0, BAR),
    );
    let out = applied("span", &chord, &[("mode", choice(2))]);
    assert_eq!(out[1].length, Ticks(2 * SIXTEENTH));
    // A note keeps at least a tick.
    let out = applied(
        "span",
        &chord,
        &[("mode", choice(1)), ("offset", whole(-100))],
    );
    assert!(out.iter().all(|n| n.length >= Ticks(1)));
}

#[test]
fn r28_variation_shortens_or_lengthens_from_the_seed_and_no_start_ever_moves() {
    for seed in 0..64 {
        let mut notes = input(phrase(seed, 10), (0, BAR));
        notes.seed = seed;
        for mode in 0..3 {
            let set = [
                ("mode", choice(mode)),
                ("offset", whole(i64::from(seed) % 5 * 25 - 50)),
                ("variation", whole(i64::from(seed) % 9 * 25 - 100)),
            ];
            let out = applied("span", &notes, &set);
            assert_eq!(starts(&out), starts(&notes.notes), "seed {seed}");
            assert!(out.iter().all(|n| n.length >= Ticks(1)));
            assert_eq!(out, applied("span", &notes, &set), "the same seed");
        }
    }
    let notes = input(
        vec![
            note(60, 0, 8 * SIXTEENTH),
            note(62, 8 * SIXTEENTH, 8 * SIXTEENTH),
        ],
        (0, BAR),
    );
    let tenuto = |variation: i64| {
        applied(
            "span",
            &notes,
            &[("mode", choice(1)), ("variation", whole(variation))],
        )
    };
    assert!(
        tenuto(100).iter().all(|n| n.length < Ticks(8 * SIXTEENTH)),
        "positive shortens"
    );
    assert!(
        tenuto(-100).iter().all(|n| n.length > Ticks(8 * SIXTEENTH)),
        "negative lengthens"
    );
}

#[test]
fn r27_a_note_span_shortens_keeps_its_envelopes_as_a_shortened_note_does() {
    let mut long = note(60, 0, 4 * SIXTEENTH);
    long.pressure = vec![
        ToolPoint {
            at: Ticks(0),
            value: 0.0,
            bend: 0.0,
        },
        ToolPoint {
            at: Ticks(4 * SIXTEENTH),
            value: 100.0,
            bend: 0.0,
        },
    ];
    let notes = input(vec![long, note(62, 2 * SIXTEENTH, SIXTEENTH)], (0, BAR));
    let out = applied("span", &notes, &[]);
    assert_eq!(out[0].length, Ticks(2 * SIXTEENTH));
    let last = out[0].pressure.last().unwrap();
    assert_eq!(last.at, Ticks(2 * SIXTEENTH));
    assert!((last.value - 50.0).abs() < 1e-9, "the value at the new end");
}

// Strum.

fn chord(keys: &[i16], start: i64, length: i64) -> Vec<ToolNote> {
    keys.iter().map(|key| note(*key, start, length)).collect()
}

#[test]
fn r30_a_chords_notes_move_along_the_strum_law_with_their_ends_in_place() {
    let notes = input(chord(&[67, 60, 64, 72], BAR, 8 * SIXTEENTH), (0, 4 * BAR));
    for (low, high, tension) in [
        (100, 0, 0),
        (0, 100, 0),
        (100, 100, 0),
        (-100, -100, 0),
        (100, 0, 100),
        (100, 0, -100),
        (50, -25, 40),
    ] {
        let out = applied(
            "strum",
            &notes,
            &[
                ("strum_low", whole(low)),
                ("strum_high", whole(high)),
                ("tension", whole(tension)),
            ],
        );
        let (low, high, t) = (
            low as f64 / 100.0,
            high as f64 / 100.0,
            tension as f64 / 100.0,
        );
        // The lowest key is rank 0, whatever order the notes come in.
        for (note, rank) in out.iter().zip([2usize, 0, 1, 3]) {
            let law = strum_position(rank, 4, t);
            let moved = (SIXTEENTH as f64 * (low + (high - low) * law) + 0.5).floor() as i64;
            assert_eq!(
                note.start.0,
                BAR + moved,
                "key {} at ({low}, {high}, {t})",
                note.key
            );
            assert_eq!(note.end(), Ticks(BAR + 8 * SIXTEENTH), "the end stays put");
        }
    }
    // Strum Low 100 % with Strum High 0 %: the lowest note a whole step
    // later, the highest not at all, the notes between along the law.
    let out = applied("strum", &notes, &[("strum_low", whole(100))]);
    assert_eq!(out[1].start, Ticks(BAR + SIXTEENTH));
    assert_eq!(out[3].start, Ticks(BAR));
}

#[test]
fn r30_a_note_alone_at_its_start_does_not_move_and_chords_are_apart() {
    let mut given = chord(&[60, 64], 0, 2 * SIXTEENTH);
    given.push(note(70, 4 * SIXTEENTH, 2 * SIXTEENTH));
    given.extend(chord(&[50, 55, 59], 8 * SIXTEENTH, 4 * SIXTEENTH));
    let notes = input(given.clone(), (0, BAR));
    let out = applied(
        "strum",
        &notes,
        &[("strum_low", whole(100)), ("strum_high", whole(100))],
    );
    assert_eq!(out[2], given[2], "a note alone at its start");
    assert_eq!(
        starts(&out),
        [
            SIXTEENTH,
            SIXTEENTH,
            4 * SIXTEENTH,
            9 * SIXTEENTH,
            9 * SIXTEENTH,
            9 * SIXTEENTH
        ]
    );
}

#[test]
fn r30_a_start_never_goes_before_zero_or_closer_than_a_tick_to_its_end() {
    let early = input(chord(&[60, 64], 10, 5), (0, BAR));
    let out = applied(
        "strum",
        &early,
        &[("strum_low", whole(-100)), ("strum_high", whole(-100))],
    );
    assert!(
        out.iter()
            .all(|n| n.start == Ticks(0) && n.end() == Ticks(15))
    );
    let out = applied(
        "strum",
        &early,
        &[("strum_low", whole(100)), ("strum_high", whole(100))],
    );
    assert!(
        out.iter()
            .all(|n| n.start == Ticks(14) && n.length == Ticks(1))
    );
}

#[test]
fn r30_strum_moves_only_chord_starts_and_never_past_an_end() {
    for seed in 0..64 {
        let notes = input(phrase(seed, 14), (0, 2 * BAR));
        let low = i64::from(seed) % 5 * 50 - 100;
        let out = applied(
            "strum",
            &notes,
            &[
                ("strum_low", whole(low)),
                ("strum_high", whole(i64::from(seed) % 3 * 100 - 100)),
                ("tension", whole(i64::from(seed) % 7 * 33 - 99)),
            ],
        );
        assert_eq!(out.len(), notes.notes.len());
        for (before, after) in notes.notes.iter().zip(&out) {
            assert_eq!(before.end(), after.end(), "seed {seed}: the end stays put");
            assert!(after.start >= Ticks::ZERO && after.start < after.end());
            let in_chord = notes
                .notes
                .iter()
                .filter(|n| n.start == before.start)
                .count()
                > 1;
            if !in_chord {
                assert_eq!(before, after, "seed {seed}: a lone note doesn't move");
            }
            assert_eq!((before.key, before.velocity), (after.key, after.velocity));
        }
    }
}

// Time Warp.

fn eight_sixteenths() -> ToolInput {
    input(
        (0..8)
            .map(|n| note(60 + n, n as i64 * SIXTEENTH, SIXTEENTH / 2))
            .collect(),
        (0, 8 * SIXTEENTH),
    )
}

/// The settings of two breakpoints, the third off.
fn two(time_1: i64, speed_1: i64, time_2: i64, speed_2: i64) -> Vec<(&'static str, ParamValue)> {
    vec![
        ("breakpoint_1_time", whole(time_1)),
        ("breakpoint_1_speed", whole(speed_1)),
        ("breakpoint_2_time", whole(time_2)),
        ("breakpoint_2_speed", whole(speed_2)),
    ]
}

#[test]
fn r31_the_defaults_leave_the_notes_where_they_are() {
    let notes = eight_sixteenths();
    assert_eq!(applied("time_warp", &notes, &[]), notes.notes);
}

#[test]
fn r32_a_constant_speed_scales_starts_and_the_lengths_stay_unless_ends_are_warped() {
    let notes = eight_sixteenths();
    let half_time = {
        let mut set = two(0, 200, 100, 200);
        set.push(("preserve_time_range", on(false)));
        set
    };
    let out = applied("time_warp", &notes, &half_time);
    assert_eq!(
        starts(&out),
        [
            0,
            SIXTEENTH / 2,
            SIXTEENTH,
            3 * SIXTEENTH / 2,
            2 * SIXTEENTH,
            5 * SIXTEENTH / 2,
            3 * SIXTEENTH,
            7 * SIXTEENTH / 2
        ]
        .map(|n| n)
    );
    assert!(
        out.iter().all(|n| n.length == Ticks(SIXTEENTH / 2)),
        "the lengths stay"
    );
    let mut with_ends = half_time;
    with_ends.push(("include_note_end", on(true)));
    let out = applied("time_warp", &notes, &with_ends);
    assert!(
        out.iter().all(|n| n.length == Ticks(SIXTEENTH / 4)),
        "the ends warp too"
    );
    // Preserve Time Range puts the end back where it was: a constant speed
    // then changes nothing.
    let out = applied("time_warp", &notes, &two(0, 200, 100, 200));
    assert_eq!(out, notes.notes);
}

#[test]
fn r32_an_accelerando_draws_the_notes_closer_along_the_integral_of_the_inverse_speed() {
    // Speed 100 % rising to 200 % across the range: U(u) = ln(1 + u).
    let notes = eight_sixteenths();
    let range = 8.0 * SIXTEENTH as f64;
    let free = applied("time_warp", &notes, &{
        let mut set = two(0, 100, 100, 200);
        set.push(("preserve_time_range", on(false)));
        set
    });
    let preserved = applied("time_warp", &notes, &two(0, 100, 100, 200));
    for (at, (free, preserved)) in free.iter().zip(&preserved).enumerate() {
        let u = at as f64 / 8.0;
        assert!(
            (free.start.0 as f64 - range * (1.0 + u).ln()).abs() <= 1.0,
            "note {at}"
        );
        assert!((preserved.start.0 as f64 - range * (1.0 + u).ln() / 2f64.ln()).abs() <= 1.0);
    }
    assert!(
        free[7].start.0 < 7 * SIXTEENTH,
        "the notes draw closer together"
    );
}

#[test]
fn r32_preserve_time_range_keeps_every_start_in_the_range_and_in_order() {
    for seed in 0..64 {
        let mut rng = Rng::new(seed);
        let notes = input(phrase(seed, 12), (0, BAR));
        let mut set = vec![
            ("breakpoint_1_time", whole(i64::from(rng.below(40)))),
            ("breakpoint_1_speed", whole(25 + i64::from(rng.below(376)))),
            ("breakpoint_2_time", whole(40 + i64::from(rng.below(61)))),
            ("breakpoint_2_speed", whole(25 + i64::from(rng.below(376)))),
            ("breakpoint_3", on(seed % 2 == 0)),
            ("breakpoint_3_time", whole(i64::from(rng.below(101)))),
            ("breakpoint_3_speed", whole(25 + i64::from(rng.below(376)))),
        ];
        for (quantize, ends) in [(false, false), (true, false), (false, true), (true, true)] {
            set.truncate(7);
            set.push(("quantize", on(quantize)));
            set.push(("include_note_end", on(ends)));
            let out = applied("time_warp", &notes, &set);
            assert!(
                out.iter()
                    .all(|n| n.start >= Ticks(0) && n.start < Ticks(BAR)),
                "seed {seed}"
            );
            // The notes are in order of start, so the warped starts keep it.
            assert!(
                out.windows(2).all(|pair| pair[0].start <= pair[1].start),
                "seed {seed}"
            );
            assert!(out.iter().all(|n| n.length >= Ticks(1)));
            assert_eq!(keys(&out), keys(&notes.notes));
            if quantize {
                assert!(
                    out.iter().all(|n| n.start.0 % SIXTEENTH == 0),
                    "on the grid"
                );
            }
            if !ends {
                assert!(
                    out.iter()
                        .zip(&notes.notes)
                        .all(|(a, b)| a.length == b.length)
                );
            }
        }
    }
}

#[test]
fn r32_a_note_outside_the_range_takes_the_level_speed_beyond_the_breakpoints() {
    // Breakpoints at 25 % and 75 % of the range, speed 200 % then 200 %:
    // time runs at twice the speed everywhere, before and after.
    let notes = input(
        vec![note(60, 0, SIXTEENTH), note(61, 3 * BAR, SIXTEENTH)],
        (BAR, 2 * BAR),
    );
    let out = applied("time_warp", &notes, &{
        let mut set = two(25, 200, 75, 200);
        set.push(("preserve_time_range", on(false)));
        set
    });
    assert_eq!(starts(&out), [BAR / 2, 2 * BAR]);
}

#[test]
fn r31_at_least_one_breakpoint_stays_on() {
    let notes = eight_sixteenths();
    let off = [("breakpoint_1", on(false)), ("breakpoint_2", on(false))];
    let error = apply("time_warp", &notes, &off).unwrap_err();
    assert!(matches!(error, ToolError::Values(_)), "{error}");
    // One breakpoint is a level speed.
    let one = applied(
        "time_warp",
        &notes,
        &[
            ("breakpoint_2", on(false)),
            ("breakpoint_1_speed", whole(200)),
            ("preserve_time_range", on(false)),
        ],
    );
    assert_eq!(one[4].start, Ticks(2 * SIXTEENTH));
}

#[test]
fn r33_quantize_puts_warped_starts_and_ends_on_the_nearest_line() {
    let notes = eight_sixteenths();
    let set = |ends: bool| {
        let mut set = two(0, 100, 100, 200);
        set.extend([
            ("quantize", on(true)),
            ("preserve_time_range", on(false)),
            ("include_note_end", on(ends)),
        ]);
        set
    };
    let out = applied("time_warp", &notes, &set(false));
    assert!(out.iter().all(|n| n.start.0 % SIXTEENTH == 0));
    let out = applied("time_warp", &notes, &set(true));
    assert!(
        out.iter()
            .all(|n| n.start.0 % SIXTEENTH == 0 && n.end().0 % SIXTEENTH == 0)
    );
    assert!(
        out.iter().all(|n| n.length >= Ticks(1)),
        "an end on or before its start goes to the next line"
    );
}

#[test]
fn r14_time_warp_scales_a_notes_envelopes_by_the_factor_it_scales_the_note() {
    let mut long = note(60, 0, 4 * SIXTEENTH);
    long.pitch = vec![
        ToolPoint {
            at: Ticks(0),
            value: 0.0,
            bend: 0.0,
        },
        ToolPoint {
            at: Ticks(2 * SIXTEENTH),
            value: 12.0,
            bend: 0.0,
        },
        ToolPoint {
            at: Ticks(4 * SIXTEENTH),
            value: -3.0,
            bend: 0.0,
        },
    ];
    let notes = input(vec![long.clone()], (0, 4 * SIXTEENTH));
    let set = [
        ("breakpoint_1_speed", whole(200)),
        ("breakpoint_2_speed", whole(200)),
        ("preserve_time_range", on(false)),
        ("include_note_end", on(true)),
    ];
    let out = &applied("time_warp", &notes, &set)[0];
    assert_eq!(out.length, Ticks(2 * SIXTEENTH));
    let at: Vec<i64> = out.pitch.iter().map(|p| p.at.0).collect();
    assert_eq!(
        at,
        [0, SIXTEENTH, 2 * SIXTEENTH],
        "positions halve with the length"
    );
    assert_eq!(out.pitch[1].value, 12.0);
    // Without Include Note End the note keeps its length, and its envelopes.
    let kept = &applied(
        "time_warp",
        &notes,
        &[
            ("breakpoint_1_speed", whole(200)),
            ("breakpoint_2_speed", whole(200)),
            ("preserve_time_range", on(false)),
        ],
    )[0];
    assert_eq!(kept.pitch, long.pitch);
}

// Velocity Shaper.

fn ramp_notes(count: i64) -> ToolInput {
    input(
        (0..count)
            .map(|n| note(60, n * BAR / count, SIXTEENTH))
            .collect(),
        (0, BAR),
    )
}

fn velocities(notes: &[ToolNote]) -> Vec<u8> {
    notes.iter().map(|n| n.velocity).collect()
}

#[test]
fn r35_the_default_envelope_ramps_from_the_minimum_to_the_maximum() {
    let out = applied("velocity_shaper", &ramp_notes(4), &[]);
    // 1 + 126 × (0, ¼, ½, ¾), rounded.
    assert_eq!(velocities(&out), [1, 33, 64, 96]);
    let out = applied(
        "velocity_shaper",
        &ramp_notes(4),
        &[
            ("minimum_velocity", whole(20)),
            ("maximum_velocity", whole(100)),
        ],
    );
    assert_eq!(velocities(&out), [20, 40, 60, 80]);
    // A minimum above the maximum turns the shape upside down.
    let out = applied(
        "velocity_shaper",
        &ramp_notes(4),
        &[
            ("minimum_velocity", whole(100)),
            ("maximum_velocity", whole(20)),
        ],
    );
    assert_eq!(velocities(&out), [100, 80, 60, 40]);
}

fn envelope(points: &[(f64, f64)]) -> ParamValue {
    ParamValue::Breakpoints(
        points
            .iter()
            .map(|(x, y)| Breakpoint { x: *x, y: *y })
            .collect(),
    )
}

#[test]
fn r35_loop_repeats_the_envelope_and_rotate_shifts_it_by_steps_of_the_division() {
    let notes = ramp_notes(8);
    let min_max = [
        ("minimum_velocity", whole(1)),
        ("maximum_velocity", whole(101)),
    ];
    let with = |extra: &[(&str, ParamValue)]| {
        let mut set = min_max.to_vec();
        set.extend_from_slice(extra);
        velocities(&applied("velocity_shaper", &notes, &set))
    };
    assert_eq!(with(&[]), [1, 14, 26, 39, 51, 64, 76, 89]);
    assert_eq!(with(&[("loop", whole(2))]), [1, 26, 51, 76, 1, 26, 51, 76]);
    // Rotate 1 of a division of an eighth of the range: the envelope moves
    // one note later, so each note reads the one before's phase, and the
    // first wraps round to the last.
    let rotated = with(&[("rotate", whole(1)), ("division", choice(1 + 5))]);
    assert_eq!(rotated, [89, 1, 14, 26, 39, 51, 64, 76]);
    let by_grid = with(&[("rotate", whole(-1))]);
    // Division Grid is a sixteenth here.
    assert_eq!(by_grid.len(), 8);
    assert!(by_grid[0] > 1, "the first note reads ahead of its phase");
}

#[test]
fn r35_a_shaped_envelope_is_read_along_straight_lines_and_stays_within_the_range() {
    let notes = ramp_notes(4);
    let triangle = envelope(&[(0.0, 0.0), (0.5, 1.0), (1.0, 0.0)]);
    let out = applied(
        "velocity_shaper",
        &notes,
        &[
            ("envelope", triangle),
            ("minimum_velocity", whole(1)),
            ("maximum_velocity", whole(101)),
        ],
    );
    assert_eq!(velocities(&out), [1, 51, 101, 51]);
    for seed in 0..64 {
        let mut rng = Rng::new(seed);
        let points = 2 + rng.below(8) as usize;
        let mut xs: Vec<f64> = (1..points - 1).map(|_| rng.unit()).collect();
        xs.sort_by(f64::total_cmp);
        xs.dedup();
        let mut shape = vec![(0.0, rng.unit())];
        shape.extend(xs.iter().map(|x| (*x, rng.unit())));
        shape.push((1.0, rng.unit()));
        let (low, high) = (1 + rng.below(127) as i64, 1 + rng.below(127) as i64);
        let notes = input(phrase(seed, 16), (0, BAR));
        let out = applied(
            "velocity_shaper",
            &notes,
            &[
                ("envelope", envelope(&shape)),
                ("minimum_velocity", whole(low)),
                ("maximum_velocity", whole(high)),
                ("loop", whole(1 + i64::from(seed) % 16)),
                ("rotate", whole(i64::from(seed) % 33 - 16)),
            ],
        );
        let (floor, ceiling) = (low.min(high) as u8, low.max(high) as u8);
        assert!(
            out.iter().all(|n| (floor..=ceiling).contains(&n.velocity)),
            "seed {seed}: {:?} within {floor} to {ceiling}",
            velocities(&out)
        );
        assert_eq!(starts(&out), starts(&notes.notes), "nothing else changes");
        assert_eq!(keys(&out), keys(&notes.notes));
    }
}

#[test]
fn r34_an_envelope_has_two_to_thirty_two_breakpoints_from_the_start_to_the_end() {
    let notes = ramp_notes(4);
    let refused =
        |points: ParamValue| apply("velocity_shaper", &notes, &[("envelope", points)]).is_err();
    assert!(refused(envelope(&[(0.0, 0.5)])), "one breakpoint");
    assert!(
        refused(envelope(&[(0.1, 0.0), (1.0, 1.0)])),
        "the first isn't at 0"
    );
    assert!(
        refused(envelope(&[(0.0, 0.0), (0.9, 1.0)])),
        "the last isn't at 1"
    );
    let many: Vec<(f64, f64)> = (0..33).map(|n| (f64::from(n) / 32.0, 0.5)).collect();
    assert!(refused(envelope(&many)), "thirty-three breakpoints");
    let thirty_two: Vec<(f64, f64)> = (0..32).map(|n| (f64::from(n) / 31.0, 0.5)).collect();
    assert!(!refused(envelope(&thirty_two)), "thirty-two");
}

#[test]
fn r23_r26_r29_r31_r34_each_tools_parameters_have_their_ranges_and_defaults() {
    let describe = |id: &str| -> Vec<(String, String)> {
        Registry::native()
            .find(id)
            .unwrap()
            .descriptor()
            .parameters
            .iter()
            .map(|p| (p.id.clone(), format!("{:?} {:?}", p.kind, p.default)))
            .collect()
    };
    let ids = |id: &str| -> Vec<String> { describe(id).into_iter().map(|(id, _)| id).collect() };
    assert_eq!(
        ids("recombine"),
        [
            "dimension",
            "shuffle",
            "mirror",
            "rotation_steps",
            "rotate_on_grid"
        ]
    );
    assert_eq!(ids("span"), ["mode", "offset", "variation"]);
    assert_eq!(ids("strum"), ["strum_low", "strum_high", "tension"]);
    assert_eq!(
        ids("velocity_shaper"),
        [
            "envelope",
            "minimum_velocity",
            "maximum_velocity",
            "loop",
            "rotate",
            "division"
        ]
    );
    // Time Warp: three breakpoints of a switch, a Time and a Speed, then the
    // three switches.
    let time_warp = ids("time_warp");
    assert_eq!(time_warp.len(), 12);
    assert_eq!(
        time_warp[0..3],
        ["breakpoint_1", "breakpoint_1_time", "breakpoint_1_speed"]
    );
    assert_eq!(
        time_warp[9..],
        ["quantize", "preserve_time_range", "include_note_end"]
    );
    let defaults = |id: &str| Registry::native().find(id).unwrap().descriptor().defaults();
    assert_eq!(
        defaults("time_warp"),
        [
            on(true),
            whole(0),
            whole(100),
            on(true),
            whole(100),
            whole(100),
            on(false),
            whole(50),
            whole(100),
            on(false),
            on(true),
            on(false),
        ]
    );
    assert_eq!(defaults("span"), [choice(0), whole(0), whole(0)]);
    assert_eq!(defaults("strum"), [whole(0), whole(0), whole(0)]);
    assert_eq!(
        defaults("recombine"),
        [choice(0), on(false), on(false), whole(0), on(false)]
    );
    assert_eq!(
        defaults("velocity_shaper")[1..],
        [whole(1), whole(127), whole(1), whole(0), choice(0)]
    );
    // The ranges: Speed 25 % to 400 %, Rotate and Loop as R34 gives them.
    let tool = Registry::native().find("time_warp").unwrap();
    let mut values = tool.descriptor().defaults();
    values[2] = whole(401);
    assert!(
        run(tool, &eight_sixteenths(), &values).is_err(),
        "Speed tops out at 400 %"
    );
    values[2] = whole(24);
    assert!(
        run(tool, &eight_sixteenths(), &values).is_err(),
        "and starts at 25 %"
    );
    let shaper = Registry::native().find("velocity_shaper").unwrap();
    for (at, out_of_range) in [(3, 0), (3, 17), (4, -17), (4, 17), (1, 0), (2, 128)] {
        let mut values = shaper.descriptor().defaults();
        values[at] = whole(out_of_range);
        assert!(
            run(shaper, &ramp_notes(4), &values).is_err(),
            "parameter {at} at {out_of_range}"
        );
    }
}

//! Stacks and its chord banks (the MIDI generators spec, R17 to R28): the
//! chord laws in and out of Scale Mode and on Drum Rack keys, the bank file
//! format, and Beatlayer's built-in bank.

use bl_core::time::{Ticks, TimeSignature};
use bl_midi_tools::{
    BANK_FORMAT, ChordBank, ParamValue, Registry, ToolInput, ToolNote, ToolScale, add_chord,
    built_in_bank, delete_chord, load_bank, run, stack_view,
};
use proptest::prelude::*;

const BAR: i64 = 2_822_400;

fn input(range: (i64, i64)) -> ToolInput {
    ToolInput {
        notes: Vec::new(),
        range: (Ticks(range.0), Ticks(range.1)),
        grid_step: Ticks(176_400),
        signature: TimeSignature::FOUR_FOUR,
        scale: None,
        drum_keys: None,
        seed: 1,
    }
}

fn scaled(root: u8, steps: &[u8], mut input: ToolInput) -> ToolInput {
    input.scale = Some(ToolScale {
        root,
        steps: steps.to_vec(),
    });
    input
}

const MAJOR: [u8; 7] = [0, 2, 4, 5, 7, 9, 11];
const MINOR: [u8; 7] = [0, 2, 3, 5, 7, 8, 10];

fn parameter(name: &str) -> usize {
    Registry::native()
        .find("stacks")
        .and_then(|tool| tool.descriptor().parameter(name).map(|(at, _)| at))
        .expect("a Stacks parameter")
}

/// Stacks' defaults with these parameters changed, as the shell does: the
/// tool settles them and the descriptor checks them.
fn change(from: &[ParamValue], changes: &[(&str, ParamValue)]) -> Result<Vec<ParamValue>, String> {
    let tool = Registry::native().find("stacks").expect("Stacks");
    let mut after = from.to_vec();
    let mut changed = Vec::new();
    for (name, value) in changes {
        let (at, spec) = tool.descriptor().parameter(name).expect("a parameter");
        spec.check(value)?;
        after[at] = value.clone();
        changed.push(at);
    }
    tool.settle(from, &mut after, &changed)?;
    tool.descriptor()
        .check(&after)
        .map_err(|error| error.to_string())?;
    tool.validate(&after).map_err(|error| error.to_string())?;
    Ok(after)
}

fn defaults() -> Vec<ParamValue> {
    Registry::native()
        .find("stacks")
        .expect("Stacks")
        .descriptor()
        .defaults()
}

fn whole(n: i64) -> ParamValue {
    ParamValue::Whole(n)
}

fn progression(chords: &str) -> ParamValue {
    ParamValue::Structured(chords.to_owned())
}

fn bank_entry(name: &str) -> i64 {
    built_in_bank()
        .chords
        .iter()
        .position(|entry| entry.name == name)
        .expect("a built-in entry") as i64
}

fn notes_of(input: &ToolInput, values: &[ParamValue]) -> Vec<ToolNote> {
    run(
        Registry::native().find("stacks").expect("Stacks"),
        input,
        values,
    )
    .expect("runs")
    .notes
}

fn keys(notes: &[ToolNote]) -> Vec<i16> {
    let mut keys: Vec<i16> = notes.iter().map(|note| note.key).collect();
    keys.sort_unstable();
    keys
}

/// The keys of one chord written on `root`, from the bank entry named `name`.
fn chord_keys(input: &ToolInput, name: &str, root: u8, inversion: i64) -> Vec<i16> {
    let at = built_in_bank()
        .chords
        .iter()
        .position(|entry| entry.name == name)
        .expect("a built-in entry");
    let values = change(
        &defaults(),
        &[
            ("chord", whole(at as i64)),
            ("chord_root", ParamValue::Pitch(root)),
            ("chord_inversion", whole(inversion)),
        ],
    )
    .expect("settles");
    keys(&notes_of(input, &values))
}

#[test]
fn r17_a_new_stacks_has_one_chord_with_the_tables_defaults() {
    let view = stack_view(&defaults()).expect("a view");
    assert_eq!(view.chords.len(), 1);
    let chord = view.chords[0];
    assert_eq!(
        (
            chord.chord,
            chord.root,
            chord.inversion,
            chord.duration,
            chord.offset
        ),
        (0, 60, 0, 8, 0)
    );
    assert_eq!(view.selected, 0);
    assert!(view.built_in);
    assert_eq!(view.bank.name, "Beatlayer Chords");
    // The parameters' ranges are the table's.
    let tool = Registry::native().find("stacks").expect("Stacks");
    let d = tool.descriptor();
    for (id, bad) in [
        ("chord_root", ParamValue::Pitch(128)),
        ("chord_inversion", whole(8)),
        ("chord_duration", whole(0)),
        ("chord_duration", whole(9)),
        ("chord_offset", whole(8)),
        ("chord_offset", whole(-1)),
    ] {
        let (_, spec) = d.parameter(id).expect("a parameter");
        assert!(spec.check(&bad).is_err(), "{id} refuses {bad:?}");
    }
}

#[test]
fn r17_add_chord_follows_the_selected_one_and_delete_chord_keeps_one() {
    let mut values =
        change(&defaults(), &[("chord_root", ParamValue::Pitch(62))]).expect("settles");
    let at = built_in_bank().chords.len() as i64 - 1;
    values = change(&values, &[("chord", whole(at))]).expect("settles");
    add_chord(&mut values).expect("adds");
    let view = stack_view(&values).expect("a view");
    assert_eq!(view.chords.len(), 2);
    assert_eq!(view.selected, 1, "the new chord is selected");
    // The bank's first entry with the selected chord's root.
    let new = view.chords[1];
    assert_eq!(
        (new.chord, new.root, new.inversion, new.duration, new.offset),
        (0, 62, 0, 8, 0)
    );
    assert_eq!(view.chords[0].chord as i64, at);
    // The controls show the new chord.
    assert_eq!(values[parameter("chord")], whole(0));
    // A chord added after the first goes between.
    values = change(&values, &[("selected", whole(0))]).expect("settles");
    add_chord(&mut values).expect("adds");
    assert_eq!(stack_view(&values).expect("a view").selected, 1);
    assert_eq!(stack_view(&values).expect("a view").chords.len(), 3);
    delete_chord(&mut values).expect("deletes");
    delete_chord(&mut values).expect("deletes");
    assert!(delete_chord(&mut values).is_err(), "the only chord stays");
    assert_eq!(stack_view(&values).expect("a view").chords.len(), 1);
    // A progression holds 16.
    for _ in 0..15 {
        add_chord(&mut values).expect("adds");
    }
    assert!(add_chord(&mut values).is_err());
    assert_eq!(stack_view(&values).expect("a view").chords.len(), 16);
}

#[test]
fn r17_a_change_to_the_selected_chord_is_the_progressions_and_back() {
    let mut values = defaults();
    add_chord(&mut values).expect("adds");
    values = change(
        &values,
        &[
            ("chord_root", ParamValue::Pitch(65)),
            ("chord_offset", whole(2)),
        ],
    )
    .expect("settles");
    let view = stack_view(&values).expect("a view");
    assert_eq!((view.chords[1].root, view.chords[1].offset), (65, 2));
    assert_eq!((view.chords[0].root, view.chords[0].offset), (60, 0));
    // Choosing another chord shows its settings in the controls.
    values = change(&values, &[("selected", whole(0))]).expect("settles");
    assert_eq!(values[parameter("chord_root")], ParamValue::Pitch(60));
    // A progression given whole sets the selected chord's controls too.
    values = change(
        &values,
        &[
            (
                "progression",
                progression("[{\"root\":67},{\"chord\":9,\"duration\":4}]"),
            ),
            ("selected", whole(1)),
        ],
    )
    .expect("settles");
    assert_eq!(values[parameter("chord")], whole(9));
    assert_eq!(values[parameter("chord_duration")], whole(4));
    // A value out of range, a chord past the bank, a seventeenth chord and a
    // selection past the end are refused.
    for bad in [
        change(
            &values,
            &[("progression", progression("[{\"duration\":9}]"))],
        ),
        change(&values, &[("progression", progression("[{\"chord\":99}]"))]),
        change(&values, &[("progression", progression("[]"))]),
        change(
            &values,
            &[(
                "progression",
                progression(&format!("[{}]", vec!["{}"; 17].join(","))),
            )],
        ),
        change(&values, &[("progression", progression("not json"))]),
        change(&values, &[("selected", whole(2))]),
        change(&values, &[("chord", whole(40))]),
    ] {
        assert!(bad.is_err());
    }
    // A shorter chord takes the inversion down to what it can have, and
    // asking for more is refused.
    values = change(
        &values,
        &[("chord", whole(0)), ("chord_inversion", whole(2))],
    )
    .expect("settles");
    let fifth = built_in_bank()
        .chords
        .iter()
        .position(|e| e.name == "Fifth")
        .expect("Fifth") as i64;
    let moved = change(&values, &[("chord", whole(fifth))]).expect("settles");
    assert_eq!(moved[parameter("chord_inversion")], whole(1));
    assert!(
        change(
            &values,
            &[("chord", whole(fifth)), ("chord_inversion", whole(2))]
        )
        .is_err()
    );
}

#[test]
fn r18_the_range_is_equal_slots_and_a_chord_takes_eighths_of_its_own() {
    let mut values = defaults();
    values = change(
        &values,
        &[(
            "progression",
            progression(
                "[{\"root\":60},{\"root\":62,\"duration\":4},{\"root\":64,\"offset\":2,\"duration\":3},\
                 {\"root\":65,\"offset\":6,\"duration\":8}]",
            ),
        )],
    )
    .expect("settles");
    let notes = notes_of(&input((0, BAR)), &values);
    // Four slots of a quarter note; each chord is a triad written together.
    let slot = BAR / 4;
    let spans = |at: i64| -> Vec<(i64, i64)> {
        let mut spans: Vec<(i64, i64)> = notes
            .iter()
            .filter(|note| (at * slot..(at + 1) * slot).contains(&note.start.0))
            .map(|note| (note.start.0, note.length.0))
            .collect();
        spans.dedup();
        spans
    };
    assert_eq!(notes.len(), 12);
    assert_eq!(spans(0), [(0, slot)]);
    assert_eq!(spans(1), [(slot, slot / 2)]);
    assert_eq!(spans(2), [(2 * slot + slot / 4, slot * 3 / 8)]);
    // Offset 6 and Duration 8 run past the slot's end and are cut there.
    assert_eq!(spans(3), [(3 * slot + slot * 6 / 8, slot / 4)]);
}

#[test]
fn r19_semitone_entries_sit_on_the_root_and_degree_entries_follow_the_scale() {
    let free = input((0, BAR));
    assert_eq!(chord_keys(&free, "Major", 60, 0), [60, 64, 67]);
    assert_eq!(chord_keys(&free, "Dominant 9", 60, 0), [60, 64, 67, 70, 74]);
    // Without Scale Mode, degrees are those of the major scale on the root.
    assert_eq!(chord_keys(&free, "Triad", 62, 0), [62, 66, 69]);
    assert_eq!(chord_keys(&free, "Seventh", 60, 0), [60, 64, 67, 71]);
    // In Scale Mode they count the scale's notes from the scale note nearest
    // the root: a triad in C minor, then in D minor from a root off the scale.
    let c_minor = scaled(0, &MINOR, free.clone());
    assert_eq!(chord_keys(&c_minor, "Triad", 60, 0), [60, 63, 67]);
    assert_eq!(chord_keys(&c_minor, "Triad", 62, 0), [62, 65, 68]);
    assert_eq!(chord_keys(&c_minor, "Ninth", 60, 0), [60, 63, 67, 70, 74]);
    // Semitone entries off the scale move to the nearest key of it.
    assert_eq!(chord_keys(&c_minor, "Major", 60, 0), [60, 63, 67]);
    let d_major = scaled(2, &MAJOR, free);
    assert_eq!(chord_keys(&d_major, "Triad", 61, 0), [61, 64, 67]);
    assert_eq!(
        chord_keys(&d_major, "Triad", 60, 0),
        [59, 62, 66],
        "C is off D major: B is nearest"
    );
}

#[test]
fn r19_inversions_move_one_note_an_octave_each_and_negative_ones_go_down() {
    let free = input((0, BAR));
    assert_eq!(chord_keys(&free, "Major 7", 60, 1), [64, 67, 71, 72]);
    assert_eq!(chord_keys(&free, "Major 7", 60, 2), [67, 71, 72, 76]);
    assert_eq!(chord_keys(&free, "Major 7", 60, 3), [71, 72, 76, 79]);
    assert_eq!(chord_keys(&free, "Major 7", 60, -1), [59, 60, 64, 67]);
    assert_eq!(chord_keys(&free, "Major 7", 60, -2), [55, 59, 60, 64]);
    // A negative inversion is the same chord as the positive one an octave
    // lower: -1 of a four-note chord is +3 of it.
    let up: Vec<i16> = chord_keys(&free, "Major 7", 60, 3)
        .iter()
        .map(|k| k - 12)
        .collect();
    assert_eq!(chord_keys(&free, "Major 7", 60, -1), up);
    // In Scale Mode an inverted chord stays on the scale.
    let c_major = scaled(0, &MAJOR, free);
    assert_eq!(chord_keys(&c_major, "Triad", 60, 1), [64, 67, 72]);
    assert_eq!(chord_keys(&c_major, "Triad", 60, -1), [55, 60, 64]);
}

#[test]
fn r19_a_note_past_the_keyboard_moves_by_octaves_until_it_is_inside() {
    let free = input((0, BAR));
    // 125 + 7 = 132 folds to 120; 125 + 4 = 129 to 117.
    assert_eq!(chord_keys(&free, "Major", 125, 0), [117, 120, 125]);
    // Notes below 0 fold up: the inversions take the two top notes down to
    // -3 and -6, which come back as 9 and 6.
    assert_eq!(chord_keys(&free, "Major", 2, -2), [2, 6, 9]);
}

#[test]
fn r19_on_drum_rack_keys_every_note_is_a_pad_and_chords_do_not_repeat_one() {
    let mut rack = input((0, BAR));
    rack.drum_keys = Some(vec![36, 38, 42, 46]);
    for name in ["Major", "Triad", "Seventh", "Power"] {
        let found = chord_keys(&rack, name, 36, 0);
        assert!(!found.is_empty(), "{name}");
        for key in &found {
            assert!([36, 38, 42, 46].contains(&(*key as u8)), "{name}: {key}");
        }
        let mut unique = found.clone();
        unique.dedup();
        assert_eq!(unique, found, "{name}");
    }
    // A degree entry counts the pads and leaves out what is past the last;
    // a semitone entry moves each note to the nearest pad.
    assert_eq!(chord_keys(&rack, "Triad", 36, 0), [36, 42]);
    assert_eq!(chord_keys(&rack, "Triad", 38, 0), [38, 46]);
    assert_eq!(chord_keys(&rack, "Major", 36, 0), [36, 38, 42]);
}

#[test]
fn r20_in_scale_mode_a_new_stacks_starts_on_the_scales_root_and_a_set_root_is_written_on_a_scale_key()
 {
    let tool = Registry::native().find("stacks").expect("Stacks");
    let d_minor = scaled(2, &MINOR, input((0, BAR)));
    // Untouched: the scale's root in the octave from 60 up.
    let mut values = defaults();
    tool.start(&mut values, &d_minor);
    assert_eq!(values[parameter("chord_root")], ParamValue::Pitch(62));
    assert_eq!(stack_view(&values).expect("a view").chords[0].root, 62);
    let b_minor = scaled(11, &MINOR, input((0, BAR)));
    let mut values = defaults();
    tool.start(&mut values, &b_minor);
    assert_eq!(values[parameter("chord_root")], ParamValue::Pitch(71));
    // Not in Scale Mode, nothing moves, and a Stacks that was changed stays.
    let mut values = defaults();
    tool.start(&mut values, &input((0, BAR)));
    assert_eq!(values, defaults());
    let mut values = change(&defaults(), &[("chord_offset", whole(1))]).expect("settles");
    let before = values.clone();
    tool.start(&mut values, &d_minor);
    assert_eq!(values, before);
    // A root that was set is written on the nearest key of the scale, the
    // lower of two equally near: C is in D minor and stays, C sharp is
    // between C and D and takes C, D sharp between D and E takes D.
    for (set, written) in [(60, 60), (61, 60), (63, 62)] {
        let values = change(
            &defaults(),
            &[
                ("chord_root", ParamValue::Pitch(set)),
                ("chord", whole(bank_entry("Power"))),
            ],
        )
        .expect("settles");
        assert_eq!(values[parameter("chord_root")], ParamValue::Pitch(set));
        let lowest = keys(&notes_of(&d_minor, &values))[0];
        assert_eq!(lowest, written, "{set}");
    }
}

#[test]
fn r21_selecting_a_chord_changes_no_notes_but_choosing_its_entry_does() {
    let tool = Registry::native().find("stacks").expect("Stacks");
    assert!(!tool.changes_notes(&[parameter("selected")]));
    for name in [
        "progression",
        "chord",
        "chord_root",
        "chord_inversion",
        "chord_duration",
        "chord_offset",
        "bank",
    ] {
        assert!(tool.changes_notes(&[parameter(name)]), "{name}");
    }
    assert!(tool.changes_notes(&[parameter("selected"), parameter("chord")]));
}

#[test]
fn r23_a_loaded_bank_keeps_the_chords_by_place_and_a_place_past_its_end_takes_the_first() {
    let mut values = defaults();
    values = change(
        &values,
        &[(
            "progression",
            progression("[{\"chord\":0},{\"chord\":1},{\"chord\":20,\"inversion\":2}]"),
        )],
    )
    .expect("settles");
    let bank = ChordBank::parse(
        br#"{"format":"beatlayer-stacks","schema":1,"name":"Two","chords":[
            {"name":"Pair","semitones":[0,7]},{"name":"Trio","degrees":[0,2,4]}]}"#,
    )
    .expect("a bank");
    load_bank(&mut values, Some(bank.clone())).expect("loads");
    let view = stack_view(&values).expect("a view");
    assert_eq!(view.bank, bank);
    assert!(!view.built_in);
    assert_eq!(
        view.chords.iter().map(|c| c.chord).collect::<Vec<_>>(),
        [0, 1, 0],
        "chord 20 is past the new bank's end"
    );
    // Its inversion is held to the first entry's two notes.
    assert_eq!(view.chords[2].inversion, 1);
    // The bank is part of the settings: it survives as the text a set saves.
    let text = values[parameter("bank")].clone();
    assert!(matches!(&text, ParamValue::Structured(t) if t.contains("\"Pair\"")));
    let tool = Registry::native().find("stacks").expect("Stacks");
    assert!(tool.validate(&values).is_ok());
    // The built-in bank loads back, and a bank equal to it is the built-in one.
    load_bank(&mut values, None).expect("loads");
    assert!(stack_view(&values).expect("a view").built_in);
    load_bank(&mut values, Some(built_in_bank().clone())).expect("loads");
    assert!(stack_view(&values).expect("a view").built_in);
    assert_eq!(
        values[parameter("bank")],
        ParamValue::Structured(String::new())
    );
    // Changing the bank through the settings is settled the same way.
    let by_setting = change(
        &defaults(),
        &[("bank", ParamValue::Structured(bank.write()))],
    )
    .expect("settles");
    assert_eq!(stack_view(&by_setting).expect("a view").bank, bank);
    assert!(
        change(
            &defaults(),
            &[("bank", ParamValue::Structured("{}".to_owned()))]
        )
        .is_err()
    );
}

fn bank_text(chords: &str) -> String {
    format!("{{\"format\":\"{BANK_FORMAT}\",\"schema\":1,\"name\":\"B\",\"chords\":{chords}}}")
}

fn refusal(text: &str) -> String {
    ChordBank::parse(text.as_bytes())
        .expect_err("refused")
        .to_string()
}

#[test]
fn r24_a_valid_bank_round_trips_through_write_and_read_unchanged() {
    let bank = ChordBank::parse(
        bank_text(
            r#"[{"name":"A \"quoted\" name é","semitones":[0,3,7,48]},{"name":"Degrees","degrees":[0,2,4,6,8,10,12,14]}]"#,
        )
        .as_bytes(),
    )
    .expect("a bank");
    let written = bank.write();
    assert_eq!(ChordBank::parse(written.as_bytes()).expect("reads"), bank);
    assert_eq!(
        ChordBank::parse(bank.write().as_bytes())
            .expect("reads")
            .write(),
        written
    );
    // The fields are written in the order the format lists them.
    let order: Vec<usize> = ["\"format\"", "\"schema\"", "\"name\"", "\"chords\""]
        .iter()
        .map(|field| written.find(field).expect("a field"))
        .collect();
    assert!(order.windows(2).all(|pair| pair[0] < pair[1]));
    let name = written.find("\"name\": \"A").expect("an entry's name");
    assert!(name < written.find("\"semitones\"").expect("semitones"));
    // The built-in bank is a valid bank too.
    assert_eq!(
        ChordBank::parse(built_in_bank().write().as_bytes()).expect("reads"),
        *built_in_bank()
    );
    // Fields the format doesn't name are ignored.
    let extra = r#"{"format":"beatlayer-stacks","schema":1,"name":"B","comment":1,"chords":[{"name":"A","semitones":[0,7],"colour":"red"}]}"#;
    assert_eq!(
        ChordBank::parse(extra.as_bytes())
            .expect("reads")
            .chords
            .len(),
        1
    );
}

#[test]
fn r25_each_broken_file_is_refused_with_what_is_wrong_and_where() {
    // Over 1 MiB.
    let padding = " ".repeat(bl_midi_tools::MAX_BANK_BYTES);
    let big = format!(
        "{}{padding}",
        bank_text(r#"[{"name":"A","semitones":[0,7]}]"#)
    );
    assert!(refusal(&big).contains("bytes"), "{}", refusal(&big));
    // Not JSON, not an object, the wrong format, no schema, a newer schema.
    assert!(refusal("{ nope").contains("line 1"));
    assert!(refusal("[1,2]").contains("object"));
    assert!(
        refusal(r#"{"format":"beatlayer-preset","schema":1,"name":"B","chords":[]}"#)
            .starts_with("format:")
    );
    assert!(
        refusal(r#"{"format":"beatlayer-stacks","name":"B","chords":[]}"#).starts_with("schema:")
    );
    assert!(
        refusal(r#"{"format":"beatlayer-stacks","schema":2,"name":"B","chords":[]}"#)
            .contains("newer")
    );
    // The name and the list.
    assert!(
        refusal(r#"{"format":"beatlayer-stacks","schema":1,"chords":[]}"#).starts_with("name:")
    );
    assert!(refusal(&bank_text("[]")).starts_with("chords:"));
    let many = vec![r#"{"name":"A","semitones":[0,7]}"#; 257].join(",");
    assert!(refusal(&bank_text(&format!("[{many}]"))).contains("257"));
    let most = vec![r#"{"name":"A","semitones":[0,7]}"#; 256].join(",");
    assert!(ChordBank::parse(bank_text(&format!("[{most}]")).as_bytes()).is_ok());
    // An entry with nine notes, a falling list, a repeated number, a first
    // interval that isn't 0, one note, a number out of range, a fraction.
    let nine = refusal(&bank_text(
        r#"[{"name":"A","semitones":[0,7]},{"name":"Nine","degrees":[0,1,2,3,4,5,6,7,8]}]"#,
    ));
    assert!(
        nine.starts_with("chords[1] \"Nine\":") && nine.contains('9'),
        "{nine}"
    );
    let falling = refusal(&bank_text(r#"[{"name":"A","semitones":[0,7,4]}]"#));
    assert!(
        falling.contains("semitones[2]") && falling.contains("larger"),
        "{falling}"
    );
    assert!(refusal(&bank_text(r#"[{"name":"A","semitones":[0,4,4]}]"#)).contains("semitones[2]"));
    let first = refusal(&bank_text(r#"[{"name":"A","semitones":[1,4,7]}]"#));
    assert!(first.contains("semitones[0] must be 0"), "{first}");
    assert!(refusal(&bank_text(r#"[{"name":"A","semitones":[0]}]"#)).contains("2 to 8"));
    assert!(refusal(&bank_text(r#"[{"name":"A","semitones":[0,49]}]"#)).contains("0 to 48"));
    assert!(refusal(&bank_text(r#"[{"name":"A","semitones":[0,3.5]}]"#)).contains("semitones[1]"));
    assert!(refusal(&bank_text(r#"[{"name":"A","semitones":[0,-3]}]"#)).contains("semitones[1]"));
    // A name of 0 or 65 characters, neither or both lists, and a non-object.
    assert!(refusal(&bank_text(r#"[{"name":"","semitones":[0,7]}]"#)).contains("0 characters"));
    let long = "é".repeat(65);
    assert!(
        refusal(&bank_text(&format!(
            r#"[{{"name":"{long}","semitones":[0,7]}}]"#
        )))
        .contains("65 characters")
    );
    let ok = "é".repeat(64);
    assert!(
        ChordBank::parse(
            bank_text(&format!(r#"[{{"name":"{ok}","semitones":[0,7]}}]"#)).as_bytes()
        )
        .is_ok()
    );
    assert!(refusal(&bank_text(r#"[{"name":"A"}]"#)).contains("neither"));
    assert!(
        refusal(&bank_text(
            r#"[{"name":"A","semitones":[0,7],"degrees":[0,2]}]"#
        ))
        .contains("both")
    );
    assert!(refusal(&bank_text(r#"[3]"#)).starts_with("chords[0]:"));
    // The least a valid entry holds, and the greatest.
    assert!(ChordBank::parse(bank_text(r#"[{"name":"A","degrees":[0,48]}]"#).as_bytes()).is_ok());
}

#[test]
fn r27_the_built_in_bank_holds_the_twenty_seven_entries_in_order() {
    let bank = built_in_bank();
    assert_eq!(bank.name, "Beatlayer Chords");
    let names: Vec<&str> = bank
        .chords
        .iter()
        .map(|entry| entry.name.as_str())
        .collect();
    assert_eq!(
        names,
        [
            "Triad",
            "Seventh",
            "Ninth",
            "Sixth",
            "Add Nine",
            "Sus2",
            "Sus4",
            "Open Triad",
            "Fifth",
            "Major",
            "Minor",
            "Diminished",
            "Augmented",
            "Suspended 2",
            "Suspended 4",
            "Major 7",
            "Minor 7",
            "Dominant 7",
            "Half-Diminished 7",
            "Diminished 7",
            "Minor-Major 7",
            "Major 6",
            "Minor 6",
            "Major 9",
            "Minor 9",
            "Dominant 9",
            "Power"
        ]
    );
    assert_eq!(bank.chords.iter().filter(|entry| entry.degrees).count(), 9);
    assert_eq!(bank.chords[7].intervals, [0, 4, 9]);
    assert_eq!(bank.chords[23].intervals, [0, 4, 7, 11, 14]);
    assert!(bank.is_built_in());
}

proptest! {
    #[test]
    fn every_written_note_lies_in_its_chords_slot(
        chords in prop::collection::vec((0usize..27, 0u8..=127, -1i64..=1, 1i64..=8, 0i64..=7), 1..=16),
        bars in 1i64..=4,
        scale in prop_oneof![Just(None), Just(Some((0u8, MAJOR))), Just(Some((9u8, MINOR)))],
    ) {
        let list: Vec<String> = chords
            .iter()
            .map(|(chord, root, inversion, duration, offset)| {
                format!(
                    "{{\"chord\":{chord},\"root\":{root},\"inversion\":{inversion},\"duration\":{duration},\"offset\":{offset}}}"
                )
            })
            .collect();
        let values = change(&defaults(), &[("progression", progression(&format!("[{}]", list.join(","))))]);
        let values = values.expect("settles");
        let mut free = input((0, bars * BAR));
        if let Some((root, steps)) = scale {
            free = scaled(root, &steps, free);
        }
        let tool = Registry::native().find("stacks").expect("Stacks");
        let written = run(tool, &free, &values).expect("runs").notes;
        let count = chords.len() as i64;
        let span = bars * BAR;
        let mut remaining = written.iter().collect::<Vec<_>>();
        for (at, (_, _, _, duration, offset)) in chords.iter().enumerate() {
            let at = at as i64;
            let (from, to) = (span * at / count, span * (at + 1) / count);
            let inside = |note: &&ToolNote| note.start.0 >= from && note.end().0 <= to;
            let slot = to - from;
            let start = from + (slot * offset + 4).div_euclid(8);
            let end = (from + (slot * (offset + duration) + 4).div_euclid(8)).min(to);
            let here: Vec<&ToolNote> = remaining.iter().copied().filter(inside).collect();
            for note in &here {
                prop_assert_eq!((note.start.0, note.end().0), (start, end));
                prop_assert!((0..=127).contains(&note.key));
                if let Some(s) = &free.scale {
                    prop_assert!(bl_core::scale::holds(s.root, &s.steps, note.key as u8));
                }
            }
            remaining.retain(|note| !inside(note));
        }
        prop_assert!(remaining.is_empty(), "a note outside its slot: {remaining:?}");
    }
}

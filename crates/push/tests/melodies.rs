//! Melodies on Push 2 (`docs/specs/push-2-notes.md`, R6, R7 and R32 to R47):
//! pad pressure, the touch strip, the 64 Notes layout, the Scale menu, the
//! melodic sequencers and the arrows, on the simulated Push 2 with the real
//! Note Mode.

use bl_core::time::Ticks;
use bl_model::{
    Clip, ClipData, ClipId, Command, MidiData, Note, PadDirection, PadSpacing, ScaleName, SceneId,
};
use bl_push::content::StripUse;
use bl_push::curve::Workflow;
use bl_push::sysex;
use serde_json::{Value, json};

use crate::rig::Rig;

/// A rig with the Push 2 running and the first track selected.
fn rig() -> Rig {
    let mut rig = Rig::noting();
    rig.start();
    let track = rig.tracks[0];
    rig.model.selection.tracks = vec![track];
    // The first tick sets what the content wants, the second sends it.
    rig.advance(20);
    rig.advance(20);
    rig
}

/// Makes C major the scale, In Key, as a new set that turned Scale Mode on
/// has it.
fn c_major(rig: &mut Rig) {
    rig.apply(Command::SetScale {
        enabled: Some(true),
        root: Some(0),
        name: Some("major".to_owned()),
    });
    rig.advance(20);
}

fn scene(rig: &Rig, at: usize) -> SceneId {
    rig.editor.document().scenes().nth(at).unwrap().id
}

/// Puts a clip of one bar with `notes` in `track`'s slot at scene `at`, and
/// selects the slot.
fn put_clip(rig: &mut Rig, track_at: usize, at: usize, notes: Vec<Note>) -> ClipId {
    let track = rig.tracks[track_at];
    let id = rig.editor.new_id();
    let scene = scene(rig, at);
    rig.apply(Command::CreateSlotClip {
        track,
        scene,
        clip: Clip::new(
            id,
            Ticks::ZERO,
            Ticks::WHOLE,
            ClipData::Midi(MidiData::new(notes)),
        ),
    });
    rig.model.selection.slots = vec![(track, scene)];
    rig.advance(20);
    id
}

/// The size of a step at the default 1/16.
fn step() -> Ticks {
    bl_push::note::Size::default().ticks()
}

fn note(key: u8, start: Ticks) -> Note {
    Note::new(key, start, step(), 100)
}

/// The commands with `id` among `commands`, with their arguments.
fn run(commands: &[String], id: &str) -> Vec<Value> {
    commands
        .iter()
        .filter_map(|c| c.strip_prefix(id)?.strip_prefix(' '))
        .map(|args| serde_json::from_str(args).unwrap())
        .collect()
}

fn pad(rig: &Rig, row: u8, column: u8) -> [u8; 3] {
    rig.sim.colour(&format!("pad_{row}_{column}")).unwrap()
}

fn track_colour(rig: &Rig) -> [u8; 3] {
    let colour = rig.editor.document().track(rig.tracks[0]).unwrap().colour();
    bl_model::swatch_rgb(&bl_model::DEFAULT_TRACK_PALETTE, colour)
}

const WHITE: [u8; 3] = [204, 204, 204];
const GREEN: [u8; 3] = [0, 255, 0];
const RED: [u8; 3] = [255, 0, 0];
const OFF: [u8; 3] = [0, 0, 0];

fn tap_pad(rig: &mut Rig, row: u8, column: u8) {
    assert!(rig.sim.press_pad(row, column, 100));
    rig.settle();
    rig.advance(20);
    assert!(rig.sim.release_pad(row, column));
    rig.settle();
    rig.advance(20);
}

fn hold(rig: &mut Rig, name: &str) {
    assert!(rig.sim.press(name));
    rig.settle();
    rig.advance(400);
}

fn release(rig: &mut Rig, name: &str) {
    assert!(rig.sim.let_go(name));
    rig.settle();
    rig.advance(20);
}

fn melody(rig: &Rig) -> bl_push::note::Melody {
    rig.snapshot()
        .note
        .and_then(|note| note.melody)
        .expect("a melodic track")
}

fn layout(rig: &Rig) -> String {
    rig.snapshot().note.unwrap().layout
}

fn text(rig: &Rig) -> Vec<String> {
    rig.snapshot().screen.text()
}

fn sent(rig: &mut Rig) -> Vec<Vec<u8>> {
    rig.reached_tracks()
        .iter()
        .map(|event| event.message.bytes().to_vec())
        .collect()
}

// Spec: push-2-notes R6. Aftertouch is set to polyphonic key pressure, and a
// held pad's pressure reaches the track as polyphonic key pressure on the
// note that pad plays.
#[test]
fn a_pads_pressure_reaches_the_track_as_key_pressure_on_its_note() {
    let mut rig = rig();
    assert!(rig.sim.sysex().contains(&sysex::aftertouch_mode(true)));
    rig.reached_tracks();
    assert!(rig.sim.press_pad(0, 1, 100));
    rig.settle();
    rig.advance(20);
    assert!(rig.sim.pressure(0, 1, 70));
    rig.settle();
    rig.advance(20);
    assert!(rig.sim.release_pad(0, 1));
    rig.settle();
    rig.advance(20);
    // Chromatic from C1 with the default scale off: the second pad is C♯1.
    assert_eq!(
        sent(&mut rig),
        [vec![0x90, 37, 100], vec![0xA0, 37, 70], vec![0x80, 37, 64]]
    );
}

// Spec: push-2-notes R7. 64 Notes' strip is pitch bend, 8192 at the centre;
// Select and a tap switch it to the modulation wheel and back, and the display
// notifies the use.
#[test]
fn the_strip_bends_pitch_and_a_select_tap_switches_it_to_modulation() {
    let mut rig = rig();
    assert_eq!(rig.snapshot().strip, StripUse::PitchBend);
    rig.reached_tracks();
    assert!(rig.sim.strip_touch());
    assert!(rig.sim.strip_move(0.5));
    assert!(rig.sim.strip_move(1.0));
    assert!(rig.sim.strip_release());
    rig.settle();
    rig.advance(20);
    assert_eq!(
        sent(&mut rig),
        [vec![0xE0, 0, 64], vec![0xE0, 64, 127]],
        "14 bits: the centre is 8192"
    );

    hold(&mut rig, "select");
    assert!(rig.sim.strip_touch());
    rig.settle();
    rig.advance(20);
    assert!(rig.sim.strip_release());
    release(&mut rig, "select");
    assert!(
        text(&rig).contains(&"notification: Touch Strip: Modulation Wheel".to_owned()),
        "{:?}",
        text(&rig)
    );
    assert_eq!(rig.snapshot().strip, StripUse::Modulation);
    assert!(melody(&rig).modulation);
    rig.reached_tracks();
    assert!(rig.sim.strip_touch());
    assert!(rig.sim.strip_move(0.5));
    assert!(rig.sim.strip_release());
    rig.settle();
    rig.advance(20);
    assert_eq!(sent(&mut rig), [vec![0xB0, 1, 64]]);

    hold(&mut rig, "select");
    assert!(rig.sim.strip_touch());
    rig.settle();
    assert!(rig.sim.strip_release());
    release(&mut rig, "select");
    assert_eq!(rig.snapshot().strip, StripUse::PitchBend, "and back");
}

// Spec: push-2-notes R32, R34. 64 Notes in C major: the root in the track's
// colour, the scale's other notes white, and Chromatic leaves the notes
// outside the scale unlit; every pad of a playing note is green, red while
// the track records.
#[test]
fn the_pads_colours_say_what_they_hold() {
    let mut rig = rig();
    c_major(&mut rig);
    let track = track_colour(&rig);
    assert_eq!(pad(&rig, 0, 0), track, "the root");
    assert_eq!(pad(&rig, 0, 1), WHITE, "D");
    assert_eq!(pad(&rig, 1, 0), WHITE, "a row up is three degrees: F");
    // The root is the track's colour wherever it recurs: seven degrees on.
    assert_eq!(pad(&rig, 1, 4), track);
    rig.apply(Command::SetPadLayout {
        in_key: Some(false),
        fixed: None,
        spacing: None,
        direction: None,
    });
    rig.advance(20);
    assert_eq!(pad(&rig, 0, 1), OFF, "C♯ is outside C major");
    assert_eq!(pad(&rig, 0, 2), WHITE, "D");

    // Chromatic 4ths: F (41) is on two pads, which both turn green.
    assert!(rig.sim.press_pad(0, 5, 100));
    rig.settle();
    rig.advance(20);
    assert_eq!(pad(&rig, 0, 5), GREEN);
    assert_eq!(pad(&rig, 1, 0), GREEN, "every pad of the note");
    assert!(rig.sim.release_pad(0, 5));
    rig.settle();
    rig.advance(20);
    assert_eq!(pad(&rig, 1, 0), WHITE);

    // Recording.
    let track_id = rig.tracks[0];
    rig.apply(Command::SetArm {
        track: track_id,
        arm: true,
    });
    rig.model.transport.recording = true;
    assert!(rig.sim.press_pad(0, 5, 100));
    rig.settle();
    rig.advance(20);
    assert_eq!(pad(&rig, 0, 5), RED, "red while recording");
    assert!(rig.sim.release_pad(0, 5));
    rig.settle();
}

// Spec: push-2-notes R33. Octave moves every pad an octave and goes dark where
// the bottom-left pad can't move further; a pad whose note is past 127 is
// unlit and silent.
#[test]
fn octave_stops_within_the_keyboard_and_its_led_goes_off() {
    let mut rig = rig();
    c_major(&mut rig);
    let lit = |rig: &Rig, name: &str| rig.sim.led(name).unwrap().colour != 0;
    assert!(lit(&rig, "octave_up") && lit(&rig, "octave_down"));
    for _ in 0..7 {
        assert!(lit(&rig, "octave_up"));
        rig.tap("octave_up");
    }
    assert_eq!(melody(&rig).octave, 7);
    assert!(!lit(&rig, "octave_up"), "the end of the keyboard");
    rig.tap("octave_up");
    assert_eq!(melody(&rig).octave, 7, "it doesn't move past it");
    assert_eq!(pad(&rig, 7, 7), OFF, "past note 127");
    rig.reached_tracks();
    assert!(rig.sim.press_pad(7, 7, 100));
    rig.settle();
    rig.advance(20);
    assert!(rig.sim.release_pad(7, 7));
    rig.settle();
    assert!(sent(&mut rig).is_empty(), "silent");
    for _ in 0..10 {
        rig.tap("octave_down");
    }
    assert_eq!(melody(&rig).octave, -3);
    assert!(!lit(&rig, "octave_down"));
    assert_eq!(melody(&rig).low, 0);
}

// Spec: push-2-notes R35, R36. The Scale menu: keys on the display buttons,
// the scale list on encoders 2 to 7, Layout and Direction on 1 and 8, In Key
// and Fixed on the lower buttons; the key and scale are the set's current
// scale, or the selected clips'.
#[test]
fn the_scale_menu_sets_the_scale_and_the_layout_options() {
    let mut rig = rig();
    assert!(rig.sim.led("scale").unwrap().colour != 0, "Scale is on");
    rig.tap("scale");
    assert!(rig.snapshot().menu.is_some());
    let lines = text(&rig);
    for expected in [
        "upper 2: C",
        "upper 3: G",
        "upper 7: B",
        "lower 2: F",
        "lower 3: B♭",
        "lower 7: G♭",
        "lower 1: In Key (selected)",
        "lower 8: Fixed",
        "column 1: Layout = 4ths",
        "column 8: Direction = Vertical",
    ] {
        assert!(lines.iter().any(|l| l.starts_with(expected)), "{expected}");
    }
    let first: Vec<_> = ScaleName::ALL.iter().take(3).map(|n| n.label()).collect();
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("list 2") && l.contains(&format!("*{}", first[0]))),
        "{lines:?}"
    );
    // The chosen key is white and the others grey.
    assert_eq!(rig.sim.led("upper_2").unwrap().colour, 122);
    rig.commands();

    rig.tap("upper_3");
    rig.tap("lower_3");
    rig.tap("lower_1");
    rig.tap("lower_8");
    assert_eq!(
        rig.commands(),
        [
            r#"song.set_scale {"root":7}"#,
            r#"song.set_scale {"root":10}"#,
            r#"song.set_pad_layout {"in_key":false}"#,
            r#"song.set_pad_layout {"fixed":true}"#,
        ]
    );

    // Encoders: Layout, Direction and the scale list.
    assert!(rig.sim.turn(0, 10));
    assert!(rig.sim.turn(7, 10));
    rig.settle();
    assert!(rig.sim.touch(2));
    assert!(rig.sim.turn(2, 10));
    rig.settle();
    assert!(rig.sim.release(2));
    rig.settle();
    rig.advance(20);
    let commands = rig.commands();
    assert_eq!(
        commands[..3],
        [
            r#"song.set_pad_layout {"spacing":"thirds"}"#.to_owned(),
            r#"song.set_pad_layout {"direction":"horizontal"}"#.to_owned(),
            format!(r#"song.set_scale {{"name":"{}"}}"#, ScaleName::ALL[1].id()),
        ]
    );
    assert!(
        commands[3].starts_with("end "),
        "one undo step: {commands:?}"
    );

    // With a clip selected the choosers set the clip's scale.
    let clip = put_clip(&mut rig, 0, 0, Vec::new());
    rig.tap("upper_2");
    assert_eq!(
        run(&rig.commands(), "clip.set_scale"),
        [json!({ "clips": [clip], "root": 0 })]
    );

    // Tapping Scale again closes the menu.
    rig.tap("scale");
    assert!(rig.snapshot().menu.is_none());
}

// Spec: push-2-notes R36. The pads follow the current scale whenever it
// changes, by a selection or an edit.
#[test]
fn the_pads_follow_the_current_scale() {
    let mut rig = rig();
    c_major(&mut rig);
    assert_eq!(pad(&rig, 0, 0), track_colour(&rig));
    rig.apply(Command::SetScale {
        enabled: None,
        root: Some(2),
        name: Some("minor".to_owned()),
    });
    rig.advance(20);
    // D minor: the bottom left is the root D1 (38), and E♭ is a note of the
    // scale, so the pad that played E in C major now plays F.
    assert_eq!(melody(&rig).low, 38);
    rig.reached_tracks();
    assert!(rig.sim.press_pad(0, 0, 90));
    rig.settle();
    assert!(rig.sim.release_pad(0, 0));
    rig.settle();
    assert_eq!(sent(&mut rig)[0], [0x90, 38, 90]);
}

// Spec: push-2-notes R38. Delete and a pad delete every note of its pitch
// inside the loop of the clip.
#[test]
fn delete_and_a_pad_delete_one_pitch_inside_the_loop() {
    let mut rig = rig();
    c_major(&mut rig);
    let clip = put_clip(
        &mut rig,
        0,
        0,
        vec![
            note(36, Ticks::ZERO),
            note(38, step()),
            note(36, step() * 2),
        ],
    );
    rig.commands();
    hold(&mut rig, "delete");
    tap_pad(&mut rig, 0, 0);
    release(&mut rig, "delete");
    assert_eq!(
        run(&rig.commands(), "note.delete"),
        [json!({ "clip": clip, "notes": [0, 2] })]
    );
    assert!(sent(&mut rig).is_empty(), "nothing plays");
}

// Spec: push-2-notes R39. Layout steps a melodic track through its three
// layouts and back; holding it shows the loop length pads in the top row.
#[test]
fn layout_steps_through_the_melodic_layouts_and_shows_the_loop_pads() {
    let mut rig = rig();
    c_major(&mut rig);
    let clip = put_clip(&mut rig, 0, 0, Vec::new());
    assert_eq!(layout(&rig), "64 Notes");
    for expected in [
        "Melodic Sequencer",
        "Melodic Sequencer + 32 Notes",
        "64 Notes",
    ] {
        rig.tap("layout");
        assert_eq!(layout(&rig), expected);
    }
    rig.commands();
    hold(&mut rig, "layout");
    assert_eq!(
        rig.snapshot().note.unwrap().overlay.as_deref(),
        Some("Loop Length Pads")
    );
    // Page 3 is outside the one-bar loop: tapping it sets the loop to it.
    tap_pad(&mut rig, 7, 3);
    release(&mut rig, "layout");
    assert_eq!(layout(&rig), "64 Notes", "no step after a used hold");
    let page = step() * 8;
    assert_eq!(
        run(&rig.commands(), "clip.set_midi_loop"),
        [json!({ "clip": clip, "start": page * 3, "end": page * 4 })]
    );
    assert!(rig.snapshot().note.unwrap().overlay.is_none());

    // Shift and Layout keep the pads; Layout alone hides them without
    // stepping.
    assert!(rig.sim.press("shift"));
    rig.settle();
    rig.advance(20);
    assert!(rig.sim.press("layout"));
    rig.settle();
    rig.advance(20);
    assert!(rig.sim.let_go("layout"));
    assert!(rig.sim.let_go("shift"));
    rig.settle();
    rig.advance(20);
    assert!(rig.snapshot().note.unwrap().overlay_locked);
    rig.tap("layout");
    assert!(!rig.snapshot().note.unwrap().overlay_locked);
    assert_eq!(layout(&rig), "64 Notes");
}

// Spec: push-2-notes R40, R41, R42. The Melodic Sequencer: a row for each
// scale note, steps add, delete and deactivate, Octave, Shift and Octave and
// the strip move the rows, the display notifies the range and the strip's
// LEDs show it and the clip's notes outside it.
#[test]
fn the_melodic_sequencer_edits_steps_and_moves_its_rows() {
    let mut rig = rig();
    c_major(&mut rig);
    let clip = put_clip(
        &mut rig,
        0,
        0,
        vec![note(38, step() * 2), note(100, Ticks::ZERO)],
    );
    rig.tap("layout");
    assert_eq!(layout(&rig), "Melodic Sequencer");
    assert_eq!(melody(&rig).range, "C1 to C2");
    // Rows are white at the root, grey in the scale.
    assert_eq!(pad(&rig, 0, 5), WHITE);
    rig.commands();

    // An empty step adds the row's note; a step with one deletes it, or with
    // Mute held deactivates it.
    tap_pad(&mut rig, 2, 0);
    let adds = run(&rig.commands(), "note.add");
    assert_eq!(
        adds,
        [json!({ "clip": clip, "key": 40, "start": 0, "length": step(), "velocity": 100 })]
    );
    tap_pad(&mut rig, 1, 2);
    assert_eq!(
        run(&rig.commands(), "note.delete"),
        [json!({ "clip": clip, "notes": [1] })]
    );
    hold(&mut rig, "mute");
    tap_pad(&mut rig, 1, 2);
    release(&mut rig, "mute");
    assert_eq!(
        run(&rig.commands(), "note.deactivate"),
        [json!({ "clip": clip, "notes": [1] })]
    );

    // Octave: an octave; with Shift, one scale note.
    rig.tap("octave_up");
    assert_eq!(melody(&rig).range, "C2 to C3");
    assert!(text(&rig).contains(&"notification: C2 to C3".to_owned()));
    assert!(rig.sim.press("shift"));
    rig.settle();
    rig.advance(20);
    assert!(rig.sim.press("octave_up"));
    rig.settle();
    rig.advance(20);
    assert!(rig.sim.let_go("octave_up"));
    assert!(rig.sim.let_go("shift"));
    rig.settle();
    rig.advance(20);
    assert_eq!(melody(&rig).low, 50, "one scale note up from C2");

    // The strip.
    assert!(rig.sim.strip_touch());
    assert!(rig.sim.strip_move(0.0));
    rig.settle();
    rig.advance(20);
    assert_eq!(melody(&rig).low, 0);
    let leds = rig.snapshot().strip_leds;
    assert_eq!(leds[0], 7, "the range is bright");
    assert_eq!(leds[30], 0);
    assert_eq!(leds[100 * 31 / 128], 2, "the clip's note outside it is dim");
    assert!(rig.sim.strip_move(1.0));
    rig.settle();
    rig.advance(20);
    assert_eq!(melody(&rig).high, 127);
    assert!(rig.sim.strip_release());
    rig.settle();
}

// Spec: push-2-notes R40. Chromatic rows are semitones, those outside the
// scale unlit but usable.
#[test]
fn chromatic_rows_are_semitones_and_those_outside_the_scale_are_unlit_but_usable() {
    let mut rig = rig();
    c_major(&mut rig);
    let clip = put_clip(&mut rig, 0, 0, Vec::new());
    rig.apply(Command::SetPadLayout {
        in_key: Some(false),
        fixed: None,
        spacing: None,
        direction: None,
    });
    rig.tap("layout");
    assert_eq!(melody(&rig).range, "C1 to G1");
    assert_eq!(pad(&rig, 1, 4), OFF, "C♯ is outside the scale");
    rig.commands();
    tap_pad(&mut rig, 1, 4);
    assert_eq!(
        run(&rig.commands(), "note.add"),
        [
            json!({ "clip": clip, "key": 37, "start": step() * 4, "length": step(), "velocity": 100 })
        ]
    );
}

// Spec: push-2-notes R43, R44. The Melodic Sequencer + 32 Notes: tapping
// plays and selects notes, pads held together are selected together, steps
// take the selected notes, holding a step shows its notes and tapping one
// removes it, and several steps held together each take them.
#[test]
fn the_32_notes_select_notes_and_the_steps_take_them() {
    let mut rig = rig();
    c_major(&mut rig);
    let clip = put_clip(
        &mut rig,
        0,
        0,
        vec![note(36, Ticks::ZERO), note(38, Ticks::ZERO)],
    );
    rig.tap("layout");
    rig.tap("layout");
    assert_eq!(layout(&rig), "Melodic Sequencer + 32 Notes");
    let track = track_colour(&rig);
    assert_eq!(pad(&rig, 0, 0), track, "the root");
    assert_eq!(pad(&rig, 0, 1), WHITE);
    // Eight notes to a row from the bottom left: row 1 starts at the ninth
    // scale note, D2.
    rig.reached_tracks();
    tap_pad(&mut rig, 1, 0);
    assert_eq!(
        sent(&mut rig),
        [vec![0x90, 50, 100], vec![0x80, 50, 64]],
        "tapping plays"
    );
    assert_eq!(melody(&rig).selected_notes, [50]);
    // Select and a tap selects without playing.
    hold(&mut rig, "select");
    tap_pad(&mut rig, 0, 2);
    release(&mut rig, "select");
    assert_eq!(melody(&rig).selected_notes, [40]);
    assert!(sent(&mut rig).is_empty());
    // Pads held together are selected together.
    assert!(rig.sim.press_pad(0, 0, 100));
    assert!(rig.sim.press_pad(0, 1, 100));
    rig.settle();
    rig.advance(20);
    assert_eq!(melody(&rig).selected_notes, [36, 38]);
    assert!(rig.sim.release_pad(0, 0));
    assert!(rig.sim.release_pad(0, 1));
    rig.settle();
    rig.advance(20);
    assert_eq!(pad(&rig, 0, 0), bl_push::note::grid::lighter(track));

    // A step takes every selected note it lacks: step 3 gets both.
    rig.commands();
    tap_pad(&mut rig, 4, 2);
    let adds = run(&rig.commands(), "note.add");
    let keys: Vec<_> = adds.iter().map(|a| a["key"].clone()).collect();
    assert_eq!(keys, [json!(36), json!(38)]);
    assert!(adds.iter().all(|a| a["start"] == json!(step() * 2)));

    // Holding a step shows its notes in the lighter colour; tapping one
    // removes it from the step.
    rig.model.selection.slots = vec![(rig.tracks[0], scene(&rig, 0))];
    tap_pad(&mut rig, 0, 4);
    assert_eq!(melody(&rig).selected_notes, [43]);
    assert!(rig.sim.press_pad(4, 0, 100));
    rig.settle();
    rig.advance(20);
    assert_eq!(pad(&rig, 0, 0), bl_push::note::grid::lighter(track));
    assert_eq!(pad(&rig, 0, 1), bl_push::note::grid::lighter(track));
    assert_eq!(pad(&rig, 0, 2), WHITE);
    rig.commands();
    tap_pad(&mut rig, 0, 1);
    assert_eq!(
        run(&rig.commands(), "note.delete"),
        [json!({ "clip": clip, "notes": [1] })]
    );
    assert!(rig.sim.release_pad(4, 0));
    rig.settle();
    rig.advance(20);

    // Several steps held together each take the selected note.
    rig.commands();
    assert!(rig.sim.press_pad(4, 5, 100));
    assert!(rig.sim.press_pad(4, 6, 100));
    rig.settle();
    rig.advance(20);
    let adds = run(&rig.commands(), "note.add");
    let starts: Vec<_> = adds.iter().map(|a| a["start"].clone()).collect();
    assert_eq!(starts, [json!(step() * 5), json!(step() * 6)]);
    assert!(adds.iter().all(|a| a["key"] == json!(43)));
    assert!(rig.sim.release_pad(4, 5));
    assert!(rig.sim.release_pad(4, 6));
    rig.settle();

    // Delete with a step deletes its notes.
    rig.commands();
    hold(&mut rig, "delete");
    tap_pad(&mut rig, 4, 0);
    release(&mut rig, "delete");
    assert_eq!(
        run(&rig.commands(), "note.delete"),
        [json!({ "clip": clip, "notes": [0, 1] })]
    );
}

// Spec: push-2-notes R45. Left and Right select the track before or after,
// and the ring moves a track at its edge.
#[test]
fn left_and_right_select_neighbouring_tracks_and_move_the_ring() {
    let mut rig = rig();
    rig.commands();
    rig.tap("right");
    let commands = rig.commands();
    assert_eq!(
        run(&commands, "session.select"),
        [json!({ "track": rig.tracks[1] })]
    );
    assert_eq!(
        run(&commands, "arrangement.select_tracks"),
        [json!({ "tracks": [rig.tracks[1]] })]
    );
    assert!(run(&commands, "control_surface.move_ring").is_empty());
    // The eighth track is the last the ring shows: the ninth moves it.
    rig.model.selection.tracks = vec![rig.tracks[7]];
    rig.advance(20);
    rig.commands();
    rig.tap("right");
    assert_eq!(
        run(&rig.commands(), "control_surface.move_ring"),
        [json!({ "row": 1, "step": "right" })]
    );
    // The first track has none before it.
    rig.model.selection.tracks = vec![rig.tracks[0]];
    rig.advance(20);
    assert_eq!(rig.sim.led("left").unwrap().colour, 0);
    rig.commands();
    rig.tap("left");
    assert!(rig.commands().is_empty());
}

// Spec: push-2-notes R46. Up and Down select the scene above or below and
// launch it as Legato at once: every clip of the scene with Workflow on
// Scene, the selected track's with Workflow on Clip; the first scene has no
// Up and the last no Down.
#[test]
fn up_and_down_launch_the_neighbouring_scene_as_legato() {
    let mut rig = rig();
    let (first, second) = (rig.tracks[0], rig.tracks[1]);
    put_clip(&mut rig, 0, 0, Vec::new());
    put_clip(&mut rig, 0, 1, Vec::new());
    put_clip(&mut rig, 1, 1, Vec::new());
    let (s0, s1) = (scene(&rig, 0), scene(&rig, 1));
    rig.model.selection.slots = Vec::new();
    rig.model.selection.scenes = vec![s0];
    rig.advance(20);
    assert_eq!(rig.sim.led("up").unwrap().colour, 0, "no scene above");
    assert_ne!(rig.sim.led("down").unwrap().colour, 0);

    rig.commands();
    rig.tap("down");
    let commands = rig.commands();
    assert_eq!(run(&commands, "session.select"), [json!({ "scene": s1 })]);
    assert_eq!(
        run(&commands, "session.launch"),
        [
            json!({ "track": first, "scene": s1, "legato": true }),
            json!({ "track": second, "scene": s1, "legato": true }),
        ]
    );

    // Workflow on Clip launches only the selected track's slot, and nothing
    // from an empty one.
    let mut setup = rig.shared.setup();
    setup.workflow = Workflow::Clip;
    rig.shared.set_setup(setup);
    rig.model.selection.scenes = vec![s0];
    rig.advance(20);
    rig.commands();
    rig.tap("down");
    let commands = rig.commands();
    assert_eq!(
        run(&commands, "session.select"),
        [json!({ "track": first, "scene": s1 })]
    );
    assert_eq!(
        run(&commands, "session.launch"),
        [json!({ "track": first, "scene": s1, "legato": true })]
    );
    rig.model.selection.tracks = vec![rig.tracks[2]];
    rig.model.selection.scenes = vec![s0];
    rig.advance(20);
    rig.commands();
    rig.tap("down");
    assert!(run(&rig.commands(), "session.launch").is_empty());
}

// Spec: push-2-notes R47. Browse Mode keeps the arrows for its lists.
#[test]
fn the_arrows_belong_to_browse_mode_while_it_shows() {
    let mut rig = rig();
    rig.tap("browse");
    rig.commands();
    rig.tap("right");
    assert!(run(&rig.commands(), "session.select").is_empty());
}

// Spec: push-2-notes R37. The layout options are the set's, in the state an
// agent reads.
#[test]
fn the_state_reports_the_options_the_set_holds() {
    let mut rig = rig();
    rig.apply(Command::SetPadLayout {
        in_key: Some(false),
        fixed: Some(true),
        spacing: Some(PadSpacing::Sequent),
        direction: Some(PadDirection::Horizontal),
    });
    rig.advance(20);
    let melody = melody(&rig);
    assert!(!melody.in_key && melody.fixed);
    assert_eq!(melody.spacing, "Sequent");
    assert_eq!(melody.direction, "Horizontal");
}

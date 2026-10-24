//! The MIDI Tools framework, driven headless through `Shell::run` and
//! `Shell::gesture` with the test tools (WP-6.11; the MIDI Tools spec, R1 to
//! R26). `test_transpose` moves the notes it takes by scale degrees, cuts
//! them into pieces and spreads their velocity from the seed; `test_fill`
//! writes a note at every step of its range.

use std::path::{Path, PathBuf};

use bl_core::time::Ticks;
use bl_model::{
    Breakpoint, Chain, Clip, ClipData, ClipId, Command, Document, Editor, MidiData, Note, RackKind,
    Scale, ScaleName, TrackId,
};
use bl_shell::audio::AudioHost;
use bl_shell::clip_view::notes_in_order;
use bl_shell::files::{SetStore, factory_default_set};
use bl_shell::midi_tool_commands::{self as ids, MidiToolState, Panel, ToolList};
use bl_shell::settings::Preferences;
use bl_shell::{GesturePhase, Invocation, Shell};
use serde_json::{Value, json};

const BAR: i64 = Ticks::WHOLE.get();
const QUARTER: i64 = Ticks::QUARTER.get();
const SIXTEENTH: i64 = QUARTER / 4;

/// Opens a set held in memory.
struct Preloaded(Option<Editor>);

impl SetStore for Preloaded {
    fn open(&mut self, path: &Path) -> Result<(Editor, PathBuf), String> {
        let editor = self.0.take().ok_or("opened already")?;
        Ok((editor, path.to_owned()))
    }

    fn save(&mut self, _: &Path, _: &Editor) -> Result<Vec<String>, String> {
        Err("not saved".to_owned())
    }

    fn save_as(&mut self, _: &Path, _: &Editor) -> Result<(PathBuf, Vec<String>), String> {
        Err("not saved".to_owned())
    }

    fn close(&mut self) {}
}

fn note(key: u8, start: i64, length: i64) -> Note {
    Note::new(key, Ticks(start), Ticks(length), 100)
}

/// A song with a MIDI clip on the first MIDI track, selected, as a click on
/// it leaves it.
struct Song {
    shell: Shell,
    clip: ClipId,
}

fn song(notes: Vec<Note>) -> Song {
    song_with(MidiData::new(notes), Scale::default())
}

fn song_with(midi: MidiData, scale: Scale) -> Song {
    song_on(midi, scale, |_, _| {})
}

/// A song whose first MIDI track `setup` may fit out before the set opens.
fn song_on(midi: MidiData, scale: Scale, setup: impl FnOnce(&mut Editor, TrackId)) -> Song {
    let mut editor = factory_default_set();
    let track = editor.document().top_level().next().unwrap();
    setup(&mut editor, track);
    let clip = editor.new_id();
    let mut value = Clip::new(clip, Ticks::ZERO, Ticks(4 * BAR), ClipData::Midi(midi));
    value.scale = scale;
    editor
        .apply(Command::CreateClip { track, clip: value })
        .unwrap();
    editor.clear_history().unwrap();
    let mut shell = Shell::new(
        Preferences::default(),
        None,
        AudioHost::null(Default::default()),
        Box::new(Preloaded(Some(editor))),
    );
    shell
        .run("set.open", json!({ "path": "Tools.blset" }))
        .unwrap();
    shell
        .run("arrangement.select_clips", json!({ "clips": [clip] }))
        .unwrap();
    Song { shell, clip }
}

impl Song {
    fn doc(&self) -> &Document {
        self.shell.editor().document()
    }

    fn notes(&self) -> Vec<Note> {
        match &self.doc().clip(self.clip).unwrap().data {
            ClipData::Midi(midi) => notes_in_order(midi),
            ClipData::Audio(_) => panic!("the clip is MIDI"),
        }
    }

    fn keys(&self) -> Vec<u8> {
        self.notes().iter().map(|note| note.key).collect()
    }

    fn steps(&self) -> usize {
        self.shell.editor().undo_history().len()
    }

    fn last_step(&self) -> Option<String> {
        self.shell.editor().undo_history().next().map(str::to_owned)
    }

    /// Runs a command and reads the state it returns.
    fn tool(&mut self, id: &str, args: Value) -> MidiToolState {
        serde_json::from_value(
            self.shell
                .run(id, args)
                .unwrap_or_else(|e| panic!("{id} failed: {e}")),
        )
        .unwrap()
    }

    fn state(&mut self) -> MidiToolState {
        self.tool(ids::STATE, json!({}))
    }

    fn refused(&mut self, id: &str, args: Value) -> String {
        self.shell
            .run(id, args)
            .expect_err(&format!("{id} should be refused"))
            .to_string()
    }

    fn select_notes(&mut self, places: &[u32]) {
        self.shell
            .run("clip_view.select_notes", json!({ "notes": places }))
            .unwrap();
    }

    fn select_time(&mut self, start: i64, end: i64) {
        self.shell
            .run(
                "clip_view.select_time",
                json!({ "start": start, "end": end }),
            )
            .unwrap();
    }

    /// Changes `test_transpose`'s degrees, with Auto on.
    fn transpose(&mut self, degrees: i64) -> MidiToolState {
        self.tool(
            ids::SET,
            json!({ "tool": "test_transpose", "values": { "degrees": degrees } }),
        )
    }

    fn apply_transpose(&mut self, degrees: i64) -> MidiToolState {
        self.tool(
            ids::APPLY,
            json!({ "tool": "test_transpose", "values": { "degrees": degrees } }),
        )
    }
}

fn three() -> Vec<Note> {
    vec![
        note(60, 0, QUARTER),
        note(62, QUARTER, QUARTER),
        note(64, 2 * QUARTER, QUARTER),
    ]
}

#[test]
fn r1_r23_the_panels_are_view_state_and_the_commands_read_back() {
    // Spec: midi-tools R1, R23. A new Clip View shows Pitch and Time
    // Utilities; showing a panel is no undo step and the set isn't dirtied.
    let mut song = song(three());
    assert_eq!(song.state().panel, Panel::Utilities);
    let steps = song.steps();
    let state = song.tool(ids::SHOW_PANEL, json!({ "panel": "transform" }));
    assert_eq!(state.panel, Panel::Transform);
    assert_eq!(
        song.tool(ids::SHOW_PANEL, json!({ "panel": "generate" }))
            .panel,
        Panel::Generate
    );
    assert_eq!(song.steps(), steps);
    assert!(!song.shell.is_dirty());
}

#[test]
fn r2_r3_r5_the_list_describes_every_tool_in_order() {
    // Spec: midi-tools R2, R3, R5, R23. Each tool, with its kind and its
    // parameters' kinds, ranges, defaults, units, and descriptions.
    let mut song = song(Vec::new());
    let list: ToolList =
        serde_json::from_value(song.shell.run(ids::LIST, json!({})).unwrap()).unwrap();
    let names: Vec<&str> = list.tools.iter().map(|tool| tool.id.as_str()).collect();
    assert_eq!(
        names,
        [
            "arpeggiate",
            "chop",
            "connect",
            "ornament",
            "quantize",
            "recombine",
            "span",
            "strum",
            "time_warp",
            "velocity_shaper",
            "test_transpose",
            "test_fill"
        ],
        "transformations, then generators"
    );
    let transpose = &list.tools[10];
    assert_eq!(transpose.parameters[0].id, "degrees");
    assert_eq!(transpose.parameters[0].kind, "whole");
    assert!(transpose.parameters[0].scale_aware);
    assert_eq!(transpose.parameters[0].default, json!(0));
    let fill = &list.tools[11];
    assert_eq!(fill.parameters[1].kind, "division");
    assert_eq!(fill.parameters[1].default, json!("1/16"));
    assert_eq!(fill.parameters[2].unit.as_deref(), Some("%"));
    assert!(!fill.parameters[2].description.is_empty());
}

#[test]
fn r4_each_track_keeps_its_choice_and_choosing_is_no_undo_step() {
    // Spec: midi-tools R4. A new track shows Arpeggiate and Rhythm; choosing
    // sets the track of the clip the Clip View shows, changes no notes, and
    // is never an undo step.
    let mut song = song(three());
    let state = song.state();
    assert_eq!(
        (state.transformation.as_str(), state.generator.as_str()),
        ("arpeggiate", "rhythm")
    );
    let steps = song.steps();
    let notes = song.notes();
    let state = song.tool(ids::CHOOSE, json!({ "tool": "test_fill" }));
    assert_eq!(state.generator, "test_fill");
    assert_eq!(
        state.transformation, "arpeggiate",
        "the other panel keeps its tool"
    );
    assert_eq!(
        state.current_tool.as_deref(),
        Some("test_fill"),
        "the panel to apply from"
    );
    assert_eq!((song.steps(), song.notes()), (steps, notes));
    assert!(!song.shell.is_dirty());
    assert!(
        song.shell
            .run(ids::CHOOSE, json!({ "tool": "nope" }))
            .is_err()
    );
}

#[test]
fn r5_r6_settings_are_checked_reset_and_never_undo_steps() {
    // Spec: midi-tools R5, R6. A value outside its range is refused and
    // changes nothing; Reset puts every parameter back and leaves Auto.
    let mut song = song(three());
    song.tool(ids::SET_AUTO, json!({ "tool": "test_fill", "auto": false }));
    let steps = song.steps();
    let state = song.tool(
        ids::SET,
        json!({ "tool": "test_fill", "values": { "key": 48, "division": "1/8", "length": 80 } }),
    );
    let fill = state
        .tools
        .iter()
        .find(|tool| tool.tool == "test_fill")
        .unwrap();
    assert_eq!(fill.values["key"], json!(48));
    assert_eq!(fill.values["division"], json!("1/8"));
    assert!(!fill.at_defaults && !fill.auto);
    assert_eq!(
        song.steps(),
        steps,
        "Auto off changes no notes and records no step"
    );
    let message = song.refused(
        ids::SET,
        json!({ "tool": "test_fill", "values": { "length": 900 } }),
    );
    assert!(message.contains("length"), "{message}");
    song.refused(
        ids::SET,
        json!({ "tool": "test_fill", "values": { "colour": 1 } }),
    );
    let state = song.tool(ids::RESET, json!({ "tool": "test_fill" }));
    let fill = state
        .tools
        .iter()
        .find(|tool| tool.tool == "test_fill")
        .unwrap();
    assert!(fill.at_defaults);
    assert!(!fill.auto, "Reset leaves the Auto switch as it is");
    assert_eq!(fill.values["key"], json!(60));
}

#[test]
fn a_tool_that_takes_part_of_a_probability_group_ends_the_group() {
    // Spec: midi-tools R14; note-editing R21, R22. A written note joins no
    // group, and a group left with one note ends, so the edit isn't refused.
    let mut notes = three();
    notes[0].group = Some(1);
    notes[1].group = Some(1);
    let midi = MidiData {
        groups: vec![bl_model::NoteGroup {
            id: 1,
            kind: bl_model::GroupKind::PlayAll,
        }],
        ..MidiData::new(notes)
    };
    let mut song = song_with(midi, Scale::default());
    song.select_notes(&[0]);
    song.apply_transpose(12);
    assert_eq!(song.keys(), [72, 62, 64]);
    assert!(song.notes().iter().all(|note| note.group.is_none()));
    let ClipData::Midi(midi) = &song.doc().clip(song.clip).unwrap().data else {
        panic!("the clip is MIDI");
    };
    assert!(midi.groups.is_empty());
}

#[test]
fn r10_r11_a_transformation_acts_on_the_selected_notes_first() {
    // Spec: midi-tools R10, R11. The selected notes, else the notes that
    // start in the time selection, else those that start in the loop.
    let mut song = song(three());
    song.select_notes(&[1]);
    let state = song.apply_transpose(12);
    assert_eq!(song.keys(), [60, 74, 64], "only the selected note moved");
    assert_eq!(
        state.run.unwrap().notes,
        [1],
        "and it is selected afterwards"
    );

    // A time selection with no notes selected: the notes that start in it.
    let mut song = self::song(three());
    song.select_time(QUARTER, 3 * QUARTER);
    song.apply_transpose(12);
    assert_eq!(
        song.keys(),
        [60, 74, 76],
        "the notes starting at its start and inside it"
    );

    // Neither: the clip's loop.
    let looped = MidiData {
        looping: true,
        loop_start: Ticks::ZERO,
        loop_end: Some(Ticks(2 * QUARTER)),
        ..MidiData::new(three())
    };
    let mut song = song_with(looped, Scale::default());
    song.apply_transpose(12);
    assert_eq!(
        song.keys(),
        [72, 74, 64],
        "the notes starting in the loop brace"
    );

    // The loop off: the Start marker to the End marker.
    let markers = MidiData {
        start: Ticks(QUARTER),
        end: Some(Ticks(3 * QUARTER)),
        ..MidiData::new(three())
    };
    let mut song = song_with(markers, Scale::default());
    song.apply_transpose(12);
    assert_eq!(song.keys(), [60, 74, 76]);
}

#[test]
fn r19_a_transformation_with_no_input_is_refused_saying_why() {
    // Spec: midi-tools R19. No notes selected, none in the range.
    let ended = MidiData {
        end: Some(Ticks(BAR)),
        ..MidiData::new(vec![note(60, 3 * BAR, QUARTER)])
    };
    let mut song = song_with(ended, Scale::default());
    song.shell
        .run(
            ids::SET_AUTO,
            json!({ "tool": "test_transpose", "auto": true }),
        )
        .unwrap();
    let steps = song.steps();
    let message = song.refused(ids::APPLY, json!({ "tool": "test_transpose" }));
    assert!(message.contains("no notes"), "{message}");
    assert_eq!(song.steps(), steps);
    // With Auto on, moving a transformation's control with no input starts
    // nothing (R17).
    let state = song.transpose(3);
    assert_eq!(state.run, None);
    assert_eq!(song.steps(), steps);
    assert!(state.application.is_none());
}

#[test]
fn r17_with_nothing_selected_a_transformations_control_starts_nothing() {
    // Spec: midi-tools R17. Notes in the loop are not a selection: moving a
    // transformation's parameters applies it only while the note editor has
    // a time selection or selected notes, though Apply takes the loop.
    let looped = MidiData {
        looping: true,
        loop_start: Ticks::ZERO,
        loop_end: Some(Ticks(4 * QUARTER)),
        ..MidiData::new(three())
    };
    let mut song = song_with(looped, Scale::default());
    let before = song.notes();
    let state = song.transpose(3);
    assert_eq!(state.run, None);
    assert_eq!(song.notes(), before);
    assert!(state.application.is_none());
    song.select_time(0, QUARTER);
    song.transpose(4);
    assert_eq!(song.keys(), [64, 62, 64], "a time selection starts it");
    // Apply, with nothing selected, takes the loop.
    let mut song = song_with(
        MidiData {
            looping: true,
            loop_start: Ticks::ZERO,
            loop_end: Some(Ticks(4 * QUARTER)),
            ..MidiData::new(three())
        },
        Scale::default(),
    );
    song.apply_transpose(1);
    assert_eq!(song.keys(), [61, 63, 65]);
}

#[test]
fn r12_r13_a_generator_replaces_overlaps_and_is_cut_at_the_range_end() {
    // Spec: midi-tools R12, R13. A note at the same key whose span overlaps a
    // generated note goes; the clip's other notes stay; the last note is cut
    // at the range's end.
    let mut song = song(vec![
        note(60, SIXTEENTH / 2, SIXTEENTH),
        note(61, 0, QUARTER),
        note(60, 4 * SIXTEENTH, SIXTEENTH),
    ]);
    song.select_time(0, SIXTEENTH * 5 / 2);
    let state = song.tool(
        ids::APPLY,
        json!({ "tool": "test_fill", "values": { "length": 100 } }),
    );
    let run = state.run.unwrap();
    assert_eq!(run.wrote, 3);
    let notes = song.notes();
    assert_eq!(notes.len(), 5);
    let generated: Vec<_> = notes
        .iter()
        .filter(|n| n.key == 60 && n.start.get() < 3 * SIXTEENTH)
        .collect();
    assert_eq!(generated.len(), 3, "the overlapped note at 60 is gone");
    assert_eq!(
        generated[2].length,
        Ticks(SIXTEENTH / 2),
        "the last is cut at the range's end"
    );
    assert!(notes.iter().any(|n| n.key == 61), "the other key stays");
    assert!(
        notes
            .iter()
            .any(|n| n.key == 60 && n.start.get() == 4 * SIXTEENTH),
        "so does a note outside"
    );
    // Generated notes: release velocity 64, chance 100 %, no deviation, active.
    assert!(generated.iter().all(|n| n.release_velocity == 64
        && n.probability == 1.0
        && n.velocity_deviation == 0
        && !n.mute
        && n.pressure.is_empty()));
}

#[test]
fn r13_a_key_past_the_keyboard_moves_by_octaves() {
    // Spec: midi-tools R13. 100 moved 70 semitones is 170, which folds to 122.
    let mut song = song(vec![note(100, 0, QUARTER)]);
    song.select_notes(&[0]);
    song.apply_transpose(70);
    assert_eq!(song.keys(), [122]);
}

#[test]
fn r14_a_note_a_tool_makes_carries_what_the_input_note_had() {
    // Spec: midi-tools R14, mpe R4. Release velocity, chance, deviation and
    // deactivation carry; each piece takes the stretch of each envelope over
    // its part.
    let mut input = note(60, 0, 2 * SIXTEENTH);
    input.release_velocity = 30;
    input.probability = 0.5;
    input.velocity_deviation = 5;
    input.mute = true;
    input.pressure = vec![
        Breakpoint {
            at: Ticks(0),
            value: 10.0,
            bend: 0.0,
        },
        Breakpoint {
            at: Ticks(SIXTEENTH),
            value: 90.0,
            bend: 0.0,
        },
    ];
    input.pitch = vec![Breakpoint {
        at: Ticks(0),
        value: 2.0,
        bend: 0.0,
    }];
    let mut song = song(vec![input]);
    song.select_notes(&[0]);
    song.tool(
        ids::APPLY,
        json!({ "tool": "test_transpose", "values": { "degrees": 1, "pieces": 2 } }),
    );
    let notes = song.notes();
    assert_eq!(notes.len(), 2);
    for piece in &notes {
        assert_eq!(piece.key, 61);
        assert_eq!(
            (
                piece.release_velocity,
                piece.probability,
                piece.velocity_deviation,
                piece.mute
            ),
            (30, 0.5, 5, true)
        );
        assert_eq!(piece.length, Ticks(SIXTEENTH));
    }
    let point = |at: i64, value: f64| Breakpoint {
        at: Ticks(at),
        value,
        bend: 0.0,
    };
    assert_eq!(notes[0].pressure, [point(0, 10.0), point(SIXTEENTH, 90.0)]);
    assert_eq!(notes[1].pressure, [point(0, 90.0), point(SIXTEENTH, 90.0)]);
    assert_eq!(notes[1].pitch, [point(0, 2.0), point(SIXTEENTH, 2.0)]);
}

#[test]
fn r15_the_written_notes_are_selected_and_the_time_selection_follows() {
    // Spec: midi-tools R15, piano-roll R24.
    let mut song = song(three());
    song.select_notes(&[0, 2]);
    song.apply_transpose(1);
    let target = song.state().target.unwrap();
    assert_eq!(
        target.selected,
        [0, 2],
        "the two written notes, and no others"
    );
    assert_eq!(target.time_selection, Some((Ticks(0), Ticks(3 * QUARTER))));
}

#[test]
fn r8_with_a_scale_a_degree_is_a_scale_step() {
    // Spec: midi-tools R8, scales R2. C major: 60 up two degrees is 64; with
    // Scale Mode off a degree is a semitone.
    let mut song = song_with(
        MidiData::new(vec![note(60, 0, QUARTER)]),
        Scale::new(0, ScaleName::Major),
    );
    song.select_notes(&[0]);
    song.apply_transpose(2);
    assert_eq!(song.keys(), [64]);
    let mut song = self::song(vec![note(60, 0, QUARTER)]);
    song.select_notes(&[0]);
    song.apply_transpose(2);
    assert_eq!(song.keys(), [62]);
    // A generator writes only keys of the scale.
    let mut song = song_with(MidiData::new(Vec::new()), Scale::new(0, ScaleName::Major));
    song.select_time(0, SIXTEENTH);
    song.tool(
        ids::APPLY,
        json!({ "tool": "test_fill", "values": { "key": 61 } }),
    );
    assert!(
        song.keys()[0] == 60,
        "61 is not in C major: the nearest scale key, the lower of two"
    );
}

#[test]
fn r9_a_drum_racks_keys_take_the_scales_place() {
    // Spec: midi-tools R9, drum-racks R1. The keys the chains receive.
    let mut song = song_on(
        MidiData::new(vec![note(36, 0, QUARTER), note(40, QUARTER, QUARTER)]),
        Scale::default(),
        |editor, track| {
            let rack = editor.new_id();
            editor
                .apply(Command::InsertDevice {
                    track,
                    index: 0,
                    device: bl_model::Device::rack(rack, RackKind::Drum),
                })
                .unwrap();
            for key in [36u8, 38, 42] {
                let chain = editor.new_id();
                editor
                    .apply(Command::AddChain {
                        rack,
                        index: 0,
                        chain: Chain::for_pad(chain, "Pad", key),
                    })
                    .unwrap();
            }
        },
    );
    song.select_notes(&[0]);
    song.apply_transpose(1);
    assert_eq!(
        song.keys(),
        [38, 40],
        "one step is the next key the rack receives"
    );
}

#[test]
fn r17_auto_runs_again_from_the_kept_input() {
    // Spec: midi-tools R17. Two changes: the second is made from the input,
    // not from the first's notes.
    let mut song = song(three());
    song.select_notes(&[0, 1, 2]);
    let steps = song.steps();
    let state = song.transpose(1);
    assert_eq!(song.keys(), [61, 63, 65]);
    assert!(
        state.application.is_some(),
        "the first change opened an application"
    );
    let state = song.transpose(3);
    assert_eq!(song.keys(), [63, 65, 67], "from the input: three, not four");
    assert_eq!(state.application.unwrap().input_notes, 3);
    assert_eq!(song.steps(), steps + 2, "each change is a step of its own");
    // Choosing a tool applies nothing until a parameter changes.
    let notes = song.notes();
    song.tool(ids::CHOOSE, json!({ "tool": "test_transpose" }));
    assert_eq!(song.notes(), notes);
}

#[test]
fn r18_auto_off_puts_the_input_back_as_one_step() {
    // Spec: midi-tools R18. Switching Auto off with the application open
    // puts the notes back and closes it; switching it on applies nothing.
    let mut song = song(three());
    song.select_notes(&[0, 1, 2]);
    let original = song.notes();
    song.transpose(2);
    assert_ne!(song.notes(), original);
    let steps = song.steps();
    let state = song.tool(
        ids::SET_AUTO,
        json!({ "tool": "test_transpose", "auto": false }),
    );
    assert_eq!(song.notes(), original);
    assert_eq!(song.steps(), steps + 1, "as one undo step");
    assert!(state.application.is_none());
    // With Auto off a change changes no notes.
    song.transpose(5);
    assert_eq!(song.notes(), original);
    assert_eq!(song.steps(), steps + 1);
    let state = song.tool(
        ids::SET_AUTO,
        json!({ "tool": "test_transpose", "auto": true }),
    );
    assert_eq!(
        song.notes(),
        original,
        "Auto on applies nothing until a parameter changes"
    );
    assert!(state.application.is_none());
}

#[test]
fn r19_applying_twice_builds_up() {
    // Spec: midi-tools R19. Each apply acts on the notes the last left, with
    // Auto on or off.
    let mut song = song(three());
    song.select_notes(&[0, 1, 2]);
    song.apply_transpose(1);
    assert_eq!(song.keys(), [61, 63, 65]);
    song.tool(ids::APPLY, json!({ "tool": "test_transpose" }));
    assert_eq!(song.keys(), [62, 64, 66], "the effect builds up");
    // The apply left its application open, so Auto off puts the notes back
    // as they were before it (R18); applying again, with Auto off, builds
    // up from there.
    song.tool(
        ids::SET_AUTO,
        json!({ "tool": "test_transpose", "auto": false }),
    );
    assert_eq!(song.keys(), [61, 63, 65]);
    song.tool(ids::APPLY, json!({ "tool": "test_transpose" }));
    assert_eq!(song.keys(), [62, 64, 66], "also with Auto off");
    song.tool(ids::APPLY, json!({ "tool": "test_transpose" }));
    assert_eq!(song.keys(), [63, 65, 67]);
}

#[test]
fn r19_the_default_tool_is_the_chosen_tool_of_the_panel_shown_last() {
    // Spec: midi-tools R19. Cmd+Enter and the menu item apply it.
    let mut song = song(three());
    song.select_notes(&[0]);
    song.tool(
        ids::SET_AUTO,
        json!({ "tool": "test_transpose", "auto": false }),
    );
    song.tool(
        ids::SET,
        json!({ "tool": "test_transpose", "values": { "degrees": 7 } }),
    );
    song.tool(ids::CHOOSE, json!({ "tool": "test_fill" }));
    song.tool(ids::CHOOSE, json!({ "tool": "test_transpose" }));
    song.tool(ids::SHOW_PANEL, json!({ "panel": "generate" }));
    assert_eq!(song.state().current_tool.as_deref(), Some("test_fill"));
    song.tool(ids::SHOW_PANEL, json!({ "panel": "transform" }));
    assert_eq!(song.state().current_tool.as_deref(), Some("test_transpose"));
    song.tool(ids::APPLY, json!({}));
    assert_eq!(
        song.keys(),
        [67, 62, 64],
        "applied with the settings it keeps"
    );
}

#[test]
fn r20_the_same_input_settings_and_seed_give_the_same_notes() {
    // Spec: midi-tools R20. A seed given, kept across parameter changes, a
    // new one drawn per apply, and a reseed.
    let values = json!({ "degrees": 0, "spread": 40 });
    let mut one = song(three());
    one.select_notes(&[0, 1, 2]);
    one.tool(
        ids::APPLY,
        json!({ "tool": "test_transpose", "values": values, "seed": 7 }),
    );
    let mut two = song(three());
    two.select_notes(&[0, 1, 2]);
    two.tool(
        ids::APPLY,
        json!({ "tool": "test_transpose", "values": values, "seed": 7 }),
    );
    assert_eq!(one.notes(), two.notes(), "the same seed, the same notes");
    let mut other = song(three());
    other.select_notes(&[0, 1, 2]);
    other.tool(
        ids::APPLY,
        json!({ "tool": "test_transpose", "values": values, "seed": 8 }),
    );
    assert_ne!(one.notes(), other.notes());

    // No seed given: a new one on every apply.
    let mut song = song(three());
    song.select_notes(&[0, 1, 2]);
    let a = song
        .tool(
            ids::APPLY,
            json!({ "tool": "test_transpose", "values": values }),
        )
        .run
        .unwrap()
        .seed;
    let b = song
        .tool(ids::APPLY, json!({ "tool": "test_transpose" }))
        .run
        .unwrap()
        .seed;
    assert_ne!(a, b);

    // The seed is kept across parameter changes of an open application, and
    // a reseed gives a new one and runs the tool again.
    let first = song.transpose(1).run.unwrap().seed;
    assert_eq!(song.transpose(2).run.unwrap().seed, first, "kept");
    let before = song.notes();
    let run = song.tool(ids::RESEED, json!({ "seed": 99 })).run.unwrap();
    assert_eq!(run.seed, 99);
    assert_ne!(song.notes(), before);
    assert_eq!(song.state().application.unwrap().seed, 99);
}

#[test]
fn r21_the_application_closes_when_something_else_changes_the_notes() {
    // Spec: midi-tools R21. Another clip showing, a change of the selection
    // or the time selection, a command that isn't a tool's changing the
    // notes, undo, Reset, close, and another tool.
    fn open(song: &mut Song) {
        song.shell.run(ids::CLOSE, json!({})).unwrap();
        song.select_notes(&[0, 1, 2]);
        song.transpose(1);
        assert!(song.state().application.is_some());
    }
    let mut song = song(three());
    open(&mut song);
    song.select_notes(&[0]);
    assert!(song.state().application.is_none(), "the selection changed");
    open(&mut song);
    song.select_time(0, QUARTER);
    assert!(
        song.state().application.is_none(),
        "so did the time selection"
    );
    open(&mut song);
    song.shell
        .run("note.move", json!({ "by": SIXTEENTH }))
        .unwrap();
    assert!(
        song.state().application.is_none(),
        "another command changed the notes"
    );
    open(&mut song);
    song.shell.run("edit.undo", json!({})).unwrap();
    assert!(song.state().application.is_none(), "undo");
    open(&mut song);
    song.tool(ids::RESET, json!({ "tool": "test_transpose" }));
    assert!(song.state().application.is_none(), "Reset");
    open(&mut song);
    song.tool(ids::CLOSE, json!({}));
    assert!(song.state().application.is_none(), "close");
    open(&mut song);
    song.tool(ids::SET_AUTO, json!({ "tool": "test_fill", "auto": true }));
    song.tool(ids::APPLY, json!({ "tool": "test_fill" }));
    assert_eq!(
        song.state().application.unwrap().tool,
        "test_fill",
        "another tool applied"
    );
    // After it closes, the next change starts a new application on the notes
    // as they are then.
    song.tool(ids::CLOSE, json!({}));
    song.select_notes(&[0]);
    let before = song.notes();
    song.transpose(1);
    let state = song.state();
    assert_eq!(state.application.unwrap().input_notes, 1);
    assert_ne!(song.notes(), before);
}

#[test]
fn r21_another_clip_showing_closes_the_application() {
    // Spec: midi-tools R21.
    let mut editor = factory_default_set();
    let track = editor.document().top_level().next().unwrap();
    let ids_: Vec<ClipId> = (0..2).map(|_| editor.new_id()).collect();
    for (at, id) in ids_.iter().enumerate() {
        let clip = Clip::new(
            *id,
            Ticks(at as i64 * 4 * BAR),
            Ticks(4 * BAR),
            ClipData::Midi(MidiData::new(three())),
        );
        editor.apply(Command::CreateClip { track, clip }).unwrap();
    }
    editor.clear_history().unwrap();
    let mut shell = Shell::new(
        Preferences::default(),
        None,
        AudioHost::null(Default::default()),
        Box::new(Preloaded(Some(editor))),
    );
    shell
        .run("set.open", json!({ "path": "Two.blset" }))
        .unwrap();
    shell
        .run("arrangement.select_clips", json!({ "clips": [ids_[0]] }))
        .unwrap();
    shell
        .run("clip_view.select_notes", json!({ "notes": [0, 1, 2] }))
        .unwrap();
    shell
        .run(
            ids::SET,
            json!({ "tool": "test_transpose", "values": { "degrees": 1 } }),
        )
        .unwrap();
    let state: MidiToolState =
        serde_json::from_value(shell.run(ids::STATE, json!({})).unwrap()).unwrap();
    assert!(state.application.is_some());
    shell
        .run("arrangement.select_clips", json!({ "clips": [ids_[1]] }))
        .unwrap();
    let state: MidiToolState =
        serde_json::from_value(shell.run(ids::STATE, json!({})).unwrap()).unwrap();
    assert!(state.application.is_none());
    assert_eq!(state.clip, Some(ids_[1]));
}

#[test]
fn r22_each_application_is_one_step_named_for_its_tool_even_when_nothing_changes() {
    // Spec: midi-tools R22.
    let mut song = song(three());
    song.select_notes(&[0, 1, 2]);
    let before = song.notes();
    let steps = song.steps();
    let run = song
        .tool(ids::APPLY, json!({ "tool": "test_transpose" }))
        .run
        .unwrap();
    assert_eq!(song.notes(), before, "the notes came out as they were");
    assert!(run.undo_step);
    assert_eq!(song.steps(), steps + 1);
    assert_eq!(song.last_step().as_deref(), Some("Test Transpose"));
    song.apply_transpose(4);
    assert_eq!(song.steps(), steps + 2);
    song.shell.run("edit.undo", json!({})).unwrap();
    assert_eq!(song.notes(), before, "undo puts the notes back");
    // Undo leaves the tools' settings as they are.
    let state = song.state();
    let transpose = state
        .tools
        .iter()
        .find(|t| t.tool == "test_transpose")
        .unwrap();
    assert_eq!(transpose.values["degrees"], json!(4));
    song.shell.run("edit.redo", json!({})).unwrap();
    assert_eq!(song.keys(), [64, 66, 68]);
}

#[test]
fn r16_a_run_of_more_than_65536_notes_is_refused_and_changes_nothing() {
    // Spec: midi-tools R16.
    let mut song = song(three());
    let before = song.notes();
    let steps = song.steps();
    let range = json!({ "start": 0, "end": 22_050 * 65_537i64 });
    let message = song.refused(
        ids::APPLY,
        json!({ "tool": "test_fill", "values": { "division": "1/128" }, "range": range }),
    );
    assert!(message.contains("65536"), "{message}");
    assert_eq!(song.notes(), before);
    assert_eq!(song.steps(), steps);
    // The most there can be is fine.
    song.shell
        .run(
            ids::APPLY,
            json!({ "tool": "test_fill", "values": { "division": "1/128" },
                    "range": { "start": 0, "end": 22_050 * 65_536i64 } }),
        )
        .unwrap();
    assert_eq!(
        song.notes().len(),
        65_536 + 2,
        "and replaces the one note it overlaps"
    );
}

#[test]
fn r22_a_controls_drag_is_one_step_and_esc_puts_the_notes_back() {
    // Spec: midi-tools R22, piano-roll R11. The frames of a drag on a
    // control are `midi_tool.set` gestures; the drag is one undo step named
    // after the tool, and Esc undoes it.
    let mut song = song(three());
    song.select_notes(&[0, 1, 2]);
    let original = song.notes();
    let steps = song.steps();
    let frame = |degrees: i64| {
        Invocation::new(
            ids::SET,
            json!({ "tool": "test_transpose", "values": { "degrees": degrees } }),
        )
    };
    for degrees in 1..=4 {
        song.shell
            .gesture(GesturePhase::Update, Some(&frame(degrees)))
            .unwrap();
    }
    assert_eq!(
        song.keys(),
        [64, 66, 68],
        "each frame runs from the kept input"
    );
    song.shell.gesture(GesturePhase::Cancel, None).unwrap();
    assert_eq!(song.notes(), original, "Esc puts the notes back");
    assert_eq!(song.steps(), steps);
    for degrees in 1..=4 {
        song.shell
            .gesture(GesturePhase::Update, Some(&frame(degrees)))
            .unwrap();
    }
    song.shell
        .gesture(GesturePhase::Commit, Some(&frame(5)))
        .unwrap();
    assert_eq!(song.keys(), [65, 67, 69]);
    assert_eq!(song.steps(), steps + 1, "however many frames, one step");
    assert_eq!(song.last_step().as_deref(), Some("Test Transpose"));
    // The application the drag opened stays open for the next change.
    assert!(song.state().application.is_some());
}

#[test]
fn r24_a_tool_that_leaves_fewer_notes_is_destructive_as_well() {
    // Spec: midi-tools R24, agent-permissions R11. The dry run says so.
    let mut song = song(vec![
        note(60, 0, SIXTEENTH),
        note(60, 10, SIXTEENTH),
        note(60, 20, SIXTEENTH),
    ]);
    song.select_time(0, SIXTEENTH);
    let args = json!({ "tool": "test_fill", "values": { "division": "1" } });
    let permission = song.shell.call_permission(ids::APPLY, &args);
    let fewer = permission
        .fewer_notes
        .expect("the generated note replaces all three");
    assert_eq!((fewer.has, fewer.leaves), (3, 1));
    assert!(permission.is_destructive());
    // Nothing changed by asking.
    assert_eq!(song.notes().len(), 3);
    // A tool that keeps the count doesn't.
    let permission = song.shell.call_permission(
        ids::APPLY,
        &json!({ "tool": "test_transpose", "notes": [0, 1, 2] }),
    );
    assert!(permission.fewer_notes.is_none());
    assert!(!permission.is_destructive());
    // Auto's set is judged the same way.
    let permission = song.shell.call_permission(
        ids::SET,
        &json!({ "tool": "test_fill", "values": { "division": "1" } }),
    );
    assert!(permission.fewer_notes.is_some());
}

#[test]
fn r25_the_notes_reach_the_engine_as_a_change_of_the_clips_notes() {
    // Spec: midi-tools R25. A tool's notes are the clip's data after a
    // `SetClipData`, like any note edit (midi-clips R9, R10): the document
    // holds them, and nothing else about the song changes.
    let mut song = song(three());
    song.select_notes(&[0]);
    let before = song.doc().clone();
    song.apply_transpose(1);
    let after = song.doc();
    assert_ne!(*after, before);
    assert_eq!(after.check_invariants(), Ok(()));
    assert_eq!(after.track_count(), before.track_count());
    assert_eq!(song.keys(), [61, 62, 64]);
}

#[test]
fn r4_r5_choices_and_settings_are_saved_with_the_set_and_older_sets_load_unchanged() {
    // Spec: midi-tools R4, R5. Saved beside the song, no undo step; values
    // are checked against the descriptors on load.
    let mut editor = factory_default_set();
    let track = editor.document().top_level().next().unwrap();
    let clip = editor.new_id();
    editor
        .apply(Command::CreateClip {
            track,
            clip: Clip::new(
                clip,
                Ticks::ZERO,
                Ticks(4 * BAR),
                ClipData::Midi(MidiData::new(three())),
            ),
        })
        .unwrap();
    let dir = std::env::temp_dir().join(format!(
        "bl-shell-midi-tools-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("Tools.blset");
    let mut files = bl_shell::files::ProjectFiles::new();
    files.save(&path, &editor).unwrap();

    let mut shell = Shell::new(
        Preferences::default(),
        None,
        AudioHost::null(Default::default()),
        Box::new(bl_shell::files::ProjectFiles::new()),
    );
    shell
        .run("set.open", json!({ "path": path.to_string_lossy() }))
        .unwrap();
    shell
        .run("arrangement.select_clips", json!({ "clips": [clip] }))
        .unwrap();
    let state: MidiToolState =
        serde_json::from_value(shell.run(ids::STATE, json!({})).unwrap()).unwrap();
    assert_eq!(
        state.transformation, "arpeggiate",
        "a set saved before MIDI Tools: the defaults"
    );
    let steps = shell.editor().undo_history().len();
    shell
        .run(ids::CHOOSE, json!({ "tool": "test_fill" }))
        .unwrap();
    shell
        .run(ids::SET_AUTO, json!({ "tool": "test_fill", "auto": false }))
        .unwrap();
    shell
        .run(
            ids::SET,
            json!({ "tool": "test_fill", "values": { "key": 50, "division": "1/8" } }),
        )
        .unwrap();
    assert_eq!(shell.editor().undo_history().len(), steps, "no undo step");
    assert!(!shell.is_dirty());
    shell.run("set.save", json!({})).unwrap();
    shell.run("set.new", json!({})).unwrap();
    shell
        .run("set.open", json!({ "path": path.to_string_lossy() }))
        .unwrap();
    shell
        .run("arrangement.select_clips", json!({ "clips": [clip] }))
        .unwrap();
    let state: MidiToolState =
        serde_json::from_value(shell.run(ids::STATE, json!({})).unwrap()).unwrap();
    assert_eq!(state.generator, "test_fill");
    let fill = state.tools.iter().find(|t| t.tool == "test_fill").unwrap();
    assert_eq!(fill.values["key"], json!(50));
    assert_eq!(fill.values["division"], json!("1/8"));
    assert!(!fill.auto);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn r5_a_saved_value_outside_its_range_loads_as_the_default() {
    // Spec: midi-tools R5.
    use bl_project::midi_tools::{MidiToolSettings, SavedTool};
    let saved = MidiToolSettings {
        chosen: Default::default(),
        tools: [(
            "test_fill".to_owned(),
            SavedTool {
                auto: false,
                values: [
                    ("key".to_owned(), json!(300)),
                    ("length".to_owned(), json!(70)),
                    ("colour".to_owned(), json!(1)),
                ]
                .into(),
            },
        )]
        .into(),
    };
    let mut view = bl_shell::clip_view::ClipView::default();
    view.restore_midi_tools(&saved);
    let again = view.midi_tools().saved();
    let fill = &again.tools["test_fill"];
    assert!(!fill.auto);
    assert_eq!(fill.values["key"], json!(60), "out of range: the default");
    assert_eq!(fill.values["length"], json!(70), "in range: kept");
    assert!(!fill.values.contains_key("colour"), "no such parameter");
}

// The transformations of WP-6.12 (the MIDI transformations spec, R3 to R22),
// through the commands.

/// Three notes with room between them, none on the grid.
fn spaced() -> Vec<Note> {
    vec![
        note(60, 37, SIXTEENTH),
        note(64, 2 * QUARTER + 1000, SIXTEENTH),
        note(67, 3 * QUARTER + 11, SIXTEENTH),
    ]
}

#[test]
fn r3_to_r19_each_transformation_applies_by_id_with_a_seed_as_one_undo_step() {
    for (tool, name, values) in [
        (
            "arpeggiate",
            "Arpeggiate",
            json!({ "steps": 1, "gate": 50 }),
        ),
        ("chop", "Chop", json!({ "parts": 4 })),
        ("connect", "Connect", json!({ "spread": 2 })),
        ("ornament", "Ornament", json!({})),
        ("quantize", "Quantize", json!({ "end": true })),
    ] {
        let mut one = song(spaced());
        one.select_notes(&[0, 1, 2]);
        let before = one.notes();
        let steps = one.steps();
        let args = json!({ "tool": tool, "values": values, "seed": 5 });
        let run = one.tool(ids::APPLY, args.clone()).run.unwrap();
        assert_eq!(run.seed, 5, "{tool}");
        assert!(run.undo_step, "{tool}");
        assert_eq!(one.steps(), steps + 1, "{tool}: one undo step");
        assert_eq!(one.last_step().as_deref(), Some(name), "{tool}");
        assert_ne!(one.notes(), before, "{tool} changed the notes");
        // The same input, settings and seed give the same notes.
        let mut two = song(spaced());
        two.select_notes(&[0, 1, 2]);
        two.tool(ids::APPLY, args);
        assert_eq!(one.notes(), two.notes(), "{tool}");
        one.shell.run("edit.undo", json!({})).unwrap();
        assert_eq!(one.notes(), before, "{tool}: undo");
    }
}

#[test]
fn r4_to_r6_arpeggiate_in_and_out_of_scale_mode_and_on_a_drum_rack() {
    let chord = || {
        vec![
            note(60, 0, BAR / 2),
            note(64, 0, BAR / 2),
            note(67, 0, BAR / 2),
        ]
    };
    let values = json!({ "steps": 1, "distance": 1, "rate": "1/16" });
    // Without a scale a degree is a semitone.
    let mut plain = song(chord());
    plain.select_notes(&[0, 1, 2]);
    plain.tool(
        ids::APPLY,
        json!({ "tool": "arpeggiate", "values": values }),
    );
    assert_eq!(plain.keys()[..6], [60, 64, 67, 61, 65, 68]);
    // In C major one degree up from C is D, from E is F and from G is A.
    let mut major = song_with(MidiData::new(chord()), Scale::new(0, ScaleName::Major));
    major.select_notes(&[0, 1, 2]);
    major.tool(
        ids::APPLY,
        json!({ "tool": "arpeggiate", "values": values }),
    );
    assert_eq!(major.keys()[..6], [60, 64, 67, 62, 65, 69]);
    // On a Drum Rack a step counts through the pads' keys.
    let pads = |editor: &mut Editor, track: TrackId| {
        let rack = editor.new_id();
        editor
            .apply(Command::InsertDevice {
                track,
                index: 0,
                device: bl_model::Device::rack(rack, RackKind::Drum),
            })
            .unwrap();
        for key in [36u8, 38, 42] {
            let chain = editor.new_id();
            editor
                .apply(Command::AddChain {
                    rack,
                    index: 0,
                    chain: Chain::for_pad(chain, "Pad", key),
                })
                .unwrap();
        }
    };
    let mut drums = song_on(
        MidiData::new(vec![note(36, 0, BAR / 2), note(38, 0, BAR / 2)]),
        Scale::default(),
        pads,
    );
    drums.select_notes(&[0, 1]);
    drums.tool(
        ids::APPLY,
        json!({ "tool": "arpeggiate", "values": values }),
    );
    assert!(
        drums.keys().iter().all(|key| [36, 38, 42].contains(key)),
        "{:?}",
        drums.keys()
    );
}

#[test]
fn r12_to_r13_connect_in_a_scale_and_on_a_drum_rack_stays_on_their_keys() {
    let ends = || vec![note(60, 0, SIXTEENTH), note(71, 4 * QUARTER, SIXTEENTH)];
    let mut major = song_with(MidiData::new(ends()), Scale::new(0, ScaleName::Major));
    major.select_notes(&[0, 1]);
    major.tool(
        ids::APPLY,
        json!({ "tool": "connect", "values": { "spread": 3 }, "seed": 2 }),
    );
    assert!(major.notes().len() > 2);
    assert!(
        major
            .keys()
            .iter()
            .all(|key| [0, 2, 4, 5, 7, 9, 11].contains(&(key % 12))),
        "{:?}",
        major.keys()
    );
}

#[test]
fn r20_r19_cmd_u_quantizes_with_the_tools_settings_which_are_saved_and_reloaded() {
    // Spec: midi-transformations R19, R20; piano-roll R19.
    let mut editor = factory_default_set();
    let track = editor.document().top_level().next().unwrap();
    let clip = editor.new_id();
    editor
        .apply(Command::CreateClip {
            track,
            clip: Clip::new(
                clip,
                Ticks::ZERO,
                Ticks(4 * BAR),
                ClipData::Midi(MidiData::new(vec![note(60, QUARTER + 200, QUARTER - 100)])),
            ),
        })
        .unwrap();
    let dir = std::env::temp_dir().join(format!(
        "bl-shell-quantize-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("Quantize.blset");
    bl_shell::files::ProjectFiles::new()
        .save(&path, &editor)
        .unwrap();
    let mut shell = Shell::new(
        Preferences::default(),
        None,
        AudioHost::null(Default::default()),
        Box::new(bl_shell::files::ProjectFiles::new()),
    );
    let open = |shell: &mut Shell| {
        shell
            .run("set.open", json!({ "path": path.to_string_lossy() }))
            .unwrap();
        shell
            .run("arrangement.select_clips", json!({ "clips": [clip] }))
            .unwrap();
    };
    open(&mut shell);
    let steps = shell.editor().undo_history().len();
    let settings = shell
        .run(
            "clip_view.quantize_settings",
            json!({ "grid": QUARTER, "target": "both", "triplets": true, "amount": 100 }),
        )
        .unwrap();
    assert_eq!(settings["target"], json!("both"));
    assert_eq!(settings["triplets"], json!(true));
    assert_eq!(shell.editor().undo_history().len(), steps, "no undo step");
    shell.run("set.save", json!({})).unwrap();
    shell.run("set.new", json!({})).unwrap();
    open(&mut shell);
    let settings = shell
        .run("clip_view.quantize_settings", json!({ "amount": 100 }))
        .unwrap();
    assert_eq!(settings["grid"], json!(QUARTER));
    assert_eq!(settings["target"], json!("both"));
    assert_eq!(settings["triplets"], json!(true));
    let state: MidiToolState =
        serde_json::from_value(shell.run(ids::STATE, json!({})).unwrap()).unwrap();
    let quantize = state.tools.iter().find(|t| t.tool == "quantize").unwrap();
    assert_eq!(quantize.values["start"], json!(true));
    assert_eq!(quantize.values["end"], json!(true));
    assert_eq!(quantize.values["grid"], json!("1_4"));
    assert_eq!(quantize.values["triplets"], json!(true));
    // Cmd+U quantises through `note.quantize` with them: lines a quarter
    // triplet apart, the start first and then the end.
    shell
        .run("clip_view.select_notes", json!({ "notes": [0] }))
        .unwrap();
    shell.run("note.quantize", json!({})).unwrap();
    let ClipData::Midi(midi) = &shell.editor().document().clip(clip).unwrap().data else {
        panic!("a MIDI clip");
    };
    let third = QUARTER * 2 / 3;
    let moved = notes_in_order(midi);
    assert_eq!(
        (moved[0].start, moved[0].length),
        (Ticks(2 * third), Ticks(third))
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn r21_quantize_settings_shows_the_transform_panel_with_quantize_chosen_without_an_undo_step() {
    let mut song = song(three());
    let (steps, notes) = (song.steps(), song.notes());
    assert_eq!(song.state().panel, Panel::Utilities);
    song.shell
        .run("clip_view.quantize_settings", json!({}))
        .unwrap();
    let state = song.state();
    assert_eq!(state.panel, Panel::Transform);
    assert_eq!(state.transformation, "quantize");
    assert_eq!((song.steps(), song.notes()), (steps, notes));
    // Reading a field back does not move the panel.
    song.tool(ids::SHOW_PANEL, json!({ "panel": "utilities" }));
    song.shell
        .run("clip_view.quantize_settings", json!({ "amount": 40 }))
        .unwrap();
    assert_eq!(song.state().panel, Panel::Utilities);
}

#[test]
fn r8_r43_chops_gaps_write_its_pattern_and_a_toggle_click_is_a_set() {
    let mut song = song(three());
    let chop = |state: &MidiToolState| {
        state
            .tools
            .iter()
            .find(|t| t.tool == "chop")
            .unwrap()
            .values
            .clone()
    };
    song.tool(ids::SET_AUTO, json!({ "tool": "chop", "auto": false }));
    let state = song.tool(
        ids::SET,
        json!({ "tool": "chop", "values": { "parts": 4, "gaps": 1 } }),
    );
    let values = chop(&state);
    assert_eq!(
        values["pattern"].as_array().unwrap()[..4],
        [json!(true), json!(false), json!(true), json!(false)]
    );
    // A toggle click sets the pattern; the change is refused if it leaves no part.
    let mut toggles = vec![true; 16];
    toggles[1] = false;
    let state = song.tool(
        ids::SET,
        json!({ "tool": "chop", "values": { "pattern": toggles } }),
    );
    assert_eq!(
        chop(&state)["pattern"].as_array().unwrap()[..4],
        [json!(true), json!(false), json!(true), json!(true)]
    );
    let message = song.refused(
        ids::SET,
        json!({ "tool": "chop", "values": { "pattern": vec![false; 16] } }),
    );
    assert!(message.contains("at least one part"), "{message}");
    assert!(
        song.refused(ids::SET, json!({ "tool": "chop", "values": { "gaps": 9 } }))
            .contains("gaps"),
    );
    // Applying cuts the selected notes; the gaps write nothing.
    song.select_notes(&[0]);
    song.tool(
        ids::APPLY,
        json!({ "tool": "chop", "values": { "gaps": 1, "parts": 4 } }),
    );
    assert_eq!(song.notes().iter().filter(|n| n.key == 60).count(), 2);
}

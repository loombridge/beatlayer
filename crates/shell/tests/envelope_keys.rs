//! Editing envelopes from the keyboard, driven headless through
//! `Shell::run` (WP-6.26): the insert marker along an envelope, making,
//! selecting, moving and deleting breakpoints, typed values, and what an edit
//! from selecting a breakpoint to letting it go is. The behaviour is in the
//! automation editing spec (R20 to R29) and the clip envelopes spec (R14).

use std::path::{Path, PathBuf};

use bl_core::time::Ticks;
use bl_model::{
    AudioData, AutomationTarget, Breakpoint, Clip, ClipData, Command, ContentHash, Document,
    Editor, MixerParam, TrackId,
};
use bl_shell::arrangement_commands::{self as view_ids, ArrangementState, Lane};
use bl_shell::envelope_commands::{self as ids, EnvelopeKeyState, EnvelopeKeys};
use bl_shell::files::{SetStore, factory_default_set};
use bl_shell::settings::Preferences;
use bl_shell::{Area, Shell};
use serde_json::{Value, json};

const BAR: i64 = Ticks::WHOLE.get();

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

fn volume() -> AutomationTarget {
    AutomationTarget::Mixer {
        param: MixerParam::Volume,
    }
}

fn pan() -> AutomationTarget {
    AutomationTarget::Mixer {
        param: MixerParam::Pan,
    }
}

/// An audio track with a clip two bars long and a volume envelope with
/// breakpoints at bars 1, 2 and 3, and a MIDI track with a clip carrying a
/// pan envelope for the Envelope Editor.
struct Song {
    shell: Shell,
    track: TrackId,
}

fn song() -> Song {
    let mut editor = factory_default_set();
    let track = editor
        .document()
        .top_level()
        .find(|id| {
            editor
                .document()
                .track(*id)
                .is_some_and(|track| track.kind() == bl_model::TrackKind::Audio)
        })
        .unwrap();
    let clip: bl_model::ClipId = editor.new_id();
    editor
        .apply(Command::CreateClip {
            track,
            clip: Clip::new(
                clip,
                Ticks::ZERO,
                Ticks(BAR * 2),
                ClipData::Audio(AudioData::new(ContentHash([7; 32]))),
            ),
        })
        .unwrap();
    for (bar, value) in [(0, 0.0), (1, 1.0), (2, 0.5)] {
        editor
            .apply(Command::SetAutomationBreakpoint {
                track,
                target: volume(),
                at: Ticks(bar * BAR),
                value,
            })
            .unwrap();
    }
    editor.clear_history().unwrap();
    let mut shell = Shell::new(
        Preferences::default(),
        None,
        bl_shell::audio::AudioHost::null(Default::default()),
        Box::new(Preloaded(Some(editor))),
    );
    shell
        .run("set.open", json!({ "path": "Keys.blset" }))
        .unwrap();
    Song { shell, track }
}

impl Song {
    fn doc(&self) -> &Document {
        self.shell.editor().document()
    }

    fn run(&mut self, id: &str, args: Value) -> Value {
        self.shell
            .run(id, args)
            .unwrap_or_else(|e| panic!("{id} failed: {e}"))
    }

    fn keys(&mut self, id: &str, args: Value) -> EnvelopeKeyState {
        serde_json::from_value(self.run(id, args)).unwrap()
    }

    /// The volume envelope's breakpoints, as `(ticks, value)`.
    fn envelope(&self) -> Vec<(i64, f64)> {
        self.doc()
            .track(self.track)
            .unwrap()
            .automation_for(&volume())
            .map(|a| {
                a.envelope
                    .points()
                    .map(|point| (point.at.get(), point.value))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// How many undo steps the history holds that can be undone.
    fn steps(&self) -> usize {
        self.shell
            .editor()
            .history()
            .iter()
            .filter(|step| !step.undone && step.index > 0)
            .count()
    }

    fn state(&mut self) -> ArrangementState {
        serde_json::from_value(self.run(view_ids::STATE, json!({}))).unwrap()
    }

    /// Automation Mode on, and the track's automation lane selected with the
    /// insert marker at `at`, as clicking the lane does.
    fn lane_at(&mut self, at: i64) {
        self.run(view_ids::SET_GRID, json!({ "fixed": "quarter" }));
        self.run(view_ids::AUTOMATION_MODE, json!({ "on": true }));
        self.run(
            view_ids::SET_INSERT_MARKER,
            json!({ "at": Ticks(at), "track": self.track, "automation_lane": true }),
        );
    }

    /// A grid step, in ticks.
    fn grid(&self) -> i64 {
        self.shell.arrangement().grid_ticks(self.doc()).get()
    }

    fn can(&self, id: &str) -> bool {
        self.shell.can_invoke(id, &json!({}))
    }
}

// Spec: automation-editing R1, R20.
#[test]
fn clicking_a_lane_gives_it_the_keyboard_ahead_of_the_arrangement() {
    let mut song = song();
    song.lane_at(0);
    let state = song.state();
    assert_eq!(
        state.selected_lane,
        Some(Lane::Automation {
            track: song.track,
            target: volume()
        })
    );
    let contexts = song.shell.active_contexts();
    let envelope = contexts
        .iter()
        .position(|c| *c == bl_commands::Context::Envelope)
        .expect("the envelope context applies");
    let arrangement = contexts
        .iter()
        .position(|c| *c == bl_commands::Context::Arrangement)
        .unwrap();
    assert!(envelope < arrangement);
    // Selecting the track's own lane takes the keyboard back.
    song.run(
        view_ids::SELECT_TRACKS,
        json!({ "tracks": [song.track], "automation_lane": false }),
    );
    assert_eq!(song.state().selected_lane, None);
    assert!(
        !song
            .shell
            .active_contexts()
            .contains(&bl_commands::Context::Envelope)
    );
}

// Spec: automation-editing R20. Up and Down select the track's automation
// lane.
#[test]
fn selecting_a_tracks_lane_with_the_arrow_keys_gives_it_the_keyboard() {
    let mut song = song();
    song.run(view_ids::AUTOMATION_MODE, json!({ "on": true }));
    song.run(
        view_ids::SELECT_TRACKS,
        json!({ "tracks": [song.track], "automation_lane": true }),
    );
    assert!(matches!(
        song.state().selected_lane,
        Some(Lane::Automation { .. })
    ));
}

// Spec: automation-editing R21.
#[test]
fn left_and_right_move_the_insert_marker_a_grid_step_and_show_the_value() {
    let mut song = song();
    song.lane_at(BAR);
    let grid = song.grid();
    let moved = song.keys(
        ids::MOVE_MARKER,
        json!({ "direction": "right", "by": "grid" }),
    );
    assert_eq!(moved.envelope, EnvelopeKeys::Lane);
    assert_eq!(moved.marker, Ticks(BAR + grid));
    assert_eq!(song.state().insert_marker, Ticks(BAR + grid));
    // The value at the marker, as the volume reads: a little under the top
    // of the fader, towards the next breakpoint's 0.5.
    assert!(moved.value.ends_with(" dB"), "{}", moved.value);
    let back = song.keys(
        ids::MOVE_MARKER,
        json!({ "direction": "left", "by": "grid" }),
    );
    assert_eq!(back.marker, Ticks(BAR));
}

// Spec: automation-editing R21.
#[test]
fn option_left_and_right_jump_to_the_previous_and_next_breakpoint() {
    let mut song = song();
    song.lane_at(BAR / 2);
    let next = song.keys(
        ids::MOVE_MARKER,
        json!({ "direction": "right", "by": "breakpoint" }),
    );
    assert_eq!(next.marker, Ticks(BAR));
    let again = song.keys(
        ids::MOVE_MARKER,
        json!({ "direction": "right", "by": "breakpoint" }),
    );
    assert_eq!(again.marker, Ticks(2 * BAR));
    // Past the last, it stays.
    let last = song.keys(
        ids::MOVE_MARKER,
        json!({ "direction": "right", "by": "breakpoint" }),
    );
    assert_eq!(last.marker, Ticks(2 * BAR));
    let before = song.keys(
        ids::MOVE_MARKER,
        json!({ "direction": "left", "by": "breakpoint" }),
    );
    assert_eq!(before.marker, Ticks(BAR));
}

// Spec: automation-editing R22, R23.
#[test]
fn enter_selects_a_breakpoint_and_enter_again_keeps_the_edit_as_one_undo_step() {
    let mut song = song();
    song.lane_at(BAR);
    let before = song.envelope();
    let steps = song.steps();
    let selected = song.keys(ids::ENTER, json!({}));
    assert_eq!(selected.selected, Some(Ticks(BAR)));
    assert_eq!(
        song.state().selected_breakpoint.map(|b| b.at),
        Some(Ticks(BAR))
    );
    song.keys(ids::MOVE_SELECTED, json!({ "direction": "down" }));
    song.keys(ids::MOVE_SELECTED, json!({ "direction": "down" }));
    song.keys(ids::MOVE_SELECTED, json!({ "direction": "right" }));
    // Nothing is a step until it's let go of, and an agent's calls don't
    // wait for it.
    assert_eq!(song.steps(), steps);
    assert!(!song.shell.gesture_under_way());
    let kept = song.keys(ids::ENTER, json!({}));
    assert_eq!(kept.selected, None);
    assert!(!song.shell.gesture_under_way());
    assert_ne!(song.envelope(), before);
    assert_eq!(song.steps(), steps + 1, "one undo step for the whole edit");
    song.run("edit.undo", json!({}));
    assert_eq!(song.envelope(), before);
}

// Spec: automation-editing R22. With no breakpoint at the insert marker,
// Enter makes one at the envelope's value there.
#[test]
fn enter_makes_a_breakpoint_at_the_envelopes_value_and_selects_it() {
    let mut song = song();
    song.lane_at(BAR / 2);
    let on_line = song
        .doc()
        .track(song.track)
        .unwrap()
        .automation_for(&volume())
        .unwrap()
        .envelope
        .value_at(Ticks(BAR / 2))
        .unwrap();
    let made = song.keys(ids::ENTER, json!({}));
    assert_eq!(made.selected, Some(Ticks(BAR / 2)));
    assert!(song.envelope().contains(&(BAR / 2, on_line)));
    song.keys(ids::ENTER, json!({}));
    assert!(song.envelope().contains(&(BAR / 2, on_line)));
}

// Spec: automation-editing R23. Left and Right move the breakpoint a grid
// step, not past its neighbours; Up and Down change its value by a step of
// the control.
#[test]
fn the_arrows_move_a_selected_breakpoint_in_time_and_value() {
    let mut song = song();
    song.lane_at(BAR);
    let grid = song.grid();
    song.keys(ids::ENTER, json!({}));
    let moved = song.keys(ids::MOVE_SELECTED, json!({ "direction": "right" }));
    assert_eq!(moved.selected, Some(Ticks(BAR + grid)));
    assert_eq!(moved.marker, Ticks(BAR + grid));
    assert!(song.envelope().iter().any(|(at, _)| *at == BAR + grid));
    let (_, before) = song
        .envelope()
        .into_iter()
        .find(|(at, _)| *at == BAR + grid)
        .unwrap();
    song.keys(ids::MOVE_SELECTED, json!({ "direction": "up" }));
    let (_, after) = song
        .envelope()
        .into_iter()
        .find(|(at, _)| *at == BAR + grid)
        .unwrap();
    // The volume fader takes a step of 1 dB.
    let db = |gain: f64| 20.0 * gain.log10();
    assert!(
        (db(after) - db(before) - 1.0).abs() < 0.01,
        "{before} {after}"
    );
    // Not past a neighbour: the last breakpoint can't go earlier than the
    // one before it.
    song.keys(ids::SELECT_ADJACENT, json!({}));
    assert_eq!(
        song.state().selected_breakpoint.map(|b| b.at),
        Some(Ticks(2 * BAR))
    );
    for _ in 0..200 {
        song.keys(ids::MOVE_SELECTED, json!({ "direction": "left" }));
    }
    let points = song.envelope();
    assert!(points.windows(2).all(|pair| pair[0].0 < pair[1].0));
    assert_eq!(points.len(), 3);
    assert!(points[2].0 > points[1].0);
}

// Spec: automation-editing R23. Esc puts the breakpoint back as it was, and
// removes one Enter made.
#[test]
fn esc_drops_the_edit_and_removes_a_breakpoint_enter_made() {
    let mut song = song();
    song.lane_at(BAR);
    let before = song.envelope();
    let steps = song.steps();
    song.keys(ids::ENTER, json!({}));
    song.keys(ids::MOVE_SELECTED, json!({ "direction": "up" }));
    song.keys(ids::MOVE_SELECTED, json!({ "direction": "right" }));
    let dropped = song.keys(ids::CANCEL, json!({}));
    assert_eq!(dropped.selected, None);
    assert_eq!(dropped.marker, Ticks(BAR), "the marker is where it was");
    assert_eq!(song.envelope(), before);
    assert_eq!(song.steps(), steps, "no undo step");
    // A breakpoint Enter made is removed.
    song.run(
        view_ids::SET_INSERT_MARKER,
        json!({ "at": Ticks(BAR / 2), "track": song.track, "automation_lane": true }),
    );
    song.keys(ids::ENTER, json!({}));
    assert_eq!(song.envelope().len(), 4);
    song.keys(ids::CANCEL, json!({}));
    assert_eq!(song.envelope(), before);
    // With nothing selected, Esc isn't the envelope's to answer.
    assert!(!song.can(ids::CANCEL));
}

// Spec: automation-editing R22, R24. Tab and Delete answer only while there
// is a breakpoint to select or delete.
#[test]
fn tab_and_delete_answer_only_with_a_breakpoint_to_act_on() {
    let mut song = song();
    song.lane_at(BAR / 2);
    assert!(
        !song.can(ids::SELECT_ADJACENT),
        "Tab only while one is selected"
    );
    assert!(!song.can(ids::DELETE_BREAKPOINT), "none at the marker");
    song.run(
        view_ids::SET_INSERT_MARKER,
        json!({ "at": Ticks(BAR), "track": song.track, "automation_lane": true }),
    );
    assert!(song.can(ids::DELETE_BREAKPOINT), "one at the marker");
    song.keys(ids::ENTER, json!({}));
    assert!(song.can(ids::SELECT_ADJACENT));
    let next = song.keys(ids::SELECT_ADJACENT, json!({}));
    assert_eq!(next.selected, Some(Ticks(2 * BAR)));
    let previous = song.keys(ids::SELECT_ADJACENT, json!({ "reverse": true }));
    assert_eq!(previous.selected, Some(Ticks(BAR)));
}

// Spec: automation-editing R23, R24.
#[test]
fn delete_deletes_the_selected_breakpoint_or_the_one_at_the_marker() {
    let mut song = song();
    song.lane_at(BAR);
    let steps = song.steps();
    song.keys(ids::DELETE_BREAKPOINT, json!({}));
    assert_eq!(song.envelope().len(), 2);
    assert_eq!(song.steps(), steps + 1);
    song.run("edit.undo", json!({}));
    assert_eq!(song.envelope().len(), 3);
    // After Undo, the breakpoint it restored is the focused one.
    song.run(
        view_ids::SET_INSERT_MARKER,
        json!({ "at": Ticks(0), "track": song.track, "automation_lane": true }),
    );
    song.run(
        ids::SELECT_BREAKPOINT,
        json!({ "track": song.track, "target": volume(), "at": Ticks(2 * BAR) }),
    );
    song.keys(ids::DELETE_BREAKPOINT, json!({}));
    assert_eq!(song.envelope().len(), 2);
    song.run(
        view_ids::SET_INSERT_MARKER,
        json!({ "at": Ticks(0), "track": song.track, "automation_lane": true }),
    );
    song.run("edit.undo", json!({}));
    assert_eq!(song.state().insert_marker, Ticks(2 * BAR));
    song.run("edit.redo", json!({}));
    assert_eq!(song.envelope().len(), 2);
}

// Spec: automation-editing R23. A typed value is in the units the control
// shows.
#[test]
fn a_typed_value_sets_the_selected_breakpoint_in_the_controls_units() {
    let mut song = song();
    song.lane_at(BAR);
    song.keys(ids::ENTER, json!({}));
    let set = song.keys(ids::SET_VALUE, json!({ "text": "-6" }));
    assert_eq!(set.selected, Some(Ticks(BAR)), "still selected");
    let (_, gain) = song
        .envelope()
        .into_iter()
        .find(|(at, _)| *at == BAR)
        .unwrap();
    assert!((20.0 * gain.log10() + 6.0).abs() < 0.05, "{gain}");
    assert!(
        song.shell
            .run(ids::SET_VALUE, json!({ "text": "loud" }))
            .is_err()
    );
    song.keys(ids::ENTER, json!({}));
    assert_eq!(song.steps(), 1);
}

// Spec: automation-editing R27. Add Value types the value of a new
// breakpoint at a place on the line, as one undo step.
#[test]
fn add_value_makes_a_breakpoint_with_the_typed_value() {
    let mut song = song();
    song.lane_at(BAR / 2);
    let steps = song.steps();
    song.keys(
        ids::SET_VALUE,
        json!({ "text": "-12 dB", "at": Ticks(BAR / 2) }),
    );
    let (_, gain) = song
        .envelope()
        .into_iter()
        .find(|(at, _)| *at == BAR / 2)
        .unwrap();
    assert!((20.0 * gain.log10() + 12.0).abs() < 0.05);
    assert_eq!(song.steps(), steps + 1);
    assert!(!song.shell.gesture_under_way());
}

// Spec: automation-editing R27. Edit Value moves the other selected
// breakpoints by the same share of the travel.
#[test]
fn edit_value_moves_the_other_selected_breakpoints_by_the_same_share() {
    let mut song = song();
    song.run(view_ids::AUTOMATION_MODE, json!({ "on": true }));
    song.run(
        view_ids::SELECT_TIME,
        json!({
            "start": Ticks(0),
            "end": Ticks(BAR * 2),
            "lanes": [{ "type": "automation", "track": song.track,
                         "target": { "type": "mixer", "param": "volume" } }],
        }),
    );
    // The time selection selects the breakpoints at bars 1 and 2.
    let before = song.envelope();
    song.run(
        ids::SELECT_BREAKPOINT,
        json!({ "track": song.track, "target": volume(), "at": Ticks(BAR) }),
    );
    song.keys(ids::SET_VALUE, json!({ "text": "3" }));
    let after = song.envelope();
    assert_eq!(after.len(), before.len());
    let gain = |db: f64| 10.0_f64.powf(db / 20.0);
    assert!((after[1].1 - gain(3.0)).abs() < 1e-3, "{after:?}");
    // The first breakpoint, at the bottom of the fader, rose by the same
    // share of the travel; the one outside the selection stayed.
    let travel = |gain: f64| bl_model::fader::fader_position(gain);
    let share = travel(after[1].1) - travel(before[1].1);
    assert!(
        (travel(after[0].1) - travel(before[0].1) - share).abs() < 1e-6,
        "{before:?} {after:?}"
    );
    assert_eq!(after[2], before[2]);
}

// Spec: automation-editing R23. The edit ends when another command edits
// the set.
#[test]
fn the_edit_ends_when_another_command_edits_the_set() {
    let mut song = song();
    song.lane_at(BAR);
    song.keys(ids::ENTER, json!({}));
    song.keys(ids::MOVE_SELECTED, json!({ "direction": "up" }));
    assert!(song.state().selected_breakpoint.is_some());
    song.run("mixer.set_pan", json!({ "track": song.track, "pan": 0.25 }));
    assert!(!song.shell.gesture_under_way());
    assert_eq!(song.state().selected_breakpoint, None);
    assert_eq!(song.steps(), 2, "the breakpoint edit, then the pan");
}

// Spec: automation-editing R23. The edit ends when the keyboard focus
// leaves.
#[test]
fn the_edit_ends_when_the_focus_leaves() {
    let mut song = song();
    song.lane_at(BAR);
    song.keys(ids::ENTER, json!({}));
    song.keys(ids::MOVE_SELECTED, json!({ "direction": "up" }));
    song.shell.set_focused_area(Area::Browser);
    assert!(!song.shell.gesture_under_way());
    assert_eq!(song.steps(), 1);
}

// Spec: automation-editing R26.
#[test]
fn delete_envelope_deletes_the_automation_of_the_shown_parameter() {
    let mut song = song();
    song.lane_at(BAR);
    song.keys(ids::DELETE, json!({}));
    assert!(song.envelope().is_empty());
    song.run("edit.undo", json!({}));
    assert_eq!(song.envelope().len(), 3);
}

// Spec: automation-editing R29. The selection is view state that
// `arrangement.state` reports, and an agent sets it.
#[test]
fn select_breakpoint_is_view_state_that_arrangement_state_reports() {
    let mut song = song();
    song.run(view_ids::AUTOMATION_MODE, json!({ "on": true }));
    let before = song.envelope();
    let state: EnvelopeKeyState = serde_json::from_value(song.run(
        ids::SELECT_BREAKPOINT,
        json!({ "track": song.track, "target": volume(), "at": Ticks(2 * BAR) }),
    ))
    .unwrap();
    assert_eq!(state.selected, Some(Ticks(2 * BAR)));
    let reported = song.state();
    assert_eq!(
        reported
            .selected_breakpoint
            .map(|b| (b.track, b.target, b.at)),
        Some((song.track, volume(), Ticks(2 * BAR)))
    );
    assert_eq!(reported.insert_marker, Ticks(2 * BAR));
    assert_eq!(song.envelope(), before, "it changes nothing in the set");
    // `null` lets go of it.
    song.run(
        ids::SELECT_BREAKPOINT,
        json!({ "track": song.track, "target": volume(), "at": null }),
    );
    assert_eq!(song.state().selected_breakpoint, None);
    // A lane that shows another parameter is refused.
    assert!(
        song.shell
            .run(
                ids::SELECT_BREAKPOINT,
                json!({ "track": song.track, "target": pan(), "at": Ticks(0) }),
            )
            .is_err()
    );
}

/// A MIDI clip on the first MIDI track with a pan envelope, the Clip View on
/// its Envelopes tab showing Pan, and the detail area focused.
struct EditorSong {
    shell: Shell,
    clip: bl_model::ClipId,
}

fn editor() -> EditorSong {
    let mut song = song();
    let track = song
        .doc()
        .top_level()
        .find(|id| {
            song.doc()
                .track(*id)
                .is_some_and(|track| track.kind() == bl_model::TrackKind::Midi)
        })
        .unwrap();
    let scene = song.doc().scenes().next().unwrap().id;
    song.run(
        "session.insert_empty_midi_clip",
        json!({ "track": track, "scene": scene }),
    );
    song.run("session.select", json!({ "track": track, "scene": scene }));
    let clip = song.doc().slot(track, scene).unwrap().id;
    song.run("view.toggle_clip_view", json!({}));
    song.run("view.clip_tab", json!({ "tab": "envelopes" }));
    song.run("view.clip_envelope", json!({ "target": pan() }));
    for (at, value) in [(0, 0.0), (BAR, 0.5)] {
        song.run(
            "clip.set_envelope_breakpoint",
            json!({ "clip": clip, "target": pan(), "at": Ticks(at), "value": value }),
        );
    }
    song.run(view_ids::SET_GRID, json!({ "fixed": "quarter" }));
    song.shell.set_focused_area(Area::Detail);
    EditorSong {
        shell: song.shell,
        clip,
    }
}

impl EditorSong {
    fn run(&mut self, id: &str, args: Value) -> Value {
        self.shell
            .run(id, args)
            .unwrap_or_else(|e| panic!("{id} failed: {e}"))
    }

    fn keys(&mut self, id: &str, args: Value) -> EnvelopeKeyState {
        serde_json::from_value(self.run(id, args)).unwrap()
    }

    fn points(&self) -> Vec<Breakpoint> {
        self.shell
            .editor()
            .document()
            .clip(self.clip)
            .unwrap()
            .envelope(&pan())
            .map(|e| e.envelope.points().collect())
            .unwrap_or_default()
    }
}

// Spec: clip-envelopes R14, automation-editing R20, R21.
#[test]
fn the_envelope_editor_has_the_keys_and_its_own_insert_marker() {
    let mut editor = editor();
    assert!(
        editor
            .shell
            .active_contexts()
            .contains(&bl_commands::Context::Envelope)
    );
    let moved = editor.keys(
        ids::MOVE_MARKER,
        json!({ "direction": "right", "by": "breakpoint" }),
    );
    assert_eq!(moved.envelope, EnvelopeKeys::Clip);
    assert_eq!(moved.marker, Ticks(BAR));
    assert_eq!(moved.value, "50%");
    let state = editor.run("view.clip_envelope", json!({ "target": pan() }));
    assert_eq!(state["envelope_marker"], json!(BAR));
}

// Spec: clip-envelopes R14, automation-editing R22, R23.
#[test]
fn the_envelope_editors_breakpoints_are_made_moved_kept_and_dropped_from_the_keys() {
    let mut editor = editor();
    let before = editor.points();
    editor.keys(
        ids::MOVE_MARKER,
        json!({ "direction": "right", "by": "grid" }),
    );
    let made = editor.keys(ids::ENTER, json!({}));
    let at = made.marker;
    assert_eq!(made.selected, Some(at));
    assert_eq!(editor.points().len(), 3);
    editor.keys(ids::MOVE_SELECTED, json!({ "direction": "up" }));
    editor.keys(ids::CANCEL, json!({}));
    assert_eq!(editor.points(), before, "Esc removes the one Enter made");
    editor.keys(
        ids::MOVE_MARKER,
        json!({ "direction": "right", "by": "breakpoint" }),
    );
    editor.keys(ids::ENTER, json!({}));
    editor.keys(ids::SET_VALUE, json!({ "text": "25%" }));
    let kept = editor.keys(ids::ENTER, json!({}));
    assert_eq!(kept.selected, None);
    let moved = editor.points();
    assert_eq!(moved[1].value, 0.25);
    editor.run("edit.undo", json!({}));
    assert_eq!(editor.points(), before, "one undo step");
    // Cmd-Delete clears the shown envelope.
    editor.keys(ids::DELETE, json!({}));
    assert!(editor.points().is_empty());
}

// Spec: clip-envelopes R14, automation-editing R23, R24.
#[test]
fn the_envelope_editors_delete_and_tab_answer_only_with_a_breakpoint() {
    let mut editor = editor();
    editor.keys(
        ids::MOVE_MARKER,
        json!({ "direction": "right", "by": "grid" }),
    );
    assert!(!editor.shell.can_invoke(ids::DELETE_BREAKPOINT, &json!({})));
    assert!(!editor.shell.can_invoke(ids::SELECT_ADJACENT, &json!({})));
    editor.keys(ids::ENTER, json!({}));
    assert!(editor.shell.can_invoke(ids::SELECT_ADJACENT, &json!({})));
    let next = editor.keys(ids::SELECT_ADJACENT, json!({}));
    assert_eq!(next.selected, Some(Ticks(BAR)));
    editor.keys(ids::DELETE_BREAKPOINT, json!({}));
    assert_eq!(editor.points().len(), 2);
}

fn clip_setting(param: &str) -> Value {
    json!({ "type": "clip", "param": param })
}

impl Song {
    /// The audio clip's envelope for `target`, as `(ticks, value)`.
    fn clip_envelope(&self, clip: bl_model::ClipId, target: &Value) -> Vec<(i64, f64)> {
        let target: AutomationTarget = serde_json::from_value(target.clone()).unwrap();
        self.doc()
            .clip(clip)
            .unwrap()
            .envelope(&target)
            .map(|e| e.envelope.points().map(|p| (p.at.get(), p.value)).collect())
            .unwrap_or_default()
    }
}

// Spec: warp-modes R29, clip-envelopes R14. A clip's Gain envelope is a share
// from 0 to 100 %, and an offset envelope is in the setting's units, so an
// arrow key moves them by 1 % of their travel rather than through the
// setting's own descriptor.
#[test]
fn a_clip_gain_breakpoint_steps_as_a_share() {
    let mut song = song();
    let clip = song
        .doc()
        .track(song.track)
        .unwrap()
        .clips()
        .next()
        .unwrap()
        .id;
    song.run("arrangement.select_clips", json!({ "clips": [clip] }));
    song.run("view.toggle_clip_view", json!({}));
    song.run("view.clip_tab", json!({ "tab": "envelopes" }));
    let gain = clip_setting("gain");
    song.run("view.clip_envelope", json!({ "target": gain }));
    song.shell.set_focused_area(Area::Detail);
    song.run(
        "clip.set_envelope_breakpoint",
        json!({ "clip": clip, "target": gain, "at": Ticks(0), "value": 100.0 }),
    );
    song.run(
        ids::SELECT_ENVELOPE_BREAKPOINT,
        json!({ "clip": clip, "target": gain, "at": Ticks(0) }),
    );
    song.keys(ids::MOVE_SELECTED, json!({ "direction": "down" }));
    let at_zero = |song: &Song| song.clip_envelope(clip, &gain)[0].1;
    assert!((at_zero(&song) - 99.0).abs() < 1e-9, "{}", at_zero(&song));
    for _ in 0..2 {
        song.keys(ids::MOVE_SELECTED, json!({ "direction": "up" }));
    }
    assert!((at_zero(&song) - 100.0).abs() < 1e-9, "held at the top");
    assert_eq!(
        song.shell.keyboard_breakpoint_text(Ticks(0)).as_deref(),
        Some("100%")
    );
    // Typed text is a share too, with or without its percent sign.
    song.run(ids::SET_VALUE, json!({ "text": "40%" }));
    assert!((at_zero(&song) - 40.0).abs() < 1e-9, "{}", at_zero(&song));

    // Texture Flux's envelope is an offset in per cent, from -100 to 100.
    song.run(
        "clip.set_warp_mode",
        json!({ "clip": clip, "mode": "texture" }),
    );
    let flux = clip_setting("texture_flux");
    song.run("view.clip_envelope", json!({ "target": flux }));
    song.run(
        "clip.set_envelope_breakpoint",
        json!({ "clip": clip, "target": flux, "at": Ticks(0), "value": -20.0 }),
    );
    song.run(
        ids::SELECT_ENVELOPE_BREAKPOINT,
        json!({ "clip": clip, "target": flux, "at": Ticks(0) }),
    );
    song.keys(ids::MOVE_SELECTED, json!({ "direction": "up" }));
    let flux_value = song.clip_envelope(clip, &flux)[0].1;
    assert!((flux_value + 19.0).abs() < 1e-9, "{flux_value}");
    assert_eq!(
        song.shell.keyboard_breakpoint_text(Ticks(0)).as_deref(),
        Some("-19 %")
    );
}

// Spec: warp-modes R28. A track's lane for a clip setting holds the setting's
// own values, so typed text reads in its unit.
#[test]
fn a_clip_setting_lane_reads_typed_values_in_its_units() {
    let mut song = song();
    let target = clip_setting("transpose");
    song.run(view_ids::AUTOMATION_MODE, json!({ "on": true }));
    song.run(
        view_ids::SHOW_AUTOMATION,
        json!({ "track": song.track, "target": target }),
    );
    song.run(
        "automation.set_breakpoint",
        json!({ "track": song.track, "target": target, "at": Ticks(0), "value": 0 }),
    );
    song.run(
        ids::SELECT_BREAKPOINT,
        json!({ "track": song.track, "target": target, "at": Ticks(0) }),
    );
    song.run(ids::SET_VALUE, json!({ "text": "12" }));
    let target: AutomationTarget = serde_json::from_value(target).unwrap();
    let value = song
        .doc()
        .track(song.track)
        .unwrap()
        .automation_for(&target)
        .unwrap()
        .envelope
        .points()
        .next()
        .unwrap()
        .value;
    assert!((value - 12.0).abs() < 1e-9, "{value}");
    assert_eq!(
        song.shell.keyboard_breakpoint_text(Ticks(0)).as_deref(),
        Some("12 st")
    );
}

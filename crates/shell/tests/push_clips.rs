//! Clip Mode on Push 2 in the shell (`docs/specs/push-2-tracks.md`, R43 to
//! R58): the clip's loop, markers, zoom and audio settings, Crop, loading a
//! sample into an empty slot, the layouts' displays and the note settings of
//! held steps, driven through the simulated Push 2 on the real engine. Every
//! edit runs the app's own commands, one undo step for each gesture.

use std::path::{Path, PathBuf};

use bl_core::time::Ticks;
use bl_model::{AudioData, ClipData, ClipId, MidiData, SceneId, TrackId, TrackKind};
use serde_json::{Value, json};

use super::push_beats::Rig;

/// A beat, in ticks.
const BEAT: i64 = 705_600;
/// A sixteenth, which a step is by default.
const STEP: i64 = BEAT / 4;

fn session_view(rig: &mut Rig) {
    if rig.shell.main_view() != bl_shell::MainView::Session {
        rig.run("view.toggle_session_arrangement", json!({}));
    }
    rig.pass(40);
}

fn scene(rig: &Rig) -> SceneId {
    rig.shell
        .editor()
        .document()
        .scenes()
        .next()
        .map(|s| s.id)
        .unwrap()
}

/// A one-bar MIDI clip in the first scene of the rig's MIDI track.
fn midi_clip(rig: &mut Rig) -> ClipId {
    let (track, scene) = (rig.track, scene(rig));
    rig.run(
        "session.insert_empty_midi_clip",
        json!({ "track": track, "scene": scene }),
    );
    rig.shell.editor().document().slot(track, scene).unwrap().id
}

fn midi(rig: &Rig, clip: ClipId) -> MidiData {
    match &rig.shell.editor().document().clip(clip).unwrap().data {
        ClipData::Midi(midi) => midi.clone(),
        ClipData::Audio(_) => panic!("a MIDI clip"),
    }
}

fn audio(rig: &Rig, clip: ClipId) -> AudioData {
    match &rig.shell.editor().document().clip(clip).unwrap().data {
        ClipData::Audio(audio) => audio.clone(),
        ClipData::Midi(_) => panic!("an audio clip"),
    }
}

fn clip_state(rig: &mut Rig) -> Value {
    rig.state()["clip"].clone()
}

/// One touch of encoder `index` (from 0), turned by `steps`.
fn turn(rig: &mut Rig, index: u8, steps: i32) {
    assert!(rig.sim.touch(index));
    rig.pass(10);
    assert!(rig.sim.turn(index, steps));
    rig.pass(10);
    assert!(rig.sim.release(index));
    rig.pass(10);
}

/// What the display says the controls on the encoders are, by name.
fn controls(rig: &mut Rig) -> Vec<(String, String)> {
    clip_state(rig)["controls"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            (
                c["name"].as_str().unwrap().to_owned(),
                c["value"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

/// A folder of its own under the temporary folder, removed when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("bl-push-clips-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Writes a 16-bit mono WAV file of one second at 48 kHz with a quiet sine.
fn wav(path: &Path) {
    let frames = 48_000u32;
    let data = frames * 2;
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&48_000u32.to_le_bytes());
    bytes.extend_from_slice(&96_000u32.to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data.to_le_bytes());
    for i in 0..frames {
        let phase = f64::from(i) * 220.0 / 48_000.0 * std::f64::consts::TAU;
        bytes.extend_from_slice(&((phase.sin() * 8000.0) as i16).to_le_bytes());
    }
    std::fs::write(path, bytes).unwrap();
}

/// The set's first audio track, selected, with the Session View shown.
fn audio_track(rig: &mut Rig) -> TrackId {
    let track = rig
        .shell
        .editor()
        .document()
        .tracks()
        .find(|t| t.kind() == TrackKind::Audio)
        .map(bl_model::Track::id)
        .expect("the default set has audio tracks");
    rig.run("arrangement.select_tracks", json!({ "tracks": [track] }));
    session_view(rig);
    track
}

/// An audio clip of `file` in the first scene of `track`.
fn audio_clip(rig: &mut Rig, track: TrackId, file: &Path) -> ClipId {
    let scene = scene(rig);
    rig.run(
        "session.drop_files",
        json!({ "paths": [file.display().to_string()], "track": track, "scene": scene }),
    );
    rig.pass(40);
    rig.shell
        .editor()
        .document()
        .slot(track, scene)
        .expect("the clip")
        .id
}

// Spec: push-2-tracks R43, R44, R45. Clip Mode on a looping MIDI clip
// switches Loop with the second upper button, moves Loop Position by a beat
// and, with Shift, by a sixteenth, carrying Start Offset where they meet;
// with Loop off the first two encoders set Start and End.
#[test]
fn loop_position_moves_by_a_beat_and_a_sixteenth_and_carries_the_start() {
    let mut rig = Rig::new();
    session_view(&mut rig);
    let id = midi_clip(&mut rig);
    rig.run(
        "clip.set_midi_loop",
        json!({ "clip": id, "start": BEAT, "end": 3 * BEAT }),
    );
    rig.run("clip.set_midi_region", json!({ "clip": id, "start": BEAT }));
    rig.tap("clip");
    assert_eq!(rig.state()["display_mode"], "clip");
    let state = clip_state(&mut rig);
    assert_eq!(state["shows"], "midi", "{state}");
    assert_eq!(state["clip"], json!(id));
    assert_eq!(state["looping"], false);

    // Loop on the second upper button.
    let steps = rig.steps();
    rig.tap("upper_2");
    assert!(midi(&rig, id).looping);
    assert_eq!(rig.steps(), steps + 1);
    assert_eq!(clip_state(&mut rig)["looping"], true);
    let names: Vec<_> = controls(&mut rig).into_iter().map(|c| c.0).collect();
    assert_eq!(
        names,
        ["Zoom", "Loop Position", "Loop Length", "Start Offset"]
    );

    // A beat: the loop and the Start marker move together, in one step.
    let steps = rig.steps();
    turn(&mut rig, 1, 10);
    let data = midi(&rig, id);
    assert_eq!(
        (data.loop_start.get(), data.loop_end.map(Ticks::get)),
        (2 * BEAT, Some(4 * BEAT))
    );
    assert_eq!(data.start.get(), 2 * BEAT, "Start Offset came along");
    assert_eq!(rig.steps(), steps + 1, "one undo step for a touch");
    turn(&mut rig, 1, -10);
    assert_eq!(midi(&rig, id).loop_start.get(), BEAT);

    // A sixteenth with Shift.
    assert!(rig.sim.press("shift"));
    rig.pass(20);
    turn(&mut rig, 1, 10);
    assert!(rig.sim.let_go("shift"));
    rig.pass(20);
    let data = midi(&rig, id);
    assert_eq!(data.loop_start.get(), BEAT + STEP);
    assert_eq!(data.start.get(), BEAT + STEP);

    // Start Offset moves the Start marker alone, and then Loop Position
    // leaves it where it is.
    assert!(rig.sim.press("shift"));
    rig.pass(20);
    turn(&mut rig, 3, 10);
    assert!(rig.sim.let_go("shift"));
    rig.pass(20);
    assert_eq!(midi(&rig, id).start.get(), BEAT + 2 * STEP);
    turn(&mut rig, 1, -10);
    let data = midi(&rig, id);
    assert_eq!(data.loop_start.get(), STEP);
    assert_eq!(data.start.get(), BEAT + 2 * STEP, "not at one place");

    // Loop Length.
    turn(&mut rig, 2, 10);
    let data = midi(&rig, id);
    assert_eq!(data.loop_end.map(Ticks::get), Some(STEP + 3 * BEAT));

    // Loop off: Start and End.
    rig.tap("upper_2");
    assert!(!midi(&rig, id).looping);
    let names: Vec<_> = controls(&mut rig).into_iter().map(|c| c.0).collect();
    assert_eq!(names, ["Zoom", "Start", "End"]);
    turn(&mut rig, 1, 10);
    let data = midi(&rig, id);
    assert_eq!(data.start.get(), 2 * BEAT + 2 * STEP);
    assert!(rig.sim.press("shift"));
    rig.pass(20);
    turn(&mut rig, 2, -10);
    assert!(rig.sim.let_go("shift"));
    rig.pass(20);
    assert_eq!(
        midi(&rig, id).end.map(Ticks::get),
        Some(4 * BEAT - STEP),
        "a sixteenth back from the end of the clip"
    );
}

// Spec: push-2-tracks R47, R48. An audio clip's Clip Mode steps Warp Mode,
// sets Gain, and sets Transpose in semitones and, with Shift, cents.
#[test]
fn an_audio_clip_steps_warp_mode_sets_gain_and_transposes() {
    let scratch = Scratch::new("audio");
    wav(&scratch.0.join("Tone.wav"));
    let mut rig = Rig::new();
    let track = audio_track(&mut rig);
    let id = audio_clip(&mut rig, track, &scratch.0.join("Tone.wav"));
    rig.tap("clip");
    let state = clip_state(&mut rig);
    assert_eq!(state["shows"], "audio", "{state}");
    let names: Vec<_> = controls(&mut rig).into_iter().map(|c| c.0).collect();
    assert_eq!(
        names,
        ["Zoom", "Start", "End", "Warp Mode", "Gain", "Transpose"]
    );

    let before = audio(&rig, id);
    turn(&mut rig, 4, 10);
    let after = audio(&rig, id);
    assert_ne!(before.warp_mode, after.warp_mode, "one list step");
    turn(&mut rig, 4, -10);
    assert_eq!(audio(&rig, id).warp_mode, before.warp_mode);

    let steps = rig.steps();
    turn(&mut rig, 5, 20);
    assert!(audio(&rig, id).gain > before.gain);
    assert_eq!(rig.steps(), steps + 1);

    turn(&mut rig, 6, 20);
    assert_eq!(audio(&rig, id).transpose, 2);
    assert!(rig.sim.press("shift"));
    rig.pass(20);
    turn(&mut rig, 6, 30);
    assert!(rig.sim.let_go("shift"));
    rig.pass(20);
    let data = audio(&rig, id);
    assert_eq!((data.transpose, data.detune), (2, 3.0), "cents with Shift");

    // Start moves by a beat of the set's tempo, in seconds, as the clip is
    // not warped.
    assert!(!data.warp);
    turn(&mut rig, 1, 10);
    assert!(audio(&rig, id).start > 0.0);
}

// Spec: push-2-tracks R49. Crop on the third upper button deletes the notes
// outside the loop, as one undo step.
#[test]
fn crop_deletes_the_notes_outside_the_loop_as_one_step() {
    let mut rig = Rig::new();
    session_view(&mut rig);
    let id = midi_clip(&mut rig);
    for start in [0, 3 * BEAT / 2, 5 * BEAT / 2, 7 * BEAT / 2] {
        rig.run(
            "note.add",
            json!({ "clip": id, "key": 36, "start": start, "length": STEP }),
        );
    }
    rig.run(
        "clip.set_midi_loop",
        json!({ "clip": id, "start": 0, "end": 2 * BEAT }),
    );
    rig.run(
        "clip.set_loop_enabled",
        json!({ "clip": id, "enabled": true }),
    );
    rig.tap("clip");
    assert_eq!(midi(&rig, id).notes.len(), 4);
    let steps = rig.steps();
    rig.tap("upper_3");
    assert_eq!(rig.steps(), steps + 1, "one undo step");
    let mut starts: Vec<_> = midi(&rig, id).notes.iter().map(|n| n.start.get()).collect();
    starts.sort_unstable();
    assert_eq!(starts, [0, 3 * BEAT / 2], "only the notes in the loop stay");
    rig.run("edit.undo", json!({}));
    assert_eq!(midi(&rig, id).notes.len(), 4);
}

// Spec: push-2-tracks R43. An empty audio slot offers to load a sample, and
// pressing its button opens Browse Mode listing Samples, where Load puts the
// file in that slot as a new clip; an empty MIDI slot says it is empty.
#[test]
fn an_empty_audio_slot_loads_a_sample_through_browse_mode() {
    let scratch = Scratch::new("samples");
    let library = scratch.0.join("Library");
    std::fs::create_dir_all(library.join("Samples")).unwrap();
    wav(&library.join("Samples/Kick.wav"));
    let mut rig = Rig::new();
    rig.shell.set_user_library(Some(library));
    let track = audio_track(&mut rig);
    assert!(
        rig.shell
            .wait_for_browser_index(std::time::Duration::from_secs(20))
    );
    rig.tap("clip");
    assert_eq!(clip_state(&mut rig)["shows"], "empty_audio");
    let text = rig.sim_text();
    assert!(
        text.iter()
            .any(|line| line.contains("upper 1: Load Sample")),
        "{text:?}"
    );

    rig.tap("upper_1");
    let state = rig.state();
    assert_eq!(state["display_mode"], "browse", "{state}");
    assert_eq!(state["browse"]["load"], "sample");
    assert_eq!(state["browse"]["columns"][0]["entries"], json!(["Samples"]));
    // Down opens Samples, and its first entry is the file.
    rig.tap("upper_8");
    rig.step();
    let state = rig.state();
    assert_eq!(
        state["browse"]["columns"][1]["entries"],
        json!(["Kick"]),
        "{state}"
    );
    rig.tap("upper_8");
    let steps = rig.steps();
    rig.tap("lower_8");
    rig.pass(40);
    assert_eq!(rig.steps(), steps + 1, "one undo step");
    let scene = scene(&rig);
    let clip = rig.shell.editor().document().slot(track, scene);
    assert!(
        clip.is_some_and(|c| matches!(c.data, ClipData::Audio(_))),
        "the file is a clip in that slot"
    );

    // An empty MIDI slot says so.
    rig.run(
        "arrangement.select_tracks",
        json!({ "tracks": [rig.track] }),
    );
    rig.tap("clip");
    rig.pass(40);
    assert_eq!(clip_state(&mut rig)["shows"], "empty_midi");
    let text = rig.sim_text();
    assert!(
        text.iter().any(|line| line.contains("This slot is empty")),
        "{text:?}"
    );
}

// Spec: push-2-tracks R43. While the Arrangement is shown, Clip Mode shows
// the clip the Clip View shows.
#[test]
fn with_the_arrangement_shown_clip_mode_shows_the_clip_view_s_clip() {
    let mut rig = Rig::new();
    let id = midi_clip(&mut rig);
    let (track, scene) = (rig.track, scene(&rig));
    rig.run("session.select", json!({ "track": track, "scene": scene }));
    rig.pass(40);
    assert_eq!(rig.shell.main_view(), bl_shell::MainView::Arrangement);
    rig.tap("clip");
    assert_eq!(clip_state(&mut rig)["clip"], json!(id));
}

/// The clip's notes as key, start, length and velocity, in order.
fn notes_of(rig: &Rig, clip: ClipId) -> Vec<(u8, i64, i64, u8)> {
    let mut notes: Vec<_> = midi(rig, clip)
        .notes
        .iter()
        .map(|n| (n.key, n.start.get(), n.length.get(), n.velocity))
        .collect();
    notes.sort_unstable();
    notes
}

/// A clip of pad 36 with a note on each of the first two steps, Clip Mode
/// shown.
fn two_steps() -> (Rig, ClipId) {
    let mut rig = Rig::new();
    session_view(&mut rig);
    rig.tap_pad(4, 0);
    rig.pass(40);
    rig.tap_pad(4, 1);
    rig.pass(40);
    let (id, _) = rig.clip().expect("the clip the first step made");
    rig.tap("clip");
    assert_eq!(notes_of(&rig, id).len(), 2);
    (rig, id)
}

fn value_of(rig: &mut Rig, name: &str) -> String {
    controls(rig)
        .into_iter()
        .find(|(n, _)| n == name)
        .unwrap_or_else(|| panic!("no control {name}"))
        .1
}

// Spec: push-2-tracks R55, R1. Holding a step puts Nudge, Length, Fine and
// Velocity on the encoders; the changes made while it is held are one undo
// step, and a step held rather than tapped keeps its note.
#[test]
fn holding_a_step_sets_nudge_length_fine_and_velocity_as_one_undo_step() {
    let (mut rig, id) = two_steps();
    let steps = rig.steps();
    assert!(rig.sim.press_pad(4, 0, 100));
    rig.pass(400);
    let state = clip_state(&mut rig);
    let names: Vec<_> = controls(&mut rig).into_iter().map(|c| c.0).collect();
    assert_eq!(names, ["Nudge", "Length", "Fine", "Velocity"], "{state}");
    assert_eq!(value_of(&mut rig, "Velocity"), "100");
    assert_eq!(value_of(&mut rig, "Length"), "1 step");
    // The display zooms to the page holding the step.
    assert_eq!(state["held_steps"].as_array().unwrap().len(), 1);
    assert_eq!(state["view"], json!([0.0, 8.0]));

    turn(&mut rig, 3, 10);
    turn(&mut rig, 1, 10);
    turn(&mut rig, 2, 25);
    turn(&mut rig, 0, 20);
    assert_eq!(value_of(&mut rig, "Velocity"), "110");
    assert!(rig.sim.release_pad(4, 0));
    rig.pass(40);
    assert_eq!(
        notes_of(&rig, id),
        [
            (36, 20 * STEP / 100, 2 * STEP + STEP / 4, 110),
            (36, STEP, STEP, 100)
        ],
        "a fifth of a step late, two and a quarter steps long"
    );
    assert_eq!(rig.steps(), steps + 1, "one undo step for the hold");
    // The encoders are the clip's again.
    assert_eq!(controls(&mut rig)[0].0, "Zoom");
    rig.run("edit.undo", json!({}));
    assert_eq!(notes_of(&rig, id)[0], (36, 0, STEP, 100));
}

// Spec: push-2-tracks R56. With several steps held the encoders change
// every held step's notes by the same amount and the display shows the
// lowest and highest value where they differ.
#[test]
fn two_held_steps_show_a_range_and_change_together() {
    let (mut rig, id) = two_steps();
    rig.sim.press_pad(4, 0, 100);
    rig.pass(20);
    turn(&mut rig, 3, 10);
    rig.sim.release_pad(4, 0);
    rig.pass(40);
    assert_eq!(notes_of(&rig, id)[0].3, 110);

    assert!(rig.sim.press_pad(4, 0, 100));
    assert!(rig.sim.press_pad(4, 1, 100));
    rig.pass(400);
    assert_eq!(value_of(&mut rig, "Velocity"), "100 to 110");
    assert_eq!(value_of(&mut rig, "Length"), "1 step");
    turn(&mut rig, 3, 5);
    assert!(rig.sim.release_pad(4, 0));
    assert!(rig.sim.release_pad(4, 1));
    rig.pass(40);
    let velocities: Vec<u8> = notes_of(&rig, id).iter().map(|n| n.3).collect();
    assert_eq!(velocities, [115, 105], "each by the same amount");
}

// Spec: push-2-tracks R57. Holding an empty step and turning an encoder
// adds a note there with the values set, and the note and its changes are
// one undo step.
#[test]
fn an_empty_held_step_gets_a_note_as_one_undo_step() {
    let (mut rig, id) = two_steps();
    let steps = rig.steps();
    assert!(rig.sim.press_pad(4, 5, 100));
    rig.pass(400);
    assert_eq!(value_of(&mut rig, "Velocity"), "new note");
    assert!(rig.sim.touch(3));
    assert!(rig.sim.turn(3, 10));
    assert!(rig.sim.turn(3, 10));
    rig.pass(20);
    assert!(rig.sim.release(3));
    rig.pass(20);
    assert!(rig.sim.release_pad(4, 5));
    rig.pass(40);
    let notes = notes_of(&rig, id);
    assert_eq!(notes.len(), 3);
    assert_eq!(notes[2], (36, 5 * STEP, STEP, 120), "{notes:?}");
    assert_eq!(
        rig.steps(),
        steps + 1,
        "the note and its changes are one step"
    );
}

// Spec: push-2-tracks R57. Holding Select, pressing a drum pad and turning
// the encoders changes every note of that pad in the clip, as one step.
#[test]
fn select_with_a_drum_pad_changes_all_its_notes() {
    let (mut rig, id) = two_steps();
    // Pad 37 gets a note on two steps.
    rig.tap("select");
    rig.tap_pad(0, 1);
    assert_eq!(rig.note()["selected_pad"], 37);
    rig.tap_pad(4, 3);
    rig.tap_pad(4, 6);
    rig.pass(40);
    assert_eq!(notes_of(&rig, id).iter().filter(|n| n.0 == 37).count(), 2);
    let steps = rig.steps();
    assert!(rig.sim.press("select"));
    rig.pass(50);
    rig.tap_pad(0, 1);
    assert_eq!(clip_state(&mut rig)["select_pad"], 37);
    turn(&mut rig, 3, 10);
    assert!(rig.sim.let_go("select"));
    rig.pass(40);
    let velocities: Vec<(u8, u8)> = notes_of(&rig, id).iter().map(|n| (n.0, n.3)).collect();
    assert_eq!(
        velocities,
        [(36, 100), (36, 100), (37, 110), (37, 110)],
        "every note of the pad, and no other"
    );
    assert_eq!(rig.steps(), steps + 1, "one undo step");
}

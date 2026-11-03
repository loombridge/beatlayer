//! Melodies on Push 2 in the shell (`docs/specs/push-2-notes.md`, R6, R32 to
//! R47): pad pressure into a recording, the scale options in the set and
//! `push.state`, and the arrows launching Legato, driven through the
//! simulated Push 2 on the real engine.

use bl_engine::SlotState;
use bl_model::{ClipData, SceneId, TrackId};
use serde_json::json;

use super::push_beats::{Rig, new_track};

fn scene(rig: &Rig, at: usize) -> SceneId {
    rig.shell
        .editor()
        .document()
        .scenes()
        .nth(at)
        .map(|s| s.id)
        .unwrap()
}

/// An empty one-bar MIDI clip in `track`'s slot at `scene`.
fn clip(rig: &mut Rig, track: TrackId, scene: SceneId) {
    rig.run(
        "session.insert_empty_midi_clip",
        json!({ "track": track, "scene": scene }),
    );
}

fn state_of(rig: &Rig, track: TrackId, scene: SceneId) -> Option<SlotState> {
    rig.shell
        .session_states()
        .iter()
        .find(|s| s.track == track && s.scene == scene)
        .map(|s| s.state)
}

/// How far into its own time the clip in a slot has played, in ticks.
fn clip_time(rig: &mut Rig, track: TrackId, scene: SceneId) -> Option<i64> {
    let launches = rig.run("session.state", json!({}))["launches"].clone();
    launches.as_array()?.iter().find_map(|launch| {
        (launch["track"] == json!(track) && launch["scene"] == json!(scene))
            .then(|| launch["clip_time"].as_i64())
            .flatten()
    })
}

// Spec: push-2-notes R46. Down launches the next scene's clips as Legato at
// once, though the global launch quantisation is a bar and the clips have no
// Legato of their own: each takes over the clip time of the clip before it on
// its track. With Workflow on Clip only the selected track's slot launches.
#[test]
fn down_launches_the_next_scene_at_once_and_the_clips_take_over_the_position() {
    let mut rig = Rig::new();
    let (track, other) = (rig.track, new_track(&mut rig, "Keys"));
    let (s0, s1, s2) = (scene(&rig, 0), scene(&rig, 1), scene(&rig, 2));
    for (t, s) in [
        (track, s0),
        (track, s1),
        (other, s1),
        (track, s2),
        (other, s2),
    ] {
        clip(&mut rig, t, s);
    }
    rig.run("session.launch", json!({ "track": track, "scene": s0 }));
    rig.pass(400);
    assert_eq!(state_of(&rig, track, s0), Some(SlotState::Playing));
    let before = clip_time(&mut rig, track, s0).expect("the first clip plays");
    assert!(before > 0, "{before}");

    rig.run("session.select", json!({ "scene": s0 }));
    rig.pass(50);
    rig.tap("down");
    rig.pass(100);
    assert_eq!(
        state_of(&rig, track, s1),
        Some(SlotState::Playing),
        "at once"
    );
    assert_eq!(
        state_of(&rig, other, s1),
        Some(SlotState::Playing),
        "the whole scene"
    );
    let at = clip_time(&mut rig, track, s1).expect("the next clip plays");
    assert!(
        at >= before,
        "it took over the position: {at} after {before}"
    );

    // Workflow on Clip: the selected track's slot alone.
    rig.run("push.set_setup", json!({ "workflow": "clip" }));
    rig.run("arrangement.select_tracks", json!({ "tracks": [track] }));
    rig.run("session.select", json!({ "track": track, "scene": s1 }));
    rig.pass(50);
    rig.tap("down");
    rig.pass(100);
    assert_eq!(state_of(&rig, track, s2), Some(SlotState::Playing));
    assert_ne!(
        state_of(&rig, other, s2),
        Some(SlotState::Playing),
        "the other track carries on"
    );
    assert_eq!(state_of(&rig, other, s1), Some(SlotState::Playing));
}

// Spec: push-2-notes R37. The layout options are the set's: the Scale menu
// changes them with no undo step, and `push.state` reads them back.
#[test]
fn the_scale_menu_changes_options_that_are_no_undo_step_and_the_state_reads_them() {
    let mut rig = Rig::new();
    new_track(&mut rig, "Keys");
    rig.pass(50);
    let note = rig.note();
    assert_eq!(note["layout"], "64 Notes");
    let melody = &note["melody"];
    assert_eq!(melody["in_key"], true);
    assert_eq!(melody["layout"], "4ths");
    assert_eq!(melody["direction"], "Vertical");
    assert_eq!(melody["fixed"], false);

    let steps = rig.steps();
    rig.tap("scale");
    rig.tap("lower_1");
    rig.tap("lower_8");
    assert!(rig.sim.turn(0, 10));
    rig.pass(50);
    assert!(rig.sim.turn(7, 10));
    rig.pass(50);
    rig.pass(50);
    let melody = rig.note()["melody"].clone();
    assert_eq!(melody["in_key"], false);
    assert_eq!(melody["fixed"], true);
    assert_eq!(melody["layout"], "3rds");
    assert_eq!(melody["direction"], "Horizontal");
    assert_eq!(rig.steps(), steps, "no undo step");
    let options = rig.shell.editor().document().settings().pad_layout;
    assert!(!options.in_key && options.fixed);

    // Undoing leaves them as they are.
    rig.run("edit.undo", json!({}));
    assert_eq!(rig.note()["melody"]["layout"], "3rds");
}

// Spec: push-2-notes R6. Pad pressure reaches the recording as the note's
// Pressure envelope.
#[test]
fn pad_pressure_is_recorded_as_the_notes_pressure() {
    let mut rig = Rig::new();
    let keys = new_track(&mut rig, "Keys");
    rig.run(
        "session.set_quantization",
        json!({ "quantization": "none" }),
    );
    rig.run("transport.set_recording", json!({ "recording": true }));
    rig.run("transport.play", json!({}));
    rig.pass(100);
    assert!(rig.sim.press_pad(0, 0, 100));
    rig.pass(100);
    assert!(rig.sim.pressure(0, 0, 90));
    rig.pass(100);
    assert!(rig.sim.release_pad(0, 0));
    rig.pass(100);
    rig.run("transport.stop", json!({}));
    bl_shell::testing::wait_for(&rig.control, &mut rig.shell, "the clip", |shell| {
        shell.running_recording().is_none()
            && shell
                .editor()
                .document()
                .track(keys)
                .is_some_and(|t| t.clips().count() > 0)
    });
    let doc = rig.shell.editor().document();
    let notes: Vec<_> = doc
        .track(keys)
        .unwrap()
        .clips()
        .flat_map(|clip| match &clip.data {
            ClipData::Midi(midi) => midi.notes.iter().cloned().collect(),
            ClipData::Audio(_) => Vec::<bl_model::Note>::new(),
        })
        .collect();
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].key, 36);
    assert!(
        notes[0].pressure.iter().any(|point| point.value == 90.0),
        "{:?}",
        notes[0].pressure
    );
}

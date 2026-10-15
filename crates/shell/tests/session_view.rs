//! The Session View grid's commands, driven headless through `Shell::run` as
//! agents and the grid's mouse drive them (WP-3.7; D17). Every edit is one
//! undo step, and undo and redo restore the set exactly. The behaviour is in
//! the session view spec.

use bl_model::{ClipData, SceneId, TrackId, TrackKind};
use bl_shell::Shell;
use bl_shell::session_view_commands as ids;
use serde_json::{Value, json};

fn shell() -> Shell {
    Shell::headless()
}

fn columns(shell: &Shell) -> Vec<TrackId> {
    shell.mixer_columns()
}

fn scenes(shell: &Shell) -> Vec<SceneId> {
    shell
        .editor()
        .document()
        .scenes()
        .map(|scene| scene.id)
        .collect()
}

/// A MIDI track with a column and a scene, for a slot.
fn a_midi_slot(shell: &Shell) -> (TrackId, SceneId) {
    let track = columns(shell)
        .into_iter()
        .find(|track| {
            shell
                .editor()
                .document()
                .track(*track)
                .is_some_and(|t| t.kind() == TrackKind::Midi)
        })
        .expect("the default set has a MIDI track");
    let scene = scenes(shell)[0];
    (track, scene)
}

fn steps(shell: &Shell) -> usize {
    shell.editor().undo_history().len()
}

/// Runs `id`, checks it changed the set in exactly one undo step, and that
/// undo and redo restore it exactly, then leaves it done.
fn one_step(shell: &mut Shell, id: &str, args: Value) {
    let before = shell.editor().document().clone();
    let before_steps = steps(shell);
    shell
        .run(id, args)
        .unwrap_or_else(|e| panic!("{id} failed: {e}"));
    let after = shell.editor().document().clone();
    assert_ne!(after, before, "{id} changed nothing");
    assert_eq!(steps(shell), before_steps + 1, "{id} is one undo step");
    shell.run("edit.undo", json!({})).unwrap();
    assert_eq!(
        *shell.editor().document(),
        before,
        "undoing {id} restores the set exactly"
    );
    assert!(shell.editor().document().check_invariants().is_ok());
    shell.run("edit.redo", json!({})).unwrap();
    assert_eq!(
        *shell.editor().document(),
        after,
        "redoing {id} restores it exactly"
    );
}

// Spec: session-view R1. A new set opens with eight unnamed scenes, and
// `session.state` reports what nothing selected looks like.
#[test]
fn a_new_set_has_the_default_scenes_and_no_selection() {
    let mut shell = shell();
    assert_eq!(scenes(&shell).len(), bl_model::DEFAULT_SCENE_COUNT);
    let state: ids::SessionViewState =
        serde_json::from_value(shell.run(ids::STATE, json!({})).unwrap()).unwrap();
    assert!(state.slots.is_empty() && state.scenes.is_empty() && state.tracks.is_empty());
    assert_eq!(state.scroll_y, 0.0);
}

// Spec: session-view R4, R5.
#[test]
fn clicking_selects_a_slot_a_scene_and_a_title_bar() {
    let mut shell = shell();
    let (track, scene) = a_midi_slot(&shell);
    let scene2 = scenes(&shell)[1];
    let state = |shell: &mut Shell| -> ids::SessionViewState {
        serde_json::from_value(shell.run(ids::STATE, json!({})).unwrap()).unwrap()
    };
    // A plain click replaces the selection with the slot.
    shell
        .run(ids::SELECT, json!({ "track": track, "scene": scene }))
        .unwrap();
    assert_eq!(state(&mut shell).slots.len(), 1);
    // Cmd-click adds a second slot.
    shell
        .run(
            ids::SELECT,
            json!({ "track": track, "scene": scene2, "mode": "add" }),
        )
        .unwrap();
    assert_eq!(state(&mut shell).slots.len(), 2);
    // Cmd-clicking it again takes it out.
    shell
        .run(
            ids::SELECT,
            json!({ "track": track, "scene": scene2, "mode": "add" }),
        )
        .unwrap();
    assert_eq!(state(&mut shell).slots.len(), 1);
    // Shift-click selects the range of scenes from the anchor.
    shell.run(ids::SELECT, json!({ "scene": scene })).unwrap();
    shell
        .run(
            ids::SELECT,
            json!({ "scene": scenes(&shell)[2], "mode": "range" }),
        )
        .unwrap();
    assert_eq!(state(&mut shell).scenes.len(), 3);
    // A click on a title bar selects the track.
    shell.run(ids::SELECT, json!({ "track": track })).unwrap();
    let state = state(&mut shell);
    assert_eq!(state.tracks, [track]);
    assert!(state.title_tracks);
    // Select All takes every slot.
    let state: ids::SessionViewState =
        serde_json::from_value(shell.run(ids::SELECT_ALL, json!({})).unwrap()).unwrap();
    assert_eq!(
        state.slots.len(),
        columns(&shell).len() * scenes(&shell).len()
    );
}

// Spec: session-view R9 (I-CLICK-004). Insert Empty MIDI Clip makes a
// one-bar empty MIDI clip in the slot, in one undo step.
#[test]
fn inserting_an_empty_midi_clip_is_one_undo_step() {
    let mut shell = shell();
    let (track, scene) = a_midi_slot(&shell);
    let bar = shell
        .editor()
        .document()
        .settings()
        .time_signatures
        .signature_at(bl_core::time::Ticks::ZERO)
        .bar_length();
    one_step(
        &mut shell,
        ids::INSERT_EMPTY_MIDI_CLIP,
        json!({ "track": track, "scene": scene }),
    );
    let clip = shell
        .editor()
        .document()
        .slot(track, scene)
        .expect("the slot holds a clip")
        .clone();
    assert_eq!(clip.start, bl_core::time::Ticks::ZERO);
    assert_eq!(clip.length, bar, "one bar in the set's signature");
    assert!(matches!(clip.data, ClipData::Midi(_)));
    // A second one is refused: the slot isn't empty.
    assert!(
        shell
            .run(
                ids::INSERT_EMPTY_MIDI_CLIP,
                json!({ "track": track, "scene": scene })
            )
            .is_err()
    );
}

// Spec: session-view R4, R7.
#[test]
fn inserting_and_duplicating_scenes_are_one_undo_step() {
    let mut shell = shell();
    let before = scenes(&shell).len();
    one_step(&mut shell, ids::INSERT_SCENE, json!({ "index": 0 }));
    assert_eq!(scenes(&shell).len(), before + 1);
    // A scene with a clip duplicates with its clips.
    let (track, scene) = a_midi_slot(&shell);
    shell
        .run(
            ids::INSERT_EMPTY_MIDI_CLIP,
            json!({ "track": track, "scene": scene }),
        )
        .unwrap();
    let clip = shell.editor().document().slot(track, scene).unwrap().id;
    one_step(&mut shell, ids::DUPLICATE_SCENE, json!({ "scene": scene }));
    let position = shell.editor().document().scene_position(scene).unwrap();
    let copy = scenes(&shell)[position + 1];
    let copied = shell
        .editor()
        .document()
        .slot(track, copy)
        .expect("the copy holds the clip")
        .clone();
    assert_ne!(copied.id, clip, "an independent clip");
    assert_eq!(
        copied.data,
        shell.editor().document().slot(track, scene).unwrap().data
    );
}

// Spec: session-view R10.
#[test]
fn copying_a_clip_to_another_slot_is_one_undo_step() {
    let mut shell = shell();
    let (track, scene) = a_midi_slot(&shell);
    shell
        .run(
            ids::INSERT_EMPTY_MIDI_CLIP,
            json!({ "track": track, "scene": scene }),
        )
        .unwrap();
    let clip = shell.editor().document().slot(track, scene).unwrap().id;
    let to = scenes(&shell)[1];
    one_step(
        &mut shell,
        ids::COPY_CLIP_TO_SLOT,
        json!({ "clip": clip, "track": track, "scene": to }),
    );
    let doc = shell.editor().document();
    assert!(doc.slot(track, scene).is_some(), "the source stays");
    let copy = doc.slot(track, to).expect("the target holds a copy");
    assert_ne!(copy.id, clip);
    assert_eq!(copy.start, bl_core::time::Ticks::ZERO);
}

// Spec: session-view R5, R11.
#[test]
fn deleting_the_selection_removes_slot_clips_and_scenes() {
    let mut shell = shell();
    let (track, scene) = a_midi_slot(&shell);
    shell
        .run(
            ids::INSERT_EMPTY_MIDI_CLIP,
            json!({ "track": track, "scene": scene }),
        )
        .unwrap();
    shell
        .run(ids::SELECT, json!({ "track": track, "scene": scene }))
        .unwrap();
    one_step(&mut shell, ids::DELETE, json!({}));
    assert!(shell.editor().document().slot(track, scene).is_none());
    // Deleting a scene takes its clips with it.
    shell
        .run(
            ids::INSERT_EMPTY_MIDI_CLIP,
            json!({ "track": track, "scene": scene }),
        )
        .unwrap();
    shell.run(ids::SELECT, json!({ "scene": scene })).unwrap();
    one_step(&mut shell, ids::DELETE, json!({}));
    assert!(shell.editor().document().scene(scene).is_none());
    assert!(shell.editor().document().slot(track, scene).is_none());
}

// Spec: session-view R11 (S-SES-018). Deactivating switches the selected
// clips off, and a second press switches them back on.
#[test]
fn deactivating_the_selected_clips_switches_them_off() {
    let mut shell = shell();
    let (track, scene) = a_midi_slot(&shell);
    shell
        .run(
            ids::INSERT_EMPTY_MIDI_CLIP,
            json!({ "track": track, "scene": scene }),
        )
        .unwrap();
    shell
        .run(ids::SELECT, json!({ "track": track, "scene": scene }))
        .unwrap();
    one_step(&mut shell, ids::DEACTIVATE, json!({}));
    assert!(shell.editor().document().slot(track, scene).unwrap().mute);
    one_step(&mut shell, ids::DEACTIVATE, json!({}));
    assert!(!shell.editor().document().slot(track, scene).unwrap().mute);
}

// Spec: session-view R4 (S-SES-020 to S-SES-022). Home and End select the
// first and last track of the current scene; a second End selects the
// scene.
#[test]
fn home_and_end_select_the_scene_s_edges() {
    let mut shell = shell();
    let (track, scene) = a_midi_slot(&shell);
    let columns = columns(&shell);
    let last = *columns.last().unwrap();
    let state = |shell: &mut Shell| -> ids::SessionViewState {
        serde_json::from_value(shell.run(ids::STATE, json!({})).unwrap()).unwrap()
    };
    shell
        .run(ids::SELECT, json!({ "track": track, "scene": scene }))
        .unwrap();
    shell
        .run(ids::SELECT_EDGE, json!({ "edge": "last" }))
        .unwrap();
    assert_eq!(state(&mut shell).slots.len(), 1);
    assert_eq!(state(&mut shell).slots[0].track, last);
    assert_eq!(state(&mut shell).slots[0].scene, scene);
    shell
        .run(ids::SELECT_EDGE, json!({ "edge": "first" }))
        .unwrap();
    assert_eq!(state(&mut shell).slots[0].track, columns[0]);
    // End on the last track selects the scene.
    shell
        .run(ids::SELECT, json!({ "track": last, "scene": scene }))
        .unwrap();
    shell
        .run(ids::SELECT_EDGE, json!({ "edge": "last" }))
        .unwrap();
    let state = state(&mut shell);
    assert_eq!(state.scenes, [scene]);
    assert!(state.slots.is_empty());
}

// Spec: session-view R12. Every view command runs by ID with JSON
// arguments.
#[test]
fn every_view_command_runs_by_id() {
    let mut shell = shell();
    let (track, scene) = a_midi_slot(&shell);
    for (id, args) in [
        (ids::STATE, json!({})),
        (ids::SELECT, json!({ "track": track, "scene": scene })),
        (ids::SELECT_ALL, json!({})),
        (ids::SELECT_EDGE, json!({ "edge": "first" })),
        (ids::SCROLL, json!({ "y": 18.0 })),
        (ids::FOCUS, json!({})),
    ] {
        shell.run(id, args).unwrap_or_else(|e| panic!("{id}: {e}"));
    }
    assert_eq!(shell.session().state().scroll_y, 18.0);
    assert_eq!(shell.session().focus_requests(), 1);
}

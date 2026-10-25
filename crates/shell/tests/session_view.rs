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

/// Puts a clip in each of the track's slots at the given scene numbers
/// (from 1) and selects them all.
fn select_clips(shell: &mut Shell, track: TrackId, numbers: &[usize]) {
    let scenes = scenes(shell);
    for (n, &number) in numbers.iter().enumerate() {
        let scene = scenes[number - 1];
        shell
            .run(
                ids::INSERT_EMPTY_MIDI_CLIP,
                json!({ "track": track, "scene": scene }),
            )
            .unwrap();
        let mode = if n == 0 { "replace" } else { "add" };
        shell
            .run(
                ids::SELECT,
                json!({ "track": track, "scene": scene, "mode": mode }),
            )
            .unwrap();
    }
}

fn follow(shell: &Shell, track: TrackId, number: usize) -> bl_model::FollowActions {
    let scene = scenes(shell)[number - 1];
    shell
        .editor()
        .document()
        .slot(track, scene)
        .unwrap()
        .launch
        .follow
}

// Spec: follow-actions R15 (S-SES-013). Shift+Enter switches mixed or off
// clips all on, and all on all off.
#[test]
fn shift_enter_switches_the_selected_clips_follow_actions_on_then_off() {
    let mut shell = shell();
    let (track, _) = a_midi_slot(&shell);
    select_clips(&mut shell, track, &[1, 2]);
    // One of the two is on already: mixed goes all on.
    let first = shell
        .editor()
        .document()
        .slot(track, scenes(&shell)[0])
        .unwrap()
        .id;
    shell
        .run(
            "clip.set_follow_actions",
            json!({ "clip": first, "enabled": true }),
        )
        .unwrap();
    one_step(&mut shell, ids::TOGGLE_FOLLOW_ACTIONS, json!({}));
    assert!(follow(&shell, track, 1).enabled && follow(&shell, track, 2).enabled);
    one_step(&mut shell, ids::TOGGLE_FOLLOW_ACTIONS, json!({}));
    assert!(!follow(&shell, track, 1).enabled && !follow(&shell, track, 2).enabled);
}

// Spec: follow-actions R16 (S-SES-014). A chain runs Next through the
// selection, jumps over an empty slot, and loops back to the first clip.
#[test]
fn create_follow_action_chain_wires_the_selection_into_a_loop() {
    use bl_model::FollowAction::{Jump, Next, NoAction};
    let mut shell = shell();
    let (track, _) = a_midi_slot(&shell);
    // Slots 1 to 3, an empty slot 4, then slot 5.
    select_clips(&mut shell, track, &[1, 2, 3, 5]);
    one_step(&mut shell, ids::CREATE_FOLLOW_ACTION_CHAIN, json!({}));
    let chain: Vec<_> = (1..=5)
        .filter(|&n| n != 4)
        .map(|n| follow(&shell, track, n))
        .collect();
    let actions: Vec<_> = chain.iter().map(|f| (f.action_a, f.jump_a)).collect();
    assert_eq!(actions, [(Next, 1), (Next, 1), (Jump, 5), (Jump, 1)]);
    for f in &chain {
        assert!(f.enabled && f.linked && f.multiplier == 1 && f.chance_a == 100);
        assert_eq!(f.action_b, NoAction);
    }
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

// Spec: session-context-menus R7, R8, R11. Rename types in place: the shell
// asks the view to, for the selected clip or else the selected scene, and
// the name is committed by `clip.rename` and `scene.rename`.
#[test]
fn rename_asks_the_view_to_type_the_name_of_the_selected_clip_or_scene() {
    use bl_shell::Effect;

    let mut shell = shell();
    let (track, scene) = a_midi_slot(&shell);
    // Nothing selected: refused, and the menu's item is off.
    assert!(shell.run(ids::RENAME, json!({})).is_err());
    let contexts = [bl_commands::Context::Session];
    assert!(!shell.can_invoke_in(ids::RENAME, &json!({}), &contexts));

    shell
        .run(
            ids::INSERT_EMPTY_MIDI_CLIP,
            json!({ "track": track, "scene": scene }),
        )
        .unwrap();
    let clip = shell.editor().document().slot(track, scene).unwrap().id;
    shell
        .run(ids::SELECT, json!({ "track": track, "scene": scene }))
        .unwrap();
    assert!(shell.can_invoke_in(ids::RENAME, &json!({}), &contexts));
    shell.take_effects();
    shell.run(ids::RENAME, json!({})).unwrap();
    assert!(matches!(shell.take_effects()[..], [Effect::RenameClip(c)] if c == clip));

    // With only a scene selected, the scene's name is typed.
    shell.run(ids::SELECT, json!({ "scene": scene })).unwrap();
    shell.run(ids::RENAME, json!({})).unwrap();
    assert!(matches!(shell.take_effects()[..], [Effect::RenameScene(s)] if s == scene));
    // Renaming changes nothing until the name is committed.
    assert_eq!(shell.editor().document().scene(scene).unwrap().name, "");
    shell
        .run("scene.rename", json!({ "scene": scene, "name": "Verse" }))
        .unwrap();
    assert_eq!(
        shell.editor().document().scene(scene).unwrap().name,
        "Verse"
    );
}

// Spec: session-context-menus R11. The Delete key (S-EDIT-005) and Cmd+R
// (S-EDIT-008) are the Session View's own: `session.delete` and
// `session.rename` in its context, and the items follow the selection.
#[test]
fn delete_and_rename_are_bound_in_the_session_context_and_follow_the_selection() {
    use bl_commands::Context;
    use bl_shell_spec::keymap::Keymap;

    let keymap = Keymap::live12();
    let command = |shortcut: &str| {
        keymap
            .command_for(shortcut, &[Context::Session])
            .map(|c| c.id.clone())
    };
    assert_eq!(command("S-EDIT-005").as_deref(), Some("session.delete"));
    assert_eq!(command("S-EDIT-008").as_deref(), Some("session.rename"));

    let mut shell = shell();
    let (track, scene) = a_midi_slot(&shell);
    let contexts = [Context::Session];
    // An empty slot or nothing selected has nothing to delete or rename.
    assert!(!shell.can_invoke_in(ids::DELETE, &json!({}), &contexts));
    shell
        .run(ids::SELECT, json!({ "track": track, "scene": scene }))
        .unwrap();
    assert!(!shell.can_invoke_in(ids::DELETE, &json!({}), &contexts));
    assert!(!shell.can_invoke_in(ids::DEACTIVATE, &json!({}), &contexts));
    assert!(shell.can_invoke_in(ids::INSERT_EMPTY_MIDI_CLIP, &json!({}), &contexts));
    shell.run(ids::INSERT_EMPTY_MIDI_CLIP, json!({})).unwrap();
    assert!(shell.can_invoke_in(ids::DELETE, &json!({}), &contexts));
    assert!(shell.can_invoke_in(ids::DEACTIVATE, &json!({}), &contexts));
    assert!(!shell.can_invoke_in(ids::INSERT_EMPTY_MIDI_CLIP, &json!({}), &contexts));
    // A selected scene can be deleted too.
    shell.run(ids::SELECT, json!({ "scene": scene })).unwrap();
    assert!(shell.can_invoke_in(ids::DELETE, &json!({}), &contexts));
}

// Spec: session-context-menus R16. Export MIDI Clip... without a path asks
// where to save the file; with one it writes it.
#[test]
fn export_midi_clip_without_a_path_asks_where_to_save() {
    use bl_shell::Effect;

    let mut shell = shell();
    let (track, scene) = a_midi_slot(&shell);
    shell
        .run(
            ids::INSERT_EMPTY_MIDI_CLIP,
            json!({ "track": track, "scene": scene }),
        )
        .unwrap();
    let clip = shell.editor().document().slot(track, scene).unwrap().id;
    shell
        .run("clip.rename", json!({ "clip": clip, "name": "Hook" }))
        .unwrap();
    shell.take_effects();
    shell
        .run("midi.export_clip", json!({ "clip": clip }))
        .unwrap();
    let effects = shell.take_effects();
    assert!(
        matches!(&effects[..], [Effect::PickMidiExportPath { clip: c, default_name }]
            if *c == clip && default_name == "Hook.mid"),
        "{effects:?}"
    );
    assert!(
        shell.can_invoke_in(
            "midi.export_clip",
            &json!({ "clip": clip }),
            &[bl_commands::Context::Global]
        ),
        "a MIDI clip can be exported"
    );
}

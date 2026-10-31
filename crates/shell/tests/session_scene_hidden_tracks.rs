//! Duplicate Scene and Capture and Insert Scene copy the clips of every
//! track, including those a folded group or Show Only Matches hides from the
//! Session View's columns (the session view spec, R24 and R29).

use bl_core::time::Ticks;
use bl_engine::SlotState;
use bl_model::{SceneId, TrackId};
use bl_shell::Shell;
use bl_shell::session_view_commands as ids;
use bl_shell::testing;
use serde_json::json;

fn scenes(shell: &Shell) -> Vec<SceneId> {
    shell
        .editor()
        .document()
        .scenes()
        .map(|scene| scene.id)
        .collect()
}

/// The first MIDI track moved into a new group, the second one left outside,
/// and a clip in the first track's slot of the second scene.
fn grouped_clip(shell: &mut Shell) -> (TrackId, TrackId, TrackId, SceneId) {
    shell
        .invoke("track.create", json!({ "kind": "group" }))
        .unwrap();
    let top: Vec<TrackId> = shell.editor().document().top_level().collect();
    let group = *top.last().unwrap();
    let (child, outside) = (top[0], top[1]);
    shell
        .run(
            "track.move",
            json!({ "track": child, "parent": group, "index": 0 }),
        )
        .unwrap();
    let scene = scenes(shell)[1];
    shell
        .run(
            "session.insert_empty_midi_clip",
            json!({ "track": child, "scene": scene }),
        )
        .unwrap();
    (group, child, outside, scene)
}

fn duplicated_copy_matches(shell: &mut Shell, child: TrackId, scene: SceneId) {
    let before = scenes(shell);
    shell
        .run(ids::DUPLICATE_SCENE, json!({ "scene": scene }))
        .unwrap();
    let after = scenes(shell);
    assert_eq!(after.len(), before.len() + 1);
    let copy = *after.iter().find(|id| !before.contains(id)).unwrap();
    let document = shell.editor().document();
    let source = document.slot(child, scene).unwrap();
    let copied = document
        .slot(child, copy)
        .expect("the hidden track's clip is copied");
    assert_ne!(copied.id, source.id);
    assert_eq!(copied.data, source.data);
}

#[test]
fn duplicate_scene_copies_clips_on_tracks_in_a_folded_group() {
    let mut shell = Shell::headless();
    let (group, child, _, scene) = grouped_clip(&mut shell);
    shell
        .run(
            "arrangement.fold_tracks",
            json!({ "tracks": [group], "fold": true }),
        )
        .unwrap();
    assert!(
        !shell.mixer_columns().contains(&child),
        "the group hides it"
    );
    duplicated_copy_matches(&mut shell, child, scene);
}

#[test]
fn duplicate_scene_copies_clips_on_tracks_show_only_matches_hides() {
    let mut shell = Shell::headless();
    let (_, child, outside, scene) = grouped_clip(&mut shell);
    shell
        .run(
            "track.rename",
            json!({ "track": outside, "name": "Only This One" }),
        )
        .unwrap();
    shell
        .run("search.open", json!({ "scope": "tracks" }))
        .unwrap();
    shell
        .run("search.set_text", json!({ "text": "only this" }))
        .unwrap();
    shell
        .run("search.set_only_matches", json!({ "on": true }))
        .unwrap();
    assert!(
        !shell.mixer_columns().contains(&child),
        "the filter hides it"
    );
    duplicated_copy_matches(&mut shell, child, scene);
}

#[test]
fn capture_copies_a_playing_clip_on_a_track_in_a_folded_group() {
    let (mut shell, control) = testing::rig();
    shell.start_audio();
    let (group, child, _, scene) = grouped_clip(&mut shell);
    shell
        .run(
            "arrangement.fold_tracks",
            json!({ "tracks": [group], "fold": true }),
        )
        .unwrap();
    assert!(
        !shell.mixer_columns().contains(&child),
        "the group hides it"
    );
    shell
        .run("session.launch_scene", json!({ "scene": scene }))
        .unwrap();
    testing::wait_for(&control, &mut shell, "the clip plays", |shell| {
        shell.session_states().iter().any(|slot| {
            slot.track == child && slot.scene == scene && slot.state == SlotState::Playing
        })
    });
    let before = scenes(&shell);
    shell.run(ids::CAPTURE_AND_INSERT_SCENE, json!({})).unwrap();
    let after = scenes(&shell);
    let captured = *after.iter().find(|id| !before.contains(id)).unwrap();
    assert!(
        shell
            .editor()
            .document()
            .slot(child, captured)
            .is_some_and(|clip| clip.start == Ticks::ZERO),
        "the playing clip on the hidden track is copied"
    );
}

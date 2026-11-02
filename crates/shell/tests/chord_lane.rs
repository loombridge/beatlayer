//! The chord lane's commands in the shell (WP-6.48), headless: showing the
//! lane, selecting a region, drawing and moving one as gestures, and what a
//! saved set keeps. Spec: chord-lane R1 to R5, R10.

use bl_core::time::Ticks;
use bl_model::{ChordRegionId, TrackId};
use bl_shell::arrangement_commands::ArrangementState;
use bl_shell::files::ProjectFiles;
use bl_shell::settings::Preferences;
use bl_shell::{GesturePhase, Invocation, Shell};
use serde_json::{Value, json};

const BAR: i64 = Ticks::WHOLE.get();

fn state(shell: &mut Shell) -> ArrangementState {
    serde_json::from_value(shell.run("arrangement.state", json!({})).unwrap()).unwrap()
}

fn steps(shell: &Shell) -> usize {
    shell.editor().undo_history().len()
}

fn regions(shell: &Shell) -> Vec<(i64, i64)> {
    shell
        .editor()
        .document()
        .chord_lane()
        .iter()
        .map(|r| (r.start.get(), r.end.get()))
        .collect()
}

fn add(shell: &mut Shell, start: i64, end: i64) -> ChordRegionId {
    let id: ChordRegionId = shell.new_id();
    shell
        .run(
            "chord_lane.add_region",
            json!({ "region": { "id": id, "start": start, "end": end } }),
        )
        .unwrap();
    id
}

fn gesture(shell: &mut Shell, phase: GesturePhase, id: &str, args: Value) {
    shell
        .gesture(phase, Some(&Invocation::new(id, args)))
        .unwrap();
}

#[test]
fn the_lane_is_hidden_in_a_new_set_and_view_chord_lane_shows_and_hides_it() {
    // Spec: chord-lane R1.
    let mut shell = Shell::headless();
    assert!(!state(&mut shell).chord_lane);
    let steps_before = steps(&shell);
    let shown: ArrangementState =
        serde_json::from_value(shell.run("view.chord_lane", json!({})).unwrap()).unwrap();
    assert!(shown.chord_lane, "the switch flips");
    assert!(shell.arrangement().chord_lane());
    assert!(
        !shell.is_dirty(),
        "showing the lane marks nothing as changed"
    );
    assert_eq!(steps(&shell), steps_before, "and is no undo step");
    shell.run("view.chord_lane", json!({ "on": true })).unwrap();
    assert!(state(&mut shell).chord_lane, "on stays on");
    shell
        .run("view.chord_lane", json!({ "on": false }))
        .unwrap();
    assert!(!state(&mut shell).chord_lane);
    // Hiding the lane leaves the regions and deselects.
    shell.run("view.chord_lane", json!({ "on": true })).unwrap();
    let region = add(&mut shell, BAR, 2 * BAR);
    shell
        .run("chord_lane.select_region", json!({ "region": region }))
        .unwrap();
    shell
        .run("view.chord_lane", json!({ "on": false }))
        .unwrap();
    assert_eq!(regions(&shell).len(), 1);
    assert_eq!(state(&mut shell).selected_chord_region, None);
}

#[test]
fn selecting_a_region_is_no_undo_step_and_a_missing_region_is_refused() {
    // Spec: chord-lane R3.
    let mut shell = Shell::headless();
    let region = add(&mut shell, BAR, 2 * BAR);
    let before = steps(&shell);
    shell
        .run("chord_lane.select_region", json!({ "region": region }))
        .unwrap();
    assert_eq!(state(&mut shell).selected_chord_region, Some(region));
    assert_eq!(steps(&shell), before);
    let missing: ChordRegionId = shell.new_id();
    assert!(
        shell
            .run("chord_lane.select_region", json!({ "region": missing }))
            .is_err()
    );
    assert_eq!(state(&mut shell).selected_chord_region, Some(region));
    shell.run("chord_lane.select_region", json!({})).unwrap();
    assert_eq!(state(&mut shell).selected_chord_region, None);
    // Deleting the selected region deselects it.
    shell
        .run("chord_lane.select_region", json!({ "region": region }))
        .unwrap();
    shell
        .run("chord_lane.delete_region", json!({ "region": region }))
        .unwrap();
    assert_eq!(state(&mut shell).selected_chord_region, None);
}

#[test]
fn selecting_a_region_clears_the_time_selection_so_delete_deletes_only_it() {
    // Spec: chord-lane R3. Edit > Delete shares the Delete key, so a time
    // selection left beside the region would go with it.
    let mut shell = Shell::headless();
    let region = add(&mut shell, BAR, 2 * BAR);
    shell
        .run("arrangement.select_time", json!({ "start": 0, "end": BAR }))
        .unwrap();
    assert!(state(&mut shell).time_selection.is_some());
    shell
        .run("chord_lane.select_region", json!({ "region": region }))
        .unwrap();
    let state = state(&mut shell);
    assert_eq!(state.selected_chord_region, Some(region));
    assert!(state.time_selection.is_none());
    assert!(state.selected_clips.is_empty());
}

#[test]
fn drawing_is_one_undo_step_however_far_the_drag_goes_and_selects_the_region() {
    // Spec: chord-lane R3.
    let mut shell = Shell::headless();
    let before = steps(&shell);
    for end in [2, 3, 4] {
        gesture(
            &mut shell,
            GesturePhase::Update,
            "chord_lane.add_region",
            json!({ "region": { "start": BAR, "end": end * BAR } }),
        );
    }
    assert_eq!(
        regions(&shell),
        [(BAR, 4 * BAR)],
        "each frame replaces the last"
    );
    shell.gesture(GesturePhase::Commit, None).unwrap();
    assert_eq!(steps(&shell), before + 1);
    let id = shell.editor().document().chord_lane()[0].id;
    assert_eq!(state(&mut shell).selected_chord_region, Some(id));
    assert_eq!(
        shell.editor().undo_history().next(),
        Some("Add Chord Region")
    );
    // A drag abandoned leaves nothing.
    gesture(
        &mut shell,
        GesturePhase::Update,
        "chord_lane.add_region",
        json!({ "region": { "start": 6 * BAR, "end": 7 * BAR } }),
    );
    shell.gesture(GesturePhase::Cancel, None).unwrap();
    assert_eq!(regions(&shell), [(BAR, 4 * BAR)]);
    assert_eq!(state(&mut shell).selected_chord_region, Some(id));
    // A frame that would overlap is refused and the set stays as it was.
    let refused = shell.gesture(
        GesturePhase::Update,
        Some(&Invocation::new(
            "chord_lane.add_region",
            json!({ "region": { "start": 2 * BAR, "end": 5 * BAR } }),
        )),
    );
    assert!(refused.is_err());
    assert_eq!(regions(&shell), [(BAR, 4 * BAR)]);
    shell.gesture(GesturePhase::Cancel, None).unwrap();
}

#[test]
fn moving_and_resizing_are_one_undo_step_each() {
    // Spec: chord-lane R3.
    let mut shell = Shell::headless();
    let region = add(&mut shell, 2 * BAR, 3 * BAR);
    let other = add(&mut shell, 6 * BAR, 7 * BAR);
    let before = steps(&shell);
    for start in [3, 4, 5] {
        gesture(
            &mut shell,
            GesturePhase::Update,
            "chord_lane.move_region",
            json!({ "region": region, "start": start * BAR, "end": (start + 1) * BAR }),
        );
    }
    shell.gesture(GesturePhase::Commit, None).unwrap();
    assert_eq!(regions(&shell), [(5 * BAR, 6 * BAR), (6 * BAR, 7 * BAR)]);
    assert_eq!(steps(&shell), before + 1);
    assert_eq!(
        shell.editor().undo_history().next(),
        Some("Move Chord Region")
    );
    // Resized from its end, never over the other region.
    let refused = shell.gesture(
        GesturePhase::Update,
        Some(&Invocation::new(
            "chord_lane.move_region",
            json!({ "region": region, "start": 5 * BAR, "end": 6 * BAR + 1 }),
        )),
    );
    assert!(refused.is_err(), "overlapping the other region is refused");
    shell.gesture(GesturePhase::Cancel, None).unwrap();
    assert_eq!(regions(&shell), [(5 * BAR, 6 * BAR), (6 * BAR, 7 * BAR)]);
    let _ = other;
}

fn open_again(path: &std::path::Path) -> Shell {
    let mut shell = Shell::new(
        Preferences::default(),
        None,
        bl_shell::audio::AudioHost::null(Default::default()),
        Box::new(ProjectFiles::new()),
    );
    shell
        .run("set.open", json!({ "path": path.to_string_lossy() }))
        .unwrap();
    shell
}

#[test]
fn a_saved_set_keeps_the_lane_its_regions_and_each_track_s_switch() {
    // Spec: chord-lane R1, R10.
    let mut shell = Shell::headless();
    let track: TrackId = shell.new_id();
    shell
        .run(
            "track.create",
            json!({ "id": track, "kind": "midi", "name": "Keys", "parent": null, "index": 0 }),
        )
        .unwrap();
    let region = add(&mut shell, BAR, 3 * BAR);
    shell
        .run(
            "chord_lane.set_region",
            json!({ "region": region, "enabled": true, "root": 2, "name": "dorian",
                    "transpose": 2 }),
        )
        .unwrap();
    shell
        .run(
            "track.set_follow_chord_lane",
            json!({ "track": track, "follow": true }),
        )
        .unwrap();
    shell.run("view.chord_lane", json!({ "on": true })).unwrap();
    let dir = std::env::temp_dir().join(format!("bl-chord-lane-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let result = shell
        .run(
            "set.save_as",
            json!({ "path": dir.join("Lane.blset").to_string_lossy() }),
        )
        .unwrap();
    let path = std::path::PathBuf::from(result["path"].as_str().unwrap());
    let mut again = open_again(&path);
    assert!(again.arrangement().chord_lane(), "the lane shows as it did");
    let doc = again.editor().document();
    assert_eq!(doc.chord_lane().len(), 1);
    assert_eq!(doc.chord_lane()[0].name(), "D Dorian +2");
    assert!(doc.track(track).unwrap().follow_chord_lane());
    assert_eq!(steps(&again), 0, "opening a set leaves no undo steps");
    // A new set has no lane showing and no regions.
    again.run("set.new", json!({})).unwrap();
    assert!(!again.arrangement().chord_lane());
    assert!(again.editor().document().chord_lane().is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_duplicated_track_follows_the_lane_as_its_source_did() {
    // Spec: chord-lane R5.
    let mut shell = Shell::headless();
    let track: TrackId = shell.new_id();
    shell
        .run(
            "track.create",
            json!({ "id": track, "kind": "midi", "name": "Keys", "parent": null, "index": 0 }),
        )
        .unwrap();
    shell
        .run(
            "track.set_follow_chord_lane",
            json!({ "track": track, "follow": true }),
        )
        .unwrap();
    shell
        .run("track.duplicate", json!({ "tracks": [track] }))
        .unwrap();
    let keys: Vec<_> = shell
        .editor()
        .document()
        .tracks()
        .filter(|t| t.name().starts_with("Keys"))
        .collect();
    assert_eq!(keys.len(), 2, "the track and its copy");
    assert!(keys.iter().all(|t| t.follow_chord_lane()));
}

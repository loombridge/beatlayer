//! A scene's tempo and time signature, the Scene View's fields, and Capture
//! and Insert Scene (WP-3.5), driven headless through `Shell::run` as the
//! Main column, the Create menu, and agents drive them (D17). The behaviour
//! is in the session view spec, R20 to R25.

use bl_audio_io::NullControl;
use bl_core::time::{Bpm, TimeSignature};
use bl_engine::SlotState;
use bl_model::{SceneId, TrackId, TrackKind};
use bl_shell::Shell;
use bl_shell::mixer_view_commands as mixer_ids;
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

fn a_midi_track(shell: &Shell) -> TrackId {
    shell
        .mixer_columns()
        .into_iter()
        .find(|track| {
            shell
                .editor()
                .document()
                .track(*track)
                .is_some_and(|t| t.kind() == TrackKind::Midi)
        })
        .expect("the default set has a MIDI track")
}

fn steps(shell: &Shell) -> usize {
    shell.editor().undo_history().len()
}

/// Waits until the Session slots the engine publishes satisfy `done`. The
/// shell's null device is paced by hand (T199), so this lands on the same
/// frames however busy the machine is.
fn wait_for(
    control: &NullControl,
    shell: &mut Shell,
    what: &str,
    done: impl Fn(&[bl_engine::SlotStatus]) -> bool,
) {
    testing::wait_for(control, shell, what, |shell| done(shell.session_states()));
}

// Spec: session-view R20, R21. Each setting is one undo step and reads back.
#[test]
fn a_scenes_tempo_and_signature_are_set_read_back_and_switched_off() {
    let mut shell = Shell::headless();
    let scene = scenes(&shell)[0];
    let before = steps(&shell);
    shell
        .run("scene.set_tempo", json!({ "scene": scene, "tempo": 90.0 }))
        .unwrap();
    shell
        .run(
            "scene.set_time_signature",
            json!({ "scene": scene, "signature": { "numerator": 7, "denominator": 8 } }),
        )
        .unwrap();
    assert_eq!(steps(&shell), before + 2);
    let own = shell.editor().document().scene(scene).unwrap().clone();
    assert_eq!(own.tempo, Some(Bpm::new(90.0).unwrap()));
    assert_eq!(own.signature, Some(TimeSignature::new(7, 8).unwrap()));
    // Double-clicking a field, or Return to Default, switches it off.
    shell
        .run("scene.set_tempo", json!({ "scene": scene, "tempo": null }))
        .unwrap();
    assert_eq!(shell.editor().document().scene(scene).unwrap().tempo, None);
    shell.run("edit.undo", json!({})).unwrap();
    assert_eq!(
        shell.editor().document().scene(scene).unwrap().tempo,
        Some(Bpm::new(90.0).unwrap())
    );
    // A tempo outside 20 to 999 BPM is refused.
    for bad in [19.0, 1000.0] {
        let error = shell
            .run("scene.set_tempo", json!({ "scene": scene, "tempo": bad }))
            .unwrap_err();
        assert!(
            matches!(error, bl_shell::ShellError::Failed { .. }),
            "{error:?}"
        );
    }
}

// Spec: session-view R24. Nothing plays: an empty scene, after the selected
// scene, holding the tempo and signature in force.
#[test]
fn capture_with_nothing_playing_inserts_an_empty_scene_after_the_selected_one() {
    let mut shell = Shell::headless();
    let first = scenes(&shell);
    shell
        .run("session.select", json!({ "scene": first[2] }))
        .unwrap();
    let before = steps(&shell);
    shell.run(ids::CAPTURE_AND_INSERT_SCENE, json!({})).unwrap();
    assert_eq!(steps(&shell), before + 1, "one undo step");
    let after = scenes(&shell);
    assert_eq!(after.len(), first.len() + 1);
    let new = after[3];
    assert!(!first.contains(&new), "the scene is new, after the third");
    let scene = shell.editor().document().scene(new).unwrap();
    assert_eq!(scene.tempo, Some(Bpm::new(120.0).unwrap()));
    assert_eq!(scene.signature, Some(TimeSignature::FOUR_FOUR));
    assert!(
        shell
            .editor()
            .document()
            .tracks()
            .all(|track| track.slot(new).is_none()),
        "nothing playing, nothing copied"
    );
    shell.run("edit.undo", json!({})).unwrap();
    assert_eq!(scenes(&shell), first);
}

// Spec: session-view R24, R25. The playing clips are copied into a scene
// after the one launched last, and play on in the copies.
#[test]
fn capture_copies_the_playing_clips_and_the_clips_play_on_in_the_copies() {
    let (mut shell, control) = testing::rig();
    shell.start_audio();
    assert!(shell.audio().engine_running());
    let track = a_midi_track(&shell);
    let launched = scenes(&shell)[1];
    shell
        .run(
            "session.insert_empty_midi_clip",
            json!({ "track": track, "scene": launched }),
        )
        .unwrap();
    shell
        .run(
            "scene.set_tempo",
            json!({ "scene": launched, "tempo": 100.0 }),
        )
        .unwrap();
    shell
        .run("session.launch_scene", json!({ "scene": launched }))
        .unwrap();
    wait_for(&control, &mut shell, "the scene plays", |slots| {
        slots
            .iter()
            .any(|slot| slot.scene == launched && slot.state == SlotState::Playing)
    });
    testing::wait_for(&control, &mut shell, "the scene's tempo to apply", |shell| {
        shell.scene_override().tempo.is_some()
    });
    assert_eq!(shell.current_tempo(), Bpm::new(100.0).unwrap());
    // A selection elsewhere doesn't matter once a scene has launched.
    shell
        .run("session.select", json!({ "scene": scenes(&shell)[5] }))
        .unwrap();
    let before = steps(&shell);
    let scenes_before = scenes(&shell);
    shell.run(ids::CAPTURE_AND_INSERT_SCENE, json!({})).unwrap();
    assert_eq!(steps(&shell), before + 1, "one undo step");
    let after = scenes(&shell);
    let new = after[2];
    assert!(
        !scenes_before.contains(&new),
        "directly after the launched scene"
    );
    let document = shell.editor().document();
    let original = document.slot(track, launched).unwrap();
    let copy = document
        .slot(track, new)
        .expect("the playing clip is copied");
    assert_ne!(copy.id, original.id);
    assert_eq!(copy.data, original.data);
    assert_eq!(
        document.scene(new).unwrap().tempo,
        Some(shell.current_tempo()),
        "the scene holds the tempo in force"
    );
    wait_for(&control, &mut shell, "the copy plays on", |slots| {
        slots.len() == 1 && slots[0].scene == new && slots[0].state == SlotState::Playing
    });
}

// Spec: session-view R21. The View menu's item shows the fields.
#[test]
fn the_scene_tempo_fields_are_shown_and_hidden_by_a_view_command() {
    let mut shell = Shell::headless();
    assert!(!shell.mixer_view().scene_tempo(), "hidden in a new set");
    shell.run(mixer_ids::SCENE_TEMPO, json!({})).unwrap();
    assert!(shell.mixer_view().scene_tempo());
    shell
        .run(mixer_ids::SCENE_TEMPO, json!({ "shown": false }))
        .unwrap();
    assert!(!shell.mixer_view().scene_tempo());
}

// Spec: session-view R21. The fields take values as the Control Bar's do.
#[test]
fn the_scene_fields_show_the_sets_values_until_the_scene_has_its_own() {
    use bl_shell::scene_fields::{SceneField, off, step, typed, view};
    let mut shell = Shell::headless();
    let scene = scenes(&shell)[0];
    let run = |shell: &mut Shell, change: Option<bl_shell::Invocation>| {
        let change = change.expect("the entry changes the scene");
        shell.run(&change.id, change.args).unwrap();
    };

    // Grey, showing the set's tempo and signature.
    let tempo = view(&shell, scene, SceneField::Tempo);
    assert_eq!((tempo.text.as_str(), tempo.own), ("120.00", false));
    let numerator = view(&shell, scene, SceneField::Numerator);
    assert_eq!((numerator.text.as_str(), numerator.own), ("4", false));
    assert!(off(&shell, scene, SceneField::Tempo).is_none());

    // A comma is a decimal point; a tempo comes within 20 to 999 BPM.
    let change = typed(&shell, scene, SceneField::Tempo, "95,5");
    run(&mut shell, change);
    let tempo = view(&shell, scene, SceneField::Tempo);
    assert_eq!((tempo.text.as_str(), tempo.own), ("95.50", true));
    let change = typed(&shell, scene, SceneField::Tempo, "5000");
    run(&mut shell, change);
    assert_eq!(view(&shell, scene, SceneField::Tempo).text, "999.00");
    let change = typed(&shell, scene, SceneField::Tempo, "3");
    run(&mut shell, change);
    assert_eq!(view(&shell, scene, SceneField::Tempo).text, "20.00");
    // Text that isn't a number leaves the field as it was.
    assert!(typed(&shell, scene, SceneField::Tempo, "fast").is_none());
    // A step is a whole BPM, or a hundredth when fine.
    let change = step(&shell, scene, SceneField::Tempo, 1, false);
    run(&mut shell, change);
    assert_eq!(view(&shell, scene, SceneField::Tempo).text, "21.00");
    let change = step(&shell, scene, SceneField::Tempo, -1, true);
    run(&mut shell, change);
    assert_eq!(view(&shell, scene, SceneField::Tempo).text, "20.99");

    // The signature's parts: a denominator is the largest note value that
    // doesn't pass what was typed, and a numerator comes within 1 to 99.
    let change = typed(&shell, scene, SceneField::Numerator, "7");
    run(&mut shell, change);
    let change = typed(&shell, scene, SceneField::Denominator, "9");
    run(&mut shell, change);
    let own = shell.editor().document().scene(scene).unwrap().signature;
    assert_eq!(own, Some(TimeSignature::new(7, 8).unwrap()));
    let change = typed(&shell, scene, SceneField::Numerator, "120");
    run(&mut shell, change);
    assert_eq!(view(&shell, scene, SceneField::Numerator).text, "99");
    let change = step(&shell, scene, SceneField::Denominator, 1, false);
    run(&mut shell, change);
    assert_eq!(view(&shell, scene, SceneField::Denominator).text, "16");

    // Either part's double-click turns the whole signature off; the tempo's
    // turns the tempo off.
    let change = off(&shell, scene, SceneField::Denominator);
    run(&mut shell, change);
    let scene_now = shell.editor().document().scene(scene).unwrap().clone();
    assert_eq!(scene_now.signature, None);
    assert!(scene_now.tempo.is_some());
    let change = off(&shell, scene, SceneField::Tempo);
    run(&mut shell, change);
    assert_eq!(shell.editor().document().scene(scene).unwrap().tempo, None);
}

// Spec: session-view R23. A launched scene's tempo holds until Re-Enable
// Automation gives the set's own back.
#[test]
fn re_enable_automation_gives_back_the_sets_tempo() {
    let (mut shell, control) = testing::rig();
    shell.start_audio();
    assert!(shell.audio().engine_running());
    let scene = scenes(&shell)[0];
    shell
        .run("scene.set_tempo", json!({ "scene": scene, "tempo": 80.0 }))
        .unwrap();
    shell
        .run("session.launch_scene", json!({ "scene": scene }))
        .unwrap();
    testing::wait_for(&control, &mut shell, "the scene's tempo to apply", |shell| {
        shell.scene_override().tempo.is_some()
    });
    assert_eq!(shell.current_tempo(), Bpm::new(80.0).unwrap());
    shell.run("automation.re_enable", json!({})).unwrap();
    testing::wait_for(&control, &mut shell, "the override to clear", |shell| {
        shell.scene_override().is_none()
    });
    assert_eq!(shell.current_tempo(), Bpm::new(120.0).unwrap());
}

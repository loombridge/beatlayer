//! The global launch quantisation and the Session View's Cmd+Enter stop
//! (WP-3.2), driven headless through `Shell::run` as the Control Bar's
//! menu, the Options items, and the keys drive them (D17). The quantisation
//! is saved with the set and changing it is no undo step. The behaviour is
//! in the launch quantisation spec.

use std::path::{Path, PathBuf};

use bl_audio_io::NullControl;
use bl_model::{LaunchMode, LaunchQuantization, LaunchSettings, TrackKind};
use bl_shell::audio::AudioHost;
use bl_shell::files::ProjectFiles;
use bl_shell::session_launch_commands as ids;
use bl_shell::settings::{LaunchPrefs, Preferences};
use bl_shell::testing;
use bl_shell::{Shell, ShellError};
use serde_json::json;

/// A folder for one test's files, removed when the test ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "bl-session-launch-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

// Spec: launch-quantisation R1, R3, R13.
#[test]
fn the_quantisation_commands_change_the_value_and_the_switch() {
    let mut shell = Shell::headless();
    assert_eq!(shell.launch_settings(), LaunchSettings::default());

    // S-GQ-001: a sixteenth. The result says what the setting is now.
    let result = shell
        .run(
            ids::SET_QUANTIZATION,
            json!({ "quantization": "sixteenth" }),
        )
        .unwrap();
    assert_eq!(result["quantization"], "sixteenth");
    assert_eq!(
        shell.launch_settings().quantization,
        LaunchQuantization::Sixteenth
    );

    // S-SES-024 to S-SES-026: smaller, larger, triplet.
    shell.run(ids::SMALLER_QUANTIZATION, json!({})).unwrap();
    assert_eq!(
        shell.launch_settings().quantization,
        LaunchQuantization::SixteenthTriplet
    );
    shell.run(ids::LARGER_QUANTIZATION, json!({})).unwrap();
    assert_eq!(
        shell.launch_settings().quantization,
        LaunchQuantization::Sixteenth
    );
    shell.run(ids::TRIPLET_QUANTIZATION, json!({})).unwrap();
    assert_eq!(
        shell.launch_settings().quantization,
        LaunchQuantization::SixteenthTriplet
    );
    shell.run(ids::TRIPLET_QUANTIZATION, json!({})).unwrap();
    assert_eq!(
        shell.launch_settings().quantization,
        LaunchQuantization::Sixteenth
    );

    // S-GQ-005: off, the menu's None.
    shell
        .run(ids::SET_QUANTIZATION, json!({ "quantization": "none" }))
        .unwrap();
    assert_eq!(
        shell.launch_settings().quantization,
        LaunchQuantization::None
    );

    // S-SES-027 toggles the switch, with `enabled` given or on its own.
    shell
        .run(ids::SET_QUANTIZATION_ENABLED, json!({ "enabled": false }))
        .unwrap();
    assert!(!shell.launch_settings().enabled);
    shell.run(ids::SET_QUANTIZATION_ENABLED, json!({})).unwrap();
    assert!(shell.launch_settings().enabled);

    // The engine is told, and the next engine starts with it.
    assert_eq!(shell.audio().launch_settings(), shell.launch_settings());

    // Changing them is no undo step.
    assert_eq!(shell.editor().undo_history().count(), 0);
}

// Spec: launch-quantisation R1.
#[test]
fn the_quantisation_saves_with_the_set_and_opens_with_it() {
    let scratch = Scratch::new("save");
    let mut shell = Shell::new(
        Preferences::default(),
        None,
        AudioHost::null(Default::default()),
        Box::new(ProjectFiles::new()),
    );
    shell
        .run(
            ids::SET_QUANTIZATION,
            json!({ "quantization": "half_triplet" }),
        )
        .unwrap();
    shell
        .run(ids::SET_QUANTIZATION_ENABLED, json!({ "enabled": false }))
        .unwrap();
    let saved = PathBuf::from(
        shell
            .run(
                "set.save_as",
                json!({ "path": text(&scratch.path("Quantised.blset")) }),
            )
            .unwrap()["path"]
            .as_str()
            .unwrap(),
    );

    // A new set starts at the default; opening the saved one brings the
    // quantisation back, and the engine is told it.
    shell.run("set.new", json!({})).unwrap();
    assert_eq!(shell.launch_settings(), LaunchSettings::default());
    shell
        .run("set.open", json!({ "path": text(&saved) }))
        .unwrap();
    let wanted = LaunchSettings {
        quantization: LaunchQuantization::HalfTriplet,
        enabled: false,
        ..LaunchSettings::default()
    };
    assert_eq!(shell.launch_settings(), wanted);
    assert_eq!(shell.audio().launch_settings(), wanted);
}

// Spec: follow-actions R8. The global switch is on in a new set, toggles
// when no value is given, reaches the engine, saves with the set and is no
// undo step.
#[test]
fn the_global_follow_action_switch_saves_with_the_set_and_the_engine_hears_it() {
    let scratch = Scratch::new("follow-switch");
    let mut shell = Shell::new(
        Preferences::default(),
        None,
        AudioHost::null(Default::default()),
        Box::new(ProjectFiles::new()),
    );
    assert!(shell.launch_settings().follow_actions);
    shell
        .run(ids::SET_FOLLOW_ACTIONS_ENABLED, json!({}))
        .unwrap();
    assert!(!shell.launch_settings().follow_actions);
    assert_eq!(shell.audio().launch_settings(), shell.launch_settings());
    let saved = PathBuf::from(
        shell
            .run(
                "set.save_as",
                json!({ "path": text(&scratch.path("Switch.blset")) }),
            )
            .unwrap()["path"]
            .as_str()
            .unwrap(),
    );
    shell.run("set.new", json!({})).unwrap();
    assert!(shell.launch_settings().follow_actions);
    shell
        .run("set.open", json!({ "path": text(&saved) }))
        .unwrap();
    assert!(!shell.launch_settings().follow_actions);
    shell
        .run(ids::SET_FOLLOW_ACTIONS_ENABLED, json!({ "enabled": true }))
        .unwrap();
    assert!(shell.launch_settings().follow_actions);
    assert_eq!(shell.editor().undo_history().count(), 0);
}

// Spec: launch-quantisation R6.
#[test]
fn stop_selected_stops_the_tracks_with_a_selected_slot() {
    let mut shell = Shell::headless();
    shell.start_audio();
    assert!(shell.audio().engine_running());

    // With no slot selected the command can't run.
    let error = shell.run(ids::STOP_SELECTED, json!({})).unwrap_err();
    assert!(matches!(error, ShellError::Unavailable { .. }), "{error:?}");

    // A MIDI track's slot, selected: it runs and the engine hears a stop per
    // track (the frames are the engine's, and the engine tests').
    let track = shell
        .mixer_columns()
        .into_iter()
        .find(|track| {
            shell
                .editor()
                .document()
                .track(*track)
                .is_some_and(|track| track.kind() == TrackKind::Midi)
        })
        .expect("the default set has a MIDI track");
    let scene = shell
        .editor()
        .document()
        .scenes()
        .next()
        .expect("a new set has scenes")
        .id;
    shell
        .run("session.select", json!({ "track": track, "scene": scene }))
        .unwrap();
    let result = shell.run(ids::STOP_SELECTED, json!({}));
    assert!(result.is_ok(), "{result:?}");
}

/// A started headless shell with a MIDI clip in the first slot of the
/// first MIDI track, and the ids of that track and the first two scenes.
fn shell_with_a_slot_clip() -> (
    Shell,
    NullControl,
    bl_model::TrackId,
    bl_model::SceneId,
    bl_model::SceneId,
) {
    let (mut shell, control) = testing::rig();
    shell.start_audio();
    assert!(shell.audio().engine_running());
    let track = shell
        .mixer_columns()
        .into_iter()
        .find(|track| {
            shell
                .editor()
                .document()
                .track(*track)
                .is_some_and(|track| track.kind() == TrackKind::Midi)
        })
        .expect("the default set has a MIDI track");
    let scenes: Vec<_> = shell.editor().document().scenes().map(|s| s.id).collect();
    shell
        .run(
            "session.insert_empty_midi_clip",
            json!({ "track": track, "scene": scenes[0] }),
        )
        .unwrap();
    (shell, control, track, scenes[0], scenes[1])
}

// Spec: launch-quantisation R16.
#[test]
fn a_launched_clip_selects_itself_and_a_launched_scene_selects_the_next_one() {
    let (mut shell, _control, track, first, second) = shell_with_a_slot_clip();
    let selected = |shell: &mut Shell| shell.run("session.state", json!({})).unwrap();

    // Select on Launch: the launched slot is selected.
    shell
        .run("session.launch", json!({ "track": track, "scene": first }))
        .unwrap();
    let state = selected(&mut shell);
    assert_eq!(state["slots"].as_array().unwrap().len(), 1, "{state}");
    assert_eq!(state["slots"][0]["scene"], json!(first));

    // Select Next Scene on Launch: the scene after the launched one.
    shell
        .run("session.launch_scene", json!({ "scene": first }))
        .unwrap();
    let state = selected(&mut shell);
    assert_eq!(state["scenes"], json!([second]), "{state}");

    // Off, a launch leaves the selection as it is.
    shell
        .run("settings.set_select_on_launch", json!({ "on": false }))
        .unwrap();
    shell
        .run(
            "settings.set_select_next_scene_on_launch",
            json!({ "on": false }),
        )
        .unwrap();
    shell
        .run("session.select", json!({ "scene": first }))
        .unwrap();
    let before = selected(&mut shell);
    shell
        .run("session.launch", json!({ "track": track, "scene": first }))
        .unwrap();
    shell
        .run("session.launch_scene", json!({ "scene": first }))
        .unwrap();
    assert_eq!(selected(&mut shell), before);
}

// Spec: launch-quantisation R16.
#[test]
fn launching_an_empty_slot_or_the_last_scene_selects_nothing_new() {
    let (mut shell, _control, track, first, _) = shell_with_a_slot_clip();
    let scenes: Vec<_> = shell.editor().document().scenes().map(|s| s.id).collect();
    let last = *scenes.last().unwrap();
    shell
        .run("session.select", json!({ "scene": first }))
        .unwrap();
    let before = shell.run("session.state", json!({})).unwrap();

    shell
        .run("session.launch", json!({ "track": track, "scene": last }))
        .unwrap();
    shell
        .run("session.launch_scene", json!({ "scene": last }))
        .unwrap();
    assert_eq!(shell.run("session.state", json!({})).unwrap(), before);
}

// Spec: launch-quantisation R15.
#[test]
fn the_launch_defaults_start_as_observed_keep_their_values_and_seed_new_clips() {
    use bl_shell::settings::LaunchPrefs;

    let mut shell = Shell::headless();
    assert_eq!(shell.launch_prefs(), LaunchPrefs::default());
    assert_eq!(
        shell.launch_prefs().default_mode,
        bl_model::LaunchMode::Trigger
    );
    assert_eq!(shell.launch_prefs().default_quantization, None);
    assert!(shell.launch_prefs().select_on_launch);
    assert!(shell.launch_prefs().select_next_scene_on_launch);
    assert!(!shell.launch_prefs().start_recording_on_scene_launch);

    // Each is a preference: the next start reads what was chosen.
    shell
        .run(
            "settings.set_default_launch_quantization",
            json!({ "quantization": "quarter" }),
        )
        .unwrap();
    shell
        .run(
            "settings.set_default_launch_mode",
            json!({ "mode": "toggle" }),
        )
        .unwrap();
    shell
        .run(
            "settings.set_start_recording_on_scene_launch",
            json!({ "on": true }),
        )
        .unwrap();
    let chosen = shell.launch_prefs();
    assert_eq!(chosen.default_mode, bl_model::LaunchMode::Toggle);
    assert_eq!(
        chosen.default_quantization,
        Some(bl_model::LaunchQuantization::Quarter)
    );
    assert!(chosen.start_recording_on_scene_launch);
    let saved = serde_json::to_string(&Preferences {
        launch: shell.launch_prefs(),
        ..Preferences::default()
    })
    .unwrap();
    let again: Preferences = serde_json::from_str(&saved).unwrap();
    assert_eq!(again.launch, shell.launch_prefs());
    let older: Preferences = serde_json::from_str("{}").unwrap();
    assert_eq!(older.launch, LaunchPrefs::default(), "an older file");
    assert_eq!(shell.editor().undo_history().count(), 0, "no undo step");

    // A new clip in a slot starts with the default quantisation.
    let track = shell
        .mixer_columns()
        .into_iter()
        .find(|track| {
            shell
                .editor()
                .document()
                .track(*track)
                .is_some_and(|track| track.kind() == TrackKind::Midi)
        })
        .unwrap();
    let scene = shell.editor().document().scenes().next().unwrap().id;
    shell
        .run(
            "session.insert_empty_midi_clip",
            json!({ "track": track, "scene": scene }),
        )
        .unwrap();
    let clip = shell.editor().document().slot(track, scene).unwrap();
    assert_eq!(
        clip.launch.quantization,
        Some(bl_model::LaunchQuantization::Quarter)
    );
}

// Spec: launch-quantisation R22.
#[test]
fn a_new_clip_takes_the_default_launch_mode_and_the_mode_is_one_saved_undo_step() {
    let scratch = Scratch::new("mode");
    let mut shell = Shell::new(
        Preferences {
            launch: LaunchPrefs {
                default_mode: LaunchMode::Gate,
                ..LaunchPrefs::default()
            },
            ..Preferences::default()
        },
        None,
        AudioHost::null(Default::default()),
        Box::new(ProjectFiles::new()),
    );
    let track = shell
        .mixer_columns()
        .into_iter()
        .find(|track| {
            shell
                .editor()
                .document()
                .track(*track)
                .is_some_and(|track| track.kind() == TrackKind::Midi)
        })
        .expect("the default set has a MIDI track");
    let scene = shell
        .editor()
        .document()
        .scenes()
        .next()
        .expect("a new set has scenes")
        .id;
    shell
        .run(
            "session.insert_empty_midi_clip",
            json!({ "track": track, "scene": scene }),
        )
        .unwrap();
    let clip = |shell: &Shell| {
        shell
            .editor()
            .document()
            .slot(track, scene)
            .expect("the slot holds a clip")
            .clone()
    };
    assert_eq!(clip(&shell).launch.mode, LaunchMode::Gate);

    // Changing the mode is one undo step, by command ID.
    let steps = shell.editor().undo_history().len();
    shell
        .run(
            "clip.set_launch_mode",
            json!({ "clip": clip(&shell).id, "mode": "toggle" }),
        )
        .unwrap();
    assert_eq!(shell.editor().undo_history().len(), steps + 1);
    assert_eq!(clip(&shell).launch.mode, LaunchMode::Toggle);
    shell.run("edit.undo", json!({})).unwrap();
    assert_eq!(clip(&shell).launch.mode, LaunchMode::Gate);
    shell.run("edit.redo", json!({})).unwrap();

    // It saves with the set and opens with it.
    let saved = PathBuf::from(
        shell
            .run(
                "set.save_as",
                json!({ "path": text(&scratch.path("Modes.blset")) }),
            )
            .unwrap()["path"]
            .as_str()
            .unwrap(),
    );
    shell.run("set.new", json!({})).unwrap();
    shell
        .run("set.open", json!({ "path": text(&saved) }))
        .unwrap();
    assert_eq!(clip(&shell).launch.mode, LaunchMode::Toggle);
}

/// Advances the set until the slots the engine publishes satisfy `done`.
/// The null device is paced by hand (T199), so this lands on the same frames
/// however busy the machine is.
fn wait_for_slots(
    control: &NullControl,
    shell: &mut Shell,
    what: &str,
    done: impl Fn(&[bl_engine::SlotStatus]) -> bool,
) {
    testing::wait_for(control, shell, what, |shell| done(shell.session_states()));
}

// Spec: session-view R32 (S-SES-001). Enter's command launches the selected
// slots that hold a clip and the selected scenes, and a scripted agent
// launches and stops by the same command IDs.
#[test]
fn launch_selected_launches_the_selection_and_agents_launch_and_stop_by_id() {
    let (mut shell, control, track, first, second) = shell_with_a_slot_clip();
    shell
        .run(ids::SET_QUANTIZATION, json!({ "quantization": "none" }))
        .unwrap();

    // Nothing selected, or only an empty slot: nothing to launch.
    let error = shell.run(ids::LAUNCH_SELECTED, json!({})).unwrap_err();
    assert!(matches!(error, ShellError::Unavailable { .. }), "{error:?}");
    shell
        .run("session.select", json!({ "track": track, "scene": second }))
        .unwrap();
    let error = shell.run(ids::LAUNCH_SELECTED, json!({})).unwrap_err();
    assert!(matches!(error, ShellError::Unavailable { .. }), "{error:?}");
    assert!(shell.session_states().is_empty());

    // The slot with a clip launches.
    shell
        .run("session.select", json!({ "track": track, "scene": first }))
        .unwrap();
    shell.run(ids::LAUNCH_SELECTED, json!({})).unwrap();
    wait_for_slots(&control, &mut shell, "the selected clip", |slots| {
        slots.len() == 1 && slots[0].track == track && slots[0].scene == first
    });

    // An agent stops the track by ID, and launches the clip again.
    shell
        .run("session.stop_track", json!({ "track": track }))
        .unwrap();
    wait_for_slots(&control, &mut shell, "the stop", <[_]>::is_empty);
    shell
        .run("session.launch", json!({ "track": track, "scene": first }))
        .unwrap();
    wait_for_slots(&control, &mut shell, "the launch", |slots| slots.len() == 1);

    // A selected scene launches its clips.
    shell.run("session.stop_all", json!({})).unwrap();
    wait_for_slots(&control, &mut shell, "stop all", <[_]>::is_empty);
    shell
        .run("session.select", json!({ "scene": first }))
        .unwrap();
    shell.run(ids::LAUNCH_SELECTED, json!({})).unwrap();
    wait_for_slots(&control, &mut shell, "the scene", |slots| slots.len() == 1);
}

// Spec: remote-mapping R37. A scripted agent launches a clip with a velocity
// and a transposition by `session.launch`'s ID, and the slot the engine
// publishes carries them; values outside the ranges are refused.
#[test]
fn an_agent_launches_with_a_velocity_and_a_transposition_by_id() {
    let (mut shell, control, track, first, _) = shell_with_a_slot_clip();
    shell
        .run(ids::SET_QUANTIZATION, json!({ "quantization": "none" }))
        .unwrap();
    for args in [
        json!({ "track": track, "scene": first, "velocity": 0 }),
        json!({ "track": track, "scene": first, "transpose": 128 }),
        json!({ "track": track, "scene": first, "transpose": -128 }),
    ] {
        assert!(shell.run("session.launch", args).is_err());
    }
    shell
        .run(
            "session.launch",
            json!({ "track": track, "scene": first, "velocity": 64, "transpose": -5 }),
        )
        .unwrap();
    wait_for_slots(&control, &mut shell, "the transposed launch", |slots| {
        slots.len() == 1 && (slots[0].velocity, slots[0].transpose) == (64, -5)
    });
    // A plain launch is untransposed again.
    shell
        .run("session.launch", json!({ "track": track, "scene": first }))
        .unwrap();
    wait_for_slots(&control, &mut shell, "the plain launch", |slots| {
        slots.len() == 1 && (slots[0].velocity, slots[0].transpose) == (127, 0)
    });
}

// Spec: session-context-menus R10. Cancel Scene Launch drops what a scene's
// launch queued and hasn't played, leaves the clips that play, and is refused
// with nothing queued; `session.state` reads the slots' states back.
#[test]
fn cancel_scene_launch_drops_a_queued_launch_and_leaves_what_plays() {
    use bl_shell::session_view_commands::{LaunchState, SessionViewState};

    let (mut shell, control, track, first, second) = shell_with_a_slot_clip();
    shell
        .run(
            "session.insert_empty_midi_clip",
            json!({ "track": track, "scene": second }),
        )
        .unwrap();
    let cancel =
        |shell: &mut Shell| shell.run("session.cancel_scene_launch", json!({ "scene": second }));
    // Nothing queued: refused, and the menu's item is off.
    assert!(matches!(
        cancel(&mut shell),
        Err(ShellError::Unavailable { .. })
    ));
    let contexts = [bl_commands::Context::Session];
    assert!(!shell.can_invoke_in(
        "session.cancel_scene_launch",
        &json!({ "scene": second }),
        &contexts
    ));

    // The first scene plays; the second waits for its line.
    shell
        .run(ids::SET_QUANTIZATION, json!({ "quantization": "none" }))
        .unwrap();
    shell
        .run("session.launch_scene", json!({ "scene": first }))
        .unwrap();
    wait_for_slots(&control, &mut shell, "the first scene", |slots| {
        slots.len() == 1 && slots[0].state == bl_engine::SlotState::Playing
    });
    shell
        .run(ids::SET_QUANTIZATION, json!({ "quantization": "8_bars" }))
        .unwrap();
    shell
        .run("session.launch_scene", json!({ "scene": second }))
        .unwrap();
    wait_for_slots(&control, &mut shell, "the queued launch", |slots| {
        slots
            .iter()
            .any(|s| s.scene == second && s.state == bl_engine::SlotState::Queued)
    });
    let state: SessionViewState =
        serde_json::from_value(shell.run("session.state", json!({})).unwrap()).unwrap();
    let state_of = |scene| {
        state
            .launches
            .iter()
            .find(|l| l.slot.scene == scene)
            .map(|l| l.state)
    };
    assert_eq!(state_of(second), Some(LaunchState::Queued));
    assert_eq!(state_of(first), Some(LaunchState::QueuedToStop));
    assert!(shell.can_invoke_in(
        "session.cancel_scene_launch",
        &json!({ "scene": second }),
        &contexts
    ));

    cancel(&mut shell).unwrap();
    wait_for_slots(&control, &mut shell, "the cancel", |slots| {
        slots.len() == 1
            && slots[0].scene == first
            && slots[0].state == bl_engine::SlotState::Playing
    });
    assert!(matches!(
        cancel(&mut shell),
        Err(ShellError::Unavailable { .. })
    ));
}

//! The global launch quantisation and the Session View's Cmd+Enter stop
//! (WP-3.2), driven headless through `Shell::run` as the Control Bar's
//! menu, the Options items, and the keys drive them (D17). The quantisation
//! is saved with the set and changing it is no undo step. The behaviour is
//! in the launch quantisation spec.

use std::path::{Path, PathBuf};

use bl_model::{LaunchQuantization, LaunchSettings, TrackKind};
use bl_shell::audio::AudioHost;
use bl_shell::files::ProjectFiles;
use bl_shell::session_launch_commands as ids;
use bl_shell::settings::Preferences;
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
    };
    assert_eq!(shell.launch_settings(), wanted);
    assert_eq!(shell.audio().launch_settings(), wanted);
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

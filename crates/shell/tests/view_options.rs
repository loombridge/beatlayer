//! The small view options (WP-6.49): note colours, the mark on empty tracks
//! and the mouse wheel on controls, as the shell keeps and reads them.
//! Spec: view-options R2, R6, R7, R10, R14, R15.

use std::path::{Path, PathBuf};

use bl_model::{SceneId, TrackId, TrackKind};
use bl_shell::Shell;
use bl_shell::audio::AudioHost;
use bl_shell::files::ProjectFiles;
use bl_shell::settings::Preferences;
use serde_json::json;

/// A folder for one test's preferences, removed when the test ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "bl-view-options-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A shell that keeps its preferences in `config`, as it starts with the
/// ones it kept there.
fn launch(config: &Path) -> Shell {
    Shell::new(
        Preferences::load(config).unwrap_or_default(),
        Some(config.to_owned()),
        AudioHost::null(Default::default()),
        Box::new(ProjectFiles::new()),
    )
}

fn empty(shell: &mut Shell) -> Vec<TrackId> {
    let listed = shell.run("track.list_empty", json!({})).unwrap();
    serde_json::from_value(listed["tracks"].clone()).unwrap()
}

// Spec: view-options R2, R6, R10, R14.
#[test]
fn the_options_are_off_at_first_read_back_and_survive_a_relaunch_outside_the_set() {
    let scratch = Scratch::new("relaunch");
    let mut shell = launch(&scratch.0);
    let view = |shell: &mut Shell| shell.run("view.state", json!({})).unwrap();
    let settings = |shell: &mut Shell| shell.run("settings.state", json!({})).unwrap();
    assert_eq!(view(&mut shell)["note_colors"], "velocity");
    assert_eq!(view(&mut shell)["mark_empty_tracks"], false);
    assert_eq!(
        settings(&mut shell)["display_input"]["wheel_adjusts_values"],
        false
    );

    let steps = shell.editor().history().len();
    shell
        .run("view.set_note_colors", json!({ "colors": "scale_degree" }))
        .unwrap();
    shell
        .run("view.set_mark_empty_tracks", json!({ "on": true }))
        .unwrap();
    shell
        .run("settings.set_wheel_adjusts_values", json!({ "on": true }))
        .unwrap();
    assert_eq!(view(&mut shell)["note_colors"], "scale_degree");
    assert_eq!(view(&mut shell)["mark_empty_tracks"], true);
    assert_eq!(
        settings(&mut shell)["display_input"]["wheel_adjusts_values"],
        true
    );
    assert!(
        shell
            .run("view.set_note_colors", json!({ "colors": "rainbow" }))
            .is_err(),
        "a colouring that isn't offered is refused"
    );
    assert_eq!(shell.editor().history().len(), steps, "no undo step");
    assert!(!shell.unsaved_changes(), "nothing in the set changed");

    // A relaunch reads them back from the preferences.
    let mut again = launch(&scratch.0);
    assert_eq!(view(&mut again)["note_colors"], "scale_degree");
    assert_eq!(view(&mut again)["mark_empty_tracks"], true);
    assert_eq!(
        settings(&mut again)["display_input"]["wheel_adjusts_values"],
        true
    );
}

// Spec: view-options R7, R15.
#[test]
fn track_list_empty_lists_tracks_with_no_clip_and_no_device_whatever_the_switch() {
    let mut shell = Shell::headless();
    let kinds = |shell: &Shell, tracks: &[TrackId]| -> Vec<TrackKind> {
        let doc = shell.editor().document();
        tracks
            .iter()
            .map(|t| doc.track(*t).unwrap().kind())
            .collect()
    };
    let listed = empty(&mut shell);
    assert!(!listed.is_empty(), "the default set's tracks are empty");
    assert!(
        !kinds(&shell, &listed).contains(&TrackKind::Main),
        "Main is never listed"
    );

    // A clip in a Session slot makes its track not empty; the switch changes
    // nothing about the list.
    let top: Vec<TrackId> = shell.editor().document().top_level().collect();
    let scene: SceneId = shell.editor().document().scenes().next().unwrap().id;
    shell
        .run(
            "session.insert_empty_midi_clip",
            json!({ "track": top[0], "scene": scene }),
        )
        .unwrap();
    assert!(!empty(&mut shell).contains(&top[0]));
    assert!(empty(&mut shell).contains(&top[1]));
    // So does a device.
    let eq = bl_devices::eq_type();
    shell
        .run(
            "device.add",
            json!({ "track": top[2], "device": {
                "kind": eq.kind, "name": eq.name, "role": eq.role, "sidechain": eq.sidechain } }),
        )
        .unwrap();
    assert!(!empty(&mut shell).contains(&top[2]));
    shell
        .run("view.set_mark_empty_tracks", json!({ "on": true }))
        .unwrap();
    assert!(!empty(&mut shell).contains(&top[0]));
    assert!(shell.shows_empty_mark(top[1]) && !shell.shows_empty_mark(top[0]));

    // A group is empty until a track in it holds something.
    shell
        .invoke("track.create", json!({ "kind": "group" }))
        .unwrap();
    let group = *shell
        .editor()
        .document()
        .top_level()
        .collect::<Vec<_>>()
        .last()
        .unwrap();
    shell
        .run(
            "track.move",
            json!({ "track": top[0], "parent": group, "index": 0 }),
        )
        .unwrap();
    assert!(
        !empty(&mut shell).contains(&group),
        "its child holds a clip"
    );
    // A group of an empty track is empty too.
    shell
        .invoke("track.create", json!({ "kind": "group" }))
        .unwrap();
    let second = *shell
        .editor()
        .document()
        .top_level()
        .collect::<Vec<_>>()
        .last()
        .unwrap();
    shell
        .run(
            "track.move",
            json!({ "track": top[1], "parent": second, "index": 0 }),
        )
        .unwrap();
    assert!(empty(&mut shell).contains(&second));
    assert!(!empty(&mut shell).contains(&group));
}

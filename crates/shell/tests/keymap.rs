//! The person's own keyboard shortcuts: the file beside the preferences,
//! the keymap in use, and the commands that change them (the custom
//! shortcuts spec, R1 to R6 and R14 to R16).

use std::path::{Path, PathBuf};

use bl_commands::Context;
use bl_shell::Shell;
use bl_shell::audio::AudioHost;
use bl_shell::files::ProjectFiles;
use bl_shell::keymap::{Keymap, KeymapState, Mods, Origin, Platform, Trigger};
use bl_shell::settings::Preferences;
use serde_json::json;

/// A directory for the test's preferences and keymap.
fn dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("bl-shell-keymap-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A shell with a config directory, so the person's keymap file is read
/// and written.
fn shell_with(dir: &Path) -> Shell {
    Shell::new(
        Preferences::default(),
        Some(dir.to_owned()),
        AudioHost::null(Default::default()),
        Box::new(ProjectFiles::new()),
    )
}

/// The modifiers of a key press.
fn mods(cmd: bool, ctrl: bool, alt: bool, shift: bool) -> Mods {
    Mods {
        ctrl,
        alt,
        shift,
        cmd,
    }
}

#[test]
fn with_no_file_the_keymap_is_lives() {
    // Spec: custom-shortcuts R1.
    let dir = dir("no-file");
    let shell = shell_with(&dir);
    assert_eq!(shell.keymap(), &Keymap::live12());
    assert!(!dir.join("keymap.toml").exists());
}

#[test]
fn an_empty_file_leaves_the_keymap_alone() {
    // Spec: custom-shortcuts R1.
    let dir = dir("empty-file");
    std::fs::write(dir.join("keymap.toml"), "schema = 1\n").unwrap();
    let shell = shell_with(&dir);
    assert_eq!(shell.keymap(), &Keymap::live12());
}

#[test]
fn a_bind_gives_a_command_other_keys() {
    // Spec: custom-shortcuts R3, R11.
    let dir = dir("bind");
    let mut shell = shell_with(&dir);
    shell
        .run(
            "keymap.bind",
            json!({
                "command": "track.duplicate_without_clips",
                "keys": ["Cmd+Option+D"],
                "platform": "mac",
                "context": "global",
            }),
        )
        .unwrap();
    // The keys run the command from the Arrangement.
    let hit = shell.keymap().resolve(
        Platform::Mac,
        "D",
        mods(true, false, true, false),
        &[Context::Arrangement, Context::Global],
    );
    assert_eq!(
        hit.map(|hit| hit.command.id.clone()),
        Some("track.duplicate_without_clips".to_owned())
    );
    // The file holds the binding (R11).
    let text = std::fs::read_to_string(dir.join("keymap.toml")).unwrap();
    assert!(text.contains("track.duplicate_without_clips"), "{text}");
    assert!(text.contains("Cmd+Option+D"), "{text}");
}

#[test]
fn a_bind_over_lives_own_key_takes_it() {
    // Spec: custom-shortcuts R3.
    let dir = dir("over");
    let mut shell = shell_with(&dir);
    shell
        .run(
            "keymap.bind",
            json!({
                "command": "track.duplicate_without_clips",
                "keys": ["Cmd+D"],
                "platform": "mac",
                "context": "global",
                "replace": true,
            }),
        )
        .unwrap();
    let hit = shell.keymap().resolve(
        Platform::Mac,
        "D",
        mods(true, false, false, false),
        &[Context::Arrangement, Context::Global],
    );
    assert_eq!(
        hit.map(|hit| hit.command.id.clone()),
        Some("track.duplicate_without_clips".to_owned())
    );
    // The default shortcut keeps its entry, without the key.
    let duplicate = shell.keymap().entry_for("S-EDIT-004").unwrap();
    assert!(duplicate.mac.is_empty());
}

#[test]
fn undo_leaves_the_keymap_alone() {
    // Spec: custom-shortcuts R11.
    let dir = dir("undo");
    let mut shell = shell_with(&dir);
    shell
        .run(
            "keymap.bind",
            json!({
                "command": "track.duplicate_without_clips",
                "keys": ["Cmd+Option+D"],
                "platform": "mac",
                "context": "global",
            }),
        )
        .unwrap();
    let before = shell.keymap().clone();
    shell
        .invoke("track.create", json!({ "kind": "audio" }))
        .unwrap();
    shell.run("edit.undo", json!({})).unwrap();
    assert_eq!(shell.keymap(), &before, "a shortcut is not an undo step");
}

#[test]
fn an_unbind_takes_the_keys_away_on_one_platform_only() {
    // Spec: custom-shortcuts R2, R3.
    let dir = dir("unbind");
    let mut shell = shell_with(&dir);
    shell
        .run(
            "keymap.unbind",
            json!({ "shortcut": "S-EDIT-004", "platform": "mac" }),
        )
        .unwrap();
    let contexts = [Context::Arrangement, Context::Global];
    assert!(
        shell
            .keymap()
            .resolve(
                Platform::Mac,
                "D",
                mods(true, false, false, false),
                &contexts
            )
            .is_none(),
        "macOS lost the key"
    );
    assert!(
        shell
            .keymap()
            .resolve(
                Platform::Other,
                "D",
                mods(false, true, false, false),
                &contexts
            )
            .is_some(),
        "Windows keeps it"
    );
}

#[test]
fn a_latching_command_keeps_its_latch_on_new_keys() {
    // Spec: custom-shortcuts R4.
    let dir = dir("latch");
    let mut shell = shell_with(&dir);
    shell
        .run(
            "keymap.bind",
            json!({
                "command": "view.draw_mode",
                "keys": ["Cmd+Shift+B"],
                "platform": "mac",
                "context": "global",
            }),
        )
        .unwrap();
    let own = shell
        .keymap()
        .entries()
        .iter()
        .find(|entry| {
            entry.commands.iter().any(|c| c.id == "view.draw_mode")
                && entry.mac_origin == Origin::User
        })
        .expect("the person's own shortcut");
    assert_eq!(own.trigger, Trigger::Latch);
}

#[test]
fn entries_that_cant_be_used_are_skipped_and_listed() {
    // Spec: custom-shortcuts R6.
    let dir = dir("skipped");
    std::fs::write(
        dir.join("keymap.toml"),
        r#"schema = 1

[[bind]]
command = "no.such.command"
mac = ["Cmd+Shift+F12"]

[[bind]]
command = "track.duplicate_without_clips"
mac = ["NotAKey"]

[[bind]]
command = "track.duplicate_without_clips"
mac = ["Cmd+Shift+F11"]
context = "text-field"

[[bind]]
command = "track.duplicate_without_clips"
mac = ["Cmd+Shift+F10"]
context = "nowhere"

[[bind]]
command = "track.duplicate_without_clips"
mac = ["Cmd+Shift+F9"]
"#,
    )
    .unwrap();
    let shell = shell_with(&dir);
    let state = shell.keymap_state();
    assert_eq!(state.skipped.len(), 4, "{:?}", state.skipped);
    // The rest loads (R6).
    let hit = shell.keymap().resolve(
        Platform::Mac,
        "F9",
        mods(true, false, false, true),
        &[Context::Global],
    );
    assert_eq!(
        hit.map(|hit| hit.command.id.clone()),
        Some("track.duplicate_without_clips".to_owned())
    );
}

#[test]
fn the_commands_read_back_through_keymap_state() {
    // Spec: custom-shortcuts R14, R15.
    let dir = dir("readback");
    let mut shell = shell_with(&dir);
    shell
        .run(
            "keymap.bind",
            json!({
                "command": "track.duplicate_without_clips",
                "keys": ["Cmd+Option+D"],
                "platform": "mac",
                "context": "global",
            }),
        )
        .unwrap();
    let state: KeymapState =
        serde_json::from_value(shell.run("keymap.state", json!({})).unwrap()).unwrap();
    let own = state
        .shortcuts
        .iter()
        .find(|shortcut| shortcut.mac.origin == Origin::User)
        .expect("the person's own shortcut");
    assert_eq!(own.commands, ["track.duplicate_without_clips"]);
    assert_eq!(own.mac.keys, ["Cmd+Option+D"]);
    // The default shortcuts are Live's own.
    let live = state
        .shortcuts
        .iter()
        .find(|shortcut| shortcut.id == "S-EDIT-004")
        .expect("Duplicate");
    assert_eq!(live.mac.origin, Origin::Live);
}

#[test]
fn a_reset_gives_the_command_lives_keys_back() {
    // Spec: custom-shortcuts R10, R15.
    let dir = dir("reset");
    let mut shell = shell_with(&dir);
    shell
        .run(
            "keymap.bind",
            json!({
                "command": "track.duplicate_without_clips",
                "keys": ["Cmd+Option+D"],
                "platform": "mac",
                "context": "global",
            }),
        )
        .unwrap();
    shell
        .run(
            "keymap.reset",
            json!({ "command": "track.duplicate_without_clips" }),
        )
        .unwrap();
    let state: KeymapState =
        serde_json::from_value(shell.run("keymap.state", json!({})).unwrap()).unwrap();
    assert!(
        state
            .shortcuts
            .iter()
            .all(|shortcut| shortcut.mac.origin != Origin::User),
        "the person's own shortcut is gone"
    );
    assert_eq!(shell.keymap(), &Keymap::live12());
}

#[test]
fn the_file_written_survives_a_reload() {
    // Spec: custom-shortcuts R1, R11.
    let dir = dir("reload");
    let mut shell = shell_with(&dir);
    shell
        .run(
            "keymap.bind",
            json!({
                "command": "track.duplicate_without_clips",
                "keys": ["Cmd+Option+D"],
                "platform": "mac",
                "context": "global",
            }),
        )
        .unwrap();
    let again = shell_with(&dir);
    let hit = again.keymap().resolve(
        Platform::Mac,
        "D",
        mods(true, false, true, false),
        &[Context::Global],
    );
    assert_eq!(
        hit.map(|hit| hit.command.id.clone()),
        Some("track.duplicate_without_clips".to_owned())
    );
}

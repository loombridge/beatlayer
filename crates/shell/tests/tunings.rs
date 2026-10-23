//! Tuning systems in the shell (WP-6.9; the tunings spec, R2, R10, R11, R14,
//! R15 and R19): `tuning.load` over the generated tuning files, `tuning.state`,
//! the retune setting and the commands that follow it, and what a person is
//! asked before a tuning changes. Driven through `Shell::run` and
//! `Shell::invoke`, as an agent and a person drive them.

use std::path::{Path, PathBuf};

use bl_model::{ClipData, ClipId, TrackId};
use bl_shell::settings::Preferences;
use bl_shell::{Effect, Shell};
use bl_shell_spec::tunings::TuningState;
use serde_json::json;

fn fixture(name: &str) -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/tunings")
        .join(name)
        .to_string_lossy()
        .into_owned()
}

/// A shell with a MIDI track holding a clip of notes on keys 61 and 63 (the
/// clip's `(key, start, length)` as the notes are made).
fn shell_with_notes() -> (Shell, TrackId, ClipId) {
    let mut shell = Shell::headless();
    let track: TrackId = shell.new_id();
    shell
        .run(
            "track.create",
            json!({ "id": track, "kind": "midi", "name": "Keys", "parent": null, "index": 0 }),
        )
        .unwrap();
    let clip: ClipId = shell.new_id();
    let note = |key: u8, start: i64| json!({ "key": key, "velocity": 100, "start": start, "length": 1000 });
    shell
        .run(
            "clip.create",
            json!({
                "track": track,
                "clip": {
                    "id": clip,
                    "name": "Riff",
                    "start": 0,
                    "length": 705_600,
                    "offset": 0,
                    "mute": false,
                    "data": { "midi": { "notes": [note(61, 0), note(63, 2000)] } },
                },
            }),
        )
        .unwrap();
    (shell, track, clip)
}

fn keys(shell: &Shell, clip: ClipId) -> Vec<u8> {
    let ClipData::Midi(midi) = &shell.editor().document().clip(clip).unwrap().data else {
        panic!("a MIDI clip");
    };
    let mut notes: Vec<_> = midi.notes.iter().map(|n| (n.start, n.key)).collect();
    notes.sort();
    notes.into_iter().map(|(_, key)| key).collect()
}

fn state(shell: &mut Shell) -> TuningState {
    serde_json::from_value(shell.run("tuning.state", json!({})).unwrap()).unwrap()
}

// Spec: tunings R2, R3, R5, R19
#[test]
fn tuning_load_reads_a_file_and_tuning_state_reads_it_back() {
    let (mut shell, _, _) = shell_with_notes();
    assert_eq!(state(&mut shell).tuning, None, "twelve equal steps");
    let steps = shell.editor().undo_history().len();
    let loaded: TuningState = serde_json::from_value(
        shell
            .run("tuning.load", json!({ "path": fixture("equal-19.scl") }))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(shell.editor().undo_history().len(), steps + 1);
    assert_eq!(shell.editor().undo_history().next(), Some("Load Tuning"));
    let tuning = loaded.tuning.expect("a tuning");
    assert_eq!(tuning.name, "equal-19");
    assert_eq!(tuning.notes_per_octave, 19);
    assert_eq!(tuning.key_count, 128);
    assert!((tuning.frequencies[60].unwrap() - 261.626).abs() < 1e-9);
    assert_eq!(state(&mut shell).tuning, Some(tuning));
    // The query tool's song reads the same (checked in the agent tests).
    shell.run("edit.undo", json!({})).unwrap();
    assert_eq!(state(&mut shell).tuning, None);
}

// Spec: tunings R3. A file R3 refuses leaves the set's tuning as it was.
#[test]
fn a_file_that_breaks_the_rules_is_refused_with_its_reason() {
    let (mut shell, _, _) = shell_with_notes();
    shell
        .run("tuning.load", json!({ "path": fixture("equal-5.scl") }))
        .unwrap();
    let before = state(&mut shell);
    let steps = shell.editor().undo_history().len();
    for (file, why) in [
        ("no-pitches.scl", "no pitches"),
        ("falling.scl", "pseudo-octave"),
        ("short.scl", "says it holds 4"),
        ("ratio-zero.scl", "pitch 1"),
        ("missing.scl", "can't be read"),
    ] {
        let error = shell
            .run("tuning.load", json!({ "path": fixture(file) }))
            .unwrap_err()
            .to_string();
        assert!(error.contains(why), "{file}: {error}");
    }
    assert_eq!(state(&mut shell), before);
    assert_eq!(shell.editor().undo_history().len(), steps);
}

// Spec: tunings R15
#[test]
fn retune_follows_the_setting_unless_the_command_says() {
    // Off: the notes stay.
    let (mut shell, _, clip) = shell_with_notes();
    assert!(!shell.retune_on_tuning_load());
    shell
        .run("tuning.load", json!({ "path": fixture("equal-5.scl") }))
        .unwrap();
    assert_eq!(keys(&shell, clip), vec![61, 63]);
    shell.run("tuning.remove", json!({})).unwrap();
    assert_eq!(keys(&shell, clip), vec![61, 63]);

    // On: loading and removing retune, in the step of the load.
    shell
        .run("settings.set_retune_on_tuning_load", json!({ "on": true }))
        .unwrap();
    let steps = shell.editor().undo_history().len();
    shell
        .run("tuning.load", json!({ "path": fixture("equal-5.scl") }))
        .unwrap();
    assert_eq!(shell.editor().undo_history().len(), steps + 1);
    // Five steps to the octave: a step is 2.4 semitones, so key 61 (pitch
    // 61) lands nearest the step at 60 and key 63 nearest the one at 62.4.
    assert_eq!(keys(&shell, clip), vec![60, 61]);
    shell.run("edit.undo", json!({})).unwrap();
    assert_eq!(keys(&shell, clip), vec![61, 63]);

    // `retune: false` overrides the setting, `tuning.set` follows it.
    shell
        .run(
            "tuning.load",
            json!({ "path": fixture("equal-5.scl"), "retune": false }),
        )
        .unwrap();
    assert_eq!(keys(&shell, clip), vec![61, 63]);
    shell
        .run("tuning.remove", json!({ "retune": true }))
        .unwrap();
    assert_eq!(keys(&shell, clip).len(), 2);
}

// Spec: tunings R15. `tuning.set` and `tuning.remove` follow the setting too.
#[test]
fn the_commands_that_set_and_remove_follow_the_setting() {
    let (mut shell, _, clip) = shell_with_notes();
    shell
        .run("settings.set_retune_on_tuning_load", json!({ "on": true }))
        .unwrap();
    let cents: Vec<f64> = (1..=5).map(|i| 240.0 * f64::from(i)).collect();
    let tuning = bl_model::Tuning::new("five".into(), String::new(), cents).unwrap();
    shell
        .run("tuning.set", json!({ "tuning": tuning }))
        .unwrap();
    assert_eq!(keys(&shell, clip), vec![60, 61], "retuned");
    shell
        .run("settings.set_retune_on_tuning_load", json!({ "on": false }))
        .unwrap();
    shell.run("tuning.remove", json!({})).unwrap();
    assert_eq!(keys(&shell, clip), vec![60, 61], "left alone");
}

// Spec: tunings R15. A person who loads or removes a tuning is asked first
// while the setting is on; the answer is the `retune` the command then runs
// with.
#[test]
fn a_person_is_asked_before_a_tuning_changes_when_the_setting_is_on() {
    let (mut shell, _, clip) = shell_with_notes();
    let load = json!({ "path": fixture("equal-5.scl") });
    // Off: it runs at once.
    shell.invoke("tuning.load", load.clone()).unwrap();
    assert!(shell.take_effects().is_empty());
    assert!(state(&mut shell).tuning.is_some());
    shell.invoke("tuning.remove", json!({})).unwrap();
    assert!(shell.take_effects().is_empty());

    // On: the shell asks, and changes nothing until the answer.
    shell
        .invoke("settings.set_retune_on_tuning_load", json!({}))
        .unwrap();
    assert!(
        shell.retune_on_tuning_load(),
        "the menu item switches it on"
    );
    let steps = shell.editor().undo_history().len();
    shell.invoke("tuning.load", load.clone()).unwrap();
    let effects = shell.take_effects();
    let [Effect::ConfirmRetune { then }] = &effects[..] else {
        panic!("{effects:?}");
    };
    assert_eq!(then.id, "tuning.load");
    assert_eq!(
        shell.editor().undo_history().len(),
        steps,
        "Cancel changes nothing"
    );
    assert_eq!(state(&mut shell).tuning, None);

    // Retune: the command runs again with `retune` true.
    let mut args = then.args.clone();
    args["retune"] = json!(true);
    shell.invoke(&then.id, args).unwrap();
    assert!(
        shell.take_effects().is_empty(),
        "an answer isn't asked again"
    );
    assert_eq!(keys(&shell, clip), vec![60, 61]);

    // Removing asks too, but not with no tuning to remove.
    shell.invoke("tuning.remove", json!({})).unwrap();
    assert!(matches!(
        &shell.take_effects()[..],
        [Effect::ConfirmRetune { .. }]
    ));
}

// Spec: tunings R15 (M-OPT-022). The preference is off in a new install and
// kept between launches; the menu item runs the setting's command.
#[test]
fn the_setting_is_off_in_a_new_install_and_kept() {
    let prefs = Preferences::default();
    assert!(!prefs.retune_on_tuning_load);
    let saved = serde_json::to_value(Preferences {
        retune_on_tuning_load: true,
        ..Preferences::default()
    })
    .unwrap();
    assert_eq!(saved["retune_on_tuning_load"], true);
    let older: Preferences = serde_json::from_str("{}").unwrap();
    assert!(!older.retune_on_tuning_load, "preferences from before it");
    let kept: Preferences = serde_json::from_value(saved).unwrap();
    assert!(kept.retune_on_tuning_load);

    let mut shell = Shell::headless();
    let read = |shell: &mut Shell| shell.run("settings.state", json!({})).unwrap();
    assert_eq!(read(&mut shell)["retune_on_tuning_load"], false);
    shell
        .run("settings.set_retune_on_tuning_load", json!({ "on": true }))
        .unwrap();
    assert_eq!(read(&mut shell)["retune_on_tuning_load"], true);
    assert_eq!(
        shell.switch_state("settings.set_retune_on_tuning_load"),
        Some(true)
    );
}

// Spec: tunings R11. The set saves its tuning inside it and opens with it
// whether or not the file is still there.
#[test]
fn a_saved_set_keeps_its_tuning_without_the_file() {
    let (mut shell, track, _) = shell_with_notes();
    let dir = std::env::temp_dir().join(format!("bl-tunings-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let copy: PathBuf = dir.join("copy.scl");
    std::fs::copy(fixture("equal-19.scl"), &copy).unwrap();
    shell
        .run("tuning.load", json!({ "path": copy.to_string_lossy() }))
        .unwrap();
    shell
        .run(
            "track.set_tuning_bypass",
            json!({ "track": track, "bypass": true }),
        )
        .unwrap();
    let before = state(&mut shell);
    let saved: bl_shell::commands::SetResult = serde_json::from_value(
        shell
            .run(
                "set.save_as",
                json!({ "path": dir.join("Song.blset").to_string_lossy() }),
            )
            .unwrap(),
    )
    .unwrap();
    std::fs::remove_file(&copy).unwrap();
    let mut other = Shell::headless();
    other
        .run("set.open", json!({ "path": saved.path.unwrap() }))
        .unwrap();
    assert_eq!(state(&mut other), before);
    assert!(
        other
            .editor()
            .document()
            .track(track)
            .unwrap()
            .tuning_bypass()
    );
}

//! The scales in the Clip View (WP-6.7b): Fold to Scale, Highlight Scales and
//! the note spelling, as the shell runs them. The behaviour is in the scales
//! spec, R22 to R25.

use bl_commands::Context;
use bl_core::time::Ticks;
use bl_model::{ClipId, NoteSpelling};
use bl_shell::Shell;
use bl_shell::commands::{
    VIEW_FOLD_NOTES, VIEW_FOLD_SCALE, VIEW_HIGHLIGHT_SCALES, VIEW_NOTE_SPELLING,
};
use bl_shell::files::ProjectFiles;
use bl_shell::note_commands::ClipViewState;
use bl_shell::settings::Preferences;
use serde_json::{Value, json};

/// A shell with a MIDI clip holding C#3 and A#3 (keys 61 and 70), selected.
fn shell_with_a_clip(prefs: Preferences, config: Option<std::path::PathBuf>) -> (Shell, ClipId) {
    let mut shell = Shell::new(
        prefs,
        config,
        bl_shell::audio::AudioHost::null(Default::default()),
        Box::new(ProjectFiles::new()),
    );
    let track: bl_model::TrackId = shell.new_id();
    shell
        .run(
            "track.create",
            json!({ "id": track, "kind": "midi", "name": "Keys", "parent": null, "index": 0 }),
        )
        .unwrap();
    let clip: ClipId = shell.new_id();
    let note = |key: u8| json!({ "key": key, "start": 0, "length": 1000, "velocity": 100 });
    shell
        .run(
            "clip.create",
            json!({
                "track": track,
                "clip": {
                    "id": clip,
                    "name": "",
                    "start": 0,
                    "length": Ticks::WHOLE.get() * 4,
                    "offset": 0,
                    "mute": false,
                    "data": { "midi": { "notes": [note(61), note(70)] } },
                },
            }),
        )
        .unwrap();
    shell
        .run("arrangement.select_clips", json!({ "clips": [clip] }))
        .unwrap();
    (shell, clip)
}

fn state(shell: &mut Shell, id: &str, args: Value) -> ClipViewState {
    serde_json::from_value(shell.run(id, args).unwrap()).unwrap()
}

fn set_scale(shell: &mut Shell, clip: ClipId, enabled: bool) {
    shell
        .run(
            "clip.set_scale",
            json!({ "clips": [clip], "enabled": enabled, "root": 0, "name": "major" }),
        )
        .unwrap();
}

fn temp_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("bl-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

// Spec: scales R23
#[test]
fn fold_to_scale_needs_scale_mode_and_shows_the_scale_and_the_notes() {
    let (mut shell, clip) = shell_with_a_clip(Preferences::default(), None);
    assert!(
        shell.run(VIEW_FOLD_SCALE, json!({})).is_err(),
        "Scale Mode is off"
    );
    let editor = [Context::NoteEditor, Context::ClipView];
    assert!(!shell.can_invoke_in(VIEW_FOLD_SCALE, &json!({}), &editor));
    set_scale(&mut shell, clip, true);
    assert!(shell.can_invoke_in(VIEW_FOLD_SCALE, &json!({}), &editor));
    let steps = shell.editor().undo_history().len();
    let dirty = shell.is_dirty();
    let shown = state(&mut shell, VIEW_FOLD_SCALE, json!({}));
    assert_eq!(shown.scale_folds, vec![clip], "view.state reads it");
    assert_eq!(shell.switch_state(VIEW_FOLD_SCALE), Some(true));
    assert_eq!(shell.editor().undo_history().len(), steps, "no undo step");
    assert_eq!(shell.is_dirty(), dirty, "and no change to the set");

    // The rows: the 75 keys of C Major, and the two keys that hold notes.
    let keys = shell
        .clip_view()
        .fold(shell.editor().document(), clip)
        .keys([61, 70]);
    assert_eq!(keys.len(), 77);
    assert!(keys.contains(&61) && keys.contains(&70) && keys.contains(&60));
    assert!(!keys.contains(&63), "D#3 is neither");
    assert!(keys.windows(2).all(|pair| pair[0] > pair[1]), "top down");

    // With Fold to Notes on too, a row shows only if each fold shows it.
    state(&mut shell, VIEW_FOLD_NOTES, json!({}));
    let keys = shell
        .clip_view()
        .fold(shell.editor().document(), clip)
        .keys([61, 70]);
    assert_eq!(keys, [70, 61]);

    // Turning Scale Mode off shows every row but Fold to Notes', and on
    // again brings the fold back.
    state(&mut shell, VIEW_FOLD_NOTES, json!({}));
    set_scale(&mut shell, clip, false);
    let fold = shell.clip_view().fold(shell.editor().document(), clip);
    assert!(!fold.on(), "Scale Mode off: every row");
    set_scale(&mut shell, clip, true);
    let fold = shell.clip_view().fold(shell.editor().document(), clip);
    assert!(fold.scale.is_some(), "and on again: folded again");

    // `folded` says which way.
    let unfolded = state(&mut shell, VIEW_FOLD_SCALE, json!({ "folded": false }));
    assert!(unfolded.scale_folds.is_empty());
}

// Spec: scales R22
#[test]
fn highlight_scales_is_a_preference_kept_between_launches() {
    let config = temp_dir("highlight");
    let (mut shell, _) = shell_with_a_clip(Preferences::default(), Some(config.clone()));
    assert_eq!(shell.switch_state(VIEW_HIGHLIGHT_SCALES), Some(false));
    let steps = shell.editor().undo_history().len();
    let dirty = shell.is_dirty();
    let shown = state(&mut shell, VIEW_HIGHLIGHT_SCALES, json!({}));
    assert!(shown.highlight_scales, "with no argument the switch flips");
    assert_eq!(shell.switch_state(VIEW_HIGHLIGHT_SCALES), Some(true));
    assert_eq!(shell.editor().undo_history().len(), steps, "no undo step");
    assert_eq!(shell.is_dirty(), dirty, "and not part of the set");
    assert!(Preferences::load(&config).unwrap().highlight_scales);

    let (restarted, _) = shell_with_a_clip(Preferences::load(&config).unwrap(), None);
    assert_eq!(
        restarted.switch_state(VIEW_HIGHLIGHT_SCALES),
        Some(true),
        "on after a relaunch"
    );
    std::fs::remove_file(config.join("preferences.json")).ok();
}

// Spec: scales R24
#[test]
fn the_note_spelling_is_a_preference_kept_between_launches() {
    let config = temp_dir("spelling");
    let (mut shell, _) = shell_with_a_clip(Preferences::default(), Some(config.clone()));
    assert_eq!(shell.clip_view().note_spelling(), NoteSpelling::Sharps);
    let shown = state(
        &mut shell,
        VIEW_NOTE_SPELLING,
        json!({ "spelling": "flats" }),
    );
    assert_eq!(shown.note_spelling, NoteSpelling::Flats);
    assert!(
        shell.choice_state(VIEW_NOTE_SPELLING, &json!({ "spelling": "flats" })) == Some(true)
            && shell.choice_state(VIEW_NOTE_SPELLING, &json!({ "spelling": "auto" }))
                == Some(false),
        "the menu ticks the one in use"
    );
    assert!(
        shell
            .run(VIEW_NOTE_SPELLING, json!({ "spelling": "nonsense" }))
            .is_err()
    );
    assert_eq!(
        Preferences::load(&config).unwrap().note_spelling,
        NoteSpelling::Flats
    );
    let (restarted, _) = shell_with_a_clip(Preferences::load(&config).unwrap(), None);
    assert_eq!(restarted.clip_view().note_spelling(), NoteSpelling::Flats);
    std::fs::remove_file(config.join("preferences.json")).ok();
}

// Spec: scales R23
#[test]
fn a_clip_folded_to_its_scale_is_folded_again_when_the_set_is_reopened() {
    let dir = temp_dir("scale-folds");
    let path = dir.join("Folds.blset");
    let (mut shell, clip) = shell_with_a_clip(Preferences::default(), None);
    set_scale(&mut shell, clip, true);
    state(&mut shell, VIEW_FOLD_SCALE, json!({}));
    let saved = shell
        .run("set.save_as", json!({ "path": path.display().to_string() }))
        .unwrap();
    shell.run("set.new", json!({})).unwrap();
    assert!(shell.clip_view().scale_folds().is_empty());
    shell
        .run("set.open", json!({ "path": saved["path"] }))
        .unwrap();
    assert!(shell.clip_view().is_scale_folded(clip));
    assert!(!shell.clip_view().is_folded(clip), "Fold to Notes is apart");
    let _ = std::fs::remove_dir_all(&dir);
}

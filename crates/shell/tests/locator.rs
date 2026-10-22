//! The Arrangement's locators, driven headless through `Shell::run` and
//! `Shell::invoke`, as agents, menus, and keys drive them (WP-6.24; D17).
//! The behaviour is in the time editing spec.

use bl_commands::Permission;
use bl_core::time::Ticks;
use std::path::{Path, PathBuf};

use bl_audio_io::NullControl;
use bl_model::{Editor, LocatorId};
use bl_shell::arrangement_commands::ArrangementState;
use bl_shell::audio::AudioHost;
use bl_shell::files::{SetStore, factory_default_set};
use bl_shell::locator_commands as ids;
use bl_shell::settings::Preferences;
use bl_shell::testing;
use bl_shell::{Shell, ShellError};
use serde_json::{Value, json};

const BAR: i64 = Ticks::WHOLE.get();

fn view(shell: &mut Shell) -> ArrangementState {
    serde_json::from_value(shell.run("arrangement.state", json!({})).unwrap()).unwrap()
}

fn run(shell: &mut Shell, id: &str, args: Value) -> Value {
    shell
        .run(id, args)
        .unwrap_or_else(|e| panic!("{id} failed: {e}"))
}

/// Opens a set held in memory.
struct Preloaded(Option<Editor>);

impl SetStore for Preloaded {
    fn open(&mut self, path: &Path) -> Result<(Editor, PathBuf), String> {
        let editor = self.0.take().ok_or("opened already")?;
        Ok((editor, path.to_owned()))
    }

    fn save(&mut self, _: &Path, _: &Editor) -> Result<Vec<String>, String> {
        Err("not saved".to_owned())
    }

    fn save_as(&mut self, _: &Path, _: &Editor) -> Result<(PathBuf, Vec<String>), String> {
        Err("not saved".to_owned())
    }

    fn close(&mut self) {}
}

/// A shell showing the factory default set.
fn shell() -> Shell {
    let editor = factory_default_set();
    let mut shell = Shell::new(
        Preferences::default(),
        None,
        AudioHost::null(Default::default()),
        Box::new(Preloaded(Some(editor))),
    );
    shell
        .run("set.open", json!({ "path": "Clips.blset" }))
        .unwrap();
    shell
}

/// The locators the set holds, in time order.
fn locators(shell: &Shell) -> Vec<(LocatorId, Ticks, String)> {
    shell
        .editor()
        .document()
        .locators()
        .iter()
        .map(|locator| (locator.id, locator.position, locator.name.clone()))
        .collect()
}

/// How many steps the undo history holds.
fn steps(shell: &Shell) -> usize {
    shell.editor().undo_history().len()
}

#[test]
fn every_locator_command_is_registered_with_its_class() {
    let shell = shell();
    for (id, permission) in [
        (ids::ADD, Permission::Edit),
        (ids::DELETE, Permission::Destructive),
        (ids::MOVE, Permission::Edit),
        (ids::RENAME, Permission::Edit),
        (ids::SET_INFO_TEXT, Permission::Edit),
        (ids::LOOP_TO_NEXT, Permission::Edit),
        (ids::SELECT, Permission::Read),
        (ids::JUMP, Permission::Read),
        (ids::JUMP_PREVIOUS, Permission::Read),
        (ids::JUMP_NEXT, Permission::Read),
        (ids::SET_START_TIME, Permission::Edit),
    ] {
        let spec = shell
            .registry()
            .get(id)
            .unwrap_or_else(|| panic!("{id} isn't registered"));
        assert_eq!(spec.permission, Some(permission), "{id}");
    }
}

#[test]
fn a_locator_is_added_named_and_selected_in_one_undo_step() {
    let mut shell = shell();
    let before = steps(&shell);
    run(&mut shell, ids::ADD, json!({ "at": "2.1.1" }));
    let held = locators(&shell);
    assert_eq!(held.len(), 1);
    assert_eq!(held[0].1, Ticks(BAR));
    // Spec: time-editing R3. A new locator is named with the next whole
    // number.
    assert_eq!(held[0].2, "1");
    assert_eq!(view(&mut shell).selected_locators, vec![held[0].0]);
    assert_eq!(steps(&shell), before + 1);
    // A second one is named "2".
    run(&mut shell, ids::ADD, json!({ "at": "3.1.1" }));
    let held = locators(&shell);
    assert_eq!(held.len(), 2);
    assert_eq!(held[1].2, "2");
}

#[test]
fn two_locators_never_share_a_position() {
    let mut shell = shell();
    run(&mut shell, ids::ADD, json!({ "at": "2.1.1" }));
    let before = steps(&shell);
    // Spec: time-editing R1. Adding where a locator is leaves it as it is.
    run(&mut shell, ids::ADD, json!({ "at": "2.1.1" }));
    assert_eq!(locators(&shell).len(), 1);
    assert_eq!(steps(&shell), before);
}

#[test]
fn a_locator_is_deleted_in_one_undo_step_and_clears_the_song_start_time() {
    let mut shell = shell();
    run(&mut shell, ids::ADD, json!({ "at": "2.1.1" }));
    let locator = locators(&shell)[0].0;
    run(
        &mut shell,
        ids::SET_START_TIME,
        json!({ "locator": locator }),
    );
    assert!(shell.editor().document().settings().song_start.is_some());
    let before = steps(&shell);
    run(&mut shell, ids::DELETE, json!({ "locators": [locator] }));
    assert!(locators(&shell).is_empty());
    // Spec: time-editing R10. Deleting the locator clears the song start
    // time.
    assert!(shell.editor().document().settings().song_start.is_none());
    assert_eq!(steps(&shell), before + 1);
}

#[test]
fn a_locator_is_renamed_and_moved_in_one_undo_step_each() {
    let mut shell = shell();
    run(&mut shell, ids::ADD, json!({ "at": "2.1.1" }));
    let locator = locators(&shell)[0].0;
    let before = steps(&shell);
    run(
        &mut shell,
        ids::RENAME,
        json!({ "locators": [locator], "name": "Intro" }),
    );
    assert_eq!(locators(&shell)[0].2, "Intro");
    assert_eq!(steps(&shell), before + 1);
    run(
        &mut shell,
        ids::MOVE,
        json!({ "locator": locator, "to": "5.1.1" }),
    );
    assert_eq!(locators(&shell)[0].1, Ticks(4 * BAR));
    assert_eq!(steps(&shell), before + 2);
    // Undo puts the set back as it was.
    shell.run("edit.undo", json!({})).unwrap();
    assert_eq!(locators(&shell)[0].1, Ticks(BAR));
}

#[test]
fn an_empty_name_keeps_the_old_one() {
    let mut shell = shell();
    run(&mut shell, ids::ADD, json!({ "at": "2.1.1" }));
    let locator = locators(&shell)[0].0;
    run(
        &mut shell,
        ids::RENAME,
        json!({ "locators": [locator], "name": "Intro" }),
    );
    run(
        &mut shell,
        ids::RENAME,
        json!({ "locators": [locator], "name": "" }),
    );
    assert_eq!(locators(&shell)[0].2, "Intro");
}

#[test]
fn moving_a_locator_onto_another_changes_nothing() {
    let mut shell = shell();
    run(&mut shell, ids::ADD, json!({ "at": "2.1.1" }));
    run(&mut shell, ids::ADD, json!({ "at": "3.1.1" }));
    let held = locators(&shell);
    let before = steps(&shell);
    let error = shell
        .run(
            ids::MOVE,
            json!({ "locator": held[1].0, "to": held[0].1.get() }),
        )
        .unwrap_err();
    assert!(matches!(error, ShellError::Failed { .. }), "{error}");
    assert_eq!(steps(&shell), before);
    assert_eq!(locators(&shell)[1].1, Ticks(2 * BAR));
}

#[test]
fn loop_to_next_locator_sets_the_brace_and_switches_the_loop_on() {
    let mut shell = shell();
    run(&mut shell, ids::ADD, json!({ "at": "2.1.1" }));
    run(&mut shell, ids::ADD, json!({ "at": "5.1.1" }));
    let held = locators(&shell);
    run(
        &mut shell,
        ids::LOOP_TO_NEXT,
        json!({ "locator": held[0].0 }),
    );
    let brace = shell.editor().document().settings().arrangement_loop;
    assert_eq!(brace.start, Ticks(BAR));
    assert_eq!(brace.length, Ticks(3 * BAR));
    assert!(brace.enabled);
    // The last locator has none after it.
    let error = shell
        .run(ids::LOOP_TO_NEXT, json!({ "locator": held[1].0 }))
        .unwrap_err();
    assert!(matches!(error, ShellError::Failed { .. }), "{error}");
}

#[test]
fn the_song_start_time_follows_its_locator_and_the_brace() {
    let mut shell = shell();
    run(&mut shell, ids::ADD, json!({ "at": "2.1.1" }));
    let locator = locators(&shell)[0].0;
    run(
        &mut shell,
        ids::SET_START_TIME,
        json!({ "locator": locator }),
    );
    assert_eq!(
        shell.editor().document().song_start_ticks(),
        Some(Ticks(BAR))
    );
    // It follows the locator as it moves.
    run(
        &mut shell,
        ids::MOVE,
        json!({ "locator": locator, "to": "5.1.1" }),
    );
    assert_eq!(
        shell.editor().document().song_start_ticks(),
        Some(Ticks(4 * BAR))
    );
    // The loop brace's start, too.
    run(
        &mut shell,
        ids::SET_START_TIME,
        json!({ "loop_brace": true }),
    );
    let brace = shell.editor().document().settings().arrangement_loop;
    assert_eq!(
        shell.editor().document().song_start_ticks(),
        Some(brace.start)
    );
    // And it clears.
    run(&mut shell, ids::SET_START_TIME, json!({ "clear": true }));
    assert!(shell.editor().document().song_start_ticks().is_none());
}

#[test]
fn the_jumps_are_refused_without_an_engine() {
    let mut shell = shell();
    run(&mut shell, ids::ADD, json!({ "at": "2.1.1" }));
    for id in [ids::JUMP, ids::JUMP_PREVIOUS, ids::JUMP_NEXT] {
        let error = shell.run(id, json!({})).unwrap_err();
        assert!(
            matches!(error, ShellError::Unavailable { .. }),
            "{id}: {error}"
        );
    }
}

#[test]
fn the_selection_reports_the_selected_locators() {
    let mut shell = shell();
    run(&mut shell, ids::ADD, json!({ "at": "2.1.1" }));
    let locator = locators(&shell)[0].0;
    assert_eq!(view(&mut shell).selected_locators, vec![locator]);
    // Selecting is never an undo step.
    let before = steps(&shell);
    run(&mut shell, ids::SELECT, json!({ "locators": [] }));
    assert!(view(&mut shell).selected_locators.is_empty());
    assert_eq!(steps(&shell), before);
}

#[test]
fn the_info_text_is_stored_on_the_locator() {
    let mut shell = shell();
    run(&mut shell, ids::ADD, json!({ "at": "2.1.1" }));
    let locator = locators(&shell)[0].0;
    run(
        &mut shell,
        ids::SET_INFO_TEXT,
        json!({ "locator": locator, "info_text": "The chorus" }),
    );
    assert_eq!(
        shell
            .editor()
            .document()
            .locator(locator)
            .unwrap()
            .info_text,
        "The chorus"
    );
}

#[test]
fn a_locator_is_added_at_the_insert_marker_while_the_song_is_stopped() {
    let mut shell = shell();
    run(
        &mut shell,
        "arrangement.set_insert_marker",
        json!({ "at": BAR + Ticks::QUARTER.get() }),
    );
    run(&mut shell, ids::ADD, json!({}));
    assert_eq!(locators(&shell)[0].1, Ticks(BAR + Ticks::QUARTER.get()));
}

/// A shell playing through the null device, paced by hand (T199), with the
/// control that runs its blocks.
fn playing_shell() -> (Shell, NullControl) {
    let (mut shell, control) = testing::rig();
    shell.start_audio();
    assert!(shell.audio().engine_running(), "{:?}", shell.audio());
    (shell, control)
}

#[test]
fn the_button_places_a_locator_on_the_nearest_line_while_the_song_plays() {
    // Spec: time-editing R3. While the song plays, the Set Locator button
    // puts one on the nearest line of the global launch quantisation. The
    // null device is paced by hand, so the playhead lands on the same
    // frames however busy the machine is.
    let (mut shell, control) = playing_shell();
    run(
        &mut shell,
        "session.set_quantization",
        json!({ "quantization": "1_bar" }),
    );
    run(&mut shell, "transport.play", json!({}));
    testing::advance(&control, &mut shell, 1_500);
    let playing = shell.poll_engine();
    assert!(playing.playing, "{playing:?}");
    run(&mut shell, ids::ADD, json!({}));
    let at = locators(&shell)[0].1;
    let bar = Ticks(BAR);
    assert_eq!(at % bar, Ticks::ZERO, "a locator lands on a bar line");
    assert!(
        at <= playing.position && playing.position < at + bar,
        "on the bar the playhead is in: {at:?} against {:?}",
        playing.position
    );
}

#[test]
fn previous_and_next_jump_across_both_ends() {
    // Spec: time-editing R6. Stopped, the jumps move the insert marker to
    // the nearest locator before or after it.
    let (mut shell, _control) = playing_shell();
    // The locators are at 3.1.1 and 5.1.1, bars being one-based, so
    // 2 * BAR and 4 * BAR.
    run(&mut shell, ids::ADD, json!({ "at": "3.1.1" }));
    run(&mut shell, ids::ADD, json!({ "at": "5.1.1" }));
    run(
        &mut shell,
        "arrangement.set_insert_marker",
        json!({ "at": 3 * BAR }),
    );
    run(&mut shell, ids::JUMP_PREVIOUS, json!({}));
    assert_eq!(view(&mut shell).insert_marker, Ticks(2 * BAR));
    run(&mut shell, ids::JUMP_NEXT, json!({}));
    assert_eq!(view(&mut shell).insert_marker, Ticks(4 * BAR));
    // At the first locator, Previous goes to 1.1.1; past the last, Next
    // goes to the Arrangement's end, which is 1.1.1 in a set with no clips.
    run(
        &mut shell,
        "arrangement.set_insert_marker",
        json!({ "at": 2 * BAR }),
    );
    run(&mut shell, ids::JUMP_PREVIOUS, json!({}));
    assert_eq!(view(&mut shell).insert_marker, Ticks::ZERO);
    run(
        &mut shell,
        "arrangement.set_insert_marker",
        json!({ "at": 4 * BAR }),
    );
    run(&mut shell, ids::JUMP_NEXT, json!({}));
    assert_eq!(view(&mut shell).insert_marker, Ticks::ZERO);
}

#[test]
fn next_stops_at_the_end_of_the_last_clip() {
    // Spec: time-editing R6. With no locator after it, Next jumps to the
    // Arrangement's end, the end of its last clip on any track.
    let (mut shell, _control) = playing_shell();
    let track = shell
        .editor()
        .document()
        .top_level()
        .next()
        .expect("the factory set has a track");
    let clip = bl_model::Clip::new(
        shell.new_id(),
        Ticks(BAR),
        Ticks(BAR * 2),
        bl_model::ClipData::Midi(bl_model::MidiData::default()),
    );
    let end = clip.end();
    shell
        .run("clip.create", json!({ "track": track, "clip": clip }))
        .unwrap();
    // The locator is at 1.1.1, the only one, so it has none after it.
    run(&mut shell, ids::ADD, json!({ "at": "1.1.1" }));
    run(&mut shell, ids::JUMP_NEXT, json!({}));
    assert_eq!(view(&mut shell).insert_marker, end);
}

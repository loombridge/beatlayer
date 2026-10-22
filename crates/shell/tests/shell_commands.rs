//! The shell's commands, driven headless the way the menus, the keymap, and
//! agents drive them (D17).

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use bl_commands::{Context, Permission};
use bl_model::{Editor, TrackId};
use bl_shell::audio::AudioHost;
use bl_shell::files::{ProjectFiles, SetStore};
use bl_shell::settings::{Appearance, Preferences, SettingsPage};
use bl_shell::{Area, Effect, Invocation, MainView, Shell, ShellError, WindowKind};
use serde_json::json;

fn track_names(shell: &Shell) -> Vec<String> {
    let doc = shell.editor().document();
    doc.top_level()
        .map(|id| doc.track(id).unwrap().name().to_owned())
        .collect()
}

#[test]
fn a_new_shell_shows_the_factory_default_set_in_the_arrangement() {
    // Spec: app-shell R1.
    let shell = Shell::headless();
    assert_eq!(shell.main_view(), MainView::Arrangement);
    assert_eq!(
        track_names(&shell),
        ["1 MIDI", "2 MIDI", "3 Audio", "4 Audio"]
    );
    assert_eq!(shell.set_name(), "Untitled");
    assert!(!shell.is_dirty());
    assert_eq!(shell.dynamic_label("edit.undo").unwrap(), "Nothing to Undo");
}

#[test]
fn new_set_returns_to_the_arrangement_and_clears_the_history() {
    let mut shell = Shell::headless();
    shell
        .run("view.toggle_session_arrangement", json!({}))
        .unwrap();
    assert_eq!(shell.main_view(), MainView::Session);
    shell
        .invoke("track.create", json!({ "kind": "audio" }))
        .unwrap();
    assert!(shell.is_dirty());
    let result = shell.run("set.new", json!({})).unwrap();
    assert_eq!(result, json!({ "path": null, "name": "Untitled" }));
    assert_eq!(shell.main_view(), MainView::Arrangement);
    assert_eq!(track_names(&shell).len(), 4);
    assert!(!shell.editor().can_undo());
}

#[test]
fn the_insert_track_keys_fill_in_the_new_track() {
    // Spec: app-shell R10.
    let mut shell = Shell::headless();
    assert!(shell.can_invoke("track.create", &json!({ "kind": "audio" })));
    shell
        .invoke("track.create", json!({ "kind": "audio" }))
        .unwrap();
    shell
        .invoke("track.create", json!({ "kind": "midi" }))
        .unwrap();
    shell
        .invoke("track.create", json!({ "kind": "return" }))
        .unwrap();
    assert_eq!(
        track_names(&shell),
        [
            "1 MIDI", "2 MIDI", "3 Audio", "4 Audio", "5 Audio", "6 MIDI"
        ]
    );
    let doc = shell.editor().document();
    let last_return = doc.returns().last().unwrap();
    assert_eq!(doc.track(last_return).unwrap().name(), "C Return");
    assert_eq!(
        shell.dynamic_label("edit.undo").unwrap(),
        "Undo Create track"
    );
    shell.invoke("edit.undo", json!({})).unwrap();
    assert_eq!(shell.editor().document().returns().len(), 2);
    assert_eq!(
        shell.dynamic_label("edit.redo").unwrap(),
        "Redo Create track"
    );
}

#[test]
fn the_insert_track_keys_put_the_new_track_after_the_selected_track() {
    // Spec: app-shell R10.
    let mut shell = Shell::headless();
    let selected = shell.editor().document().top_level().next().unwrap();
    shell
        .run("arrangement.select_tracks", json!({ "tracks": [selected] }))
        .unwrap();
    shell
        .invoke("track.create", json!({ "kind": "audio" }))
        .unwrap();
    assert_eq!(
        track_names(&shell),
        ["1 MIDI", "2 Audio", "2 MIDI", "3 Audio", "4 Audio"]
    );
    let new = shell.editor().document().top_level().nth(1).unwrap();
    assert_eq!(shell.arrangement().selected_tracks(), &[new]);
}

#[test]
fn the_insert_track_keys_put_the_new_track_after_the_last_of_several_selected() {
    // Spec: app-shell R10.
    let mut shell = Shell::headless();
    let selected: Vec<TrackId> = shell.editor().document().top_level().take(3).collect();
    shell
        .run("arrangement.select_tracks", json!({ "tracks": selected }))
        .unwrap();
    shell
        .invoke("track.create", json!({ "kind": "midi" }))
        .unwrap();
    assert_eq!(
        track_names(&shell),
        ["1 MIDI", "2 MIDI", "3 Audio", "4 MIDI", "4 Audio"]
    );
    let new = shell.editor().document().top_level().nth(3).unwrap();
    assert_eq!(shell.arrangement().selected_tracks(), &[new]);
}

#[test]
fn the_insert_track_key_puts_the_new_return_after_the_selected_return() {
    // Spec: app-shell R10.
    let mut shell = Shell::headless();
    let selected = shell.editor().document().returns().last().unwrap();
    shell
        .run("arrangement.select_tracks", json!({ "tracks": [selected] }))
        .unwrap();
    shell
        .invoke("track.create", json!({ "kind": "return" }))
        .unwrap();
    let doc = shell.editor().document();
    let names: Vec<String> = doc
        .returns()
        .map(|r| doc.track(r).unwrap().name().to_owned())
        .collect();
    assert_eq!(names, ["A Return", "B Return", "C Return"]);
    let new = doc.returns().next_back().unwrap();
    assert_eq!(shell.arrangement().selected_tracks(), &[new]);
}

#[test]
fn the_insert_track_keys_add_after_the_last_track_with_none_selected() {
    // Spec: app-shell R10.
    let mut shell = Shell::headless();
    let first = shell.editor().document().top_level().next().unwrap();
    shell
        .run("arrangement.select_tracks", json!({ "tracks": [first] }))
        .unwrap();
    shell
        .run("arrangement.select_tracks", json!({ "tracks": [] }))
        .unwrap();
    shell
        .invoke("track.create", json!({ "kind": "audio" }))
        .unwrap();
    assert_eq!(
        track_names(&shell),
        ["1 MIDI", "2 MIDI", "3 Audio", "4 Audio", "5 Audio"]
    );
    let new = shell.editor().document().top_level().next_back().unwrap();
    assert_eq!(shell.arrangement().selected_tracks(), &[new]);
}

#[test]
fn an_audio_insert_with_returns_in_the_selection_goes_after_the_last_other_selected_track() {
    // Spec: app-shell R10. The new track goes after the last selected track
    // in the list it belongs to, so a selected return can't anchor an audio
    // track.
    let mut shell = Shell::headless();
    let doc = shell.editor().document();
    let audio = doc.top_level().nth(2).unwrap();
    let return_track = doc.returns().next().unwrap();
    shell
        .run(
            "arrangement.select_tracks",
            json!({ "tracks": [audio, return_track] }),
        )
        .unwrap();
    shell
        .invoke("track.create", json!({ "kind": "audio" }))
        .unwrap();
    assert_eq!(
        track_names(&shell),
        ["1 MIDI", "2 MIDI", "3 Audio", "4 Audio", "4 Audio"]
    );
    let new = shell.editor().document().top_level().nth(3).unwrap();
    assert_eq!(shell.arrangement().selected_tracks(), &[new]);
}

#[test]
fn quitting_or_starting_over_with_unsaved_changes_asks_first() {
    // Spec: app-shell R4.
    let mut shell = Shell::headless();
    shell.invoke("app.quit", json!({})).unwrap();
    assert_eq!(shell.take_effects(), [Effect::Quit], "a clean set quits");

    shell
        .invoke("track.create", json!({ "kind": "midi" }))
        .unwrap();
    for id in ["app.quit", "set.new", "set.open"] {
        shell.invoke(id, json!({})).unwrap();
        let effects = shell.take_effects();
        assert_eq!(
            effects,
            [Effect::ConfirmDiscard {
                then: Invocation::new(id, json!({}))
            }],
            "{id}"
        );
    }
    // "Don't Save" before opening asks for the file first, and the changes
    // stay until one is chosen, so cancelling the dialog loses nothing.
    shell
        .discard_and_invoke(&Invocation::new("set.open", json!({})))
        .unwrap();
    assert_eq!(
        shell.take_effects(),
        [Effect::PickOpenPath {
            discard_changes: true
        }]
    );
    assert!(shell.is_dirty());
    assert_eq!(track_names(&shell).len(), 5);
    // "Don't Save" before a new set discards, then runs it without asking.
    shell
        .discard_and_invoke(&Invocation::new("set.new", json!({})))
        .unwrap();
    assert!(shell.take_effects().is_empty());
    assert!(!shell.is_dirty());
    assert_eq!(track_names(&shell).len(), 4);
    // An agent's run never asks.
    shell
        .invoke("track.create", json!({ "kind": "midi" }))
        .unwrap();
    shell.run("app.quit", json!({})).unwrap();
    assert_eq!(shell.take_effects(), [Effect::Quit]);
}

#[test]
fn saving_an_untitled_set_asks_for_a_file() {
    // Spec: app-shell R3.
    let mut shell = Shell::headless();
    shell.invoke("set.save", json!({})).unwrap();
    assert_eq!(shell.take_effects(), [Effect::PickSavePath { then: None }]);
    shell.invoke("set.save_as", json!({})).unwrap();
    assert_eq!(shell.take_effects(), [Effect::PickSavePath { then: None }]);
    // An agent must name the file.
    let error = shell.run("set.save", json!({})).unwrap_err();
    assert!(matches!(error, ShellError::Unavailable { .. }), "{error}");
}

/// A folder for one test's files, removed when the test ends.
struct TestDir(PathBuf);

impl TestDir {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "bl-shell-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    /// `name` under the folder, its `/`-separated parts joined with the
    /// platform's separator, so Windows spells it the way Beatlayer does.
    fn path(&self, name: &str) -> String {
        name.split('/')
            .fold(self.0.clone(), |path, part| path.join(part))
            .to_string_lossy()
            .into_owned()
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn sets_round_trip_through_project_files_headless() {
    // Spec: app-shell R2, R3, R4, R17. D17: driven through Shell::run, as
    // an agent drives it, on bl-project's real set files.
    let dir = TestDir::new("round-trip");
    let song = dir.path("Demo Project/Demo.blset");
    let mut shell = Shell::headless();
    shell
        .invoke("track.create", json!({ "kind": "audio", "name": "Bass" }))
        .unwrap();
    // The first save makes the set's project folder.
    let saved = shell
        .run("set.save_as", json!({ "path": dir.path("Demo.blset") }))
        .unwrap();
    assert_eq!(saved, json!({ "path": song, "name": "Demo" }));
    assert!(!shell.is_dirty());
    assert_eq!(bl_project::load(&song).unwrap().1.revision, 1);

    // A second save is the next revision, and backs up the first.
    shell
        .invoke("track.create", json!({ "kind": "midi", "name": "Keys" }))
        .unwrap();
    shell.run("set.save", json!({})).unwrap();
    let (project, info) = bl_project::load(&song).unwrap();
    assert_eq!(info.revision, 2);
    assert_eq!(project.document, *shell.editor().document());
    let backups = std::fs::read_dir(dir.0.join("Demo Project/Backup"))
        .unwrap()
        .count();
    assert_eq!(backups, 1, "the replaced revision is kept");

    // A new set, then the saved one opens exactly as it was saved.
    let before = shell.editor().document().clone();
    shell.run("set.new", json!({})).unwrap();
    assert_eq!(track_names(&shell).len(), 4);
    assert_eq!(shell.set_path(), None);
    let opened = shell.run("set.open", json!({ "path": song })).unwrap();
    assert_eq!(opened["name"], "Demo");
    assert_eq!(*shell.editor().document(), before);
    assert_eq!(
        track_names(&shell),
        ["1 MIDI", "2 MIDI", "3 Audio", "4 Audio", "Bass", "Keys"]
    );
    assert!(!shell.editor().can_undo(), "an opened set has no history");

    // A project folder with one set opens that set.
    shell.run("set.new", json!({})).unwrap();
    let folder = dir.path("Demo Project");
    let opened = shell.run("set.open", json!({ "path": folder })).unwrap();
    // Compared as paths: Windows writes the separator `song` spells as `/`
    // as `\`.
    assert_eq!(
        Path::new(opened["path"].as_str().unwrap()),
        Path::new(&song)
    );

    // Saving after opening carries the revision count on.
    shell.run("set.save", json!({})).unwrap();
    assert_eq!(bl_project::load(&song).unwrap().1.revision, 3);
}

#[test]
fn opening_never_loses_unsaved_changes_without_an_explicit_discard() {
    // Finding 7 of the WP-1.7 review: set.open is external, and losing work
    // is its own, destructive step (set.discard_changes).
    let dir = TestDir::new("discard");
    let mut shell = Shell::headless();
    let saved = shell
        .run("set.save_as", json!({ "path": dir.path("Song.blset") }))
        .unwrap();
    let song = saved["path"].as_str().unwrap().to_owned();
    shell
        .invoke(
            "track.create",
            json!({ "kind": "audio", "name": "Unsaved" }),
        )
        .unwrap();
    assert!(shell.is_dirty());
    let error = shell.run("set.open", json!({ "path": song })).unwrap_err();
    assert!(matches!(error, ShellError::Unavailable { .. }), "{error}");
    assert!(error.to_string().contains("set.discard_changes"), "{error}");
    assert_eq!(track_names(&shell).len(), 5, "nothing changed");

    // Discarding goes back to the last save, with an empty history.
    shell.run("set.discard_changes", json!({})).unwrap();
    assert!(!shell.is_dirty());
    assert_eq!(track_names(&shell).len(), 4);
    assert!(!shell.editor().can_undo());
    shell.run("set.open", json!({ "path": song })).unwrap();

    // An untitled set's changes go back to the factory default set.
    shell.run("set.new", json!({})).unwrap();
    shell
        .invoke("track.create", json!({ "kind": "midi", "name": "Gone" }))
        .unwrap();
    shell.run("set.discard_changes", json!({})).unwrap();
    assert_eq!(
        track_names(&shell),
        ["1 MIDI", "2 MIDI", "3 Audio", "4 Audio"]
    );

    // The permission classes say so.
    let class = |id: &str| shell.registry().get(id).unwrap().permission;
    assert_eq!(class("set.open"), Some(Permission::External));
    assert_eq!(class("set.discard_changes"), Some(Permission::Destructive));
    assert_eq!(class("set.new"), Some(Permission::Destructive));
}

#[test]
fn files_that_cant_be_read_or_written_change_nothing_and_say_why() {
    // Spec: app-shell R2.
    let dir = TestDir::new("errors");
    let mut shell = Shell::headless();
    let missing = dir.path("Missing.blset");
    let error = shell
        .invoke("set.open", json!({ "path": missing }))
        .unwrap_err();
    assert!(matches!(error, ShellError::Failed { .. }), "{error}");
    assert!(shell.message().unwrap().error, "the Status Bar shows it");
    assert_eq!(shell.set_name(), "Untitled");

    let not_a_set = dir.path("Notes.blset");
    std::fs::write(&not_a_set, "just text").unwrap();
    let error = shell
        .run("set.open", json!({ "path": not_a_set }))
        .unwrap_err();
    assert!(
        error.to_string().contains("isn't a Beatlayer set"),
        "{error}"
    );
    assert_eq!(track_names(&shell).len(), 4);

    // A path whose folder is a file can't be written.
    let blocked = format!("{not_a_set}/Song.blset");
    let error = shell
        .run("set.save_as", json!({ "path": blocked }))
        .unwrap_err();
    assert!(matches!(error, ShellError::Failed { .. }), "{error}");
    assert_eq!(shell.set_path(), None);
}

/// A set store that keeps sets in memory, by path.
#[derive(Clone, Default)]
struct MemoryStore(Rc<RefCell<Vec<(PathBuf, usize)>>>);

impl SetStore for MemoryStore {
    fn open(&mut self, path: &Path) -> Result<(Editor, PathBuf), String> {
        let saved = self.0.borrow();
        let (_, tracks) = saved
            .iter()
            .find(|(p, _)| p == path)
            .ok_or_else(|| format!("no set at {}", path.display()))?;
        let mut editor = bl_shell::files::factory_default_set();
        for _ in 4..*tracks {
            let id: bl_model::TrackId = editor.new_id();
            bl_model::registry::run(
                &mut editor,
                "track.create",
                json!({ "id": id, "kind": "audio", "name": "Loaded", "index": 4 }),
            )
            .unwrap();
        }
        editor.clear_history().unwrap();
        Ok((editor, path.to_owned()))
    }

    fn save(&mut self, path: &Path, editor: &Editor) -> Result<Vec<String>, String> {
        let tracks = editor.document().top_level().len();
        let mut saved = self.0.borrow_mut();
        saved.retain(|(p, _)| p != path);
        saved.push((path.to_owned(), tracks));
        Ok(Vec::new())
    }

    /// Saves exactly where asked: project folders are `ProjectFiles`'s.
    fn save_as(
        &mut self,
        chosen: &Path,
        editor: &Editor,
    ) -> Result<(PathBuf, Vec<String>), String> {
        let warnings = self.save(chosen, editor)?;
        Ok((chosen.to_owned(), warnings))
    }

    fn close(&mut self) {}
}

#[test]
fn save_as_then_save_then_open_go_through_the_store() {
    // Spec: app-shell R2, R3.
    let store = MemoryStore::default();
    let mut shell = Shell::new(
        Preferences::default(),
        None,
        AudioHost::null(Default::default()),
        Box::new(store.clone()),
    );
    shell
        .invoke("track.create", json!({ "kind": "audio" }))
        .unwrap();
    let saved = shell
        .run("set.save_as", json!({ "path": "/songs/Demo.blset" }))
        .unwrap();
    assert_eq!(
        saved,
        json!({ "path": "/songs/Demo.blset", "name": "Demo" })
    );
    assert!(!shell.is_dirty());
    assert_eq!(shell.set_name(), "Demo");
    shell
        .invoke("track.create", json!({ "kind": "audio" }))
        .unwrap();
    shell.invoke("set.save", json!({})).unwrap();
    assert!(
        shell.take_effects().is_empty(),
        "a titled set saves in place"
    );
    assert_eq!(store.0.borrow()[0], (PathBuf::from("/songs/Demo.blset"), 6));

    shell.run("set.new", json!({})).unwrap();
    shell
        .run("set.open", json!({ "path": "/songs/Demo.blset" }))
        .unwrap();
    assert_eq!(track_names(&shell).len(), 6);
    assert_eq!(shell.set_name(), "Demo");
    let error = shell
        .run("set.open", json!({ "path": "/songs/Missing.blset" }))
        .unwrap_err();
    assert!(matches!(error, ShellError::Failed { .. }));
    assert_eq!(shell.set_name(), "Demo", "a failed open changes nothing");
}

#[test]
fn view_commands_switch_the_view_and_zoom() {
    let mut shell = Shell::headless();
    let browser = shell.run("view.toggle_browser", json!({})).unwrap();
    assert_eq!(browser["browser"], json!(false));
    assert_eq!(shell.switch_state("view.toggle_browser"), Some(false));

    shell.run("view.full_screen", json!({})).unwrap();
    assert_eq!(shell.take_effects(), [Effect::SetFullScreen(true)]);

    // Every 10% (interface-zoom R3; crates/shell/tests/interface_zoom.rs
    // covers the rest).
    for expected in [1.1, 1.2, 1.3, 1.4, 1.5, 1.6, 1.7, 1.8, 1.9, 2.0, 2.0] {
        let result = shell.run("view.zoom_in", json!({})).unwrap();
        assert!((result["zoom"].as_f64().unwrap() - expected).abs() < 1e-6);
    }
    shell.run("view.zoom_reset", json!({})).unwrap();
    for expected in [0.9, 0.8, 0.7, 0.6, 0.5, 0.5] {
        shell.run("view.zoom_out", json!({})).unwrap();
        assert!((shell.zoom() - expected).abs() < 1e-6, "{}", shell.zoom());
    }
}

#[test]
fn the_info_view_toggle_works_like_the_browsers() {
    // Spec: info-view R2, R5 (WP-2.24): view state, not set state.
    let mut shell = Shell::headless();
    assert!(shell.info_view_visible(), "a new set opens with it shown");
    let result = shell.run("view.toggle_info_view", json!({})).unwrap();
    assert_eq!(result["info_view"], json!(false));
    assert_eq!(shell.switch_state("view.toggle_info_view"), Some(false));
    let result = shell.run("view.toggle_info_view", json!({})).unwrap();
    assert_eq!(result["info_view"], json!(true));
    assert!(shell.info_view_visible());
}

#[test]
fn focus_commands_move_the_focus_and_set_the_contexts() {
    // Spec: app-shell R11.
    let mut shell = Shell::headless();
    assert_eq!(
        shell.active_contexts(),
        [Context::Arrangement, Context::Global]
    );
    shell.run("focus.control_bar", json!({})).unwrap();
    assert_eq!(shell.focused_area(), Area::ControlBar);
    assert_eq!(shell.active_contexts(), [Context::Global]);
    assert_eq!(shell.take_effects(), [Effect::FocusArea(Area::ControlBar)]);
    assert!(
        !shell.can_invoke("focus.next", &json!({})),
        "Tab doesn't move focus by default"
    );

    shell.run("focus.toggle_tab_navigation", json!({})).unwrap();
    assert_eq!(
        shell.active_contexts(),
        [Context::TabFocus, Context::Global]
    );
    assert!(shell.can_invoke("focus.next", &json!({})));
    shell.run("focus.next", json!({})).unwrap();
    assert_eq!(
        shell.take_effects(),
        [Effect::MoveFocus {
            backwards: false,
            neighbor: false
        }]
    );

    shell
        .run("view.toggle_session_arrangement", json!({}))
        .unwrap();
    shell.run("focus.arrangement", json!({})).unwrap();
    assert_eq!(shell.main_view(), MainView::Arrangement);
    assert_eq!(shell.focused_area(), Area::MainView);
}

#[test]
fn the_settings_window_has_its_own_contexts_and_pages() {
    // Spec: app-shell R5.
    let mut shell = Shell::headless();
    assert!(!shell.can_invoke("window.close", &json!({})));
    assert!(shell.run("settings.next_page", json!({})).is_err());

    shell.invoke("app.settings", json!({})).unwrap();
    assert_eq!(shell.take_effects(), [Effect::OpenSettings]);
    assert!(shell.settings_open());
    assert_eq!(shell.focused_window(), WindowKind::Settings);
    assert_eq!(shell.settings_page(), SettingsPage::Audio);
    assert_eq!(
        shell.active_contexts(),
        [Context::Settings, Context::Dialog, Context::Global]
    );
    assert!(shell.can_invoke("window.close", &json!({})));
    shell.run("settings.next_page", json!({})).unwrap();
    assert_eq!(shell.settings_page(), SettingsPage::Link);
    shell.run("settings.previous_page", json!({})).unwrap();
    shell.run("settings.previous_page", json!({})).unwrap();
    assert_eq!(shell.settings_page(), SettingsPage::ThemeColors);
    shell
        .run("settings.set_theme", json!({ "theme": "light" }))
        .unwrap();
    assert_eq!(shell.appearance(), Appearance::Light);

    shell.invoke("window.close", json!({})).unwrap();
    assert!(!shell.settings_open());
    assert_eq!(shell.focused_window(), WindowKind::Main);
    shell
        .invoke("app.settings", json!({ "page": "plug-ins" }))
        .unwrap();
    assert_eq!(shell.settings_page(), SettingsPage::PlugIns);
}

#[test]
fn preferences_are_saved_and_read_back() {
    let dir = std::env::temp_dir().join(format!("bl-shell-commands-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut shell = Shell::new(
        Preferences::default(),
        Some(dir.clone()),
        AudioHost::null(Default::default()),
        Box::new(ProjectFiles::new()),
    );
    shell
        .run("settings.set_theme", json!({ "theme": "light" }))
        .unwrap();
    shell
        .run("audio.configure", json!({ "buffer_size": 512 }))
        .unwrap();
    shell.run("view.zoom_in", json!({})).unwrap();
    let prefs = Preferences::load(&dir).unwrap();
    assert_eq!(prefs.appearance, Appearance::Light);
    assert_eq!(prefs.audio.buffer_size, Some(512));
    assert_eq!(prefs.audio.driver.as_deref(), Some("null"));
    assert_eq!(prefs.zoom, Some(1.1));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn unknown_commands_and_bad_arguments_are_refused() {
    let mut shell = Shell::headless();
    assert!(matches!(
        shell.run("app.fly", json!({})),
        Err(ShellError::UnknownCommand(_))
    ));
    assert!(matches!(
        shell.run("app.settings", json!({ "page": "nowhere" })),
        Err(ShellError::InvalidArgs { .. })
    ));
    assert!(
        matches!(
            shell.run("view.zoom_in", json!({ "by": 2 })),
            Err(ShellError::InvalidArgs { .. })
        ),
        "a command without arguments refuses any, as its schema says"
    );
}

#[test]
fn group_and_ungroup_tracks_lifecycle() {
    let mut shell = Shell::headless();
    // Default set has: 1 MIDI, 2 MIDI, 3 Audio, 4 Audio, A Reverb, B Delay, Main
    let doc = shell.editor().document().clone();
    let initial_tracks = doc.track_order();
    assert_eq!(initial_tracks.len(), 7);
    let t3 = initial_tracks[2]; // 3 Audio
    let t4 = initial_tracks[3]; // 4 Audio

    // With nothing selected (default headless shell has empty selection), can_invoke for group/ungroup is false
    assert!(!shell.can_invoke("track.group", &json!({})));
    assert!(!shell.can_invoke("track.ungroup", &json!({})));

    // Select tracks 3 and 4 in arrangement
    shell
        .run("arrangement.select_tracks", json!({ "tracks": [t3, t4] }))
        .unwrap();

    // Group Tracks can invoke; Ungroup cannot
    assert!(shell.can_invoke("track.group", &json!({})));
    assert!(!shell.can_invoke("track.ungroup", &json!({})));

    // Run track.group
    let res = shell.run("track.group", json!({})).unwrap();
    let group_id: TrackId = serde_json::from_value(res["track"].clone()).unwrap();

    let doc = shell.editor().document();
    assert_eq!(doc.track(group_id).unwrap().name(), "Group");
    assert_eq!(
        doc.track(group_id).unwrap().kind(),
        bl_model::TrackKind::Group
    );
    // Inserts group track where the first selected track was
    assert_eq!(
        doc.track_order(),
        [
            initial_tracks[0],
            initial_tracks[1],
            group_id,
            t3,
            t4,
            initial_tracks[4],
            initial_tracks[5],
            initial_tracks[6]
        ]
    );
    // Routes each member to the group
    assert_eq!(doc.output_of(t3), Some(group_id));
    assert_eq!(doc.output_of(t4), Some(group_id));
    // Routes group to Main
    assert_eq!(doc.output_of(group_id), Some(doc.main_id()));
    // Leaves group unfolded
    assert!(!shell.arrangement().is_folded(group_id));
    // Selects the group track
    assert_eq!(shell.arrangement().selected_tracks(), [group_id]);

    // Now group track is selected: Ungroup can invoke; Group can also invoke (groups can be grouped)
    assert!(shell.can_invoke("track.ungroup", &json!({})));
    assert!(shell.can_invoke("track.group", &json!({})));

    // Single undo transaction named "Group Tracks"
    assert_eq!(shell.editor().undo_history().next(), Some("Group Tracks"));
    shell.run("edit.undo", json!({})).unwrap();
    assert_eq!(
        shell.editor().document(),
        &doc_before_group(&initial_tracks, &shell)
    );

    // Redo grouping
    shell.run("edit.redo", json!({})).unwrap();
    let doc = shell.editor().document();
    assert_eq!(
        doc.track_order(),
        [
            initial_tracks[0],
            initial_tracks[1],
            group_id,
            t3,
            t4,
            initial_tracks[4],
            initial_tracks[5],
            initial_tracks[6]
        ]
    );

    // Re-select group track and ungroup
    shell
        .run("arrangement.select_tracks", json!({ "tracks": [group_id] }))
        .unwrap();

    let ungroup_res = shell.run("track.ungroup", json!({})).unwrap();
    let members: Vec<TrackId> = serde_json::from_value(ungroup_res["tracks"].clone()).unwrap();
    assert_eq!(members, [t3, t4]);

    let doc = shell.editor().document();
    assert_eq!(doc.track_order(), initial_tracks);
    // Routes members back to Main
    assert_eq!(doc.output_of(t3), Some(doc.main_id()));
    assert_eq!(doc.output_of(t4), Some(doc.main_id()));
    // Selects the first member
    assert_eq!(shell.arrangement().selected_tracks(), [t3]);

    // Single undo transaction named "Ungroup Tracks"
    assert_eq!(shell.editor().undo_history().next(), Some("Ungroup Tracks"));

    // Undo Ungroup restores group exactly
    shell.run("edit.undo", json!({})).unwrap();
    let doc = shell.editor().document();
    assert_eq!(
        doc.track_order(),
        [
            initial_tracks[0],
            initial_tracks[1],
            group_id,
            t3,
            t4,
            initial_tracks[4],
            initial_tracks[5],
            initial_tracks[6]
        ]
    );
    assert_eq!(doc.output_of(t3), Some(group_id));

    // Agent API explicit args test: run with args
    shell.run("edit.redo", json!({})).unwrap();
    let group_res2 = shell
        .run(
            "track.group",
            json!({ "tracks": [initial_tracks[0], initial_tracks[1]] }),
        )
        .unwrap();
    let g2: TrackId = serde_json::from_value(group_res2["track"].clone()).unwrap();
    let ungroup_res2 = shell.run("track.ungroup", json!({ "track": g2 })).unwrap();
    let g2_members: Vec<TrackId> = serde_json::from_value(ungroup_res2["tracks"].clone()).unwrap();
    assert_eq!(g2_members, [initial_tracks[0], initial_tracks[1]]);
}

fn doc_before_group(initial_tracks: &[TrackId], shell: &Shell) -> bl_model::Document {
    let doc = shell.editor().document().clone();
    assert_eq!(doc.track_order(), initial_tracks);
    doc
}

#[test]
fn delete_and_rename_from_a_key_or_menu_act_on_the_selected_tracks() {
    // Spec: track-context-menu R4, R5, R8.
    let mut shell = Shell::headless();
    let order = shell.editor().document().track_order();
    let (t3, t4, main) = (order[2], order[3], order[6]);
    let headers = [Context::TrackHeaders, Context::Global];
    assert!(!shell.can_invoke_in("track.delete", &json!({}), &headers));
    assert!(!shell.can_invoke_in("track.rename", &json!({}), &headers));

    shell
        .run(
            "arrangement.select_tracks",
            json!({ "tracks": [t3, t4], "title_bars": true }),
        )
        .unwrap();
    assert!(shell.can_invoke("track.delete", &json!({})));
    // The Session View doesn't have the title bars' keys, but its title
    // bars' menu does.
    shell
        .run("view.toggle_session_arrangement", json!({}))
        .unwrap();
    assert!(!shell.can_invoke("track.delete", &json!({})));
    assert!(shell.can_invoke_in("track.delete", &json!({}), &shell.track_menu_contexts()));
    shell
        .run("view.toggle_session_arrangement", json!({}))
        .unwrap();

    // Rename asks the title bar for the name of the first selected track.
    shell.take_effects();
    shell.invoke("track.rename", json!({})).unwrap();
    assert!(matches!(shell.take_effects()[..], [Effect::RenameTrack(t)] if t == t3));
    shell
        .invoke("track.rename", json!({ "track": t3, "name": "Bass" }))
        .unwrap();
    assert_eq!(shell.editor().document().track(t3).unwrap().name(), "Bass");

    // Delete takes every selected track in one undo step.
    shell.invoke("track.delete", json!({})).unwrap();
    let doc = shell.editor().document();
    assert!(doc.track(t3).is_none() && doc.track(t4).is_none());
    assert_eq!(
        shell.editor().undo_history().next(),
        Some("Delete Track(s)")
    );
    shell.run("edit.undo", json!({})).unwrap();
    assert!(shell.editor().document().track(t3).is_some());
    assert!(shell.editor().document().track(t4).is_some());

    // Main can be neither deleted nor renamed.
    shell
        .run(
            "arrangement.select_tracks",
            json!({ "tracks": [main], "title_bars": true }),
        )
        .unwrap();
    assert!(!shell.can_invoke("track.delete", &json!({})));
    assert!(!shell.can_invoke("track.rename", &json!({})));
}

// Spec: themes R1, R3, R7, R12.
#[test]
fn the_theme_and_customization_commands_set_and_read_back_and_touch_no_set() {
    let mut shell = Shell::headless();
    let state =
        |shell: &mut Shell| shell.run("settings.state", json!({})).unwrap()["theme_colors"].clone();
    let before = state(&mut shell);
    assert_eq!(before["theme"], "Beatlayer");
    assert_eq!(before["appearance"], "dark");
    assert_eq!(before["tone"], "neutral");
    assert_eq!(before["high_contrast"], false);
    assert_eq!(
        [
            before["grid_line_intensity"].as_i64(),
            before["brightness"].as_i64(),
            before["color_intensity"].as_i64(),
            before["color_hue"].as_i64()
        ],
        [Some(100), Some(0), Some(100), Some(0)]
    );
    for (command, args, field, value) in [
        (
            "settings.set_appearance",
            json!({ "appearance": "light" }),
            "appearance",
            json!("light"),
        ),
        (
            "settings.set_tone",
            json!({ "tone": "cool" }),
            "tone",
            json!("cool"),
        ),
        (
            "settings.set_high_contrast",
            json!({ "on": true }),
            "high_contrast",
            json!(true),
        ),
        (
            "settings.set_grid_line_intensity",
            json!({ "percent": 35 }),
            "grid_line_intensity",
            json!(35),
        ),
        (
            "settings.set_brightness",
            json!({ "value": -12 }),
            "brightness",
            json!(-12),
        ),
        (
            "settings.set_color_intensity",
            json!({ "percent": 140 }),
            "color_intensity",
            json!(140),
        ),
        (
            "settings.set_color_hue",
            json!({ "degrees": 65 }),
            "color_hue",
            json!(65),
        ),
    ] {
        shell.run(command, args).unwrap();
        assert_eq!(state(&mut shell)[field], value, "{command}");
    }
    assert!(!shell.is_dirty(), "a preference isn't part of the set");
    assert!(!shell.editor().can_undo(), "and isn't an undo step");
    // Out of range is refused, and an unknown theme is named in the refusal.
    shell.invoke("app.settings", json!({})).unwrap();
    assert!(!shell.can_invoke("settings.set_brightness", &json!({ "value": 51 })));
    assert!(!shell.can_invoke("settings.set_color_hue", &json!({ "degrees": 181 })));
    assert!(shell.can_invoke("settings.set_color_hue", &json!({ "degrees": -180 })));
    let refused = shell
        .run("settings.set_theme", json!({ "theme": "Nope" }))
        .unwrap_err();
    assert!(refused.to_string().contains("Beatlayer"), "{refused}");
    // The values `dark` and `light` set the Appearance (R12).
    shell
        .run("settings.set_theme", json!({ "theme": "dark" }))
        .unwrap();
    assert_eq!(state(&mut shell)["appearance"], "dark");
    assert_eq!(state(&mut shell)["theme"], "Beatlayer");
}

// Spec: themes R3.
#[test]
fn follow_system_draws_the_systems_mode_as_it_changes() {
    let mut shell = Shell::headless();
    shell
        .run(
            "settings.set_appearance",
            json!({ "appearance": "follow-system" }),
        )
        .unwrap();
    for dark in [false, true, false] {
        shell.set_system_dark(dark);
        assert_eq!(shell.theme_dark(), dark);
        let state = shell.run("settings.state", json!({})).unwrap();
        assert_eq!(
            state["theme_colors"]["drawn_appearance"],
            if dark { "dark" } else { "light" }
        );
        assert_eq!(state["theme_colors"]["appearance"], "follow-system");
    }
    // Light and Dark ignore the system.
    shell
        .run("settings.set_appearance", json!({ "appearance": "dark" }))
        .unwrap();
    assert!(shell.theme_dark());
}

// Spec: themes R11.
#[test]
fn preferences_saved_before_the_page_keep_their_look() {
    let light: Preferences =
        serde_json::from_str(r#"{"theme": "light", "tab_navigation": true}"#).unwrap();
    assert_eq!(light.appearance, Appearance::Light);
    assert_eq!(light.look, Default::default());
    let dark: Preferences = serde_json::from_str(r#"{"theme": "dark"}"#).unwrap();
    assert_eq!(dark.appearance, Appearance::Dark);
    let saved = serde_json::to_value(&light).unwrap();
    assert_eq!(saved["appearance"], "light");
    assert!(saved.get("theme").is_none());
}

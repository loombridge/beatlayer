//! The shell's commands, driven headless the way the menus, the keymap, and
//! agents drive them (D17).

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use bl_commands::Context;
use bl_model::Editor;
use bl_shell::audio::AudioHost;
use bl_shell::files::{NO_SET_FILES, SetStore};
use bl_shell::settings::{Preferences, SettingsPage, ThemeChoice};
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
    // "Don't Save" runs the command without asking again.
    shell
        .discard_and_invoke(&Invocation::new("set.open", json!({})))
        .unwrap();
    assert_eq!(shell.take_effects(), [Effect::PickOpenPath]);
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

#[test]
fn without_project_files_open_and_save_report_why() {
    let mut shell = Shell::headless();
    let error = shell
        .invoke("set.save_as", json!({ "path": "/tmp/Song.blset" }))
        .unwrap_err();
    assert!(error.to_string().contains(NO_SET_FILES), "{error}");
    assert!(shell.message().unwrap().error, "the Status Bar shows it");
    assert_eq!(shell.set_path(), None);
}

/// A set store that keeps sets in memory, by path.
#[derive(Clone, Default)]
struct MemoryStore(Rc<RefCell<Vec<(PathBuf, usize)>>>);

impl SetStore for MemoryStore {
    fn load(&self, path: &Path) -> Result<Editor, String> {
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
        Ok(editor)
    }

    fn save(&self, path: &Path, editor: &Editor) -> Result<(), String> {
        let tracks = editor.document().top_level().len();
        let mut saved = self.0.borrow_mut();
        saved.retain(|(p, _)| p != path);
        saved.push((path.to_owned(), tracks));
        Ok(())
    }
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

    for expected in [1.1, 1.25, 1.5, 1.75, 2.0, 2.0] {
        let result = shell.run("view.zoom_in", json!({})).unwrap();
        assert_eq!(result["zoom"].as_f64().unwrap() as f32, expected);
    }
    shell.run("view.zoom_reset", json!({})).unwrap();
    for expected in [0.9, 0.8, 0.75, 0.67, 0.5, 0.5] {
        shell.run("view.zoom_out", json!({})).unwrap();
        assert!((shell.zoom() - expected).abs() < 1e-6, "{}", shell.zoom());
    }
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
        [Effect::MoveFocus { backwards: false }]
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
    assert_eq!(shell.theme(), ThemeChoice::Light);

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
        Box::new(bl_shell::files::NoSetFiles),
    );
    shell
        .run("settings.set_theme", json!({ "theme": "light" }))
        .unwrap();
    shell
        .run("audio.configure", json!({ "buffer_size": 512 }))
        .unwrap();
    shell.run("view.zoom_in", json!({})).unwrap();
    let prefs = Preferences::load(&dir).unwrap();
    assert_eq!(prefs.theme, ThemeChoice::Light);
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

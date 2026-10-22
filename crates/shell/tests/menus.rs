//! The menu bar against the command registry: every item reaches a
//! registered command or is marked pending, and items enable with what can
//! run now (D16, D17). Its rows, separators, and labels are checked against
//! the menu reference in `bl-shell-spec`.
//!
//! Spec: export R16.

use std::collections::BTreeSet;
use std::path::PathBuf;

use bl_commands::Registry;
use bl_shell::menus::{Action, MenuPlatform, menu_bar};
use bl_shell::{Shell, commands};
use serde_json::json;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// A file of the UI parity reference, or `None` in a checkout without the
/// planning documents (they aren't published yet), where the test skips.
fn ui_parity(file: &str) -> Option<String> {
    let docs = repo().join("docs");
    if !docs.exists() {
        eprintln!(
            "skipped: the UI parity reference isn't published yet, and this checkout doesn't have it"
        );
        return None;
    }
    Some(std::fs::read_to_string(docs.join("ui-parity").join(file)).unwrap())
}

// Spec: app-shell R15.
#[test]
fn every_item_reaches_a_command_or_is_pending() {
    let mut registry = Registry::new();
    bl_model::registry::register(&mut registry).unwrap();
    // The shell's commands, the Arrangement View's, and the browser's.
    bl_shell::register(&mut registry).unwrap();
    // The transport's commands, which the Playback menu runs (WP-1.18).
    bl_engine::transport::register(&mut registry).unwrap();
    let Some(shortcuts) = ui_parity("shortcuts.toml") else {
        return;
    };
    for platform in [MenuPlatform::Mac, MenuPlatform::Other] {
        for menu in menu_bar(platform) {
            for item in menu.walk() {
                match item.action {
                    Action::Command(id) => assert!(registry.contains(id), "{}: {id}", item.row),
                    Action::Choice(id, args) => {
                        assert!(registry.contains(id), "{}: {id}", item.row);
                        serde_json::from_str::<serde_json::Value>(args).expect(item.row);
                    }
                    Action::Shortcut(ids) => {
                        assert!(!ids.is_empty());
                        for id in ids {
                            assert!(
                                shortcuts.contains(&format!("id = \"{id}\"")),
                                "{}: {id} isn't in shortcuts.toml",
                                item.row
                            );
                        }
                    }
                    Action::Pending(Some(id)) => {
                        assert!(shortcuts.contains(&format!("id = \"{id}\"")), "{id}");
                    }
                    Action::Submenu => assert!(
                        !item.items.is_empty() || bl_shell::menus::DYNAMIC_ROWS.contains(&item.row),
                        "{}",
                        item.row
                    ),
                    Action::Pending(None)
                    | Action::System(_)
                    | Action::Omitted(_)
                    | Action::Separator => {}
                }
                if item.is_shown() {
                    assert!(!item.label.contains("Live"), "{}: {}", item.row, item.label);
                }
            }
        }
    }
}

#[test]
fn every_shell_command_is_on_a_menu_or_a_control() {
    // Commands reached only from the Settings window's controls, the
    // Status Bar, the save-changes dialog (Don't Save), or agents.
    let elsewhere: BTreeSet<&str> = [
        commands::SET_DISCARD_CHANGES,
        commands::SETTINGS_NEXT_PAGE,
        commands::SETTINGS_PREVIOUS_PAGE,
        // The Theme & Colors page's Theme and Customization sections
        // (themes R12).
        commands::SETTINGS_SET_THEME,
        commands::SETTINGS_SET_APPEARANCE,
        commands::SETTINGS_SET_TONE,
        commands::SETTINGS_SET_HIGH_CONTRAST,
        commands::SETTINGS_SET_GRID_LINE_INTENSITY,
        commands::SETTINGS_SET_BRIGHTNESS,
        commands::SETTINGS_SET_COLOR_INTENSITY,
        commands::SETTINGS_SET_COLOR_HUE,
        // The Record, Warp & Launch page's switch (the audio clips spec,
        // R22).
        commands::SETTINGS_SET_CREATE_FADES_ON_CLIP_EDGES,
        // The File & Folder page's Temporary Folder (the audio clip editing
        // spec, R5).
        commands::SETTINGS_SET_TEMPORARY_FOLDER,
        // The same page's Warp recorded audio switch (the recording spec,
        // R13).
        commands::SETTINGS_SET_WARP_RECORDED_AUDIO,
        commands::SETTINGS_SET_LOOP_WARP_SHORT_SAMPLES,
        commands::SETTINGS_SET_AUTO_WARP_LONG_SAMPLES,
        commands::SETTINGS_SET_DEFAULT_WARP_MODE,
        // The same page's Launch section (the launch quantisation spec,
        // R15).
        commands::SETTINGS_SET_DEFAULT_LAUNCH_MODE,
        commands::SETTINGS_SET_DEFAULT_LAUNCH_QUANTIZATION,
        commands::SETTINGS_SET_SELECT_ON_LAUNCH,
        commands::SETTINGS_SET_SELECT_NEXT_SCENE_ON_LAUNCH,
        commands::SETTINGS_SET_START_RECORDING_ON_SCENE_LAUNCH,
        // The same page's Record Session automation in chooser (the Session
        // automation spec, R5).
        commands::SETTINGS_SET_RECORD_SESSION_AUTOMATION_IN,
        // The Plug-Ins page's switch (the device chains spec, R31).
        commands::SETTINGS_SET_AUTO_HIDE_PLUG_IN_WINDOWS,
        // The Display & Input page's Zoom (interface-zoom R4).
        commands::VIEW_SET_ZOOM,
        // The Display & Input page's Outline View in Focus (the
        // accessibility spec, R7).
        commands::SETTINGS_SET_OUTLINE_VIEW_IN_FOCUS,
        commands::AUDIO_DEVICES,
        commands::AUDIO_STATUS,
        commands::AUDIO_CONFIGURE,
        commands::AUDIO_SET_TEST_TONE,
        // The File Manager's Automatic Search, which Manage Files opens
        // (M-FILE-007); it also runs by itself when a set is opened
        // (WP-2.25).
        commands::SET_RELINK_MISSING,
        // The MIDI Note Editor's lane and fold (WP-2.3): the View menu's
        // Fold to Notes reaches view.fold_notes through its key (M-VIEW-039,
        // S-NOTE-038), the lane header's own menu reaches view.lane
        // (L-CLIP-014), and agents reach both. The footer's Lane Selector,
        // its Show/Hide All switch and the lane header's Swap Lane reach
        // view.lanes (note-editing R1, R2).
        commands::VIEW_FOLD_NOTES,
        commands::VIEW_LANE,
        commands::VIEW_LANES,
        // The Clip View's Envelopes tab (WP-3.16): the View menu's Content
        // Editor and Envelope Editor reach view.clip_tab through their keys
        // (M-VIEW-018, M-VIEW-019, S-CLIPMODE-003, S-CLIPMODE-004), and the
        // control chooser reaches view.clip_envelope (L-CLIP-017).
        commands::VIEW_CLIP_TAB,
        commands::VIEW_CLIP_ENVELOPE,
        // The list of the set's plug-ins' Show Only Missing switch (the
        // plug-in list spec, R3), which the list's own switch reaches.
        commands::VIEW_SET_PLUGIN_LIST_ONLY_MISSING,
        // A query for agents, like `transport.state`.
        commands::VIEW_STATE,
        // The Shortcuts page's buttons (the custom shortcuts spec, R8 and
        // R10), and the query for agents.
        commands::KEYMAP_BIND,
        commands::KEYMAP_UNBIND,
        commands::KEYMAP_RESET,
        commands::KEYMAP_STATE,
        // The File & Folder page's Sample Editor row (the sample editor
        // spec, R16), the Audio Utilities panel's Edit button (R16), and the
        // file dropped on the Clip View that replaces a clip's sample (R18).
        commands::SETTINGS_SET_SAMPLE_EDITOR,
        bl_shell::audio_panel_commands::EDIT_SAMPLE,
        bl_shell::audio_panel_commands::REPLACE_SAMPLE,
    ]
    .into();
    let bar = menu_bar(MenuPlatform::Mac);
    let on_menus: BTreeSet<&str> = bar
        .iter()
        .flat_map(|menu| menu.walk().filter_map(|item| item.command()))
        .collect();
    for id in commands::IDS {
        assert!(
            on_menus.contains(id) || elsewhere.contains(id),
            "{id} is on no menu"
        );
    }
}

// Spec: app-shell R16.
#[test]
fn menu_items_enable_with_what_can_run_now() {
    let mut shell = Shell::headless();
    assert!(shell.can_invoke("set.new", &json!({})));
    assert!(shell.can_invoke("app.settings", &json!({})));
    assert!(
        !shell.can_invoke("edit.undo", &json!({})),
        "nothing to undo"
    );
    assert!(
        !shell.can_invoke("window.close", &json!({})),
        "no secondary window"
    );
    assert!(
        !shell.can_invoke("track.delete", &json!({})),
        "deleting needs a selected track"
    );
    assert!(
        !shell.can_invoke("set.discard_changes", &json!({})),
        "nothing to discard"
    );
    shell
        .invoke("track.create", json!({ "kind": "midi" }))
        .unwrap();
    assert!(shell.can_invoke("edit.undo", &json!({})));
    assert!(shell.can_invoke("set.discard_changes", &json!({})));

    // Close Window (Cmd/Ctrl-W, S-FILE-009) acts on a secondary window in
    // front, and is disabled with the main window in front.
    shell.run("app.settings", json!({})).unwrap();
    assert!(shell.can_invoke("window.close", &json!({})));
    shell.set_focused_window(bl_shell::WindowKind::Main);
    assert!(!shell.can_invoke("window.close", &json!({})));
}

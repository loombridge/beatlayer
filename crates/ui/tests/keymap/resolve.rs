//! Keys resolve to commands in their contexts, per platform.

use bl_commands::Context;
use bl_ui::keymap::{Keymap, Mods, Platform, Status};
use egui::Key;

use crate::keymap::helpers::*;

#[test]
fn keys_resolve_to_commands_in_their_contexts() {
    // Spec: app-shell R16.
    let keymap = Keymap::live12();
    let main = [Context::Arrangement, Context::Global];
    let resolve = |platform, key, m, contexts: &[Context]| {
        keymap
            .resolve(platform, key, m, contexts)
            .map(|hit| hit.command.id.clone())
    };
    assert_eq!(
        resolve(
            Platform::Mac,
            Key::S,
            mods(true, false, false, false),
            &main
        )
        .as_deref(),
        Some("set.save")
    );
    assert_eq!(
        resolve(
            Platform::Other,
            Key::S,
            mods(false, true, false, false),
            &main
        )
        .as_deref(),
        Some("set.save")
    );
    assert_eq!(
        resolve(
            Platform::Mac,
            Key::S,
            mods(false, true, false, false),
            &main
        ),
        None,
        "Ctrl+S isn't Save on macOS"
    );
    assert_eq!(
        resolve(
            Platform::Mac,
            Key::Comma,
            mods(true, false, false, false),
            &main
        )
        .as_deref(),
        Some("app.settings")
    );
    assert_eq!(
        resolve(
            Platform::Other,
            Key::Y,
            mods(false, true, false, false),
            &main
        )
        .as_deref(),
        Some("edit.redo")
    );
    assert_eq!(
        resolve(
            Platform::Mac,
            Key::Equals,
            mods(true, false, false, false),
            &main
        )
        .as_deref(),
        Some("view.zoom_in")
    );
    assert_eq!(
        resolve(Platform::Mac, Key::B, mods(true, false, true, false), &main).as_deref(),
        Some("view.toggle_browser")
    );
    assert_eq!(
        resolve(
            Platform::Mac,
            Key::Num0,
            mods(false, false, true, false),
            &main
        )
        .as_deref(),
        Some("focus.control_bar")
    );
    // Tab switches views unless Tab moves the focus.
    assert_eq!(
        resolve(Platform::Mac, Key::Tab, Mods::default(), &main).as_deref(),
        Some("view.toggle_session_arrangement")
    );
    let tab_focus = [Context::Arrangement, Context::TabFocus, Context::Global];
    assert_eq!(
        resolve(Platform::Mac, Key::Tab, Mods::default(), &tab_focus).as_deref(),
        Some("focus.next")
    );
    // The same keys run different commands in different places: in the
    // Arrangement, Delete deletes the selection, clips or time (WP-1.8b).
    assert_eq!(
        resolve(Platform::Mac, Key::Delete, Mods::default(), &main).as_deref(),
        Some("arrangement.delete")
    );
    assert_eq!(
        resolve(
            Platform::Mac,
            Key::Delete,
            Mods::default(),
            &[Context::DeviceView, Context::Global]
        )
        .as_deref(),
        Some("device.remove")
    );
    let settings = [Context::Settings, Context::Dialog, Context::Global];
    assert_eq!(
        resolve(Platform::Mac, Key::Escape, Mods::default(), &settings).as_deref(),
        Some("window.close")
    );
    assert_eq!(
        resolve(Platform::Mac, Key::Tab, Mods::default(), &settings).as_deref(),
        Some("focus.next")
    );
    assert_eq!(
        resolve(
            Platform::Mac,
            Key::Tab,
            mods(false, false, true, false),
            &settings
        )
        .as_deref(),
        Some("settings.next_page")
    );
    assert_eq!(
        resolve(
            Platform::Other,
            Key::Tab,
            mods(false, true, false, false),
            &settings
        )
        .as_deref(),
        Some("settings.next_page")
    );
    // Esc closes nothing in the main window: in the Arrangement it goes to
    // the highlighted track's title bar (S-ARR-034, WP-1.8a).
    assert_eq!(
        resolve(Platform::Mac, Key::Escape, Mods::default(), &main).as_deref(),
        Some("arrangement.focus_title_bar"),
        "Esc closes nothing in the main window"
    );
    assert_eq!(
        resolve(
            Platform::Mac,
            Key::Escape,
            Mods::default(),
            &[Context::Global]
        ),
        None,
        "Esc closes nothing in the main window"
    );
    // Cmd/Ctrl-W closes the frontmost window (S-FILE-009); with only the
    // main window open the shell disables it.
    for (platform, m) in [
        (Platform::Mac, mods(true, false, false, false)),
        (Platform::Other, mods(false, true, false, false)),
    ] {
        assert_eq!(
            resolve(platform, Key::W, m, &settings).as_deref(),
            Some("window.close")
        );
    }
}

#[test]
fn platform_items_and_corrected_keys_follow_the_reference() {
    let keymap = Keymap::live12();
    // macOS's standard menus provide Hide Others and Emoji & Symbols with
    // their keys.
    for id in ["S-FILE-010", "S-MENU-006"] {
        let entry = keymap.entry_for(id).unwrap();
        assert!(matches!(entry.status, Status::System(_)), "{id}");
    }
    // The video window's keys as Live 12.4.3's View menu shows them, not
    // the manual's (S-VIEW-009, corrected in WP-0.13b). It comes in M9.
    let video = keymap.entry_for("S-VIEW-009").unwrap();
    assert_eq!(video.mac, ["Cmd+Option+Shift+V"]);
    assert_eq!(video.win, ["Ctrl+Alt+Shift+V"]);
    assert!(matches!(video.status, Status::Pending { .. }));
}

//! The browser tags, detail views and track headers shortcuts.

use bl_commands::Context;
use bl_ui::keymap::{Keymap, Platform, Status};
use egui::Key;

use crate::keymap::helpers::*;

#[test]
fn the_tags_shortcuts_reach_their_commands() {
    // Spec: browser-tags R10 and R14.
    let keymap = Keymap::live12();
    // Cmd-Shift-E shows and hides the Tag Editor (S-BRW-014).
    let entry = keymap.entry_for("S-BRW-014").expect("the shortcut");
    assert_eq!(entry.status, Status::Bound);
    assert_eq!(entry.mac, vec!["Cmd+Shift+E".to_owned()]);
    assert_eq!(entry.win, vec!["Ctrl+Shift+E".to_owned()]);
    assert!(
        entry
            .commands
            .iter()
            .any(|command| command.id == "browser.show_tag_editor"),
        "{:?}",
        entry.commands
    );
    // Cmd-E moves the keyboard focus to Quick Tags' Add field
    // (S-BRW-015): the browser's own widget reads it.
    let entry = keymap.entry_for("S-BRW-015").expect("the shortcut");
    assert!(
        matches!(entry.status, Status::Widget(_)),
        "{:?}",
        entry.status
    );
    assert_eq!(entry.mac, vec!["Cmd+E".to_owned()]);
    assert_eq!(entry.win, vec!["Ctrl+E".to_owned()]);
}

#[test]
fn the_collection_keys_and_rename_reach_the_browser() {
    // Spec: browser-tags R35 and R38. The keys 1 to 7 and 0 are the
    // browser's list's own (S-BRW-008, S-BRW-009), and Cmd-R (Ctrl-R)
    // renames the chosen label, a collection among them, while the sidebar
    // has the keyboard (S-EDIT-008).
    let keymap = Keymap::live12();
    for (shortcut, keys) in [
        (
            "S-BRW-008",
            vec![
                Key::Num1,
                Key::Num2,
                Key::Num3,
                Key::Num4,
                Key::Num5,
                Key::Num6,
                Key::Num7,
            ],
        ),
        ("S-BRW-009", vec![Key::Num0]),
    ] {
        let entry = keymap.entry_for(shortcut).expect("the shortcut");
        assert!(
            matches!(entry.status, Status::Widget(_)),
            "{shortcut}: {:?}",
            entry.status
        );
        for platform in [Platform::Mac, Platform::Other] {
            let chords: Vec<Key> = entry
                .chords(platform)
                .iter()
                .map(|chord| chord.key)
                .collect();
            assert_eq!(chords, keys, "{shortcut}");
            assert!(
                entry
                    .chords(platform)
                    .iter()
                    .all(|chord| chord.mods == mods(false, false, false, false))
            );
        }
    }
    for (platform, cmd, ctrl) in [(Platform::Mac, true, false), (Platform::Other, false, true)] {
        let hit = keymap
            .resolve(
                platform,
                Key::R,
                mods(cmd, ctrl, false, false),
                &[Context::BrowserSidebar, Context::Global],
            )
            .expect("Cmd+R is bound");
        assert_eq!(hit.command.id, "browser.rename_label");
    }
}

// Spec: second-window R1, R4, R8, R11. Cmd+Shift+W (Ctrl+Shift+W) opens and
// closes the second window from any context, and Option+1 and Option+2 move
// the focus to the main view in whichever window shows it.
#[test]
fn the_second_window_shortcut_reaches_its_command() {
    let keymap = Keymap::live12();
    let entry = keymap.entry_for("S-VIEW-002").unwrap();
    assert_eq!(entry.status, Status::Bound);
    assert_eq!(entry.commands.len(), 1);
    assert_eq!(entry.commands[0].id, "view.toggle_second_window");
    assert_eq!(entry.commands[0].contexts, &[Context::Global][..]);
    for (platform, cmd, ctrl) in [(Platform::Mac, true, false), (Platform::Other, false, true)] {
        let hit = keymap
            .resolve(
                platform,
                Key::W,
                mods(cmd, ctrl, false, true),
                &[Context::Arrangement, Context::Global],
            )
            .expect("Cmd+Shift+W is bound");
        assert_eq!(hit.command.id, "view.toggle_second_window");
        // Plain Cmd+W stays Close Window.
        let close = keymap
            .resolve(
                platform,
                Key::W,
                mods(cmd, ctrl, false, false),
                &[Context::Global],
            )
            .expect("Cmd+W is bound");
        assert_eq!(close.command.id, "window.close");
    }
}

// Spec: detail-views R1, R2, R5, R6, R11, R14. The detail views' seven
// shortcuts, and Cmd+Option+3, are bound to the commands that carry them
// out, on both platforms, in the contexts the spec gives.
#[test]
fn the_detail_views_shortcuts_reach_their_commands() {
    let keymap = Keymap::live12();
    for (id, command, contexts) in [
        (
            "S-VIEW-004",
            "view.switch_detail_view",
            &[Context::Global][..],
        ),
        (
            "S-VIEW-005",
            "view.show_detail_views",
            &[Context::Global][..],
        ),
        (
            "S-VIEW-015",
            "view.toggle_clip_view",
            &[Context::Global][..],
        ),
        (
            "S-VIEW-016",
            "view.toggle_device_view",
            &[Context::Global][..],
        ),
        (
            "S-VIEW-022",
            "view.toggle_detail_area",
            &[Context::Global][..],
        ),
        (
            "S-NOTE-030",
            "view.expand_clip_view",
            &[Context::ClipView][..],
        ),
        ("S-FOCUS-004", "focus.clip_view", &[Context::Global][..]),
        ("S-FOCUS-005", "focus.device_view", &[Context::Global][..]),
    ] {
        let entry = keymap.entry_for(id).unwrap();
        assert_eq!(entry.status, Status::Bound, "{id}");
        assert_eq!(entry.commands.len(), 1, "{id}");
        assert_eq!(entry.commands[0].id, command, "{id}");
        assert_eq!(entry.commands[0].contexts, contexts, "{id}");
    }
    let plain = mods(false, false, false, false);
    for platform in [Platform::Mac, Platform::Other] {
        // Shift+Tab and F12 switch the views in the global context, which
        // Tab's own setting (the `tab-focus` context) comes before.
        let at = |key, m, contexts: &[Context]| {
            keymap
                .resolve(platform, key, m, contexts)
                .map(|hit| hit.command.id.clone())
        };
        let shift = mods(false, false, false, true);
        assert_eq!(
            at(Key::F12, plain, &[Context::Global]).as_deref(),
            Some("view.switch_detail_view")
        );
        assert_eq!(
            at(Key::Tab, shift, &[Context::Global]).as_deref(),
            Some("view.switch_detail_view")
        );
        assert_eq!(
            at(Key::Tab, shift, &[Context::TabFocus, Context::Global]).as_deref(),
            Some("focus.previous")
        );
        // Expand Clip View is the Clip View's own key.
        let (cmd, ctrl) = match platform {
            Platform::Mac => (true, false),
            Platform::Other => (false, true),
        };
        let expand = mods(cmd, ctrl, true, false);
        assert_eq!(
            at(Key::E, expand, &[Context::ClipView, Context::Global]).as_deref(),
            Some("view.expand_clip_view")
        );
        assert_eq!(
            at(Key::E, expand, &[Context::DeviceView, Context::Global]),
            None
        );
    }
}

// Spec: track-context-menu R16. The whole-track commands have their keys
// on the track headers, and the Arrangement's own edits keep them where
// clips or time are selected.
#[test]
fn the_track_headers_keys_reach_the_track_commands() {
    let keymap = Keymap::live12();
    let headers = [Context::TrackHeaders, Context::Arrangement, Context::Global];
    let arrangement = [Context::Arrangement, Context::Global];
    // The Session View has no title-bar keys (R16: track headers only).
    let session = [Context::Session, Context::Global];
    let resolve = |platform, key, m, contexts: &[Context]| {
        keymap
            .resolve(platform, key, m, contexts)
            .map(|hit| hit.command.id.clone())
    };
    for (platform, cmd) in [
        (Platform::Mac, mods(true, false, false, false)),
        (Platform::Other, mods(false, true, false, false)),
    ] {
        for (key, command, edit) in [
            (Key::X, "track.cut", "arrangement.cut"),
            (Key::C, "track.copy", "arrangement.copy"),
            (Key::V, "track.paste", "arrangement.paste"),
            (Key::D, "track.duplicate", "arrangement.duplicate"),
        ] {
            assert_eq!(
                resolve(platform, key, cmd, &headers).as_deref(),
                Some(command),
                "{platform:?}: {command}"
            );
            assert_eq!(
                resolve(platform, key, cmd, &arrangement).as_deref(),
                Some(edit),
                "{platform:?}: {edit}"
            );
            assert_ne!(
                resolve(platform, key, cmd, &session).as_deref(),
                Some(command),
                "{platform:?}: {command} is not bound in the Session View"
            );
        }
    }
}

// Spec: automation-editing R29. Every key of R21 to R26 is bound or read by
// the lane and the Envelope Editor, which the keymap names.
#[test]
fn the_envelope_keys_are_bound_or_read_by_the_lane_and_the_envelope_editor() {
    let keymap = Keymap::live12();
    for id in [
        "S-A11Y-018",
        "S-A11Y-019",
        "S-A11Y-020",
        "S-A11Y-021",
        "S-A11Y-022",
        "S-A11Y-023",
        "S-A11Y-024",
        "S-A11Y-025",
        "S-A11Y-026",
        "S-A11Y-027",
        "S-A11Y-028",
        "S-A11Y-029",
        "S-A11Y-030",
        "S-ENV-005",
    ] {
        let entry = keymap
            .entry_for(id)
            .unwrap_or_else(|| panic!("{id} is in the keymap"));
        assert!(
            matches!(entry.status, Status::Bound | Status::Widget(_)),
            "{id} is handled: {:?}",
            entry.status
        );
    }
    let envelope = [Context::Envelope, Context::Arrangement, Context::Global];
    let resolve = |key, m| {
        keymap
            .resolve(Platform::Other, key, m, &envelope)
            .map(|hit| hit.command.id.clone())
    };
    let none = mods(false, false, false, false);
    assert_eq!(resolve(Key::Enter, none).as_deref(), Some("envelope.enter"));
    assert_eq!(
        resolve(Key::Escape, none).as_deref(),
        Some("envelope.cancel")
    );
    assert_eq!(
        resolve(Key::Delete, none).as_deref(),
        Some("envelope.delete_breakpoint")
    );
    assert_eq!(
        resolve(Key::Delete, mods(false, true, false, false)).as_deref(),
        Some("envelope.delete")
    );
    assert_eq!(
        resolve(Key::Tab, none).as_deref(),
        Some("envelope.select_adjacent")
    );
    assert_eq!(
        resolve(Key::Tab, mods(false, false, false, true)).as_deref(),
        Some("envelope.select_adjacent")
    );
    // The keys with two directions are read by the lane itself, so no wider
    // context runs a command for them.
    assert_eq!(resolve(Key::ArrowLeft, none), None);
    assert_eq!(resolve(Key::ArrowUp, mods(false, false, true, false)), None);
}

// Spec: automation-editing R22 to R24. Tab, Esc and Delete decline when the
// envelope has nothing for them, and fall through to the wider contexts.
#[test]
fn tab_esc_and_delete_in_the_envelope_context_fall_through_when_they_cannot_run() {
    let keymap = Keymap::live12();
    let contexts = [Context::Envelope, Context::TabFocus, Context::Global];
    let never = |hit: &bl_ui::keymap::Hit<'_>| !hit.command.id.starts_with("envelope.");
    let hit = keymap
        .resolve_with(
            Platform::Other,
            Key::Tab,
            mods(false, false, false, false),
            &contexts,
            never,
        )
        .expect("Tab still does something");
    assert_eq!(hit.command.id, "focus.next");
    let hit = keymap
        .resolve_with(
            Platform::Other,
            Key::Tab,
            mods(false, false, false, true),
            &contexts,
            never,
        )
        .expect("Shift-Tab still does something");
    assert_eq!(hit.command.id, "focus.previous");
    // Enter doesn't decline: it always makes or selects a breakpoint.
    let hit = keymap
        .resolve_with(
            Platform::Other,
            Key::Enter,
            mods(false, false, false, false),
            &contexts,
            never,
        )
        .expect("Enter is the envelope's");
    assert_eq!(hit.command.id, "envelope.enter");
}

// Spec: session-view R39 to R43, second-window R22 (S-EDIT-001 to S-EDIT-004,
// S-EDIT-014). Cut, Copy, Paste and Duplicate reach the Session View's
// commands in the `session` context, and Shift with them runs the same with
// `all_tracks` (S-SES-029 to S-SES-032).
#[test]
fn the_edit_keys_reach_the_session_views_commands() {
    let keymap = Keymap::live12();
    let session = [Context::Session, Context::Global];
    for (shortcut, command, key, all_tracks) in [
        ("S-EDIT-001", "session.cut", Key::X, false),
        ("S-EDIT-002", "session.copy", Key::C, false),
        ("S-EDIT-003", "session.paste", Key::V, false),
        ("S-EDIT-004", "session.duplicate", Key::D, false),
        ("S-SES-029", "session.cut", Key::X, true),
        ("S-SES-030", "session.copy", Key::C, true),
        ("S-SES-031", "session.paste", Key::V, true),
        ("S-SES-032", "session.duplicate", Key::D, true),
    ] {
        let entry = keymap.entry_for(shortcut).unwrap();
        assert_eq!(entry.status, Status::Bound, "{shortcut}");
        for (platform, held) in [
            (Platform::Mac, mods(true, false, false, all_tracks)),
            (Platform::Other, mods(false, true, false, all_tracks)),
        ] {
            let hit = keymap
                .resolve(platform, key, held, &session)
                .unwrap_or_else(|| panic!("{shortcut} on {platform:?}"));
            assert_eq!(hit.command.id, command, "{shortcut} on {platform:?}");
            assert_eq!(hit.context, Context::Session, "{shortcut}");
            assert_eq!(
                hit.command.args.get("all_tracks").and_then(|v| v.as_bool()),
                all_tracks.then_some(true),
                "{shortcut}"
            );
        }
    }
    match &keymap.entry_for("S-EDIT-014").unwrap().status {
        Status::Widget(handler) => {
            for command in [
                "session.cut",
                "session.copy",
                "session.paste",
                "session.duplicate",
            ] {
                assert!(handler.contains(command), "{handler}");
            }
        }
        other => panic!("S-EDIT-014 is {other:?}"),
    }
}

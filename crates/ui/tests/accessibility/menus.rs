//! The Navigate and Options menus, the drawn menu bar and the context menus, from the keyboard.

use crate::accessibility::helpers::*;
use crate::common::Harness;
use bl_shell::Area;
use bl_shell::menus::{MenuPlatform, menu_bar};
use bl_ui::arrangement::headers::title_id;
use bl_ui::keymap::Platform;
use bl_ui::main_window::{UiEvent, main_view_id};
use bl_ui::menus::item_state;
use egui::{FullOutput, Key, Modifiers};
use serde_json::json;

fn menu_item(h: &Harness, row: &str) -> bl_ui::menus::ItemState {
    let bar = menu_bar(MenuPlatform::current());
    let item = bar
        .iter()
        .flat_map(bl_shell::menus::Menu::walk)
        .find(|item| item.row == row)
        .unwrap_or_else(|| panic!("{row} is in the menus"));
    item_state(item, &h.shell, &h.keymap, Platform::current())
}

// Spec: accessibility R4, R5, R9.
#[test]
fn the_navigate_and_options_menus_reach_their_commands() {
    let (mut h, tracks, _) = set();
    // Next and Previous Neighbor follow whether there's a neighbour, not
    // the Tab setting (observed in Live 12.4.6), and show their keys.
    assert!(!h.shell.tab_navigation());
    for row in ["M-NAV-014", "M-NAV-015"] {
        let item = menu_item(&h, row);
        assert!(!item.enabled, "{row}: no track selected or focused");
        assert!(item.chord.is_some(), "{row} shows its keys");
    }
    h.shell
        .run(
            "arrangement.select_tracks",
            json!({ "tracks": [tracks[0]] }),
        )
        .unwrap();
    let previous = menu_item(&h, "M-NAV-014");
    let next = menu_item(&h, "M-NAV-015");
    assert!(!previous.enabled, "track 1 has no previous neighbour");
    assert!(next.enabled);
    assert_eq!(next.invocation.as_ref().unwrap().id, "focus.next_neighbor");
    // Chosen with the Arrangement focused, Next Neighbor moves the focus to
    // the next track and leaves the selection.
    press(&mut h, Key::Num2, Modifiers::ALT);
    h.shell
        .run(
            "arrangement.select_tracks",
            json!({ "tracks": [tracks[0]] }),
        )
        .unwrap();
    h.shell.invoke("focus.next_neighbor", json!({})).unwrap();
    let effects = h.shell.take_effects();
    for effect in &effects {
        h.main.apply_effect(&h.ctx, effect);
    }
    h.settle();
    assert_eq!(focused_id(&h), Some(title_id(tracks[1])));
    assert_eq!(h.shell.arrangement().selected_tracks(), [tracks[0]]);
    assert!(menu_item(&h, "M-NAV-014").enabled, "now track 2's");

    let wrap = menu_item(&h, "M-NAV-019");
    assert_eq!((wrap.enabled, wrap.checked), (true, Some(true)));
    h.shell
        .invoke(&wrap.invocation.unwrap().id, json!({}))
        .unwrap();
    assert_eq!(menu_item(&h, "M-NAV-019").checked, Some(false));
    // Options ▸ Solo in Place switches the set's setting, one undo step.
    let solo = menu_item(&h, "M-OPT-025");
    assert_eq!((solo.enabled, solo.checked), (true, Some(true)));
    let invocation = solo.invocation.unwrap();
    h.shell.invoke(&invocation.id, invocation.args).unwrap();
    assert!(!h.shell.editor().document().settings().solo_in_place);
    assert_eq!(menu_item(&h, "M-OPT-025").checked, Some(false));
    h.shell.invoke("edit.undo", json!({})).unwrap();
    assert!(h.shell.editor().document().settings().solo_in_place);
    // Options ▸ Lock Envelopes switches the set's setting, one undo step
    // (automation-editing R8).
    let lock = menu_item(&h, "M-OPT-026");
    assert_eq!((lock.enabled, lock.checked), (true, Some(false)));
    let invocation = lock.invocation.unwrap();
    h.shell.invoke(&invocation.id, invocation.args).unwrap();
    assert!(h.shell.editor().document().settings().lock_envelopes);
    assert_eq!(menu_item(&h, "M-OPT-026").checked, Some(true));
    h.shell.invoke("edit.undo", json!({})).unwrap();
    assert!(!h.shell.editor().document().settings().lock_envelopes);
}

// Spec: accessibility R5 (S-MENU-001 to S-MENU-004).
#[test]
fn alt_and_a_letter_open_the_drawn_menu_bar_s_menus() {
    // The menu bar Beatlayer draws where there's no native one (Linux).
    let (mut h, _, _) = set();
    h.menu_bar = Some(menu_bar(MenuPlatform::Other));
    h.settle();
    let output = press(&mut h, Key::N, Modifiers::ALT);
    assert!(egui::Popup::is_any_open(&h.ctx), "Alt-N opened Navigate");
    // An item reads its label, then its keys.
    assert_eq!(
        focused_label(&output),
        "Control Bar Alt+0",
        "its first item"
    );
    // Down moves to the next item that can be chosen; Enter chooses it.
    // Navigate's Session View (Option-1) is live now (WP-3.7).
    let output = press(&mut h, Key::ArrowDown, Modifiers::NONE);
    assert_eq!(focused_label(&output), "Session View Alt+1");
    let output = press(&mut h, Key::ArrowDown, Modifiers::NONE);
    assert_eq!(focused_label(&output), "Arrangement View Alt+2");
    h.shell.set_focused_area(Area::ControlBar);
    h.take_events();
    press(&mut h, Key::Enter, Modifiers::NONE);
    assert!(
        h.take_events()
            .iter()
            .any(|e| matches!(e, UiEvent::Invoke(i) if i.id == "focus.arrangement"))
    );
    assert_eq!(h.shell.focused_area(), Area::MainView);
    assert!(!egui::Popup::is_any_open(&h.ctx), "choosing closed it");
    // Esc closes a menu without choosing.
    press(&mut h, Key::E, Modifiers::ALT);
    assert!(egui::Popup::is_any_open(&h.ctx));
    press(&mut h, Key::Escape, Modifiers::NONE);
    assert!(!egui::Popup::is_any_open(&h.ctx));
}

/// The keys that open the focused item's context menu (S-CTX-002):
/// Ctrl-Option-Shift-M on macOS, Shift-F10 elsewhere.
fn context_menu_keys(h: &mut Harness) -> FullOutput {
    if cfg!(target_os = "macos") {
        press(
            &mut *h,
            Key::M,
            Modifiers::CTRL | Modifiers::ALT | Modifiers::SHIFT,
        )
    } else {
        press(&mut *h, Key::F10, Modifiers::SHIFT)
    }
}

/// Presses Down until a screen reader would read `label` as focused.
fn down_to(h: &mut Harness, label: &str) -> bool {
    for _ in 0..40 {
        let output = h.settle();
        if focused_label(&output).starts_with(label) {
            return true;
        }
        h.key(Key::ArrowDown, Modifiers::NONE);
    }
    false
}

// Spec: accessibility R8 (S-CTX-002).
#[test]
fn the_keys_open_the_arrangement_s_context_menus() {
    let (mut h, _, clip) = set();
    press(&mut h, Key::Num2, Modifiers::ALT);
    // With a clip selected, the clip's menu, its first item focused.
    h.shell
        .run("arrangement.select_clips", json!({ "clips": [clip] }))
        .unwrap();
    let output = context_menu_keys(&mut h);
    assert!(egui::Popup::is_any_open(&h.ctx));
    assert!(
        focused_label(&output).starts_with("Cut"),
        "{}",
        focused_label(&output)
    );
    assert!(
        down_to(&mut h, "Loop Selection"),
        "the arrow keys move through it"
    );
    press(&mut h, Key::Escape, Modifiers::NONE);
    assert!(!egui::Popup::is_any_open(&h.ctx));
    // With none, the time selection's: its grid choices set the grid.
    h.shell
        .run("arrangement.select_clips", json!({ "clips": [] }))
        .unwrap();
    h.ctx.memory_mut(|m| m.request_focus(main_view_id()));
    h.settle();
    context_menu_keys(&mut h);
    assert!(egui::Popup::is_any_open(&h.ctx));
    assert!(
        down_to(&mut h, "Narrowest"),
        "Down reaches the grid choices"
    );
    h.take_events();
    press(&mut h, Key::Enter, Modifiers::NONE);
    assert!(
        h.take_events()
            .iter()
            .any(|e| matches!(e, UiEvent::Invoke(i) if i.id == "arrangement.set_grid"))
    );
}

// Spec: accessibility R8 (S-CTX-002).
#[test]
fn the_keys_open_a_sidebar_place_s_context_menu() {
    let dir = std::env::temp_dir().join(format!("bl-ui-a11y-{}", std::process::id()));
    let loops = dir.join("Loops");
    std::fs::create_dir_all(&loops).unwrap();
    let (mut h, _, _) = set();
    h.shell
        .run(
            "browser.add_folder",
            json!({ "path": loops.to_string_lossy() }),
        )
        .unwrap();
    h.shell
        .run(
            "browser.open_place",
            json!({ "label": { "folder": loops.to_string_lossy() } }),
        )
        .unwrap();
    h.shell
        .run("focus.browser", json!({ "part": "sidebar" }))
        .unwrap();
    h.settle();
    assert_eq!(focused_id(&h), Some(bl_ui::browser::sidebar_id()));
    let output = context_menu_keys(&mut h);
    assert_eq!(focused_label(&output), "Remove from Sidebar");
    h.take_events();
    press(&mut h, Key::Enter, Modifiers::NONE);
    assert!(
        h.take_events()
            .iter()
            .any(|e| matches!(e, UiEvent::Invoke(i) if i.id == "browser.remove_folder"))
    );
    let _ = std::fs::remove_dir_all(&dir);
}

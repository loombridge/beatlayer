//! The Info View (WP-2.24), headless: it shows at Live's place in the
//! detail area, reads whatever the pointer is over — the control's name,
//! the registered description of the command it runs, and its key on a
//! separate line — shows nothing while the pointer is over empty space,
//! and hides and shows from its Status Bar toggle and its key.
//!
//! Spec: info-view R1 to R5.

use crate::common;
use bl_shell::commands as ids;
use bl_shell::transport_commands;
use bl_ui::layout::{Element, REFERENCE, ShellLayout};
use common::{Harness, Window};
use egui::accesskit::Role;
use egui::{Event, Key, Modifiers, Pos2};
use serde_json::json;

/// Moves the pointer to `at` and lets the frame settle on it.
fn hover(h: &mut Harness, at: Pos2) {
    h.frame(vec![Event::PointerMoved(at)]);
    h.settle();
}

#[test]
fn a_new_set_shows_the_info_view_where_live_puts_it() {
    // Spec: info-view R1, R3. L-INFO-001: the detail area's left, 231
    // points wide; L-WIN-006's right half stays the Device View's.
    let mut h = Harness::new(Window::Main);
    let output = h.settle();
    assert!(h.shell.info_view_visible(), "a new set opens with it shown");
    let layout = h.main.drawn_layout().expect("the last frame's layout");
    let info = layout.info_view.expect("the Info View shows");
    assert_eq!(info.min, layout.detail.min);
    assert_eq!(info.right(), 240.0);
    assert_eq!(info.bottom(), layout.detail.bottom());
    assert_eq!(layout.device_view.unwrap().left(), 249.0);
    let nodes = Harness::nodes(&output);
    assert!(
        nodes
            .iter()
            .any(|n| n.role == Role::Group && n.label == "Info View"),
        "the region is in the tree"
    );
}

#[test]
fn hovering_a_control_reads_the_command_registered_for_it() {
    // Spec: info-view R4, D17: the text comes from the command's registered
    // description.
    let mut h = Harness::new(Window::Main);
    h.settle();
    let play = ShellLayout::compute(REFERENCE, true, true).rect(Element::Play);
    hover(&mut h, play.center());
    let description = h
        .shell
        .registry()
        .get(transport_commands::PLAY)
        .expect("transport.play is registered")
        .description
        .to_string();
    let content = h.main.info_view_content();
    assert_eq!(content.title, "Play");
    assert_eq!(content.body, description);
    assert!(!content.body.is_empty(), "the description is written");
    assert_eq!(content.shortcut, None, "Play has no recorded key");
    // Spec: info-view R5. Keyboard focus moves; the pointer stays, and the
    // Info View keeps reading the pointer.
    h.shell
        .run(bl_shell::commands::FOCUS_TOGGLE_TAB_NAVIGATION, json!({}))
        .unwrap();
    h.key(Key::Tab, Modifiers::NONE);
    h.settle();
    let content = h.main.info_view_content();
    assert_eq!(content.title, "Play", "keyboard focus doesn't move it");
}

#[test]
fn the_control_under_the_pointer_is_the_toggle_itself() {
    // Spec: info-view R4. A Status Bar element is read too, and its key
    // shows on its own line (L-INFO-002).
    let mut h = Harness::new(Window::Main);
    h.settle();
    let toggle = ShellLayout::compute(REFERENCE, true, true).rect(Element::InfoToggle);
    hover(&mut h, toggle.center());
    let expected_key = {
        let keymap = bl_ui::keymap::Keymap::live12();
        let platform = bl_ui::keymap::Platform::current();
        keymap
            .chord_for_command(ids::VIEW_TOGGLE_INFO_VIEW, platform)
            .map(|chord| bl_ui::keymap::chord_text(chord, platform))
    };
    assert!(expected_key.is_some(), "S-VIEW-008 is bound in the keymap");
    let content = h.main.info_view_content();
    assert_eq!(content.title, "Show/Hide Info View");
    assert_eq!(
        content.body,
        h.shell
            .registry()
            .get(ids::VIEW_TOGGLE_INFO_VIEW)
            .unwrap()
            .description
            .to_string()
    );
    assert_eq!(content.shortcut, expected_key);
}

#[test]
fn hovering_empty_space_clears_it() {
    // Spec: info-view R4. Nothing under the pointer: nothing shown.
    let mut h = Harness::new(Window::Main);
    h.settle();
    let play = ShellLayout::compute(REFERENCE, true, true).rect(Element::Play);
    hover(&mut h, play.center());
    assert!(!h.main.info_view_content().title.is_empty());
    // The Control Bar's background, between its controls: no element.
    hover(&mut h, Pos2::new(400.0, 30.0));
    let content = h.main.info_view_content();
    assert_eq!(content.title, "", "cleared: nothing under the pointer");
    assert_eq!(content.body, "");
    assert_eq!(content.shortcut, None);
}

#[test]
fn the_status_bar_toggle_hides_it_and_the_device_view_widens() {
    // Spec: info-view R2. The Device View takes the Info View's place, as
    // the main view does when the browser hides.
    let mut h = Harness::new(Window::Main);
    h.settle();
    let toggle = ShellLayout::compute(REFERENCE, true, true).rect(Element::InfoToggle);
    h.click(toggle.center());
    assert!(!h.shell.info_view_visible());
    let nodes = Harness::nodes(&h.settle());
    assert!(
        !nodes
            .iter()
            .any(|n| n.role == Role::Group && n.label == "Info View"),
        "the region is gone"
    );
    let layout = h.main.drawn_layout().expect("the last frame's layout");
    assert!(layout.info_view.is_none());
    assert_eq!(
        layout.device_view.unwrap().left(),
        layout.detail.left(),
        "the Device View takes its place"
    );
    assert_eq!(
        h.main.info_view_content().title,
        "",
        "hidden: it shows nothing"
    );
    // Back again.
    h.click(toggle.center());
    assert!(h.shell.info_view_visible());
    let nodes = Harness::nodes(&h.settle());
    assert!(
        nodes
            .iter()
            .any(|n| n.role == Role::Group && n.label == "Info View")
    );
}

#[test]
fn the_key_hides_and_shows_it() {
    // Spec: info-view R5. S-VIEW-008: Shift+Slash.
    let mut h = Harness::new(Window::Main);
    h.settle();
    h.key(Key::Slash, Modifiers::SHIFT);
    assert!(!h.shell.info_view_visible());
    h.key(Key::Slash, Modifiers::SHIFT);
    assert!(h.shell.info_view_visible());
}

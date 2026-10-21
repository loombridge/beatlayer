//! The Shortcuts page in the Settings window: the person's own keyboard
//! shortcuts, the keys the page records, the questions it asks, and the
//! rows assistive technology reaches (the custom shortcuts spec, R7 to
//! R13).

mod common;

use bl_shell::keymap::{Origin, Platform};
use bl_shell::settings::SettingsPage;
use common::{Harness, Window, command};
use egui::accesskit::Role;
use egui::{Key, Modifiers, Pos2};
use serde_json::json;

/// The Settings window on the Shortcuts page.
fn open_shortcuts() -> Harness {
    let mut h = Harness::manual(Window::Settings);
    h.shell
        .invoke("app.settings", json!({ "page": SettingsPage::Shortcuts }))
        .unwrap();
    h.shell.take_effects();
    h.settings.refresh();
    h.settle();
    h
}

/// The middle of a node, where a click lands.
fn middle(node: &common::Node) -> Pos2 {
    let bounds = node.bounds.expect("the node's bounds");
    Pos2::new(
        ((bounds.x0 + bounds.x1) / 2.0) as f32,
        ((bounds.y0 + bounds.y1) / 2.0) as f32,
    )
}

/// The Settings window on the Shortcuts page, with the search field
/// narrowed to `search`, so the command's row is on screen.
fn open_shortcuts_searching(search: &str) -> Harness {
    let mut h = open_shortcuts();
    // The search field, at the top of the page.
    h.click(egui::Pos2::new(400.0, 20.0));
    h.settle();

    for key in search.chars() {
        h.frame(vec![egui::Event::Text(key.to_string())]);
    }
    h.settle();
    h
}

/// The tabs of the page chooser, in order.
fn tabs(nodes: &[common::Node]) -> Vec<String> {
    let list = nodes
        .iter()
        .find(|n| n.role == Role::TabList)
        .expect("a tab list");
    let mut tabs = Vec::new();
    let mut stack = vec![list.id];
    while let Some(id) = stack.pop() {
        let node = nodes.iter().find(|n| n.id == id).unwrap();
        if node.role == Role::Tab {
            tabs.push(node.label.clone());
        }
        stack.extend(node.children.iter().rev());
    }
    tabs
}

#[test]
fn the_shortcuts_page_sits_after_display_and_input() {
    // Spec: custom-shortcuts R7.
    let mut h = open_shortcuts();
    let nodes = Harness::nodes(&h.frame(vec![]));
    let tabs = tabs(&nodes);
    let at = |name: &str| {
        tabs.iter()
            .position(|tab| tab == name)
            .unwrap_or_else(|| panic!("no {name} page"))
    };
    assert_eq!(at("Shortcuts"), at("Display & Input") + 1);
    assert_eq!(h.shell.settings_page(), SettingsPage::Shortcuts);
}

#[test]
fn every_row_reaches_accesskit() {
    // Spec: custom-shortcuts R12.
    let mut h = open_shortcuts();
    let nodes = Harness::nodes(&h.frame(vec![]));
    let rows: Vec<&common::Node> = nodes.iter().filter(|n| n.role == Role::Row).collect();
    assert!(rows.len() > 10, "{} rows", rows.len());
    for row in rows {
        assert!(!row.label.is_empty(), "a row names its command");
        assert!(
            row.value.contains(':'),
            "a row reads its context and keys: {:?}",
            row.value
        );
    }
}

#[test]
fn recording_sets_a_key() {
    // Spec: custom-shortcuts R8, R11.
    let mut h = open_shortcuts_searching("Duplicate");
    let nodes = Harness::nodes(&h.frame(vec![]));
    let keys = Harness::node(&nodes, "Duplicate Track Without Clips keys");
    h.click(middle(keys));
    h.settle();
    // The cell reads "Press keys" while the keys are recorded.
    let nodes = Harness::nodes(&h.frame(vec![]));
    assert_eq!(
        Harness::node(&nodes, "Duplicate Track Without Clips keys").value,
        "Press keys"
    );
    // The next key press is the shortcut.
    h.key(Key::D, command() | Modifiers::ALT);
    h.settle();
    let state = h.shell.keymap_state();
    let own = state
        .shortcuts
        .iter()
        .find(|shortcut| shortcut.mac.origin == Origin::User)
        .expect("the person's own shortcut");
    assert_eq!(own.commands, ["track.duplicate_without_clips"]);
}

#[test]
fn a_conflict_asks_and_replace_moves_the_key() {
    // Spec: custom-shortcuts R9.
    let mut h = open_shortcuts_searching("Duplicate");
    let nodes = Harness::nodes(&h.frame(vec![]));
    // Duplicate Track Without Clips takes the keys Duplicate has.
    let keys = Harness::node(&nodes, "Duplicate Track Without Clips keys");
    h.click(middle(keys));
    h.settle();
    h.key(Key::D, command());
    h.settle();
    // The page asks which command the keys already run.
    let nodes = Harness::nodes(&h.frame(vec![]));
    let question = nodes
        .iter()
        .find(|n| n.value.contains("already runs") || n.label.contains("already runs"))
        .expect("the question");
    assert!(
        question.value.contains("arrangement.duplicate"),
        "{question:?}"
    );
    // Replace moves the keys.
    let replace = Harness::node(&nodes, "Replace");
    h.click(middle(replace));
    h.settle();
    let state = h.shell.keymap_state();
    let own = state
        .shortcuts
        .iter()
        .find(|shortcut| shortcut.mac.origin == Origin::User)
        .expect("the person's own shortcut");
    assert_eq!(own.commands, ["track.duplicate_without_clips"]);
    // Duplicate has lost the key, on this platform.
    let duplicate = state
        .shortcuts
        .iter()
        .find(|shortcut| shortcut.id == "S-EDIT-004")
        .expect("Duplicate");
    let keys = match Platform::current() {
        Platform::Mac => &duplicate.mac.keys,
        Platform::Other => &duplicate.win.keys,
    };
    assert!(keys.is_empty(), "{keys:?}");
}

#[test]
fn reset_and_reset_all_give_lives_keys_back() {
    // Spec: custom-shortcuts R10.
    let mut h = open_shortcuts_searching("Duplicate");
    let nodes = Harness::nodes(&h.frame(vec![]));
    let keys = Harness::node(&nodes, "Duplicate Track Without Clips keys");
    h.click(middle(keys));
    h.settle();
    h.key(Key::D, command() | Modifiers::ALT);
    h.settle();
    // The row's Reset gives the command Live's keys back.
    let nodes = Harness::nodes(&h.frame(vec![]));
    let reset = Harness::node(&nodes, "Reset Duplicate Track Without Clips's keys");
    h.click(middle(reset));
    h.settle();
    let state = h.shell.keymap_state();
    assert!(
        state
            .shortcuts
            .iter()
            .all(|shortcut| shortcut.mac.origin != Origin::User),
        "the person's own shortcut is gone"
    );
    // Reset All asks first.
    let nodes = Harness::nodes(&h.frame(vec![]));
    let all = Harness::node(&nodes, "Reset All");
    h.click(middle(all));
    h.settle();
    let nodes = Harness::nodes(&h.frame(vec![]));
    let question = nodes
        .iter()
        .find(|n| n.label.contains("Live's own keys"))
        .expect("the question");
    assert!(!question.label.is_empty());
}

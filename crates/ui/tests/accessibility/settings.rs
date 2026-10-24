//! Display & Input's keyboard settings.

use crate::accessibility::helpers::*;
use crate::common::{Harness, Window};
use bl_shell::Shell;
use egui::accesskit::Role;
use egui::{Key, Modifiers};
use serde_json::json;

pub(crate) fn open_settings(page: &str) -> Harness {
    let mut h = Harness::new(Window::Settings);
    h.shell
        .invoke("app.settings", json!({ "page": page }))
        .unwrap();
    h.shell.take_effects();
    h.settings.refresh();
    h.settle();
    h
}

/// Presses Tab until a screen reader would read `label` as focused, as a
/// person moving through a window with the keyboard does. False if Tab
/// never gets there.
pub(crate) fn tab_to(h: &mut Harness, label: &str) -> bool {
    for _ in 0..120 {
        let output = h.settle();
        if focused_label(&output) == label {
            return true;
        }
        h.key(Key::Tab, Modifiers::NONE);
    }
    false
}

// Spec: accessibility R6.
#[test]
fn display_and_input_has_live_s_keyboard_settings_in_order() {
    let mut h = open_settings("display-input");
    let nodes = Harness::nodes(&h.settle());
    let top = |label: &str| {
        Harness::node(&nodes, label)
            .bounds
            .unwrap_or_else(|| panic!("{label} has bounds"))
            .y0
    };
    let order = [
        "Zoom",
        "Outline View in Focus",
        "Show Scroll Bars",
        "Follow Behavior",
        "Hide Labels",
        "Use Tab Key to Move Focus",
        "Wrap Tab Navigation",
        "Move Clips With Arrow Keys",
    ];
    for pair in order.windows(2) {
        assert!(
            top(pair[0]) < top(pair[1]),
            "{} before {}",
            pair[0],
            pair[1]
        );
    }
    for built in [
        "Outline View in Focus",
        "Use Tab Key to Move Focus",
        "Wrap Tab Navigation",
    ] {
        let node = Harness::node(&nodes, built);
        assert!(!node.disabled, "{built}");
        assert_eq!(node.role, Role::Button, "{built}");
    }
    // The rows later packages build show the values Live 12.4.6 showed.
    for (pending, value) in [
        ("Show Scroll Bars", "Follow System"),
        ("Follow Behavior", "Scroll"),
        ("Hide Labels", "Show"),
        ("Move Clips With Arrow Keys", "On"),
    ] {
        let node = Harness::node(&nodes, pending);
        assert!(
            node.disabled && node.description.contains("later package"),
            "{pending}"
        );
        assert_eq!(node.value, value, "{pending}");
    }
    // Each switch by the keyboard: Tab to it, Enter. Live's new-install
    // values: Outline View in Focus on, Use Tab Key off, Wrap Tab on.
    for (label, on, was) in [
        (
            "Outline View in Focus",
            Shell::outline_view_in_focus as fn(&Shell) -> bool,
            true,
        ),
        ("Use Tab Key to Move Focus", Shell::tab_navigation, false),
        ("Wrap Tab Navigation", Shell::wrap_navigation, true),
    ] {
        assert_eq!(on(&h.shell), was, "{label} in a new install");
        assert!(tab_to(&mut h, label), "Tab reaches {label}");
        press(&mut h, Key::Enter, Modifiers::NONE);
        assert_eq!(on(&h.shell), !was, "Enter switched {label}");
    }
}

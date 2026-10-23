//! The Theme & Colors page's Theme and Customization sections, headless
//! (WP-6.30).
//!
//! Spec: themes R1, R3, R5, R7, R12, R13.

use crate::common;
use bl_shell::settings::{Appearance, SettingsPage, Tone};
use common::{Harness, Window};
use egui::accesskit::{Action, Role};
use egui::{Event, Id, Key, Modifiers, PointerButton, pos2, vec2};
use serde_json::json;

fn open_page() -> Harness {
    let mut h = Harness::new(Window::Settings);
    h.shell
        .invoke("app.settings", json!({ "page": SettingsPage::ThemeColors }))
        .unwrap();
    h.shell.take_effects();
    h.settings.refresh();
    h.settle();
    h
}

const ROWS: [&str; 8] = [
    "Theme",
    "Appearance",
    "Tone",
    "High Contrast",
    "Grid Line Intensity",
    "Brightness",
    "Color Intensity",
    "Color Hue",
];

#[test]
fn the_rows_are_in_l_set_011_s_order_at_l_set_003_s_places() {
    // Spec: themes R1, R13.
    let mut h = open_page();
    let nodes = Harness::nodes(&h.frame(vec![]));
    let bounds: Vec<_> = ROWS
        .iter()
        .map(|name| Harness::node(&nodes, name).bounds.expect(name))
        .collect();
    let (x0, y0) = (bounds[0].x0, bounds[0].y0);
    for (name, b) in ROWS.iter().zip(&bounds) {
        assert!((b.x0 - x0).abs() < 0.5, "{name} shares the control column");
    }
    for pair in bounds.windows(2) {
        assert!(pair[0].y0 < pair[1].y0, "in order");
    }
    // Rows of a section are 22 points apart; a heading starts the next.
    for i in [0, 1, 2, 4, 5, 6] {
        assert!(
            (bounds[i + 1].y0 - bounds[i].y0 - 22.0).abs() < 0.5,
            "{}",
            ROWS[i]
        );
    }
    assert!(
        bounds[4].y0 - bounds[3].y0 > 22.0,
        "Customization has a heading"
    );
    // Choosers 108 points wide, the switch 54.
    for i in [0, 1, 2] {
        assert!(
            (bounds[i].x1 - bounds[i].x0 - 108.0).abs() < 0.5,
            "{}",
            ROWS[i]
        );
    }
    assert!((bounds[3].x1 - bounds[3].x0 - 54.0).abs() < 0.5);
    assert!(y0 > 0.0);

    // The tree: pop-up buttons, a switch, and sliders with value and range.
    let theme = Harness::node(&nodes, "Theme");
    assert_eq!(theme.value, "Beatlayer");
    assert_eq!(Harness::node(&nodes, "Appearance").value, "Dark");
    assert_eq!(Harness::node(&nodes, "Tone").value, "Neutral");
    assert_eq!(Harness::node(&nodes, "High Contrast").toggled, Some(false));
    for (name, value, numeric) in [
        ("Grid Line Intensity", "100 %", 100.0),
        ("Brightness", "0", 0.0),
        ("Color Intensity", "100 %", 100.0),
        ("Color Hue", "0°", 0.0),
    ] {
        let node = Harness::node(&nodes, name);
        assert_eq!(node.role, Role::SpinButton, "{name}");
        assert_eq!(node.value, value, "{name}");
        assert_eq!(node.numeric, Some(numeric), "{name}");
    }
}

#[test]
fn the_choosers_and_the_switch_run_their_commands() {
    // Spec: themes R3, R4, R5.
    let mut h = open_page();
    for (salt, key, check) in [("appearance", Key::ArrowUp, 0), ("tone", Key::ArrowDown, 1)] {
        h.accesskit(Id::new(("chooser", salt)), Action::Focus);
        h.settle();
        h.key(key, Modifiers::NONE);
        match check {
            0 => assert_eq!(h.shell.appearance(), Appearance::Light),
            _ => assert_eq!(h.shell.look().tone, Tone::Warm),
        }
    }
    h.accesskit(Id::new(("switch", "high-contrast")), Action::Click);
    h.settle();
    assert!(h.shell.look().high_contrast);
    // Every window draws the result from the next frame: the harness's theme
    // is made from the shell by the host, here by hand.
    let nodes = Harness::nodes(&h.frame(vec![]));
    assert_eq!(Harness::node(&nodes, "Appearance").value, "Light");
    assert_eq!(Harness::node(&nodes, "Tone").value, "Warm");
    assert_eq!(Harness::node(&nodes, "High Contrast").toggled, Some(true));
}

#[test]
fn the_number_boxes_step_by_key_by_drag_and_by_a_screen_reader() {
    // Spec: themes R7.
    let mut h = open_page();
    let id = |salt: &str| Id::new(("number-box", salt.to_owned()));
    // By key: Up and Down with the keyboard focus on the box.
    h.accesskit(id("settings.set_brightness"), Action::Focus);
    h.settle();
    h.key(Key::ArrowUp, Modifiers::NONE);
    h.key(Key::ArrowUp, Modifiers::NONE);
    h.key(Key::ArrowDown, Modifiers::NONE);
    assert_eq!(h.shell.look().brightness, 1);
    // By a screen reader, in each box's step.
    h.accesskit(id("settings.set_grid_line_intensity"), Action::Increment);
    h.accesskit(id("settings.set_color_intensity"), Action::Decrement);
    h.accesskit(id("settings.set_color_hue"), Action::Increment);
    h.settle();
    let look = h.shell.look();
    assert_eq!(look.grid_line_intensity, 105);
    assert_eq!(look.color_intensity, 95);
    assert_eq!(look.color_hue, 5);
    // The ends hold.
    for _ in 0..60 {
        h.accesskit(id("settings.set_brightness"), Action::Increment);
        h.settle();
    }
    assert_eq!(h.shell.look().brightness, 50);
    // By drag: three points a step.
    let nodes = Harness::nodes(&h.frame(vec![]));
    let b = Harness::node(&nodes, "Color Hue").bounds.unwrap();
    let at = pos2(((b.x0 + b.x1) / 2.0) as f32, ((b.y0 + b.y1) / 2.0) as f32);
    h.frame(vec![Event::PointerMoved(at)]);
    h.frame(vec![Event::PointerButton {
        pos: at,
        button: PointerButton::Primary,
        pressed: true,
        modifiers: Modifiers::NONE,
    }]);
    for dy in [3.0, 6.0, 9.0, 12.0, 15.0, 18.0] {
        h.frame(vec![Event::PointerMoved(at - vec2(0.0, dy))]);
    }
    h.frame(vec![Event::PointerButton {
        pos: at - vec2(0.0, 18.0),
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    }]);
    h.settle();
    let hue = h.shell.look().color_hue;
    assert!(
        hue >= 15 && hue % 5 == 0,
        "dragged up in steps of 5 from 5: {hue}"
    );
    let nodes = Harness::nodes(&h.frame(vec![]));
    assert_eq!(Harness::node(&nodes, "Color Hue").value, format!("{hue}°"));
    assert_eq!(Harness::node(&nodes, "Brightness").value, "50");
}

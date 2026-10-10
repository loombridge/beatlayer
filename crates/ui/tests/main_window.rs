//! The main window, headless: every control is in the accessibility tree
//! with a role, a name, and a value; the live ones work by mouse, keyboard,
//! and assistive technology; and the inert ones say which package brings
//! them.
//!
//! Spec: app-shell R1, R9, R11, R12.

mod common;

use bl_shell::{Area, MainView, commands as ids};
use bl_ui::layout::{ELEMENTS, Element, REFERENCE, ShellLayout};
use bl_ui::main_window::{UiEvent, element_id, main_view_id};
use common::{Harness, Window, command};
use egui::accesskit::{Action, Role};
use egui::{Key, Modifiers};
use serde_json::json;

#[test]
fn every_control_has_a_role_a_name_and_a_value_or_a_note() {
    let mut h = Harness::new(Window::Main);
    let output = h.settle();
    let nodes = Harness::nodes(&output);
    for placement in ELEMENTS {
        let node = Harness::node(&nodes, placement.name);
        assert_ne!(node.role, Role::Unknown, "{}", placement.name);
        match placement.pending {
            Some(package) => {
                assert!(node.disabled, "{} is inert", placement.name);
                assert!(
                    node.description.contains(package),
                    "{}: {:?}",
                    placement.name,
                    node.description
                );
            }
            None => assert!(!node.disabled, "{} works", placement.name),
        }
    }
    assert_eq!(Harness::node(&nodes, "Tempo").value, "120.00");
    assert_eq!(Harness::node(&nodes, "Tempo").role, Role::SpinButton);
    assert_eq!(Harness::node(&nodes, "Time Signature Numerator").value, "4");
    assert_eq!(Harness::node(&nodes, "Sample Rate").value, "48 kHz");
    let cpu = Harness::node(&nodes, "CPU Load");
    assert_eq!(cpu.role, Role::Meter);
    assert!(cpu.value.ends_with("percent"), "{}", cpu.value);
    let arrangement = Harness::node(&nodes, "Arrangement View");
    assert!(
        nodes
            .iter()
            .any(|n| n.role == Role::Pane && n.label == "Arrangement View"),
        "the main view is a pane"
    );
    let _ = arrangement;
    for (role, name) in [
        (Role::Toolbar, "Control Bar"),
        (Role::Group, "Status Bar"),
        (Role::Group, "Browser"),
        (Role::Group, "Info View"),
        (Role::Group, "Device View"),
    ] {
        let region = nodes
            .iter()
            .find(|n| n.role == role && n.label == name)
            .unwrap_or_else(|| panic!("no {role:?} {name}"));
        if name == "Control Bar" {
            let tempo = Harness::node(&nodes, "Tempo");
            assert!(
                Harness::descends_from(&nodes, tempo.id, region.id),
                "the Control Bar's controls hang from it"
            );
        }
    }
}

#[test]
fn a_new_set_shows_the_arrangement_selected() {
    // Spec: app-shell R1 (IMP-04, D20).
    let mut h = Harness::new(Window::Main);
    let output = h.settle();
    let nodes = Harness::nodes(&output);
    assert_eq!(h.shell.main_view(), MainView::Arrangement);
    let arrangement = nodes
        .iter()
        .find(|n| n.label == "Arrangement View" && n.role == Role::Button)
        .expect("the view selector's Arrangement button");
    let session = Harness::node(&nodes, "Session View");
    assert!(
        nodes
            .iter()
            .any(|n| n.role == Role::Pane && n.label == "Arrangement View")
    );
    let toggled = |id| {
        output
            .platform_output
            .accesskit_update
            .as_ref()
            .unwrap()
            .nodes
            .iter()
            .find(|(node_id, _)| *node_id == id)
            .and_then(|(_, node)| node.toggled())
    };
    assert_eq!(
        toggled(arrangement.id),
        Some(egui::accesskit::Toggled::True)
    );
    assert_eq!(toggled(session.id), Some(egui::accesskit::Toggled::False));
}

#[test]
fn clicking_the_sample_rate_opens_the_audio_settings() {
    // Spec: app-shell R9 (L-CTRL-023).
    let mut h = Harness::new(Window::Main);
    h.settle();
    let rect = ShellLayout::compute(REFERENCE, true).rect(Element::SampleRate);
    h.click(rect.center());
    let events = h.take_events();
    assert!(events.contains(&UiEvent::Invoke(bl_shell::Invocation::new(
        ids::APP_SETTINGS,
        json!({ "page": "audio" })
    ))));
    assert!(h.shell.settings_open());
}

#[test]
fn the_browser_toggle_hides_the_browser_and_widens_the_main_view() {
    let mut h = Harness::new(Window::Main);
    h.settle();
    let rect = ShellLayout::compute(REFERENCE, true).rect(Element::BrowserToggle);
    h.click(rect.center());
    assert!(!h.shell.browser_visible());
    let nodes = Harness::nodes(&h.settle());
    assert!(
        !nodes
            .iter()
            .any(|n| n.role == Role::Group && n.label == "Browser")
    );
    assert_eq!(Harness::node(&nodes, "Show/Hide Browser").value, "hidden");
    // The keys do the same, through the keymap.
    let alt = Modifiers::ALT;
    h.key(Key::B, command() | alt);
    assert!(h.shell.browser_visible());
}

#[test]
fn a_screen_reader_can_press_the_view_selector() {
    let mut h = Harness::new(Window::Main);
    h.settle();
    h.accesskit(element_id(Element::SessionView), Action::Click);
    h.settle();
    assert_eq!(h.shell.main_view(), MainView::Session);
    let nodes = Harness::nodes(&h.frame(vec![]));
    assert!(
        nodes
            .iter()
            .any(|n| n.role == Role::Pane && n.label == "Session View")
    );
}

#[test]
fn shortcuts_run_commands_through_the_keymap() {
    // Spec: app-shell R11.
    let mut h = Harness::new(Window::Main);
    h.settle();
    h.key(Key::Comma, command());
    assert!(h.shell.settings_open(), "Cmd/Ctrl+Comma opens Settings");
    h.shell.close_settings();
    h.shell.take_effects();

    h.key(Key::T, command());
    let doc = h.shell.editor().document();
    assert_eq!(
        doc.top_level().len(),
        5,
        "Cmd/Ctrl+T inserts an audio track"
    );
    h.key(Key::Z, command());
    assert_eq!(
        h.shell.editor().document().top_level().len(),
        4,
        "and undo removes it"
    );

    // Tab switches the main view; held past half a second it switches back
    // when released (a momentary shortcut).
    h.key(Key::Tab, Modifiers::NONE);
    assert_eq!(h.shell.main_view(), MainView::Session);
    h.frame(vec![egui::Event::Key {
        key: Key::Tab,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: Modifiers::NONE,
    }]);
    assert_eq!(h.shell.main_view(), MainView::Arrangement);
    for _ in 0..40 {
        h.frame(vec![egui::Event::Key {
            key: Key::Tab,
            physical_key: None,
            pressed: true,
            repeat: true,
            modifiers: Modifiers::NONE,
        }]);
    }
    assert_eq!(
        h.shell.main_view(),
        MainView::Arrangement,
        "repeats do nothing"
    );
    h.frame(vec![egui::Event::Key {
        key: Key::Tab,
        physical_key: None,
        pressed: false,
        repeat: false,
        modifiers: Modifiers::NONE,
    }]);
    assert_eq!(
        h.shell.main_view(),
        MainView::Session,
        "released after a hold: back"
    );
}

#[test]
fn the_keyboard_reaches_the_control_bar() {
    // Spec: app-shell R12.
    let mut h = Harness::new(Window::Main);
    h.settle();
    h.key(Key::Num0, Modifiers::ALT);
    h.settle();
    assert_eq!(h.shell.focused_area(), Area::ControlBar);
    let focused = h.ctx.memory(|m| m.focused());
    assert_eq!(
        focused,
        Some(element_id(Element::BrowserToggle)),
        "the first control has the focus"
    );
    // Enter on the focused control presses it, as a click does.
    h.key(Key::Enter, Modifiers::NONE);
    assert!(!h.shell.browser_visible());

    // With Use Tab Key to Move Focus on, Tab moves along the Control Bar's
    // live controls.
    h.shell
        .run(ids::FOCUS_TOGGLE_TAB_NAVIGATION, json!({}))
        .unwrap();
    h.key(Key::Tab, Modifiers::NONE);
    h.settle();
    assert_eq!(
        h.shell.main_view(),
        MainView::Arrangement,
        "Tab didn't switch views"
    );
    let focused = h.ctx.memory(|m| m.focused());
    assert_ne!(
        focused,
        Some(element_id(Element::BrowserToggle)),
        "the focus moved on"
    );

    h.key(Key::Num2, Modifiers::ALT);
    h.settle();
    assert_eq!(h.ctx.memory(|m| m.focused()), Some(main_view_id()));
}

#[test]
fn with_the_engine_off_the_cpu_meter_reads_off() {
    // Spec: app-shell R8.
    let mut h = Harness::new(Window::Main);
    h.settle();
    let shift = Modifiers::SHIFT | Modifiers::ALT;
    h.key(Key::E, command() | shift);
    assert!(!h.shell.audio().engine_on());
    let nodes = Harness::nodes(&h.settle());
    assert_eq!(Harness::node(&nodes, "CPU Load").value, "audio engine off");
    let status = Harness::node(&nodes, "Status");
    assert!(
        status.value.contains("audio engine is off"),
        "{}",
        status.value
    );
    // Clicking the message opens the Audio settings.
    let rect = ShellLayout::compute(REFERENCE, true).rect(Element::Message);
    h.click(rect.left_center() + egui::vec2(20.0, 0.0));
    assert!(h.shell.settings_open());
}

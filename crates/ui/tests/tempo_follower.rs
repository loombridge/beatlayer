//! Tempo Follower in the app, headless (WP-9.13, `docs/specs/tempo-follower.md`
//! R1 to R5, R23 and `docs/specs/control-bar-transport.md` R24 to R26): the
//! Control Bar's Follow toggle after the Link toggle, its states, and
//! Settings' Tempo Follower rows kept in real preference files.

use std::path::{Path, PathBuf};

use crate::common;
use bl_shell::settings::SettingsPage;
use bl_ui::layout::Element;
use bl_ui::main_window::element_id;
use common::{Harness, Node, Window};
use egui::accesskit::{Action, Role};
use egui::{Id, Key, Modifiers, Rect, pos2};
use serde_json::json;

fn bounds(node: &Node) -> Rect {
    let b = node.bounds.unwrap();
    Rect::from_min_max(
        pos2(b.x0 as f32, b.y0 as f32),
        pos2(b.x1 as f32, b.y1 as f32),
    )
}

fn labelled(nodes: &[Node], label: &str) -> bool {
    nodes.iter().any(|node| node.label == label)
}

/// The Control Bar's node called `label`, or None.
fn bar_node(h: &mut Harness, label: &str) -> Option<Node> {
    Harness::nodes(&h.settle())
        .into_iter()
        .find(|node| node.label == label && node.bounds.is_some_and(|b| b.y1 < 38.0))
}

fn preferences_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "bl-ui-tempo-follower-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// A main window on a running engine with the null device's input on and
/// the toggle shown.
fn main_window() -> Harness {
    let mut h = Harness::manual(Window::Main);
    h.shell
        .run("audio.configure", json!({ "input_device": "null:null" }))
        .unwrap();
    h.shell
        .run(
            "settings.set_show_tempo_follower_toggle",
            json!({ "on": true }),
        )
        .unwrap();
    h.settle();
    h
}

fn click(h: &mut Harness, label: &str) {
    let nodes = Harness::nodes(&h.settle());
    h.click(bounds(Harness::node(&nodes, label)).center());
}

/// The Settings window on the Tempo & MIDI page.
fn tempo_midi_page(mut h: Harness) -> Harness {
    h.shell
        .invoke("app.settings", json!({ "page": SettingsPage::TempoMidi }))
        .unwrap();
    h.shell.take_effects();
    h.settings.refresh();
    h.settle();
    h
}

// Spec: tempo-follower R2; control-bar-transport R24. The toggle is hidden
// until Show Tempo Follower Toggle says Show, then sits after the Link toggle
// and its arrow and before Tap Tempo, which moves right by its width.
#[test]
fn the_toggle_is_hidden_at_first_and_sits_between_link_and_tap() {
    let mut h = Harness::manual(Window::Main);
    h.shell
        .run("link.set_show_toggle", json!({ "on": true }))
        .unwrap();
    h.settle();
    assert!(
        bar_node(&mut h, "Tempo Follower").is_none(),
        "hidden on a new install"
    );
    let tap = bounds(&bar_node(&mut h, "Tap Tempo").unwrap()).left();

    h.shell
        .run(
            "settings.set_show_tempo_follower_toggle",
            json!({ "on": true }),
        )
        .unwrap();
    let toggle = bar_node(&mut h, "Tempo Follower").expect("shown");
    assert_eq!(toggle.role, Role::Button);
    assert_eq!(toggle.value, "off");
    assert_eq!(toggle.toggled, Some(false));
    let arrow = bar_node(&mut h, "Link Options").unwrap();
    let after = bounds(&bar_node(&mut h, "Tap Tempo").unwrap()).left();
    assert!(
        bounds(&arrow).right() <= bounds(&toggle).left(),
        "after Link"
    );
    assert!(bounds(&toggle).right() <= after, "before Tap Tempo");
    assert!(after > tap, "the tempo group moved right");

    h.shell
        .run(
            "settings.set_show_tempo_follower_toggle",
            json!({ "on": false }),
        )
        .unwrap();
    assert!(bar_node(&mut h, "Tempo Follower").is_none(), "hidden again");
    assert_eq!(bounds(&bar_node(&mut h, "Tap Tempo").unwrap()).left(), tap);
}

// Spec: tempo-follower R4, R23; control-bar-transport R25, R26. While the
// device's inputs are off the toggle is greyed out, its description gives the
// reason and a click does nothing.
#[test]
fn the_toggle_is_greyed_out_with_its_reason_while_the_input_cant_be_reached() {
    let mut h = Harness::manual(Window::Main);
    h.shell
        .run(
            "settings.set_show_tempo_follower_toggle",
            json!({ "on": true }),
        )
        .unwrap();
    let toggle = bar_node(&mut h, "Tempo Follower").unwrap();
    assert!(toggle.disabled);
    assert!(
        toggle.description.contains("inputs are off"),
        "{}",
        toggle.description
    );
    click(&mut h, "Tempo Follower");
    assert!(!h.shell.tempo_follower_on(), "a click does nothing");

    h.shell
        .run("audio.configure", json!({ "input_device": "null:null" }))
        .unwrap();
    let toggle = bar_node(&mut h, "Tempo Follower").unwrap();
    assert!(!toggle.disabled, "available with an input");
}

// Spec: tempo-follower R3, R5, R21, R23; control-bar-transport R25, R26. A
// click and Return with the focus switch Follow; it lights and reads on, its
// description says it listens, EXT and Tap Tempo are disabled while it's on.
#[test]
fn a_click_and_return_switch_follow_and_tap_tempo_is_disabled_while_it_is_on() {
    let mut h = main_window();
    let toggle = bar_node(&mut h, "Tempo Follower").unwrap();
    assert_eq!(toggle.toggled, Some(false), "off at first");
    assert!(!bar_node(&mut h, "Tap Tempo").unwrap().disabled);

    click(&mut h, "Tempo Follower");
    assert!(h.shell.tempo_follower_on());
    let toggle = bar_node(&mut h, "Tempo Follower").unwrap();
    assert_eq!(toggle.toggled, Some(true), "lit");
    assert_eq!(toggle.value, "on");
    assert_eq!(toggle.description, "Listening for a tempo.");
    let tap = bar_node(&mut h, "Tap Tempo").unwrap();
    assert!(tap.disabled, "Tap Tempo is refused while Follow is on");
    assert!(
        tap.description.contains("Tempo Follower is on"),
        "{}",
        tap.description
    );

    click(&mut h, "Tempo Follower");
    assert!(!h.shell.tempo_follower_on());
    assert!(!bar_node(&mut h, "Tap Tempo").unwrap().disabled);

    h.accesskit(element_id(Element::TempoFollower), Action::Focus);
    h.key(Key::Enter, Modifiers::NONE);
    h.settle();
    assert!(
        h.shell.tempo_follower_on(),
        "Return switches it with the focus"
    );
    h.accesskit(element_id(Element::TempoFollower), Action::Focus);
    h.key(Key::Enter, Modifiers::NONE);
    h.settle();
    assert!(!h.shell.tempo_follower_on());
}

// Spec: tempo-follower R1, R2. Settings' Tempo & MIDI page starts with the
// Tempo Follower section: Show Tempo Follower Toggle and Input Channel, which
// run their commands and are kept across a relaunch in the real preference
// files.
#[test]
fn the_settings_rows_run_their_commands_and_are_kept_across_a_relaunch() {
    let dir = preferences_dir("rows");
    let page = |dir: &Path| {
        let mut h = Harness::manual_in(Window::Settings, dir);
        h.shell
            .run("audio.configure", json!({ "input_device": "null:null" }))
            .unwrap();
        tempo_midi_page(h)
    };
    let choose = |h: &mut Harness, salt: &str, key: Key| {
        h.accesskit(Id::new(("chooser", salt)), Action::Focus);
        h.key(key, Modifiers::NONE);
        h.settle();
    };

    let mut h = page(&dir);
    let nodes = Harness::nodes(&h.settle());
    assert!(labelled(&nodes, "Show Tempo Follower Toggle"));
    assert!(labelled(&nodes, "Input Channel"));
    assert!(!h.shell.tempo_follower_toggle_shown(), "Hide at first");
    assert_eq!(h.shell.tempo_follower_input(), bl_model::Channel::Mono(1));

    choose(&mut h, "show-tempo-follower-toggle", Key::ArrowUp);
    assert!(h.shell.tempo_follower_toggle_shown(), "Show");
    // The device's channels in order: 1, 2, then the pair 1/2.
    choose(&mut h, "tempo-follower-input-channel", Key::ArrowDown);
    assert_eq!(h.shell.tempo_follower_input(), bl_model::Channel::Mono(2));
    choose(&mut h, "tempo-follower-input-channel", Key::ArrowDown);
    assert_eq!(h.shell.tempo_follower_input(), bl_model::Channel::Pair(1));
    drop(h);

    let h = page(&dir);
    assert!(h.shell.tempo_follower_toggle_shown(), "kept");
    assert_eq!(
        h.shell.tempo_follower_input(),
        bl_model::Channel::Pair(1),
        "kept"
    );
    assert!(!h.shell.tempo_follower_on(), "Follow isn't");
    let _ = std::fs::remove_dir_all(&dir);
}

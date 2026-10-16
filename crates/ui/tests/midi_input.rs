//! MIDI input in the windows, headless (WP-2.2; the MIDI I/O spec, R2, R17,
//! R18 to R22): the Settings window's MIDI ports table, the Control Bar's
//! Computer MIDI Keyboard switch, and the letter keys that play notes while
//! the keyboard is on and run their shortcuts while it is off.

mod common;

use bl_core::midi::{MidiInputEvent, MidiSource};
use bl_midi_io::{Direction, TestBackend};
use bl_shell::settings::SettingsPage;
use bl_ui::layout::{Element, REFERENCE, ShellLayout};
use common::{Harness, Node, Window};
use egui::{Key, Modifiers, Rect, pos2};
use rtrb::Consumer;
use serde_json::json;

fn bounds(node: &Node) -> Rect {
    let b = node.bounds.unwrap();
    Rect::from_min_max(
        pos2(b.x0 as f32, b.y0 as f32),
        pos2(b.x1 as f32, b.y1 as f32),
    )
}

/// The messages the keyboard has sent since the last call, as status, note
/// and velocity.
fn played(queue: &mut Consumer<MidiInputEvent>) -> Vec<(u8, u8, u8)> {
    std::iter::from_fn(|| queue.pop().ok())
        .map(|e| {
            assert_eq!(e.source, MidiSource::ComputerKeyboard);
            (e.message.status(), e.message.data1(), e.message.data2())
        })
        .collect()
}

fn keyboard_harness() -> (Harness, Consumer<MidiInputEvent>) {
    let mut h = Harness::new(Window::Main);
    let queue = h.shell.midi().sender().attach(256);
    h.settle();
    (h, queue)
}

// Spec: midi-io R17, R18, R19, R20, R21, R22
#[test]
fn m_switches_the_keyboard_on_and_its_letters_play_notes() {
    let (mut h, mut queue) = keyboard_harness();

    // Off, the letters play nothing.
    h.key(Key::A, Modifiers::NONE);
    assert!(played(&mut queue).is_empty());
    assert!(!h.shell.computer_keyboard_on());

    h.key(Key::M, Modifiers::NONE);
    assert!(h.shell.computer_keyboard_on(), "M switches it on");

    // A key plays its note while held: a note-on when pressed, a note-off
    // when released.
    h.key(Key::A, Modifiers::NONE);
    h.key(Key::W, Modifiers::NONE);
    assert_eq!(
        played(&mut queue),
        [
            (0x90, 60, 100),
            (0x80, 60, 64),
            (0x90, 61, 100),
            (0x80, 61, 64)
        ]
    );

    // Z and X move the octave, C and V the velocity.
    h.key(Key::X, Modifiers::NONE);
    h.key(Key::C, Modifiers::NONE);
    h.key(Key::A, Modifiers::NONE);
    assert_eq!(played(&mut queue)[0], (0x90, 72, 80));
    h.key(Key::Z, Modifiers::NONE);
    h.key(Key::Z, Modifiers::NONE);
    h.key(Key::V, Modifiers::NONE);
    h.key(Key::V, Modifiers::NONE);
    h.key(Key::A, Modifiers::NONE);
    assert_eq!(played(&mut queue)[0], (0x90, 48, 120));

    // With Shift, the letter runs its shortcut instead of playing.
    h.key(Key::A, Modifiers::SHIFT);
    assert!(played(&mut queue).is_empty());

    // M keeps its shortcut, and switching off ends held notes.
    h.shell
        .run(
            "midi.computer_keyboard_key",
            json!({ "key": "K", "pressed": true }),
        )
        .unwrap();
    h.key(Key::M, Modifiers::NONE);
    assert!(!h.shell.computer_keyboard_on());
    assert_eq!(played(&mut queue), [(0x90, 60, 120), (0x80, 60, 64)]);
}

// Spec: midi-io R17
#[test]
fn the_control_bars_switch_follows_and_changes_the_keyboard() {
    let (mut h, mut queue) = keyboard_harness();
    let rect = ShellLayout::compute(REFERENCE, true, true).rect(Element::ComputerKeyboard);
    let state = |h: &mut Harness| {
        let nodes = Harness::nodes(&h.settle());
        Harness::node(&nodes, "Computer MIDI Keyboard")
            .value
            .clone()
    };
    assert_eq!(state(&mut h), "off");
    h.click(rect.center());
    assert!(h.shell.computer_keyboard_on());
    assert_eq!(state(&mut h), "on");
    h.key(Key::A, Modifiers::NONE);
    assert_eq!(played(&mut queue).len(), 2);
    h.click(rect.center());
    assert!(!h.shell.computer_keyboard_on());
    assert_eq!(state(&mut h), "off");
}

// Spec: midi-io R2, R3, R5
#[test]
fn the_tempo_and_midi_page_lists_each_port_with_its_switches() {
    let mut h = Harness::new(Window::Settings);
    let test = TestBackend::new();
    test.add_port(Direction::Input, "Keys A");
    test.add_port(Direction::Output, "Synth");
    h.shell.use_midi_backend(Box::new(test.clone()));
    h.shell
        .run("app.settings", json!({ "page": SettingsPage::TempoMidi }))
        .unwrap();
    let nodes = Harness::nodes(&h.settle());
    for switch in ["Track", "Sync", "Remote"] {
        Harness::node(&nodes, &format!("In: Keys A {switch}"));
        Harness::node(&nodes, &format!("Out: Synth {switch}"));
    }
    // A new input has Track on, a new output nothing.
    assert_eq!(
        Harness::node(&nodes, "In: Keys A Track").toggled,
        Some(true)
    );
    assert_eq!(
        Harness::node(&nodes, "In: Keys A Sync").toggled,
        Some(false)
    );
    assert_eq!(
        Harness::node(&nodes, "Out: Synth Track").toggled,
        Some(false)
    );

    let track = Harness::node(&nodes, "In: Keys A Track");
    let center = bounds(track).center();
    h.click(center);
    let nodes = Harness::nodes(&h.settle());
    assert_eq!(
        Harness::node(&nodes, "In: Keys A Track").toggled,
        Some(false)
    );
    assert!(
        !test.is_open(Direction::Input, "Keys A"),
        "all off closes it"
    );

    let remote = Harness::node(&nodes, "Out: Synth Remote");
    h.click(bounds(remote).center());
    assert!(test.is_open(Direction::Output, "Synth"));
}

// Spec: midi-io R21
#[test]
fn a_note_ends_when_the_window_loses_the_keyboard() {
    let (mut h, mut queue) = keyboard_harness();
    h.key(Key::M, Modifiers::NONE);
    h.frame(vec![egui::Event::Key {
        key: Key::A,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: Modifiers::NONE,
    }]);
    assert_eq!(played(&mut queue), [(0x90, 60, 100)]);
    h.frame(vec![egui::Event::WindowFocused(false)]);
    assert_eq!(played(&mut queue), [(0x80, 60, 64)]);
    // The key plays again once the window is back.
    h.frame(vec![egui::Event::WindowFocused(true)]);
    h.key(Key::A, Modifiers::NONE);
    assert_eq!(played(&mut queue).len(), 2);
}

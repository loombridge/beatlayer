//! MIDI input in the windows, headless (WP-2.2; the MIDI I/O spec, R2, R17,
//! R18 to R22): the Settings window's MIDI ports table, the Control Bar's
//! Computer MIDI Keyboard switch, and the letter keys that play notes while
//! the keyboard is on and run their shortcuts while it is off.

use crate::common;
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

// Spec: remote-mapping R13. A mapped key acts on its press and wins over the
// key's shortcut; a letter the computer MIDI keyboard plays plays it instead.
#[test]
fn a_mapped_key_wins_over_its_shortcut_and_yields_to_the_computer_keyboard() {
    let (mut h, mut queue) = keyboard_harness();
    h.shell
        .invoke("track.create", json!({ "kind": "audio", "name": "T" }))
        .unwrap();
    let track = h.shell.editor().document().top_level().last().unwrap();
    let solo = |h: &Harness| {
        h.shell
            .editor()
            .document()
            .track(track)
            .unwrap()
            .mixer()
            .solo
    };
    let map = |h: &mut Harness, key: &str| {
        h.shell
            .run(
                "remote.map",
                json!({ "source": { "type": "key", "key": key },
                        "target": { "type": "solo", "track": track } }),
            )
            .unwrap();
    };

    // B is Draw Mode's shortcut; mapped, it solos the track instead.
    map(&mut h, "B");
    let drawing = h.shell.clip_view().draw_mode();
    h.key(Key::B, Modifiers::NONE);
    assert!(solo(&h), "the mapping ran");
    assert_eq!(
        h.shell.clip_view().draw_mode(),
        drawing,
        "and B's shortcut didn't"
    );
    h.key(Key::B, Modifiers::NONE);
    assert!(!solo(&h), "each press toggles");

    // With the computer MIDI keyboard on, A plays a note and maps nothing.
    map(&mut h, "A");
    h.key(Key::M, Modifiers::NONE);
    h.key(Key::A, Modifiers::NONE);
    assert_eq!(played(&mut queue).len(), 2, "A played its note");
    assert!(!solo(&h));
    // With Shift it doesn't play, and the mapping acts again.
    h.key(Key::A, Modifiers::SHIFT);
    assert!(played(&mut queue).is_empty());
    assert!(solo(&h));
    // Cmd keeps its shortcuts: Cmd-A is never a key mapping's.
    h.key(Key::A, Modifiers::COMMAND);
    assert!(solo(&h));
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

// Spec: remote-mapping R28 (L-SET-014). Takeover Mode is a chooser on the
// Tempo & MIDI page, before the MIDI ports table, None in a new install; the
// keyboard picks another and the command runs.
#[test]
fn the_tempo_and_midi_page_has_the_takeover_mode_chooser_before_the_ports() {
    use egui::Id;
    let mut h = Harness::new(Window::Settings);
    let test = TestBackend::new();
    test.add_port(Direction::Input, "Keys A");
    h.shell.use_midi_backend(Box::new(test));
    h.shell
        .run("app.settings", json!({ "page": SettingsPage::TempoMidi }))
        .unwrap();
    let nodes = Harness::nodes(&h.settle());
    let chooser = Harness::node(&nodes, "Takeover Mode");
    assert_eq!(chooser.value, "None", "a new install's choice");
    assert!(
        bounds(chooser).bottom() <= bounds(Harness::node(&nodes, "In: Keys A Track")).top(),
        "before the ports"
    );

    h.accesskit(
        Id::new(("chooser", "takeover-mode")),
        egui::accesskit::Action::Focus,
    );
    h.key(Key::ArrowDown, Modifiers::NONE);
    h.settle();
    assert_eq!(
        h.shell.takeover_mode(),
        bl_shell::settings::TakeoverMode::PickUp
    );
    assert!(
        h.events.iter().any(|e| matches!(
            e,
            bl_ui::main_window::UiEvent::Invoke(i) if i.id == "settings.set_takeover_mode"
        )),
        "the chooser runs its command"
    );
    let nodes = Harness::nodes(&h.frame(vec![]));
    assert_eq!(Harness::node(&nodes, "Takeover Mode").value, "Pick-Up");
}

// Spec: midi-io R27. An input's row has a fourth switch, MPE, after Remote,
// off for a port seen for the first time; clicking it sets it, and an output
// has none.
#[test]
fn an_inputs_row_has_an_mpe_switch_after_remote_and_an_outputs_has_none() {
    let mut h = Harness::new(Window::Settings);
    let test = TestBackend::new();
    test.add_port(Direction::Input, "Keys A");
    test.add_port(Direction::Output, "Synth");
    h.shell.use_midi_backend(Box::new(test.clone()));
    h.shell
        .run("app.settings", json!({ "page": SettingsPage::TempoMidi }))
        .unwrap();
    let nodes = Harness::nodes(&h.settle());
    let mpe = Harness::node(&nodes, "In: Keys A MPE");
    assert_eq!(mpe.toggled, Some(false), "off when first seen");
    let remote = Harness::node(&nodes, "In: Keys A Remote");
    assert!(
        bounds(mpe).left() > bounds(remote).right(),
        "after Remote in the row"
    );
    assert!(
        nodes.iter().all(|node| node.label != "Out: Synth MPE"),
        "an output has no MPE switch"
    );
    h.click(bounds(mpe).center());
    let nodes = Harness::nodes(&h.settle());
    assert_eq!(Harness::node(&nodes, "In: Keys A MPE").toggled, Some(true));
    assert!(h.shell.midi().ports()[0].mpe);
    assert!(
        test.is_open(Direction::Input, "Keys A"),
        "Track still holds it open"
    );
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

// Spec: midi-io R25; external-devices R7
#[test]
fn the_output_indicator_lights_when_a_message_leaves() {
    use std::time::{Duration, Instant};

    use bl_core::midi::{MidiMessage, MidiOutputEvent, PortId};

    let mut h = Harness::new(Window::Main);
    let test = TestBackend::new();
    test.add_port(Direction::Output, "Synth");
    h.shell.use_midi_backend(Box::new(test.clone()));
    h.shell
        .run(
            "midi.set_port",
            json!({ "direction": "output", "name": "Synth", "track": true }),
        )
        .unwrap();
    let mut engine = h.shell.midi().output().attach(16);
    h.settle();
    let indicators = |h: &mut Harness| {
        // The host polls the shell every frame.
        h.shell.poll();
        let nodes = Harness::nodes(&h.settle());
        Harness::node(&nodes, "MIDI Track Input and Output Indicators")
            .value
            .clone()
    };
    assert_eq!(indicators(&mut h), "", "dark until a message leaves");

    // The engine queues a message for the port; the output thread sends it,
    // and the lower lamp lights.
    engine
        .push(MidiOutputEvent {
            port: PortId::from_name("Synth"),
            time: Instant::now(),
            message: MidiMessage::note_on(0, 60, 100),
        })
        .unwrap();
    let began = Instant::now();
    while test.sent("Synth").is_empty() && began.elapsed() < Duration::from_secs(2) {
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(test.sent("Synth").len(), 1);
    assert_eq!(indicators(&mut h), "sending");

    // A moment later it goes dark again.
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(indicators(&mut h), "");
}

// Spec: remote-mapping R41 (L-CTRL-026). The remote pair's upper lamp lights
// for a message a mapping takes and its lower for feedback.
#[test]
fn the_remote_pair_lights_for_a_mapped_message_and_for_feedback() {
    use std::time::Instant;

    let mut h = Harness::new(Window::Main);
    let test = TestBackend::new();
    test.add_port(Direction::Input, "Pads");
    test.add_port(Direction::Output, "Lamps");
    h.shell.use_midi_backend(Box::new(test.clone()));
    for (direction, name) in [("input", "Pads"), ("output", "Lamps")] {
        h.shell
            .run(
                "midi.set_port",
                json!({ "direction": direction, "name": name, "remote": true, "track": false }),
            )
            .unwrap();
    }
    h.shell
        .invoke("track.create", json!({ "kind": "audio", "name": "T" }))
        .unwrap();
    let track = h.shell.editor().document().top_level().last().unwrap();
    h.settle();
    let lamps = |h: &mut Harness| {
        h.shell.poll();
        let nodes = Harness::nodes(&h.settle());
        Harness::node(&nodes, "Remote Input and Output Indicators")
            .value
            .clone()
    };
    assert_eq!(lamps(&mut h), "", "dark until something crosses");

    // Making a mapping sends its value to the output: the lower lamp.
    h.shell
        .run(
            "remote.map",
            json!({ "source": { "type": "cc", "channel": 1, "controller": 20 },
                    "target": { "type": "mixer", "track": track, "param": "volume" } }),
        )
        .unwrap();
    assert_eq!(lamps(&mut h), "sending");
    assert_eq!(test.sent("Lamps").len(), 1);

    // A message the mapping takes: the upper lamp.
    assert!(test.send("Pads", Instant::now(), &[0xB0, 20, 100]));
    let value = lamps(&mut h);
    assert!(value.contains("receiving"), "{value:?}");
}

// Spec: remote-mapping R41 (L-CTRL-021). The key indicators' upper lamp
// lights for a key a key mapping takes; a key that is not mapped leaves it
// dark.
#[test]
fn the_key_indicator_lights_for_a_mapped_key() {
    let mut h = Harness::new(Window::Main);
    h.shell
        .invoke("track.create", json!({ "kind": "audio", "name": "T" }))
        .unwrap();
    let track = h.shell.editor().document().top_level().last().unwrap();
    h.shell
        .run(
            "remote.map",
            json!({ "source": { "type": "key", "key": "B" },
                    "target": { "type": "solo", "track": track } }),
        )
        .unwrap();
    h.settle();
    let lamp = |h: &mut Harness| {
        h.shell.poll();
        let nodes = Harness::nodes(&h.settle());
        Harness::node(&nodes, "Key Input and Output Indicators")
            .value
            .clone()
    };
    h.key(Key::X, Modifiers::NONE);
    assert_eq!(lamp(&mut h), "", "an unmapped key");
    h.key(Key::B, Modifiers::NONE);
    assert_eq!(lamp(&mut h), "receiving");
}

fn labelled(nodes: &[Node], label: &str) -> bool {
    nodes.iter().any(|node| node.label == label)
}

// Spec: midi-clock R1, R10
#[test]
fn an_outputs_triangle_opens_its_sync_options_and_show_resync_button_is_a_switch() {
    let mut h = Harness::new(Window::Settings);
    let test = TestBackend::new();
    test.add_port(Direction::Input, "Keys A");
    test.add_port(Direction::Output, "Synth");
    h.shell.use_midi_backend(Box::new(test.clone()));
    h.shell
        .run("app.settings", json!({ "page": SettingsPage::TempoMidi }))
        .unwrap();
    let nodes = Harness::nodes(&h.settle());
    // With Sync off there is no triangle and no row, and Show Resync Button
    // is off.
    assert!(!labelled(&nodes, "Out: Synth Sync Options"));
    assert_eq!(
        Harness::node(&nodes, "Show Resync Button").toggled,
        Some(false)
    );
    h.click(bounds(Harness::node(&nodes, "Show Resync Button")).center());
    assert!(h.shell.resync_button_shown());

    h.click(bounds(Harness::node(&nodes, "Out: Synth Sync")).center());
    let nodes = Harness::nodes(&h.settle());
    let triangle = Harness::node(&nodes, "Out: Synth Sync Options");
    assert_eq!(triangle.role, egui::accesskit::Role::DisclosureTriangle);
    assert!(
        !labelled(&nodes, "Out: Synth MIDI Clock Type"),
        "closed at first"
    );
    // An input with Sync on has no output options.
    assert!(!labelled(&nodes, "In: Keys A Sync Options"));

    h.click(bounds(triangle).center());
    let nodes = Harness::nodes(&h.settle());
    assert_eq!(
        Harness::node(&nodes, "Out: Synth MIDI Clock Sync Delay").value,
        "0 ms"
    );
    assert_eq!(
        Harness::node(&nodes, "Out: Synth MIDI Clock Type").value,
        "Song"
    );
    assert_eq!(
        Harness::node(&nodes, "Out: Synth Hardware Resync").value,
        "Stop and Start"
    );

    // The rows follow what `midi.set_port` sets, and the values are kept
    // while Sync is off (the triangle and rows are gone).
    h.shell
        .run(
            "midi.set_port",
            json!({ "direction": "output", "name": "Synth", "sync_delay_ms": 20,
                    "clock_type": "pattern", "hardware_resync": "dont_resync" }),
        )
        .unwrap();
    let nodes = Harness::nodes(&h.settle());
    assert_eq!(
        Harness::node(&nodes, "Out: Synth MIDI Clock Sync Delay").value,
        "20 ms"
    );
    assert_eq!(
        Harness::node(&nodes, "Out: Synth MIDI Clock Type").value,
        "Pattern"
    );
    assert_eq!(
        Harness::node(&nodes, "Out: Synth Hardware Resync").value,
        "Don't Resync"
    );
    h.click(bounds(Harness::node(&nodes, "Out: Synth Sync")).center());
    let nodes = Harness::nodes(&h.settle());
    assert!(!labelled(&nodes, "Out: Synth Sync Options"));
    assert!(!labelled(&nodes, "Out: Synth MIDI Clock Type"));
    assert_eq!(h.shell.midi().ports()[1].sync_delay_ms, 20);
}

// Spec: midi-clock R10, R11, R27
#[test]
fn the_resync_button_shows_while_the_setting_is_on_and_leaves_room_in_the_tempo_group() {
    let mut h = Harness::manual(Window::Main);
    let nodes = Harness::nodes(&h.settle());
    let tap_before = bounds(Harness::node(&nodes, "Tap Tempo")).left();
    assert!(
        !labelled(&nodes, "Resync External Hardware"),
        "off at first"
    );

    h.shell
        .run("midi.set_show_resync_button", json!({ "on": true }))
        .unwrap();
    let nodes = Harness::nodes(&h.settle());
    let button = Harness::node(&nodes, "Resync External Hardware");
    assert_eq!(button.role, egui::accesskit::Role::Button);
    assert!(
        bounds(button).right() <= bounds(Harness::node(&nodes, "Tap Tempo")).left(),
        "before the tempo controls"
    );
    assert!(bounds(Harness::node(&nodes, "Tap Tempo")).left() > tap_before);
    h.click(bounds(button).center());

    h.shell
        .run("midi.set_show_resync_button", json!({ "on": false }))
        .unwrap();
    let nodes = Harness::nodes(&h.settle());
    assert!(!labelled(&nodes, "Resync External Hardware"));
    assert_eq!(
        bounds(Harness::node(&nodes, "Tap Tempo")).left(),
        tap_before
    );
}

// Spec: midi-clock R12, R15, R27
#[test]
fn the_lower_sync_lamp_lights_for_a_quarter_note_of_clock_sent() {
    let mut h = Harness::manual(Window::Main);
    let test = TestBackend::new();
    test.add_port(Direction::Input, "Clock in");
    test.add_port(Direction::Output, "Drum machine");
    h.shell.use_midi_backend(Box::new(test));
    let name = "Sync Input and Output Indicators";
    let nodes = Harness::nodes(&h.settle());
    assert!(!labelled(&nodes, name), "hidden while no input has Sync on");

    h.shell
        .run(
            "midi.set_port",
            json!({ "direction": "input", "name": "Clock in", "sync": true }),
        )
        .unwrap();
    h.shell
        .run(
            "midi.set_port",
            json!({ "direction": "output", "name": "Drum machine", "sync": true }),
        )
        .unwrap();
    let nodes = Harness::nodes(&h.settle());
    assert_eq!(Harness::node(&nodes, name).value, "", "dark while stopped");

    h.shell.run("transport.play", json!({})).unwrap();
    h.wait("the lower sync lamp to light", |h| {
        h.shell.sync_output_lit()
    });
    let nodes = Harness::nodes(&h.settle());
    assert_eq!(Harness::node(&nodes, name).value, "sending");
    let state = h.shell.run("sync.state", json!({})).unwrap();
    assert_eq!(state["sending"], true);
    assert_eq!(state["output_ports"], 1);
    assert!(state["clocks_sent"].as_u64().unwrap() >= 1);
}

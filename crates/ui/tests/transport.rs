//! WP-1.18's acceptance in the window, headless: Space starts and stops
//! playback, the playhead the window draws is the one the engine published
//! as the frame began, and every control in the Control Bar's transport
//! section is reachable from the keyboard and runs a registered command
//! that agents run by ID. The shell plays through the null device, paced
//! like a sound card. Spec: control-bar-transport.

use crate::common;
use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use bl_core::time::Ticks;
use bl_shell::menus::{MenuPlatform, menu_bar};
use bl_shell::transport;
use bl_shell::transport_commands::{PhaseNudge, TransportState};
use bl_shell::{Area, Shell, commands as ids};
use bl_ui::control_bar::{self, SECTION};
use bl_ui::keymap::Platform;
use bl_ui::layout::{Element, REFERENCE, ShellLayout};
use bl_ui::main_window::{UiEvent, element_id};
use bl_ui::menus::item_state;
use common::{Harness, Window};
use egui::accesskit::{Action, Role};
use egui::{Event, Key, Modifiers, PointerButton, Pos2};
use serde_json::json;

const BAR: Ticks = Ticks::WHOLE;

/// Runs frames until `done` holds. The null stream is paced by hand, so the
/// engine advances with the frames and nothing here races the wall clock.
fn wait(h: &mut Harness, what: &str, done: impl Fn(&Harness) -> bool) {
    h.wait(what, done);
}

fn state(shell: &mut Shell) -> TransportState {
    serde_json::from_value(shell.run("transport.state", json!({})).unwrap()).unwrap()
}

/// Where an element is drawn, in a harness without a drawn title bar.
fn rect(element: Element) -> egui::Rect {
    ShellLayout::compute(REFERENCE, true, true).rect(element)
}

fn focus(h: &mut Harness, element: Element) {
    h.ctx.memory_mut(|m| m.request_focus(element_id(element)));
    h.shell.set_focused_area(Area::ControlBar);
    h.settle();
}

fn press(h: &mut Harness, key: Key, modifiers: Modifiers) {
    h.key(key, modifiers);
}

/// Types characters into the focused field: digits, and Period.
fn type_text(h: &mut Harness, text: &str) {
    for c in text.chars() {
        let key = match c {
            '.' => Key::Period,
            ',' => Key::Comma,
            c => Key::from_name(&c.to_string()).expect("a digit"),
        };
        press(h, key, Modifiers::NONE);
    }
}

fn drag(h: &mut Harness, from: Pos2, dy: f32, modifiers: Modifiers) {
    let held = Event::ModifiersChanged(modifiers);
    h.frame(vec![held.clone(), Event::PointerMoved(from)]);
    h.frame(vec![
        held.clone(),
        Event::PointerButton {
            pos: from,
            button: PointerButton::Primary,
            pressed: true,
            modifiers,
        },
    ]);
    // Moves of 7 points or more, so the first is past egui's drag
    // threshold and the whole distance counts.
    let steps = ((dy.abs() / 7.0).floor() as i32).max(1);
    for step in 1..=steps {
        let at = from + egui::vec2(0.0, dy * step as f32 / steps as f32);
        h.frame(vec![held.clone(), Event::PointerMoved(at)]);
    }
    h.frame(vec![
        held,
        Event::PointerButton {
            pos: from + egui::vec2(0.0, dy),
            button: PointerButton::Primary,
            pressed: false,
            modifiers,
        },
    ]);
    h.settle();
}

#[test]
fn space_starts_and_stops_playback_in_the_app() {
    // Spec: control-bar-transport R1, R3; the acceptance's first part.
    let mut h = Harness::manual(Window::Main);
    h.settle();
    assert_eq!(h.shell.focused_area(), Area::MainView);
    press(&mut h, Key::Space, Modifiers::NONE);
    wait(&mut h, "playback", |h| h.engine.playing);
    let nodes = Harness::nodes(&h.frame(vec![]));
    assert_eq!(Harness::node(&nodes, "Play").value, "playing");
    // The Playback menu's first item reads Stop now, and runs Play/Stop.
    let menus = menu_bar(MenuPlatform::current());
    let playback = menus.iter().find(|m| m.title == "Playback").unwrap();
    let item = item_state(&playback.items[0], &h.shell, &h.keymap, Platform::current());
    assert_eq!(item.label, "Stop");
    assert!(item.enabled);
    assert_eq!(item.invocation.unwrap().id, "transport.play_stop");
    press(&mut h, Key::Space, Modifiers::NONE);
    wait(&mut h, "the stop", |h| !h.engine.playing);
    let stopped = h.engine;
    assert!(
        stopped.position > Ticks::ZERO,
        "Space never returns to 1.1.1"
    );
    let nodes = Harness::nodes(&h.frame(vec![]));
    assert_eq!(Harness::node(&nodes, "Play").value, "stopped");
    // Space works from the Control Bar too, and Shift-Space continues.
    focus(&mut h, Element::Tempo);
    press(&mut h, Key::Space, Modifiers::SHIFT);
    wait(&mut h, "Continue", |h| h.engine.playing);
    assert!(h.engine.position >= stopped.position);
    press(&mut h, Key::Space, Modifiers::NONE);
    wait(&mut h, "the stop", |h| !h.engine.playing);
    // Home returns to 1.1.1.
    press(&mut h, Key::Home, Modifiers::NONE);
    wait(&mut h, "the return", |h| {
        h.engine.insert_marker == Ticks::ZERO
    });
    h.shell.audio_mut().stop();
}

#[test]
fn the_playhead_moves_with_the_engine_within_a_frame() {
    // Spec: control-bar-transport R14; the acceptance's second part. Each
    // frame draws the playhead the engine had published when the frame
    // began, so it lags the engine by at most the frame's own time and an
    // audio block.
    let mut h = Harness::new(Window::Main);
    h.settle();
    h.shell.run("transport.play", json!({})).unwrap();
    wait(&mut h, "playback", |h| h.engine.playing);
    let block = 256; // The null device's buffer, in frames.
    let rate = 48_000.0;
    let mut frames_seen = 0;
    let mut moved = 0;
    let mut last = h.engine;
    let mut worst = 0u64;
    let mut late = Vec::new();
    for _ in 0..40 {
        std::thread::sleep(Duration::from_millis(16)); // A 60 Hz display.
        let began = Instant::now();
        let output = h.frame(vec![]);
        let published = h.engine;
        let (drawn, _) = h
            .main
            .arrangement()
            .drawn_playhead()
            .expect("the playhead is in view");
        // The frame drew exactly what the engine had published.
        assert_eq!(drawn, published.position);
        let nodes = Harness::nodes(&output);
        let bbs = h
            .shell
            .editor()
            .document()
            .settings()
            .time_signatures
            .to_bbs(published.position);
        assert_eq!(
            Harness::node(&nodes, "Arrangement Position Bars").value,
            bbs.bar.to_string()
        );
        assert_eq!(
            Harness::node(&nodes, "Arrangement Position Beats").value,
            bbs.beat.to_string()
        );
        // How far the engine had got by the end of the frame.
        let now = h.shell.audio_mut().transport_state();
        let allowed = (began.elapsed().as_secs_f64() * rate) as u64 + 2 * block;
        let behind = now.clock.saturating_sub(published.clock);
        worst = worst.max(behind);
        if behind > allowed {
            // A device thread that fell behind and caught up in a burst
            // (a loaded test machine) can outrun a frame now and then.
            late.push((behind, allowed));
        }
        assert!(published.clock >= last.clock);
        if published.clock > last.clock {
            assert!(published.position > last.position, "the playhead moves on");
            moved += 1;
        }
        last = published;
        frames_seen += 1;
    }
    assert!(
        moved >= frames_seen - 2,
        "{moved} of {frames_seen} frames moved"
    );
    assert!(
        late.len() <= 2,
        "frames drawn more than a frame and two blocks behind the engine (behind, allowed): \
         {late:?}"
    );
    eprintln!(
        "{frames_seen} frames, the playhead moved in {moved}; at most {worst} audio frames \
         ({:.1} ms) behind the engine at the end of a frame",
        worst as f64 / rate * 1000.0
    );
    h.shell.audio_mut().stop();
}

#[test]
fn follow_pages_the_view_with_the_engines_playhead() {
    // Spec: control-bar-transport R14; arrangement-view R10, driven by the
    // engine.
    let mut h = Harness::manual(Window::Main);
    h.settle();
    assert!(h.shell.arrangement().follow());
    h.shell
        .run("transport.locate", json!({ "position": BAR * 200 }))
        .unwrap();
    h.shell.run("transport.play", json!({})).unwrap();
    wait(&mut h, "the page to turn", |h| {
        h.main
            .arrangement()
            .drawn_playhead()
            .is_some_and(|(at, _)| at >= BAR * 200)
    });
    let view: bl_shell::arrangement_commands::ArrangementState =
        serde_json::from_value(h.shell.run("arrangement.state", json!({})).unwrap()).unwrap();
    assert!(
        view.visible.start >= BAR * 200 && view.visible.start < BAR * 201,
        "the playhead at the left edge: {:?}",
        view.visible
    );
    h.shell.audio_mut().stop();
}

#[test]
fn every_control_in_the_transport_section_is_reachable_from_the_keyboard() {
    // Spec: control-bar-transport R2, R11; the acceptance's third part.
    let mut h = Harness::manual(Window::Main);
    h.settle();
    // Option-0 (Alt-0) to the Control Bar; with Use Tab Key to Move Focus
    // on, Tab goes from control to control.
    h.shell
        .run(ids::FOCUS_TOGGLE_TAB_NAVIGATION, json!({}))
        .unwrap();
    press(&mut h, Key::Num0, Modifiers::ALT);
    h.settle();
    assert_eq!(h.shell.focused_area(), Area::ControlBar);
    let mut reached: BTreeSet<Element> = BTreeSet::new();
    for _ in 0..32 {
        let focused = h.ctx.memory(|m| m.focused());
        reached.extend(
            SECTION
                .iter()
                .copied()
                .filter(|el| Some(element_id(*el)) == focused),
        );
        press(&mut h, Key::Tab, Modifiers::NONE);
        h.settle();
    }
    let missed: Vec<_> = SECTION.iter().filter(|el| !reached.contains(el)).collect();
    assert!(missed.is_empty(), "Tab never reached {missed:?}");

    // And each works from the keyboard. Value fields: Up and Down
    // (Shift for fine steps), Delete, and typing.
    focus(&mut h, Element::Tempo);
    press(&mut h, Key::ArrowUp, Modifiers::NONE);
    assert_eq!(state(&mut h.shell).tempo, 121.0);
    press(&mut h, Key::ArrowDown, Modifiers::SHIFT);
    assert_eq!(state(&mut h.shell).tempo, 120.99);
    type_text(&mut h, "98.5");
    press(&mut h, Key::Enter, Modifiers::NONE);
    assert_eq!(state(&mut h.shell).tempo, 98.5);
    press(&mut h, Key::Delete, Modifiers::NONE);
    assert_eq!(state(&mut h.shell).tempo, 120.0);
    focus(&mut h, Element::SignatureNumerator);
    press(&mut h, Key::ArrowUp, Modifiers::NONE);
    focus(&mut h, Element::SignatureDenominator);
    press(&mut h, Key::ArrowUp, Modifiers::NONE);
    assert_eq!(state(&mut h.shell).time_signature, "5/8");
    type_text(&mut h, "4");
    press(&mut h, Key::Enter, Modifiers::NONE);
    assert_eq!(state(&mut h.shell).time_signature, "5/4");
    // In 5/4, 3.2.1 is two five-beat bars and a beat in.
    focus(&mut h, Element::PositionBars);
    type_text(&mut h, "3.2.1");
    press(&mut h, Key::Enter, Modifiers::NONE);
    wait(&mut h, "the locate", |h| {
        h.engine.insert_marker == Ticks::QUARTER * 11
    });
    assert_eq!(state(&mut h.shell).arrangement_position, "3.2.1");
    focus(&mut h, Element::LoopStartBars);
    press(&mut h, Key::ArrowUp, Modifiers::NONE);
    focus(&mut h, Element::LoopLengthBeats);
    press(&mut h, Key::ArrowUp, Modifiers::NONE);
    let shown = state(&mut h.shell);
    assert_eq!(shown.loop_start_position, "2.1.1");
    // Five beats a bar now: four 4/4 bars are three 5/4 bars and a beat;
    // one more beat makes two.
    assert_eq!(shown.loop_length_bars, "3.2.0");
    // Buttons and switches: Enter presses the focused one.
    for (element, check) in [
        (Element::Loop, "the loop on"),
        (Element::Play, "playback"),
        (Element::Stop, "the stop"),
        (Element::Follow, "Follow off"),
    ] {
        focus(&mut h, element);
        press(&mut h, Key::Enter, Modifiers::NONE);
        match element {
            Element::Loop => assert!(state(&mut h.shell).loop_on, "{check}"),
            Element::Play => wait(&mut h, check, |h| h.engine.playing),
            Element::Stop => wait(&mut h, check, |h| !h.engine.playing),
            _ => assert!(!h.shell.arrangement().follow(), "{check}"),
        }
    }
    h.shell.audio_mut().stop();
}

#[test]
fn every_control_in_the_transport_section_is_reachable_from_the_agent_api() {
    // Spec: control-bar-transport R12. Each control runs a registered
    // command with a schema, a description, and a permission class; the
    // window's changes and an agent's runs are the same command.
    let mut h = Harness::manual(Window::Main);
    h.settle();
    for element in SECTION {
        let id = control_bar::command(*element).unwrap();
        let spec = h
            .shell
            .registry()
            .get(id)
            .unwrap_or_else(|| panic!("{element:?}: {id} isn't registered"));
        assert!(
            !spec.description.is_empty() && spec.permission.is_some() && spec.args.is_some(),
            "{id}"
        );
        if let Some(field) = control_bar::field(*element) {
            let change = transport::step(&h.shell, field, 1, false)
                .or_else(|| transport::step(&h.shell, field, -1, false))
                .unwrap_or_else(|| panic!("{element:?} changes"));
            assert_eq!(change.id, id, "{element:?}");
        }
    }
    // An agent runs them by ID with JSON, as `bl-agent` does.
    let shell = &mut h.shell;
    for (id, args) in [
        (
            "song.set_tempo",
            json!({ "tempo": serde_json::to_value(
            bl_core::time::TempoMap::constant(bl_core::time::Bpm::new(132.0).unwrap())).unwrap() }),
        ),
        (
            "song.set_time_signatures",
            json!({ "time_signatures": serde_json::to_value(
            bl_core::time::TimeSignatureMap::constant(
                bl_core::time::TimeSignature::new(3, 4).unwrap())).unwrap() }),
        ),
        ("transport.locate", json!({ "position": BAR * 4 })),
        (
            "transport.set_loop",
            json!({ "start": BAR * 2, "length": BAR }),
        ),
        ("transport.set_loop_enabled", json!({ "enabled": true })),
        ("arrangement.follow", json!({ "on": false })),
        ("transport.play", json!({})),
    ] {
        shell.run(id, args).unwrap_or_else(|e| panic!("{id}: {e}"));
    }
    wait(&mut h, "playback", |h| h.engine.playing);
    let shown = state(&mut h.shell);
    assert_eq!(shown.tempo, 132.0);
    assert_eq!(shown.time_signature, "3/4");
    assert_eq!(shown.insert_marker, BAR * 4);
    assert_eq!(
        (shown.loop_start, shown.loop_length, shown.loop_on),
        (BAR * 2, BAR, true)
    );
    assert!(!h.shell.arrangement().follow());
    h.shell.run("transport.stop", json!({})).unwrap();
    wait(&mut h, "the stop", |h| !h.engine.playing);
    // And the window shows what the agent did.
    let nodes = Harness::nodes(&h.frame(vec![]));
    assert_eq!(Harness::node(&nodes, "Tempo").value, "132.00");
    assert_eq!(Harness::node(&nodes, "Time Signature Numerator").value, "3");
    assert_eq!(Harness::node(&nodes, "Arrangement Loop").value, "on");
    // Eight quarter notes in, the loop starts at 3/4's 3.3.1.
    assert_eq!(Harness::node(&nodes, "Loop Start Bars").value, "3");
    assert_eq!(Harness::node(&nodes, "Loop Start Beats").value, "3");
    h.shell.audio_mut().stop();
}

#[test]
fn typed_keys_go_to_the_field_and_esc_drops_them() {
    // Spec: control-bar-transport R9, R10 (I-FOCUS-002, S-VAL-005 to 008).
    let mut h = Harness::manual(Window::Main);
    h.settle();
    h.click(rect(Element::PositionBars).center());
    assert_eq!(h.shell.focused_area(), Area::ControlBar);
    assert_eq!(
        h.ctx.memory(|m| m.focused()),
        Some(element_id(Element::PositionBars))
    );
    type_text(&mut h, "1");
    assert!(h.main.is_typing());
    // While typing, Option-2 types 2 rather than moving the focus, and
    // Space doesn't play.
    press(&mut h, Key::Num2, Modifiers::ALT);
    press(&mut h, Key::Space, Modifiers::NONE);
    let nodes = Harness::nodes(&h.frame(vec![]));
    assert_eq!(
        Harness::node(&nodes, "Arrangement Position Bars").value,
        "1"
    );
    assert!(!h.shell.playing());
    assert_eq!(h.shell.focused_area(), Area::ControlBar);
    let drawn: Vec<_> = h
        .events
        .iter()
        .filter(|e| matches!(e, UiEvent::Invoke(i) if i.id.starts_with("transport.")))
        .collect();
    assert!(drawn.is_empty(), "{drawn:?}");
    press(&mut h, Key::Enter, Modifiers::NONE);
    assert!(!h.main.is_typing());
    wait(&mut h, "the locate", |h| h.engine.insert_marker == BAR * 11);
    // Esc drops a typed value; so does moving the focus away.
    type_text(&mut h, "5");
    press(&mut h, Key::Escape, Modifiers::NONE);
    assert!(!h.main.is_typing());
    type_text(&mut h, "7");
    h.click(rect(Element::Tempo).center());
    assert!(!h.main.is_typing());
    h.settle();
    assert_eq!(h.engine.insert_marker, BAR * 11, "neither moved it");
    // Backspace takes back a digit.
    h.click(rect(Element::LoopLengthBars).center());
    type_text(&mut h, "29");
    press(&mut h, Key::Backspace, Modifiers::NONE);
    press(&mut h, Key::Enter, Modifiers::NONE);
    assert_eq!(state(&mut h.shell).loop_length_bars, "2.0.0");
    h.shell.audio_mut().stop();
}

#[test]
fn dragging_a_field_steps_it_as_one_undo_step() {
    // Spec: control-bar-transport R8 (I-DRAG-022, S-VAL-003, I-CLICK-021).
    let mut h = Harness::manual(Window::Main);
    h.settle();
    let tempo = rect(Element::Tempo).center();
    drag(&mut h, tempo, -30.0, Modifiers::NONE);
    assert_eq!(state(&mut h.shell).tempo, 130.0, "a step every 3 points");
    assert_eq!(
        h.shell.editor().undo_history().collect::<Vec<_>>(),
        ["Change tempo"],
        "one undo step"
    );
    drag(&mut h, tempo, 15.0, Modifiers::SHIFT);
    assert_eq!(state(&mut h.shell).tempo, 129.95, "Shift for fine steps");
    // A double-click returns the default.
    let double = |h: &mut Harness, at: Pos2| {
        for _ in 0..2 {
            for pressed in [true, false] {
                h.frame(vec![Event::PointerButton {
                    pos: at,
                    button: PointerButton::Primary,
                    pressed,
                    modifiers: Modifiers::NONE,
                }]);
            }
        }
        h.settle();
    };
    h.frame(vec![Event::PointerMoved(tempo)]);
    double(&mut h, tempo);
    assert_eq!(state(&mut h.shell).tempo, 120.0);
    // Dragging the position moves the insert marker, bar by bar.
    drag(
        &mut h,
        rect(Element::PositionBars).center(),
        -9.0,
        Modifiers::NONE,
    );
    wait(&mut h, "the locate", |h| h.engine.insert_marker == BAR * 3);
    h.shell.run("edit.undo", json!({})).unwrap();
    assert_eq!(state(&mut h.shell).tempo, 129.95, "the double-click's step");
    h.shell.audio_mut().stop();
}

#[test]
fn the_section_reaches_assistive_technology() {
    // Spec: control-bar-transport R2, R11.
    let mut h = Harness::manual(Window::Main);
    let nodes = Harness::nodes(&h.settle());
    for element in SECTION {
        let placement = bl_ui::layout::placement(*element);
        let node = Harness::node(&nodes, placement.name);
        assert!(!node.disabled, "{} works", placement.name);
        if control_bar::field(*element).is_some() {
            assert_eq!(node.role, Role::SpinButton, "{}", placement.name);
            assert!(node.numeric.is_some(), "{}", placement.name);
            assert!(!node.value.is_empty(), "{}", placement.name);
            assert!(!node.description.is_empty(), "{}", placement.name);
        }
    }
    assert_eq!(Harness::node(&nodes, "Play").value, "stopped");
    assert_eq!(Harness::node(&nodes, "Arrangement Loop").value, "off");
    assert_eq!(Harness::node(&nodes, "Punch-In").value, "off");
    assert_eq!(Harness::node(&nodes, "Punch-Out").value, "off");
    assert_eq!(Harness::node(&nodes, "Loop Length Bars").value, "4");
    assert!(
        Harness::node(&nodes, "Arrangement Position Bars")
            .description
            .contains("1.1.1")
    );
    // Increment and decrement, as a screen reader sends them.
    h.accesskit(element_id(Element::Tempo), Action::Increment);
    h.settle();
    assert_eq!(state(&mut h.shell).tempo, 121.0);
    h.accesskit(element_id(Element::LoopLengthBars), Action::Decrement);
    h.settle();
    assert_eq!(state(&mut h.shell).loop_length_bars, "3.0.0");
    // Pressing Play as a screen reader does.
    h.accesskit(element_id(Element::Play), Action::Click);
    wait(&mut h, "playback", |h| h.engine.playing);
    h.accesskit(element_id(Element::Stop), Action::Click);
    wait(&mut h, "the stop", |h| !h.engine.playing);
    // The inert controls say which package brings them.
    for (name, package) in [
        ("Arrangement Record", "WP-2.13"),
        ("MIDI Arrangement Overdub", "WP-2.14"),
        ("Tap Tempo", "M2"),
    ] {
        let node = Harness::node(&nodes, name);
        assert!(
            node.disabled && node.description.contains(package),
            "{name}"
        );
    }
    // Metronome is now active (WP-2.15).
    let metro = Harness::node(&nodes, "Metronome");
    assert!(!metro.disabled, "Metronome should be active");
    assert!(!state(&mut h.shell).metronome.enabled);

    // Toggling via shortcut 'O'.
    press(&mut h, Key::O, Modifiers::NONE);
    h.settle();
    assert!(state(&mut h.shell).metronome.enabled);

    // Toggling via clicking the metronome switch.
    h.accesskit(element_id(Element::Metronome), Action::Click);
    h.settle();
    assert!(!state(&mut h.shell).metronome.enabled);

    // Punch-In and Punch-Out are active (WP-2.13): each click switches its
    // own setting, and the Loop switch stays as it was.
    for element in [Element::PunchIn, Element::PunchOut] {
        h.accesskit(element_id(element), Action::Click);
        h.settle();
    }
    let document = h.shell.editor().document();
    assert!(document.settings().punch_in && document.settings().punch_out);
    assert!(!document.settings().arrangement_loop.enabled);

    h.shell.audio_mut().stop();
}

/// Presses the primary button at the middle of `element`.
fn press_button(h: &mut Harness, element: Element) {
    let at = rect(element).center();
    h.frame(vec![Event::PointerMoved(at)]);
    h.frame(vec![Event::PointerButton {
        pos: at,
        button: PointerButton::Primary,
        pressed: true,
        modifiers: Modifiers::NONE,
    }]);
}

/// Lets go of the primary button at the middle of `element`.
fn release_button(h: &mut Harness, element: Element) {
    h.frame(vec![Event::PointerButton {
        pos: rect(element).center(),
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    }]);
}

fn phase_nudge(shell: &mut Shell) -> PhaseNudge {
    state(shell).phase_nudge
}

/// The pressed state a screen reader reads of a Phase Nudge button.
fn toggled(h: &mut Harness, name: &str) -> Option<bool> {
    Harness::node(&Harness::nodes(&h.frame(vec![])), name).toggled
}

#[test]
fn the_phase_nudge_buttons_nudge_while_the_mouse_holds_them_and_show_pressed() {
    // Spec: transport R27, R28. Nothing in the set changes.
    let mut h = Harness::manual(Window::Main);
    h.settle();
    h.shell.run("transport.play", json!({})).unwrap();
    wait(&mut h, "playback", |h| h.engine.playing);
    let undoable = h.shell.editor().undo_history().count();
    let tempo = state(&mut h.shell).tempo;
    for (element, name, held) in [
        (Element::NudgeUp, "Phase Nudge Up", PhaseNudge::Up),
        (Element::NudgeDown, "Phase Nudge Down", PhaseNudge::Down),
    ] {
        assert_eq!(toggled(&mut h, name), Some(false), "{name} rests");
        press_button(&mut h, element);
        wait(&mut h, name, |h| h.engine.phase_nudge.is_some());
        assert_eq!(phase_nudge(&mut h.shell), held);
        assert_eq!(toggled(&mut h, name), Some(true), "{name} shows pressed");
        let nodes = Harness::nodes(&h.frame(vec![]));
        assert_eq!(Harness::node(&nodes, name).value, "pressed");
        release_button(&mut h, element);
        wait(&mut h, name, |h| h.engine.phase_nudge.is_none());
        assert_eq!(phase_nudge(&mut h.shell), PhaseNudge::None);
        assert_eq!(toggled(&mut h, name), Some(false), "{name} lets go");
    }
    // The Tempo field and the set stay as they were, and no undo step is made.
    assert_eq!(state(&mut h.shell).tempo, tempo);
    assert_eq!(h.shell.editor().undo_history().count(), undoable);
    h.shell.audio_mut().stop();
}

#[test]
fn a_phase_nudge_ends_when_the_window_loses_the_mouse_or_the_focus() {
    // Spec: transport R28.
    let mut h = Harness::manual(Window::Main);
    h.settle();
    h.shell.run("transport.play", json!({})).unwrap();
    wait(&mut h, "playback", |h| h.engine.playing);
    for lost in [Event::PointerGone, Event::WindowFocused(false)] {
        press_button(&mut h, Element::NudgeUp);
        wait(&mut h, "the nudge", |h| h.engine.phase_nudge.is_some());
        h.frame(vec![lost]);
        wait(&mut h, "the nudge to end", |h| {
            h.engine.phase_nudge.is_none()
        });
        // The button is still down, but the nudge doesn't start again.
        h.settle();
        assert_eq!(phase_nudge(&mut h.shell), PhaseNudge::None);
        release_button(&mut h, Element::NudgeUp);
        h.settle();
        assert_eq!(phase_nudge(&mut h.shell), PhaseNudge::None);
    }
    h.shell.audio_mut().stop();
}

#[test]
fn the_phase_nudge_buttons_are_disabled_and_refused_while_an_outside_tempo_leads() {
    // Spec: transport R29.
    let mut h = Harness::manual(Window::Main);
    let nodes = Harness::nodes(&h.settle());
    for name in ["Phase Nudge Down", "Phase Nudge Up"] {
        let node = Harness::node(&nodes, name);
        assert_eq!(node.role, Role::Button, "{name}");
        assert!(!node.disabled, "{name} works with no follower");
    }
    h.shell.run("transport.play", json!({})).unwrap();
    wait(&mut h, "playback", |h| h.engine.playing);
    // A nudge held when a follower arrives can still end.
    press_button(&mut h, Element::NudgeDown);
    wait(&mut h, "the nudge", |h| h.engine.phase_nudge.is_some());
    h.shell
        .audio()
        .set_follower(Some(Box::new(bl_engine::testing::following(140.0))));
    wait(&mut h, "the follower", |h| h.engine.following);
    release_button(&mut h, Element::NudgeDown);
    wait(&mut h, "the release", |h| h.engine.phase_nudge.is_none());
    let nodes = Harness::nodes(&h.frame(vec![]));
    for name in ["Phase Nudge Down", "Phase Nudge Up"] {
        assert!(Harness::node(&nodes, name).disabled, "{name} is off");
    }
    // A press does nothing, and the command is refused.
    press_button(&mut h, Element::NudgeUp);
    h.settle();
    release_button(&mut h, Element::NudgeUp);
    h.settle();
    assert_eq!(phase_nudge(&mut h.shell), PhaseNudge::None);
    let refused = h
        .shell
        .run(
            "transport.phase_nudge",
            json!({ "direction": "up", "held": true }),
        )
        .unwrap_err()
        .to_string();
    assert!(refused.contains("leads the tempo"), "{refused}");
    h.shell
        .run(
            "transport.phase_nudge",
            json!({ "direction": "up", "held": false }),
        )
        .unwrap();
    // When the follower goes, the buttons work again.
    h.shell.audio().set_follower(None);
    wait(&mut h, "the follower to go", |h| !h.engine.following);
    let nodes = Harness::nodes(&h.frame(vec![]));
    assert!(!Harness::node(&nodes, "Phase Nudge Up").disabled);
    h.shell.audio_mut().stop();
}

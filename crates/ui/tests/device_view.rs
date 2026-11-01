//! The Device View (WP-2.4), headless: the selected track's chain in
//! frames with Live's title bar (L-DEV-001, L-DEV-003, L-DEV-004, L-DEV-006,
//! L-DEV-009), selecting, folding, switching off, renaming, moving, the
//! context menu, and hot-swapping's bar, each running the registered
//! command an agent runs. Devices are the engine's test devices.
//!
//! Spec: device-chains R6, R10, R11, R19, R21 to R25.

use crate::common;
use bl_engine::testing;
use bl_model::{DeviceId, TrackId};
use bl_shell::Area;
use bl_shell::device_commands::{DEVICE_GAP, DeviceAdded, TITLE_BAR_HEIGHT};
use bl_shell::menus::MenuPlatform;
use bl_shell::menus::menu_bar;
use bl_ui::device_view::{
    EMPTY_TEXT, activator_rect, button_rects, control_id, layout, title_rect, title_switch_rect,
};
use bl_ui::keymap::Platform;
use bl_ui::main_window::UiEvent;
use bl_ui::menus::item_state;
use common::{Harness, Node, Window, command};
use egui::accesskit::Role;
use egui::{Event, Key, Modifiers, PointerButton, Pos2, Rect, pos2};
use serde_json::{Value, json};

fn harness() -> Harness {
    let mut h = Harness::with_title_bar(Window::Main);
    h.settle();
    let audio: Vec<TrackId> = h.shell.editor().document().top_level().collect();
    h.shell
        .invoke("arrangement.select_tracks", json!({ "tracks": [audio[2]] }))
        .unwrap();
    h.settle();
    h
}

fn gain() -> Value {
    let t = testing::gain_type();
    json!({ "kind": t.kind, "name": t.name, "role": t.role })
}

fn through() -> Value {
    let t = testing::through_type();
    json!({ "kind": t.kind, "name": t.name, "role": t.role })
}

/// A native instrument, for the menus that only instruments offer (R64).
fn instrument() -> Value {
    let t = bl_devices::polysynth_type();
    json!({ "kind": t.kind, "name": t.name, "role": t.role })
}

fn add(h: &mut Harness, device: Value) -> DeviceId {
    let added: DeviceAdded = serde_json::from_value(
        h.shell
            .run("device.add", json!({ "device": device }))
            .unwrap(),
    )
    .unwrap();
    h.settle();
    added.device
}

fn device_view(h: &mut Harness) -> Rect {
    let nodes = Harness::nodes(&h.settle());
    let b = Harness::node(&nodes, "Device View").bounds.unwrap();
    Rect::from_min_max(
        pos2(b.x0 as f32, b.y0 as f32),
        pos2(b.x1 as f32, b.y1 as f32),
    )
}

fn frame_of(h: &mut Harness, device: DeviceId) -> Rect {
    let view = device_view(h);
    layout(&h.shell, view)
        .into_iter()
        .find(|(id, _)| *id == device)
        .unwrap()
        .1
}

fn folded(h: &Harness, device: DeviceId) -> bool {
    h.shell
        .device_view_state()
        .devices
        .iter()
        .any(|d| d.id == device && d.folded)
}

fn press(at: Pos2, pressed: bool, button: PointerButton) -> Event {
    Event::PointerButton {
        pos: at,
        button,
        pressed,
        modifiers: Modifiers::NONE,
    }
}

fn double_click(h: &mut Harness, at: Pos2) {
    // Long enough after any earlier click that egui doesn't pair them.
    h.time += 1.0;
    h.frame(vec![Event::PointerMoved(at)]);
    h.frame(vec![
        press(at, true, PointerButton::Primary),
        press(at, false, PointerButton::Primary),
    ]);
    h.frame(vec![
        press(at, true, PointerButton::Primary),
        press(at, false, PointerButton::Primary),
    ]);
    h.settle();
}

fn right_click(h: &mut Harness, at: Pos2) -> egui::FullOutput {
    h.frame(vec![Event::PointerMoved(at)]);
    h.frame(vec![press(at, true, PointerButton::Secondary)]);
    h.frame(vec![press(at, false, PointerButton::Secondary)]);
    h.settle()
}

fn last_step(h: &Harness) -> String {
    h.shell
        .editor()
        .undo_history()
        .next()
        .unwrap_or_default()
        .to_owned()
}

// Spec: device-chains R21 (L-DEV-001)
#[test]
fn an_empty_chain_says_where_to_drop() {
    let mut h = harness();
    let nodes = Harness::nodes(&h.settle());
    let view = Harness::node(&nodes, "Device View");
    assert_eq!(view.role, Role::Group);
    assert_eq!(view.description, EMPTY_TEXT);
    let b = view.bounds.unwrap();
    // L-DEV-001: right of the Info View, at 1600 x 1000.
    assert_eq!((b.x0.round(), b.x1.round()), (249.0, 1591.0));
}

// Spec: device-chains R22 (L-DEV-003, L-DEV-004)
#[test]
fn devices_sit_left_to_right_with_lives_title_bar() {
    let mut h = harness();
    let a = add(&mut h, gain());
    let b = add(&mut h, through());
    let view = device_view(&mut h);
    let (fa, fb) = (frame_of(&mut h, a), frame_of(&mut h, b));
    assert_eq!(fa.left(), view.left() + DEVICE_GAP);
    assert_eq!(fb.left() - fa.right(), DEVICE_GAP, "about 7 points apart");
    // The title bar, left to right: the activator, the name, then
    // Hot-Swap Presets, Save Preset, and Show Options at the right.
    let activator = activator_rect(fa, false);
    let title = title_rect(fa, false);
    let [hot_swap, save, options] = button_rects(fa, false);
    assert!(activator.right() < title.left());
    assert!(title.right() < hot_swap.left());
    assert!(hot_swap.right() < save.left() && save.right() < options.left());
    assert!(options.right() <= fa.right());
    for rect in [activator, title, hot_swap, save, options] {
        assert!(rect.top() >= fa.top() && rect.bottom() <= fa.top() + TITLE_BAR_HEIGHT);
    }
    // Each control is reachable and named as Live's Info View names it.
    let nodes = Harness::nodes(&h.settle());
    let frame = Harness::node(&nodes, "Test Gain");
    for label in [
        "Device Activator",
        "Hot-Swap Presets",
        "Save Preset",
        "Show Options",
    ] {
        let found = nodes
            .iter()
            .filter(|n| n.label == label)
            .any(|n| Harness::descends_from(&nodes, n.id, frame.id) || n.role == Role::Button);
        assert!(found, "{label}");
    }
}

// Spec: device-chains R11, R23
#[test]
fn clicking_selects_double_clicking_folds() {
    let mut h = harness();
    let a = add(&mut h, gain());
    let b = add(&mut h, through());
    assert_eq!(
        h.shell.devices().selected(),
        Some(b),
        "a new device is selected"
    );
    let fa = frame_of(&mut h, a);
    h.click(title_rect(fa, false).center());
    h.settle();
    assert_eq!(h.shell.devices().selected(), Some(a));

    let steps = h.shell.editor().undo_history().len();
    double_click(&mut h, title_rect(fa, false).center());
    assert!(folded(&h, a));
    let folded_frame = frame_of(&mut h, a);
    assert_eq!(
        folded_frame.width(),
        TITLE_BAR_HEIGHT,
        "a strip a title bar wide"
    );
    // The next device moves up to it.
    assert_eq!(
        frame_of(&mut h, b).left() - folded_frame.right(),
        DEVICE_GAP
    );
    double_click(&mut h, title_rect(folded_frame, true).center());
    assert!(!folded(&h, a));
    assert_eq!(
        h.shell.editor().undo_history().len(),
        steps,
        "folding isn't an undo step"
    );
}

// Spec: device-chains R6
#[test]
fn the_activator_switches_the_device() {
    let mut h = harness();
    let a = add(&mut h, gain());
    let fa = frame_of(&mut h, a);
    h.click(activator_rect(fa, false).center());
    h.settle();
    assert!(!h.shell.editor().document().device(a).unwrap().enabled);
    assert_eq!(last_step(&h), "Change \"Device On\"");
    let nodes = Harness::nodes(&h.settle());
    assert_eq!(Harness::node(&nodes, "Device Activator").value, "off");
}

// Spec: device-chains R24 (C-DEVICE)
#[test]
fn the_context_menu_lists_lives_items_and_names_later_packages() {
    let mut h = harness();
    let a = add(&mut h, gain());
    let fa = frame_of(&mut h, a);
    let out = right_click(&mut h, title_rect(fa, false).center());
    assert_eq!(h.shell.focused_area(), Area::Detail);
    let nodes = Harness::nodes(&out);
    let item = |label: &str| {
        nodes
            .iter()
            .find(|n| n.label.trim_start_matches("✔ ").starts_with(label))
            .unwrap_or_else(|| panic!("no menu item {label:?}"))
            .clone()
    };
    for label in [
        "Cut",
        "Copy",
        "Duplicate",
        "Rename",
        "Delete",
        "Group",
        "Fold",
        "Show Preset Name",
        "Compare: Switch to B",
        "Compare: Copy A to B",
        "Save as Default Preset",
    ] {
        assert!(!item(label).disabled, "{label} works");
    }
    // Show Preset Name is ticked on a new device (observed).
    assert!(item("Show Preset Name").label.starts_with('✔'));
    // Spec: info-view R10. Edit Info Text works since WP-6.58.
    let node: Node = item("Edit Info Text");
    assert!(!node.disabled, "Edit Info Text works");
}

// Spec: multisampler R64 (C-DEVICE-015)
#[test]
fn an_instruments_menu_offers_autosample() {
    let mut h = harness();
    // An effect's menu doesn't offer it.
    let effect = add(&mut h, gain());
    let frame = frame_of(&mut h, effect);
    let out = right_click(&mut h, title_rect(frame, false).center());
    assert!(
        !Harness::nodes(&out)
            .iter()
            .any(|node| node.label.starts_with("Autosample")),
        "an effect's menu doesn't offer Autosample…"
    );
    // An instrument's does, and choosing it opens the dialog for that
    // instrument.
    let instrument = add(&mut h, instrument());
    let frame = frame_of(&mut h, instrument);
    let out = right_click(&mut h, title_rect(frame, false).center());
    let nodes = Harness::nodes(&out);
    let item = nodes
        .iter()
        .find(|node| node.label.starts_with("Autosample"))
        .expect("the instrument's menu offers Autosample…");
    assert!(!item.disabled, "Autosample… works");
    let bounds = item.bounds.expect("the item's bounds");
    let at = pos2(
        ((bounds.x0 + bounds.x1) / 2.0) as f32,
        ((bounds.y0 + bounds.y1) / 2.0) as f32,
    );
    h.click(at);
    let dialog = h
        .shell
        .autosampler_dialog()
        .expect("the autosampler's dialog opens");
    assert_eq!(
        dialog.device, instrument,
        "the dialog is open for the instrument it was chosen on"
    );
}

// Spec: device-chains R24 (Show Options opens the same menu)
#[test]
fn show_options_opens_the_menu() {
    let mut h = harness();
    let a = add(&mut h, gain());
    let fa = frame_of(&mut h, a);
    h.click(button_rects(fa, false)[2].center());
    let nodes = Harness::nodes(&h.settle());
    assert!(
        nodes
            .iter()
            .any(|n| n.label.starts_with("Save as Default Preset"))
    );
}

// Spec: device-chains R10, R20 (keys act on the selected device)
#[test]
fn rename_and_delete_from_the_keyboard() {
    let mut h = harness();
    let a = add(&mut h, gain());
    h.shell.set_focused_area(Area::Detail);
    h.key(Key::R, command());
    h.frame(vec![Event::Key {
        key: Key::A,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: command(),
    }]);
    h.frame(vec![Event::Text("Trim".to_owned())]);
    h.key(Key::Enter, Modifiers::NONE);
    h.settle();
    assert_eq!(
        h.shell.editor().document().device(a).unwrap().title(),
        "Trim"
    );
    assert_eq!(last_step(&h), "Change \"Device Name\"");

    h.shell.set_focused_area(Area::Detail);
    h.key(Key::Delete, Modifiers::NONE);
    h.settle();
    assert!(h.shell.editor().document().device(a).is_none());
    assert_eq!(last_step(&h), "Delete Device(s)");
}

// Spec: device-chains R10 (a rename never leaves the keys with a field that is gone)
#[test]
fn a_rename_ends_when_its_device_goes() {
    let mut h = harness();
    let a = add(&mut h, gain());
    h.shell.set_focused_area(Area::Detail);
    h.key(Key::R, command());
    assert!(h.main.is_typing(), "the title is a text field");
    h.shell
        .run("device.remove", json!({ "device": a }))
        .unwrap();
    h.settle();
    assert!(!h.main.is_typing(), "the keys are back");
}

// Spec: device-chains R25 (dragging a title bar moves the device)
#[test]
fn dragging_a_title_bar_moves_the_device() {
    let mut h = harness();
    let a = add(&mut h, gain());
    let b = add(&mut h, through());
    let (fa, fb) = (frame_of(&mut h, a), frame_of(&mut h, b));
    let from = title_rect(fa, false).center();
    let to = pos2(fb.right() + 2.0, fb.center().y);
    h.frame(vec![Event::PointerMoved(from)]);
    h.frame(vec![press(from, true, PointerButton::Primary)]);
    for step in 1..=6 {
        let t = step as f32 / 6.0;
        h.frame(vec![Event::PointerMoved(from + (to - from) * t)]);
    }
    h.frame(vec![press(to, false, PointerButton::Primary)]);
    h.settle();
    let order: Vec<DeviceId> = h
        .shell
        .device_view_state()
        .devices
        .iter()
        .map(|d| d.id)
        .collect();
    assert_eq!(order, [b, a]);
    assert_eq!(last_step(&h), "Move Device");
}

// Spec: device-chains R19 (the browser's bar while hot-swapping)
#[test]
fn hot_swap_shows_the_browsers_bar() {
    let mut h = harness();
    let a = add(&mut h, gain());
    let fa = frame_of(&mut h, a);
    h.click(button_rects(fa, false)[0].center());
    let nodes = Harness::nodes(&h.settle());
    let close = Harness::node(&nodes, "Close Hot-Swap");
    assert_eq!(close.description, "Swapping Audio Effect");
    assert_eq!(Harness::node(&nodes, "Hot-Swap Presets").value, "on");
    let id = egui::Id::new("hot-swap-close");
    h.accesskit(id, egui::accesskit::Action::Click);
    h.settle();
    assert!(h.shell.device_view_state().hot_swap.is_none());
    let events = h.take_events();
    assert!(events.iter().any(|e| matches!(
        e,
        UiEvent::Invoke(i) if i.id == "device.hot_swap"
    )));
    let _ = control_id(a, "hot_swap");
}

fn sidechained() -> Value {
    let t = testing::key_type();
    json!({ "kind": t.kind, "name": t.name, "role": t.role, "sidechain": t.sidechain })
}

fn section(h: &Harness, device: DeviceId) -> bl_shell::device_commands::SidechainSection {
    h.shell
        .device_view_state()
        .devices
        .into_iter()
        .find(|d| d.id == device)
        .and_then(|d| d.sidechain)
        .unwrap()
}

// Spec: routing R29, R30, R31 (a device's sidechain strip and section)
#[test]
fn the_sidechain_strip_shows_the_section_and_its_controls_set_the_sidechain() {
    use bl_ui::sidechain::{rows, strip_buttons};
    let mut h = harness();
    let plain = add(&mut h, gain());
    let key = add(&mut h, sidechained());
    // A device with a sidechain input is wider by its strip; one without
    // has none.
    let (fp, fk) = (frame_of(&mut h, plain), frame_of(&mut h, key));
    assert_eq!(
        fk.width() - fp.width(),
        bl_shell::device_commands::SIDECHAIN_STRIP_WIDTH
    );
    let nodes = Harness::nodes(&h.settle());
    assert_eq!(Harness::node(&nodes, "Show/Hide Sidechain").value, "off");
    assert_eq!(Harness::node(&nodes, "Sidechain Listen").value, "off");

    // The toggle shows the section, which changes only the view.
    let steps = h.shell.editor().undo_history().len();
    h.click(strip_buttons(fk)[0].center());
    h.settle();
    assert!(section(&h, key).shown);
    assert_eq!(h.shell.editor().undo_history().len(), steps);
    let fk = frame_of(&mut h, key);
    assert_eq!(
        fk.width() - fp.width(),
        bl_shell::device_commands::SIDECHAIN_STRIP_WIDTH
            + bl_shell::device_commands::SIDECHAIN_SECTION_WIDTH
    );
    let nodes = Harness::nodes(&h.settle());
    for (name, value) in [
        ("External", "off"),
        ("Sidechain Source", "No Input"),
        ("Mix", "100 %"),
        ("SC Gain", "0.00 dB"),
    ] {
        assert_eq!(Harness::node(&nodes, name).value, value, "{name}");
    }

    // External, then a source from the chooser's list.
    let [external, _, source, _, mix, _] = rows(fk);
    h.click(external.center());
    h.settle();
    assert!(section(&h, key).enabled);
    assert_eq!(last_step(&h), "Change \"Sidechain On\"");
    h.click(source.center());
    let nodes = Harness::nodes(&h.settle());
    let entry = nodes
        .iter()
        .find(|n| n.label == "3-Audio" && n.bounds.is_some_and(|b| b.y0 as f32 > source.top()))
        .expect("the list is open");
    let b = entry.bounds.unwrap();
    h.click(pos2(
        ((b.x0 + b.x1) / 2.0) as f32,
        ((b.y0 + b.y1) / 2.0) as f32,
    ));
    h.settle();
    assert_eq!(section(&h, key).source, "3-Audio");
    assert_eq!(last_step(&h), "Change \"Routing\"");

    // A drag on Mix is one undo step; a double-click sets it back.
    let steps = h.shell.editor().undo_history().len();
    let from = mix.center();
    h.frame(vec![Event::PointerMoved(from)]);
    h.frame(vec![press(from, true, PointerButton::Primary)]);
    for step in 1..=5 {
        h.frame(vec![Event::PointerMoved(
            from + egui::vec2(0.0, 5.0 * step as f32),
        )]);
    }
    h.frame(vec![press(
        from + egui::vec2(0.0, 25.0),
        false,
        PointerButton::Primary,
    )]);
    h.settle();
    let mix_now = section(&h, key).mix;
    assert!(mix_now < 1.0 && mix_now > 0.7, "{mix_now}");
    assert_eq!(h.shell.editor().undo_history().len(), steps + 1);
    assert_eq!(last_step(&h), "Change \"Mix\"");
    double_click(&mut h, mix.center());
    assert_eq!(section(&h, key).mix, 1.0);

    // Listen, from the strip.
    h.click(strip_buttons(fk)[1].center());
    h.settle();
    assert!(section(&h, key).listen);
}

fn midi_section(h: &Harness, device: DeviceId) -> bl_shell::device_commands::MidiSidechainSection {
    h.shell
        .device_view_state()
        .devices
        .into_iter()
        .find(|d| d.id == device)
        .and_then(|d| d.midi_sidechain)
        .unwrap()
}

// Spec: routing R34 (a device's MIDI sidechain strip and its MIDI source
// chooser)
#[test]
fn the_midi_sidechain_strip_shows_the_section_and_its_chooser_sets_the_source() {
    use bl_ui::sidechain::{rows, strip_buttons};
    let mut h = harness();
    let plain = add(&mut h, gain());
    let t = testing::hear_type();
    let hear = add(
        &mut h,
        json!({ "kind": t.kind, "name": t.name, "role": t.role, "midi_sidechain": true }),
    );
    let (fp, fh) = (frame_of(&mut h, plain), frame_of(&mut h, hear));
    assert_eq!(
        fh.width() - fp.width(),
        bl_shell::device_commands::SIDECHAIN_STRIP_WIDTH,
        "a MIDI sidechain has its strip"
    );
    let nodes = Harness::nodes(&h.settle());
    assert_eq!(Harness::node(&nodes, "Show/Hide Sidechain").value, "off");

    // The toggle shows the section, which changes only the view.
    let steps = h.shell.editor().undo_history().len();
    h.click(strip_buttons(fh)[0].center());
    h.settle();
    assert!(midi_section(&h, hear).shown);
    assert_eq!(h.shell.editor().undo_history().len(), steps);
    let fh = frame_of(&mut h, hear);
    assert_eq!(
        fh.width() - fp.width(),
        bl_shell::device_commands::SIDECHAIN_STRIP_WIDTH
            + bl_shell::device_commands::SIDECHAIN_SECTION_WIDTH
    );
    let nodes = Harness::nodes(&h.settle());
    assert_eq!(
        Harness::node(&nodes, "MIDI Sidechain Source").value,
        "No Input"
    );

    // A source from the chooser's list.
    let [_, source, ..] = rows(fh);
    h.click(source.center());
    let nodes = Harness::nodes(&h.settle());
    let entry = nodes
        .iter()
        .find(|n| n.label == "1-MIDI" && n.bounds.is_some_and(|b| b.y0 as f32 > source.top()))
        .expect("the list is open");
    let b = entry.bounds.unwrap();
    h.click(pos2(
        ((b.x0 + b.x1) / 2.0) as f32,
        ((b.y0 + b.y1) / 2.0) as f32,
    ));
    h.settle();
    assert_eq!(midi_section(&h, hear).source, "1-MIDI");
    assert_eq!(last_step(&h), "Change \"Routing\"");
}

fn corpus() -> Value {
    let t = bl_devices::corpus::corpus_type();
    json!({ "kind": t.kind, "name": t.name, "role": t.role, "midi_sidechain": true })
}

fn param_of(h: &Harness, device: DeviceId, param: bl_model::ParamId) -> f64 {
    let d = h.shell.editor().document().device(device).unwrap();
    d.params.get(&param).copied().unwrap_or_else(|| {
        bl_devices::corpus::corpus_type()
            .description
            .param(param)
            .unwrap()
            .default
    })
}

/// Picks the open list's entry labelled `label`, under `below`.
fn pick(h: &mut Harness, label: &str, below: Rect) {
    let nodes = Harness::nodes(&h.settle());
    let entry = nodes
        .iter()
        .find(|n| n.label == label && n.bounds.is_some_and(|b| b.y0 as f32 > below.top()))
        .unwrap_or_else(|| panic!("{label} is in the open list"));
    let b = entry.bounds.unwrap();
    h.click(pos2(
        ((b.x0 + b.x1) / 2.0) as f32,
        ((b.y0 + b.y1) / 2.0) as f32,
    ));
    h.settle();
}

/// A drag from `from` to `to` in a few steps.
fn drag(h: &mut Harness, from: Pos2, to: Pos2) {
    h.frame(vec![Event::PointerMoved(from)]);
    h.frame(vec![press(from, true, PointerButton::Primary)]);
    for step in 1..=5 {
        h.frame(vec![Event::PointerMoved(
            from + (to - from) * (step as f32 / 5.0),
        )]);
    }
    h.frame(vec![press(to, false, PointerButton::Primary)]);
    h.settle();
}

// Spec: corpus R1, R11 (Corpus's MIDI source and its tapping point), routing
// R39
#[test]
fn corpus_chooses_its_midi_source_and_its_tapping_point_in_its_sidechain_strip() {
    use bl_ui::sidechain::{rows, strip_buttons};
    let mut h = harness();
    let corpus = add(&mut h, corpus());
    let fc = frame_of(&mut h, corpus);
    h.click(strip_buttons(fc)[0].center());
    h.settle();
    let fc = frame_of(&mut h, corpus);
    let nodes = Harness::nodes(&h.settle());
    assert_eq!(
        Harness::node(&nodes, "MIDI Sidechain Source").value,
        "No Input"
    );
    assert_eq!(
        Harness::node(&nodes, "MIDI Sidechain Tapping Point").value,
        "Post FX",
        "a new Corpus takes the notes Post FX"
    );
    assert_eq!(midi_section(&h, corpus).tap.as_deref(), Some("Post FX"));
    let [_, source, tap, ..] = rows(fc);
    h.click(source.center());
    pick(&mut h, "1-MIDI", source);
    assert_eq!(midi_section(&h, corpus).source, "1-MIDI");
    assert_eq!(last_step(&h), "Change \"Routing\"");
    // The tapping point is one undo step too, and keeps the source.
    let steps = h.shell.editor().undo_history().len();
    h.click(tap.center());
    pick(&mut h, "Pre FX", tap);
    assert_eq!(midi_section(&h, corpus).tap.as_deref(), Some("Pre FX"));
    assert_eq!(midi_section(&h, corpus).source, "1-MIDI");
    assert_eq!(h.shell.editor().undo_history().len(), steps + 1);
    assert_eq!(last_step(&h), "Change \"Routing\"");
    let saved = h
        .shell
        .editor()
        .document()
        .device(corpus)
        .unwrap()
        .midi_sidechain
        .clone()
        .unwrap();
    assert_eq!(saved.tapped(), bl_model::Tap::PreFx);
    h.shell.run("edit.undo", json!({})).unwrap();
    h.settle();
    assert_eq!(midi_section(&h, corpus).tap.as_deref(), Some("Post FX"));
}

// Spec: corpus R1, R2 (the X–Y control, the dimmed controls)
#[test]
fn dragging_corpus_xy_control_sets_decay_and_material_or_radius_in_one_undo_step() {
    use bl_devices::corpus::{DECAY, HIT, MATERIAL, QUALITY, RADIUS, TYPE, unused};
    let mut h = harness();
    let corpus = add(&mut h, corpus());
    let cell = |h: &mut Harness| {
        let nodes = Harness::nodes(&h.settle());
        let b = Harness::node(&nodes, "X-Y").bounds.unwrap();
        Rect::from_min_max(
            pos2(b.x0 as f32, b.y0 as f32),
            pos2(b.x1 as f32, b.y1 as f32),
        )
    };
    let at = |r: Rect, across: f32, up: f32| {
        pos2(r.left() + across * r.width(), r.bottom() - up * r.height())
    };
    let r = cell(&mut h);
    let steps = h.shell.editor().undo_history().len();
    drag(&mut h, at(r, 0.5, 0.5), at(r, 0.8, 0.25));
    let (decay, material) = (param_of(&h, corpus, DECAY), param_of(&h, corpus, MATERIAL));
    assert!((decay - 80.0).abs() < 3.0, "Decay {decay}");
    assert!((material + 50.0).abs() < 6.0, "Material {material}");
    assert_eq!(
        h.shell.editor().undo_history().len(),
        steps + 1,
        "one undo step"
    );
    assert_eq!(last_step(&h), "Change \"Decay\" and \"Material\"");
    h.shell.run("edit.undo", json!({})).unwrap();
    assert_eq!(param_of(&h, corpus, DECAY), 50.0);
    assert_eq!(param_of(&h, corpus, MATERIAL), 0.0);
    // A pipe's vertical axis is Radius, and its dimmed controls are the
    // spec's.
    h.shell
        .run(
            "device.set_param",
            json!({ "device": corpus, "param": TYPE, "value": 5.0 }),
        )
        .unwrap();
    h.settle();
    let locked = h
        .shell
        .device_view_state()
        .devices
        .into_iter()
        .find(|d| d.id == corpus)
        .and_then(|d| d.panel)
        .unwrap()
        .locked;
    assert_eq!(locked, unused(5));
    assert!(locked.contains(&HIT) && locked.contains(&QUALITY) && !locked.contains(&RADIUS));
    let r = cell(&mut h);
    drag(&mut h, at(r, 0.5, 0.5), at(r, 0.25, 0.75));
    assert!((param_of(&h, corpus, DECAY) - 25.0).abs() < 3.0);
    assert!((param_of(&h, corpus, RADIUS) - 75.0).abs() < 6.0);
    assert_eq!(param_of(&h, corpus, MATERIAL), 0.0, "Material stays");
}

fn hardware() -> Value {
    let t = bl_devices::hardware_instrument_type();
    json!({ "kind": t.kind, "name": t.name, "role": t.role })
}

fn effect() -> Value {
    let t = bl_devices::hardware_effect_type();
    json!({ "kind": t.kind, "name": t.name, "role": t.role })
}

fn external_of(h: &Harness, device: DeviceId) -> bl_shell::device_commands::ExternalSection {
    h.shell
        .device_view_state()
        .devices
        .into_iter()
        .find(|d| d.id == device)
        .and_then(|d| d.external)
        .unwrap()
}

// Spec: external-devices R9, R12, R15 (an external instrument's section)
#[test]
fn the_external_instruments_choosers_route_it() {
    use bl_midi_io::{Direction, TestBackend};
    use bl_model::HardwareLatency;
    use bl_ui::external::rows;

    let mut h = harness();
    let test = TestBackend::new();
    test.add_port(Direction::Output, "Synth");
    h.shell.use_midi_backend(Box::new(test));
    h.shell
        .run(
            "midi.set_port",
            json!({ "direction": "output", "name": "Synth", "track": true }),
        )
        .unwrap();
    let device = add(&mut h, hardware());
    let frame = frame_of(&mut h, device);
    // A new device is routed nowhere: channel 1, the first input pair,
    // 0 ms; its section sits left of its body.
    let section = external_of(&h, device);
    assert_eq!(section.port, None);
    assert_eq!(section.channel, 1);
    assert_eq!(section.hardware_latency, HardwareLatency::default());
    let nodes = Harness::nodes(&h.settle());
    for (name, value) in [
        ("External MIDI To", "No Output"),
        ("External MIDI Channel", "1"),
        ("External Audio From", "1/2"),
        ("Hardware Latency", "0.00 ms"),
        ("Hardware Latency Unit", "ms"),
    ] {
        assert_eq!(Harness::node(&nodes, name).value, value, "{name}");
    }
    let [midi_to, channel, _, latency, unit] = rows(frame);

    // MIDI To lists the port; choosing it is a routing change.
    h.click(midi_to.center());
    let nodes = Harness::nodes(&h.settle());
    let entry = nodes
        .iter()
        .find(|n| n.label == "Synth" && n.bounds.is_some())
        .expect("the list is open");
    let b = entry.bounds.unwrap();
    h.click(pos2(
        ((b.x0 + b.x1) / 2.0) as f32,
        ((b.y0 + b.y1) / 2.0) as f32,
    ));
    h.settle();
    assert_eq!(external_of(&h, device).port.as_deref(), Some("Synth"));
    assert_eq!(last_step(&h), "Change \"Routing\"");

    // Its channel.
    h.click(channel.center());
    let nodes = Harness::nodes(&h.settle());
    let entry = nodes
        .iter()
        .find(|n| n.label == "5" && n.bounds.is_some_and(|b| b.y1 - b.y0 > 8.0))
        .expect("the list is open");
    let b = entry.bounds.unwrap();
    h.click(pos2(
        ((b.x0 + b.x1) / 2.0) as f32,
        ((b.y0 + b.y1) / 2.0) as f32,
    ));
    h.settle();
    assert_eq!(external_of(&h, device).channel, 5);

    // A drag on Hardware Latency is one undo step.
    let steps = h.shell.editor().undo_history().len();
    let from = latency.center();
    h.frame(vec![Event::PointerMoved(from)]);
    h.frame(vec![press(from, true, PointerButton::Primary)]);
    for step in 1..=5 {
        h.frame(vec![Event::PointerMoved(
            from - egui::vec2(0.0, 5.0 * step as f32),
        )]);
    }
    h.frame(vec![press(
        from - egui::vec2(0.0, 25.0),
        false,
        PointerButton::Primary,
    )]);
    h.settle();
    let HardwareLatency::Milliseconds { value } = external_of(&h, device).hardware_latency else {
        panic!("still in milliseconds");
    };
    assert!(value > 150.0 && value < 250.0, "{value}");
    assert_eq!(h.shell.editor().undo_history().len(), steps + 1);
    assert_eq!(last_step(&h), "Change \"Hardware Latency\"");

    // The unit switch converts the value at the open rate (48 kHz with no
    // device), to whole samples.
    h.click(unit.center());
    h.settle();
    let HardwareLatency::Samples { value: samples } = external_of(&h, device).hardware_latency
    else {
        panic!("samples now");
    };
    assert_eq!(f64::from(samples), (value * 48.0).round());
    let nodes = Harness::nodes(&h.settle());
    assert_eq!(Harness::node(&nodes, "Hardware Latency Unit").value, "smp");
}

// Spec: external-devices R17. The External Audio Effect sends its signal to
// the interface's outputs and hears it back: its section has an Audio To
// chooser and no MIDI To.
#[test]
fn the_external_effects_choosers_route_it() {
    let mut h = harness();
    let device = add(&mut h, effect());
    let frame = frame_of(&mut h, device);
    let section = external_of(&h, device);
    assert!(section.output, "the effect sends its signal out");
    assert_eq!(section.audio_to.as_deref(), Some("1/2"));
    assert_eq!(section.port, None, "the effect sends no notes");
    let nodes = Harness::nodes(&h.settle());
    for (name, value) in [
        ("External Audio To", "1/2"),
        ("External Audio From", "1/2"),
        ("Hardware Latency", "0.00 ms"),
        ("Hardware Latency Unit", "ms"),
    ] {
        assert_eq!(Harness::node(&nodes, name).value, value, "{name}");
    }
    assert!(
        Harness::nodes(&h.settle())
            .iter()
            .all(|n| n.label != "External MIDI To"),
        "the effect has no MIDI To"
    );
    let _ = frame;
}

// Spec: delay-compensation R7; external-devices R14. While Delay Compensation
// is off the Hardware Latency controls are disabled, keeping their value: a
// drag and the unit switch do nothing.
#[test]
fn hardware_latency_is_disabled_while_delay_compensation_is_off() {
    use bl_model::HardwareLatency;
    use bl_ui::external::rows;

    let mut h = harness();
    let device = add(&mut h, hardware());
    let frame = frame_of(&mut h, device);
    let [_, _, _, latency, unit] = rows(frame);
    // A value worth keeping.
    h.shell
        .run(
            "device.set_external",
            json!({
                "device": device,
                "hardware_latency": { "unit": "milliseconds", "value": 100.0 },
            }),
        )
        .unwrap();
    h.settle();
    h.shell
        .run("mixer.set_delay_compensation", json!({ "enabled": false }))
        .unwrap();
    let nodes = Harness::nodes(&h.settle());
    for name in ["Hardware Latency", "Hardware Latency Unit"] {
        let node = Harness::node(&nodes, name);
        assert!(node.disabled, "{name} is disabled");
        assert_eq!(
            node.description, "Unavailable while Delay Compensation is off",
            "{name} says why"
        );
    }
    // A drag on the box and a click on the switch change nothing.
    let steps = h.shell.editor().undo_history().len();
    let from = latency.center();
    h.frame(vec![Event::PointerMoved(from)]);
    h.frame(vec![press(from, true, PointerButton::Primary)]);
    for step in 1..=5 {
        h.frame(vec![Event::PointerMoved(
            from - egui::vec2(0.0, 5.0 * step as f32),
        )]);
    }
    h.frame(vec![press(
        from - egui::vec2(0.0, 25.0),
        false,
        PointerButton::Primary,
    )]);
    h.click(unit.center());
    h.settle();
    assert_eq!(
        external_of(&h, device).hardware_latency,
        HardwareLatency::Milliseconds { value: 100.0 },
        "the value is kept"
    );
    assert_eq!(h.shell.editor().undo_history().len(), steps, "no undo step");
}

/// A file dropped into the window, as the app's drag and drop gives egui.
#[derive(Debug)]
struct DroppedFile(std::path::PathBuf);

impl egui::DroppedFile for DroppedFile {
    fn path(&self) -> &std::path::Path {
        &self.0
    }

    fn bytes(&self) -> Result<Vec<u8>, String> {
        std::fs::read(&self.0).map_err(|error| error.to_string())
    }
}

// Spec: sampler R11
#[test]
fn a_dropped_sample_loads_into_the_sampler_under_the_pointer() {
    let mut h = harness();
    let sampler = bl_devices::sampler_type();
    let device = add(
        &mut h,
        json!({ "kind": sampler.kind, "name": sampler.name, "role": sampler.role }),
    );
    let frame = frame_of(&mut h, device);

    // A real audio file, as one dragged from Finder.
    let file = std::env::temp_dir().join(format!("bl-ui-sampler-{}.wav", std::process::id()));
    let samples: Vec<f32> = (0..480)
        .map(|i| (i as f32 * 440.0 / 48_000.0 * std::f32::consts::TAU).sin() * 0.25)
        .collect();
    let bytes = bl_media::wav_bytes(
        &samples,
        1,
        48_000,
        bl_media::BitDepth::Int16,
        bl_media::Dither::None,
    )
    .unwrap();
    std::fs::write(&file, bytes).unwrap();

    assert_eq!(
        h.shell
            .editor()
            .document()
            .device(device)
            .unwrap()
            .media_at(0),
        None,
        "a new sampler has no sample"
    );
    h.frame(vec![Event::PointerMoved(frame.center())]);
    h.dropped
        .push(std::sync::Arc::new(DroppedFile(file.clone())));
    h.frame(Vec::new());
    assert!(
        h.shell
            .editor()
            .document()
            .device(device)
            .unwrap()
            .media_at(0)
            .is_some(),
        "the drop loaded the sample"
    );
    let _ = std::fs::remove_file(&file);
}

/// The drum sampler's cell for sample slot `slot` (counted from 0), as the
/// panel draws it.
fn slot_cell_rect(h: &mut Harness, device: DeviceId, slot: u32) -> Rect {
    let frame = frame_of(h, device);
    let panel = h
        .shell
        .device_view_state()
        .devices
        .into_iter()
        .find(|d| d.id == device)
        .and_then(|d| d.panel)
        .expect("a panel");
    bl_ui::device_panel::place(
        &panel,
        bl_ui::device_panel::body_rect(frame, TITLE_BAR_HEIGHT),
    )
    .1
    .into_iter()
    .find(|(_, _, s)| *s == Some(slot))
    .expect("a cell for the slot")
    .1
}

fn add_kitbag(h: &mut Harness) -> DeviceId {
    let kit = bl_devices::kitbag_type();
    add(
        h,
        json!({ "kind": kit.kind, "name": kit.name, "role": kit.role }),
    )
}

fn selected_slot(h: &Harness, device: DeviceId) -> u32 {
    h.shell
        .device_view_state()
        .devices
        .into_iter()
        .find(|d| d.id == device)
        .and_then(|d| d.panel)
        .map(|p| p.selected_slot)
        .unwrap()
}

// Spec: eight-slot-drum-sampler R18
#[test]
fn clicking_a_slot_cell_shows_that_slots_controls() {
    let mut h = harness();
    let device = add_kitbag(&mut h);
    // Slot 4 pans left; the panel shows slot 1's Pan until slot 4 is picked.
    let pan = bl_devices::kitbag::param(3, bl_devices::kitbag::slot::PAN);
    h.shell
        .run(
            "device.set_param",
            json!({ "device": device, "param": pan, "value": -20.0 }),
        )
        .unwrap();
    h.settle();
    let nodes = Harness::nodes(&h.settle());
    assert_eq!(Harness::node(&nodes, "Slot 1 Pan").value, "0");
    let cell = Harness::node(&nodes, "Slot 4");
    assert_eq!(cell.role, Role::Button);
    assert!(
        cell.description.contains("plays F3") && cell.description.contains("empty"),
        "{}",
        cell.description
    );
    assert!(
        Harness::node(&nodes, "Slot 1")
            .description
            .contains("selected"),
        "slot 1 is selected to start"
    );

    let at = slot_cell_rect(&mut h, device, 3).center();
    h.click(at);
    h.settle();
    assert_eq!(selected_slot(&h, device), 3);
    let nodes = Harness::nodes(&h.settle());
    assert_eq!(Harness::node(&nodes, "Slot 4 Pan").value, "-20");
    assert!(
        nodes.iter().all(|n| n.label != "Slot 1 Pan"),
        "slot 1's controls are gone"
    );
    assert!(
        Harness::node(&nodes, "Slot 4")
            .description
            .contains("selected")
    );
    assert!(
        !Harness::node(&nodes, "Slot 1")
            .description
            .contains("selected")
    );
    // Choosing a slot is a view change: nothing for undo.
    assert_ne!(last_step(&h), "Select Sample Slot");
}

// Spec: eight-slot-drum-sampler R18
#[test]
fn a_sample_dropped_on_a_slot_cell_loads_into_that_slot_and_delete_clears_it() {
    let mut h = harness();
    let device = add_kitbag(&mut h);
    let file = std::env::temp_dir().join(format!("bl-ui-kitbag-{}.wav", std::process::id()));
    let samples: Vec<f32> = (0..480)
        .map(|i| (i as f32 * 440.0 / 48_000.0 * std::f32::consts::TAU).sin() * 0.25)
        .collect();
    let bytes = bl_media::wav_bytes(
        &samples,
        1,
        48_000,
        bl_media::BitDepth::Int16,
        bl_media::Dither::None,
    )
    .unwrap();
    std::fs::write(&file, bytes).unwrap();

    let media = |h: &Harness| {
        h.shell
            .editor()
            .document()
            .device(device)
            .unwrap()
            .media
            .clone()
    };
    assert!(media(&h).is_empty(), "a new drum sampler has no samples");
    // Dropped on slot 6's cell, the file goes to slot 6 whatever is
    // selected.
    let cell = slot_cell_rect(&mut h, device, 5).center();
    h.frame(vec![Event::PointerMoved(cell)]);
    h.dropped
        .push(std::sync::Arc::new(DroppedFile(file.clone())));
    h.frame(Vec::new());
    let loaded = media(&h);
    assert_eq!(loaded.len(), 6, "{loaded:?}");
    assert!(loaded[..5].iter().all(Option::is_none) && loaded[5].is_some());
    assert_eq!(last_step(&h), "Load Sample");
    let nodes = Harness::nodes(&h.settle());
    assert!(
        Harness::node(&nodes, "Slot 6")
            .description
            .contains("holds a sample")
    );
    assert!(
        Harness::node(&nodes, "Slot 5")
            .description
            .contains("empty")
    );

    // Dropped elsewhere on the device, it goes to the selected slot (1).
    let frame = frame_of(&mut h, device);
    let title = pos2(frame.center().x, frame.top() + 4.0);
    h.frame(vec![Event::PointerMoved(title)]);
    h.dropped
        .push(std::sync::Arc::new(DroppedFile(file.clone())));
    h.frame(Vec::new());
    assert!(media(&h)[0].is_some(), "slot 1, the selected one");

    // Delete on a focused cell clears its slot: one undo step.
    h.click(cell);
    h.settle();
    h.key(Key::Delete, Modifiers::NONE);
    h.settle();
    assert_eq!(media(&h).len(), 1, "slot 6 is clear: {:?}", media(&h));
    assert_eq!(last_step(&h), "Clear Sample");
    let _ = std::fs::remove_file(&file);
}

/// The Edit menu's item of this row, as it reads and works now.
fn edit_item(h: &Harness, row: &str) -> bl_ui::menus::ItemState {
    let bar = menu_bar(MenuPlatform::current());
    let item = bar
        .iter()
        .flat_map(bl_shell::menus::Menu::walk)
        .find(|item| item.row == row)
        .unwrap_or_else(|| panic!("{row} is in the menus"));
    item_state(item, &h.shell, &h.keymap, Platform::current())
}

// Spec: device-compare R5
#[test]
fn the_title_bar_shows_b_while_b_is_active() {
    let mut h = harness();
    let a = add(&mut h, gain());
    let title = |h: &mut Harness| {
        let nodes = Harness::nodes(&h.settle());
        nodes
            .iter()
            .find(|n| n.label.starts_with("Test Gain"))
            .map(|n| n.label.clone())
            .expect("the title bar")
    };
    assert_eq!(title(&mut h), "Test Gain");
    h.shell
        .run("device.compare_switch", json!({ "device": a }))
        .unwrap();
    assert_eq!(title(&mut h), "Test Gain (B)");
    // Folded, the strip reads it too.
    h.shell.run("device.fold", json!({ "device": a })).unwrap();
    assert_eq!(title(&mut h), "Test Gain (B)");
    h.shell
        .run("device.compare_switch", json!({ "device": a }))
        .unwrap();
    assert_eq!(title(&mut h), "Test Gain");
}

// Spec: device-compare R6 (M-EDIT-016, M-EDIT-017, C-DEVICE-010, C-DEVICE-011)
#[test]
fn compare_items_follow_the_state_and_are_disabled_for_racks_and_plugins() {
    let mut h = harness();
    // With no device selected the Edit menu's items are disabled.
    for row in ["M-EDIT-016", "M-EDIT-017"] {
        assert!(!edit_item(&h, row).enabled, "{row} with no device");
    }
    let a = add(&mut h, gain());
    h.shell
        .invoke("device.select", json!({ "device": a }))
        .unwrap();
    // The items work with the Device View focused, as Float's key does.
    h.shell.set_focused_area(Area::Detail);
    h.settle();
    let (switch, copy) = (edit_item(&h, "M-EDIT-016"), edit_item(&h, "M-EDIT-017"));
    assert_eq!(
        (switch.label.as_str(), switch.enabled),
        ("Compare: Switch to B", true)
    );
    assert_eq!(
        (copy.label.as_str(), copy.enabled),
        ("Compare: Copy A to B", true)
    );
    assert_eq!(switch.invocation.unwrap().id, "device.compare_switch");
    assert!(switch.chord.is_some(), "the Edit menu shows P");

    // The items run the commands on the selected device.
    h.shell.invoke("device.compare_switch", json!({})).unwrap();
    assert_eq!(last_step(&h), "Compare: Switch to B");
    h.settle();
    assert_eq!(edit_item(&h, "M-EDIT-016").label, "Compare: Switch to A");
    assert_eq!(edit_item(&h, "M-EDIT-017").label, "Compare: Copy B to A");
    let fa = frame_of(&mut h, a);
    let out = right_click(&mut h, title_rect(fa, false).center());
    let nodes = Harness::nodes(&out);
    let menu = |label: &str| {
        nodes
            .iter()
            .find(|n| n.label.starts_with(label))
            .unwrap_or_else(|| panic!("no menu item {label:?}"))
            .clone()
    };
    assert!(!menu("Compare: Switch to A").disabled);
    assert!(!menu("Compare: Copy B to A").disabled);
    h.frame(vec![Event::Key {
        key: Key::Escape,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: Modifiers::NONE,
    }]);

    // A plug-in device has no states, as in Live: the items are disabled.
    let plugin: DeviceId = h.shell.new_id();
    let track = h.shell.editor().document().device_track(a).unwrap();
    h.shell
        .run(
            "device.insert",
            json!({ "track": track, "index": 1, "device": {
                "id": plugin, "name": "Reverb", "enabled": true, "params": [],
                "kind": { "plugin": { "format": "clap", "plugin_id": "org.example.reverb" } },
            } }),
        )
        .unwrap();
    h.shell
        .invoke("device.select", json!({ "device": plugin }))
        .unwrap();
    h.settle();
    for row in ["M-EDIT-016", "M-EDIT-017"] {
        assert!(!edit_item(&h, row).enabled, "{row} on a plug-in");
    }
    let fp = frame_of(&mut h, plugin);
    let out = right_click(&mut h, title_rect(fp, false).center());
    let nodes = Harness::nodes(&out);
    for label in ["Compare: Switch to B", "Compare: Copy A to B"] {
        let item = nodes
            .iter()
            .find(|n| n.label.starts_with(label))
            .unwrap_or_else(|| panic!("no menu item {label:?}"));
        assert!(item.disabled, "{label} on a plug-in");
    }
}

// Spec: multisampler R29
#[test]
fn the_multisamplers_title_bar_menu_holds_its_options() {
    let mut h = harness();
    let tracks: Vec<TrackId> = h.shell.editor().document().top_level().collect();
    h.shell
        .invoke(
            "arrangement.select_tracks",
            json!({ "tracks": [tracks[0]] }),
        )
        .unwrap();
    h.shell.set_device_types(bl_devices::device_types());
    let t = bl_devices::multisampler_type();
    let device = add(
        &mut h,
        json!({ "kind": t.kind, "name": t.name, "role": t.role }),
    );
    let frame = frame_of(&mut h, device);
    h.click(button_rects(frame, false)[2].center());
    let nodes = Harness::nodes(&h.settle());
    // The device's own items, then the multisampler's.
    assert!(
        nodes
            .iter()
            .any(|n| n.label.starts_with("Save as Default Preset"))
    );
    let fade = nodes
        .iter()
        .find(|n| n.label.starts_with("Use Constant Power Fade for Loops"))
        .expect("the multisampler's option is in the menu");
    assert!(
        fade.selected == Some(true) || fade.toggled == Some(true),
        "on in a new device: {fade:?}"
    );
    assert!(
        nodes
            .iter()
            .any(|n| n.label == "Convert to Simpler" && !n.disabled),
        "the conversion is in the menu (multisampler R62)"
    );
    let b = fade.bounds.unwrap();
    h.click(pos2(
        ((b.x0 + b.x1) / 2.0) as f32,
        ((b.y0 + b.y1) / 2.0) as f32,
    ));
    h.settle();
    let layers = &h.shell.editor().document().device(device).unwrap().layers;
    assert!(layers.linear_loop_fades, "off now: crossfades are linear");
    assert_eq!(last_step(&h), "Change Sample Options");
}

// Spec: view-options R1.
#[test]
fn the_middle_button_pans_the_device_chain() {
    let mut h = harness();
    for _ in 0..12 {
        add(&mut h, gain());
    }
    let view = device_view(&mut h);
    let over = view.center();
    let before = h.main.devices().scroll();
    // A middle-button drag of 100 points right pans the chain with the
    // pointer.
    h.middle_drag(over, over + egui::vec2(100.0, 0.0));
    assert!(
        (before - h.main.devices().scroll() - 100.0).abs() < 0.5,
        "the chain moved 100 points: {}",
        before - h.main.devices().scroll()
    );
}

// Spec: device-chains R25 (the chain's scroll is a command with a readback)
#[test]
fn an_agent_scrolls_the_device_chain_and_reads_where_it_is() {
    let mut h = harness();
    for _ in 0..12 {
        add(&mut h, gain());
    }
    let scroll = |h: &mut Harness| -> f32 {
        let state: bl_shell::device_commands::DeviceViewState =
            serde_json::from_value(h.shell.run("device_view.state", json!({})).unwrap()).unwrap();
        state.scroll
    };
    let held = scroll(&mut h);
    assert_eq!(
        held,
        h.main.devices().scroll(),
        "the view shows what is held"
    );
    h.shell
        .run("device_view.scroll", json!({ "x": -100.0 }))
        .unwrap();
    h.settle();
    assert!((held - scroll(&mut h) - 100.0).abs() < 0.5);
    assert_eq!(scroll(&mut h), h.main.devices().scroll());
    h.shell
        .run("device_view.scroll", json!({ "x": -1.0e6 }))
        .unwrap();
    h.settle();
    assert_eq!(scroll(&mut h), 0.0, "not before the first device");
    assert_eq!(h.main.devices().scroll(), 0.0);
}

// Spec: scales R16, device-chains L-DEV-004. Pitch's Use Current Scale is a
// switch in its title bar, after the activator and before the name: the panel
// no longer draws it, assistive technology reads a switch with its state, and
// pressing it runs `device.set_param` as one undo step.
#[test]
fn the_use_current_scale_switch_is_drawn_in_the_title_bar_and_not_the_panel() {
    use bl_devices::lift::{LIFT, USE_SCALE};

    let mut h = harness();
    let t = bl_devices::lift_type();
    let device = add(
        &mut h,
        json!({ "kind": t.kind, "name": t.name, "role": t.role }),
    );
    let frame = frame_of(&mut h, device);
    let switch = title_switch_rect(frame, false);
    assert!(
        switch.left() > activator_rect(frame, false).right(),
        "after the activator"
    );
    // Drawn once, as a switch, off, where its rect says.
    let nodes = Harness::nodes(&h.settle());
    let found: Vec<&Node> = nodes
        .iter()
        .filter(|n| n.label == "Use Current Scale")
        .collect();
    assert_eq!(found.len(), 1, "the panel draws no control for it");
    assert_eq!(found[0].role, Role::Switch);
    assert_eq!(found[0].toggled, Some(false));
    let b = found[0].bounds.unwrap();
    assert!(
        (b.x0 as f32 - switch.left()).abs() < 3.0,
        "drawn where its rect says"
    );
    let params = |h: &Harness| {
        h.shell
            .editor()
            .document()
            .device(device)
            .unwrap()
            .params
            .get(&USE_SCALE)
            .copied()
            .unwrap_or(0.0)
    };
    assert_eq!(params(&h), 0.0);
    let steps = h.shell.editor().undo_history().len();
    h.click(switch.center());
    assert_eq!(params(&h), 1.0, "the click ran device.set_param");
    assert_eq!(
        h.shell.editor().undo_history().len(),
        steps + 1,
        "one undo step"
    );
    let nodes = Harness::nodes(&h.settle());
    let on = nodes
        .iter()
        .find(|n| n.label == "Use Current Scale")
        .unwrap();
    assert_eq!(on.toggled, Some(true));
    // A second press turns it off; the agent reads the same by id.
    h.click(switch.center());
    assert_eq!(params(&h), 0.0);
    let read = h
        .shell
        .run("device.parameters", json!({ "device": device }))
        .unwrap();
    let state = read["params"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == json!(USE_SCALE.0))
        .unwrap();
    assert_eq!(state["title_switch"], json!(true));
    let _ = LIFT;
}

// Spec: tunings R16. Under a tuning of other than twelve notes a scale-aware
// device's Use Current Scale is drawn off and disabled, and pressing it does
// nothing; a twelve-note tuning changes nothing; it is back with the tuning
// gone, as the device kept it.
#[test]
fn the_use_current_scale_switch_is_disabled_under_a_tuning_of_other_than_twelve_notes() {
    use bl_devices::lift::USE_SCALE;

    let mut h = harness();
    let t = bl_devices::lift_type();
    let device = add(
        &mut h,
        json!({ "kind": t.kind, "name": t.name, "role": t.role }),
    );
    h.shell
        .run(
            "device.set_param",
            json!({ "device": device, "param": USE_SCALE.0, "value": 1.0 }),
        )
        .unwrap();
    let switch = |h: &mut Harness| {
        let nodes = Harness::nodes(&h.settle());
        let node = nodes
            .iter()
            .find(|n| n.label == "Use Current Scale")
            .expect("the switch is in the title bar");
        (node.toggled, node.disabled)
    };
    let load = |h: &mut Harness, file: &str| {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/tunings")
            .join(file);
        h.shell
            .run("tuning.load", json!({ "path": path.to_string_lossy() }))
            .unwrap();
    };
    assert_eq!(switch(&mut h), (Some(true), false));
    load(&mut h, "equal-12.scl");
    assert_eq!(
        switch(&mut h),
        (Some(true), false),
        "twelve notes change nothing"
    );
    load(&mut h, "equal-19.scl");
    assert_eq!(switch(&mut h), (Some(false), true), "off and disabled");
    let frame = frame_of(&mut h, device);
    let steps = h.shell.editor().undo_history().len();
    h.click(title_switch_rect(frame, false).center());
    assert_eq!(
        h.shell.editor().undo_history().len(),
        steps,
        "it does nothing"
    );
    h.shell.run("tuning.remove", json!({})).unwrap();
    assert_eq!(
        switch(&mut h),
        (Some(true), false),
        "back as the device kept it"
    );
}

// Spec: midi-scale-map R12, arpeggiator R16. With Use Current Scale on, Scale's
// Base and Scale Name show the set's current scale, and they and
// Arpeggiator's Root and Scale are drawn disabled.
#[test]
fn the_choosers_a_device_takes_from_the_current_scale_are_disabled() {
    use bl_devices::{arpeggiator, remap};

    let mut h = harness();
    let t = bl_devices::remap_type();
    let scale = add(
        &mut h,
        json!({ "kind": t.kind, "name": t.name, "role": t.role }),
    );
    let t = bl_devices::arpeggiator_type();
    let arp = add(
        &mut h,
        json!({ "kind": t.kind, "name": t.name, "role": t.role }),
    );
    h.shell
        .run(
            "song.set_scale",
            json!({ "enabled": true, "root": 2, "name": "dorian" }),
        )
        .unwrap();
    let panel = |h: &Harness, device: DeviceId| {
        h.shell
            .device_view_state()
            .devices
            .iter()
            .find(|d| d.id == device)
            .and_then(|d| d.panel.clone())
            .unwrap()
    };
    let value = |panel: &bl_shell::device_commands::DevicePanel, id| {
        let at = panel.description.params.iter().position(|p| p.id == id);
        panel.value(at.unwrap())
    };
    assert!(panel(&h, scale).locked.is_empty(), "off: free to choose");
    for (device, switch) in [(scale, remap::USE_SCALE), (arp, arpeggiator::USE_SCALE)] {
        h.shell
            .run(
                "device.set_param",
                json!({ "device": device, "param": switch.0, "value": 1.0 }),
            )
            .unwrap();
    }
    let followed = panel(&h, scale);
    assert_eq!(followed.locked, [remap::BASE, remap::SCALE_NAME]);
    assert_eq!(value(&followed, remap::BASE), 2.0, "D");
    assert_eq!(
        value(&followed, remap::SCALE_NAME),
        3.0,
        "User, Major, Minor, Dorian"
    );
    let arpeggio = panel(&h, arp);
    assert_eq!(arpeggio.locked, [arpeggiator::ROOT, arpeggiator::SCALE]);
}

// Spec: info-view R10, R11 (C-DEVICE-005).
#[test]
fn a_devices_menu_edit_info_text_types_the_text_in_the_info_view() {
    let mut h = harness();
    let device = add(&mut h, gain());
    let frame = frame_of(&mut h, device);
    let out = right_click(&mut h, title_rect(frame, false).center());
    let nodes = Harness::nodes(&out);
    let item = nodes
        .iter()
        .find(|n| n.label == "Edit Info Text")
        .expect("the device's menu lists Edit Info Text");
    h.frame(vec![Event::AccessKitActionRequest(
        egui::accesskit::ActionRequest {
            action: egui::accesskit::Action::Click,
            target_tree: egui::accesskit::TreeId::ROOT,
            target_node: item.id,
            data: None,
        },
    )]);
    h.settle();
    assert!(h.main.is_typing(), "the Info View is a text field");
    h.frame(vec![Event::Text("Gain staging".to_owned())]);
    h.key(Key::Enter, Modifiers::NONE);
    h.settle();
    assert_eq!(
        h.shell
            .editor()
            .document()
            .device(device)
            .unwrap()
            .info_text,
        "Gain staging"
    );
    assert_eq!(last_step(&h), "Change Info Text");
}

// Spec: slice-to-midi R15 (C-DEVICE-015). Simpler's title bar menu offers
// Slice to Drum Rack and Slice to New MIDI Track, enabled only in Slice mode
// with a slice, and Slice to Drum Rack replaces the Simpler.
#[test]
fn simplers_menu_offers_the_slicing_commands_in_slice_mode_only() {
    let mut h = harness();
    h.shell.set_device_types(bl_devices::device_types());
    let simpler = {
        let t = bl_devices::sampler::sampler_type();
        add(
            &mut h,
            json!({ "kind": t.kind, "name": t.name, "role": t.role }),
        )
    };
    let file =
        std::env::temp_dir().join(format!("bl-device-view-slice-{}.wav", std::process::id()));
    let samples: Vec<f32> = (0..48_000).map(|i| i as f32 / 48_000.0 * 0.5).collect();
    std::fs::write(
        &file,
        bl_media::wav_bytes(
            &samples,
            1,
            48_000,
            bl_media::BitDepth::Int16,
            bl_media::Dither::None,
        )
        .unwrap(),
    )
    .unwrap();
    h.shell
        .run(
            "device.load_sample",
            json!({ "device": simpler, "path": file.to_string_lossy() }),
        )
        .unwrap();
    h.settle();
    let items = |h: &mut Harness| {
        let frame = frame_of(h, simpler);
        let out = right_click(h, title_rect(frame, false).center());
        let nodes = Harness::nodes(&out);
        let find = |label: &str| {
            nodes
                .iter()
                .find(|node| node.label == label)
                .unwrap_or_else(|| panic!("no menu item {label:?}"))
                .clone()
        };
        (find("Slice to Drum Rack"), find("Slice to New MIDI Track"))
    };
    let (rack, track) = items(&mut h);
    assert!(
        rack.disabled && track.disabled,
        "Classic mode: both disabled"
    );
    h.key(Key::Escape, Modifiers::NONE);
    for (param, value) in [(0, 2.0), (24, 2.0), (27, 4.0)] {
        h.shell
            .run(
                "device.set_param",
                json!({ "device": simpler, "param": param, "value": value }),
            )
            .unwrap();
    }
    h.settle();
    let (rack, track) = items(&mut h);
    assert!(
        !rack.disabled && !track.disabled,
        "Slice mode: both enabled"
    );
    let bounds = rack.bounds.expect("the item's bounds");
    h.click(pos2(
        ((bounds.x0 + bounds.x1) / 2.0) as f32,
        ((bounds.y0 + bounds.y1) / 2.0) as f32,
    ));
    h.settle();
    assert!(
        h.shell.editor().document().device(simpler).is_none(),
        "the Simpler is replaced"
    );
    assert_eq!(last_step(&h), "Slice to Drum Rack");
}

// Spec: cv-tools R3, R9, R14; L-DEV-026. A CV Instrument's section has a
// chooser per CV output under its name, listing No Output and the interface's
// single channels, shows a channel the interface lacks dimmed, and its
// Calibrate runs against the oscillator on the null device's cable and shows
// the fit; Clear takes it away.
#[test]
fn a_cv_instruments_section_routes_it_and_calibrate_runs() {
    use bl_audio_io::{NullBackend, NullOptions, Pacing};
    use bl_ui::external::cv_row;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    // An oscillator on Pitch (output 3) whose sound comes back on input 4.
    let cable = bl_shell::testing::oscillator(&Arc::default(), &Arc::default(), &Arc::default());
    let (backend, control) = NullBackend::with_devices(
        NullOptions {
            pacing: Pacing::Manual,
            channels: 4,
            ..NullOptions::default()
        },
        &[("null", "Null")],
    );
    let (mut shell, control) = bl_shell::testing::rig_on(backend.with_cable(cable), control);
    shell
        .run("audio.configure", json!({ "input_device": "null:null" }))
        .unwrap();
    let mut h = Harness::on_rig(Window::Main, shell, control.clone());
    h.settle();
    let tracks: Vec<TrackId> = h.shell.editor().document().top_level().collect();
    h.shell
        .invoke(
            "arrangement.select_tracks",
            json!({ "tracks": [tracks[2]] }),
        )
        .unwrap();
    let t = bl_devices::cv_instrument_type();
    let device = add(
        &mut h,
        json!({ "kind": t.kind, "name": t.name, "role": t.role }),
    );
    let frame = frame_of(&mut h, device);

    // A new device: every output at No Output, Audio From at No Input.
    let nodes = Harness::nodes(&h.settle());
    for (name, value) in [
        ("CV Output Pitch", "No Output"),
        ("CV Output Gate", "No Output"),
        ("CV Output Mod 1", "No Output"),
        ("CV Output Mod 2", "No Output"),
        ("External Audio From", "No Input"),
    ] {
        assert_eq!(Harness::node(&nodes, name).value, value, "{name}");
    }

    // Pitch's list: No Output, the four channels, Configure….
    h.click(cv_row(frame, 0).1.center());
    let nodes = Harness::nodes(&h.settle());
    for label in ["No Output", "1", "4", "Configure…"] {
        assert!(
            nodes.iter().any(|n| n.label == label && n.bounds.is_some()),
            "the list holds {label}"
        );
    }
    let entry = nodes
        .iter()
        .find(|n| n.label == "3" && n.bounds.is_some_and(|b| b.y1 - b.y0 > 8.0))
        .expect("the list is open");
    let b = entry.bounds.unwrap();
    h.click(pos2(
        ((b.x0 + b.x1) / 2.0) as f32,
        ((b.y0 + b.y1) / 2.0) as f32,
    ));
    h.settle();
    assert_eq!(last_step(&h), "Change \"Routing\"");
    let nodes = Harness::nodes(&h.settle());
    assert_eq!(Harness::node(&nodes, "CV Output Pitch").value, "3");

    // A channel the interface lacks stays, dimmed: nothing to choose.
    h.shell
        .run(
            "device.set_external",
            json!({ "device": device, "cv_outputs": [{ "name": "Gate", "channel": 9 }],
                    "audio_from": { "type": "mono", "channel": 4 } }),
        )
        .unwrap();
    let nodes = Harness::nodes(&h.settle());
    assert_eq!(Harness::node(&nodes, "CV Output Gate").value, "9");
    assert_eq!(Harness::node(&nodes, "Calibration").value, "Not calibrated");

    // Calibrate runs against the oscillator and shows the fit.
    let stop = Arc::new(AtomicBool::new(false));
    let driver = {
        let (stop, control) = (stop.clone(), control);
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                control.run_blocks(8);
                std::thread::yield_now();
            }
        })
    };
    let (_, row) = cv_row(frame, 6);
    h.click(pos2(row.left() - 20.0, row.center().y));
    h.settle();
    stop.store(true, Ordering::Relaxed);
    driver.join().unwrap();
    let nodes = Harness::nodes(&h.settle());
    let readout = Harness::node(&nodes, "Calibration").value.clone();
    assert!(readout.contains("oct/V"), "{readout}");
    assert_eq!(last_step(&h), "Calibrate CV Instrument");

    // Clear takes it away.
    h.click(pos2(row.right() - 10.0, row.center().y));
    h.settle();
    assert_eq!(last_step(&h), "Clear CV Calibration");
    let nodes = Harness::nodes(&h.settle());
    assert_eq!(Harness::node(&nodes, "Calibration").value, "Not calibrated");
}

// Spec: cv-tools R3, R24, R30 (WP-7.18); L-DEV-026. The CV effects and CV In
// have a section of choosers for their ports and no Audio From or Hardware
// Latency; CV Clock In's shows the tempo it measures and what leads.
#[test]
fn the_cv_effects_sections_list_their_ports_and_clock_in_shows_what_leads() {
    let mut h = harness();
    for (t, port, kind) in [
        (bl_devices::cv_clock_out_type(), "CV Output Clock", "output"),
        (bl_devices::cv_clock_in_type(), "CV Input Clock", "input"),
        (bl_devices::cv_utility_type(), "CV Output CV", "output"),
        (bl_devices::cv_in_type(), "CV Input CV", "input"),
    ] {
        let device = add(
            &mut h,
            json!({ "kind": t.kind, "name": t.name, "role": t.role }),
        );
        let nodes = Harness::nodes(&h.settle());
        let chooser = Harness::node(&nodes, port);
        assert_eq!(
            chooser.value,
            if kind == "output" {
                "No Output"
            } else {
                "No Input"
            },
            "{}",
            t.name
        );
        assert!(
            !nodes.iter().any(|n| n.label == "External Audio From"),
            "{} brings no sound back",
            t.name
        );
        if t.name == "CV Clock In" {
            let readout = Harness::node(&nodes, "Clock");
            assert!(
                readout.value.contains("No tempo") && readout.value.contains("Leader: Nothing"),
                "{}",
                readout.value
            );
        }
        h.shell
            .run("device.remove", json!({ "device": device }))
            .unwrap();
        h.settle();
    }
}

// Spec: cv-tools R9, R32 (WP-7.18b). CV Envelope Follower, CV LFO and CV
// Shaper have a section with their CV output's chooser, controls for Depth,
// Min and Max, and no Map button or Mod switch.
#[test]
fn the_cv_modulators_panels_have_a_cv_output_and_min_max_depth_and_no_mapping() {
    let mut h = harness();
    for t in [
        bl_devices::cv_level_type(),
        bl_devices::cv_wave_type(),
        bl_devices::cv_curve_type(),
    ] {
        let device = add(
            &mut h,
            json!({ "kind": t.kind, "name": t.name, "role": t.role }),
        );
        let nodes = Harness::nodes(&h.settle());
        assert_eq!(
            Harness::node(&nodes, "CV Output CV").value,
            "No Output",
            "{}",
            t.name
        );
        for control in ["Depth", "Min", "Max"] {
            assert!(
                nodes.iter().any(|n| n.label == control),
                "{} has {control}",
                t.name
            );
        }
        for absent in ["Map 1", "Mod", "Polarity"] {
            assert!(
                !nodes.iter().any(|n| n.label == absent),
                "{} has no {absent}",
                t.name
            );
        }
        h.shell
            .run("device.remove", json!({ "device": device }))
            .unwrap();
        h.settle();
    }
}

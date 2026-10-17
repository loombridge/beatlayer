//! The Device View (WP-2.4), headless: the selected track's chain in
//! frames with Live's title bar (L-DEV-001, L-DEV-003, L-DEV-004, L-DEV-006,
//! L-DEV-009), selecting, folding, switching off, renaming, moving, the
//! context menu, and hot-swapping's bar, each running the registered
//! command an agent runs. Devices are the engine's test devices.
//!
//! Spec: device-chains R6, R10, R11, R19, R21 to R25.

mod common;

use bl_engine::testing;
use bl_model::{DeviceId, TrackId};
use bl_shell::Area;
use bl_shell::device_commands::{DEVICE_GAP, DeviceAdded, TITLE_BAR_HEIGHT};
use bl_ui::device_view::{
    EMPTY_TEXT, activator_rect, button_rects, control_id, layout, title_rect,
};
use bl_ui::main_window::UiEvent;
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
        "Save as Default Preset",
    ] {
        assert!(!item(label).disabled, "{label} works");
    }
    // Show Preset Name is ticked on a new device (observed).
    assert!(item("Show Preset Name").label.starts_with('✔'));
    for (label, package) in [
        ("Edit Info Text", "WP-2.24"),
        ("Compare: Switch to B", "M5"),
        ("Compare: Copy A to B", "M5"),
    ] {
        let node: Node = item(label);
        assert!(node.disabled, "{label} waits");
        assert!(
            node.description.contains(package),
            "{label}: {}",
            node.description
        );
    }
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
        h.shell.editor().document().device(device).unwrap().media,
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
            .media
            .is_some(),
        "the drop loaded the sample"
    );
    let _ = std::fs::remove_file(&file);
}

//! Floating device windows, headless (WP-2.5b, IMP-14; the device chains
//! spec, R26 to R28 and R31): Float in the device's menu floats it and
//! leaves a strip in the chain; its window draws the device with a Dock
//! button that docks it; a larger window grows only the display cells, the
//! controls keeping their size and place; and the Plug-Ins page's
//! Auto-Hide Plug-In Windows switch. Devices are the framework's test
//! devices.

mod common;

use bl_devices::testing::{self, SCOPE_TIME};
use bl_model::{DeviceId, ParamId, TrackId};
use bl_shell::device_commands::{DeviceAdded, TITLE_BAR_HEIGHT, frame_width};
use bl_shell::settings::SettingsPage;
use bl_ui::device_panel::{body_rect, place};
use bl_ui::device_view::{dock_rect, layout, title_rect};
use common::{Harness, Node, Window};
use egui::accesskit::Role;
use egui::{Event, Modifiers, PointerButton, Pos2, Rect, pos2, vec2};
use serde_json::json;

fn harness() -> Harness {
    let mut h = Harness::with_title_bar(Window::Main);
    h.shell.set_device_types(testing::device_types());
    h.settle();
    let tracks: Vec<TrackId> = h.shell.editor().document().top_level().collect();
    h.shell
        .invoke(
            "arrangement.select_tracks",
            json!({ "tracks": [tracks[2]] }),
        )
        .unwrap();
    h.settle();
    h
}

fn add(h: &mut Harness, t: &bl_engine::DeviceType) -> DeviceId {
    let device = json!({ "kind": t.kind, "name": t.name, "role": t.role });
    let added: DeviceAdded = serde_json::from_value(
        h.shell
            .run("device.add", json!({ "device": device }))
            .unwrap(),
    )
    .unwrap();
    h.settle();
    added.device
}

fn frame_of(h: &mut Harness, device: DeviceId) -> Rect {
    let nodes = Harness::nodes(&h.settle());
    let b = Harness::node(&nodes, "Device View").bounds.unwrap();
    let view = Rect::from_min_max(
        pos2(b.x0 as f32, b.y0 as f32),
        pos2(b.x1 as f32, b.y1 as f32),
    );
    layout(&h.shell, view)
        .into_iter()
        .find(|(id, _)| *id == device)
        .unwrap()
        .1
}

fn bounds(node: &Node) -> Rect {
    let b = node.bounds.unwrap();
    Rect::from_min_max(
        pos2(b.x0 as f32, b.y0 as f32),
        pos2(b.x1 as f32, b.y1 as f32),
    )
}

fn press(at: Pos2, pressed: bool, button: PointerButton) -> Event {
    Event::PointerButton {
        pos: at,
        button,
        pressed,
        modifiers: Modifiers::NONE,
    }
}

fn last_step(h: &Harness) -> String {
    h.shell
        .editor()
        .undo_history()
        .next()
        .unwrap_or_default()
        .to_owned()
}

fn floating(h: &Harness, device: DeviceId) -> bool {
    h.shell.editor().document().device(device).unwrap().floating
}

/// The window's contents at `size` points: a frame drawn at that size.
fn window_at(h: &mut Harness, device: DeviceId, size: egui::Vec2) -> Vec<Node> {
    h.window = Window::Device(device);
    h.size = size;
    Harness::nodes(&h.settle())
}

// Spec: device-chains R26, R27
#[test]
fn float_in_the_menu_leaves_a_strip_and_the_dock_button_docks() {
    let mut h = harness();
    let utility = add(&mut h, &testing::utility_type());
    let docked = frame_of(&mut h, utility);
    // Right-click the title bar: Float follows Fold.
    let at = title_rect(docked, false).center();
    h.frame(vec![Event::PointerMoved(at)]);
    h.frame(vec![press(at, true, PointerButton::Secondary)]);
    h.frame(vec![press(at, false, PointerButton::Secondary)]);
    let nodes = Harness::nodes(&h.settle());
    let labels: Vec<&str> = nodes.iter().map(|n| n.label.as_str()).collect();
    let fold = labels.iter().position(|l| l.starts_with("Fold")).unwrap();
    let float = labels
        .iter()
        .position(|l| l.trim_start_matches("✔ ").starts_with("Float"))
        .unwrap();
    assert!(fold < float, "Float comes after Fold");
    let item = nodes[float].clone();
    assert!(!item.disabled);
    assert!(!item.label.starts_with('✔'), "not ticked while docked");
    // As a screen reader or the keyboard chooses it.
    h.frame(vec![Event::AccessKitActionRequest(
        egui::accesskit::ActionRequest {
            action: egui::accesskit::Action::Click,
            target_tree: egui::accesskit::TreeId::ROOT,
            target_node: item.id,
            data: None,
        },
    )]);
    h.settle();
    assert!(floating(&h, utility));
    assert_eq!(last_step(&h), "Float Device");

    // Its place in the chain is a strip, described as floating.
    let strip = frame_of(&mut h, utility);
    assert_eq!(strip.width(), TITLE_BAR_HEIGHT);
    let nodes = Harness::nodes(&h.settle());
    assert!(
        nodes.iter().any(|n| n.description.contains("floating")),
        "the strip says the device floats"
    );
    // A double-click on the strip doesn't fold the device.
    let title = title_rect(strip, true).center();
    h.frame(vec![Event::PointerMoved(title)]);
    for _ in 0..2 {
        h.frame(vec![press(title, true, PointerButton::Primary)]);
        h.frame(vec![press(title, false, PointerButton::Primary)]);
    }
    h.settle();
    assert!(!h.shell.devices().is_folded(utility));

    // The window: the device whole, with a Dock button that docks it.
    let windows = h.shell.floating_windows().windows;
    let size = vec2(windows[0].min_width, windows[0].min_height);
    let nodes = window_at(&mut h, utility, size);
    assert_eq!(Harness::node(&nodes, "Dock").role, Role::Button);
    let window_rect = Rect::from_min_size(Pos2::ZERO, size);
    h.click(dock_rect(window_rect).center());
    h.settle();
    assert!(!floating(&h, utility));
    assert_eq!(last_step(&h), "Dock Device");
    h.window = Window::Main;
    h.size = bl_ui::layout::REFERENCE + vec2(0.0, bl_ui::layout::TITLE_BAR);
    assert_eq!(frame_of(&mut h, utility).width(), docked.width());
    let frames = h.shell.device_view_state().devices;
    assert_eq!(frame_width(&frames[0]), docked.width());
}

// Spec: device-chains R28 (device-framework R11, R15)
#[test]
fn a_larger_window_grows_only_the_display_cells() {
    let mut h = harness();
    let scope = add(&mut h, &testing::scope_type());
    h.shell
        .run(
            "device.set_floating",
            json!({ "device": scope, "floating": true }),
        )
        .unwrap();
    let window = h.shell.floating_windows().windows[0].clone();
    assert!(window.grows_across && window.grows_down);
    let panel = window.view.panel.clone().unwrap();
    let docked = vec2(window.min_width, window.min_height);
    let grown = docked + vec2(300.0, 200.0);
    let placed = |size: egui::Vec2| {
        place(
            &panel,
            body_rect(Rect::from_min_size(Pos2::ZERO, size), TITLE_BAR_HEIGHT),
        )
    };
    let (small, small_cells) = placed(docked);
    let (big, big_cells) = placed(grown);
    // The smallest window holds the body at its docked size.
    let size = panel.description.layout.docked_size();
    assert_eq!(
        docked - vec2(0.0, TITLE_BAR_HEIGHT),
        vec2(size.width, size.height)
    );
    for ((p, _, a), (q, _, b)) in small.iter().zip(&big) {
        assert_eq!(p, q);
        assert_eq!(a.size(), b.size(), "a control keeps its size");
    }
    let rect = |c: &[(ParamId, bl_model::device_layout::Widget, Rect)], param| {
        c.iter().find(|(p, _, _)| *p == param).unwrap().2
    };
    assert_eq!(rect(&small, SCOPE_TIME), rect(&big, SCOPE_TIME));
    assert_eq!(big_cells[0].1.width(), small_cells[0].1.width() + 300.0);
    assert_eq!(big_cells[0].1.height(), small_cells[0].1.height() + 200.0);

    // The grown window draws its controls where that places them: a drag
    // on the time knob there changes the time.
    let nodes = window_at(&mut h, scope, grown);
    assert_eq!(Harness::node(&nodes, "Scope").role, Role::Image);
    let before = value(&h, scope, SCOPE_TIME);
    let at = rect(&big, SCOPE_TIME).center();
    h.frame(vec![Event::PointerMoved(at)]);
    h.frame(vec![press(at, true, PointerButton::Primary)]);
    for step in 1..=5 {
        h.frame(vec![Event::PointerMoved(at - vec2(0.0, 4.0 * step as f32))]);
    }
    h.frame(vec![press(
        at - vec2(0.0, 20.0),
        false,
        PointerButton::Primary,
    )]);
    h.settle();
    assert!(value(&h, scope, SCOPE_TIME) > before, "the knob is there");
}

/// A device parameter's value now.
fn value(h: &Harness, device: DeviceId, param: ParamId) -> f64 {
    let windows = h.shell.floating_windows().windows;
    let window = windows.iter().find(|w| w.device == device).unwrap();
    let panel = window.view.panel.as_ref().unwrap();
    let index = panel
        .description
        .params
        .iter()
        .position(|p| p.id == param)
        .unwrap();
    panel.value(index)
}

// Spec: device-chains R31
#[test]
fn the_plug_ins_page_switches_auto_hide_plug_in_windows() {
    let mut h = Harness::new(Window::Settings);
    h.shell
        .run("app.settings", json!({ "page": SettingsPage::PlugIns }))
        .unwrap();
    let nodes = Harness::nodes(&h.settle());
    let switch = Harness::node(&nodes, "Auto-Hide Plug-In Windows");
    assert!(!switch.disabled);
    for pending in ["Multiple Plug-In Windows", "Auto-Open Plug-In Windows"] {
        let node = Harness::node(&nodes, pending);
        assert!(node.description.contains("WP-2.6"), "{pending}");
    }
    assert!(h.shell.auto_hide_plug_in_windows());
    h.click(bounds(switch).center());
    h.settle();
    assert!(!h.shell.auto_hide_plug_in_windows());
}

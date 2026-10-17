//! The multisampler's zone editor (WP-5.13; the multisampler spec, R1, R3,
//! R7, R10, R11), headless: the Zone tab opens the editor above the Device
//! View, a drag on an edge, the body or a fade is one undo step acting on
//! every selected layer, the list's menu runs its commands, and hovering a
//! zone shows it in the Status Bar.

mod common;

use bl_model::{DeviceId, Layer, LayerId, ZoneKind};
use bl_ui::multisampler::{areas, bar_parts, row_height};
use common::{Harness, Window};
use egui::{Event, Modifiers, PointerButton, Pos2, Rect, pos2, vec2};
use serde_json::json;

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

/// A window with a multisampler on a MIDI track, `count` layers in it and
/// its zone editor open.
fn editor(count: usize) -> (Harness, DeviceId, Vec<LayerId>) {
    let mut h = Harness::with_title_bar(Window::Main);
    h.shell.set_device_types(bl_devices::device_types());
    h.settle();
    let midi = h
        .shell
        .editor()
        .document()
        .tracks()
        .find(|t| t.kind() == bl_model::TrackKind::Midi)
        .map(bl_model::Track::id)
        .unwrap();
    h.shell
        .run("arrangement.select_tracks", json!({ "tracks": [midi] }))
        .unwrap();
    let t = bl_devices::multisampler_type();
    let added = h
        .shell
        .run(
            "device.add",
            json!({ "device": { "kind": t.kind, "name": t.name, "role": t.role } }),
        )
        .unwrap();
    let device: DeviceId = serde_json::from_value(added["device"].clone()).unwrap();
    let layers: Vec<Layer> = (0..count)
        .map(|i| {
            Layer::new(
                LayerId(0),
                format!("Layer {}", i + 1),
                bl_model::ContentHash([i as u8 + 1; 32]),
                48_000,
                48_000,
                1,
            )
        })
        .collect();
    h.shell
        .run(
            "device.insert_layers",
            json!({ "device": device, "index": null, "layers": layers }),
        )
        .unwrap();
    h.shell
        .run("device.select", json!({ "device": device }))
        .unwrap();
    h.shell
        .run(
            "device.set_multisampler_view",
            json!({ "device": device, "zone_editor": true }),
        )
        .unwrap();
    h.settle();
    let ids = h
        .shell
        .editor()
        .document()
        .device(device)
        .unwrap()
        .layers
        .list
        .iter()
        .map(|l| l.id)
        .collect();
    (h, device, ids)
}

/// The zone editor's rectangle, from its accessibility node.
fn editor_rect(h: &mut Harness) -> Rect {
    let nodes = Harness::nodes(&h.settle());
    let b = Harness::node(&nodes, "Zone Editor").bounds.unwrap();
    Rect::from_min_max(
        pos2(b.x0 as f32, b.y0 as f32),
        pos2(b.x1 as f32, b.y1 as f32),
    )
}

fn zone(h: &Harness, device: DeviceId, layer: LayerId) -> bl_model::Zone {
    h.shell
        .editor()
        .document()
        .device(device)
        .unwrap()
        .layers
        .get(layer)
        .unwrap()
        .key
}

fn steps(h: &Harness) -> usize {
    h.shell.editor().undo_history().len()
}

fn press(at: Pos2, pressed: bool, modifiers: Modifiers) -> Event {
    Event::PointerButton {
        pos: at,
        button: PointerButton::Primary,
        pressed,
        modifiers,
    }
}

/// Drags from `from` by `dx` points, in ten moves.
fn drag(h: &mut Harness, from: Pos2, dx: f32) {
    h.frame(vec![Event::PointerMoved(from)]);
    h.frame(vec![press(from, true, Modifiers::NONE)]);
    for i in 1..=10 {
        h.frame(vec![Event::PointerMoved(
            from + vec2(dx * i as f32 / 10.0, 0.0),
        )]);
    }
    h.frame(vec![press(from + vec2(dx, 0.0), false, Modifiers::NONE)]);
    h.settle();
}

/// Where a layer's bar parts are, in row `index` of the key editor.
fn parts(
    h: &mut Harness,
    device: DeviceId,
    index: usize,
    layer: LayerId,
) -> bl_ui::multisampler::BarParts {
    let rect = editor_rect(h);
    let areas = areas(rect);
    let row_h = row_height(bl_model::EditorSize::Medium);
    let row = Rect::from_min_size(
        pos2(areas.bars.left(), areas.bars.top() + index as f32 * row_h),
        vec2(areas.bars.width(), row_h),
    );
    bar_parts(areas.bars, row, ZoneKind::Key, zone(h, device, layer))
}

// Spec: multisampler R1
#[test]
fn the_zone_tab_opens_the_editor_above_the_device_view() {
    let (mut h, device, _) = editor(2);
    // The six tabs after the name, in order.
    let nodes = Harness::nodes(&h.settle());
    let tab = |nodes: &[common::Node], name: &str| -> egui::accesskit::Rect {
        nodes
            .iter()
            .find(|n| n.label == name && n.role == egui::accesskit::Role::Button)
            .unwrap_or_else(|| panic!("no {name} tab"))
            .bounds
            .unwrap()
    };
    let xs: Vec<f64> = [
        "Zone",
        "Sample",
        "Pitch/Osc",
        "Filter/Global",
        "Modulation",
        "MIDI",
    ]
    .iter()
    .map(|name| tab(&nodes, name).x0)
    .collect();
    assert!(xs.windows(2).all(|w| w[0] < w[1]), "in order: {xs:?}");
    let view = Harness::node(&nodes, "Device View").bounds.unwrap();
    let editor = Harness::node(&nodes, "Zone Editor").bounds.unwrap();
    assert!(editor.y1 <= view.y0, "above the Device View");
    // Zone toggles it: a click on the tab closes it, and again opens it.
    let zone = tab(&nodes, "Zone");
    let centre = pos2(
        ((zone.x0 + zone.x1) / 2.0) as f32,
        ((zone.y0 + zone.y1) / 2.0) as f32,
    );
    h.click(centre);
    h.settle();
    let nodes = Harness::nodes(&h.settle());
    assert!(!nodes.iter().any(|n| n.label == "Zone Editor"), "closed");
    assert!(
        !h.shell
            .editor()
            .undo_history()
            .next()
            .unwrap_or("")
            .is_empty()
    );
    h.click(centre);
    h.settle();
    assert!(
        Harness::nodes(&h.settle())
            .iter()
            .any(|n| n.label == "Zone Editor")
    );
    let _ = device;
}

// Spec: multisampler R10
#[test]
fn dragging_an_edge_the_body_or_a_corner_is_one_undo_step() {
    let (mut h, device, ids) = editor(3);
    let before = steps(&h);
    let first = ids[0];
    // Give the layer a narrow zone to work on, as the editor's own commands
    // do: keys 40 to 60.
    h.shell
        .run(
            "device.set_layer_zone",
            json!({ "device": device, "kind": "key",
                    "zones": [{ "layer": first, "zone": { "low": 40, "high": 60 } }] }),
        )
        .unwrap();
    h.settle();
    let after_setup = steps(&h);
    assert_eq!(after_setup, before + 1);

    // The low edge, five keys up: one undo step.
    let per_key = areas(editor_rect(&mut h)).bars.width() / 128.0;
    let low = parts(&mut h, device, 0, first).low.center();
    drag(&mut h, low, 5.0 * per_key + 1.0);
    let moved = zone(&h, device, first);
    assert_eq!((moved.low, moved.high), (45, 60));
    assert_eq!(steps(&h), after_setup + 1, "one step however far it went");

    // The body: ten keys down.
    let body = parts(&mut h, device, 0, first).body.center();
    drag(&mut h, body, -10.0 * per_key - 1.0);
    let moved = zone(&h, device, first);
    assert_eq!((moved.low, moved.high), (35, 50));
    assert_eq!(steps(&h), after_setup + 2);

    // A top corner sets the low end's fade.
    let corner = parts(&mut h, device, 0, first).fade_low.center();
    drag(&mut h, corner, 4.0 * per_key + 1.0);
    let moved = zone(&h, device, first);
    assert_eq!((moved.low, moved.high, moved.fade_low), (35, 50, 4));
    assert_eq!(steps(&h), after_setup + 3);
    h.shell.run("edit.undo", json!({})).unwrap();
    assert_eq!(zone(&h, device, first).fade_low, 0, "undone as one");
}

// Spec: multisampler R10
#[test]
fn a_drag_acts_on_every_selected_layer_alike() {
    let (mut h, device, ids) = editor(3);
    for (layer, low) in [(ids[0], 10), (ids[1], 30), (ids[2], 50)] {
        h.shell
            .run(
                "device.set_layer_zone",
                json!({ "device": device, "kind": "key",
                        "zones": [{ "layer": layer, "zone": { "low": low, "high": low + 9 } }] }),
            )
            .unwrap();
    }
    h.shell
        .run(
            "device.select_layers",
            json!({ "device": device, "layers": [ids[0], ids[1]] }),
        )
        .unwrap();
    h.settle();
    let per_key = areas(editor_rect(&mut h)).bars.width() / 128.0;
    let before = steps(&h);
    let body = parts(&mut h, device, 0, ids[0]).body.center();
    drag(&mut h, body, 5.0 * per_key + 1.0);
    assert_eq!(zone(&h, device, ids[0]).low, 15);
    assert_eq!(
        zone(&h, device, ids[1]).low,
        35,
        "the selected neighbour moved too"
    );
    assert_eq!(
        zone(&h, device, ids[2]).low,
        50,
        "an unselected layer stayed"
    );
    assert_eq!(steps(&h), before + 1);
}

// Spec: multisampler R10
#[test]
fn the_editor_zooms_and_hovering_shows_the_zone() {
    let (mut h, device, ids) = editor(2);
    let medium = editor_rect(&mut h);
    h.shell
        .run(
            "device.set_multisampler_view",
            json!({ "device": device, "size": "large" }),
        )
        .unwrap();
    let large = editor_rect(&mut h);
    assert!(
        large.height() > medium.height(),
        "Large is taller than Medium"
    );
    // Hovering a layer's bar shows its name and range in the Status Bar.
    h.shell
        .run(
            "device.set_layer_zone",
            json!({ "device": device, "kind": "key",
                    "zones": [{ "layer": ids[0], "zone": { "low": 36, "high": 48 } }] }),
        )
        .unwrap();
    h.settle();
    let areas = areas(editor_rect(&mut h));
    let row_h = row_height(bl_model::EditorSize::Large);
    let at = pos2(areas.list.left() + 20.0, areas.bars.top() + row_h / 2.0);
    h.frame(vec![Event::PointerMoved(at)]);
    let nodes = Harness::nodes(&h.frame(vec![Event::PointerMoved(at + vec2(1.0, 0.0))]));
    assert!(
        nodes
            .iter()
            .any(|n| n.value.contains("Layer 1") && n.value.contains("C1 to C2")),
        "the readout names the layer and its keys"
    );
}

// Spec: multisampler R11
#[test]
fn the_layer_lists_menu_holds_lives_items_in_order() {
    let (mut h, device, ids) = editor(2);
    let areas = areas(editor_rect(&mut h));
    let row_h = row_height(bl_model::EditorSize::Medium);
    let at = pos2(areas.list.left() + 20.0, areas.bars.top() + row_h * 1.5);
    h.frame(vec![Event::PointerMoved(at)]);
    h.frame(vec![Event::PointerButton {
        pos: at,
        button: PointerButton::Secondary,
        pressed: true,
        modifiers: Modifiers::NONE,
    }]);
    h.frame(vec![Event::PointerButton {
        pos: at,
        button: PointerButton::Secondary,
        pressed: false,
        modifiers: Modifiers::NONE,
    }]);
    let nodes = Harness::nodes(&h.settle());
    let wanted = [
        "Delete",
        "Duplicate",
        "Rename",
        "Distribute Ranges Equally",
        "Distribute Ranges Around Root Key",
        "Small",
        "Medium",
        "Large",
        "Normalize Volume",
        "Normalize Pan",
        "Select All With Same Range",
        "Sort Alphabetically, Ascending",
    ];
    let tops: Vec<f64> = wanted
        .iter()
        .map(|item| {
            nodes
                .iter()
                .find(|n| n.label == *item && n.bounds.is_some())
                .unwrap_or_else(|| panic!("the menu has {item}"))
                .bounds
                .unwrap()
                .y0
        })
        .collect();
    assert!(
        tops.windows(2).all(|w| w[0] < w[1]),
        "in Live's order: {tops:?}"
    );
    // Delete runs the command on the layer the menu opened on.
    let delete = Harness::node(&nodes, "Delete").bounds.unwrap();
    h.click(pos2(
        ((delete.x0 + delete.x1) / 2.0) as f32,
        ((delete.y0 + delete.y1) / 2.0) as f32,
    ));
    h.settle();
    let left = h
        .shell
        .editor()
        .document()
        .device(device)
        .unwrap()
        .layers
        .list
        .len();
    assert_eq!(left, 1);
    assert!(
        h.shell
            .editor()
            .document()
            .device(device)
            .unwrap()
            .layers
            .get(ids[0])
            .is_some()
    );
}

// Spec: multisampler R3
#[test]
fn dropping_samples_on_the_layer_list_adds_them_at_the_drop_point() {
    let (mut h, device, ids) = editor(2);
    let dir = std::env::temp_dir().join(format!("bl-zone-drop-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("Dropped.wav");
    std::fs::write(
        &file,
        bl_media::wav_bytes(
            &[0.5; 480],
            1,
            48_000,
            bl_media::BitDepth::Int16,
            bl_media::Dither::None,
        )
        .unwrap(),
    )
    .unwrap();
    let areas = areas(editor_rect(&mut h));
    let row_h = row_height(bl_model::EditorSize::Medium);
    // Dropped between the two layers: the new one lands second.
    let at = pos2(areas.list.left() + 20.0, areas.bars.top() + row_h + 1.0);
    h.frame(vec![Event::PointerMoved(at)]);
    h.dropped
        .push(std::sync::Arc::new(DroppedFile(file.clone())));
    h.settle();
    let names: Vec<String> = h
        .shell
        .editor()
        .document()
        .device(device)
        .unwrap()
        .layers
        .list
        .iter()
        .map(|l| l.name.clone())
        .collect();
    let _ = ids;
    std::fs::remove_dir_all(&dir).ok();
    assert_eq!(names, ["Layer 1", "Dropped", "Layer 2"]);
}

fn button_centre(h: &mut Harness, label: &str) -> Pos2 {
    let nodes = Harness::nodes(&h.settle());
    let b = nodes
        .iter()
        .find(|n| n.label == label && n.role == egui::accesskit::Role::Button)
        .unwrap_or_else(|| panic!("no {label} button"))
        .bounds
        .unwrap();
    pos2(((b.x0 + b.x1) / 2.0) as f32, ((b.y0 + b.y1) / 2.0) as f32)
}

// Spec: multisampler R6
#[test]
fn the_selector_indicator_drags_the_sample_selector() {
    let (mut h, device, _) = editor(1);
    let sel = button_centre(&mut h, "Sel");
    h.click(sel);
    h.settle();
    let areas = areas(editor_rect(&mut h));
    let per_value = areas.bars.width() / 128.0;
    let nodes = Harness::nodes(&h.settle());
    let indicator = nodes
        .iter()
        .find(|n| n.label == "Sample Selector")
        .expect("the select editor shows the selector")
        .bounds
        .unwrap();
    let from = pos2(((indicator.x0 + indicator.x1) / 2.0) as f32, ((indicator.y0 + indicator.y1) / 2.0) as f32);
    let before = steps(&h);
    drag(&mut h, from, 20.0 * per_value);
    let value = h
        .shell
        .editor()
        .document()
        .device(device)
        .unwrap()
        .params
        .get(&bl_devices::multisampler::SELECTOR)
        .copied();
    assert!(value.is_some_and(|v| (19.0..=21.0).contains(&v)), "{value:?}");
    assert_eq!(steps(&h), before + 1, "one undo step");
}

// Spec: multisampler R10
#[test]
fn the_hot_swap_button_replaces_the_selected_sample() {
    let (mut h, device, ids) = editor(0);
    let dir = std::env::temp_dir().join(format!("bl-zone-hot-swap-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let write = |name: &str, frames: usize| {
        let path = dir.join(name);
        std::fs::write(
            &path,
            bl_media::wav_bytes(&vec![0.5; frames], 1, 48_000, bl_media::BitDepth::Int16, bl_media::Dither::None)
                .unwrap(),
        )
        .unwrap();
        path
    };
    let (old, new) = (write("Old.wav", 4800), write("New.wav", 9600));
    let _ = ids;
    h.shell
        .run("device.add_layers", json!({ "device": device, "paths": [old.to_string_lossy()] }))
        .unwrap();
    h.settle();
    let button = button_centre(&mut h, "Hot-Swap Sample");
    h.click(button);
    h.settle();
    let state = h.shell.run("device_view.state", json!({})).unwrap();
    assert!(state["devices"][0]["multisampler"]["hot_swap"].is_number(), "hot-swapping");
    assert!(h.shell.browser_visible(), "the browser shows");
    h.shell
        .run("browser.select", json!({ "paths": [new.to_string_lossy()] }))
        .unwrap();
    h.shell.run("browser.load_selected", json!({})).unwrap();
    h.settle();
    let layer = h.shell.editor().document().device(device).unwrap().layers.list[0].clone();
    std::fs::remove_dir_all(&dir).ok();
    assert_eq!(layer.length, 9600, "the chosen sample replaced the layer's");
}

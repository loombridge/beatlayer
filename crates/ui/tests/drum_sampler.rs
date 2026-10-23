//! Drum Sampler in the Device View (WP-5.18), headless: a sample dropped on it
//! loads, and its context menu ends with its own items, which run the
//! commands an agent runs.
//!
//! Spec: one-shot-drum-sampler R22 and R23.

use crate::common;
use bl_devices::drum_sampler::{ENV_FOLLOWS_PITCH, PER_NOTE_BEND, drum_sampler_type};
use bl_model::{DeviceId, DeviceKind, TrackId};
use bl_shell::device_commands::DeviceAdded;
use bl_ui::device_view::{layout, title_rect};
use common::{Harness, Node, Window};
use egui::{Event, PointerButton, Rect, pos2};
use serde_json::json;

fn harness() -> Harness {
    let mut h = Harness::with_title_bar(Window::Main);
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

fn add_strike(h: &mut Harness) -> DeviceId {
    let strike = drum_sampler_type();
    let device = json!({ "kind": strike.kind, "name": strike.name, "role": strike.role });
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

/// Opens the device's context menu and returns the nodes with it open.
fn open_menu(h: &mut Harness, device: DeviceId) -> Vec<Node> {
    let frame = frame_of(h, device);
    let at = title_rect(frame, false).center();
    h.frame(vec![Event::PointerMoved(at)]);
    let press = |pressed| Event::PointerButton {
        pos: at,
        button: PointerButton::Secondary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    h.frame(vec![press(true)]);
    h.frame(vec![press(false)]);
    Harness::nodes(&h.settle())
}

fn place(nodes: &[Node], label: &str) -> usize {
    nodes
        .iter()
        .position(|n| n.label.trim_start_matches("✔ ") == label)
        .unwrap_or_else(|| panic!("no menu item {label:?}"))
}

fn click(h: &mut Harness, node: &Node) {
    let spot = node.bounds.unwrap();
    h.click(pos2(
        ((spot.x0 + spot.x1) / 2.0) as f32,
        ((spot.y0 + spot.y1) / 2.0) as f32,
    ));
    h.settle();
}

fn param(h: &Harness, device: DeviceId, id: bl_model::ParamId) -> Option<f64> {
    h.shell
        .editor()
        .document()
        .device(device)
        .unwrap()
        .params
        .get(&id)
        .copied()
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

// Spec: one-shot-drum-sampler R22
#[test]
fn a_dropped_sample_loads_into_strike_under_the_pointer() {
    let mut h = harness();
    let device = add_strike(&mut h);
    let frame = frame_of(&mut h, device);
    let file = std::env::temp_dir().join(format!("bl-ui-strike-{}.wav", std::process::id()));
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

// Spec: one-shot-drum-sampler R23
#[test]
fn the_menu_ends_with_strikes_own_items_and_they_run_their_commands() {
    let mut h = harness();
    let device = add_strike(&mut h);
    let nodes = open_menu(&mut h, device);
    let after = place(&nodes, "Save as Default Preset");

    // The two switches, in the manual's order: bend on in a new device.
    let bend = place(&nodes, "Enable Per-Note Pitch Bend");
    let follows = place(&nodes, "Envelope Follows Pitch");
    let swap = place(&nodes, "Swap to Simpler");
    let pad = place(&nodes, "Save as Default Pad");
    // Listed top to bottom in that order, all after the general items.
    let top = |at: usize| nodes[at].bounds.unwrap().y0;
    assert!(top(after) < top(bend) && top(bend) < top(follows));
    assert!(top(follows) < top(swap) && top(swap) < top(pad));
    assert!(nodes[bend].label.starts_with('✔'), "bend is on");
    assert!(!nodes[follows].label.starts_with('✔'));
    // Save as Default Pad waits for the Drum Rack's pads.
    assert!(nodes[pad].disabled);

    // A switch turns over in one undo step.
    let steps = h.shell.editor().undo_history().count();
    click(&mut h, &nodes[follows]);
    assert_eq!(param(&h, device, ENV_FOLLOWS_PITCH), Some(1.0));
    assert_eq!(param(&h, device, PER_NOTE_BEND), None, "still the default");
    assert_eq!(h.shell.editor().undo_history().count(), steps + 1);

    // The swap replaces it with Simpler, in one undo step.
    let nodes = open_menu(&mut h, device);
    let steps = h.shell.editor().undo_history().count();
    click(&mut h, &nodes[place(&nodes, "Swap to Simpler")]);
    assert!(h.shell.editor().document().device(device).is_none());
    assert_eq!(h.shell.editor().undo_history().count(), steps + 1);
    assert_eq!(
        h.shell.editor().undo_history().next().unwrap(),
        "Swap to Simpler"
    );
    let snapshot = h
        .shell
        .editor()
        .document()
        .tracks()
        .flat_map(|t| t.devices().cloned().collect::<Vec<_>>())
        .find(|d| matches!(&d.kind, DeviceKind::Native { device_type } if device_type == bl_devices::sampler::SAMPLER))
        .expect("Simpler took its place");
    assert_eq!(snapshot.params.get(&bl_devices::sampler::MODE), Some(&1.0));
}

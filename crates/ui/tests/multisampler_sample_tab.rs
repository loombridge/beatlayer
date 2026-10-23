//! The multisampler's Sample tab (WP-5.13; the multisampler spec, R14),
//! headless: the selected layer's waveform with its markers, each dragged
//! as one undo step, the Sample chooser, the channel views, and the
//! readout while hovering.

use crate::common;
use bl_model::{DeviceId, LayerId};
use common::{Harness, Window};
use egui::{Event, Modifiers, PointerButton, Pos2, pos2, vec2};
use serde_json::json;

/// The folder the sample's file is in stays until the process ends, as the
/// file is what the waveform draws.
fn window() -> (Harness, DeviceId, LayerId) {
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
    let dir = std::env::temp_dir().join(format!("bl-sample-tab-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("Strings.wav");
    let samples: Vec<f32> = (0..48_000)
        .map(|i| (i as f32 * 220.0 / 48_000.0 * std::f32::consts::TAU).sin() * 0.5)
        .collect();
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
            "device.add_layers",
            json!({ "device": device, "paths": [file.to_string_lossy()] }),
        )
        .unwrap();
    h.shell
        .run("device.select", json!({ "device": device }))
        .unwrap();
    h.settle();
    let layer = h
        .shell
        .editor()
        .document()
        .device(device)
        .unwrap()
        .layers
        .list[0]
        .id;
    (h, device, layer)
}

fn centre(b: egui::accesskit::Rect) -> Pos2 {
    pos2(((b.x0 + b.x1) / 2.0) as f32, ((b.y0 + b.y1) / 2.0) as f32)
}

fn layer(h: &Harness, device: DeviceId) -> bl_model::Layer {
    h.shell
        .editor()
        .document()
        .device(device)
        .unwrap()
        .layers
        .list[0]
        .clone()
}

// Spec: multisampler R14
#[test]
fn the_sample_tab_draws_the_selected_layer_and_its_markers() {
    let (mut h, device, id) = window();
    let nodes = Harness::nodes(&h.settle());
    let chooser = nodes.iter().find(|n| n.label == "Sample Chooser").unwrap();
    assert_eq!(
        chooser.value, "Strings",
        "the Sample chooser names the layer"
    );
    let start = nodes
        .iter()
        .find(|n| n.label == "Sample Start" && n.role == egui::accesskit::Role::Slider)
        .unwrap()
        .bounds
        .unwrap();
    let end = nodes
        .iter()
        .find(|n| n.label == "Sample End" && n.role == egui::accesskit::Role::Slider)
        .unwrap()
        .bounds
        .unwrap();
    assert!(start.x0 < end.x0, "start is left of end");
    for channel in ["B", "M", "L", "R"] {
        assert!(nodes.iter().any(|n| n.label == channel), "{channel}");
    }
    // The loop's markers show once a sustain loop is on.
    let marker = |nodes: &[common::Node], name: &str| {
        nodes
            .iter()
            .find(|n| n.label == name && n.role == egui::accesskit::Role::Slider)
            .and_then(|n| n.bounds)
    };
    assert!(
        marker(&nodes, "Loop End").is_none(),
        "no sustain loop, no loop markers"
    );
    h.shell
        .run(
            "device.update_layers",
            json!({ "device": device, "layers": [id], "settings": { "sustain": "loop",
                    "loop_start": 12_000, "loop_end": 36_000 } }),
        )
        .unwrap();
    let nodes = Harness::nodes(&h.settle());
    let loop_end = marker(&nodes, "Loop End").unwrap();
    assert!(
        loop_end.x0 > start.x0 && loop_end.x0 < end.x0,
        "inside the sample"
    );

    // Dragging a marker moves it, and the drag is one undo step.
    let steps = h.shell.editor().undo_history().len();
    let from = centre(loop_end);
    h.frame(vec![Event::PointerMoved(from)]);
    h.frame(vec![Event::PointerButton {
        pos: from,
        button: PointerButton::Primary,
        pressed: true,
        modifiers: Modifiers::NONE,
    }]);
    for i in 1..=10 {
        h.frame(vec![Event::PointerMoved(from + vec2(-8.0 * i as f32, 0.0))]);
    }
    h.frame(vec![Event::PointerButton {
        pos: from + vec2(-80.0, 0.0),
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    }]);
    h.settle();
    let moved = layer(&h, device);
    assert!(moved.loop_end < 36_000, "{}", moved.loop_end);
    assert_eq!(
        h.shell.editor().undo_history().len(),
        steps + 1,
        "one step however far the marker went"
    );
}

// Spec: multisampler R14
#[test]
fn the_waveforms_hover_reads_the_sample_and_the_wheel_zooms() {
    let (mut h, device, _) = window();
    let nodes = Harness::nodes(&h.settle());
    let end = centre(
        nodes
            .iter()
            .find(|n| n.label == "Sample End" && n.role == egui::accesskit::Role::Slider)
            .unwrap()
            .bounds
            .unwrap(),
    );
    let inside = end - vec2(200.0, 0.0);
    h.frame(vec![Event::PointerMoved(inside)]);
    let nodes = Harness::nodes(&h.frame(vec![Event::PointerMoved(inside + vec2(1.0, 0.0))]));
    assert!(
        nodes.iter().any(|n| n.value.contains("Strings")
            && n.value.contains("48000 frames")
            && n.value.contains("48000 Hz")),
        "the Status Bar names the sample's length and rate"
    );
    // Cmd with the wheel zooms the time: Sample End leaves the view.
    for _ in 0..8 {
        h.frame(vec![Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: vec2(0.0, 80.0),
            modifiers: Modifiers::COMMAND,
            phase: egui::TouchPhase::Move,
        }]);
    }
    let nodes = Harness::nodes(&h.settle());
    assert!(
        !nodes
            .iter()
            .any(|n| n.label == "Sample End" && n.role == egui::accesskit::Role::Slider),
        "zoomed in, the end marker is out of view"
    );
    let _ = device;
}

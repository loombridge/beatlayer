//! The drum instruments' panels in the Device View, headless (layout
//! L-DEV-047, L-DEV-048 and L-DEV-015): what each draws of Live's panel, and
//! that its new kinds of control change their parameters as one undo step.
//!
//! Spec: drum-synths R6; one-shot-drum-sampler R29; eight-slot-drum-sampler
//! R22.

use crate::common;
use bl_model::device_layout::Widget;
use bl_model::{DeviceId, ParamId, TrackId};
use bl_shell::device_commands::{DeviceAdded, TITLE_BAR_HEIGHT};
use bl_ui::device_panel::{body_rect, place};
use bl_ui::device_view::layout;
use common::{Harness, Window};
use egui::{Rect, Shape, pos2};
use serde_json::json;

/// A window with the device named `name` on a track.
fn window(name: &str) -> (Harness, DeviceId) {
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
    let t = bl_devices::device_types()
        .into_iter()
        .find(|t| t.name == name)
        .expect("a device");
    let device = json!({ "kind": t.kind, "name": t.name, "role": t.role });
    let added: DeviceAdded = serde_json::from_value(
        h.shell
            .run("device.add", json!({ "device": device }))
            .unwrap(),
    )
    .unwrap();
    h.settle();
    (h, added.device)
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

/// Where each control of the device is drawn, with its widget.
fn controls(h: &mut Harness, device: DeviceId) -> Vec<(ParamId, Widget, Rect)> {
    let frame = frame_of(h, device);
    let state = h.shell.device_view_state();
    let panel = state
        .devices
        .iter()
        .find(|d| d.id == device)
        .unwrap()
        .panel
        .clone()
        .unwrap();
    place(&panel, body_rect(frame, TITLE_BAR_HEIGHT)).0
}

/// The text painted in `area`.
fn texts(output: &egui::FullOutput, area: Rect) -> Vec<String> {
    fn walk(shape: &Shape, out: &mut Vec<String>, area: Rect) {
        match shape {
            Shape::Vec(all) => all.iter().for_each(|s| walk(s, out, area)),
            Shape::Text(t) if area.intersects(t.visual_bounding_rect()) => {
                out.push(t.galley.text().to_owned());
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    for clipped in &output.shapes {
        walk(&clipped.shape, &mut out, area);
    }
    out
}

fn has(found: &[String], text: &str) -> bool {
    found.iter().any(|t| t == text)
}

// Spec: drum-synths R6
#[test]
fn ds_kick_draws_its_pitch_large_with_its_unit_and_its_note() {
    let (mut h, device) = window("DS Kick");
    let frame = frame_of(&mut h, device);
    let found = texts(&h.settle(), frame);
    for text in [
        "Pitch", "43.3", "Hz", "F0", "Drive/OT", "Attack", "Click", "Ø", "Decay", "48.2 %", "Env",
        "47.4 %", "Volume", "-6.0 dB",
    ] {
        assert!(has(&found, text), "{text} is drawn: {found:?}");
    }
    assert_eq!(found.iter().filter(|t| *t == "0.00 %").count(), 3);
}

// Spec: drum-synths R6
#[test]
fn ds_tom_reads_its_hertz_under_a_plain_pitch() {
    let (mut h, device) = window("DS Tom");
    let frame = frame_of(&mut h, device);
    let found = texts(&h.settle(), frame);
    for text in ["29", "84.0 Hz", "E1", "43", "100", "Bend", "31.0 %"] {
        assert!(has(&found, text), "{text} is drawn: {found:?}");
    }
}

// Spec: drum-synths R6
#[test]
fn ds_clap_draws_two_large_numbers_and_a_pair_of_boxes_under_one_name() {
    let (mut h, device) = window("DS Clap");
    let frame = frame_of(&mut h, device);
    let found = texts(&h.settle(), frame);
    for text in [
        "Sloppy",
        "24",
        "Tail",
        "65",
        "Spread/Tone",
        "76 %",
        "32 %",
        "18.0 %",
    ] {
        assert!(has(&found, text), "{text} is drawn: {found:?}");
    }
}

// Spec: one-shot-drum-sampler R29
#[test]
fn drum_samplers_source_button_turns_over_in_one_undo_step() {
    let (mut h, device) = window("Drum Sampler");
    let found = texts(&h.settle(), frame_of(&mut h, device));
    for text in [
        "Factor", "1.00", "Grain", "100 ms", "C", "Velocity", "Mod", "Transp",
    ] {
        assert!(has(&found, text), "{text} is drawn: {found:?}");
    }
    let source = bl_devices::drum_sampler::MOD_SOURCE;
    let rect = controls(&mut h, device)
        .into_iter()
        .find(|(p, _, _)| *p == source)
        .map(|c| c.2)
        .unwrap();
    let before = h.shell.editor().undo_history().len();
    h.click(rect.center());
    h.settle();
    let read = |h: &Harness| {
        h.shell
            .editor()
            .document()
            .device(device)
            .unwrap()
            .params
            .get(&source)
            .copied()
            .unwrap_or(0.0)
    };
    assert_eq!(read(&h), 1.0, "Slide");
    assert_eq!(h.shell.editor().undo_history().len(), before + 1);
    let found = texts(&h.settle(), frame_of(&mut h, device));
    assert!(has(&found, "Slide"), "the button reads the source");
    h.click(rect.center());
    h.settle();
    assert_eq!(read(&h), 0.0, "and back to Velocity");
}

// Spec: eight-slot-drum-sampler R22
#[test]
fn impulse_writes_its_boxes_names_and_short_buttons_as_live_does() {
    let (mut h, device) = window("Impulse");
    let found = texts(&h.settle(), frame_of(&mut h, device));
    for text in [
        "Start", "0.00 ms", "Transp", "Velocity", "Random", "Mode", "Sat", "Filter", "Res", "Freq",
        "Decay", "2.00 s", "Trig", "Pan", "C", "Volume", "70.0 %", "M", "S", "Time",
    ] {
        assert!(has(&found, text), "{text} is drawn: {found:?}");
    }
    // The long names stay off the panel.
    for long in ["Transpose Velocity", "Saturator", "Mute", "Solo"] {
        assert!(!has(&found, long), "{long} isn't drawn");
    }
}

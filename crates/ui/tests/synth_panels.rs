//! The synthesisers' panels in the Device View, headless (layout L-DEV-102
//! to L-DEV-105): what each draws of Live's panel, and that the sections
//! show their pages when clicked, as one undo step each.
//!
//! Spec: electric R16; external-devices R9.

use crate::common;
use bl_model::device_layout::Widget;
use bl_model::{DeviceId, ParamId, TrackId};
use bl_shell::device_commands::{DeviceAdded, TITLE_BAR_HEIGHT, sidechain_width};
use bl_ui::device_panel::{body_rect, place_with_pages};
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

/// The panel's controls where they are drawn, and its pages nodes' regions.
fn placed(
    h: &mut Harness,
    device: DeviceId,
) -> (
    Vec<(ParamId, Widget, Rect)>,
    Vec<bl_model::device_layout::PlacedPages>,
) {
    let frame = frame_of(h, device);
    let state = h.shell.device_view_state();
    let shown = state.devices.iter().find(|d| d.id == device).unwrap();
    let panel = shown.panel.clone().unwrap();
    // Right of the sections down the frame's left (the External Instrument's).
    let mut body = body_rect(frame, TITLE_BAR_HEIGHT);
    body.min.x += sidechain_width(shown);
    let (controls, _, pages) = place_with_pages(&panel, false, body);
    (controls, pages)
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

fn value(h: &Harness, device: DeviceId, param: ParamId) -> f64 {
    let document = h.shell.editor().document();
    let device = document.device(device).unwrap();
    device.params.get(&param).copied().unwrap_or_else(|| {
        bl_devices::device_types()
            .into_iter()
            .find(|t| t.name == device.name)
            .and_then(|t| t.description.param(param).map(|p| p.default))
            .expect("a parameter")
    })
}

// Spec: electric R16
#[test]
fn electric_draws_its_sections_over_the_hammer_table_and_each_click_shows_its_page() {
    use bl_devices::electric::{SECTIONS, TINE};
    let (mut h, device) = window("Electric");
    let frame = frame_of(&mut h, device);
    assert_eq!(frame.width(), 345.0, "Live's panel is 345 points wide");
    let found = texts(&h.settle(), frame);
    for text in [
        "Hammer",
        "Fork",
        "Damper / Pickup",
        "Global",
        "Stiffness",
        "Noise",
        "Tine",
        "Tone",
        "Symmetry",
        "Type",
        "Volume",
        "Force",
        "Amount",
        "Vel",
        "Key",
        "Pitch",
        "Decay",
    ] {
        assert!(has(&found, text), "{text} is drawn: {found:?}");
    }
    // The Hammer's table is the display's, so the Fork's isn't.
    assert!(
        !has(&found, "Color") && !has(&found, "Release"),
        "{found:?}"
    );
    // A click on the Fork's heading shows the Fork's table, as one undo step.
    let (controls, pages) = placed(&mut h, device);
    let fork = pages
        .iter()
        .find(|p| p.name == SECTIONS)
        .expect("the sections");
    assert_eq!(fork.names, ["Hammer", "Fork", "Damper/Pickup", "Global"]);
    let region = fork.buttons[1];
    let region = Rect::from_min_size(
        pos2(region.x, region.y),
        egui::vec2(region.width, region.height),
    );
    let tine = controls.iter().find(|c| c.0 == TINE).map(|c| c.2).unwrap();
    assert!(
        region.contains(tine.center()),
        "the Fork's region holds its knobs"
    );
    // Over the heading, where no control takes the click.
    h.click(pos2(region.center().x, region.top() + 2.0));
    let found = texts(&h.settle(), frame);
    for text in ["Color", "Release", "Tine", "Tone"] {
        assert!(has(&found, text), "{text} is drawn: {found:?}");
    }
    assert!(!has(&found, "Force"), "the Hammer's table is gone");
}

// Spec: electric R16
#[test]
fn electrics_voices_is_a_chooser_of_whole_numbers_that_sets_one_undo_step() {
    use bl_devices::electric::{SECTIONS, VOICES};
    let (mut h, device) = window("Electric");
    h.shell
        .run(
            "device.set_page",
            json!({ "device": device, "node": SECTIONS, "page": "Global" }),
        )
        .unwrap();
    h.settle();
    let (controls, _) = placed(&mut h, device);
    let voices = controls.iter().find(|c| c.0 == VOICES).expect("Voices");
    assert_eq!(voices.1, Widget::UnboxedChooser);
    let steps = h.shell.editor().undo_history().len();
    h.click(voices.2.center());
    let nodes = Harness::nodes(&h.settle());
    let entry = nodes
        .iter()
        .find(|n| n.label == "8" && n.bounds.is_some_and(|b| b.y1 - b.y0 > 8.0))
        .expect("the menu lists 8");
    let b = entry.bounds.unwrap();
    h.click(pos2(
        ((b.x0 + b.x1) / 2.0) as f32,
        ((b.y0 + b.y1) / 2.0) as f32,
    ));
    h.settle();
    assert_eq!(value(&h, device, VOICES), 8.0);
    assert_eq!(h.shell.editor().undo_history().len(), steps + 1);
}

// Spec: external-devices R9
#[test]
fn the_external_instruments_frame_is_lives_129_points_with_its_choosers_in_lives_order() {
    use bl_ui::external::rows;
    let (mut h, device) = window("External Instrument");
    let frame = frame_of(&mut h, device);
    assert_eq!(frame.width(), 129.0, "88 of section and 41 of body");
    let [midi_to, channel, audio_from, latency, unit] = rows(frame);
    // Top to bottom as Live has them, MIDI To and its channel across the
    // frame, Audio From beside the Gain knob, Hardware Latency at the foot.
    assert!(midi_to.bottom() < channel.top());
    assert!(channel.bottom() < audio_from.top());
    assert!(audio_from.bottom() < latency.top());
    assert_eq!(midi_to.width(), 121.0);
    assert_eq!(audio_from.width(), 78.0);
    assert_eq!(latency.top(), unit.top());
    assert!(latency.right() < unit.left());
    let (controls, _) = placed(&mut h, device);
    let gain = controls[0].2;
    assert!(
        gain.left() >= audio_from.right(),
        "Gain is right of Audio From"
    );
    assert!(gain.top() > channel.bottom(), "and under the channel");
    assert!(frame.contains_rect(gain));
    let found = texts(&h.settle(), frame);
    for text in ["MIDI To", "Audio From", "Hardware Latency", "Gain", "ms"] {
        assert!(has(&found, text), "{text} is drawn: {found:?}");
    }
    // The box holds the number and the switch its unit.
    assert!(has(&found, "0.00") && has(&found, "0.00 dB"), "{found:?}");
}

// Spec: polysynth R37
#[test]
fn drift_draws_its_sections_and_the_mod_tab_and_an_arrow_sends_its_source_round_the_filter() {
    use bl_devices::polysynth::{OSC2_FILTER, POLYSYNTH};
    let (mut h, device) = window("Drift");
    let frame = frame_of(&mut h, device);
    assert!((frame.width() - 762.3).abs() < 0.5, "{}", frame.width());
    let found = texts(&h.settle(), frame);
    for text in [
        "Osc Mix",
        "Filter",
        "Envelopes",
        "LFO",
        "Mod",
        "Osc 1",
        "Osc 2",
        "Noise",
        "Pitch Mod",
        "Freq Mod",
        "Freq",
        "Type",
        "Attack",
        "Decay",
        "Sustain",
        "Release",
        "Rate",
        "Amount",
        "Volume",
        "Vel > Vol",
        "Transpose",
        "Glide",
        "20.0 kHz",
        "0.40 Hz",
        "7.2 %",
    ] {
        assert!(has(&found, text), "{text} is drawn: {found:?}");
    }
    // The Mod tab replaces the LFO's controls with three slots.
    h.shell
        .run(
            "device.set_page",
            json!({ "device": device, "node": "Tabs", "page": "Mod" }),
        )
        .unwrap();
    let found = texts(&h.settle(), frame);
    for text in ["Modwheel", "HP Frequency", "Velocity", "Pressure", "None"] {
        assert!(has(&found, text), "{text} is drawn: {found:?}");
    }
    assert!(
        !has(&found, "0.40 Hz"),
        "the LFO's rate is on the other tab"
    );
    // An arrow beside Osc 2's gain is its filter switch: a click turns it off
    // as one undo step.
    let _ = POLYSYNTH;
    let (controls, _) = placed(&mut h, device);
    let arrow = controls
        .iter()
        .find(|c| c.0 == OSC2_FILTER)
        .map(|c| c.2)
        .expect("Osc 2's arrow");
    let steps = h.shell.editor().undo_history().len();
    h.click(arrow.center());
    h.settle();
    assert_eq!(value(&h, device, OSC2_FILTER), 0.0);
    assert_eq!(h.shell.editor().undo_history().len(), steps + 1);
}

// Spec: operator R38
#[test]
fn operator_draws_its_rows_and_headings_and_the_check_boxes_show_the_views() {
    use bl_devices::operator::{FILTER_ON, PAN};
    let (mut h, device) = window("Operator");
    let frame = frame_of(&mut h, device);
    assert!((frame.width() - 674.0).abs() < 1.0, "{}", frame.width());
    let found = texts(&h.settle(), frame);
    for text in [
        "Coarse",
        "Fine",
        "Fixed",
        "Level",
        "Envelope",
        "Oscillator",
        "Attack",
        "Decay",
        "Release",
        "Initial",
        "Peak",
        "Sustain",
        "Loop",
        "Wave",
        "Feedback",
        "Repeat",
        "Phase",
        "LFO",
        "Filter",
        "Rate",
        "Amount",
        "Freq",
        "Res",
        "Pitch Env",
        "Spread",
        "Transpose",
        "Time",
        "Tone",
        "Volume",
        "12.0 kHz",
        "-18.0 dB",
    ] {
        assert!(has(&found, text), "{text} is drawn: {found:?}");
    }
    // The Envelope check box shows that view; the Oscillator's shows the
    // partials.
    h.shell
        .run(
            "device.set_page",
            json!({ "device": device, "node": "Sections", "page": "Osc B" }),
        )
        .unwrap();
    h.settle();
    let nodes = Harness::nodes(&h.settle());
    assert!(Harness::node(&nodes, "Osc B Envelope view").selected == Some(true));
    h.click(centre_of(&nodes, "Osc B Oscillator view"));
    let nodes = Harness::nodes(&h.settle());
    assert!(Harness::node(&nodes, "Osc B Oscillator view").selected == Some(true));
    // The filter is on in a new device, and a click on its box turns it off.
    let (controls, _) = placed(&mut h, device);
    let filter = controls
        .iter()
        .find(|c| c.0 == FILTER_ON)
        .map(|c| c.2)
        .unwrap();
    h.click(filter.center());
    h.settle();
    assert_eq!(value(&h, device, FILTER_ON), 0.0);
    // Pan reads C at the middle.
    h.shell
        .run(
            "device.set_page",
            json!({ "device": device, "node": "Sections", "page": "Global" }),
        )
        .unwrap();
    let found = texts(&h.settle(), frame);
    assert!(has(&found, "C"), "Pan reads C: {found:?}");
    let _ = PAN;
}

/// The centre of the node labelled `label`.
fn centre_of(nodes: &[common::Node], label: &str) -> egui::Pos2 {
    let b = Harness::node(nodes, label).bounds.expect("bounds");
    pos2(((b.x0 + b.x1) / 2.0) as f32, ((b.y0 + b.y1) / 2.0) as f32)
}

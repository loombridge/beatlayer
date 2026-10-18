//! The multisampler's Pitch/Osc, Filter/Global, Modulation and MIDI tabs
//! (WP-5.14; the multisampler spec, R1 and R32 to R46), headless: each tab
//! shows its own controls in a device as wide whichever tab shows, a drag
//! on a control is one undo step named for its parameter, and a tab's
//! indicator lights while a routing sends to a control in it.

mod common;

use bl_devices::multisampler::{AUX_AMOUNT_A, AUX_DEST_A, AUX_ON, TRANSPOSE, dest, midi_param};
use bl_model::{DeviceId, ParamId};
use bl_shell::device_commands::{DeviceAdded, TITLE_BAR_HEIGHT, frame_width};
use bl_ui::device_panel::{DRAG_TRAVEL, body_rect, place};
use bl_ui::device_view::layout;
use common::{Harness, Window};
use egui::{Event, Modifiers, PointerButton, Pos2, Rect, pos2, vec2};
use serde_json::json;

fn multisampler() -> (Harness, DeviceId) {
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
    let added: DeviceAdded = serde_json::from_value(
        h.shell
            .run(
                "device.add",
                json!({ "device": { "kind": t.kind, "name": t.name, "role": t.role } }),
            )
            .unwrap(),
    )
    .unwrap();
    h.settle();
    (h, added.device)
}

fn show(h: &mut Harness, device: DeviceId, tab: &str) {
    h.shell
        .run(
            "device.set_multisampler_view",
            json!({ "device": device, "tab": tab }),
        )
        .unwrap();
    h.settle();
}

fn frame_state(h: &Harness, device: DeviceId) -> bl_shell::device_commands::DeviceFrame {
    h.shell
        .device_view_state()
        .devices
        .into_iter()
        .find(|d| d.id == device)
        .unwrap()
}

/// The parameters whose controls the shown tab places.
fn placed(h: &mut Harness, device: DeviceId) -> Vec<(ParamId, Rect)> {
    let nodes = Harness::nodes(&h.settle());
    let b = Harness::node(&nodes, "Device View").bounds.unwrap();
    let view = Rect::from_min_max(
        pos2(b.x0 as f32, b.y0 as f32),
        pos2(b.x1 as f32, b.y1 as f32),
    );
    let frame = layout(&h.shell, view)
        .into_iter()
        .find(|(id, _)| *id == device)
        .unwrap()
        .1;
    let panel = frame_state(h, device).panel.unwrap();
    place(&panel, body_rect(frame, TITLE_BAR_HEIGHT))
        .0
        .into_iter()
        .map(|(param, _, rect)| (param, rect))
        .collect()
}

fn names(h: &Harness, device: DeviceId, shown: &[ParamId]) -> Vec<String> {
    let panel = frame_state(h, device).panel.unwrap();
    shown
        .iter()
        .map(|id| {
            panel
                .description
                .params
                .iter()
                .find(|p| p.id == *id)
                .unwrap()
                .name
                .clone()
        })
        .collect()
}

// Spec: multisampler R1
#[test]
fn each_tab_shows_its_own_controls_in_a_device_as_wide_whichever_tab_shows() {
    let (mut h, device) = multisampler();
    let expected: [(&str, &[&str]); 4] = [
        (
            "pitch_osc",
            &[
                "Transpose",
                "Detune",
                "Spread",
                "Key Zone Shift",
                "Glide",
                "Glide Time",
                "Pitch Env Amount",
                "Pitch Init",
                "Osc On",
                "Osc Type",
                "Osc Volume",
                "Osc Fine",
            ],
        ),
        (
            "filter_global",
            &[
                "Filter On",
                "Filter Type",
                "Filter Freq",
                "Filter Res",
                "Filter Morph",
                "Filter Drive",
                "Filter Env Amount",
                "Shaper Type",
                "Shaper Post",
                "Env Attack",
                "Volume",
                "Time",
            ],
        ),
        (
            "modulation",
            &[
                "Aux On",
                "Aux Dest A",
                "Aux Amount B",
                "LFO 1 On",
                "LFO 1 Pitch",
                "LFO 2 Dest A",
                "LFO 2 Phase",
                "LFO 3 Spin",
            ],
        ),
        (
            "midi",
            &[
                "Key Dest A",
                "Velocity Amount A",
                "Release Velocity Dest B",
                "Aftertouch Amount A",
                "Mod Wheel Dest A",
                "Foot Controller Amount B",
                "Pitch Bend Dest A",
                "Pitch Bend Range",
            ],
        ),
    ];
    let mut widths = Vec::new();
    for (tab, controls) in expected {
        show(&mut h, device, tab);
        let shown: Vec<ParamId> = placed(&mut h, device).into_iter().map(|(p, _)| p).collect();
        let shown = names(&h, device, &shown);
        for name in controls {
            assert!(
                shown.iter().any(|n| n == name),
                "{tab} lacks {name}: {shown:?}"
            );
        }
        widths.push(frame_width(&frame_state(&h, device)));
    }
    show(&mut h, device, "sample");
    widths.push(frame_width(&frame_state(&h, device)));
    assert!(
        widths.windows(2).all(|w| (w[0] - w[1]).abs() < 0.01),
        "one width: {widths:?}"
    );
    // A tab's controls are its own: Transpose isn't on the MIDI tab.
    show(&mut h, device, "midi");
    let on_midi: Vec<ParamId> = placed(&mut h, device).into_iter().map(|(p, _)| p).collect();
    assert!(!on_midi.contains(&TRANSPOSE));
}

fn press(at: Pos2, pressed: bool) -> Event {
    Event::PointerButton {
        pos: at,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    }
}

// Spec: multisampler R32
#[test]
fn a_drag_on_a_tabs_knob_is_one_undo_step_named_for_its_parameter() {
    let (mut h, device) = multisampler();
    show(&mut h, device, "pitch_osc");
    let knob = placed(&mut h, device)
        .into_iter()
        .find(|(p, _)| *p == TRANSPOSE)
        .unwrap()
        .1;
    let before = h.shell.editor().undo_history().len();
    let from = knob.center();
    h.frame(vec![Event::PointerMoved(from)]);
    h.frame(vec![press(from, true)]);
    for i in 1..=10 {
        h.frame(vec![Event::PointerMoved(
            from + vec2(0.0, -(DRAG_TRAVEL / 4.0) * i as f32 / 10.0),
        )]);
    }
    h.frame(vec![press(from - vec2(0.0, DRAG_TRAVEL / 4.0), false)]);
    h.settle();
    assert_eq!(h.shell.editor().undo_history().len(), before + 1);
    assert_eq!(
        h.shell.editor().undo_history().next(),
        Some("Change \"Transpose\"")
    );
    let value = h
        .shell
        .editor()
        .document()
        .device(device)
        .unwrap()
        .params
        .get(&TRANSPOSE)
        .copied()
        .unwrap();
    // A quarter of the travel up from 0 on −48 to 48 semitones.
    assert!((value - 24.0).abs() < 4.0, "{value}");
}

/// Whether the tab named `label` says modulation is assigned in it.
fn marked(h: &mut Harness, label: &str) -> bool {
    Harness::nodes(&h.settle())
        .iter()
        .find(|n| n.label == label && n.role == egui::accesskit::Role::Button)
        .unwrap_or_else(|| panic!("no {label} tab"))
        .description
        .contains("modulation is assigned")
}

// Spec: multisampler R1
#[test]
fn a_tabs_indicator_lights_while_a_routing_sends_to_a_control_in_it() {
    let (mut h, device) = multisampler();
    let set = |h: &mut Harness, param: ParamId, value: f64| {
        h.shell
            .run(
                "device.set_param",
                json!({ "device": device, "param": param.0, "value": value }),
            )
            .unwrap();
        h.settle();
    };
    for tab in ["Sample", "Pitch/Osc", "Filter/Global", "Modulation", "MIDI"] {
        assert!(!marked(&mut h, tab), "{tab} starts dark");
    }
    // The auxiliary envelope to the filter's cutoff lights Filter/Global; with
    // no amount, or off, nothing does.
    set(&mut h, AUX_DEST_A, dest::FILTER_FREQ as f64);
    set(&mut h, AUX_ON, 1.0);
    assert!(!marked(&mut h, "Filter/Global"), "no amount, no routing");
    set(&mut h, AUX_AMOUNT_A, 40.0);
    assert!(marked(&mut h, "Filter/Global"));
    assert!(!marked(&mut h, "Pitch/Osc") && !marked(&mut h, "Modulation"));
    // A MIDI source to Pitch lights Pitch/Osc, and to Sample Start, Sample.
    set(&mut h, midi_param(1, 0), dest::PITCH as f64);
    set(&mut h, midi_param(1, 2), -10.0);
    assert!(marked(&mut h, "Pitch/Osc"));
    set(&mut h, midi_param(4, 1), dest::SAMPLE_START as f64);
    set(&mut h, midi_param(4, 3), 10.0);
    assert!(marked(&mut h, "Sample"));
    set(&mut h, midi_param(0, 0), dest::LFO2_RATE as f64);
    set(&mut h, midi_param(0, 2), 5.0);
    assert!(marked(&mut h, "Modulation"));
    // Sending to None, or switching the envelope off, puts them out again.
    set(&mut h, AUX_ON, 0.0);
    assert!(!marked(&mut h, "Filter/Global"));
    set(&mut h, midi_param(1, 0), dest::NONE as f64);
    assert!(!marked(&mut h, "Pitch/Osc"));
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

// Spec: multisampler R47
#[test]
fn an_sfz_dropped_on_a_midi_tracks_device_view_makes_a_multisampler_or_adds_layers_to_one() {
    let sfz = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/sfz/Sine Bank.sfz");
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
    let nodes = Harness::nodes(&h.settle());
    let b = Harness::node(&nodes, "Device View").bounds.unwrap();
    let view = Rect::from_min_max(
        pos2(b.x0 as f32, b.y0 as f32),
        pos2(b.x1 as f32, b.y1 as f32),
    );
    let devices = |h: &Harness| {
        h.shell
            .editor()
            .document()
            .track(midi)
            .unwrap()
            .devices()
            .count()
    };
    let tracks = |h: &Harness| h.shell.editor().document().tracks().count();
    let before = tracks(&h);
    assert_eq!(devices(&h), 0);
    // Dropped on an empty Device View: a multisampler on the track.
    h.frame(vec![Event::PointerMoved(view.center())]);
    h.dropped
        .push(std::sync::Arc::new(DroppedFile(sfz.clone())));
    h.settle();
    assert_eq!(devices(&h), 1);
    let device = h
        .shell
        .editor()
        .document()
        .track(midi)
        .unwrap()
        .devices()
        .next()
        .unwrap()
        .id;
    let layers = |h: &Harness| {
        h.shell
            .editor()
            .document()
            .device(device)
            .unwrap()
            .layers
            .list
            .len()
    };
    assert_eq!(layers(&h), 5);
    // Dropped on that multisampler: its regions join its layers.
    let frame = layout(&h.shell, view)
        .into_iter()
        .find(|(id, _)| *id == device)
        .unwrap()
        .1;
    h.frame(vec![Event::PointerMoved(frame.center())]);
    h.dropped.push(std::sync::Arc::new(DroppedFile(sfz)));
    h.settle();
    assert_eq!(devices(&h), 1, "no second device");
    assert_eq!(layers(&h), 10);
    assert_eq!(tracks(&h), before, "the drop makes no new track");
}

// Spec: multisampler R37
#[test]
fn the_morph_knobs_menu_snaps_it_to_a_type() {
    let (mut h, device) = multisampler();
    show(&mut h, device, "filter_global");
    let knob = placed(&mut h, device)
        .into_iter()
        .find(|(p, _)| *p == bl_devices::multisampler::FILTER_MORPH)
        .unwrap()
        .1;
    let at = knob.center();
    h.frame(vec![Event::PointerMoved(at)]);
    for pressed in [true, false] {
        h.frame(vec![Event::PointerButton {
            pos: at,
            button: PointerButton::Secondary,
            pressed,
            modifiers: Modifiers::NONE,
        }]);
    }
    let nodes = Harness::nodes(&h.settle());
    for item in ["Low-pass", "Band-pass", "High-pass", "Notch"] {
        assert!(
            nodes.iter().any(|n| n.label == item && n.bounds.is_some()),
            "the menu has {item}"
        );
    }
    let b = nodes
        .iter()
        .find(|n| n.label == "High-pass" && n.bounds.is_some())
        .unwrap()
        .bounds
        .unwrap();
    let before = h.shell.editor().undo_history().len();
    h.click(pos2(
        ((b.x0 + b.x1) / 2.0) as f32,
        ((b.y0 + b.y1) / 2.0) as f32,
    ));
    h.settle();
    let value = h
        .shell
        .editor()
        .document()
        .device(device)
        .unwrap()
        .params
        .get(&bl_devices::multisampler::FILTER_MORPH)
        .copied();
    assert_eq!(value, Some(50.0));
    assert_eq!(h.shell.editor().undo_history().len(), before + 1);
}

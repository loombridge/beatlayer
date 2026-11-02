//! The generic device UI (WP-2.5), headless: a described device's controls
//! drawn from its descriptors and its layout, each change one undo step
//! named for its parameter, and the layout placed at larger sizes for
//! floating windows. Devices are the framework's test devices.
//!
//! Spec: device-framework R15 to R20.

use crate::common;
use bl_devices::testing::{self, SCOPE_TIME, UTILITY_GAIN, UTILITY_MUTE};
use bl_model::param::db_to_gain;
use bl_model::{DeviceId, TrackId};
use bl_shell::device_commands::{DeviceAdded, TITLE_BAR_HEIGHT, frame_width};
use bl_ui::device_panel::{DRAG_TRAVEL, body_rect, highlight, param_control_id, place};
use bl_ui::device_view::layout;
use bl_ui::theme::Theme;
use common::{Harness, Window};
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

/// Where a parameter's control is drawn.
fn control(h: &mut Harness, device: DeviceId, param: bl_model::ParamId) -> Rect {
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
    place(&panel, body_rect(frame, TITLE_BAR_HEIGHT))
        .0
        .into_iter()
        .find(|(p, _, _)| *p == param)
        .unwrap()
        .2
}

fn value(h: &Harness, device: DeviceId, param: bl_model::ParamId) -> Option<f64> {
    h.shell
        .editor()
        .document()
        .device(device)
        .unwrap()
        .params
        .get(&param)
        .copied()
}

fn last_step(h: &Harness) -> String {
    h.shell
        .editor()
        .undo_history()
        .next()
        .unwrap_or_default()
        .to_owned()
}

fn press(at: Pos2, pressed: bool) -> Event {
    Event::PointerButton {
        pos: at,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    }
}

fn drag(h: &mut Harness, from: Pos2, dy: f32) {
    h.frame(vec![Event::PointerMoved(from)]);
    h.frame(vec![press(from, true)]);
    let steps = 10;
    for i in 1..=steps {
        h.frame(vec![Event::PointerMoved(
            from + vec2(0.0, dy * i as f32 / steps as f32),
        )]);
    }
    h.frame(vec![press(from + vec2(0.0, dy), false)]);
    h.settle();
}

// Spec: device-framework R15, R16
#[test]
fn a_described_device_draws_a_control_for_every_parameter() {
    let mut h = harness();
    let utility = add(&mut h, &testing::utility_type());
    let nodes = Harness::nodes(&h.settle());
    let gain = Harness::node(&nodes, "Gain");
    assert_eq!(gain.role, Role::Slider);
    assert_eq!(gain.value, "0.00 dB");
    assert_eq!(Harness::node(&nodes, "Balance").role, Role::Slider);
    assert_eq!(Harness::node(&nodes, "Mute").role, Role::Switch);
    let polarity = Harness::node(&nodes, "Polarity");
    assert_eq!(polarity.role, Role::ComboBox);
    assert_eq!(polarity.value, "Normal");
    // The frame is the layout's docked width.
    let frame = frame_of(&mut h, utility);
    let state = h.shell.device_view_state();
    assert_eq!(frame.width(), frame_width(&state.devices[0]));
    assert_eq!(
        frame.width(),
        testing::utility_type()
            .description
            .layout
            .docked_size()
            .width
    );
}

// Spec: device-framework R17, R18
#[test]
fn dragging_a_knob_is_one_undo_step_named_for_its_parameter() {
    let mut h = harness();
    let utility = add(&mut h, &testing::utility_type());
    let knob = control(&mut h, utility, UTILITY_GAIN);
    let before = h.shell.editor().undo_history().len();
    // Down the whole travel: to −∞ dB.
    drag(&mut h, knob.center(), DRAG_TRAVEL + 10.0);
    assert_eq!(value(&h, utility, UTILITY_GAIN), Some(0.0));
    assert_eq!(h.shell.editor().undo_history().len(), before + 1);
    assert_eq!(last_step(&h), "Change \"Gain\"");
    let nodes = Harness::nodes(&h.settle());
    assert_eq!(Harness::node(&nodes, "Gain").value, "-inf dB");
    // Up a tenth of the travel moves a tenth of the way: relative.
    drag(&mut h, knob.center(), -DRAG_TRAVEL / 10.0);
    let gain = testing::utility_type().description.params[0].clone();
    let moved = gain.normalize(value(&h, utility, UTILITY_GAIN).unwrap());
    assert!((moved - 0.1).abs() < 0.02, "{moved}");
}

// Spec: device-framework R17
#[test]
fn a_double_click_or_delete_sets_the_default() {
    let mut h = harness();
    let utility = add(&mut h, &testing::utility_type());
    h.shell
        .invoke(
            "device.set_param",
            json!({ "device": utility, "param": UTILITY_GAIN, "value": 0.5 }),
        )
        .unwrap();
    let knob = control(&mut h, utility, UTILITY_GAIN);
    h.time += 1.0;
    h.frame(vec![Event::PointerMoved(knob.center())]);
    h.frame(vec![
        press(knob.center(), true),
        press(knob.center(), false),
    ]);
    h.frame(vec![
        press(knob.center(), true),
        press(knob.center(), false),
    ]);
    h.settle();
    assert_eq!(value(&h, utility, UTILITY_GAIN), Some(1.0));
    assert_eq!(last_step(&h), "Change \"Gain\"");

    h.shell
        .invoke(
            "device.set_param",
            json!({ "device": utility, "param": UTILITY_GAIN, "value": db_to_gain(-12.0) }),
        )
        .unwrap();
    h.settle();
    h.time += 1.0;
    h.click(knob.center());
    h.settle();
    assert_eq!(
        h.shell.active_contexts()[0],
        bl_commands::Context::ValueField
    );
    h.key(egui::Key::Delete, Modifiers::NONE);
    h.settle();
    assert_eq!(value(&h, utility, UTILITY_GAIN), Some(1.0));
}

// Spec: device-framework R19
#[test]
fn a_switch_and_a_chooser_change_their_parameter() {
    let mut h = harness();
    let utility = add(&mut h, &testing::utility_type());
    let mute = control(&mut h, utility, UTILITY_MUTE);
    h.click(pos2(mute.center().x, mute.bottom() - 10.0));
    h.settle();
    assert_eq!(value(&h, utility, UTILITY_MUTE), Some(1.0));
    assert_eq!(last_step(&h), "Change \"Mute\"");
    // A screen reader's steps reach a knob too.
    let id = param_control_id(utility, UTILITY_GAIN);
    h.accesskit(id, egui::accesskit::Action::Decrement);
    h.settle();
    assert!(value(&h, utility, UTILITY_GAIN).unwrap() < 1.0);
}

// Spec: device-framework R20 (observed: off, highlights turn grey)
#[test]
fn a_switched_off_device_greys_its_highlights_but_not_its_text() {
    let theme = Theme::bundled(true);
    assert_eq!(highlight(&theme, true), theme.colors.accent.0);
    assert_eq!(highlight(&theme, false), theme.colors.inactive.0);
    assert_ne!(theme.colors.inactive.0, theme.colors.text.0);
}

// Spec: device-framework R11, R16 (IMP-14: a floating window places the
// layout at a larger body)
#[test]
fn a_larger_body_grows_only_the_display_cells() {
    let mut h = harness();
    let scope = add(&mut h, &testing::scope_type());
    let frame = frame_of(&mut h, scope);
    let nodes = Harness::nodes(&h.settle());
    assert_eq!(Harness::node(&nodes, "Scope").role, Role::Image);
    let state = h.shell.device_view_state();
    let panel = state.devices[0].panel.clone().unwrap();
    let docked = body_rect(frame, TITLE_BAR_HEIGHT);
    let (controls, cells) = place(&panel, docked);
    let big = Rect::from_min_size(docked.min, docked.size() + vec2(300.0, 200.0));
    let (big_controls, big_cells) = place(&panel, big);
    for ((p, _, a), (q, _, b)) in controls.iter().zip(&big_controls) {
        assert_eq!(p, q);
        assert_eq!(a.size(), b.size(), "a control keeps its size");
    }
    assert_eq!(big_cells[0].1.width(), cells[0].1.width() + 300.0);
    assert_eq!(big_cells[0].1.height(), cells[0].1.height() + 200.0);
    // The time knob on the left stays where it was.
    let time = |c: &[(bl_model::ParamId, bl_model::device_layout::Widget, Rect)]| {
        c.iter().find(|(p, _, _)| *p == SCOPE_TIME).unwrap().2
    };
    assert_eq!(time(&controls), time(&big_controls));
}

/// A harness with Beatlayer's own devices, and the engine off so the
/// display data a test sets stays (the engine would publish its own).
fn display_harness() -> Harness {
    let mut h = harness();
    h.shell.set_device_types(bl_devices::device_types());
    h.shell.run("audio.toggle_engine", json!({})).unwrap();
    h.frame(vec![]);
    h
}

fn add_native(h: &mut Harness, device_type: &str) -> DeviceId {
    let types = bl_devices::device_types();
    let t = types
        .iter()
        .find(|t| matches!(&t.kind, bl_model::DeviceKind::Native { device_type: d } if d == device_type))
        .unwrap();
    add(h, t)
}

fn set(h: &mut Harness, device: DeviceId, param: bl_model::ParamId, value: f64) {
    h.shell
        .invoke(
            "device.set_param",
            json!({ "device": device, "param": param, "value": value }),
        )
        .unwrap();
}

/// What was painted inside `cell` in a frame: line paths, circles and
/// rectangles.
fn painted(output: &egui::FullOutput, cell: Rect) -> (usize, usize, usize) {
    fn count(shape: &egui::Shape, cell: Rect, found: &mut (usize, usize, usize)) {
        // Lines and dots at the cell's edges reach a stroke beyond it.
        let cell = cell.expand(4.0);
        match shape {
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| count(s, cell, found)),
            _ if !cell.contains_rect(shape.visual_bounding_rect()) => {}
            egui::Shape::Path(_) => found.0 += 1,
            egui::Shape::Circle(_) => found.1 += 1,
            egui::Shape::Rect(_) => found.2 += 1,
            _ => {}
        }
    }
    let mut found = (0, 0, 0);
    for clipped in &output.shapes {
        count(&clipped.shape, cell, &mut found);
    }
    found
}

/// Where the device's first display cell is drawn in its frame at `frame`.
fn cell_in(h: &Harness, device: DeviceId, frame: Rect) -> Rect {
    let state = h.shell.device_view_state();
    let shown = state.devices.iter().find(|d| d.id == device).unwrap();
    let mut body = body_rect(frame, TITLE_BAR_HEIGHT);
    body.min.x += bl_shell::device_commands::sidechain_width(shown);
    place(shown.panel.as_ref().unwrap(), body).1[0].1
}

fn cell_rect(h: &mut Harness, device: DeviceId, _name: &str) -> Rect {
    let frame = frame_of(h, device);
    cell_in(h, device, frame)
}

// Spec: device-framework R36, eq R14
#[test]
fn the_contours_cell_draws_its_curve_dots_and_spectrum_and_says_them() {
    use bl_devices::eq::{ANALYSER_BANDS, FREQ, GAIN_SLOT, ON, Q, band_param};
    let mut h = display_harness();
    let contour = add_native(&mut h, bl_devices::eq::EQ);
    let cell = cell_rect(&mut h, contour, "EQ Curve");
    // Nothing is on and no spectrum: the cell is the framework's black frame.
    let output = h.frame(vec![]);
    let (flat, circles, _) = painted(&output, cell);
    assert_eq!((flat, circles), (1, 0), "the flat curve, no dots");

    for (param, value) in [
        (band_param(0, 3, ON), 1.0),
        (band_param(0, 3, FREQ), 1000.0),
        (band_param(0, 3, GAIN_SLOT), 12.0),
        (band_param(0, 3, Q), 1.0),
    ] {
        set(&mut h, contour, param, value);
    }
    let mut bands = vec![-80.0f32; ANALYSER_BANDS];
    bands[32] = -6.0;
    h.shell.set_device_display_data(contour, &bands);
    let output = h.frame(vec![]);
    let (paths, circles, rects) = painted(&output, cell);
    assert_eq!(paths, 2, "the sum and the band's curve");
    assert_eq!(circles, 1, "the band's dot");
    assert!(rects >= ANALYSER_BANDS, "the spectrum's bars: {rects}");
    let nodes = Harness::nodes(&output);
    let node = Harness::node(&nodes, "EQ Curve");
    assert_eq!(node.role, Role::Image);
    assert!(
        node.value.contains("A4 Peak 1.00 kHz 12.0 dB"),
        "{}",
        node.value
    );
    assert!(node.value.contains("Spectrum peak"), "{}", node.value);
}

// Spec: device-framework R36, compressor R16
#[test]
fn the_squeezes_cell_draws_its_gain_reduction_and_the_chosen_view() {
    let mut h = display_harness();
    let squeeze = add_native(&mut h, bl_devices::compressor::COMPRESSOR);
    let cell = cell_rect(&mut h, squeeze, "Gain Reduction");
    h.shell
        .set_device_display_data(squeeze, &[-6.0, -12.0, -15.0]);
    let output = h.frame(vec![]);
    let nodes = Harness::nodes(&output);
    let node = Harness::node(&nodes, "Gain Reduction");
    assert_eq!(node.role, Role::Image);
    assert!(
        node.value.contains("Gain Reduction -6.0 dB"),
        "{}",
        node.value
    );
    let transfer = painted(&output, cell);
    assert!(
        transfer.0 >= 1 && transfer.1 == 2,
        "the curve, the threshold's and the level's dots: {transfer:?}"
    );
    // The three views are buttons, the chosen one selected.
    assert_eq!(
        Harness::node(&nodes, "Transfer Curve view").selected,
        Some(true)
    );
    assert_eq!(Harness::node(&nodes, "Activity view").selected, Some(false));

    // Collapsed: only the meter.
    let button = bounds_of(&nodes, "Collapsed view");
    h.click(button.center());
    h.settle();
    let nodes = Harness::nodes(&h.frame(vec![]));
    assert_eq!(Harness::node(&nodes, "Collapsed view").selected, Some(true));
    let output = h.frame(vec![]);
    let collapsed = painted(&output, cell);
    assert_eq!(
        (collapsed.0, collapsed.1),
        (transfer.0 - 1, 0),
        "no curve, no dots"
    );
    assert_eq!(
        h.shell.device_view_state().devices[0]
            .display_view
            .as_deref(),
        Some("Collapsed")
    );

    // Activity: the history of the levels scrolls.
    let button = bounds_of(&nodes, "Activity view");
    h.click(button.center());
    for _ in 0..5 {
        h.frame(vec![]);
    }
    let activity = painted(&h.frame(vec![]), cell);
    assert!(
        activity.0 >= 1 && activity.1 == 0,
        "the input's line: {activity:?}"
    );
}

fn bounds_of(nodes: &[common::Node], label: &str) -> Rect {
    let b = Harness::node(nodes, label).bounds.unwrap();
    Rect::from_min_max(
        pos2(b.x0 as f32, b.y0 as f32),
        pos2(b.x1 as f32, b.y1 as f32),
    )
}

// Spec: device-framework R36, R11
#[test]
fn a_floating_window_grows_the_display_and_draws_it_there_too() {
    let mut h = display_harness();
    let contour = add_native(&mut h, bl_devices::eq::EQ);
    h.shell
        .run(
            "device.set_floating",
            json!({ "device": contour, "floating": true }),
        )
        .unwrap();
    h.shell
        .set_device_display_data(contour, &[-40.0; bl_devices::eq::ANALYSER_BANDS]);
    let (min_width, min_height) = {
        let state = h.shell.device_view_state();
        bl_shell::device_commands::window_min_size(&state.devices[0])
    };
    h.window = Window::Device(contour);
    h.size = vec2(min_width, min_height);
    let docked_cell = cell_in(&h, contour, Rect::from_min_size(Pos2::ZERO, h.size));
    h.size = vec2(min_width + 300.0, min_height + 200.0);
    let grown = h.settle();
    let nodes = Harness::nodes(&grown);
    let grown_cell = cell_in(&h, contour, Rect::from_min_size(Pos2::ZERO, h.size));
    assert!(
        grown_cell.width() > docked_cell.width() + 100.0,
        "the cell grows with the window"
    );
    assert!(
        Harness::node(&nodes, "EQ Curve")
            .value
            .contains("Spectrum peak")
    );
    let (paths, _, rects) = painted(&grown, grown_cell);
    assert!(
        paths >= 1 && rects >= bl_devices::eq::ANALYSER_BANDS,
        "drawn at the larger size: {paths} {rects}"
    );
}

// Spec: spectrum-analyser R8, device-framework R36
#[test]
fn the_spectrums_cell_draws_in_docked_and_floating_windows() {
    let mut h = display_harness();
    let spec = add_native(&mut h, bl_devices::spectrum_analyser::SPECTRUM);
    let mut bands = vec![-80.0f32; bl_devices::spectrum_analyser::DISPLAY_BINS];
    bands[128] = -3.0;
    h.shell.set_device_display_data(spec, &bands);

    let output = h.frame(vec![]);
    let nodes = Harness::nodes(&output);
    // The title bar's name and the cell are both labelled Spectrum.
    let node = nodes
        .iter()
        .find(|n| n.label == "Spectrum" && n.role == Role::Image)
        .expect("the Spectrum cell");
    assert!(
        node.value.contains("Spectrum peak"),
        "expected peak reading, got {}",
        node.value
    );

    // Floating window
    h.shell
        .run(
            "device.set_floating",
            json!({ "device": spec, "floating": true }),
        )
        .unwrap();
    let (min_width, min_height) = {
        let state = h.shell.device_view_state();
        bl_shell::device_commands::window_min_size(&state.devices[0])
    };
    h.window = Window::Device(spec);
    h.size = vec2(min_width + 200.0, min_height + 150.0);
    let grown = h.settle();
    let grown_cell = cell_in(&h, spec, Rect::from_min_size(Pos2::ZERO, h.size));
    let (_, _, rects) = painted(&grown, grown_cell);
    assert!(rects > 0, "spectrum bars painted in floating window");
}

/// The vertices of the meshes painted inside `cell` in a frame.
fn mesh_vertices(output: &egui::FullOutput, cell: Rect) -> Vec<usize> {
    fn count(shape: &egui::Shape, cell: Rect, found: &mut Vec<usize>) {
        match shape {
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| count(s, cell, found)),
            egui::Shape::Mesh(mesh)
                if cell.expand(1.0).contains_rect(shape.visual_bounding_rect()) =>
            {
                found.push(mesh.vertices.len());
            }
            _ => {}
        }
    }
    let mut found = Vec::new();
    for clipped in &output.shapes {
        count(&clipped.shape, cell, &mut found);
    }
    found
}

// Spec: device-framework R36, R46
#[test]
fn a_spectrograms_cell_draws_its_grid_as_one_image_whatever_the_size_of_the_cell() {
    let grid = testing::SPECTROGRAM_GRID;
    let mut h = display_harness();
    h.shell.set_device_types(testing::device_types());
    let device = add(&mut h, &testing::spectrogram_type());
    // Quiet everywhere, and a tone at -6 dB through the newest ten columns.
    let mut values = vec![grid.levels[0]; grid.columns * grid.rows];
    let lit = 10 * 3;
    for column in grid.columns - 10..grid.columns {
        values[column * grid.rows + 20..column * grid.rows + 23].fill(-6.0);
    }
    h.shell.set_device_display_data(device, &values);

    let output = h.frame(vec![]);
    let nodes = Harness::nodes(&output);
    let node = nodes
        .iter()
        .find(|n| n.label == "Spectrogram" && n.role == Role::Image)
        .expect("the Spectrogram cell");
    assert!(
        node.value.contains("Spectrogram peak"),
        "the cell says its loudest row: {}",
        node.value
    );
    let frame = frame_of(&mut h, device);
    let docked = cell_in(&h, device, frame);
    assert_eq!(
        mesh_vertices(&output, docked),
        [4 * lit],
        "one image, a quad for each cell above the floor"
    );

    // Floating, and larger: the same grid, drawn bigger.
    h.shell
        .run(
            "device.set_floating",
            json!({ "device": device, "floating": true }),
        )
        .unwrap();
    let (min_width, min_height) = {
        let state = h.shell.device_view_state();
        bl_shell::device_commands::window_min_size(&state.devices[0])
    };
    h.window = Window::Device(device);
    h.size = vec2(min_width + 200.0, min_height + 150.0);
    let grown = h.settle();
    let grown_cell = cell_in(&h, device, Rect::from_min_size(Pos2::ZERO, h.size));
    assert!(grown_cell.width() > docked.width());
    assert_eq!(mesh_vertices(&grown, grown_cell), [4 * lit]);
}

// Spec: spectral-time R16
#[test]
fn spectral_times_cell_draws_its_spectrogram_as_one_image() {
    use bl_devices::spectral_time::{GRID, SPECTRAL_TIME};
    let mut h = display_harness();
    let device = add_native(&mut h, SPECTRAL_TIME);
    let mut values = vec![GRID.levels[0]; GRID.columns * GRID.rows];
    for column in GRID.columns - 10..GRID.columns {
        values[column * GRID.rows + 20..column * GRID.rows + 23].fill(-6.0);
    }
    h.shell.set_device_display_data(device, &values);

    let output = h.frame(vec![]);
    let nodes = Harness::nodes(&output);
    let node = nodes
        .iter()
        .find(|n| n.label == "Spectrogram" && n.role == Role::Image)
        .expect("the Spectrogram cell");
    assert!(
        node.value.contains("Spectrogram peak"),
        "the cell says its loudest row: {}",
        node.value
    );
    let frame = frame_of(&mut h, device);
    let cell = cell_in(&h, device, frame);
    assert_eq!(
        mesh_vertices(&output, cell),
        [4 * 30],
        "one image, a quad for each cell above the floor"
    );
}

// Spec: tuner R7, device-framework R36
#[test]
fn the_tuners_cell_draws_in_docked_and_floating_windows() {
    let mut h = display_harness();
    let tun = add_native(&mut h, bl_devices::tuner::TUNER);
    h.shell
        .set_device_display_data(tun, &[440.0, 69.0, 0.0, 1.0, 1.0]);

    let output = h.frame(vec![]);
    let nodes = Harness::nodes(&output);
    // The title bar's name and the cell are both labelled Tuner.
    let node = nodes
        .iter()
        .find(|n| n.label == "Tuner" && n.role == Role::Image)
        .expect("the Tuner cell");
    assert!(
        node.value.contains("Note A4"),
        "expected note reading, got {}",
        node.value
    );

    // Floating window
    h.shell
        .run(
            "device.set_floating",
            json!({ "device": tun, "floating": true }),
        )
        .unwrap();
    let (min_width, min_height) = {
        let state = h.shell.device_view_state();
        bl_shell::device_commands::window_min_size(&state.devices[0])
    };
    h.window = Window::Device(tun);
    h.size = vec2(min_width + 200.0, min_height + 150.0);
    h.settle();
    let grown_cell = cell_in(&h, tun, Rect::from_min_size(Pos2::ZERO, h.size));
    assert!(grown_cell.width() > 100.0);
}

// Spec: oscilloscope R7, device-framework R36
#[test]
fn the_oscilloscopes_cell_draws_in_docked_and_floating_windows() {
    let mut h = display_harness();
    let osc = add_native(&mut h, bl_devices::oscilloscope::OSCILLOSCOPE);
    let wave = vec![0.5f32; bl_devices::oscilloscope::WAVEFORM_POINTS];
    h.shell.set_device_display_data(osc, &wave);

    let output = h.frame(vec![]);
    let nodes = Harness::nodes(&output);
    let node = Harness::node(&nodes, "Waveform");
    assert_eq!(node.role, Role::Image);

    // Floating window
    h.shell
        .run(
            "device.set_floating",
            json!({ "device": osc, "floating": true }),
        )
        .unwrap();
    let (min_width, min_height) = {
        let state = h.shell.device_view_state();
        bl_shell::device_commands::window_min_size(&state.devices[0])
    };
    h.window = Window::Device(osc);
    h.size = vec2(min_width + 200.0, min_height + 150.0);
    let grown = h.settle();
    let grown_cell = cell_in(&h, osc, Rect::from_min_size(Pos2::ZERO, h.size));
    let (paths, _, _) = painted(&grown, grown_cell);
    assert!(paths >= 1, "waveform painted in floating window");
}

// Spec: automation R29. A device parameter's context menu holds the
// automation items; Show Automation In New Lane gives the parameter a lane of
// its own and turns Automation Mode on, and a click on the control while the
// mode is on shows the parameter in the track's main lane.
#[test]
fn a_parameters_menu_has_the_automation_items_and_a_click_shows_its_envelope() {
    let mut h = harness();
    let utility = add(&mut h, &testing::utility_type());
    let track = h.shell.editor().document().device_track(utility).unwrap();
    let target = bl_model::AutomationTarget::Device {
        device: utility,
        param: UTILITY_GAIN,
    };
    let knob = control(&mut h, utility, UTILITY_GAIN).center();
    h.frame(vec![Event::PointerMoved(knob)]);
    h.frame(vec![Event::PointerButton {
        pos: knob,
        button: PointerButton::Secondary,
        pressed: true,
        modifiers: Modifiers::NONE,
    }]);
    h.frame(vec![Event::PointerButton {
        pos: knob,
        button: PointerButton::Secondary,
        pressed: false,
        modifiers: Modifiers::NONE,
    }]);
    let nodes = Harness::nodes(&h.settle());
    for label in [
        "Delete Automation",
        "Re-Enable Automation",
        "Show Automation",
        "Show Automation In New Lane",
    ] {
        assert!(nodes.iter().any(|n| n.label == label), "no {label:?}");
    }
    assert!(
        Harness::node(&nodes, "Delete Automation").disabled,
        "the parameter has no envelope"
    );
    let item = Harness::node(&nodes, "Show Automation In New Lane")
        .bounds
        .unwrap();
    h.click(pos2(
        ((item.x0 + item.x1) / 2.0) as f32,
        ((item.y0 + item.y1) / 2.0) as f32,
    ));
    h.settle();
    assert!(h.shell.arrangement().automation_mode());
    assert_eq!(
        h.shell
            .arrangement()
            .track_lanes(track)
            .lanes
            .iter()
            .map(|lane| lane.target)
            .collect::<Vec<_>>(),
        [Some(target)]
    );

    // Back on the main lane: a click on the control shows it there.
    h.shell
        .run(
            "arrangement.remove_automation_lane",
            json!({ "track": track, "lane": 1 }),
        )
        .unwrap();
    h.settle();
    assert_ne!(h.shell.arrangement().automation_lane(track), Some(target));
    let knob = control(&mut h, utility, UTILITY_GAIN).center();
    h.click(knob);
    h.settle();
    assert_eq!(h.shell.arrangement().automation_lane(track), Some(target));
}

// The mouse wheel on controls (WP-6.49; the view options spec, R10 to R13).

/// One notch of a mouse wheel, up (positive) or down.
fn notch(h: &mut Harness, notches: f32, modifiers: Modifiers) {
    h.frame(vec![Event::MouseWheel {
        unit: egui::MouseWheelUnit::Line,
        delta: vec2(0.0, notches),
        phase: egui::TouchPhase::Move,
        modifiers,
    }]);
}

/// Turns the switch on or off, puts the pointer over the Gain knob, and lets
/// a frame pass so the knob has seen it.
fn over_gain(h: &mut Harness, switch: bool) -> (DeviceId, Rect) {
    h.shell
        .run("settings.set_wheel_adjusts_values", json!({ "on": switch }))
        .unwrap();
    let utility = add(h, &testing::utility_type());
    h.shell
        .invoke(
            "device.set_param",
            json!({ "device": utility, "param": UTILITY_GAIN, "value": 0.5 }),
        )
        .unwrap();
    let knob = control(h, utility, UTILITY_GAIN);
    h.frame(vec![Event::PointerMoved(knob.center())]);
    h.frame(vec![]);
    (utility, knob)
}

fn gain_position(h: &Harness, device: DeviceId) -> f64 {
    let gain = testing::utility_type().description.params[0].clone();
    gain.normalize(value(h, device, UTILITY_GAIN).unwrap())
}

// Spec: view-options R11, R13.
#[test]
fn the_wheel_moves_a_knob_a_percent_a_notch_and_a_run_is_one_undo_step() {
    let mut h = harness();
    let (utility, _) = over_gain(&mut h, true);
    let start = gain_position(&h, utility);
    let steps = h.shell.editor().undo_history().len();
    for _ in 0..3 {
        notch(&mut h, 1.0, Modifiers::NONE);
    }
    h.frame(vec![]);
    let moved = gain_position(&h, utility) - start;
    assert!((moved - 0.03).abs() < 1e-6, "three notches are 3%: {moved}");
    // Half a second on the app's clock with no turn ends the run.
    for _ in 0..40 {
        h.frame(vec![]);
    }
    assert_eq!(h.shell.editor().undo_history().len(), steps + 1);
    assert_eq!(last_step(&h), "Change \"Gain\"");
    // Down is down, and a new run is a new step.
    notch(&mut h, -1.0, Modifiers::NONE);
    for _ in 0..40 {
        h.frame(vec![]);
    }
    assert!((gain_position(&h, utility) - start - 0.02).abs() < 1e-6);
    assert_eq!(h.shell.editor().undo_history().len(), steps + 2);
}

// Spec: view-options R11.
#[test]
fn shift_makes_the_wheel_step_finely() {
    let mut h = harness();
    let (utility, _) = over_gain(&mut h, true);
    let start = gain_position(&h, utility);
    notch(&mut h, 1.0, Modifiers::SHIFT);
    h.frame(vec![]);
    let moved = gain_position(&h, utility) - start;
    assert!((moved - 0.001).abs() < 1e-6, "a tenth of a step: {moved}");
}

// Spec: view-options R11.
#[test]
fn a_trackpad_moves_a_knob_a_step_for_every_twenty_points() {
    let mut h = harness();
    let (utility, _) = over_gain(&mut h, true);
    let start = gain_position(&h, utility);
    let scroll = |h: &mut Harness, points: f32| {
        h.frame(vec![Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: vec2(0.0, points),
            phase: egui::TouchPhase::Move,
            modifiers: Modifiers::NONE,
        }]);
    };
    // Fifteen points make no step; five more make one.
    scroll(&mut h, 15.0);
    h.frame(vec![]);
    assert_eq!(gain_position(&h, utility), start);
    scroll(&mut h, 5.0);
    h.frame(vec![]);
    assert!((gain_position(&h, utility) - start - 0.01).abs() < 1e-6);
}

// Spec: view-options R10.
#[test]
fn with_the_switch_off_the_wheel_leaves_the_knob_alone() {
    let mut h = harness();
    let (utility, _) = over_gain(&mut h, false);
    let start = gain_position(&h, utility);
    let steps = h.shell.editor().undo_history().len();
    notch(&mut h, 1.0, Modifiers::NONE);
    for _ in 0..40 {
        h.frame(vec![]);
    }
    assert_eq!(gain_position(&h, utility), start);
    assert_eq!(h.shell.editor().undo_history().len(), steps);
}

// Spec: view-options R13.
#[test]
fn a_run_ends_when_the_pointer_leaves_the_control_or_a_key_is_pressed() {
    let mut h = harness();
    let (_, knob) = over_gain(&mut h, true);
    let steps = h.shell.editor().undo_history().len();
    notch(&mut h, 1.0, Modifiers::NONE);
    h.frame(vec![]);
    h.frame(vec![Event::PointerMoved(knob.center() + vec2(0.0, 200.0))]);
    h.frame(vec![]);
    assert_eq!(
        h.shell.editor().undo_history().len(),
        steps + 1,
        "leaving the knob ended the run well inside half a second"
    );
    h.frame(vec![Event::PointerMoved(knob.center())]);
    h.frame(vec![]);
    notch(&mut h, 1.0, Modifiers::NONE);
    h.frame(vec![]);
    h.key(egui::Key::A, Modifiers::NONE);
    h.frame(vec![]);
    assert_eq!(h.shell.editor().undo_history().len(), steps + 2);
}

// Spec: view-options R12.
#[test]
fn the_status_bar_reads_the_knob_while_the_wheel_moves_it() {
    let mut h = harness();
    over_gain(&mut h, true);
    notch(&mut h, 1.0, Modifiers::NONE);
    let nodes = Harness::nodes(&h.frame(vec![]));
    assert!(
        nodes
            .iter()
            .any(|n| n.value.starts_with("Gain: ") || n.label.starts_with("Gain: ")),
        "the Status Bar names the control and its value"
    );
}

// Spec: view-options R12 (automation R5). A turn of the wheel on an
// automated control is a hand move: the envelope is turned off with it.
#[test]
fn the_wheel_on_an_automated_knob_overrides_its_envelope_as_a_hand_move_does() {
    let mut h = harness();
    let (utility, _) = over_gain(&mut h, true);
    let track = h.shell.editor().document().top_level().nth(2).unwrap();
    for (at, level) in [(0, 0.5), (bl_core::time::Ticks::WHOLE.get(), 0.25)] {
        h.shell
            .run(
                "automation.set_breakpoint",
                json!({ "track": track,
                        "target": { "type": "device", "device": utility, "param": UTILITY_GAIN },
                        "at": at, "value": level }),
            )
            .unwrap();
    }
    h.settle();
    assert!(
        !h.shell
            .editor()
            .document()
            .track(track)
            .unwrap()
            .automation_overridden()
    );
    notch(&mut h, 1.0, Modifiers::NONE);
    h.frame(vec![]);
    assert!(
        h.shell
            .editor()
            .document()
            .track(track)
            .unwrap()
            .automation_overridden(),
        "the envelope is off"
    );
}

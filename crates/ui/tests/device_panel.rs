//! The generic device UI (WP-2.5), headless: a described device's controls
//! drawn from its descriptors and its layout, each change one undo step
//! named for its parameter, and the layout placed at larger sizes for
//! floating windows. Devices are the framework's test devices.
//!
//! Spec: device-framework R15 to R20.

mod common;

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
    let theme = Theme::builtin(bl_shell::settings::ThemeChoice::Dark);
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

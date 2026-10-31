//! Automation in the UI (WP-2.12): the Control Bar's Re-Enable Automation
//! button, the Arrangement's Automation Mode switch and the A key, the
//! automation lane's device and parameter choosers, and the mouse edits that
//! draw, move, curve and delete breakpoints, each one undo step.
//!
//! The behaviour is in the automation spec.

use crate::common;
use bl_core::time::Ticks;
use bl_model::{AutomationTarget, Breakpoint, Document, MixerParam, TrackId};
use bl_shell::device_commands::DeviceAdded;
use bl_shell::settings::AutomationRecordMode;
use bl_ui::arrangement::automation::{AutomationDevice, control_choices, device_choices};
use bl_ui::layout::{Element, REFERENCE, ShellLayout};
use bl_ui::main_window::UiEvent;
use bl_ui::{control_bar, main_window};
use common::Harness;
use egui::accesskit::Action;
use egui::{Event, Key, Modifiers, PointerButton, Pos2, Rect, pos2};
use serde_json::json;

/// The accesskit ID an element's node has.
fn element_id(element: Element) -> egui::Id {
    main_window::element_id(element)
}

/// A right-click at `at`, which opens a context menu.
pub(crate) fn right_click(h: &mut Harness, at: Pos2) -> egui::FullOutput {
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
    h.settle()
}

/// The middle of the node labelled `label`.
pub(crate) fn centre_of(nodes: &[common::Node], label: &str) -> Pos2 {
    let node = nodes
        .iter()
        .rfind(|n| n.label == label && n.bounds.is_some())
        .unwrap_or_else(|| panic!("no node labelled {label:?}"));
    let b = node.bounds.unwrap();
    pos2(((b.x0 + b.x1) / 2.0) as f32, ((b.y0 + b.y1) / 2.0) as f32)
}

// Spec: automation R5
#[test]
fn the_re_enable_automation_button_lights_and_re_enables() {
    let mut h = Harness::new(common::Window::Main);
    let mut output = h.settle();
    assert!(
        !control_bar::automation_overridden(&h.shell),
        "a new set overrides nothing"
    );
    let nodes = Harness::nodes(&output);
    let node = Harness::node(&nodes, "Re-Enable Automation");
    assert!(!node.disabled, "it can be pressed");
    assert!(node.value.contains("every envelope"));

    // A control with an envelope, moved by hand: its automation is off and
    // the button lights.
    let track = h.shell.new_id();
    h.shell
        .run(
            "track.create",
            json!({ "id": track, "kind": "audio", "name": "Tone", "index": 0 }),
        )
        .unwrap();
    let volume = json!({ "type": "mixer", "param": "volume" });
    h.shell
        .run(
            "automation.set_breakpoint",
            json!({ "track": track, "target": volume, "at": 0, "value": 1.0 }),
        )
        .unwrap();
    h.shell
        .run("mixer.set_volume", json!({ "track": track, "volume": 0.5 }))
        .unwrap();
    assert!(control_bar::automation_overridden(&h.shell));
    output = h.settle();
    let nodes = Harness::nodes(&output);
    let node = Harness::node(&nodes, "Re-Enable Automation");
    assert!(node.value.contains("some automation is off"));

    // Pressing it, as a screen reader does, re-enables every envelope.
    h.accesskit(element_id(Element::ReEnableAutomation), Action::Click);
    h.settle();
    assert!(!control_bar::automation_overridden(&h.shell));
    assert!(
        h.shell
            .editor()
            .document()
            .track(track)
            .unwrap()
            .automation_for(&bl_model::AutomationTarget::Mixer {
                param: bl_model::MixerParam::Volume
            })
            .unwrap()
            .enabled,
        "the envelope drives the control again"
    );
}

// Spec: automation R17
#[test]
fn the_automation_arm_button_switches_the_arm() {
    let mut h = Harness::new(common::Window::Main);
    let output = h.settle();
    let nodes = Harness::nodes(&output);
    let node = Harness::node(&nodes, "Automation Arm");
    assert!(!node.disabled, "it can be pressed");
    assert_eq!(node.value, "off");
    assert!(!h.shell.arrangement().automation_arm());

    // Pressing it, as a screen reader does, arms it; pressing again disarms.
    h.accesskit(element_id(Element::AutomationArm), Action::Click);
    let output = h.settle();
    assert!(h.shell.arrangement().automation_arm());
    let nodes = Harness::nodes(&output);
    assert_eq!(Harness::node(&nodes, "Automation Arm").value, "on");
    h.accesskit(element_id(Element::AutomationArm), Action::Click);
    h.settle();
    assert!(!h.shell.arrangement().automation_arm());
}

// Spec: automation R18
#[test]
fn the_automation_arm_button_has_a_menu_of_the_recording_modes() {
    let mut h = Harness::new(common::Window::Main);
    h.settle();
    let at = ShellLayout::compute(REFERENCE, true, true)
        .rect(Element::AutomationArm)
        .center();

    // Right-clicking the Automation Arm button opens its context menu: the
    // three modes, with the current one ticked.
    let output = right_click(&mut h, at);
    let nodes = Harness::nodes(&output);
    for label in ["✔ Touch", "Latch", "Write"] {
        assert!(
            nodes.iter().any(|n| n.label == label),
            "no {label:?} in the menu"
        );
    }

    // Choosing Latch runs the command, and the menu ticks it next time.
    h.click(centre_of(&nodes, "Latch"));
    h.settle();
    assert_eq!(
        h.shell.automation_record_mode(),
        AutomationRecordMode::Latch
    );
    let output = right_click(&mut h, at);
    let nodes = Harness::nodes(&output);
    assert!(nodes.iter().any(|n| n.label == "✔ Latch"));
}

pub(crate) const BAR: i64 = Ticks::WHOLE.get();

/// A track with a pan envelope from −0.8 at 1.1.1 to 0.8 at bar 3, showing
/// in Automation Mode with snapping off, headless.
struct Set {
    h: Harness,
    track: TrackId,
    target: AutomationTarget,
}

fn pan() -> AutomationTarget {
    AutomationTarget::Mixer {
        param: MixerParam::Pan,
    }
}

fn pan_json() -> serde_json::Value {
    json!({ "type": "mixer", "param": "pan" })
}

fn set() -> Set {
    let mut h = Harness::with_title_bar(common::Window::Main);
    let track = h.shell.editor().document().top_level().next().unwrap();
    for (at, value) in [(0, -0.8), (BAR * 2, 0.8)] {
        h.shell
            .run(
                "automation.set_breakpoint",
                json!({ "track": track, "target": pan_json(), "at": at, "value": value }),
            )
            .unwrap();
    }
    // The A switch, the lane's parameter, and snapping off so clicks land
    // where they're aimed.
    h.shell
        .run("arrangement.snap_to_grid", json!({ "on": false }))
        .unwrap();
    h.shell
        .run("arrangement.automation_mode", json!({ "on": true }))
        .unwrap();
    h.shell
        .run(
            "arrangement.show_automation",
            json!({ "track": track, "target": pan_json() }),
        )
        .unwrap();
    h.settle();
    Set {
        h,
        track,
        target: pan(),
    }
}

impl Set {
    fn doc(&self) -> &Document {
        self.h.shell.editor().document()
    }

    fn points(&self) -> Vec<Breakpoint> {
        self.doc()
            .track(self.track)
            .unwrap()
            .automation_for(&self.target)
            .unwrap()
            .envelope
            .points()
            .collect()
    }

    fn value_at(&self, at: Ticks) -> f64 {
        self.doc()
            .track(self.track)
            .unwrap()
            .automation_for(&self.target)
            .unwrap()
            .envelope
            .value_at(at)
            .unwrap()
    }

    fn steps(&self) -> usize {
        self.h.shell.editor().undo_history().len()
    }

    fn lane(&self) -> Rect {
        let view = self.h.shell.arrangement();
        let doc = self.doc();
        self.h
            .main
            .arrangement()
            .automation_lane_rect(view, doc, self.track)
            .expect("the track shows an automation lane")
    }

    fn x(&self, at: Ticks) -> f32 {
        let view = self.h.shell.arrangement();
        let doc = self.doc();
        self.lane().left() + view.horizontal(doc).x(at) as f32
    }

    fn y(&self, value: f64) -> f32 {
        let rect = self.lane();
        rect.bottom() - (((value + 1.0) / 2.0) as f32) * rect.height()
    }

    /// The command IDs the last gestures invoked, in order.
    fn commands(&mut self) -> Vec<String> {
        self.h
            .take_events()
            .into_iter()
            .filter_map(|event| match event {
                UiEvent::Invoke(invocation) => Some(invocation.id),
                UiEvent::Gesture {
                    invocation: Some(invocation),
                    ..
                } => Some(invocation.id),
                _ => None,
            })
            .collect()
    }
}

/// A click with modifiers held.
pub(crate) fn click(h: &mut Harness, at: Pos2, modifiers: Modifiers) {
    let held = Event::ModifiersChanged(modifiers);
    h.frame(vec![held.clone(), Event::PointerMoved(at)]);
    for pressed in [true, false] {
        h.frame(vec![
            held.clone(),
            Event::PointerButton {
                pos: at,
                button: PointerButton::Primary,
                pressed,
                modifiers,
            },
        ]);
    }
    h.frame(vec![Event::ModifiersChanged(Modifiers::NONE)]);
    h.settle();
}

/// Two clicks in quick succession, as a double-click.
pub(crate) fn double_click(h: &mut Harness, at: Pos2, modifiers: Modifiers) {
    let held = Event::ModifiersChanged(modifiers);
    h.frame(vec![held.clone(), Event::PointerMoved(at)]);
    for _ in 0..2 {
        for pressed in [true, false] {
            h.frame(vec![
                held.clone(),
                Event::PointerButton {
                    pos: at,
                    button: PointerButton::Primary,
                    pressed,
                    modifiers,
                },
            ]);
        }
    }
    h.frame(vec![Event::ModifiersChanged(Modifiers::NONE)]);
    h.settle();
}

/// A press, a drag over eight frames, and a release, with modifiers held
/// throughout.
pub(crate) fn drag(h: &mut Harness, from: Pos2, to: Pos2, modifiers: Modifiers) {
    let held = Event::ModifiersChanged(modifiers);
    h.frame(vec![held.clone(), Event::PointerMoved(from)]);
    h.frame(vec![
        held.clone(),
        Event::PointerButton {
            pos: from,
            button: PointerButton::Primary,
            pressed: true,
            modifiers,
        },
    ]);
    for step in 1..=8 {
        let t = step as f32 / 8.0;
        h.frame(vec![
            held.clone(),
            Event::PointerMoved(from + (to - from) * t),
        ]);
    }
    h.frame(vec![
        Event::ModifiersChanged(modifiers),
        Event::PointerButton {
            pos: to,
            button: PointerButton::Primary,
            pressed: false,
            modifiers,
        },
    ]);
    h.frame(vec![Event::ModifiersChanged(Modifiers::NONE)]);
    h.settle();
}

// Spec: automation R13 (S-ENV-001)
#[test]
fn the_a_key_shows_the_automation_lanes() {
    let mut h = Harness::with_title_bar(common::Window::Main);
    h.settle();
    assert!(
        !h.shell.arrangement().automation_mode(),
        "a new set hides the lanes"
    );
    h.key(Key::A, Modifiers::NONE);
    h.settle();
    assert!(h.shell.arrangement().automation_mode());
    let nodes = Harness::nodes(&h.settle());
    let node = Harness::node(&nodes, "Automation Mode");
    assert_eq!(node.toggled, Some(true), "the switch reads as on");
}

// Spec: automation R13 (S2, the choosers)
#[test]
fn the_lane_choosers_list_automatable_parameters() {
    let mut s = set();

    // The device chooser offers the track mixer, the track's devices, and
    // None.
    let labels: Vec<String> = device_choices(s.doc(), s.track)
        .into_iter()
        .map(|choice| choice.label)
        .collect();
    assert!(
        labels.iter().any(|label| label == "Track Mixer"),
        "{labels:?}"
    );
    assert!(labels.iter().any(|label| label == "None"), "{labels:?}");

    // The parameter chooser lists the mixer's automatable controls.
    let labels: Vec<String> = control_choices(&s.h.shell, s.doc(), AutomationDevice::Mixer)
        .into_iter()
        .map(|choice| choice.label)
        .collect();
    assert!(labels.iter().any(|label| label == "Volume"), "{labels:?}");
    assert!(labels.iter().any(|label| label == "Pan"), "{labels:?}");
    assert!(
        labels.iter().any(|label| label == "Track Activator"),
        "{labels:?}"
    );

    // A device's automatable parameters are listed too.
    s.h.shell
        .invoke("arrangement.select_tracks", json!({ "tracks": [s.track] }))
        .unwrap();
    let device_type = bl_devices::device_types()
        .into_iter()
        .find(|device| !device.description.params.is_empty())
        .expect("a described device type");
    let added: DeviceAdded = serde_json::from_value(
        s.h.shell
            .run(
                "device.add",
                json!({
                    "device": {
                        "kind": device_type.kind,
                        "name": device_type.name,
                        "role": device_type.role,
                    },
                }),
            )
            .unwrap(),
    )
    .unwrap();
    assert!(
        device_choices(s.doc(), s.track)
            .iter()
            .any(|choice| choice.value == AutomationDevice::Device(added.device)),
        "the device chooser lists the device"
    );
    let controls = control_choices(&s.h.shell, s.doc(), AutomationDevice::Device(added.device));
    assert!(
        !controls.is_empty(),
        "the gain device's parameters are automatable"
    );
    assert!(
        controls.iter().all(|choice| matches!(
            choice.value,
            AutomationTarget::Device { device, .. } if device == added.device
        )),
        "they all name the device"
    );

    // The choosers show, drawn in the header at the lane's row.
    let output = s.h.settle();
    let nodes = Harness::nodes(&output);
    Harness::node(&nodes, "Automation Device");
    Harness::node(&nodes, "Automation Control");
}

// Spec: automation-editing R2. A lane paints only the breakpoints in view,
// however many it holds: a recorded touch can hold thousands, and a time
// selection over them used to cost a search per breakpoint on every frame.
#[test]
fn a_lane_paints_only_the_breakpoints_in_view() {
    const POINTS: i64 = 20_000;
    const BREAKPOINT: f32 = 5.0;
    // Far enough apart that a few hundred are in view at the closest zoom.
    const SPACING: i64 = 2400;
    let mut s = set();
    for index in 0..POINTS {
        let value = if index % 2 == 0 { -0.5 } else { 0.5 };
        s.h.shell
            .run(
                "automation.set_breakpoint",
                json!({ "track": s.track, "target": pan_json(), "at": index * SPACING, "value": value }),
            )
            .unwrap();
    }
    s.h.shell
        .run(
            "arrangement.select_time",
            json!({
                "start": 0,
                "end": POINTS * SPACING + 1,
                "lanes": [{ "type": "automation", "track": s.track, "target": pan_json() }],
            }),
        )
        .unwrap();
    // As close as the view goes.
    for _ in 0..4 {
        s.h.shell
            .run("arrangement.zoom", json!({ "factor": 64.0, "anchor": 0 }))
            .unwrap();
    }
    let output = s.h.settle();
    let squares = common::painted_rects(&output)
        .iter()
        .filter(|(rect, _)| rect.size() == egui::vec2(BREAKPOINT, BREAKPOINT))
        .count();
    assert!(squares > 0, "the breakpoints in view are painted");
    assert!(squares < 500, "{squares} breakpoints painted of {POINTS}");
}

// Spec: automation R13 (S3)
#[test]
fn clicking_a_breakpoint_deletes_it() {
    let mut s = set();
    assert_eq!(s.points().len(), 2);
    let at = pos2(s.x(Ticks::ZERO), s.y(-0.8));
    click(&mut s.h, at, Modifiers::NONE);
    let commands = s.commands();
    assert!(
        commands.contains(&"automation.delete_breakpoint".to_owned()),
        "{commands:?}"
    );
    assert_eq!(s.points().len(), 1);
}

// Spec: automation R13 (S3)
#[test]
fn clicking_a_segment_adds_a_breakpoint() {
    let mut s = set();
    // One bar along (the middle of the two-bar segment), the pan is 0.
    let at = pos2(s.x(Ticks(BAR)), s.y(0.0));
    click(&mut s.h, at, Modifiers::NONE);
    let commands = s.commands();
    assert!(
        commands.contains(&"automation.set_breakpoint".to_owned()),
        "{commands:?}"
    );
    let points = s.points();
    assert_eq!(points.len(), 3);
    assert!(
        points
            .iter()
            .any(|point| point.at == Ticks(BAR) && point.value.abs() < 0.02),
        "{points:?}"
    );
}

// Spec: automation R13 (S3)
#[test]
fn double_clicking_off_a_segment_adds_a_breakpoint_at_the_clicked_value() {
    let mut s = set();
    // Three quarters along, the line is at 0.4; the click lands at 0.7.
    let at = pos2(s.x(Ticks(BAR * 3 / 2)), s.y(0.7));
    double_click(&mut s.h, at, Modifiers::NONE);
    let commands = s.commands();
    assert!(
        commands.contains(&"automation.set_breakpoint".to_owned()),
        "{commands:?}"
    );
    let points = s.points();
    assert_eq!(points.len(), 3);
    assert!(
        points.iter().any(|point| (point.value - 0.7).abs() < 0.03),
        "{points:?}"
    );
}

// Spec: automation R13 (S3), automation-editing R3. The drag runs
// `automation.move_breakpoints`, which moves a breakpoint past a neighbour
// by removing it.
#[test]
fn dragging_a_breakpoint_moves_its_position_and_value_in_one_undo_step() {
    let mut s = set();
    let before = s.steps();
    let from = pos2(s.x(Ticks::ZERO), s.y(-0.8));
    let to = pos2(s.x(Ticks(BAR / 2)), s.y(0.5));
    drag(&mut s.h, from, to, Modifiers::NONE);
    let commands = s.commands();
    assert!(
        commands.contains(&"automation.move_breakpoints".to_owned()),
        "{commands:?}"
    );
    let points = s.points();
    assert!(
        points
            .iter()
            .any(|point| point.at == Ticks(BAR / 2) && (point.value - 0.5).abs() < 0.03),
        "{points:?}"
    );
    assert_eq!(s.steps(), before + 1, "the drag is one undo step");
}

// Spec: automation-editing R2. Dragging a selected breakpoint moves every
// selected breakpoint by the same time and the same share of the travel, as
// one undo step.
#[test]
fn dragging_selected_breakpoints_moves_them_all_in_one_undo_step() {
    let mut s = set();
    // A breakpoint in the middle, so the time selection covers two of them.
    s.h.shell
        .run(
            "automation.set_breakpoint",
            json!({ "track": s.track, "target": pan_json(), "at": BAR, "value": 0.0 }),
        )
        .unwrap();
    s.h.shell
        .run(
            "arrangement.select_time",
            json!({
                "start": 0,
                "end": BAR * 2,
                "lanes": [{ "type": "automation", "track": s.track, "target": pan_json() }],
            }),
        )
        .unwrap();
    s.h.settle();
    let before = s.steps();
    let from = pos2(s.x(Ticks::ZERO), s.y(-0.8));
    let to = pos2(s.x(Ticks::ZERO) + 20.0, s.y(-0.4));
    drag(&mut s.h, from, to, Modifiers::NONE);
    let commands = s.commands();
    assert!(
        commands.contains(&"automation.move_breakpoints".to_owned()),
        "{commands:?}"
    );
    let points = s.points();
    assert_eq!(points.len(), 3, "{points:?}");
    let kept = points
        .iter()
        .find(|point| point.at == Ticks(BAR * 2))
        .expect("the breakpoint outside the selection stays");
    assert!((kept.value - 0.8).abs() < 0.001, "{points:?}");
    let moved: Vec<&Breakpoint> = points
        .iter()
        .filter(|point| point.at != Ticks(BAR * 2))
        .collect();
    assert!(
        moved.iter().all(|point| point.value > -0.8),
        "both selected breakpoints went up the travel: {points:?}"
    );
    assert!(
        moved.iter().any(|point| point.at > Ticks(BAR)),
        "the second selected breakpoint moved in time too: {points:?}"
    );
    assert_eq!(s.steps(), before + 1, "the drag is one undo step");
}

// Spec: automation-editing R2, R3. Shift keeps a breakpoint drag to the axis
// it started along; a vertical drag changes the value and leaves the time.
#[test]
fn shift_keeps_a_breakpoint_drag_to_one_axis() {
    let mut s = set();
    let before = s.steps();
    let from = pos2(s.x(Ticks::ZERO), s.y(-0.8));
    let to = pos2(from.x + 2.0, s.y(-0.4));
    drag(&mut s.h, from, to, Modifiers::SHIFT);
    let points = s.points();
    let point = points
        .iter()
        .find(|point| point.at == Ticks::ZERO)
        .unwrap_or_else(|| panic!("the breakpoint stays in time: {points:?}"));
    assert!(
        point.value > -0.75,
        "the breakpoint moved up the travel: {points:?}"
    );
    assert_eq!(s.steps(), before + 1, "the drag is one undo step");
}

// Spec: automation-editing R4. Dragging a segment inside the time selection
// adds breakpoints at the selection's edges and moves the stretch between
// them, as one undo step.
#[test]
fn dragging_a_segment_inside_a_time_selection_adds_breakpoints_at_its_edges() {
    let mut s = set();
    s.h.shell
        .run(
            "arrangement.select_time",
            json!({
                "start": 0,
                "end": BAR,
                "lanes": [{ "type": "automation", "track": s.track, "target": pan_json() }],
            }),
        )
        .unwrap();
    s.h.settle();
    let before = s.steps();
    let at = pos2(s.x(Ticks(BAR / 2)), s.y(-0.4));
    let to = pos2(at.x + 20.0, at.y - 20.0);
    drag(&mut s.h, at, to, Modifiers::NONE);
    let commands = s.commands();
    assert!(
        commands.contains(&"automation.move_breakpoints".to_owned()),
        "{commands:?}"
    );
    let points = s.points();
    assert_eq!(
        points.len(),
        3,
        "the selection's edge gained a breakpoint: {points:?}"
    );
    assert!(
        points.iter().any(|point| point.at == Ticks(BAR * 2)),
        "the envelope outside the selection stays: {points:?}"
    );
    assert_eq!(s.steps(), before + 1, "the drag is one undo step");
}

// Spec: automation-editing R6. Option (Alt) with an edit key applies it to
// the automation lanes shown under the selection's tracks.
#[test]
fn option_with_an_edit_key_acts_on_the_shown_automation_lanes() {
    let mut s = set();
    s.h.shell
        .run(
            "arrangement.select_time",
            json!({ "start": 0, "end": BAR, "tracks": [s.track] }),
        )
        .unwrap();
    s.h.settle();
    s.h.key(
        Key::C,
        Modifiers {
            alt: true,
            command: true,
            ..Modifiers::NONE
        },
    );
    s.h.settle();
    let commands = s.commands();
    assert!(
        commands.contains(&"automation.copy".to_owned()),
        "{commands:?}"
    );
    assert!(
        s.h.shell.envelope_clipboard().is_some(),
        "the envelope was copied, not the clips"
    );
}

// Spec: automation-editing R4. Shift-clicking a segment selects its two
// breakpoints, and dragging the selected segment moves them, as one undo
// step.
#[test]
fn dragging_a_selected_segment_moves_it_in_one_undo_step() {
    let mut s = set();
    // The middle of the two-bar segment: the pan is 0 there.
    let at = pos2(s.x(Ticks(BAR)), s.y(0.0));
    click(&mut s.h, at, Modifiers::SHIFT);
    let commands = s.commands();
    assert!(
        commands.contains(&"automation.select_segment".to_owned()),
        "{commands:?}"
    );
    let before = s.steps();
    let to = pos2(at.x + 40.0, at.y - 20.0);
    drag(&mut s.h, at, to, Modifiers::NONE);
    let commands = s.commands();
    assert!(
        commands.contains(&"automation.move_breakpoints".to_owned()),
        "{commands:?}"
    );
    let points = s.points();
    assert_eq!(points.len(), 2, "{points:?}");
    assert!(
        points
            .iter()
            .all(|point| point.at != Ticks::ZERO && point.at != Ticks(BAR * 2)),
        "both of the segment's breakpoints moved: {points:?}"
    );
    assert_eq!(s.steps(), before + 1, "the drag is one undo step");
}

// Spec: automation-editing R2. Esc during a drag puts the envelope back and
// records nothing.
#[test]
fn esc_cancels_an_automation_drag() {
    let mut s = set();
    let before = s.doc().clone();
    let steps = s.steps();
    let from = pos2(s.x(Ticks::ZERO), s.y(-0.8));
    let to = pos2(s.x(Ticks(BAR / 2)), s.y(0.5));
    let held = Event::ModifiersChanged(Modifiers::NONE);
    s.h.frame(vec![held.clone(), Event::PointerMoved(from)]);
    s.h.frame(vec![
        held.clone(),
        Event::PointerButton {
            pos: from,
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        },
    ]);
    s.h.frame(vec![held.clone(), Event::PointerMoved(to)]);
    assert!(s.h.shell.gesture_open(), "the drag is under way");
    s.h.key(Key::Escape, Modifiers::NONE);
    s.h.frame(vec![
        held.clone(),
        Event::PointerButton {
            pos: to,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        },
    ]);
    s.h.frame(vec![Event::ModifiersChanged(Modifiers::NONE)]);
    s.h.settle();
    assert_eq!(*s.doc(), before, "the envelope is back");
    assert!(!s.h.shell.gesture_open());
    assert_eq!(s.steps(), steps, "nothing is recorded");
}

// Spec: accessibility R10, automation-editing R8. The Lock Envelopes
// switch above the track headers is live, and in the Tab order.
#[test]
fn the_lock_envelopes_switch_is_in_the_tab_order() {
    let mut h = Harness::new(common::Window::Main);
    let output = h.settle();
    let nodes = Harness::nodes(&output);
    let node = Harness::node(&nodes, "Lock Envelopes");
    assert!(!node.disabled, "it can be pressed");
    assert_eq!(node.toggled, Some(false), "it is off in a new set");
    // Pressing it, as a screen reader does, switches the setting on.
    h.accesskit(bl_ui::arrangement::lock_envelopes_id(), Action::Click);
    h.settle();
    assert!(h.shell.editor().document().settings().lock_envelopes);
}

// Spec: automation-editing R6. Option (Alt on Windows) with an edit
// command applies it to the automation lanes under the tracks of the time
// selection (S-EDIT-016).
#[test]
fn the_edit_keys_in_an_automation_lane_act_on_the_envelopes() {
    let mut s = set();
    // The time selection is in the track's automation lane.
    s.h.shell
        .run(
            "arrangement.select_time",
            json!({
                "start": 0,
                "end": BAR,
                "lanes": [{
                    "type": "automation",
                    "track": s.track,
                    "target": pan_json(),
                }],
            }),
        )
        .unwrap();
    s.h.settle();
    s.h.key(
        Key::C,
        Modifiers {
            alt: true,
            command: true,
            ..Modifiers::NONE
        },
    );
    s.h.settle();
    let commands = s.commands();
    assert!(
        commands.contains(&"automation.copy".to_owned()),
        "{commands:?}"
    );
}

// Spec: automation R13 (S-ENV-003)
#[test]
fn option_dragging_a_segment_curves_it_in_one_undo_step() {
    let mut s = set();
    let before = s.steps();
    let value = s.value_at(Ticks(BAR));
    let from = pos2(s.x(Ticks(BAR)), s.y(value));
    let to = pos2(s.x(Ticks(BAR)), s.y(value) - 20.0);
    drag(&mut s.h, from, to, Modifiers::ALT);
    let commands = s.commands();
    assert!(
        commands.contains(&"automation.set_curve".to_owned()),
        "{commands:?}"
    );
    let first = s
        .points()
        .into_iter()
        .find(|point| point.at == Ticks::ZERO)
        .unwrap();
    assert!(first.bend.abs() > 0.05, "the segment is curved: {first:?}");
    assert_eq!(s.steps(), before + 1, "the drag is one undo step");
}

// Spec: automation R13 (S-ENV-003)
#[test]
fn option_double_clicking_a_segment_straightens_it() {
    let mut s = set();
    s.h.shell
        .run(
            "automation.set_curve",
            json!({ "track": s.track, "target": pan_json(), "at": 0, "bend": 0.5 }),
        )
        .unwrap();
    s.h.settle();
    let value = s.value_at(Ticks(BAR));
    let at = pos2(s.x(Ticks(BAR)), s.y(value));
    double_click(&mut s.h, at, Modifiers::ALT);
    let commands = s.commands();
    assert!(
        commands.contains(&"automation.set_curve".to_owned()),
        "{commands:?}"
    );
    let first = s
        .points()
        .into_iter()
        .find(|point| point.at == Ticks::ZERO)
        .unwrap();
    assert_eq!(first.bend, 0.0, "the segment is straight again");
}

// Shapes, handles and Simplify Envelope (WP-6.25b).

/// Selects bar 1 to bar 2 in the track's pan lane.
fn select_span(s: &mut Set) {
    s.h.shell
        .run(
            "arrangement.select_time",
            json!({
                "start": BAR,
                "end": BAR * 2,
                "lanes": [{ "type": "automation", "track": s.track, "target": pan_json() }],
            }),
        )
        .unwrap();
    s.h.settle();
}

/// The Edit menu's Simplify Envelope (M-EDIT-039).
fn edit_menu_simplify(h: &Harness) -> bl_ui::menus::ItemState {
    let bar = bl_shell::menus::menu_bar(bl_shell::menus::MenuPlatform::current());
    let item = bar
        .iter()
        .flat_map(bl_shell::menus::Menu::walk)
        .find(|item| item.row == "M-EDIT-039")
        .expect("Simplify Envelope is in the Edit menu");
    bl_ui::menus::item_state(
        item,
        &h.shell,
        &h.keymap,
        bl_ui::keymap::Platform::current(),
    )
}

// Spec: automation-editing R11, R19. Each shape button in the time selection's
// menu inserts its shape, as one undo step.
#[test]
fn the_time_selections_menu_inserts_each_shape_in_one_undo_step() {
    for shape in bl_model::Shape::ALL {
        let mut s = set();
        select_span(&mut s);
        let before = s.steps();
        let at = pos2(s.x(Ticks(BAR + BAR / 2)), s.y(0.0));
        let output = right_click(&mut s.h, at);
        let nodes = Harness::nodes(&output);
        s.h.click(centre_of(&nodes, shape.label()));
        s.h.settle();
        let commands = s.commands();
        assert!(
            commands.contains(&"automation.insert_shape".to_owned()),
            "{shape:?}: {commands:?}"
        );
        assert_eq!(s.steps(), before + 1, "{shape:?} is one undo step");
    }
}

// Spec: automation-editing R11. Without a time selection in an automation
// lane the menu has no shapes.
#[test]
fn a_menu_outside_automation_lanes_has_no_shapes() {
    let mut s = set();
    s.h.shell
        .run(
            "arrangement.select_time",
            json!({ "start": 0, "end": BAR, "tracks": [s.track] }),
        )
        .unwrap();
    s.h.settle();
    let at = pos2(s.x(Ticks(BAR / 2)), s.lane().top() - 10.0);
    let output = right_click(&mut s.h, at);
    let nodes = Harness::nodes(&output);
    assert!(!nodes.iter().any(|n| n.label == "Sine"));
    assert!(!nodes.iter().any(|n| n.label == "Simplify Envelope"));
}

// Spec: automation-editing R18. Simplify Envelope runs from the time
// selection's menu and from the Edit menu, and the Edit menu's item is
// disabled without a time selection in an automation lane.
#[test]
fn simplify_envelope_runs_from_both_menus() {
    let mut s = set();
    // A straight run of breakpoints, which two replace.
    for i in 1..8 {
        s.h.shell
            .run(
                "automation.set_breakpoint",
                json!({
                    "track": s.track,
                    "target": pan_json(),
                    "at": i * BAR / 4,
                    "value": -0.8 + 1.6 * (i as f64 / 8.0),
                }),
            )
            .unwrap();
    }
    assert_eq!(s.points().len(), 9);
    assert!(!edit_menu_simplify(&s.h).enabled, "no time selection");
    s.h.shell
        .run(
            "arrangement.select_time",
            json!({
                "start": 0,
                "end": BAR * 2 + 1,
                "lanes": [{ "type": "automation", "track": s.track, "target": pan_json() }],
            }),
        )
        .unwrap();
    s.h.settle();
    // The menu in the lane.
    let before = s.steps();
    let at = pos2(s.x(Ticks(BAR)), s.y(0.0) - 30.0);
    let output = right_click(&mut s.h, at);
    let nodes = Harness::nodes(&output);
    s.h.click(centre_of(&nodes, "Simplify Envelope"));
    s.h.settle();
    assert!(s.commands().contains(&"automation.simplify".to_owned()));
    assert_eq!(s.points().len(), 2);
    assert_eq!(s.steps(), before + 1);
    // And the Edit menu's.
    s.h.shell.run("edit.undo", json!({})).unwrap();
    assert_eq!(s.points().len(), 9);
    let state = edit_menu_simplify(&s.h);
    assert!(state.enabled);
    let invocation = state.invocation.unwrap();
    s.h.shell.invoke(&invocation.id, invocation.args).unwrap();
    assert_eq!(s.points().len(), 2);
}

// Spec: automation-editing R14, R15. The top handle stretches the values,
// as one undo step.
#[test]
fn dragging_the_top_handle_stretches_the_values_in_one_undo_step() {
    let mut s = set();
    select_span(&mut s);
    // A breakpoint to stretch, half way up the lane.
    s.h.shell
        .run(
            "automation.set_breakpoint",
            json!({ "track": s.track, "target": pan_json(), "at": BAR + BAR / 4, "value": 0.0 }),
        )
        .unwrap();
    s.h.settle();
    let before = s.steps();
    let lane = s.lane();
    let middle = (s.x(Ticks(BAR)) + s.x(Ticks(BAR * 2))) / 2.0;
    let top = pos2(middle, lane.top());
    // Hover first, so the handles show, then drag the top to the middle.
    s.h.frame(vec![Event::PointerMoved(top)]);
    drag(
        &mut s.h,
        top,
        pos2(middle, lane.center().y),
        Modifiers::NONE,
    );
    let commands = s.commands();
    assert!(
        commands.contains(&"automation.stretch_values".to_owned()),
        "{commands:?}"
    );
    // 0 was half way up the travel, and the top edge is now half way up.
    let moved = s
        .points()
        .into_iter()
        .find(|point| point.at == Ticks(BAR + BAR / 4))
        .unwrap();
    assert!((moved.value - -0.5).abs() < 0.1, "{moved:?}");
    assert_eq!(s.steps(), before + 1, "the drag is one undo step");
}

// Spec: automation-editing R14, R16. The right handle stretches in time and
// the time selection follows.
#[test]
fn dragging_the_right_handle_stretches_in_time_and_the_selection_follows() {
    let mut s = set();
    select_span(&mut s);
    let before = s.steps();
    let lane = s.lane();
    let right = pos2(s.x(Ticks(BAR * 2)), lane.center().y);
    s.h.frame(vec![Event::PointerMoved(right)]);
    let to = pos2(s.x(Ticks(BAR * 3)), right.y);
    drag(&mut s.h, right, to, Modifiers::NONE);
    let commands = s.commands();
    assert!(
        commands.contains(&"automation.stretch_time".to_owned()),
        "{commands:?}"
    );
    let selection = s.h.shell.arrangement().time_selection().unwrap();
    assert_eq!(selection.0, Ticks(BAR));
    assert!(selection.1 > Ticks(BAR * 5 / 2), "{selection:?}");
    assert_eq!(s.steps(), before + 1, "the drag is one undo step");
}

// Spec: automation-editing R14, R17. A corner handle skews the values.
#[test]
fn dragging_a_corner_handle_skews_in_one_undo_step() {
    let mut s = set();
    select_span(&mut s);
    s.h.shell
        .run(
            "automation.set_breakpoint",
            json!({ "track": s.track, "target": pan_json(), "at": BAR + BAR / 2, "value": 0.5 }),
        )
        .unwrap();
    s.h.settle();
    let before = s.steps();
    let lane = s.lane();
    let corner = pos2(s.x(Ticks(BAR)), lane.top());
    s.h.frame(vec![Event::PointerMoved(corner)]);
    drag(
        &mut s.h,
        corner,
        pos2(corner.x, lane.center().y),
        Modifiers::NONE,
    );
    let commands = s.commands();
    assert!(
        commands.contains(&"automation.skew".to_owned()),
        "{commands:?}"
    );
    assert_eq!(s.steps(), before + 1, "the drag is one undo step");
    let moved = s
        .points()
        .into_iter()
        .find(|point| point.at == Ticks(BAR + BAR / 2))
        .unwrap();
    assert!(moved.value < 0.5, "{moved:?}");
}

// Spec: automation-editing R14. Esc during a handle drag puts the envelope
// and the selection back.
#[test]
fn esc_cancels_a_handle_drag() {
    let mut s = set();
    select_span(&mut s);
    let (doc, selection, steps) = (
        s.doc().clone(),
        s.h.shell.arrangement().time_selection(),
        s.steps(),
    );
    let lane = s.lane();
    let right = pos2(s.x(Ticks(BAR * 2)), lane.center().y);
    let to = pos2(s.x(Ticks(BAR * 3)), right.y);
    let held = Event::ModifiersChanged(Modifiers::NONE);
    s.h.frame(vec![held.clone(), Event::PointerMoved(right)]);
    s.h.frame(vec![
        held.clone(),
        Event::PointerButton {
            pos: right,
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        },
    ]);
    s.h.frame(vec![held.clone(), Event::PointerMoved(to)]);
    assert!(s.h.shell.gesture_open(), "the drag is under way");
    assert_ne!(s.h.shell.arrangement().time_selection(), selection);
    s.h.key(Key::Escape, Modifiers::NONE);
    s.h.frame(vec![
        held,
        Event::PointerButton {
            pos: to,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        },
    ]);
    s.h.settle();
    assert_eq!(*s.doc(), doc, "the envelope is back");
    assert_eq!(s.h.shell.arrangement().time_selection(), selection);
    assert!(!s.h.shell.gesture_open());
    assert_eq!(s.steps(), steps, "nothing is recorded");
}

// Spec: automation-editing R17. Option mirrors a handle's movement on the
// opposite one.
#[test]
fn option_mirrors_a_handle_drag_on_the_opposite_handle() {
    let mut s = set();
    select_span(&mut s);
    s.h.shell
        .run(
            "automation.set_breakpoint",
            json!({ "track": s.track, "target": pan_json(), "at": BAR + BAR / 2, "value": 0.0 }),
        )
        .unwrap();
    s.h.settle();
    let lane = s.lane();
    let middle = (s.x(Ticks(BAR)) + s.x(Ticks(BAR * 2))) / 2.0;
    let top = pos2(middle, lane.top());
    s.h.frame(vec![Event::PointerMoved(top)]);
    // Dragging the top handle a quarter of the way down pulls the bottom
    // one a quarter of the way up: a breakpoint at the middle stays there.
    drag(
        &mut s.h,
        top,
        pos2(middle, lane.top() + lane.height() / 4.0),
        Modifiers::ALT,
    );
    let moved = s
        .points()
        .into_iter()
        .find(|point| point.at == Ticks(BAR + BAR / 2))
        .unwrap();
    assert!(moved.value.abs() < 0.1, "{moved:?}");
    let ends: Vec<f64> = s
        .points()
        .iter()
        .filter(|point| point.at == Ticks::ZERO)
        .map(|point| point.value)
        .collect();
    assert_eq!(ends, [-0.8], "outside the span nothing moved");
}

// ---------------------------------------------------------------------------
// Several lanes per track (WP-6.57; the automation spec, R24 to R33).

use bl_model::{ClipId, LaneChoice};
use bl_shell::arrangement_commands::{AutomationLane, Lane};
use egui::{Id, vec2};

/// Alt on Windows and Linux, Cmd on macOS: what makes the lane and hide
/// buttons act on every lane (R25, R26) and bypasses snapping (R31).
fn all_key() -> Modifiers {
    if cfg!(target_os = "macos") {
        Modifiers::MAC_CMD
    } else {
        Modifiers::ALT
    }
}

fn volume() -> AutomationTarget {
    AutomationTarget::Mixer {
        param: MixerParam::Volume,
    }
}

fn mute() -> AutomationTarget {
    AutomationTarget::Mixer {
        param: MixerParam::Mute,
    }
}

impl Set {
    /// A MIDI clip on the track, from bar `from` for `bars` bars.
    fn put_clip(&mut self, from: i64, bars: i64) -> ClipId {
        let clip: ClipId = self.h.shell.new_id();
        self.h
            .shell
            .run(
                "clip.create",
                json!({
                    "track": self.track,
                    "clip": {
                        "id": clip,
                        "name": "",
                        "start": from * BAR,
                        "length": bars * BAR,
                        "offset": 0,
                        "mute": false,
                        "data": { "midi": { "notes": [] } },
                    },
                }),
            )
            .unwrap();
        self.h.settle();
        clip
    }

    /// An envelope of two breakpoints on `target`, from `a` at 1.1.1 to `b`
    /// at bar 5.
    fn envelope(&mut self, target: AutomationTarget, a: f64, b: f64) {
        for (at, value) in [(0, a), (BAR * 4, b)] {
            self.h
                .shell
                .run(
                    "automation.set_breakpoint",
                    json!({ "track": self.track, "target": target, "at": at, "value": value }),
                )
                .unwrap();
        }
        self.h.settle();
    }

    /// The track's lanes as `arrangement.state` reports them.
    fn lanes(&mut self) -> AutomationLane {
        let state: bl_shell::arrangement_commands::ArrangementState =
            serde_json::from_value(self.h.shell.run("arrangement.state", json!({})).unwrap())
                .unwrap();
        state
            .automation_lanes
            .into_iter()
            .find(|lane| lane.track == self.track)
            .expect("the track's lanes are reported")
    }

    /// The parameters the lanes of its own show, top to bottom.
    fn own_targets(&mut self) -> Vec<Option<AutomationTarget>> {
        self.lanes().lanes.iter().map(|lane| lane.target).collect()
    }

    fn row(&self) -> bl_shell::arrangement::rows::Row {
        self.h.shell.arrangement().rows(self.doc())[0]
    }

    /// Lane `number`'s rectangle on screen.
    fn lane_of(&self, number: usize) -> Rect {
        self.h
            .main
            .arrangement()
            .automation_lane_rect_of(self.h.shell.arrangement(), self.doc(), self.track, number)
            .expect("the lane shows")
    }

    /// The middle of the first node labelled `label`, top to bottom.
    fn button(&mut self, label: &str) -> Pos2 {
        let nodes = Harness::nodes(&self.h.settle());
        first_centre(&nodes, label)
    }
}

/// The middle of the topmost node labelled `label`.
fn first_centre(nodes: &[common::Node], label: &str) -> Pos2 {
    nodes_labelled(nodes, label)
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("no node labelled {label:?}"))
}

/// The middles of the nodes labelled `label`, top to bottom.
fn nodes_labelled(nodes: &[common::Node], label: &str) -> Vec<Pos2> {
    let mut found: Vec<Pos2> = nodes
        .iter()
        .filter(|n| n.label == label)
        .filter_map(|n| n.bounds)
        .map(|b| pos2(((b.x0 + b.x1) / 2.0) as f32, ((b.y0 + b.y1) / 2.0) as f32))
        .collect();
    found.sort_by(|a, b| a.y.total_cmp(&b.y));
    found
}

// Spec: automation R24. The track's own lane is its main automation lane: it
// draws the envelope over the clips, and no lane is added under the track.
// The envelope is edited in the clips' bodies, and a clip's title bar keeps
// the clip's gestures.
#[test]
fn the_main_lane_draws_over_the_clips_and_the_title_bar_keeps_the_clips_gestures() {
    let mut s = set();
    let clip = s.put_clip(0, 4);
    s.envelope(pan(), -0.8, 0.8);
    s.h.shell
        .run(
            "automation.set_breakpoint",
            json!({ "track": s.track, "target": pan_json(), "at": BAR, "value": 0.0 }),
        )
        .unwrap();
    s.h.settle();
    let row = s.row();
    assert_eq!(row.automation, 0.0, "no lane under the track");
    let lane = s.lane();
    assert_eq!(lane.height(), row.height, "the lane is the track's own");

    // A breakpoint in a clip's body drags, and the clip stays where it is.
    s.commands();
    let from = pos2(s.x(Ticks(BAR)), s.y(0.0));
    drag(&mut s.h, from, from + vec2(0.0, -12.0), Modifiers::NONE);
    let commands = s.commands();
    assert!(
        commands.contains(&"automation.move_breakpoints".to_owned()),
        "{commands:?}"
    );
    assert!(!commands.iter().any(|c| c.starts_with("arrangement.")));
    assert_eq!(s.doc().clip(clip).unwrap().start, Ticks::ZERO);
    let moved = s.value_at(Ticks(BAR));
    assert!(moved > 0.05, "the breakpoint went up: {moved}");

    // The title bar moves the clip, and with Lock Envelopes on the envelope
    // stays.
    s.h.shell
        .run("song.set_lock_envelopes", json!({ "enabled": true }))
        .unwrap();
    let before = s.points();
    let grab = pos2(s.x(Ticks(BAR * 3)), lane.top() + 5.0);
    drag(&mut s.h, grab, grab + vec2(60.0, 0.0), Modifiers::NONE);
    let commands = s.commands();
    assert!(
        commands.contains(&"arrangement.move_clips".to_owned()),
        "{commands:?}"
    );
    assert_ne!(s.doc().clip(clip).unwrap().start, Ticks::ZERO);
    assert_eq!(s.points(), before, "the envelope didn't move");
}

// Spec: automation R24, R15. A double-click in a clip's body adds a
// breakpoint rather than opening the clip, and a click there selects the
// lane.
#[test]
fn a_double_click_in_a_clips_body_adds_a_breakpoint_and_a_click_selects_the_lane() {
    let mut s = set();
    s.put_clip(0, 6);
    let at = pos2(s.x(Ticks(BAR * 3)), s.y(0.3));
    double_click(&mut s.h, at, Modifiers::NONE);
    let commands = s.commands();
    assert!(
        commands.contains(&"automation.set_breakpoint".to_owned()),
        "{commands:?}"
    );
    assert!(!commands.contains(&"arrangement.select_clips".to_owned()));
    assert_eq!(s.points().len(), 3);

    let empty = pos2(s.x(Ticks(BAR * 5)), s.y(-0.5));
    click(&mut s.h, empty, Modifiers::NONE);
    assert_eq!(
        s.h.shell.arrangement().keyboard_lane(),
        Some((s.track, pan())),
        "the main lane has the keyboard"
    );
}

// Spec: automation R25. The lane button moves the envelope the choosers show
// into a lane of its own; the main lane then shows None, there being no other
// automated parameter.
#[test]
fn the_lane_button_moves_the_envelope_into_a_lane_of_its_own() {
    let mut s = set();
    let buttons = nodes_labelled(
        &Harness::nodes(&s.h.settle()),
        "Show Automation in New Lane",
    )
    .len();
    let at = s.button("Show Automation in New Lane");
    click(&mut s.h, at, Modifiers::NONE);
    let commands = s.commands();
    assert!(
        commands.contains(&"arrangement.add_automation_lane".to_owned()),
        "{commands:?}"
    );
    let lanes = s.lanes();
    assert_eq!(lanes.target, None, "the main lane shows None");
    assert_eq!(lanes.lanes, [LaneChoice::showing(Some(pan()))]);
    assert_eq!(s.row().automation, 43.0, "a lane of its own is 43 points");
    assert_eq!(s.lane_of(1).height(), 43.0);
    // The envelope drives the parameter as before, and it draws in the new
    // lane.
    assert_eq!(s.points().len(), 2);
    // The button hides while the choosers show None: the other tracks keep
    // theirs.
    let after = Harness::nodes(&s.h.settle());
    assert_eq!(
        nodes_labelled(&after, "Show Automation in New Lane").len() + 1,
        buttons
    );
}

// Spec: automation R25. With Alt (Windows) or Cmd (Mac) held, the button gives
// every automated parameter of the track a lane of its own, in the choosers'
// order.
#[test]
fn the_lane_button_with_the_key_gives_every_automated_parameter_a_lane() {
    let mut s = set();
    s.envelope(volume(), 1.0, 0.5);
    s.envelope(mute(), 0.0, 1.0);
    let at = s.button("Show Automation in New Lane");
    click(&mut s.h, at, all_key());
    assert_eq!(
        s.own_targets(),
        [Some(volume()), Some(pan()), Some(mute())],
        "the choosers' order: Volume, Pan, Track Activator"
    );
    assert_eq!(s.lanes().target, None);
    assert_eq!(s.row().automation, 43.0 * 3.0);
}

// Spec: automation R26. A lane's hide button removes the lane from view, its
// envelope still driving its parameter; with the key held, that lane and the
// lanes after it.
#[test]
fn the_hide_button_removes_a_lane_and_with_the_key_the_lanes_after_it() {
    let mut s = set();
    s.envelope(volume(), 1.0, 0.5);
    s.envelope(mute(), 0.0, 1.0);
    s.h.shell
        .run(
            "arrangement.add_automation_lane",
            json!({ "track": s.track, "all": true }),
        )
        .unwrap();
    assert_eq!(s.own_targets().len(), 3);
    let nodes = Harness::nodes(&s.h.settle());
    let hide = nodes_labelled(&nodes, "Hide Automation Lane");
    assert_eq!(hide.len(), 3, "one hide button for each lane");

    // The second lane's button removes only that lane.
    click(&mut s.h, hide[1], Modifiers::NONE);
    assert_eq!(s.own_targets(), [Some(volume()), Some(mute())]);
    assert!(
        s.doc()
            .track(s.track)
            .unwrap()
            .automation_for(&pan())
            .is_some(),
        "its envelope still drives pan"
    );

    // With the key held on the first, the lanes after it go too.
    let nodes = Harness::nodes(&s.h.settle());
    let hide = nodes_labelled(&nodes, "Hide Automation Lane");
    click(&mut s.h, hide[0], all_key());
    assert!(s.own_targets().is_empty());
    assert_eq!(s.row().automation, 0.0);
}

// Spec: automation R26. Once a track has lanes of its own, a toggle in its
// header shows or hides them all; Left and Right on its title bar do the same.
#[test]
fn the_header_toggle_and_the_title_bar_keys_show_and_hide_the_lanes() {
    let mut s = set();
    let nodes = Harness::nodes(&s.h.settle());
    assert!(
        nodes_labelled(&nodes, "Hide Automation Lanes").is_empty(),
        "no toggle while the track has no lanes of its own"
    );
    s.envelope(volume(), 1.0, 0.5);
    s.h.shell
        .run(
            "arrangement.add_automation_lane",
            json!({ "track": s.track, "target": volume() }),
        )
        .unwrap();
    assert_eq!(s.row().automation, 43.0);

    let at = s.button("Hide Automation Lanes");
    click(&mut s.h, at, Modifiers::NONE);
    assert!(s.lanes().lanes_hidden);
    assert_eq!(s.row().automation, 0.0);
    assert_eq!(s.lanes().lanes.len(), 1, "the lane is kept, folded away");
    let at = s.button("Show Automation Lanes");
    click(&mut s.h, at, Modifiers::NONE);
    assert!(!s.lanes().lanes_hidden);
    assert_eq!(s.row().automation, 43.0);

    // With the title bar selected, Left hides them and Right shows them.
    s.h.shell
        .run(
            "arrangement.select_tracks",
            json!({ "tracks": [s.track], "title_bars": true }),
        )
        .unwrap();
    s.h.settle();
    s.h.key(Key::ArrowLeft, Modifiers::NONE);
    s.h.settle();
    assert!(s.lanes().lanes_hidden);
    assert!(
        !s.h.shell.arrangement().is_folded(s.track),
        "it didn't fold"
    );
    s.h.key(Key::ArrowRight, Modifiers::NONE);
    s.h.settle();
    assert!(!s.lanes().lanes_hidden);
}

// Spec: automation R27. The device chooser's last entry, and the lane
// header's menu, switch the lane's choosers to list only automated
// parameters.
#[test]
fn show_automated_parameters_only_filters_the_choosers() {
    let mut s = set();
    let nodes = Harness::nodes(&s.h.settle());
    // The control chooser lists the mixer's controls.
    let control = Id::new((
        "chooser",
        format!("arrangement-automation-control-{}", s.track.0),
    ));
    s.h.accesskit(control, Action::Click);
    for _ in 0..12 {
        s.h.frame(vec![]);
    }
    let open = Harness::nodes(&s.h.frame(vec![]));
    assert!(
        open.iter().any(|n| n.label == "Volume"),
        "all controls list"
    );
    s.h.key(Key::Escape, Modifiers::NONE);
    s.h.settle();

    // The lane header's menu has the filter first, ticked once on.
    let at = first_centre(&nodes, "Automation Device");
    let output = right_click(&mut s.h, at);
    let menu = Harness::nodes(&output);
    click(
        &mut s.h,
        first_centre(&menu, "Show Automated Parameters Only"),
        Modifiers::NONE,
    );
    assert!(s.lanes().only_automated);

    s.h.accesskit(control, Action::Click);
    for _ in 0..12 {
        s.h.frame(vec![]);
    }
    let open = Harness::nodes(&s.h.frame(vec![]));
    assert!(open.iter().any(|n| n.label == "Pan"), "pan has an envelope");
    assert!(
        !open.iter().any(|n| n.label == "Volume"),
        "volume has none, so it isn't listed"
    );
}

// Spec: automation R28. Right-clicking a lane's header opens its menu; Clear
// All Envelopes deletes every envelope of the track as one undo step.
#[test]
fn the_lane_header_menu_clears_envelopes_in_one_undo_step() {
    let mut s = set();
    s.envelope(volume(), 1.0, 0.5);
    let at = s.button("Automation Device");
    let output = right_click(&mut s.h, at);
    let menu = Harness::nodes(&output);
    for label in [
        "Show Automated Parameters Only",
        "Show All Lanes",
        "Hide All Lanes",
        "Clear All Envelopes",
        "Clear Envelopes of Track Mixer",
    ] {
        assert!(menu.iter().any(|n| n.label == label), "no {label:?}");
    }
    let steps = s.steps();
    click(
        &mut s.h,
        first_centre(&menu, "Clear All Envelopes"),
        Modifiers::NONE,
    );
    let commands = s.commands();
    assert!(
        commands.contains(&"automation.clear_track".to_owned()),
        "{commands:?}"
    );
    assert!(
        s.doc()
            .track(s.track)
            .unwrap()
            .automation()
            .next()
            .is_none()
    );
    assert_eq!(s.steps(), steps + 1);
    // One undo brings them all back.
    s.h.shell.run("edit.undo", json!({})).unwrap();
    assert!(s.doc().track(s.track).unwrap().automation().count() >= 2);
}

// Spec: automation R28. Clear Envelopes of the device the lane shows clears
// that device's, here the mixer's, and leaves a device's.
#[test]
fn clear_envelopes_clears_the_device_the_lane_shows() {
    let mut s = set();
    s.envelope(volume(), 1.0, 0.5);
    let at = s.button("Automation Device");
    let output = right_click(&mut s.h, at);
    let menu = Harness::nodes(&output);
    click(
        &mut s.h,
        first_centre(&menu, "Clear Envelopes of Track Mixer"),
        Modifiers::NONE,
    );
    let commands = s.commands();
    assert!(
        commands.contains(&"automation.clear_device".to_owned()),
        "{commands:?}"
    );
    assert!(
        s.doc()
            .track(s.track)
            .unwrap()
            .automation()
            .next()
            .is_none()
    );
}

// Spec: automation R29. A mixer control's context menu holds the automation
// items after its own; Show Automation In New Lane gives the parameter a
// lane, Delete Automation deletes its envelope.
#[test]
fn a_mixer_controls_menu_has_the_automation_items() {
    let mut s = set();
    let pan_control = s.button("Track Pan");
    let output = right_click(&mut s.h, pan_control);
    let menu = Harness::nodes(&output);
    for label in [
        "Stereo Pan Mode",
        "Split Stereo Pan Mode",
        "Delete Automation",
        "Re-Enable Automation",
        "Show Automation",
        "Show Automation In New Lane",
    ] {
        assert!(menu.iter().any(|n| n.label == label), "no {label:?}");
    }
    assert!(
        !Harness::node(&menu, "Delete Automation").disabled,
        "pan has an envelope"
    );
    assert!(
        Harness::node(&menu, "Re-Enable Automation").disabled,
        "it isn't overridden"
    );
    click(
        &mut s.h,
        first_centre(&menu, "Show Automation In New Lane"),
        Modifiers::NONE,
    );
    assert_eq!(s.own_targets(), [Some(pan())]);
    assert!(s.h.shell.arrangement().automation_mode());

    // Delete Automation runs `automation.delete`.
    let pan_control = s.button("Track Pan");
    let menu = Harness::nodes(&right_click(&mut s.h, pan_control));
    click(
        &mut s.h,
        first_centre(&menu, "Delete Automation"),
        Modifiers::NONE,
    );
    let commands = s.commands();
    assert!(
        commands.contains(&"automation.delete".to_owned()),
        "{commands:?}"
    );
    assert!(
        s.doc()
            .track(s.track)
            .unwrap()
            .automation_for(&pan())
            .is_none()
    );
}

// Spec: automation R29. Show Automation turns Automation Mode on and shows
// the parameter in the main lane.
#[test]
fn show_automation_turns_automation_mode_on_and_shows_the_parameter() {
    let mut s = set();
    s.h.shell
        .run("arrangement.automation_mode", json!({ "on": false }))
        .unwrap();
    s.h.settle();
    let pan_control = s.button("Track Pan");
    let menu = Harness::nodes(&right_click(&mut s.h, pan_control));
    click(
        &mut s.h,
        first_centre(&menu, "Show Automation"),
        Modifiers::NONE,
    );
    assert!(s.h.shell.arrangement().automation_mode());
    assert_eq!(
        s.h.shell.arrangement().automation_lane(s.track),
        Some(pan())
    );
}

// Spec: automation R29. A click on a control while Automation Mode is on
// shows its envelope in the main lane.
#[test]
fn clicking_a_control_shows_its_envelope_while_automation_mode_is_on() {
    let mut s = set();
    s.h.shell
        .run(
            "arrangement.show_automation",
            json!({ "track": s.track, "target": volume() }),
        )
        .unwrap();
    s.h.settle();
    assert_eq!(
        s.h.shell.arrangement().automation_lane(s.track),
        Some(volume())
    );
    let pan_control = s.button("Track Pan");
    click(&mut s.h, pan_control, Modifiers::NONE);
    assert_eq!(
        s.h.shell.arrangement().automation_lane(s.track),
        Some(pan())
    );
    // With the mode off a click shows nothing.
    s.h.shell
        .run("arrangement.automation_mode", json!({ "on": false }))
        .unwrap();
    s.h.shell
        .run(
            "arrangement.show_automation",
            json!({ "track": s.track, "target": volume() }),
        )
        .unwrap();
    s.h.settle();
    let pan_control = s.button("Track Pan");
    click(&mut s.h, pan_control, Modifiers::NONE);
    assert_eq!(
        s.h.shell.arrangement().automation_lane(s.track),
        Some(volume())
    );
}

// Spec: automation R30, automation-editing R20. A time selection dragged
// across several lanes covers them all, Delete acts on every envelope it
// covers as one undo step, and Down moves through the lanes in the order
// they're drawn.
#[test]
fn a_selection_across_lanes_edits_every_envelope_and_down_moves_through_them() {
    let mut s = set();
    s.envelope(pan(), -0.8, 0.8);
    s.envelope(volume(), 1.0, 0.5);
    s.h.shell
        .run(
            "arrangement.add_automation_lane",
            json!({ "track": s.track, "target": volume() }),
        )
        .unwrap();
    s.h.settle();
    // From the main lane down into the lane of its own.
    let from = pos2(s.x(Ticks(BAR)), s.lane_of(0).top() + 30.0);
    let to = pos2(s.x(Ticks(BAR * 2)), s.lane_of(1).center().y);
    drag(&mut s.h, from, to, Modifiers::NONE);
    let state: bl_shell::arrangement_commands::ArrangementState =
        serde_json::from_value(s.h.shell.run("arrangement.state", json!({})).unwrap()).unwrap();
    let covered: Vec<Option<AutomationTarget>> = state
        .time_selection_lanes
        .iter()
        .filter(|lane| lane.track() == s.track)
        .map(Lane::target)
        .collect();
    assert_eq!(covered, [Some(pan()), Some(volume())]);

    let steps = s.steps();
    s.h.key(Key::Delete, Modifiers::ALT);
    s.h.settle();
    let commands = s.commands();
    assert!(
        commands.contains(&"automation.delete_range".to_owned()),
        "{commands:?}"
    );
    assert_eq!(s.steps(), steps + 1, "one undo step for both envelopes");
    for target in [pan(), volume()] {
        let points: Vec<Ticks> = s
            .doc()
            .track(s.track)
            .unwrap()
            .automation_for(&target)
            .unwrap()
            .envelope
            .points()
            .map(|p| p.at)
            .collect();
        assert!(points.contains(&Ticks(BAR)), "{target:?}: {points:?}");
        assert!(points.contains(&Ticks(BAR * 2)), "{target:?}: {points:?}");
    }

    // Down: the track, its main lane, then its lane of its own.
    s.h.shell
        .run("arrangement.select_tracks", json!({ "tracks": [s.track] }))
        .unwrap();
    s.h.settle();
    let mut seen = Vec::new();
    for _ in 0..2 {
        s.h.key(Key::ArrowDown, Modifiers::NONE);
        s.h.settle();
        seen.push(s.h.shell.arrangement().keyboard_lane_number());
    }
    assert_eq!(seen, [Some((s.track, 0)), Some((s.track, 1))]);
}

// Spec: automation R31 (M-OPT-015). With Snap Automation to Grid on, a
// breakpoint dragged with the mouse lands on the grid; off, where the pointer
// is, and Cmd (Alt on Windows) makes that drag snap.
#[test]
fn a_breakpoint_drag_snaps_to_the_grid_only_with_snap_automation_to_grid_on() {
    let mut s = set();
    s.h.shell
        .run("arrangement.snap_to_grid", json!({ "on": true }))
        .unwrap();
    s.h.shell
        .run(
            "automation.set_breakpoint",
            json!({ "track": s.track, "target": pan_json(), "at": BAR, "value": 0.0 }),
        )
        .unwrap();
    s.h.settle();
    let step = s.h.shell.arrangement().grid_ticks(s.doc());
    let tpp = s.h.shell.arrangement().horizontal(s.doc()).ticks_per_point;
    // A drag that ends a bit past a grid line.
    let by = (step.get() as f64 * 2.37 / tpp) as f32;
    let landed = |s: &mut Set, modifiers: Modifiers| -> Ticks {
        let from = pos2(s.x(Ticks(BAR)), s.y(0.0));
        drag(&mut s.h, from, from + vec2(by, 0.0), modifiers);
        let at = s
            .points()
            .into_iter()
            .map(|p| p.at)
            .find(|at| *at != Ticks::ZERO && *at != Ticks(BAR * 2))
            .expect("the dragged breakpoint");
        // Put it back for the next drag.
        s.h.shell.run("edit.undo", json!({})).unwrap();
        s.h.settle();
        at
    };
    let on_grid = |at: Ticks| at.get() % step.get() == 0;

    // On, as a new install has it: on the grid; the key does the opposite.
    assert!(s.h.shell.snap_automation_to_grid());
    assert!(on_grid(landed(&mut s, Modifiers::NONE)));
    assert!(!on_grid(landed(&mut s, all_key())), "the key bypasses it");

    // Off: where the pointer is; the key snaps that drag.
    s.h.shell
        .run(
            "settings.set_snap_automation_to_grid",
            json!({ "on": false }),
        )
        .unwrap();
    assert!(!on_grid(landed(&mut s, Modifiers::NONE)));
    assert!(on_grid(landed(&mut s, all_key())), "the key snaps it");

    // The editing grid's own Snap to Grid still turns all snapping off.
    s.h.shell
        .run("arrangement.snap_to_grid", json!({ "on": false }))
        .unwrap();
    assert!(!on_grid(landed(&mut s, all_key())));
}

/// Whether a breakpoint made with a double-click, aimed a little past a grid
/// line, lands on the grid, with Snap Automation to Grid `snap` and the
/// editing grid's snapping on.
fn double_click_lands_on_the_grid(snap: bool) -> bool {
    let mut s = set();
    s.h.shell
        .run("arrangement.snap_to_grid", json!({ "on": true }))
        .unwrap();
    s.h.shell
        .run(
            "settings.set_snap_automation_to_grid",
            json!({ "on": snap }),
        )
        .unwrap();
    let step = s.h.shell.arrangement().grid_ticks(s.doc());
    let aim = Ticks(BAR * 3) + Ticks((step.get() as f64 * 0.37) as i64);
    let at = pos2(s.x(aim), s.y(0.3));
    double_click(&mut s.h, at, Modifiers::NONE);
    let made = s
        .points()
        .into_iter()
        .map(|p| p.at)
        .find(|at| *at > Ticks(BAR * 2))
        .expect("a breakpoint after the envelope");
    made.get() % step.get() == 0
}

// Spec: automation R31. A breakpoint made with a double-click lands on the
// grid while Snap Automation to Grid is on, and where the pointer is while
// it's off.
#[test]
fn a_double_click_makes_a_breakpoint_on_the_grid_with_snap_automation_to_grid_on() {
    assert!(double_click_lands_on_the_grid(true));
}

#[test]
fn a_double_click_makes_a_breakpoint_where_the_pointer_is_with_it_off() {
    assert!(!double_click_lands_on_the_grid(false));
}

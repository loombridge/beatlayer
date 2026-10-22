//! Automation in the UI (WP-2.12): the Control Bar's Re-Enable Automation
//! button, the Arrangement's Automation Mode switch and the A key, the
//! automation lane's device and parameter choosers, and the mouse edits that
//! draw, move, curve and delete breakpoints, each one undo step.
//!
//! The behaviour is in the automation spec.

mod common;

use bl_core::time::Ticks;
use bl_model::{AutomationTarget, Breakpoint, Document, MixerParam, TrackId};
use bl_shell::device_commands::DeviceAdded;
use bl_ui::arrangement::automation::{AutomationDevice, control_choices, device_choices};
use bl_ui::layout::Element;
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

const BAR: i64 = Ticks::WHOLE.get();

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
fn click(h: &mut Harness, at: Pos2, modifiers: Modifiers) {
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
fn double_click(h: &mut Harness, at: Pos2, modifiers: Modifiers) {
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
fn drag(h: &mut Harness, from: Pos2, to: Pos2, modifiers: Modifiers) {
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

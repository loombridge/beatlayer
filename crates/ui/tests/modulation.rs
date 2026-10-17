//! Relative modulation in the UI (WP-5.9), headless: a modulated control's
//! ring segment and fader mark, the choosers' LEDs, what the Arrangement's
//! automation lane draws, and the Clip View's modulation envelope on its own
//! scale. Spec: modulation R7 to R9.

mod common;

use bl_core::time::Ticks;
use bl_devices::testing::{self, DRIVE_DRIVE, UTILITY_BALANCE, UTILITY_GAIN};
use bl_engine::{PlayedParam, target_node};
use bl_model::{AutomationTarget, ClipData, ClipId, DeviceId, MixerParam, TrackId};
use bl_shell::device_commands::{DeviceAdded, TITLE_BAR_HEIGHT};
use bl_shell::mixer_controls::{self as controls, db_gain};
use bl_shell::mixer_view::MixerOf;
use bl_ui::arrangement::automation::{AutomationDevice, control_choices, device_choices};
use bl_ui::clip_envelope::{axis_start, lane_rect};
use bl_ui::clip_view::x_of_tick;
use bl_ui::device_panel::{body_rect, place};
use bl_ui::device_view::layout;
use bl_ui::mixer::{Place, fader_id, pan_id, send_id};
use bl_ui::widgets::ring_segment;
use common::{Harness, Node, Window};
use egui::accesskit::Action;
use egui::epaint::ColorMode;
use egui::{
    Color32, Event, FullOutput, Id, Key, Modifiers, PointerButton, Pos2, Rect, Shape, pos2, vec2,
};
use serde_json::json;

fn volume() -> AutomationTarget {
    AutomationTarget::Mixer {
        param: MixerParam::Volume,
    }
}

fn pan() -> AutomationTarget {
    AutomationTarget::Mixer {
        param: MixerParam::Pan,
    }
}

fn json_of(target: AutomationTarget) -> serde_json::Value {
    serde_json::to_value(target).unwrap()
}

/// Stops the engine, as the mixer's meter test does, so that what a test sets
/// as modulated stays.
fn engine_off(h: &mut Harness) {
    h.shell.run("audio.toggle_engine", json!({})).unwrap();
    h.frame(vec![]);
}

/// Sets the parameters as the engine would publish them while they're
/// modulated.
fn modulate(h: &mut Harness, params: &[(TrackId, AutomationTarget, f64, f64)]) {
    h.shell.set_modulated(
        params
            .iter()
            .map(|&(track, target, base, played)| {
                let (node, param) = target_node(track, &target);
                PlayedParam {
                    node,
                    param,
                    base,
                    played,
                }
            })
            .collect(),
    );
}

/// Every shape of a frame, the nested ones flattened.
fn shapes(output: &FullOutput) -> Vec<Shape> {
    fn walk(shape: &Shape, out: &mut Vec<Shape>) {
        match shape {
            Shape::Vec(shapes) => shapes.iter().for_each(|s| walk(s, out)),
            other => out.push(other.clone()),
        }
    }
    let mut out = Vec::new();
    for clipped in &output.shapes {
        walk(&clipped.shape, &mut out);
    }
    out
}

/// The points of every line (a path) drawn in `colour` that starts in `rect`.
fn paths(output: &FullOutput, colour: Color32, rect: Rect) -> Vec<Vec<Pos2>> {
    shapes(output)
        .into_iter()
        .filter_map(|shape| match shape {
            Shape::Path(path)
                if path.stroke.color == ColorMode::Solid(colour)
                    && path.points.first().is_some_and(|p| rect.contains(*p)) =>
            {
                Some(path.points)
            }
            _ => None,
        })
        .collect()
}

/// The rectangles filled with `colour` that lie in `rect`.
fn fills(output: &FullOutput, colour: Color32, rect: Rect) -> Vec<Rect> {
    shapes(output)
        .into_iter()
        .filter_map(|shape| match shape {
            Shape::Rect(r) if r.fill == colour && rect.contains_rect(r.rect) => Some(r.rect),
            _ => None,
        })
        .collect()
}

/// The ends of the straight lines drawn in `colour` that start in `rect`.
fn lines(output: &FullOutput, colour: Color32, rect: Rect) -> Vec<[Pos2; 2]> {
    shapes(output)
        .into_iter()
        .filter_map(|shape| match shape {
            Shape::LineSegment { points, stroke }
                if stroke.color == colour && rect.contains(points[0]) =>
            {
                Some(points)
            }
            _ => None,
        })
        .collect()
}

/// The text drawn inside `rect`.
fn texts(output: &FullOutput, rect: Rect) -> Vec<String> {
    shapes(output)
        .into_iter()
        .filter_map(|shape| match shape {
            Shape::Text(text) if rect.contains(text.pos + text.galley.rect.center().to_vec2()) => {
                Some(text.galley.text().to_owned())
            }
            _ => None,
        })
        .collect()
}

fn rect_of(nodes: &[Node], id: Id) -> Rect {
    rect_of_node(
        nodes
            .iter()
            .find(|n| n.id == id.accesskit_id())
            .expect("a node with that ID"),
    )
}

fn rect_of_node(node: &Node) -> Rect {
    let b = node.bounds.expect("a node with bounds");
    Rect::from_min_max(
        pos2(b.x0 as f32, b.y0 as f32),
        pos2(b.x1 as f32, b.y1 as f32),
    )
}

/// The Session View, where each strip has its knobs and its fader.
fn session() -> (Harness, Vec<TrackId>, TrackId) {
    let mut h = Harness::with_title_bar(Window::Main);
    engine_off(&mut h);
    h.settle();
    h.shell
        .run("view.toggle_session_arrangement", json!({}))
        .unwrap();
    h.settle();
    let tracks: Vec<TrackId> = h.shell.editor().document().top_level().collect();
    let ret = h.shell.editor().document().returns().next().unwrap();
    (h, tracks, ret)
}

fn strip() -> Place {
    Place::Mixer(MixerOf::Session)
}

/// A ring's geometry from what a knob drew: its centre and radius, taken
/// from the pointer's line, and the pointer's tip.
fn pointer(output: &FullOutput, theme: &bl_ui::theme::Theme, knob: Rect) -> (Pos2, Pos2) {
    let found = lines(output, theme.colors.text.0, knob.expand(2.0));
    assert_eq!(found.len(), 1, "one pointer in {knob:?}: {found:?}");
    (found[0][0], found[0][1])
}

/// Checks a knob at `knob`, which `modulated` modulates: the pointer stays
/// where it was, and a segment of the ring in the modulation colour runs
/// from it to `played`'s place, given by `at` from the ring's centre and
/// radius.
fn check_knob(
    h: &mut Harness,
    knob: Rect,
    modulated: impl FnOnce(&mut Harness),
    played: impl Fn(Pos2, f32) -> Pos2,
) {
    let colour = h.theme.colors.modulation.0;
    let output = h.frame(vec![]);
    assert!(
        paths(&output, colour, knob.expand(4.0)).is_empty(),
        "a knob that plays its base has no segment"
    );
    let (centre, tip) = pointer(&output, &h.theme, knob);
    modulated(h);
    let output = h.frame(vec![]);
    let (centre_now, tip_now) = pointer(&output, &h.theme, knob);
    assert!(
        centre.distance(centre_now) < 0.01 && tip.distance(tip_now) < 0.01,
        "the pointer stays at the base: {tip:?} then {tip_now:?}"
    );
    let found = paths(&output, colour, knob.expand(4.0));
    assert_eq!(found.len(), 1, "one segment: {found:?}");
    let (first, last) = (found[0][0], *found[0].last().unwrap());
    assert!(first.distance(tip) < 0.01, "it starts at the base");
    let want = played(centre, tip.distance(centre));
    assert!(
        last.distance(want) < 0.01 && last.distance(tip) > 2.0,
        "it ends where the control plays: {last:?}, not {want:?}"
    );
}

// Spec: modulation R7
#[test]
fn a_modulated_knob_shows_a_ring_segment_and_keeps_its_pointer() {
    // The pure rule: no segment while it plays its base.
    assert_eq!(ring_segment(0.5, 0.5), None);
    assert_eq!(ring_segment(0.5, 0.25), Some((0.5, 0.25)));

    // A device's knob.
    let mut h = Harness::with_title_bar(Window::Main);
    engine_off(&mut h);
    h.shell.set_device_types(testing::device_types());
    h.settle();
    let track = h.shell.editor().document().top_level().nth(2).unwrap();
    h.shell
        .invoke("arrangement.select_tracks", json!({ "tracks": [track] }))
        .unwrap();
    let t = testing::utility_type();
    let device = json!({ "kind": t.kind, "name": t.name, "role": t.role });
    let added: DeviceAdded = serde_json::from_value(
        h.shell
            .run("device.add", json!({ "device": device }))
            .unwrap(),
    )
    .unwrap();
    let nodes = Harness::nodes(&h.settle());
    let view = rect_of_node(Harness::node(&nodes, "Device View"));
    let frame = layout(&h.shell, view)
        .into_iter()
        .find(|(id, _)| *id == added.device)
        .unwrap()
        .1;
    let panel = h
        .shell
        .device_view_state()
        .devices
        .iter()
        .find(|d| d.id == added.device)
        .unwrap()
        .panel
        .clone()
        .unwrap();
    let knob = place(&panel, body_rect(frame, TITLE_BAR_HEIGHT))
        .0
        .into_iter()
        .find(|(p, _, _)| *p == UTILITY_GAIN)
        .unwrap()
        .2;
    let gain = t.description.params[0].clone();
    let target = AutomationTarget::Device {
        device: added.device,
        param: UTILITY_GAIN,
    };
    let (base, played) = (db_gain(-6.0), db_gain(-18.0));
    let n_played = gain.normalize(played) as f32;
    h.shell
        .run(
            "device.set_param",
            json!({ "device": added.device, "param": UTILITY_GAIN, "value": base }),
        )
        .unwrap();
    check_knob(
        &mut h,
        knob,
        |h| modulate(h, &[(track, target, base, played)]),
        |centre, radius| {
            // The device knob's arc: 300 degrees from the lower left.
            let from = std::f32::consts::PI * 0.5 + std::f32::consts::PI / 6.0;
            let angle = from + std::f32::consts::PI * 5.0 / 3.0 * n_played;
            centre + vec2(angle.cos(), angle.sin()) * radius
        },
    );
    // Assistive technology reads the base with the played value.
    let nodes = Harness::nodes(&h.frame(vec![]));
    assert_eq!(
        Harness::node(&nodes, "Gain").value,
        "-6.0 dB, modulated to -18.0 dB"
    );

    // A mixer's send and pan knobs.
    let (mut h, tracks, ret) = session();
    let t = tracks[2];
    h.shell
        .run(
            "mixer.set_send",
            json!({ "track": t, "to": ret, "level": 0.5 }),
        )
        .unwrap();
    h.shell
        .run("mixer.set_pan", json!({ "track": t, "pan": -0.25 }))
        .unwrap();
    let nodes = Harness::nodes(&h.settle());
    let send = rect_of(&nodes, send_id(strip(), t, ret));
    let pan_knob = rect_of(&nodes, pan_id(strip(), t));
    // A send and a pan knob turn 135 degrees either side of straight up.
    let ring = |centre: Pos2, radius: f32, position: f32| {
        let angle = (position * 2.0 - 1.0) * std::f32::consts::FRAC_PI_4 * 3.0;
        centre + vec2(angle.sin(), -angle.cos()) * radius
    };
    let send_target = AutomationTarget::Send { to: ret };
    let (send_played, pan_played) = (0.1, 0.75);
    let n = controls::send_position(send_played) as f32;
    check_knob(
        &mut h,
        send,
        |h| modulate(h, &[(t, send_target, 0.5, send_played)]),
        |centre, radius| ring(centre, radius, n),
    );
    check_knob(
        &mut h,
        pan_knob,
        |h| modulate(h, &[(t, pan(), -0.25, pan_played)]),
        |centre, radius| ring(centre, radius, ((pan_played + 1.0) / 2.0) as f32),
    );
}

// Spec: modulation R7
#[test]
fn a_modulated_fader_shows_its_mark() {
    let (mut h, tracks, _) = session();
    let t = tracks[2];
    let (base, played) = (db_gain(-6.0), db_gain(-12.0));
    h.shell
        .run("mixer.set_volume", json!({ "track": t, "volume": base }))
        .unwrap();
    let nodes = Harness::nodes(&h.settle());
    let fader = rect_of(&nodes, fader_id(strip(), t));
    let c = &h.theme.colors;
    let (mark, thumb) = (c.modulation.0, c.text_dim.0);
    let output = h.frame(vec![]);
    assert!(fills(&output, mark, fader.expand(4.0)).is_empty());
    let thumbs = fills(&output, thumb, fader.expand(4.0));
    assert_eq!(thumbs.len(), 1, "{thumbs:?}");
    modulate(&mut h, &[(t, volume(), base, played)]);
    let output = h.frame(vec![]);
    let y_of = |gain: f64| fader.bottom() - fader.height() * controls::fader_position(gain) as f32;
    let marks = fills(&output, mark, fader.expand(4.0));
    assert_eq!(marks.len(), 1, "one mark: {marks:?}");
    assert!(
        (marks[0].center().y - y_of(played)).abs() < 0.01,
        "{marks:?}"
    );
    assert!(marks[0].height() < 5.0, "it's a small mark");
    // The thumb stays at the base.
    let after = fills(&output, thumb, fader.expand(4.0));
    assert_eq!(after, thumbs, "the thumb didn't move");
    assert!((after[0].center().y - y_of(base)).abs() < 0.01);
}

/// A MIDI track with a Session clip, two devices, envelopes and modulation:
/// Pan and the utility's Balance have automation driving them, the send's and
/// the drive's were moved by hand (an overridden envelope); Volume, the send
/// and the utility's Gain are modulated by the clip.
struct Set {
    h: Harness,
    track: TrackId,
    clip: ClipId,
    ret: TrackId,
    utility: DeviceId,
    drive: DeviceId,
}

fn set() -> Set {
    let mut h = Harness::with_title_bar(Window::Main);
    h.shell.set_device_types(testing::device_types());
    h.settle();
    let track = h.shell.editor().document().top_level().next().unwrap();
    let ret = h.shell.editor().document().returns().next().unwrap();
    let scene = h.shell.editor().document().scenes().next().unwrap().id;
    h.shell
        .run(
            "session.insert_empty_midi_clip",
            json!({ "track": track, "scene": scene }),
        )
        .unwrap();
    h.shell
        .run("session.select", json!({ "track": track, "scene": scene }))
        .unwrap();
    let clip = h.shell.editor().document().slot(track, scene).unwrap().id;
    h.shell
        .invoke("arrangement.select_tracks", json!({ "tracks": [track] }))
        .unwrap();
    let mut add = |t: bl_engine::DeviceType| -> DeviceId {
        let device = json!({ "kind": t.kind, "name": t.name, "role": t.role });
        let added: DeviceAdded = serde_json::from_value(
            h.shell
                .run("device.add", json!({ "device": device }))
                .unwrap(),
        )
        .unwrap();
        added.device
    };
    let utility = add(testing::utility_type());
    let drive = add(testing::drive_type());
    let mut set = Set {
        h,
        track,
        clip,
        ret,
        utility,
        drive,
    };
    for target in [pan(), set.send(), set.balance(), set.drive_param()] {
        set.run(
            "automation.set_breakpoint",
            json!({ "track": track, "target": json_of(target), "at": 0, "value": 0.25 }),
        );
    }
    set.run(
        "mixer.set_send",
        json!({ "track": track, "to": ret, "level": 0.5 }),
    );
    set.run(
        "device.set_param",
        json!({ "device": drive, "param": DRIVE_DRIVE, "value": 0.5 }),
    );
    for target in [volume(), set.send(), set.gain()] {
        set.run(
            "clip.set_envelope_breakpoint",
            json!({ "clip": clip, "target": json_of(target), "at": 0, "value": 0.5 }),
        );
    }
    set.h.settle();
    set
}

impl Set {
    fn run(&mut self, id: &str, args: serde_json::Value) {
        self.h
            .shell
            .run(id, args)
            .unwrap_or_else(|e| panic!("{id} failed: {e}"));
    }

    fn doc(&self) -> &bl_model::Document {
        self.h.shell.editor().document()
    }

    fn send(&self) -> AutomationTarget {
        AutomationTarget::Send { to: self.ret }
    }

    fn gain(&self) -> AutomationTarget {
        AutomationTarget::Device {
            device: self.utility,
            param: UTILITY_GAIN,
        }
    }

    fn balance(&self) -> AutomationTarget {
        AutomationTarget::Device {
            device: self.utility,
            param: UTILITY_BALANCE,
        }
    }

    fn drive_param(&self) -> AutomationTarget {
        AutomationTarget::Device {
            device: self.drive,
            param: DRIVE_DRIVE,
        }
    }

    /// Opens the chooser `salt` and reads the LEDs on the row of each of
    /// `labels` from the frame that shows its list, left to right, then
    /// closes the list.
    fn leds(&mut self, salt: &str, labels: &[String]) -> Vec<Vec<Color32>> {
        let id = Id::new(("chooser", salt));
        let nodes = Harness::nodes(&self.h.settle());
        let chooser = rect_of(&nodes, id);
        self.h.accesskit(id, Action::Click);
        // The list fades in.
        for _ in 0..12 {
            self.h.frame(vec![]);
        }
        let found = shapes(&self.h.frame(vec![]));
        let circles: Vec<(Pos2, Color32)> = found
            .iter()
            .filter_map(|shape| match shape {
                Shape::Circle(c) => Some((c.center, c.fill)),
                _ => None,
            })
            .collect();
        let rows = labels
            .iter()
            .map(|label| {
                let mut at: Vec<(f32, Color32)> = Vec::new();
                for shape in &found {
                    let Shape::Text(text) = shape else { continue };
                    let rect = text.galley.rect.translate(text.pos.to_vec2());
                    if text.galley.text() != label
                        || rect.min.x < chooser.left() - 1.0
                        || rect.max.x > chooser.right() + 1.0
                    {
                        continue;
                    }
                    // The LEDs on the text's row, inside the chooser's width
                    // (the chosen one is in the box too).
                    at.extend(circles.iter().filter_map(|&(centre, colour)| {
                        let row = (rect.top() - 2.0)..=(rect.bottom() + 2.0);
                        (row.contains(&centre.y)
                            && (chooser.left()..=chooser.right()).contains(&centre.x))
                        .then_some((centre.x, colour))
                    }));
                }
                at.sort_by(|a, b| a.0.total_cmp(&b.0));
                let mut colours: Vec<Color32> = Vec::new();
                for (_, colour) in at {
                    if !colours.contains(&colour) {
                        colours.push(colour);
                    }
                }
                colours
            })
            .collect();
        self.h.key(Key::Escape, Modifiers::NONE);
        self.h.settle();
        rows
    }

    /// Checks the LEDs of the lane's two choosers and the Clip View's two,
    /// with `target` shown and `device` chosen.
    fn check_choosers(&mut self, target: AutomationTarget, device: AutomationDevice) {
        let track = self.track;
        self.run(
            "arrangement.show_automation",
            json!({ "track": track, "target": json_of(target) }),
        );
        self.run("view.clip_envelope", json!({ "target": json_of(target) }));
        let c = self.h.theme.colors.clone();
        let (auto, modulation, grey) = (c.automation.0, c.modulation.0, c.inactive.0);
        let (utility, drive) = (self.utility, self.drive);
        let want_device = |value: AutomationDevice| match value {
            // Pan drives, the send's envelope is overridden, and Volume and
            // the send are modulated: both LEDs, in the automation colour
            // because one envelope is enabled.
            AutomationDevice::Mixer => vec![auto, modulation],
            AutomationDevice::Device(d) if d == utility => vec![auto, modulation],
            // Only an overridden envelope: grey.
            AutomationDevice::Device(d) if d == drive => vec![grey],
            _ => vec![],
        };
        let (balance, gain, drive_param, send) =
            (self.balance(), self.gain(), self.drive_param(), self.send());
        let want_control = |value: AutomationTarget| match value {
            v if v == volume() || v == gain => vec![modulation],
            v if v == pan() || v == balance => vec![auto],
            v if v == send => vec![grey, modulation],
            v if v == drive_param => vec![grey],
            _ => vec![],
        };
        let lane = format!("arrangement-automation-{}-{}", "{}", track.0);
        let (lane_device, lane_control) =
            (lane.replace("{}", "device"), lane.replace("{}", "control"));
        let controls = control_choices(&self.h.shell, self.doc(), device);
        let control_labels: Vec<String> = controls.iter().map(|c| c.label.clone()).collect();
        // The lane lists every choice, None too; the editor has no None and
        // calls the mixer "Mixer".
        let all = device_choices(self.doc(), track);
        let mut clip_devices: Vec<_> = all
            .iter()
            .filter(|c| c.value != AutomationDevice::None)
            .map(|c| (c.label.clone(), c.value))
            .collect();
        clip_devices[0].0 = "Mixer".to_owned();
        let lane_devices: Vec<_> = all.iter().map(|c| (c.label.clone(), c.value)).collect();
        for (salt, devices) in [
            (lane_device, lane_devices),
            ("clip-envelope-device".to_owned(), clip_devices),
        ] {
            let labels: Vec<String> = devices.iter().map(|(label, _)| label.clone()).collect();
            let rows = self.leds(&salt, &labels);
            for ((label, value), got) in devices.iter().zip(rows) {
                assert_eq!(got, want_device(*value), "{salt}: {label}");
            }
        }
        for salt in [lane_control, "clip-envelope-control".to_owned()] {
            let rows = self.leds(&salt, &control_labels);
            for (choice, got) in controls.iter().zip(rows) {
                assert_eq!(got, want_control(choice.value), "{salt}: {}", choice.label);
            }
        }
    }
}

// Spec: modulation R8
#[test]
fn the_choosers_mark_automation_modulation_and_both() {
    let mut s = set();
    // The send's envelope was overridden by moving the send by hand.
    let track = s.doc().track(s.track).unwrap();
    assert!(track.automation_for(&pan()).unwrap().enabled);
    assert!(!track.automation_for(&s.send()).unwrap().enabled);
    assert!(!track.automation_for(&s.drive_param()).unwrap().enabled);
    // The Arrangement shows its automation lanes, and the Clip View the
    // envelopes.
    s.run("arrangement.automation_mode", json!({ "on": true }));
    s.run("view.toggle_clip_view", json!({}));
    s.run("view.clip_tab", json!({ "tab": "envelopes" }));
    s.check_choosers(pan(), AutomationDevice::Mixer);
    let (gain, drive) = (s.gain(), s.drive_param());
    s.check_choosers(gain, AutomationDevice::Device(s.utility));
    s.check_choosers(drive, AutomationDevice::Device(s.drive));
}

// Spec: modulation R9
#[test]
fn a_lane_draws_the_base_only() {
    let mut s = set();
    let track = s.track;
    s.run("arrangement.automation_mode", json!({ "on": true }));
    s.run(
        "arrangement.show_automation",
        json!({ "track": track, "target": json_of(pan()) }),
    );
    // Pan's base: an envelope, and a clip modulating it.
    s.run(
        "automation.set_breakpoint",
        json!({ "track": track, "target": json_of(pan()), "at": Ticks::WHOLE.get() * 2, "value": -0.5 }),
    );
    s.run(
        "clip.set_envelope_breakpoint",
        json!({ "clip": s.clip, "target": json_of(pan()), "at": 0, "value": 1.0 }),
    );
    s.h.settle();
    let lane =
        s.h.main
            .arrangement()
            .automation_lane_rect(s.h.shell.arrangement(), s.doc(), track)
            .expect("the track shows an automation lane");
    let c = &s.h.theme.colors;
    let (auto, modulation) = (c.automation.0, c.modulation.0);
    let output = s.h.frame(vec![]);
    let base = paths(&output, auto, lane);
    assert_eq!(base.len(), 1, "the envelope is drawn: {base:?}");
    assert!(
        paths(&output, modulation, lane.expand(1.0)).is_empty(),
        "no modulation in the lane"
    );
    // Taking the clip's modulation away changes nothing in the lane.
    s.run(
        "clip.delete_envelope",
        json!({ "clip": s.clip, "target": json_of(pan()) }),
    );
    let output = s.h.frame(vec![]);
    assert_eq!(paths(&output, auto, lane), base);
}

/// The Clip View's editor area and the contents tick at its left edge, with
/// the Envelopes tab showing `target`.
fn editor(s: &mut Set, target: AutomationTarget) -> (Rect, Ticks) {
    s.run("view.toggle_clip_view", json!({}));
    s.run("view.clip_tab", json!({ "tab": "envelopes" }));
    s.run("view.clip_envelope", json!({ "target": json_of(target) }));
    let nodes = Harness::nodes(&s.h.settle());
    let b = Harness::node(&nodes, "Clip View").bounds.unwrap();
    let view = Rect::from_min_max(
        pos2(b.x0 as f32, b.y0 as f32),
        pos2(b.x1 as f32, b.y1 as f32),
    );
    let clip = s.doc().clip(s.clip).unwrap();
    let ClipData::Midi(midi) = &clip.data else {
        panic!("the clip is MIDI");
    };
    (lane_rect(view), axis_start(clip, midi, Some(target)))
}

fn drag(h: &mut Harness, from: Pos2, to: Pos2) {
    h.frame(vec![Event::PointerMoved(from)]);
    h.frame(vec![Event::PointerButton {
        pos: from,
        button: PointerButton::Primary,
        pressed: true,
        modifiers: Modifiers::NONE,
    }]);
    for step in 1..=8 {
        h.frame(vec![Event::PointerMoved(
            from + (to - from) * (step as f32 / 8.0),
        )]);
    }
    h.frame(vec![Event::PointerButton {
        pos: to,
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    }]);
    h.settle();
}

/// The editor on `target`, whose scale runs from `low` to 100 % and reads
/// `labels`: what it draws, the dashed line, and the breakpoints placed and
/// dragged in shares.
fn check_scale(target: impl Fn(&Set) -> AutomationTarget, low: f64, labels: &[&str]) {
    let mut s = set();
    let target = target(&s);
    let (lane, start) = editor(&mut s, target);
    let clip = s.clip;
    let length = s.doc().clip(clip).unwrap().length;
    let colours = s.h.theme.colors.clone();
    let y_of = |share: f64| lane.bottom() - ((share - low) / (1.0 - low)) as f32 * lane.height();
    let x_of = |at: Ticks| x_of_tick(lane, start, length, at);
    let share_at = |s: &Set, at: Ticks| {
        s.doc()
            .clip(clip)
            .unwrap()
            .envelope(&target)
            .unwrap()
            .envelope
            .point(at)
            .map(|point| point.value)
    };
    // The envelope the set made, a share of 0.5, is drawn in the modulation
    // colour at that share of the scale.
    let output = s.h.frame(vec![]);
    let drawn = paths(&output, colours.modulation.0, lane.expand(1.0));
    assert_eq!(drawn.len(), 1, "{drawn:?}");
    assert!(
        drawn[0].iter().all(|p| (p.y - y_of(0.5)).abs() < 0.5),
        "a share of 0.5 sits at {}: {:?}",
        y_of(0.5),
        drawn[0]
    );
    // The left axis reads in percent.
    let mut found = texts(&output, lane);
    found.sort();
    let mut want: Vec<String> = labels.iter().map(|label| (*label).to_owned()).collect();
    want.sort();
    assert_eq!(found, want);
    // Without an envelope the dashed line sits where nothing modulates: at
    // 100 % for a level, at the middle of a scale that runs both ways.
    let json = json_of(target);
    s.run(
        "clip.delete_envelope",
        json!({ "clip": clip, "target": json }),
    );
    let output = s.h.settle();
    let dashed = lines(&output, colours.record.0, lane);
    assert!(!dashed.is_empty(), "a dashed line");
    let rest = if low == 0.0 { 1.0 } else { 0.0 };
    for line in &dashed {
        assert!((line[0].y - y_of(rest)).abs() < 0.01, "{line:?}");
    }
    // A click off the line adds a breakpoint at the share clicked, and
    // dragging one to a place sends the share there.
    let quarter = Ticks(length.get() / 4);
    let at = pos2(x_of(quarter), y_of(0.25));
    s.h.frame(vec![Event::PointerMoved(at)]);
    for _ in 0..2 {
        for pressed in [true, false] {
            s.h.frame(vec![Event::PointerButton {
                pos: at,
                button: PointerButton::Primary,
                pressed,
                modifiers: Modifiers::NONE,
            }]);
        }
    }
    s.h.settle();
    let points: Vec<_> = s
        .doc()
        .clip(clip)
        .unwrap()
        .envelope(&target)
        .expect("a double-click draws an envelope")
        .envelope
        .points()
        .collect();
    assert_eq!(points.len(), 1, "{points:?}");
    assert!((points[0].value - 0.25).abs() < 0.03, "{points:?}");
    let on = points[0].at;
    drag(
        &mut s.h,
        pos2(x_of(on), y_of(0.25)),
        pos2(x_of(on), y_of(0.75)),
    );
    let moved = share_at(&s, on).expect("the breakpoint stays where it is");
    assert!((moved - 0.75).abs() < 0.03, "{moved}");
}

// Spec: modulation R9
#[test]
fn the_clip_editor_draws_modulation_on_its_own_scale() {
    // A level: 0 to 100 %.
    check_scale(|_| volume(), 0.0, &["100%", "0%"]);
    // A device's parameter: -100 % to 100 %.
    check_scale(Set::gain, -1.0, &["100%", "0%", "-100%"]);
}

// Spec: modulation R9
#[test]
fn an_arrangement_clip_has_no_automation_switch() {
    let mut editor = bl_shell::files::factory_default_set();
    let track = editor.document().top_level().next().unwrap();
    let id: ClipId = editor.new_id();
    editor
        .apply(bl_model::Command::CreateClip {
            track,
            clip: bl_model::Clip::new(
                id,
                Ticks::ZERO,
                Ticks(4 * Ticks::WHOLE.get()),
                ClipData::Midi(bl_model::MidiData::new(Vec::new())),
            ),
        })
        .unwrap();
    editor.clear_history().unwrap();
    let mut h = Harness::with_title_bar(Window::Main);
    h.shell = bl_ui::arrangement::benchmark::shell_with(editor);
    h.settle();
    h.shell
        .run("arrangement.select_clips", json!({ "clips": [id] }))
        .unwrap();
    h.shell.run("view.toggle_clip_view", json!({})).unwrap();
    h.shell
        .run("view.clip_tab", json!({ "tab": "envelopes" }))
        .unwrap();
    h.shell
        .run("view.clip_envelope", json!({ "target": json_of(pan()) }))
        .unwrap();
    let nodes = Harness::nodes(&h.settle());
    // The tab is showing: its choosers and the Linked switch are there.
    for name in ["Envelope Device", "Envelope Control", "Linked"] {
        Harness::node(&nodes, name);
    }
    // It edits modulation only, so it has no switch between the two.
    let switches: Vec<&str> = nodes
        .iter()
        .filter(|n| matches!(n.label.as_str(), "Automation" | "Modulation"))
        .map(|n| n.label.as_str())
        .collect();
    assert!(switches.is_empty(), "{switches:?}");
}

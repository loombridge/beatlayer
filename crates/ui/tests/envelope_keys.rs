//! Editing envelopes from the keyboard, headless (WP-6.26): in an automation
//! lane and in the Clip View's Envelope Editor, the insert marker's steps and
//! the value shown, Enter making and selecting, the arrows moving a
//! breakpoint and changing its value, a typed value, Enter keeping and Esc
//! dropping the edit, Delete, Tab, Option-Up and Option-Down, and Cmd-Delete,
//! each edit one undo step, and what a screen reader is told. Spec:
//! automation-editing R20 to R29; clip-envelopes R14.

mod common;

use bl_core::time::Ticks;
use bl_model::{AutomationTarget, Breakpoint, ClipId, Document, MixerParam, TrackId};
use bl_shell::Area;
use bl_ui::main_window::UiEvent;
use common::{Harness, Window};
use egui::{Event, Key, Modifiers, PointerButton, Pos2, Rect};
use serde_json::json;

const BAR: i64 = Ticks::WHOLE.get();

fn pan() -> AutomationTarget {
    AutomationTarget::Mixer {
        param: MixerParam::Pan,
    }
}

fn volume() -> AutomationTarget {
    AutomationTarget::Mixer {
        param: MixerParam::Volume,
    }
}

fn option() -> Modifiers {
    Modifiers::ALT
}

/// A track with a pan envelope from −0.8 at 1.1.1 to 0.8 at bar 3, showing
/// in Automation Mode on a quarter-note grid.
struct Lane {
    h: Harness,
    track: TrackId,
}

fn lane() -> Lane {
    let mut h = Harness::manual_with_title_bar(Window::Main);
    let track = h.shell.editor().document().top_level().next().unwrap();
    for (at, value) in [(0, -0.8), (BAR * 2, 0.8)] {
        h.shell
            .run(
                "automation.set_breakpoint",
                json!({ "track": track, "target": pan(), "at": at, "value": value }),
            )
            .unwrap();
    }
    h.shell
        .run("arrangement.set_grid", json!({ "fixed": "quarter" }))
        .unwrap();
    h.shell
        .run("arrangement.automation_mode", json!({ "on": true }))
        .unwrap();
    h.shell
        .run(
            "arrangement.show_automation",
            json!({ "track": track, "target": pan() }),
        )
        .unwrap();
    h.settle();
    Lane { h, track }
}

impl Lane {
    fn doc(&self) -> &Document {
        self.h.shell.editor().document()
    }

    fn points(&self) -> Vec<Breakpoint> {
        self.doc()
            .track(self.track)
            .unwrap()
            .automation_for(&pan())
            .map(|a| a.envelope.points().collect())
            .unwrap_or_default()
    }

    fn steps(&self) -> usize {
        self.h.shell.editor().undo_history().len()
    }

    fn rect(&self) -> Rect {
        self.h
            .main
            .arrangement()
            .automation_lane_rect(self.h.shell.arrangement(), self.doc(), self.track)
            .expect("the track shows an automation lane")
    }

    fn x(&self, at: Ticks) -> f32 {
        self.rect().left() + self.h.shell.arrangement().horizontal(self.doc()).x(at) as f32
    }

    /// Clicks the lane's empty background at the top, at `at`: the lane is
    /// selected and the insert marker goes there.
    fn click_at(&mut self, at: Ticks) {
        let pos = Pos2::new(self.x(at), self.rect().top() + 3.0);
        self.h.click(pos);
        self.h.settle();
    }

    fn press(&mut self, key: Key, modifiers: Modifiers) {
        self.h.key(key, modifiers);
        self.h.settle();
    }

    /// The command IDs invoked since the last call.
    fn commands(&mut self) -> Vec<String> {
        self.h
            .take_events()
            .into_iter()
            .filter_map(|event| match event {
                UiEvent::Invoke(invocation) => Some(invocation.id),
                _ => None,
            })
            .collect()
    }

    fn marker(&self) -> Ticks {
        self.h.shell.arrangement().insert_marker()
    }

    fn selected(&self) -> Option<Ticks> {
        self.h
            .shell
            .arrangement()
            .selected_breakpoint()
            .map(|(_, _, at)| at)
    }

    /// The lane's node as a screen reader reads it, with its value.
    fn node_value(&mut self) -> (String, String) {
        let output = self.h.settle();
        let nodes = Harness::nodes(&output);
        let node = nodes
            .iter()
            .find(|n| n.label == "Pan Automation")
            .expect("the lane's group");
        (node.label.clone(), node.value.clone())
    }
}

// Spec: automation-editing R1, R20. Clicking a lane's background selects it
// and gives it the keyboard; Up and Down move through each track's lane and
// then its automation lane.
#[test]
fn clicking_a_lane_selects_it_and_up_and_down_move_through_the_lanes() {
    let mut lane = lane();
    lane.click_at(Ticks(BAR));
    assert!(lane.h.shell.arrangement().keyboard_lane().is_some());
    assert_eq!(lane.marker(), Ticks(BAR));
    // Up goes to the track's own lane, then on to the track above.
    lane.press(Key::ArrowUp, Modifiers::NONE);
    assert_eq!(lane.h.shell.arrangement().keyboard_lane(), None);
    assert_eq!(lane.h.shell.arrangement().selected_tracks(), [lane.track]);
    // Down goes to the lane again.
    lane.press(Key::ArrowDown, Modifiers::NONE);
    assert_eq!(
        lane.h.shell.arrangement().keyboard_lane().map(|(t, _)| t),
        Some(lane.track)
    );
}

// Spec: automation-editing R21, R28. Left and Right move the insert marker a
// grid step and the value there is shown, and read as the lane's value.
#[test]
fn left_and_right_move_the_insert_marker_and_the_value_is_shown() {
    let mut lane = lane();
    lane.click_at(Ticks(BAR));
    let step = lane.h.shell.arrangement().grid_ticks(lane.doc());
    lane.press(Key::ArrowRight, Modifiers::NONE);
    assert_eq!(lane.marker(), Ticks(BAR) + step);
    lane.press(Key::ArrowRight, Modifiers::NONE);
    assert_eq!(lane.marker(), Ticks(BAR) + step * 2);
    lane.press(Key::ArrowLeft, Modifiers::NONE);
    assert_eq!(lane.marker(), Ticks(BAR) + step);
    // The envelope's value at the marker, as pan reads it.
    let (label, value) = lane.node_value();
    assert_eq!(label, "Pan Automation");
    let expected = lane
        .h
        .shell
        .keyboard_envelope()
        .expect("the lane has the keyboard")
        .value;
    assert_eq!(value, expected);
    assert!(
        value.ends_with('R') || value.ends_with('L') || value == "C",
        "{value}"
    );
}

// Spec: automation-editing R21. Option-Left and Option-Right jump to the
// previous and next breakpoint.
#[test]
fn option_left_and_right_jump_between_breakpoints() {
    let mut lane = lane();
    lane.click_at(Ticks(BAR / 2));
    lane.press(Key::ArrowRight, option());
    assert_eq!(lane.marker(), Ticks(BAR * 2));
    lane.press(Key::ArrowLeft, option());
    assert_eq!(lane.marker(), Ticks::ZERO);
}

// Spec: automation-editing R22, R23, R28. Enter makes a breakpoint at the
// envelope's value, the arrows move it and change its value, Enter keeps the
// edit as one undo step, and the screen reader reads the breakpoint as the
// lane's active descendant.
#[test]
fn enter_makes_a_breakpoint_the_arrows_move_it_and_enter_keeps_it() {
    let mut lane = lane();
    let before = lane.points();
    let steps = lane.steps();
    lane.click_at(Ticks(BAR));
    lane.press(Key::Enter, Modifiers::NONE);
    assert_eq!(lane.selected(), Some(Ticks(BAR)));
    assert_eq!(lane.points().len(), 3);
    assert!(lane.commands().contains(&"envelope.enter".to_owned()));
    lane.press(Key::ArrowRight, Modifiers::NONE);
    let step = lane.h.shell.arrangement().grid_ticks(lane.doc());
    assert_eq!(lane.selected(), Some(Ticks(BAR) + step));
    lane.press(Key::ArrowUp, Modifiers::NONE);
    lane.press(Key::ArrowUp, Modifiers::NONE);
    let moved = lane
        .points()
        .into_iter()
        .find(|p| p.at == Ticks(BAR) + step)
        .expect("the breakpoint moved");
    let on_line = before
        .iter()
        .zip(before.iter().skip(1))
        .map(|(a, b)| a.value + (b.value - a.value) * 0.5)
        .next()
        .unwrap();
    // Pan steps by 1/50 for an arrow key: two presses up.
    assert!(
        (moved.value - on_line - 0.04).abs() < 0.01,
        "{moved:?} {on_line}"
    );
    // The screen reader reads the selected breakpoint as the lane's focus.
    let output = lane.h.settle();
    let nodes = Harness::nodes(&output);
    let breakpoint = nodes
        .iter()
        .find(|n| n.label == "Breakpoint")
        .expect("the focused breakpoint's node");
    assert_eq!(breakpoint.selected, Some(true));
    assert!(
        breakpoint.value.starts_with("2.2.1"),
        "{}",
        breakpoint.value
    );
    assert_eq!(lane.steps(), steps, "the edit is one step only when kept");
    lane.press(Key::Enter, Modifiers::NONE);
    assert_eq!(lane.selected(), None);
    assert_eq!(lane.steps(), steps + 1);
    lane.h.shell.run("edit.undo", json!({})).unwrap();
    assert_eq!(lane.points(), before);
}

// Spec: automation-editing R23. Esc puts the breakpoint back as it was, and
// removes one Enter made.
#[test]
fn esc_drops_the_edit() {
    let mut lane = lane();
    let before = lane.points();
    let steps = lane.steps();
    lane.click_at(Ticks(BAR));
    lane.press(Key::Enter, Modifiers::NONE);
    lane.press(Key::ArrowUp, Modifiers::NONE);
    lane.press(Key::Escape, Modifiers::NONE);
    assert_eq!(lane.selected(), None);
    assert_eq!(lane.points(), before);
    assert_eq!(lane.steps(), steps);
    assert!(lane.h.shell.arrangement().keyboard_lane().is_some());
}

// Spec: automation-editing R23. Digits start a value in the units the
// control shows, which Return sets.
#[test]
fn a_typed_value_is_set_with_return() {
    let mut lane = lane();
    lane.click_at(Ticks::ZERO);
    lane.press(Key::Enter, Modifiers::NONE);
    for key in [Key::Num2, Key::Num5, Key::L] {
        lane.press(key, Modifiers::NONE);
    }
    assert!(bl_ui::envelope_keys::is_typing(&lane.h.ctx));
    lane.press(Key::Enter, Modifiers::NONE);
    assert!(!bl_ui::envelope_keys::is_typing(&lane.h.ctx));
    assert_eq!(lane.points()[0].value, -0.5);
    // The breakpoint is still selected: Enter keeps the edit.
    assert_eq!(lane.selected(), Some(Ticks::ZERO));
    lane.press(Key::Enter, Modifiers::NONE);
    assert_eq!(lane.selected(), None);
    assert_eq!(lane.steps(), 3);
}

// Spec: automation-editing R22, R24. Tab selects the next breakpoint only
// while one is selected, and Delete deletes the selected one or the one at
// the insert marker.
#[test]
fn tab_and_delete_answer_only_with_a_breakpoint() {
    let mut lane = lane();
    lane.click_at(Ticks(BAR));
    lane.commands();
    lane.press(Key::Tab, Modifiers::NONE);
    assert!(
        !lane
            .commands()
            .contains(&"envelope.select_adjacent".to_owned()),
        "Tab isn't the envelope's with nothing selected"
    );
    // It falls through to the Arrangement's own Tab, which here switches the
    // main view; back again.
    assert_eq!(lane.h.shell.main_view(), bl_shell::MainView::Session);
    lane.h
        .shell
        .run("view.toggle_session_arrangement", json!({}))
        .unwrap();
    lane.click_at(Ticks(BAR));
    lane.press(Key::Delete, Modifiers::NONE);
    assert_eq!(lane.points().len(), 2, "no breakpoint at the marker");
    lane.click_at(Ticks(BAR));
    lane.press(Key::ArrowLeft, option());
    assert_eq!(lane.marker(), Ticks::ZERO);
    lane.press(Key::Enter, Modifiers::NONE);
    lane.commands();
    lane.press(Key::Tab, Modifiers::NONE);
    assert!(
        lane.commands()
            .contains(&"envelope.select_adjacent".to_owned())
    );
    assert_eq!(lane.selected(), Some(Ticks(BAR * 2)));
    lane.press(Key::Tab, Modifiers::SHIFT);
    assert_eq!(lane.selected(), Some(Ticks::ZERO));
    let steps = lane.steps();
    lane.press(Key::Delete, Modifiers::NONE);
    assert_eq!(lane.points().len(), 1);
    assert_eq!(lane.steps(), steps + 1);
    // With the marker on a breakpoint and none selected, Delete deletes it.
    lane.press(Key::ArrowRight, option());
    assert_eq!(lane.marker(), Ticks(BAR * 2));
    lane.press(Key::Delete, Modifiers::NONE);
    assert!(lane.points().is_empty());
}

// Spec: automation-editing R25. Option-Up and Option-Down show the previous
// or next parameter that has an envelope, and with Shift any the choosers
// list, going round.
#[test]
fn option_up_and_down_cycle_the_shown_parameter() {
    let mut lane = lane();
    lane.h
        .shell
        .run(
            "automation.set_breakpoint",
            json!({ "track": lane.track, "target": volume(), "at": 0, "value": 1.0 }),
        )
        .unwrap();
    lane.click_at(Ticks(BAR));
    let shown = |lane: &Lane| lane.h.shell.arrangement().automation_lane(lane.track);
    assert_eq!(shown(&lane), Some(pan()));
    // The mixer lists Volume before Pan: two envelopes, so Down goes round.
    lane.press(Key::ArrowDown, option());
    assert_eq!(shown(&lane), Some(volume()));
    lane.press(Key::ArrowDown, option());
    assert_eq!(shown(&lane), Some(pan()));
    lane.press(Key::ArrowUp, option());
    assert_eq!(shown(&lane), Some(volume()));
    // With Shift, every parameter the choosers list.
    lane.press(Key::ArrowDown, Modifiers::ALT | Modifiers::SHIFT);
    assert_eq!(shown(&lane), Some(pan()));
    lane.press(Key::ArrowDown, Modifiers::ALT | Modifiers::SHIFT);
    assert_eq!(
        shown(&lane),
        Some(AutomationTarget::Mixer {
            param: MixerParam::Mute
        })
    );
}

// Spec: automation-editing R26. Cmd-Delete deletes the shown parameter's
// automation.
#[test]
fn cmd_delete_deletes_the_shown_envelope() {
    let mut lane = lane();
    lane.click_at(Ticks(BAR));
    lane.press(Key::Delete, common::command());
    assert!(lane.points().is_empty());
}

/// A MIDI clip with a pan envelope in the Envelope Editor, the detail area
/// focused.
struct Editor {
    h: Harness,
    clip: ClipId,
}

fn editor() -> Editor {
    let mut h = Harness::manual_with_title_bar(Window::Main);
    let track = h.shell.editor().document().top_level().next().unwrap();
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
    h.shell.run("view.toggle_clip_view", json!({})).unwrap();
    h.shell
        .run("arrangement.set_grid", json!({ "fixed": "quarter" }))
        .unwrap();
    h.shell
        .run("view.clip_tab", json!({ "tab": "envelopes" }))
        .unwrap();
    h.shell
        .run("view.clip_envelope", json!({ "target": pan() }))
        .unwrap();
    for (at, value) in [(0, -0.5), (BAR, 0.5)] {
        h.shell
            .run(
                "clip.set_envelope_breakpoint",
                json!({ "clip": clip, "target": pan(), "at": at, "value": value }),
            )
            .unwrap();
    }
    h.shell.set_focused_area(Area::Detail);
    h.settle();
    Editor { h, clip }
}

impl Editor {
    fn points(&self) -> Vec<Breakpoint> {
        self.h
            .shell
            .editor()
            .document()
            .clip(self.clip)
            .unwrap()
            .envelope(&pan())
            .map(|e| e.envelope.points().collect())
            .unwrap_or_default()
    }

    fn steps(&self) -> usize {
        self.h.shell.editor().undo_history().len()
    }

    fn press(&mut self, key: Key, modifiers: Modifiers) {
        self.h.key(key, modifiers);
        self.h.settle();
    }

    fn state(&self) -> bl_shell::envelope_commands::EnvelopeKeyState {
        self.h
            .shell
            .keyboard_envelope()
            .expect("the Envelope Editor has the keyboard")
    }
}

// Spec: clip-envelopes R14, automation-editing R21. The Envelope Editor has
// its own insert marker, which Left and Right move with the value shown.
#[test]
fn the_envelope_editors_insert_marker_moves_along_the_envelope() {
    let mut e = editor();
    let step =
        e.h.shell
            .arrangement()
            .grid_ticks(e.h.shell.editor().document());
    e.press(Key::ArrowRight, Modifiers::NONE);
    assert_eq!(e.state().marker, step);
    e.press(Key::ArrowRight, option());
    assert_eq!(e.state().marker, Ticks(BAR));
    assert_eq!(e.state().value, "50%");
    e.press(Key::ArrowLeft, Modifiers::NONE);
    assert_eq!(e.state().marker, Ticks(BAR) - step);
    let output = e.h.settle();
    let nodes = Harness::nodes(&output);
    let group = Harness::node(&nodes, "Clip Envelope");
    assert!(group.value.starts_with("Pan, "), "{}", group.value);
}

// Spec: clip-envelopes R14, automation-editing R22, R23. Enter makes and
// selects, the arrows move and change, a typed value sets, Enter keeps (one
// undo step) and Esc drops.
#[test]
fn the_envelope_editors_breakpoints_are_edited_from_the_keys() {
    let mut e = editor();
    let before = e.points();
    let steps = e.steps();
    e.press(Key::ArrowRight, Modifiers::NONE);
    e.press(Key::Enter, Modifiers::NONE);
    assert_eq!(e.points().len(), 3);
    assert!(e.state().selected.is_some());
    e.press(Key::ArrowDown, Modifiers::NONE);
    e.press(Key::ArrowRight, Modifiers::NONE);
    e.press(Key::Escape, Modifiers::NONE);
    assert_eq!(e.points(), before, "Esc removes the breakpoint Enter made");
    assert_eq!(e.steps(), steps);
    // Select the first, type a value, keep it.
    e.press(Key::ArrowLeft, Modifiers::NONE);
    e.press(Key::Enter, Modifiers::NONE);
    assert_eq!(e.state().selected, Some(Ticks::ZERO));
    for key in [Key::Num2, Key::Num5] {
        e.press(key, Modifiers::NONE);
    }
    e.press(Key::Enter, Modifiers::NONE);
    assert_eq!(e.points()[0].value, 0.25);
    e.press(Key::Enter, Modifiers::NONE);
    assert_eq!(e.state().selected, None);
    assert_eq!(e.steps(), steps + 1);
    e.h.shell.run("edit.undo", json!({})).unwrap();
    assert_eq!(e.points(), before);
    // Delete deletes the breakpoint at the marker.
    e.press(Key::Delete, Modifiers::NONE);
    assert_eq!(e.points().len(), 1);
    // Cmd-Delete clears the envelope.
    e.press(Key::Delete, common::command());
    assert!(e.points().is_empty());
}

// Spec: clip-envelopes R14, automation-editing R22. Tab selects the next
// breakpoint only while one is selected.
#[test]
fn the_envelope_editors_tab_answers_only_with_a_breakpoint_selected() {
    let mut e = editor();
    e.h.take_events();
    e.press(Key::Tab, Modifiers::NONE);
    assert!(!e.h.take_events().iter().any(|event| matches!(
        event,
        UiEvent::Invoke(i) if i.id == "envelope.select_adjacent"
    )));
    e.press(Key::Enter, Modifiers::NONE);
    e.press(Key::Tab, Modifiers::NONE);
    assert_eq!(e.state().selected, Some(Ticks(BAR)));
    e.press(Key::Tab, Modifiers::SHIFT);
    assert_eq!(e.state().selected, Some(Ticks::ZERO));
}

// Spec: automation-editing R25. Option-Up and Option-Down show the next
// parameter the clip has an envelope for, in the choosers' order.
#[test]
fn the_envelope_editors_option_up_and_down_cycle_the_shown_parameter() {
    let mut e = editor();
    e.h.shell
        .run(
            "clip.set_envelope_breakpoint",
            json!({ "clip": e.clip, "target": volume(), "at": 0, "value": 0.5 }),
        )
        .unwrap();
    e.h.settle();
    e.press(Key::ArrowDown, option());
    assert_eq!(e.h.shell.clip_view().envelope(), Some(volume()));
    e.press(Key::ArrowDown, option());
    assert_eq!(e.h.shell.clip_view().envelope(), Some(pan()));
    // With Shift, every parameter the choosers list.
    e.press(Key::ArrowDown, Modifiers::ALT | Modifiers::SHIFT);
    assert_eq!(
        e.h.shell.clip_view().envelope(),
        Some(AutomationTarget::Mixer {
            param: MixerParam::Mute
        })
    );
}

// Spec: automation-editing R28. The lane is a group named for the parameter
// it shows, valued as the envelope is at the insert marker, and the focused
// breakpoint is its active descendant.
#[test]
fn the_focused_breakpoint_is_the_lanes_active_descendant() {
    let mut lane = lane();
    lane.click_at(Ticks::ZERO);
    let output = lane.h.settle();
    let nodes = Harness::nodes(&output);
    let group = Harness::node(&nodes, "Pan Automation");
    let breakpoint = Harness::node(&nodes, "Breakpoint");
    assert_eq!(group.active_descendant, Some(breakpoint.id));
    assert_eq!(group.value, "40L");
    assert_eq!(breakpoint.value, "1.1.1, 40L");
    assert_eq!(breakpoint.selected, Some(false), "focused, not selected");
    // Selecting it changes what is read.
    lane.press(Key::Enter, Modifiers::NONE);
    let output = lane.h.settle();
    let nodes = Harness::nodes(&output);
    assert_eq!(Harness::node(&nodes, "Breakpoint").selected, Some(true));
    // The view's own focus is the lane.
    let lane_node = Harness::node(&nodes, "Pan Automation").id;
    assert!(
        nodes.iter().any(|n| n.active_descendant == Some(lane_node)),
        "the Arrangement View reads the lane as its focus"
    );
}

// Spec: automation-editing R22 (S-A11Y-021). Option-click (Alt-click) a
// breakpoint selects it and moves the insert marker to it.
#[test]
fn option_click_selects_a_breakpoint_and_moves_the_insert_marker_to_it() {
    let mut lane = lane();
    lane.click_at(Ticks(BAR / 2));
    let rect = lane.rect();
    // The last breakpoint, pan 0.8: 90% of the way up.
    let pos = Pos2::new(lane.x(Ticks(BAR * 2)), rect.bottom() - 0.9 * rect.height());
    let held = Event::ModifiersChanged(option());
    lane.h.frame(vec![held.clone(), Event::PointerMoved(pos)]);
    for pressed in [true, false] {
        lane.h.frame(vec![
            held.clone(),
            Event::PointerButton {
                pos,
                button: PointerButton::Primary,
                pressed,
                modifiers: option(),
            },
        ]);
    }
    lane.h.frame(vec![Event::ModifiersChanged(Modifiers::NONE)]);
    lane.h.settle();
    assert_eq!(lane.selected(), Some(Ticks(BAR * 2)));
    assert_eq!(lane.marker(), Ticks(BAR * 2));
    assert_eq!(lane.points().len(), 2, "Option-click doesn't delete it");
}

// Spec: automation-editing R27. Edit Value types a value for the
// breakpoint, the other selected breakpoints moving by the same share, as
// one undo step; Add Value types the value of a new breakpoint on the line.
#[test]
fn edit_value_and_add_value_type_a_value_and_return_sets_it() {
    let mut lane = lane();
    lane.click_at(Ticks(BAR));
    let rect = lane.rect();
    let steps = lane.steps();
    // Edit Value on the first breakpoint (pan −0.8, 10% up the lane).
    let first = Pos2::new(lane.x(Ticks::ZERO), rect.bottom() - 0.1 * rect.height());
    open_menu(&mut lane.h, first);
    click_item(&mut lane.h, "Edit Value");
    assert_eq!(
        lane.selected(),
        Some(Ticks::ZERO),
        "the breakpoint is selected"
    );
    for key in [Key::Num2, Key::Num5, Key::R] {
        lane.press(key, Modifiers::NONE);
    }
    lane.press(Key::Enter, Modifiers::NONE);
    assert_eq!(lane.points()[0].value, 0.5);
    assert_eq!(lane.selected(), None, "Return lets go of it");
    assert_eq!(lane.steps(), steps + 1, "one undo step");
    // Add Value on the line, half way along.
    let line = {
        let value = (lane.points()[0].value + lane.points()[1].value) / 2.0;
        Pos2::new(
            lane.x(Ticks(BAR)),
            rect.bottom() - (((value + 1.0) / 2.0) as f32) * rect.height(),
        )
    };
    open_menu(&mut lane.h, line);
    click_item(&mut lane.h, "Add Value");
    for key in [Key::Num1, Key::Num0, Key::L] {
        lane.press(key, Modifiers::NONE);
    }
    lane.press(Key::Enter, Modifiers::NONE);
    assert_eq!(lane.points().len(), 3);
    assert_eq!(lane.points()[1].at, Ticks(BAR));
    assert!((lane.points()[1].value + 0.2).abs() < 1e-9);
    assert_eq!(lane.steps(), steps + 2);
}

fn open_menu(h: &mut Harness, pos: Pos2) {
    h.frame(vec![Event::PointerMoved(pos)]);
    for pressed in [true, false] {
        h.frame(vec![Event::PointerButton {
            pos,
            button: PointerButton::Secondary,
            pressed,
            modifiers: Modifiers::NONE,
        }]);
    }
    h.settle();
}

fn click_item(h: &mut Harness, label: &str) {
    let output = h.settle();
    let nodes = Harness::nodes(&output);
    let id = Harness::node(&nodes, label).id;
    h.frame(vec![Event::AccessKitActionRequest(
        egui::accesskit::ActionRequest {
            action: egui::accesskit::Action::Click,
            target_tree: egui::accesskit::TreeId::ROOT,
            target_node: id,
            data: None,
        },
    )]);
    h.settle();
}

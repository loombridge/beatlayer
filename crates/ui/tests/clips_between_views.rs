//! Clips between the Session and the Arrangement, headless (WP-6.29b):
//! dragging them over a view selector, dragging them between two windows,
//! cancelling with Esc, and Cut, Copy and Paste by key in the Session View
//! and across the views.
//!
//! Spec: second-window R18 to R23; session-view R39 to R43.

use crate::common;

use bl_core::time::Ticks;
use bl_model::{
    AudioData, Clip, ClipData, ClipId, Command, ContentHash, Document, MidiData, SceneId, TrackId,
};
use bl_shell::clip_drag::DropSpot;
use bl_shell::files::factory_default_set;
use bl_shell::{MainView, WindowKind};
use bl_ui::arrangement::benchmark;
use bl_ui::layout::Element;
use common::{Harness, Window, command};
use egui::{Event, Key, Modifiers, PointerButton, Pos2, pos2};
use serde_json::json;

const BAR: i64 = Ticks::WHOLE.get();

/// The lanes' left edge and the new set's zoom: 30 points to a bar.
const LEFT: f32 = 320.0;
const POINTS_PER_BAR: f32 = 30.0;

/// Where bar `bar` (counting from 0) is across the lanes.
fn x(bar: f32) -> f32 {
    LEFT + bar * POINTS_PER_BAR
}

/// Where a track's lane is, down the window: each track 88 points from y 119
/// in the factory set, the middle of its row.
fn lane_y(track: usize) -> f32 {
    119.0 + 88.0 * track as f32 + 44.0
}

struct Rig {
    h: Harness,
    tracks: Vec<TrackId>,
    scenes: Vec<SceneId>,
}

/// The factory set with a two-bar MIDI Session clip "Slot" in the first
/// track's first scene, a one-bar audio Session clip "Sound" in the third
/// track's first scene, and a two-bar MIDI Arrangement clip "Lane" on the
/// first track from bar 10, with the main window on the Session View.
fn rig() -> Rig {
    let mut editor = factory_default_set();
    let tracks: Vec<TrackId> = editor.document().top_level().collect();
    let scenes: Vec<SceneId> = editor.document().scenes().map(|s| s.id).collect();
    let midi = |editor: &mut bl_model::Editor, name: &str, start: i64, bars: i64| {
        let id: ClipId = editor.new_id();
        let mut clip = Clip::new(
            id,
            Ticks(start * BAR),
            Ticks(bars * BAR),
            ClipData::Midi(MidiData::new(std::iter::empty())),
        );
        clip.name = name.to_owned();
        clip
    };
    let slot = midi(&mut editor, "Slot", 0, 2);
    editor
        .apply(Command::CreateSlotClip {
            track: tracks[0],
            scene: scenes[0],
            clip: slot,
        })
        .unwrap();
    let id: ClipId = editor.new_id();
    let mut sound = Clip::new(
        id,
        Ticks(0),
        Ticks(BAR),
        ClipData::Audio(AudioData::new(ContentHash([7; 32]))),
    );
    sound.name = "Sound".to_owned();
    editor
        .apply(Command::CreateSlotClip {
            track: tracks[2],
            scene: scenes[0],
            clip: sound,
        })
        .unwrap();
    let lane = midi(&mut editor, "Lane", 10, 2);
    editor
        .apply(Command::CreateClip {
            track: tracks[0],
            clip: lane,
        })
        .unwrap();
    editor.clear_history().unwrap();
    let mut h = Harness::with_title_bar(Window::Main);
    h.shell = benchmark::shell_with(editor);
    h.shell
        .run("view.toggle_session_arrangement", json!({}))
        .unwrap();
    h.settle();
    Rig { h, tracks, scenes }
}

impl Rig {
    fn doc(&self) -> &Document {
        self.h.shell.editor().document()
    }

    /// The centre of a slot, by its accessible name (the track's, then the
    /// scene's).
    fn slot(&mut self, label: &str) -> Pos2 {
        let output = self.h.settle();
        let nodes = Harness::nodes(&output);
        let node = nodes
            .iter()
            .find(|n| n.label == label || n.label.starts_with(&format!("{label} (")))
            .unwrap_or_else(|| panic!("no slot {label:?}"));
        let b = node.bounds.unwrap();
        pos2(((b.x0 + b.x1) / 2.0) as f32, ((b.y0 + b.y1) / 2.0) as f32)
    }

    fn selector(&self, element: Element) -> Pos2 {
        self.h.main.drawn_layout().unwrap().rect(element).center()
    }

    /// A track's Arrangement clips: `(start bar, bars, name)`, by start.
    fn lane(&self, track: usize) -> Vec<(i64, i64, String)> {
        let mut clips: Vec<_> = self
            .doc()
            .track(self.tracks[track])
            .unwrap()
            .clips()
            .map(|c| (c.start.get() / BAR, c.length.get() / BAR, c.name.clone()))
            .collect();
        clips.sort();
        clips
    }

    fn slot_name(&self, track: usize, scene: usize) -> Option<String> {
        self.doc()
            .slot(self.tracks[track], self.scenes[scene])
            .map(|c| c.name.clone())
    }

    fn steps(&self) -> usize {
        self.h.shell.editor().undo_history().len()
    }

    fn view(&self) -> MainView {
        self.h.shell.main_view_of(WindowKind::Main)
    }
}

fn press(h: &mut Harness, at: Pos2) {
    h.frame(vec![Event::PointerMoved(at)]);
    h.frame(vec![Event::PointerButton {
        pos: at,
        button: PointerButton::Primary,
        pressed: true,
        modifiers: Modifiers::NONE,
    }]);
}

fn move_to(h: &mut Harness, from: Pos2, to: Pos2) {
    for step in 1..=4 {
        let at = from + (to - from) * (step as f32 / 4.0);
        h.frame(vec![Event::PointerMoved(at)]);
    }
    h.frame(vec![]);
}

fn release(h: &mut Harness, at: Pos2) {
    h.frame(vec![Event::PointerButton {
        pos: at,
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    }]);
    h.settle();
}

// Spec: second-window R18, R20, R23 (I-DROP-014). A Session clip dragged over
// the Arrangement View button switches the window to the Arrangement at once,
// and is dropped there at the pointer: copied, the slot as it was, one undo
// step.
#[test]
fn dragging_a_session_clip_onto_the_arrangement_view_button_drops_it_there() {
    let mut r = rig();
    let slot = r.slot("1 MIDI: 1");
    let selector = r.selector(Element::ArrangementView);
    let before_slot = r.doc().slot(r.tracks[0], r.scenes[0]).cloned();
    let steps = r.steps();
    press(&mut r.h, slot);
    move_to(&mut r.h, slot, selector);
    assert_eq!(r.view(), MainView::Arrangement, "switched while dragging");
    assert!(r.h.shell.gesture_under_way(), "agents wait for the drag");
    let at = pos2(x(4.0), lane_y(0));
    move_to(&mut r.h, selector, at);
    release(&mut r.h, at);
    assert!(!r.h.shell.gesture_under_way());
    assert_eq!(
        r.lane(0),
        [(4, 2, "Slot".to_owned()), (10, 2, "Lane".to_owned())]
    );
    assert_eq!(
        r.doc().slot(r.tracks[0], r.scenes[0]).cloned(),
        before_slot,
        "the slot stays as it was"
    );
    assert_eq!(r.steps(), steps + 1);
    assert_eq!(
        r.h.shell.editor().undo_history().next(),
        Some("Copy Clips to Arrangement")
    );
}

// Spec: second-window R18, R20. Several selected Session clips, the one
// dragged among them, go together: the clip of the third track two tracks
// down from the first.
#[test]
fn the_selected_session_clips_are_dragged_together() {
    let mut r = rig();
    let slot = r.slot("1 MIDI: 1");
    // Select both, then drag one of them.
    for (track, mode) in [(0, "replace"), (2, "add")] {
        let (track, scene) = (r.tracks[track], r.scenes[0]);
        r.h.shell
            .run(
                "session.select",
                json!({ "track": track, "scene": scene, "mode": mode }),
            )
            .unwrap();
    }
    r.h.settle();
    assert_eq!(r.h.shell.session().selected_slots().len(), 2);
    let selector = r.selector(Element::ArrangementView);
    press(&mut r.h, slot);
    move_to(&mut r.h, slot, selector);
    let at = pos2(x(4.0), lane_y(0));
    move_to(&mut r.h, selector, at);
    release(&mut r.h, at);
    assert_eq!(r.lane(0)[0], (4, 2, "Slot".to_owned()));
    assert_eq!(r.lane(2), [(4, 1, "Sound".to_owned())]);
}

// Spec: second-window R18. Esc ends the drag with nothing changed and leaves
// the view switched; letting go outside a place that takes clips changes
// nothing either.
#[test]
fn esc_and_a_drop_on_nothing_change_nothing() {
    let mut r = rig();
    let slot = r.slot("1 MIDI: 1");
    let selector = r.selector(Element::ArrangementView);
    let before = r.doc().clone();
    // Esc during the drag.
    press(&mut r.h, slot);
    move_to(&mut r.h, slot, selector);
    assert!(r.h.shell.clip_drag().is_some());
    r.h.key(Key::Escape, Modifiers::NONE);
    assert!(r.h.shell.clip_drag().is_none(), "Esc ended the drag");
    let at = pos2(x(4.0), lane_y(0));
    move_to(&mut r.h, selector, at);
    release(&mut r.h, at);
    assert_eq!(*r.doc(), before, "nothing changed");
    assert_eq!(r.view(), MainView::Arrangement, "the view stays switched");
    // Letting go over the Status Bar: no place that takes clips.
    r.h.shell
        .run("view.toggle_session_arrangement", json!({}))
        .unwrap();
    r.h.settle();
    let slot = r.slot("1 MIDI: 1");
    press(&mut r.h, slot);
    let selector = r.selector(Element::ArrangementView);
    move_to(&mut r.h, slot, selector);
    let nothing = pos2(300.0, 985.0);
    move_to(&mut r.h, selector, nothing);
    release(&mut r.h, nothing);
    assert!(r.h.shell.clip_drag().is_none());
    assert_eq!(*r.doc(), before, "a drop on nothing changes nothing");
}

/// Finds a point in the other window where clips dragged from this one would
/// drop on `wanted`, by moving the pointer over its main view.
fn find_in_other(h: &mut Harness, wanted: impl Fn(&DropSpot) -> bool) -> Pos2 {
    let view = h.other_layout().main_view;
    let mut y = view.top() + 4.0;
    while y < view.bottom() {
        let mut x = view.left() + 4.0;
        while x < view.right() {
            let at = pos2(x, y);
            h.other_frame(vec![Event::PointerMoved(at)]);
            if let Some(drag) = h.shell.clip_drag()
                && let Some((_, spot)) = &drag.over
                && wanted(spot)
            {
                return at;
            }
            x += 12.0;
        }
        y += 6.0;
    }
    panic!("no place in the other window takes the clips");
}

// Spec: second-window R19, R20, R23 (I-DROP-015). With two windows open, a
// Session clip dragged from one window's Session View into the other's
// Arrangement drops there, without switching either window's view.
#[test]
fn a_session_clip_dragged_into_the_other_window_drops_there() {
    let mut r = rig();
    r.h.open_other();
    r.h.other_settle();
    assert_eq!(r.view(), MainView::Session);
    assert_eq!(
        r.h.shell.main_view_of(WindowKind::Second),
        MainView::Arrangement
    );
    let slot = r.slot("1 MIDI: 1");
    let steps = r.steps();
    press(&mut r.h, slot);
    move_to(&mut r.h, slot, slot + egui::vec2(30.0, 0.0));
    let track = r.tracks[0];
    let at = find_in_other(
        &mut r.h,
        |spot| matches!(spot, DropSpot::Arrangement { track: t, .. } if *t == track),
    );
    let DropSpot::Arrangement { at: start, .. } = r.h.shell.clip_drag().unwrap().over.unwrap().1
    else {
        unreachable!()
    };
    // The pointer is let go outside the window the drag began in.
    let outside = pos2(-40.0, 300.0);
    r.h.frame(vec![Event::PointerMoved(outside)]);
    r.h.frame(vec![Event::PointerButton {
        pos: outside,
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    }]);
    let _ = at;
    r.h.settle();
    assert!(r.h.shell.clip_drag().is_none());
    assert!(
        r.lane(0)
            .contains(&(start.get() / BAR, 2, "Slot".to_owned())),
        "{:?}",
        r.lane(0)
    );
    assert_eq!(r.steps(), steps + 1);
    assert_eq!(r.view(), MainView::Session, "neither view switched");
    assert_eq!(
        r.h.shell.main_view_of(WindowKind::Second),
        MainView::Arrangement
    );
}

// Spec: second-window R19, R21, R23. Arrangement clips dragged into the other
// window's Session View go into its slots; the clip goes back where it was,
// and the Arrangement stays as it was.
#[test]
fn an_arrangement_clip_dragged_into_the_other_window_drops_in_a_slot() {
    let mut r = rig();
    r.h.shell
        .run("view.toggle_session_arrangement", json!({}))
        .unwrap();
    r.h.open_other();
    r.h.settle();
    r.h.other_settle();
    assert_eq!(r.view(), MainView::Arrangement);
    assert_eq!(
        r.h.shell.main_view_of(WindowKind::Second),
        MainView::Session
    );
    let lane_before = r.lane(0);
    let title = pos2(x(10.5), 119.0 + 6.0);
    press(&mut r.h, title);
    move_to(&mut r.h, title, title + egui::vec2(60.0, 40.0));
    let (track, scene) = (r.tracks[1], r.scenes[2]);
    find_in_other(
        &mut r.h,
        |spot| matches!(spot, DropSpot::Session { track: t, scene: s } if *t == track && *s == scene),
    );
    let outside = pos2(-40.0, 300.0);
    r.h.frame(vec![Event::PointerMoved(outside)]);
    r.h.frame(vec![Event::PointerButton {
        pos: outside,
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    }]);
    r.h.settle();
    assert_eq!(r.slot_name(1, 2).as_deref(), Some("Lane"));
    assert_eq!(r.lane(0), lane_before, "the clip is where it was");
    assert_eq!(
        r.h.shell.editor().undo_history().next(),
        Some("Copy Clips to Session")
    );
}

// Spec: second-window R18. A clip drag over the second window's own view
// selector switches it (the windows swap).
#[test]
fn a_clip_drag_over_the_second_windows_selector_switches_its_view() {
    let mut r = rig();
    r.h.open_other();
    // The Control Bar is cut off at the window's default width, so the window
    // is widened to reach the view selector.
    r.h.other.as_mut().unwrap().size = egui::vec2(1340.0, 700.0);
    r.h.other_settle();
    let slot = r.slot("1 MIDI: 1");
    press(&mut r.h, slot);
    move_to(&mut r.h, slot, slot + egui::vec2(30.0, 0.0));
    // The second window shows the Arrangement, so the Session View's button is
    // the one that switches it.
    let button = r.h.other_layout().rect(Element::SessionView).center();
    r.h.other_frame(vec![Event::PointerMoved(button)]);
    r.h.other_frame(vec![]);
    r.h.frame(vec![]);
    assert_eq!(
        r.h.shell.main_view_of(WindowKind::Second),
        MainView::Session
    );
    assert_eq!(r.view(), MainView::Arrangement);
    r.h.key(Key::Escape, Modifiers::NONE);
    assert!(r.h.shell.clip_drag().is_none());
}

// Spec: session-view R39 to R43 (S-EDIT-001 to S-EDIT-004). Copy, Paste, Cut
// and Duplicate by key in the Session View.
#[test]
fn cut_copy_paste_and_duplicate_work_by_key_in_the_session_view() {
    let mut r = rig();
    let first = r.slot("1 MIDI: 1");
    r.h.click(first);
    r.h.key(Key::C, command());
    r.h.settle();
    assert!(matches!(
        r.h.shell.clipboard(),
        Some(bl_shell::arrangement::editing::Clipboard::Slots(_))
    ));
    let fourth = r.slot("1 MIDI: 4 (empty)");
    r.h.click(fourth);
    r.h.key(Key::V, command());
    r.h.settle();
    assert_eq!(r.slot_name(0, 3).as_deref(), Some("Slot"), "pasted");
    assert_eq!(
        r.slot_name(0, 0).as_deref(),
        Some("Slot"),
        "the source stays"
    );
    // Duplicate puts a copy below the selection.
    r.h.key(Key::D, command());
    r.h.settle();
    assert_eq!(r.slot_name(0, 4).as_deref(), Some("Slot"), "duplicated");
    // Cut takes the clip out.
    let fourth = r.slot("1 MIDI: 4");
    r.h.click(fourth);
    r.h.key(Key::X, command());
    r.h.settle();
    assert_eq!(r.slot_name(0, 3), None, "cut");
}

// Spec: second-window R22. Session clips copied in the Session View and pasted
// in the Arrangement, and Arrangement clips copied and pasted in the Session
// View, by key.
#[test]
fn copy_and_paste_by_key_carry_clips_across_the_views() {
    let mut r = rig();
    let slot = r.slot("1 MIDI: 1");
    r.h.click(slot);
    r.h.key(Key::C, command());
    r.h.settle();
    r.h.shell
        .run("view.toggle_session_arrangement", json!({}))
        .unwrap();
    r.h.settle();
    // A click in the second track's lane sets the insert marker and selects
    // the track.
    r.h.click(pos2(x(3.0), lane_y(1)));
    r.h.key(Key::V, command());
    r.h.settle();
    assert_eq!(r.lane(1), [(3, 2, "Slot".to_owned())]);
    // The Arrangement's clip, selected by its title bar, copied and pasted in
    // the Session View from the selected slot.
    r.h.click(pos2(x(10.5), 119.0 + 6.0));
    r.h.key(Key::C, command());
    r.h.settle();
    r.h.shell
        .run("view.toggle_session_arrangement", json!({}))
        .unwrap();
    r.h.settle();
    let target = r.slot("2 MIDI: 6 (empty)");
    r.h.click(target);
    r.h.key(Key::V, command());
    r.h.settle();
    assert_eq!(r.slot_name(1, 5).as_deref(), Some("Lane"));
}

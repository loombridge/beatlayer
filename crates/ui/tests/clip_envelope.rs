//! The Clip View's tabs and its Envelopes tab, headless (WP-3.16): the tab
//! keys and the tab buttons, the chooser's list, and the breakpoint gestures
//! reach the clip envelope commands agents run, each gesture as one undo
//! step. Spec: clip-envelopes R8 to R11; piano-roll R1.

use crate::common;
use bl_core::time::Ticks;
use bl_model::{AutomationTarget, ClipData, ClipId, MixerParam, SceneId};
use bl_shell::note_commands::ClipTab;
use bl_ui::arrangement::automation::{AutomationDevice, clip_choices, control_choices};
use bl_ui::clip_envelope::{axis_start, lane_rect};
use bl_ui::clip_view::{tick_at, x_of_tick};
use common::{Harness, Window};
use egui::{Event, Modifiers, PointerButton, Pos2, Rect, pos2};
use serde_json::{Value, json};

const PAN: AutomationTarget = AutomationTarget::Mixer {
    param: MixerParam::Pan,
};

/// The Option (Alt) key with Shift, for the tab keys.
fn option_shift() -> Modifiers {
    Modifiers::ALT | Modifiers::SHIFT
}

/// Option+Tab on macOS, Ctrl+Tab on Windows and Linux (S-CLIPMODE-002).
fn next_tab() -> Modifiers {
    if cfg!(target_os = "macos") {
        Modifiers::ALT
    } else {
        Modifiers::CTRL
    }
}

struct Env {
    h: Harness,
    clip: ClipId,
}

/// A Session clip on the first MIDI track, selected, with the Clip View
/// open on its Envelopes tab showing Pan.
fn set() -> Env {
    let mut h = Harness::with_title_bar(Window::Main);
    let track = h.shell.editor().document().top_level().next().unwrap();
    let scene: SceneId = h.shell.editor().document().scenes().next().unwrap().id;
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
    // Sixteenths, so a breakpoint dragged to a point lands where the test
    // expects.
    h.shell
        .run("arrangement.set_grid", json!({ "fixed": "sixteenth" }))
        .unwrap();
    h.shell
        .run("view.clip_tab", json!({ "tab": "envelopes" }))
        .unwrap();
    h.shell
        .run("view.clip_envelope", json!({ "target": PAN }))
        .unwrap();
    h.settle();
    Env { h, clip }
}

impl Env {
    fn doc(&self) -> &bl_model::Document {
        self.h.shell.editor().document()
    }

    fn length(&self) -> Ticks {
        self.doc().clip(self.clip).unwrap().length
    }

    fn run(&mut self, id: &str, args: Value) {
        self.h
            .shell
            .run(id, args)
            .unwrap_or_else(|e| panic!("{id} failed: {e}"));
        self.h.settle();
    }

    /// Puts a breakpoint on Pan's envelope.
    fn point(&mut self, at: Ticks, value: f64) {
        self.run(
            "clip.set_envelope_breakpoint",
            json!({ "clip": self.clip, "target": PAN, "at": at, "value": value }),
        );
    }

    /// The breakpoints on Pan's envelope: position and value.
    fn points(&self) -> Vec<(Ticks, f64)> {
        self.doc()
            .clip(self.clip)
            .unwrap()
            .envelope(&PAN)
            .map(|e| e.envelope.points().map(|p| (p.at, p.value)).collect())
            .unwrap_or_default()
    }

    fn steps(&self) -> usize {
        self.h.shell.editor().undo_history().len()
    }

    fn view(&mut self) -> Rect {
        let output = self.h.settle();
        let nodes = Harness::nodes(&output);
        let bounds = Harness::node(&nodes, "Clip View").bounds.unwrap();
        // The clip is a Session clip: the editor sits beside the panel
        // groups.
        bl_ui::clip_view::editor_rect(Rect::from_min_max(
            pos2(bounds.x0 as f32, bounds.y0 as f32),
            pos2(bounds.x1 as f32, bounds.y1 as f32),
        ))
    }

    /// The editor's area and the contents tick at its left edge.
    fn lane(&mut self) -> (Rect, Ticks) {
        let view = self.view();
        let clip = self.doc().clip(self.clip).unwrap();
        let ClipData::Midi(midi) = &clip.data else {
            panic!("the clip is MIDI");
        };
        (lane_rect(view), axis_start(clip, midi, Some(PAN)))
    }

    /// The point where Pan's envelope has `value` at `at`.
    fn at(&mut self, at: Ticks, value: f64) -> Pos2 {
        let (lane, start) = self.lane();
        let x = x_of_tick(lane, start, self.length(), at);
        // Pan runs from −1 at the bottom to 1 at the top.
        let y = lane.bottom() - ((value + 1.0) / 2.0) as f32 * lane.height();
        pos2(x, y)
    }

    fn node_centre(&mut self, label: &str) -> Pos2 {
        let output = self.h.settle();
        let nodes = Harness::nodes(&output);
        let b = Harness::node(&nodes, label).bounds.unwrap();
        pos2(((b.x0 + b.x1) / 2.0) as f32, ((b.y0 + b.y1) / 2.0) as f32)
    }

    fn click(&mut self, at: Pos2) {
        self.h.click(at);
        self.h.settle();
    }

    fn double_click(&mut self, at: Pos2, modifiers: Modifiers) {
        let held = Event::ModifiersChanged(modifiers);
        self.h.frame(vec![held.clone(), Event::PointerMoved(at)]);
        for _ in 0..2 {
            for pressed in [true, false] {
                self.h.frame(vec![
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
        self.h.frame(vec![Event::ModifiersChanged(Modifiers::NONE)]);
        self.h.settle();
    }

    fn drag(&mut self, from: Pos2, to: Pos2, modifiers: Modifiers) {
        let held = Event::ModifiersChanged(modifiers);
        self.h.frame(vec![held.clone(), Event::PointerMoved(from)]);
        self.h.frame(vec![
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
            self.h.frame(vec![
                held.clone(),
                Event::PointerMoved(from + (to - from) * t),
            ]);
        }
        self.h.frame(vec![
            held,
            Event::PointerButton {
                pos: to,
                button: PointerButton::Primary,
                pressed: false,
                modifiers,
            },
        ]);
        self.h.frame(vec![Event::ModifiersChanged(Modifiers::NONE)]);
        self.h.settle();
    }

    /// Checks the gesture just made is one undo step called `label`, and
    /// that undo and redo restore the set exactly.
    fn one_step(&mut self, before: &bl_model::Document, steps: usize, label: &str) {
        assert_eq!(self.steps(), steps + 1, "{label} is one undo step");
        assert_eq!(self.h.shell.editor().undo_history().next(), Some(label));
        let after = self.doc().clone();
        self.run("edit.undo", json!({}));
        assert_eq!(self.doc(), before, "undo restores the set exactly");
        self.run("edit.redo", json!({}));
        assert_eq!(*self.doc(), after, "redo restores it exactly");
    }
}

// Spec: piano-roll R22; S-CLIPMODE-001 to 004. Option+Shift+2 shows the
// Envelopes tab and Option+Shift+1 the Notes tab, and Option+Tab goes on to
// the next tab.
#[test]
fn the_tab_keys_show_and_cycle_the_tabs() {
    let mut e = set();
    e.h.shell.set_focused_area(bl_shell::Area::Detail);
    e.h.shell
        .run("view.clip_tab", json!({ "tab": "notes" }))
        .unwrap();
    e.h.key(egui::Key::Num2, option_shift());
    assert_eq!(e.h.shell.clip_view().tab(), ClipTab::Envelopes);
    e.h.key(egui::Key::Num1, option_shift());
    assert_eq!(e.h.shell.clip_view().tab(), ClipTab::Notes);
    e.h.key(egui::Key::Tab, next_tab());
    assert_eq!(e.h.shell.clip_view().tab(), ClipTab::Envelopes);
    // The reference records Option+Tab for the note editor and the sample
    // editor only, so from the Envelopes tab the other tab keys go back.
    e.h.key(egui::Key::Tab, next_tab());
    assert_eq!(e.h.shell.clip_view().tab(), ClipTab::Envelopes);
    e.h.key(egui::Key::Num1, option_shift());
    assert_eq!(e.h.shell.clip_view().tab(), ClipTab::Notes);
}

// Spec: piano-roll R22; AccessKit. The toolbar's tabs are in the tree, the
// shown one selected and MPE disabled, and clicking a tab runs
// `view.clip_tab`.
#[test]
fn the_tabs_are_accessible_and_clicking_one_shows_it() {
    let mut e = set();
    let output = e.h.settle();
    let nodes = Harness::nodes(&output);
    assert_eq!(Harness::node(&nodes, "Envelopes").toggled, Some(true));
    assert_eq!(Harness::node(&nodes, "Notes").toggled, Some(false));
    assert!(Harness::node(&nodes, "MPE").disabled);
    let notes = e.node_centre("Notes");
    e.click(notes);
    assert_eq!(e.h.shell.clip_view().tab(), ClipTab::Notes);
    let envelopes = e.node_centre("Envelopes");
    e.click(envelopes);
    assert_eq!(e.h.shell.clip_view().tab(), ClipTab::Envelopes);
}

// Spec: clip-envelopes R8. The control chooser lists the Mixer's controls
// for the clip's track, the device chooser lists the Mixer, and both reach
// the tree with the choice they hold; the Linked switch is there too.
#[test]
fn the_choosers_list_the_clips_parameters_and_the_tab_shows_the_choice() {
    let mut e = set();
    let labels: Vec<String> = control_choices(&e.h.shell, e.doc(), AutomationDevice::Mixer)
        .into_iter()
        .map(|choice| choice.label)
        .collect();
    for want in ["Volume", "Pan", "Track Activator"] {
        assert!(labels.iter().any(|label| label == want), "{labels:?}");
    }
    let output = e.h.settle();
    let nodes = Harness::nodes(&output);
    assert_eq!(Harness::node(&nodes, "Envelope Device").value, "Mixer");
    assert_eq!(Harness::node(&nodes, "Envelope Control").value, "Pan");
    assert_eq!(Harness::node(&nodes, "Clip Envelope").value, "Pan");
    assert_eq!(Harness::node(&nodes, "Linked").toggled, Some(true));
    // Another control is shown by the command the chooser runs.
    e.run(
        "view.clip_envelope",
        json!({ "target": { "type": "mixer", "param": "volume" } }),
    );
    let output = e.h.settle();
    let nodes = Harness::nodes(&output);
    assert_eq!(Harness::node(&nodes, "Envelope Control").value, "Volume");
}

// Spec: clip-envelopes R5, R9. Double-clicking empty lane adds a breakpoint
// at the clicked value on the grid, and clicking a breakpoint deletes it,
// each one undo step of the right command.
#[test]
fn double_clicking_adds_a_breakpoint_and_clicking_one_deletes_it() {
    let mut e = set();
    let step = e.h.shell.arrangement().grid_ticks(e.doc());
    let before = e.doc().clone();
    let steps = e.steps();
    let at = e.at(4 * step, 0.5);
    e.double_click(at, Modifiers::NONE);
    let points = e.points();
    assert_eq!(points.len(), 1, "{points:?}");
    assert_eq!(points[0].0, 4 * step);
    assert!((points[0].1 - 0.5).abs() < 0.05, "{points:?}");
    e.one_step(&before, steps, "Add Envelope Breakpoint");

    let before = e.doc().clone();
    let steps = e.steps();
    let on = e.at(4 * step, points[0].1);
    e.click(on);
    assert!(e.points().is_empty(), "the breakpoint is gone");
    assert!(
        e.doc().clip(e.clip).unwrap().envelope(&PAN).is_none(),
        "deleting the last breakpoint takes the envelope with it"
    );
    e.one_step(&before, steps, "Delete Envelope Breakpoint");
}

// Spec: clip-envelopes R5, R9. Dragging a breakpoint moves it in time, on
// the grid, and in value, and the whole drag is one undo step.
#[test]
fn dragging_a_breakpoint_moves_it_as_one_undo_step() {
    let mut e = set();
    let step = e.h.shell.arrangement().grid_ticks(e.doc());
    e.point(Ticks::ZERO, -0.5);
    e.point(8 * step, 0.0);
    let before = e.doc().clone();
    let steps = e.steps();
    let from = e.at(8 * step, 0.0);
    let to = e.at(12 * step, 0.5);
    e.drag(from, to, Modifiers::NONE);
    let points = e.points();
    assert_eq!(points.len(), 2, "{points:?}");
    assert_eq!(points[1].0, 12 * step, "it moved on the grid");
    assert!((points[1].1 - 0.5).abs() < 0.05, "{points:?}");
    assert_eq!(points[0], (Ticks::ZERO, -0.5), "the other is untouched");
    e.one_step(&before, steps, "Move Envelope Breakpoint");
}

// Spec: clip-envelopes R9. Esc during a drag puts the envelope back and
// records nothing.
#[test]
fn esc_during_a_drag_puts_the_breakpoint_back() {
    let mut e = set();
    let step = e.h.shell.arrangement().grid_ticks(e.doc());
    e.point(8 * step, 0.0);
    let before = e.doc().clone();
    let steps = e.steps();
    let from = e.at(8 * step, 0.0);
    let to = e.at(12 * step, 0.5);
    e.h.frame(vec![Event::PointerMoved(from)]);
    e.h.frame(vec![Event::PointerButton {
        pos: from,
        button: PointerButton::Primary,
        pressed: true,
        modifiers: Modifiers::NONE,
    }]);
    for step in 1..=4 {
        e.h.frame(vec![Event::PointerMoved(
            from + (to - from) * (step as f32 / 4.0),
        )]);
    }
    assert_ne!(*e.doc(), before, "the set follows the drag");
    e.h.key(egui::Key::Escape, Modifiers::NONE);
    assert_eq!(*e.doc(), before, "Esc puts the set back");
    assert_eq!(e.steps(), steps, "and records nothing");
}

// Spec: clip-envelopes R5, R9. Option (Alt) with a drag on a segment curves
// it, as one undo step.
#[test]
fn option_dragging_a_segment_curves_it_as_one_undo_step() {
    let mut e = set();
    let step = e.h.shell.arrangement().grid_ticks(e.doc());
    e.point(Ticks::ZERO, -0.5);
    e.point(16 * step, 0.5);
    let before = e.doc().clone();
    let steps = e.steps();
    let from = e.at(8 * step, 0.0);
    let to = from + egui::vec2(0.0, -20.0);
    e.drag(from, to, Modifiers::ALT);
    let bend = e
        .doc()
        .clip(e.clip)
        .unwrap()
        .envelope(&PAN)
        .unwrap()
        .envelope
        .point(Ticks::ZERO)
        .unwrap()
        .bend;
    assert!(bend > 0.1, "the segment is curved, {bend}");
    e.one_step(&before, steps, "Curve Envelope Segment");
}

// Spec: clip-envelopes R2, R5, R10. The Linked switch runs
// `clip.set_envelope_linked`, and an unlinked envelope's axis starts at the
// clip's start rather than its Start marker.
#[test]
fn the_linked_switch_runs_the_command_and_unlinks_the_axis() {
    let mut e = set();
    e.point(Ticks::ZERO, 0.25);
    let before = e.doc().clone();
    let steps = e.steps();
    let switch = e.node_centre("Linked");
    e.click(switch);
    let linked = e.doc().clip(e.clip).unwrap().envelope(&PAN).unwrap().linked;
    assert!(!linked, "the switch unlinked the envelope");
    assert_eq!(
        Harness::node(&Harness::nodes(&e.h.settle()), "Linked").toggled,
        Some(false)
    );
    e.one_step(&before, steps, "Link Envelope");
    // Move the Start marker: a linked envelope follows it, an unlinked one
    // doesn't.
    e.run(
        "clip.set_midi_region",
        json!({ "clip": e.clip, "start": Ticks::QUARTER }),
    );
    let (lane, start) = e.lane();
    assert_eq!(start, Ticks::ZERO);
    assert_eq!(
        tick_at(lane, start, e.length(), lane.left()),
        Ticks::ZERO,
        "the unlinked envelope's left edge is the clip's start"
    );
    e.run(
        "clip.set_envelope_linked",
        json!({ "clip": e.clip, "target": PAN, "linked": true }),
    );
    let (_, start) = e.lane();
    assert_eq!(start, Ticks::QUARTER, "linked, it follows the Start marker");
}

/// Opens the chooser `salt`, reads which of `labels` are marked with a dot
/// in the modulation colour, and closes it.
fn dotted(e: &mut Env, salt: &str, labels: &[String]) -> Vec<bool> {
    use egui::{Id, Key, Shape, accesskit::Action};
    let id = Id::new(("chooser", salt));
    let nodes = Harness::nodes(&e.h.settle());
    let b = nodes
        .iter()
        .find(|n| n.id == id.accesskit_id())
        .and_then(|n| n.bounds)
        .unwrap();
    let chooser = Rect::from_min_max(
        pos2(b.x0 as f32, b.y0 as f32),
        pos2(b.x1 as f32, b.y1 as f32),
    );
    e.h.accesskit(id, Action::Click);
    for _ in 0..12 {
        e.h.frame(vec![]);
    }
    let dot = e.h.theme.colors.modulation.0;
    let output = e.h.frame(vec![]);
    let mut shapes = Vec::new();
    fn walk(shape: &Shape, out: &mut Vec<Shape>) {
        match shape {
            Shape::Vec(shapes) => shapes.iter().for_each(|s| walk(s, out)),
            other => out.push(other.clone()),
        }
    }
    for clipped in &output.shapes {
        walk(&clipped.shape, &mut shapes);
    }
    let circles: Vec<Pos2> = shapes
        .iter()
        .filter_map(|shape| match shape {
            Shape::Circle(c) if c.fill == dot => Some(c.center),
            _ => None,
        })
        .collect();
    let rows = labels
        .iter()
        .map(|label| {
            shapes.iter().any(|shape| {
                let Shape::Text(text) = shape else {
                    return false;
                };
                let rect = text.galley.rect.translate(text.pos.to_vec2());
                text.galley.text() == label
                    && rect.min.x >= chooser.left() - 1.0
                    && rect.max.x <= chooser.right() + 1.0
                    && circles.iter().any(|c| {
                        (rect.top() - 2.0..=rect.bottom() + 2.0).contains(&c.y)
                            && (chooser.left()..=chooser.right()).contains(&c.x)
                    })
            })
        })
        .collect();
    e.h.key(Key::Escape, Modifiers::NONE);
    e.h.settle();
    rows
}

// Spec: midi-controller-data R8. A MIDI clip's device chooser lists MIDI
// Ctrl first; its control chooser lists Pitch Bend, Channel Pressure, then
// the controllers by their General MIDI names, with a dot on each that has
// an envelope; choosing MIDI Ctrl shows the first control with an envelope,
// else Pitch Bend; pitch bend is drawn centred.
#[test]
fn midi_ctrl_lists_the_controllers_and_dots_the_ones_with_envelopes() {
    use egui::{Id, Key, accesskit::Action};
    let mut e = set();
    let labels: Vec<String> = control_choices(&e.h.shell, e.doc(), AutomationDevice::MidiCtrl)
        .into_iter()
        .map(|choice| choice.label)
        .collect();
    assert_eq!(labels[0], "Pitch Bend");
    assert_eq!(labels[1], "Channel Pressure");
    assert_eq!(labels[2], "1 Modulation Wheel");
    assert_eq!(labels[3], "2 Breath Controller");
    assert!(labels.contains(&"31 Controller 31".to_owned()));
    assert!(labels.contains(&"33 LSB for CC 1".to_owned()));
    assert!(labels.contains(&"64 Sustain Pedal".to_owned()));
    assert!(labels.contains(&"74 MPE Slide (Sound Controller 5)".to_owned()));
    for missing in ["0 ", "32 ", "120 ", "127 "] {
        assert!(
            !labels.iter().any(|label| label.starts_with(missing)),
            "{missing}"
        );
    }
    assert_eq!(labels.len(), 2 + 31 + 87);

    // Controllers 2 and 1 have envelopes in the clip.
    for number in [2, 1] {
        e.run(
            "clip.set_envelope_breakpoint",
            json!({
                "clip": e.clip,
                "target": { "type": "midi.cc", "number": number },
                "at": 0,
                "value": 50.0
            }),
        );
    }
    // Choosing MIDI Ctrl shows the first control that has an envelope.
    e.h.accesskit(Id::new(("chooser", "clip-envelope-device")), Action::Focus);
    e.h.key(Key::ArrowUp, Modifiers::NONE);
    let nodes = Harness::nodes(&e.h.settle());
    assert_eq!(Harness::node(&nodes, "Envelope Device").value, "MIDI Ctrl");
    assert_eq!(
        Harness::node(&nodes, "Envelope Control").value,
        "1 Modulation Wheel"
    );
    assert_eq!(
        Harness::node(&nodes, "Clip Envelope").value,
        "1 Modulation Wheel"
    );

    let dots = dotted(&mut e, "clip-envelope-control", &labels[..6]);
    assert_eq!(dots, [false, false, true, true, false, false]);

    // With no controller envelope in the clip, MIDI Ctrl shows Pitch Bend.
    for number in [2, 1] {
        e.run(
            "clip.delete_envelope",
            json!({ "clip": e.clip, "target": { "type": "midi.cc", "number": number } }),
        );
    }
    e.run("view.clip_envelope", json!({ "target": PAN }));
    e.h.accesskit(Id::new(("chooser", "clip-envelope-device")), Action::Focus);
    e.h.key(Key::ArrowUp, Modifiers::NONE);
    let nodes = Harness::nodes(&e.h.settle());
    assert_eq!(
        Harness::node(&nodes, "Envelope Control").value,
        "Pitch Bend"
    );
}

// Spec: warp-modes R29. An audio clip's "Clip" entry lists Gain,
// Transposition and, in Beats, Sample Offset, then Detune, HiQ and the controls
// of the clip's mode.
#[test]
fn an_audio_clips_clip_entry_lists_its_modes_controls() {
    use bl_model::{AudioData, ContentHash, WarpMode};
    let names = |warp_mode| -> Vec<String> {
        let audio = AudioData {
            warp_mode,
            ..AudioData::new(ContentHash([0; 32]))
        };
        clip_choices(&audio)
            .into_iter()
            .map(|choice| choice.label)
            .collect()
    };
    assert_eq!(
        names(WarpMode::Beats),
        [
            "Gain",
            "Transposition",
            "Sample Offset",
            "Detune",
            "HiQ",
            "Preserve",
            "Transient Loop Mode",
            "Transient Envelope"
        ]
    );
    let common = ["Gain", "Transposition", "Detune", "HiQ"];
    for (mode, extra) in [
        (WarpMode::Tones, &["Grain Size"][..]),
        (WarpMode::Texture, &["Grain Size", "Flux"]),
        (WarpMode::ComplexPro, &["Formants", "Envelope"]),
        (WarpMode::Complex, &[]),
        (WarpMode::RePitch, &[]),
    ] {
        let want: Vec<&str> = common.iter().chain(extra).copied().collect();
        assert_eq!(names(mode), want, "{mode:?}");
    }
}

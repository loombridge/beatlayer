//! Several MIDI clips in the note editor, headless (WP-6.6): the loop bars,
//! the foreground clip's LED, dragging a selected bar's loop, Focus Mode on
//! its button and on N tapped and held, and what hovering shows. Spec:
//! multi-clip-editing.

use crate::common;
use bl_core::time::Ticks;
use bl_model::{Clip, ClipData, ClipId, Command, Document, MidiData, Note, TrackId};
use bl_shell::Area;
use bl_shell::files::factory_default_set;
use bl_ui::arrangement::benchmark;
use bl_ui::clip_view::{geometry, x_of_tick};
use common::{Harness, Window};
use egui::{Event, Key, Modifiers, PointerButton, Pos2, Rect, pos2};
use serde_json::json;

const BAR: i64 = Ticks::WHOLE.get();

struct Multi {
    h: Harness,
    clips: Vec<ClipId>,
}

/// Clips named "A", "B" and so on, each `(track, start, length)` in bars with
/// a note, selected in the Arrangement and open in the Clip View.
fn set(clips: &[(usize, i64, i64)]) -> Multi {
    let mut editor = factory_default_set();
    let tracks: Vec<TrackId> = editor.document().top_level().collect();
    let mut ids = Vec::new();
    for (at, (track, start, length)) in clips.iter().enumerate() {
        let id: ClipId = editor.new_id();
        let midi = MidiData {
            looping: true,
            loop_end: Some(Ticks(BAR)),
            ..MidiData::new([Note::new(
                60 + at as u8,
                Ticks(BAR / 8),
                Ticks(BAR / 8),
                100,
            )])
        };
        let mut clip = Clip::new(
            id,
            Ticks(start * BAR),
            Ticks(length * BAR),
            ClipData::Midi(midi),
        );
        clip.name = ((b'A' + at as u8) as char).to_string();
        editor
            .apply(Command::CreateClip {
                track: tracks[*track],
                clip,
            })
            .unwrap();
        ids.push(id);
    }
    editor.clear_history().unwrap();
    let mut h = Harness::with_title_bar(Window::Main);
    h.shell = benchmark::shell_with(editor);
    h.settle();
    h.shell
        .run("arrangement.select_clips", json!({ "clips": ids }))
        .unwrap();
    h.shell.run("view.toggle_clip_view", json!({})).unwrap();
    h.shell.set_focused_area(Area::Detail);
    h.settle();
    Multi { h, clips: ids }
}

impl Multi {
    fn doc(&self) -> &Document {
        self.h.shell.editor().document()
    }

    fn steps(&self) -> usize {
        self.h.shell.editor().undo_history().len()
    }

    fn midi(&self, clip: ClipId) -> MidiData {
        match &self.doc().clip(clip).unwrap().data {
            ClipData::Midi(midi) => midi.clone(),
            ClipData::Audio(_) => panic!("the clip is MIDI"),
        }
    }

    fn view(&mut self) -> Rect {
        let output = self.h.settle();
        let nodes = Harness::nodes(&output);
        let bounds = Harness::node(&nodes, "Clip View").bounds.unwrap();
        // The editor sits beside the panel groups (L-CLIP-003).
        bl_ui::clip_view::editor_rect(Rect::from_min_max(
            pos2(bounds.x0 as f32, bounds.y0 as f32),
            pos2(bounds.x1 as f32, bounds.y1 as f32),
        ))
    }

    /// The loop bar of the clip named `name`: its rectangle and whether it's
    /// marked the foreground clip.
    fn bar(&mut self, name: &str) -> (Rect, bool) {
        let output = self.h.settle();
        let nodes = Harness::nodes(&output);
        let node = Harness::node(&nodes, &format!("Loop bar: {name}"));
        let bounds = node.bounds.unwrap();
        (
            Rect::from_min_max(
                pos2(bounds.x0 as f32, bounds.y0 as f32),
                pos2(bounds.x1 as f32, bounds.y1 as f32),
            ),
            node.description == "Foreground clip",
        )
    }

    /// The note area and the shared axis the editor draws.
    fn grid_and_axis(&mut self) -> (Rect, (Ticks, Ticks)) {
        let view = self.view();
        let shown = self.h.shell.clip_view().lanes().shown();
        let state = self.h.shell.clip_view();
        let layout = state.layout(self.doc(), self.h.shell.arrangement());
        let rows = state
            .shown(self.doc(), self.h.shell.arrangement())
            .iter()
            .filter_map(|clip| self.doc().clip_track(*clip))
            .collect::<std::collections::BTreeSet<_>>()
            .len();
        let geometry = geometry(
            view,
            rows,
            self.h.main.clip_view().bar_height(),
            &self.h.main.clip_view().lane_heights(&shown),
        );
        (geometry.layout.grid, (layout.from, layout.to - layout.from))
    }

    fn click(&mut self, at: Pos2, modifiers: Modifiers) {
        let held = Event::ModifiersChanged(modifiers);
        self.h.frame(vec![held.clone(), Event::PointerMoved(at)]);
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
        self.h.frame(vec![Event::ModifiersChanged(Modifiers::NONE)]);
        self.h.settle();
    }

    fn drag(&mut self, from: Pos2, to: Pos2) {
        self.h.frame(vec![Event::PointerMoved(from)]);
        self.h.frame(vec![Event::PointerButton {
            pos: from,
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        }]);
        for step in 1..=8 {
            let t = step as f32 / 8.0;
            self.h
                .frame(vec![Event::PointerMoved(from + (to - from) * t)]);
        }
        self.h.frame(vec![Event::PointerButton {
            pos: to,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        }]);
        self.h.settle();
    }

    fn loop_bars(&self) -> Vec<ClipId> {
        self.h
            .shell
            .clip_view()
            .loop_bars(self.doc(), self.h.shell.arrangement())
    }

    fn foreground(&self) -> Option<ClipId> {
        self.h
            .shell
            .clip_view()
            .shown_clip(self.doc(), self.h.shell.arrangement())
    }
}

// Spec: multi-clip-editing R2, R3. A loop bar sits above the editor for each
// clip, and the foreground clip's carries the LED: the clip selected last,
// and after that the clip whose bar was clicked.
#[test]
fn each_clip_has_a_loop_bar_and_the_foreground_clip_wears_the_led() {
    let mut m = set(&[(0, 0, 4), (1, 0, 4)]);
    let (_, a_led) = m.bar("A");
    let (_, b_led) = m.bar("B");
    assert!(
        !a_led && b_led,
        "the clip selected last is in the foreground"
    );
    let (a, _) = m.bar("A");
    m.click(a.center(), Modifiers::NONE);
    assert_eq!(m.foreground(), Some(m.clips[0]));
    assert_eq!(m.loop_bars(), [m.clips[0]], "a clicked bar is selected");
    let (_, a_led) = m.bar("A");
    let (_, b_led) = m.bar("B");
    assert!(a_led && !b_led, "the LED follows the click");
}

// Spec: multi-clip-editing R10. Cmd-click adds a bar or takes it out, and
// Shift-click selects the bars between (I-DRAG-041).
#[test]
fn cmd_click_and_shift_click_select_bars() {
    let mut m = set(&[(0, 0, 2), (0, 2, 2), (1, 0, 4)]);
    let [a, b, c] = [m.clips[0], m.clips[1], m.clips[2]];
    let command = if cfg!(target_os = "macos") {
        Modifiers::MAC_CMD | Modifiers::COMMAND
    } else {
        Modifiers::CTRL | Modifiers::COMMAND
    };
    let (bar_a, _) = m.bar("A");
    m.click(bar_a.center(), Modifiers::NONE);
    assert_eq!(m.loop_bars(), [a]);
    let (bar_b, _) = m.bar("B");
    m.click(bar_b.center(), command);
    assert_eq!(m.loop_bars(), [a, b]);
    m.click(bar_b.center(), command);
    assert_eq!(
        m.loop_bars(),
        [a],
        "Cmd-click on a selected bar takes it out"
    );
    let (bar_c, _) = m.bar("C");
    m.click(bar_c.center(), Modifiers::SHIFT);
    assert_eq!(
        m.loop_bars(),
        [a, b, c],
        "Shift-click selects the bars between"
    );
    // Clicking in the note area lets go of the bars.
    let (grid, _) = m.grid_and_axis();
    m.click(pos2(grid.center().x, grid.top() + 4.0), Modifiers::NONE);
    assert!(m.loop_bars().is_empty());
}

// Spec: multi-clip-editing R10. A drag on one of several selected bars
// moves the same marker of every selected bar by the same amount, and the
// whole drag is one undo step.
#[test]
fn dragging_one_of_several_selected_bars_moves_every_loop_in_one_step() {
    let mut m = set(&[(0, 0, 4), (1, 0, 4)]);
    let [a, b] = [m.clips[0], m.clips[1]];
    m.h.shell
        .run("clip.set_midi_loop", json!({ "clips": [{ "clip": a, "start": 0, "end": BAR }, { "clip": b, "start": 0, "end": 2 * BAR }] }))
        .unwrap();
    m.h.shell
        .run("clip_view.select_loop_bars", json!({ "clips": [a, b] }))
        .unwrap();
    m.h.settle();
    let steps = m.steps();
    let (grid, axis) = m.grid_and_axis();
    let (bar_a, _) = m.bar("A");
    let x = |tick: i64| x_of_tick(grid, axis.0, axis.1, Ticks(tick));
    let y = bar_a.center().y;
    m.drag(pos2(x(BAR), y), pos2(x(2 * BAR), y));
    assert_eq!(m.midi(a).loop_end, Some(Ticks(2 * BAR)));
    assert_eq!(
        m.midi(b).loop_end,
        Some(Ticks(3 * BAR)),
        "the other bar's end moves by the same amount"
    );
    assert_eq!(m.steps(), steps + 1, "one undo step for the drag");
    m.h.shell.run("edit.undo", json!({})).unwrap();
    assert_eq!(m.midi(a).loop_end, Some(Ticks(BAR)));
    assert_eq!(m.midi(b).loop_end, Some(Ticks(2 * BAR)));
}

// Spec: multi-clip-editing R13. The Focus button switches Focus Mode, and N
// switches it when tapped and while held (I-HOLD-003, S-NOTE-040).
#[test]
fn the_focus_button_and_n_switch_focus_mode() {
    let mut m = set(&[(0, 0, 4), (1, 0, 4)]);
    let focus = |m: &mut Multi| m.h.shell.clip_view().focus_mode();
    assert!(!focus(&mut m), "Focus Mode is off to begin with");
    let output = m.h.settle();
    let nodes = Harness::nodes(&output);
    let button = Harness::node(&nodes, "Focus").bounds.unwrap();
    let centre = pos2(
        ((button.x0 + button.x1) / 2.0) as f32,
        ((button.y0 + button.y1) / 2.0) as f32,
    );
    m.click(centre, Modifiers::NONE);
    assert!(focus(&mut m), "the Focus button switches it on");
    m.click(centre, Modifiers::NONE);
    assert!(!focus(&mut m));
    // N tapped switches it, and again.
    m.h.key(Key::N, Modifiers::NONE);
    m.h.settle();
    assert!(focus(&mut m));
    m.h.key(Key::N, Modifiers::NONE);
    m.h.settle();
    assert!(!focus(&mut m));
    // N held switches it while held: on, then back when released.
    m.h.frame(vec![Event::Key {
        key: Key::N,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: Modifiers::NONE,
    }]);
    assert!(focus(&mut m));
    for _ in 0..45 {
        m.h.frame(vec![]);
    }
    m.h.frame(vec![Event::Key {
        key: Key::N,
        physical_key: None,
        pressed: false,
        repeat: false,
        modifiers: Modifiers::NONE,
    }]);
    assert!(!focus(&mut m), "released after a hold, it goes back");
    // No undo step along the way.
    assert_eq!(m.steps(), 0);
}

// Spec: multi-clip-editing R14. With Focus Mode on, clicking another clip's
// bar makes it the active clip, and hovering an inactive bar shows its
// notes in its colour.
#[test]
fn in_focus_mode_a_clicked_bar_becomes_active_and_hovering_reveals_a_clip() {
    let mut m = set(&[(0, 0, 4), (1, 0, 4)]);
    let [a, b] = [m.clips[0], m.clips[1]];
    m.h.shell
        .run("clip_view.focus_mode", json!({ "on": true }))
        .unwrap();
    m.h.settle();
    assert_eq!(m.foreground(), Some(b));
    let (note_a, note_b) = {
        let theme = &m.h.theme;
        let track = |clip| m.doc().clip_track(clip).unwrap();
        (
            bl_ui::theme::note_colour(
                bl_ui::arrangement::scene::track_colour(theme, track(a)),
                100,
            ),
            bl_ui::theme::note_colour(
                bl_ui::arrangement::scene::track_colour(theme, track(b)),
                100,
            ),
        )
    };
    let count = |output: &egui::FullOutput, colour: egui::Color32| {
        output
            .shapes
            .iter()
            .filter(
                |clipped| matches!(&clipped.shape, egui::Shape::Rect(rect) if rect.fill == colour),
            )
            .count()
    };
    let (bar_a, _) = m.bar("A");
    // Pointer off the bars: the inactive clip A's notes are grey.
    m.h.frame(vec![Event::PointerMoved(pos2(5.0, 5.0))]);
    let plain = m.h.settle();
    assert!(
        count(&plain, note_b) > 0,
        "the active clip's notes are in its colour"
    );
    assert_eq!(count(&plain, note_a), 0, "the others are grey");
    // Hovering A's bar shows its notes in its colour.
    m.h.frame(vec![Event::PointerMoved(bar_a.center())]);
    let hovered = m.h.settle();
    assert!(
        count(&hovered, note_a) > 0,
        "hovering a bar reveals its clip"
    );
    // Clicking A's bar makes it the active clip.
    m.click(bar_a.center(), Modifiers::NONE);
    assert_eq!(m.foreground(), Some(a));
    assert!(m.h.shell.clip_view().focus_mode());
}

// Spec: multi-clip-editing R7. A drag of a selected note moves every
// selected note of every clip, as one undo step.
#[test]
fn dragging_a_selected_note_moves_the_selection_in_every_clip_in_one_step() {
    let mut m = set(&[(0, 0, 4), (1, 0, 4)]);
    let [a, b] = [m.clips[0], m.clips[1]];
    m.h.shell
        .run(
            "clip_view.select_notes",
            json!({ "clips": [{ "clip": a, "notes": [0] }, { "clip": b, "notes": [0] }] }),
        )
        .unwrap();
    m.h.settle();
    let steps = m.steps();
    let (grid, axis) = m.grid_and_axis();
    let top = m.h.main.clip_view().top();
    let y = bl_ui::clip_view::y_of_key(grid, top, 61) + bl_ui::clip_view::KEY_HEIGHT / 2.0;
    // Clip B's note (key 61) starts an eighth of a bar in.
    let from = pos2(
        x_of_tick(grid, axis.0, axis.1, Ticks(BAR / 8 + BAR / 16)),
        y,
    );
    let to = pos2(from.x + grid.width() / 4.0, y);
    m.drag(from, to);
    let moved = |clip| match &m.doc().clip(clip).unwrap().data {
        ClipData::Midi(midi) => midi.notes.iter().next().unwrap().start,
        ClipData::Audio(_) => panic!("MIDI"),
    };
    assert_eq!(
        moved(a),
        moved(b),
        "both clips' notes moved by the same amount"
    );
    assert!(moved(a) > Ticks(BAR / 8));
    assert_eq!(m.steps(), steps + 1, "one undo step for both clips");
}

// Spec: multi-clip-editing R11, S-LOOP-012. A click on a single clip's loop
// brace selects it, and Cmd-D then duplicates the loop; before, Cmd-D
// duplicates the notes.
#[test]
fn cmd_d_on_a_single_clips_selected_loop_brace_duplicates_the_loop() {
    let mut m = set(&[(0, 0, 8)]);
    let clip = m.clips[0];
    let view = m.view();
    let shown = m.h.shell.clip_view().lanes().shown();
    let layout = bl_ui::note_lanes::note_layout(view, &m.h.main.clip_view().lane_heights(&shown));
    let strip = bl_ui::clip_view::strip_rect(view);
    let length = m.doc().clip(clip).unwrap().length;
    let brace = pos2(
        x_of_tick(layout.grid, Ticks::ZERO, length, Ticks(BAR / 2)),
        strip.bottom() - 3.0,
    );
    assert!(m.loop_bars().is_empty());
    m.click(brace, Modifiers::NONE);
    assert_eq!(m.loop_bars(), [clip], "the brace is selected");
    let steps = m.steps();
    let command = if cfg!(target_os = "macos") {
        Modifiers::MAC_CMD | Modifiers::COMMAND
    } else {
        Modifiers::CTRL | Modifiers::COMMAND
    };
    m.h.key(Key::D, command);
    m.h.settle();
    assert_eq!(m.midi(clip).loop_end, Some(Ticks(2 * BAR)));
    assert_eq!(
        m.midi(clip).notes.len(),
        2,
        "the notes in the loop are copied"
    );
    assert_eq!(m.steps(), steps + 1);
    assert_eq!(
        m.h.shell.editor().undo_history().next(),
        Some("Duplicate Loop")
    );
}

// Spec: multi-clip-editing R2. Dragging up just above the title bar makes the
// loop bars taller, and down shorter; it's view state, no undo step.
#[test]
fn dragging_above_the_title_bar_changes_the_height_of_the_loop_bars() {
    let mut m = set(&[(0, 0, 4), (1, 0, 4)]);
    let view = m.view();
    let shown = m.h.shell.clip_view().lanes().shown();
    let before = m.h.main.clip_view().bar_height();
    let handle = geometry(view, 2, before, &m.h.main.clip_view().lane_heights(&shown)).handle;
    m.drag(handle.center(), handle.center() + egui::vec2(0.0, -8.0));
    let taller = m.h.main.clip_view().bar_height();
    assert!(
        taller > before,
        "dragging up makes the bars taller: {taller}"
    );
    let handle = geometry(view, 2, taller, &m.h.main.clip_view().lane_heights(&shown)).handle;
    m.drag(handle.center(), handle.center() + egui::vec2(0.0, 6.0));
    assert!(m.h.main.clip_view().bar_height() < taller);
    assert_eq!(m.steps(), 0);
}

// Spec: multi-clip-editing R8. In Draw Mode a note drawn on the axis belongs
// to the clip that plays where its start lies.
#[test]
fn a_drawn_note_belongs_to_the_clip_its_start_lies_in() {
    let mut m = set(&[(0, 0, 2), (1, 2, 2)]);
    let [a, b] = [m.clips[0], m.clips[1]];
    // The clip selected last is the foreground clip: A.
    m.h.shell
        .run("arrangement.select_clips", json!({ "clips": [b, a] }))
        .unwrap();
    m.h.shell.run("view.draw_mode", json!({})).unwrap();
    m.h.settle();
    assert_eq!(m.foreground(), Some(a));
    let (grid, axis) = m.grid_and_axis();
    let top = m.h.main.clip_view().top();
    let at = pos2(
        x_of_tick(grid, axis.0, axis.1, Ticks(3 * BAR + BAR / 2)),
        bl_ui::clip_view::y_of_key(grid, top, 60) + bl_ui::clip_view::KEY_HEIGHT / 2.0,
    );
    m.click(at, Modifiers::NONE);
    assert_eq!(m.midi(a).notes.len(), 1, "A doesn't play there");
    let drawn: Vec<Ticks> = m.midi(b).notes.iter().map(|note| note.start).collect();
    assert_eq!(drawn.len(), 2, "the note went to B");
    assert!(
        drawn.iter().all(|start| *start < Ticks(BAR)),
        "B's loop is on its second pass there, so its start is inside the loop: {drawn:?}"
    );
    assert_eq!(m.steps(), 1);
}

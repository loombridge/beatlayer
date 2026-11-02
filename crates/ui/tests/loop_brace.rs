//! The loop brace in the Arrangement, headless (WP-6.73): clicking its bar
//! selects it, dragging its ends or bar moves it, and its keys move and
//! resize it, none of it an undo step. Spec: transport R31 to R34.

use crate::common::{Harness, Window};
use bl_core::time::Ticks;
use bl_shell::arrangement_commands::ArrangementState;
use egui::{Event, Key, Modifiers, PointerButton, pos2, vec2};
use serde_json::json;

const BAR: i64 = Ticks::WHOLE.get();
const BEAT: i64 = BAR / 4;

fn state(h: &mut Harness) -> ArrangementState {
    serde_json::from_value(h.shell.run("arrangement.state", json!({})).unwrap()).unwrap()
}

/// The brace as `transport.state` reads it back: start and length.
fn brace(h: &mut Harness) -> (i64, i64) {
    let read = h.shell.run("transport.state", json!({})).unwrap();
    (
        read["loop_start"].as_i64().unwrap(),
        read["loop_length"].as_i64().unwrap(),
    )
}

fn steps(h: &Harness) -> usize {
    h.shell.editor().undo_history().len()
}

fn cmd() -> Modifiers {
    if cfg!(target_os = "macos") {
        Modifiers::MAC_CMD | Modifiers::COMMAND
    } else {
        Modifiers::CTRL | Modifiers::COMMAND
    }
}

/// A harness in the Arrangement with the brace at `start` for `length`, the
/// grid fixed at `grid`, and the undo history as the set opened.
fn open(start: i64, length: i64, grid: &str) -> Harness {
    let mut h = Harness::with_title_bar(Window::Main);
    h.settle();
    h.shell
        .run(
            "transport.set_loop",
            json!({ "start": start, "length": length }),
        )
        .unwrap();
    h.shell
        .run(
            "arrangement.set_grid",
            json!({ "fixed": grid, "snap": true }),
        )
        .unwrap();
    h.settle();
    h
}

/// Where the brace's ruler row is: the x of a position, and the y of its bar.
fn x_of(h: &Harness, at: i64) -> f32 {
    let shell = bl_ui::layout::ShellLayout::compute(
        h.size - vec2(0.0, bl_ui::layout::TITLE_BAR),
        true,
        true,
    )
    .translate(vec2(0.0, bl_ui::layout::TITLE_BAR));
    let doc = h.shell.editor().document();
    let layout = bl_ui::layout::ArrangementLayout::compute(
        shell.main_view,
        h.shell.arrangement().overview(),
        doc.returns().len() + 1,
    );
    layout.beat_ruler.left() + h.shell.arrangement().horizontal(doc).x(Ticks(at)) as f32
}

fn y_of(h: &Harness) -> f32 {
    let shell = bl_ui::layout::ShellLayout::compute(
        h.size - vec2(0.0, bl_ui::layout::TITLE_BAR),
        true,
        true,
    )
    .translate(vec2(0.0, bl_ui::layout::TITLE_BAR));
    let doc = h.shell.editor().document();
    bl_ui::layout::ArrangementLayout::compute(
        shell.main_view,
        h.shell.arrangement().overview(),
        doc.returns().len() + 1,
    )
    .beat_ruler
    .bottom()
        - 3.0
}

/// Drags along the brace's row from a position (and some points beside it)
/// to another, with the primary button.
fn drag(h: &mut Harness, from: (i64, f32), to: (i64, f32)) {
    let y = y_of(h);
    let (from, to) = (
        pos2(x_of(h, from.0) + from.1, y),
        pos2(x_of(h, to.0) + to.1, y),
    );
    h.frame(vec![Event::PointerMoved(from)]);
    h.frame(vec![Event::PointerButton {
        pos: from,
        button: PointerButton::Primary,
        pressed: true,
        modifiers: Modifiers::NONE,
    }]);
    for step in 1..=6 {
        h.frame(vec![Event::PointerMoved(
            from + (to - from) * (step as f32 / 6.0),
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

/// Clicks the brace's bar.
fn click_bar(h: &mut Harness) {
    let (start, length) = brace(h);
    let at = pos2(x_of(h, start + length / 2), y_of(h));
    h.click(at);
    h.settle();
}

// Spec: transport R32 (I-CLICK-017).
#[test]
fn clicking_the_bar_selects_the_brace_and_its_span() {
    let mut h = open(2 * BAR, 4 * BAR, "quarter");
    let before = steps(&h);
    click_bar(&mut h);
    let selected = state(&mut h);
    assert!(selected.loop_brace_selected);
    let time = selected.time_selection.unwrap();
    assert_eq!((time.start.get(), time.end.get()), (2 * BAR, 6 * BAR));
    assert_eq!(selected.insert_marker.get(), 2 * BAR);
    assert_eq!(
        selected.selected_tracks.len(),
        h.shell.editor().document().track_order().len()
    );
    assert!(
        !h.shell
            .editor()
            .document()
            .settings()
            .arrangement_loop
            .enabled
    );
    assert_eq!(steps(&h), before);
}

// Spec: transport R33 (S-LOOP-007 to S-LOOP-010, S-ARR-012).
#[test]
fn each_key_moves_or_resizes_the_brace_at_every_grid() {
    // (grid, step in ticks)
    for (grid, step) in [("eighth", BEAT / 2), ("quarter", BEAT), ("half", 2 * BEAT)] {
        let (start, length) = (2 * BAR, 4 * BAR);
        let mut h = open(start, length, grid);
        let before = steps(&h);
        click_bar(&mut h);
        // (key, command held, start, length, insert marker)
        let keys = [
            (Key::ArrowRight, false, start + step, length, start + step),
            (Key::ArrowLeft, false, start, length, start),
            (Key::ArrowDown, false, 0, length, 0),
            (Key::ArrowUp, false, length, length, length),
            (
                Key::ArrowDown,
                true,
                length,
                length / 2,
                length + length / 2,
            ),
            (Key::ArrowUp, true, length, length, 2 * length),
            (
                Key::ArrowRight,
                true,
                length,
                length + step,
                2 * length + step,
            ),
            (Key::ArrowLeft, true, length, length, 2 * length),
        ];
        for (key, command, start, length, marker) in keys {
            h.key(key, if command { cmd() } else { Modifiers::NONE });
            h.settle();
            assert_eq!(brace(&mut h), (start, length), "{grid} {key:?} {command}");
            let now = state(&mut h);
            assert_eq!(now.insert_marker.get(), marker, "{grid} {key:?} {command}");
            let time = now.time_selection.unwrap();
            assert_eq!(
                (time.start.get(), time.end.get()),
                (start, start + length),
                "{grid} {key:?} {command}"
            );
        }
        assert_eq!(steps(&h), before, "{grid}: no undo step");
    }
}

// Spec: transport R33.
#[test]
fn down_at_the_songs_start_stops_at_1_1_1() {
    let mut h = open(BAR, 4 * BAR, "quarter");
    click_bar(&mut h);
    h.key(Key::ArrowDown, Modifiers::NONE);
    h.settle();
    assert_eq!(brace(&mut h), (0, 4 * BAR));
    h.key(Key::ArrowDown, Modifiers::NONE);
    h.settle();
    assert_eq!(brace(&mut h), (0, 4 * BAR));
}

// Spec: transport R34 (I-DRAG-019).
#[test]
fn dragging_an_end_or_the_bar_sets_the_brace_on_the_grid_in_no_undo_step() {
    for (grid, step) in [("eighth", BEAT / 2), ("quarter", BEAT), ("half", 2 * BEAT)] {
        let mut h = open(2 * BAR, 4 * BAR, grid);
        let before = steps(&h);
        // The right end, to a line a little before it: the end snaps to it.
        let end = 6 * BAR;
        let target = 8 * BAR + 3 * step;
        drag(&mut h, (end, -1.0), (target + step / 4, 0.0));
        assert_eq!(brace(&mut h), (2 * BAR, target - 2 * BAR), "{grid} end");
        assert_eq!(state(&mut h).insert_marker.get(), target, "{grid} end");
        // The left end.
        let start = 3 * BAR + 2 * step;
        drag(&mut h, (2 * BAR, 1.0), (start - step / 4, 0.0));
        assert_eq!(brace(&mut h), (start, target - start), "{grid} start");
        // The bar keeps the length.
        let length = target - start;
        let moved = 4 * BAR + step;
        drag(
            &mut h,
            (start + length / 2, 0.0),
            (start + length / 2 + (moved - start) + step / 4, 0.0),
        );
        assert_eq!(brace(&mut h), (moved, length), "{grid} bar");
        let time = state(&mut h).time_selection.unwrap();
        assert_eq!((time.start.get(), time.end.get()), (moved, moved + length));
        assert_eq!(steps(&h), before, "{grid}: no undo step");
    }
}

#[test]
fn dragging_with_snapping_off_sets_the_end_where_the_pointer_is() {
    let mut h = open(0, 4 * BAR, "quarter");
    h.shell
        .run("arrangement.set_grid", json!({ "snap": false }))
        .unwrap();
    h.settle();
    let target = 6 * BAR + BEAT / 3;
    drag(&mut h, (4 * BAR, -1.0), (target, 0.0));
    let (_, length) = brace(&mut h);
    assert_ne!(length % BEAT, 0, "the end isn't on the grid: {length}");
    assert!((length - target).abs() < BAR / 8, "{length} near {target}");
}

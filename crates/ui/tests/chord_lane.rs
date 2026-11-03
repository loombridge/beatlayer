//! The chord lane, headless (WP-6.48): View ▸ Chord Lane showing it above the
//! first track, drawing, moving, resizing and deleting a region with the
//! pointer and the grid, its context menu setting the scale and the
//! transposition, and a MIDI track's Follow Chord Lane switch.
//!
//! Spec: chord-lane R1 to R5.

use crate::common;
use bl_core::time::Ticks;
use bl_model::ChordRegion;
use bl_shell::arrangement_commands::ArrangementState;
use bl_shell::menus::{MenuPlatform, menu_bar};
use bl_ui::keymap::Platform;
use bl_ui::layout::{ArrangementLayout, CHORD_LANE};
use bl_ui::menus::item_state;
use common::{Harness, Node, Window};
use egui::accesskit::Role;
use egui::{Event, FullOutput, Key, Modifiers, PointerButton, Pos2, pos2};
use serde_json::json;

const BAR: i64 = Ticks::WHOLE.get();

fn state(h: &mut Harness) -> ArrangementState {
    serde_json::from_value(h.shell.run("arrangement.state", json!({})).unwrap()).unwrap()
}

fn shown() -> Harness {
    let mut h = Harness::with_title_bar(Window::Main);
    h.shell
        .run("view.chord_lane", json!({ "on": true }))
        .unwrap();
    h.settle();
    h
}

/// The Arrangement's layout in a harness that draws Live's title bar, as the
/// view has it now.
fn layout(h: &Harness) -> ArrangementLayout {
    let shell = bl_ui::layout::ShellLayout::compute(
        h.size - egui::vec2(0.0, bl_ui::layout::TITLE_BAR),
        true,
        true,
    )
    .translate(egui::vec2(0.0, bl_ui::layout::TITLE_BAR));
    let doc = h.shell.editor().document();
    let pinned = doc.returns().len() + 1;
    let layout =
        ArrangementLayout::compute(shell.main_view, h.shell.arrangement().overview(), pinned);
    let layout = if doc.settings().time_signatures.has_changes() {
        layout.with_signature_lane()
    } else {
        layout
    };
    if h.shell.arrangement().chord_lane() {
        layout.with_chord_lane()
    } else {
        layout
    }
}

/// The x of `at` across the lane.
fn x_of(h: &Harness, at: Ticks) -> f32 {
    let horizontal = h
        .shell
        .arrangement()
        .horizontal(h.shell.editor().document());
    layout(h).scrub.left() + horizontal.x(at) as f32
}

fn regions(h: &Harness) -> Vec<ChordRegion> {
    h.shell.editor().document().chord_lane().to_vec()
}

fn steps(h: &Harness) -> usize {
    h.shell.editor().undo_history().len()
}

fn middle(h: &Harness) -> f32 {
    layout(h).chord_lane.unwrap().center().y
}

/// Presses at `from`, drags to `to` in `n` steps and releases.
fn drag(h: &mut Harness, from: Pos2, to: Pos2, n: usize) {
    h.frame(vec![
        Event::PointerMoved(from),
        Event::PointerButton {
            pos: from,
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        },
    ]);
    for step in 1..=n {
        let at = from + (to - from) * (step as f32 / n as f32);
        h.frame(vec![Event::PointerMoved(at)]);
    }
    h.frame(vec![Event::PointerButton {
        pos: to,
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    }]);
    h.settle();
}

/// A region drawn from bar `from` to bar `to`.
fn draw(h: &mut Harness, from: i64, to: i64) {
    let y = middle(h);
    let (a, b) = (
        x_of(h, Ticks(from * BAR)) + 1.0,
        x_of(h, Ticks(to * BAR)) + 1.0,
    );
    drag(h, pos2(a, y), pos2(b, y), 6);
}

fn right_click(h: &mut Harness, at: Pos2) -> FullOutput {
    for pressed in [true, false] {
        h.frame(vec![
            Event::PointerMoved(at),
            Event::PointerButton {
                pos: at,
                button: PointerButton::Secondary,
                pressed,
                modifiers: Modifiers::NONE,
            },
        ]);
    }
    h.settle()
}

fn click_node(h: &mut Harness, node: &Node) {
    let b = node.bounds.unwrap();
    h.click(pos2(
        ((b.x0 + b.x1) / 2.0) as f32,
        ((b.y0 + b.y1) / 2.0) as f32,
    ));
    h.settle();
}

fn choose(h: &mut Harness, label: &str) {
    let nodes = Harness::nodes(&h.settle());
    let node = nodes
        .iter()
        .find(|n| n.role == Role::Button && plain(&n.label) == label)
        .unwrap_or_else(|| panic!("no menu item {label:?}"))
        .clone();
    click_node(h, &node);
}

/// A menu item's label without its tick and its submenu arrow.
fn plain(label: &str) -> &str {
    label.trim_start_matches("✔ ").trim_end_matches(" ⏵")
}

fn menu_item(h: &Harness, row: &str) -> bl_ui::menus::ItemState {
    let menus = menu_bar(MenuPlatform::current());
    let item = menus
        .iter()
        .flat_map(bl_shell::menus::Menu::walk)
        .find(|item| item.row == row)
        .unwrap();
    item_state(item, &h.shell, &h.keymap, Platform::current())
}

#[test]
fn the_lane_is_hidden_in_a_new_set_and_shows_from_the_view_menu_above_the_first_track() {
    // Spec: chord-lane R1; layout L-ARR-024.
    let mut h = Harness::with_title_bar(Window::Main);
    h.settle();
    let without = layout(&h);
    assert!(without.chord_lane.is_none());
    assert_eq!(menu_item(&h, "IMP-33").checked, Some(false));
    assert!(!state(&mut h).chord_lane);
    // The View menu's item runs `view.chord_lane`, with no undo step.
    let before = steps(&h);
    let item = menu_item(&h, "IMP-33");
    assert_eq!(item.label, "Chord Lane");
    h.shell
        .invoke(
            &item.invocation.expect("the item runs a command").id,
            json!({}),
        )
        .unwrap();
    h.settle();
    assert_eq!(steps(&h), before, "showing the lane is no undo step");
    assert_eq!(menu_item(&h, "IMP-33").checked, Some(true));
    let with = layout(&h);
    let lane = with.chord_lane.expect("the lane shows");
    assert_eq!(lane.height(), CHORD_LANE);
    // The track lanes and their headers start under it.
    assert_eq!(lane.bottom(), with.lanes.top());
    assert_eq!(with.lanes.top() - without.lanes.top(), CHORD_LANE);
    assert_eq!(with.headers.top() - without.headers.top(), CHORD_LANE);
    assert!(lane.top() >= with.scrub.bottom());
    assert!(state(&mut h).chord_lane);
    // The header column's cell names it.
    assert!(
        with.rows().iter().any(|row| row.id == "L-ARR-024"),
        "the layout lists the lane"
    );
    // Hiding it again leaves the regions.
    draw(&mut h, 1, 3);
    h.shell
        .run("view.chord_lane", json!({ "on": false }))
        .unwrap();
    h.settle();
    assert!(layout(&h).chord_lane.is_none());
    assert_eq!(regions(&h).len(), 1);
}

#[test]
fn drawing_in_the_lane_makes_a_region_on_the_grid_in_one_undo_step() {
    // Spec: chord-lane R3.
    let mut h = shown();
    let before = steps(&h);
    draw(&mut h, 2, 4);
    assert_eq!(steps(&h), before + 1, "the whole drag is one undo step");
    let all = regions(&h);
    assert_eq!(all.len(), 1);
    let (start, end) = (all[0].start, all[0].end);
    assert!(end > start);
    let view = h.shell.arrangement();
    let doc = h.shell.editor().document();
    for edge in [start, end] {
        assert_eq!(
            view.snap_drag(doc, edge, false),
            edge,
            "{edge:?} is on the grid"
        );
    }
    // It reads its scale and transposition, and it is the selected one.
    assert_eq!(all[0].name(), "No Scale +0");
    assert_eq!(state(&mut h).selected_chord_region, Some(all[0].id));
    let nodes = Harness::nodes(&h.settle());
    assert!(
        nodes
            .iter()
            .any(|n| n.label == "No Scale +0" && n.role == Role::Button)
    );
    // Undo takes it away.
    h.shell.run("edit.undo", json!({})).unwrap();
    assert!(regions(&h).is_empty());
}

#[test]
fn a_region_is_moved_and_resized_with_the_pointer_one_undo_step_each() {
    // Spec: chord-lane R3.
    let mut h = shown();
    draw(&mut h, 2, 4);
    let first = regions(&h)[0];
    let (y, before) = (middle(&h), steps(&h));
    // Dragging the body moves it, keeping its length.
    let from = pos2(x_of(&h, first.start) + 40.0, y);
    drag(&mut h, from, from + egui::vec2(120.0, 0.0), 6);
    let moved = regions(&h)[0];
    assert_eq!(steps(&h), before + 1);
    assert!(moved.start > first.start);
    assert_eq!(moved.end - moved.start, first.end - first.start);
    // Dragging the end edge makes it longer, the start edge earlier.
    let from = pos2(x_of(&h, moved.end) - 2.0, y);
    drag(&mut h, from, from + egui::vec2(80.0, 0.0), 6);
    let longer = regions(&h)[0];
    assert_eq!(steps(&h), before + 2);
    assert_eq!(longer.start, moved.start);
    assert!(longer.end > moved.end);
    let from = pos2(x_of(&h, longer.start) + 2.0, y);
    drag(&mut h, from, from - egui::vec2(60.0, 0.0), 6);
    let earlier = regions(&h)[0];
    assert_eq!(steps(&h), before + 3);
    assert!(earlier.start < longer.start);
    assert_eq!(earlier.end, longer.end);
}

#[test]
fn a_region_cannot_be_dragged_over_another() {
    // Spec: chord-lane R2, R3.
    let mut h = shown();
    draw(&mut h, 2, 3);
    draw(&mut h, 6, 7);
    let all = regions(&h);
    assert_eq!(all.len(), 2);
    let (first, second) = (all[0], all[1]);
    let y = middle(&h);
    // Dragged in small steps to where it would overlap the first, the second
    // stops at its edge.
    let from = pos2(x_of(&h, second.start) + 20.0, y);
    let into = pos2(x_of(&h, Ticks(BAR * 5 / 2)) + 20.0, y);
    drag(&mut h, from, into, 80);
    let after = regions(&h);
    assert_eq!(after.len(), 2);
    assert_eq!(after[0], first);
    assert!(
        after[1].start >= first.end,
        "{:?} stays clear of {:?}",
        after[1],
        first
    );
    assert_eq!(after[1].end - after[1].start, second.end - second.start);
    // Drawing across an existing region adds nothing.
    let count = steps(&h);
    draw(&mut h, 1, 4);
    assert_eq!(regions(&h).len(), 2);
    assert_eq!(steps(&h), count, "a refused drawing is no undo step");
}

#[test]
fn clicking_selects_a_region_and_delete_deletes_it() {
    // Spec: chord-lane R3.
    let mut h = shown();
    draw(&mut h, 2, 4);
    draw(&mut h, 6, 8);
    let all = regions(&h);
    let y = middle(&h);
    h.click(pos2(x_of(&h, all[0].start) + 30.0, y));
    h.settle();
    assert_eq!(state(&mut h).selected_chord_region, Some(all[0].id));
    // Clicking the empty lane deselects.
    h.click(pos2(x_of(&h, Ticks(12 * BAR)), y));
    h.settle();
    assert_eq!(state(&mut h).selected_chord_region, None);
    h.click(pos2(x_of(&h, all[1].start) + 30.0, y));
    h.settle();
    let before = steps(&h);
    h.key(Key::Delete, Modifiers::NONE);
    h.settle();
    assert_eq!(regions(&h), [all[0]]);
    assert_eq!(steps(&h), before + 1);
    assert_eq!(state(&mut h).selected_chord_region, None);
    h.shell.run("edit.undo", json!({})).unwrap();
    assert_eq!(regions(&h).len(), 2);
}

#[test]
fn the_region_s_menu_sets_its_scale_and_its_transposition() {
    // Spec: chord-lane R4.
    let mut h = shown();
    draw(&mut h, 2, 4);
    let region = regions(&h)[0];
    let at = pos2(x_of(&h, region.start) + 30.0, middle(&h));
    let before = steps(&h);
    let nodes = Harness::nodes(&right_click(&mut h, at));
    for label in ["No Scale", "Root", "Scale", "Transposition", "Delete"] {
        assert!(
            nodes.iter().any(|n| plain(&n.label) == label),
            "the menu holds {label:?}: {:?}",
            nodes.iter().map(|n| (&n.label, n.role)).collect::<Vec<_>>()
        );
    }
    // The first choice of a root names the scale too: D, then Dorian.
    choose(&mut h, "Root");
    choose(&mut h, "D");
    let held = regions(&h)[0];
    assert!(held.scale.enabled);
    assert_eq!(held.scale.root, 2);
    assert_eq!(steps(&h), before + 1);
    right_click(&mut h, at);
    choose(&mut h, "Scale");
    choose(&mut h, "Dorian");
    // Up an octave, then down 10 in steps: +2.
    right_click(&mut h, at);
    choose(&mut h, "Transposition");
    choose(&mut h, "Up an Octave");
    right_click(&mut h, at);
    choose(&mut h, "Transposition");
    choose(&mut h, "Down 1");
    assert_eq!(regions(&h)[0].transpose, 11);
    right_click(&mut h, at);
    choose(&mut h, "Transposition");
    choose(&mut h, "None");
    right_click(&mut h, at);
    choose(&mut h, "Transposition");
    choose(&mut h, "Up 1");
    right_click(&mut h, at);
    choose(&mut h, "Transposition");
    choose(&mut h, "Up 1");
    let held = regions(&h)[0];
    assert_eq!(held.name(), "D Dorian +2");
    assert_eq!(steps(&h), before + 7, "each choice is one undo step");
    // The menu ticks the current choices, and No Scale clears the scale.
    right_click(&mut h, at);
    choose(&mut h, "No Scale");
    assert!(!regions(&h)[0].scale.enabled);
    assert_eq!(regions(&h)[0].transpose, 2, "the transposition stays");
    // Delete is in the menu too.
    right_click(&mut h, at);
    choose(&mut h, "Delete");
    assert!(regions(&h).is_empty());
}

#[test]
fn a_midi_track_s_menu_has_follow_chord_lane_ticked_while_it_follows() {
    // Spec: chord-lane R5.
    let mut h = Harness::with_title_bar(Window::Main);
    h.settle();
    let track = h
        .shell
        .editor()
        .document()
        .top_level()
        .find(|id| {
            h.shell.editor().document().track(*id).unwrap().kind() == bl_model::TrackKind::Midi
        })
        .unwrap();
    let name = h
        .shell
        .editor()
        .document()
        .track(track)
        .unwrap()
        .name()
        .to_owned();
    let title = |h: &mut Harness| {
        let nodes = Harness::nodes(&h.settle());
        let b = nodes
            .iter()
            .find(|n| n.role == Role::RowHeader && n.label == name)
            .unwrap()
            .bounds
            .unwrap();
        pos2(b.x0 as f32 + 60.0, b.y0 as f32 + 7.0)
    };
    let before = steps(&h);
    let at = title(&mut h);
    let nodes = Harness::nodes(&right_click(&mut h, at));
    let item = nodes
        .iter()
        .find(|n| plain(&n.label) == "Follow Chord Lane")
        .expect("a MIDI track's menu has the switch");
    assert_ne!(item.toggled, Some(true), "a new track doesn't follow");
    click_node(&mut h, &item.clone());
    assert!(
        h.shell
            .editor()
            .document()
            .track(track)
            .unwrap()
            .follow_chord_lane()
    );
    assert_eq!(steps(&h), before + 1);
    // Ticked now; choosing it again switches it off.
    let at = title(&mut h);
    let nodes = Harness::nodes(&right_click(&mut h, at));
    let item = nodes
        .iter()
        .find(|n| plain(&n.label) == "Follow Chord Lane")
        .unwrap();
    assert_eq!(item.toggled, Some(true));
    click_node(&mut h, &item.clone());
    assert!(
        !h.shell
            .editor()
            .document()
            .track(track)
            .unwrap()
            .follow_chord_lane()
    );
    // An audio track's menu has none.
    let audio = h
        .shell
        .editor()
        .document()
        .top_level()
        .find(|id| {
            h.shell.editor().document().track(*id).unwrap().kind() == bl_model::TrackKind::Audio
        })
        .unwrap();
    let audio_name = h
        .shell
        .editor()
        .document()
        .track(audio)
        .unwrap()
        .name()
        .to_owned();
    let nodes = Harness::nodes(&h.settle());
    let b = nodes
        .iter()
        .find(|n| n.role == Role::RowHeader && n.label == audio_name)
        .unwrap()
        .bounds
        .unwrap();
    let nodes = Harness::nodes(&right_click(
        &mut h,
        pos2(b.x0 as f32 + 60.0, b.y0 as f32 + 7.0),
    ));
    assert!(!nodes.iter().any(|n| n.label.contains("Follow Chord Lane")));
}

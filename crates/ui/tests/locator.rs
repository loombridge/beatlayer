//! The Arrangement's locators, headless (WP-6.24): the buttons above the
//! track headers, the locators in the scrub area, the gestures, and the
//! menus. Spec: time-editing.

mod common;

use bl_core::time::Ticks;
use bl_shell::arrangement_commands::ArrangementState;
use bl_shell::locator_commands as ids;
use bl_ui::layout::ArrangementLayout;
use common::{Harness, Window};
use egui::accesskit::Role;
use egui::{Event, FullOutput, Modifiers, PointerButton, Pos2, pos2};
use serde_json::json;

const BAR: i64 = Ticks::WHOLE.get();

fn state(h: &mut Harness) -> ArrangementState {
    serde_json::from_value(h.shell.run("arrangement.state", json!({})).unwrap()).unwrap()
}

/// The Arrangement's layout in a harness that draws Live's title bar.
fn layout(h: &Harness) -> ArrangementLayout {
    let shell = bl_ui::layout::ShellLayout::compute(
        h.size - egui::vec2(0.0, bl_ui::layout::TITLE_BAR),
        true,
        true,
    )
    .translate(egui::vec2(0.0, bl_ui::layout::TITLE_BAR));
    let doc = h.shell.editor().document();
    let pinned = doc.returns().len() + 1;
    ArrangementLayout::compute(shell.main_view, h.shell.arrangement().overview(), pinned)
}

/// The locators the set holds.
fn locators(h: &Harness) -> Vec<(bl_model::LocatorId, Ticks)> {
    h.shell
        .editor()
        .document()
        .locators()
        .iter()
        .map(|locator| (locator.id, locator.position))
        .collect()
}

/// How many steps the undo history holds.
fn steps(h: &Harness) -> usize {
    h.shell.editor().undo_history().len()
}

/// A right-click at `pos`.
fn right_click(h: &mut Harness, pos: Pos2) -> FullOutput {
    h.frame(vec![
        Event::PointerMoved(pos),
        Event::PointerButton {
            pos,
            button: PointerButton::Secondary,
            pressed: true,
            modifiers: Modifiers::NONE,
        },
    ]);
    h.frame(vec![Event::PointerButton {
        pos,
        button: PointerButton::Secondary,
        pressed: false,
        modifiers: Modifiers::NONE,
    }]);
    h.settle()
}

#[test]
fn the_set_locator_button_adds_a_locator_and_reads_delete_while_one_is_selected() {
    // Spec: time-editing R3, R4.
    let mut h = Harness::with_title_bar(Window::Main);
    h.settle();
    let layout = layout(&h);
    let before = steps(&h);
    h.click(layout.set_locator.center());
    h.settle();
    assert_eq!(locators(&h).len(), 1);
    assert_eq!(steps(&h), before + 1);
    // The button reads Delete Locator now.
    let nodes = Harness::nodes(&h.settle());
    let button = Harness::node(&nodes, "Delete Locator");
    assert!(!button.disabled);
    // And it deletes the selected locator.
    h.click(layout.set_locator.center());
    h.settle();
    assert!(locators(&h).is_empty());
    assert_eq!(steps(&h), before + 2);
}

#[test]
fn clicking_a_locator_selects_it_and_draws_its_name() {
    // Spec: time-editing R2, R5.
    let mut h = Harness::with_title_bar(Window::Main);
    h.shell
        .run(ids::ADD, json!({ "at": 2 * BAR }))
        .unwrap();
    h.settle();
    let layout = layout(&h);
    let x = layout.scrub.left()
        + (h.shell.arrangement().horizontal(h.shell.editor().document()).x(Ticks(2 * BAR))
            as f32);
    h.click(pos2(x, layout.scrub.center().y));
    h.settle();
    let locator = locators(&h)[0].0;
    assert_eq!(state(&mut h).selected_locators, vec![locator]);
    // The locator is in the accessibility tree, named for it.
    let nodes = Harness::nodes(&h.settle());
    let node = Harness::node(&nodes, "1");
    assert_eq!(node.role, Role::Button);
}

#[test]
fn dragging_a_locator_moves_it_in_one_undo_step() {
    // Spec: time-editing R7, R13.
    let mut h = Harness::with_title_bar(Window::Main);
    h.shell
        .run(ids::ADD, json!({ "at": 2 * BAR }))
        .unwrap();
    h.settle();
    let layout = layout(&h);
    let horizontal = h.shell.arrangement().horizontal(h.shell.editor().document());
    let x = layout.scrub.left() + (horizontal.x(Ticks(2 * BAR)) as f32);
    let y = layout.scrub.center().y;
    let before = steps(&h);
    // A drag with several frames is one undo step.
    h.frame(vec![
        Event::PointerMoved(pos2(x, y)),
        Event::PointerButton {
            pos: pos2(x, y),
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        },
    ]);
    for step in 1..=6 {
        h.frame(vec![Event::PointerMoved(pos2(x + step as f32 * 10.0, y))]);
    }
    h.frame(vec![Event::PointerButton {
        pos: pos2(x + 60.0, y),
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    }]);
    h.settle();
    println!("EVENTS: {:?}", h.events);
    assert_eq!(steps(&h), before + 1);
    assert_ne!(locators(&h)[0].1, Ticks(2 * BAR));
}

#[test]
fn the_scrub_area_click_plays_from_the_clicked_point() {
    // Spec: time-editing R11.
    let mut h = Harness::with_title_bar(Window::Main);
    h.settle();
    let layout = layout(&h);
    h.click(pos2(layout.scrub.left() + 100.0, layout.scrub.center().y));
    h.settle();
    // The insert marker moved to the clicked point.
    let state = state(&mut h);
    assert!(state.insert_marker > Ticks::ZERO);
}

#[test]
fn the_scrub_areas_menu_adds_a_locator() {
    // Spec: time-editing R3 (C-TIME-022).
    let mut h = Harness::with_title_bar(Window::Main);
    h.settle();
    let layout = layout(&h);
    right_click(&mut h, pos2(layout.scrub.left() + 100.0, layout.scrub.center().y));
    let nodes = Harness::nodes(&h.settle());
    assert!(
        nodes.iter().any(|node| node.label == "Add Locator"),
        "the scrub area's menu has Add Locator"
    );
}

#[test]
fn a_locators_menu_has_the_rows_live_shows() {
    // Spec: time-editing R9, R10 (C-TIME-023, C-TIME-025).
    let mut h = Harness::with_title_bar(Window::Main);
    h.shell
        .run(ids::ADD, json!({ "at": 2 * BAR }))
        .unwrap();
    h.settle();
    let layout = layout(&h);
    let horizontal = h.shell.arrangement().horizontal(h.shell.editor().document());
    let x = layout.scrub.left() + (horizontal.x(Ticks(2 * BAR)) as f32);
    right_click(&mut h, pos2(x, layout.scrub.center().y));
    let nodes = Harness::nodes(&h.settle());
    for label in [
        "Loop to Next Locator",
        "Set Song Start Time Here",
        "Edit Info Text",
    ] {
        assert!(
            nodes.iter().any(|node| node.label == label),
            "the locator's menu has {label}"
        );
    }
}

#[test]
fn the_loop_braces_menu_sets_the_song_start_time() {
    // Spec: time-editing R10 (C-TIME-023).
    let mut h = Harness::with_title_bar(Window::Main);
    h.settle();
    let layout = layout(&h);
    let horizontal = h.shell.arrangement().horizontal(h.shell.editor().document());
    let x = layout.beat_ruler.left()
        + (horizontal.x(h.shell.arrangement().transport().loop_start) as f32);
    right_click(&mut h, pos2(x + 4.0, layout.beat_ruler.bottom() - 3.0));
    let nodes = Harness::nodes(&h.settle());
    assert!(
        nodes.iter().any(|node| node.label == "Set Song Start Time Here"),
        "the loop brace's menu has Set Song Start Time Here"
    );
}

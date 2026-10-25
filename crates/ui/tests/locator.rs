//! The Arrangement's locators, headless (WP-6.24): the buttons above the
//! track headers, the locators in the scrub area, the gestures, and the
//! menus. Spec: time-editing.

use crate::common;
use bl_core::time::Ticks;
use bl_shell::arrangement_commands::ArrangementState;
use bl_shell::locator_commands as ids;
use bl_shell::menus::{MenuPlatform, menu_bar};
use bl_ui::keymap::Platform;
use bl_ui::layout::ArrangementLayout;
use bl_ui::main_window::UiEvent;
use bl_ui::menus::item_state;
use common::{Harness, Window};
use egui::accesskit::Role;
use egui::{Event, FullOutput, Key, Modifiers, PointerButton, Pos2, pos2};
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
    h.shell.run(ids::ADD, json!({ "at": 2 * BAR })).unwrap();
    h.settle();
    let layout = layout(&h);
    let x = layout.scrub.left()
        + (h.shell
            .arrangement()
            .horizontal(h.shell.editor().document())
            .x(Ticks(2 * BAR)) as f32);
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
    h.shell.run(ids::ADD, json!({ "at": 2 * BAR })).unwrap();
    h.settle();
    let layout = layout(&h);
    let horizontal = h
        .shell
        .arrangement()
        .horizontal(h.shell.editor().document());
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
    right_click(
        &mut h,
        pos2(layout.scrub.left() + 100.0, layout.scrub.center().y),
    );
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
    h.shell.run(ids::ADD, json!({ "at": 2 * BAR })).unwrap();
    h.settle();
    let layout = layout(&h);
    let horizontal = h
        .shell
        .arrangement()
        .horizontal(h.shell.editor().document());
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
    let horizontal = h
        .shell
        .arrangement()
        .horizontal(h.shell.editor().document());
    let x = layout.beat_ruler.left()
        + (horizontal.x(h.shell.arrangement().transport().loop_start) as f32);
    right_click(&mut h, pos2(x + 4.0, layout.beat_ruler.bottom() - 3.0));
    let nodes = Harness::nodes(&h.settle());
    assert!(
        nodes
            .iter()
            .any(|node| node.label == "Set Song Start Time Here"),
        "the loop brace's menu has Set Song Start Time Here"
    );
}

/// Two clicks in quick succession, as a double-click.
fn double_click(h: &mut Harness, at: Pos2) {
    // Long enough after any earlier click that egui doesn't pair them.
    h.time += 1.0;
    h.frame(vec![Event::PointerMoved(at)]);
    for _ in 0..2 {
        for pressed in [true, false] {
            h.frame(vec![Event::PointerButton {
                pos: at,
                button: PointerButton::Primary,
                pressed,
                modifiers: Modifiers::NONE,
            }]);
        }
    }
    h.settle();
}

/// The state of the menu item `row`.
fn menu_item(h: &Harness, row: &str) -> bl_ui::menus::ItemState {
    let menus = menu_bar(MenuPlatform::current());
    let item = menus
        .iter()
        .flat_map(bl_shell::menus::Menu::walk)
        .find(|item| item.row == row)
        .unwrap();
    item_state(item, &h.shell, &h.keymap, Platform::current())
}

/// The x of the locator at `bar`, in the scrub area's coordinates.
fn locator_x(h: &Harness, layout: &ArrangementLayout, bar: i64) -> f32 {
    let horizontal = h
        .shell
        .arrangement()
        .horizontal(h.shell.editor().document());
    layout.scrub.left() + (horizontal.x(Ticks(bar * BAR)) as f32)
}

#[test]
fn escaping_a_locator_drag_leaves_it_where_it_was() {
    // Spec: time-editing R7, R13.
    let mut h = Harness::with_title_bar(Window::Main);
    h.shell.run(ids::ADD, json!({ "at": 2 * BAR })).unwrap();
    h.settle();
    let layout = layout(&h);
    let x = locator_x(&h, &layout, 2);
    let y = layout.scrub.center().y;
    let before = steps(&h);
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
    // Esc abandons the drag.
    h.key(Key::Escape, Modifiers::NONE);
    h.frame(vec![Event::PointerButton {
        pos: pos2(x + 60.0, y),
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    }]);
    h.settle();
    assert_eq!(locators(&h)[0].1, Ticks(2 * BAR), "the locator stayed put");
    assert_eq!(steps(&h), before, "a cancelled drag is no undo step");
}

#[test]
fn double_clicking_a_locator_selects_it_and_asks_to_play_from_it() {
    // Spec: time-editing R5.
    let mut h = Harness::with_title_bar(Window::Main);
    h.shell.run(ids::ADD, json!({ "at": 2 * BAR })).unwrap();
    h.settle();
    let layout = layout(&h);
    let x = locator_x(&h, &layout, 2);
    h.take_events();
    double_click(&mut h, pos2(x, layout.scrub.center().y));
    let locator = locators(&h)[0].0;
    assert_eq!(state(&mut h).selected_locators, vec![locator]);
    let asked: Vec<&str> = h
        .events
        .iter()
        .filter_map(|event| match event {
            UiEvent::Invoke(invocation) => Some(invocation.id.as_str()),
            _ => None,
        })
        .collect();
    assert!(asked.contains(&ids::JUMP), "the double-click jumps to it");
    assert!(
        asked.contains(&"transport.play_from_insert_marker"),
        "and starts playback from it"
    );
}

#[test]
fn option_right_stops_at_a_locator() {
    // Spec: time-editing R12; arrangement-editing R20.
    let mut h = Harness::with_title_bar(Window::Main);
    h.shell.run(ids::ADD, json!({ "at": 3 * BAR })).unwrap();
    h.settle();
    let layout = layout(&h);
    // Focus the Arrangement with the insert marker at 1.1.1.
    h.click(pos2(layout.lanes.left() + 1.0, layout.lanes.center().y));
    h.settle();
    assert_eq!(state(&mut h).insert_marker, Ticks::ZERO);
    h.take_events();
    h.key(Key::ArrowRight, Modifiers::ALT);
    assert_eq!(
        state(&mut h).insert_marker,
        Ticks(3 * BAR),
        "Option-Right stops at the locator"
    );
}

#[test]
fn the_create_menus_item_reads_delete_while_a_locator_is_selected() {
    // Spec: time-editing R4 (M-CREATE-016).
    let mut h = Harness::with_title_bar(Window::Main);
    h.settle();
    assert_eq!(menu_item(&h, "M-CREATE-016").label, "Add Locator");
    assert!(menu_item(&h, "M-CREATE-016").enabled);
    h.shell.run(ids::ADD, json!({ "at": 2 * BAR })).unwrap();
    h.settle();
    assert_eq!(menu_item(&h, "M-CREATE-016").label, "Delete Locator");
}

#[test]
fn cmd_r_renames_the_selected_locator_in_place() {
    // Spec: time-editing R8. Cmd-R types the new name in the scrub area,
    // and Return keeps it.
    let mut h = Harness::with_title_bar(Window::Main);
    h.shell.run(ids::ADD, json!({ "at": 2 * BAR })).unwrap();
    h.settle();
    let locator = locators(&h)[0].0;
    h.shell
        .run(ids::SELECT, json!({ "locators": [locator] }))
        .unwrap();
    h.settle();
    h.key(Key::R, Modifiers::COMMAND);
    h.settle();
    h.frame(vec![Event::Text("Intro".to_owned())]);
    h.key(Key::Enter, Modifiers::NONE);
    h.settle();
    let name = h.shell.editor().document().locators()[0].name.clone();
    assert!(name.ends_with("Intro"), "the locator was renamed: {name}");
}

#[test]
fn the_scrub_areas_add_locator_adds_one_where_it_was_right_clicked() {
    // Spec: time-editing R3 (C-TIME-022).
    let mut h = Harness::with_title_bar(Window::Main);
    h.settle();
    let layout = layout(&h);
    let at = pos2(layout.scrub.left() + 100.0, layout.scrub.center().y);
    right_click(&mut h, at);
    let nodes = Harness::nodes(&h.settle());
    let add = nodes
        .iter()
        .find(|node| node.label == "Add Locator")
        .expect("the scrub area's menu has Add Locator");
    let bounds = add.bounds.unwrap();
    h.click(pos2(
        ((bounds.x0 + bounds.x1) / 2.0) as f32,
        ((bounds.y0 + bounds.y1) / 2.0) as f32,
    ));
    h.settle();
    assert_eq!(locators(&h).len(), 1);
    // The locator is where the area was right-clicked, on the editing grid.
    let doc = h.shell.editor().document();
    let horizontal = h.shell.arrangement().horizontal(doc);
    let clicked = horizontal.at(f64::from(at.x - layout.scrub.left()));
    assert_eq!(locators(&h)[0].1, h.shell.arrangement().snap(doc, clicked));
}

#[test]
fn a_locators_menu_edit_info_text_types_the_text_in_the_info_view() {
    // Spec: info-view R10, R11 (C-TIME-025).
    let mut h = Harness::with_title_bar(Window::Main);
    h.shell.run(ids::ADD, json!({ "at": 2 * BAR })).unwrap();
    h.settle();
    let locator = locators(&h)[0].0;
    let layout = layout(&h);
    let horizontal = h
        .shell
        .arrangement()
        .horizontal(h.shell.editor().document());
    let x = layout.scrub.left() + (horizontal.x(Ticks(2 * BAR)) as f32);
    right_click(&mut h, pos2(x, layout.scrub.center().y));
    let nodes = Harness::nodes(&h.settle());
    let item = nodes
        .iter()
        .find(|node| node.label == "Edit Info Text")
        .expect("the locator's menu lists Edit Info Text");
    assert!(!item.disabled, "it works now");
    h.frame(vec![Event::AccessKitActionRequest(
        egui::accesskit::ActionRequest {
            action: egui::accesskit::Action::Click,
            target_tree: egui::accesskit::TreeId::ROOT,
            target_node: item.id,
            data: None,
        },
    )]);
    h.settle();
    assert!(h.main.is_typing(), "the Info View is a text field");
    h.frame(vec![Event::Text("Drop here".to_owned())]);
    h.key(Key::Enter, Modifiers::NONE);
    h.settle();
    let doc = h.shell.editor().document();
    assert_eq!(doc.locator(locator).unwrap().info_text, "Drop here");
}

#[test]
fn a_locator_dragged_near_a_clip_edge_snaps_to_it_and_shows_a_line() {
    // Spec: arrangement-editing R22, R23. The scrub area is 30 points to a
    // bar; the locator, dragged to 7.3, is 1.5 points from the clip edge at
    // 7.25 and 9 from the grid line at 7.
    let mut s = Harness::with_title_bar(Window::Main);
    let mut editor = bl_shell::files::factory_default_set();
    let track = editor.document().top_level().nth(2).unwrap();
    let id: bl_model::ClipId = editor.new_id();
    editor
        .apply(bl_model::Command::CreateClip {
            track,
            clip: bl_model::Clip::new(
                id,
                Ticks(7 * BAR + BAR / 4),
                Ticks(BAR),
                bl_model::ClipData::Audio(bl_model::AudioData::new(bl_model::ContentHash([1; 32]))),
            ),
        })
        .unwrap();
    editor.clear_history().unwrap();
    s.shell = bl_ui::arrangement::benchmark::shell_with(editor);
    s.shell.run(ids::ADD, json!({ "at": 2 * BAR })).unwrap();
    s.settle();
    let layout = layout(&s);
    let (x, y) = (locator_x(&s, &layout, 2), layout.scrub.center().y);
    let to = locator_x(&s, &layout, 7) + 9.0;
    let before = steps(&s);
    s.frame(vec![
        Event::PointerMoved(pos2(x, y)),
        Event::PointerButton {
            pos: pos2(x, y),
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        },
    ]);
    for step in 1..=6 {
        s.frame(vec![Event::PointerMoved(pos2(
            x + (to - x) * step as f32 / 6.0,
            y,
        ))]);
    }
    assert_eq!(
        s.main.arrangement().drawn_snap_line(),
        Some(Ticks(7 * BAR + BAR / 4))
    );
    s.frame(vec![Event::PointerButton {
        pos: pos2(to, y),
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    }]);
    s.settle();
    assert_eq!(locators(&s)[0].1, Ticks(7 * BAR + BAR / 4));
    assert_eq!(steps(&s), before + 1);
    assert_eq!(s.main.arrangement().drawn_snap_line(), None);
}

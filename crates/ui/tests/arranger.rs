//! The arranger track, headless (WP-6.45): its place above the track lanes,
//! the sections in it and their gestures, its menus and the View menu's item,
//! the keys, the Tab order and the accessibility tree. Spec: arranger-track
//! R1, R2, R4 to R14.

use crate::common::{self, Harness, Window};
use bl_core::time::Ticks;
use bl_model::SectionId;
use bl_shell::arrangement_commands::ArrangementState;
use bl_shell::arranger_commands as ids;
use bl_shell::menus::{MenuPlatform, menu_bar};
use bl_ui::keymap::Platform;
use bl_ui::layout::{ARRANGER_LANE, ArrangementLayout};
use bl_ui::menus::item_state;
use egui::accesskit::Role;
use egui::{Event, Key, Modifiers, PointerButton, Pos2, pos2};
use serde_json::json;

const BAR: i64 = Ticks::WHOLE.get();

/// A harness showing the arranger track, with Intro, Verse and Chorus over
/// bars 1 to 2, 3 to 4 and 5 to 6.
fn with_sections() -> Harness {
    let mut h = Harness::with_title_bar(Window::Main);
    for (name, from, to) in [("Intro", 0, 2), ("Verse", 2, 4), ("Chorus", 4, 6)] {
        h.shell
            .run(
                ids::ADD_SECTION,
                json!({ "start": from * BAR, "end": to * BAR, "name": name }),
            )
            .unwrap();
    }
    h.shell
        .run(ids::ARRANGER_TRACK, json!({ "on": true }))
        .unwrap();
    h.settle();
    h
}

fn state(h: &mut Harness) -> ArrangementState {
    serde_json::from_value(h.shell.run("arrangement.state", json!({})).unwrap()).unwrap()
}

fn steps(h: &Harness) -> usize {
    h.shell.editor().undo_history().len()
}

fn last_step(h: &Harness) -> String {
    h.shell
        .editor()
        .undo_history()
        .next()
        .unwrap_or_default()
        .to_owned()
}

/// The Arrangement's layout, as the set has it now.
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
    if doc.settings().arranger_track {
        layout.with_arranger_lane()
    } else {
        layout
    }
}

/// The x of a position in the Arrangement, apart from the harness.
fn column(h: &Harness, layout: &ArrangementLayout) -> impl Fn(Ticks) -> f32 + use<> {
    let horizontal = h
        .shell
        .arrangement()
        .horizontal(h.shell.editor().document());
    let left = layout.scrub.left();
    move |at| left + horizontal.x(at) as f32
}

/// The sections, as `(name, start, end)` in bars.
fn spans(h: &Harness) -> Vec<(String, i64, i64)> {
    h.shell
        .editor()
        .document()
        .sections()
        .iter()
        .map(|s| (s.name.clone(), s.start.get() / BAR, s.end().get() / BAR))
        .collect()
}

fn id_of(h: &Harness, name: &str) -> SectionId {
    h.shell
        .editor()
        .document()
        .sections()
        .iter()
        .find(|s| s.name == name)
        .unwrap()
        .id
}

/// A drag from `from` to `to` with `modifiers` held, released at the end.
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
        held,
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

fn right_click(h: &mut Harness, pos: Pos2) {
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
    h.settle();
}

#[test]
fn the_lane_is_hidden_until_the_view_menu_shows_it_and_sits_above_the_track_lanes() {
    // Spec: arranger-track R1, R2; layout L-ARR-023.
    let mut h = Harness::with_title_bar(Window::Main);
    h.settle();
    assert!(!state(&mut h).arranger_track);
    let hidden = layout(&h);
    assert!(hidden.arranger.is_none());
    // The View menu's item reads Arranger Track, straight after Overview, and
    // is ticked while the lane shows.
    let menus = menu_bar(MenuPlatform::current());
    let view = menus.iter().find(|menu| menu.title == "View").unwrap();
    let rows: Vec<&str> = view.walk().map(|item| item.row).collect();
    let overview = rows.iter().position(|row| *row == "M-VIEW-034").unwrap();
    assert_eq!(rows[overview + 1], "IMP-22");
    let item = |h: &Harness| {
        let item = menus
            .iter()
            .flat_map(bl_shell::menus::Menu::walk)
            .find(|item| item.row == "IMP-22")
            .unwrap();
        item_state(item, &h.shell, &h.keymap, Platform::current())
    };
    assert_eq!(item(&h).label, "Arranger Track");
    assert_eq!(item(&h).checked, Some(false));
    let invocation = item(&h).invocation.unwrap();
    h.shell.invoke(&invocation.id, invocation.args).unwrap();
    h.settle();
    assert_eq!(item(&h).checked, Some(true));
    // Shown, it is an 18-point row under the scrub area, with the track
    // lanes and their headers below it; nothing above it moves.
    let shown = layout(&h);
    let lane = shown.arranger.expect("the lane shows");
    assert_eq!(lane.height(), ARRANGER_LANE);
    assert!(lane.top() >= shown.scrub.bottom());
    assert_eq!(lane.top(), hidden.lanes.top());
    assert_eq!(shown.lanes.top(), lane.bottom());
    assert_eq!(shown.headers.top(), lane.bottom());
    assert_eq!(shown.arranger_header.unwrap().top(), lane.top());
    assert_eq!(shown.scrub, hidden.scrub);
    assert_eq!(shown.beat_ruler, hidden.beat_ruler);
    assert_eq!(shown.lanes.bottom(), hidden.lanes.bottom());
    // Hiding it again puts every coordinate back.
    h.shell
        .run(ids::ARRANGER_TRACK, json!({ "on": false }))
        .unwrap();
    h.settle();
    assert_eq!(layout(&h), hidden);
}

#[test]
fn the_lane_is_a_list_of_sections_named_with_their_span_and_selection() {
    // Spec: arranger-track R4.
    let mut h = with_sections();
    h.shell
        .run(
            ids::SELECT_SECTION,
            json!({ "sections": [id_of(&h, "Verse")] }),
        )
        .unwrap();
    let nodes = Harness::nodes(&h.settle());
    let list = Harness::node(&nodes, "Arranger Track");
    assert_eq!(list.role, Role::List);
    for (name, value, selected) in [
        ("Intro", "1.1.1 to 3.1.1", false),
        ("Verse", "3.1.1 to 5.1.1", true),
        ("Chorus", "5.1.1 to 7.1.1", false),
    ] {
        let node = Harness::node(&nodes, name);
        assert_eq!(node.role, Role::ListItem, "{name}");
        assert_eq!(node.value, value, "{name}");
        assert_eq!(node.selected, Some(selected), "{name}");
        assert!(Harness::descends_from(&nodes, node.id, list.id), "{name}");
    }
}

#[test]
fn dragging_across_an_empty_stretch_makes_a_section_cut_short_at_the_next() {
    // Spec: arranger-track R5.
    let mut h = with_sections();
    let layout = layout(&h);
    let x = column(&h, &layout);
    let lane = layout.arranger.unwrap();
    // A section already at bars 9 to 10 cuts the new one short.
    h.shell
        .run(
            ids::ADD_SECTION,
            json!({ "start": 8 * BAR, "end": 9 * BAR, "name": "Outro" }),
        )
        .unwrap();
    h.settle();
    let before = steps(&h);
    let (from, to) = (
        pos2(x(Ticks(6 * BAR)) + 1.0, lane.center().y),
        pos2(x(Ticks(9 * BAR)), lane.center().y),
    );
    drag(&mut h, from, to, Modifiers::NONE);
    assert_eq!(steps(&h), before + 1);
    assert_eq!(last_step(&h), "Add Section");
    assert_eq!(spans(&h)[3], ("Section 1".to_owned(), 6, 8));
}

#[test]
fn clicking_selects_a_section_and_its_time_and_shift_extends() {
    // Spec: arranger-track R6.
    let mut h = with_sections();
    let layout = layout(&h);
    let x = column(&h, &layout);
    let lane = layout.arranger.unwrap();
    let at = |bar: i64| pos2(x(Ticks(bar * BAR)) + 20.0, lane.center().y);
    let before = steps(&h);
    h.click(at(2));
    h.settle();
    let selected = state(&mut h);
    assert_eq!(selected.selected_sections, [id_of(&h, "Verse")]);
    let time = selected.time_selection.unwrap();
    assert_eq!((time.start, time.end), (Ticks(2 * BAR), Ticks(4 * BAR)));
    // Shift-click extends to the one clicked.
    h.frame(vec![Event::ModifiersChanged(Modifiers::SHIFT)]);
    h.frame(vec![Event::PointerMoved(at(4))]);
    h.frame(vec![Event::PointerButton {
        pos: at(4),
        button: PointerButton::Primary,
        pressed: true,
        modifiers: Modifiers::SHIFT,
    }]);
    h.frame(vec![Event::PointerButton {
        pos: at(4),
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::SHIFT,
    }]);
    h.frame(vec![Event::ModifiersChanged(Modifiers::NONE)]);
    h.settle();
    assert_eq!(
        state(&mut h).selected_sections,
        [id_of(&h, "Verse"), id_of(&h, "Chorus")]
    );
    assert_eq!(steps(&h), before, "selecting is no undo step");
}

#[test]
fn dragging_a_section_moves_it_and_option_copies_it_each_as_one_undo_step() {
    // Spec: arranger-track R8, R9.
    let mut h = with_sections();
    let layout = layout(&h);
    let x = column(&h, &layout);
    let lane = layout.arranger.unwrap();
    let y = lane.center().y;
    let before = steps(&h);
    // Chorus (bars 5 to 6) dragged to the front of Intro: its start drops on
    // Intro's start edge.
    drag(
        &mut h,
        pos2(x(Ticks(5 * BAR)) + 20.0, y),
        pos2(x(Ticks::ZERO) + 2.0, y),
        Modifiers::NONE,
    );
    assert_eq!(steps(&h), before + 1);
    assert_eq!(last_step(&h), "Move Section");
    assert_eq!(
        spans(&h).iter().map(|s| s.0.as_str()).collect::<Vec<_>>(),
        ["Chorus", "Intro", "Verse"]
    );
    // Option (Ctrl off macOS) copies it instead.
    let copy = if cfg!(target_os = "macos") {
        Modifiers::ALT
    } else {
        Modifiers::CTRL
    };
    drag(
        &mut h,
        pos2(x(Ticks::ZERO) + 20.0, y),
        pos2(x(Ticks(8 * BAR)), y),
        copy,
    );
    assert_eq!(steps(&h), before + 2);
    assert_eq!(last_step(&h), "Copy Section");
    assert_eq!(spans(&h).len(), 4);
    // Esc abandons a drag in progress.
    let held = steps(&h);
    let (from, to) = (pos2(x(Ticks::ZERO) + 20.0, y), pos2(x(Ticks(12 * BAR)), y));
    h.frame(vec![Event::PointerMoved(from)]);
    h.frame(vec![Event::PointerButton {
        pos: from,
        button: PointerButton::Primary,
        pressed: true,
        modifiers: Modifiers::NONE,
    }]);
    for step in 1..=4 {
        h.frame(vec![Event::PointerMoved(
            from + (to - from) * (step as f32 / 4.0),
        )]);
    }
    h.key(Key::Escape, Modifiers::NONE);
    h.frame(vec![Event::PointerButton {
        pos: to,
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    }]);
    h.settle();
    assert_eq!(steps(&h), held, "Esc leaves the song as it was");
}

#[test]
fn dragging_an_edge_moves_it_and_nothing_on_the_tracks() {
    // Spec: arranger-track R7.
    let mut h = with_sections();
    let layout = layout(&h);
    let x = column(&h, &layout);
    let y = layout.arranger.unwrap().center().y;
    let before = steps(&h);
    // Chorus's right edge, at bar 7, to bar 8.
    drag(
        &mut h,
        pos2(x(Ticks(6 * BAR)) - 1.5, y),
        pos2(x(Ticks(7 * BAR)), y),
        Modifiers::NONE,
    );
    assert_eq!(steps(&h), before + 1);
    assert_eq!(last_step(&h), "Resize Section");
    assert_eq!(spans(&h)[2], ("Chorus".to_owned(), 4, 7));
    // Dragging an edge into another section is refused and changes nothing.
    let held = steps(&h);
    drag(
        &mut h,
        pos2(x(Ticks(4 * BAR)) + 1.5, y),
        pos2(x(Ticks(3 * BAR)), y),
        Modifiers::NONE,
    );
    assert_eq!(steps(&h), held);
}

#[test]
fn double_click_and_cmd_r_type_a_name_and_return_keeps_it() {
    // Spec: arranger-track R7.
    let mut h = with_sections();
    let layout = layout(&h);
    let x = column(&h, &layout);
    let lane = layout.arranger.unwrap();
    let verse = pos2(x(Ticks(2 * BAR)) + 20.0, lane.center().y);
    let before = steps(&h);
    h.click(verse);
    h.click(verse);
    h.settle();
    h.frame(vec![Event::Key {
        key: Key::A,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: common::command(),
    }]);
    h.frame(vec![Event::Text("Bridge".to_owned())]);
    h.key(Key::Enter, Modifiers::NONE);
    h.settle();
    assert_eq!(steps(&h), before + 1);
    assert_eq!(spans(&h)[1].0, "Bridge");
    // Cmd-R on the selected section types over its name; Esc drops it.
    h.key(Key::R, common::command());
    h.settle();
    h.frame(vec![Event::Text("Drop".to_owned())]);
    h.key(Key::Escape, Modifiers::NONE);
    h.settle();
    assert_eq!(steps(&h), before + 1);
    assert_eq!(spans(&h)[1].0, "Bridge");
}

#[test]
fn cmd_d_duplicates_and_delete_deletes_the_selected_section() {
    // Spec: arranger-track R10, R11.
    let mut h = with_sections();
    h.shell
        .run(
            ids::SELECT_SECTION,
            json!({ "sections": [id_of(&h, "Verse")] }),
        )
        .unwrap();
    h.settle();
    let before = steps(&h);
    h.key(Key::D, common::command());
    h.settle();
    assert_eq!(steps(&h), before + 1);
    assert_eq!(last_step(&h), "Duplicate Section");
    assert_eq!(spans(&h).len(), 4);
    h.key(Key::Delete, Modifiers::NONE);
    h.settle();
    assert_eq!(steps(&h), before + 2);
    assert_eq!(last_step(&h), "Delete Section");
    assert_eq!(spans(&h).len(), 3);
}

/// Clicks the menu item named `label`, which the last frame drew.
fn click_item(h: &mut Harness, label: &str) {
    let nodes = Harness::nodes(&h.settle());
    let bounds = Harness::node(&nodes, label).bounds.unwrap();
    h.click(pos2(
        ((bounds.x0 + bounds.x1) / 2.0) as f32,
        ((bounds.y0 + bounds.y1) / 2.0) as f32,
    ));
    h.settle();
}

#[test]
fn a_sections_menu_moves_colours_removes_and_deletes_it() {
    // Spec: arranger-track R7, R11, R12.
    let mut h = with_sections();
    let layout = layout(&h);
    let x = column(&h, &layout);
    let lane = layout.arranger.unwrap();
    let at = |bar: i64| pos2(x(Ticks(bar * BAR)) + 20.0, lane.center().y);
    // The first section has none before it, so Move Earlier is disabled.
    right_click(&mut h, at(0));
    let nodes = Harness::nodes(&h.settle());
    assert!(Harness::node(&nodes, "Move Earlier").disabled);
    assert!(!Harness::node(&nodes, "Move Later").disabled);
    for label in ["Rename Section", "Remove Section", "Delete Section"] {
        Harness::node(&nodes, label);
    }
    h.key(Key::Escape, Modifiers::NONE);
    h.settle();
    // Verse's menu moves it before Intro, as one undo step.
    let before = steps(&h);
    right_click(&mut h, at(2));
    click_item(&mut h, "Move Earlier");
    assert_eq!(steps(&h), before + 1);
    assert_eq!(last_step(&h), "Move Section");
    assert_eq!(spans(&h)[0].0, "Verse");
    // Remove Section leaves the tracks, Delete Section deletes the time.
    right_click(&mut h, at(0));
    click_item(&mut h, "Remove Section");
    assert_eq!(last_step(&h), "Remove Section");
    assert_eq!(spans(&h).len(), 2);
    right_click(&mut h, at(2));
    click_item(&mut h, "Delete Section");
    assert_eq!(last_step(&h), "Delete Section");
}

#[test]
fn the_lanes_menu_creates_sections_from_the_time_selection_and_the_locators() {
    // Spec: arranger-track R5.
    let mut h = Harness::with_title_bar(Window::Main);
    h.shell
        .run(ids::ARRANGER_TRACK, json!({ "on": true }))
        .unwrap();
    h.settle();
    let layout = layout(&h);
    let x = column(&h, &layout);
    let lane = layout.arranger.unwrap();
    right_click(&mut h, pos2(x(Ticks(3 * BAR)), lane.center().y));
    let nodes = Harness::nodes(&h.settle());
    // With no time selection Create Section has no span to take.
    assert!(Harness::node(&nodes, "Create Section").disabled);
    assert!(!Harness::node(&nodes, "Create Sections From Locators").disabled);
}

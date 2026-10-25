//! The Clip View's panel layout, headless (WP-6.72): the panels beside the
//! editor or stacked in a column that scrolls, the View menu's and the title
//! bar's choice between the two, the drag of the panels' edge, folding, the
//! keys, and what assistive technology reads.
//!
//! Spec: detail-views R19 to R26.

use crate::common::{Harness, Node};
use crate::main_properties::{audio_clip, session_clip};
use bl_shell::clip_properties_commands::PanelArrangement;
use bl_shell::menus::{MenuPlatform, menu_bar};
use bl_ui::clip_panels::{Parts, parts};
use bl_ui::keymap::Platform;
use bl_ui::menus::item_state;
use bl_ui::tool_panel::SELECTOR;
use egui::accesskit::{Action, Role};
use egui::{
    Event, Id, Key, Modifiers, MouseWheelUnit, PointerButton, Pos2, Rect, TouchPhase, pos2, vec2,
};
use serde_json::json;

fn nodes(h: &mut Harness) -> Vec<Node> {
    Harness::nodes(&h.settle())
}

fn rect(node: &Node) -> Rect {
    let b = node.bounds.expect("the node has bounds");
    Rect::from_min_max(
        pos2(b.x0 as f32, b.y0 as f32),
        pos2(b.x1 as f32, b.y1 as f32),
    )
}

fn view(h: &mut Harness) -> Rect {
    let all = nodes(h);
    rect(Harness::node(&all, "Clip View"))
}

fn with_role<'a>(all: &'a [Node], role: Role, label: &str) -> Option<&'a Node> {
    all.iter().find(|n| n.role == role && n.label == label)
}

fn arrange(h: &mut Harness, arrangement: &str) {
    h.shell
        .run(
            "clip_view.arrange_panels",
            json!({ "arrangement": arrangement }),
        )
        .unwrap();
    h.settle();
}

fn overlap(a: Rect, b: Rect) -> bool {
    let both = a.intersect(b);
    both.width() > 0.0 && both.height() > 0.0
}

fn double_click(h: &mut Harness, at: Pos2) {
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

fn drag(h: &mut Harness, from: Pos2, to: Pos2) {
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
    h.frame(vec![Event::PointerButton {
        pos: to,
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    }]);
    h.settle();
}

fn layout_of(h: &mut Harness) -> (Rect, bool, Parts) {
    let view = view(h);
    let vertical = bl_ui::clip_panels::is_vertical(&h.shell, view);
    let folded = h.shell.clip_view().panels_folded();
    (view, vertical, parts(view, vertical, folded, 0.0))
}

/// The tab lists and panels the last frame drew, by their rectangles.
fn drawn(all: &[Node], panels: [&str; 2]) -> [Rect; 4] {
    let get = |role, label| rect(with_role(all, role, label).unwrap_or_else(|| panic!("{label}")));
    [
        get(Role::TabList, "Properties Panels"),
        get(Role::TabList, "Tool Panels"),
        get(Role::TabPanel, panels[0]),
        get(Role::TabPanel, panels[1]),
    ]
}

// Spec: detail-views R19, R26 (L-CLIP-003, L-CLIP-009). Horizontally the
// right group sits beside the left; vertically it sits under it at the left
// group's width; each has its own part of the selector row, and nothing
// overlaps the editor, for a MIDI and an audio clip at 50 %, 100 % and 200 %.
#[test]
fn the_panels_sit_beside_the_editor_or_stacked_without_overlap() {
    for audio in [false, true] {
        for percent in [50, 100, 200] {
            for arrangement in ["horizontal", "vertical"] {
                let (mut h, _tone) = if audio {
                    let (h, _, tone) = audio_clip(&format!("layout-{percent}-{arrangement}"));
                    (h, Some(tone))
                } else {
                    (session_clip().0, None)
                };
                h.shell
                    .run("view.set_zoom", json!({ "percent": percent }))
                    .unwrap();
                arrange(&mut h, arrangement);
                let (view, vertical, at) = layout_of(&mut h);
                let case = format!("audio {audio}, {percent} %, {arrangement}");
                assert_eq!(vertical, arrangement == "vertical", "{case}");
                let all = nodes(&mut h);
                let right = if audio {
                    "Audio Utilities"
                } else {
                    "Pitch and Time Utilities"
                };
                let [left_tabs, right_tabs, left, right] = drawn(&all, ["Main Properties", right]);
                assert_eq!(left, at.left, "{case}: the left group");
                assert_eq!(right, at.right, "{case}: the right group");
                assert_eq!(left_tabs.size(), vec2(at.left.width(), SELECTOR), "{case}");
                assert_eq!(left_tabs.min, at.left.min, "{case}");
                assert_eq!(
                    right_tabs.size(),
                    vec2(at.right.width(), SELECTOR),
                    "{case}"
                );
                assert_eq!(right_tabs.min, at.right.min, "{case}");
                if vertical {
                    assert_eq!(
                        at.right.left(),
                        at.left.left(),
                        "{case}: under the left group"
                    );
                    assert_eq!(at.right.top(), at.left.bottom(), "{case}");
                    assert_eq!(at.right.width(), at.left.width(), "{case}: as wide");
                } else {
                    assert_eq!(
                        at.right.left(),
                        at.left.right(),
                        "{case}: beside the left group"
                    );
                    assert_eq!(at.right.top(), at.left.top(), "{case}");
                }
                // Nothing overlaps: not the groups, the title bar or the
                // editor, which takes the width the panels give up.
                let columns = [
                    at.title,
                    at.left.intersect(at.column),
                    at.right.intersect(at.column),
                ];
                for (index, one) in columns.iter().enumerate() {
                    assert!(
                        !overlap(*one, at.editor),
                        "{case}: part {index} and the editor"
                    );
                    for other in &columns[index + 1..] {
                        assert!(!overlap(*one, *other), "{case}: part {index}");
                    }
                }
                assert!(
                    view.contains_rect(at.editor) && view.contains_rect(at.title),
                    "{case}"
                );
                assert_eq!(at.editor.right(), view.right(), "{case}");
                assert_eq!(at.editor.left(), at.column.right(), "{case}");
            }
        }
    }
}

// Spec: detail-views R19. Where the two groups are taller than the Clip
// View, the stacked column scrolls with the wheel.
#[test]
fn a_stacked_column_taller_than_the_clip_view_scrolls() {
    let (mut h, _) = session_clip();
    arrange(&mut h, "vertical");
    let (view, _, at) = layout_of(&mut h);
    let steps = h.shell.editor().undo_history().len();
    assert!(
        at.scrollable > 0.0,
        "225 points hold less than the two groups"
    );
    assert!(
        at.right.top() >= view.bottom() - 1.0,
        "the right group starts below the view"
    );
    let wheel = |h: &mut Harness, by: f32| {
        h.frame(vec![Event::PointerMoved(at.column.center())]);
        h.frame(vec![Event::MouseWheel {
            unit: MouseWheelUnit::Point,
            delta: vec2(0.0, -by),
            phase: TouchPhase::Move,
            modifiers: Modifiers::NONE,
        }]);
        // The wheel's delta is spread over a few frames.
        let mut last = f32::MAX;
        for _ in 0..60 {
            let all = nodes(h);
            let left = rect(with_role(&all, Role::TabPanel, "Main Properties").unwrap());
            if left.top() == last {
                return (
                    left,
                    rect(with_role(&all, Role::TabPanel, "Pitch and Time Utilities").unwrap()),
                );
            }
            last = left.top();
        }
        panic!("the column never stopped scrolling");
    };
    let (left, _) = wheel(&mut h, 100.0);
    assert!(
        (at.left.top() - left.top() - 100.0).abs() < 1.0,
        "100 points down"
    );
    // It stops where the column ends.
    let (left, right) = wheel(&mut h, 5000.0);
    assert!((at.left.top() - left.top() - at.scrollable).abs() < 1.0);
    assert!(
        (right.bottom() - view.bottom()).abs() < 1.0,
        "the right group's end"
    );
    assert_eq!(h.shell.editor().undo_history().len(), steps, "no undo step");
}

fn menu_item(h: &Harness, row: &str) -> bl_ui::menus::ItemState {
    let bar = menu_bar(MenuPlatform::current());
    let item = bar
        .iter()
        .flat_map(bl_shell::menus::Menu::walk)
        .find(|item| item.row == row)
        .unwrap_or_else(|| panic!("{row} is in the menus"));
    item_state(item, &h.shell, &h.keymap, Platform::current())
}

// Spec: detail-views R20, R22 (M-VIEW-015 to M-VIEW-017). The View menu's
// three items run `clip_view.arrange_panels`, the one in use ticked, and the
// choice is no part of the set.
#[test]
fn the_view_menu_ticks_and_switches_the_arrangement() {
    let (mut h, _) = session_clip();
    let steps = h.shell.editor().undo_history().len();
    let rows = [
        ("M-VIEW-015", "automatic"),
        ("M-VIEW-016", "horizontal"),
        ("M-VIEW-017", "vertical"),
    ];
    let ticks = |h: &Harness| rows.map(|(row, _)| menu_item(h, row).checked);
    assert_eq!(
        ticks(&h),
        [Some(true), Some(false), Some(false)],
        "a new install"
    );
    for (row, name) in rows.into_iter().rev() {
        let item = menu_item(&h, row);
        assert!(item.enabled, "{row}");
        let invocation = item.invocation.unwrap();
        assert_eq!(invocation.id, "clip_view.arrange_panels");
        assert_eq!(invocation.args, json!({ "arrangement": name }));
        h.shell.run(&invocation.id, invocation.args).unwrap();
        let expected = rows.map(|(_, other)| Some(other == name));
        assert_eq!(ticks(&h), expected, "{name}");
    }
    assert_eq!(h.shell.editor().undo_history().len(), steps);
}

// Spec: detail-views R20, R23 (C-CLIPTITLE-004 to C-CLIPTITLE-007). The title
// bar's menu has the same items, and Fold ticked while the panels are
// folded.
#[test]
fn the_title_bars_menu_arranges_and_folds_the_panels() {
    let (mut h, _) = session_clip();
    let (_, _, at) = layout_of(&mut h);
    let click_item = |h: &mut Harness, label: &str| {
        let all = nodes(h);
        let item = all
            .iter()
            // A ticked item reads "✔ Fold".
            .rfind(|n| {
                n.role == Role::Button
                    && n.bounds.is_some()
                    && (n.label.starts_with(label) || n.label == format!("\u{2714} {label}"))
            })
            .unwrap_or_else(|| {
                let names: Vec<_> = all.iter().filter(|n| n.role == Role::Button).map(|n| n.label.clone()).collect();
                panic!("no menu item {label:?} in {names:?}")
            })
            .clone();
        h.click(rect(&item).center());
        h.settle();
    };
    let open = |h: &mut Harness| {
        let at = at.title.center();
        h.frame(vec![Event::PointerMoved(at)]);
        for pressed in [true, false] {
            h.frame(vec![Event::PointerButton {
                pos: at,
                button: PointerButton::Secondary,
                pressed,
                modifiers: Modifiers::NONE,
            }]);
        }
        h.settle();
    };
    open(&mut h);
    click_item(&mut h, "Arrange Clip View Panels Vertically");
    assert_eq!(
        h.shell.clip_view().panel_arrangement(),
        PanelArrangement::Vertical
    );
    open(&mut h);
    click_item(&mut h, "Fold");
    assert!(h.shell.clip_view().panels_folded());
    assert_eq!(
        h.shell.switch_state("clip_view.fold_panels"),
        Some(true),
        "ticked"
    );
    let (_, _, folded) = layout_of(&mut h);
    open(&mut h);
    let _ = folded;
    click_item(&mut h, "Fold");
    assert!(!h.shell.clip_view().panels_folded());
}

// Spec: detail-views R21 (I-DRAG-029). A drag of the panels' right edge to
// the left chooses Vertically only past half the right group's width, and
// back to the right chooses Horizontally; a shorter drag springs back.
#[test]
fn dragging_the_panels_edge_switches_only_past_its_distance() {
    let (mut h, _) = session_clip();
    arrange(&mut h, "horizontal");
    let (_, _, at) = layout_of(&mut h);
    let from = at.edge.center();
    let steps = h.shell.editor().undo_history().len();
    drag(&mut h, from, from - vec2(60.0, 0.0));
    assert_eq!(
        h.shell.clip_view().panel_arrangement(),
        PanelArrangement::Horizontal,
        "60 is short"
    );
    drag(&mut h, from, from + vec2(120.0, 0.0));
    assert_eq!(
        h.shell.clip_view().panel_arrangement(),
        PanelArrangement::Horizontal,
        "the wrong way changes nothing"
    );
    drag(&mut h, from, from - vec2(100.0, 0.0));
    assert_eq!(
        h.shell.clip_view().panel_arrangement(),
        PanelArrangement::Vertical
    );
    let (_, _, stacked) = layout_of(&mut h);
    let from = stacked.edge.center();
    drag(&mut h, from, from + vec2(60.0, 0.0));
    assert_eq!(
        h.shell.clip_view().panel_arrangement(),
        PanelArrangement::Vertical
    );
    drag(&mut h, from, from + vec2(100.0, 0.0));
    assert_eq!(
        h.shell.clip_view().panel_arrangement(),
        PanelArrangement::Horizontal
    );
    assert_eq!(h.shell.editor().undo_history().len(), steps, "no undo step");
}

// Spec: detail-views R23 (I-CLICK-008). A double-click on the title bar
// folds the panels, the editor takes the Clip View's width, and another
// unfolds them.
#[test]
fn a_double_click_on_the_title_bar_folds_and_unfolds_the_panels() {
    let (mut h, _) = session_clip();
    let (view, _, open) = layout_of(&mut h);
    let steps = h.shell.editor().undo_history().len();
    double_click(&mut h, open.title.left_center() + vec2(60.0, 0.0));
    assert!(h.shell.clip_view().panels_folded());
    let all = nodes(&mut h);
    assert!(
        with_role(&all, Role::TabList, "Properties Panels").is_none(),
        "the selector hides"
    );
    assert!(
        with_role(&all, Role::TabPanel, "Main Properties").is_none(),
        "and the panels"
    );
    let (_, _, folded) = layout_of(&mut h);
    assert_eq!(folded.title.width(), view.width(), "the title bar stays");
    assert_eq!(
        folded.editor.width(),
        view.width(),
        "the editor takes the width"
    );
    assert!(!overlap(folded.title, folded.editor));
    double_click(&mut h, folded.title.left_center() + vec2(60.0, 0.0));
    assert!(!h.shell.clip_view().panels_folded());
    assert_eq!(h.shell.editor().undo_history().len(), steps, "no undo step");
}

fn focused(h: &Harness) -> Option<Id> {
    h.ctx.memory(|m| m.focused())
}

// Spec: detail-views R24 (S-FOCUS-009, S-FOCUS-010). Both focus keys unfold
// the panels first; Option+8 goes to the first control of the shown panel of
// the group the focus was last in.
#[test]
fn the_focus_keys_unfold_the_panels_and_option_8_follows_the_last_group() {
    let (mut h, _) = session_clip();
    h.shell.set_focused_area(bl_shell::Area::MainView);
    h.shell
        .run("clip_view.fold_panels", json!({ "folded": true }))
        .unwrap();
    h.key(Key::P, Modifiers::ALT | Modifiers::SHIFT);
    h.settle();
    assert!(
        !h.shell.clip_view().panels_folded(),
        "Option+Shift+P unfolds"
    );
    assert_eq!(focused(&h), Some(Id::new(("number-box", "Start"))));

    // A value being typed keeps the keys for itself, so the focus leaves the
    // field first, to a tab of the left group.
    h.ctx
        .memory_mut(|m| m.request_focus(Id::new(("launch-panel-selector", 0))));
    h.settle();
    h.shell.set_focused_area(bl_shell::Area::MainView);
    h.shell
        .run("clip_view.fold_panels", json!({ "folded": true }))
        .unwrap();
    h.key(Key::Num8, Modifiers::ALT);
    h.settle();
    assert!(!h.shell.clip_view().panels_folded(), "Option+8 unfolds");
    assert_eq!(
        focused(&h),
        Some(Id::new(("number-box", "Start"))),
        "the left group, where the focus was last"
    );

    // With the focus last in the right group, Option+8 goes to its panel's
    // first control: the Transformation Selector.
    h.shell
        .run("clip_view.show_panel", json!({ "panel": "transform" }))
        .unwrap();
    h.settle();
    h.ctx
        .memory_mut(|m| m.request_focus(Id::new(("clip-view-panel-selector", false, 1))));
    h.settle();
    h.ctx
        .memory_mut(|m| m.surrender_focus(Id::new(("clip-view-panel-selector", false, 1))));
    h.settle();
    h.shell.set_focused_area(bl_shell::Area::MainView);
    h.key(Key::Num8, Modifiers::ALT);
    h.settle();
    let all = nodes(&mut h);
    let panel = rect(with_role(&all, Role::TabPanel, "MIDI Transformation Tools").unwrap());
    let id = focused(&h).expect("a control has the focus");
    let control = all
        .iter()
        .find(|n| n.id == id.accesskit_id())
        .expect("it is in the tree");
    assert!(
        panel.contains_rect(rect(control)),
        "{:?} is in the right panel",
        control.label
    );
    assert_ne!(control.role, Role::Tab, "and it is a control, not a tab");
}

// Spec: detail-views R24. In the selector row Left and Right move between
// the buttons and stop at the ends, and Enter or Space show the focused
// button's panel.
#[test]
fn the_selector_row_takes_left_right_enter_and_space() {
    let (mut h, _) = session_clip();
    let main = Id::new(("launch-panel-selector", 0));
    let launch = Id::new(("launch-panel-selector", 1));
    h.ctx.memory_mut(|m| m.request_focus(main));
    h.settle();
    h.key(Key::ArrowLeft, Modifiers::NONE);
    assert_eq!(focused(&h), Some(main), "Left stops at the first button");
    h.key(Key::ArrowRight, Modifiers::NONE);
    assert_eq!(focused(&h), Some(launch));
    h.key(Key::ArrowRight, Modifiers::NONE);
    assert_eq!(focused(&h), Some(launch), "Right stops at the last");
    assert_eq!(
        h.shell
            .clip_view()
            .shown_properties(h.shell.editor().document(), h.shell.arrangement()),
        bl_shell::clip_properties_commands::Properties::Main,
        "moving shows nothing"
    );
    h.key(Key::Enter, Modifiers::NONE);
    h.settle();
    assert_eq!(
        h.shell
            .clip_view()
            .shown_properties(h.shell.editor().document(), h.shell.arrangement()),
        bl_shell::clip_properties_commands::Properties::Launch,
        "Enter shows Launch Properties"
    );
    h.ctx.memory_mut(|m| m.request_focus(main));
    h.settle();
    h.key(Key::Space, Modifiers::NONE);
    h.settle();
    assert_eq!(
        h.shell
            .clip_view()
            .shown_properties(h.shell.editor().document(), h.shell.arrangement()),
        bl_shell::clip_properties_commands::Properties::Main,
        "Space shows Main Properties"
    );
    // The right group's row works the same way.
    let first = Id::new(("clip-view-panel-selector", false, 0));
    let second = Id::new(("clip-view-panel-selector", false, 1));
    h.ctx.memory_mut(|m| m.request_focus(first));
    h.settle();
    h.key(Key::ArrowRight, Modifiers::NONE);
    assert_eq!(focused(&h), Some(second));
    h.key(Key::Enter, Modifiers::NONE);
    h.settle();
    assert_eq!(
        h.shell.clip_view().midi_tools().panel(),
        bl_shell::midi_tool_commands::Panel::Transform
    );
}

// Spec: detail-views R25. The selector row is two tab lists, each tab named
// for its panel and selected while it shows, each panel is a tab panel, and
// the panels' edge is a splitter whose increment and decrement choose
// Horizontally and Vertically.
#[test]
fn assistive_technology_reads_two_tab_lists_the_panels_and_a_splitter() {
    let (mut h, _) = session_clip();
    let all = nodes(&mut h);
    for list in ["Properties Panels", "Tool Panels"] {
        assert!(with_role(&all, Role::TabList, list).is_some(), "{list}");
    }
    let tab = |label: &str| {
        with_role(&all, Role::Tab, label).unwrap_or_else(|| panic!("no tab {label:?}"))
    };
    assert_eq!(tab("Main Properties").selected, Some(true));
    assert_eq!(tab("Launch Properties").selected, Some(false));
    assert_eq!(tab("Pitch and Time Utilities").selected, Some(true));
    assert_eq!(tab("MIDI Transformation Tools").selected, Some(false));
    assert_eq!(tab("MIDI Generative Tools").selected, Some(false));
    for panel in ["Main Properties", "Pitch and Time Utilities"] {
        assert!(with_role(&all, Role::TabPanel, panel).is_some(), "{panel}");
    }
    let splitter = with_role(&all, Role::Splitter, "Clip View Panels").expect("the edge");
    assert_eq!(splitter.numeric, Some(1.0), "horizontal");
    let id = Id::new("clip-panels-edge");
    h.accesskit(id, Action::Decrement);
    assert_eq!(
        h.shell.clip_view().panel_arrangement(),
        PanelArrangement::Vertical
    );
    let all = nodes(&mut h);
    assert_eq!(
        with_role(&all, Role::Splitter, "Clip View Panels")
            .unwrap()
            .numeric,
        Some(0.0)
    );
    h.accesskit(id, Action::Increment);
    assert_eq!(
        h.shell.clip_view().panel_arrangement(),
        PanelArrangement::Horizontal
    );
    // Folded, the selector and the splitter go.
    h.shell
        .run("clip_view.fold_panels", json!({ "folded": true }))
        .unwrap();
    let all = nodes(&mut h);
    assert!(with_role(&all, Role::Splitter, "Clip View Panels").is_none());
}

// Spec: detail-views R24 (S-FOCUS-009). Option+8 (Alt+8 elsewhere) is
// `focus.selected_clip_panel`.
#[test]
fn the_keymap_reaches_the_selected_clip_panel_with_option_8() {
    let keymap = bl_ui::keymap::Keymap::live12();
    for platform in [bl_ui::keymap::Platform::Mac, bl_ui::keymap::Platform::Other] {
        let hit = keymap
            .resolve(
                platform,
                Key::Num8,
                bl_ui::keymap::Mods {
                    alt: true,
                    ..bl_ui::keymap::Mods::default()
                },
                &[bl_commands::Context::Global],
            )
            .expect("Option+8");
        assert_eq!(hit.command.id, "focus.selected_clip_panel", "{platform:?}");
    }
}

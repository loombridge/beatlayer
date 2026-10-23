//! The second window, headless (WP-6.29): its layout at its first size and at
//! its smallest, its Control Bar cut off, the keys going to the window in
//! front, the focus between the windows, the clicks that run commands for the
//! window they are in, and the two accessibility roots.
//!
//! Spec: second-window R1 to R17; interface-zoom R11 to R15.

mod common;

use bl_shell::menus::{MenuPlatform, menu_bar};
use bl_shell::{Area, Effect, MainView, WindowKind};
use bl_ui::keymap::Platform;
use bl_ui::layout::{
    Element, SECOND_MINIMUM, SECOND_WINDOW, ShellLayout, TITLE_BAR, fit_second_window,
    minimum_second_window,
};
use bl_ui::main_window::{UiEvent, window_title};
use bl_ui::menus::{ItemState, item_state};
use common::{Harness, Node, Window};
use egui::accesskit::Role;
use egui::{Key, Modifiers, Pos2, Rect, pos2, vec2};
use serde_json::json;

fn layout(h: &Harness) -> ShellLayout {
    h.main.drawn_layout().expect("a frame was drawn").clone()
}

fn centre(h: &Harness, element: Element) -> Pos2 {
    layout(h).rect(element).center()
}

fn shows(h: &Harness, window: WindowKind) -> (bool, bool) {
    let detail = h.shell.detail_of(window);
    (detail.clip_shown(), detail.device_shown())
}

fn rect_of(node: &Node) -> Rect {
    let bounds = node.bounds.expect("the node has bounds");
    Rect::from_min_max(
        pos2(bounds.x0 as f32, bounds.y0 as f32),
        pos2(bounds.x1 as f32, bounds.y1 as f32),
    )
}

fn press(h: &mut Harness, key: Key, modifiers: Modifiers) {
    h.key(key, modifiers);
    h.settle();
}

/// A second window with its zoom set, at `size` window points with the
/// title bar.
fn second_at(zoom_percent: u16, size: Option<egui::Vec2>) -> Harness {
    let mut h = Harness::with_title_bar(Window::Second);
    h.shell
        .run(
            "view.set_zoom",
            json!({ "percent": zoom_percent, "window": "second" }),
        )
        .unwrap();
    let zoom = f32::from(zoom_percent) / 100.0;
    let first = vec2(SECOND_WINDOW.x, SECOND_WINDOW.y);
    h.size = size.unwrap_or_else(|| fit_second_window(first, zoom, true, None).1);
    h.settle();
    h
}

#[test]
fn the_second_window_is_990_by_703_points_the_first_time() {
    // Spec: second-window R1; L-WIN2-001.
    let h = second_at(100, None);
    assert_eq!(h.size, vec2(990.0, 703.0));
    let layout = layout(&h);
    assert_eq!(layout.content.size(), vec2(990.0, 703.0 - TITLE_BAR));
}

#[test]
fn it_lays_out_at_its_first_size_and_its_smallest_at_50_100_and_200_percent() {
    // Spec: second-window R2, R17; interface-zoom R15; L-WIN2-002.
    for percent in [50u16, 100, 200] {
        let zoom = f32::from(percent) / 100.0;
        let smallest = minimum_second_window(zoom, true);
        assert_eq!(smallest, SECOND_MINIMUM * zoom + vec2(0.0, TITLE_BAR));
        for (what, size) in [("first size", None), ("smallest size", Some(smallest))] {
            let h = second_at(percent, size);
            let what = format!("{what} at {percent}%");
            let layout = layout(&h);
            let window = layout.content;
            // The window's panes are laid out for its own width, none past
            // its right edge and none over another.
            let mut panes = vec![
                ("Control Bar", layout.control_bar),
                ("main view", layout.main_view),
                ("Status Bar", layout.status_bar),
            ];
            if let Some(clip) = layout.clip_view {
                panes.push(("Clip View", clip));
            }
            if let Some(device) = layout.device_view {
                panes.push(("Device View", device));
            }
            for (name, pane) in &panes {
                assert!(
                    window.contains_rect(*pane),
                    "{what}: the {name} {pane:?} is inside the window {window:?}"
                );
            }
            for (i, (a, pa)) in panes.iter().enumerate() {
                for (b, pb) in &panes[i + 1..] {
                    assert!(
                        !pa.shrink(0.01).intersects(pb.shrink(0.01)),
                        "{what}: the {a} and the {b} overlap: {pa:?} {pb:?}"
                    );
                }
            }
            // The Status Bar's controls all fit in the window and don't
            // overlap.
            let status: Vec<(Element, Rect)> = layout
                .elements
                .iter()
                .copied()
                .filter(|(element, _)| {
                    bl_ui::layout::placement(*element).strip == bl_ui::layout::Strip::StatusBar
                })
                .collect();
            for (element, rect) in &status {
                assert!(
                    layout.status_bar.contains_rect(*rect),
                    "{what}: {element:?} {rect:?} is in the Status Bar"
                );
            }
            for (i, (a, ra)) in status.iter().enumerate() {
                for (b, rb) in &status[i + 1..] {
                    assert!(
                        !ra.shrink(0.01).intersects(rb.shrink(0.01)),
                        "{what}: {a:?} and {b:?} overlap"
                    );
                }
            }
            // The Control Bar's controls are cut off at its right edge
            // rather than moved: still in order, those past the edge out of
            // view, and none overlapping.
            let bar: Vec<(Element, Rect)> = layout
                .elements
                .iter()
                .copied()
                .filter(|(element, _)| {
                    bl_ui::layout::placement(*element).strip == bl_ui::layout::Strip::ControlBar
                })
                .collect();
            assert!(
                bar.windows(2)
                    .all(|pair| pair[0].1.left() <= pair[1].1.left()),
                "{what}: the Control Bar keeps its order"
            );
            assert!(
                window.width() >= bl_ui::layout::MINIMUM.x
                    || bar.iter().any(|(_, rect)| rect.right() > window.right()),
                "{what}: the far right of the Control Bar is cut off"
            );
            assert!(
                bar.iter()
                    .take(3)
                    .all(|(_, rect)| window.contains_rect(*rect)),
                "{what}: its left end shows"
            );
            for (i, (a, ra)) in bar.iter().enumerate() {
                for (b, rb) in &bar[i + 1..] {
                    assert!(
                        !ra.shrink(0.01).intersects(rb.shrink(0.01)),
                        "{what}: {a:?} and {b:?} overlap"
                    );
                }
            }
        }
    }
}

#[test]
fn the_control_bar_is_measured_from_the_windows_left_edge_like_the_main_windows() {
    // Spec: second-window R17; L-WIN2-002. The left end of the second
    // window's Control Bar is where the main window's is at its smallest.
    let second = ShellLayout::compute_second(
        vec2(990.0, 703.0 - TITLE_BAR),
        false,
        bl_ui::layout::DetailViews::default(),
    );
    let main = ShellLayout::compute(bl_ui::layout::MINIMUM, false, false);
    for element in [
        Element::BrowserToggle,
        Element::Tempo,
        Element::Play,
        Element::LoopLengthSixteenths,
        Element::SessionView,
    ] {
        assert_eq!(second.rect(element), main.rect(element), "{element:?}");
    }
    // Its Status Bar spans its own width.
    assert_eq!(second.status_bar.right(), 990.0);
}

#[test]
fn it_has_no_assistant_toggle_and_no_info_view() {
    // Spec: second-window R2, R12.
    let mut h = second_at(100, None);
    let output = h.settle();
    let nodes = Harness::nodes(&output);
    assert!(
        !nodes.iter().any(|n| n.label == "Show/Hide Assistant"),
        "the assistant's toggle stays the main window's"
    );
    assert!(layout(&h).info_view.is_none());
    assert!(nodes.iter().any(|n| n.label == "Status Bar"));
    assert!(nodes.iter().any(|n| n.label == "Control Bar"));
}

#[test]
fn it_shows_the_other_main_view_and_a_click_on_a_selector_swaps_them() {
    // Spec: second-window R4; L-CTRL-027. The Control Bar is cut off at its
    // default width, so the window is widened to reach the view selector.
    let mut h = Harness::with_title_bar(Window::Main);
    h.open_other();
    h.other.as_mut().unwrap().size = vec2(1340.0, 700.0);
    h.settle();
    h.other_settle();
    let output = h.other_settle();
    let nodes = Harness::nodes(&output);
    assert!(
        nodes
            .iter()
            .any(|n| n.label == "Session View" && n.role == Role::Pane)
    );
    assert_eq!(h.shell.main_view(), MainView::Arrangement);
    // A click on the view the window shows changes nothing.
    let session = h.other_layout().rect(Element::SessionView).center();
    h.other_click(session);
    assert_eq!(h.shell.main_view(), MainView::Arrangement);
    // A click on the other view swaps them.
    let arrangement = h.other_layout().rect(Element::ArrangementView).center();
    h.other_click(arrangement);
    h.other_settle();
    assert_eq!(h.shell.main_view(), MainView::Session);
    assert_eq!(
        h.shell.main_view_of(WindowKind::Second),
        MainView::Arrangement
    );
    assert!(!h.shell.is_dirty());
    // Tab swaps them back from either window.
    h.other_key(Key::Tab, Modifiers::NONE);
    assert_eq!(h.shell.main_view(), MainView::Arrangement);
    h.frame(vec![]);
    h.key(Key::Tab, Modifiers::NONE);
    assert_eq!(h.shell.main_view(), MainView::Session);
}

#[test]
fn a_key_goes_to_the_window_in_front() {
    // Spec: second-window R10. F12 moves what the window in front shows.
    let mut h = Harness::with_title_bar(Window::Main);
    h.open_other();
    h.shell
        .run("view.toggle_clip_view", json!({ "window": "second" }))
        .unwrap();
    h.settle();
    h.other_settle();
    assert_eq!(shows(&h, WindowKind::Main), (false, true));
    assert_eq!(shows(&h, WindowKind::Second), (true, false));
    // In the second window: its Clip View moves to the main window.
    h.other_key(Key::F12, Modifiers::NONE);
    assert_eq!(h.shell.focused_window(), WindowKind::Second);
    assert_eq!(shows(&h, WindowKind::Main), (true, true));
    assert_eq!(shows(&h, WindowKind::Second), (false, false));
    // In the main window: its two views move to the second window.
    h.key(Key::F12, Modifiers::NONE);
    assert_eq!(h.shell.focused_window(), WindowKind::Main);
    assert_eq!(shows(&h, WindowKind::Main), (false, false));
    assert_eq!(shows(&h, WindowKind::Second), (true, true));
    assert!(!h.shell.is_dirty());
}

#[test]
fn option_1_and_option_2_move_the_focus_between_the_windows() {
    // Spec: second-window R11; S-FOCUS-002, S-FOCUS-003.
    let mut h = Harness::with_title_bar(Window::Main);
    h.open_other();
    h.settle();
    h.other_settle();
    assert_eq!(h.shell.main_view(), MainView::Arrangement);
    // The Session View is in the second window.
    press(&mut h, Key::Num1, Modifiers::ALT);
    assert_eq!(h.shell.focused_window(), WindowKind::Second);
    assert_eq!(h.shell.focused_area_of(WindowKind::Second), Area::MainView);
    assert!(h.effects.contains(&Effect::FocusSecondArea(Area::MainView)));
    // Option+2 from the second window brings the main window back.
    h.other_key(Key::Num2, Modifiers::ALT);
    assert_eq!(h.shell.focused_window(), WindowKind::Main);
    assert_eq!(
        h.shell.main_view(),
        MainView::Arrangement,
        "no view changed"
    );
    assert_eq!(h.shell.main_view_of(WindowKind::Second), MainView::Session);
    // And Option+1 pressed in the second window stays there.
    h.other_key(Key::Num1, Modifiers::ALT);
    assert_eq!(h.shell.focused_window(), WindowKind::Second);
}

#[test]
fn clicks_in_the_second_window_run_their_commands_for_it() {
    // Spec: second-window R7, R15. The Status Bar's Clip View toggle and
    // selector, and the Option-click that shows both views.
    let mut h = second_at(100, None);
    // Even with the main window in front last, the clicks name their window.
    h.shell.set_focused_window(WindowKind::Main);
    let toggle = centre(&h, Element::ClipViewToggle);
    h.click(toggle);
    h.settle();
    assert_eq!(shows(&h, WindowKind::Second), (true, false));
    assert_eq!(shows(&h, WindowKind::Main), (false, true));
    let invoked: Vec<_> = h
        .take_events()
        .into_iter()
        .filter_map(|event| match event {
            UiEvent::Invoke(i) if i.id == "view.toggle_clip_view" => Some(i.args),
            _ => None,
        })
        .collect();
    assert_eq!(invoked, [json!({ "window": "second" })]);
    // Its Device View toggle brings the Device View across.
    let toggle = centre(&h, Element::DeviceViewToggle);
    h.click(toggle);
    h.settle();
    assert_eq!(shows(&h, WindowKind::Second), (true, true));
    assert_eq!(shows(&h, WindowKind::Main), (false, false));
    // The layout stacks them in the second window.
    let layout = layout(&h);
    let (clip, device) = (layout.clip_view.unwrap(), layout.device_view.unwrap());
    assert!(clip.bottom() <= device.top());
    assert_eq!(device.bottom(), layout.detail.bottom());
}

#[test]
fn closing_it_returns_the_views_and_the_main_window_keeps_its_view() {
    // Spec: second-window R9, R1. Cmd+W with it in front closes it.
    let mut h = Harness::with_title_bar(Window::Main);
    h.open_other();
    h.shell
        .run("view.show_detail_views", json!({ "window": "second" }))
        .unwrap();
    h.shell
        .run("view.toggle_session_arrangement", json!({}))
        .unwrap();
    h.other_settle();
    assert_eq!(shows(&h, WindowKind::Second), (true, true));
    h.other_key(Key::W, common::command());
    assert!(!h.shell.second_window_open());
    assert_eq!(shows(&h, WindowKind::Main), (true, true));
    assert_eq!(h.shell.main_view(), MainView::Session);
}

#[test]
fn each_window_is_a_root_of_the_accessibility_tree_and_names_itself() {
    // Spec: second-window R16.
    let mut h = Harness::with_title_bar(Window::Main);
    h.open_other();
    h.settle();
    h.other_settle();
    let main = Harness::nodes(&h.settle());
    let second = Harness::nodes(&h.other_settle());
    for (name, nodes) in [("main", &main), ("second", &second)] {
        let roots: Vec<_> = nodes.iter().filter(|n| n.role == Role::Window).collect();
        assert_eq!(roots.len(), 1, "{name}: one root");
        assert!(
            !roots[0].children.is_empty(),
            "{name}: the root holds the window"
        );
        for area in ["Control Bar", "Status Bar"] {
            assert!(
                Harness::descends_from(nodes, Harness::node(nodes, area).id, roots[0].id),
                "{name}: the {area} hangs from its root"
            );
        }
    }
    // The first named for the set, the second "<set name>, Second Window".
    let set = h.shell.set_name();
    assert_eq!(window_title(&h.shell, WindowKind::Main), set);
    assert_eq!(
        window_title(&h.shell, WindowKind::Second),
        format!("{set}, Second Window")
    );
    // Each draws the title it announces (macOS draws its title bar).
    let shapes_text = |output: &egui::FullOutput, text: &str| {
        output
            .shapes
            .iter()
            .any(|clipped| text_in(&clipped.shape, text))
    };
    let output = h.other_settle();
    assert!(shapes_text(&output, &format!("{set}, Second Window")));
}

fn text_in(shape: &egui::Shape, wanted: &str) -> bool {
    match shape {
        egui::Shape::Text(text) => text.galley.text() == wanted,
        egui::Shape::Vec(shapes) => shapes.iter().any(|shape| text_in(shape, wanted)),
        _ => false,
    }
}

/// Every rectangle drawn with the focus colour in a frame.
fn focus_strokes(output: &egui::FullOutput, colour: egui::Color32) -> Vec<Rect> {
    fn walk(shape: &egui::Shape, colour: egui::Color32, out: &mut Vec<Rect>) {
        match shape {
            egui::Shape::Rect(rect) if rect.stroke.color == colour && rect.stroke.width > 0.0 => {
                out.push(rect.rect);
            }
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    walk(shape, colour, out);
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    for clipped in &output.shapes {
        walk(&clipped.shape, colour, &mut out);
    }
    out
}

#[test]
fn only_the_window_in_front_outlines_its_focused_area() {
    // Spec: second-window R16; accessibility R7.
    let mut h = Harness::with_title_bar(Window::Main);
    h.open_other();
    h.settle();
    h.other_settle();
    let colour = h.theme.colors.focus.0;
    assert!(h.shell.outline_view_in_focus());
    // The main window is in front.
    h.shell.set_focused_window(WindowKind::Main);
    let main = h.settle();
    let second = h.other_settle();
    let view = layout(&h).main_view.shrink(1.0);
    let around = |strokes: &[Rect], area: Rect| {
        strokes
            .iter()
            .any(|r| (r.min - area.min).length() < 1.5 && (r.max - area.max).length() < 1.5)
    };
    assert!(around(&focus_strokes(&main, colour), view));
    let second_view = h.other_layout().main_view.shrink(1.0);
    assert!(
        !around(&focus_strokes(&second, colour), second_view),
        "the window behind draws no outline"
    );
    // The second window comes to the front.
    h.shell.set_focused_window(WindowKind::Second);
    let main = h.settle();
    let second = h.other_settle();
    assert!(around(&focus_strokes(&second, colour), second_view));
    assert!(!around(&focus_strokes(&main, colour), view));
}

#[test]
fn the_second_window_zooms_on_its_own() {
    // Spec: interface-zoom R11, R13, R15. A zoom key zooms the window in
    // front; the other window's layout points don't change.
    let mut h = Harness::with_title_bar(Window::Main);
    h.open_other();
    h.settle();
    h.other_settle();
    let before = h.other_layout().content;
    press(&mut h, Key::Plus, common::command());
    assert_eq!(h.shell.zoom_percent(), 110, "the main window zoomed");
    assert_eq!(h.shell.zoom_percent_of(WindowKind::Second), 100);
    h.other_key(Key::Plus, common::command());
    assert_eq!(h.shell.zoom_percent_of(WindowKind::Second), 110);
    h.other_key(Key::Plus, common::command());
    assert_eq!(h.shell.zoom_percent_of(WindowKind::Second), 120);
    assert_eq!(h.shell.zoom_percent(), 110);
    h.other_settle();
    // 120 % of the window's points shows fewer layout points.
    assert!(h.other_layout().content.width() < before.width());
}

#[test]
fn display_and_input_shows_second_window_zoom_only_while_it_is_open() {
    // Spec: interface-zoom R12.
    let mut h = Harness::with_title_bar(Window::Settings);
    h.shell
        .invoke("app.settings", json!({ "page": "display-input" }))
        .unwrap();
    h.shell.take_effects();
    h.settings.refresh();
    let labels = |h: &mut Harness| {
        Harness::nodes(&h.settle())
            .into_iter()
            .map(|n| n.label)
            .collect::<Vec<_>>()
    };
    assert!(!labels(&mut h).iter().any(|l| l == "Second Window Zoom"));
    h.shell.run("view.toggle_second_window", json!({})).unwrap();
    h.settings.refresh();
    let nodes = Harness::nodes(&h.settle());
    let zoom = Harness::node(&nodes, "Zoom");
    let second = Harness::node(&nodes, "Second Window Zoom");
    assert!(rect_of(second).top() > rect_of(zoom).top(), "after Zoom");
    h.shell.take_effects();
    // It sets the second window's zoom, and Zoom stays the main window's.
    h.take_events();
    let id = egui::Id::new(("number-box", "second-window-zoom"));
    h.accesskit(id, egui::accesskit::Action::Increment);
    h.settle();
    assert_eq!(h.shell.zoom_percent_of(WindowKind::Second), 110);
    assert_eq!(h.shell.zoom_percent(), 100);
    h.shell.run("view.toggle_second_window", json!({})).unwrap();
    h.settings.refresh();
    assert!(!labels(&mut h).iter().any(|l| l == "Second Window Zoom"));
}

fn menu_item(h: &Harness, row: &str) -> ItemState {
    let bar = menu_bar(MenuPlatform::current());
    let item = bar
        .iter()
        .flat_map(bl_shell::menus::Menu::walk)
        .find(|item| item.row == row)
        .unwrap_or_else(|| panic!("{row} is in the menus"));
    item_state(item, &h.shell, &h.keymap, Platform::current())
}

#[test]
fn view_second_window_is_ticked_while_it_is_open_and_the_zoom_items_follow_the_window() {
    // Spec: second-window R1; interface-zoom R13. M-VIEW-002 runs
    // `view.toggle_second_window`; the Zoom submenu's items act on the window
    // in front and are disabled at that window's ends.
    let mut h = Harness::new(Window::Main);
    h.settle();
    let item = menu_item(&h, "M-VIEW-002");
    assert_eq!(item.label, "Second Window");
    assert_eq!(item.checked, Some(false));
    assert!(item.enabled);
    assert_eq!(item.invocation.unwrap().id, "view.toggle_second_window");
    h.shell.run("view.toggle_second_window", json!({})).unwrap();
    assert_eq!(menu_item(&h, "M-VIEW-002").checked, Some(true));
    // The second window is in front: its zoom is the one at its end.
    h.shell
        .run("view.set_zoom", json!({ "percent": 200 }))
        .unwrap();
    assert_eq!(h.shell.zoom_percent_of(WindowKind::Second), 200);
    assert!(!menu_item(&h, "M-VIEW-003a").enabled, "Zoom In at its end");
    assert!(menu_item(&h, "M-VIEW-003b").enabled);
    h.shell.set_focused_window(WindowKind::Main);
    assert!(
        menu_item(&h, "M-VIEW-003a").enabled,
        "the main window's own"
    );
    // Clip View's tick follows the window in front.
    h.shell
        .run("view.toggle_clip_view", json!({ "window": "second" }))
        .unwrap();
    assert_eq!(menu_item(&h, "M-VIEW-010").checked, Some(false));
    h.shell.set_focused_window(WindowKind::Second);
    assert_eq!(menu_item(&h, "M-VIEW-010").checked, Some(true));
}

#[test]
fn the_main_windows_info_view_describes_what_the_pointer_is_over_in_the_second_window() {
    // Spec: info-view R8, R4. The second window has no Info View of its
    // own.
    let mut h = Harness::with_title_bar(Window::Main);
    h.open_other();
    h.settle();
    h.other_settle();
    assert!(layout(&h).info_view.is_some());
    assert!(h.main.info_view_content().title.is_empty());
    // Over Play in the second window.
    let play = h.other_layout().rect(Element::Play).center();
    h.other_frame(vec![egui::Event::PointerMoved(play)]);
    h.other_settle();
    h.settle();
    let info = h.main.info_view_content().clone();
    assert_eq!(info.title, "Play");
    assert!(!info.body.is_empty(), "the registered description");
    // The pointer leaves for empty space: nothing shows.
    h.other_frame(vec![egui::Event::PointerGone]);
    h.other_settle();
    h.settle();
    assert!(h.main.info_view_content().title.is_empty());
}

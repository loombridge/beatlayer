//! The browser's right edge, headless: dragging it widens and narrows the
//! browser, the main view keeps its least width, a narrowed browser
//! narrows its content pane and then its sidebar, a drag far left hides the
//! browser and back shows it, and assistive technology finds a splitter.
//!
//! Spec: browser R29.

use crate::common;
use bl_shell::commands::{BROWSER_MIN_WIDTH, BROWSER_WIDTH, MAIN_VIEW_MIN_WIDTH};
use bl_ui::layout::{BrowserLayout, CONTENT_MIN_WIDTH, MARGIN, ShellLayout};
use common::{Harness, Window};
use egui::accesskit::{Action, Role};
use egui::{Event, Id, Modifiers, PointerButton, Pos2, pos2};
use serde_json::json;

fn layout(h: &Harness) -> ShellLayout {
    h.main
        .drawn_layout()
        .expect("the last frame's layout")
        .clone()
}

fn browser_width(h: &Harness) -> f32 {
    layout(h).browser.expect("the browser shows").width()
}

/// The edge's grab point: the middle of the gap right of the browser.
fn edge(h: &Harness) -> Pos2 {
    let browser = layout(h).browser.expect("the browser shows");
    pos2(browser.right() + MARGIN / 2.0, browser.center().y)
}

/// A press at the first point, the pointer moving through the rest, and a
/// release at the last.
fn drag_through(h: &mut Harness, points: &[Pos2]) {
    let (first, last) = (points[0], points[points.len() - 1]);
    h.frame(vec![Event::PointerMoved(first)]);
    h.frame(vec![Event::PointerButton {
        pos: first,
        button: PointerButton::Primary,
        pressed: true,
        modifiers: Modifiers::NONE,
    }]);
    for pair in points.windows(2) {
        for step in 1..=4 {
            h.frame(vec![Event::PointerMoved(
                pair[0] + (pair[1] - pair[0]) * (step as f32 / 4.0),
            )]);
        }
    }
    h.frame(vec![Event::PointerButton {
        pos: last,
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    }]);
    h.settle();
}

#[test]
fn dragging_the_browsers_edge_widens_and_narrows_it() {
    let mut h = Harness::new(Window::Main);
    h.settle();
    let steps = h.shell.editor().undo_history().len();
    assert_eq!(browser_width(&h), BROWSER_WIDTH);

    // 100 points right: 100 wider, and the main view starts 9 points
    // right of it.
    let grab = edge(&h);
    drag_through(&mut h, &[grab, grab + egui::vec2(100.0, 0.0)]);
    assert_eq!(h.shell.browser_view().width(), BROWSER_WIDTH + 100.0);
    assert_eq!(browser_width(&h), BROWSER_WIDTH + 100.0);
    let shown = layout(&h);
    assert_eq!(
        shown.main_view.left(),
        shown.browser.unwrap().right() + MARGIN
    );

    // Far right stops where the main view keeps its least width.
    let grab = edge(&h);
    let far = pos2(layout(&h).content.right(), grab.y);
    drag_through(&mut h, &[grab, far]);
    let widest = layout(&h);
    assert_eq!(widest.main_view.width(), MAIN_VIEW_MIN_WIDTH);
    assert_eq!(widest.browser.unwrap().width(), widest.largest_browser);

    // Back to 120 points wide: the content pane keeps its least width and
    // the sidebar narrows.
    let grab = edge(&h);
    let left = layout(&h).browser.unwrap().left();
    drag_through(&mut h, &[grab, pos2(left + 120.0 + MARGIN / 2.0, grab.y)]);
    assert_eq!(browser_width(&h), 120.0);
    let parts = BrowserLayout::compute(layout(&h).browser.unwrap());
    assert_eq!(parts.content.width(), CONTENT_MIN_WIDTH);
    assert!(parts.sidebar.width() < 100.0);
    assert!(parts.search.width() >= 0.0);

    assert!(!h.shell.is_dirty());
    assert_eq!(h.shell.editor().undo_history().len(), steps);
}

#[test]
fn a_drag_far_left_hides_the_browser_and_back_shows_it() {
    let mut h = Harness::new(Window::Main);
    h.settle();
    let left = layout(&h).browser.unwrap().left();

    // Past the middle of the narrowest browser and back again in one drag:
    // it hides, then shows at the width the pointer leaves it.
    let grab = edge(&h);
    let hidden = pos2(left + 10.0, grab.y);
    let back = pos2(left + 200.0 + MARGIN / 2.0, grab.y);
    drag_through(&mut h, &[grab, hidden, back]);
    assert!(h.shell.browser_of(bl_shell::WindowKind::Main));
    assert_eq!(browser_width(&h), 200.0);

    // Released there, it stays hidden, and shows again at its narrowest.
    let grab = edge(&h);
    drag_through(&mut h, &[grab, hidden]);
    assert!(!h.shell.browser_of(bl_shell::WindowKind::Main));
    assert!(layout(&h).browser.is_none());
    assert_eq!(layout(&h).main_view.left(), MARGIN);
    h.shell.run("view.toggle_browser", json!({})).unwrap();
    h.settle();
    assert_eq!(browser_width(&h), BROWSER_MIN_WIDTH);
}

#[test]
fn the_browsers_edge_is_a_splitter() {
    let mut h = Harness::new(Window::Main);
    h.settle();
    let nodes = Harness::nodes(&h.settle());
    let splitter = Harness::node(&nodes, "Browser Width");
    assert_eq!(splitter.role, Role::Splitter);
    assert_eq!(splitter.numeric, Some(f64::from(BROWSER_WIDTH)));
    let id = Id::new("browser-edge");
    h.accesskit(id, Action::Increment);
    h.settle();
    assert_eq!(h.shell.browser_view().width(), BROWSER_WIDTH + 10.0);
    h.accesskit(id, Action::Decrement);
    h.accesskit(id, Action::Decrement);
    h.settle();
    assert_eq!(h.shell.browser_view().width(), BROWSER_WIDTH - 10.0);
}

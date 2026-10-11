//! The interface zoom (WP-1.17), headless: at every zoom from 50% to 200%
//! every layout.md row the layout layer places scales by the zoom, keeps its
//! place relative to every other row, and overlaps none it doesn't overlap
//! at 100%, down to the minimum window size at that zoom; the windows draw
//! exactly that through egui's points-per-pixel; and the View menu's keys,
//! the Settings window's Zoom, and agents change it.
//!
//! Spec: interface-zoom R1 to R10. Run with `--nocapture` to print the
//! minimum window sizes.

mod common;

use bl_shell::ZOOM_STEPS;
use bl_shell::commands as ids;
use bl_shell::settings::{SettingsPage, ZOOM_MAX_PERCENT, ZOOM_MIN_PERCENT, zoom_factor};
use bl_ui::layout::{
    ArrangementLayout, ELEMENTS, MARGIN, MINIMUM, REFERENCE, Row, SETTINGS_WINDOW, SettingsLayout,
    ShellLayout, Strip, TITLE_BAR, fit_main_window, layout_size, minimum_window, settings_window,
};
use common::{Harness, Window, command};
use egui::accesskit::{Action, Role};
use egui::{Event, Id, Key, Modifiers, PointerButton, Pos2, Rect, Vec2, pos2, vec2};
use serde_json::json;

/// Floating-point slack for rectangles computed two ways, in points.
const EPS: f32 = 1e-3;

/// Every whole percentage from 50% to 200%, as zoom factors.
fn every_percent() -> impl Iterator<Item = f32> {
    (ZOOM_MIN_PERCENT..=ZOOM_MAX_PERCENT).map(zoom_factor)
}

/// Every zoom step, as factors.
fn every_step() -> impl Iterator<Item = f32> {
    ZOOM_STEPS.into_iter().map(zoom_factor)
}

/// Every row the main window's layout layer places in a content area of
/// `size` layout points: the panes and the Control Bar's and Status Bar's
/// controls, then the Arrangement View's parts in the main view (with the
/// Overview and the factory default set's two returns and Main pinned).
fn main_window_rows(size: Vec2, browser: bool) -> Vec<Row> {
    let shell = ShellLayout::compute(size, browser);
    let arrangement = ArrangementLayout::compute(shell.main_view, true, 3);
    let mut rows = shell.rows();
    rows.extend(arrangement.rows());
    rows
}

/// How two rectangles sit relative to each other along one axis.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Order {
    Before,
    After,
    Across,
}

fn order(a: (f32, f32), b: (f32, f32)) -> Order {
    if a.1 <= b.0 + EPS {
        Order::Before
    } else if b.1 <= a.0 + EPS {
        Order::After
    } else {
        Order::Across
    }
}

/// How `a` sits relative to `b`: their order across and down, and whether
/// either holds the other. Two rows that don't overlap are before or after
/// each other on at least one axis.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Relation {
    across: Order,
    down: Order,
    holds: bool,
    held: bool,
}

fn relation(a: Rect, b: Rect) -> Relation {
    let holds = |outer: Rect, inner: Rect| outer.expand(EPS).contains_rect(inner);
    Relation {
        across: order((a.left(), a.right()), (b.left(), b.right())),
        down: order((a.top(), a.bottom()), (b.top(), b.bottom())),
        holds: holds(a, b),
        held: holds(b, a),
    }
}

fn overlaps(relation: Relation) -> bool {
    relation.across == Order::Across && relation.down == Order::Across
}

/// Checks that every pair of rows relates at a zoom as it does at 100% in
/// the same window: the same order across and down, the same nesting, so a
/// row keeps its place relative to every other.
fn assert_relations_kept(at_100: &[Row], zoomed: &[Row], what: &str) {
    assert_eq!(at_100.len(), zoomed.len(), "{what}: the same rows");
    for i in 0..at_100.len() {
        assert_eq!(at_100[i].id, zoomed[i].id, "{what}");
        for j in (i + 1)..at_100.len() {
            let before = relation(at_100[i].rect, at_100[j].rect);
            let after = relation(zoomed[i].rect, zoomed[j].rect);
            assert_eq!(
                before,
                after,
                "{what}: {} ({}) against {} ({}): {:?} then {:?}",
                at_100[i].name,
                at_100[i].id,
                at_100[j].name,
                at_100[j].id,
                at_100[i].rect,
                zoomed[i].rect,
            );
        }
    }
}

/// Checks that no two rows overlap at a zoom unless they overlap in Live's
/// reference window at 100% (a pane and the controls in it).
fn assert_no_new_overlaps(reference: &[Row], zoomed: &[Row], what: &str) {
    assert_eq!(reference.len(), zoomed.len(), "{what}: the same rows");
    for i in 0..reference.len() {
        for j in (i + 1)..reference.len() {
            if !overlaps(relation(reference[i].rect, reference[j].rect)) {
                assert!(
                    !overlaps(relation(zoomed[i].rect, zoomed[j].rect)),
                    "{what}: {} ({}) overlaps {} ({}): {:?} and {:?}",
                    zoomed[i].name,
                    zoomed[i].id,
                    zoomed[j].name,
                    zoomed[j].id,
                    zoomed[i].rect,
                    zoomed[j].rect,
                );
            }
        }
    }
}

fn assert_rect_near(drawn: Rect, expected: Rect, tolerance: f32, what: &str) {
    let close = (drawn.min - expected.min).length() <= tolerance
        && (drawn.max - expected.max).length() <= tolerance;
    assert!(close, "{what}: drawn {drawn:?}, expected {expected:?}");
}

#[test]
fn every_row_scales_by_the_zoom_at_every_percentage() {
    // Spec: interface-zoom R1. A window `zoom` times the size of another
    // shows what the other shows at 100%, each rectangle `zoom` times as
    // large and as far from the content area's corner.
    for zoom in every_percent() {
        for base in [MINIMUM, REFERENCE, REFERENCE + vec2(640.0, 360.0)] {
            for browser in [true, false] {
                let at_100 = main_window_rows(base, browser);
                let window = base * zoom;
                let zoomed: Vec<Row> = main_window_rows(layout_size(window, zoom), browser)
                    .into_iter()
                    .map(|row| row.scale(zoom))
                    .collect();
                assert_eq!(at_100.len(), zoomed.len());
                for (one, scaled) in at_100.iter().zip(&zoomed) {
                    assert_eq!(one.id, scaled.id);
                    assert_rect_near(
                        scaled.rect,
                        one.scale(zoom).rect,
                        EPS * zoom * 10.0,
                        &format!("{} ({}) at {zoom}", one.name, one.id),
                    );
                }
                // The shell's own scaling agrees with the rows'.
                let scaled = ShellLayout::compute(layout_size(window, zoom), browser).scale(zoom);
                assert!((scaled.content.width() - window.x).abs() < EPS * 10.0);
                assert!((scaled.content.height() - window.y).abs() < EPS * 10.0);
            }
        }
    }
}

#[test]
fn rows_keep_their_places_and_nothing_overlaps_at_every_percentage() {
    // Spec: interface-zoom R1, R7. At every zoom, in the smallest window it
    // allows, in Live's reference window where that's big enough, and in a
    // large one: every pair of rows relates as it does at 100% in a window
    // of the same layout size, no two rows overlap that don't in Live's
    // reference window, and every row is inside the window.
    for browser in [true, false] {
        let reference = main_window_rows(REFERENCE, browser);
        for zoom in every_percent() {
            let minimum = minimum_window(zoom, false);
            for window in [
                minimum,
                REFERENCE.max(minimum),
                vec2(3200.0, 1900.0).max(minimum),
            ] {
                let what = format!("{:.0}% in {window:?}, browser {browser}", zoom * 100.0);
                let at_100 = main_window_rows(layout_size(window, zoom), browser);
                let rows: Vec<Row> = at_100.iter().map(|row| row.scale(zoom)).collect();
                assert_relations_kept(&at_100, &rows, &what);
                assert_no_new_overlaps(&reference, &rows, &what);
                let content = Rect::from_min_size(Pos2::ZERO, window);
                for row in &rows {
                    assert!(
                        content.expand(EPS * 10.0).contains_rect(row.rect),
                        "{what}: {} is outside the window: {:?}",
                        row.name,
                        row.rect
                    );
                }
            }
        }
    }
}

#[test]
fn controls_keep_their_live_size_times_the_zoom() {
    // Spec: interface-zoom R1. The Control Bar's and Status Bar's controls
    // keep their measured widths and heights (L-WIN-003, L-STAT), times the
    // zoom, and stay in Live's order with the margins scaled too.
    for zoom in every_percent() {
        let window = minimum_window(zoom, false);
        let layout = ShellLayout::compute(layout_size(window, zoom), true).scale(zoom);
        for (placement, (_, rect)) in ELEMENTS.iter().zip(&layout.elements) {
            if placement.strip == Strip::ControlBar {
                assert!((rect.height() - 18.0 * zoom).abs() < EPS * 10.0);
            }
            if placement.element != bl_ui::layout::Element::Message {
                assert!(
                    (rect.width() - placement.width * zoom).abs() < EPS * 10.0,
                    "{} at {zoom}",
                    placement.name
                );
            }
        }
        for strip in [Strip::ControlBar, Strip::StatusBar] {
            let rects: Vec<Rect> = ELEMENTS
                .iter()
                .zip(&layout.elements)
                .filter(|(p, _)| p.strip == strip)
                .map(|(_, (_, rect))| *rect)
                .collect();
            for pair in rects.windows(2) {
                assert!(pair[0].right() <= pair[1].left() + EPS * 10.0);
            }
            assert!(rects.last().unwrap().right() <= window.x - MARGIN * zoom + EPS * 10.0);
        }
    }
}

#[test]
fn the_minimum_window_grows_with_the_zoom() {
    // Spec: interface-zoom R7, R9. The smallest window lays out exactly the
    // layout layer's minimum, the title bar where Beatlayer draws it keeping
    // its 23 points.
    println!("| Zoom | Minimum window (content) | With Live's title bar (macOS) |");
    for zoom in every_step() {
        let content = minimum_window(zoom, false);
        let framed = minimum_window(zoom, true);
        assert!((content - MINIMUM * zoom).length() < EPS);
        assert!((framed - content - vec2(0.0, TITLE_BAR)).length() < EPS);
        let laid_out = layout_size(content, zoom);
        assert!((laid_out - MINIMUM).length() < EPS, "{laid_out:?}");
        // A window any smaller lays out the minimum and shows part of it:
        // still nothing overlaps (layout.rs's tests), but the far right and
        // bottom are out of view.
        let smaller = ShellLayout::compute(layout_size(content - vec2(40.0, 40.0), zoom), true);
        assert_eq!(smaller.content.size(), MINIMUM);
        println!(
            "| {:.0}% | {:.0} × {:.0} | {:.0} × {:.0} |",
            zoom * 100.0,
            content.x,
            content.y,
            framed.x,
            framed.y
        );
    }
}

#[test]
fn the_window_grows_to_its_minimum_as_far_as_the_screen_allows() {
    // Spec: interface-zoom R7. Zooming keeps the window's size unless it's
    // under the new minimum; it's never shrunk; a screen too small for the
    // minimum caps both.
    let reference = vec2(1600.0, 1000.0);
    // Live's reference window holds every zoom up to 120% as it is.
    for zoom in [0.5, 1.0, 1.2] {
        let (minimum, size) = fit_main_window(reference, zoom, true, None);
        assert_eq!(size, reference, "{zoom}");
        assert!((minimum - minimum_window(zoom, true)).length() < EPS);
    }
    // At 150% the minimum is 1980 × 983 points: it grows wider and keeps
    // its height.
    let (_, size) = fit_main_window(reference, 1.5, true, None);
    assert!((size - vec2(1980.0, 1000.0)).length() < EPS, "{size:?}");
    // On a 1728 × 1117-point screen (95% × 90% of it usable) at 200%, the
    // minimum and the window are as big as the screen allows.
    let screen = vec2(1728.0 * 0.95, 1117.0 * 0.9);
    let (minimum, size) = fit_main_window(reference, 2.0, true, Some(screen));
    assert_eq!(minimum, screen);
    assert_eq!(size, screen);
    // A window bigger than the minimum is left alone, even on a small screen.
    let (_, size) = fit_main_window(vec2(2400.0, 1400.0), 1.5, false, Some(screen));
    assert_eq!(size, vec2(2400.0, 1400.0));
}

#[test]
fn the_settings_window_scales_with_the_zoom() {
    // Spec: interface-zoom R8. Live's 562 × 617 at 100%, its content scaled.
    assert_eq!(settings_window(1.0, true), SETTINGS_WINDOW);
    let at_100 = SettingsLayout::compute(SETTINGS_WINDOW - vec2(0.0, TITLE_BAR))
        .rows(SettingsPage::ALL.len());
    for zoom in every_percent() {
        let window = settings_window(zoom, false);
        assert!((window - (SETTINGS_WINDOW - vec2(0.0, TITLE_BAR)) * zoom).length() < EPS);
        assert!((settings_window(zoom, true) - window - vec2(0.0, TITLE_BAR)).length() < EPS);
        let rows: Vec<Row> = SettingsLayout::compute(layout_size(window, zoom))
            .rows(SettingsPage::ALL.len())
            .into_iter()
            .map(|row| row.scale(zoom))
            .collect();
        for (one, scaled) in at_100.iter().zip(&rows) {
            assert_rect_near(scaled.rect, one.scale(zoom).rect, EPS * 10.0, one.name);
        }
        assert_relations_kept(&at_100, &rows, &format!("Settings at {zoom}"));
    }
}

/// The scale of the accessibility tree's root: window points per layout
/// point, which a screen reader uses to place every node.
fn root_scale(output: &egui::FullOutput) -> f64 {
    let update = output.platform_output.accesskit_update.as_ref().unwrap();
    let transform = update
        .nodes
        .iter()
        .find_map(|(_, node)| node.transform())
        .expect("the root carries the zoom");
    let [a, b, c, d, e, f] = transform.as_coeffs();
    assert!(b == 0.0 && c == 0.0 && e == 0.0 && f == 0.0 && (a - d).abs() < 1e-9);
    a
}

/// A node's bounds in layout points.
fn drawn(nodes: &[common::Node], label: &str) -> Option<Rect> {
    drawn_as(nodes, label, None)
}

/// A node's bounds in layout points, the node with `role` if one is given.
fn drawn_as(nodes: &[common::Node], label: &str, role: Option<Role>) -> Option<Rect> {
    nodes
        .iter()
        .find(|n| n.label == label && n.bounds.is_some() && role.is_none_or(|r| n.role == r))
        .map(|n| {
            let b = n.bounds.unwrap();
            Rect::from_min_max(
                pos2(b.x0 as f32, b.y0 as f32),
                pos2(b.x1 as f32, b.y1 as f32),
            )
        })
}

#[test]
fn the_main_window_draws_every_row_scaled_at_every_step() {
    // Spec: interface-zoom R1, R2, R9. Through egui's points-per-pixel, in
    // the smallest window each step allows and in Live's 1600 × 1000 where
    // that's big enough, with and without Live's drawn title bar: every
    // placed row is drawn where the layout layer puts it, the whole tree
    // scaled by the zoom, and the drawn title bar stays 23 points.
    for title_bar in [false, true] {
        for zoom in every_step() {
            let minimum = minimum_window(zoom, title_bar);
            let reference = vec2(1600.0, if title_bar { 1000.0 } else { 977.0 });
            for window in [minimum, reference.max(minimum)] {
                let mut h = if title_bar {
                    Harness::with_title_bar(Window::Main)
                } else {
                    Harness::new(Window::Main)
                };
                h.shell
                    .run(
                        ids::VIEW_SET_ZOOM,
                        json!({ "percent": (zoom * 100.0).round() as u16 }),
                    )
                    .unwrap();
                h.size = window;
                let output = h.settle();
                let what = format!("{:.0}% in {window:?}, title bar {title_bar}", zoom * 100.0);
                assert!((output.pixels_per_point - zoom).abs() < 1e-6, "{what}");
                assert!(
                    (root_scale(&output) - f64::from(zoom)).abs() < 1e-6,
                    "{what}"
                );

                let top = if title_bar { TITLE_BAR / zoom } else { 0.0 };
                let content = (window - vec2(0.0, if title_bar { TITLE_BAR } else { 0.0 })) / zoom;
                let expected: Vec<Row> = main_window_rows(content, true)
                    .into_iter()
                    .map(|row| Row {
                        rect: row.rect.translate(vec2(0.0, top)),
                        ..row
                    })
                    .collect();
                let nodes = Harness::nodes(&output);
                let mut checked = 0;
                for row in &expected {
                    // Panes are named as their regions are.
                    let (label, role) = match row.name {
                        "Main view" => ("Arrangement View", Some(Role::Pane)),
                        other => (other, None),
                    };
                    let Some(rect) = drawn_as(&nodes, label, role) else {
                        continue;
                    };
                    assert_rect_near(rect, row.rect, 0.1, &format!("{what}: {}", row.name));
                    checked += 1;
                }
                assert!(
                    checked >= ELEMENTS.len() + 17,
                    "{what}: only {checked} rows drawn"
                );

                // In window points, the drawn title bar is 23 points at every
                // zoom, and the Control Bar's controls sit 5 points under it
                // (37 less 23, less half their 18), times the zoom.
                let tap = drawn(&nodes, "Tap Tempo").unwrap();
                let tap_top = tap.top() * zoom;
                let expected_top = if title_bar { TITLE_BAR } else { 0.0 } + 5.0 * zoom;
                assert!((tap_top - expected_top).abs() < 0.1, "{what}: {tap_top}");
            }
        }
    }
}

#[test]
fn the_settings_window_draws_its_rows_scaled_at_every_step() {
    // Spec: interface-zoom R8, R9: the page chooser's rows 25 points tall
    // from y 34 (L-SET-002) times the zoom, under the 23-point title bar.
    for zoom in every_step() {
        let mut h = Harness::with_title_bar(Window::Settings);
        h.shell.invoke(ids::APP_SETTINGS, json!({})).unwrap();
        h.shell.take_effects();
        h.shell
            .run(
                ids::VIEW_SET_ZOOM,
                json!({ "percent": (zoom * 100.0).round() as u16 }),
            )
            .unwrap();
        h.size = settings_window(zoom, true);
        let output = h.settle();
        assert!((h.zoom - zoom).abs() < 1e-6);
        let nodes = Harness::nodes(&output);
        let scale = root_scale(&output) as f32;
        for (index, page) in SettingsPage::ALL.iter().enumerate() {
            let rect = drawn(&nodes, page.label()).unwrap();
            let top = TITLE_BAR + (34.0 - TITLE_BAR + 25.0 * index as f32) * zoom;
            let window_rect = Rect::from_min_max(
                (rect.min.to_vec2() * scale).to_pos2(),
                (rect.max.to_vec2() * scale).to_pos2(),
            );
            assert_rect_near(
                window_rect,
                Rect::from_min_size(pos2(0.0, top), vec2(140.0 * zoom, 25.0 * zoom)),
                0.1,
                &format!("{} at {zoom}", page.label()),
            );
        }
    }
}

#[test]
fn the_zoom_keys_zoom_every_layout_point_and_egui_s_own_keys_are_off() {
    // Spec: interface-zoom R3, R10. Cmd-plus (or Cmd-=) and Cmd-minus are
    // the View menu's Zoom In and Zoom Out (S-ZOOM-001, 002); Cmd-0 is
    // Live's key for launch quantisation (S-GQ-005, M3), not a zoom.
    let mut h = Harness::new(Window::Main);
    h.settle();
    let zoom_after = |h: &mut Harness, key: Key| {
        h.key(key, command());
        let output = h.settle();
        (h.shell.zoom_percent(), output.pixels_per_point)
    };
    assert_eq!(zoom_after(&mut h, Key::Plus), (110, 1.1));
    assert_eq!(zoom_after(&mut h, Key::Equals), (120, 1.2));
    assert_eq!(zoom_after(&mut h, Key::Minus), (110, 1.1));
    assert_eq!(
        zoom_after(&mut h, Key::Num0),
        (110, 1.1),
        "Cmd-0 doesn't zoom"
    );
    assert!((h.ctx.zoom_factor() - 1.1).abs() < 1e-6);
    // Without the command modifier, + and − zoom the Arrangement instead.
    let ticks_per_point = |h: &Harness| {
        h.shell
            .arrangement()
            .horizontal(h.shell.editor().document())
            .ticks_per_point
    };
    let before = ticks_per_point(&h);
    h.key(Key::Plus, Modifiers::NONE);
    h.settle();
    assert_eq!(h.shell.zoom_percent(), 110);
    assert!(ticks_per_point(&h) < before, "the time ruler zoomed in");
}

fn open_display_page() -> Harness {
    let mut h = Harness::new(Window::Settings);
    h.shell
        .invoke(ids::APP_SETTINGS, json!({ "page": "display-input" }))
        .unwrap();
    h.shell.take_effects();
    h.settings.refresh();
    h.settle();
    h
}

#[test]
fn display_and_input_has_the_zoom_after_the_language() {
    // Spec: interface-zoom R4: L-SET-010's Display section, Language then
    // Zoom, the zoom a number box a screen reader can read and step.
    let mut h = open_display_page();
    let nodes = Harness::nodes(&h.frame(vec![]));
    let language = Harness::node(&nodes, "Language");
    assert_eq!(language.value, "English");
    let zoom = Harness::node(&nodes, "Zoom");
    assert_eq!(zoom.role, Role::SpinButton);
    assert_eq!(zoom.value, "100 %");
    assert_eq!(zoom.numeric, Some(100.0));
    assert!(!zoom.disabled);
    assert!(language.bounds.unwrap().y0 < zoom.bounds.unwrap().y0);
    let tab = nodes
        .iter()
        .find(|n| n.role == Role::Tab && n.label == "Display & Input")
        .unwrap();
    assert!(!tab.disabled);

    // A screen reader's increment and decrement step it.
    let id = Id::new(("number-box", "zoom"));
    h.accesskit(id, Action::Increment);
    h.settle();
    assert_eq!(h.shell.zoom_percent(), 110);
    assert!((h.zoom - 1.1).abs() < 1e-6, "no mouse button down: at once");
    h.accesskit(id, Action::Decrement);
    h.accesskit(id, Action::Decrement);
    h.settle();
    assert_eq!(h.shell.zoom_percent(), 90);
    let nodes = Harness::nodes(&h.frame(vec![]));
    assert_eq!(Harness::node(&nodes, "Zoom").value, "90 %");
}

#[test]
fn dragging_the_zoom_rescales_the_settings_window_on_release() {
    // Spec: interface-zoom R4. The main window follows the setting as it's
    // dragged; the Settings window keeps its zoom until the mouse button is
    // released, as Live's does.
    let mut h = open_display_page();
    let nodes = Harness::nodes(&h.frame(vec![]));
    let b = Harness::node(&nodes, "Zoom").bounds.unwrap();
    let at = pos2(((b.x0 + b.x1) / 2.0) as f32, ((b.y0 + b.y1) / 2.0) as f32);
    h.frame(vec![Event::PointerMoved(at)]);
    h.frame(vec![Event::PointerButton {
        pos: at,
        button: PointerButton::Primary,
        pressed: true,
        modifiers: Modifiers::NONE,
    }]);
    // Up 12 points: four steps of 3 points.
    for dy in [3.0, 6.0, 9.0, 12.0] {
        h.frame(vec![Event::PointerMoved(at - vec2(0.0, dy))]);
    }
    h.frame(vec![]);
    assert!(
        h.shell.zoom_percent() > 100,
        "the setting follows the drag: {}",
        h.shell.zoom_percent()
    );
    assert!((h.zoom - 1.0).abs() < 1e-6, "the Settings window waits");
    assert!((h.ctx.zoom_factor() - 1.0).abs() < 1e-6);
    h.frame(vec![Event::PointerButton {
        pos: at - vec2(0.0, 12.0),
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    }]);
    h.settle();
    let percent = h.shell.zoom_percent();
    assert!(
        (h.zoom - zoom_factor(percent)).abs() < 1e-6,
        "released: {}",
        h.zoom
    );
    let output = h.frame(vec![]);
    assert!((output.pixels_per_point - zoom_factor(percent)).abs() < 1e-6);
}

#[test]
fn every_row_the_zoom_tests_check_is_a_layout_md_row() {
    // The rows above are layout.md's: each ID is defined there.
    let layout_md = std::fs::read_to_string(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/ui-parity/layout.md"),
    )
    .unwrap();
    let settings = SettingsLayout::compute(SETTINGS_WINDOW).rows(SettingsPage::ALL.len());
    for row in main_window_rows(REFERENCE, true).iter().chain(&settings) {
        assert!(
            layout_md.contains(&format!("| {} |", row.id)),
            "{} ({}) isn't in layout.md",
            row.id,
            row.name
        );
    }
}

#[test]
fn the_view_menu_has_the_zoom_items_in_live_s_order() {
    // Spec: interface-zoom R3: View ▸ Zoom ▸ Zoom In, Zoom Out, Reset Zoom
    // to 100% (M-VIEW-003a to 003c), each a registered command.
    let bar = bl_shell::menus::menu_bar(bl_shell::menus::MenuPlatform::Mac);
    let commands: Vec<&str> = bar
        .iter()
        .flat_map(|menu| menu.walk().filter_map(|item| item.command()))
        .filter(|id| id.starts_with("view.zoom"))
        .collect();
    assert_eq!(
        commands,
        [ids::VIEW_ZOOM_IN, ids::VIEW_ZOOM_OUT, ids::VIEW_ZOOM_RESET]
    );
}

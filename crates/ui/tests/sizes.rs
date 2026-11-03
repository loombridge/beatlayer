//! The size tokens of the theme, headless: the typeface's capitals stand as
//! high as Live's text does, and the Control Bar's fields, a menu's rows and
//! its separators draw at their tokens at every zoom.
//!
//! UI parity: L-CTRL, L-BRW, L-INFO, L-ARR.

use crate::common;
use bl_ui::layout::{Element, REFERENCE, ShellLayout};
use bl_ui::theme::{Sizes, UI_FONT};
use common::{Harness, Window};
use egui::accesskit::Role;
use serde_json::json;

/// The height of the typeface's capitals as a fraction of its size: the
/// OS/2 table's `sCapHeight` over the head table's `unitsPerEm`.
fn cap_height_ratio() -> f32 {
    let table = |tag: &[u8; 4]| {
        let count = usize::from(u16::from_be_bytes([UI_FONT[4], UI_FONT[5]]));
        (0..count)
            .map(|i| &UI_FONT[12 + 16 * i..28 + 16 * i])
            .find(|record| &record[..4] == tag)
            .map(|record| u32::from_be_bytes(record[8..12].try_into().unwrap()) as usize)
            .expect("the font has the table")
    };
    let be16 = |at: usize| f32::from(u16::from_be_bytes([UI_FONT[at], UI_FONT[at + 1]]));
    be16(table(b"OS/2") + 88) / be16(table(b"head") + 18)
}

#[test]
fn the_text_tokens_set_the_capitals_to_the_heights_measured() {
    // Body text 7 points to the top of capitals, the Control Bar's 6.5, the
    // Info View's title 7.5.
    let sizes = Sizes::bundled();
    let ratio = cap_height_ratio();
    for (size, capitals) in [
        (sizes.text, 7.0),
        (sizes.small_text, 6.5),
        (sizes.heading, 7.5),
    ] {
        assert!(
            (size * ratio - capitals).abs() < 0.1,
            "{size} pt text has capitals {} pt high, not {capitals}",
            size * ratio
        );
    }
}

#[test]
fn the_control_bars_fields_draw_at_their_token_and_scale_with_the_zoom() {
    let layout = ShellLayout::compute(REFERENCE, true, true);
    let field = Sizes::bundled().control_field;
    assert_eq!(field, 16.0);
    assert_eq!(layout.rect(Element::Tempo).height(), field);
    for percent in [50, 100, 150, 200] {
        let mut h = Harness::with_title_bar(Window::Main);
        h.shell
            .run("view.set_zoom", json!({ "percent": percent }))
            .unwrap();
        let output = h.settle();
        let zoom = percent as f32 / 100.0;
        assert!((output.pixels_per_point - zoom).abs() < 1e-6);
        let nodes = Harness::nodes(&output);
        let tap = nodes
            .iter()
            .find(|n| n.role == Role::Button && n.label == "Tap Tempo")
            .and_then(|n| n.bounds)
            .expect("Tap Tempo is drawn");
        // In layout points, so egui draws it `zoom` times as large.
        assert!(
            (tap.y1 - tap.y0 - f64::from(field)).abs() < 1e-3,
            "{percent}%"
        );
    }
}

#[test]
fn a_menus_rows_and_separators_are_their_tokens_apart() {
    // The CPU meter's menu: two rows one above the other, and a separator
    // between the Average and Current row and the Warn item.
    let sizes = Sizes::bundled();
    let mut h = Harness::new(Window::Main);
    h.settle();
    let layout = ShellLayout::compute(REFERENCE, true, true);
    h.click(layout.rect(Element::Cpu).center());
    let nodes = Harness::nodes(&h.settle());
    let top = |label: &str| {
        nodes
            .iter()
            .find(|n| n.label.strip_prefix("✔ ").unwrap_or(&n.label) == label)
            .and_then(|n| n.bounds)
            .unwrap_or_else(|| panic!("no menu item {label:?}"))
            .y0
    };
    let step = top("Current 0 %") - top("Average ---");
    assert!((step - f64::from(sizes.menu_row)).abs() < 0.1, "{step}");
    let gap = top("Warn on Current CPU Overload") - top("Average");
    let wanted = f64::from(sizes.menu_row + sizes.menu_separator);
    assert!((gap - wanted).abs() < 0.1, "{gap}");
}

#[test]
fn a_track_headers_choosers_are_their_tokens_tall_and_apart() {
    let sizes = Sizes::bundled();
    let mut h = Harness::with_title_bar(Window::Main);
    let nodes = Harness::nodes(&h.settle());
    let first = nodes
        .iter()
        .find(|n| n.label == "Input Type")
        .and_then(|n| n.bounds)
        .expect("the In/Out column's first chooser");
    let mut column: Vec<_> = nodes
        .iter()
        .filter(|n| n.role == Role::ComboBox)
        .filter_map(|n| n.bounds)
        .filter(|b| b.x0 == first.x0)
        .collect();
    column.sort_by(|a, b| a.y0.total_cmp(&b.y0));
    assert_eq!(column[0].y1 - column[0].y0, f64::from(sizes.header_chooser));
    assert_eq!(
        column[1].y0 - column[0].y0,
        f64::from(sizes.header_chooser_pitch)
    );
}

#[test]
fn the_info_views_lines_are_a_line_apart() {
    // The title, then the body wrapped to lines of the same pitch.
    let line = Sizes::bundled().info_line;
    let mut h = Harness::new(Window::Main);
    h.settle();
    let play = ShellLayout::compute(REFERENCE, true, true).rect(Element::Play);
    h.frame(vec![egui::Event::PointerMoved(play.center())]);
    let output = h.settle();
    let texts: Vec<_> = output
        .shapes
        .iter()
        .filter_map(|clipped| match &clipped.shape {
            egui::epaint::Shape::Text(text) => Some(text),
            _ => None,
        })
        .collect();
    let title = texts
        .iter()
        .find(|t| t.galley.text() == "Play" && t.pos.x == 17.0)
        .expect("the Info View's title");
    let body = texts
        .iter()
        .find(|t| t.pos.x == 17.0 && t.pos.y > title.pos.y && t.galley.text().len() > 20)
        .expect("the Info View's body");
    assert_eq!(body.pos.y - title.pos.y, line);
    assert!(body.galley.rows.iter().all(|row| row.height() == line));
}

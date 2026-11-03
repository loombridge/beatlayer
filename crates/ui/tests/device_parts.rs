//! The parts a device panel is made of, headless (layout L-DEV-042 to
//! L-DEV-046): both knobs, the boxes, the text buttons, the rows, stacks and
//! grids of buttons, the unit pair, the greyed control, the headings, the side
//! strip and the title bar's parts, on the test device that has one of each.
//!
//! Spec: device-framework R52 to R58.

use crate::common;
use bl_devices::testing::{self, *};
use bl_model::device_layout::{BOX_HEIGHT, Glyph, Widget};
use bl_model::{DeviceId, ParamId, TrackId};
use bl_shell::device_commands::{DeviceAdded, TITLE_BAR_HEIGHT};
use bl_ui::device_panel::{body_rect, paint_glyph, param_control_id, place};
use bl_ui::device_view::layout;
use bl_ui::theme::Theme;
use common::{Harness, Window};
use egui::accesskit::Role;
use egui::{Color32, Pos2, Rect, Shape, pos2};
use serde_json::json;

fn harness() -> (Harness, DeviceId) {
    let mut h = Harness::with_title_bar(Window::Main);
    h.shell.set_device_types(testing::device_types());
    h.settle();
    let tracks: Vec<TrackId> = h.shell.editor().document().top_level().collect();
    h.shell
        .invoke(
            "arrangement.select_tracks",
            json!({ "tracks": [tracks[2]] }),
        )
        .unwrap();
    h.settle();
    let t = testing::parts_type();
    let device = json!({ "kind": t.kind, "name": t.name, "role": t.role });
    let added: DeviceAdded = serde_json::from_value(
        h.shell
            .run("device.add", json!({ "device": device }))
            .unwrap(),
    )
    .unwrap();
    h.settle();
    (h, added.device)
}

fn frame_of(h: &mut Harness, device: DeviceId) -> Rect {
    let nodes = Harness::nodes(&h.settle());
    let b = Harness::node(&nodes, "Device View").bounds.unwrap();
    let view = Rect::from_min_max(
        pos2(b.x0 as f32, b.y0 as f32),
        pos2(b.x1 as f32, b.y1 as f32),
    );
    layout(&h.shell, view)
        .into_iter()
        .find(|(id, _)| *id == device)
        .unwrap()
        .1
}

/// Where each control of the device is drawn, with its widget.
fn controls(h: &mut Harness, device: DeviceId) -> Vec<(ParamId, Widget, Rect)> {
    let frame = frame_of(h, device);
    let state = h.shell.device_view_state();
    let panel = state
        .devices
        .iter()
        .find(|d| d.id == device)
        .unwrap()
        .panel
        .clone()
        .unwrap();
    place(&panel, body_rect(frame, TITLE_BAR_HEIGHT)).0
}

fn control(h: &mut Harness, device: DeviceId, param: ParamId) -> Rect {
    controls(h, device)
        .into_iter()
        .find(|(p, _, _)| *p == param)
        .unwrap_or_else(|| panic!("no control for parameter {}", param.0))
        .2
}

fn value(h: &Harness, device: DeviceId, param: ParamId) -> f64 {
    h.shell
        .editor()
        .document()
        .device(device)
        .unwrap()
        .params
        .get(&param)
        .copied()
        .unwrap_or_else(|| {
            testing::parts_type()
                .description
                .param(param)
                .unwrap()
                .default
        })
}

fn last_step(h: &Harness) -> String {
    h.shell
        .editor()
        .undo_history()
        .next()
        .unwrap_or_default()
        .to_owned()
}

/// Every shape of a frame, flattened.
fn shapes(output: &egui::FullOutput) -> Vec<Shape> {
    fn walk(shape: &Shape, out: &mut Vec<Shape>) {
        match shape {
            Shape::Vec(all) => all.iter().for_each(|s| walk(s, out)),
            other => out.push(other.clone()),
        }
    }
    let mut out = Vec::new();
    for clipped in &output.shapes {
        walk(&clipped.shape, &mut out);
    }
    out
}

/// The colours the rectangles inside `area` are filled with.
fn fills(output: &egui::FullOutput, area: Rect) -> Vec<Color32> {
    common::painted_rects(output)
        .into_iter()
        .filter(|(r, _)| area.expand(0.5).contains_rect(*r))
        .map(|(_, c)| c)
        .collect()
}

/// The rectangles inside `area` that are outlined in `colour`.
fn outlined(output: &egui::FullOutput, area: Rect, colour: Color32) -> usize {
    shapes(output)
        .iter()
        .filter(|s| match s {
            Shape::Rect(r) => area.expand(0.5).contains_rect(r.rect) && r.stroke.color == colour,
            _ => false,
        })
        .count()
}

/// The accent-coloured line paths inside `area`: a lit arc.
fn lit_paths(output: &egui::FullOutput, area: Rect, colour: Color32) -> Vec<Vec<Pos2>> {
    shapes(output)
        .into_iter()
        .filter_map(|s| match s {
            Shape::Path(p)
                if p.stroke.color == egui::epaint::ColorMode::Solid(colour)
                    && area.contains_rect(p.visual_bounding_rect()) =>
            {
                Some(p.points)
            }
            _ => None,
        })
        .collect()
}

/// The text painted in `area`.
fn texts(output: &egui::FullOutput, area: Rect) -> Vec<String> {
    shapes(output)
        .into_iter()
        .filter_map(|s| match s {
            Shape::Text(t) if area.contains(t.pos) || area.intersects(t.visual_bounding_rect()) => {
                Some(t.galley.text().to_owned())
            }
            _ => None,
        })
        .collect()
}

fn set(h: &mut Harness, device: DeviceId, param: ParamId, value: f64) {
    h.shell
        .invoke(
            "device.set_param",
            json!({ "device": device, "param": param, "value": value }),
        )
        .unwrap();
}

fn pressed(h: &mut Harness, rect: Rect) {
    h.click(rect.center());
    h.settle();
}

// Spec: device-framework R52
#[test]
fn a_panel_draws_each_part_at_the_size_live_draws_it() {
    let (mut h, device) = harness();
    let all = controls(&mut h, device);
    let size = |param: ParamId| all.iter().find(|(p, _, _)| *p == param).unwrap().2.size();
    let points = |widget: Widget| egui::vec2(widget.size().width, widget.size().height);
    assert_eq!(size(PARTS_LEVEL), points(Widget::Knob));
    assert_eq!(size(PARTS_AMOUNT), points(Widget::SmallKnob));
    assert_eq!(size(PARTS_MIX), points(Widget::Slider));
    assert_eq!(size(PARTS_HOLD).y, BOX_HEIGHT);
    assert_eq!(size(PARTS_ROUND), egui::vec2(BOX_HEIGHT, BOX_HEIGHT));
    // Every part reads to a screen reader under its parameter's name.
    let nodes = Harness::nodes(&h.settle());
    assert_eq!(Harness::node(&nodes, "Level").role, Role::Slider);
    assert_eq!(Harness::node(&nodes, "Plain").role, Role::SpinButton);
    assert_eq!(Harness::node(&nodes, "Chooser").role, Role::ComboBox);
    assert_eq!(Harness::node(&nodes, "Hold").role, Role::Switch);
}

// Spec: device-framework R53
#[test]
fn a_knob_lights_its_arc_from_its_low_end_and_a_bipolar_one_from_zero() {
    let (mut h, device) = harness();
    let theme = Theme::bundled(true);
    let accent = theme.colors.accent.0;
    let level = control(&mut h, device, PARTS_LEVEL);
    let amount = control(&mut h, device, PARTS_AMOUNT);
    let output = h.settle();
    // Level is at half: its lit arc runs from the lower left to the top.
    let arcs = lit_paths(&output, level, accent);
    assert_eq!(arcs.len(), 1, "Level has a lit arc");
    let arc = &arcs[0];
    let ring_centre_x = level.center().x;
    assert!(arc[0].x < ring_centre_x - 5.0, "it starts at the left foot");
    assert!(
        (arc.last().unwrap().x - ring_centre_x).abs() < 2.0,
        "and ends at the top, half way"
    );
    // Amount is at zero, the middle of its range: nothing to light, and a
    // notch at the top of the ring where the arc would start.
    let still = h.settle();
    assert!(lit_paths(&still, amount, accent).is_empty());
    let notches = shapes(&still)
        .iter()
        .filter(|s| match s {
            Shape::Path(p) => {
                p.fill == theme.colors.control.0 && amount.contains_rect(p.visual_bounding_rect())
            }
            _ => false,
        })
        .count();
    assert_eq!(notches, 1, "the notch marks zero");
    // Moved up it lights from the centre, to the right of it.
    set(&mut h, device, PARTS_AMOUNT, 25.0);
    let moved = h.settle();
    let arcs = lit_paths(&moved, amount, accent);
    assert_eq!(arcs.len(), 1);
    assert!(
        (arcs[0][0].x - amount.center().x).abs() < 2.0,
        "from the top"
    );
    assert!(arcs[0].last().unwrap().x > amount.center().x + 2.0);
    // The level knob has no notch: its range doesn't span zero.
    let level_notches = shapes(&moved)
        .iter()
        .filter(|s| matches!(s, Shape::Path(p) if p.fill == theme.colors.control.0 && level.contains_rect(p.visual_bounding_rect())))
        .count();
    assert_eq!(level_notches, 0);
}

// Spec: device-framework R54
#[test]
fn a_lit_box_fills_to_its_value_and_a_plain_one_doesnt() {
    let (mut h, device) = harness();
    let theme = Theme::bundled(true);
    let blue = theme.colors.value_fill.0;
    let mix = control(&mut h, device, PARTS_MIX);
    let plain = control(&mut h, device, PARTS_PLAIN);
    let output = h.settle();
    assert!(fills(&output, mix).contains(&blue), "Mix is lit");
    assert!(!fills(&output, plain).contains(&blue), "Plain isn't");
    // The fill is the value's place in its range: Mix is at 25 %.
    let fill = common::painted_rects(&output)
        .into_iter()
        .find(|(r, c)| *c == blue && mix.expand(0.5).contains_rect(*r))
        .unwrap()
        .0;
    let box_width = mix.width();
    assert!((fill.width() - box_width * 0.25).abs() < 1.0, "{fill:?}");
    assert_eq!(fill.height(), BOX_HEIGHT, "a box is 12 points tall");
    // Typing or dragging changes the value as one undo step.
    set(&mut h, device, PARTS_MIX, 100.0);
    let full = h.settle();
    let fill = common::painted_rects(&full)
        .into_iter()
        .find(|(r, c)| *c == blue && mix.expand(0.5).contains_rect(*r))
        .unwrap()
        .0;
    assert!((fill.width() - box_width).abs() < 1.0);
}

// Spec: device-framework R54
#[test]
fn a_marked_box_has_the_marker_at_its_right() {
    let (mut h, device) = harness();
    let marked = control(&mut h, device, PARTS_MARKED);
    let plain = control(&mut h, device, PARTS_PLAIN);
    let output = h.settle();
    let triangles = |area: Rect| {
        shapes(&output)
            .iter()
            .filter_map(|s| match s {
                Shape::Path(p) if p.closed && area.contains_rect(p.visual_bounding_rect()) => {
                    Some(p.visual_bounding_rect())
                }
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    let found = triangles(marked);
    assert_eq!(found.len(), 1, "one marker");
    assert!(found[0].center().x > marked.center().x, "at the right");
    assert!(triangles(plain).is_empty());
}

// Spec: device-framework R54
#[test]
fn a_text_button_is_lit_while_on_and_an_outlined_one_is_outlined_while_off() {
    let (mut h, device) = harness();
    let theme = Theme::bundled(true);
    let accent = theme.colors.accent.0;
    let hold = control(&mut h, device, PARTS_HOLD);
    let edge = control(&mut h, device, PARTS_EDGE);
    let off = h.settle();
    assert!(!fills(&off, hold).contains(&accent), "off: dark");
    assert_eq!(
        outlined(&off, hold, accent),
        0,
        "a plain one has no outline"
    );
    assert!(!fills(&off, edge).contains(&accent));
    assert_eq!(outlined(&off, edge, accent), 1, "off: outlined");
    assert!(
        texts(&off, hold).contains(&"Hold".to_owned()),
        "its name on it"
    );
    // A click lights it as one undo step named for the parameter.
    let before = h.shell.editor().undo_history().len();
    pressed(&mut h, hold);
    assert_eq!(value(&h, device, PARTS_HOLD), 1.0);
    assert_eq!(h.shell.editor().undo_history().len(), before + 1);
    assert_eq!(last_step(&h), "Change \"Hold\"");
    pressed(&mut h, edge);
    let on = h.settle();
    assert!(fills(&on, hold).contains(&accent), "on: orange");
    assert!(fills(&on, edge).contains(&accent), "on: filled");
    assert_eq!(outlined(&on, edge, accent), 0, "no outline once lit");
}

// Spec: device-framework R54
#[test]
fn icon_and_round_buttons_light_and_switch_like_text_ones() {
    let (mut h, device) = harness();
    let theme = Theme::bundled(true);
    let accent = theme.colors.accent.0;
    let icon = control(&mut h, device, PARTS_ICON);
    let round = control(&mut h, device, PARTS_ROUND);
    let off = h.settle();
    assert!(!fills(&off, icon).contains(&accent));
    assert!(
        texts(&off, icon).is_empty(),
        "an icon button has an icon, not text"
    );
    pressed(&mut h, icon);
    pressed(&mut h, round);
    assert_eq!(value(&h, device, PARTS_ICON), 1.0);
    assert_eq!(value(&h, device, PARTS_ROUND), 1.0);
    let on = h.settle();
    assert!(fills(&on, icon).contains(&accent));
    // The round one is a circle in the accent.
    let lit_circle = shapes(&on).iter().any(|s| {
        matches!(s, Shape::Circle(c) if c.fill == accent && round.expand(0.5).contains(c.center) && (c.radius - BOX_HEIGHT / 2.0).abs() < 0.1)
    });
    assert!(lit_circle, "a 12-point round button");
}

// Spec: device-framework R54
#[test]
fn a_row_a_stack_and_a_grid_of_buttons_choose_one_choice_each() {
    let (mut h, device) = harness();
    let theme = Theme::bundled(true);
    let accent = theme.colors.accent.0;
    // A row of three: Peak, RMS, Expand.
    let mode = control(&mut h, device, PARTS_MODE);
    let third = Rect::from_min_size(
        pos2(mode.right() - 36.0, mode.top()),
        egui::vec2(36.0, mode.height()),
    );
    let before = h.shell.editor().undo_history().len();
    pressed(&mut h, third);
    assert_eq!(value(&h, device, PARTS_MODE), 2.0);
    assert_eq!(h.shell.editor().undo_history().len(), before + 1);
    assert_eq!(last_step(&h), "Change \"Mode\"");
    let output = h.settle();
    assert!(
        fills(&output, third).contains(&accent),
        "the chosen one is lit"
    );
    let first = Rect::from_min_size(mode.min, egui::vec2(36.0, mode.height()));
    assert!(!fills(&output, first).contains(&accent));
    // A stack of two.
    let stack = control(&mut h, device, PARTS_STACK);
    let second = Rect::from_min_size(
        pos2(stack.left(), stack.bottom() - BOX_HEIGHT),
        egui::vec2(stack.width(), BOX_HEIGHT),
    );
    pressed(&mut h, second);
    assert_eq!(value(&h, device, PARTS_STACK), 1.0);
    // A grid of 4 by 2: the fifth choice is the first of the second row.
    let grid = control(&mut h, device, PARTS_GRID);
    let fifth = Rect::from_min_size(
        pos2(grid.left(), grid.bottom() - BOX_HEIGHT),
        egui::vec2(22.0, BOX_HEIGHT),
    );
    pressed(&mut h, fifth);
    assert_eq!(value(&h, device, PARTS_GRID), 4.0);
    let output = h.settle();
    assert!(
        texts(&output, grid).contains(&"16".to_owned()),
        "the last choice is on the grid too"
    );
    assert!(fills(&output, fifth).contains(&accent));
    // Row of filter icons: the third is Band Pass, drawn as an icon.
    let filter = control(&mut h, device, PARTS_FILTER);
    let band = Rect::from_min_size(
        pos2(filter.left() + 2.0 * (22.0 + 3.0), filter.top()),
        egui::vec2(22.0, BOX_HEIGHT),
    );
    pressed(&mut h, band);
    assert_eq!(value(&h, device, PARTS_FILTER), 2.0);
    assert!(texts(&h.settle(), filter).is_empty(), "icons, not text");
}

// Spec: device-framework R55
#[test]
fn a_unit_pair_switches_which_parameter_the_knob_shows() {
    let (mut h, device) = harness();
    let rate = control(&mut h, device, PARTS_RATE_MS);
    let nodes = Harness::nodes(&h.settle());
    let rates: Vec<_> = nodes
        .iter()
        .filter(|n| n.label == "Rate" && n.role == Role::Slider)
        .collect();
    assert_eq!(rates.len(), 1, "the knob shows one parameter, never both");
    assert_eq!(rates[0].value, "100 ms");
    // The pair takes the left 18 points of the control, the second button
    // the sync one; the knob has the rest.
    let pair_x = rate.left() + 9.0;
    let sync = pos2(pair_x, rate.center().y + BOX_HEIGHT / 2.0 + 1.5);
    let before = h.shell.editor().undo_history().len();
    h.click(sync);
    h.settle();
    assert_eq!(value(&h, device, PARTS_SYNC), 1.0);
    assert_eq!(h.shell.editor().undo_history().len(), before + 1);
    assert_eq!(last_step(&h), "Change \"Sync\"");
    // The knob now stands for the synced rate, at the same place.
    assert_eq!(
        control(&mut h, device, PARTS_RATE_SYNC),
        rate,
        "the same place"
    );
    let nodes = Harness::nodes(&h.settle());
    let rates: Vec<_> = nodes
        .iter()
        .filter(|n| n.label == "Rate" && n.role == Role::Slider)
        .collect();
    assert_eq!(rates.len(), 1);
    assert_eq!(rates[0].value, "50 %");
    // The first button switches back.
    let ms = pos2(pair_x, rate.center().y - BOX_HEIGHT / 2.0 - 1.5);
    h.click(ms);
    h.settle();
    assert_eq!(value(&h, device, PARTS_SYNC), 0.0);
}

// Spec: device-framework R56
#[test]
fn a_control_is_greyed_by_another_controls_value_and_stays_usable() {
    let (mut h, device) = harness();
    let theme = Theme::bundled(true);
    let gated = control(&mut h, device, PARTS_GATED);
    let dim = |output: &egui::FullOutput| {
        common::painted_rects(output)
            .into_iter()
            .any(|(r, c)| r == gated && c == theme.colors.window.0.gamma_multiply(0.5))
    };
    // Hold is off: Gated is greyed.
    assert!(dim(&h.settle()), "greyed while Hold is off");
    // Greyed, it still takes a screen reader's step.
    let id = param_control_id(device, PARTS_GATED);
    h.accesskit(id, egui::accesskit::Action::Increment);
    h.settle();
    assert!(value(&h, device, PARTS_GATED) > 0.0, "usable while greyed");
    // Hold on: not greyed.
    set(&mut h, device, PARTS_HOLD, 1.0);
    assert!(!dim(&h.settle()), "not greyed while Hold is on");
}

// Spec: device-framework R56
#[test]
fn a_heading_is_drawn_over_its_group() {
    let (mut h, device) = harness();
    let frame = frame_of(&mut h, device);
    let body = body_rect(frame, TITLE_BAR_HEIGHT);
    let output = h.settle();
    let at_heading = texts(
        &output,
        Rect::from_min_size(body.min + egui::vec2(22.0, 0.0), egui::vec2(80.0, 12.0)),
    );
    assert!(
        at_heading.contains(&"Controls".to_owned()),
        "the heading is at the top of its group: {at_heading:?}"
    );
    // The controls under it start below it.
    let level = control(&mut h, device, PARTS_LEVEL);
    assert!(level.top() >= body.top() + 12.0);
}

// Spec: device-framework R56
#[test]
fn a_display_cell_holds_a_row_of_controls_along_its_top_and_foot() {
    let (mut h, device) = harness();
    let top = control(&mut h, device, PARTS_CELL_TOP);
    let bottom = control(&mut h, device, PARTS_CELL_BOTTOM);
    let frame = frame_of(&mut h, device);
    let state = h.shell.device_view_state();
    let panel = state.devices[0].panel.clone().unwrap();
    let cell = place(&panel, body_rect(frame, TITLE_BAR_HEIGHT)).1[0].1;
    assert!(cell.contains_rect(top) && cell.contains_rect(bottom));
    assert!(top.top() - cell.top() <= 4.5, "along its top");
    assert!(
        cell.bottom() - bottom.bottom() <= 4.5 + 12.0,
        "along its foot"
    );
    pressed(&mut h, top);
    assert_eq!(value(&h, device, PARTS_CELL_TOP), 1.0);
}

// Spec: device-framework R57
#[test]
fn a_side_strip_shows_its_section_while_its_toggle_is_on() {
    let (mut h, device) = harness();
    let theme = Theme::bundled(true);
    let frame = frame_of(&mut h, device);
    let body = body_rect(frame, TITLE_BAR_HEIGHT);
    // Hidden in a new device: the strip alone, its name written upwards.
    let hidden = h.settle();
    assert!(
        controls(&mut h, device)
            .iter()
            .all(|(p, _, _)| *p != PARTS_STRIP_GAIN),
        "the section isn't placed"
    );
    let upward = shapes(&hidden)
        .iter()
        .any(|s| matches!(s, Shape::Text(t) if t.angle != 0.0 && t.galley.text() == PARTS_STRIP));
    assert!(upward, "its name reads up the strip");
    let width_hidden = frame.width();
    // Its toggle, a round ▼ button at the top of the strip, shows the section.
    let toggle = Rect::from_min_size(
        body.min + egui::vec2(4.0, 4.0),
        egui::vec2(BOX_HEIGHT, BOX_HEIGHT),
    );
    let before = h.shell.editor().undo_history().len();
    pressed(&mut h, toggle);
    let state = h.shell.device_view_state();
    let pages = &state.devices[0].panel.as_ref().unwrap().pages;
    assert_eq!(pages[0].node, PARTS_STRIP);
    assert_eq!(pages[0].shown, "Shown");
    assert_eq!(
        h.shell.editor().undo_history().len(),
        before,
        "a view setting, not an edit"
    );
    let strip_gain = control(&mut h, device, PARTS_STRIP_GAIN);
    assert!(
        strip_gain.left() >= body.left() + 22.0,
        "right of the strip"
    );
    let wider = frame_of(&mut h, device).width();
    assert!(wider > width_hidden, "the device widens");
    let shown = h.settle();
    let lit = shapes(&shown).iter().any(|s| {
        matches!(s, Shape::Circle(c) if c.fill == theme.colors.accent.0 && toggle.contains(c.center))
    });
    assert!(lit, "the toggle is lit while its section shows");
    // And again hides it.
    pressed(&mut h, toggle);
    assert!(
        controls(&mut h, device)
            .iter()
            .all(|(p, _, _)| *p != PARTS_STRIP_GAIN)
    );
}

// Spec: device-framework R58
#[test]
fn the_title_bar_holds_the_parts_a_device_adds() {
    let (mut h, device) = harness();
    let frame = frame_of(&mut h, device);
    let title = Rect::from_min_size(frame.min, egui::vec2(frame.width(), TITLE_BAR_HEIGHT));
    let output = h.settle();
    let words = texts(&output, title);
    for word in ["MPE", "Softube", "Parts", "A", "Learn"] {
        assert!(words.contains(&word.to_owned()), "{word:?} in {words:?}");
    }
    // The round button after the activator, and the lettered and text buttons,
    // switch their parameters.
    let nodes = Harness::nodes(&h.settle());
    for (name, param) in [
        ("Letter", PARTS_LETTER),
        ("Info", PARTS_INFO),
        ("Learn", PARTS_LEARN),
    ] {
        let node = Harness::node(&nodes, name);
        assert_eq!(node.role, Role::Switch, "{name}");
        let b = node.bounds.unwrap();
        let rect = Rect::from_min_max(
            pos2(b.x0 as f32, b.y0 as f32),
            pos2(b.x1 as f32, b.y1 as f32),
        );
        assert!(title.contains_rect(rect), "{name} is in the title bar");
        assert_eq!(rect.height(), BOX_HEIGHT, "{name}: 12 points tall");
        let before = h.shell.editor().undo_history().len();
        pressed(&mut h, rect);
        assert_eq!(value(&h, device, param), 1.0, "{name}");
        assert_eq!(h.shell.editor().undo_history().len(), before + 1);
        assert_eq!(last_step(&h), format!("Change \"{name}\""));
    }
    // The round button comes first after the activator, left of the name.
    let nodes = Harness::nodes(&h.settle());
    let info = Harness::node(&nodes, "Info").bounds.unwrap();
    assert!(
        f64::from(frame.left()) + 12.0 < info.x0,
        "right of the activator"
    );
}

// Spec: device-framework R58
#[test]
fn a_name_wider_than_its_title_bar_ends_in_an_ellipsis() {
    let wide = |text: &str| text.chars().count() <= 8;
    assert_eq!(bl_ui::device_panel::truncated("Note On", wide), "Note On");
    assert_eq!(
        bl_ui::device_panel::truncated("Note Length", wide),
        "Note Le…"
    );
    assert_eq!(
        bl_ui::device_panel::truncated("Envelope Follower", |t| t.chars().count() <= 1),
        "…"
    );
    assert_eq!(bl_ui::device_panel::truncated("Amp", |_| false), "…");
}

// Spec: device-framework R52
#[test]
fn every_icon_draws_something_inside_its_button() {
    let glyphs = [
        Glyph::LowPass,
        Glyph::HighPass,
        Glyph::BandPass,
        Glyph::Notch,
        Glyph::Metronome,
        Glyph::Play,
        Glyph::Down,
        Glyph::Info,
        Glyph::Left,
        Glyph::Right,
        Glyph::Plus,
        Glyph::Minus,
        Glyph::FlatSharp,
        Glyph::Link,
        Glyph::Freeze,
        Glyph::PhaseInvert,
    ];
    let ctx = egui::Context::default();
    let area = Rect::from_min_size(pos2(10.0, 10.0), egui::vec2(8.0, 8.0));
    for glyph in glyphs {
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            paint_glyph(ui.painter(), area, glyph, Color32::WHITE);
        });
        output.textures_delta.clear();
        let found = shapes(&output);
        assert!(!found.is_empty(), "{glyph:?} draws");
        for shape in found {
            assert!(
                area.expand(2.0).contains_rect(shape.visual_bounding_rect()),
                "{glyph:?} stays in its button: {:?}",
                shape.visual_bounding_rect()
            );
        }
    }
    // A filter's name finds its icon; any other name stays text.
    assert_eq!(
        bl_ui::device_panel::glyph_for("Low Pass"),
        Some(Glyph::LowPass)
    );
    assert_eq!(bl_ui::device_panel::glyph_for("Notch"), Some(Glyph::Notch));
    assert_eq!(bl_ui::device_panel::glyph_for("Peak"), None);
}

//! Wavetable's display cells (WP-8.11; the wavetable spec, R35 to R40): each
//! oscillator's table in perspective or polar with a drag on Position and a
//! drop for a sample, the filter, envelope and LFO displays drawn from the
//! device's display data, and the matrix as a grid of sources and
//! destinations. Every gesture runs the commands an agent runs
//! (`device.set_param`, `device.load_wavetable`, `device.set_route`,
//! `device.clear_route`, `device.add_destination` and
//! `device.remove_destination`).

use bl_devices::wavetable::{self, MATRIX_CELL, OSC_ON, POSITION};
use bl_devices::wavetable_display::{ENVELOPE_CELLS, FILTER_CELL, LFO_CELLS, OSC_CELLS, VIEWS};
use bl_model::device_layout::Glyph;
use bl_model::{DeviceDisplay, DeviceKind};
use bl_shell::Invocation;
use bl_shell::device_commands::{
    ADD_DESTINATION, CLEAR_ROUTE, DeviceFrame, DevicePanel, GridRow, LOAD_WAVETABLE, MatrixGrid,
    REMOVE_DESTINATION, SET_ROUTE,
};
use egui::accesskit::{Action, Role};
use egui::{
    Align2, Color32, Event, FontId, Id, Key, Mesh, Painter, Pos2, Rect, Sense, Shape, Stroke,
    StrokeKind, Ui, pos2, vec2,
};
use serde_json::json;

use crate::device_panel::{DEVICE_TEXT, gesture_name, paint_glyph, truncated};
use crate::main_window::UiEvent;
use crate::theme::Theme;

/// The width of a grid row's destination name, the height of its header and
/// its rows, and the height of the chooser under it, in points.
const LABEL_WIDTH: f32 = 112.0;
const HEADER_HEIGHT: f32 = 12.0;
const ROW_HEIGHT: f32 = 14.0;
const CHOOSER_HEIGHT: f32 = 14.0;
/// How many per cent of amount a point of vertical drag moves a grid cell.
const PER_POINT: f64 = 2.0;
/// The step the arrow keys and a screen reader's steps move a grid cell, in
/// per cent.
const STEP: f64 = 1.0;

/// Where the id of the places this frame drew that take a dropped file
/// themselves is kept.
fn drop_zones_id() -> Id {
    Id::new("wavetable-drop-zones")
}

/// The places drawn since the last call that take a dropped file themselves
/// (an oscillator's display), so the Arrangement's drop doesn't take it too.
#[must_use]
pub fn take_drop_zones(ctx: &egui::Context) -> Vec<Rect> {
    ctx.data_mut(|data| data.remove_temp::<Vec<Rect>>(drop_zones_id()))
        .unwrap_or_default()
}

/// Whether `frame` is a Wavetable.
#[must_use]
pub fn is_wavetable(frame: &DeviceFrame) -> bool {
    matches!(&frame.kind, DeviceKind::Native { device_type } if device_type == wavetable::WAVETABLE)
}

/// Whether `path` names a file a table can be made from: a WAV or AIFF
/// (the wavetable spec, R37).
#[must_use]
pub fn is_table_file(path: &str) -> bool {
    std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| {
            ["wav", "aif", "aiff"]
                .iter()
                .any(|kind| e.eq_ignore_ascii_case(kind))
        })
}

/// The oscillator (1 or 2) whose display cell is called `name`.
#[must_use]
pub fn oscillator_of(name: &str) -> Option<u8> {
    OSC_CELLS
        .iter()
        .position(|cell| *cell == name)
        .map(|n| n as u8 + 1)
}

/// What `display` holds for the cell called `name`: the series, markers
/// and readings whose names are the cell's or begin with its name and a
/// space.
fn own(display: &DeviceDisplay, name: &str) -> DeviceDisplay {
    let mine = |item: &str| {
        item == name
            || item
                .strip_prefix(name)
                .is_some_and(|rest| rest.starts_with(' '))
    };
    DeviceDisplay {
        device: display.device,
        live: display.live,
        view: display.view.clone(),
        series: display
            .series
            .iter()
            .filter(|s| mine(&s.name))
            .cloned()
            .collect(),
        markers: display
            .markers
            .iter()
            .filter(|m| mine(&m.series))
            .cloned()
            .collect(),
        readings: display
            .readings
            .iter()
            .filter(|r| mine(&r.name))
            .cloned()
            .collect(),
        spectrograms: Vec::new(),
    }
}

/// Draws the Wavetable display cell called `name` in `rect` and takes its
/// gestures; false for a cell that isn't one of its own.
pub fn cell(
    ui: &mut Ui,
    theme: &Theme,
    frame: &DeviceFrame,
    panel: &DevicePanel,
    display: Option<&DeviceDisplay>,
    (id, name, rect): (Id, &str, Rect),
    events: &mut Vec<UiEvent>,
) -> bool {
    if name == MATRIX_CELL {
        let _ = ui.interact(rect, id, Sense::hover());
        grid(ui, theme, frame, panel, (id, rect), events);
        ui.ctx().accesskit_node_builder(id, |node| {
            node.set_role(Role::Table);
            node.set_label(MATRIX_CELL.to_owned());
        });
        return true;
    }
    let Some(display) = display else {
        return false;
    };
    let plain = ENVELOPE_CELLS.contains(&name) || LFO_CELLS.contains(&name) || name == FILTER_CELL;
    let shown = own(display, name);
    let _ = ui.interact(rect, id, Sense::hover());
    if plain {
        crate::device_display::draw_cell(
            ui,
            theme,
            rect,
            (id, frame.enabled),
            &shown,
            (panel, false),
            events,
        );
    } else if let Some(osc) = oscillator_of(name) {
        oscillator(ui, theme, frame, panel, (&shown, osc), (id, rect), events);
    } else {
        return false;
    }
    ui.ctx().accesskit_node_builder(id, |node| {
        node.set_role(Role::Image);
        node.set_label(name.to_owned());
        node.set_value(shown.text());
    });
    true
}

/// An oscillator's display: its table in perspective, or its frame at
/// Position in polar, with the view buttons along its top; a drag across it
/// moves Position as one undo step named as the knob's, and the outline
/// shows while a WAV or AIFF hovers over it.
fn oscillator(
    ui: &mut Ui,
    theme: &Theme,
    frame: &DeviceFrame,
    panel: &DevicePanel,
    (shown, osc): (&DeviceDisplay, u8),
    (id, rect): (Id, Rect),
    events: &mut Vec<UiEvent>,
) {
    let c = &theme.colors;
    let on = frame.enabled
        && panel
            .description
            .params
            .iter()
            .position(|p| p.id == wavetable::osc(usize::from(osc), OSC_ON))
            .is_none_or(|i| panel.value(i) >= 0.5);
    let ink = if on { c.accent.0 } else { c.inactive.0 };
    let painter = ui.painter_at(rect);
    let area = rect.shrink(4.0);
    let name = OSC_CELLS[usize::from(osc) - 1];
    let position = shown
        .reading(&format!("{name} Position"))
        .map_or(0.0, |r| f64::from(r.value) / 100.0);
    if shown.view.as_deref() == Some(VIEWS[1]) {
        polar(&painter, area, shown, name, (ink, c.separator.0));
    } else {
        perspective(
            &painter,
            area,
            shown,
            name,
            position,
            (ink, c.text_dim.0, c.display.0),
        );
    }
    drag_position(ui, panel, (frame.id, osc), (id, rect), events);
    ui.data_mut(|data| {
        data.get_temp_mut_or_default::<Vec<Rect>>(drop_zones_id())
            .push(rect);
    });
    let hovering = ui.input(|i| {
        i.raw.hovered_files.iter().any(|file| {
            file.path
                .as_ref()
                .is_none_or(|path| is_table_file(&path.to_string_lossy()))
        })
    }) && ui
        .input(|i| i.pointer.hover_pos())
        .is_some_and(|at| rect.contains(at));
    if hovering {
        painter.rect_stroke(
            rect.shrink(1.0),
            0.0,
            Stroke::new(2.0, c.accent.0),
            StrokeKind::Inside,
        );
    }
    // One view for both oscillators: the first one's cell has its buttons.
    if osc == 1 {
        crate::device_display::view_buttons(
            ui,
            theme,
            rect,
            shown,
            &VIEWS.map(str::to_owned),
            frame.enabled,
            events,
        );
    }
}

/// The table's frames from the back to the front, each drawn over what lies
/// behind it, the frame at `position` in the accent.
fn perspective(
    painter: &Painter,
    area: Rect,
    shown: &DeviceDisplay,
    osc: &str,
    position: f64,
    (ink, faint, background): (Color32, Color32, Color32),
) {
    let table = format!("{osc} Table ");
    let frames: Vec<&[f32]> = shown
        .series
        .iter()
        .filter(|s| s.name.starts_with(&table))
        .map(|s| s.values.as_slice())
        .collect();
    let current = shown.series(&format!("{osc} Frame"));
    let depth = |k: usize| k as f32 / frames.len().saturating_sub(1).max(1) as f32;
    let draw = |values: &[f32], t: f32, colour: Color32| {
        let base = area.top() + area.height() * (0.4 + 0.45 * t);
        let amplitude = area.height() * 0.2;
        let left = area.left() + area.width() * 0.2 * (1.0 - t);
        let width = area.width() * 0.8;
        let points: Vec<Pos2> = values
            .iter()
            .enumerate()
            .map(|(i, v)| {
                pos2(
                    left + width * i as f32 / (values.len() - 1).max(1) as f32,
                    base - amplitude * v,
                )
            })
            .collect();
        // What is behind the frame is hidden under it.
        let mut mesh = Mesh::default();
        for point in &points {
            mesh.colored_vertex(*point, background);
            mesh.colored_vertex(pos2(point.x, base + amplitude), background);
        }
        for i in 0..points.len().saturating_sub(1) as u32 {
            let at = 2 * i;
            mesh.add_triangle(at, at + 1, at + 2);
            mesh.add_triangle(at + 1, at + 2, at + 3);
        }
        painter.add(Shape::mesh(mesh));
        painter.add(Shape::line(points, Stroke::new(1.0, colour)));
    };
    let position = position as f32;
    let mut drawn_current = false;
    for (k, values) in frames.iter().enumerate() {
        if let Some(current) = current.filter(|_| !drawn_current && depth(k) > position) {
            draw(&current.values, position, ink);
            drawn_current = true;
        }
        draw(values, depth(k), faint.gamma_multiply(0.6));
    }
    if let Some(current) = current.filter(|_| !drawn_current) {
        draw(&current.values, position, ink);
    }
}

/// The frame at Position as a closed curve about a ring, each point's
/// distance from the centre following its value.
fn polar(
    painter: &Painter,
    area: Rect,
    shown: &DeviceDisplay,
    osc: &str,
    (ink, ring): (Color32, Color32),
) {
    let centre = area.center();
    let reach = area.width().min(area.height()) * 0.45;
    painter.circle_stroke(centre, reach * 0.6, Stroke::new(1.0, ring));
    let Some(frame) = shown.series(&format!("{osc} Frame")) else {
        return;
    };
    let n = frame.values.len().max(1);
    let mut points: Vec<Pos2> = frame
        .values
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let turn = std::f32::consts::TAU * i as f32 / n as f32 - std::f32::consts::FRAC_PI_2;
            centre + vec2(turn.cos(), turn.sin()) * reach * (0.6 + 0.4 * v)
        })
        .collect();
    points.extend(points.first().copied());
    painter.add(Shape::line(points, Stroke::new(1.5, ink)));
}

/// A drag across the display sets the oscillator's Position by the share of
/// the cell it covers, as one undo step named as a drag on the knob is.
fn drag_position(
    ui: &mut Ui,
    panel: &DevicePanel,
    (device, osc): (bl_model::DeviceId, u8),
    (id, rect): (Id, Rect),
    events: &mut Vec<UiEvent>,
) {
    let params = &panel.description.params;
    let param = wavetable::osc(usize::from(osc), POSITION);
    let Some(index) = params.iter().position(|p| p.id == param) else {
        return;
    };
    let response = ui.interact(rect, id.with("drag"), Sense::drag());
    let drag_id = id.with("travel");
    if response.drag_started() {
        let start = params[index].normalize(panel.value(index));
        ui.data_mut(|data| data.insert_temp(drag_id, (start, 0.0f64)));
        events.push(UiEvent::BeginNamedGesture(gesture_name(&params[index])));
    }
    if response.dragged() {
        let moved = f64::from(response.drag_delta().x / rect.width());
        let (start, travelled) = ui.data_mut(|data| {
            let entry = data.get_temp_mut_or_default::<(f64, f64)>(drag_id);
            entry.1 += moved;
            *entry
        });
        let value = params[index].denormalize((start + travelled).clamp(0.0, 1.0));
        if value != panel.value(index) {
            events.push(UiEvent::Invoke(Invocation::new(
                "device.set_param",
                json!({ "device": device, "param": param, "value": value }),
            )));
        }
    }
    if response.drag_stopped() {
        ui.data_mut(|data| data.remove::<(f64, f64)>(drag_id));
        events.push(UiEvent::EndGesture);
    }
}

/// What a cell of the grid says: the source routed to the destination.
fn cell_name(source: &str, destination: &str) -> String {
    format!("{source} to {destination}")
}

/// The matrix as a grid (R39): the sources as columns, the destinations
/// shown as rows, each cell a bar of its route's amount. A vertical drag in
/// a cell sets the amount, and in an empty cell takes a free slot for the
/// pair in the same undo step; Delete clears the route. An Add Destination
/// chooser under the grid lists the destinations not shown, and a row's
/// menu removes it.
fn grid(
    ui: &mut Ui,
    theme: &Theme,
    frame: &DeviceFrame,
    panel: &DevicePanel,
    (id, rect): (Id, Rect),
    events: &mut Vec<UiEvent>,
) {
    let Some(grid) = &panel.matrix_grid else {
        return;
    };
    let c = &theme.colors;
    let ink = if frame.enabled {
        c.accent.0
    } else {
        c.inactive.0
    };
    let inner = rect.shrink(2.0);
    let chooser = Rect::from_min_size(
        pos2(inner.left(), inner.bottom() - CHOOSER_HEIGHT),
        vec2(LABEL_WIDTH, CHOOSER_HEIGHT),
    );
    let columns = grid.sources.len().max(1) as f32;
    let width = ((inner.width() - LABEL_WIDTH) / columns).max(8.0);
    let painter = ui.painter_at(rect);
    let font = FontId::proportional(DEVICE_TEXT);
    for (i, source) in grid.sources.iter().enumerate() {
        let at = Rect::from_min_size(
            pos2(inner.left() + LABEL_WIDTH + width * i as f32, inner.top()),
            vec2(width, HEADER_HEIGHT),
        );
        let text = truncated(source, |t| {
            painter
                .layout_no_wrap(t.to_owned(), font.clone(), c.text.0)
                .size()
                .x
                <= width - 2.0
        });
        painter.text(
            at.center(),
            Align2::CENTER_CENTER,
            text,
            font.clone(),
            c.text_dim.0,
        );
    }
    let rows = Rect::from_min_max(
        pos2(inner.left(), inner.top() + HEADER_HEIGHT),
        pos2(inner.right(), chooser.top() - 2.0),
    );
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rows));
    child.spacing_mut().item_spacing = vec2(0.0, 0.0);
    // Only the rows in view are drawn: a grid can hold every destination.
    egui::ScrollArea::vertical()
        .id_salt(id.with("rows"))
        .auto_shrink([false, false])
        .show_rows(&mut child, ROW_HEIGHT, grid.rows.len(), |ui, shown| {
            for row in &grid.rows[shown] {
                self::row(ui, theme, frame, (grid, row), (id, width, ink), events);
            }
        });
    add_destination(ui, theme, frame, grid, (id, chooser), events);
}

/// One row of the grid: its destination's name, with a menu that removes
/// it, and a cell for each source.
fn row(
    ui: &mut Ui,
    theme: &Theme,
    frame: &DeviceFrame,
    (grid, row): (&MatrixGrid, &GridRow),
    (id, width, ink): (Id, f32, Color32),
    events: &mut Vec<UiEvent>,
) {
    let c = &theme.colors;
    let font = FontId::proportional(DEVICE_TEXT);
    let (strip, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::hover());
    let label = Rect::from_min_size(strip.min, vec2(LABEL_WIDTH, ROW_HEIGHT));
    let response = ui.interact(label, id.with(("row", &row.destination)), Sense::click());
    let text = truncated(&row.destination, |t| {
        ui.painter()
            .layout_no_wrap(t.to_owned(), font.clone(), c.text.0)
            .size()
            .x
            <= LABEL_WIDTH - 6.0
    });
    ui.painter().text(
        pos2(label.left() + 3.0, label.center().y),
        Align2::LEFT_CENTER,
        text,
        font,
        c.text.0,
    );
    response.context_menu(|ui| {
        if ui.button("Remove").clicked() {
            events.push(UiEvent::Invoke(Invocation::new(
                REMOVE_DESTINATION,
                json!({ "device": frame.id, "destination": row.destination }),
            )));
            ui.close();
        }
    });
    ui.ctx().accesskit_node_builder(response.id, |node| {
        node.set_role(Role::Row);
        node.set_label(row.destination.clone());
    });
    for (i, source) in grid.sources.iter().enumerate() {
        let at = Rect::from_min_size(
            pos2(strip.left() + LABEL_WIDTH + width * i as f32, strip.top()),
            vec2(width, ROW_HEIGHT),
        )
        .shrink(0.5);
        let amount = row.cells[i].map(|cell| cell.amount);
        grid_cell(
            ui,
            theme,
            frame,
            (source, &row.destination, amount),
            (id.with(("cell", &row.destination, source)), at, ink),
            events,
        );
    }
}

/// A grid cell: a bar from its middle up for a positive amount and down for
/// a negative one, and the gestures that set and clear it.
fn grid_cell(
    ui: &mut Ui,
    theme: &Theme,
    frame: &DeviceFrame,
    (source, destination, amount): (&str, &str, Option<f64>),
    (id, rect, ink): (Id, Rect, Color32),
    events: &mut Vec<UiEvent>,
) {
    let c = &theme.colors;
    let response = ui.interact(rect, id, Sense::click_and_drag());
    crate::focus::register(ui.ctx(), id, bl_shell::Area::Detail, None);
    if response.clicked() || response.drag_started() {
        response.request_focus();
    }
    if response.gained_focus() {
        events.push(UiEvent::FocusValue(true));
    }
    if response.lost_focus() {
        events.push(UiEvent::FocusValue(false));
    }
    let shown = amount.unwrap_or(0.0);
    let set = |value: f64| {
        UiEvent::Invoke(Invocation::new(
            SET_ROUTE,
            json!({
                "device": frame.id,
                "source": source,
                "destination": destination,
                "amount": value.clamp(-100.0, 100.0),
            }),
        ))
    };
    let drag_id = id.with("drag");
    if response.drag_started() {
        ui.data_mut(|data| data.insert_temp(drag_id, (shown, 0.0f64)));
        events.push(UiEvent::BeginNamedGesture(
            "Change Modulation Route".to_owned(),
        ));
    }
    if response.dragged() {
        let moved = -f64::from(response.drag_delta().y) * PER_POINT;
        let (start, travelled) = ui.data_mut(|data| {
            let entry = data.get_temp_mut_or_default::<(f64, f64)>(drag_id);
            entry.1 += moved;
            *entry
        });
        let value = (start + travelled).clamp(-100.0, 100.0).round();
        if amount != Some(value) {
            events.push(set(value));
        }
    }
    if response.drag_stopped() {
        ui.data_mut(|data| data.remove::<(f64, f64)>(drag_id));
        events.push(UiEvent::EndGesture);
    }
    // The arrow keys and a screen reader's steps, and Delete.
    let mut steps = ui.input(|i| {
        i.num_accesskit_action_requests(id, Action::Increment) as i32
            - i.num_accesskit_action_requests(id, Action::Decrement) as i32
    });
    let mut clear = false;
    if response.has_focus() {
        ui.memory_mut(|m| {
            m.set_focus_lock_filter(
                id,
                egui::EventFilter {
                    vertical_arrows: true,
                    ..Default::default()
                },
            );
        });
        ui.input(|i| {
            for event in &i.events {
                if let Event::Key {
                    key, pressed: true, ..
                } = event
                {
                    match key {
                        Key::ArrowUp => steps += 1,
                        Key::ArrowDown => steps -= 1,
                        Key::Delete | Key::Backspace => clear = amount.is_some(),
                        _ => {}
                    }
                }
            }
        });
    }
    if steps != 0 {
        events.push(UiEvent::BeginNamedGesture(
            "Change Modulation Route".to_owned(),
        ));
        events.push(set(shown + f64::from(steps) * STEP));
        events.push(UiEvent::EndGesture);
    }
    if clear {
        events.push(UiEvent::Invoke(Invocation::new(
            CLEAR_ROUTE,
            json!({ "device": frame.id, "source": source, "destination": destination }),
        )));
    }
    let painter = ui.painter();
    painter.rect_filled(rect, 0.0, c.display.0);
    let middle = rect.center().y;
    if let Some(amount) = amount.filter(|a| *a != 0.0) {
        let half = rect.height() / 2.0 - 1.0;
        let reach = (amount / 100.0) as f32 * half;
        painter.rect_filled(
            Rect::from_min_max(
                pos2(rect.left() + 1.0, (middle - reach).min(middle)),
                pos2(rect.right() - 1.0, (middle - reach).max(middle)),
            ),
            0.0,
            ink,
        );
    }
    if amount.is_some() {
        painter.line_segment(
            [pos2(rect.left(), middle), pos2(rect.right(), middle)],
            Stroke::new(1.0, c.separator.0),
        );
    }
    if response.hovered() {
        painter.rect_stroke(
            rect,
            0.0,
            Stroke::new(1.0, c.text_dim.0),
            StrokeKind::Inside,
        );
    }
    crate::device_panel::focus_ring(ui, theme, rect, &response);
    ui.ctx().accesskit_node_builder(id, |node| {
        node.set_role(Role::Slider);
        node.set_label(cell_name(source, destination));
        node.set_value(format!("{shown:.0} %"));
        node.set_numeric_value(shown);
        node.set_min_numeric_value(-100.0);
        node.set_max_numeric_value(100.0);
        node.add_action(Action::Increment);
        node.add_action(Action::Decrement);
    });
}

/// The Add Destination chooser under the grid: a dark box that lists the
/// destinations not shown.
fn add_destination(
    ui: &mut Ui,
    theme: &Theme,
    frame: &DeviceFrame,
    grid: &MatrixGrid,
    (id, field): (Id, Rect),
    events: &mut Vec<UiEvent>,
) {
    let c = &theme.colors;
    let response = ui.interact(field, id.with("add"), Sense::click());
    crate::focus::register(ui.ctx(), id.with("add"), bl_shell::Area::Detail, None);
    let painter = ui.painter();
    painter.rect_filled(field, 1.0, c.window.0);
    painter.text(
        pos2(field.left() + 3.0, field.center().y),
        Align2::LEFT_CENTER,
        "Add Destination",
        FontId::proportional(DEVICE_TEXT),
        c.text.0,
    );
    paint_glyph(
        painter,
        Rect::from_center_size(pos2(field.right() - 6.0, field.center().y), vec2(5.0, 5.0)),
        Glyph::Down,
        c.text.0,
    );
    crate::device_panel::focus_ring(ui, theme, field, &response);
    egui::Popup::menu(&response).show(|ui| {
        egui::ScrollArea::vertical()
            .max_height(220.0)
            .show(ui, |ui| {
                for name in &grid.addable {
                    if ui.button(name).clicked() {
                        events.push(UiEvent::Invoke(Invocation::new(
                            ADD_DESTINATION,
                            json!({ "device": frame.id, "destination": name }),
                        )));
                        ui.close();
                    }
                }
            });
    });
    ui.ctx().accesskit_node_builder(response.id, |node| {
        node.set_role(Role::ComboBox);
        node.set_label("Add Destination".to_owned());
    });
}

/// The command a sample dropped on an oscillator's display runs.
#[must_use]
pub fn drop_command(frame: &DeviceFrame, osc: u8, path: &str) -> Invocation {
    Invocation::new(
        LOAD_WAVETABLE,
        json!({ "device": frame.id, "oscillator": osc, "file": path }),
    )
}

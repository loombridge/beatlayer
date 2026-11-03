//! Meld's grids and their buttons (WP-8.20; the meld spec, R63 to R66): the
//! Matrix, MIDI and MPE tabs draw the sources of the engine shown across
//! and its destinations down, the Expanded View draws every source and
//! destination of both engines, a touch on a control adds its parameter as a
//! row while one of the tabs shows, and Copy to the other engine and X act on
//! the engine shown. Every gesture runs the commands an agent runs
//! (`device.set_route`, `device.clear_route`, `device.add_destination`,
//! `device.remove_destination`, `meld.copy_modulation` and
//! `meld.clear_modulation`).

use bl_devices::meld::{self, EXPANDED_CELL, GRID_CELLS};
use bl_model::DeviceKind;
use bl_shell::Invocation;
use bl_shell::device_commands::{ADD_DESTINATION, DeviceFrame, DevicePanel, GridRow, MatrixGrid};
use egui::accesskit::Role;
use egui::{Align2, FontId, Id, Rect, Sense, Ui, pos2, vec2};
use serde_json::json;

use crate::device_panel::DEVICE_TEXT;
use crate::main_window::UiEvent;
use crate::theme::Theme;

/// The height of the strip above a grid that holds its buttons.
const HEADER: f32 = 14.0;
/// The widths of Copy to the other engine and of X.
const COPY_WIDTH: f32 = 54.0;
const CLEAR_WIDTH: f32 = 18.0;

/// The destinations no engine owns, which both engines' routes can reach.
const GLOBAL: [&str; 2] = ["Mixer Drive", "Volume"];

/// The columns each tab shows besides the engine's own sources: the Matrix
/// tab's Spread, the MIDI tab's and the MPE tab's sources.
const MATRIX_EXTRA: [&str; 1] = ["Spread"];
const MIDI: [&str; 6] = [
    "Velocity",
    "Pitch",
    "Random",
    "Pitch Bend",
    "Press",
    "Mod Wheel",
];
const MPE: [&str; 3] = ["MPE Pitch Bend", "MPE Slide", "MPE Press"];

/// Whether `frame` is a Meld.
#[must_use]
pub fn is_meld(frame: &DeviceFrame) -> bool {
    matches!(&frame.kind, DeviceKind::Native { device_type } if device_type == meld::MELD)
}

/// Which grid the cell called `name` is: the engine (0 or 1) and the tab
/// (0 Matrix, 1 MIDI, 2 MPE) of one of the tabs', or none for the Expanded
/// View's.
fn grid_of_cell(name: &str) -> Option<(usize, usize)> {
    ["A", "B"].iter().enumerate().find_map(|(engine, letter)| {
        let rest = name.strip_prefix(letter)?.strip_prefix(' ')?;
        let tab = GRID_CELLS.iter().position(|cell| *cell == rest)?;
        Some((engine, tab))
    })
}

/// `grid` narrowed to `columns` and to the rows `rows` keeps.
fn narrowed(
    grid: &MatrixGrid,
    columns: impl Fn(&str) -> bool,
    rows: impl Fn(&str) -> bool,
) -> MatrixGrid {
    let keep: Vec<usize> = grid
        .sources
        .iter()
        .enumerate()
        .filter(|(_, source)| columns(source))
        .map(|(at, _)| at)
        .collect();
    MatrixGrid {
        sources: keep.iter().map(|&at| grid.sources[at].clone()).collect(),
        rows: grid
            .rows
            .iter()
            .filter(|row| rows(&row.destination))
            .map(|row| GridRow {
                destination: row.destination.clone(),
                cells: keep.iter().map(|&at| row.cells[at]).collect(),
            })
            .collect(),
        addable: grid
            .addable
            .iter()
            .filter(|name| rows(name))
            .cloned()
            .collect(),
    }
}

/// Whether `name` is one of engine `engine`'s destinations or a global one.
fn engine_row(name: &str, engine: usize) -> bool {
    let letter = ["A", "B"][engine];
    name.strip_prefix(letter)
        .is_some_and(|rest| rest.starts_with(' '))
        || GLOBAL.contains(&name)
}

/// The grid a tab or the Expanded View draws, from the device's whole one.
fn view(grid: &MatrixGrid, (engine, tab): (usize, usize)) -> MatrixGrid {
    let letter = ["A", "B"][engine];
    let own = |source: &str| {
        source
            .strip_prefix(letter)
            .is_some_and(|rest| rest.starts_with(' '))
    };
    narrowed(
        grid,
        |source| match tab {
            0 => own(source) || MATRIX_EXTRA.contains(&source),
            1 => MIDI.contains(&source),
            _ => MPE.contains(&source),
        },
        |name| engine_row(name, engine),
    )
}

/// The engine whose page the device shows (0 for A).
#[must_use]
pub fn engine_shown(panel: &DevicePanel) -> usize {
    panel
        .pages
        .iter()
        .find(|node| node.node == "Engine")
        .map_or(0, |node| usize::from(node.shown == "B"))
}

/// Whether one of the engine shown's grid tabs is the page shown.
fn grid_tab_shown(panel: &DevicePanel) -> bool {
    let letter = ["A", "B"][engine_shown(panel)];
    panel
        .pages
        .iter()
        .find(|node| node.node == format!("{letter} Tabs"))
        .is_some_and(|node| ["Matrix", "MIDI", "MPE"].contains(&node.shown.as_str()))
}

/// Draws Meld's cell called `name` in `rect` if it is one of its grids;
/// false for any other.
pub fn cell(
    ui: &mut Ui,
    theme: &Theme,
    frame: &DeviceFrame,
    panel: &DevicePanel,
    (id, name, rect): (Id, &str, Rect),
    events: &mut Vec<UiEvent>,
) -> bool {
    let tab = grid_of_cell(name);
    if tab.is_none() && name != EXPANDED_CELL {
        return false;
    }
    let Some(grid) = &panel.matrix_grid else {
        return false;
    };
    let _ = ui.interact(rect, id, Sense::hover());
    let shown = match tab {
        Some(of) => view(grid, of),
        None => narrowed(grid, |_| true, |_| true),
    };
    // The Matrix tab has Copy to the other engine and X in its header.
    let rows = if let Some((engine, 0)) = tab {
        buttons(ui, theme, frame, engine, (id, rect), events);
        Rect::from_min_max(pos2(rect.left(), rect.top() + HEADER), rect.max)
    } else {
        rect
    };
    crate::wavetable_panel::grid_of(ui, theme, frame, &shown, (id, rows), events);
    ui.ctx().accesskit_node_builder(id, |node| {
        node.set_role(Role::Table);
        node.set_label(name.to_owned());
    });
    true
}

/// Copy to the other engine and X, along the top of an engine's Matrix tab.
fn buttons(
    ui: &mut Ui,
    theme: &Theme,
    frame: &DeviceFrame,
    engine: usize,
    (id, rect): (Id, Rect),
    events: &mut Vec<UiEvent>,
) {
    let c = &theme.colors;
    let letter = ["A", "B"][engine];
    let other = ["B", "A"][engine];
    let label = format!("Copy to {other}");
    let clear = Rect::from_min_size(
        pos2(rect.right() - CLEAR_WIDTH - 2.0, rect.top() + 1.0),
        vec2(CLEAR_WIDTH, HEADER - 2.0),
    );
    let copy = Rect::from_min_size(
        pos2(clear.left() - COPY_WIDTH - 3.0, clear.top()),
        vec2(COPY_WIDTH, HEADER - 2.0),
    );
    let font = FontId::proportional(DEVICE_TEXT);
    for (key, at, text, command, access) in [
        (
            "copy",
            copy,
            label.clone(),
            "meld.copy_modulation",
            label.clone(),
        ),
        (
            "clear",
            clear,
            "X".to_owned(),
            "meld.clear_modulation",
            format!("Clear {letter} Modulation"),
        ),
    ] {
        let response = ui.interact(at, id.with(key), Sense::click());
        crate::focus::register(ui.ctx(), id.with(key), bl_shell::Area::Detail, None);
        let painter = ui.painter();
        painter.rect_filled(
            at,
            1.0,
            if response.hovered() {
                c.separator.0
            } else {
                c.window.0
            },
        );
        painter.text(
            at.center(),
            Align2::CENTER_CENTER,
            text,
            font.clone(),
            c.text.0,
        );
        crate::device_panel::focus_ring(ui, theme, at, &response);
        if response.clicked() {
            events.push(UiEvent::Invoke(Invocation::new(
                command,
                json!({ "device": frame.id, "engine": letter }),
            )));
        }
        ui.ctx().accesskit_node_builder(response.id, |node| {
            node.set_role(Role::Button);
            node.set_label(access);
        });
    }
}

/// A touch on a control while one of the engine shown's grid tabs shows adds
/// its parameter as a row (R63): the destinations are named as their
/// parameters are, so the control's name is the destination's.
pub fn touched(
    ui: &Ui,
    frame: &DeviceFrame,
    panel: &DevicePanel,
    (name, rect): (&str, Rect),
    events: &mut Vec<UiEvent>,
) {
    if !is_meld(frame) || !grid_tab_shown(panel) {
        return;
    }
    let pressed = ui.input(|i| {
        i.pointer.primary_pressed() && i.pointer.press_origin().is_some_and(|at| rect.contains(at))
    });
    let addable = panel
        .matrix_grid
        .as_ref()
        .is_some_and(|grid| grid.addable.iter().any(|row| row == name));
    if pressed && addable {
        events.push(UiEvent::Invoke(Invocation::new(
            ADD_DESTINATION,
            json!({ "device": frame.id, "destination": name }),
        )));
    }
}

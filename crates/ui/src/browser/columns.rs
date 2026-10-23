//! The content pane's column header row and its Content Options menu (spec:
//! browser-tags R27, R28): the columns after Name, sorting by a click on a
//! column's name, and moving a column by dragging it.

use bl_shell::Shell;
use bl_shell::browser_commands::{self as ids, Column, ItemDetails};
use egui::accesskit::{Role, SortDirection};
use egui::{Align2, DragAndDrop, Id, Rect, Sense, Stroke, Ui, pos2, vec2};
use serde_json::json;

use super::{BrowserUi, Glyph, bounds, glyph, invoke};
use crate::layout::BrowserLayout;
use crate::main_window::UiEvent;
use crate::theme::Theme;

/// A column's header cell, by the column.
#[must_use]
pub fn column_id(column: Column) -> Id {
    Id::new(("browser-column", column))
}

/// The Content Options menu's button, at the header row's right.
#[must_use]
pub fn content_options_id() -> Id {
    Id::new("browser-content-options")
}

/// What a drag of a column's name carries.
#[derive(Clone, Copy, Debug)]
struct ColumnDrag(Column);

/// Where a column sits in the header row and the list, in window
/// coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Span {
    /// The column.
    pub column: Column,
    /// Its left edge.
    pub left: f32,
    /// Its right edge.
    pub right: f32,
}

/// The least the Name column keeps.
const NAME_MIN: f32 = 70.0;

/// How wide a column likes to be, in points.
const fn preferred(column: Column) -> f32 {
    match column {
        Column::Name => 0.0,
        Column::DateModified => 82.0,
        Column::Size => 46.0,
        Column::Kind => 78.0,
        Column::Length => 46.0,
        Column::SampleRate => 62.0,
        Column::Channels => 56.0,
    }
}

/// Where the shown columns sit in a row from `left` to `right`: they share
/// the width and Name takes what they leave (spec: browser-tags R27).
/// Returns Name's right edge and the others.
pub(super) fn spans(shown: &[Column], left: f32, right: f32) -> (f32, Vec<Span>) {
    let wanted: f32 = shown.iter().map(|&c| preferred(c)).sum();
    let room = (right - left - NAME_MIN).max(0.0);
    let scale = if wanted > room && wanted > 0.0 {
        room / wanted
    } else {
        1.0
    };
    let mut edge = right;
    let mut out: Vec<Span> = Vec::new();
    for &column in shown.iter().rev() {
        let width = preferred(column) * scale;
        out.push(Span {
            column,
            left: edge - width,
            right: edge,
        });
        edge -= width;
    }
    out.reverse();
    (edge, out)
}

/// What a column shows for an item, if it has anything.
pub(super) fn cell_text(column: Column, details: &ItemDetails) -> Option<String> {
    match column {
        Column::Name => None,
        Column::DateModified => details.modified.map(date),
        Column::Size => details.size.map(size),
        Column::Kind => details.kind.clone(),
        Column::Length => details.length_seconds.map(length),
        Column::SampleRate => details.sample_rate.map(sample_rate),
        Column::Channels => details.channels.map(channels),
    }
}

/// A date and time, as the Date Modified column shows it: UTC, as
/// `2026-10-23 14:05`.
fn date(seconds: i64) -> String {
    let days = seconds.div_euclid(86_400);
    let of_day = seconds.rem_euclid(86_400);
    // Days since 1970-01-01 to a civil date (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let mp = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}",
        of_day / 3_600,
        of_day % 3_600 / 60
    )
}

/// A size in bytes, as `812 B`, `4.8 KB` or `12 MB`.
fn size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 || value >= 10.0 {
        format!("{value:.0} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// A length in seconds, as `0:01.2`.
fn length(seconds: f64) -> String {
    let tenths = (seconds * 10.0).round() as i64;
    format!("{}:{:02}.{}", tenths / 600, tenths / 10 % 60, tenths % 10)
}

/// A sample rate in hertz, as `44.1 kHz`.
fn sample_rate(hertz: u32) -> String {
    let khz = f64::from(hertz) / 1000.0;
    if (khz - khz.round()).abs() < 1e-9 {
        format!("{khz:.0} kHz")
    } else {
        format!("{khz:.1} kHz")
    }
}

/// A channel count, as `Mono` or `Stereo`.
fn channels(count: u16) -> String {
    match count {
        1 => "Mono".to_owned(),
        2 => "Stereo".to_owned(),
        n => format!("{n} ch"),
    }
}

impl BrowserUi {
    /// The column header row (L-BRW-009): Name and the columns shown, each
    /// a click away from sorting by it, a drag away from moving, and a
    /// right-click away from the Content Options menu; and the menu's
    /// button at its right.
    #[expect(clippy::too_many_arguments, reason = "the row's inputs")]
    pub(super) fn header(
        &mut self,
        ui: &mut Ui,
        shell: &Shell,
        theme: &Theme,
        layout: &BrowserLayout,
        columns: &[Span],
        name_right: f32,
        events: &mut Vec<UiEvent>,
    ) {
        let view = shell.browser_view();
        let state = view.columns().clone();
        let header = layout.header;
        let painter = ui.painter_at(layout.content);
        painter.rect_filled(header, 0.0, theme.colors.ruler.0);
        painter.line_segment(
            [header.left_bottom(), header.right_bottom()],
            Stroke::new(1.0, theme.colors.separator.0),
        );
        painter.line_segment(
            [layout.content.left_top(), layout.content.left_bottom()],
            Stroke::new(1.0, theme.colors.separator.0),
        );
        let mut cells: Vec<(Column, Rect)> = vec![(
            Column::Name,
            Rect::from_min_max(header.min, pos2(name_right, header.bottom())),
        )];
        cells.extend(columns.iter().map(|span| {
            (
                span.column,
                Rect::from_min_max(
                    pos2(span.left, header.top()),
                    pos2(span.right, header.bottom()),
                ),
            )
        }));
        let dragging = DragAndDrop::payload::<ColumnDrag>(ui.ctx()).map(|drag| drag.0);
        let pointer = ui.input(|i| i.pointer.hover_pos());
        let released = ui.input(|i| i.pointer.any_released());
        for &(column, rect) in &cells {
            let id = column_id(column);
            let response = ui.interact(rect, id, Sense::click_and_drag());
            if response.hovered() {
                painter.rect_filled(rect, 0.0, theme.colors.control_hover.0);
            }
            painter.with_clip_rect(rect).text(
                pos2(rect.left() + 6.0, rect.center().y),
                Align2::LEFT_CENTER,
                column.title(),
                theme.small_font(),
                theme.colors.text_dim.0,
            );
            if state.sort == column {
                // The column sorted by shows an arrow (spec: R28).
                let arrow = Rect::from_center_size(
                    pos2(rect.right() - 8.0, rect.center().y),
                    vec2(8.0, 8.0),
                );
                glyph(
                    &painter,
                    arrow,
                    if state.descending {
                        Glyph::Up
                    } else {
                        Glyph::Down
                    },
                    theme.colors.text.0,
                );
            }
            if response.clicked() {
                let args = if state.sort == column {
                    json!({ "descending": !state.descending })
                } else {
                    json!({ "sort": column, "descending": false })
                };
                invoke(events, ids::SET_COLUMNS, args);
            }
            if column != Column::Name && response.drag_started() {
                DragAndDrop::set_payload(ui.ctx(), ColumnDrag(column));
            }
            response.context_menu(|ui| self.content_options(ui, shell, events));
            ui.ctx().accesskit_node_builder(id, |node| {
                node.set_role(Role::ColumnHeader);
                node.set_label(column.title().to_owned());
                node.set_bounds(bounds(rect));
                if state.sort == column {
                    node.set_sort_direction(if state.descending {
                        SortDirection::Descending
                    } else {
                        SortDirection::Ascending
                    });
                }
            });
        }
        // A column dragged over the row shows where it would land; released
        // there it moves (spec: browser-tags R27, I-DRAG-057).
        if let (Some(moving), Some(at)) = (dragging, pointer)
            && header.contains(at)
        {
            let target = Self::column_drop(moving, columns, at.x);
            let x = columns
                .iter()
                .filter(|span| span.column != moving)
                .nth(target)
                .map_or_else(
                    || columns.last().map_or(name_right, |span| span.right),
                    |span| span.left,
                );
            painter.line_segment(
                [pos2(x, header.top()), pos2(x, header.bottom())],
                Stroke::new(2.0, theme.colors.accent.0),
            );
            if released {
                let mut shown = state.shown.clone();
                shown.retain(|&c| c != moving);
                shown.insert(target.min(shown.len()), moving);
                invoke(events, ids::SET_COLUMNS, json!({ "shown": shown }));
                DragAndDrop::clear_payload(ui.ctx());
            }
        } else if dragging.is_some() && released {
            DragAndDrop::clear_payload(ui.ctx());
        }
        // The Content Options menu's button.
        let button = layout.column_menu;
        let response = ui.interact(button, content_options_id(), Sense::click());
        crate::focus::register(
            ui.ctx(),
            content_options_id(),
            bl_shell::Area::Browser,
            None,
        );
        if response.hovered() {
            painter.rect_filled(button, 0.0, theme.colors.control_hover.0);
        }
        glyph(&painter, button, Glyph::Menu, theme.colors.text_dim.0);
        ui.ctx()
            .accesskit_node_builder(content_options_id(), |node| {
                node.set_role(Role::Button);
                node.set_label("Content Options".to_owned());
                node.set_bounds(bounds(button));
            });
        egui::Popup::menu(&response).show(|ui| self.content_options(ui, shell, events));
    }

    /// Where among the other shown columns a dragged column would land if
    /// released with the pointer at `x`: after each column the pointer is
    /// right of the middle of.
    fn column_drop(moving: Column, columns: &[Span], x: f32) -> usize {
        columns
            .iter()
            .filter(|span| span.column != moving)
            .filter(|span| x > (span.left + span.right) / 2.0)
            .count()
    }

    /// The Content Options menu (C-CONTENT; spec: browser-tags R27): the
    /// optional columns, ticked while shown, and Show File Extensions.
    pub(super) fn content_options(&self, ui: &mut Ui, shell: &Shell, events: &mut Vec<UiEvent>) {
        let state = shell.browser_view().columns().clone();
        let mut first = true;
        for column in Column::OPTIONAL {
            let on = state.shown.contains(&column);
            let mark = if on { "\u{2714} " } else { "" };
            let response = ui.button(format!("{mark}{}", column.title()));
            crate::menus::register_item(ui.ctx(), &response);
            if first {
                crate::menus::take_focus_first(ui, &response);
                first = false;
            }
            if response.clicked() {
                let mut shown = state.shown.clone();
                if on {
                    shown.retain(|&c| c != column);
                } else {
                    shown.push(column);
                }
                invoke(events, ids::SET_COLUMNS, json!({ "shown": shown }));
                ui.close();
            }
        }
        ui.separator();
        let mark = if state.file_extensions {
            "\u{2714} "
        } else {
            ""
        };
        let response = ui.button(format!("{mark}Show File Extensions"));
        crate::menus::register_item(ui.ctx(), &response);
        if response.clicked() {
            invoke(
                events,
                ids::SET_COLUMNS,
                json!({ "file_extensions": !state.file_extensions }),
            );
            ui.close();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Spec: browser-tags R27.
    #[test]
    fn the_columns_share_the_width_and_name_takes_the_rest() {
        let (name_right, placed) = spans(&[Column::Size, Column::Kind], 110.0, 311.0);
        assert_eq!(placed.len(), 2);
        assert!((placed[1].right - 311.0).abs() < 1e-3);
        assert!((placed[0].right - placed[1].left).abs() < 1e-3);
        assert!(name_right >= 110.0 + NAME_MIN - 1e-3, "Name keeps its room");
        let (name_right, none) = spans(&[], 110.0, 311.0);
        assert!(none.is_empty());
        assert_eq!(name_right, 311.0);
        // More columns than the pane holds shrink to fit.
        let (name_right, many) = spans(&Column::OPTIONAL, 110.0, 311.0);
        assert!((name_right - (110.0 + NAME_MIN)).abs() < 1e-3);
        assert!(many.iter().all(|s| s.right - s.left > 0.0));
    }

    #[test]
    fn cells_read_in_words() {
        assert_eq!(date(0), "1970-01-01 00:00");
        assert_eq!(date(1_792_764_300), "2026-10-23 14:05");
        assert_eq!(size(812), "812 B");
        assert_eq!(size(4_900), "4.8 KB");
        assert_eq!(size(12 * 1024 * 1024), "12 MB");
        assert_eq!(length(1.24), "0:01.2");
        assert_eq!(length(75.0), "1:15.0");
        assert_eq!(sample_rate(44_100), "44.1 kHz");
        assert_eq!(sample_rate(48_000), "48 kHz");
        assert_eq!(channels(1), "Mono");
        assert_eq!(channels(6), "6 ch");
    }
}

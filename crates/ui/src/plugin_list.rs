//! The list of the set's plug-ins (IMP-29, WP-6.41; the plug-in list spec):
//! the detail area's contents while File ▸ Manage Plug-Ins shows it. It
//! lists every plug-in the set uses — its name and format, its vendor and
//! version, the preset names its devices hold, and the tracks it is on —
//! marks the plug-ins this computer doesn't have, and writes the list to a
//! text file.

use bl_shell::Shell;
use egui::accesskit::Role;
use egui::{Align2, Id, Rect, Sense, Ui, WidgetInfo, WidgetType, pos2, vec2};
use serde_json::json;

use crate::main_window::UiEvent;
use crate::theme::Theme;

/// The title bar's height, as the other views' title bars.
const TITLE: f32 = 17.0;
/// A row's height.
const ROW: f32 = 16.0;
/// The count line's height.
const LINE: f32 = 14.0;
/// The switch's and the Export button's height.
const CONTROL: f32 = 15.0;
/// Where the columns start, from the area's left.
const VENDOR: f32 = 220.0;
const VERSION: f32 = 340.0;
const PRESETS: f32 = 410.0;
const TRACKS: f32 = 560.0;

/// What the rows are drawn with: the set, the theme, the area they are in,
/// and the commands they run.
struct Rows<'a> {
    shell: &'a Shell,
    theme: &'a Theme,
    /// The area the rows are in.
    area: Rect,
    events: &'a mut Vec<UiEvent>,
}

/// The list of the set's plug-ins.
#[derive(Clone, Debug, Default)]
pub struct PluginListUi {
    /// How far the rows are scrolled.
    scroll: f32,
}

/// The ID of the list's table.
#[must_use]
pub fn list_id() -> Id {
    Id::new("plugin-list")
}

/// The ID of the list's close button.
#[must_use]
pub fn close_id() -> Id {
    Id::new("plugin-list-close")
}

/// The ID of the Show Only Missing switch.
#[must_use]
pub fn only_missing_id() -> Id {
    Id::new("plugin-list-only-missing")
}

/// The ID of the Export… button.
#[must_use]
pub fn export_id() -> Id {
    Id::new("plugin-list-export")
}

/// The ID of a track's name in a row.
#[must_use]
pub fn track_id(row: usize, track: usize) -> Id {
    Id::new(("plugin-list-track", row, track))
}

impl PluginListUi {
    /// Draws the list of the set's plug-ins in `area`.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        shell: &Shell,
        theme: &Theme,
        area: Rect,
        events: &mut Vec<UiEvent>,
    ) {
        let list = shell.set_plugin_list();
        let only_missing = shell.plugin_list().only_missing();
        let missing = list.plugins.iter().filter(|plugin| plugin.missing).count();
        let c = &theme.colors;
        let painter = ui.painter_at(area);
        painter.rect_filled(area, 0.0, c.panel.0);

        // The title bar, and the button that hides the list.
        let title = Rect::from_min_size(area.min, vec2(area.width(), TITLE));
        painter.rect_filled(title, 0.0, c.lane.0);
        painter.text(
            pos2(title.left() + 6.0, title.center().y),
            Align2::LEFT_CENTER,
            "Plug-Ins",
            theme.small_font(),
            c.text.0,
        );
        let close = Rect::from_center_size(
            pos2(title.right() - 10.0, title.center().y),
            vec2(12.0, 12.0),
        );
        crate::focus::register(ui.ctx(), close_id(), bl_shell::Area::Detail, None);
        let close_response = ui.interact(close, close_id(), Sense::click());
        painter.text(
            close.center(),
            Align2::CENTER_CENTER,
            "×",
            theme.small_font(),
            c.text_dim.0,
        );
        if close_response.clicked()
            || (close_response.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
        {
            invoke(events, ids::VIEW_TOGGLE_PLUGIN_LIST, json!({}));
        }
        close_response
            .widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, "Close Plug-Ins"));
        ui.ctx().accesskit_node_builder(close_id(), |node| {
            node.set_role(Role::Button);
            node.set_label("Close Plug-Ins".to_owned());
        });

        // Spec: plugin-list R2. How many plug-ins the set uses, and how
        // many of them this computer doesn't have.
        let count = Rect::from_min_size(
            area.min + vec2(6.0, TITLE + 2.0),
            vec2(area.width() - 12.0, LINE),
        );
        painter.text(
            pos2(count.left(), count.center().y),
            Align2::LEFT_CENTER,
            count_text(list.plugins.len(), missing),
            theme.small_font(),
            c.text_dim.0,
        );

        // Spec: plugin-list R3. The Show Only Missing switch.
        let switch = Rect::from_min_size(
            area.min + vec2(6.0, TITLE + 4.0 + LINE),
            vec2(area.width() - 12.0, CONTROL),
        );
        crate::focus::register(ui.ctx(), only_missing_id(), bl_shell::Area::Detail, None);
        let switch_response = ui.interact(switch, only_missing_id(), Sense::click());
        painter.text(
            pos2(switch.left(), switch.center().y),
            Align2::LEFT_CENTER,
            "Show Only Missing",
            theme.small_font(),
            c.text.0,
        );
        let state = if only_missing { "On" } else { "Off" };
        painter.text(
            pos2(switch.left() + 100.0, switch.center().y),
            Align2::LEFT_CENTER,
            state,
            theme.small_font(),
            c.text_dim.0,
        );
        if switch_response.clicked()
            || (switch_response.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
        {
            invoke(
                events,
                ids::VIEW_SET_PLUGIN_LIST_ONLY_MISSING,
                json!({ "only_missing": !only_missing }),
            );
        }
        switch_response
            .widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, "Show Only Missing"));
        ui.ctx().accesskit_node_builder(only_missing_id(), |node| {
            node.set_role(Role::Switch);
            node.set_label("Show Only Missing".to_owned());
            node.set_value(state.to_owned());
        });

        // The rows.
        let foot = Rect::from_min_size(
            pos2(area.left(), area.bottom() - CONTROL - 2.0),
            vec2(area.width(), CONTROL),
        );
        let rows_area = Rect::from_min_max(
            pos2(area.left(), switch.bottom() + 4.0),
            pos2(area.right(), foot.top()),
        );
        let shown: Vec<&bl_shell::plugin_commands::SetPlugin> = list
            .plugins
            .iter()
            .filter(|plugin| !only_missing || plugin.missing)
            .collect();
        self.scroll = scrolled(ui, rows_area, self.scroll, shown.len() as f32 * ROW);
        let first = (self.scroll / ROW).floor().max(0.0) as usize;
        let shown_rows =
            ((rows_area.height() / ROW).ceil() as usize + 1).min(shown.len().saturating_sub(first));
        let table = list_id();
        let _ = ui.interact(rows_area, table, Sense::hover());
        ui.ctx().accesskit_node_builder(table, |node| {
            node.set_role(Role::Table);
            node.set_label("Plug-Ins".to_owned());
            node.set_bounds(bounds(rows_area));
        });
        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(rows_area)
                .accessibility_parent(table),
            |ui| {
                ui.set_clip_rect(rows_area);
                let mut rows = Rows {
                    shell,
                    theme,
                    area: rows_area,
                    events,
                };
                for (at, plugin) in shown.iter().skip(first).take(shown_rows).enumerate() {
                    self.row(ui, &mut rows, first + at, plugin);
                }
            },
        );

        // Spec: plugin-list R7. The Export… button, at the list's foot.
        let export = Rect::from_min_size(pos2(area.left() + 6.0, foot.top()), vec2(80.0, CONTROL));
        crate::focus::register(ui.ctx(), export_id(), bl_shell::Area::Detail, None);
        let export_response = ui.interact(export, export_id(), Sense::click());
        painter.rect_filled(
            export,
            theme.sizes.corner,
            if export_response.hovered() {
                c.control_hover.0
            } else {
                c.control.0
            },
        );
        painter.text(
            export.center(),
            Align2::CENTER_CENTER,
            "Export…",
            theme.small_font(),
            c.text.0,
        );
        if export_response.clicked()
            || (export_response.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
        {
            invoke(events, ids::EXPORT_LIST, json!({}));
        }
        export_response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, "Export…"));
        ui.ctx().accesskit_node_builder(export_id(), |node| {
            node.set_role(Role::Button);
            node.set_label("Export…".to_owned());
        });
    }

    /// Draws one plug-in's row: its name and format, its vendor and
    /// version, the preset names its devices hold, and the tracks it is on.
    fn row(
        &mut self,
        ui: &mut Ui,
        rows: &mut Rows<'_>,
        index: usize,
        plugin: &bl_shell::plugin_commands::SetPlugin,
    ) {
        let (area, theme, shell) = (rows.area, rows.theme, rows.shell);
        let rect = Rect::from_min_size(
            pos2(area.left(), area.top() + index as f32 * ROW - self.scroll),
            vec2(area.width(), ROW),
        );
        let c = &theme.colors;
        let painter = ui.painter_at(rect);
        let ink = c.text.0;
        let dim = c.text_dim.0;

        // The row's columns, left to right: the name, the vendor, the
        // version, the preset names, the tracks, and the Missing mark.
        let column = |at: f32, to: f32| {
            Rect::from_min_size(pos2(rect.left() + at, rect.top()), vec2(to - at, ROW))
        };
        let name_at = column(0.0, VENDOR);
        let vendor_at = column(VENDOR, VERSION);
        let version_at = column(VERSION, PRESETS);
        let presets_at = column(PRESETS, TRACKS);
        let tracks_at = column(TRACKS, rect.width() - 70.0);
        let missing_at = column(rect.width() - 70.0, rect.width());

        // The name, with its format's badge before it (L-BRW-016).
        let mut left = name_at.left() + 6.0;
        let badge = painter.text(
            pos2(left + 2.0, rect.center().y),
            Align2::LEFT_CENTER,
            plugin.format.short_name(),
            theme.small_font(),
            dim,
        );
        painter.rect_stroke(
            badge.expand2(vec2(2.0, 0.0)),
            2.0,
            egui::Stroke::new(1.0, dim),
            egui::StrokeKind::Inside,
        );
        left = badge.right() + 6.0;
        painter.text(
            pos2(left, rect.center().y),
            Align2::LEFT_CENTER,
            &plugin.name,
            theme.small_font(),
            ink,
        );
        painter.text(
            pos2(vendor_at.left() + 4.0, rect.center().y),
            Align2::LEFT_CENTER,
            plugin.vendor.as_deref().unwrap_or_default(),
            theme.small_font(),
            dim,
        );
        painter.text(
            pos2(version_at.left() + 4.0, rect.center().y),
            Align2::LEFT_CENTER,
            plugin.version.as_deref().unwrap_or_default(),
            theme.small_font(),
            dim,
        );
        painter.text(
            pos2(presets_at.left() + 4.0, rect.center().y),
            Align2::LEFT_CENTER,
            plugin.preset_names.join(", "),
            theme.small_font(),
            dim,
        );
        // Spec: plugin-list R2. A plug-in this computer doesn't have is
        // marked.
        let marker = if plugin.missing { "Missing" } else { "" };
        painter.text(
            pos2(missing_at.right() - 8.0, rect.center().y),
            Align2::RIGHT_CENTER,
            marker,
            theme.small_font(),
            if plugin.missing { c.error_text.0 } else { dim },
        );

        // The row, and its cells, for assistive technology (R8).
        let row = Id::new(("plugin-list-row", index));
        let response = ui.interact(rect, row, Sense::hover());
        // Spec: plugin-list R2. The reason the scanner blocklisted a
        // plug-in's file, on the row.
        if let Some(reason) = &plugin.blocked {
            response.on_hover_text(format!("Missing: {reason}"));
        }
        ui.ctx().accesskit_node_builder(row, |node| {
            node.set_role(Role::Row);
            node.set_label(plugin.name.clone());
            node.set_bounds(bounds(rect));
            if let Some(reason) = &plugin.blocked {
                node.set_description(format!("Missing: {reason}"));
            }
        });
        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(rect)
                .accessibility_parent(row),
            |ui| {
                cell(
                    ui,
                    ("plugin-list-name", index),
                    plugin.name.clone(),
                    name_at,
                );
                cell(
                    ui,
                    ("plugin-list-vendor", index),
                    plugin.vendor.clone().unwrap_or_default(),
                    vendor_at,
                );
                cell(
                    ui,
                    ("plugin-list-version", index),
                    plugin.version.clone().unwrap_or_default(),
                    version_at,
                );
                cell(
                    ui,
                    ("plugin-list-presets", index),
                    plugin.preset_names.join(", "),
                    presets_at,
                );
                let names: Vec<String> = plugin
                    .tracks
                    .iter()
                    .filter_map(|track| {
                        shell
                            .editor()
                            .document()
                            .track(*track)
                            .map(|track| track.name().to_owned())
                    })
                    .collect();
                let tracks = Id::new(("plugin-list-tracks", index));
                let _ = ui.interact(tracks_at, tracks, Sense::hover());
                ui.ctx().accesskit_node_builder(tracks, |node| {
                    node.set_role(Role::Cell);
                    node.set_label(names.join(", "));
                    node.set_bounds(bounds(tracks_at));
                });
                ui.scope_builder(
                    egui::UiBuilder::new()
                        .max_rect(rect)
                        .accessibility_parent(tracks),
                    |ui| {
                        self.tracks(ui, rows, rect, index, plugin);
                    },
                );
                cell(
                    ui,
                    ("plugin-list-missing", index),
                    marker.to_owned(),
                    missing_at,
                );
            },
        );
    }

    /// Draws the tracks a plug-in is on, each a button that selects the
    /// track and the first of the plug-in's devices on it (R6).
    fn tracks(
        &mut self,
        ui: &mut Ui,
        rows: &mut Rows<'_>,
        rect: Rect,
        index: usize,
        plugin: &bl_shell::plugin_commands::SetPlugin,
    ) {
        let (shell, theme, events) = (rows.shell, rows.theme, &mut *rows.events);
        let c = &theme.colors;
        let painter = ui.painter_at(rect);
        let mut left = rect.left() + TRACKS + 4.0;
        for (at, (track, device)) in plugin.tracks.iter().zip(&plugin.devices).enumerate() {
            let Some(track) = shell.editor().document().track(*track) else {
                continue;
            };
            let name = track.name().to_owned();
            let galley = painter.layout_no_wrap(name.clone(), theme.small_font(), c.text.0);
            let width = galley.size().x + 8.0;
            let button = Rect::from_min_size(pos2(left, rect.center().y - 7.0), vec2(width, 14.0));
            crate::focus::register(ui.ctx(), track_id(index, at), bl_shell::Area::Detail, None);
            let response = ui.interact(button, track_id(index, at), Sense::click());
            if response.hovered() {
                painter.rect_filled(button, 2.0, c.control_hover.0);
            }
            painter.galley(
                pos2(
                    button.left() + 4.0,
                    button.center().y - galley.size().y / 2.0,
                ),
                galley,
                c.text.0,
            );
            if response.clicked()
                || (response.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
            {
                invoke(
                    events,
                    bl_shell::device_commands::SELECT,
                    json!({ "device": device }),
                );
            }
            response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, &name));
            ui.ctx()
                .accesskit_node_builder(track_id(index, at), |node| {
                    node.set_role(Role::Button);
                    node.set_label(name.clone());
                });
            left = button.right() + 6.0;
        }
    }
}

/// A cell of a row, for assistive technology: what it says, and where it is.
fn cell(ui: &Ui, salt: (&str, usize), label: String, at: Rect) {
    let id = Id::new(salt);
    // The widget makes the cell a child of the row in the tree.
    let _ = ui.interact(at, id, Sense::hover());
    ui.ctx().accesskit_node_builder(id, |node| {
        node.set_role(Role::Cell);
        node.set_label(label);
        node.set_bounds(bounds(at));
    });
}

/// The count line's text: how many plug-ins the set uses, and how many are
/// missing.
fn count_text(plugins: usize, missing: usize) -> String {
    format!(
        "{} plug-in{}, {} missing",
        plugins,
        if plugins == 1 { "" } else { "s" },
        missing
    )
}

/// How far the rows are scrolled, after the wheel over the rows.
fn scrolled(ui: &Ui, area: Rect, offset: f32, content: f32) -> f32 {
    let over = ui
        .input(|i| i.pointer.hover_pos())
        .is_some_and(|p| area.contains(p));
    let delta = if over {
        ui.input(|i| i.smooth_scroll_delta().y)
    } else {
        0.0
    };
    (offset - delta).clamp(0.0, (content - area.height()).max(0.0))
}

/// The bounds of a rectangle, for assistive technology.
fn bounds(rect: Rect) -> egui::accesskit::Rect {
    egui::accesskit::Rect {
        x0: rect.left().into(),
        y0: rect.top().into(),
        x1: rect.right().into(),
        y1: rect.bottom().into(),
    }
}

/// Runs a command.
fn invoke(events: &mut Vec<UiEvent>, id: &str, args: serde_json::Value) {
    events.push(UiEvent::Invoke(bl_shell::Invocation::new(id, args)));
}

/// The commands the list runs.
mod ids {
    pub use bl_shell::commands::{VIEW_SET_PLUGIN_LIST_ONLY_MISSING, VIEW_TOGGLE_PLUGIN_LIST};
    pub use bl_shell::plugin_commands::EXPORT_LIST;
}

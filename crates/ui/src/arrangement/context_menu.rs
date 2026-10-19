//! The Arrangement's context menus (WP-1.8b; menus.md C-CLIP-ARR and
//! C-TIME): right-clicking a clip opens the clip's menu, selecting the clip
//! first if it wasn't; right-clicking anywhere else in the lanes opens the
//! time selection's menu, placing the insert marker there if the click is
//! outside the time selection. Each item runs what its shortcut runs in the
//! Arrangement (`bl_shell::context_menus`), or shows disabled until a
//! package binds it; the grid rows offer each setting as a choice.

use bl_shell::Shell;
use bl_shell::arrangement_commands::{self as view_ids, AdaptiveGrid, FixedGrid};
use bl_shell::context_menus::{self, ADAPTIVE_GRID_ROWS, FIXED_GRID_ROWS, SelectionMenu};
use bl_shell::editing_commands as ids;
use bl_shell::menus::shown;
use egui::{Pos2, Ui};
use serde_json::json;

use super::editing::Lanes;
use super::invoke;
use crate::keymap::{Keymap, Platform};
use crate::main_window::UiEvent;

/// Which menu a right-click opened.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MenuKind {
    /// A clip's (C-CLIP-ARR).
    Clip,
    /// The time selection's (C-TIME).
    #[default]
    Time,
}

/// The two menus and which one is open.
pub(super) struct ContextMenus {
    keymap: Keymap,
    platform: Platform,
    kind: MenuKind,
}

impl ContextMenus {
    pub fn new() -> Self {
        Self {
            keymap: Keymap::live12(),
            platform: Platform::current(),
            kind: MenuKind::default(),
        }
    }

    /// A right-click at `pos`: picks the menu and selects what it acts on.
    pub fn open(&mut self, lanes: &Lanes<'_>, pos: Pos2, events: &mut Vec<UiEvent>) {
        if let Some(hit) = lanes.hit(pos) {
            self.kind = MenuKind::Clip;
            if !lanes.view.is_clip_selected(hit.clip) {
                invoke(events, ids::SELECT_CLIPS, json!({ "clips": [hit.clip] }));
            }
            return;
        }
        self.kind = MenuKind::Time;
        let view = lanes.view;
        let at = view.snap(lanes.doc, lanes.at(pos));
        let track = lanes.track_at(pos);
        let inside = view.time_selection().is_some_and(|(start, end)| {
            at >= start && at <= end && track.is_none_or(|t| view.selected_tracks().contains(&t))
        });
        if !inside {
            let mut args = json!({ "at": at });
            if let Some(track) = track {
                args["track"] = json!(track);
            }
            invoke(events, view_ids::SET_INSERT_MARKER, args);
        }
    }

    /// The keys opened a menu (S-CTX-002): the clips' if any are selected,
    /// otherwise the time selection's.
    pub fn open_from_keys(&mut self, clips_selected: bool) {
        self.kind = if clips_selected {
            MenuKind::Clip
        } else {
            MenuKind::Time
        };
    }

    /// Draws the open menu's items. A selection that holds clips brings the
    /// rows menus.md records for those states (C-CLIP-ARR-025, C-CLIP-ARR-026,
    /// C-TIME-019, C-TIME-020).
    pub fn show(&self, ui: &mut Ui, shell: &Shell, events: &mut Vec<UiEvent>) {
        let state = selection_menu(shell);
        let menu = match self.kind {
            MenuKind::Clip if audio_selected(shell) => {
                context_menus::arrangement_audio_clip_in(state)
            }
            MenuKind::Clip => context_menus::arrangement_clip_in(state),
            MenuKind::Time => context_menus::arrangement_time_in(state),
        };
        let menu = &menu;
        let grid = shell.arrangement().grid();
        for item in shown(menu) {
            if item.is_separator() {
                ui.separator();
            } else if ADAPTIVE_GRID_ROWS.contains(&item.row) {
                // Five choices in three columns (C-CLIP-ARR-020).
                choices(ui, item.row, 3, AdaptiveGrid::ALL.len(), |ui, i| {
                    let choice = AdaptiveGrid::ALL[i];
                    let on = !grid.fixed_on && grid.adaptive == choice;
                    let response = ui.selectable_label(on, choice.label());
                    crate::menus::register_item(ui.ctx(), &response);
                    if response.clicked() {
                        invoke(events, view_ids::SET_GRID, json!({ "adaptive": choice }));
                        ui.close();
                    }
                });
            } else if FIXED_GRID_ROWS.contains(&item.row) {
                // Nine choices in five columns (C-CLIP-ARR-021).
                choices(ui, item.row, 5, FixedGrid::ALL.len(), |ui, i| {
                    let choice = FixedGrid::ALL[i];
                    let on = grid.fixed_on && grid.fixed == choice;
                    let response = ui.selectable_label(on, choice.label());
                    crate::menus::register_item(ui.ctx(), &response);
                    if response.clicked() {
                        invoke(events, view_ids::SET_GRID, json!({ "fixed": choice }));
                        ui.close();
                    }
                });
            } else {
                let response =
                    crate::menus::item(ui, item, shell, &self.keymap, self.platform, events);
                crate::menus::take_focus_first(ui, &response);
            }
        }
    }
}

/// What the Arrangement's selection adds to the open menu: the clip rows
/// when clips are selected, and the time rows when the time selection holds
/// a clip.
fn selection_menu(shell: &Shell) -> SelectionMenu {
    let doc = shell.editor().document();
    let view = shell.arrangement();
    let clips = if view.time_selection().is_some() {
        let (start, end) = view.time_selection().unwrap_or_default();
        doc.track_order()
            .into_iter()
            .filter_map(|track| doc.track(track))
            .any(|track| {
                track
                    .clips()
                    .any(|clip| clip.start < end && clip.end() > start)
            })
    } else {
        !view.selected_clips().is_empty()
    };
    SelectionMenu { clips }
}

/// Whether the selected clips are audio clips, whose menu adds the
/// Convert and Separate Stems rows.
fn audio_selected(shell: &Shell) -> bool {
    let doc = shell.editor().document();
    let selected = shell.arrangement().selected_clips();
    !selected.is_empty()
        && selected.iter().all(|clip| {
            doc.clip(*clip)
                .is_some_and(|c| matches!(c.data, bl_model::ClipData::Audio(_)))
        })
}

/// Lays out `count` choices in `columns` columns.
fn choices(
    ui: &mut Ui,
    row: &str,
    columns: usize,
    count: usize,
    mut choice: impl FnMut(&mut Ui, usize),
) {
    egui::Grid::new(("arrangement-grid-choices", row))
        .num_columns(columns)
        .show(ui, |ui| {
            for i in 0..count {
                choice(ui, i);
                if (i + 1) % columns == 0 {
                    ui.end_row();
                }
            }
        });
}

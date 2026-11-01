//! The Arrangement's context menus (WP-1.8b; menus.md C-CLIP-ARR and
//! C-TIME): right-clicking a clip opens the clip's menu, selecting the clip
//! first if it wasn't; right-clicking anywhere else in the lanes opens the
//! time selection's menu, placing the insert marker there if the click is
//! outside the time selection. Each item runs what its shortcut runs in the
//! Arrangement (`bl_shell::context_menus`), or shows disabled until a
//! package binds it; the grid rows offer each setting as a choice.

use bl_model::Shape;
use bl_shell::Shell;
use bl_shell::arrangement_commands::{self as view_ids, AdaptiveGrid, FixedGrid};
use bl_shell::context_menus::{
    self, ADAPTIVE_GRID_ROWS, CLIP_COLOUR_ROW, FIXED_GRID_ROWS, SHAPE_ROWS, SelectionMenu,
};
use bl_shell::editing_commands as ids;
use bl_shell::menus::{Action, shown};
use egui::{Pos2, Sense, Stroke, Ui, pos2, vec2};
use serde_json::json;

use super::editing::Lanes;
use super::invoke;
use crate::keymap::{Keymap, Platform};
use crate::main_window::UiEvent;
use crate::theme::Theme;

/// Which menu a right-click opened.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MenuKind {
    /// A clip's (C-CLIP-ARR).
    Clip,
    /// The time selection's (C-TIME).
    #[default]
    Time,
    /// The scrub area's (C-TIME-022).
    Scrub,
    /// A locator's (C-TIME-023, C-TIME-025).
    Locator,
    /// The loop brace's (C-TIME-023).
    Brace,
    /// A short bar's (C-TIME-024).
    FragmentaryBar,
    /// A time-signature marker's (C-TIME-027).
    Signature,
    /// An automation breakpoint's: Edit Value (C-TIME-026).
    Breakpoint,
    /// An envelope's line: Add Value (C-TIME-026).
    Line,
}

/// The two menus and which one is open.
pub(super) struct ContextMenus {
    platform: Platform,
    kind: MenuKind,
    /// Where the scrub area was right-clicked, so its Add Locator adds one
    /// there (the time editing spec, R3).
    scrub_at: Option<bl_core::time::Ticks>,
    /// The automation lane's breakpoint, or the place on its line, that was
    /// right-clicked: the track, the parameter and the position.
    envelope: Option<(
        bl_model::TrackId,
        bl_model::AutomationTarget,
        bl_core::time::Ticks,
    )>,
}

impl ContextMenus {
    pub fn new() -> Self {
        Self {
            platform: Platform::current(),
            kind: MenuKind::default(),
            scrub_at: None,
            envelope: None,
        }
    }

    /// A right-click on a breakpoint of an automation lane (`line` false),
    /// or on its envelope's line (`line` true): Edit Value or Add Value.
    pub fn open_envelope(
        &mut self,
        track: bl_model::TrackId,
        target: bl_model::AutomationTarget,
        at: bl_core::time::Ticks,
        line: bool,
    ) {
        self.kind = if line {
            MenuKind::Line
        } else {
            MenuKind::Breakpoint
        };
        self.envelope = Some((track, target, at));
    }

    /// Edit Value and Add Value (the automation editing spec, R27): each
    /// types a value, which Return sets, sharing the lane's typed-value
    /// entry.
    fn show_value(&self, ui: &mut Ui, events: &mut Vec<UiEvent>) {
        use bl_shell::envelope_commands::SELECT_BREAKPOINT;
        let Some((track, target, at)) = self.envelope else {
            return;
        };
        let line = self.kind == MenuKind::Line;
        let response = ui.button(if line { "Add Value" } else { "Edit Value" });
        crate::menus::register_item(ui.ctx(), &response);
        crate::menus::take_focus_first(ui, &response);
        if !response.clicked() {
            return;
        }
        let typed = if line {
            // The lane has the keyboard, with the insert marker where the
            // value goes.
            invoke(
                events,
                view_ids::SET_INSERT_MARKER,
                json!({ "at": at, "track": track, "automation_lane": true }),
            );
            crate::envelope_keys::Typed {
                text: String::new(),
                add_at: Some(at),
                release: None,
            }
        } else {
            invoke(
                events,
                SELECT_BREAKPOINT,
                json!({ "track": track, "target": target, "at": at }),
            );
            crate::envelope_keys::Typed {
                text: String::new(),
                add_at: None,
                release: Some((
                    SELECT_BREAKPOINT.to_owned(),
                    json!({ "track": track, "target": target, "at": null }),
                )),
            }
        };
        crate::envelope_keys::set_typed(ui.ctx(), Some(typed));
        ui.close();
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
        let place = lanes
            .place_at(pos)
            .map(|(row, lane)| (lanes.rows[row].track, lane));
        let track = place.map(|(track, _)| track);
        let inside = view.time_selection().is_some_and(|(start, end)| {
            at >= start
                && at <= end
                && track.is_none_or(|t| view.selected_tracks().contains(&t))
                // Spec: comping R29. On a take lane, inside the lanes the
                // selection covers.
                && place.is_none_or(|(_, lane)| {
                    lane.is_none_or(|lane| view.selection_take_lanes().contains(&lane))
                })
        });
        if !inside {
            let mut args = json!({ "at": at });
            if let Some(track) = track {
                args["track"] = json!(track);
            }
            if let Some((_, Some(lane))) = place {
                args["lane"] = json!(lane);
            }
            invoke(events, view_ids::SET_INSERT_MARKER, args);
        }
    }

    /// A right-click in the scrub area: the locator's menu on a locator,
    /// and the scrub area's own menu elsewhere.
    pub fn open_scrub(&mut self, locator: Option<bl_model::LocatorId>, at: bl_core::time::Ticks) {
        self.kind = if locator.is_some() {
            MenuKind::Locator
        } else {
            MenuKind::Scrub
        };
        self.scrub_at = Some(at);
    }

    /// A right-click on the loop brace.
    pub fn open_brace(&mut self) {
        self.kind = MenuKind::Brace;
    }

    /// A right-click on the short bar that starts at `start` in the
    /// time-signature lane.
    pub fn open_fragmentary_bar(&mut self, start: bl_core::time::Ticks) {
        self.kind = MenuKind::FragmentaryBar;
        self.scrub_at = Some(start);
    }

    /// A right-click on a time-signature marker, which is selected.
    pub fn open_signature(&mut self) {
        self.kind = MenuKind::Signature;
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
    pub fn show(&self, ui: &mut Ui, theme: &Theme, shell: &Shell, events: &mut Vec<UiEvent>) {
        if matches!(self.kind, MenuKind::Breakpoint | MenuKind::Line) {
            self.show_value(ui, events);
            return;
        }
        // The keymap in use, so the menu shows the keys in use (R10).
        let keymap = Keymap::from_data(shell.keymap().clone());
        let state = selection_menu(shell);
        let menu = match self.kind {
            MenuKind::Clip if audio_selected(shell) => {
                context_menus::arrangement_audio_clip_in(state)
            }
            MenuKind::Clip => context_menus::arrangement_clip_in(state),
            MenuKind::Time => context_menus::arrangement_time_in(state),
            MenuKind::Scrub => context_menus::arrangement_scrub_in(),
            MenuKind::Locator => context_menus::arrangement_locator_in(),
            MenuKind::Brace => context_menus::arrangement_brace_in(),
            MenuKind::FragmentaryBar => context_menus::arrangement_fragmentary_bar_in(),
            MenuKind::Signature => context_menus::arrangement_signature_in(),
            MenuKind::Breakpoint | MenuKind::Line => return,
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
            } else if item.row == CLIP_COLOUR_ROW {
                // Spec: themes R16. The palette colours the selected clips.
                let doc = shell.editor().document();
                let selected = shell.arrangement().selected_clips();
                let current = selected
                    .first()
                    .and_then(|clip| doc.clip(*clip))
                    .and_then(|clip| clip.colour);
                if let Some(colour) = crate::palette::show(ui, theme, "clip-colour", current) {
                    invoke(
                        events,
                        "clip.set_colour",
                        json!({ "clips": selected, "colour": colour }),
                    );
                    ui.close();
                }
            } else if SHAPE_ROWS.contains(&item.row) {
                // Spec: automation-editing R11. Five buttons a row.
                let shapes = if item.row == SHAPE_ROWS[0] {
                    &Shape::ALL[..5]
                } else {
                    &Shape::ALL[5..]
                };
                ui.horizontal(|ui| {
                    for shape in shapes {
                        shape_button(
                            ui,
                            shell,
                            *shape,
                            bl_shell::automation_commands::INSERT_SHAPE,
                            &shell.active_contexts(),
                            events,
                        );
                    }
                });
            } else {
                // The scrub area's Add Locator adds one where the area was
                // right-clicked (the time editing spec, R3).
                let args = match (self.kind, item.action) {
                    (MenuKind::Scrub, Action::Command(id))
                        if id == bl_shell::locator_commands::ADD =>
                    {
                        self.scrub_at.map(|at| json!({ "at": at }))
                    }
                    // Spec: time-editing R34. Both items act on the short bar
                    // that was right-clicked.
                    (MenuKind::FragmentaryBar, Action::Command(_)) => {
                        self.scrub_at.map(|at| json!({ "at": at }))
                    }
                    // Spec: session-context-menus R16. Export MIDI Clip...
                    // writes the first selected clip.
                    (MenuKind::Clip, Action::Command(id))
                        if id == bl_shell::midi_file_commands::EXPORT =>
                    {
                        shell
                            .arrangement()
                            .selected_clips()
                            .first()
                            .map(|clip| json!({ "clip": clip }))
                    }
                    // Spec: stem-separation R22. The time selection's audio
                    // tracks and range.
                    (MenuKind::Time, Action::Command(id))
                        if id == bl_shell::stems_commands::OPEN =>
                    {
                        let view = shell.arrangement();
                        view.time_selection().map(|(start, end)| {
                            json!({ "source": {
                                "kind": "time_selection",
                                "tracks": view.selected_tracks(),
                                "start": start,
                                "end": end,
                            } })
                        })
                    }
                    _ => None,
                };
                let response = match args {
                    Some(args) => crate::menus::item_with_args(
                        ui,
                        item,
                        shell,
                        &keymap,
                        self.platform,
                        events,
                        args,
                    ),
                    None => crate::menus::item(ui, item, shell, &keymap, self.platform, events),
                };
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
    // Spec: automation-editing R11, R18. A time selection in an automation
    // lane brings the shapes and Simplify Envelope.
    let automation = view.time_selection().is_some()
        && view
            .time_selection_lanes()
            .iter()
            .any(|lane| lane.target().is_some());
    // Spec: stem-separation R22. A time selection that holds audio on audio
    // tracks brings Separate Stems to New Audio Tracks.
    let audio = view.time_selection().is_some_and(|(start, end)| {
        doc.track_order()
            .into_iter()
            .filter_map(|track| doc.track(track))
            .filter(|track| track.kind() == bl_model::TrackKind::Audio)
            .any(|track| {
                track
                    .clips()
                    .any(|clip| clip.start < end && clip.end() > start)
            })
    });
    SelectionMenu {
        clips,
        automation,
        take_lane: view.take_lane_selected(),
        time_commands: true,
        audio,
    }
}

/// One of the shape buttons (the automation editing spec, R11): a drawing
/// of the shape that assistive technology reads by its name. It runs `id`,
/// the lane's or the Envelope Editor's insert-shape command, when that can
/// run in `contexts`.
pub(crate) fn shape_button(
    ui: &mut Ui,
    shell: &Shell,
    shape: Shape,
    id: &'static str,
    contexts: &[bl_commands::Context],
    events: &mut Vec<UiEvent>,
) {
    let args = json!({ "shape": shape });
    let enabled = shell.can_invoke_in(id, &args, contexts);
    let (rect, response) = ui.allocate_exact_size(vec2(34.0, 24.0), Sense::click());
    let response = if enabled {
        response
    } else {
        response.on_disabled_hover_text(shape.label())
    };
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, shape.label())
    });
    crate::menus::register_item(ui.ctx(), &response);
    let visuals = ui.style().interact(&response);
    ui.painter().rect(
        rect,
        3.0,
        visuals.bg_fill,
        visuals.bg_stroke,
        egui::StrokeKind::Inside,
    );
    // The shape drawn between a quarter and three quarters of the way up the
    // button, for the ramps starting from the value before and ending on the
    // value after.
    let inner = rect.shrink2(vec2(6.0, 5.0));
    let line: Vec<Pos2> = (0..=24)
        .map(|i| {
            let along = f64::from(i) / 24.0;
            let level = shape.level(along, 0.25, 0.75) as f32;
            pos2(
                inner.left() + inner.width() * along as f32,
                inner.bottom() - inner.height() * level,
            )
        })
        .collect();
    let colour = if enabled {
        visuals.fg_stroke.color
    } else {
        ui.visuals().weak_text_color()
    };
    ui.painter()
        .add(egui::Shape::line(line, Stroke::new(1.5, colour)));
    if enabled && response.clicked() {
        events.push(UiEvent::Invoke(bl_shell::Invocation::new(id, args)));
        ui.close();
    }
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

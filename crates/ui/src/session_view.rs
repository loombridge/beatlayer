//! The Session View grid (WP-3.7): the clip slots, the scenes' rows in the
//! Main track's column, and what a person does to them — selecting, arrow
//! navigation, dragging a clip between slots, dropping files into one, and
//! double-clicking an empty MIDI slot to make a clip there.
//!
//! Placement comes from the layout layer ([`crate::layout::mixer`]). Every
//! gesture becomes one of the Session View's registered commands
//! (`session.*`), which the host runs, so a scripted agent drives the grid
//! the same way the person does. Only the rows on screen are built (the
//! Arrangement's virtualised pattern). What a slot holds is `bl-model`'s.
//!
//! The grid launches (WP-3.8): a slot's launch triangle and a scene's launch
//! button run `session.launch` and `session.launch_scene`, a stop square and
//! the status row's stop buttons run `session.stop_track` and
//! `session.stop_all`, and the Back to Arrangement button runs
//! `session.back_to_arrangement`. What they draw comes from the states the
//! engine published, `Shell::session_states` and `Shell::session_over`.

use bl_core::time::TICKS_PER_QUARTER;
use bl_engine::SlotState;
use bl_model::{ClipId, Document, RemoteTarget, SceneId, TrackId, TrackKind};
use bl_shell::clip_drag::{ClipDragEvent, DragSource, DropSpot};
use bl_shell::control_surface_commands::RingBox;
use bl_shell::mixer_view::MixerOf;
use bl_shell::mixer_view_commands as mixer_ids;
use bl_shell::session_view_commands as ids;
use bl_shell::session_view_commands::SessionViewState;
use bl_shell::{Area, InfoTarget, Invocation, Shell};
use egui::accesskit::{self, Role};
use egui::{
    Align2, Color32, Id, Key, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Ui, WidgetInfo,
    WidgetType, pos2, vec2,
};
use serde_json::{Value, json};

use crate::layout::mixer::{
    Column, ColumnKind, MainStatusCells, MixerLayout, SLOT_ROW, SceneRow, SessionLayout,
    StatusCells, TrackColumn,
};
use crate::main_window::UiEvent;
use crate::session_menus::{Rename, SessionMenus};
use crate::theme::Theme;

/// The ID of the grid's group, which takes the keyboard when a slot or a
/// scene row is clicked.
#[must_use]
pub fn grid_id() -> Id {
    Id::new("session-grid")
}

/// The ID of a slot's widget.
#[must_use]
pub fn slot_id(track: TrackId, scene: SceneId) -> Id {
    Id::new(("session-slot", track, scene))
}

/// The ID of a scene's row in the Main column.
#[must_use]
pub fn scene_row_id(scene: SceneId) -> Id {
    Id::new(("session-scene", scene))
}

/// The ID of a scene's launch button.
#[must_use]
pub fn scene_launch_id(scene: SceneId) -> Id {
    Id::new(("session-scene-launch", scene))
}

/// The ID of a slot's launch triangle, or its stop square.
#[must_use]
pub fn slot_button_id(track: TrackId, scene: SceneId) -> Id {
    Id::new(("session-slot-button", track, scene))
}

/// The ID of a track's stop button in the status row.
#[must_use]
pub fn track_stop_id(track: TrackId) -> Id {
    Id::new(("session-track-stop", track))
}

/// The ID of a track's status field in the status row.
fn status_field_id(track: TrackId) -> Id {
    Id::new(("session-track-status", track))
}

/// The ID of Stop All Clips, at the Main column's left in the status row.
#[must_use]
pub fn stop_all_id() -> Id {
    Id::new("session-stop-all")
}

/// The ID of the Back to Arrangement button, at the Main column's right in
/// the status row.
#[must_use]
pub fn back_to_arrangement_id() -> Id {
    Id::new("session-back-to-arrangement")
}

/// A drag under way in the grid.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Drag {
    /// Dragging the clip in this slot: moved to the slot it's released on,
    /// or copied with the command modifier held.
    Clip {
        clip: ClipId,
        from: (TrackId, SceneId),
    },
}

/// The grid's state between frames.
#[derive(Debug, Default)]
pub struct GridUi {
    drag: Option<Drag>,
    /// A clip drag that ended this frame, moved once its release point is
    /// known (handled in [`GridUi::show`], which has the layout).
    released: Option<(ClipId, (TrackId, SceneId))>,
    /// The slots the last frame built, out of how many there are (the
    /// Arrangement's virtualised pattern).
    built: usize,
    total: usize,
    /// Whether the grid held the keyboard when the last frame ended.
    focused: bool,
    /// The widgets the grid drew that can hold its keyboard: the group, the
    /// slots, and the scenes' rows.
    focus_ids: Vec<Id>,
    /// The slot under the pointer in the last frame, which clips dragged
    /// from the Arrangement are dropped on (second-window R21).
    hover: Option<(TrackId, SceneId)>,
    /// The slot and scene menus (WP-6.56).
    menus: SessionMenus,
    /// How many times each surface's ring had been moved when the grid last
    /// looked, so a move brings the ring into view once (R37).
    rings_seen: Vec<u64>,
}

impl GridUi {
    /// A grid with nothing under way.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The slots the last frame built, out of how many; `None` before the
    /// first frame.
    #[must_use]
    pub fn built_slots(&self) -> (usize, usize) {
        (self.built, self.total)
    }

    /// Whether the grid held the keyboard when the last frame ended.
    #[must_use]
    pub fn focused(&self) -> bool {
        self.focused
    }

    /// The slot the pointer was over in the last frame, for clips dragged
    /// from the Arrangement to drop on.
    #[must_use]
    pub fn drop_spot(&self) -> Option<DropSpot> {
        let (track, scene) = self.hover?;
        Some(DropSpot::Session { track, scene })
    }

    /// Draws the grid in the Session View's slots area and adds what the
    /// person did to `events`.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        shell: &Shell,
        theme: &Theme,
        session: &SessionLayout,
        events: &mut Vec<UiEvent>,
    ) {
        let doc = shell.editor().document();
        let columns = shell.mixer_columns();
        let state = shell.session().state();
        let scroll_y = state.scroll_y as f32;
        let folds = crate::mixer::folds(shell, &columns);
        let layout = crate::mixer::mixer_layout(shell, session.slots, MixerOf::Session, &folds);
        let scenes: Vec<(SceneId, String)> = doc
            .scenes()
            .map(|scene| (scene.id, scene.name.clone()))
            .collect();

        let mut grid = ui.new_child(egui::UiBuilder::new().max_rect(session.slots));
        grid.set_clip_rect(session.slots);
        self.focus_ids.clear();
        self.focus_ids.push(grid_id());
        {
            let painter = grid.painter().clone();
            let response = grid.interact(session.slots, grid_id(), Sense::click());
            response.widget_info(|| WidgetInfo::labeled(WidgetType::Other, true, "Session View"));
            grid.ctx().accesskit_node_builder(grid_id(), |node| {
                node.set_role(Role::Group);
                node.set_label("Session View".to_owned());
                node.set_bounds(bounds(session.slots));
            });
            // The drop area between the last track and the returns
            // (L-SES-004).
            if let Some(drop) = layout.drop_area {
                let drop = drop.intersect(session.slots);
                if drop.width() > 40.0 {
                    painter.text(
                        pos2(drop.center().x, session.slots.top() + 9.0),
                        Align2::CENTER_CENTER,
                        "Drop Files and Devices Here",
                        theme.small_font(),
                        theme.colors.text_dim.0,
                    );
                }
            }
        }

        let rings = shell.session_rings();
        // Only the rows on screen are built; one extra above and below keeps
        // a partly scrolled row whole.
        let first = session.first_scene(scroll_y).saturating_sub(1);
        let rows = (session.slots.height() / SLOT_ROW).ceil() as usize + 2;
        self.built = 0;
        self.total = scenes.len() * columns.len();
        // The scene each track's clip slot stands for in the strip's Tab
        // order: the selected one, else the first (observed in Live 12.4.6).
        let first_scene = scenes.first().map(|(id, _)| *id);
        let tab_scene = |track: TrackId| {
            state
                .slots
                .iter()
                .find(|slot| slot.track == track)
                .map(|slot| slot.scene)
                .or(first_scene)
        };
        for (index, (scene, scene_name)) in scenes.iter().enumerate().skip(first).take(rows) {
            let name = if scene_name.is_empty() {
                (index + 1).to_string()
            } else {
                scene_name.clone()
            };
            let row = session.scene_row(index, scroll_y);
            if row.bottom() < session.slots.top() || row.top() > session.slots.bottom() {
                continue;
            }
            let scene_row = SceneRow::compute(row, layout.main.rect);
            self.scene_row(
                &mut grid, shell, theme, &scene_row, &name, index, &state, events,
            );
            for column in &layout.columns {
                // A chain's column has no clip slots (racks spec, R27).
                if matches!(column.kind, ColumnKind::Chain(_)) {
                    continue;
                }
                let track = columns[column.index];
                let cell = column.rect.intersect(row).shrink(1.0);
                if cell.width() <= 0.0 || cell.height() <= 0.0 {
                    continue;
                }
                let selected = state
                    .slots
                    .iter()
                    .any(|slot| slot.track == track && slot.scene == *scene);
                let tab = tab_scene(track) == Some(*scene);
                self.slot(
                    &mut grid, shell, theme, track, *scene, &name, cell, selected, tab, events,
                );
                self.built += 1;
                // Spec: session-view R38. A slot inside a ring says whose.
                let held: Vec<&str> = rings
                    .iter()
                    .filter(|r| r.tracks.contains(&track) && r.scenes.contains(scene))
                    .map(|r| r.surface.as_str())
                    .collect();
                if !held.is_empty() {
                    let note = held
                        .iter()
                        .map(|surface| format!("in the ring of {surface}"))
                        .collect::<Vec<_>>()
                        .join(", ");
                    grid.ctx()
                        .accesskit_node_builder(slot_id(track, *scene), |node| {
                            let text = match node.description() {
                                Some(text) => format!("{text}; {note}"),
                                None => note,
                            };
                            node.set_description(text);
                        });
                }
            }
        }

        // Spec: session-view R36, R37, R38. Each running surface's ring is
        // drawn over the slots, and a ring a surface moved is brought into
        // view.
        self.show_rings(
            &grid,
            shell,
            theme,
            (session, &layout, &folds.columns),
            (&columns, scroll_y),
            &rings,
            events,
        );
        self.status_row(ui, shell, theme, session, &layout, &columns, events);
        self.keys(&grid, shell, &columns, &scenes, events);
        self.take_drop(&grid, shell, session, &layout, &columns, &scenes, events);
        // Spec: view-options R1, session-view R35. A middle-button drag
        // pans the grid: sideways through the columns and up and down
        // through the scenes.
        if let Some(delta) = crate::middle::pan(ui, session.slots) {
            events.push(UiEvent::Invoke(Invocation::new(
                mixer_ids::SCROLL,
                json!({ "view": MixerOf::Session, "x": -delta.x }),
            )));
            events.push(UiEvent::Invoke(Invocation::new(
                ids::SCROLL,
                json!({ "y": -delta.y }),
            )));
        }
        self.hover = ui
            .input(|i| i.pointer.latest_pos())
            .and_then(|pos| slot_at(session, &layout, &columns, &scenes, scroll_y, pos));
        // A clip drag that ended: move it to the slot it was released on, or
        // copy it with the copy modifier held (I-DRAG-001, I-DRAG-002). Esc
        // ended the drag before it, which leaves everything as it was
        // (second-window R18).
        if let Some((clip, from)) = self.released.take()
            && shell.clip_drag().is_some()
        {
            let (pos, modifiers) = ui.input(|i| (i.pointer.interact_pos(), i.modifiers));
            // Released over a folder or label of the browser, the clip is
            // saved as a clip file there instead (clip files R3).
            let target = crate::browser::end_clip_drag(ui.ctx());
            if let Some(target) = target {
                // Spec: set-exchange R12 (I-DROP-011). Several selected
                // clips, the one dragged among them, are saved as a set.
                let selected: Vec<bl_model::ClipId> = state
                    .slots
                    .iter()
                    .filter_map(|slot| shell.editor().document().slot(slot.track, slot.scene))
                    .map(|clip| clip.id)
                    .collect();
                if selected.len() > 1 && selected.contains(&clip) {
                    crate::browser::save_clips_as_set(ui.ctx(), shell, selected, &target, events);
                } else {
                    crate::browser::save_clip(ui.ctx(), shell, clip, &target, events);
                }
            } else if let Some(pos) = pos
                && let Some((to_track, to_scene)) =
                    slot_at(session, &layout, &columns, &scenes, scroll_y, pos)
                && (to_track, to_scene) != from
            {
                let command = if copy_modifier(modifiers) {
                    ids::COPY_CLIP_TO_SLOT
                } else {
                    "clip.move_to_slot"
                };
                events.push(UiEvent::Invoke(Invocation::new(
                    command,
                    json!({ "clip": clip, "track": to_track, "scene": to_scene }),
                )));
            }
        }
    }

    /// Draws each running surface's ring as a border over the grid, in its
    /// row's colour, and brings a ring a surface has just moved into view
    /// (session view R36, R37).
    #[expect(clippy::too_many_arguments, reason = "the rings' inputs")]
    fn show_rings(
        &mut self,
        grid: &Ui,
        shell: &Shell,
        theme: &Theme,
        (session, layout, tracks): (&SessionLayout, &MixerLayout, &[TrackColumn]),
        (columns, scroll_y): (&[TrackId], f32),
        rings: &[RingBox],
        events: &mut Vec<UiEvent>,
    ) {
        let doc = shell.editor().document();
        let returns: Vec<TrackId> = doc.returns().collect();
        // Where a ring's track sits across the grid, in view or not.
        let span = |track: TrackId| -> Option<(f32, f32)> {
            if let Some(index) = columns.iter().position(|t| *t == track) {
                let (left, right) = layout.track_span(tracks, index)?;
                let origin = layout.tracks.left() - layout.scroll;
                return Some((origin + left, origin + right));
            }
            let index = returns.iter().position(|t| *t == track)?;
            let column = layout.returns.iter().find(|c| c.index == index)?;
            Some((column.rect.left(), column.rect.right()))
        };
        let moves = shell.ring_moves();
        self.rings_seen.resize(moves.len(), 0);
        for ring in rings {
            let row = usize::from(ring.row).saturating_sub(1);
            let moved = moves.get(row).is_some_and(|m| self.rings_seen[row] != *m);
            if let Some(count) = moves.get(row) {
                self.rings_seen[row] = *count;
            }
            if moved && self.drag.is_none() {
                // The least it takes to show the first track and scene.
                if let Some(index) = ring
                    .tracks
                    .first()
                    .and_then(|t| columns.iter().position(|c| c == t))
                {
                    crate::mixer::scroll_into_view(layout, tracks, index, MixerOf::Session, events);
                }
                if let Some(scene) = ring.scenes.first().and_then(|s| doc.scene_position(*s)) {
                    let top = SLOT_ROW * scene as f32;
                    let view = session.slots.height();
                    let by = if top < scroll_y {
                        top - scroll_y
                    } else if top + SLOT_ROW > scroll_y + view {
                        top + SLOT_ROW - scroll_y - view
                    } else {
                        0.0
                    };
                    if by != 0.0 {
                        events.push(UiEvent::Invoke(Invocation::new(
                            ids::SCROLL,
                            json!({ "y": by }),
                        )));
                    }
                }
            }
        }
        // The lower row's ring is drawn on top.
        let painter = grid.painter().with_clip_rect(session.slots);
        for ring in rings {
            let spans: Vec<(f32, f32)> = ring.tracks.iter().filter_map(|t| span(*t)).collect();
            let (Some(left), Some(right)) = (
                spans.iter().map(|s| s.0).reduce(f32::min),
                spans.iter().map(|s| s.1).reduce(f32::max),
            ) else {
                continue;
            };
            let colour = theme.ring_colour(usize::from(ring.row).saturating_sub(1));
            let stroke = Stroke::new(2.0, colour);
            let first = ring.scenes.first().and_then(|s| doc.scene_position(*s));
            let Some(first) = first else {
                // A ring with no scenes is a line under the title bars.
                let y = session.slots.top() + 1.0;
                painter.line_segment([pos2(left, y), pos2(right, y)], stroke);
                continue;
            };
            let top = session.scene_row(first, scroll_y).top();
            let bottom = session
                .scene_row(first + ring.scenes.len() - 1, scroll_y)
                .bottom();
            painter.rect_stroke(
                Rect::from_min_max(pos2(left, top), pos2(right, bottom)),
                0.0,
                stroke,
                StrokeKind::Inside,
            );
        }
    }

    /// Draws one track's slot for a scene, and what a person does to it.
    #[expect(clippy::too_many_arguments, reason = "one slot, drawn in full")]
    fn slot(
        &mut self,
        ui: &mut Ui,
        shell: &Shell,
        theme: &Theme,
        track: TrackId,
        scene: SceneId,
        scene_name: &str,
        cell: Rect,
        selected: bool,
        tab: bool,
        events: &mut Vec<UiEvent>,
    ) {
        let doc = shell.editor().document();
        let Some(t) = doc.track(track) else {
            return;
        };
        let clip = doc.slot(track, scene);
        let clip_id = clip.map(|clip| clip.id);
        let response = ui.interact(cell, slot_id(track, scene), Sense::click_and_drag());
        let track_name = t.name().to_owned();
        let label = if clip.is_some() {
            format!("{track_name}: {scene_name}")
        } else {
            format!("{track_name}: {scene_name} (empty)")
        };
        response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, &label));
        // Spec: session-context-menus R1, R2, R14. A right-click, or the keys
        // while the grid has the keyboard, opens the slot's or the clip's
        // menu.
        let first = self.focused
            && shell
                .session()
                .selected_slots()
                .first()
                .is_some_and(|slot| slot.track == track && slot.scene == scene);
        if t.kind().can_be_grouped() {
            self.menus.slot(
                ui,
                theme,
                &response,
                shell,
                (track, scene),
                (cell.center(), first),
                events,
            );
        }
        ui.ctx()
            .accesskit_node_builder(slot_id(track, scene), |node| {
                node.set_role(Role::ListItem);
                node.set_label(label);
                node.set_selected(selected);
                node.set_bounds(bounds(cell));
            });
        if let Some(clip) = clip_id {
            crate::info_text::describe(ui.ctx(), &response, shell, InfoTarget::Clip(clip));
        }
        // The track's clip slot in the strip's Tab order, after its title
        // bar: one per track — the selected scene's, else the first
        // (observed in Live 12.4.6; the accessibility spec, R4).
        if tab {
            crate::focus::register(
                ui.ctx(),
                response.id,
                Area::MainView,
                Some(crate::focus::TrackControl {
                    place: crate::mixer::Place::Mixer(MixerOf::Session),
                    track,
                    slot: crate::focus::Slot::ClipSlot,
                }),
            );
        }
        let painter = ui.painter_at(cell.expand(1.0));
        let fill = match clip {
            Some(held) => theme
                .swatch(held.colour.unwrap_or_default())
                .gamma_multiply(0.75),
            None => theme.colors.lane.0,
        };
        painter.rect_filled(cell, 0.0, fill);
        if selected {
            painter.rect_stroke(
                cell,
                0.0,
                Stroke::new(1.5, theme.colors.focus.0),
                StrokeKind::Inside,
            );
        }
        if response.has_focus() && crate::focus::outlined(&response.ctx) {
            painter.rect_stroke(
                cell,
                0.0,
                Stroke::new(1.5, theme.colors.focus.0),
                StrokeKind::Outside,
            );
        }
        // Spec: session-context-menus R7. Rename types the clip's name over
        // its slot.
        if let Some(held) = clip {
            crate::session_menus::rename_edit(
                ui,
                theme,
                cell,
                &Rename::Clip(held.id),
                &held.name,
                None,
                events,
            );
        }
        // The launch triangle on a slot with a clip, the stop square on an
        // empty one that has its stop button, or a record circle on an armed
        // track's empty slot (observed). The triangle and the square are
        // buttons (I-CLICK-001, I-CLICK-003); the circle waits for WP-3.11.
        let mark = Rect::from_min_size(cell.min + vec2(1.0, 1.0), vec2(9.0, cell.height() - 2.0));
        let launch_state = shell
            .session_states()
            .iter()
            .find(|status| status.track == track && status.scene == scene)
            .map(|status| status.state);
        if clip.is_some() {
            let time = ui.input(|i| i.time);
            triangle(&painter, mark, launch_colour(theme, launch_state, time));
            let press = ui.interact(mark, slot_button_id(track, scene), Sense::click());
            let label = format!("Launch {track_name}: {scene_name}");
            self.button(
                &press,
                &label,
                launch_state.is_some(),
                events,
                "session.launch",
                json!({ "track": track, "scene": scene }),
            );
        } else if t.mixer().arm {
            painter.circle_filled(mark.center(), 3.0, theme.colors.record.0);
        } else if t.has_stop_button(scene) {
            painter.rect_filled(mark.shrink(1.5), 0.0, theme.colors.separator.0);
            let press = ui.interact(mark, slot_button_id(track, scene), Sense::click());
            let label = format!("Stop {track_name}");
            self.button(
                &press,
                &label,
                false,
                events,
                "session.stop_track",
                json!({ "track": track }),
            );
        }
        // Spec: map-modes R3, R11. The map modes tint every slot.
        crate::mapping::remote_overlay(
            ui,
            theme,
            cell,
            slot_id(track, scene),
            RemoteTarget::Slot { track, scene },
            false,
            events,
        );

        let modifiers = ui.input(|i| i.modifiers);
        self.focus_ids.push(response.id);
        if response.clicked() || response.double_clicked() {
            response.request_focus();
            events.push(UiEvent::FocusArea(Area::MainView));
        }
        if response.clicked() {
            select(events, Some(track), Some(scene), click_mode(modifiers));
        }
        // I-CLICK-004: double-clicking an empty MIDI slot makes a one-bar
        // MIDI clip there.
        if response.double_clicked() && clip.is_none() && t.kind() == TrackKind::Midi {
            events.push(UiEvent::Invoke(Invocation::new(
                ids::INSERT_EMPTY_MIDI_CLIP,
                json!({ "track": track, "scene": scene }),
            )));
        }
        // Spec: detail-views R4 (I-CLICK-005): double-clicking a clip's slot
        // selects it, which the click already did, and shows the Clip View.
        if response.double_clicked() && clip.is_some() {
            show_clip_view(shell, events);
        }
        // I-DRAG-001, I-DRAG-002: dragging a clip moves it to another slot;
        // Option-drag (Ctrl-drag on Windows and Linux) copies it.
        if response.drag_started()
            && let Some(clip) = clip_id
        {
            self.drag = Some(Drag::Clip {
                clip,
                from: (track, scene),
            });
            // Spec: clip files R3. A clip can be dropped on the browser.
            crate::browser::begin_clip_drag(ui.ctx());
            // Spec: second-window R18, R19. The selected clips, the one
            // dragged among them, can be carried to the Arrangement.
            let clips: Vec<ClipId> = if selected {
                shell
                    .session()
                    .selected_slots()
                    .iter()
                    .filter_map(|slot| doc.slot(slot.track, slot.scene))
                    .map(|clip| clip.id)
                    .collect()
            } else {
                vec![clip]
            };
            events.push(UiEvent::ClipDrag(ClipDragEvent::Begin(DragSource::Slots(
                clips,
            ))));
        }
        if response.drag_stopped()
            && let Some(Drag::Clip { clip, from }) = self.drag.take()
        {
            self.released = Some((clip, from));
        }
        if response.hovered() && clip.is_some() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        }
    }

    /// A button that runs `command` when it's clicked, and gives the grid
    /// the keyboard as a click on a slot does, so Enter launches next. It
    /// reaches AccessKit as a button named `label`, pressed while `on`.
    fn button(
        &mut self,
        response: &egui::Response,
        label: &str,
        on: bool,
        events: &mut Vec<UiEvent>,
        command: &'static str,
        args: Value,
    ) {
        response.widget_info(|| WidgetInfo::selected(WidgetType::Button, true, on, label));
        self.focus_ids.push(response.id);
        if response.clicked() {
            response.request_focus();
            events.push(UiEvent::FocusArea(Area::MainView));
            events.push(UiEvent::Invoke(Invocation::new(command, args)));
        }
    }

    /// Draws the status row under the slots (L-SES-008): each track's stop
    /// button and status display, and in the Main column Stop All Clips, the
    /// global follow-action switch and the Back to Arrangement button.
    #[expect(clippy::too_many_arguments, reason = "the frame's inputs")]
    fn status_row(
        &mut self,
        ui: &mut Ui,
        shell: &Shell,
        theme: &Theme,
        session: &SessionLayout,
        layout: &MixerLayout,
        columns: &[TrackId],
        events: &mut Vec<UiEvent>,
    ) {
        let doc = shell.editor().document();
        let states = shell.session_states();
        // A column scrolled partly out of view is cut at the tracks' edges,
        // as its slots are.
        let tracks = Rect::from_x_y_ranges(layout.tracks.x_range(), session.status_row.y_range());
        let painter = ui.painter_at(tracks);
        let colours = &theme.colors;
        let stop_square = |rect: Rect, colour: Color32| {
            painter.rect_filled(rect.shrink(5.0), 0.0, colour);
        };
        for column in &layout.columns {
            // A chain's column has no status row of its own (racks spec,
            // R27).
            if matches!(column.kind, ColumnKind::Chain(_)) {
                continue;
            }
            let track = columns[column.index];
            let Some(t) = doc.track(track).filter(|t| t.kind().holds_clips()) else {
                continue;
            };
            let cells = StatusCells::track(column.rect, session.status_row);
            let stop = cells.stop.intersect(tracks);
            if stop.width() <= 0.0 {
                continue;
            }
            let queued_to_stop = states
                .iter()
                .any(|s| s.track == track && s.state == SlotState::QueuedToStop);
            stop_square(
                cells.stop,
                if queued_to_stop {
                    colours.meter.0
                } else {
                    colours.separator.0
                },
            );
            let press = ui.interact(stop, track_stop_id(track), Sense::click());
            let label = format!("Stop Clips {}", t.name());
            self.button(
                &press,
                &label,
                queued_to_stop,
                events,
                "session.stop_track",
                json!({ "track": track }),
            );
            // Spec: map-modes R3, R11. The map modes tint the stop buttons.
            crate::mapping::remote_overlay(
                ui,
                theme,
                stop,
                track_stop_id(track),
                RemoteTarget::TrackStop { track },
                false,
                events,
            );
            painter.rect_filled(cells.field.shrink(1.0), 0.0, colours.lane.0);
            // The playing clip's length in beats at the status display's
            // right (observed); the counter and the progress circle wait
            // for the engine to publish the clip's position.
            let playing = states
                .iter()
                .find(|s| s.track == track && s.state != SlotState::Queued);
            if let Some((state, clip)) = playing.and_then(|s| Some((s, doc.slot(track, s.scene)?)))
            {
                let beats = clip.length.get() as f64 / TICKS_PER_QUARTER as f64;
                let beats = (beats * 100.0).round() / 100.0;
                painter.text(
                    pos2(cells.field.right() - 4.0, cells.field.center().y),
                    Align2::RIGHT_CENTER,
                    format!("{beats}"),
                    theme.small_font(),
                    colours.text.0,
                );
                // Spec: detail-views R4 (L-SES-014). A click on the field
                // selects the clip that plays and shows the Clip View.
                let field = ui.interact(
                    cells.field.intersect(tracks),
                    status_field_id(track),
                    Sense::click(),
                );
                if field.clicked() {
                    select(events, Some(track), Some(state.scene), "replace");
                    show_clip_view(shell, events);
                }
            }
        }

        let main = MainStatusCells::compute(layout.main.rect, session.status_row);
        // Main lies right of the tracks, outside the painter above.
        let painter = ui.painter_at(main.stop_all.union(main.back));
        painter.rect_filled(main.stop_all.shrink(5.0), 0.0, colours.separator.0);
        let press = ui.interact(main.stop_all, stop_all_id(), Sense::click());
        self.button(
            &press,
            "Stop All Clips",
            false,
            events,
            "session.stop_all",
            json!({}),
        );
        crate::mapping::remote_overlay(
            ui,
            theme,
            main.stop_all,
            stop_all_id(),
            RemoteTarget::StopAll,
            false,
            events,
        );
        // Spec: follow-actions R8. The global follow-action switch (L-SES-008):
        // outlined in the accent colour while follow actions are on.
        let on = shell.launch_settings().follow_actions;
        let follow = main.follow.shrink(4.0);
        painter.rect_stroke(
            follow,
            theme.sizes.corner,
            Stroke::new(
                1.0,
                if on {
                    colours.accent.0
                } else {
                    colours.pending.0
                },
            ),
            StrokeKind::Inside,
        );
        let press = ui.interact(
            main.follow,
            Id::new("session-follow-actions"),
            Sense::click(),
        );
        self.button(
            &press,
            "Enable Follow Actions Globally",
            on,
            events,
            "session.set_follow_actions_enabled",
            json!({}),
        );
        // Back to Arrangement lights while a Session clip has taken a track
        // over from the Arrangement (L-SES-012).
        let lit = !shell.session_over().is_empty();
        let back = main.back.shrink2(vec2(5.0, 4.0));
        painter.add(Shape::convex_polygon(
            vec![
                pos2(back.right(), back.top()),
                pos2(back.left(), back.center().y),
                pos2(back.right(), back.bottom()),
            ],
            if lit {
                colours.accent.0
            } else {
                colours.pending.0
            },
            Stroke::NONE,
        ));
        let press = ui.interact(main.back, back_to_arrangement_id(), Sense::click());
        self.button(
            &press,
            "Back to Arrangement",
            lit,
            events,
            "session.back_to_arrangement",
            json!({}),
        );
    }

    /// Draws one scene's row in the Main column (L-SES-006).
    #[expect(clippy::too_many_arguments, reason = "one scene row, drawn in full")]
    fn scene_row(
        &mut self,
        ui: &mut Ui,
        shell: &Shell,
        theme: &Theme,
        row: &SceneRow,
        name: &str,
        index: usize,
        state: &SessionViewState,
        events: &mut Vec<UiEvent>,
    ) {
        let Some(scene) = shell.editor().document().scenes().nth(index).map(|s| s.id) else {
            return;
        };
        let row_rect = row.row.intersect(ui.max_rect());
        if row_rect.height() <= 0.0 {
            return;
        }
        let selected = state.scenes.contains(&scene);
        let painter = ui.painter().clone();
        // Spec: themes R18. A scene with a colour has its name field filled
        // with it.
        let fill = shell
            .editor()
            .document()
            .scene(scene)
            .and_then(|held| held.colour)
            .map_or(theme.colors.lane.0, |swatch| theme.swatch(swatch));
        painter.rect_filled(row.name.shrink(1.0), 0.0, fill);
        // Spec: session-view R21. View > Scene Tempo and Time Signature
        // puts the scene's fields where the name was (L-SES-007); they're
        // drawn last, above the row's own click.
        let fields = shell.mixer_view().scene_tempo();
        if !fields {
            painter.text(
                pos2(row.name.left() + 4.0, row.name.center().y),
                Align2::LEFT_CENTER,
                name,
                theme.small_font(),
                theme.colors.text.0,
            );
        }
        painter.text(
            row.number.center(),
            Align2::CENTER_CENTER,
            (index + 1).to_string(),
            theme.small_font(),
            theme.colors.text_dim.0,
        );
        triangle(&painter, row.launch.shrink(4.0), theme.colors.text_dim.0);
        let response = ui.interact(row_rect, scene_row_id(scene), Sense::click());
        response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, name));
        self.focus_ids.push(response.id);
        ui.ctx()
            .accesskit_node_builder(scene_row_id(scene), |node| {
                node.set_role(Role::ListItem);
                node.set_label(format!("Scene {name}"));
                node.set_selected(selected);
                node.set_bounds(bounds(row_rect));
            });
        crate::info_text::describe(ui.ctx(), &response, shell, InfoTarget::Scene(scene));
        // I-CLICK-009: the scene's launch button launches every clip in it.
        let launch = ui.interact(row.launch, scene_launch_id(scene), Sense::click());
        let label = format!("Launch Scene {name}");
        self.button(
            &launch,
            &label,
            false,
            events,
            "session.launch_scene",
            json!({ "scene": scene }),
        );
        crate::mapping::remote_overlay(
            ui,
            theme,
            row.launch,
            scene_launch_id(scene),
            RemoteTarget::SceneLaunch { scene },
            false,
            events,
        );
        if response.clicked() {
            response.request_focus();
            let modifiers = ui.input(|i| i.modifiers);
            select(events, None, Some(scene), click_mode(modifiers));
        }
        // Spec: session-context-menus R3, R14. A right-click on the scene's
        // launch button, name or number, or the keys while the grid has the
        // keyboard, open the scene's menu.
        let first = self.focused && state.slots.is_empty() && state.scenes.first() == Some(&scene);
        let at = row_rect.center();
        self.menus
            .scene(ui, theme, &launch, shell, scene, (at, false), events);
        self.menus
            .scene(ui, theme, &response, shell, scene, (at, first), events);
        if fields {
            self.scene_fields(ui, shell, theme, row, scene, events);
        }
        // Spec: session-context-menus R8. Rename types the name in the
        // scene's name field.
        let held = shell.editor().document().scene(scene);
        crate::session_menus::rename_edit(
            ui,
            theme,
            row.name.shrink(1.0),
            &Rename::Scene(scene),
            held.map_or("", |scene| scene.name.as_str()),
            crate::session_menus::next_scene(shell.editor().document(), scene),
            events,
        );
    }

    /// The scene's tempo, and the numerator and denominator of its time
    /// signature, side by side across the Main column's name area.
    fn scene_fields(
        &mut self,
        ui: &mut Ui,
        shell: &Shell,
        theme: &Theme,
        row: &SceneRow,
        scene: SceneId,
        events: &mut Vec<UiEvent>,
    ) {
        use bl_shell::scene_fields::SceneField::{Denominator, Numerator, Tempo};
        let area = row.name.shrink2(vec2(1.0, 1.0));
        let part = (area.width() - 2.0) / 4.0;
        let tempo = Rect::from_min_size(area.min, vec2(part * 2.0, area.height()));
        let numerator = Rect::from_min_size(tempo.right_top(), vec2(part, area.height()));
        let denominator = Rect::from_min_size(numerator.right_top(), vec2(part, area.height()));
        let label = crate::scene_fields::scene_label(shell, scene);
        for (rect, field) in [
            (tempo, Tempo),
            (numerator, Numerator),
            (denominator, Denominator),
        ] {
            crate::scene_fields::show(ui, shell, theme, rect, scene, field, &label, events);
        }
    }

    /// The arrow keys and Esc the grid reads itself, because each of their
    /// shortcuts has keys that act differently (the keymap gives a shortcut
    /// one command with one set of arguments). Delete is the keymap's:
    /// S-EDIT-005 runs `session.delete` in the Session View, and a focused
    /// scene field keeps the key for itself (session-context-menus R11).
    fn keys(
        &mut self,
        ui: &Ui,
        shell: &Shell,
        columns: &[TrackId],
        scenes: &[(SceneId, String)],
        events: &mut Vec<UiEvent>,
    ) {
        self.focused = crate::window::has_focus(ui.ctx(), shell, Area::MainView)
            && !ui.ctx().text_edit_focused()
            && ui.ctx().memory(|m| {
                m.focused().is_none_or(|id| {
                    id == grid_id()
                        || id == crate::main_window::main_view_id()
                        || self.focus_ids.contains(&id)
                })
            });
        if !self.focused || columns.is_empty() || scenes.is_empty() {
            return;
        }
        // The arrows and Esc are the grid's, not egui's focus navigation
        // (the Arrangement does the same; the accessibility spec, R4).
        if let Some(focused) = ui.ctx().memory(egui::Memory::focused) {
            ui.ctx().memory_mut(|m| {
                m.set_focus_lock_filter(
                    focused,
                    egui::EventFilter {
                        horizontal_arrows: true,
                        vertical_arrows: true,
                        escape: true,
                        ..egui::EventFilter::default()
                    },
                );
            });
        }
        let (up, down, left, right, page_up, page_down, esc, extend) = ui.input(|i| {
            let pressed = |key: Key| {
                i.events.iter().any(|event| {
                    matches!(
                        event,
                        egui::Event::Key { key: k, pressed: true, modifiers, .. }
                            if *k == key && !modifiers.command && !modifiers.ctrl && !modifiers.alt
                    )
                })
            };
            (
                pressed(Key::ArrowUp),
                pressed(Key::ArrowDown),
                pressed(Key::ArrowLeft),
                pressed(Key::ArrowRight),
                pressed(Key::PageUp),
                pressed(Key::PageDown),
                pressed(Key::Escape),
                i.modifiers.shift,
            )
        });
        // Esc during a drag of clips ends the drag, nothing else.
        let esc = esc && shell.clip_drag().is_none();
        let state = shell.session().state();
        // Spec: session-view R44; track-context-menu R18. Cmd (Ctrl) with Left
        // and Right moves the selected tracks a place (S-SES-015), and with Up
        // and Down the selected scenes (S-SES-016), keeping the gaps between
        // them.
        let (cmd_up, cmd_down, cmd_left, cmd_right) = ui.input(|i| {
            let pressed = |key: Key| {
                i.events.iter().any(|event| {
                    matches!(
                        event,
                        egui::Event::Key { key: k, pressed: true, modifiers, .. }
                            if *k == key
                                && modifiers.command
                                && !modifiers.alt
                                && !modifiers.shift
                    )
                })
            };
            (
                pressed(Key::ArrowUp),
                pressed(Key::ArrowDown),
                pressed(Key::ArrowLeft),
                pressed(Key::ArrowRight),
            )
        });
        if cmd_left || cmd_right {
            crate::track_menu::move_tracks(
                events,
                shell.editor().document(),
                &state.tracks,
                cmd_right,
            );
            return;
        }
        if cmd_up || cmd_down {
            move_scenes(events, shell.editor().document(), &state.scenes, cmd_down);
            return;
        }
        // Where the arrows move from: the selected slot, else the selected
        // scene or track.
        let (track_at, scene_at) = if let Some(slot) = state.slots.first().copied() {
            (
                columns.iter().position(|t| *t == slot.track).unwrap_or(0),
                scenes
                    .iter()
                    .position(|(id, _)| *id == slot.scene)
                    .unwrap_or(0),
            )
        } else if let Some(scene) = state.scenes.first().copied() {
            (
                0,
                scenes.iter().position(|(id, _)| *id == scene).unwrap_or(0),
            )
        } else if let Some(track) = state.tracks.first().copied() {
            (columns.iter().position(|t| *t == track).unwrap_or(0), 0)
        } else {
            (0, 0)
        };
        let mode = if extend { "range" } else { "replace" };
        if esc {
            // S-SES-019: Esc goes to the highlighted track's title bar.
            select(events, Some(columns[track_at]), None, "replace");
            return;
        }
        let scene_step = if page_up {
            -8_i64
        } else if page_down {
            8
        } else if up {
            -1
        } else if down {
            1
        } else {
            0
        };
        let track_step = if left {
            -1_i64
        } else if right {
            1
        } else {
            0
        };
        if scene_step == 0 && track_step == 0 {
            return;
        }
        let t = (track_at as i64 + track_step).clamp(0, columns.len() as i64 - 1) as usize;
        let s = (scene_at as i64 + scene_step).clamp(0, scenes.len() as i64 - 1) as usize;
        select(events, Some(columns[t]), Some(scenes[s].0), mode);
    }

    /// Drops files from the browser, Finder, or Explorer into a slot
    /// (I-DROP-001).
    #[expect(clippy::too_many_arguments, reason = "the frame's inputs")]
    fn take_drop(
        &mut self,
        ui: &Ui,
        shell: &Shell,
        session: &SessionLayout,
        layout: &MixerLayout,
        columns: &[TrackId],
        scenes: &[(SceneId, String)],
        events: &mut Vec<UiEvent>,
    ) {
        let ctx = ui.ctx().clone();
        let payload = egui::DragAndDrop::payload::<crate::browser::BrowserDrag>(&ctx);
        let set_payload = egui::DragAndDrop::payload::<crate::browser::SetDrag>(&ctx);
        let groove_payload = egui::DragAndDrop::payload::<crate::groove_pool::GrooveDrag>(&ctx);
        let (dropped, released, pointer, modifiers) = ctx.input(|i| {
            (
                i.raw
                    .dropped_files
                    .iter()
                    .map(|file| file.path().to_string_lossy().into_owned())
                    .collect::<Vec<String>>(),
                i.pointer.any_released(),
                i.pointer.hover_pos().or_else(|| i.pointer.latest_pos()),
                i.modifiers,
            )
        });
        if payload.is_none()
            && set_payload.is_none()
            && groove_payload.is_none()
            && dropped.is_empty()
        {
            return;
        }
        let state = shell.session().state();
        let Some(pos) = pointer else {
            return;
        };
        if let Some(drag) = groove_payload {
            // Spec: grooves R28 (I-DROP-017). A groove dropped on a slot's
            // clip becomes the clip's groove, loaded first if it is a file.
            let clip = slot_at(session, layout, columns, scenes, state.scroll_y as f32, pos)
                .and_then(|(track, scene)| shell.editor().document().slot(track, scene))
                .map(|clip| clip.id);
            if released {
                egui::DragAndDrop::clear_payload(&ctx);
                if let Some(clip) = clip {
                    events.push(UiEvent::Invoke(match (drag.groove, &drag.path) {
                        (Some(groove), _) => Invocation::new(
                            "clip.set_groove",
                            json!({ "clips": [clip], "groove": groove }),
                        ),
                        (None, Some(path)) => Invocation::new(
                            bl_shell::groove_commands::LOAD,
                            json!({ "path": path, "clips": [clip] }),
                        ),
                        (None, None) => return,
                    }));
                }
            } else if clip.is_none() {
                ctx.set_cursor_icon(egui::CursorIcon::NoDrop);
            }
            return;
        }
        if let Some(drag) = set_payload {
            // Spec: set-exchange R3, R8 (I-DROP-021, I-DROP-022). A set, its
            // tracks or a chain go into the drop area right of the tracks.
            let area = layout
                .drop_area
                .map(|area| area.intersect(session.slots))
                .filter(|area| area.contains(pos));
            if let Some(area) = area {
                ui.painter_at(area).rect_stroke(
                    area,
                    0.0,
                    egui::Stroke::new(2.0, ui.visuals().selection.stroke.color),
                    egui::StrokeKind::Inside,
                );
            }
            if released {
                egui::DragAndDrop::clear_payload(&ctx);
                if area.is_some() {
                    events.push(UiEvent::Invoke(Invocation::new(
                        "set.import",
                        drag.args(None, true),
                    )));
                }
            }
            return;
        }
        // Clip files can also be dropped in the drop area, where each makes
        // a track of its kind in the scene it is dropped at (clip files R7).
        let clips = payload
            .as_ref()
            .is_some_and(|p| p.paths.iter().all(|p| crate::browser::is_clip(p)))
            || (!dropped.is_empty() && dropped.iter().all(|p| crate::browser::is_clip(p)));
        let slot = slot_at(session, layout, columns, scenes, state.scroll_y as f32, pos);
        let area = layout
            .drop_area
            .map(|area| area.intersect(session.slots))
            .filter(|area| clips && slot.is_none() && area.contains(pos));
        let Some((track, scene)) = slot.map(|(track, scene)| (Some(track), scene)).or_else(|| {
            let row = ((pos.y - session.slots.top() + state.scroll_y as f32) / SLOT_ROW).floor();
            let scene = scenes.get(usize::try_from(row as i64).ok()?)?.0;
            area.map(|_| (None, scene))
        }) else {
            return;
        };
        if let Some(area) = area {
            ui.painter_at(area).rect_stroke(
                area,
                0.0,
                egui::Stroke::new(2.0, ui.visuals().selection.stroke.color),
                egui::StrokeKind::Inside,
            );
        }
        // Several files spread across the tracks with the command modifier
        // held (I-DROP-003), as the Arrangement's drop overlay does.
        let spread = spread_modifier(modifiers);
        let args = |paths: &[String]| {
            let mut args = json!({ "paths": paths, "scene": scene, "spread": spread });
            if let Some(track) = track {
                args["track"] = json!(track);
            }
            args
        };
        if let Some(payload) = payload
            && released
        {
            egui::DragAndDrop::clear_payload(&ctx);
            events.push(UiEvent::Invoke(Invocation::new(
                ids::DROP_FILES,
                args(&payload.paths),
            )));
        }
        if !dropped.is_empty() {
            events.push(UiEvent::Invoke(Invocation::new(
                ids::DROP_FILES,
                args(&dropped),
            )));
        }
    }
}

/// The slot under a window point, if any.
fn slot_at(
    session: &SessionLayout,
    layout: &MixerLayout,
    columns: &[TrackId],
    scenes: &[(SceneId, String)],
    scroll_y: f32,
    pos: Pos2,
) -> Option<(TrackId, SceneId)> {
    if !session.slots.contains(pos) {
        return None;
    }
    let row = ((pos.y - session.slots.top() + scroll_y) / SLOT_ROW).floor();
    if row < 0.0 {
        return None;
    }
    let (scene, _) = scenes.get(row as usize)?;
    layout
        .columns
        .iter()
        .find(|column| pos.x >= column.rect.left() && pos.x < column.rect.right())
        .map(|column| (columns[column.index], *scene))
}

/// Shows the Clip View, as a click on its selector does (R4).
fn show_clip_view(shell: &Shell, events: &mut Vec<UiEvent>) {
    events.extend(
        crate::main_window::open_view(shell, bl_shell::commands::DetailView::Clip)
            .into_iter()
            .map(UiEvent::Invoke),
    );
}

fn click_mode(modifiers: egui::Modifiers) -> &'static str {
    if modifiers.command || modifiers.ctrl {
        "add"
    } else if modifiers.shift {
        "range"
    } else {
        "replace"
    }
}

/// Whether the copy modifier is held: Option on macOS, Ctrl elsewhere
/// (S-SES-004, I-DRAG-002).
fn copy_modifier(modifiers: egui::Modifiers) -> bool {
    if cfg!(target_os = "macos") {
        modifiers.alt
    } else {
        modifiers.ctrl
    }
}

/// Whether the command modifier is held: Cmd on macOS, Ctrl elsewhere.
/// Several files dropped with it spread across the tracks (I-DROP-003).
fn spread_modifier(modifiers: egui::Modifiers) -> bool {
    if cfg!(target_os = "macos") {
        modifiers.mac_cmd
    } else {
        modifiers.ctrl
    }
}

fn select(events: &mut Vec<UiEvent>, track: Option<TrackId>, scene: Option<SceneId>, mode: &str) {
    let mut args = serde_json::Map::new();
    if let Some(track) = track {
        args.insert("track".to_owned(), json!(track));
    }
    if let Some(scene) = scene {
        args.insert("scene".to_owned(), json!(scene));
    }
    args.insert("mode".to_owned(), json!(mode));
    events.push(UiEvent::Invoke(Invocation::new(
        ids::SELECT,
        Value::Object(args),
    )));
}

/// The colour of a slot's launch triangle: grey while it's stopped,
/// flashing white and green while a launch is queued, and green while the
/// clip plays (observed). A stop still queued keeps the playing green.
fn launch_colour(theme: &Theme, state: Option<SlotState>, time: f64) -> Color32 {
    let colours = &theme.colors;
    match state {
        None => colours.text_dim.0,
        Some(SlotState::Queued) if (time * 4.0) as i64 % 2 == 0 => colours.text.0,
        Some(_) => colours.meter.0,
    }
}

/// A right-pointing launch triangle, as Live draws it on a slot and a
/// scene's launch button.
fn triangle(painter: &egui::Painter, rect: Rect, colour: Color32) {
    if rect.width() <= 0.0 || rect.height() <= 0.0 {
        return;
    }
    let (w, h) = (rect.width().min(rect.height()), rect.height());
    let centre = rect.center();
    painter.add(Shape::convex_polygon(
        vec![
            pos2(centre.x - w / 2.0, centre.y - h / 2.0),
            pos2(centre.x + w / 2.0, centre.y),
            pos2(centre.x - w / 2.0, centre.y + h / 2.0),
        ],
        colour,
        Stroke::NONE,
    ));
}

fn bounds(rect: Rect) -> accesskit::Rect {
    accesskit::Rect {
        x0: rect.min.x.into(),
        y0: rect.min.y.into(),
        x1: rect.max.x.into(),
        y1: rect.max.y.into(),
    }
}

/// The track a column shows.
#[must_use]
pub fn column_track(
    doc: &Document,
    columns: &[TrackId],
    returns: &[TrackId],
    column: &Column,
) -> TrackId {
    match column.kind {
        ColumnKind::Track => columns[column.index],
        ColumnKind::Chain(_) => columns[column.index],
        ColumnKind::Return => returns[column.index],
        ColumnKind::Main => doc.main_id(),
    }
}

/// Moves every one of `selected` a scene down (or up) as one undo step
/// (S-SES-016).
fn move_scenes(
    events: &mut Vec<UiEvent>,
    doc: &bl_model::Document,
    selected: &[SceneId],
    later: bool,
) {
    let moves = bl_shell::tracks::scenes_moved_by_one(doc, selected, later);
    if moves.is_empty() {
        return;
    }
    events.push(UiEvent::BeginNamedGesture("Move scene".to_owned()));
    for (scene, index) in moves {
        events.push(UiEvent::Invoke(Invocation::new(
            "scene.move",
            json!({ "scene": scene, "index": index }),
        )));
    }
    events.push(UiEvent::EndGesture);
}

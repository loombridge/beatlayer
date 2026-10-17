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
use bl_model::{ClipId, Document, SceneId, TrackId, TrackKind};
use bl_shell::mixer_view::MixerOf;
use bl_shell::session_view_commands as ids;
use bl_shell::session_view_commands::SessionViewState;
use bl_shell::{Area, Invocation, Shell, WindowKind};
use egui::accesskit::{self, Role};
use egui::{
    Align2, Color32, Id, Key, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Ui, WidgetInfo,
    WidgetType, pos2, vec2,
};
use serde_json::{Value, json};

use crate::arrangement::scene::track_colour;
use crate::layout::mixer::{
    Column, ColumnKind, MainStatusCells, MixerLayout, SLOT_ROW, SceneRow, SessionLayout,
    StatusCells,
};
use crate::main_window::UiEvent;
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
        let returns: Vec<TrackId> = doc.returns().collect();
        let sections = crate::mixer::sections(shell);
        let scroll_x = shell.mixer_view().of(MixerOf::Session).scroll as f32;
        let state = shell.session().state();
        let scroll_y = state.scroll_y as f32;
        let layout = MixerLayout::compute(
            session.slots,
            columns.len(),
            returns.len(),
            sections,
            scroll_x,
        );
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
            }
        }

        self.status_row(ui, shell, theme, session, &layout, &columns, events);
        self.keys(&grid, shell, &columns, &scenes, events);
        self.take_drop(&grid, shell, session, &layout, &columns, &scenes, events);
        // A clip drag that ended: move it to the slot it was released on, or
        // copy it with the copy modifier held (I-DRAG-001, I-DRAG-002).
        if let Some((clip, from)) = self.released.take() {
            let (pos, modifiers) = ui.input(|i| (i.pointer.interact_pos(), i.modifiers));
            if let Some(pos) = pos
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
        ui.ctx()
            .accesskit_node_builder(slot_id(track, scene), |node| {
                node.set_role(Role::ListItem);
                node.set_label(label);
                node.set_selected(selected);
                node.set_bounds(bounds(cell));
            });
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
            Some(_) => track_colour(theme, track).gamma_multiply(0.75),
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
        // I-DRAG-001, I-DRAG-002: dragging a clip moves it to another slot;
        // Option-drag (Ctrl-drag on Windows and Linux) copies it.
        if response.drag_started()
            && let Some(clip) = clip_id
        {
            self.drag = Some(Drag::Clip {
                clip,
                from: (track, scene),
            });
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
            painter.rect_filled(cells.field.shrink(1.0), 0.0, colours.lane.0);
            // The playing clip's length in beats at the status display's
            // right (observed); the counter and the progress circle wait
            // for the engine to publish the clip's position.
            let playing = states
                .iter()
                .find(|s| s.track == track && s.state != SlotState::Queued);
            if let Some(clip) = playing.and_then(|s| doc.slot(track, s.scene)) {
                let beats = clip.length.get() as f64 / TICKS_PER_QUARTER as f64;
                let beats = (beats * 100.0).round() / 100.0;
                painter.text(
                    pos2(cells.field.right() - 4.0, cells.field.center().y),
                    Align2::RIGHT_CENTER,
                    format!("{beats}"),
                    theme.small_font(),
                    colours.text.0,
                );
            }
        }

        let main = MainStatusCells::compute(layout.main.rect, session.status_row);
        stop_square(main.stop_all, colours.separator.0);
        let press = ui.interact(main.stop_all, stop_all_id(), Sense::click());
        self.button(
            &press,
            "Stop All Clips",
            false,
            events,
            "session.stop_all",
            json!({}),
        );
        // The global follow-action switch: drawn in place, disabled until
        // WP-3.10 brings follow actions.
        let follow = main.follow.shrink(4.0);
        painter.rect_stroke(
            follow,
            theme.sizes.corner,
            Stroke::new(1.0, colours.pending.0),
            StrokeKind::Inside,
        );
        let id = Id::new("session-follow-actions");
        ui.interact(main.follow, id, Sense::hover());
        ui.ctx().accesskit_node_builder(id, |node| {
            node.set_role(Role::Button);
            node.set_label("Enable Follow Actions Globally".to_owned());
            node.set_disabled();
            node.set_description("Not built yet: arrives with WP-3.10.".to_owned());
            node.set_bounds(bounds(main.follow));
        });
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
        painter.rect_filled(row.name.shrink(1.0), 0.0, theme.colors.lane.0);
        painter.text(
            pos2(row.name.left() + 4.0, row.name.center().y),
            Align2::LEFT_CENTER,
            name,
            theme.small_font(),
            theme.colors.text.0,
        );
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
        if response.clicked() {
            response.request_focus();
            let modifiers = ui.input(|i| i.modifiers);
            select(events, None, Some(scene), click_mode(modifiers));
        }
    }

    /// The arrow keys, Delete, and Esc the grid reads itself, because each
    /// of their shortcuts has keys that act differently (the keymap gives a
    /// shortcut one command with one set of arguments), and because a
    /// focused value field keeps Delete for itself.
    fn keys(
        &mut self,
        ui: &Ui,
        shell: &Shell,
        columns: &[TrackId],
        scenes: &[(SceneId, String)],
        events: &mut Vec<UiEvent>,
    ) {
        self.focused = shell.focused_area() == Area::MainView
            && shell.focused_window() == WindowKind::Main
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
        let (up, down, left, right, page_up, page_down, delete, esc, extend) = ui.input(|i| {
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
                pressed(Key::Delete),
                pressed(Key::Escape),
                i.modifiers.shift,
            )
        });
        let state = shell.session().state();
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
        if delete && (!state.slots.is_empty() || !state.scenes.is_empty()) {
            // Delete removes the selected clips or scenes (S-EDIT-005); the
            // grid reads the key itself so a focused value field keeps it.
            events.push(UiEvent::Invoke(Invocation::new(ids::DELETE, json!({}))));
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
        if payload.is_none() && dropped.is_empty() {
            return;
        }
        let state = shell.session().state();
        let Some(pos) = pointer else {
            return;
        };
        let Some((track, scene)) =
            slot_at(session, layout, columns, scenes, state.scroll_y as f32, pos)
        else {
            return;
        };
        // Several files spread across the tracks with the command modifier
        // held (I-DROP-003), as the Arrangement's drop overlay does.
        let spread = spread_modifier(modifiers);
        if let Some(payload) = payload
            && released
        {
            egui::DragAndDrop::clear_payload(&ctx);
            events.push(UiEvent::Invoke(Invocation::new(
                ids::DROP_FILES,
                json!({ "paths": payload.paths, "track": track, "scene": scene, "spread": spread }),
            )));
        }
        if !dropped.is_empty() {
            events.push(UiEvent::Invoke(Invocation::new(
                ids::DROP_FILES,
                json!({ "paths": dropped, "track": track, "scene": scene, "spread": spread }),
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
        ColumnKind::Return => returns[column.index],
        ColumnKind::Main => doc.main_id(),
    }
}

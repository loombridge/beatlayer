//! The arranger track's input (WP-6.45; the arranger track spec, R5 to R14):
//! dragging across the lane to make a section, dragging a section to move
//! or (with Option, Ctrl on Windows) copy it, dragging its edges, clicking,
//! double-clicking and typing its name, and its menus. The lane is drawn by
//! [`super::rulers::arranger_lane`].
//!
//! A drag is previewed here and runs one command when the pointer is
//! released, so each gesture is one undo step; Esc abandons it.

use bl_core::time::Ticks;
use bl_model::SectionId;
use bl_shell::Invocation;
use bl_shell::Shell;
use bl_shell::arranger_commands as ids;
use egui::accesskit::Role;
use egui::{
    CursorIcon, Id, Key, Pos2, Rect, Sense, Stroke, Ui, UiBuilder, WidgetInfo, WidgetType, pos2,
};
use serde_json::json;

use super::context_menu::ContextMenus;
use super::editing::EditKeys;
use super::invoke;
use crate::main_window::UiEvent;
use crate::theme::Theme;

/// How wide a section's edge zone is, which resizes it.
const EDGE: f32 = 4.0;

/// The lane's ID.
pub fn lane_id() -> Id {
    Id::new("arrangement-arranger")
}

/// A section's ID, in the lane.
pub fn section_id(section: SectionId) -> Id {
    Id::new(("arrangement-section", section))
}

/// Where a name being typed is kept.
fn renaming_id() -> Id {
    Id::new("arrangement-section-renaming")
}

/// A section being renamed in place.
#[derive(Clone, Debug)]
struct Renaming {
    section: SectionId,
    /// The text typed so far, once the field has been shown.
    text: Option<String>,
    /// Whether the field has the keyboard.
    focused: bool,
}

/// Starts typing a new name for `section` in the lane, as Cmd-R does (the
/// arranger track spec, R7).
pub fn start_rename(ctx: &egui::Context, section: SectionId) {
    ctx.data_mut(|d| {
        d.insert_temp(
            renaming_id(),
            Some(Renaming {
                section,
                text: None,
                focused: false,
            }),
        );
    });
}

/// Whether a section's name is being typed.
pub fn is_renaming(ctx: &egui::Context) -> bool {
    renaming(ctx).is_some()
}

fn renaming(ctx: &egui::Context) -> Option<Renaming> {
    ctx.data(|d| d.get_temp::<Option<Renaming>>(renaming_id()))
        .flatten()
}

/// A drag in the lane, which runs its command when the pointer is released.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Drag {
    /// Across an empty stretch: a new section from `from` to `to`.
    Make { from: Ticks, to: Ticks },
    /// A section by its body, to be dropped at `to`.
    Move {
        section: SectionId,
        to: Ticks,
        copy: bool,
    },
    /// A section's left or right edge, to `at`.
    Edge {
        section: SectionId,
        right: bool,
        at: Ticks,
    },
}

/// What the lane keeps between frames.
#[derive(Debug, Default)]
pub(super) struct ArrangerLane {
    drag: Option<Drag>,
    /// The sections that have a widget this frame, in time order, for the
    /// Arrangement's Tab order (the arranger track spec, R14).
    pub tab: Vec<Id>,
}

impl ArrangerLane {
    /// Handles what the person did in the lane `lane` this frame.
    #[expect(clippy::too_many_arguments, reason = "the frame's inputs")]
    pub fn show(
        &mut self,
        ui: &mut Ui,
        shell: &Shell,
        theme: &Theme,
        lane: Rect,
        menus: &mut ContextMenus,
        snap_line: &mut Option<Ticks>,
        events: &mut Vec<UiEvent>,
    ) {
        self.tab.clear();
        ui.scope_builder(
            UiBuilder::new().id_salt("arranger-track").max_rect(lane),
            |ui| {
                ui.ctx().accesskit_node_builder(ui.unique_id(), |node| {
                    node.set_role(Role::List);
                    node.set_label("Arranger Track".to_owned());
                });
                self.sections(ui, shell, theme, lane, menus, snap_line, events);
            },
        );
    }

    #[expect(clippy::too_many_arguments, reason = "the frame's inputs")]
    fn sections(
        &mut self,
        ui: &mut Ui,
        shell: &Shell,
        theme: &Theme,
        lane: Rect,
        menus: &mut ContextMenus,
        snap_line: &mut Option<Ticks>,
        events: &mut Vec<UiEvent>,
    ) {
        let view = shell.arrangement();
        let doc = shell.editor().document();
        let horizontal = view.horizontal(doc);
        let ppp = horizontal.ticks_per_point;
        let keys = EditKeys::from(ui.input(|i| i.modifiers));
        let at_x = |pos: Pos2| horizontal.at(f64::from(pos.x - lane.left()));
        let x_of = |at: Ticks| lane.left() + horizontal.x(at) as f32;
        // Where an edge or a new section lands: on the grid, which Cmd skips.
        let snapped = |pos: Pos2| {
            let at = at_x(pos);
            if keys.bypass { at } else { view.snap(doc, at) }.max(Ticks::ZERO)
        };
        // Esc abandons a drag: the lane runs nothing.
        if ui.input(|i| i.key_pressed(Key::Escape)) {
            self.drag = None;
        }
        // The lane itself, under the sections' widgets.
        let background = ui.interact(lane, lane_id().with("background"), Sense::click_and_drag());
        if background.secondary_clicked() {
            menus.open_arranger();
        }
        background.context_menu(|ui| menus.show(ui, theme, shell, events));
        if background.clicked() && !view.selected_sections().is_empty() {
            invoke(events, ids::SELECT_SECTION, json!({ "sections": [] }));
        }
        if background.drag_started()
            && let Some(pos) = background.interact_pointer_pos()
        {
            let from = snapped(pos);
            self.drag = Some(Drag::Make { from, to: from });
        }
        if background.dragged()
            && let (Some(Drag::Make { from, .. }), Some(pos)) =
                (self.drag, background.interact_pointer_pos())
        {
            self.drag = Some(Drag::Make {
                from,
                to: snapped(pos),
            });
        }
        if background.drag_stopped()
            && let Some(Drag::Make { from, to }) = self.drag.take()
            && from != to
        {
            invoke(
                events,
                ids::ADD_SECTION,
                json!({ "start": from.min(to), "end": from.max(to) }),
            );
        }

        for section in doc.sections() {
            let (x0, x1) = (x_of(section.start), x_of(section.end()));
            let held = match self.drag {
                Some(Drag::Move { section: s, .. } | Drag::Edge { section: s, .. }) => {
                    s == section.id
                }
                _ => false,
            };
            if !held && (x1 < lane.left() || x0 > lane.right()) {
                continue;
            }
            let id = section_id(section.id);
            let rect = Rect::from_min_max(pos2(x0, lane.top()), pos2(x1, lane.bottom()));
            let edges = rect.width() > 3.0 * EDGE;
            let body = if edges {
                Rect::from_min_max(pos2(x0 + EDGE, lane.top()), pos2(x1 - EDGE, lane.bottom()))
            } else {
                rect
            };
            let selected = view.selected_sections().contains(&section.id);
            let response = ui.interact(body, id, Sense::click_and_drag());
            self.tab.push(id);

            // Spec: arranger-track R6. Clicking selects the section; Shift
            // extends to it. A right-click selects it too and opens its menu.
            if response.secondary_clicked() {
                if !selected {
                    invoke(
                        events,
                        ids::SELECT_SECTION,
                        json!({ "sections": [section.id] }),
                    );
                }
                menus.open_section();
            } else if response.double_clicked() {
                // Spec: arranger-track R7.
                invoke(
                    events,
                    ids::RENAME_SECTION,
                    json!({ "section": section.id }),
                );
            } else if response.clicked() || response.drag_started() {
                let mode = if keys.extend { "extend" } else { "replace" };
                if !selected || mode == "extend" {
                    invoke(
                        events,
                        ids::SELECT_SECTION,
                        json!({ "sections": [section.id], "mode": mode }),
                    );
                }
            }
            response.context_menu(|ui| menus.show(ui, theme, shell, events));
            // Spec: arranger-track R14. Tab reaching a section selects it,
            // and the context-menu key opens its menu.
            if response.has_focus() {
                if !selected {
                    invoke(
                        events,
                        ids::SELECT_SECTION,
                        json!({ "sections": [section.id] }),
                    );
                }
                if crate::menus::context_menu_keys(ui) {
                    menus.open_section();
                    crate::menus::open_keyboard_context_menu(
                        ui.ctx(),
                        id,
                        rect.left_top() + egui::vec2(4.0, 4.0),
                    );
                }
            }
            crate::menus::keyboard_context_menu(&response, |ui| {
                menus.show(ui, theme, shell, events);
            });

            let signatures = &doc.settings().time_signatures;
            let name = if section.name.is_empty() {
                "Section"
            } else {
                section.name.as_str()
            };
            response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, name));
            ui.ctx().accesskit_node_builder(id, |node| {
                node.set_role(Role::ListItem);
                node.set_label(name.to_owned());
                node.set_value(format!(
                    "{} to {}",
                    signatures.to_bbs(section.start),
                    signatures.to_bbs(section.end())
                ));
                node.set_selected(selected);
            });

            // Spec: arranger-track R8, R9. Dragging the body moves the
            // section, or copies it with Option (Ctrl on Windows): the drop
            // goes to a section's edge near the pointer, else the grid, and
            // Cmd skips both.
            if response.drag_started() {
                self.drag = Some(Drag::Move {
                    section: section.id,
                    to: section.start,
                    copy: false,
                });
            }
            if response.dragged()
                && let (Some(Drag::Move { section: s, .. }), Some(pos)) =
                    (self.drag, response.interact_pointer_pos())
                && s == section.id
            {
                let to = view.section_drop(doc, at_x(pos), ppp, keys.bypass);
                self.drag = Some(Drag::Move {
                    section: s,
                    to,
                    copy: keys.copy,
                });
            }
            if response.drag_stopped()
                && let Some(Drag::Move {
                    section: s,
                    to,
                    copy,
                }) = self.drag
                && s == section.id
            {
                self.drag = None;
                let command = if copy {
                    ids::COPY_SECTION
                } else {
                    ids::MOVE_SECTION
                };
                invoke(events, command, json!({ "section": s, "to": to }));
            }

            // Spec: arranger-track R7. Dragging an edge moves it on the grid.
            if edges {
                for right in [false, true] {
                    let zone = if right {
                        Rect::from_min_max(pos2(x1 - EDGE, lane.top()), pos2(x1, lane.bottom()))
                    } else {
                        Rect::from_min_max(pos2(x0, lane.top()), pos2(x0 + EDGE, lane.bottom()))
                    };
                    let edge = ui
                        .interact(zone, id.with(("edge", right)), Sense::drag())
                        .on_hover_cursor(CursorIcon::ResizeHorizontal);
                    if edge.drag_started() {
                        self.drag = Some(Drag::Edge {
                            section: section.id,
                            right,
                            at: if right { section.end() } else { section.start },
                        });
                    }
                    if edge.dragged()
                        && let (Some(Drag::Edge { section: s, .. }), Some(pos)) =
                            (self.drag, edge.interact_pointer_pos())
                        && s == section.id
                    {
                        let at = snapped(pos);
                        // An edge doesn't cross the other one.
                        let at = if right {
                            at.max(section.start + Ticks(1))
                        } else {
                            at.min(section.end() - Ticks(1))
                        };
                        self.drag = Some(Drag::Edge {
                            section: s,
                            right,
                            at,
                        });
                    }
                    if edge.drag_stopped()
                        && let Some(Drag::Edge {
                            section: s,
                            right: dragged_right,
                            at,
                        }) = self.drag
                        && s == section.id
                        && dragged_right == right
                    {
                        self.drag = None;
                        let args = if right {
                            json!({ "section": s, "end": at })
                        } else {
                            json!({ "section": s, "start": at })
                        };
                        invoke(events, ids::RESIZE_SECTION, args);
                    }
                }
            }

            // Spec: arranger-track R7. The name being typed, over the
            // section: Return keeps it and Esc drops it.
            if let Some(mut typing) = renaming(ui.ctx()).filter(|r| r.section == section.id) {
                let text = typing.text.get_or_insert_with(|| section.name.clone());
                let edit = ui.put(
                    Rect::from_min_max(
                        pos2(x0.max(lane.left()) + 2.0, lane.top() + 1.0),
                        pos2(
                            (x0.max(lane.left()) + 142.0).min(lane.right()),
                            lane.bottom() - 1.0,
                        ),
                    ),
                    egui::TextEdit::singleline(text)
                        .id(Id::new(("arrangement-section-rename", section.id)))
                        .font(theme.small_font()),
                );
                if !typing.focused {
                    edit.request_focus();
                    typing.focused = true;
                }
                let (enter, escape) =
                    ui.input(|i| (i.key_pressed(Key::Enter), i.key_pressed(Key::Escape)));
                if edit.lost_focus() || enter || escape {
                    if enter || (edit.lost_focus() && !escape) {
                        events.push(UiEvent::Invoke(Invocation::new(
                            ids::RENAME_SECTION,
                            json!({ "section": section.id, "name": text }),
                        )));
                    }
                    ui.ctx()
                        .data_mut(|d| d.insert_temp(renaming_id(), None::<Renaming>));
                } else {
                    ui.ctx()
                        .data_mut(|d| d.insert_temp(renaming_id(), Some(typing)));
                }
            }
        }

        // The drag's preview: where the section or span would land, and the
        // line through the lanes the drop snaps to.
        let painter = ui.painter_at(lane);
        let outline = Stroke::new(1.5, theme.colors.accent.0);
        let span = |from: Ticks, to: Ticks| {
            Rect::from_min_max(
                pos2(x_of(from), lane.top() + 1.0),
                pos2(x_of(to), lane.bottom() - 1.0),
            )
        };
        match self.drag {
            Some(Drag::Make { from, to }) if from != to => {
                painter.rect_stroke(
                    span(from.min(to), from.max(to)),
                    0.0,
                    outline,
                    egui::StrokeKind::Inside,
                );
                *snap_line = Some(to);
            }
            Some(Drag::Move { section, to, .. }) => {
                if let Some(held) = doc.section(section) {
                    painter.rect_stroke(
                        span(to, to + held.length),
                        0.0,
                        outline,
                        egui::StrokeKind::Inside,
                    );
                }
                *snap_line = Some(to);
            }
            Some(Drag::Edge { section, right, at }) => {
                if let Some(held) = doc.section(section) {
                    let (from, to) = if right {
                        (held.start, at)
                    } else {
                        (at, held.end())
                    };
                    painter.rect_stroke(span(from, to), 0.0, outline, egui::StrokeKind::Inside);
                }
                *snap_line = Some(at);
            }
            _ => {}
        }
        // A drag whose widget is gone never sees its release: end it once
        // the pointer is up.
        if self.drag.is_some() && ui.input(|i| !i.pointer.any_down()) {
            self.drag = None;
        }
    }
}

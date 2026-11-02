//! The chord lane's input (WP-6.48; the chord lane spec, R3 and R4):
//! drawing a region with the pointer, moving it, resizing it from its edges,
//! selecting it, and its context menu. The lane is drawn by
//! [`super::rulers::chord_lane`].
//!
//! Every drag is a gesture of the document's `chord_lane.*` commands, so it
//! is one undo step; the grid snaps its edges as it snaps a clip's.

use bl_core::time::Ticks;
use bl_model::{ChordRegion, ChordRegionId, ScaleName};
use bl_shell::Shell;
use bl_shell::chord_lane_commands as ids;
use bl_shell::{GesturePhase, Invocation};
use egui::accesskit::Role;
use egui::{Id, Key, Pos2, Rect, Sense, Ui, WidgetInfo, WidgetType, pos2};
use serde_json::json;

use super::editing::EditKeys;
use super::{invoke, rulers};
use crate::main_window::UiEvent;

/// How wide a region's edge handle is, for resizing it.
const EDGE: f32 = 5.0;

/// The lane's own ID, which a press on empty lane draws from.
pub fn lane_id() -> Id {
    Id::new("arrangement-chord-lane")
}

/// A region's ID, in the lane.
pub fn region_id(region: ChordRegionId) -> Id {
    Id::new(("arrangement-chord-region", region.0.get()))
}

/// A region's edge handle's ID: its start's, or its end's.
fn edge_id(region: ChordRegionId, end: bool) -> Id {
    Id::new(("arrangement-chord-edge", region.0.get(), end))
}

/// What a drag in the lane is doing.
#[derive(Clone, Copy, Debug)]
enum Drag {
    /// Drawing a new region from the grid line it began on.
    Draw { anchor: Ticks },
    /// Moving a region, by where the pointer took hold of it.
    Move { region: ChordRegionId, grab: Ticks },
    /// Resizing a region from its start or its end.
    Edge { region: ChordRegionId, end: bool },
}

/// What the lane keeps between frames.
#[derive(Debug, Default)]
pub(super) struct ChordLane {
    drag: Option<Drag>,
}

impl ChordLane {
    /// Handles what the person did in the lane `lane` this frame.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        shell: &Shell,
        lane: Rect,
        snap_line: &mut Option<Ticks>,
        events: &mut Vec<UiEvent>,
    ) {
        let view = shell.arrangement();
        let doc = shell.editor().document();
        let horizontal = view.horizontal(doc);
        let at_x = |pos: Pos2| horizontal.at(f64::from(pos.x - lane.left()));
        let bypass = ui.input(|i| EditKeys::from(i.modifiers).bypass);
        let snap = |at: Ticks| view.snap_drag(doc, at, bypass).max(Ticks::ZERO);
        let gesture = |events: &mut Vec<UiEvent>, phase, id: &str, args| {
            events.push(UiEvent::Gesture {
                phase,
                invocation: Some(Invocation::new(id, args)),
            });
        };
        // Esc abandons a drag, as it does a locator's.
        if ui.input(|i| i.key_pressed(Key::Escape)) && self.drag.take().is_some() {
            events.push(UiEvent::Gesture {
                phase: GesturePhase::Cancel,
                invocation: None,
            });
        }
        let selected = view.selected_chord_region();

        // Empty lane: a click deselects, a drag draws a region.
        let empty = ui.interact(lane, lane_id(), Sense::click_and_drag());
        empty.widget_info(|| WidgetInfo::labeled(WidgetType::Other, true, "Chord lane"));
        if empty.clicked() && selected.is_some() {
            invoke(events, ids::SELECT_REGION, json!({}));
        }
        let pointer = empty.interact_pointer_pos();
        if empty.drag_started()
            && let Some(pos) = pointer
        {
            self.drag = Some(Drag::Draw {
                anchor: snap(at_x(pos)),
            });
        }
        if empty.dragged()
            && let (Some(Drag::Draw { anchor }), Some(pos)) = (self.drag, pointer)
        {
            let now = snap(at_x(pos));
            *snap_line = Some(now);
            let (start, end) = (anchor.min(now), anchor.max(now));
            if end > start {
                gesture(
                    events,
                    GesturePhase::Update,
                    ids::ADD_REGION,
                    json!({ "region": { "start": start, "end": end } }),
                );
            }
        }
        if empty.drag_stopped() && matches!(self.drag, Some(Drag::Draw { .. })) {
            self.drag = None;
            events.push(UiEvent::Gesture {
                phase: GesturePhase::Commit,
                invocation: None,
            });
        }

        for region in doc.chord_lane() {
            let Some(block) = rulers::region_rect(lane, horizontal, region) else {
                continue;
            };
            self.region(
                ui,
                shell,
                (lane, block),
                region,
                (&snap, &at_x),
                snap_line,
                events,
            );
        }

        // A drag whose region is gone never sees its release: end it once the
        // pointer is up.
        if self.drag.is_some() && ui.input(|i| !i.pointer.any_down()) {
            self.drag = None;
            events.push(UiEvent::Gesture {
                phase: GesturePhase::Commit,
                invocation: None,
            });
        }
    }

    /// One region: its body, which selects and moves it, its two edges,
    /// which resize it, and its menu.
    #[expect(clippy::too_many_arguments, reason = "the frame's inputs")]
    fn region(
        &mut self,
        ui: &mut Ui,
        shell: &Shell,
        (lane, block): (Rect, Rect),
        region: &ChordRegion,
        (snap, at_x): (&dyn Fn(Ticks) -> Ticks, &dyn Fn(Pos2) -> Ticks),
        snap_line: &mut Option<Ticks>,
        events: &mut Vec<UiEvent>,
    ) {
        let view = shell.arrangement();
        let chosen = view.selected_chord_region() == Some(region.id);
        let name = region.name();
        let id = region_id(region.id);
        let body = ui.interact(block, id, Sense::click_and_drag());
        body.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, &name));
        ui.ctx().accesskit_node_builder(id, |node| {
            node.set_role(Role::Button);
            node.set_label(name.as_str());
            node.set_selected(chosen);
            node.set_description(format!(
                "From {} to {}. Delete deletes it; its context menu sets the scale and the \
                 transposition.",
                shell
                    .editor()
                    .document()
                    .settings()
                    .time_signatures
                    .to_bbs(region.start),
                shell
                    .editor()
                    .document()
                    .settings()
                    .time_signatures
                    .to_bbs(region.end),
            ));
        });
        crate::focus::register_above(
            ui.ctx(),
            id,
            bl_shell::Area::MainView,
            crate::focus::ABOVE_LOCATORS,
        );
        // Selecting: a click, a drag's start, a right-click and the keyboard's
        // focus.
        if (body.clicked() || body.drag_started() || body.secondary_clicked() || body.has_focus())
            && !chosen
        {
            invoke(events, ids::SELECT_REGION, json!({ "region": region.id }));
        }
        if body.has_focus() && crate::menus::context_menu_keys(ui) {
            crate::menus::open_keyboard_context_menu(ui.ctx(), body.id, block.left_bottom());
        }
        body.context_menu(|ui| menu(ui, region, events));
        crate::menus::keyboard_context_menu(&body, |ui| menu(ui, region, events));

        // The body drags the region along.
        if body.drag_started()
            && let Some(press) = ui.input(|i| i.pointer.press_origin())
        {
            self.drag = Some(Drag::Move {
                region: region.id,
                grab: at_x(press) - region.start,
            });
        }
        if body.dragged()
            && let (
                Some(Drag::Move {
                    region: moving,
                    grab,
                }),
                Some(pos),
            ) = (self.drag, body.interact_pointer_pos())
            && moving == region.id
        {
            let start = snap(at_x(pos) - grab);
            *snap_line = Some(start);
            events.push(UiEvent::Gesture {
                phase: GesturePhase::Update,
                invocation: Some(Invocation::new(
                    ids::MOVE_REGION,
                    json!({
                        "region": region.id,
                        "start": start,
                        "end": start + (region.end - region.start),
                    }),
                )),
            });
        }
        if body.drag_stopped() && matches!(self.drag, Some(Drag::Move { .. })) {
            self.drag = None;
            events.push(UiEvent::Gesture {
                phase: GesturePhase::Commit,
                invocation: None,
            });
        }

        // The edges resize it.
        for end in [false, true] {
            let x = lane.left()
                + shell
                    .arrangement()
                    .horizontal(shell.editor().document())
                    .x(if end { region.end } else { region.start }) as f32;
            if x < lane.left() - EDGE || x > lane.right() + EDGE {
                continue;
            }
            let handle = Rect::from_min_max(
                pos2(if end { x - EDGE } else { x }, lane.top()),
                pos2(if end { x } else { x + EDGE }, lane.bottom()),
            )
            .intersect(lane);
            let edge = ui.interact(handle, edge_id(region.id, end), Sense::click_and_drag());
            edge.widget_info(|| {
                WidgetInfo::labeled(
                    WidgetType::Other,
                    true,
                    if end { "Region end" } else { "Region start" },
                )
            });
            if edge.hovered() || edge.dragged() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
            }
            if edge.secondary_clicked() && !chosen {
                invoke(events, ids::SELECT_REGION, json!({ "region": region.id }));
            }
            edge.context_menu(|ui| menu(ui, region, events));
            if edge.drag_started() {
                self.drag = Some(Drag::Edge {
                    region: region.id,
                    end,
                });
                if !chosen {
                    invoke(events, ids::SELECT_REGION, json!({ "region": region.id }));
                }
            }
            if edge.dragged()
                && let (
                    Some(Drag::Edge {
                        region: sizing,
                        end: sizing_end,
                    }),
                    Some(pos),
                ) = (self.drag, edge.interact_pointer_pos())
                && sizing == region.id
                && sizing_end == end
            {
                let at = snap(at_x(pos));
                *snap_line = Some(at);
                let (start, finish) = if end {
                    (region.start, at)
                } else {
                    (at, region.end)
                };
                // A drag past the other edge keeps the last length that works.
                if finish > start {
                    events.push(UiEvent::Gesture {
                        phase: GesturePhase::Update,
                        invocation: Some(Invocation::new(
                            ids::MOVE_REGION,
                            json!({ "region": region.id, "start": start, "end": finish }),
                        )),
                    });
                }
            }
            if edge.drag_stopped() && matches!(self.drag, Some(Drag::Edge { .. })) {
                self.drag = None;
                events.push(UiEvent::Gesture {
                    phase: GesturePhase::Commit,
                    invocation: None,
                });
            }
        }
    }
}

/// A region's context menu (R4): its scale, by root and by name or none,
/// its transposition by a step or an octave, and Delete.
fn menu(ui: &mut Ui, region: &ChordRegion, events: &mut Vec<UiEvent>) {
    let set = |events: &mut Vec<UiEvent>, ui: &mut Ui, args: serde_json::Value| {
        let mut args = args;
        args["region"] = json!(region.id);
        invoke(events, ids::SET_REGION, args);
        ui.close();
    };
    let tick = |on: bool, text: &str| {
        if on {
            format!("✔ {text}")
        } else {
            text.to_owned()
        }
    };
    if ui.button(tick(!region.scale.enabled, "No Scale")).clicked() {
        set(events, ui, json!({ "enabled": false }));
    }
    ui.menu_button("Root", |ui| {
        for root in 0..12u8 {
            let on = region.scale.enabled && region.scale.root == root;
            if ui
                .button(tick(on, bl_model::Scale::root_name(root)))
                .clicked()
            {
                set(events, ui, json!({ "enabled": true, "root": root }));
            }
        }
    });
    ui.menu_button("Scale", |ui| {
        for name in ScaleName::ALL {
            let on = region.scale.enabled && region.scale.name == name;
            if ui.button(tick(on, name.label())).clicked() {
                set(events, ui, json!({ "enabled": true, "name": name.id() }));
            }
        }
    });
    // The transposition moves by a step or an octave, from -24 to 24.
    ui.menu_button("Transposition", |ui| {
        let limit = i32::from(bl_model::MAX_TRANSPOSE);
        let now = i32::from(region.transpose);
        for (label, to) in [
            ("Up 1", now + 1),
            ("Down 1", now - 1),
            ("Up an Octave", now + 12),
            ("Down an Octave", now - 12),
            ("None", 0),
        ] {
            let allowed = to.abs() <= limit && to != now;
            if ui.add_enabled(allowed, egui::Button::new(label)).clicked() {
                set(events, ui, json!({ "transpose": to }));
            }
        }
    });
    ui.separator();
    if ui.button("Delete").clicked() {
        invoke(events, ids::DELETE_REGION, json!({ "region": region.id }));
        ui.close();
    }
}

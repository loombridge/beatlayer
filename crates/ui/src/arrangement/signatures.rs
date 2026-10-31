//! The time-signature lane's input (WP-6.61; the time editing spec, R29 to
//! R34): clicking, dragging and typing a marker's signature, its menu, and
//! the menu of a fragmentary bar. The lane is drawn by
//! [`super::rulers::signature_lane`].

use bl_core::time::Ticks;
use bl_shell::Shell;
use bl_shell::arrangement::snapping::Dragged;
use bl_shell::locator_commands as ids;
use bl_shell::{GesturePhase, Invocation};
use egui::accesskit::Role;
use egui::{Id, Key, Rect, Sense, Ui, WidgetInfo, WidgetType, pos2};
use serde_json::json;

use super::context_menu::ContextMenus;
use super::editing::EditKeys;
use super::invoke;
use crate::main_window::UiEvent;
use crate::theme::Theme;

/// How wide a marker's hit area and typing field are, from its line.
const MARKER_WIDTH: f32 = 34.0;

/// A marker's ID, in the lane.
pub fn signature_id(at: Ticks) -> Id {
    Id::new(("arrangement-time-signature", at.get()))
}

/// A fragmentary bar's ID, in the lane.
fn fragmentary_bar_id(start: Ticks) -> Id {
    Id::new(("arrangement-fragmentary-bar", start.get()))
}

/// A signature being typed over its marker.
#[derive(Clone, Debug)]
struct Typing {
    /// The marker's position.
    at: Ticks,
    /// The text typed so far, once the field has been shown.
    text: Option<String>,
    /// Whether the field has the keyboard.
    focused: bool,
}

fn typing_id() -> Id {
    Id::new("arrangement-time-signature-typing")
}

/// Starts typing a new signature for the marker at `at`, as Cmd-R does (the
/// time editing spec, R31).
pub fn start_edit(ctx: &egui::Context, at: Ticks) {
    ctx.data_mut(|d| {
        d.insert_temp(
            typing_id(),
            Some(Typing {
                at,
                text: None,
                focused: false,
            }),
        );
    });
}

fn typing(ctx: &egui::Context) -> Option<Typing> {
    ctx.data(|d| d.get_temp::<Option<Typing>>(typing_id()))
        .flatten()
}

/// What the lane keeps between frames.
#[derive(Debug, Default)]
pub(super) struct SignatureLane {
    /// The marker being dragged, by the position it had when the drag began.
    drag: Option<Ticks>,
}

impl SignatureLane {
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
        let view = shell.arrangement();
        let doc = shell.editor().document();
        let signatures = &doc.settings().time_signatures;
        let horizontal = view.horizontal(doc);
        let at_x = |pos: egui::Pos2| horizontal.at(f64::from(pos.x - lane.left()));
        // Esc abandons a marker drag, as it does a locator's.
        if ui.input(|i| i.key_pressed(Key::Escape)) && self.drag.take().is_some() {
            events.push(UiEvent::Gesture {
                phase: GesturePhase::Cancel,
                invocation: None,
            });
        }
        // A short bar's menu (C-TIME-024), under the markers.
        for bar in signatures.fragmentary_bars() {
            let (x0, x1) = (
                lane.left() + horizontal.x(bar.start) as f32,
                lane.left() + horizontal.x(bar.end) as f32,
            );
            let rect = Rect::from_min_max(
                pos2(x0.max(lane.left()), lane.top()),
                pos2(x1.min(lane.right()), lane.bottom()),
            );
            if rect.width() <= 0.0 {
                continue;
            }
            let id = fragmentary_bar_id(bar.start);
            let response = ui.interact(rect, id, Sense::click());
            if response.secondary_clicked() {
                menus.open_fragmentary_bar(bar.start);
            }
            response.context_menu(|ui| menus.show(ui, theme, shell, events));
            response.widget_info(|| WidgetInfo::labeled(WidgetType::Other, true, "Short bar"));
            ui.ctx().accesskit_node_builder(id, |node| {
                node.set_role(Role::Group);
                node.set_value(signatures.to_bbs(bar.start).to_string());
                node.set_description(format!(
                    "Cut short by the next change; it lacks {} ticks of a whole {} bar. Its menu \
                     deletes that time or completes the bar.",
                    bar.missing().get(),
                    bar.signature
                ));
            });
        }
        for change in signatures.changes() {
            let x = lane.left() + horizontal.x(change.at) as f32;
            let at = change.at;
            // The marker being dragged keeps its widget, which alone sees
            // the release.
            let dragging = self.drag.is_some() && view.selected_signature() == Some(at);
            if !dragging && (x < lane.left() - MARKER_WIDTH || x > lane.right()) {
                continue;
            }
            let rect = Rect::from_min_max(
                pos2(x - 2.0, lane.top()),
                pos2(x + MARKER_WIDTH, lane.bottom()),
            );
            let selected = view.selected_signature() == Some(at);
            // A marker dragged keeps its ID as the drag moves it.
            let origin = self.drag.filter(|_| selected).unwrap_or(at);
            let id = signature_id(origin);
            let response = ui.interact(rect, id, Sense::click_and_drag());
            if response.secondary_clicked() {
                invoke(events, ids::SELECT_SIGNATURE, json!({ "at": at }));
                menus.open_signature();
            } else if response.clicked() || response.drag_started() {
                // Spec: time-editing R31. Clicking a marker selects it.
                if !selected {
                    invoke(events, ids::SELECT_SIGNATURE, json!({ "at": at }));
                }
            }
            response.context_menu(|ui| menus.show(ui, theme, shell, events));
            let name = change.signature.to_string();
            response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, &name));
            ui.ctx().accesskit_node_builder(id, |node| {
                node.set_role(Role::Button);
                node.set_label(name.as_str());
                node.set_value(signatures.to_bbs(at).to_string());
                node.set_selected(selected);
            });
            crate::focus::register_above(
                ui.ctx(),
                id,
                bl_shell::Area::MainView,
                crate::focus::ABOVE_LOCATORS,
            );
            if response.has_focus() && !selected {
                invoke(events, ids::SELECT_SIGNATURE, json!({ "at": at }));
            }
            // Spec: time-editing R31. The signature being typed, over the
            // marker's: Return sets it and Esc drops it.
            if let Some(mut typed) = typing(ui.ctx()).filter(|t| t.at == at) {
                let text = typed.text.get_or_insert_with(|| name.clone());
                let edit = ui.put(
                    Rect::from_min_max(pos2(x + 1.0, lane.top()), pos2(x + 60.0, lane.bottom())),
                    egui::TextEdit::singleline(text)
                        .id(Id::new(("arrangement-time-signature-edit", at.get())))
                        .font(theme.small_font()),
                );
                if !typed.focused {
                    edit.request_focus();
                    typed.focused = true;
                }
                let (enter, escape) =
                    ui.input(|i| (i.key_pressed(Key::Enter), i.key_pressed(Key::Escape)));
                if edit.lost_focus() || enter || escape {
                    if enter || (edit.lost_focus() && !escape) {
                        events.push(UiEvent::Invoke(Invocation::new(
                            ids::EDIT_SIGNATURE,
                            json!({ "at": at, "signature": text }),
                        )));
                    }
                    ui.ctx()
                        .data_mut(|d| d.insert_temp(typing_id(), None::<Typing>));
                } else {
                    ui.ctx()
                        .data_mut(|d| d.insert_temp(typing_id(), Some(typed)));
                }
            }
            // Spec: time-editing R32. Dragging a marker moves it, snapping as
            // a clip's start does; the marker at 1.1.1 stays.
            if origin == Ticks::ZERO {
                continue;
            }
            if response.drag_started() {
                self.drag = Some(at);
            }
            if response.dragged()
                && self.drag == Some(origin)
                && let Some(pos) = response.interact_pointer_pos()
            {
                let snapped = view.snap_to_items(
                    doc,
                    at_x(pos),
                    Ticks::ZERO,
                    Dragged {
                        marker: Some(origin),
                        ..Dragged::default()
                    },
                    horizontal.ticks_per_point,
                    ui.input(|i| EditKeys::from(i.modifiers).bypass),
                );
                *snap_line = snapped.line;
                events.push(UiEvent::Gesture {
                    phase: GesturePhase::Update,
                    invocation: Some(Invocation::new(
                        ids::MOVE_SIGNATURE,
                        json!({ "at": origin, "to": snapped.at.max(Ticks(1)) }),
                    )),
                });
            }
            if response.drag_stopped() && self.drag.take() == Some(origin) {
                events.push(UiEvent::Gesture {
                    phase: GesturePhase::Commit,
                    invocation: None,
                });
            }
        }
        // A drag whose marker is gone never sees its release: end it once
        // the pointer is up.
        if self.drag.is_some() && ui.input(|i| !i.pointer.any_down()) {
            self.drag = None;
            events.push(UiEvent::Gesture {
                phase: GesturePhase::Commit,
                invocation: None,
            });
        }
    }
}

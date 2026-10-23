//! The detail area's top edge (detail-views R10, R18): dragging it sets the
//! Clip View's height, a drag that ends below the smallest height closes the
//! view, and a screen reader finds it as a splitter.
//!
//! Every frame of a drag runs `view.set_detail_height`, and a release that
//! closes a view runs the commands that hide it; none is an undo step.

use bl_shell::commands::{
    CLIP_VIEW_MIN_HEIGHT, DETAIL_GAP, DEVICE_VIEW_HEIGHT, VIEW_SET_DETAIL_HEIGHT,
    VIEW_TOGGLE_CLIP_VIEW, VIEW_TOGGLE_DEVICE_VIEW,
};
use bl_shell::{Area, Invocation, Shell};
use egui::accesskit::{Action, Role};
use egui::{CursorIcon, Id, Key, Pos2, Sense, Stroke, Ui, pos2};
use serde_json::json;

use crate::layout::ShellLayout;
use crate::main_window::UiEvent;
use crate::theme::Theme;

/// How far a screen reader's increment and decrement, and the arrow keys,
/// move the Clip View's height, in points (R18).
const STEP: f64 = 10.0;

/// A drag of the edge under way.
#[derive(Clone, Copy, Debug)]
struct Drag {
    /// How far below the detail area's top the pointer took hold.
    grab: f32,
    /// The Clip View's height before the drag, which a view closed by the
    /// drag opens at again.
    before: f64,
    /// Where the detail area's top would be at the pointer.
    top: f32,
}

/// The detail area's top edge between frames.
#[derive(Debug, Default)]
pub struct DetailEdge {
    drag: Option<Drag>,
}

fn invoke(events: &mut Vec<UiEvent>, id: &str, args: serde_json::Value) {
    events.push(UiEvent::Invoke(Invocation::new(id, args)));
}

impl DetailEdge {
    /// Draws the edge in the gap above the detail area and runs its gestures.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        theme: &Theme,
        shell: &Shell,
        layout: &ShellLayout,
        events: &mut Vec<UiEvent>,
    ) {
        let detail = layout.detail;
        if detail.height() <= 0.0 {
            self.drag = None;
            return;
        }
        let strip = egui::Rect::from_min_max(
            pos2(detail.left(), detail.top() - DETAIL_GAP),
            pos2(detail.right(), detail.top()),
        );
        let id = Id::new("detail-edge");
        let response = ui
            .interact(strip, id, Sense::drag())
            .on_hover_cursor(CursorIcon::ResizeVertical);
        crate::focus::register(ui.ctx(), id, Area::Detail, None);
        let height = f64::from(layout.clip_view.map_or(0.0, |clip| clip.height()));
        if layout.clip_view.is_some() {
            ui.ctx().accesskit_node_builder(id, |node| {
                node.set_role(Role::Splitter);
                node.set_label("Clip View Height".to_owned());
                node.set_numeric_value(height);
                node.set_min_numeric_value(f64::from(CLIP_VIEW_MIN_HEIGHT));
                node.set_max_numeric_value(f64::from(layout.largest_clip_view));
                node.add_action(Action::Increment);
                node.add_action(Action::Decrement);
            });
            let focused = response.has_focus();
            let step = ui.input(|i| {
                let keys = f64::from(i.key_pressed(Key::ArrowUp) as i8)
                    - f64::from(i.key_pressed(Key::ArrowDown) as i8);
                i.num_accesskit_action_requests(id, Action::Increment) as f64
                    - i.num_accesskit_action_requests(id, Action::Decrement) as f64
                    + if focused { keys } else { 0.0 }
            });
            if step != 0.0 {
                invoke(
                    events,
                    VIEW_SET_DETAIL_HEIGHT,
                    json!({ "points": (height + step * STEP).min(f64::from(layout.largest_clip_view)) }),
                );
            }
        }
        if response.drag_started()
            && let Some(pointer) = response.interact_pointer_pos()
        {
            self.drag = Some(Drag {
                grab: pointer.y - detail.top(),
                before: shell.detail().clip_height(),
                top: detail.top(),
            });
        }
        if response.dragged()
            && let Some(pointer) = response.interact_pointer_pos()
            && let Some(drag) = &mut self.drag
        {
            drag.top = pointer.y - drag.grab;
            if let Some(clip) = layout.clip_view {
                let points = f64::from((clip.bottom() - drag.top).min(layout.largest_clip_view));
                invoke(events, VIEW_SET_DETAIL_HEIGHT, json!({ "points": points }));
            }
        }
        if response.drag_stopped()
            && let Some(drag) = self.drag.take()
        {
            self.release(shell, layout, drag, events);
        }
        if response.hovered() || response.dragged() {
            ui.painter().line_segment(
                [
                    Pos2::new(detail.left(), detail.top() - DETAIL_GAP / 2.0),
                    Pos2::new(detail.right(), detail.top() - DETAIL_GAP / 2.0),
                ],
                Stroke::new(2.0, theme.colors.focus.0),
            );
        }
    }

    /// A drag that ends lower than the Clip View's smallest height closes
    /// the Clip View, which opens again at the height it had; with only the
    /// Device View showing, one that ends lower than half its height closes
    /// it (R10).
    fn release(&self, shell: &Shell, layout: &ShellLayout, drag: Drag, events: &mut Vec<UiEvent>) {
        if let Some(clip) = layout.clip_view {
            if clip.bottom() - drag.top < CLIP_VIEW_MIN_HEIGHT {
                invoke(
                    events,
                    VIEW_SET_DETAIL_HEIGHT,
                    json!({ "points": drag.before }),
                );
                invoke(events, VIEW_TOGGLE_CLIP_VIEW, json!({}));
            }
        } else if shell.detail().device_shown()
            && drag.top - layout.detail.top() > DEVICE_VIEW_HEIGHT / 2.0
        {
            invoke(events, VIEW_TOGGLE_DEVICE_VIEW, json!({}));
        }
    }
}

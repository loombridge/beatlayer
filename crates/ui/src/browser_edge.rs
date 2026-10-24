//! The browser's right edge (browser R29; I-DRAG-027): dragging it sets the
//! browser's width, a drag that goes left of the middle of the narrowest
//! browser hides it and one that comes back shows it again, and a screen
//! reader finds it as a splitter.
//!
//! Every frame of a drag runs `view.set_browser_width`; hiding and showing
//! run `view.toggle_browser`. None is an undo step.

use bl_shell::commands::{
    BROWSER_MIN_WIDTH, VIEW_SET_BROWSER_WIDTH, VIEW_TOGGLE_BROWSER, clamp_browser_width,
};
use bl_shell::{Area, Invocation, Shell};
use egui::accesskit::{Action, Role};
use egui::{CursorIcon, Id, Key, Pos2, Rect, Sense, Stroke, Ui, pos2};
use serde_json::json;

use crate::layout::{MARGIN, ShellLayout};
use crate::main_window::UiEvent;
use crate::theme::Theme;

/// How far a screen reader's increment and decrement, and the arrow keys,
/// move the browser's edge, in points.
const STEP: f32 = 10.0;

/// A drag of the edge under way.
#[derive(Clone, Copy, Debug)]
struct Drag {
    /// How far right of the browser's right edge the pointer took hold.
    grab: f32,
    /// The browser's left edge, which stays put.
    left: f32,
    /// The browser's top and bottom, where the edge is while it's hidden.
    top: f32,
    bottom: f32,
}

/// The browser's right edge between frames.
#[derive(Debug, Default)]
pub struct BrowserEdge {
    drag: Option<Drag>,
}

fn invoke(events: &mut Vec<UiEvent>, id: &str, args: serde_json::Value) {
    events.push(UiEvent::Invoke(Invocation::new(id, args)));
}

impl BrowserEdge {
    /// Takes the gap right of the browser and runs its gestures.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        theme: &Theme,
        shell: &Shell,
        layout: &ShellLayout,
        events: &mut Vec<UiEvent>,
    ) {
        let shown = layout.browser;
        let strip = match (shown, self.drag) {
            (Some(browser), _) => Rect::from_min_max(
                pos2(browser.right(), browser.top()),
                pos2(browser.right() + MARGIN, browser.bottom()),
            ),
            // Hidden by this drag: the edge waits at the window's left
            // until the pointer brings the browser back.
            (None, Some(drag)) => Rect::from_min_max(
                pos2(drag.left - MARGIN, drag.top),
                pos2(drag.left, drag.bottom),
            ),
            (None, None) => return,
        };
        let id = Id::new("browser-edge");
        let response = ui
            .interact(strip, id, Sense::drag())
            .on_hover_cursor(CursorIcon::ResizeHorizontal);
        crate::focus::register(ui.ctx(), id, Area::Browser, None);
        let width = shell.browser_view().width();
        if let Some(browser) = shown {
            ui.ctx().accesskit_node_builder(id, |node| {
                node.set_role(Role::Splitter);
                node.set_label("Browser Width".to_owned());
                node.set_numeric_value(f64::from(browser.width()));
                node.set_min_numeric_value(f64::from(BROWSER_MIN_WIDTH));
                node.set_max_numeric_value(f64::from(layout.largest_browser));
                node.add_action(Action::Increment);
                node.add_action(Action::Decrement);
            });
            let focused = response.has_focus();
            let step = ui.input(|i| {
                let keys = f32::from(i8::from(i.key_pressed(Key::ArrowRight)))
                    - f32::from(i8::from(i.key_pressed(Key::ArrowLeft)));
                i.num_accesskit_action_requests(id, Action::Increment) as f32
                    - i.num_accesskit_action_requests(id, Action::Decrement) as f32
                    + if focused { keys } else { 0.0 }
            });
            if step != 0.0 {
                let points = (browser.width() + step * STEP).min(layout.largest_browser);
                invoke(events, VIEW_SET_BROWSER_WIDTH, json!({ "points": points }));
            }
            if response.drag_started()
                && let Some(pointer) = response.interact_pointer_pos()
            {
                self.drag = Some(Drag {
                    grab: pointer.x - browser.right(),
                    left: browser.left(),
                    top: browser.top(),
                    bottom: browser.bottom(),
                });
            }
        }
        if response.dragged()
            && let Some(pointer) = response.interact_pointer_pos()
            && let Some(drag) = self.drag
        {
            let wanted = pointer.x - drag.grab - drag.left;
            // Spec: browser R29. Left of the middle of the narrowest
            // browser, it hides, at its narrowest; back past it, it shows.
            let hide = wanted < BROWSER_MIN_WIDTH / 2.0;
            if hide && shown.is_some() {
                if width > BROWSER_MIN_WIDTH {
                    invoke(
                        events,
                        VIEW_SET_BROWSER_WIDTH,
                        json!({ "points": BROWSER_MIN_WIDTH }),
                    );
                }
                invoke(events, VIEW_TOGGLE_BROWSER, json!({}));
            } else if !hide {
                if shown.is_none() {
                    invoke(events, VIEW_TOGGLE_BROWSER, json!({}));
                }
                let points = clamp_browser_width(wanted).min(layout.largest_browser);
                if (points - width).abs() > f32::EPSILON {
                    invoke(events, VIEW_SET_BROWSER_WIDTH, json!({ "points": points }));
                }
            }
        }
        if response.drag_stopped() {
            self.drag = None;
        }
        if response.hovered() || response.dragged() {
            let x = strip.center().x;
            ui.painter().line_segment(
                [Pos2::new(x, strip.top()), Pos2::new(x, strip.bottom())],
                Stroke::new(2.0, theme.colors.focus.0),
            );
        }
    }
}

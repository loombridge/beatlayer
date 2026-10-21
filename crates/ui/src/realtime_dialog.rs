//! The real-time render's dialog (WP-5.35; the external devices spec,
//! R23): the waiting period before a render that runs through external
//! hardware, with Restart and Skip, and the render's progress with the
//! number of attempts, Auto-Restart on Drop-Outs and Cancel.

use bl_shell::Shell;
use bl_shell::export_commands::{self as ids, RealtimePhase, RealtimeStatus};
use egui::{Context, Id, Sense, Ui, Vec2};
use serde_json::json;

use crate::main_window::UiEvent;
use crate::theme::Theme;

fn invoke(events: &mut Vec<UiEvent>, id: &str, args: serde_json::Value) {
    events.push(UiEvent::Invoke(bl_shell::Invocation::new(id, args)));
}

/// Draws the dialog while a real-time render is under way.
pub fn show(ctx: &Context, shell: &Shell, theme: &Theme, events: &mut Vec<UiEvent>) {
    let status = shell.realtime_status();
    if status.phase == RealtimePhase::Idle {
        return;
    }
    egui::Modal::new(Id::new("realtime-dialog")).show(ctx, |ui| {
        ui.set_width(340.0);
        ui.vertical_centered(|ui| {
            ui.label(
                egui::RichText::new("Real-time Rendering")
                    .size(theme.sizes.heading)
                    .strong(),
            );
        });
        ui.add_space(8.0);
        match status.phase {
            RealtimePhase::Waiting { seconds } => waiting(ui, seconds, events),
            RealtimePhase::Rendering => rendering(ui, theme, &status, events),
            RealtimePhase::Idle => {}
        }
    });
    // The render's progress moves on its own, so the dialog is redrawn
    // while it runs.
    ctx.request_repaint_after(std::time::Duration::from_millis(100));
}

/// The waiting period, counting down, with Restart and Skip (R23).
fn waiting(ui: &mut Ui, seconds: f64, events: &mut Vec<UiEvent>) {
    ui.label("Waiting for the hardware's sound to die away.");
    ui.add_space(6.0);
    ui.label(format!("{seconds:.0} seconds"));
    ui.add_space(12.0);
    ui.horizontal(|ui| {
        if ui.button("Restart").clicked() {
            invoke(events, ids::REALTIME_RESTART, json!({}));
        }
        if ui.button("Skip").clicked() {
            invoke(events, ids::REALTIME_SKIP, json!({}));
        }
        if ui.button("Cancel").clicked() {
            invoke(events, ids::REALTIME_CANCEL, json!({}));
        }
    });
}

/// The render's progress, with the attempts, Auto-Restart on Drop-Outs and
/// Cancel (R23).
fn rendering(ui: &mut Ui, theme: &Theme, status: &RealtimeStatus, events: &mut Vec<UiEvent>) {
    ui.label("Rendering in real time.");
    ui.add_space(6.0);
    bar(ui, theme, status.progress as f32, "Rendering");
    ui.label(format!("{:.0}%", status.progress * 100.0));
    if status.attempts > 1 {
        ui.label(format!("Attempt {}", status.attempts));
    }
    ui.add_space(6.0);
    let mut auto_restart = status.auto_restart;
    if ui
        .checkbox(&mut auto_restart, "Auto-Restart on Drop-Outs")
        .changed()
    {
        invoke(
            events,
            ids::REALTIME_AUTO_RESTART,
            json!({ "auto_restart": auto_restart }),
        );
    }
    ui.add_space(12.0);
    if ui.button("Cancel").clicked() {
        invoke(events, ids::REALTIME_CANCEL, json!({}));
    }
}

/// The render's progress bar.
fn bar(ui: &mut Ui, theme: &Theme, fraction: f32, name: &str) {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 10.0), Sense::hover());
    let painter = ui.painter_at(rect);
    let c = &theme.colors;
    painter.rect_filled(rect, theme.sizes.corner, c.control.0);
    let mut filled = rect;
    filled.set_width(rect.width() * fraction.clamp(0.0, 1.0));
    painter.rect_filled(filled, theme.sizes.corner, c.accent.0);
    let id = response.id;
    let value = f64::from(fraction.clamp(0.0, 1.0)) * 100.0;
    ui.ctx().accesskit_node_builder(id, |node| {
        node.set_role(egui::accesskit::Role::ProgressIndicator);
        node.set_label(name.to_owned());
        node.set_numeric_value(value);
        node.set_min_numeric_value(0.0);
        node.set_max_numeric_value(100.0);
    });
}

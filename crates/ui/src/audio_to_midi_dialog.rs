//! The window of Convert Harmony, Melody and Drums to New MIDI Track (IMP-06;
//! the audio-to-midi spec, R2, R3, R25): a modal over the main window that
//! shows the package page the first time Harmony or Melody runs without its
//! packages, the download's progress, and the conversion's progress with
//! Cancel. The shell keeps its step (`audio_to_midi.status`'s `dialog`).

use bl_shell::Shell;
use bl_shell::audio_to_midi_commands::{self as ids, ConvertDialogStep};
use bl_shell::stems_commands::JobStatus;
use egui::accesskit::Role;
use egui::{Context, Id};
use serde_json::json;

use crate::main_window::UiEvent;
use crate::stems_dialog::bar;
use crate::theme::Theme;

fn invoke(events: &mut Vec<UiEvent>, id: &str) {
    events.push(UiEvent::Invoke(bl_shell::Invocation::new(id, json!({}))));
}

/// Draws the window if it's open.
pub fn show(ctx: &Context, shell: &Shell, theme: &Theme, events: &mut Vec<UiEvent>) {
    let Some(dialog) = shell.audio_to_midi_dialog() else {
        return;
    };
    let status = shell.audio_to_midi_status();
    let name = dialog.kind.command();
    let missing: Vec<&str> = status
        .packages
        .iter()
        .filter(|p| !p.installed)
        .map(|p| p.id.as_str())
        .collect();
    egui::Modal::new(Id::new("audio-to-midi-window")).show(ctx, |ui| {
        ui.set_width(if dialog.step == ConvertDialogStep::Packages {
            460.0
        } else {
            320.0
        });
        ui.vertical_centered(|ui| {
            ui.label(egui::RichText::new(name).size(theme.sizes.heading).strong());
        });
        ui.add_space(8.0);
        match dialog.step {
            ConvertDialogStep::Packages => crate::packages_page::show(
                ui,
                theme,
                &crate::packages_page::Page {
                    feature: "Audio to MIDI",
                    intro: None,
                    packages: &status.packages,
                    folder: &status.folder,
                    install: &status.install,
                    install_command: "packages.install",
                    install_args: &json!({ "ids": missing }),
                    close_command: ids::CANCEL,
                },
                events,
            ),
            ConvertDialogStep::Installing => progress(
                ui,
                theme,
                &status.install,
                "Downloading and installing the packages",
                events,
            ),
            ConvertDialogStep::Converting => progress(ui, theme, &status.conversion, name, events),
        }
    });
    // Spec: audio-to-midi R25. The window reaches assistive technology as a
    // dialog named after the command.
    ctx.accesskit_node_builder(Id::new("audio-to-midi-dialog"), |node| {
        node.set_role(Role::Dialog);
        node.set_label(name);
    });
    // Spec: audio-to-midi R3. Esc is Cancel.
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        invoke(events, ids::CANCEL);
    }
    if dialog.step != ConvertDialogStep::Packages {
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }
}

/// A job's progress: what it's doing, a bar of its share, and Cancel (R3).
fn progress(
    ui: &mut egui::Ui,
    theme: &Theme,
    job: &JobStatus,
    what: &str,
    events: &mut Vec<UiEvent>,
) {
    ui.label(if job.stage.is_empty() {
        what
    } else {
        &job.stage
    });
    ui.add_space(6.0);
    bar(ui, theme, job.progress, what);
    ui.label(
        egui::RichText::new(format!("{:.0}%", job.progress * 100.0))
            .font(theme.small_font())
            .color(theme.colors.text_dim.0),
    );
    ui.add_space(10.0);
    ui.vertical_centered(|ui| {
        if ui.button("Cancel").clicked() {
            invoke(events, ids::CANCEL);
        }
    });
}

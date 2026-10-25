//! DAWproject in the main window: a `.dawproject` file dropped on it, and the
//! window that lists the report of an import or export (the DAWproject spec,
//! R85 and R87).

use bl_shell::Shell;
use bl_shell::dawproject_commands::{CLOSE_REPORT, IMPORT_DAWPROJECT, ImportReport};
use egui::accesskit::Role;
use egui::{Context, Id, Key, Rect, Sense, Ui, vec2};
use serde_json::json;

use crate::main_window::UiEvent;
use crate::theme::Theme;

/// A `.dawproject` file dropped on the main window is imported, whichever view
/// it lands on (R85, I-DROP-025). It is taken out of the frame's dropped
/// files first, so no view places it as a clip; call this before they draw.
pub fn drop_file(ctx: &Context, events: &mut Vec<UiEvent>) {
    let files: Vec<String> = ctx.input_mut(|i| {
        let (taken, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut i.raw.dropped_files)
            .into_iter()
            .partition(|file| {
                file.path()
                    .extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("dawproject"))
            });
        i.raw.dropped_files = kept;
        taken
            .iter()
            .map(|file| file.path().to_string_lossy().into_owned())
            .collect()
    });
    // One set opens at a time: the first file dropped.
    if let Some(file) = files.into_iter().next() {
        events.push(UiEvent::Invoke(bl_shell::Invocation::new(
            IMPORT_DAWPROJECT,
            json!({ "path": file }),
        )));
    }
}

/// Draws the report window if it is open (R87): the entries under Left Out
/// and Changed, Copy and Close. Esc closes it.
pub fn show(ctx: &Context, shell: &Shell, theme: &Theme, events: &mut Vec<UiEvent>) {
    let Some(window) = shell.dawproject_report_window() else {
        return;
    };
    let close = |events: &mut Vec<UiEvent>| {
        events.push(UiEvent::Invoke(bl_shell::Invocation::new(
            CLOSE_REPORT,
            json!({}),
        )));
    };
    egui::Modal::new(Id::new("dawproject-report")).show(ctx, |ui| {
        ui.set_width(460.0);
        ui.vertical_centered(|ui| {
            ui.label(
                egui::RichText::new(window.title())
                    .size(theme.sizes.heading)
                    .strong(),
            );
        });
        ui.add_space(8.0);
        if let Some(application) = &window.report.application {
            ui.label(format!("Made by {application}"));
            ui.add_space(4.0);
        }
        egui::ScrollArea::vertical()
            .max_height(320.0)
            .show(ui, |ui| {
                for (heading, left_out) in [("Left Out", true), ("Changed", false)] {
                    list(ui, theme, &window.report, heading, left_out);
                }
            });
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            if ui.button("Copy").clicked() {
                ui.ctx().copy_text(window.report.text());
            }
            if ui.button("Close").clicked() {
                close(events);
            }
        });
    });
    if ctx.input(|i| i.key_pressed(Key::Escape)) {
        close(events);
    }
}

/// One section of the report, read by assistive technology as a list of its
/// entries; nothing is drawn for a section without entries.
fn list(ui: &mut Ui, theme: &Theme, report: &ImportReport, heading: &str, left_out: bool) {
    let entries: Vec<_> = report
        .entries
        .iter()
        .filter(|entry| entry.is_left_out() == left_out)
        .collect();
    if entries.is_empty() {
        return;
    }
    ui.add_space(4.0);
    let title = ui.label(
        egui::RichText::new(heading)
            .font(theme.small_font())
            .strong(),
    );
    let list = ui.interact(
        Rect::from_min_size(title.rect.min, vec2(1.0, 1.0)),
        Id::new(("dawproject-report-list", heading)),
        Sense::hover(),
    );
    ui.ctx().accesskit_node_builder(list.id, |node| {
        node.set_role(Role::List);
        node.set_label(heading.to_owned());
    });
    for entry in entries {
        let item = ui.label(entry.line());
        ui.ctx().accesskit_node_builder(item.id, |node| {
            node.set_role(Role::ListItem);
            node.set_label(entry.line());
        });
    }
}

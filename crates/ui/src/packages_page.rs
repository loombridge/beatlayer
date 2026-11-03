//! The package page every on-device feature shows the first time its command
//! runs without its packages (the stem separation spec, R2 and R16): what
//! will be downloaded, from where, how big, and where it goes. A feature's
//! dialog draws it with its own name and its own install and close
//! commands.

use bl_shell::package_commands::PackageStatus;
use bl_shell::{Invocation, stems_commands::JobStatus};
use egui::{Ui, Vec2};
use serde_json::json;

use crate::main_window::UiEvent;
use crate::theme::Theme;

/// What a feature's package page shows.
pub struct Page<'a> {
    /// The feature, as the sentence names it: "Stem separation".
    pub feature: &'a str,
    /// The packages the feature needs.
    pub packages: &'a [PackageStatus],
    /// The folder they install in.
    pub folder: &'a str,
    /// The last download's outcome, for its reason if it failed.
    pub install: &'a JobStatus,
    /// The command the button runs.
    pub install_command: &'a str,
    /// The command Cancel runs.
    pub close_command: &'a str,
}

fn invoke(events: &mut Vec<UiEvent>, id: &str) {
    events.push(UiEvent::Invoke(Invocation::new(id, json!({}))));
}

/// Draws the page: the packages, the total, the folder, and Download
/// Required Packages Now or Cancel.
pub fn show(ui: &mut Ui, theme: &Theme, page: &Page<'_>, events: &mut Vec<UiEvent>) {
    ui.label(format!(
        "{} runs on this computer, with open-source software that doesn't come \
         with Beatlayer. These packages need to be downloaded once:",
        page.feature
    ));
    ui.add_space(6.0);
    let total: u64 = page
        .packages
        .iter()
        .filter(|p| !p.installed)
        .map(|p| p.size)
        .sum();
    for package in page.packages {
        ui.horizontal(|ui| {
            let size = if package.installed {
                "Installed".to_owned()
            } else {
                megabytes(package.size)
            };
            let name_width = ui.available_width() - 80.0;
            ui.allocate_ui_with_layout(
                Vec2::new(name_width, 0.0),
                egui::Layout::top_down(egui::Align::LEFT),
                |ui| {
                    ui.set_width(name_width);
                    ui.hyperlink_to(
                        format!("{} {}", package.name, package.version),
                        &package.url,
                    );
                    ui.label(
                        egui::RichText::new(&package.purpose)
                            .font(theme.small_font())
                            .color(theme.colors.text_dim.0),
                    );
                    // Spec: stem-separation R19.
                    if package.at_next_launch {
                        ui.label(
                            egui::RichText::new("Takes effect the next time Beatlayer starts")
                                .font(theme.small_font())
                                .color(theme.colors.text_dim.0),
                        );
                    }
                },
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(size);
            });
        });
        ui.add_space(4.0);
    }
    ui.add_space(6.0);
    ui.label(format!(
        "{} in all. They'll be installed in:",
        megabytes(total)
    ));
    ui.label(
        egui::RichText::new(page.folder)
            .font(theme.small_font())
            .monospace(),
    );
    if let Some(error) = &page.install.error {
        ui.add_space(6.0);
        ui.colored_label(theme.colors.error_text.0, error);
    }
    ui.add_space(12.0);
    ui.horizontal(|ui| {
        if ui.button("Download Required Packages Now").clicked() {
            invoke(events, page.install_command);
        }
        if ui.button("Cancel").clicked() {
            invoke(events, page.close_command);
        }
    });
}

fn megabytes(bytes: u64) -> String {
    format!("{:.0} MB", bytes as f64 / 1_000_000.0)
}

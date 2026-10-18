//! Separate Stems to New Audio Tracks' dialog (WP-9.1; the stem separation
//! spec, R1 to R7): a modal over the main window, in four steps the shell
//! keeps (`stems.status`'s `dialog`).
//!
//! - **Packages**, the first time: what stem separation downloads, with
//!   each package's project page and size and where it installs, and
//!   Download Required Packages Now or Cancel.
//! - **Installing**: the download's progress, and Cancel.
//! - **Options**: Vocals, Drums, Bass and Others, Merge to Single Track,
//!   High Speed or High Quality, and Separate or Cancel, as Live's.
//! - **Separating**: the separation's progress, and Cancel.
//!
//! The switches are the dialog's own until Separate sends them in
//! `stems.separate`.

use bl_shell::Shell;
use bl_shell::stems_commands::{
    self as ids, DialogStep, JobStatus, StemKind, StemQuality, StemsStatus,
};
use egui::{Align2, Color32, Context, Id, Rect, Sense, Stroke, StrokeKind, Ui, Vec2};
use serde_json::json;

use crate::main_window::UiEvent;
use crate::theme::Theme;

/// The Options step's switches, kept between frames.
#[derive(Clone, Debug)]
struct Choices {
    stems: [bool; 4],
    merge: bool,
    quality: StemQuality,
}

impl Default for Choices {
    fn default() -> Self {
        Self {
            stems: [true; 4],
            merge: false,
            quality: StemQuality::HighSpeed,
        }
    }
}

fn choices_id() -> Id {
    Id::new("stems-dialog-choices")
}

fn invoke(events: &mut Vec<UiEvent>, id: &str, args: serde_json::Value) {
    events.push(UiEvent::Invoke(bl_shell::Invocation::new(id, args)));
}

/// Each part's swatch: Beatlayer's own track colours.
fn swatch(theme: &Theme, kind: StemKind) -> Color32 {
    let palette = &theme.colors.track_palette;
    let index = match kind {
        StemKind::Vocals => 10,
        StemKind::Drums => 2,
        StemKind::Bass => 6,
        StemKind::Others => 4,
    };
    palette
        .get(index % palette.len().max(1))
        .map_or(theme.colors.accent.0, |c| c.0)
}

/// Draws the dialog if it's open.
pub fn show(ctx: &Context, shell: &Shell, theme: &Theme, events: &mut Vec<UiEvent>) {
    let Some(dialog) = shell.stems_dialog() else {
        return;
    };
    let status = shell.stems_status();
    let width = match dialog.step {
        DialogStep::Packages => 460.0,
        _ => 300.0,
    };
    egui::Modal::new(Id::new("stems-dialog")).show(ctx, |ui| {
        ui.set_width(width);
        ui.vertical_centered(|ui| {
            ui.label(
                egui::RichText::new("Separate Stems")
                    .size(theme.sizes.heading)
                    .strong(),
            );
        });
        ui.add_space(8.0);
        match dialog.step {
            DialogStep::Packages => packages(ui, theme, &status, events),
            DialogStep::Installing => progress(
                ui,
                theme,
                &status.install,
                "Downloading and installing the packages",
                events,
            ),
            DialogStep::Options => options(ui, theme, dialog.clip, events),
            DialogStep::Separating => progress(
                ui,
                theme,
                &status.separation,
                "Separating the clip into stems",
                events,
            ),
        }
    });
    // Esc is Cancel, except while work runs: Cancel there stops it, so it
    // takes the button. A click outside does nothing, as the dialog is
    // modal.
    let escape = ctx.input(|i| i.key_pressed(egui::Key::Escape));
    if escape && matches!(dialog.step, DialogStep::Packages | DialogStep::Options) {
        invoke(events, ids::CLOSE, json!({}));
    }
    if matches!(dialog.step, DialogStep::Installing | DialogStep::Separating) {
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }
}

/// The first time (R2, R3): what's downloaded, from where, how big, and
/// where it goes.
fn packages(ui: &mut Ui, theme: &Theme, status: &StemsStatus, events: &mut Vec<UiEvent>) {
    ui.label(
        "Stem separation runs on this computer, with open-source software that doesn't come \
         with Beatlayer. These packages need to be downloaded once:",
    );
    ui.add_space(6.0);
    let total: u64 = status
        .packages
        .iter()
        .filter(|p| !p.installed)
        .map(|p| p.size)
        .sum();
    for package in &status.packages {
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
        egui::RichText::new(&status.folder)
            .font(theme.small_font())
            .monospace(),
    );
    if let Some(error) = &status.install.error {
        ui.add_space(6.0);
        ui.colored_label(theme.colors.error_text.0, error);
    }
    ui.add_space(12.0);
    ui.horizontal(|ui| {
        if ui.button("Download Required Packages Now").clicked() {
            invoke(events, ids::INSTALL, json!({}));
        }
        if ui.button("Cancel").clicked() {
            invoke(events, ids::CLOSE, json!({}));
        }
    });
}

fn megabytes(bytes: u64) -> String {
    format!("{:.0} MB", bytes as f64 / 1_000_000.0)
}

/// A download's or a separation's progress, with Cancel (R4, R12).
fn progress(ui: &mut Ui, theme: &Theme, job: &JobStatus, what: &str, events: &mut Vec<UiEvent>) {
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
            invoke(events, ids::CANCEL, json!({}));
        }
    });
}

/// A progress bar, painted, read by a screen reader as a progress
/// indicator.
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

/// The parts and the quality (R5 to R7), as Live's dialog lays them out.
fn options(ui: &mut Ui, theme: &Theme, clip: bl_model::ClipId, events: &mut Vec<UiEvent>) {
    let mut choices: Choices = ui
        .ctx()
        .data(|d| d.get_temp(choices_id()))
        .unwrap_or_default();
    for (index, kind) in StemKind::ALL.into_iter().enumerate() {
        if part_row(ui, theme, kind, choices.stems[index]) {
            choices.stems[index] = !choices.stems[index];
        }
    }
    let chosen = choices.stems.iter().filter(|on| **on).count();
    // Merge to Single Track works on two or three parts (R5).
    let can_merge = (2..=3).contains(&chosen);
    ui.add_space(4.0);
    ui.add_enabled_ui(can_merge, |ui| {
        let mut merge = choices.merge && can_merge;
        if ui.checkbox(&mut merge, "Merge to Single Track").changed() {
            choices.merge = merge;
        }
    });
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.label("High Speed");
        let high = choices.quality == StemQuality::HighQuality;
        if toggle(ui, theme, high, "Quality: High Speed or High Quality") {
            choices.quality = if high {
                StemQuality::HighSpeed
            } else {
                StemQuality::HighQuality
            };
        }
        ui.label("High Quality");
    });
    ui.separator();
    // R6: playback stops when the separation starts.
    ui.label("This action will stop audio.");
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        let separate = ui.add_enabled(chosen > 0, egui::Button::new("Separate"));
        if separate.clicked() {
            let stems: Vec<StemKind> = StemKind::ALL
                .into_iter()
                .zip(choices.stems)
                .filter_map(|(kind, on)| on.then_some(kind))
                .collect();
            invoke(
                events,
                ids::SEPARATE,
                json!({
                    "clip": clip,
                    "stems": stems,
                    "quality": choices.quality,
                    "merge": choices.merge && can_merge,
                }),
            );
        }
        if ui.button("Cancel").clicked() {
            invoke(events, ids::CLOSE, json!({}));
        }
    });
    ui.ctx().data_mut(|d| d.insert_temp(choices_id(), choices));
}

/// One part's row: its swatch, its name, and a check mark when it's
/// chosen. Returns whether it was clicked.
fn part_row(ui: &mut Ui, theme: &Theme, kind: StemKind, on: bool) -> bool {
    let height = 26.0;
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::click());
    let c = &theme.colors;
    let painter = ui.painter_at(rect);
    let fill = if response.hovered() {
        c.control_hover.0
    } else {
        c.control.0
    };
    painter.rect_filled(rect.shrink2(Vec2::new(0.0, 1.0)), theme.sizes.corner, fill);
    let square = Rect::from_min_size(
        rect.left_center() - Vec2::new(0.0, height / 2.0 - 2.0),
        Vec2::splat(height - 4.0),
    );
    painter.rect_filled(square, theme.sizes.corner, swatch(theme, kind));
    painter.text(
        square.right_center() + Vec2::new(10.0, 0.0),
        Align2::LEFT_CENTER,
        kind.name(),
        theme.font(),
        c.text.0,
    );
    if on {
        painter.text(
            rect.right_center() - Vec2::new(10.0, 0.0),
            Align2::RIGHT_CENTER,
            "✓",
            theme.font(),
            c.text.0,
        );
    }
    let name = kind.name();
    response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, on, name));
    if response.has_focus() && crate::focus::outlined(&response.ctx) {
        painter.rect_stroke(
            rect,
            theme.sizes.corner,
            Stroke::new(1.5, c.focus.0),
            StrokeKind::Inside,
        );
    }
    response.clicked()
}

/// A two-way switch: off to the left, on to the right. Returns whether it
/// was clicked.
fn toggle(ui: &mut Ui, theme: &Theme, on: bool, name: &str) -> bool {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(36.0, 18.0), Sense::click());
    let c = &theme.colors;
    let painter = ui.painter_at(rect.expand(2.0));
    painter.rect_filled(rect, rect.height() / 2.0, c.control.0);
    let x = if on {
        rect.right() - rect.height() / 2.0
    } else {
        rect.left() + rect.height() / 2.0
    };
    painter.circle_filled(
        egui::pos2(x, rect.center().y),
        rect.height() / 2.0 - 2.0,
        c.accent.0,
    );
    let name = name.to_owned();
    response
        .widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, on, &name));
    response.clicked()
}

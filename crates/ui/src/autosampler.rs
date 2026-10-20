//! The autosampler's dialog (WP-5.16, IMP-27; the multisampler spec, R64,
//! R65, R73): the settings, how many samples they make and how long the
//! render takes, and the run's progress with Cancel.

use bl_shell::autosample::{Settings, plan};
use bl_shell::autosampler_commands::{self as ids, AutosampleArgs};
use bl_shell::{AutosampleStatus, Shell};
use egui::{Context, Id, Ui};
use serde_json::json;

use crate::main_window::UiEvent;
use crate::theme::Theme;

/// The settings the dialog holds, kept between the frames it's open for.
#[derive(Clone, Debug)]
struct Choices {
    lowest: u8,
    highest: u8,
    step: u8,
    velocity_layers: u8,
    note_length: f64,
    release_tail: f64,
    round_roins: u8,
    detect_loops: bool,
    name: String,
}

impl Default for Choices {
    fn default() -> Self {
        let settings = Settings::default();
        Self {
            lowest: settings.lowest,
            highest: settings.highest,
            step: settings.step,
            velocity_layers: settings.velocity_layers,
            note_length: settings.note_length,
            release_tail: settings.release_tail,
            round_roins: settings.round_roins,
            detect_loops: settings.detect_loops,
            name: String::new(),
        }
    }
}

impl Choices {
    /// The settings the choices make.
    fn settings(&self) -> Settings {
        Settings {
            lowest: self.lowest,
            highest: self.highest,
            step: self.step,
            velocity_layers: self.velocity_layers,
            note_length: self.note_length,
            release_tail: self.release_tail,
            round_roins: self.round_roins,
            detect_loops: self.detect_loops,
            name: self.name.clone(),
        }
    }
}

fn choices_id(device: bl_model::DeviceId) -> Id {
    Id::new(("autosampler-choices", device))
}

fn invoke(events: &mut Vec<UiEvent>, id: &str, args: serde_json::Value) {
    events.push(UiEvent::Invoke(bl_shell::Invocation::new(id, args)));
}

/// Draws the dialog if it's open.
pub fn show(ctx: &Context, shell: &Shell, theme: &Theme, events: &mut Vec<UiEvent>) {
    let Some(dialog) = shell.autosampler_dialog() else {
        return;
    };
    let device = dialog.device;
    let status = shell.autosample_status();
    let result = shell.autosample_result();
    egui::Modal::new(Id::new("autosampler-dialog")).show(ctx, |ui| {
        ui.set_width(320.0);
        ui.vertical_centered(|ui| {
            ui.label(
                egui::RichText::new("Autosample")
                    .size(theme.sizes.heading)
                    .strong(),
            );
        });
        ui.add_space(8.0);
        if status.running {
            running(ui, theme, &status, events);
        } else {
            settings(ui, theme, device, dialog.settings.clone(), result, events);
        }
    });
    // Esc closes the dialog, and stops the run while it goes on.
    if ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
        invoke(events, ids::AUTOSAMPLE_CANCEL, json!({}));
    }
    if shell.autosample_status().running {
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }
}

/// The settings, what they make, and the button that starts the run.
fn settings(
    ui: &mut Ui,
    theme: &Theme,
    device: bl_model::DeviceId,
    open: Settings,
    result: Option<&bl_shell::autosampler_commands::Autosampled>,
    events: &mut Vec<UiEvent>,
) {
    let mut choices = ui
        .ctx()
        .data(|data| data.get_temp(choices_id(device)))
        .unwrap_or(Choices {
            name: open.name.clone(),
            ..Choices::default()
        });
    let lowest = bl_model::note_name(choices.lowest);
    let highest = bl_model::note_name(choices.highest);
    ui.horizontal(|ui| {
        ui.label("Notes");
        ui.add(
            egui::DragValue::new(&mut choices.lowest)
                .range(0..=127)
                .speed(1)
                .suffix(format!(" ({lowest})")),
        );
        ui.add(
            egui::DragValue::new(&mut choices.highest)
                .range(0..=127)
                .speed(1)
                .suffix(format!(" ({highest})")),
        );
    });
    ui.horizontal(|ui| {
        ui.label("Step");
        ui.add(
            egui::DragValue::new(&mut choices.step)
                .range(1..=12)
                .speed(1),
        );
        ui.label("Velocity Layers");
        ui.add(
            egui::DragValue::new(&mut choices.velocity_layers)
                .range(1..=16)
                .speed(1),
        );
    });
    ui.horizontal(|ui| {
        ui.label("Note Length");
        ui.add(
            egui::DragValue::new(&mut choices.note_length)
                .range(0.1..=30.0)
                .speed(0.1)
                .suffix(" s"),
        );
        ui.label("Release Tail");
        ui.add(
            egui::DragValue::new(&mut choices.release_tail)
                .range(0.0..=30.0)
                .speed(0.1)
                .suffix(" s"),
        );
    });
    ui.horizontal(|ui| {
        ui.label("Round Robins");
        ui.add(
            egui::DragValue::new(&mut choices.round_roins)
                .range(1..=8)
                .speed(1),
        );
        ui.checkbox(&mut choices.detect_loops, "Detect Loops");
    });
    ui.horizontal(|ui| {
        ui.label("Name");
        ui.text_edit_singleline(&mut choices.name);
    });
    let settings = choices.settings().checked();
    let plan = plan(&settings);
    let seconds = plan.samples.len() as f64 * settings.sample_seconds();
    ui.label(
        egui::RichText::new(format!(
            "{} samples, about {} to render",
            plan.samples.len(),
            minutes(seconds)
        ))
        .font(theme.small_font())
        .color(theme.colors.text_dim.0),
    );
    ui.add_space(10.0);
    if let Some(result) = result {
        ui.label(
            egui::RichText::new(format!(
                "{} samples made in {:.1} s",
                result.files.len(),
                result.took
            ))
            .font(theme.small_font())
            .color(theme.colors.text_dim.0),
        );
        ui.add_space(4.0);
    }
    ui.horizontal(|ui| {
        let start = ui.add_enabled(!plan.samples.is_empty(), egui::Button::new("Autosample"));
        if start.clicked() {
            let args = AutosampleArgs {
                device: Some(device),
                lowest: Some(settings.lowest),
                highest: Some(settings.highest),
                step: Some(settings.step),
                velocity_layers: Some(settings.velocity_layers),
                note_length: Some(settings.note_length),
                release_tail: Some(settings.release_tail),
                round_roins: Some(settings.round_roins),
                detect_loops: Some(settings.detect_loops),
                name: Some(settings.name.clone()),
                wait: Some(false),
            };
            invoke(events, ids::AUTOSAMPLE, serde_json::to_value(args).unwrap());
        }
        if ui.button("Cancel").clicked() {
            invoke(events, ids::AUTOSAMPLE_CANCEL, json!({}));
        }
    });
    ui.ctx()
        .data_mut(|data| data.insert_temp(choices_id(device), choices));
}

/// The run's progress, the note it is on, and Cancel.
fn running(ui: &mut Ui, theme: &Theme, status: &AutosampleStatus, events: &mut Vec<UiEvent>) {
    let note = status.note.map_or_else(String::new, bl_model::note_name);
    ui.label(format!(
        "Sampling {note} ({}/{})",
        status.done, status.total
    ));
    ui.add_space(6.0);
    let fraction = if status.total == 0 {
        0.0
    } else {
        status.done as f32 / status.total as f32
    };
    let (rect, response) = ui.allocate_exact_size(
        egui::Vec2::new(ui.available_width(), 10.0),
        egui::Sense::hover(),
    );
    let painter = ui.painter_at(rect);
    let colors = &theme.colors;
    painter.rect_filled(rect, theme.sizes.corner, colors.control.0);
    let mut filled = rect;
    filled.set_width(rect.width() * fraction.clamp(0.0, 1.0));
    painter.rect_filled(filled, theme.sizes.corner, colors.accent.0);
    let id = response.id;
    let value = f64::from(fraction.clamp(0.0, 1.0)) * 100.0;
    ui.ctx().accesskit_node_builder(id, |node| {
        node.set_role(egui::accesskit::Role::ProgressIndicator);
        node.set_label("The autosample's progress".to_owned());
        node.set_numeric_value(value);
        node.set_min_numeric_value(0.0);
        node.set_max_numeric_value(100.0);
    });
    ui.add_space(10.0);
    ui.vertical_centered(|ui| {
        if ui.button("Cancel").clicked() {
            invoke(events, ids::AUTOSAMPLE_CANCEL, json!({}));
        }
    });
}

/// A time in seconds, as minutes and seconds.
fn minutes(seconds: f64) -> String {
    let whole = seconds.round() as u64;
    if whole < 60 {
        return format!("{whole} s");
    }
    format!("{} min {:02} s", whole / 60, whole % 60)
}

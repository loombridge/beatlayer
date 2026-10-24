//! The MPE/Multi-channel Settings dialog (the MPE spec, R18; L-OTH-008):
//! a modal over the main window that sets the zone MPE MIDI goes out on, or
//! multi-channel mode and its channels, for a plug-in device, a MIDI track's
//! MIDI To or an External Instrument's.
//!
//! Its controls are Beatlayer's own, in this order: Mode (Zone or
//! Multi-channel), then for a zone the Zone (Lower or Upper) and its Note
//! Channels, or for multi-channel mode its First and Last Channel, and OK.
//! Each change runs the command that saves it, so each is one undo step.

use bl_model::{MAX_NOTE_CHANNELS, MidiTo, MpeMode, MpeSettings, MpeZone};
use bl_shell::Shell;
use bl_shell::plugin_commands::{self as ids, MpeSettingsDialog};
use egui::{Context, Id, Ui};
use serde_json::json;

use crate::main_window::UiEvent;
use crate::theme::Theme;

fn invoke(events: &mut Vec<UiEvent>, id: &str, args: serde_json::Value) {
    events.push(UiEvent::Invoke(bl_shell::Invocation::new(id, args)));
}

/// Draws the dialog if it's open.
pub fn show(ctx: &Context, shell: &Shell, theme: &Theme, events: &mut Vec<UiEvent>) {
    let dialog = shell.mpe_settings_dialog();
    let Some(settings) = dialog.settings.filter(|_| dialog.open) else {
        return;
    };
    egui::Modal::new(Id::new("mpe-settings-dialog")).show(ctx, |ui| {
        ui.set_width(340.0);
        ui.vertical_centered(|ui| {
            ui.label(
                egui::RichText::new("MPE/Multi-channel Settings")
                    .size(theme.sizes.heading)
                    .strong(),
            );
        });
        ui.add_space(8.0);
        let mut changed = settings;
        controls(ui, &mut changed);
        if changed != settings {
            apply(shell, &dialog, changed, events);
        }
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            if ui.button("OK").clicked() {
                close(events, &dialog);
            }
        });
    });
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        close(events, &dialog);
    }
}

fn close(events: &mut Vec<UiEvent>, dialog: &MpeSettingsDialog) {
    let mut args = json!({ "shown": false });
    if let Some(device) = dialog.device {
        args["device"] = json!(device);
    }
    if let Some(track) = dialog.track {
        args["track"] = json!(track);
    }
    invoke(events, ids::SHOW_MPE_SETTINGS, args);
}

/// The dialog's controls, in the order the module says.
fn controls(ui: &mut Ui, settings: &mut MpeSettings) {
    ui.label("Mode");
    ui.horizontal(|ui| {
        if ui
            .selectable_label(settings.mode == MpeMode::Zone, "Zone")
            .clicked()
        {
            settings.mode = MpeMode::Zone;
        }
        if ui
            .selectable_label(settings.mode == MpeMode::MultiChannel, "Multi-channel")
            .clicked()
        {
            settings.mode = MpeMode::MultiChannel;
        }
    });
    ui.add_space(6.0);
    match settings.mode {
        MpeMode::Zone => {
            ui.label("Zone");
            ui.horizontal(|ui| {
                if ui
                    .selectable_label(settings.zone == MpeZone::Lower, "Lower")
                    .clicked()
                {
                    settings.zone = MpeZone::Lower;
                }
                if ui
                    .selectable_label(settings.zone == MpeZone::Upper, "Upper")
                    .clicked()
                {
                    settings.zone = MpeZone::Upper;
                }
            });
            ui.add_space(4.0);
            stepper(
                ui,
                "Note Channels",
                &mut settings.channels,
                1,
                MAX_NOTE_CHANNELS,
            );
            let master = match settings.zone {
                MpeZone::Lower => 1,
                MpeZone::Upper => 16,
            };
            ui.label(format!("Global channel {master}"));
        }
        MpeMode::MultiChannel => {
            stepper(ui, "First Channel", &mut settings.first, 1, 16);
            // The last channel never goes below the first.
            settings.last = settings.last.max(settings.first);
            let first = settings.first;
            stepper(ui, "Last Channel", &mut settings.last, first, 16);
        }
    }
}

/// A number from `low` to `high` with a button each way, named `name`; the
/// buttons read "Decrease" and "Increase" `name` to assistive technology.
fn stepper(ui: &mut Ui, name: &str, value: &mut u8, low: u8, high: u8) {
    ui.horizontal(|ui| {
        ui.label(name);
        let down = ui.add_enabled(*value > low, egui::Button::new("−"));
        name_button(ui, &down, &format!("Decrease {name}"));
        ui.label(value.to_string());
        let up = ui.add_enabled(*value < high, egui::Button::new("+"));
        name_button(ui, &up, &format!("Increase {name}"));
        if down.clicked() {
            *value = value.saturating_sub(1).max(low);
        }
        if up.clicked() {
            *value = (*value + 1).min(high);
        }
    });
}

fn name_button(ui: &Ui, response: &egui::Response, label: &str) {
    ui.ctx().accesskit_node_builder(response.id, |node| {
        node.set_label(label.to_owned());
    });
}

/// Saves `settings` where the dialog's target keeps them (R18): a plug-in
/// device's, an External Instrument's, or a track's MIDI To.
fn apply(
    shell: &Shell,
    dialog: &MpeSettingsDialog,
    settings: MpeSettings,
    events: &mut Vec<UiEvent>,
) {
    let document = shell.editor().document();
    if let Some(device) = dialog.device {
        if document
            .device(device)
            .is_some_and(|d| matches!(d.kind, bl_model::DeviceKind::Plugin { .. }))
        {
            invoke(
                events,
                "plugin.set_mpe_settings",
                json!({ "device": device, "settings": settings }),
            );
        } else {
            invoke(
                events,
                "device.set_external",
                json!({ "device": device, "mpe": settings }),
            );
        }
    } else if let Some(track) = dialog.track
        && let Some(MidiTo::Port { name, channel, .. }) = document
            .track(track)
            .and_then(|t| t.routing().midi_output.clone())
    {
        invoke(
            events,
            "routing.set_midi_output",
            json!({
                "track": track,
                "to": { "name": name, "channel": channel, "mpe": settings }
            }),
        );
    }
}

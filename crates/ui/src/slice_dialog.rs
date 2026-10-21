//! Slice to New MIDI Track's dialog (WP-4.16; the slice-to-midi spec, R1,
//! R2): a modal over the main window with the Slice By chooser, the Slicing
//! Preset chooser, Preserve warped timing, and Cancel and OK, in that order
//! and with Beatlayer's own defaults.
//!
//! The choices are the dialog's own until OK sends them in
//! `clip.slice_to_midi_track`.

use bl_model::{ClipData, ClipId};
use bl_shell::Shell;
use bl_shell::slice_commands::{self as ids, SliceBy, SliceDialogStatus};
use egui::{Context, Id, Ui};
use serde_json::json;

use crate::main_window::UiEvent;
use crate::theme::Theme;

/// The dialog's choices, kept between frames: the clip they were made for,
/// so a dialog opening on another clip starts at R2's defaults.
#[derive(Clone, Debug)]
struct Choices {
    clip: Option<ClipId>,
    by: SliceBy,
    preset: String,
    preserve: bool,
}

impl Choices {
    /// R2's defaults for `dialog`.
    fn defaults(dialog: &SliceDialogStatus) -> Self {
        Self {
            clip: dialog.clip,
            by: SliceBy::Sixteenth,
            preset: dialog
                .presets
                .first()
                .cloned()
                .unwrap_or_else(|| ids::DRUM_RACK_PRESET.to_owned()),
            preserve: true,
        }
    }
}

fn choices_id() -> Id {
    Id::new("slice-dialog-choices")
}

fn invoke(events: &mut Vec<UiEvent>, id: &str, args: serde_json::Value) {
    events.push(UiEvent::Invoke(bl_shell::Invocation::new(id, args)));
}

/// Draws the dialog if it's open.
pub fn show(ctx: &Context, shell: &Shell, theme: &Theme, events: &mut Vec<UiEvent>) {
    let Some(dialog) = shell.slice_dialog() else {
        return;
    };
    egui::Modal::new(Id::new("slice-dialog")).show(ctx, |ui| {
        ui.set_width(340.0);
        ui.vertical_centered(|ui| {
            ui.label(
                egui::RichText::new("Slice to New MIDI Track")
                    .size(theme.sizes.heading)
                    .strong(),
            );
        });
        ui.add_space(8.0);
        choices(ui, shell, &dialog, events);
    });
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        invoke(events, ids::CLOSE, json!({}));
    }
}

/// Whether the dialog's clip is a warped audio clip: Warp Markers isn't
/// offered for an unwarped clip (R2).
fn warped(shell: &Shell, dialog: &SliceDialogStatus) -> bool {
    dialog
        .clip
        .and_then(|clip| shell.editor().document().clip(clip))
        .is_some_and(|clip| match &clip.data {
            ClipData::Audio(audio) => audio.warp,
            ClipData::Midi(_) => false,
        })
}

/// The dialog's controls, in R2's order.
fn choices(ui: &mut Ui, shell: &Shell, dialog: &SliceDialogStatus, events: &mut Vec<UiEvent>) {
    let warped = warped(shell, dialog);
    let mut state: Choices = ui
        .ctx()
        .data(|d| d.get_temp(choices_id()))
        .unwrap_or_else(|| Choices::defaults(dialog));
    if state.clip != dialog.clip {
        state = Choices::defaults(dialog);
    }
    if !warped && state.by == SliceBy::WarpMarkers {
        state.by = SliceBy::Sixteenth;
    }
    ui.label("Slice By");
    // The divisions, then Transients, then Warp Markers (R2).
    ui.horizontal_wrapped(|ui| {
        for choice in SliceBy::ALL {
            if choice == SliceBy::WarpMarkers && !warped {
                continue;
            }
            let selected = state.by == choice;
            if ui.selectable_label(selected, choice.label()).clicked() {
                state.by = choice;
            }
        }
    });
    ui.add_space(6.0);
    ui.label("Slicing Preset");
    for preset in &dialog.presets {
        let selected = state.preset == *preset;
        if ui.selectable_label(selected, preset).clicked() {
            state.preset = preset.clone();
        }
    }
    ui.add_space(6.0);
    ui.checkbox(&mut state.preserve, "Preserve warped timing");
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        if ui.button("Cancel").clicked() {
            invoke(events, ids::CLOSE, json!({}));
        }
        if ui.button("OK").clicked()
            && let Some(clip) = dialog.clip
        {
            invoke(
                events,
                ids::SLICE,
                json!({
                    "clip": clip,
                    "by": state.by,
                    "preset": state.preset,
                    "preserve_warped_timing": state.preserve,
                }),
            );
        }
    });
    ui.ctx().data_mut(|d| d.insert_temp(choices_id(), state));
}

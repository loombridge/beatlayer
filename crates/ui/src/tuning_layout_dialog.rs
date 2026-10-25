//! The Configure MIDI Layout dialog (the tunings spec, R23; L-OTH-009): a
//! modal over the main window that sets, for each incoming key of a MIDI
//! track's Custom Controller Layout, the key of the tuning it plays, or
//! none.
//!
//! Its controls are Beatlayer's own: a row for each of the 128 incoming
//! keys, named as the note editor names a twelve-step key, with the key of
//! the tuning it plays and a button each way, a None button, and OK. A value
//! the set's tuning has no key for reads "unavailable" and plays nothing.
//! Each change runs `track.set_tuning_layout`, so each is one undo step.

use bl_model::{LAYOUT_KEYS, TrackId};
use bl_shell::Shell;
use bl_shell::tuning_commands::{self as ids, LayoutDialog};
use egui::{Context, Id, Ui};
use serde_json::json;

use crate::main_window::UiEvent;
use crate::theme::Theme;

fn invoke(events: &mut Vec<UiEvent>, id: &str, args: serde_json::Value) {
    events.push(UiEvent::Invoke(bl_shell::Invocation::new(id, args)));
}

/// The name of twelve-step key `key`: C3 is key 60.
fn key_name(key: usize) -> String {
    const NAMES: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    format!("{}{}", NAMES[key % 12], key as i32 / 12 - 2)
}

/// Draws the dialog if it's open.
pub fn show(ctx: &Context, shell: &Shell, theme: &Theme, events: &mut Vec<UiEvent>) {
    let dialog = shell.tuning_layout_dialog();
    let (Some(track), Some(layout)) = (dialog.track, dialog.layout.as_ref()) else {
        return;
    };
    let table: Vec<Option<u8>> = (0..LAYOUT_KEYS as u8)
        .map(|key| layout.custom(key))
        .collect();
    egui::Modal::new(Id::new("tuning-layout-dialog")).show(ctx, |ui| {
        ui.set_width(340.0);
        ui.vertical_centered(|ui| {
            ui.label(
                egui::RichText::new("Configure MIDI Layout")
                    .size(theme.sizes.heading)
                    .strong(),
            );
        });
        ui.add_space(8.0);
        egui::ScrollArea::vertical()
            .max_height(320.0)
            .show(ui, |ui| {
                for key in 0..LAYOUT_KEYS {
                    if let Some(next) = row(ui, key, table[key], dialog.tuning_keys) {
                        let mut changed = table.clone();
                        changed[key] = next;
                        invoke(
                            events,
                            "track.set_tuning_layout",
                            json!({ "track": track, "keys": changed }),
                        );
                    }
                }
            });
        ui.add_space(10.0);
        if ui.button("OK").clicked() {
            close(events, track);
        }
    });
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        close(events, track);
    }
}

fn close(events: &mut Vec<UiEvent>, track: TrackId) {
    invoke(
        events,
        ids::SHOW_LAYOUT,
        json!({ "track": track, "shown": false }),
    );
}

/// One incoming key's row. `Some(value)` is the new value the person chose.
fn row(ui: &mut Ui, key: usize, value: Option<u8>, tuning_keys: usize) -> Option<Option<u8>> {
    let name = key_name(key);
    let mut chosen = None;
    ui.horizontal(|ui| {
        ui.label(format!("{name} ({key})"));
        let down = ui.add_enabled(value.is_some_and(|v| v > 0), egui::Button::new("−"));
        name_button(ui, &down, &format!("Decrease the key {name} plays"));
        ui.label(match value {
            None => "none".to_owned(),
            Some(v) if usize::from(v) >= tuning_keys => format!("{v} (unavailable)"),
            Some(v) => v.to_string(),
        });
        let up = ui.add_enabled(value.is_some_and(|v| v < 127), egui::Button::new("+"));
        name_button(ui, &up, &format!("Increase the key {name} plays"));
        let none = ui.add_enabled(value.is_some(), egui::Button::new("None"));
        name_button(ui, &none, &format!("Play nothing on {name}"));
        if down.clicked() {
            chosen = value.map(|v| Some(v.saturating_sub(1)));
        }
        if up.clicked() {
            chosen = value.map(|v| Some((v + 1).min(127)));
        }
        if none.clicked() {
            chosen = Some(None);
        }
    });
    chosen
}

fn name_button(ui: &Ui, response: &egui::Response, label: &str) {
    ui.ctx().accesskit_node_builder(response.id, |node| {
        node.set_label(label.to_owned());
    });
}

/// Whether the dialog is open, for the window's key handling.
#[must_use]
pub fn is_open(shell: &Shell) -> bool {
    let LayoutDialog { open, .. } = shell.tuning_layout_dialog();
    open
}

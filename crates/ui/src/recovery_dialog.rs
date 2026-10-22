//! The recovery prompt (WP-6.62; the crash-recovery spec, R22 and R23): a
//! modal over the main window that lists the sets a crash left unsaved,
//! newest first, with Recover and Discard for each and Later for the lot.
//!
//! The wording and the Later button are Beatlayer's own; Live has no such
//! a prompt.

use bl_project::autosave::Recoverable;
use bl_project::local_stamp;
use bl_shell::Shell;
use bl_shell::commands as ids;
use egui::accesskit::Role;
use egui::{Context, Id, RichText, Ui};
use serde_json::json;

use crate::main_window::UiEvent;
use crate::theme::Theme;

/// The command that recovers a set, and the one that declines it.
const RECOVER: &str = "set.recover";
const DISCARD: &str = "set.discard_recovery";

/// Where the prompt keeps the note that it has focused the newest set's
/// Recover button, and which of its buttons has the focus.
const FOCUSED: &str = "recovery-prompt-focused";
const FOCUS_INDEX: &str = "recovery-prompt-focus-index";

fn invoke(events: &mut Vec<UiEvent>, id: &str, args: serde_json::Value) {
    events.push(UiEvent::Invoke(bl_shell::Invocation::new(id, args)));
}

/// Draws the prompt if there is something to offer.
pub fn show(ctx: &Context, shell: &Shell, theme: &Theme, events: &mut Vec<UiEvent>) {
    if !shell.recovery_prompt_open() {
        // The prompt is closed: the next one focuses its Recover button
        // again.
        ctx.data_mut(|d| d.remove::<bool>(Id::new(FOCUSED)));
        ctx.data_mut(|d| d.remove::<usize>(Id::new(FOCUS_INDEX)));
        return;
    }
    let offers = shell.recovery_offers();
    let mut buttons: Vec<Id> = Vec::new();
    egui::Modal::new(Id::new("recovery-dialog")).show(ctx, |ui| {
        ui.set_width(460.0);
        ui.vertical_centered(|ui| {
            ui.label(
                RichText::new("Recover Unsaved Sets")
                    .size(theme.sizes.heading)
                    .strong(),
            );
        });
        ui.add_space(8.0);
        for offer in &offers {
            buttons.extend(offer_row(ui, theme, offer, events));
        }
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            let later = ui.button("Later");
            buttons.push(later.id);
            if later.clicked() {
                invoke(events, ids::SET_LATER_RECOVERY, json!({}));
            }
        });
    });
    // Spec: crash-recovery R23. The prompt reaches assistive technology as
    // a dialog naming each set.
    ctx.accesskit_node_builder(Id::new("recovery-prompt"), |node| {
        node.set_role(Role::Dialog);
        node.set_label("Recover Unsaved Sets");
    });
    if buttons.is_empty() {
        return;
    }
    // Spec: crash-recovery R23. Recover on the newest set is focused first,
    // and Tab moves between the prompt's buttons.
    let first = ctx.data_mut(|d| !d.get_persisted::<bool>(Id::new(FOCUSED)).unwrap_or(false));
    if first {
        ctx.data_mut(|d| d.insert_persisted(Id::new(FOCUSED), true));
        ctx.data_mut(|d| d.insert_persisted(Id::new(FOCUS_INDEX), 0usize));
        ctx.memory_mut(|m| m.request_focus(buttons[0]));
    }
    let index = ctx
        .data_mut(|d| d.get_persisted::<usize>(Id::new(FOCUS_INDEX)).unwrap_or(0))
        .min(buttons.len() - 1);
    if ctx.input(|i| i.key_pressed(egui::Key::Tab)) {
        let backwards = ctx.input(|i| i.modifiers.shift);
        let next = if backwards {
            index.checked_sub(1).unwrap_or(buttons.len() - 1)
        } else {
            (index + 1) % buttons.len()
        };
        ctx.data_mut(|d| d.insert_persisted(Id::new(FOCUS_INDEX), next));
        ctx.memory_mut(|m| m.request_focus(buttons[next]));
    }
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        invoke(events, ids::SET_LATER_RECOVERY, json!({}));
    }
}

/// One set the prompt offers: what it was, and Recover and Discard.
/// Returns the two buttons' IDs, in the Tab order.
fn offer_row(
    ui: &mut Ui,
    theme: &Theme,
    offer: &Recoverable,
    events: &mut Vec<UiEvent>,
) -> Vec<Id> {
    let name = offer.name();
    let mut buttons = Vec::with_capacity(2);
    ui.group(|ui| {
        // Spec: crash-recovery R23. The set's name is read by assistive
        // technology.
        let response = ui.add(egui::Label::new(&name));
        ui.ctx().accesskit_node_builder(response.id, |node| {
            node.set_role(Role::Label);
            node.set_label(name.clone());
        });
        let file = offer.set.as_ref().map_or_else(
            || "never saved".to_owned(),
            |path| path.display().to_string(),
        );
        ui.label(format!("File: {file}"));
        let edits = offer.edits;
        ui.label(format!(
            "{edits} edit{} since the last save",
            if edits == 1 { "" } else { "s" }
        ));
        ui.label(format!(
            "Last edit: {} ({})",
            local_stamp(offer.last_edit),
            offer.last_label
        ));
        if offer.saved_since {
            // Spec: crash-recovery R22. The warning that the set was
            // saved since by something else.
            ui.label(
                RichText::new(
                    "This set's file was saved by something else since, so recovering and \
                     saving it would replace that save.",
                )
                .color(theme.colors.error_text.0),
            );
        }
        ui.horizontal(|ui| {
            let response = ui.button("Recover");
            buttons.push(response.id);
            if response.clicked() {
                invoke(events, RECOVER, json!({ "id": offer.id }));
            }
            let response = ui.button("Discard");
            buttons.push(response.id);
            if response.clicked() {
                invoke(events, DISCARD, json!({ "id": offer.id }));
            }
        });
    });
    buttons
}

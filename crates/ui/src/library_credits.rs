//! The Library Credits window (the core library spec, R12): every installed
//! library item's credit, grouped by package, with Export Credits... and
//! Close. Help ▸ Library Credits... opens it.

use bl_shell::Shell;
use bl_shell::library_commands::{CLOSE_CREDITS, EXPORT_CREDITS};
use egui::accesskit::Role;
use egui::{Context, Id, Key, Rect, Sense, Ui, vec2};
use serde_json::json;

use crate::main_window::UiEvent;
use crate::theme::Theme;

fn invoke(events: &mut Vec<UiEvent>, id: &str) {
    events.push(UiEvent::Invoke(bl_shell::Invocation::new(id, json!({}))));
}

/// Draws the window if it's open. Esc closes it.
pub fn show(ctx: &Context, shell: &Shell, theme: &Theme, events: &mut Vec<UiEvent>) {
    if !shell.library_credits_open() {
        return;
    }
    let credits = shell.library_credits(false);
    egui::Modal::new(Id::new("library-credits")).show(ctx, |ui| {
        ui.set_width(520.0);
        ui.vertical_centered(|ui| {
            ui.label(
                egui::RichText::new("Library Credits")
                    .size(theme.sizes.heading)
                    .strong(),
            );
        });
        ui.add_space(8.0);
        egui::ScrollArea::vertical()
            .max_height(360.0)
            .show(ui, |ui| {
                if credits.packages.is_empty() {
                    ui.label("No library items are installed.");
                }
                for package in &credits.packages {
                    list(
                        ui,
                        theme,
                        &package.name,
                        package.items.iter().map(|i| i.credit.as_str()),
                    );
                }
            });
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            let export = ui.add_enabled(
                !credits.packages.is_empty(),
                egui::Button::new("Export Credits..."),
            );
            if export.clicked() {
                invoke(events, EXPORT_CREDITS);
            }
            if ui.button("Close").clicked() {
                invoke(events, CLOSE_CREDITS);
            }
        });
    });
    if ctx.input(|i| i.key_pressed(Key::Escape)) {
        invoke(events, CLOSE_CREDITS);
    }
}

/// One package's credits, read by assistive technology as a list.
fn list<'a>(ui: &mut Ui, theme: &Theme, name: &str, credits: impl Iterator<Item = &'a str>) {
    ui.add_space(4.0);
    let title = ui.label(egui::RichText::new(name).font(theme.small_font()).strong());
    let list = ui.interact(
        Rect::from_min_size(title.rect.min, vec2(1.0, 1.0)),
        Id::new(("library-credits-list", name)),
        Sense::hover(),
    );
    ui.ctx().accesskit_node_builder(list.id, |node| {
        node.set_role(Role::List);
        node.set_label(name.to_owned());
    });
    for credit in credits {
        let item = ui.label(credit);
        ui.ctx().accesskit_node_builder(item.id, |node| {
            node.set_role(Role::ListItem);
            node.set_label(credit.to_owned());
        });
    }
}

//! A browser item's context menu (spec: browser R25, browser-tags R35;
//! C-BROWSER-001 to C-BROWSER-011, C-BROWSER-014): Show in Places, Show in
//! Finder, Show Similar Files, Clear All Colors, the seven collections, and
//! Hide Plug-In.

use std::path::Path;

use bl_shell::Shell;
use bl_shell::browser_commands::{self as ids, Item, ItemKind, Label, Section};
use egui::{Button, Response, Ui};
use serde_json::json;

use super::invoke;
use crate::main_window::UiEvent;

/// Whether a row has the menu: the files, folders, plug-ins and devices the
/// browser lists, not the rows of an opened set.
pub(super) const fn has_menu(kind: ItemKind) -> bool {
    matches!(
        kind,
        ItemKind::Folder
            | ItemKind::Audio
            | ItemKind::Sfz
            | ItemKind::Set
            | ItemKind::Preset
            | ItemKind::ChordBank
            | ItemKind::Groove
            | ItemKind::Plugin
            | ItemKind::Device
            | ItemKind::Clip
    )
}

/// The menu for `item`. Its entries act on the selected items; a
/// right-click selects the row first when it isn't selected.
pub(super) fn show(ui: &mut Ui, shell: &Shell, item: &Item, events: &mut Vec<UiEvent>) {
    let view = shell.browser_view();
    let mut first = true;
    let mut add = |ui: &mut Ui, button: Button<'_>, enabled: bool| -> Response {
        let response = ui.add_enabled(enabled, button);
        crate::menus::register_item(ui.ctx(), &response);
        if first {
            crate::menus::take_focus_first(ui, &response);
            first = false;
        }
        response
    };

    // C-BROWSER-001. Shown away from the item's place: under a Library
    // label, a collection, a saved label or a search.
    let in_own_place =
        !view.results_bar() && (view.label().is_place() || *view.label() == Label::All);
    if !in_own_place {
        let in_places = view
            .roots()
            .iter()
            .any(|(_, root)| Path::new(&item.path).starts_with(root));
        if add(ui, Button::new("Show in Places"), in_places).clicked() {
            invoke(events, ids::SHOW_FILE, json!({ "path": item.path }));
            ui.close();
        }
    }
    // C-BROWSER-002. A plug-in or a device isn't a file.
    let finder = if cfg!(target_os = "macos") {
        "Show in Finder"
    } else {
        "Show in File Manager"
    };
    let files = !view.selected().is_empty() && view.selected().iter().all(|path| path.exists());
    if add(ui, Button::new(finder), files).clicked() {
        invoke(events, ids::REVEAL, json!({}));
        ui.close();
    }
    // C-BROWSER-003 (spec: sound-similarity R11). Disabled for an item that
    // can't be a reference, with the reason in its description.
    let similar = if cfg!(target_os = "macos") {
        "\u{21e7}\u{2318}F"
    } else {
        "Ctrl+Shift+F"
    };
    let reference = view.selected_reference();
    let response = add(
        ui,
        Button::new("Show Similar Files").shortcut_text(similar),
        reference.is_ok(),
    );
    match reference {
        Ok(_) if response.clicked() => {
            invoke(events, ids::SHOW_SIMILAR, json!({}));
            ui.close();
        }
        Ok(_) => {}
        Err(reason) => {
            ui.ctx().accesskit_node_builder(response.id, |node| {
                node.set_description(format!("Not available: {reason}"));
            });
        }
    }
    // Spec: stem-separation R22. Separate Stems to New Audio Tracks, after
    // Show Similar Files: Beatlayer's own place (C-BROWSER-013).
    let stems = json!({ "source": { "kind": "file", "path": item.path } });
    let stems_ok = view.selected().len() <= 1
        && item.kind == ItemKind::Audio
        && shell.can_invoke(bl_shell::stems_commands::OPEN, &stems);
    if add(
        ui,
        Button::new("Separate Stems to New Audio Tracks"),
        stems_ok,
    )
    .clicked()
    {
        invoke(events, bl_shell::stems_commands::OPEN, stems);
        ui.close();
    }
    ui.separator();
    // C-BROWSER-004 to 011.
    if add(ui, Button::new("Clear All Colors").shortcut_text("0"), true).clicked() {
        invoke(events, ids::CLEAR_COLLECTIONS, json!({}));
        ui.close();
    }
    let collections = view
        .labels()
        .into_iter()
        .filter(|l| l.section == Section::Collections);
    for (number, info) in (1u8..).zip(collections) {
        if add(
            ui,
            Button::new(info.name).shortcut_text(number.to_string()),
            true,
        )
        .clicked()
        {
            invoke(events, ids::SET_COLLECTION, json!({ "collection": number }));
            ui.close();
        }
    }
    // Spec: browser-tags R41. Hide Plug-In or Show Plug-In, at the end of a
    // plug-in row's menu, after Gray (7) (Live 12.4.16).
    if item.kind == ItemKind::Plugin {
        ui.separator();
        let label = if item.hidden {
            "Show Plug-In"
        } else {
            "Hide Plug-In"
        };
        if add(ui, Button::new(label), true).clicked() {
            invoke(
                events,
                bl_shell::plugin_commands::HIDE,
                json!({ "keys": view.selected_plugin_keys(), "hidden": !item.hidden }),
            );
            ui.close();
        }
    }
    // C-BROWSER-014. A project folder's menu has Manage Project, which shows
    // the File Manager at that project (spec: file-management R10).
    if item.kind == ItemKind::Folder && shell.is_project_folder(Path::new(&item.path)) {
        ui.separator();
        if add(ui, Button::new("Manage Project"), true).clicked() {
            invoke(
                events,
                "view.toggle_file_manager",
                json!({ "view": "project", "project": item.path, "open": true }),
            );
            ui.close();
        }
    }
}

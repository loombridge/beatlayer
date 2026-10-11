//! The menu bar's live state, and the menu bar Beatlayer draws itself where
//! it has no native one (Linux).
//!
//! [`item_state`] says, for any menu item of `bl_shell::menus`, what it
//! reads, whether it's enabled or checked, which keys it shows, and what it
//! runs, so the native menus (in the app) and the drawn menu bar agree.

use bl_shell::menus::{Action, Menu, MenuItem, shown};
use bl_shell::{Invocation, Shell};
use egui::Ui;
use serde_json::{Value, json};

use crate::keymap::{Chord, Keymap, Platform, chord_text};
use crate::main_window::UiEvent;

/// What a menu item shows and does now.
#[derive(Clone, Debug, PartialEq)]
pub struct ItemState {
    /// Its label, which for Undo and Redo names the step.
    pub label: String,
    /// Whether choosing it does anything now.
    pub enabled: bool,
    /// For a switch, whether it's on (shown with a check mark).
    pub checked: Option<bool>,
    /// The keys shown beside it.
    pub chord: Option<Chord>,
    /// What choosing it runs.
    pub invocation: Option<Invocation>,
}

/// The state of a menu item now.
#[must_use]
pub fn item_state(
    item: &MenuItem,
    shell: &Shell,
    keymap: &Keymap,
    platform: Platform,
) -> ItemState {
    let resolved = |id: &str, args: Value, chord: Option<&Chord>| ItemState {
        label: shell
            .dynamic_label(id)
            .unwrap_or_else(|| item.label.to_owned()),
        enabled: shell.can_invoke(id, &args),
        checked: shell.switch_state(id),
        chord: chord.cloned(),
        invocation: Some(Invocation::new(id, args)),
    };
    let inert = |chord: Option<&Chord>| ItemState {
        label: item.label.to_owned(),
        enabled: false,
        checked: None,
        chord: chord.cloned(),
        invocation: None,
    };
    match item.action {
        Action::Command(id) => resolved(id, json!({}), keymap.chord_for_command(id, platform)),
        Action::Shortcut(shortcuts) => {
            let chord = keymap.chord_for_shortcut(shortcuts[0], platform);
            let contexts = shell.active_contexts();
            shortcuts
                .iter()
                .find_map(|shortcut| keymap.command_for(shortcut, &contexts))
                .map_or_else(
                    || inert(chord),
                    |command| resolved(&command.id, Value::Object(command.args.clone()), chord),
                )
        }
        Action::Pending(shortcut) => {
            inert(shortcut.and_then(|s| keymap.chord_for_shortcut(s, platform)))
        }
        Action::Submenu => ItemState {
            enabled: true,
            ..inert(None)
        },
        Action::System(_) | Action::Omitted(_) | Action::Separator => inert(None),
    }
}

/// Draws the menu bar with egui, for platforms without a native one. Items
/// run what [`item_state`] says.
pub fn menu_bar(
    ui: &mut Ui,
    menus: &[Menu],
    shell: &Shell,
    keymap: &Keymap,
    platform: Platform,
    events: &mut Vec<UiEvent>,
) {
    egui::MenuBar::new().ui(ui, |ui| {
        for menu in menus {
            ui.menu_button(menu.title, |ui| {
                items(ui, &menu.items, shell, keymap, platform, events);
            });
        }
    });
}

fn items(
    ui: &mut Ui,
    items_: &[MenuItem],
    shell: &Shell,
    keymap: &Keymap,
    platform: Platform,
    events: &mut Vec<UiEvent>,
) {
    for item in shown(items_) {
        if item.is_separator() {
            ui.separator();
            continue;
        }
        if item.action == Action::Submenu {
            ui.menu_button(item.label, |ui| {
                items(ui, &item.items, shell, keymap, platform, events);
            });
            continue;
        }
        self::item(ui, item, shell, keymap, platform, events);
    }
}

/// Draws one menu item as a button, with its check mark and keys, that runs
/// what [`item_state`] says and closes the menu. The menu bar and the
/// Arrangement's context menus (WP-1.8b) draw their items with it.
pub(crate) fn item(
    ui: &mut Ui,
    item: &MenuItem,
    shell: &Shell,
    keymap: &Keymap,
    platform: Platform,
    events: &mut Vec<UiEvent>,
) {
    let state = item_state(item, shell, keymap, platform);
    let mut text = state.label.clone();
    if state.checked == Some(true) {
        text = format!("✔ {text}");
    }
    let mut button = egui::Button::new(text);
    if let Some(chord) = &state.chord {
        button = button.shortcut_text(chord_text(chord, platform));
    }
    let response = ui.add_enabled(state.enabled, button);
    if response.clicked()
        && let Some(invocation) = state.invocation
    {
        events.push(UiEvent::Invoke(invocation));
        ui.close();
    }
}

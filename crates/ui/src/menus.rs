//! The menu bar's live state, and the menu bar Beatlayer draws itself where
//! it has no native one (Linux).
//!
//! [`item_state`] says, for any menu item of `bl_shell::menus`, what it
//! reads, whether it's enabled or checked, which keys it shows, and what it
//! runs, so the native menus (in the app) and the drawn menu bar agree.

use bl_commands::Context;
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
    item_state_in(item, shell, keymap, platform, &shell.active_contexts())
}

/// [`item_state`] with `contexts` as the ones that apply, for a context menu
/// that acts on what it was opened on whatever has the keyboard.
#[must_use]
pub fn item_state_in(
    item: &MenuItem,
    shell: &Shell,
    keymap: &Keymap,
    platform: Platform,
    contexts: &[Context],
) -> ItemState {
    let resolved = |id: &str, args: Value, chord: Option<&Chord>| ItemState {
        label: shell
            .dynamic_label(id)
            .unwrap_or_else(|| item.label.to_owned()),
        enabled: shell.can_invoke_in(id, &args, contexts),
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
            shortcuts
                .iter()
                .find_map(|shortcut| keymap.command_for(shortcut, contexts))
                .map_or_else(
                    || inert(chord),
                    |command| resolved(&command.id, Value::Object(command.args.clone()), chord),
                )
        }
        Action::Choice(id, args) => {
            let args: Value = serde_json::from_str(args).expect("a menu item's arguments are JSON");
            ItemState {
                checked: shell.choice_state(id, &args),
                ..resolved(id, args, None)
            }
        }
        Action::Pending(shortcut) => {
            inert(shortcut.and_then(|s| keymap.chord_for_shortcut(s, platform)))
        }
        Action::Submenu => ItemState {
            enabled: shell.can_open_submenu(item.row),
            ..inert(None)
        },
        Action::System(_) | Action::Omitted(_) | Action::Separator => inert(None),
    }
}

/// Where the menu Alt and a letter opened waits to focus its first item.
fn focus_first_id() -> egui::Id {
    egui::Id::new("menu-focus-first")
}

fn items_id() -> egui::Id {
    egui::Id::new("menu-items")
}

/// Puts a menu item in the order Up and Down move through while its menu
/// is open.
pub(crate) fn register_item(ctx: &egui::Context, response: &egui::Response) {
    if response.enabled() {
        ctx.data_mut(|d| {
            d.get_temp_mut_or_default::<Vec<egui::Id>>(items_id())
                .push(response.id);
        });
    }
}

/// Up and Down in an open menu (S-MENU-002; the accessibility spec, R5):
/// the focus moves to the item above or below, in the order the last frame
/// drew them, rather than to whatever egui finds nearest that way. Call
/// before drawing the window.
pub fn menu_keys(ctx: &egui::Context) {
    let items = ctx
        .data_mut(|d| d.remove_temp::<Vec<egui::Id>>(items_id()))
        .unwrap_or_default();
    if items.is_empty() || !egui::Popup::is_any_open(ctx) {
        return;
    }
    let step = ctx.input(|i| {
        i.events
            .iter()
            .map(|event| match event {
                egui::Event::Key {
                    key: egui::Key::ArrowDown,
                    pressed: true,
                    modifiers,
                    ..
                } if modifiers.is_none() => 1,
                egui::Event::Key {
                    key: egui::Key::ArrowUp,
                    pressed: true,
                    modifiers,
                    ..
                } if modifiers.is_none() => -1,
                _ => 0,
            })
            .sum::<i32>()
    });
    if step == 0 {
        return;
    }
    let focused = ctx.memory(egui::Memory::focused);
    let at = focused.and_then(|id| items.iter().position(|item| *item == id));
    let last = items.len() as i32 - 1;
    let next = match at {
        Some(i) => (i as i32 + step).clamp(0, last),
        None if step > 0 => 0,
        None => last,
    };
    ctx.memory_mut(|m| {
        // Instead of egui's own move to the nearest widget that way.
        m.move_focus(egui::FocusDirection::None);
        m.request_focus(items[next as usize]);
    });
}

/// Asks the next menu drawn to focus its first item that can be chosen, as
/// a menu opened from the keyboard does.
pub(crate) fn focus_first_item(ctx: &egui::Context) {
    ctx.data_mut(|d| d.insert_temp(focus_first_id(), true));
}

/// Focuses a menu item if a menu opened from the keyboard waits for its
/// first one.
pub(crate) fn take_focus_first(ui: &Ui, response: &egui::Response) {
    if response.enabled()
        && ui
            .ctx()
            .data_mut(|d| d.remove_temp::<bool>(focus_first_id()))
            .is_some()
    {
        response.request_focus();
    }
}

/// Whether the keys that open the focused item's context menu were pressed
/// this frame (S-CTX-002; the accessibility spec, R8): Shift-F10 on Windows
/// and Linux, and on macOS Ctrl-Option-Shift-M, which VoiceOver gives as
/// VO-Shift-M.
pub(crate) fn context_menu_keys(ui: &Ui) -> bool {
    ui.input(|i| {
        i.events.iter().any(|event| {
            matches!(
                event,
                egui::Event::Key { key: egui::Key::F10, pressed: true, modifiers, .. }
                    if modifiers.shift && !modifiers.alt && !modifiers.ctrl && !modifiers.mac_cmd
            ) || matches!(
                event,
                egui::Event::Key { key: egui::Key::M, pressed: true, modifiers, .. }
                    if cfg!(target_os = "macos")
                        && modifiers.shift
                        && modifiers.alt
                        && modifiers.ctrl
                        && !modifiers.mac_cmd
            )
        })
    })
}

/// The ID of the context menu the keyboard opens for the widget `owner`.
fn keyboard_menu_id(owner: egui::Id) -> egui::Id {
    owner.with("keyboard-context-menu")
}

/// Opens `owner`'s context menu from the keyboard at `at`, with its first
/// item focused; [`keyboard_context_menu`] then draws it.
pub(crate) fn open_keyboard_context_menu(ctx: &egui::Context, owner: egui::Id, at: egui::Pos2) {
    let id = keyboard_menu_id(owner);
    ctx.data_mut(|d| d.insert_temp(id.with("at"), at));
    egui::Popup::open_id(ctx, id);
    focus_first_item(ctx);
}

/// Draws the context menu the keyboard opened for `response`'s widget, if
/// it's open: the same items a right-click shows, at the place the keys
/// chose rather than the pointer's.
pub(crate) fn keyboard_context_menu(response: &egui::Response, content: impl FnOnce(&mut Ui)) {
    let id = keyboard_menu_id(response.id);
    if !egui::Popup::is_id_open(&response.ctx, id) {
        return;
    }
    let Some(at) = response
        .ctx
        .data(|d| d.get_temp::<egui::Pos2>(id.with("at")))
    else {
        return;
    };
    egui::Popup::context_menu(response)
        .id(id)
        .open_memory(None)
        .at_position(at)
        .show(content);
}

/// The letter a key types, for the menus' Alt-letter keys.
fn letter(key: egui::Key) -> Option<char> {
    let name = key.name();
    let mut chars = name.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) if c.is_ascii_alphabetic() => Some(c.to_ascii_uppercase()),
        _ => None,
    }
}

/// Draws the menu bar with egui, for platforms without a native one. Items
/// run what [`item_state`] says.
///
/// As on Windows, Alt with a menu's letter opens it with its first item
/// focused (S-MENU-001); then the arrow keys move between items, Enter
/// chooses one, and Esc closes the menu (S-MENU-002 to S-MENU-004; the
/// accessibility spec, R5).
pub fn menu_bar(
    ui: &mut Ui,
    menus: &[Menu],
    shell: &Shell,
    keymap: &Keymap,
    platform: Platform,
    events: &mut Vec<UiEvent>,
) {
    let typed = ui.input(|i| {
        i.events.iter().find_map(|event| match event {
            egui::Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } if modifiers.alt && !modifiers.ctrl && !modifiers.shift && !modifiers.command => {
                letter(*key)
            }
            _ => None,
        })
    });
    egui::MenuBar::new().ui(ui, |ui| {
        for menu in menus {
            let response = ui
                .menu_button(menu.title, |ui| {
                    items(ui, &menu.items, shell, keymap, platform, events);
                })
                .response;
            if typed.is_some() && typed == menu.mnemonic {
                egui::Popup::open_id(ui.ctx(), egui::Popup::default_response_id(&response));
                focus_first_item(ui.ctx());
            }
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
            ui.add_enabled_ui(shell.can_open_submenu(item.row), |ui| {
                ui.menu_button(item.label, |ui| {
                    items(ui, &item.items, shell, keymap, platform, events);
                });
            });
            continue;
        }
        let response = self::item(ui, item, shell, keymap, platform, events);
        take_focus_first(ui, &response);
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
) -> egui::Response {
    item_in(
        ui,
        item,
        shell,
        keymap,
        platform,
        &shell.active_contexts(),
        events,
    )
}

/// [`item`] with `contexts` as the ones that apply ([`item_state_in`]).
pub(crate) fn item_in(
    ui: &mut Ui,
    item: &MenuItem,
    shell: &Shell,
    keymap: &Keymap,
    platform: Platform,
    contexts: &[Context],
    events: &mut Vec<UiEvent>,
) -> egui::Response {
    let state = item_state_in(item, shell, keymap, platform, contexts);
    let mut text = state.label.clone();
    if state.checked == Some(true) {
        text = format!("✔ {text}");
    }
    let mut button = egui::Button::new(text);
    if let Some(chord) = &state.chord {
        button = button.shortcut_text(chord_text(chord, platform));
    }
    let response = ui.add_enabled(state.enabled, button);
    register_item(ui.ctx(), &response);
    if response.clicked()
        && let Some(invocation) = state.invocation
    {
        events.push(UiEvent::Invoke(invocation));
        ui.close();
    }
    response
}

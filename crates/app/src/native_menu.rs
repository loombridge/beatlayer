//! Native menus on macOS and Windows (muda), built from the menu bar model
//! (`bl_shell::menus`). Each item runs what `bl_ui::menus::item_state`
//! says, and shows its keys from the keymap.
//!
//! On macOS only chords with Cmd are attached as key equivalents, so plain
//! keys (Tab, Delete, Space) still reach the window and the keymap. On
//! Windows the keys are shown for reading; the keymap handles every press.

use bl_shell::menus::{Action, Menu, MenuItem, SystemItem};
use bl_shell::{Invocation, Shell};
use bl_ui::keymap::{Chord, Keymap, Platform};
use bl_ui::menus::item_state;
use egui::Key;
use muda::accelerator::{Accelerator, Code, Modifiers};
use muda::{CheckMenuItem, MenuId, PredefinedMenuItem, Submenu};

enum Native {
    Item(muda::MenuItem),
    Check(CheckMenuItem),
}

struct Entry {
    model: MenuItem,
    native: Native,
}

/// The native menu bar and the items it can update.
pub struct NativeMenu {
    menu: muda::Menu,
    #[cfg_attr(
        not(target_os = "macos"),
        expect(dead_code, reason = "only macOS has a Help menu role")
    )]
    help: Option<Submenu>,
    entries: Vec<Entry>,
}

impl NativeMenu {
    /// Builds the menu bar.
    pub fn build(
        menus: &[Menu],
        shell: &Shell,
        keymap: &Keymap,
        platform: Platform,
    ) -> Result<Self, String> {
        let menu = muda::Menu::new();
        let mut entries = Vec::new();
        let mut help = None;
        for model in menus {
            let title = match (platform, model.mnemonic) {
                (Platform::Other, Some(letter)) => {
                    model.title.replacen(letter, &format!("&{letter}"), 1)
                }
                _ => model.title.to_owned(),
            };
            let submenu = Submenu::new(title, true);
            add_items(
                &submenu,
                &model.items,
                shell,
                keymap,
                platform,
                &mut entries,
            )?;
            menu.append(&submenu).map_err(|e| e.to_string())?;
            if model.title == "Help" {
                help = Some(submenu);
            }
        }
        Ok(Self {
            menu,
            help,
            entries,
        })
    }

    /// Puts the menu bar on the app (macOS) or the window (Windows).
    pub fn attach(&self, window: &winit::window::Window) -> Result<(), String> {
        #[cfg(target_os = "macos")]
        {
            let _ = window;
            self.menu.init_for_nsapp();
            if let Some(help) = &self.help {
                help.set_as_help_menu_for_nsapp();
            }
            Ok(())
        }
        #[cfg(windows)]
        {
            use raw_window_handle::{HasWindowHandle, RawWindowHandle};
            let handle = window.window_handle().map_err(|e| e.to_string())?;
            let RawWindowHandle::Win32(handle) = handle.as_raw() else {
                return Err("the main window has no Win32 handle".to_owned());
            };
            // SAFETY: the handle is the live main window's, and muda is
            // called on the thread that created it.
            unsafe { self.menu.init_for_hwnd(handle.hwnd.get()) }.map_err(|e| e.to_string())
        }
    }

    /// Brings every item's label, enabled state, and check mark up to date.
    pub fn refresh(&self, shell: &Shell, keymap: &Keymap, platform: Platform) {
        for entry in &self.entries {
            let state = item_state(&entry.model, shell, keymap, platform);
            match &entry.native {
                Native::Item(item) => {
                    item.set_text(&state.label);
                    item.set_enabled(state.enabled);
                }
                Native::Check(item) => {
                    item.set_text(&state.label);
                    item.set_enabled(state.enabled);
                    item.set_checked(state.checked.unwrap_or(false));
                }
            }
        }
    }

    /// What a chosen item runs, if it's enabled now.
    pub fn invocation(
        &self,
        id: &MenuId,
        shell: &Shell,
        keymap: &Keymap,
        platform: Platform,
    ) -> Option<Invocation> {
        let entry = self.entries.iter().find(|e| e.model.row == id.as_ref())?;
        let state = item_state(&entry.model, shell, keymap, platform);
        state.enabled.then_some(state.invocation).flatten()
    }
}

fn add_items(
    parent: &Submenu,
    items: &[MenuItem],
    shell: &Shell,
    keymap: &Keymap,
    platform: Platform,
    entries: &mut Vec<Entry>,
) -> Result<(), String> {
    for item in items.iter().filter(|item| item.is_shown()) {
        let result = match item.action {
            Action::Submenu => {
                let submenu = Submenu::new(item.label, true);
                add_items(&submenu, &item.items, shell, keymap, platform, entries)?;
                parent.append(&submenu)
            }
            Action::System(SystemItem::Services) => {
                parent.append(&PredefinedMenuItem::services(None))
            }
            Action::System(SystemItem::HideOthers) => {
                parent.append(&PredefinedMenuItem::hide_others(None))
            }
            Action::System(SystemItem::ShowAll) => {
                parent.append(&PredefinedMenuItem::show_all(None))
            }
            Action::System(SystemItem::AddedByMacOs) | Action::Omitted(_) => continue,
            _ => {
                let state = item_state(item, shell, keymap, platform);
                let accelerator = state.chord.as_ref().and_then(|c| accelerator(c, platform));
                let id = MenuId::new(item.row);
                let native = if let Some(checked) = state.checked {
                    Native::Check(CheckMenuItem::with_id(
                        id,
                        &state.label,
                        state.enabled,
                        checked,
                        accelerator,
                    ))
                } else {
                    Native::Item(muda::MenuItem::with_id(
                        id,
                        &state.label,
                        state.enabled,
                        accelerator,
                    ))
                };
                let appended = match &native {
                    Native::Item(i) => parent.append(i),
                    Native::Check(i) => parent.append(i),
                };
                entries.push(Entry {
                    model: item.clone(),
                    native,
                });
                appended
            }
        };
        result.map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// The key equivalent a menu item shows, if the chord is one a menu should
/// carry.
fn accelerator(chord: &Chord, platform: Platform) -> Option<Accelerator> {
    let function_key = matches!(
        chord.key,
        Key::F1
            | Key::F2
            | Key::F3
            | Key::F4
            | Key::F5
            | Key::F6
            | Key::F7
            | Key::F8
            | Key::F9
            | Key::F10
            | Key::F11
            | Key::F12
    );
    let carried = match platform {
        Platform::Mac => chord.mods.cmd,
        Platform::Other => chord.mods.ctrl || chord.mods.alt || function_key,
    };
    if !carried {
        return None;
    }
    let mut mods = Modifiers::empty();
    for (on, modifier) in [
        (chord.mods.cmd, Modifiers::META),
        (chord.mods.ctrl, Modifiers::CONTROL),
        (chord.mods.alt, Modifiers::ALT),
        (chord.mods.shift, Modifiers::SHIFT),
    ] {
        if on {
            mods |= modifier;
        }
    }
    Some(Accelerator::new(mods, code(chord.key)?))
}

fn code(key: Key) -> Option<Code> {
    let letter = |c: char| -> Option<Code> {
        Some(match c {
            'A' => Code::KeyA,
            'B' => Code::KeyB,
            'C' => Code::KeyC,
            'D' => Code::KeyD,
            'E' => Code::KeyE,
            'F' => Code::KeyF,
            'G' => Code::KeyG,
            'H' => Code::KeyH,
            'I' => Code::KeyI,
            'J' => Code::KeyJ,
            'K' => Code::KeyK,
            'L' => Code::KeyL,
            'M' => Code::KeyM,
            'N' => Code::KeyN,
            'O' => Code::KeyO,
            'P' => Code::KeyP,
            'Q' => Code::KeyQ,
            'R' => Code::KeyR,
            'S' => Code::KeyS,
            'T' => Code::KeyT,
            'U' => Code::KeyU,
            'V' => Code::KeyV,
            'W' => Code::KeyW,
            'X' => Code::KeyX,
            'Y' => Code::KeyY,
            'Z' => Code::KeyZ,
            _ => return None,
        })
    };
    Some(match key {
        Key::Num0 => Code::Digit0,
        Key::Num1 => Code::Digit1,
        Key::Num2 => Code::Digit2,
        Key::Num3 => Code::Digit3,
        Key::Num4 => Code::Digit4,
        Key::Num5 => Code::Digit5,
        Key::Num6 => Code::Digit6,
        Key::Num7 => Code::Digit7,
        Key::Num8 => Code::Digit8,
        Key::Num9 => Code::Digit9,
        Key::Comma => Code::Comma,
        Key::Period => Code::Period,
        Key::Slash => Code::Slash,
        Key::Minus => Code::Minus,
        Key::Plus | Key::Equals => Code::Equal,
        Key::OpenBracket => Code::BracketLeft,
        Key::CloseBracket => Code::BracketRight,
        Key::Space => Code::Space,
        Key::Tab => Code::Tab,
        Key::Enter => Code::Enter,
        Key::Escape => Code::Escape,
        Key::Delete => Code::Delete,
        Key::Backspace => Code::Backspace,
        Key::ArrowUp => Code::ArrowUp,
        Key::ArrowDown => Code::ArrowDown,
        Key::ArrowLeft => Code::ArrowLeft,
        Key::ArrowRight => Code::ArrowRight,
        Key::Home => Code::Home,
        Key::End => Code::End,
        Key::F1 => Code::F1,
        Key::F2 => Code::F2,
        Key::F3 => Code::F3,
        Key::F4 => Code::F4,
        Key::F5 => Code::F5,
        Key::F6 => Code::F6,
        Key::F7 => Code::F7,
        Key::F8 => Code::F8,
        Key::F9 => Code::F9,
        Key::F10 => Code::F10,
        Key::F11 => Code::F11,
        Key::F12 => Code::F12,
        other => return letter(other.name().chars().next()?).filter(|_| other.name().len() == 1),
    })
}

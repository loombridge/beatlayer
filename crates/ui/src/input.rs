//! Key presses to commands: the keymap applied to a window's input before
//! egui sees it.

use bl_commands::Context;
use bl_shell::{Invocation, Shell};
use egui::{Event, Key, RawInput};
use serde_json::Value;

use crate::keymap::{Keymap, LATCH_HOLD_SECONDS, Mods, Platform, Trigger};

/// A key the dispatcher took, kept until it's released.
#[derive(Clone, Debug)]
struct Held {
    key: Key,
    pressed_at: f64,
    /// For a latching shortcut, what to run again if the key is held long
    /// enough to act only while held.
    latch: Option<Invocation>,
}

/// Turns key presses into command invocations through the keymap.
///
/// Before each frame, [`KeyDispatcher::take_bound_keys`] looks up every key
/// press in the keymap with the contexts that apply now, takes the presses
/// that run a command out of the input (so egui doesn't act on them too),
/// and returns the invocations. Presses that run nothing here go on to egui:
/// typing, focus movement, and the like.
#[derive(Clone, Debug)]
pub struct KeyDispatcher {
    platform: Platform,
    held: Vec<Held>,
}

impl KeyDispatcher {
    /// A dispatcher for a platform's bindings.
    #[must_use]
    pub fn new(platform: Platform) -> Self {
        Self {
            platform,
            held: Vec::new(),
        }
    }

    /// Takes the key presses the keymap binds out of `input` and returns
    /// what they run. `text_focused` says a text field has the keyboard, so
    /// only `text-field` shortcuts apply, and `tab-focus` ones while Tab
    /// moves the focus.
    pub fn take_bound_keys(
        &mut self,
        input: &mut RawInput,
        keymap: &Keymap,
        shell: &Shell,
        text_focused: bool,
    ) -> Vec<Invocation> {
        let now = input.time.unwrap_or_default();
        let contexts = if text_focused {
            // While Tab moves the focus, it does from a field too: from the
            // browser's search field, or a header's volume field (the
            // accessibility spec, R3).
            let mut contexts = vec![Context::TextField];
            if shell.tab_navigation() {
                contexts.push(Context::TabFocus);
            }
            contexts
        } else {
            shell.active_contexts()
        };
        let mut invocations = Vec::new();
        input.events.retain(|event| {
            let Event::Key {
                key,
                pressed,
                repeat,
                modifiers,
                ..
            } = event
            else {
                return true;
            };
            if let Some(index) = self.held.iter().position(|held| held.key == *key) {
                if *pressed {
                    // Auto-repeat of a key that ran a command runs nothing more.
                    return false;
                }
                let held = self.held.remove(index);
                if let Some(again) = held.latch
                    && now - held.pressed_at >= LATCH_HOLD_SECONDS
                {
                    invocations.push(again);
                }
                return false;
            }
            if !*pressed || *repeat {
                return true;
            }
            let mods = Mods::from_egui(*modifiers, self.platform);
            let Some(hit) = keymap.resolve(self.platform, *key, mods, &contexts) else {
                return true;
            };
            let mut fields = hit.command.args.clone();
            // F1 to F8 switch the track their number names (the mixer view
            // spec, R9): the shell turns the number into the track.
            if let (Some(index), Some(_)) = (hit.index, hit.command.toggles.as_ref()) {
                fields.insert(bl_shell::KEY_TRACK_NUMBER.to_owned(), Value::from(index));
            }
            let args = Value::Object(fields);
            if !shell.can_invoke(&hit.command.id, &args) {
                return true;
            }
            let invocation = Invocation::new(&hit.command.id, args);
            self.held.push(Held {
                key: *key,
                pressed_at: now,
                latch: (hit.entry.trigger == Trigger::Latch).then(|| invocation.clone()),
            });
            invocations.push(invocation);
            false
        });
        invocations
    }
}

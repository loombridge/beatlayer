//! Key presses to commands: the keymap applied to a window's input before
//! egui sees it.

use bl_commands::Context;
use bl_shell::midi_io_commands as midi_ids;
use bl_shell::{Invocation, Shell};
use egui::{Event, Key, RawInput};
use serde_json::{Value, json};

use crate::keymap::{Hit, Keymap, LATCH_HOLD_SECONDS, Platform, Trigger, mods_from_egui};

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
    /// The computer MIDI keyboard's note keys that are down, until released
    /// (the MIDI I/O spec, R21).
    notes: Vec<Key>,
}

/// What a letter does while the computer MIDI keyboard is on: A W S E D F T G
/// Y H U J K O L P play notes (S-MAP-006), Z and X move the octave
/// (S-MAP-004), C and V the velocity (S-MAP-005).
enum ComputerKey {
    Note,
    Octave(i8),
    Velocity(i8),
}

fn computer_key(key: Key) -> Option<ComputerKey> {
    use Key::{A, C, D, E, F, G, H, J, K, L, O, P, S, T, U, V, W, X, Y, Z};
    match key {
        A | W | S | E | D | F | T | G | Y | H | U | J | K | O | L | P => Some(ComputerKey::Note),
        Z => Some(ComputerKey::Octave(-1)),
        X => Some(ComputerKey::Octave(1)),
        C => Some(ComputerKey::Velocity(-1)),
        V => Some(ComputerKey::Velocity(1)),
        _ => None,
    }
}

impl KeyDispatcher {
    /// A dispatcher for a platform's bindings.
    #[must_use]
    pub fn new(platform: Platform) -> Self {
        Self {
            platform,
            held: Vec::new(),
            notes: Vec::new(),
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
        let keyboard_on = !text_focused && shell.computer_keyboard_on();
        let mut invocations = Vec::new();
        // A window that loses the keyboard never sees its keys come up, so
        // the notes they hold end now rather than hang.
        if input
            .events
            .iter()
            .any(|event| matches!(event, Event::WindowFocused(false)))
        {
            for key in self.notes.drain(..) {
                invocations.push(Invocation::new(
                    midi_ids::COMPUTER_KEYBOARD_KEY,
                    json!({ "key": key.name(), "pressed": false }),
                ));
            }
        }
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
            // Spec: midi-io R21, R22. A note key goes down and up as one
            // note, whatever else happens meanwhile.
            let note = |pressed| {
                Invocation::new(
                    midi_ids::COMPUTER_KEYBOARD_KEY,
                    json!({ "key": key.name(), "pressed": pressed }),
                )
            };
            if let Some(index) = self.notes.iter().position(|k| k == key) {
                if !*pressed {
                    self.notes.remove(index);
                    invocations.push(note(false));
                }
                return false;
            }
            // While the keyboard is on its letters play, unless Shift (or
            // Cmd, Ctrl, or Option) is down: then they run their shortcuts.
            let plain = !(modifiers.shift || modifiers.alt || modifiers.ctrl || modifiers.command);
            if keyboard_on && *pressed && !*repeat && plain {
                match computer_key(*key) {
                    Some(ComputerKey::Note) => {
                        self.notes.push(*key);
                        invocations.push(note(true));
                        return false;
                    }
                    Some(ComputerKey::Octave(steps)) => {
                        invocations.push(Invocation::new(
                            midi_ids::COMPUTER_KEYBOARD_OCTAVE,
                            json!({ "steps": steps }),
                        ));
                        return false;
                    }
                    Some(ComputerKey::Velocity(steps)) => {
                        invocations.push(Invocation::new(
                            midi_ids::COMPUTER_KEYBOARD_VELOCITY,
                            json!({ "steps": steps }),
                        ));
                        return false;
                    }
                    None => {}
                }
            }
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
            let mods = mods_from_egui(*modifiers, self.platform);
            let args_of = |hit: &Hit<'_>| {
                let mut fields = hit.command.args.clone();
                // F1 to F8 switch the track their number names (the mixer
                // view spec, R9): the shell turns the number into the track.
                if let (Some(index), Some(_)) = (hit.index, hit.command.toggles.as_ref()) {
                    fields.insert(bl_shell::KEY_TRACK_NUMBER.to_owned(), Value::from(index));
                }
                // Tab and Shift-Tab select the next and previous breakpoint:
                // the key's Shift is the command's `reverse` (the automation
                // editing spec, R22).
                if hit.command.id == bl_shell::envelope_commands::SELECT_ADJACENT {
                    fields.insert("reverse".to_owned(), Value::from(mods.shift));
                }
                Value::Object(fields)
            };
            let Some(hit) = keymap.resolve_with(self.platform, *key, mods, &contexts, |hit| {
                shell.can_invoke(&hit.command.id, &args_of(hit))
            }) else {
                return true;
            };
            let args = args_of(&hit);
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

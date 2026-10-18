//! MIDI input in the shell (WP-2.2; the MIDI I/O spec): the ports and their
//! switches, the computer MIDI keyboard, and what the Settings window, the
//! In/Out section, and the `midi.*` commands read and change.
//!
//! The device layer is `bl-midi-io`'s. This keeps the preferences and the
//! command-facing results around it.

use std::time::Instant;

use bl_core::midi::{MidiInputEvent, MidiMessage, MidiSource};
use bl_midi_io::{
    ComputerKeyboard, Direction, InputSender, KeyboardKey, MidiBackend, MidiPorts, OutputBus,
    PortState, PortSwitches,
};
use bl_shell_spec::midi_io::{
    KeyboardInfo, KeyboardKeyResult, MidiPortsResult, PortDirection, PortInfo,
};

use crate::settings::{MidiPrefs, PortPrefs};

/// The ports in use, the computer MIDI keyboard, and the queue to the
/// engine they share.
pub struct MidiHost {
    ports: MidiPorts,
    keyboard: ComputerKeyboard,
    sender: InputSender,
}

impl std::fmt::Debug for MidiHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MidiHost")
            .field("ports", &self.ports)
            .field("keyboard", &self.keyboard)
            .finish_non_exhaustive()
    }
}

const fn direction(direction: PortDirection) -> Direction {
    match direction {
        PortDirection::Input => Direction::Input,
        PortDirection::Output => Direction::Output,
    }
}

const fn port_direction(direction: Direction) -> PortDirection {
    match direction {
        Direction::Input => PortDirection::Input,
        Direction::Output => PortDirection::Output,
    }
}

fn info(state: PortState) -> PortInfo {
    PortInfo {
        direction: port_direction(state.direction),
        name: state.name,
        track: state.switches.track,
        sync: state.switches.sync,
        remote: state.switches.remote,
        present: state.present,
        open: state.open,
        error: state.error,
    }
}

impl MidiHost {
    /// The ports of `backend` with the switches `prefs` kept, the keyboard
    /// as they left it, `sender` as the way to the engine, and `output` the
    /// way from it.
    #[must_use]
    pub fn new(
        backend: Box<dyn MidiBackend>,
        sender: InputSender,
        output: OutputBus,
        prefs: &MidiPrefs,
    ) -> Self {
        let mut ports = MidiPorts::new(backend, sender.clone()).with_output(output);
        for port in &prefs.ports {
            ports.remember(
                direction(port.direction),
                &port.name,
                PortSwitches {
                    track: port.track,
                    sync: port.sync,
                    remote: port.remote,
                },
            );
        }
        let mut keyboard = ComputerKeyboard::new();
        keyboard.set_on(prefs.keyboard_on);
        let mut host = Self {
            ports,
            keyboard,
            sender,
        };
        host.refresh();
        host
    }

    /// Lists the system's ports again, opening and closing them as their
    /// switches say. A system that can't list its ports shows none.
    pub fn refresh(&mut self) {
        let _ = self.ports.refresh();
    }

    /// What the preferences keep.
    #[must_use]
    pub fn prefs(&self) -> MidiPrefs {
        MidiPrefs {
            ports: self
                .ports
                .remembered()
                .into_iter()
                .map(|(dir, name, switches)| PortPrefs {
                    direction: port_direction(dir),
                    name,
                    track: switches.track,
                    sync: switches.sync,
                    remote: switches.remote,
                })
                .collect(),
            keyboard_on: self.keyboard.is_on(),
        }
    }

    /// Every port the system offers.
    #[must_use]
    pub fn ports(&self) -> Vec<PortInfo> {
        self.ports.ports().into_iter().map(info).collect()
    }

    /// The names of the output ports the system offers whose Track switch
    /// is on, in the order the settings list them: what a MIDI track's MIDI
    /// To and an external instrument's choose from.
    #[must_use]
    pub fn output_names(&self) -> Vec<String> {
        self.ports()
            .into_iter()
            .filter(|p| p.direction == PortDirection::Output && p.track)
            .map(|p| p.name)
            .collect()
    }

    /// The bus from the engine to the output ports, for the audio host to
    /// attach a queue to whenever an engine starts.
    #[must_use]
    pub fn output(&self) -> &OutputBus {
        self.ports.output()
    }

    /// The names of the input ports the system offers, in the order the
    /// settings list them.
    #[must_use]
    pub fn input_names(&self) -> Vec<String> {
        self.ports()
            .into_iter()
            .filter(|p| p.direction == PortDirection::Input)
            .map(|p| p.name)
            .collect()
    }

    /// The ports and the keyboard, as `midi.ports` returns them.
    #[must_use]
    pub fn report(&self) -> MidiPortsResult {
        MidiPortsResult {
            system: self.ports.system().to_owned(),
            ports: self.ports(),
            keyboard: self.keyboard_info(),
        }
    }

    /// Sets the switches the arguments give.
    ///
    /// # Errors
    ///
    /// A message if the system doesn't offer the port now.
    pub fn set_port(
        &mut self,
        dir: PortDirection,
        name: &str,
        track: Option<bool>,
        sync: Option<bool>,
        remote: Option<bool>,
    ) -> Result<PortInfo, String> {
        let dir = direction(dir);
        let now = self
            .ports
            .port(dir, name)
            .filter(|p| p.present)
            .ok_or_else(|| format!("no MIDI {} port is called \"{name}\"", dir.label()))?
            .switches;
        let switches = PortSwitches {
            track: track.unwrap_or(now.track),
            sync: sync.unwrap_or(now.sync),
            remote: remote.unwrap_or(now.remote),
        };
        self.ports
            .set_switches(dir, name, switches)
            .map(info)
            .map_err(|e| e.to_string())
    }

    /// The sending end of the engine's input queue, for the audio host to
    /// attach a new consumer to whenever an engine starts.
    #[must_use]
    pub fn sender(&self) -> &InputSender {
        &self.sender
    }

    /// The computer keyboard's state.
    #[must_use]
    pub fn keyboard_info(&self) -> KeyboardInfo {
        KeyboardInfo {
            on: self.keyboard.is_on(),
            octave: self.keyboard.octave(),
            velocity: self.keyboard.velocity(),
            lowest_note: self.keyboard.lowest_note(),
            range: self.keyboard.range_text(),
        }
    }

    fn send(&self, message: MidiMessage) {
        self.sender.send(MidiInputEvent {
            source: MidiSource::ComputerKeyboard,
            time: Instant::now(),
            message,
        });
    }

    /// Switches the keyboard on or off (the other state if `on` is `None`).
    /// Switching it off ends every note it holds.
    pub fn set_keyboard_on(&mut self, on: Option<bool>) {
        let on = on.unwrap_or(!self.keyboard.is_on());
        for off in self.keyboard.set_on(on) {
            self.send(off);
        }
    }

    /// Moves the keyboard's octave `steps` octaves.
    pub fn shift_octave(&mut self, steps: i8) {
        self.keyboard.shift_octave(steps);
    }

    /// Steps the keyboard's velocity `steps` steps.
    pub fn step_velocity(&mut self, steps: i8) {
        self.keyboard.step_velocity(steps);
    }

    /// Presses or releases the note key `key` names.
    ///
    /// # Errors
    ///
    /// A message if `key` isn't one of the note keys, or a key is pressed
    /// while the keyboard is off.
    pub fn key(&mut self, key: &str, pressed: bool) -> Result<KeyboardKeyResult, String> {
        let semitone = match key.chars().collect::<Vec<_>>()[..] {
            [letter] => match KeyboardKey::from_letter(letter) {
                Some(KeyboardKey::Note(semitone)) => Some(semitone),
                _ => None,
            },
            _ => None,
        }
        .ok_or_else(|| format!("\"{key}\" isn't one of the note keys"))?;
        if pressed && !self.keyboard.is_on() {
            return Err("the computer MIDI keyboard is off".to_owned());
        }
        let message = if pressed {
            self.keyboard.press(semitone)
        } else {
            self.keyboard.release(semitone)
        };
        let note = message.map(|m| m.data1());
        if let Some(message) = message {
            self.send(message);
        }
        Ok(KeyboardKeyResult {
            sent: note.is_some(),
            note,
        })
    }
}

//! MIDI input in the shell (WP-2.2; the MIDI I/O spec): the ports and their
//! switches, the computer MIDI keyboard, and what the Settings window, the
//! In/Out section, and the `midi.*` commands read and change.
//!
//! The device layer is `bl-midi-io`'s. This keeps the preferences and the
//! command-facing results around it.

use std::time::Instant;

use bl_control::{RowConfig, RowReport, Surfaces};
use bl_core::midi::{MidiInputEvent, MidiMessage, MidiSource};
use bl_midi_io::{
    ComputerKeyboard, Direction, InputSender, KeyboardKey, MidiBackend, MidiPorts, OutputBus,
    PortState, PortSwitches,
};
use bl_shell_spec::midi_io::{
    KeyboardInfo, KeyboardKeyResult, MidiPortsResult, PortDirection, PortInfo, SYNC_DELAY_RANGE,
    SetPortArgs,
};

use crate::settings::{MidiPrefs, PortPrefs, SurfaceRowPrefs};

/// The ports in use, the computer MIDI keyboard, and the queue to the
/// engine they share.
pub struct MidiHost {
    // The surfaces come first, so they stop and say goodbye before the ports
    // they talk on close (control surfaces R9).
    surfaces: Surfaces,
    ports: MidiPorts,
    keyboard: ComputerKeyboard,
    sender: InputSender,
    /// A clock that stamps the keyboard's notes instead of the system's,
    /// for tests that drive a null device by hand (T199).
    clock: Option<bl_core::clock::TestClock>,
    /// Show Resync Button (the MIDI clock spec, R10).
    show_resync_button: bool,
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
        mpe: state.switches.mpe,
        sync_delay_ms: state.switches.delay_ms,
        clock_type: state.switches.clock_type.into(),
        hardware_resync: state.switches.resync.into(),
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
        let surfaces = Surfaces::new(sender.clone(), output.clone());
        let mut ports = MidiPorts::new(backend, sender.clone()).with_output(output);
        for port in &prefs.ports {
            ports.remember(
                direction(port.direction),
                &port.name,
                PortSwitches {
                    track: port.track,
                    sync: port.sync,
                    remote: port.remote,
                    mpe: port.mpe,
                    delay_ms: port
                        .sync_delay_ms
                        .clamp(*SYNC_DELAY_RANGE.start(), *SYNC_DELAY_RANGE.end()),
                    clock_type: port.clock_type.into(),
                    resync: port.hardware_resync.into(),
                },
            );
        }
        let mut keyboard = ComputerKeyboard::new();
        keyboard.set_on(prefs.keyboard_on);
        let mut host = Self {
            surfaces,
            ports,
            keyboard,
            sender,
            clock: None,
            show_resync_button: prefs.show_resync_button,
        };
        // Spec: control-surfaces R2. The rows apply again at once, and
        // wait for ports that aren't there.
        for (row, saved) in prefs.surfaces.iter().take(bl_control::ROWS).enumerate() {
            let config = RowConfig {
                surface: saved.surface.clone(),
                input: saved.input.clone(),
                output: saved.output.clone(),
            };
            // A saved row that now breaks a rule (two rows on one port) is
            // left empty.
            let _ = host.surfaces.restore(row, config);
        }
        host.refresh();
        host
    }

    /// Stamps the keyboard's notes from `clock` instead of the system's
    /// (T199), as the null device's manual pacing needs.
    pub fn set_test_clock(&mut self, clock: bl_core::clock::TestClock) {
        self.surfaces.set_clock(clock.clone());
        self.clock = Some(clock);
    }

    /// The test clock that stamps the keyboard's notes, if one was given.
    #[must_use]
    pub fn test_clock(&self) -> Option<bl_core::clock::TestClock> {
        self.clock.clone()
    }

    /// Lists the system's ports again, opening and closing them as their
    /// switches say. A system that can't list its ports shows none.
    pub fn refresh(&mut self) {
        let _ = self.ports.refresh();
        // Spec: control-surfaces R9. A surface stops when its port goes and
        // starts when it comes back.
        self.surfaces.reconcile(&mut self.ports);
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
                    mpe: switches.mpe,
                    sync_delay_ms: switches.delay_ms,
                    clock_type: switches.clock_type.into(),
                    hardware_resync: switches.resync.into(),
                })
                .collect(),
            keyboard_on: self.keyboard.is_on(),
            surfaces: {
                let mut rows: Vec<SurfaceRowPrefs> = self
                    .surfaces
                    .rows()
                    .iter()
                    .map(|row| SurfaceRowPrefs {
                        surface: row.surface.clone(),
                        input: row.input.clone(),
                        output: row.output.clone(),
                    })
                    .collect();
                // Empty rows at the end aren't kept.
                while rows.last() == Some(&SurfaceRowPrefs::default()) {
                    rows.pop();
                }
                rows
            },
            show_resync_button: self.show_resync_button,
        }
    }

    /// The control surfaces and the table of their rows.
    #[must_use]
    pub const fn surfaces(&self) -> &Surfaces {
        &self.surfaces
    }

    /// Sets the controller templates the chooser offers (control surfaces
    /// R43).
    pub fn set_surface_templates(&mut self, templates: Vec<std::sync::Arc<bl_control::Kind>>) {
        self.surfaces.set_templates(templates);
        self.refresh();
    }

    /// The six rows as the Control Surfaces table shows them.
    #[must_use]
    pub fn surface_rows(&self) -> Vec<RowReport> {
        self.surfaces.report(&self.ports)
    }

    /// Sets a row (control surfaces R1, R3), and starts or stops what the
    /// change calls for.
    ///
    /// # Errors
    ///
    /// A message if the surface isn't offered or a port is another row's.
    pub fn set_surface_row(&mut self, row: usize, config: RowConfig) -> Result<(), String> {
        self.surfaces.set_row(row, config)?;
        self.refresh();
        Ok(())
    }

    /// Sends a row's surface's setup to its output (R11); how many
    /// messages.
    ///
    /// # Errors
    ///
    /// A message if the row has no setup to send.
    pub fn dump_surface(&self, row: usize) -> Result<usize, String> {
        self.surfaces.dump(row, &self.ports)
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

    /// The names of the input ports whose MPE switch is on.
    #[must_use]
    pub fn mpe_input_names(&self) -> Vec<String> {
        self.ports()
            .into_iter()
            .filter(|p| p.direction == PortDirection::Input && p.mpe)
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
            show_resync_button: self.show_resync_button,
        }
    }

    /// Sets the switches and sync options the arguments give.
    ///
    /// # Errors
    ///
    /// A message if the system doesn't offer the port now, or an option
    /// doesn't fit the port or its range.
    pub fn set_port(&mut self, args: &SetPortArgs) -> Result<PortInfo, String> {
        let (dir, name) = (direction(args.direction), args.name.as_str());
        let now = self
            .ports
            .port(dir, name)
            .filter(|p| p.present)
            .ok_or_else(|| format!("no MIDI {} port is called \"{name}\"", dir.label()))?
            .switches;
        // Spec: midi-io R27. An output has no MPE switch.
        if args.mpe.is_some() && dir == Direction::Output {
            return Err(format!(
                "the MIDI output port \"{name}\" has no MPE switch: a track sends MPE out through its MIDI To"
            ));
        }
        // Spec: midi-clock R1, R2, R14. An input has a Sync Delay and no more.
        if (args.clock_type.is_some() || args.hardware_resync.is_some()) && dir == Direction::Input
        {
            return Err(format!(
                "the MIDI input port \"{name}\" has no MIDI Clock Type or Hardware Resync: they are an output's"
            ));
        }
        if let Some(delay) = args.sync_delay_ms
            && !SYNC_DELAY_RANGE.contains(&delay)
        {
            return Err(format!(
                "MIDI Clock Sync Delay runs from {} to {} ms, not {delay}",
                SYNC_DELAY_RANGE.start(),
                SYNC_DELAY_RANGE.end()
            ));
        }
        let switches = PortSwitches {
            track: args.track.unwrap_or(now.track),
            sync: args.sync.unwrap_or(now.sync),
            remote: args.remote.unwrap_or(now.remote),
            mpe: args.mpe.unwrap_or(now.mpe),
            delay_ms: args.sync_delay_ms.unwrap_or(now.delay_ms),
            clock_type: args.clock_type.map_or(now.clock_type, Into::into),
            resync: args.hardware_resync.map_or(now.resync, Into::into),
        };
        self.ports
            .set_switches(dir, name, switches)
            .map(info)
            .map_err(|e| e.to_string())
    }

    /// The open outputs with Sync on and their options: where the engine
    /// sends MIDI clock.
    #[must_use]
    pub fn sync_outputs(&self) -> Vec<bl_core::midi::SyncOutput> {
        self.ports.sync_outputs()
    }

    /// Whether Show Resync Button is on.
    #[must_use]
    pub fn show_resync_button(&self) -> bool {
        self.show_resync_button
    }

    /// Sets Show Resync Button (the other state if `on` is `None`) and
    /// returns it.
    pub fn set_show_resync_button(&mut self, on: Option<bool>) -> bool {
        self.show_resync_button = on.unwrap_or(!self.show_resync_button);
        self.show_resync_button
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
        self.send_as(MidiSource::ComputerKeyboard, message);
    }

    /// Queues `message` for the engine as coming from `source`, stamped now.
    pub fn send_as(&self, source: MidiSource, message: MidiMessage) {
        self.sender.send(MidiInputEvent {
            source,
            time: self.now(),
            message,
        });
    }

    /// The time messages are stamped with: the test clock's when there is
    /// one.
    #[must_use]
    pub fn now(&self) -> Instant {
        self.clock
            .as_ref()
            .map_or_else(Instant::now, |clock| clock.now())
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

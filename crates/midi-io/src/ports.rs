//! The ports in use: their switches, opening and closing them, and where an
//! input's messages go (`docs/specs/midi-io.md`, R1 to R5).

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering::Relaxed};

use bl_core::midi::{MidiInputEvent, MidiMessage, MidiSource, PortId};

use crate::backend::{Direction, InputConnection, InputHandler, MidiBackend, MidiError};
use crate::output::{OutputBus, OutputStats};
use crate::queue::InputSender;

/// A port's switches in the Tempo & MIDI settings' MIDI ports table
/// (L-SET-014): what the port is for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct PortSwitches {
    /// An input's messages reach tracks; an output takes tracks' MIDI.
    pub track: bool,
    /// An input's clock and transport messages drive sync; an output sends
    /// clock (Phase 7).
    pub sync: bool,
    /// An input controls Beatlayer through MIDI mapping; an output gets
    /// feedback for mapped controls (Phase 7).
    pub remote: bool,
}

impl PortSwitches {
    /// Every switch off.
    pub const OFF: Self = Self {
        track: false,
        sync: false,
        remote: false,
    };

    /// Whether any switch is on, so the port is open (R4).
    #[must_use]
    pub const fn any(self) -> bool {
        self.track || self.sync || self.remote
    }

    /// A port seen for the first time: Track on for an input, every switch
    /// off for an output (R3).
    #[must_use]
    pub const fn first_seen(direction: Direction) -> Self {
        match direction {
            Direction::Input => Self {
                track: true,
                ..Self::OFF
            },
            Direction::Output => Self::OFF,
        }
    }

    const fn bits(self) -> u8 {
        self.track as u8 | (self.sync as u8) << 1 | (self.remote as u8) << 2
    }

    const fn from_bits(bits: u8) -> Self {
        Self {
            track: bits & 1 != 0,
            sync: bits & 2 != 0,
            remote: bits & 4 != 0,
        }
    }
}

/// A port as the settings show it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortState {
    /// Input or output.
    pub direction: Direction,
    /// Its name.
    pub name: String,
    /// Its switches.
    pub switches: PortSwitches,
    /// Whether the system offers it now.
    pub present: bool,
    /// Whether it's open.
    pub open: bool,
    /// Why it couldn't be opened, if it couldn't.
    pub error: Option<String>,
}

/// A port the system offers, or one whose switches are remembered.
struct Entry {
    direction: Direction,
    name: String,
    switches: PortSwitches,
    present: bool,
    error: Option<String>,
    /// While open: the input's connection and the switches its handler
    /// reads, so a switch takes effect without reopening.
    input: Option<(Box<dyn InputConnection>, Arc<AtomicU8>)>,
    /// Whether the output is open; its connection is in the output
    /// thread's table.
    output: bool,
}

impl Entry {
    fn is_open(&self) -> bool {
        self.input.is_some() || self.output
    }

    fn state(&self) -> PortState {
        PortState {
            direction: self.direction,
            name: self.name.clone(),
            switches: self.switches,
            present: self.present,
            open: self.is_open(),
            error: self.error.clone(),
        }
    }
}

/// The MIDI ports in use.
///
/// It lists the system's ports ([`MidiPorts::refresh`]), keeps each port's
/// switches by direction and name, and opens a port while any of its
/// switches is on (R3, R4). An open input's messages go, on the MIDI
/// system's thread:
///
/// - with Track on, to the engine on the input queue, stamped with the
///   port's [`PortId`] and the time they arrived, to reach the tracks whose
///   input takes them (R5);
/// - with Remote on, to MIDI mapping, and with Sync on, clock and transport
///   messages to sync: both are counted until Phase 7 takes them.
///
/// System messages never go to tracks (R9), and system exclusive goes
/// nowhere yet.
pub struct MidiPorts {
    backend: Box<dyn MidiBackend>,
    sender: InputSender,
    ports: Vec<Entry>,
    /// The open outputs and the thread that sends to them.
    output: OutputBus,
    /// What the bus knows these ports by.
    owner: u64,
}

impl MidiPorts {
    /// The ports of `backend`, sending input to the engine through `sender`.
    /// Nothing is listed or opened until [`MidiPorts::refresh`].
    #[must_use]
    pub fn new(backend: Box<dyn MidiBackend>, sender: InputSender) -> Self {
        let output = OutputBus::new();
        Self {
            backend,
            sender,
            ports: Vec::new(),
            owner: output.owner(),
            output,
        }
    }

    /// These ports on `output` from now on, the bus the engine's queue and
    /// the output thread belong to (`docs/specs/external-devices.md`, R5).
    /// Call it before any port opens.
    #[must_use]
    pub fn with_output(mut self, output: OutputBus) -> Self {
        self.output.clear(self.owner);
        self.owner = output.owner();
        self.output = output;
        self
    }

    /// The MIDI output bus the open outputs are on.
    #[must_use]
    pub fn output(&self) -> &OutputBus {
        &self.output
    }

    /// The counts for everything the output thread has handled.
    #[must_use]
    pub fn output_stats(&self) -> &Arc<OutputStats> {
        self.output.stats()
    }

    /// The MIDI system's name.
    #[must_use]
    pub fn system(&self) -> &'static str {
        self.backend.name()
    }

    /// The sending end of the engine's input queue, which the computer
    /// keyboard shares.
    #[must_use]
    pub fn sender(&self) -> &InputSender {
        &self.sender
    }

    /// Sets a port's switches from the preferences, whether or not the
    /// system offers it now. Takes effect at the next refresh.
    pub fn remember(&mut self, direction: Direction, name: &str, switches: PortSwitches) {
        match self.entry_mut(direction, name) {
            Some(entry) => entry.switches = switches,
            None => self.ports.push(Entry {
                direction,
                name: name.to_owned(),
                switches,
                present: false,
                error: None,
                input: None,
                output: false,
            }),
        }
    }

    fn entry_mut(&mut self, direction: Direction, name: &str) -> Option<&mut Entry> {
        self.ports
            .iter_mut()
            .find(|e| e.direction == direction && e.name == name)
    }

    /// Lists the system's ports again: new ones get their first switches,
    /// ports that went away close, and every port that's present opens or
    /// closes as its switches say.
    ///
    /// # Errors
    ///
    /// [`MidiError`] if the system can't list its ports; the ports stay as
    /// they were.
    pub fn refresh(&mut self) -> Result<(), MidiError> {
        let inputs = self.backend.ports(Direction::Input)?;
        let outputs = self.backend.ports(Direction::Output)?;
        for entry in &mut self.ports {
            let listed = match entry.direction {
                Direction::Input => &inputs,
                Direction::Output => &outputs,
            };
            entry.present = listed.contains(&entry.name);
        }
        for (direction, listed) in [(Direction::Input, inputs), (Direction::Output, outputs)] {
            for name in listed {
                if self.entry_mut(direction, &name).is_none() {
                    self.ports.push(Entry {
                        direction,
                        switches: PortSwitches::first_seen(direction),
                        name,
                        present: true,
                        error: None,
                        input: None,
                        output: false,
                    });
                }
            }
        }
        for index in 0..self.ports.len() {
            self.apply(index);
        }
        Ok(())
    }

    /// Opens or closes a port as its switches say, and tells an open
    /// input's handler the switches.
    fn apply(&mut self, index: usize) {
        let entry = &mut self.ports[index];
        let wanted = entry.present && entry.switches.any();
        let port = PortId::from_name(&entry.name);
        if !wanted {
            entry.input = None;
            if std::mem::take(&mut entry.output) {
                self.output.remove(self.owner, port);
            }
            if !entry.present {
                entry.error = None;
            }
            return;
        }
        if let Some((_, switches)) = &entry.input {
            switches.store(entry.switches.bits(), Relaxed);
            return;
        }
        if entry.output {
            self.output
                .set_track(self.owner, port, entry.switches.track);
            return;
        }
        let opened = match entry.direction {
            Direction::Input => {
                let switches = Arc::new(AtomicU8::new(entry.switches.bits()));
                let handler = handler(&entry.name, Arc::clone(&switches), self.sender.clone());
                self.backend
                    .open_input(&entry.name, handler)
                    .map(|connection| entry.input = Some((connection, switches)))
            }
            Direction::Output => self.backend.open_output(&entry.name).map(|connection| {
                self.output
                    .insert(self.owner, port, entry.switches.track, connection);
                entry.output = true;
            }),
        };
        entry.error = opened.err().map(|error| error.to_string());
    }

    /// Every port the system offers, inputs then outputs, each in the
    /// system's order of discovery.
    #[must_use]
    pub fn ports(&self) -> Vec<PortState> {
        let mut ports: Vec<PortState> = self
            .ports
            .iter()
            .filter(|e| e.present)
            .map(Entry::state)
            .collect();
        ports.sort_by_key(|p| p.direction);
        ports
    }

    /// Every port whose switches are kept, present or not, for the
    /// preferences.
    #[must_use]
    pub fn remembered(&self) -> Vec<(Direction, String, PortSwitches)> {
        self.ports
            .iter()
            .map(|e| (e.direction, e.name.clone(), e.switches))
            .collect()
    }

    /// Sets a port's switches, opening or closing it as they say (R4).
    ///
    /// # Errors
    ///
    /// [`MidiError::NoSuchPort`] if the system doesn't offer the port now.
    /// A port that can't be opened keeps its switches; its state says why.
    pub fn set_switches(
        &mut self,
        direction: Direction,
        name: &str,
        switches: PortSwitches,
    ) -> Result<PortState, MidiError> {
        let index = self
            .ports
            .iter()
            .position(|e| e.direction == direction && e.name == name && e.present)
            .ok_or_else(|| MidiError::NoSuchPort(name.to_owned()))?;
        self.ports[index].switches = switches;
        self.apply(index);
        Ok(self.ports[index].state())
    }

    /// A port's state, if the system offers it or its switches are kept.
    #[must_use]
    pub fn port(&self, direction: Direction, name: &str) -> Option<PortState> {
        self.ports
            .iter()
            .find(|e| e.direction == direction && e.name == name)
            .map(Entry::state)
    }
}

impl Drop for MidiPorts {
    fn drop(&mut self) {
        // The ports this system opened close with it.
        self.output.clear(self.owner);
    }
}

impl std::fmt::Debug for MidiPorts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MidiPorts")
            .field("system", &self.backend.name())
            .field(
                "ports",
                &self.ports.iter().map(Entry::state).collect::<Vec<_>>(),
            )
            .finish_non_exhaustive()
    }
}

/// Whether a system message is one sync follows: clock, start, continue,
/// stop, song position, or time code.
const fn is_sync(message: &MidiMessage) -> bool {
    matches!(message.status(), 0xF1 | 0xF2 | 0xF8 | 0xFA | 0xFB | 0xFC)
}

/// The handler for an open input: it routes each message by the port's
/// switches, on the MIDI system's thread.
fn handler(name: &str, switches: Arc<AtomicU8>, sender: InputSender) -> InputHandler {
    let source = MidiSource::Port(PortId::from_name(name));
    Box::new(move |time, bytes| {
        let on = PortSwitches::from_bits(switches.load(Relaxed));
        let stats = sender.stats();
        let Some(message) = MidiMessage::from_bytes(bytes) else {
            stats.ignored.fetch_add(1, Relaxed);
            return;
        };
        if message.is_system() {
            // Spec: midi-io R5, R9
            let counter = if on.sync && is_sync(&message) {
                &stats.to_sync
            } else {
                &stats.ignored
            };
            counter.fetch_add(1, Relaxed);
            return;
        }
        if on.track {
            sender.send(MidiInputEvent {
                source,
                time,
                message,
            });
        }
        if on.remote {
            stats.to_remote.fetch_add(1, Relaxed);
        }
        if !on.track && !on.remote {
            stats.ignored.fetch_add(1, Relaxed);
        }
    })
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;
    use crate::queue::input_queue;
    use crate::test_backend::TestBackend;

    fn ports(test: &TestBackend) -> (MidiPorts, rtrb::Consumer<MidiInputEvent>) {
        let (sender, consumer) = input_queue(64);
        let mut ports = MidiPorts::new(Box::new(test.clone()), sender);
        ports.refresh().unwrap();
        (ports, consumer)
    }

    // Spec: midi-io R1, R3, R4
    #[test]
    fn ports_are_listed_and_open_while_a_switch_is_on() {
        let test = TestBackend::new();
        test.add_port(Direction::Input, "Keys");
        test.add_port(Direction::Output, "Synth");
        let (mut ports, _engine) = ports(&test);
        let listed = ports.ports();
        assert_eq!(listed.len(), 2);
        assert_eq!(
            (listed[0].direction, listed[0].name.as_str()),
            (Direction::Input, "Keys")
        );
        assert_eq!(
            listed[0].switches,
            PortSwitches::first_seen(Direction::Input)
        );
        assert!(listed[0].open && test.is_open(Direction::Input, "Keys"));
        assert_eq!(listed[1].switches, PortSwitches::OFF);
        assert!(!listed[1].open && !test.is_open(Direction::Output, "Synth"));

        let synth = ports
            .set_switches(
                Direction::Output,
                "Synth",
                PortSwitches {
                    sync: true,
                    ..PortSwitches::OFF
                },
            )
            .unwrap();
        assert!(synth.open && test.is_open(Direction::Output, "Synth"));
        ports
            .set_switches(Direction::Input, "Keys", PortSwitches::OFF)
            .unwrap();
        assert!(!test.is_open(Direction::Input, "Keys"));
        assert!(matches!(
            ports.set_switches(Direction::Input, "Gone", PortSwitches::OFF),
            Err(MidiError::NoSuchPort(_))
        ));
    }

    // Spec: midi-io R3, R8
    #[test]
    fn switches_are_kept_by_name_through_an_unplug() {
        let test = TestBackend::new();
        let (mut ports, _engine) = ports(&test);
        let remote = PortSwitches {
            remote: true,
            ..PortSwitches::OFF
        };
        ports.remember(Direction::Input, "Pads", remote);
        ports.refresh().unwrap();
        assert!(ports.ports().is_empty(), "absent ports aren't listed");
        assert!(!test.is_open(Direction::Input, "Pads"));

        test.add_port(Direction::Input, "Pads");
        ports.refresh().unwrap();
        assert_eq!(ports.ports()[0].switches, remote);
        assert!(test.is_open(Direction::Input, "Pads"));

        test.remove_port(Direction::Input, "Pads");
        ports.refresh().unwrap();
        let kept = ports.port(Direction::Input, "Pads").unwrap();
        assert!(!kept.present && !kept.open);
        assert_eq!(
            ports.remembered(),
            vec![(Direction::Input, "Pads".to_owned(), remote)]
        );
    }

    // Spec: midi-io R4
    #[test]
    fn a_port_that_cant_open_keeps_its_switches_and_says_why() {
        let test = TestBackend::new();
        test.add_port(Direction::Input, "Busy");
        test.refuse(Direction::Input, "Busy", "another program has it");
        let (ports, _engine) = ports(&test);
        let busy = &ports.ports()[0];
        assert!(busy.switches.track && !busy.open);
        assert_eq!(busy.error.as_deref(), Some("another program has it"));
    }

    // Spec: midi-io R5, R9
    #[test]
    fn an_inputs_switches_decide_where_its_messages_go() {
        let test = TestBackend::new();
        test.add_port(Direction::Input, "Keys");
        let (mut ports, mut engine) = ports(&test);
        let stats = Arc::clone(ports.sender().stats());
        let now = Instant::now();
        let note = [0x91, 60, 100];

        // Track on: notes go to the engine, stamped; clock goes nowhere.
        assert!(test.send("Keys", now, &note));
        assert!(test.send("Keys", now, &[0xF8]));
        assert!(test.send("Keys", now, &[0xF0, 1, 2, 0xF7]));
        let event = engine.pop().unwrap();
        assert_eq!(event.source, MidiSource::Port(PortId::from_name("Keys")));
        assert_eq!(event.time, now);
        assert_eq!(event.message.bytes(), &note);
        assert!(engine.pop().is_err());

        // Remote and Sync only: nothing reaches the engine.
        ports
            .set_switches(
                Direction::Input,
                "Keys",
                PortSwitches {
                    track: false,
                    sync: true,
                    remote: true,
                },
            )
            .unwrap();
        assert!(test.send("Keys", now, &note));
        assert!(test.send("Keys", now, &[0xF8]));
        assert!(engine.pop().is_err());
        let counts = stats.snapshot();
        assert_eq!(
            (
                counts.to_tracks,
                counts.to_remote,
                counts.to_sync,
                counts.ignored
            ),
            (1, 1, 1, 2)
        );
    }
}

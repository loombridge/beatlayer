//! The ports in use: their switches, opening and closing them, and where an
//! input's messages go (`docs/specs/midi-io.md`, R1 to R5).

use std::sync::atomic::{AtomicU8, AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use bl_core::midi::{
    ClockType, HardwareResync, MidiInputEvent, MidiMessage, MidiSource, PortId, SyncInput,
    SyncOutput, SyncType, TimecodeSettings,
};
use bl_core::timecode::{Timecode, TimecodeRate};

use crate::backend::{Direction, InputConnection, InputHandler, MidiBackend, MidiError, PortKey};
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
    /// feedback for mapped controls (`docs/specs/remote-mapping.md`, R38).
    pub remote: bool,
    /// An input's messages are read as MPE, each note on a channel of its
    /// own (`docs/specs/midi-io.md`, R27); an output's has no meaning yet.
    pub mpe: bool,
    /// An output's MIDI Clock Sync Delay in milliseconds, from -100 to 100
    /// (`docs/specs/midi-clock.md`, R1, R2). Kept while Sync is off.
    pub delay_ms: i16,
    /// An output's MIDI Clock Type.
    pub clock_type: ClockType,
    /// An output's Hardware Resync.
    pub resync: HardwareResync,
    /// An input's MIDI Sync Type: whether Sync follows its clock or its
    /// MIDI Timecode (`docs/specs/midi-timecode.md`, R1).
    pub sync_type: SyncType,
    /// An input's MIDI Timecode Frame Rate. Kept while the type is clock.
    pub timecode_rate: TimecodeRate,
    /// An input's MIDI Timecode Offset. Kept while the type is clock.
    pub timecode_offset: Timecode,
}

impl PortSwitches {
    /// Every switch off.
    pub const OFF: Self = Self {
        track: false,
        sync: false,
        remote: false,
        mpe: false,
        delay_ms: 0,
        clock_type: ClockType::Song,
        resync: HardwareResync::StopAndStart,
        sync_type: SyncType::Clock,
        timecode_rate: TimecodeRate::All,
        timecode_offset: Timecode::ZERO,
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
        self.track as u8
            | (self.sync as u8) << 1
            | (self.remote as u8) << 2
            | (self.mpe as u8) << 3
            | (matches!(self.sync_type, SyncType::Timecode) as u8) << 4
    }

    const fn from_bits(bits: u8) -> Self {
        Self {
            track: bits & 1 != 0,
            sync: bits & 2 != 0,
            remote: bits & 4 != 0,
            mpe: bits & 8 != 0,
            sync_type: if bits & 16 != 0 {
                SyncType::Timecode
            } else {
                SyncType::Clock
            },
            ..Self::OFF
        }
    }
}

/// What a control surface takes from its input, called on the MIDI system's
/// thread with each message's arrival time and whole bytes (a system
/// exclusive message included).
pub type SurfaceSink = Box<dyn Fn(Instant, &[u8]) + Send + Sync>;

/// What an input's handler and the main thread share: the switches the
/// handler reads, so a switch takes effect without reopening, the control
/// surface that holds the input, if one does, and the notes whose note-on
/// reached the tracks.
#[derive(Default)]
pub struct InputLive {
    switches: AtomicU8,
    surface: Mutex<Option<SurfaceSink>>,
    /// One bit for each channel and key whose note-on went to the tracks:
    /// two words a channel.
    to_tracks: [AtomicU64; 32],
}

impl InputLive {
    /// The port's switches now.
    #[must_use]
    pub fn switches(&self) -> PortSwitches {
        PortSwitches::from_bits(self.switches.load(Relaxed))
    }

    /// Sets where the input's messages go that no mapping takes (control
    /// surfaces R4): to `sink`, and to no track, or the way the switches say
    /// with `None`.
    pub fn hold(&self, sink: Option<SurfaceSink>) {
        *self.surface.lock().unwrap_or_else(PoisonError::into_inner) = sink;
    }

    /// Sets or clears the bit of a note message's channel and key; whether
    /// it was set.
    fn mark(&self, message: &MidiMessage, set: bool) -> bool {
        let channel = usize::from(message.channel().unwrap_or(0));
        let key = usize::from(message.data1() & 127);
        let (word, bit) = (&self.to_tracks[channel * 2 + key / 64], 1 << (key % 64));
        let before = if set {
            word.fetch_or(bit, Relaxed)
        } else {
            word.fetch_and(!bit, Relaxed)
        };
        before & bit != 0
    }

    /// Ends every note that reached the tracks, for an input that closes.
    fn end_notes(&self, source: MidiSource, sender: &InputSender) {
        for (index, word) in self.to_tracks.iter().enumerate() {
            let mut bits = word.swap(0, Relaxed);
            while bits != 0 {
                let key = bits.trailing_zeros() as usize;
                bits &= bits - 1;
                sender.send(MidiInputEvent {
                    source,
                    time: Instant::now(),
                    message: MidiMessage::note_off((index / 2) as u8, (index % 2 * 64 + key) as u8),
                });
            }
        }
    }

    /// Hands a message to the surface that holds the input; whether one
    /// does.
    fn to_surface(&self, time: Instant, bytes: &[u8]) -> bool {
        let surface = self.surface.lock().unwrap_or_else(PoisonError::into_inner);
        surface.as_ref().is_some_and(|sink| {
            sink(time, bytes);
            true
        })
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
    /// While open: the input's connection.
    input: Option<Box<dyn InputConnection>>,
    /// The switches its handler reads and the surface that holds it.
    live: Arc<InputLive>,
    /// Whether a control surface holds the port, which keeps it open
    /// whatever its switches say (control surfaces R3).
    claimed: bool,
    /// Whether the output is open; its connection is in the output
    /// thread's table.
    output: bool,
    /// The system's identity for the port when it was last listed.
    key: Option<PortKey>,
}

impl Entry {
    fn new(direction: Direction, name: &str, switches: PortSwitches, present: bool) -> Self {
        let live = InputLive::default();
        live.switches.store(switches.bits(), Relaxed);
        Self {
            direction,
            name: name.to_owned(),
            switches,
            present,
            error: None,
            input: None,
            live: Arc::new(live),
            claimed: false,
            output: false,
            key: None,
        }
    }

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
/// - with Remote on, to remote control when a mapping takes them, which
///   keeps them from tracks (`docs/specs/remote-mapping.md`, R7 and R8),
///   and with Sync on, clock and transport messages to the engine on the
///   same queue, stamped as sync's own (R14 of the MIDI clock spec).
///
/// System messages never go to tracks (R9), and system exclusive goes
/// to a control surface that holds the input, and nowhere else.
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
            None => self
                .ports
                .push(Entry::new(direction, name, switches, false)),
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
        for index in 0..self.ports.len() {
            let entry = &mut self.ports[index];
            let listed = match entry.direction {
                Direction::Input => &inputs,
                Direction::Output => &outputs,
            };
            let key = listed
                .iter()
                .find(|(name, _)| *name == entry.name)
                .map(|(_, key)| key);
            entry.present = key.is_some();
            let Some(key) = key else { continue };
            // Spec: midi-io R3. A port unplugged and plugged in again
            // between two listings has a new identity: its connection is
            // dead, so it opens again.
            if entry.key.as_ref().is_some_and(|old| old != key) {
                self.close(index);
            }
            self.ports[index].key = Some(key.clone());
        }
        for (direction, listed) in [(Direction::Input, inputs), (Direction::Output, outputs)] {
            for (name, key) in listed {
                if self.entry_mut(direction, &name).is_none() {
                    let switches = PortSwitches::first_seen(direction);
                    let mut entry = Entry::new(direction, &name, switches, true);
                    entry.key = Some(key);
                    self.ports.push(entry);
                }
            }
        }
        for index in 0..self.ports.len() {
            self.apply(index);
        }
        Ok(())
    }

    /// Closes a port, and ends the notes an input's tracks still hold from
    /// it (Spec: midi-io R26).
    fn close(&mut self, index: usize) {
        let entry = &mut self.ports[index];
        let port = PortId::from_name(&entry.name);
        entry.input = None;
        entry
            .live
            .end_notes(source(port, entry.switches.mpe), &self.sender);
        if std::mem::take(&mut entry.output) {
            self.output.remove(self.owner, port);
        }
    }

    /// Opens or closes a port as its switches say, and tells an open
    /// input's handler the switches.
    fn apply(&mut self, index: usize) {
        let entry = &mut self.ports[index];
        let wanted = entry.present && (entry.switches.any() || entry.claimed);
        let port = PortId::from_name(&entry.name);
        if !wanted {
            self.close(index);
            let entry = &mut self.ports[index];
            if !entry.present {
                entry.error = None;
            }
            return;
        }
        entry.live.switches.store(entry.switches.bits(), Relaxed);
        if entry.input.is_some() {
            return;
        }
        if entry.output {
            self.output
                .set_switches(self.owner, port, entry.switches.track, entry.switches.sync);
            return;
        }
        let opened = match entry.direction {
            Direction::Input => {
                let handler = handler(&entry.name, Arc::clone(&entry.live), self.sender.clone());
                self.backend
                    .open_input(&entry.name, handler)
                    .map(|connection| entry.input = Some(connection))
            }
            Direction::Output => self.backend.open_output(&entry.name).map(|connection| {
                self.output.insert(
                    self.owner,
                    port,
                    entry.switches.track,
                    entry.switches.sync,
                    connection,
                );
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

    /// The open outputs with Sync on and their options: what the engine
    /// sends clock to (`docs/specs/midi-clock.md`, R3).
    #[must_use]
    pub fn sync_outputs(&self) -> Vec<SyncOutput> {
        self.ports
            .iter()
            .filter(|e| e.direction == Direction::Output && e.output && e.switches.sync)
            .map(|e| SyncOutput {
                port: PortId::from_name(&e.name),
                delay_ms: e.switches.delay_ms,
                clock_type: e.switches.clock_type,
                resync: e.switches.resync,
            })
            .collect()
    }

    /// The open inputs with Sync on and their Sync Delay: what the engine's
    /// clock follower reads their messages with (`docs/specs/midi-clock.md`,
    /// R14).
    #[must_use]
    pub fn sync_inputs(&self) -> Vec<SyncInput> {
        self.ports
            .iter()
            .filter(|e| e.direction == Direction::Input && e.input.is_some() && e.switches.sync)
            .map(|e| SyncInput {
                port: PortId::from_name(&e.name),
                delay_ms: e.switches.delay_ms,
                timecode: (e.switches.sync_type == SyncType::Timecode).then_some(
                    TimecodeSettings {
                        rate: e.switches.timecode_rate,
                        offset: e.switches.timecode_offset,
                    },
                ),
            })
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

    /// Holds the port for a control surface (`docs/specs/control-surfaces.md`,
    /// R3): it opens whatever its switches say, and stays open while held.
    /// The port needn't be present yet; it opens when it appears. Returns
    /// what the input's handler shares: the surface sets
    /// [`InputLive::hold`] on it, and its switches say whether a message
    /// handed back reaches tracks.
    pub fn claim(&mut self, direction: Direction, name: &str) -> Arc<InputLive> {
        let index = self.index_or_new(direction, name);
        self.ports[index].claimed = true;
        self.apply(index);
        Arc::clone(&self.ports[index].live)
    }

    /// Lets go of a port a control surface held: the port closes if no
    /// switch is on, and an input's messages go the way its switches say.
    pub fn release(&mut self, direction: Direction, name: &str) {
        let Some(index) = self
            .ports
            .iter()
            .position(|e| e.direction == direction && e.name == name)
        else {
            return;
        };
        let entry = &mut self.ports[index];
        entry.claimed = false;
        entry.live.hold(None);
        self.apply(index);
    }

    fn index_or_new(&mut self, direction: Direction, name: &str) -> usize {
        self.ports
            .iter()
            .position(|e| e.direction == direction && e.name == name)
            .unwrap_or_else(|| {
                let switches = PortSwitches::first_seen(direction);
                self.ports
                    .push(Entry::new(direction, name, switches, false));
                self.ports.len() - 1
            })
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

/// The time of a MIDI Time Code full-frame message, F0 7F (device) 01 01
/// hours-and-rate minutes seconds frames F7.
fn full_frame(bytes: &[u8]) -> Option<MidiMessage> {
    let &[
        0xF0,
        0x7F,
        _,
        0x01,
        0x01,
        hours,
        minutes,
        seconds,
        frames,
        0xF7,
    ] = bytes
    else {
        return None;
    };
    MidiMessage::full_frame(hours, minutes, seconds, frames)
}

/// What an input's messages are stamped as.
const fn source(port: PortId, mpe: bool) -> MidiSource {
    if mpe {
        MidiSource::MpePort(port)
    } else {
        MidiSource::Port(port)
    }
}

/// The handler for an open input: it routes each message by the port's
/// switches, on the MIDI system's thread.
fn handler(name: &str, live: Arc<InputLive>, sender: InputSender) -> InputHandler {
    let port = PortId::from_name(name);
    Box::new(move |time, bytes| {
        let on = live.switches();
        let source = source(port, on.mpe);
        let stats = sender.stats();
        let timecode = on.sync && on.sync_type == SyncType::Timecode;
        let Some(message) = MidiMessage::from_bytes(bytes) else {
            // Spec: midi-timecode R2. A full-frame message is read before
            // anything is refused as too long, and goes to sync as a
            // message of fixed size.
            if timecode && let Some(full) = full_frame(bytes) {
                sender.send_sync(MidiInputEvent {
                    source: MidiSource::Sync(port),
                    time,
                    message: full,
                });
                return;
            }
            // Spec: control-surfaces R6. A system exclusive message goes
            // whole to the surface that holds the input.
            if !live.to_surface(time, bytes) {
                stats.ignored.fetch_add(1, Relaxed);
            }
            return;
        };
        if message.is_system() {
            // Spec: midi-io R5, R9
            if on.sync && is_sync(&message) {
                // Spec: midi-clock R14, midi-timecode R2. An input follows
                // its clock or its time code: what its sync type doesn't
                // name is counted and set aside.
                let wanted = if timecode {
                    message.status() == 0xF1
                } else {
                    matches!(message.status(), 0xF2 | 0xF8 | 0xFA..=0xFC)
                };
                if wanted {
                    sender.send_sync(MidiInputEvent {
                        source: MidiSource::Sync(port),
                        time,
                        message,
                    });
                } else {
                    stats.to_sync.fetch_add(1, Relaxed);
                }
            } else if !live.to_surface(time, bytes) {
                stats.ignored.fetch_add(1, Relaxed);
            }
            return;
        }
        let event = MidiInputEvent {
            source,
            time,
            message,
        };
        // Spec: midi-io R26, remote-mapping R8. A note-off whose note-on
        // reached a track reaches it too, whatever the table, a surface or
        // the Track switch say now; it then goes the way they say as well.
        let ended = message.is_note_off() && live.mark(&message, false);
        if ended {
            sender.send(event);
        }
        // Spec: remote-mapping R7 to R10. A message a mapping takes goes to
        // remote control and never to a track; while a map mode is on every
        // message from a Remote input also crosses, for learning.
        let remote = sender.remote();
        let taken = on.remote && remote.takes(&message);
        if on.remote {
            stats.to_remote.fetch_add(1, Relaxed);
            if taken || remote.learning() {
                sender.send_remote(event);
            }
        }
        if taken {
            return;
        }
        // Spec: control-surfaces R4. What no mapping took goes to the
        // surface that holds the input, and to no track.
        if live.to_surface(time, bytes) {
            return;
        }
        if on.track {
            if !ended && sender.send(event) && message.is_note_on() {
                live.mark(&message, true);
            }
        } else if on.remote {
            if !remote.learning() && !ended {
                stats.remote_unmapped.fetch_add(1, Relaxed);
            }
        } else if !ended {
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

        // Remote and Sync only: no track's message reaches the engine, and
        // the clock goes to it stamped as sync's own (midi-clock R14).
        ports
            .set_switches(
                Direction::Input,
                "Keys",
                PortSwitches {
                    track: false,
                    sync: true,
                    remote: true,
                    mpe: false,
                    ..PortSwitches::OFF
                },
            )
            .unwrap();
        assert!(test.send("Keys", now, &note));
        assert!(test.send("Keys", now, &[0xF8]));
        let clock = engine.pop().unwrap();
        assert_eq!(clock.source, MidiSource::Sync(PortId::from_name("Keys")));
        assert_eq!((clock.time, clock.message), (now, MidiMessage::CLOCK));
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

    // Spec: midi-timecode R2
    #[test]
    fn timecode_reaches_sync_as_fixed_size_messages_and_never_a_track() {
        let test = TestBackend::new();
        test.add_port(Direction::Input, "Tape");
        let (mut ports, mut engine) = ports(&test);
        let stats = Arc::clone(ports.sender().stats());
        let id = PortId::from_name("Tape");
        let now = Instant::now();
        let full = [0xF0, 0x7F, 0x7F, 0x01, 0x01, 0x21, 0x02, 0x03, 0x04, 0xF7];
        let timecode = PortSwitches {
            track: true,
            sync: true,
            sync_type: SyncType::Timecode,
            ..PortSwitches::OFF
        };
        ports
            .set_switches(Direction::Input, "Tape", timecode)
            .unwrap();
        assert!(test.send("Tape", now, &full));
        assert!(test.send("Tape", now, &[0xF1, 0x35]));
        // The clock and a note: clock is another type's, and the note is the
        // track's.
        assert!(test.send("Tape", now, &[0xF8]));
        assert!(test.send("Tape", now, &[0x90, 60, 100]));
        let frame = engine.pop().unwrap();
        assert_eq!((frame.source, frame.time), (MidiSource::Sync(id), now));
        assert_eq!(
            frame.message,
            MidiMessage::full_frame(0x21, 2, 3, 4).unwrap()
        );
        let quarter = engine.pop().unwrap();
        assert_eq!(quarter.source, MidiSource::Sync(id));
        assert_eq!(quarter.message.bytes(), &[0xF1, 0x35]);
        assert_eq!(engine.pop().unwrap().source, MidiSource::Port(id));
        assert!(engine.pop().is_err());
        assert_eq!((stats.snapshot().to_sync, stats.snapshot().ignored), (3, 0));

        // A port set to clock sets timecode aside, and a full frame is the
        // system exclusive message it was before: nothing holds it.
        let clock = PortSwitches {
            sync_type: SyncType::Clock,
            ..timecode
        };
        ports.set_switches(Direction::Input, "Tape", clock).unwrap();
        assert!(test.send("Tape", now, &full));
        assert!(test.send("Tape", now, &[0xF1, 0x35]));
        assert!(engine.pop().is_err());
        assert_eq!(stats.snapshot().ignored, 1);
    }

    // Spec: midi-io R27. With MPE on, an input's messages are stamped as an
    // MPE port's; MPE alone opens nothing.
    #[test]
    fn the_mpe_switch_stamps_messages_as_an_mpe_ports_and_opens_nothing() {
        let test = TestBackend::new();
        test.add_port(Direction::Input, "Roli");
        let (mut ports, mut engine) = ports(&test);
        let id = PortId::from_name("Roli");
        let now = Instant::now();
        assert!(test.send("Roli", now, &[0x91, 60, 100]));
        assert_eq!(engine.pop().unwrap().source, MidiSource::Port(id));

        let mpe = PortSwitches {
            mpe: true,
            ..PortSwitches::first_seen(Direction::Input)
        };
        ports.set_switches(Direction::Input, "Roli", mpe).unwrap();
        assert!(test.send("Roli", now, &[0x91, 60, 100]));
        assert_eq!(engine.pop().unwrap().source, MidiSource::MpePort(id));

        let only_mpe = PortSwitches {
            mpe: true,
            ..PortSwitches::OFF
        };
        let state = ports
            .set_switches(Direction::Input, "Roli", only_mpe)
            .unwrap();
        assert!(!state.open, "MPE alone leaves the port closed");
    }

    fn remote(track: bool) -> PortSwitches {
        PortSwitches {
            track,
            remote: true,
            ..PortSwitches::OFF
        }
    }

    fn took(ports: &MidiPorts) -> Vec<MidiInputEvent> {
        let mut into = Vec::new();
        ports.sender().remote().drain(&mut into);
        into
    }

    // Spec: remote-mapping R7, R8. A message a mapping takes goes to remote
    // control and never to a track; one it doesn't take goes on as before.
    #[test]
    fn a_mapped_message_never_reaches_a_track_and_an_unmapped_one_does() {
        let test = TestBackend::new();
        test.add_port(Direction::Input, "Pads");
        let (mut ports, mut engine) = ports(&test);
        ports
            .set_switches(Direction::Input, "Pads", remote(true))
            .unwrap();
        let mut bits = crate::RemoteBits::default();
        bits.controller(0, 20);
        ports.sender().remote().publish(&bits);
        let now = Instant::now();

        assert!(test.send("Pads", now, &[0xB0, 20, 99]));
        assert!(test.send("Pads", now, &[0xB0, 21, 99]));
        assert_eq!(engine.pop().unwrap().message.data1(), 21);
        assert!(engine.pop().is_err(), "the mapped controller stayed back");
        let taken = took(&ports);
        assert_eq!(taken.len(), 1);
        assert_eq!(taken[0].message.bytes(), &[0xB0, 20, 99]);
        assert_eq!(taken[0].time, now);
    }

    // Spec: remote-mapping R7, R8. Every Remote input feeds the one remote
    // input; a Remote-only input's unmapped message reaches no track and is
    // counted.
    #[test]
    fn remote_inputs_merge_and_an_unmapped_remote_only_message_is_counted() {
        let test = TestBackend::new();
        test.add_port(Direction::Input, "A");
        test.add_port(Direction::Input, "B");
        let (mut ports, mut engine) = ports(&test);
        for name in ["A", "B"] {
            ports
                .set_switches(Direction::Input, name, remote(false))
                .unwrap();
        }
        let mut bits = crate::RemoteBits::default();
        bits.note(0, 60);
        ports.sender().remote().publish(&bits);
        let now = Instant::now();

        assert!(test.send("A", now, &[0x90, 60, 100]));
        assert!(test.send("B", now, &[0x90, 60, 90]));
        assert!(test.send("A", now, &[0x90, 61, 100]));
        let taken = took(&ports);
        assert_eq!(
            taken.iter().map(|e| e.message.data2()).collect::<Vec<_>>(),
            [100, 90]
        );
        assert_ne!(taken[0].source, taken[1].source);
        assert!(engine.pop().is_err());
        let counts = ports.sender().stats().snapshot();
        assert_eq!((counts.remote_taken, counts.remote_unmapped), (2, 1));
    }

    // Spec: remote-mapping R10. While a map mode is on every Remote message
    // also crosses, and still reaches tracks as R8 says; a full queue drops
    // and counts.
    #[test]
    fn a_map_mode_sends_every_remote_message_across_and_a_full_queue_counts_drops() {
        let test = TestBackend::new();
        test.add_port(Direction::Input, "Pads");
        let (mut ports, mut engine) = ports(&test);
        ports
            .set_switches(Direction::Input, "Pads", remote(true))
            .unwrap();
        ports.sender().remote().set_learning(true);
        let now = Instant::now();
        assert!(test.send("Pads", now, &[0x90, 60, 100]));
        assert_eq!(took(&ports).len(), 1);
        assert!(
            engine.pop().is_ok(),
            "an unmapped note still reaches tracks"
        );

        for _ in 0..1100 {
            assert!(test.send("Pads", now, &[0xB0, 1, 1]));
            let _ = engine.pop();
        }
        let counts = ports.sender().stats().snapshot();
        assert_eq!(counts.remote_taken + counts.remote_dropped, 1101);
        assert!(counts.remote_dropped > 0);
        assert_eq!(took(&ports).len() as u64, counts.remote_taken - 1);
    }

    // Spec: midi-io R26, remote-mapping R8. A note that reached a track gets
    // its note-off there, whatever the table says by then.
    #[test]
    fn a_note_that_reached_a_track_gets_its_note_off_after_it_is_mapped() {
        let test = TestBackend::new();
        test.add_port(Direction::Input, "Keys");
        let (mut ports, mut engine) = ports(&test);
        ports
            .set_switches(Direction::Input, "Keys", remote(true))
            .unwrap();
        ports.sender().remote().set_learning(true);
        let now = Instant::now();
        assert!(test.send("Keys", now, &[0x90, 60, 100]));
        assert_eq!(engine.pop().unwrap().message.bytes(), &[0x90, 60, 100]);
        let mut bits = crate::RemoteBits::default();
        bits.note(0, 60);
        ports.sender().remote().publish(&bits);
        assert!(test.send("Keys", now, &[0x80, 60, 0]));
        assert_eq!(
            engine
                .pop()
                .expect("the note-off reaches the track")
                .message
                .bytes(),
            &[0x80, 60, 0]
        );
        assert!(engine.pop().is_err(), "once");
        assert_eq!(took(&ports).len(), 2, "remote control gets it too");

        // A note whose note-on a mapping took never reaches a track, and
        // neither does its note-off.
        assert!(test.send("Keys", now, &[0x90, 60, 100]));
        assert!(test.send("Keys", now, &[0x90, 60, 0]));
        assert!(engine.pop().is_err());
    }

    // Spec: midi-io R26, R5. Switching Track off, or a surface taking the
    // input, mid-note doesn't hold the note-off back from the track.
    #[test]
    fn a_note_off_reaches_the_track_after_track_is_switched_off_or_a_surface_holds_the_input() {
        let test = TestBackend::new();
        test.add_port(Direction::Input, "Keys");
        let (mut ports, mut engine) = ports(&test);
        ports
            .set_switches(Direction::Input, "Keys", remote(true))
            .unwrap();
        let now = Instant::now();
        assert!(test.send("Keys", now, &[0x90, 60, 100]));
        assert!(test.send("Keys", now, &[0x91, 61, 100]));
        assert!(engine.pop().is_ok() && engine.pop().is_ok());

        ports
            .set_switches(Direction::Input, "Keys", remote(false))
            .unwrap();
        assert!(test.send("Keys", now, &[0x80, 60, 0]));
        assert_eq!(engine.pop().unwrap().message.bytes(), &[0x80, 60, 0]);
        assert!(test.send("Keys", now, &[0x90, 62, 100]));
        assert!(engine.pop().is_err(), "a new note stays back");

        let seen = Arc::new(AtomicU8::new(0));
        let counter = Arc::clone(&seen);
        ports
            .claim(Direction::Input, "Keys")
            .hold(Some(Box::new(move |_, _| {
                counter.fetch_add(1, Relaxed);
            })));
        assert!(test.send("Keys", now, &[0x81, 61, 0]));
        assert_eq!(engine.pop().unwrap().message.bytes(), &[0x81, 61, 0]);
        assert_eq!(seen.load(Relaxed), 1, "the surface gets it as well");
    }

    // Spec: midi-io R26. An input that closes ends the notes it started.
    #[test]
    fn closing_an_input_ends_the_notes_it_started() {
        let test = TestBackend::new();
        test.add_port(Direction::Input, "Keys");
        let (mut ports, mut engine) = ports(&test);
        let now = Instant::now();
        assert!(test.send("Keys", now, &[0x92, 100, 100]));
        assert!(test.send("Keys", now, &[0x92, 5, 100]));
        assert!(engine.pop().is_ok() && engine.pop().is_ok());
        test.remove_port(Direction::Input, "Keys");
        ports.refresh().unwrap();
        let mut ended = Vec::new();
        while let Ok(event) = engine.pop() {
            assert_eq!(event.source, MidiSource::Port(PortId::from_name("Keys")));
            ended.push((event.message.status(), event.message.data1()));
        }
        ended.sort_unstable();
        assert_eq!(ended, [(0x82, 5), (0x82, 100)]);
    }

    // Spec: midi-io R3
    #[test]
    fn a_port_replugged_between_refreshes_opens_again() {
        let test = TestBackend::new();
        test.add_port(Direction::Input, "Keys");
        test.add_port(Direction::Output, "Synth");
        let (mut ports, mut engine) = ports(&test);
        ports
            .set_switches(
                Direction::Output,
                "Synth",
                PortSwitches {
                    track: true,
                    ..PortSwitches::OFF
                },
            )
            .unwrap();
        assert!(test.is_open(Direction::Input, "Keys"));
        assert!(test.is_open(Direction::Output, "Synth"));
        for (direction, name) in [(Direction::Input, "Keys"), (Direction::Output, "Synth")] {
            test.remove_port(direction, name);
            test.add_port(direction, name); // no refresh in between
        }
        ports.refresh().unwrap();
        assert!(test.is_open(Direction::Input, "Keys"), "the input is open");
        assert!(
            test.is_open(Direction::Output, "Synth"),
            "the output is open"
        );
        assert!(test.send("Keys", Instant::now(), &[0x90, 60, 100]));
        assert!(engine.pop().is_ok(), "the note reaches the engine");
        let listed = ports.ports();
        assert!(listed.iter().all(|p| p.open && p.error.is_none()));
    }
}

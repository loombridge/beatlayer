//! The device trait every MIDI backend implements.

use core::fmt;
use std::time::Instant;

/// Which way a port carries MIDI.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Direction {
    /// Into Beatlayer.
    Input,
    /// Out of Beatlayer.
    Output,
}

impl Direction {
    /// How a port's row in the settings starts: `In` or `Out`.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Input => "In",
            Self::Output => "Out",
        }
    }
}

/// What an open input calls for every message, on the MIDI system's thread:
/// when the message arrived, and its bytes (a whole message, or a whole
/// system-exclusive message).
///
/// It runs on a thread the MIDI system owns, often at raised priority, so it
/// should be quick, but it's not an audio thread: it may take a lock that
/// only other MIDI-side threads hold.
pub type InputHandler = Box<dyn FnMut(Instant, &[u8]) + Send + 'static>;

/// An open input port. Dropping it closes the port.
pub trait InputConnection: Send {}

/// An open output port. Dropping it closes the port.
pub trait OutputConnection: Send {
    /// Sends one message now.
    ///
    /// # Errors
    ///
    /// [`MidiError`] if the port refused it.
    fn send(&mut self, bytes: &[u8]) -> Result<(), MidiError>;

    /// Hands the MIDI system one message to send at `at`, the moment its
    /// frame is heard at the audio interface's output
    /// (`docs/specs/external-devices.md`, R5). A backend that can schedule
    /// does (Core MIDI with a time stamp, the ALSA sequencer's queue, a
    /// timer thread on Windows); the default sends at once. A time already
    /// past sends at once too.
    ///
    /// # Errors
    ///
    /// [`MidiError`] if the port refused it.
    fn send_at(&mut self, bytes: &[u8], at: Instant) -> Result<(), MidiError> {
        let _ = at;
        self.send(bytes)
    }
}

/// One MIDI system on this machine: it lists ports and opens them.
///
/// Ports are named as the system names them, which is how Live names them in
/// its settings and in a track's input, so the same set finds the same port
/// after a replug or on another computer (`docs/specs/midi-io.md`, R1, R8).
pub trait MidiBackend: Send {
    /// The system's name, such as `Core MIDI`.
    fn name(&self) -> &'static str;

    /// The names of the ports offered now, in the system's order.
    ///
    /// # Errors
    ///
    /// [`MidiError`] if the system can't be asked.
    fn ports(&mut self, direction: Direction) -> Result<Vec<String>, MidiError>;

    /// Opens the input port called `name`. `handler` then runs for every
    /// message until the connection is dropped.
    ///
    /// # Errors
    ///
    /// [`MidiError`] if there's no such port or it can't be opened.
    fn open_input(
        &mut self,
        name: &str,
        handler: InputHandler,
    ) -> Result<Box<dyn InputConnection>, MidiError>;

    /// Opens the output port called `name`.
    ///
    /// # Errors
    ///
    /// [`MidiError`] if there's no such port or it can't be opened.
    fn open_output(&mut self, name: &str) -> Result<Box<dyn OutputConnection>, MidiError>;
}

/// Why a MIDI system refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MidiError {
    /// No port has this name now.
    NoSuchPort(String),
    /// The system refused, saying why.
    System(String),
}

impl fmt::Display for MidiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoSuchPort(name) => write!(f, "no MIDI port is called \"{name}\""),
            Self::System(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for MidiError {}

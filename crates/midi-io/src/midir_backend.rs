//! The platform's MIDI system through `midir`: Core MIDI on macOS, the ALSA
//! sequencer on Linux, and Windows Multimedia MIDI on Windows.

use std::time::Instant;

use midir::{MidiIO, MidiInput, MidiInputConnection, MidiOutput, MidiOutputConnection};

use crate::backend::{
    Direction, InputConnection, InputHandler, MidiBackend, MidiError, OutputConnection,
};
use crate::stamp::StampClock;

/// The name Beatlayer gives its connections, where the system shows one.
const CLIENT: &str = "Beatlayer";

/// The platform's MIDI system, through `midir`.
///
/// Each input's handler gets the time each message reached the system:
/// `midir`'s stamp mapped onto an [`Instant`] by a [`StampClock`] where the
/// system stamps messages (Core MIDI, ALSA), the time it reached the
/// handler where it doesn't.
#[derive(Debug, Default)]
pub struct MidirBackend {
    _private: (),
}

impl MidirBackend {
    /// The platform's MIDI system. Nothing opens until a port does.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

fn system(error: impl std::fmt::Display) -> MidiError {
    MidiError::System(error.to_string())
}

/// The port called `name` among `io`'s.
fn find<T: MidiIO>(io: &T, name: &str) -> Result<T::Port, MidiError> {
    io.ports()
        .into_iter()
        .find(|port| io.port_name(port).is_ok_and(|n| n == name))
        .ok_or_else(|| MidiError::NoSuchPort(name.to_owned()))
}

/// The system's name on this platform.
const SYSTEM: &str = if cfg!(target_os = "macos") {
    "Core MIDI"
} else if cfg!(windows) {
    "Windows MIDI"
} else {
    "ALSA"
};

/// Whether the system stamps messages itself; Windows Multimedia MIDI's
/// stamps count milliseconds, coarser than arrival times.
const SYSTEM_STAMPS: bool = !cfg!(windows);

impl MidiBackend for MidirBackend {
    fn name(&self) -> &'static str {
        SYSTEM
    }

    fn ports(&mut self, direction: Direction) -> Result<Vec<String>, MidiError> {
        fn names<T: MidiIO>(io: &T) -> Vec<String> {
            io.ports()
                .iter()
                .filter_map(|port| io.port_name(port).ok())
                .collect()
        }
        match direction {
            Direction::Input => Ok(names(&MidiInput::new(CLIENT).map_err(system)?)),
            Direction::Output => Ok(names(&MidiOutput::new(CLIENT).map_err(system)?)),
        }
    }

    fn open_input(
        &mut self,
        name: &str,
        mut handler: InputHandler,
    ) -> Result<Box<dyn InputConnection>, MidiError> {
        let input = MidiInput::new(CLIENT).map_err(system)?;
        let port = find(&input, name)?;
        let mut clock = StampClock::new();
        let connection = input
            .connect(
                &port,
                CLIENT,
                move |stamp, bytes, _: &mut ()| {
                    let arrival = Instant::now();
                    let time = if SYSTEM_STAMPS {
                        clock.map(stamp, arrival)
                    } else {
                        arrival
                    };
                    handler(time, bytes);
                },
                (),
            )
            .map_err(system)?;
        Ok(Box::new(Input(connection)))
    }

    fn open_output(&mut self, name: &str) -> Result<Box<dyn OutputConnection>, MidiError> {
        let output = MidiOutput::new(CLIENT).map_err(system)?;
        let port = find(&output, name)?;
        let connection = output.connect(&port, CLIENT).map_err(system)?;
        Ok(Box::new(Output(connection)))
    }
}

/// An open input; `midir` closes it when dropped.
struct Input(#[expect(dead_code, reason = "held to keep the port open")] MidiInputConnection<()>);

impl InputConnection for Input {}

/// An open output; `midir` closes it when dropped.
struct Output(MidiOutputConnection);

impl OutputConnection for Output {
    fn send(&mut self, bytes: &[u8]) -> Result<(), MidiError> {
        self.0.send(bytes).map_err(system)
    }
}

//! MIDI device I/O, clock, and MPE.
//!
//! WP-2.2 builds MIDI input and the computer MIDI keyboard
//! (`docs/specs/midi-io.md`). Beatlayer owns the device trait, as it does
//! for audio (`bl-audio-io`), so a backend can change underneath without
//! touching the rest:
//!
//! - [`MidiBackend`]: one MIDI system on this machine. It lists ports by
//!   name and opens them. An open input calls its [`InputHandler`] on the
//!   MIDI system's own thread for every message, with the [`Instant`] it
//!   arrived.
//! - [`MidiPorts`]: the ports in use. It opens a port while any of its
//!   Track, Sync, and Remote switches is on, and sends what an input's Track
//!   switch lets through to the engine on a lock-free queue
//!   ([`input_queue`]), stamped with its source and time.
//! - [`ComputerKeyboard`]: the computer MIDI keyboard's state and layout,
//!   whose notes go to the engine on the same queue.
//!
//! # Backends
//!
//! | Backend | Platform | Built on | Output |
//! |---|---|---|---|
//! | Core MIDI | macOS | `midir` for ports and input, `coremidi` for output | Given each message ahead of time with its time stamp |
//! | ALSA sequencer | Linux | `midir` for ports and input, `alsa` for output | Queued with each message's time |
//! | Windows Multimedia MIDI | Windows | `midir` | A timer thread sends each message |
//! | Null | all | nothing: no ports | None |
//! | Test | all | in-process ports a test feeds | Recorded, with the stamp if it has one |
//!
//! `midir` 0.11.1 (MIT) was evaluated for WP-2.2: it lists and opens ports
//! on all three platforms, makes virtual ports on macOS and Linux (not on
//! Windows), and stamps input on Core MIDI and ALSA with the system's own
//! times, which [`MidirBackend`] maps onto [`Instant`]s ([`StampClock`]).
//! It has no MIDI 2.0 or Windows MIDI Services; the architecture expects
//! Beatlayer's own backends for those later.
//!
//! # Scheduled output
//!
//! `midir` sends output immediately, with no scheduling, so [`MidirBackend`]
//! opens outputs another way (`docs/specs/external-devices.md`, R5). The
//! audio thread stamps each message with the moment its frame is heard at
//! the audio interface's output, and the MIDI output thread hands it to the
//! port with [`OutputConnection::send_at`]. On macOS the port passes it to
//! Core MIDI ahead of time with that moment as the packet's host time, and
//! on Linux it puts the event on a queue of the ALSA sequencer with that
//! moment as its time, so the system sends it on time. Windows Multimedia
//! MIDI can only send at once, so there a timer thread ([`TimedOutput`])
//! sleeps until just before the moment, waits out the rest by polling the
//! clock, and sends within a millisecond of it. A message whose moment has
//! gone by when the output thread takes it is sent at once and counted late
//! ([`OutputStats`]).
//!
//! # The audio thread
//!
//! Nothing here runs on it. Each input's handler runs on the MIDI system's
//! thread and pushes onto an `rtrb` queue whose consuming end the engine
//! holds; the producing end sits behind a mutex that only MIDI-side threads
//! (each port's handler, the computer keyboard) ever take, so the audio
//! thread never waits on anything here. A full queue drops the message and
//! counts it.
//!
//! [`Instant`]: std::time::Instant

#[cfg(target_os = "linux")]
mod alsa_out;
mod backend;
#[cfg(target_os = "macos")]
mod coremidi_out;
mod keyboard;
mod midir_backend;
mod null;
mod output;
mod ports;
mod queue;
mod remote;
mod stamp;
pub mod test_backend;
mod timed;

pub use backend::{
    Direction, InputConnection, InputHandler, MidiBackend, MidiError, OutputConnection,
};
pub use keyboard::{ComputerKeyboard, KeyboardKey, MAX_OCTAVE, MIN_OCTAVE, VELOCITIES, note_name};
pub use midir_backend::MidirBackend;
pub use null::NullBackend;
pub use output::{OutputBus, OutputSnapshot, OutputStats};
pub use ports::{InputLive, MidiPorts, PortState, PortSwitches, SurfaceSink};
pub use queue::{InputSender, MidiStats, StatsSnapshot, input_queue};
pub use remote::{RemoteBits, RemoteInput};
pub use stamp::StampClock;
pub use test_backend::TestBackend;
pub use timed::TimedOutput;

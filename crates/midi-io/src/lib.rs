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
//! | Backend | Platform | Built on |
//! |---|---|---|
//! | Core MIDI | macOS | `midir` |
//! | ALSA sequencer | Linux | `midir` |
//! | Windows Multimedia MIDI | Windows | `midir` |
//! | Null | all | nothing: no ports |
//! | Test | all | in-process ports a test feeds |
//!
//! `midir` 0.11.1 (MIT) was evaluated for WP-2.2: it lists and opens ports
//! on all three platforms, makes virtual ports on macOS and Linux (not on
//! Windows), and stamps input on Core MIDI and ALSA with the system's own
//! times, which [`MidirBackend`] maps onto [`Instant`]s ([`StampClock`]).
//! It sends output immediately, with no scheduling, and has no MIDI 2.0
//! or Windows MIDI Services; the architecture expects Beatlayer's own
//! backends for those later.
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

mod backend;
mod keyboard;
mod midir_backend;
mod null;
mod ports;
mod queue;
mod stamp;
pub mod test_backend;

pub use backend::{
    Direction, InputConnection, InputHandler, MidiBackend, MidiError, OutputConnection,
};
pub use keyboard::{ComputerKeyboard, KeyboardKey, MAX_OCTAVE, MIN_OCTAVE, VELOCITIES, note_name};
pub use midir_backend::MidirBackend;
pub use null::NullBackend;
pub use ports::{MidiPorts, PortState, PortSwitches};
pub use queue::{InputSender, MidiStats, StatsSnapshot, input_queue};
pub use stamp::StampClock;
pub use test_backend::TestBackend;

//! Beatlayer's real-time tests under LLVM's RealtimeSanitizer. The tests are
//! in `tests/`; this crate has no code of its own.
//!
//! - `tests/harness.rs`: `bl-rt`'s harness. A test that allocates, takes a
//!   contended mutex, or sleeps in a real-time section is stopped, and the
//!   harness's own per-block work passes. The allocation trap is installed,
//!   as in every Beatlayer test binary.
//! - `tests/audio_thread.rs`: what an audio thread runs, built as a release
//!   build runs it, without the allocation trap. The set-up for a thread an
//!   audio API owns runs inside a real-time section from that thread's first
//!   instruction, and a null-device stream runs clean.
//! - `tests/engine.rs`: the engine's audio-thread work: 1,000 program swaps
//!   with node migration, parameter changes, scheduled commands, node data,
//!   and the state for the UI, every block a real-time section.
//! - `tests/shell.rs`: the app's audio callback (`bl-shell`'s
//!   `EngineCallback`): the engine's processor handed over through its
//!   ring, run in blocks of its own size within the device's, and the test
//!   tone, through the transport and program swaps.
//! - `tests/stream.rs`: disk streaming's audio-thread work: reads, jumps
//!   that land in the look-ahead, a preload, a cue, or nowhere ready,
//!   crossfades, and an underrun, every block a real-time section.
//! - `tests/transport.rs`: the transport's audio-thread work: play, stop,
//!   continue, the insert marker, commands on exact frames, the Arrangement
//!   loop wrapping inside a tempo ramp, a tempo map swapped in during
//!   playback, and the beat tone (WP-1.5).
//! - `tests/mixer.rs`: the mixer's audio-thread work: volumes, pans, and
//!   activators gliding, solo and Solo in Place, sends before and after the
//!   fader, a program swap when a return's Pre/Post switch moves, and the
//!   meters published after every block (WP-1.6).

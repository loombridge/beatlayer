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
//! - `tests/clips.rs`: clip players reading looped, faded, and overlapping
//!   clips, new clip lists swapped in as clips are edited (the old ones
//!   freed by the collector), locates into clips, and tempo changes that
//!   swap programs (WP-1.4).

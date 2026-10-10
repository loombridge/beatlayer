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

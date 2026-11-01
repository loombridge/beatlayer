//! What `bl-cli` shares with its tests: the offline render behind
//! `bl-cli render`, and the golden-test harness behind `bl-cli golden` and
//! `cargo test`.
//!
//! - [`render`]: a document played from its start through the engine on one
//!   thread, faster than real time, and written to a WAV file.
//! - [`assistant`]: one turn of the built-in assistant with no window
//!   (`bl-cli assistant`), its transcripts, and a provider that replays them.
//! - [`capacity`]: the song of the capacity target, many tracks with an EQ
//!   and a compressor each.
//! - [`transcribe_bench`]: how accurately Convert Melody and Convert Harmony
//!   find notes, on generated phrases with known notes.
//! - [`golden`]: songs built in code (the fixtures), rendered and
//!   null-tested against reference renders stored as FLAC files in
//!   `tests/golden/`.

pub mod assistant;
pub mod capacity;
pub mod golden;
pub mod render;
pub mod transcribe_bench;

/// What a command returns: an exit code, or a message for a failure.
pub type Outcome = Result<std::process::ExitCode, String>;

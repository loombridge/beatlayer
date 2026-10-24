//! Every integration test of this crate, in one test binary (23 October 2026).
//! One binary per test file linked the whole crate graph into each: about
//! 1,700 binaries and 170 GB a worktree. Add a test file here as `mod`;
//! `cargo xtask check` fails on a file in tests/ that isn't listed.

mod clip_params;
mod command_registry;
mod device_media;
mod drum_racks;
mod grooves;
mod linked_tracks;
mod macros;
mod metronome;
mod midi_controller_data;
mod racks;
mod scales;
mod take_lanes;
mod tunings;
mod undo_properties;

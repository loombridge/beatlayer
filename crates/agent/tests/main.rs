//! Every integration test of this crate, in one test binary (23 October 2026).
//! One binary per test file linked the whole crate graph into each: about
//! 1,700 binaries and 170 GB a worktree. Add a test file here as `mod`;
//! `cargo xtask check` fails on a file in tests/ that isn't listed.

mod clip_edge_fades;
mod comping;
mod control_surfaces;
mod engine_host;
mod export;
mod fades;
mod grooves;
mod http;
mod instrument_filter;
mod kit_devices;
mod link;
mod linked_tracks;
mod midi_clock;
mod midi_effects;
mod midi_tools;
mod perception;
mod permissions;
mod plugin_lv2;
mod plugin_midi_effect;
mod plugin_mpe;
mod plugin_vst3;
mod polysynth;
mod push;
mod readback;
mod readback_runtime;
mod recording;
mod recovery;
mod save_history;
mod stdio;
mod tempo_follower;
mod tools;
mod tunings;
mod undo_history;
mod warp_settings;

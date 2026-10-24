//! Every integration test of this crate, in one test binary (23 October 2026).
//! One binary per test file linked the whole crate graph into each: about
//! 1,700 binaries and 170 GB a worktree. Add a test file here as `mod`;
//! `cargo xtask check` fails on a file in tests/ that isn't listed.

// The real-time allocation trap, once for the whole binary: a test file
// can't install its own global allocator.
bl_rt::install_trap!();

mod agent;
mod agent_audio_editing;
mod agent_bounce;
mod agent_dawproject;
mod agent_device_compare;
mod agent_drum_sampler;
mod agent_export;
mod agent_macros;
mod agent_midi_files;
mod agent_midi_io;
mod agent_modulation;
mod agent_session_recording;
mod agent_slice;
mod audio;
mod capacity;
mod clip_goldens;
mod dawproject_goldens;
mod device_changes;
mod engine;
mod export;
mod export_plugins;
mod golden;
mod m2_assistant;
mod m2_song;
mod m5_demo;
mod mixer_goldens;
mod mpe_render;
mod older_sets;
mod scene_goldens;
mod transport_goldens;
mod warp_goldens;
mod warp_measurements;

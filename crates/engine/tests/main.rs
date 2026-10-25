//! Every integration test of this crate, in one test binary (23 October 2026).
//! One binary per test file linked the whole crate graph into each: about
//! 1,700 binaries and 170 GB a worktree. Add a test file here as `mod`;
//! `cargo xtask check` fails on a file in tests/ that isn't listed.

// The real-time allocation trap, once for the whole binary: a test file
// can't install its own global allocator.
bl_rt::install_trap!();

mod analysis;
mod automation;
mod capture_midi;
mod clip_envelopes;
mod clip_launch;
mod clip_settings;
mod clips;
mod common;
mod crossfades;
mod cue;
mod deactivated;
mod device_chains;
mod device_input;
mod drum_racks;
mod events;
mod ext_out;
mod external_devices;
mod external_effect;
mod follow;
mod grooves;
mod grooves_audio;
mod live_complex;
mod macros;
mod metronome;
mod midi_clips;
mod midi_controllers;
mod midi_input;
mod midi_meters;
mod midi_output;
mod midi_play;
mod midi_recording;
mod midi_sidechain;
mod mixer;
mod modulation;
mod mpe_input;
mod mpe_output;
mod multi_output;
mod pdc;
mod performance;
mod playhead;
mod probability_groups;
mod racks;
mod recording;
mod restart;
mod routing;
mod scales;
mod scene_tempo;
mod schedule;
mod scheduler;
mod session;
mod session_automation;
mod session_launch_log;
mod session_recording;
mod sidechain;
mod state;
mod stress;
mod swap;
mod transport;
mod warp_modes;
mod warp_streaming;
mod warping;
mod zones;

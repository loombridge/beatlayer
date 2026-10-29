//! Every integration test of this crate, in one test binary (23 October 2026).
//! One binary per test file linked the whole crate graph into each: about
//! 1,700 binaries and 170 GB a worktree. Add a test file here as `mod`;
//! `cargo xtask check` fails on a file in tests/ that isn't listed.

// The real-time allocation trap, once for the whole binary: a test file
// can't install its own global allocator.
bl_rt::install_trap!();

mod platform;
mod remote_input;
mod scheduled_output;
mod to_the_engine;

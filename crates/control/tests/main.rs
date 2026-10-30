//! Every integration test of this crate, in one test binary. Add a test file
//! here as `mod`; `cargo xtask check` fails on a file in tests/ that isn't
//! listed.

// The real-time allocation trap, once for the whole binary: a test file
// can't install its own global allocator.
bl_rt::install_trap!();

mod grid;
mod realtime;
mod surfaces;
mod templates;

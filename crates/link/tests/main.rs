//! Every integration test of this crate, in one test binary. Add a test file
//! here as `mod`; `cargo xtask check` fails on a file in tests/ that isn't
//! listed.

mod link_audio;
mod loopback;

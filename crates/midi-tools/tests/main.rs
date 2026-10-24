//! Every integration test of this crate, in one test binary (23 October
//! 2026). Add a test file here as `mod`; `cargo xtask check` fails on a file
//! in tests/ that isn't listed.

mod generators;
mod interface;
mod mpe_transformations;
mod stacks;
mod transformations;
mod transformations_2;

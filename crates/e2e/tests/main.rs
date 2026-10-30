//! Every end-to-end test, in one test binary: the repository's rule is one
//! binary per crate. Add a test file here as `mod`; `cargo xtask check`
//! fails on a file in tests/ that isn't listed.

mod smoke;

mod scenarios;

//! The real-time thread harness, from the plug-in kit's `lspk-rt`, under the
//! name Beatlayer's crates use: real-time thread priority and audio
//! workgroups, denormal flushing, the allocation trap with [`install_trap!`]
//! and [`rt_test!`], the lock-free logger, and RealtimeSanitizer
//! (`cargo xtask rtsan`).
//!
//! Every item is `lspk_rt`'s, at the same path: `bl_rt::realtime`,
//! `bl_rt::trap::Forbid`, `bl_rt::rt_test!` and so on.

pub use lspk_rt::*;

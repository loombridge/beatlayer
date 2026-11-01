//! WP-9.28: WebAssembly components under wasmtime, called on a real-time thread.
//! A throwaway spike: see README.md.

pub mod alloc;
pub mod guest;
#[cfg(feature = "runtime")]
pub mod host;
pub mod rt;
#[cfg(feature = "compile")]
pub mod scenario;

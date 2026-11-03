//! The extension host (WP-9.29; extensions R1 to R16): Beatlayer's own
//! extension package, validation, the compiled-code cache, installing and
//! removing extensions in the User Library, and WebAssembly components run
//! under `wasmtime` with their memory reserved up front, every call bounded,
//! and a failing component stopped alone with its reason.
//!
//! It depends on no UI crate (R13). The commands that drive it are the
//! shell's.

mod host;
mod package;

use std::fmt;

pub use host::{Extension, Host, Instance, Options};
pub use package::{
    ComponentSpec, Kind, MANIFEST, MAX_MEMORY_MIB, Manifest, Package, Version, WORLD,
};
pub use wasmtime::component::Val;

/// Why an extension operation did not happen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// The package or the request is refused, with the reason. Nothing
    /// was kept.
    Refused(String),
    /// A component trapped, ran past its limit or couldn't be made: the
    /// reason it was stopped (R10, R11).
    Failed(String),
    /// A file or folder couldn't be read or written.
    Io(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (Self::Refused(reason) | Self::Failed(reason) | Self::Io(reason)) = self;
        f.write_str(reason)
    }
}

impl std::error::Error for Error {}

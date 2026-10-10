//! The error type for every backend.

use std::fmt;

/// The kind of thing that went wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ErrorKind {
    /// This build doesn't have the backend, or the machine isn't running it
    /// (for example no PipeWire daemon).
    BackendUnavailable,
    /// No device matches the request, or the device has gone.
    DeviceNotFound,
    /// Another stream or program holds the device.
    DeviceBusy,
    /// The device can't run the requested rate, buffer size, or channels.
    UnsupportedConfig,
    /// The backend can't do this at all, for example duplex streams on `cpal`.
    Unsupported,
    /// The request itself is malformed.
    InvalidInput,
    /// The backend failed in a way that fits no other kind.
    Backend,
}

/// An audio I/O error with a message for a person.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    kind: ErrorKind,
    message: String,
}

impl Error {
    /// Makes an error.
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    /// What kind of error this is.
    pub fn kind(&self) -> ErrorKind {
        self.kind
    }

    /// The message.
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Error {}

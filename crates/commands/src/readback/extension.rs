//! Extensions.

use super::{Readback, read};

/// The readback of this area's commands, one line each, so a command's entry is easy to find.
#[rustfmt::skip]
pub(super) const ROWS: &[(&str, Readback)] = &[
    ("extension.install", read("extension.list", "extensions")),
    ("extension.remove", read("extension.list", "extensions")),
    ("extension.set_enabled", read("extension.list", "extensions[].enabled")),
];

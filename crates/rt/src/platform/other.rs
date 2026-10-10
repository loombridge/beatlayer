//! Platforms Beatlayer doesn't target: no way to read or raise priority.

use std::time::Duration;

use crate::thread::{Priority, Promotion};

#[derive(Debug, Default)]
pub(crate) struct HeldPriority;

pub(crate) fn current_priority() -> Priority {
    Priority::Unknown
}

pub(crate) fn promote(_period: Duration, _held: &mut HeldPriority) -> Promotion {
    Promotion::Unsupported
}

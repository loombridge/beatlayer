//! What a set keeps of its windows (second-window R14): the main view the
//! main window shows, whether the second window is open and whether the
//! Arrangement shows the chord lane (the chord lane spec, R1).
//!
//! Saved with the set beside the document, like the launch quantisation, so
//! switching any of them is no undo step and doesn't change the set.

use serde::{Deserialize, Serialize};

/// The main window's main view, the second window's switch and the chord
/// lane's, as a set saves them. The default is the Arrangement View with one window, where a
/// new set opens (D20).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WindowViews {
    /// Whether the main window shows the Session View.
    pub session: bool,
    /// Whether the second window is open.
    pub second_window: bool,
    /// Whether the Arrangement shows the chord lane (the chord lane spec,
    /// R1). A set with it hidden doesn't write it, so a set saved before the
    /// lane keeps its saved form.
    #[serde(skip_serializing_if = "is_off")]
    pub chord_lane: bool,
}

fn is_off(value: &bool) -> bool {
    !*value
}

impl WindowViews {
    /// Whether these are the defaults, so a set's saved form leaves them
    /// out.
    #[must_use]
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

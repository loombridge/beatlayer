//! What the MIDI Tools keep beside the song (WP-6.11): each track's chosen
//! transformation and generator, and each tool's settings.
//!
//! Like the folds, they sit beside the document rather than in it, so
//! choosing a tool or moving one of its controls is never an undo step (the
//! MIDI Tools spec, R4, R5). A value is kept as JSON because only the shell
//! knows a tool's parameters; the shell checks each against its tool's
//! description on load and takes the default where it doesn't fit.

use std::collections::BTreeMap;

use bl_model::TrackId;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The tools a track's panels show.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChosenTools {
    /// The ID of the transformation, where the track has chosen one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transformation: Option<String>,
    /// The ID of the generator, where the track has chosen one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generator: Option<String>,
}

/// One tool's settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedTool {
    /// The Auto switch.
    #[serde(default = "auto_on")]
    pub auto: bool,
    /// The parameters' values by ID.
    #[serde(default)]
    pub values: BTreeMap<String, Value>,
}

fn auto_on() -> bool {
    true
}

/// Everything the MIDI Tools save beside the song.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MidiToolSettings {
    /// What each track shows, by track.
    #[serde(default)]
    pub chosen: BTreeMap<TrackId, ChosenTools>,
    /// Each tool's settings, by tool ID.
    #[serde(default)]
    pub tools: BTreeMap<String, SavedTool>,
}

impl MidiToolSettings {
    /// Whether nothing is saved, so a set file leaves the field out.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.chosen.is_empty() && self.tools.is_empty()
    }
}

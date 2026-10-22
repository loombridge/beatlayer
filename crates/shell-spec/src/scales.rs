//! The scale list (WP-6.7): the query that reads the scales a set can use,
//! and the IDs of the commands that set them.
//!
//! The list itself is the document's ([`bl_model::ScaleName`]), so the
//! devices, the note editor and the MIDI Tools all read the same one. The
//! rules are in the scales spec.

use bl_commands::Context::Global;
use bl_commands::{CommandSpec, NoArgs, Permission, RegisterError, Registry};
use bl_model::ScaleName;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `song.scales`: the scale list, with each scale's steps.
pub const SCALES: &str = "song.scales";

/// `clip.set_scale` (`bl-model`): a clip's Scale Mode, root and scale.
pub const SET_CLIP_SCALE: &str = "clip.set_scale";
/// `song.set_scale` (`bl-model`): the set's current scale.
pub const SET_SCALE: &str = "song.set_scale";

/// Every command this module registers.
pub const IDS: &[&str] = &[SCALES];

/// Whether `id` is one of the commands [`register`] adds.
#[must_use]
pub fn handles(id: &str) -> bool {
    IDS.contains(&id)
}

/// One scale in the list.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ScaleEntry {
    /// The scale's name, as the Scale Name chooser shows it.
    pub name: String,
    /// The scale's steps above the root, in semitones: the notes it holds
    /// in every octave.
    pub steps: Vec<u8>,
}

/// What `song.scales` returns: the scale list, in the order the Scale Name
/// chooser shows it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ScaleList {
    /// The scales, in the list's order.
    pub scales: Vec<ScaleEntry>,
}

impl ScaleList {
    /// The scale list, as the chooser shows it.
    #[must_use]
    pub fn new() -> Self {
        Self {
            scales: ScaleName::ALL
                .into_iter()
                .map(|scale| ScaleEntry {
                    name: scale.label().to_owned(),
                    steps: scale.steps().to_vec(),
                })
                .collect(),
        }
    }
}

impl Default for ScaleList {
    fn default() -> Self {
        Self::new()
    }
}

/// Adds the scale list query to a registry.
///
/// # Errors
///
/// [`RegisterError`] if its ID is already registered.
pub fn register(registry: &mut Registry) -> Result<(), RegisterError> {
    registry.register(
        CommandSpec::new(SCALES, "Scale List")
            .description(
                "Reports the scale list: every scale a set can use, in the order the Scale \
                 Name chooser shows them, with each scale's steps above the root in semitones. \
                 The names are the ones `clip.set_scale` and `song.set_scale` take. Changes \
                 nothing. Use it to read the scales a set has before setting one.",
            )
            .permission(Permission::Read)
            .context(Global)
            .args::<NoArgs>()
            .result::<ScaleList>(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_query_registers_complete() {
        let mut registry = Registry::new();
        register(&mut registry).unwrap();
        assert_eq!(registry.len(), IDS.len());
        assert!(registry.problems().is_empty(), "{:?}", registry.problems());
        assert!(handles(SCALES));
    }

    #[test]
    fn the_list_holds_every_scale_with_its_steps() {
        let list = ScaleList::new();
        assert_eq!(list.scales.len(), ScaleName::ALL.len());
        assert_eq!(list.scales[0].name, "Major");
        assert_eq!(list.scales[0].steps, vec![0, 2, 4, 5, 7, 9, 11]);
    }
}

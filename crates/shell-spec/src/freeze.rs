//! Freezing and unfreezing tracks in the command registry (WP-4.14,
//! decisions D16 and D17). The behaviour is in the freeze and bounce spec,
//! R1 to R17 and R27 to R29.
//!
//! [`register`] adds their specs, and `bl-shell` renders the files and
//! applies the edit.

use bl_commands::Context::{Arrangement, Session, TrackHeaders};
use bl_commands::{CommandSpec, Permission, RegisterError, Registry};
use bl_model::TrackId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `track.freeze`: render the selected tracks to audio and play that.
pub const FREEZE: &str = "track.freeze";
/// `track.unfreeze`: play frozen tracks through their devices again.
pub const UNFREEZE: &str = "track.unfreeze";

/// Every command [`register`] adds, so a host knows which IDs to send to
/// `bl-shell`'s `Shell::run`.
pub const IDS: &[&str] = &[FREEZE, UNFREEZE];

/// Whether `id` is one of the freeze commands.
#[must_use]
pub fn handles(id: &str) -> bool {
    IDS.contains(&id)
}

/// Arguments of `track.freeze` and `track.unfreeze`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FreezeArgs {
    /// The tracks to freeze or unfreeze. Left out, the selected tracks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracks: Option<Vec<TrackId>>,
}

/// What a freeze did.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FreezeResult {
    /// The tracks that were frozen.
    pub frozen: Vec<TrackId>,
    /// The files the freeze wrote.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<String>,
}

/// Adds the freeze commands to a registry.
///
/// # Errors
///
/// [`RegisterError`] if one of their IDs is already registered.
pub fn register(registry: &mut Registry) -> Result<(), RegisterError> {
    for spec in specs() {
        registry.register(spec)?;
    }
    Ok(())
}

fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new(FREEZE, "Freeze Track")
            .description(
                "Renders the selected tracks to audio and plays that instead of their devices: \
                 an audio or MIDI track's Arrangement from 1.1.1 to the end of its last tail, \
                 and each Session clip on its own, taken after the devices and before the \
                 mixer. `tracks` names them (left out, the selected tracks). A group renders \
                 its tracks through their mixers and devices and, for each return they send \
                 to, the sum of those sends; a return renders from 1.1.1 to the end of the set. \
                 The track keeps its mixer, and its clips keep their places; its devices and \
                 clip settings are locked while frozen. The freeze runs offline through the \
                 processed-file writer, one undo step when every file is whole. It's an edit.",
            )
            .permission(Permission::Edit)
            .contexts([Arrangement, Session, TrackHeaders])
            .binding("S-TRACK-017")
            .args::<FreezeArgs>()
            .result::<FreezeResult>(),
        CommandSpec::new(UNFREEZE, "Unfreeze Track")
            .description(
                "Plays the selected frozen tracks through their devices and clips again, from \
                 the set as it now is, so edits made while frozen stay. Their freeze files stay \
                 on disk, so undoing the unfreeze freezes them again without rendering. \
                 `tracks` names them (left out, the selected tracks). It's an edit.",
            )
            .permission(Permission::Edit)
            .contexts([Arrangement, Session, TrackHeaders])
            .args::<FreezeArgs>()
            .result::<FreezeResult>(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_listed_id_is_registered_and_complete() {
        let mut registry = Registry::new();
        register(&mut registry).unwrap();
        assert_eq!(registry.len(), IDS.len());
        for id in IDS {
            assert!(registry.contains(id), "{id}");
        }
        let problems: Vec<String> = registry
            .problems()
            .iter()
            .map(ToString::to_string)
            .collect();
        assert!(problems.is_empty(), "{problems:#?}");
        assert_eq!(
            registry.get(FREEZE).unwrap().permission,
            Some(Permission::Edit)
        );
        assert_eq!(
            registry.get(UNFREEZE).unwrap().permission,
            Some(Permission::Edit)
        );
    }
}

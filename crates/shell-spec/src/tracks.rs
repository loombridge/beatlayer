//! Track commands that run on the shell (WP-2.19b): Duplicate Track Without
//! Clips (IMP-12).
//!
//! The document's own track commands (`track.create`, `track.delete`,
//! `track.move`, `track.rename`) are `bl-model`'s. Those here need the
//! shell, which knows the selection.

use bl_commands::Context::{Arrangement, Global, Session, TrackHeaders};
use bl_commands::{CommandSpec, Permission, RegisterError, Registry};
use bl_model::TrackId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `track.duplicate_without_clips`: duplicate tracks without their clips
/// (IMP-12).
pub const DUPLICATE_WITHOUT_CLIPS: &str = "track.duplicate_without_clips";

/// The undo step's name: Live's "Duplicate Track(s)" (observed in Live
/// 12.4.6), made distinct (decided).
pub const DUPLICATE_WITHOUT_CLIPS_STEP: &str = "Duplicate Track(s) Without Clips";

/// Every command [`register`] adds, so a host knows which IDs to send to
/// `bl-shell`'s `Shell::run`.
pub const IDS: &[&str] = &[DUPLICATE_WITHOUT_CLIPS];

/// Whether `id` is one of these track commands.
#[must_use]
pub fn handles(id: &str) -> bool {
    IDS.contains(&id)
}

/// Arguments of `track.duplicate_without_clips`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DuplicateTracksArgs {
    /// The tracks; the selected tracks if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracks: Option<Vec<TrackId>>,
}

/// What `track.duplicate_without_clips` returns.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TracksDuplicated {
    /// The new tracks, one for each track duplicated, in display order:
    /// now the selected tracks. A duplicated group's copies of its tracks
    /// are inside its copy.
    pub tracks: Vec<TrackId>,
}

/// Adds the track commands to a registry.
///
/// # Errors
///
/// [`RegisterError`] if one of their IDs is already registered.
pub fn register(registry: &mut Registry) -> Result<(), RegisterError> {
    registry.register(
        CommandSpec::new(DUPLICATE_WITHOUT_CLIPS, "Duplicate Track Without Clips")
            .description(
                "Duplicates tracks (`tracks`, or the selected tracks) without their clips or \
                 arrangement automation: each copy goes straight after its track, with its \
                 name (the next number where the name starts with the track's number), mixer \
                 settings, sends, routing, and devices with their settings, and the copies \
                 become the selected tracks. A group's copy holds copies of its tracks, \
                 without clips. Use it to start a new part with a track's sound. One undo \
                 step, named Duplicate Track(s) Without Clips. Main can't be duplicated.",
            )
            .permission(Permission::Edit)
            .contexts([Global, Arrangement, Session, TrackHeaders])
            .args::<DuplicateTracksArgs>()
            .result::<TracksDuplicated>(),
    )
}

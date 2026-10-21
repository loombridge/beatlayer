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

/// The undo step's name for Delete on the selected tracks (WP-4.22): Live's
/// "Delete Track(s)", like "Duplicate Track(s)".
pub const DELETE_STEP: &str = "Delete Track(s)";

/// `track.group`: group the selected tracks (T24).
pub const GROUP: &str = "track.group";

/// The undo step's name: Live's "Group Tracks" (observed in Live 12.4.3).
pub const GROUP_STEP: &str = "Group Tracks";

/// `track.ungroup`: ungroup the selected group track (T24).
pub const UNGROUP: &str = "track.ungroup";

/// The undo step's name: Live's "Ungroup Tracks" (observed in Live 12.4.3).
pub const UNGROUP_STEP: &str = "Ungroup Tracks";

/// `track.set_devices_enabled`: switch every top-level device on the selected
/// tracks on or off as one undo step (IMP-25).
pub const SET_DEVICES_ENABLED: &str = "track.set_devices_enabled";

/// The undo step's name when it switches the devices off.
pub const DEACTIVATE_ALL_DEVICES_STEP: &str = "Deactivate All Devices";

/// The undo step's name when it switches them on.
pub const ACTIVATE_ALL_DEVICES_STEP: &str = "Activate All Devices";

/// Every command [`register`] adds, so a host knows which IDs to send to
/// `bl-shell`'s `Shell::run`.
pub const IDS: &[&str] = &[DUPLICATE_WITHOUT_CLIPS, GROUP, UNGROUP, SET_DEVICES_ENABLED];

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

/// Arguments of `track.group`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GroupTracksArgs {
    /// The tracks to group; the selected tracks if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracks: Option<Vec<TrackId>>,
}

/// What `track.group` returns.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TracksGrouped {
    /// The new group track, now selected.
    pub track: TrackId,
}

/// Arguments of `track.ungroup`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UngroupTracksArgs {
    /// The group track to ungroup; the selected track if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track: Option<TrackId>,
}

/// What `track.ungroup` returns.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TracksUngrouped {
    /// The former members of the group, in order; the first of them is now selected.
    pub tracks: Vec<TrackId>,
}

/// Arguments of `track.set_devices_enabled`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetDevicesEnabledArgs {
    /// The tracks; the selected tracks if left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracks: Option<Vec<TrackId>>,
    /// Switch the devices on (true) or off (false); when left out, off unless
    /// every top-level device on the tracks is already off, when on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
}

/// What `track.set_devices_enabled` returns.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DevicesEnabled {
    /// The tracks it reached, in the order given.
    pub tracks: Vec<TrackId>,
    /// Whether their devices are on afterwards.
    pub enabled: bool,
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
    )?;
    registry.register(
        CommandSpec::new(GROUP, "Group Tracks")
            .description(
                "Groups tracks (`tracks`, or the selected tracks): inserts a group track named \
                 \"Group\" where the first track was, moves the tracks into it in order, \
                 routes each member to the group and the group to Main, leaves the group \
                 unfolded, and selects it. One undo step, named Group Tracks.",
            )
            .permission(Permission::Edit)
            .contexts([Global, Arrangement, Session, TrackHeaders])
            .binding("S-TRACK-006")
            .args::<GroupTracksArgs>()
            .result::<TracksGrouped>(),
    )?;
    registry.register(
        CommandSpec::new(UNGROUP, "Ungroup Tracks")
            .description(
                "Ungroups a group track (`track`, or the selected track): removes the group \
                 track, moves its members back where it was in order, routes each member to \
                 Main (or parent group default), and selects the first member. One undo step, \
                 named Ungroup Tracks.",
            )
            .permission(Permission::Edit)
            .contexts([Global, Arrangement, Session, TrackHeaders])
            .binding("S-TRACK-007")
            .args::<UngroupTracksArgs>()
            .result::<TracksUngrouped>(),
    )?;
    registry.register(
        CommandSpec::new(SET_DEVICES_ENABLED, "Deactivate All Devices")
            .description(
                "Switches every top-level device on the tracks (`tracks`, or the selected \
                 tracks) off, or on with `enabled`: true, as the track context menu's \
                 Deactivate All Devices does. It reaches only the tracks' own devices, not \
                 the devices inside a rack. With `enabled` left out it switches: off, unless \
                 every top-level device on the tracks is already off and there is one, when \
                 it turns them on. One undo step either way, named Deactivate All Devices or \
                 Activate All Devices.",
            )
            .permission(Permission::Edit)
            .contexts([Global, Arrangement, Session, TrackHeaders])
            .args::<SetDevicesEnabledArgs>()
            .result::<DevicesEnabled>(),
    )
}

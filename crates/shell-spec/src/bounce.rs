//! Bouncing tracks and pasting bounced audio in the command registry
//! (WP-4.15, rule G3 and IMP-10). The behaviour is in the freeze and bounce
//! spec, R18 to R26 and R27 to R29.
//!
//! [`register`] adds their specs, and `bl-shell` renders the files and
//! applies the edit.

use bl_commands::Context::{Arrangement, Session, TrackHeaders};
use bl_commands::{CommandSpec, Permission, RegisterError, Registry};
use bl_model::{ClipId, SceneId, TrackId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::editing::Position;

/// `track.bounce_in_place`: replace a track with the audio track its bounce
/// made.
pub const BOUNCE_IN_PLACE: &str = "track.bounce_in_place";
/// `track.bounce_to_new_track`: bounce the selection onto a new track.
pub const BOUNCE_TO_NEW_TRACK: &str = "track.bounce_to_new_track";
/// `track.paste_bounced_audio`: paste the copied material as audio.
pub const PASTE_BOUNCED_AUDIO: &str = "track.paste_bounced_audio";

/// Every command [`register`] adds, so a host knows which IDs to send to
/// `bl-shell`'s `Shell::run`.
pub const IDS: &[&str] = &[BOUNCE_IN_PLACE, BOUNCE_TO_NEW_TRACK, PASTE_BOUNCED_AUDIO];

/// Whether `id` is one of the bounce commands.
#[must_use]
pub fn handles(id: &str) -> bool {
    IDS.contains(&id)
}

/// How many channels a bounce's files hold (R20).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(
    description = "How many channels a bounce's files hold: `auto` (the default) one channel \
                   when the render's two channels are equal at every frame and two otherwise, \
                   `mono` one channel, the mean of the two, and `stereo` two channels, a mono \
                   render's in both."
)]
pub enum Channels {
    /// One channel when the render's two channels are equal at every frame,
    /// two otherwise.
    #[default]
    Auto,
    /// One channel: the mean of the two.
    Mono,
    /// Two channels: a mono render's in both.
    Stereo,
}

/// Arguments of `track.bounce_in_place`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BounceInPlaceArgs {
    /// The tracks to bounce. Left out, the selected tracks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracks: Option<Vec<TrackId>>,
    /// How many channels the bounced files hold.
    #[serde(default)]
    pub channels: Channels,
}

/// Arguments of `track.bounce_to_new_track`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BounceToNewTrackArgs {
    /// The tracks to bounce. Left out, the selected tracks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracks: Option<Vec<TrackId>>,
    /// The clips to bounce. Left out, the selected clips, or the time
    /// selection on the tracks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clips: Option<Vec<ClipId>>,
    /// Where the time selection starts. Left out, the Arrangement's time
    /// selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<Position>,
    /// Where the time selection ends. Left out, the Arrangement's time
    /// selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<Position>,
    /// How many channels the bounced files hold.
    #[serde(default)]
    pub channels: Channels,
}

/// Arguments of `track.paste_bounced_audio`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PasteBouncedAudioArgs {
    /// The track to paste onto. Left out, the selected track.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track: Option<TrackId>,
    /// The Session slot to paste into. Left out, the Arrangement.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scene: Option<SceneId>,
    /// Where the audio goes in the Arrangement. Left out, the time
    /// selection's start, or the insert marker.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<Position>,
    /// How many channels the pasted file holds.
    #[serde(default)]
    pub channels: Channels,
}

/// What a bounce did.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BounceResult {
    /// The tracks the bounce made or changed.
    pub tracks: Vec<TrackId>,
    /// The clips it made.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub clips: Vec<ClipId>,
    /// The files it wrote.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<String>,
}

/// Adds the bounce commands to a registry.
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
        CommandSpec::new(BOUNCE_IN_PLACE, "Bounce Track in Place")
            .description(
                "Renders the selected tracks to audio and replaces each with an audio track \
                 holding it: no devices, the track's name with \" (Bounce)\" after it, its \
                 volume, pan, sends, activator, solo, output routing, and mixer automation, and \
                 its clips playing the bounced audio. `tracks` names them (left out, the \
                 selected tracks). A group is bounced as one track, after its mixer. The files go \
                 into Samples/Processed/Bounce. It is destructive, since it deletes the track's \
                 devices and clips, and undoing it gives the source track back exactly.",
            )
            .permission(Permission::Destructive)
            .contexts([Arrangement, Session, TrackHeaders])
            .args::<BounceInPlaceArgs>()
            .result::<BounceResult>(),
        CommandSpec::new(BOUNCE_TO_NEW_TRACK, "Bounce to New Track")
            .description(
                "Renders the selected clips, or the time selection on the selected tracks, to \
                 audio on a new track directly after each source track, and switches the source \
                 material off. `tracks` names the source tracks (left out, the selected ones), \
                 `clips` the clips to bounce (left out, the selected clips, or the time \
                 selection), and `start` and `end` the time selection. The new track takes its \
                 source's name with \" (Bounce)\" after it, its colour, and its mixer settings, \
                 and the files go into Samples/Processed/Bounce. It is an edit.",
            )
            .permission(Permission::Edit)
            .contexts([Arrangement, Session, TrackHeaders])
            .binding("S-BOUNCE-001")
            .args::<BounceToNewTrackArgs>()
            .result::<BounceResult>(),
        CommandSpec::new(PASTE_BOUNCED_AUDIO, "Paste Bounced Audio")
            .description(
                "Renders what Copy last put on the clipboard as it is at the moment of pasting, \
                 and places it as Paste places clips: on the track, or in the slot, named. The \
                 destination is an audio track, an empty MIDI track, which becomes an audio \
                 track, or a take lane. The audio replaces what it covers. With nothing copied, \
                 or with the copied source deleted, the command is unavailable. It is an edit, \
                 and destructive when it would replace or shorten a clip.",
            )
            .permission(Permission::Edit)
            .contexts([Arrangement, Session, TrackHeaders])
            .binding("S-BOUNCE-002")
            .args::<PasteBouncedAudioArgs>()
            .result::<BounceResult>(),
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
            registry.get(BOUNCE_IN_PLACE).unwrap().permission,
            Some(Permission::Destructive)
        );
        assert_eq!(
            registry.get(BOUNCE_TO_NEW_TRACK).unwrap().permission,
            Some(Permission::Edit)
        );
        assert_eq!(
            registry.get(PASTE_BOUNCED_AUDIO).unwrap().permission,
            Some(Permission::Edit)
        );
    }
}

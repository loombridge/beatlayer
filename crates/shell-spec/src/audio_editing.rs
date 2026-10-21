//! Audio clip editing (WP-4.13): Crop, Consolidate, and Consolidate Time to
//! New Scene, in the command registry (decisions D16 and D17). The behaviour
//! they follow is in the audio clip editing spec (R14 to R28); WP-4.7's
//! Reverse sits beside them there.
//!
//! [`register`] adds their specs, which are pure data, so `cargo xtask` can
//! check them and list them in the command reference; `bl-shell` runs them by
//! ID, writing the new files through WP-4.20's processed-file writer.

use bl_commands::Context::{Arrangement, ClipView, Session};
use bl_commands::{Binding, CommandSpec, Permission, RegisterError, Registry};
use bl_model::{ClipId, ContentHash, SceneId, TrackId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::editing::{Position, TargetArgs};

/// `clip.crop`: crop a clip to what it plays, or to a time selection.
pub const CROP: &str = "clip.crop";
/// `clip.reverse`: give an audio clip a backwards copy of its file.
pub const REVERSE: &str = "clip.reverse";
/// `arrangement.reverse`: reverse the audio in a time selection, across
/// several clips (R).
pub const ARRANGEMENT_REVERSE: &str = "arrangement.reverse";
/// `arrangement.consolidate`: join the material in a range into one clip per
/// track (Cmd-J).
pub const CONSOLIDATE: &str = "arrangement.consolidate";
/// `arrangement.consolidate_to_scene`: consolidate a time selection into a new
/// Session scene.
pub const CONSOLIDATE_TO_SCENE: &str = "arrangement.consolidate_to_scene";

/// Every command [`register`] adds, so a host knows which IDs to send to
/// `bl-shell`'s `Shell::run`.
pub const IDS: &[&str] = &[
    CROP,
    REVERSE,
    ARRANGEMENT_REVERSE,
    CONSOLIDATE,
    CONSOLIDATE_TO_SCENE,
];

/// Whether `id` is one of the audio clip editing commands.
#[must_use]
pub fn handles(id: &str) -> bool {
    IDS.contains(&id)
}

/// Arguments of `clip.crop`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CropArgs {
    /// The clip to crop. Left out, the one clip the Arrangement or the
    /// Session View has selected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<ClipId>,
    /// Where the part to keep starts in the song. Give `start` and `end`
    /// together, or neither to keep the region the clip's own Start and End
    /// markers or loop brace hold.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<Position>,
    /// Where it ends, after `start`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<Position>,
}

/// What `clip.crop` did.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CropResult {
    /// The clip, cropped.
    pub clip: ClipId,
    /// The new file an audio clip plays, named by its content hash; `null`
    /// for a MIDI clip, which writes no file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media: Option<ContentHash>,
    /// Where the new file is, for an audio clip.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Whether the crop removed anything: audio outside the region or notes.
    pub removed: bool,
}

/// Arguments of `clip.reverse`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReverseArgs {
    /// The clip to reverse. Left out, the one clip the Arrangement or the
    /// Session View has selected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<ClipId>,
}

/// Arguments of `arrangement.reverse`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReverseRangeArgs {
    /// The tracks a span of time covers. Left out, the selected tracks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracks: Option<Vec<TrackId>>,
    /// Where the span starts. Left out, the time selection's start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<Position>,
    /// Where the span ends, after `start`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<Position>,
}

/// What `clip.reverse` did.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReverseResult {
    /// The clip, reversed.
    pub clip: ClipId,
    /// The new file the clip plays, named by its content hash; `null` for a
    /// MIDI clip, which isn't reversed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media: Option<ContentHash>,
    /// Where the new file is.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// What `arrangement.reverse` did.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReverseRangeResult {
    /// The clips it reversed.
    pub clips: Vec<ClipId>,
    /// The tracks they sit on.
    pub tracks: Vec<TrackId>,
    /// The new files, one per reversed clip, and where each is.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<String>,
}

/// What `arrangement.consolidate` did.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ConsolidateResult {
    /// The new clip on each track the range covered, in track order.
    pub created: Vec<ClipId>,
    /// The tracks they sit on, in the same order.
    pub tracks: Vec<TrackId>,
    /// The new files, one per audio track, and where each is.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<String>,
}

/// Arguments of `arrangement.consolidate_to_scene`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConsolidateSceneArgs {
    /// The tracks a span of time covers. Left out, the selected tracks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracks: Option<Vec<TrackId>>,
    /// Where the span starts. Left out, the time selection's start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<Position>,
    /// Where the span ends, after `start`. Left out, the time selection's
    /// end.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<Position>,
    /// The scene to insert the new one after. Left out, the scene the Session
    /// View has selected, or the last scene.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scene: Option<SceneId>,
}

/// What `arrangement.consolidate_to_scene` did.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ConsolidateSceneResult {
    /// The new scene, directly after the one the selection was on.
    pub scene: SceneId,
    /// The new clip in each covered track's slot, in track order.
    pub created: Vec<ClipId>,
    /// The tracks they sit on, in the same order.
    pub tracks: Vec<TrackId>,
    /// The new files, one per audio track, and where each is.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<String>,
}

/// Adds the audio clip editing commands to a registry.
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
        CommandSpec::new(CROP, "Crop Clip")
            .description(
                "Crops a clip to the part of it that plays, or to a span of song time: an \
                 audio clip's region is written as a new, shorter file and the clip plays it, \
                 with its Start and End markers, loop brace, and warp markers moved back so it \
                 plays exactly what it played before; a MIDI clip loses the notes that start \
                 outside the region, and writes no file. `clip` names the clip (left out, the \
                 selected one); `start` and `end` give the span to keep as song positions \
                 (left out, the region the clip's own markers hold). A span the clip goes round \
                 its loop over isn't one stretch of its file, and is refused, with a message \
                 naming Consolidate, which writes such a stretch out. An audio clip's new file \
                 goes through the processed-file writer, and the edit is one undo step when it \
                 is whole. It's an edit, and destructive as well when it removes anything: \
                 audio outside the region, or notes.",
            )
            .permission(Permission::Edit)
            .contexts([Arrangement, Session, ClipView])
            .binding("S-ARR-003")
            .binding("S-SES-028")
            .args::<CropArgs>()
            .result::<CropResult>(),
        CommandSpec::new(REVERSE, "Reverse Clip")
            .description(
                "Gives an audio clip a backwards copy of its file: the copy is written through \
                 the processed-file writer, and the clip plays it, with its Start and End \
                 markers, loop brace and warp markers mirrored about the file, so it plays the \
                 same stretch of sound backwards. The clip's envelopes stay where they are, and \
                 its loop switch, gain, transpose, detune, warp mode and fades don't change. \
                 Reversing a clip whose file is already a reversed copy gives it back the file \
                 it was made from, writing nothing. `clip` names the clip (left out, the \
                 selected one). A MIDI clip isn't reversed, and nothing changes.",
            )
            .permission(Permission::Edit)
            .contexts([Arrangement, Session, ClipView])
            .args::<ReverseArgs>()
            .result::<ReverseResult>(),
        CommandSpec::new(ARRANGEMENT_REVERSE, "Reverse Clip(s)")
            .description(
                "Reverses the audio in a span of the Arrangement, across several clips: the \
                 time selection, or the span of the selected clips (`tracks`, with `start` and \
                 `end`). The clips the span's edges cross are split there first, each part \
                 inside is reversed, and the parts are mirrored within the span on their \
                 tracks, so the span plays backwards as a whole. A span holding a MIDI clip \
                 isn't reversed, and nothing changes. R (Ctrl-R) runs it on the selection.",
            )
            .permission(Permission::Edit)
            .context(Arrangement)
            .binding(Binding::new("S-ARR-029"))
            .args::<ReverseRangeArgs>()
            .result::<ReverseRangeResult>(),
        CommandSpec::new(CONSOLIDATE, "Consolidate")
            .description(
                "Joins the material in a range on each selected track into one new clip per \
                 track, replacing the clips it covers: the time selection, or the span of the \
                 selected clips (`clips`, or `tracks` with `start` and `end`). Clips the \
                 range's edges cross are split there first and the parts inside replaced; a \
                 track with no clip in the range gets nothing, and the new clips are selected. \
                 On an audio track the new clip plays a file rendered offline from what the \
                 clips played over the range, before the track's devices and mixer, normalised \
                 with the clip gain making up for it, mono when its channels are equal; on a \
                 MIDI track it holds every note the range played, with each repetition of a \
                 looped clip written out, and the source clips' clip envelopes. Cmd-J (Ctrl-J) \
                 and Edit ▸ Consolidate run it on the selection. It's destructive, since it \
                 deletes the clips it replaces, so an agent asks first.",
            )
            .permission(Permission::Destructive)
            .context(Arrangement)
            .binding(Binding::new("S-ARR-002"))
            .args::<TargetArgs>()
            .result::<ConsolidateResult>(),
        CommandSpec::new(CONSOLIDATE_TO_SCENE, "Consolidate Time to New Scene")
            .description(
                "Consolidates a span of the Arrangement into a new Session scene, leaving the \
                 Arrangement as it was: a new scene is inserted directly after `scene` (left \
                 out, the scene the Session View has selected, or the last one), and each \
                 track the span covers that holds a clip in it gets one new clip in that \
                 track's slot of the new scene, made as Consolidate makes it. `tracks`, \
                 `start`, and `end` give the span (left out, the time selection). The result's \
                 `scene` is the new scene.",
            )
            .permission(Permission::Edit)
            .context(Arrangement)
            .args::<ConsolidateSceneArgs>()
            .result::<ConsolidateSceneResult>(),
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
        // Cropping removes material decided from its arguments; Consolidate
        // deletes the clips it replaces whatever it is given.
        let class = |id: &str| registry.get(id).unwrap().permission;
        assert_eq!(class(CROP), Some(Permission::Edit));
        assert_eq!(class(CONSOLIDATE), Some(Permission::Destructive));
        assert_eq!(class(CONSOLIDATE_TO_SCENE), Some(Permission::Edit));
    }
}

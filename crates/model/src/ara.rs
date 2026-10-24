//! ARA 2 hosting's part of the document (IMP-20, the ARA hosting spec): which
//! device of a track hosts ARA, and which of the track's clips it renders.
//! The engine's compiler and the plug-in host both ask here, so the clip
//! player leaves exactly the clips the plug-in is given.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{AudioData, Clip, ClipData, Device, EDGE_FADE_SECONDS, Track, TrackKind};

/// What an ARA plug-in does with the regions it is given, as its factory
/// declares it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AraSupport {
    /// Whether it plays a region at another speed than its audio's, which a
    /// warped clip needs (the factory's time-stretch transformation).
    pub timestretch: bool,
}

/// Whether the ARA plug-in `support` describes renders `clip` instead of the
/// track's clip player (ara-hosting R4). It does for an unmuted audio clip
/// that plays its file's range once, at 0 dB, with fades no longer than the
/// edge fades new clips get, which the plug-in's own region edges take over,
/// and with no crossfade, transposition, envelope or groove; and a warped
/// one only if the plug-in time-stretches. Any other clip stays with the
/// player, which does all of those.
#[must_use]
pub fn ara_renders(clip: &Clip, support: AraSupport) -> bool {
    let ClipData::Audio(audio) = &clip.data else {
        return false;
    };
    !clip.mute
        && clip.envelopes.is_empty()
        && clip.groove.is_none()
        && (support.timestretch || !audio.warp)
        && plain(audio)
}

/// The part of [`ara_renders`] that reads the clip's audio settings.
fn plain(audio: &AudioData) -> bool {
    const FADE: f64 = EDGE_FADE_SECONDS + 1e-9;
    !audio.looping
        && audio.gain == 1.0
        && audio.fade_in <= FADE
        && audio.fade_out <= FADE
        && audio.fade_in_before == 0.0
        && audio.fade_out_after == 0.0
        && audio.transpose == 0
        && audio.detune == 0.0
}

/// The device of `track` that hosts ARA, and what it supports: the first
/// switched-on device of the track's own chain that `support` names. Only an
/// audio track that isn't frozen has one.
#[must_use]
pub fn ara_device(
    track: &Track,
    support: impl Fn(&Device) -> Option<AraSupport>,
) -> Option<(&Device, AraSupport)> {
    if track.kind() != TrackKind::Audio || track.is_frozen() {
        return None;
    }
    track
        .devices()
        .filter(|device| device.enabled)
        .find_map(|device| support(device).map(|found| (device, found)))
}

/// The document archive of one ARA plug-in, as the set keeps it: what the
/// plug-in's document controller stored when the set was saved, for it to
/// restore when the set opens (ara-hosting R10).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[schemars(
    description = "An ARA plug-in's own record of the set's audio: `plugin`, the ID of its ARA \
                   factory, `archive_id`, the ID of the archive format it wrote, and `data`, its \
                   document archive, in base64."
)]
pub struct AraArchive {
    /// The ARA factory ID of the plug-in that wrote it.
    pub plugin: String,
    /// The document archive ID the plug-in's factory gave the archive, which
    /// says whether a newer version of the plug-in can read it.
    pub archive_id: String,
    /// The archive the plug-in's document controller stored; base64 in the
    /// set file.
    #[serde(with = "archive_bytes")]
    #[schemars(with = "String")]
    pub data: Vec<u8>,
}

/// Serialises an archive's bytes as base64 text.
mod archive_bytes {
    use serde::{Deserialize, Deserializer, Serializer};

    pub(super) fn serialize<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&crate::base64::encode(bytes))
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Vec<u8>, D::Error> {
        let text = String::deserialize(deserializer)?;
        crate::base64::decode(&text).ok_or_else(|| serde::de::Error::custom("not base64"))
    }
}

//! The audio track a bounce made (WP-4.15).
//!
//! A bounce renders a track's material to audio and puts that audio on a
//! track: Bounce Track in Place replaces the track with it, and Bounce to
//! New Track makes it beside the source (the freeze and bounce spec, R18
//! to R26). [`BouncedTrack`] is what the new track holds: its name, its
//! mixer settings, its sends, its output routing, its mixer automation, and
//! its clips.
//!
//! The bounced clips are ordinary audio clips: they play the bounce file
//! warped in Beats mode, as a frozen clip does (R19), so at the tempo the
//! bounce was taken at they give the frames the source gave.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::automation::{AutomationTarget, Envelope};
use crate::clip::Clip;
use crate::ids::{ClipId, SceneId, TrackId};
use crate::track::{Mixer, Output};

/// One automated mixer control of a bounced track: the envelope that drove
/// it on the source track, and whether it was driving it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(
    description = "One automated mixer control of a bounced track: the control it drives and \
                   its envelope."
)]
pub struct BouncedAutomation {
    /// The control: a mixer control or a send.
    pub target: AutomationTarget,
    /// Its envelope.
    pub envelope: Envelope,
    /// Whether the envelope drove the control.
    pub enabled: bool,
}

/// The audio track a bounce made: what the new track holds.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(
    description = "The audio track a bounce made: its name, its mixer settings, its sends, its \
                   output routing, its mixer automation, and its clips."
)]
pub struct BouncedTrack {
    /// The track's name: its source's name with " (Bounce)" after it.
    pub name: String,
    /// Its mixer settings: its source's volume, pan, activator, and solo.
    #[serde(default)]
    pub mixer: Mixer,
    /// Its sends, by return track: the level of each send that isn't at
    /// −∞ dB.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sends: Vec<(TrackId, f64)>,
    /// Where its audio goes.
    #[serde(default)]
    pub output: Output,
    /// Its mixer automation: one envelope per automated mixer control.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub automation: Vec<BouncedAutomation>,
    /// The clips on its Arrangement lane.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub clips: Vec<Clip>,
    /// The clips in its Session slots, by scene.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub slots: Vec<(SceneId, Clip)>,
}

impl BouncedTrack {
    /// The IDs of the clips the track holds, on its lane and in its slots.
    #[must_use]
    pub fn clip_ids(&self) -> Vec<ClipId> {
        self.clips
            .iter()
            .map(|clip| clip.id)
            .chain(self.slots.iter().map(|(_, clip)| clip.id))
            .collect()
    }
}

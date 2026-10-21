//! A frozen track's files and how its clips play them (WP-4.14).
//!
//! Freezing renders a track to audio and leaves the document as it was: the
//! track keeps its devices, its clips, and their settings, so unfreezing
//! plays them again. A [`FrozenTrack`] records the files the render wrote,
//! and the part of each file a clip plays, so the engine can play the frozen
//! audio instead of running the devices (the freeze and bounce spec, R1 to
//! R17).
//!
//! The files belong to the set's media table by content hash, like any
//! other processed file. A tail is the sound that goes on after an
//! Arrangement clip's end where no other clip is; it isn't a clip of the
//! set, so it's kept here with its own ID and goes when the track is
//! unfrozen.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use bl_core::time::Ticks;

use crate::ids::{ClipId, ContentHash, SceneId};

/// A placeholder for a frozen clip's original data that a set saved
/// without it loads empty: the clip keeps its frozen audio until it's
/// unfrozen and played again from its devices.
#[must_use]
pub fn no_data() -> crate::clip::ClipData {
    crate::clip::ClipData::Midi(crate::clip::MidiData::default())
}

/// The part of a freeze file one clip plays.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(
    description = "One clip's stretch of a freeze file: the file it plays, the second it \
                   starts at, and the second it ends at."
)]
pub struct FrozenClip {
    /// The clip in the set.
    pub clip: ClipId,
    /// The freeze file, by content hash.
    pub media: ContentHash,
    /// Where the clip starts in the file, in seconds.
    pub start: f64,
    /// Where it ends in the file, in seconds.
    pub end: f64,
    /// The tempo the clip was rendered at, in BPM: its warp map keeps the
    /// file on the beats it played then (R6).
    pub tempo: f64,
    /// The clip's length in ticks, for the warp map.
    pub length: Ticks,
    /// The clip's own data before the freeze, kept so unfreezing plays it
    /// again (R10).
    #[serde(default = "crate::freeze::no_data")]
    pub original: crate::clip::ClipData,
    /// The Session slot it belongs to; `null` for an Arrangement clip.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scene: Option<SceneId>,
    /// For a frozen Session clip, the length of one turn of its loop in
    /// seconds; `null` for an Arrangement clip, or a clip that doesn't loop.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cycle: Option<f64>,
}

/// A tail region: the sound after an Arrangement clip's end where no other
/// clip is (R3).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(
    description = "A frozen tail: the clip it follows, the clip ID the Arrangement draws it \
                   under, where it starts and how long it is, and the part of the freeze file it \
                   plays."
)]
pub struct FrozenTail {
    /// The clip the tail follows.
    pub clip: ClipId,
    /// The ID the Arrangement gives the tail region. Tail regions aren't
    /// clips of the set.
    pub id: ClipId,
    /// Where it starts in the song.
    pub start: Ticks,
    /// How long it is.
    pub length: Ticks,
    /// The freeze file, by content hash.
    pub media: ContentHash,
    /// Where it starts in the file, in seconds.
    pub file_start: f64,
    /// Where it ends in the file, in seconds.
    pub file_end: f64,
}

/// A frozen track's render: the parts of the files its clips play, its
/// tails, and, for a group or return track, the file its own output plays.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(
    description = "A frozen track: the freeze file stretch of each clip, its tail regions, and \
                   the whole-Arrangement output of a frozen group or return track."
)]
pub struct FrozenTrack {
    /// The stretch of frozen audio each clip plays, in clip order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub clips: Vec<FrozenClip>,
    /// The tails that follow the Arrangement clips (R3).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tails: Vec<FrozenTail>,
    /// A frozen group or return track's output over the whole Arrangement
    /// (R14, R15); `None` for an audio or MIDI track.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<FrozenOutput>,
}

/// A frozen group or return track's whole-Arrangement render (R14, R15).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(
    description = "A frozen group or return track's output: its freeze file, the frame the \
                   render starts at, and how many frames it holds."
)]
pub struct FrozenOutput {
    /// The clip ID the frozen output plays under in the Arrangement.
    pub id: ClipId,
    /// The freeze file, by content hash.
    pub media: ContentHash,
    /// The tempo the render was taken at, in BPM.
    pub tempo: f64,
    /// The first song frame the file holds.
    pub start: Ticks,
    /// The song length the file covers.
    pub length: Ticks,
}

impl FrozenTrack {
    /// The audio data a frozen clip plays: the region's stretch of the
    /// freeze file, warped in Beats mode to the beats it was rendered at
    /// (R6).
    #[must_use]
    pub fn audio_for(&self, region: &FrozenClip) -> crate::clip::ClipData {
        use crate::clip::{AudioData, ClipData};
        use crate::warp::WarpMarker;
        let mut audio = AudioData::new(region.media);
        // A warped clip's Start, End and loop brace are whole ticks of its
        // contents, not seconds in the file (the audio clips spec); the warp
        // map takes them to the file.
        let played = if region.cycle.is_some() {
            region.length * 2
        } else {
            region.length
        };
        audio.start = 0.0;
        audio.end = Some(played.get() as f64);
        audio.warp = true;
        audio.tempo = Some(region.tempo);
        audio.warp_mode = crate::warp_settings::WarpMode::Beats;
        audio.warp_markers = vec![
            WarpMarker {
                time: region.start,
                beat: Ticks::ZERO,
            },
            WarpMarker {
                time: region.end,
                beat: played,
            },
        ];
        if region.cycle.is_some() {
            audio.looping = true;
            audio.loop_start = 0.0;
            audio.loop_end = Some(played.get() as f64);
        }
        ClipData::Audio(audio)
    }

    /// The stretch of frozen audio `clip` plays, if it has one.
    #[must_use]
    pub fn clip(&self, clip: ClipId) -> Option<&FrozenClip> {
        self.clips.iter().find(|frozen| frozen.clip == clip)
    }

    /// Whether there is nothing frozen.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.clips.is_empty() && self.tails.is_empty() && self.output.is_none()
    }
}

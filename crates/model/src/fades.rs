//! The limits of an audio clip's fades and crossfades (the audio clips spec,
//! R25): a fade takes in no point where its clip goes round its loop brace
//! and reads nothing before the start of its file; a clip's fade-in ends no
//! later than its fade-out begins; and a crossfade lies between the two
//! clips' other edges.
//!
//! [`Document::fade_limits`] says how far each fade of a clip may go, which
//! the handles' dotted line draws; the commands check what they set against
//! the same arithmetic.

use bl_core::time::{TempoMap, Ticks};

use crate::clip::{AudioData, Clip, ClipData, crossfades};
use crate::document::Document;
use crate::error::ModelError;
use crate::ids::ClipId;

/// How much a float fade length may overshoot a limit before it is refused:
/// the arithmetic of ticks and seconds is not exact.
const SLACK: f64 = 1e-9;

/// How far one edge's fade may go, in seconds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EdgeRoom {
    /// The longest the fade may be inside its clip, measured from the edge.
    pub inside: f64,
    /// The furthest a crossfade there may reach past the edge, over the clip
    /// that meets this one; 0 where no clip meets it.
    pub outside: f64,
}

/// How far a clip's two fades may go (R25), with the clip's own other fade
/// and the clip that meets each edge taken into account.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FadeLimits {
    /// The fade-in's room, at the clip's start.
    pub start: EdgeRoom,
    /// The fade-out's room, at the clip's end.
    pub end: EdgeRoom,
}

/// Where a looped clip goes round its loop brace, in seconds from the clip's
/// start.
#[derive(Clone, Copy, Debug, Default)]
struct Wraps {
    /// The first one after the start.
    first: Option<f64>,
    /// The last one before the clip's end.
    before_end: Option<f64>,
    /// The first one at or after the clip's end.
    from_end: Option<f64>,
}

impl AudioData {
    /// The time of the Start marker in the file, in seconds: negative where
    /// it lies before the file's start, which only an unwarped clip trimmed
    /// by hand can.
    #[must_use]
    pub fn start_in_file(&self) -> f64 {
        if self.warp {
            self.warp_map()
                .map_or(0.0, |map| map.beat_to_time(self.start))
        } else {
            self.start
        }
    }
}

/// Where `clip`, which plays `audio`, goes round its loop brace. A brace that
/// runs to the end of the file has a length the set doesn't record, so such a
/// clip has none the model can see.
fn wraps(clip: &Clip, audio: &AudioData, tempo: &TempoMap) -> Wraps {
    let (true, Some(loop_end)) = (audio.looping, audio.loop_end) else {
        return Wraps::default();
    };
    let length = loop_end - audio.loop_start;
    let first = loop_end - audio.start;
    if !(length > 0.0 && length.is_finite() && first > 0.0) {
        return Wraps::default();
    }
    // The clip's length, and where each wrap falls, in the brace's own unit:
    // seconds in the file for an unwarped clip, ticks of its contents for a
    // warped one.
    let clip_seconds = |at: Ticks| tempo.ticks_to_seconds(at);
    let clip_length = if audio.warp {
        clip.length.get() as f64
    } else {
        clip_seconds(clip.end()) - clip_seconds(clip.start)
    };
    let to_seconds = |unit: f64| {
        if audio.warp {
            clip_seconds(clip.start + Ticks(unit.round() as i64)) - clip_seconds(clip.start)
        } else {
            unit
        }
    };
    let count = ((clip_length - first) / length).ceil();
    let before_end = (count >= 1.0).then_some(first + (count - 1.0) * length);
    let from_end = first + count.max(0.0) * length;
    Wraps {
        first: Some(to_seconds(first)),
        before_end: before_end.map(to_seconds),
        from_end: Some(to_seconds(from_end)),
    }
}

/// A clip's length in seconds under `tempo`.
fn seconds_long(clip: &Clip, tempo: &TempoMap) -> f64 {
    tempo.ticks_to_seconds(clip.end()) - tempo.ticks_to_seconds(clip.start)
}

/// Checks the fades of `audio`, the data `clip` would have, against R25's
/// limits.
///
/// # Errors
///
/// [`ModelError::InvalidInput`] naming the limit that is passed.
pub(crate) fn check_fades(
    clip: &Clip,
    audio: &AudioData,
    tempo: &TempoMap,
) -> Result<(), ModelError> {
    let longest = seconds_long(clip, tempo);
    if audio.fade_in + audio.fade_out > longest + SLACK {
        return Err(ModelError::InvalidInput(
            "a clip's fade-in must end no later than its fade-out begins",
        ));
    }
    if audio.fade_in_before > audio.start_in_file().max(0.0) + SLACK {
        return Err(ModelError::InvalidInput(
            "a fade can't reach before the start of the clip's file",
        ));
    }
    let wraps = wraps(clip, audio, tempo);
    let past = |wrap: Option<f64>, edge: f64| wrap.is_some_and(|wrap| wrap + SLACK < edge);
    if past(wraps.first, audio.fade_in)
        || past(
            wraps.from_end.map(|wrap| wrap - longest),
            audio.fade_out_after,
        )
        || wraps
            .before_end
            .is_some_and(|wrap| wrap + SLACK < longest && longest - wrap + SLACK < audio.fade_out)
    {
        return Err(ModelError::InvalidInput(
            "a fade can't take in the point where its clip goes round its loop",
        ));
    }
    Ok(())
}

impl Document {
    /// How far the fades of the audio clip `clip` may go (R25); `None` if
    /// there is no such audio clip.
    #[must_use]
    pub fn fade_limits(&self, clip: ClipId) -> Option<FadeLimits> {
        let track = self.track(self.clip_track(clip)?)?;
        let tempo = &self.settings().tempo;
        let this = track.clip(clip)?;
        let ClipData::Audio(audio) = &this.data else {
            return None;
        };
        let length = seconds_long(this, tempo);
        let own = wraps(this, audio, tempo);
        let meets = |earlier: bool| {
            track.clips().find_map(|other| {
                let adjoins = if earlier {
                    other.end() == this.start
                } else {
                    other.start == this.end()
                };
                match &other.data {
                    ClipData::Audio(data) if adjoins && other.id != this.id => Some((other, data)),
                    _ => None,
                }
            })
        };
        let existing = crossfades(track.clips());
        let crossfaded = |earlier: bool| {
            existing
                .iter()
                .any(|c| if earlier { c.later } else { c.earlier } == clip)
        };
        // The fade-in: its length inside the clip stops at the clip's
        // fade-out and the first time round the loop.
        let mut start = EdgeRoom {
            inside: (length - audio.fade_out)
                .min(own.first.unwrap_or(f64::INFINITY))
                .max(0.0),
            outside: 0.0,
        };
        if let Some((other, data)) = meets(true) {
            let other_length = seconds_long(other, tempo);
            let reach = wraps(other, data, tempo);
            start.outside = (other_length - data.fade_in)
                .min(
                    reach
                        .before_end
                        .map_or(f64::INFINITY, |wrap| other_length - wrap),
                )
                .min(audio.start_in_file())
                .max(0.0);
            if crossfaded(true) {
                // The crossfade's far end is in the earlier clip, which
                // plays on past its end up to its next time round the loop.
                let tail = reach
                    .from_end
                    .map_or(f64::INFINITY, |wrap| wrap - other_length);
                start.inside = start.inside.min(tail).max(0.0);
            }
        }
        let mut end = EdgeRoom {
            inside: (length - audio.fade_in)
                .min(own.before_end.map_or(f64::INFINITY, |wrap| length - wrap))
                .max(0.0),
            outside: 0.0,
        };
        if let Some((other, data)) = meets(false) {
            let other_length = seconds_long(other, tempo);
            let reach = wraps(other, data, tempo);
            end.outside = (other_length - data.fade_out)
                .min(reach.first.unwrap_or(f64::INFINITY))
                .min(own.from_end.map_or(f64::INFINITY, |wrap| wrap - length))
                .max(0.0);
            if crossfaded(false) {
                // The crossfade's far end is in the later clip, which plays
                // on before its start back to the start of its file.
                end.inside = end.inside.min(data.start_in_file()).max(0.0);
            }
        }
        Some(FadeLimits { start, end })
    }
}

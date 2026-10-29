//! A sampler's sample edits (WP-5.15, IMP-28; the multisampler spec, R55
//! and R56): the ordered list of Trim, Cut, Reverse, Fade In, Fade Out and
//! Normalize kept with a sample, and where they move the markers.
//!
//! The list is data only. `bl-media` renders it over the decoded sample, off
//! the audio thread, and the source file is never written.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::ModelError;

/// What an edit does (R55).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SampleEditKind {
    /// Keeps the selection and drops the rest.
    Trim,
    /// Removes the selection and closes the gap.
    Cut,
    /// Reverses the selection, or the whole sample with none.
    Reverse,
    /// A straight line in gain from silence to full over the selection, or
    /// the whole sample with none.
    FadeIn,
    /// A straight line in gain from full to silence over the selection, or
    /// the whole sample with none.
    FadeOut,
    /// Scales the selection, or the whole sample with none, so its largest
    /// magnitude reaches 0 dBFS.
    Normalize,
}

impl SampleEditKind {
    /// The undo step's name (R55).
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Trim => "Trim",
            Self::Cut => "Cut",
            Self::Reverse => "Reverse",
            Self::FadeIn => "Fade In",
            Self::FadeOut => "Fade Out",
            Self::Normalize => "Normalize",
        }
    }
}

/// A span of a sample, in frames at the sample's own rate: `from` inclusive
/// to `to` exclusive.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SampleRange {
    /// The first frame.
    pub from: u64,
    /// The frame after the last.
    pub to: u64,
}

/// One edit in a sample's list. Its range counts frames of the sample as the
/// edits before it left it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SampleEdit {
    /// What it does.
    pub kind: SampleEditKind,
    /// The selection it acts on; the whole sample if left out. Trim and Cut
    /// need one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<SampleRange>,
}

impl SampleEdit {
    /// The sample's length after this edit, for one of `length` frames.
    ///
    /// # Errors
    ///
    /// [`ModelError::InvalidValue`] for a range that is empty or past the
    /// end, a Trim or Cut with no range, or a Cut that would remove it all.
    pub fn length_after(&self, length: u64) -> Result<u64, ModelError> {
        let bad = |what: &'static str, value: u64| {
            Err(ModelError::InvalidValue {
                what,
                value: value as f64,
            })
        };
        let span = match self.range {
            Some(SampleRange { from, to }) if from < to && to <= length => Some(to - from),
            Some(SampleRange { to, .. }) => return bad("edit selection", to),
            None => None,
        };
        match (self.kind, span) {
            (SampleEditKind::Trim, Some(span)) => Ok(span),
            (SampleEditKind::Cut, Some(span)) if span < length => Ok(length - span),
            (SampleEditKind::Cut, Some(span)) => bad("edit selection", span),
            (SampleEditKind::Trim | SampleEditKind::Cut, None) => bad("edit selection", 0),
            _ => Ok(length),
        }
    }

    /// Where the audio a marker marked is after this edit (R56): a marker
    /// after a Cut moves earlier by the Cut's length, one inside it moves
    /// to its start; one outside a Trim's span moves to the span's nearer
    /// end. The other edits keep the length and move nothing.
    #[must_use]
    pub fn follow(&self, marker: u64) -> u64 {
        match (self.kind, self.range) {
            (SampleEditKind::Trim, Some(SampleRange { from, to })) => marker.clamp(from, to) - from,
            (SampleEditKind::Cut, Some(SampleRange { from, to })) => {
                if marker >= to {
                    marker - (to - from)
                } else {
                    marker.min(from)
                }
            }
            _ => marker,
        }
    }
}

/// The part of a Simpler's sample between its Start and End flags (the
/// sampler spec, R22). Absent from the device for the whole sample.
///
/// Flags are saved as frames of the edited sample at its own rate, as the
/// slices and the edits' selections are. A set saved before the region
/// existed (R24) holds the shares of the decoded sample its Start and Length
/// parameters chose until the next edit of the region writes frames, because
/// turning a share into a frame needs the sample's length, which the saved
/// file doesn't hold.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum SampleRegion {
    /// The Start and End flags, in frames; Start is before End.
    Frames {
        /// The first frame played.
        start: u64,
        /// The frame after the last one played.
        end: u64,
    },
    /// An older set's region, as shares (0 to 1) of the decoded sample.
    Shares {
        /// Where the region starts.
        start_share: f64,
        /// Where it ends.
        end_share: f64,
    },
}

impl SampleRegion {
    /// The flags as frames of a sample `length` frames long: shares round
    /// down to a frame, as playback rounded them before the flags existed,
    /// and a flag past the sample's end comes back to it.
    #[must_use]
    pub fn frames_in(&self, length: u64) -> (u64, u64) {
        match *self {
            Self::Frames { start, end } => (start.min(length), end.min(length)),
            Self::Shares {
                start_share,
                end_share,
            } => (
                ((start_share * length as f64) as u64).min(length),
                ((end_share * length as f64) as u64).min(length),
            ),
        }
    }

    /// Where the audio the flags mark is after `edit`, or `None` when the
    /// edit leaves nothing between them (R22): the flags move as a
    /// multisampler layer's markers do (the multisampler spec, R56). Shares
    /// stay shares of the edited sample.
    #[must_use]
    pub fn after(self, edit: &SampleEdit) -> Option<Self> {
        match self {
            Self::Frames { start, end } => {
                let (start, end) = (edit.follow(start), edit.follow(end));
                (start < end).then_some(Self::Frames { start, end })
            }
            shares @ Self::Shares { .. } => Some(shares),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edit(kind: SampleEditKind, from: u64, to: u64) -> SampleEdit {
        SampleEdit {
            kind,
            range: Some(SampleRange { from, to }),
        }
    }

    #[test]
    fn lengths_and_markers_follow_a_cut_and_a_trim() {
        let cut = edit(SampleEditKind::Cut, 20, 50);
        assert_eq!(cut.length_after(100).unwrap(), 70);
        assert_eq!(
            [10, 20, 30, 50, 80].map(|m| cut.follow(m)),
            [10, 20, 20, 20, 50]
        );
        let trim = edit(SampleEditKind::Trim, 20, 50);
        assert_eq!(trim.length_after(100).unwrap(), 30);
        assert_eq!(
            [10, 20, 30, 50, 80].map(|m| trim.follow(m)),
            [0, 0, 10, 30, 30]
        );
        let fade = edit(SampleEditKind::FadeIn, 20, 50);
        assert_eq!(fade.length_after(100).unwrap(), 100);
        assert_eq!(fade.follow(80), 80);
    }

    #[test]
    fn an_edit_that_does_not_fit_is_refused() {
        for bad in [
            edit(SampleEditKind::Cut, 0, 100),
            edit(SampleEditKind::Reverse, 50, 50),
            edit(SampleEditKind::FadeOut, 90, 101),
            SampleEdit {
                kind: SampleEditKind::Trim,
                range: None,
            },
        ] {
            assert!(bad.length_after(100).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn the_flags_follow_an_edit_and_shares_round_down_to_a_frame() {
        let region = SampleRegion::Frames { start: 30, end: 80 };
        let cut = edit(SampleEditKind::Cut, 10, 20);
        assert_eq!(
            region.after(&cut),
            Some(SampleRegion::Frames { start: 20, end: 70 })
        );
        let trim = edit(SampleEditKind::Trim, 0, 30);
        assert_eq!(region.after(&trim), None, "nothing is left between them");
        let shares = SampleRegion::Shares {
            start_share: 0.2,
            end_share: 0.5,
        };
        assert_eq!(shares.frames_in(1001), (200, 500));
        assert_eq!(
            region.frames_in(60),
            (30, 60),
            "a flag past the end comes back"
        );
    }
}

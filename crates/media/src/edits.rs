//! Non-destructive sample edits, rendered (WP-5.15; the multisampler spec,
//! R55 and R57): an ordered list of Trim, Cut, Reverse, Fade In, Fade Out
//! and Normalize applied to a decoded sample, off the audio thread. The
//! source is never written; the result is a new [`SampleData`] that the
//! media store shares by the source's hash and the list.

use crate::error::MediaError;
use crate::sample_data::SampleData;

/// What an edit does (the multisampler spec, R55).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EditKind {
    /// Keeps the selection and drops the rest. Needs a range.
    Trim,
    /// Removes the selection and closes the gap. Needs a range.
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

/// One edit: what it does and where. The range is in frames of the sample
/// as it is when the edit applies (after the edits before it), `from`
/// inclusive to `to` exclusive.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Edit {
    /// What it does.
    pub kind: EditKind,
    /// The selection it acts on, or `None` for the whole sample.
    pub range: Option<(u64, u64)>,
}

/// The sample's length in frames after `edit` applies to one of `length`
/// frames, or why it can't.
///
/// # Errors
///
/// [`MediaError::Malformed`] for a range that is empty or past the end, a
/// Trim or Cut with no range, or a Cut that would leave nothing.
pub fn length_after(edit: Edit, length: u64) -> Result<u64, MediaError> {
    let span = match edit.range {
        Some((from, to)) if from < to && to <= length => Some(to - from),
        Some((from, to)) => {
            return Err(MediaError::Malformed(format!(
                "the selection {from} to {to} isn't inside a sample of {length} frames"
            )));
        }
        None => None,
    };
    match (edit.kind, span) {
        (EditKind::Trim, Some(span)) => Ok(span),
        (EditKind::Cut, Some(span)) if span < length => Ok(length - span),
        (EditKind::Cut, Some(_)) => Err(MediaError::Malformed("a cut can't remove it all".into())),
        (EditKind::Trim | EditKind::Cut, None) => {
            Err(MediaError::Malformed("select what to keep or cut".into()))
        }
        _ => Ok(length),
    }
}

/// Renders `edits` over `data`, in order.
///
/// # Errors
///
/// As [`length_after`], for the first edit that doesn't fit.
pub fn apply(data: &SampleData, edits: &[Edit]) -> Result<SampleData, MediaError> {
    let rate = data.sample_rate();
    let mut channels: Vec<Vec<f32>> = data.channels().map(<[f32]>::to_vec).collect();
    for &edit in edits {
        let length = channels.first().map_or(0, Vec::len) as u64;
        length_after(edit, length)?;
        let (from, to) = edit
            .range
            .map_or((0, length as usize), |(a, b)| (a as usize, b as usize));
        match edit.kind {
            EditKind::Trim => {
                for channel in &mut channels {
                    channel.truncate(to);
                    channel.drain(..from);
                }
            }
            EditKind::Cut => {
                for channel in &mut channels {
                    channel.drain(from..to);
                }
            }
            EditKind::Reverse => {
                for channel in &mut channels {
                    channel[from..to].reverse();
                }
            }
            EditKind::FadeIn | EditKind::FadeOut => {
                let steps = (to - from).saturating_sub(1).max(1) as f32;
                for channel in &mut channels {
                    for (i, sample) in channel[from..to].iter_mut().enumerate() {
                        let rising = if to - from > 1 { i as f32 / steps } else { 1.0 };
                        *sample *= if edit.kind == EditKind::FadeIn {
                            rising
                        } else {
                            1.0 - rising
                        };
                    }
                }
            }
            EditKind::Normalize => {
                let peak = channels
                    .iter()
                    .flat_map(|channel| &channel[from..to])
                    .fold(0.0f32, |peak, sample| peak.max(sample.abs()));
                if peak > 0.0 {
                    for channel in &mut channels {
                        for sample in &mut channel[from..to] {
                            *sample /= peak;
                        }
                    }
                }
            }
        }
    }
    SampleData::new(rate, channels)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ramp(frames: usize) -> SampleData {
        // Two channels: a rising ramp and its negative.
        let left: Vec<f32> = (0..frames).map(|i| i as f32 / 10.0).collect();
        let right: Vec<f32> = left.iter().map(|s| -s).collect();
        SampleData::new(48_000, vec![left, right]).unwrap()
    }

    fn edit(kind: EditKind, range: Option<(u64, u64)>) -> Edit {
        Edit { kind, range }
    }

    #[test]
    fn trim_cut_reverse_fade_and_normalize_render_as_they_say() {
        let source = ramp(10);
        let left = |edits: &[Edit]| apply(&source, edits).unwrap().channel(0).unwrap().to_vec();
        // Trim keeps the selection; Cut closes the gap.
        assert_eq!(left(&[edit(EditKind::Trim, Some((2, 5)))]), [0.2, 0.3, 0.4]);
        assert_eq!(
            left(&[edit(EditKind::Cut, Some((2, 8)))]),
            [0.0, 0.1, 0.8, 0.9]
        );
        // Reverse turns the selection, or the whole sample with none.
        assert_eq!(
            left(&[edit(EditKind::Reverse, Some((1, 4)))])[..5],
            [0.0, 0.3, 0.2, 0.1, 0.4]
        );
        assert_eq!(left(&[edit(EditKind::Reverse, None)])[0], 0.9);
        // Fades are straight lines from silence to full and back, over the
        // selection alone.
        let faded = left(&[edit(EditKind::FadeIn, Some((4, 7)))]);
        assert_eq!(faded[3], 0.3);
        assert_eq!(faded[4], 0.0);
        assert!((faded[5] - 0.25).abs() < 1e-6);
        assert!((faded[6] - 0.6).abs() < 1e-6);
        let faded = left(&[edit(EditKind::FadeOut, None)]);
        assert!((faded[0] - 0.0).abs() < 1e-6);
        assert!((faded[9] - 0.0).abs() < 1e-6);
        assert!((faded[1] - 0.1 * 8.0 / 9.0).abs() < 1e-6);
        // Normalize scales the selection so its largest magnitude is 1,
        // channels together.
        let result = apply(&source, &[edit(EditKind::Normalize, Some((0, 5)))]).unwrap();
        assert!((result.channel(0).unwrap()[4] - 1.0).abs() < 1e-6);
        assert!((result.channel(1).unwrap()[4] + 1.0).abs() < 1e-6);
        assert_eq!(result.channel(0).unwrap()[9], 0.9);
        // The list applies in order: a range counts frames of the sample
        // as the edits before it left it.
        let both = left(&[
            edit(EditKind::Cut, Some((0, 4))),
            edit(EditKind::Trim, Some((0, 2))),
        ]);
        assert_eq!(both, [0.4, 0.5]);
    }

    #[test]
    fn an_edit_that_does_not_fit_is_refused() {
        let source = ramp(10);
        for bad in [
            edit(EditKind::Trim, None),
            edit(EditKind::Cut, Some((0, 10))),
            edit(EditKind::Reverse, Some((5, 5))),
            edit(EditKind::FadeIn, Some((8, 11))),
        ] {
            assert!(apply(&source, &[bad]).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn a_silent_selection_normalizes_to_itself() {
        let silent = SampleData::new(48_000, vec![vec![0.0; 4]]).unwrap();
        let result = apply(&silent, &[edit(EditKind::Normalize, None)]).unwrap();
        assert_eq!(result, silent);
    }

    #[test]
    fn length_after_matches_the_render() {
        let source = ramp(10);
        for e in [
            edit(EditKind::Trim, Some((3, 9))),
            edit(EditKind::Cut, Some((3, 9))),
            edit(EditKind::Reverse, None),
        ] {
            let rendered = apply(&source, &[e]).unwrap().frames() as u64;
            assert_eq!(length_after(e, 10).unwrap(), rendered);
        }
    }
}

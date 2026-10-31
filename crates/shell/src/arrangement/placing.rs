//! Placing material on a track without overlap: whatever is placed over
//! part of a clip replaces that part, so a track never plays two clips at
//! once (the arrangement editing spec).
//!
//! The Arrangement's edits (WP-1.8b: moving, copying, resizing, pasting,
//! duplicating) and the browser's drops (WP-1.10) both clear the span they
//! place into with [`clear`] or [`clear_spans`], inside their own
//! transaction, so each stays one undo step. A clip that loses its start
//! keeps playing what it played: an audio clip's Start marker moves on by
//! the time it lost, as Live's does, and a MIDI clip's offset into its
//! contents grows by the ticks it lost.

use std::collections::BTreeSet;

use bl_core::time::{TempoMap, Ticks};
use bl_model::{Clip, ClipId, Command, EditError, Editor, ModelError, TakeLaneId, TrackId};

/// A clip's part from `from` to `to`, which must lie inside it, playing what
/// it played there under `tempo` ([`Clip::part`]).
#[must_use]
pub fn piece(clip: &Clip, from: Ticks, to: Ticks, tempo: &TempoMap) -> Clip {
    clip.part(from, to, tempo)
}

/// Splits `clip` at `at`, strictly inside it: the clip keeps the part before
/// and a new clip on the same lane, which is returned, takes the part after,
/// each playing what it played before.
///
/// # Errors
///
/// [`EditError`] if there's no such clip, or the document refuses a step.
pub fn split_off(editor: &mut Editor, clip: ClipId, at: Ticks) -> Result<ClipId, EditError> {
    let doc = editor.document();
    let missing = || EditError::Command(ModelError::NoSuchClip(clip));
    let track = doc.clip_track(clip).ok_or_else(missing)?;
    // The part after the split goes on the lane the clip sits on.
    let lane = doc.clip_lane(clip);
    let original = doc.clip(clip).ok_or_else(missing)?.clone();
    let tempo = doc.settings().tempo.clone();
    let id: ClipId = editor.new_id();
    let right = Clip {
        id,
        ..piece(&original, at, original.end(), &tempo)
    };
    editor.apply(Command::SetClipLength {
        clip,
        length: at - original.start,
    })?;
    // Spec: scales R7. A split keeps the clip's scale.
    let scale = original.scale;
    editor.apply(match lane {
        Some(lane) => Command::CreateTakeClip { lane, clip: right },
        None => Command::CreateClip { track, clip: right },
    })?;
    crate::tracks::keep_scale(editor, id, scale)?;
    Ok(id)
}

/// Clears `start..end` on `track`'s main lane for something placed there:
/// clips inside it are deleted, clips crossing an edge shortened from that
/// edge, and a clip spanning it split around it, so the clips left play
/// nothing in the span. Clips in `keep` are left alone.
///
/// # Errors
///
/// [`EditError`] if the document refuses a step; the caller's transaction
/// then puts everything back.
pub fn clear(
    editor: &mut Editor,
    track: TrackId,
    start: Ticks,
    end: Ticks,
    keep: &BTreeSet<ClipId>,
) -> Result<(), EditError> {
    clear_spans(editor, track, None, &[(start, end)], keep)
}

/// Clears `start..end` on `track`'s main lane, or on its take lane `lane`,
/// as [`clear`] does.
///
/// # Errors
///
/// [`EditError`] if the document refuses a step.
pub fn clear_on(
    editor: &mut Editor,
    track: TrackId,
    lane: Option<TakeLaneId>,
    start: Ticks,
    end: Ticks,
    keep: &BTreeSet<ClipId>,
) -> Result<(), EditError> {
    clear_spans(editor, track, lane, &[(start, end)], keep)
}

/// Clears every span in `spans` on `track`'s main lane, or on its take lane
/// `lane`, as [`clear`] does each, looking through the lane's clips once
/// however many spans there are, as moving many clips at once needs. Spans may overlap and come in any order; empty
/// ones are ignored. Clips in `keep` are left alone.
///
/// # Errors
///
/// [`EditError`] if the document refuses a step; the caller's transaction
/// then puts everything back.
pub fn clear_spans(
    editor: &mut Editor,
    track: TrackId,
    lane: Option<TakeLaneId>,
    spans: &[(Ticks, Ticks)],
    keep: &BTreeSet<ClipId>,
) -> Result<(), EditError> {
    // The spans as one sorted list of disjoint spans: clearing spans that
    // overlap or touch one after another clears their union.
    let mut merged: Vec<(Ticks, Ticks)> = spans.iter().copied().filter(|(s, e)| e > s).collect();
    if merged.is_empty() {
        return Ok(());
    }
    merged.sort_unstable();
    let mut write = 0;
    for read in 1..merged.len() {
        if merged[read].0 <= merged[write].1 {
            merged[write].1 = merged[write].1.max(merged[read].1);
        } else {
            write += 1;
            merged[write] = merged[read];
        }
    }
    merged.truncate(write + 1);

    let Some(held) = editor.document().track(track) else {
        return Ok(());
    };
    let on_lane: Vec<&Clip> = match lane {
        None => held.clips().collect(),
        Some(lane) => held
            .take_lane(lane)
            .map_or_else(Vec::new, |lane| lane.clips.iter().collect()),
    };
    // Each clip a span covers part of, with the first span it meets.
    let (first_start, last_end) = (merged[0].0, merged[merged.len() - 1].1);
    let covered: Vec<(ClipId, usize)> = on_lane
        .into_iter()
        .filter(|c| c.start < last_end && c.end() > first_start)
        .filter_map(|c| {
            // The first span that ends after the clip starts.
            let first = merged.partition_point(|(_, e)| *e <= c.start);
            merged
                .get(first)
                .filter(|(s, _)| *s < c.end() && !keep.contains(&c.id))
                .map(|_| (c.id, first))
        })
        .collect();
    for (clip, first) in covered {
        // The part of the clip still to clear: a split hands the rest to a
        // new clip.
        let mut current = clip;
        for &(start, end) in &merged[first..] {
            let Some(value) = editor.document().clip(current) else {
                break;
            };
            let (from, to) = (value.start, value.end());
            if start >= to {
                break;
            }
            if from >= start && to <= end {
                editor.apply(Command::DeleteClip { clip: current })?;
                break;
            } else if from < start && to > end {
                let right = split_off(editor, current, end)?;
                editor.apply(Command::SetClipLength {
                    clip: current,
                    length: start - from,
                })?;
                current = right;
            } else if from < start {
                editor.apply(Command::SetClipLength {
                    clip: current,
                    length: start - from,
                })?;
                break;
            } else {
                editor.apply(Command::TrimClipStart {
                    clip: current,
                    start: end,
                })?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use bl_model::{ClipData, MidiData, TrackKind};

    use super::*;

    const BAR: i64 = Ticks::WHOLE.get();

    /// What a track holds, without IDs: each clip's start, end, and offset.
    fn spans(editor: &Editor, track: TrackId) -> Vec<(i64, i64, i64)> {
        let mut clips: Vec<_> = editor
            .document()
            .track(track)
            .unwrap()
            .clips()
            .map(|c| (c.start.get(), c.end().get(), c.offset.get()))
            .collect();
        clips.sort_unstable();
        clips
    }

    #[test]
    fn clearing_spans_at_once_matches_clearing_them_one_by_one() {
        let mut seed = 0x2545_f491_4f6c_dd1d_u64;
        let mut below = |n: i64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % n as u64) as i64
        };
        for round in 0..200 {
            // Clips end to end with gaps, some already trimmed: start,
            // length, offset, and whether to keep it.
            let mut clips = Vec::new();
            let mut at = below(2) * BAR;
            for _ in 0..8 {
                let length = (1 + below(4)) * BAR;
                clips.push((at, length, below(2) * BAR, below(5) == 0));
                at += length + below(2) * BAR;
            }
            let cuts: Vec<(Ticks, Ticks)> = (0..1 + below(5))
                .map(|_| {
                    let start = below(at / (BAR / 4)) * (BAR / 4);
                    (Ticks(start), Ticks(start + below(4 * BAR)))
                })
                .collect();
            let build = || {
                let mut editor = Editor::new();
                let track: TrackId = editor.new_id();
                editor
                    .apply(Command::CreateTrack {
                        id: track,
                        kind: TrackKind::Midi,
                        name: "MIDI".to_owned(),
                        parent: None,
                        index: 0,
                    })
                    .unwrap();
                let mut keep = BTreeSet::new();
                for &(start, length, offset, kept) in &clips {
                    let mut clip = Clip::new(
                        editor.new_id(),
                        Ticks(start),
                        Ticks(length),
                        // MIDI, whose parts keep an offset: an audio
                        // clip's Start marker moves instead.
                        ClipData::Midi(MidiData::default()),
                    );
                    clip.offset = Ticks(offset);
                    if kept {
                        keep.insert(clip.id);
                    }
                    editor.apply(Command::CreateClip { track, clip }).unwrap();
                }
                (editor, track, keep)
            };
            let (mut one_by_one, track, keep) = build();
            for (start, end) in &cuts {
                clear(&mut one_by_one, track, *start, *end, &keep).unwrap();
            }
            let (mut editor, _, _) = build();
            clear_spans(&mut editor, track, None, &cuts, &keep).unwrap();
            assert_eq!(
                spans(&editor, track),
                spans(&one_by_one, track),
                "round {round}: {cuts:?}"
            );
        }
    }
}

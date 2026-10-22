//! Fades and crossfades in the Arrangement's edits (WP-6.51, the audio clips
//! spec R26 to R29).
//!
//! The model keeps each fade on its clip and a crossfade on both clips it
//! joins, and refuses a fade past its limits. This module is what the
//! Arrangement's edits add: after an edit leaves two audio clips meeting,
//! with Create Fades on Clip Edges on, they get the 4 ms crossfade
//! ([`settle`]); Create Fade and Delete Fades make and delete fades from a
//! selection ([`plan`], [`delete_fades`]).

use std::collections::{BTreeMap, BTreeSet};

use bl_core::time::Ticks;
use bl_model::{
    AudioData, ChangeSet, Clip, ClipData, ClipId, Command, Document, EDGE_FADE_SECONDS, EditError,
    Editor, TrackId, crossfades,
};
use bl_shell_spec::editing::Edge;

use super::editing::{EditFailure, Target, apply, unavailable};

/// The length a fade has at a clip's edge by default: 4 ms while Create
/// Fades on Clip Edges is on, none otherwise (R7).
fn edge_seconds(edge_fades: bool) -> f64 {
    if edge_fades { EDGE_FADE_SECONDS } else { 0.0 }
}

/// A clip's audio data, if it is an audio clip.
fn audio(clip: &Clip) -> Option<&AudioData> {
    match &clip.data {
        ClipData::Audio(audio) => Some(audio),
        ClipData::Midi(_) => None,
    }
}

/// Gives the audio clips on `track` that meet at `boundary` R26's 4 ms
/// crossfade, 2 ms each side of the boundary. A later clip that starts at the
/// start of its file can't be read before its start (R25), so the crossfade
/// then runs 4 ms past the boundary instead. Returns whether it was made.
fn edge_crossfade(editor: &mut Editor, track: TrackId, boundary: Ticks) -> bool {
    [
        (EDGE_FADE_SECONDS / 2.0, EDGE_FADE_SECONDS / 2.0),
        (0.0, EDGE_FADE_SECONDS),
    ]
    .into_iter()
    .any(|(before, after)| {
        editor
            .apply(Command::SetCrossfade {
                track,
                boundary,
                before,
                after,
                curve: 0.0,
            })
            .is_ok()
    })
}

/// Sets a clip's fade at `edge` to `length` seconds, a straight line, leaving
/// its other fade as it is. A length the clip's limits refuse becomes none.
fn set_edge_fade(
    editor: &mut Editor,
    clip: ClipId,
    edge: Edge,
    length: f64,
) -> Result<(), EditError> {
    let Some(data) = editor.document().clip(clip).and_then(audio).cloned() else {
        return Ok(());
    };
    let (fade_in, fade_out) = match edge {
        Edge::Start => (length, data.fade_out),
        Edge::End => (data.fade_in, length),
    };
    let (fade_in_curve, fade_out_curve) = match edge {
        Edge::Start => (Some(0.0), None),
        Edge::End => (None, Some(0.0)),
    };
    let command = |fade_in, fade_out| Command::SetClipFades {
        clip,
        fade_in,
        fade_out,
        fade_in_curve,
        fade_out_curve,
    };
    if editor.apply(command(fade_in, fade_out)).is_err() && length > 0.0 {
        let (fade_in, fade_out) = match edge {
            Edge::Start => (0.0, data.fade_out),
            Edge::End => (data.fade_in, 0.0),
        };
        editor.apply(command(fade_in, fade_out))?;
    }
    Ok(())
}

/// After an edit that began from `before`: ends the crossfades whose clips no
/// longer meet, each edge going back to a fade of 4 ms (none with Create
/// Fades on Clip Edges off), and, with it on, gives the audio clips the edit
/// left meeting a 4 ms crossfade where they have none (R26). Part of the
/// edit's own undo step.
///
/// # Errors
///
/// [`EditError`] if the document refuses a step.
pub(crate) fn settle(
    editor: &mut Editor,
    before: &Document,
    edge_fades: bool,
) -> Result<(), EditError> {
    let changed: BTreeSet<ClipId> = ChangeSet::between(before, editor.document())
        .clips
        .into_keys()
        .collect();
    let tracks: BTreeSet<TrackId> = changed
        .iter()
        .flat_map(|id| [before.clip_track(*id), editor.document().clip_track(*id)])
        .flatten()
        .collect();
    for track in tracks {
        // An edge that was in a crossfade and isn't now.
        let was = before
            .track(track)
            .map(|t| crossfades(t.clips()))
            .unwrap_or_default();
        let now = editor
            .document()
            .track(track)
            .map(|t| crossfades(t.clips()))
            .unwrap_or_default();
        let clips: Vec<Clip> = editor
            .document()
            .track(track)
            .map(|t| t.clips().cloned().collect())
            .unwrap_or_default();
        for clip in &clips {
            let Some(data) = audio(clip) else {
                continue;
            };
            let joined = |list: &[bl_model::Crossfade], later: bool| {
                list.iter()
                    .any(|c| if later { c.later } else { c.earlier } == clip.id)
            };
            let starts = !joined(&now, true) && (joined(&was, true) || data.fade_in_before > 0.0);
            let ends = !joined(&now, false) && (joined(&was, false) || data.fade_out_after > 0.0);
            for (edge, ended) in [(Edge::Start, starts), (Edge::End, ends)] {
                if ended {
                    set_edge_fade(editor, clip.id, edge, edge_seconds(edge_fades))?;
                }
            }
        }
        if !edge_fades {
            continue;
        }
        // Clips the edit left meeting with no crossfade: look up each
        // changed clip's neighbours, not every pair on the track.
        let clips: Vec<Clip> = editor
            .document()
            .track(track)
            .map(|t| t.clips().filter(|c| c.data.is_audio()).cloned().collect())
            .unwrap_or_default();
        let starts: BTreeMap<Ticks, ClipId> = clips.iter().map(|c| (c.start, c.id)).collect();
        let ends: BTreeMap<Ticks, ClipId> = clips.iter().map(|c| (c.end(), c.id)).collect();
        let now = editor
            .document()
            .track(track)
            .map(|t| crossfades(t.clips()))
            .unwrap_or_default();
        let mut boundaries = BTreeSet::new();
        for clip in clips.iter().filter(|c| changed.contains(&c.id)) {
            // Its end meets a clip's start, and its start a clip's end.
            if starts
                .get(&clip.end())
                .is_some_and(|other| *other != clip.id)
            {
                boundaries.insert(clip.end());
            }
            if ends.get(&clip.start).is_some_and(|other| *other != clip.id) {
                boundaries.insert(clip.start);
            }
        }
        for boundary in boundaries {
            if !now.iter().any(|c| c.boundary == boundary) {
                edge_crossfade(editor, track, boundary);
            }
        }
    }
    Ok(())
}

/// What Create Fade makes from a selection (R28).
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Plan {
    /// Fade-ins and fade-outs: the clip, the edge, and the length in seconds.
    pub(crate) fades: Vec<(ClipId, Edge, f64)>,
    /// Crossfades over a span: the track, the boundary, and the seconds
    /// before and after it.
    pub(crate) crossfades: Vec<(TrackId, Ticks, f64, f64)>,
    /// Boundaries that get the 4 ms crossfade.
    pub(crate) edge_crossfades: Vec<(TrackId, Ticks)>,
}

impl Plan {
    /// Whether it makes nothing.
    pub(crate) fn is_empty(&self) -> bool {
        self.fades.is_empty() && self.crossfades.is_empty() && self.edge_crossfades.is_empty()
    }

    /// Whether it makes a crossfade, which names it (M-CREATE-010).
    pub(crate) fn has_crossfade(&self) -> bool {
        !self.crossfades.is_empty() || !self.edge_crossfades.is_empty()
    }
}

/// The audio clips of `track`, by start.
fn audio_clips(doc: &Document, track: TrackId) -> Vec<&Clip> {
    let mut clips: Vec<&Clip> = doc
        .track(track)
        .map(|t| t.clips().filter(|c| c.data.is_audio()).collect())
        .unwrap_or_default();
    clips.sort_by_key(|c| (c.start, c.id));
    clips
}

/// What Create Fade would make from `target` (R28).
pub(crate) fn plan(doc: &Document, target: &Target) -> Plan {
    let tempo = &doc.settings().tempo;
    let seconds =
        |from: Ticks, to: Ticks| tempo.ticks_to_seconds(to) - tempo.ticks_to_seconds(from);
    let mut plan = Plan::default();
    match target {
        Target::Clips(ids) => {
            let mut tracks: Vec<TrackId> =
                ids.iter().filter_map(|id| doc.clip_track(*id)).collect();
            tracks.sort_unstable();
            tracks.dedup();
            for track in tracks {
                let clips = audio_clips(doc, track);
                for earlier in clips.iter().filter(|c| ids.contains(&c.id)) {
                    let meets = clips.iter().any(|c| {
                        c.start == earlier.end() && c.id != earlier.id && ids.contains(&c.id)
                    });
                    if meets {
                        plan.edge_crossfades.push((track, earlier.end()));
                    }
                }
            }
        }
        Target::Time { tracks, start, end } => {
            for &track in tracks {
                let clips = audio_clips(doc, track);
                let mut crossfaded = BTreeSet::new();
                for earlier in &clips {
                    let boundary = earlier.end();
                    if let Some(later) = clips
                        .iter()
                        .find(|c| c.start == boundary && c.id != earlier.id)
                        && (*start..=*end).contains(&boundary)
                    {
                        plan.crossfades.push((
                            track,
                            boundary,
                            seconds(*start, boundary),
                            seconds(boundary, *end),
                        ));
                        crossfaded.extend([earlier.id, later.id]);
                    }
                }
                for clip in clips.iter().filter(|c| !crossfaded.contains(&c.id)) {
                    if *start <= clip.start && clip.start < *end && clip.end() > *end {
                        plan.fades
                            .push((clip.id, Edge::Start, seconds(clip.start, *end)));
                    } else if clip.start < *start && *start < clip.end() && clip.end() <= *end {
                        plan.fades
                            .push((clip.id, Edge::End, seconds(*start, clip.end())));
                    }
                }
            }
        }
    }
    plan
}

/// Makes what `plan` says, each fade kept within its clip's limits (R25); a
/// fade the limits leave nothing of is skipped.
///
/// # Errors
///
/// [`EditFailure`] if the document refuses a step, or the limits leave
/// nothing to make.
pub(crate) fn create(editor: &mut Editor, plan: &Plan) -> Result<(), EditFailure> {
    let mut made = false;
    for &(clip, edge, length) in &plan.fades {
        let doc = editor.document();
        let Some(limits) = doc.fade_limits(clip) else {
            continue;
        };
        let room = match edge {
            Edge::Start => limits.start.inside,
            Edge::End => limits.end.inside,
        };
        let length = length.min(room);
        if length <= 0.0 {
            continue;
        }
        let Some(data) = doc.clip(clip).and_then(audio) else {
            continue;
        };
        let (fade_in, fade_out) = match edge {
            Edge::Start => (length, data.fade_out),
            Edge::End => (data.fade_in, length),
        };
        apply(
            editor,
            Command::SetClipFades {
                clip,
                fade_in,
                fade_out,
                fade_in_curve: None,
                fade_out_curve: None,
            },
        )?;
        made = true;
    }
    for &(track, boundary, before, after) in &plan.crossfades {
        let doc = editor.document();
        let (Some(earlier), Some(later)) = (
            audio_clips(doc, track)
                .into_iter()
                .find(|c| c.end() == boundary),
            audio_clips(doc, track)
                .into_iter()
                .find(|c| c.start == boundary),
        ) else {
            continue;
        };
        let (Some(first), Some(second)) = (doc.fade_limits(earlier.id), doc.fade_limits(later.id))
        else {
            continue;
        };
        let (before, after) = (
            before.min(second.start.outside),
            after.min(first.end.outside),
        );
        if before + after <= 0.0 {
            continue;
        }
        apply(
            editor,
            Command::SetCrossfade {
                track,
                boundary,
                before,
                after,
                curve: 0.0,
            },
        )?;
        made = true;
    }
    for &(track, boundary) in &plan.edge_crossfades {
        made |= edge_crossfade(editor, track, boundary);
    }
    if made {
        Ok(())
    } else {
        Err(unavailable("the selection leaves no room for a fade"))
    }
}

/// Deletes the fades of `clips`, or only those at `edge` (R29): a fade goes
/// back to 4 ms while Create Fades on Clip Edges is on, and none otherwise;
/// a crossfade goes back to the 4 ms crossfade, or with the setting off ends
/// with its clips meeting and no fade.
///
/// # Errors
///
/// [`EditFailure`] if the document refuses a step.
pub(crate) fn delete_fades(
    editor: &mut Editor,
    clips: &[ClipId],
    edge: Option<Edge>,
    edge_fades: bool,
) -> Result<(), EditFailure> {
    let mut done: BTreeSet<(TrackId, Ticks)> = BTreeSet::new();
    for &clip in clips {
        let doc = editor.document();
        let (Some(value), Some(track)) = (doc.clip(clip), doc.clip_track(clip)) else {
            continue;
        };
        if !value.data.is_audio() {
            continue;
        }
        let joined = doc
            .track(track)
            .map(|t| crossfades(t.clips()))
            .unwrap_or_default();
        for side in [Edge::Start, Edge::End] {
            if edge.is_some_and(|only| only != side) {
                continue;
            }
            let crossfade = joined.iter().find(|c| match side {
                Edge::Start => c.later == clip,
                Edge::End => c.earlier == clip,
            });
            let Some(crossfade) = crossfade else {
                set_edge_fade(editor, clip, side, edge_seconds(edge_fades))?;
                continue;
            };
            if !done.insert((track, crossfade.boundary)) {
                continue;
            }
            apply(
                editor,
                Command::DeleteCrossfade {
                    track,
                    boundary: crossfade.boundary,
                },
            )?;
            if edge_fades && !edge_crossfade(editor, track, crossfade.boundary) {
                set_edge_fade(editor, crossfade.earlier, Edge::End, EDGE_FADE_SECONDS)?;
                set_edge_fade(editor, crossfade.later, Edge::Start, EDGE_FADE_SECONDS)?;
            }
        }
    }
    Ok(())
}

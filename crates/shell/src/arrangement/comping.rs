//! Comping: inserting and duplicating take lanes, and building a comp from
//! them (the comping spec, R4, R5 and R17 to R21). Each command is one undo
//! step, made of the document's own commands in a transaction. The matching
//! that decides which take-lane material the main lane uses is
//! [`bl_model::comp`]'s.

use std::collections::BTreeMap;

use bl_core::time::Ticks;
use bl_model::{
    Clip, Command, Document, Editor, NewTakeLane, TakeLaneId, TrackId, TrackKind, comp,
};
use bl_shell_spec::comping::{
    self as ids, CompResult, CompSourcesArgs, CompSourcesResult, CopyToMainLaneArgs, DuplicateArgs,
    InsertArgs, LanesResult, MoveCompEdgeArgs, ReplaceWithTakeArgs, TakeDirection, UsedStretch,
};
use bl_shell_spec::editing::Position;
use serde::Serialize;
use serde_json::Value;

use super::editing::{
    EditFailure, apply, invalid, place_on_main, refused, transaction, unavailable,
};
use super::{ArrangementView, parse};

fn to_value<T: Serialize>(value: &T) -> Value {
    serde_json::to_value(value).expect("results serialise")
}

/// The IDs of a command's arguments that name a take lane, to check they exist.
fn take_lane_track(doc: &Document, lane: TakeLaneId) -> Result<TrackId, EditFailure> {
    doc.take_lane_track(lane)
        .ok_or_else(|| invalid(format!("no take lane has the ID {}", lane.0)))
}

/// An audio or MIDI track, which can hold take lanes.
fn clip_track(doc: &Document, track: TrackId) -> Result<&bl_model::Track, EditFailure> {
    let held = doc
        .track(track)
        .ok_or_else(|| invalid(format!("no track has the ID {}", track.0)))?;
    if matches!(held.kind(), TrackKind::Audio | TrackKind::Midi) {
        Ok(held)
    } else {
        Err(refused(format!(
            "track {} is not an audio or MIDI track, so it has no take lanes",
            track.0
        )))
    }
}

fn span(doc: &Document, start: &Position, end: &Position) -> Result<(Ticks, Ticks), EditFailure> {
    let signatures = &doc.settings().time_signatures;
    let (start, end) = (
        start.resolve(signatures).map_err(invalid)?,
        end.resolve(signatures).map_err(invalid)?,
    );
    if end <= start {
        return Err(invalid("the range's `end` must be after its `start`"));
    }
    if start.is_negative() {
        return Err(invalid("the range starts before 1.1.1"));
    }
    Ok((start, end))
}

/// A take lane's material in `start..end`: each of its clips the range
/// reaches, cut at the range's edges as Split cuts.
fn pieces(doc: &Document, lane: TakeLaneId, start: Ticks, end: Ticks) -> Vec<Clip> {
    let tempo = &doc.settings().tempo;
    let mut clips: Vec<Clip> = doc
        .take_lane(lane)
        .map(|lane| {
            lane.clips
                .iter()
                .filter(|clip| clip.start < end && clip.end() > start)
                .map(|clip| clip.part(clip.start.max(start), clip.end().min(end), tempo))
                .collect()
        })
        .unwrap_or_default();
    clips.sort_by_key(|clip| (clip.start, clip.id));
    clips
}

/// Runs comping command `id` with `args`.
///
/// # Errors
///
/// [`EditFailure`] saying why nothing changed.
pub fn run(
    id: &str,
    args: Value,
    editor: &mut Editor,
    view: &ArrangementView,
) -> Result<Value, EditFailure> {
    let doc = editor.document().clone();
    let doc = &doc;
    match id {
        ids::INSERT => {
            let InsertArgs { tracks, after } = parse(args)?;
            let tracks = tracks.unwrap_or_else(|| view.selected_tracks().to_vec());
            if tracks.is_empty() {
                return Err(unavailable("no audio or MIDI track is selected"));
            }
            for track in &tracks {
                clip_track(doc, *track)?;
            }
            for lane in &after {
                take_lane_track(doc, *lane)?;
            }
            let (lanes, _) = transaction(editor, "Insert Take Lane", |editor| {
                let mut lanes = Vec::new();
                for track in &tracks {
                    let held = editor.document().track(*track).expect("checked");
                    // Directly after the track's selected lane, or its last.
                    let held_lanes: Vec<_> = held.take_lanes().map(|lane| lane.id).collect();
                    let index = held_lanes
                        .iter()
                        .rposition(|lane| after.contains(lane))
                        .map_or(held_lanes.len(), |at| at + 1);
                    let name = held.next_take_name();
                    let lane: TakeLaneId = editor.new_id();
                    apply(
                        editor,
                        Command::AddTakeLane {
                            track: *track,
                            lane: NewTakeLane {
                                id: Some(lane),
                                name,
                                clips: Vec::new(),
                            },
                            index: Some(index),
                        },
                    )?;
                    lanes.push(lane);
                }
                Ok(lanes)
            })?;
            Ok(to_value(&LanesResult { lanes }))
        }
        ids::DUPLICATE => {
            let DuplicateArgs { lanes } = parse(args)?;
            if lanes.is_empty() {
                return Err(invalid("`lanes` is empty"));
            }
            // Each copy goes directly after its lane: later lanes first, so
            // an earlier place doesn't move.
            let mut places = Vec::with_capacity(lanes.len());
            for lane in lanes {
                let track = take_lane_track(doc, lane)?;
                let position = doc
                    .track(track)
                    .and_then(|held| held.take_lanes().position(|held| held.id == lane))
                    .expect("the lane is on its track");
                places.push((track, position, lane));
            }
            places
                .sort_unstable_by_key(|&(track, position, _)| (track, std::cmp::Reverse(position)));
            places.dedup();
            let (copies, _) = transaction(editor, "Duplicate Take Lane(s)", |editor| {
                let mut copies = Vec::new();
                for (track, position, lane) in places {
                    let source = editor.document().take_lane(lane).expect("checked").clone();
                    let copy: TakeLaneId = editor.new_id();
                    apply(
                        editor,
                        Command::AddTakeLane {
                            track,
                            lane: NewTakeLane {
                                id: Some(copy),
                                name: source.name,
                                clips: Vec::new(),
                            },
                            index: Some(position + 1),
                        },
                    )?;
                    for clip in source.clips {
                        let clip = Clip {
                            id: editor.new_id(),
                            ..clip
                        };
                        apply(editor, Command::CreateTakeClip { lane: copy, clip })?;
                    }
                    copies.push(copy);
                }
                copies.reverse();
                Ok(copies)
            })?;
            Ok(to_value(&LanesResult { lanes: copies }))
        }
        ids::COPY_TO_MAIN_LANE => {
            let CopyToMainLaneArgs { lanes, start, end } = parse(args)?;
            let (start, end) = span(doc, &start, &end)?;
            if lanes.is_empty() {
                return Err(invalid("`lanes` is empty"));
            }
            // Of the lanes on one track, the last listed.
            let mut from: BTreeMap<TrackId, TakeLaneId> = BTreeMap::new();
            for lane in lanes {
                from.insert(take_lane_track(doc, lane)?, lane);
            }
            let (created, recorded) =
                transaction(editor, "Copy Selection to Main Lane", |editor| {
                    let mut created = Vec::new();
                    for (track, lane) in &from {
                        let copies = pieces(editor.document(), *lane, start, end);
                        created.extend(place_on_main(editor, vec![(*track, copies)])?);
                    }
                    Ok(created)
                })?;
            Ok(to_value(&CompResult {
                created,
                undo_step: recorded,
            }))
        }
        ids::REPLACE_WITH_TAKE => {
            let ReplaceWithTakeArgs {
                track,
                start,
                end,
                direction,
                lane,
            } = parse(args)?;
            let (start, end) = span(doc, &start, &end)?;
            let held = clip_track(doc, track)?;
            if let Some(lane) = lane
                && held.take_lane(lane).is_none()
            {
                return Err(invalid(format!(
                    "take lane {} isn't on track {}",
                    lane.0, track.0
                )));
            }
            let order: Vec<TakeLaneId> = held.take_lanes().map(|lane| lane.id).collect();
            // The parts of the range, each with the place it counts from: the
            // selected lane, or the lane its main-lane clip uses, or before
            // the first lane.
            let parts = replacement_parts(doc, track, start, end, lane, &order);
            let label = match direction {
                TakeDirection::Next => "Replace with Next Take",
                TakeDirection::Previous => "Replace with Previous Take",
            };
            let (created, recorded) = transaction(editor, label, |editor| {
                let mut placements = Vec::new();
                for (from, part_start, part_end) in &parts {
                    let candidates: Box<dyn Iterator<Item = &TakeLaneId>> = match direction {
                        TakeDirection::Next => {
                            Box::new(order.iter().skip(from.map_or(0, |at| at + 1)))
                        }
                        TakeDirection::Previous => {
                            Box::new(order.iter().take(from.unwrap_or(0)).rev())
                        }
                    };
                    let doc = editor.document();
                    let found = candidates
                        .into_iter()
                        .map(|lane| (*lane, pieces(doc, *lane, *part_start, *part_end)))
                        .find(|(_, clips)| !clips.is_empty());
                    if let Some((_, clips)) = found {
                        placements.push((track, clips));
                    }
                }
                place_on_main(editor, placements)
            })?;
            Ok(to_value(&CompResult {
                created,
                undo_step: recorded,
            }))
        }
        ids::MOVE_COMP_EDGE => {
            let MoveCompEdgeArgs { track, from, to } = parse(args)?;
            let signatures = &doc.settings().time_signatures;
            let (from, to) = (
                from.resolve(signatures).map_err(invalid)?,
                to.resolve(signatures).map_err(invalid)?,
            );
            let held = clip_track(doc, track)?;
            let tempo = &doc.settings().tempo;
            let sources = comp::sources(held, tempo);
            let clips: Vec<&Clip> = held.clips().collect();
            let (Some(earlier), Some(later)) = (
                clips.iter().find(|clip| clip.end() == from),
                clips.iter().find(|clip| clip.start == from),
            ) else {
                return Err(refused(format!(
                    "no two main-lane clips of track {} meet at {from}",
                    track.0
                )));
            };
            let reach = |clip: &Clip| {
                // The span of the take-lane clips the clip plays.
                sources
                    .iter()
                    .filter(|source| source.clip == clip.id)
                    .filter_map(|source| {
                        held.take_lanes()
                            .find(|lane| lane.id == source.lane)
                            .and_then(|lane| lane.clip(source.source))
                            .map(|take| (take.start, take.end()))
                    })
                    .collect::<Vec<_>>()
            };
            let (before, after) = (reach(earlier), reach(later));
            if before.is_empty() || after.is_empty() {
                return Err(refused(
                    "both clips must play a take-lane clip for their boundary to move",
                ));
            }
            if to <= earlier.start || to >= later.end() {
                return Err(refused("each clip must stay at least a tick long"));
            }
            // The growing clip goes no further than its take-lane clip does.
            if to > from && !before.iter().any(|(_, end)| *end >= to) {
                return Err(refused("the earlier clip's take doesn't reach that far"));
            }
            if to < from && !after.iter().any(|(start, _)| *start <= to) {
                return Err(refused("the later clip's take doesn't reach that far"));
            }
            let (earlier, later) = (earlier.id, later.id);
            let (_, recorded) = transaction(editor, "Move Comp Edge", |editor| {
                // The shrinking clip first, so the two never overlap.
                let shorten = Command::SetClipLength {
                    clip: earlier,
                    length: to - held.clip(earlier).expect("exists").start,
                };
                let trim = Command::TrimClipStart {
                    clip: later,
                    start: to,
                };
                if to > from {
                    apply(editor, trim)?;
                    apply(editor, shorten)
                } else {
                    apply(editor, shorten)?;
                    apply(editor, trim)
                }
            })?;
            Ok(to_value(&CompResult {
                created: Vec::new(),
                undo_step: recorded,
            }))
        }
        ids::COMP_SOURCES => {
            let CompSourcesArgs { track } = parse(args)?;
            let held = clip_track(doc, track)?;
            let tempo = &doc.settings().tempo;
            let stretches = comp::sources(held, tempo)
                .into_iter()
                .map(|source| UsedStretch {
                    clip: source.clip,
                    lane: source.lane,
                    source: source.source,
                    start: source.start.get(),
                    end: source.end.get(),
                })
                .collect();
            Ok(to_value(&CompSourcesResult {
                stretches,
                unused_lanes: comp::unused_lanes(held, tempo),
            }))
        }
        other => Err(invalid(format!("`{other}` isn't a comping command"))),
    }
}

/// The parts of `start..end` that `take_lane.replace_with_take` replaces,
/// each with the place among `order` it counts from: `Some(index)` of a lane,
/// or `None` for before the first.
fn replacement_parts(
    doc: &Document,
    track: TrackId,
    start: Ticks,
    end: Ticks,
    lane: Option<TakeLaneId>,
    order: &[TakeLaneId],
) -> Vec<(Option<usize>, Ticks, Ticks)> {
    let index_of = |lane: TakeLaneId| order.iter().position(|held| *held == lane);
    if let Some(lane) = lane {
        return vec![(index_of(lane), start, end)];
    }
    let Some(held) = doc.track(track) else {
        return Vec::new();
    };
    let sources = comp::sources(held, &doc.settings().tempo);
    let mut mains: Vec<&Clip> = held
        .clips()
        .filter(|clip| clip.start < end && clip.end() > start)
        .collect();
    mains.sort_by_key(|clip| clip.start);
    let mut parts = Vec::new();
    let mut at = start;
    for clip in mains {
        let (from, to) = (clip.start.max(start), clip.end().min(end));
        if from > at {
            parts.push((None, at, from));
        }
        // The first lane the clip uses over its part.
        let used = sources
            .iter()
            .filter(|source| source.clip == clip.id && source.start < to && source.end > from)
            .filter_map(|source| index_of(source.lane))
            .min();
        parts.push((used, from, to));
        at = to;
    }
    if at < end {
        parts.push((None, at, end));
    }
    parts
}

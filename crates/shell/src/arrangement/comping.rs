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
use bl_shell_spec::arrangement::Lane;
use bl_shell_spec::comping::{
    self as ids, AuditionArgs, CompResult, CompSourcesArgs, CompSourcesResult, CopyToMainLaneArgs,
    DuplicateArgs, InsertArgs, LanesResult, MoveCompEdgeArgs, ReplaceWithTakeArgs, TakeDirection,
    UsedStretch,
};
use bl_shell_spec::editing::Position;
use serde::Serialize;
use serde_json::Value;

use super::editing::linked;
use super::editing::{
    EditFailure, apply, invalid, place_on_main, refused, settled, transaction, unavailable,
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

/// `tracks` and the tracks linked with them, each once (the linked-track
/// editing spec, R11).
fn with_group(doc: &Document, tracks: Vec<TrackId>) -> Vec<TrackId> {
    let mut all = tracks.clone();
    for other in tracks.iter().flat_map(|track| doc.linked_with(*track)) {
        if !all.contains(&other) {
            all.push(other);
        }
    }
    all
}

/// `lanes` and the take lanes at the same place on the tracks linked with
/// theirs (R11).
fn with_group_lanes(doc: &Document, lanes: Vec<TakeLaneId>) -> Vec<TakeLaneId> {
    let mut all = lanes.clone();
    for lane in lanes {
        let Some(track) = doc.take_lane_track(lane) else {
            continue;
        };
        for other in doc.linked_with(track) {
            if let Some(Some(same)) = doc.matching_lane(track, Some(lane), other)
                && !all.contains(&same)
            {
                all.push(same);
            }
        }
    }
    all
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

/// The range a command's `start` and `end` give, or the selection's: the
/// time selection, which selecting clips sets to their span.
fn range(
    doc: &Document,
    view: &ArrangementView,
    start: Option<Position>,
    end: Option<Position>,
) -> Result<(Ticks, Ticks), EditFailure> {
    match (start, end) {
        (Some(start), Some(end)) => span(doc, &start, &end),
        (None, None) => view
            .time_selection()
            .ok_or_else(|| unavailable("no time or clip is selected")),
        _ => Err(invalid("give `start` and `end`, or neither")),
    }
}

/// The take lanes a command acts on: those given, or the ones the selection
/// is on (R15, R17).
fn lanes_or_selection(
    view: &ArrangementView,
    lanes: Option<Vec<TakeLaneId>>,
) -> Result<Vec<TakeLaneId>, EditFailure> {
    match lanes {
        Some(lanes) if lanes.is_empty() => Err(invalid("`lanes` is empty")),
        Some(lanes) => Ok(lanes),
        None => {
            let lanes = view.selection_take_lanes();
            if lanes.is_empty() {
                Err(unavailable("no take lane is selected"))
            } else {
                Ok(lanes)
            }
        }
    }
}

/// Of take lanes on one track, the last listed (R15, R17), by track.
fn last_per_track(
    doc: &Document,
    lanes: impl IntoIterator<Item = TakeLaneId>,
) -> Result<BTreeMap<TrackId, TakeLaneId>, EditFailure> {
    let mut last = BTreeMap::new();
    for lane in lanes {
        last.insert(take_lane_track(doc, lane)?, lane);
    }
    Ok(last)
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

/// Runs comping command `id` with `args`. With `edge_fades` (Create Fades on
/// Clip Edges), the commands that change the main lane give the clips they
/// leave meeting the 4 ms crossfade (the comping spec, R22).
///
/// # Errors
///
/// [`EditFailure`] saying why nothing changed.
pub fn run(
    id: &str,
    args: Value,
    editor: &mut Editor,
    view: &mut ArrangementView,
    edge_fades: bool,
) -> Result<Value, EditFailure> {
    let doc = editor.document().clone();
    let doc = &doc;
    match id {
        ids::INSERT => {
            let InsertArgs {
                tracks,
                mut after,
                linked,
            } = parse(args)?;
            let linked = linked::on(linked);
            // Spec: comping R4. Directly after the take lane selected on the
            // track, when no track or lane is named.
            if tracks.is_none() && after.is_empty() {
                after = view.selection_take_lanes();
            }
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
            let (tracks, after) = if linked {
                (with_group(doc, tracks), with_group_lanes(doc, after))
            } else {
                (tracks, after)
            };
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
            // Spec: comping R24. Inserting shows the track's take lanes, and
            // leaves Automation Mode to show them.
            for track in &tracks {
                view.show_take_lanes_of(*track);
            }
            Ok(to_value(&LanesResult { lanes }))
        }
        ids::DUPLICATE => {
            let DuplicateArgs { lanes } = parse(args)?;
            let lanes = match lanes {
                Some(lanes) if lanes.is_empty() => return Err(invalid("`lanes` is empty")),
                Some(lanes) => lanes,
                None if view.selected_take_lanes().is_empty() => {
                    return Err(unavailable("no take lane header is selected"));
                }
                None => view.selected_take_lanes().to_vec(),
            };
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
            // Spec: comping R5. The copies are selected.
            view.select_take_lane_headers(editor.document(), copies.clone());
            Ok(to_value(&LanesResult { lanes: copies }))
        }
        ids::AUDITION => {
            let AuditionArgs { lanes, on, linked } = parse(args)?;
            let lanes = lanes_or_selection(view, lanes)?;
            let lanes = if linked::on(linked) {
                with_group_lanes(doc, lanes)
            } else {
                lanes
            };
            // Spec: comping R15. One lane for each track, the last listed.
            let picked = last_per_track(doc, lanes)?;
            for track in picked.keys() {
                if doc.track(*track).is_some_and(|held| held.is_frozen()) {
                    return Err(refused(format!(
                        "the track {} is frozen; unfreeze it to audition a take lane",
                        track.0
                    )));
                }
            }
            let on = on.unwrap_or_else(|| {
                !picked.iter().all(|(track, lane)| {
                    doc.track(*track)
                        .is_some_and(|held| held.audition() == Some(*lane))
                })
            });
            let (_, _) = transaction(editor, "Audition Take Lane", |editor| {
                for lane in picked.values() {
                    apply(editor, Command::SetAudition { lane: *lane, on })?;
                }
                Ok(())
            })?;
            Ok(to_value(&LanesResult {
                lanes: picked.into_values().collect(),
            }))
        }
        ids::COPY_TO_MAIN_LANE => {
            let CopyToMainLaneArgs {
                lanes,
                start,
                end,
                linked,
            } = parse(args)?;
            let (start, end) = range(doc, view, start, end)?;
            let lanes = lanes_or_selection(view, lanes)?;
            let lanes = if linked::on(linked) {
                with_group_lanes(doc, lanes)
            } else {
                lanes
            };
            // Of the lanes on one track, the last listed.
            let from = last_per_track(doc, lanes)?;
            let (created, recorded) = settled(
                editor,
                "Copy Selection to Main Lane",
                (doc, edge_fades),
                |editor| {
                    let mut created = Vec::new();
                    for (track, lane) in &from {
                        let copies = pieces(editor.document(), *lane, start, end);
                        created.extend(place_on_main(editor, vec![(*track, copies)])?);
                    }
                    Ok(created)
                },
            )?;
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
                linked,
            } = parse(args)?;
            let selecting = track.is_none() && lane.is_none();
            let (start, end) = range(doc, view, start, end)?;
            // The tracks to change, each with the take lane the selection is
            // on there: the last selected of them (R15), or none for the main
            // lane.
            let targets: Vec<(TrackId, Option<TakeLaneId>)> = match (track, lane) {
                (Some(track), lane) => vec![(track, lane)],
                (None, Some(lane)) => vec![(take_lane_track(doc, lane)?, Some(lane))],
                (None, None) => {
                    let on = view.selection_take_lanes();
                    view.selected_tracks()
                        .iter()
                        .filter(|track| clip_track(doc, **track).is_ok())
                        .map(|track| {
                            let lane = on
                                .iter()
                                .rev()
                                .find(|lane| doc.take_lane_track(**lane) == Some(*track))
                                .copied();
                            (*track, lane)
                        })
                        .collect()
                }
            };
            if targets.is_empty() {
                return Err(unavailable("no audio or MIDI track is selected"));
            }
            // Spec: linked-track editing R12. The same range on the lane at
            // the same place of each linked track.
            let mut targets = targets;
            if linked::on(linked) {
                for (track, lane) in targets.clone() {
                    for other in doc.linked_with(track) {
                        if let Some(same) = doc.matching_lane(track, lane, other)
                            && !targets.iter().any(|(t, _)| *t == other)
                        {
                            targets.push((other, same));
                        }
                    }
                }
            }
            for (track, lane) in &targets {
                let held = clip_track(doc, *track)?;
                if let Some(lane) = lane
                    && held.take_lane(*lane).is_none()
                {
                    return Err(invalid(format!(
                        "take lane {} isn't on track {}",
                        lane.0, track.0
                    )));
                }
                if selecting && held.is_frozen() {
                    return Err(refused(format!(
                        "the track {} is frozen; unfreeze it to change its main lane",
                        track.0
                    )));
                }
            }
            let label = match direction {
                TakeDirection::Next => "Replace with Next Take",
                TakeDirection::Previous => "Replace with Previous Take",
            };
            let (outcome, recorded) = settled(editor, label, (doc, edge_fades), |editor| {
                let mut placements = Vec::new();
                // The lane each target's selection takes its part from.
                let mut taken: Vec<(TrackId, Option<TakeLaneId>)> = Vec::new();
                for (track, lane) in &targets {
                    let held = editor.document().track(*track).expect("checked");
                    let order: Vec<TakeLaneId> = held.take_lanes().map(|lane| lane.id).collect();
                    // The parts of the range, each with the place it counts
                    // from: the selected lane, or the lane its main-lane clip
                    // uses, or before the first lane.
                    let parts =
                        replacement_parts(editor.document(), *track, start, end, *lane, &order);
                    let mut now = *lane;
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
                        if let Some((found, clips)) = found {
                            placements.push((*track, clips));
                            // Spec: comping R19. On a take lane the selection
                            // moves to the lane taken from.
                            if lane.is_some() {
                                now = Some(found);
                            }
                        }
                    }
                    taken.push((*track, now));
                }
                Ok((place_on_main(editor, placements)?, taken))
            })?;
            let (created, taken) = outcome;
            if selecting && taken.iter().any(|(_, lane)| lane.is_some()) {
                let lanes = taken
                    .iter()
                    .map(|(track, lane)| match lane {
                        Some(lane) => Lane::Take {
                            track: *track,
                            lane: *lane,
                        },
                        None => Lane::Track { track: *track },
                    })
                    .collect();
                view.select_span_on(lanes, start, end);
            }
            Ok(to_value(&CompResult {
                created,
                undo_step: recorded,
            }))
        }
        ids::MOVE_COMP_EDGE => {
            let MoveCompEdgeArgs {
                track,
                from,
                to,
                linked,
            } = parse(args)?;
            let signatures = &doc.settings().time_signatures;
            let (from, to) = (
                from.resolve(signatures).map_err(invalid)?,
                to.resolve(signatures).map_err(invalid)?,
            );
            let mut edges = comp_edge(doc, track, from, to)?;
            // Spec: linked-track editing R12. The boundary at the same place
            // of each linked track that has one.
            if linked::on(linked) {
                for other in doc.linked_with(track).into_iter().filter(|t| *t != track) {
                    edges.extend(comp_edge(doc, other, from, to).unwrap_or_default());
                }
            }
            let (_, recorded) = settled(editor, "Move Comp Edge", (doc, edge_fades), |editor| {
                edges
                    .into_iter()
                    .try_for_each(|command| apply(editor, command).map(drop))
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

/// The commands that move the boundary between two adjacent main-lane clips
/// of `track` from `from` to `to`, the shrinking clip's first so the two
/// never overlap.
fn comp_edge(
    doc: &Document,
    track: TrackId,
    from: Ticks,
    to: Ticks,
) -> Result<Vec<Command>, EditFailure> {
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
    let shorten = Command::SetClipLength {
        clip: earlier.id,
        length: to - earlier.start,
    };
    let trim = Command::TrimClipStart {
        clip: later.id,
        start: to,
    };
    Ok(if to > from {
        vec![trim, shorten]
    } else {
        vec![shorten, trim]
    })
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

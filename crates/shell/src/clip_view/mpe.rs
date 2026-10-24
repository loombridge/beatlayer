//! The MPE tab's commands (WP-6.18; the MPE spec, R26 to R32): adding,
//! moving, deleting, bending, scaling, drawing and clearing the Pitch, Slide
//! and Pressure breakpoints of a clip's notes, and selecting them.
//!
//! Each edit is one transaction of [`Command::SetClipData`], so it is one
//! undo step, and a drag runs the same command on each frame from where it
//! began (see `Shell::gesture`). A breakpoint is named by its note's place in
//! the clip's notes in order and its position counted from the note's start.

use std::collections::{BTreeMap, BTreeSet};

use bl_core::time::Ticks;
use bl_model::note_expression;
use bl_model::{Document, Editor, Expression, MpeLane, Note, envelope_value};
use bl_shell_spec::notes::{
    self as ids, AddExpressionBreakpointArgs, BreakpointRef, ClearExpressionsArgs,
    DeleteExpressionBreakpointsArgs, DrawExpressionArgs, MoveExpressionBreakpointsArgs,
    NoteEditResult, NoteSelectMode, ScaleExpressionArgs, SelectExpressionBreakpointsArgs,
    SetExpressionCurveArgs,
};
use serde_json::Value;

use super::{
    ClipView, clip_and_data, invalid, label, notes_in_order, parse, places_or_selected, refused,
    set_notes, to_value, unavailable,
};
use crate::arrangement::ArrangementView;
use crate::arrangement::editing::{EditFailure, transaction};

/// Whether `id` is one of the commands here.
#[must_use]
pub(super) fn handles(id: &str) -> bool {
    matches!(
        id,
        ids::ADD_EXPRESSION_BREAKPOINT
            | ids::MOVE_EXPRESSION_BREAKPOINTS
            | ids::DELETE_EXPRESSION_BREAKPOINTS
            | ids::SET_EXPRESSION_CURVE
            | ids::SCALE_EXPRESSION
            | ids::DRAW_EXPRESSION
            | ids::CLEAR_EXPRESSIONS
            | ids::SELECT_EXPRESSION_BREAKPOINTS
            | ids::MPE_ZOOM
    )
}

/// Runs one of the commands.
pub(super) fn run(
    id: &str,
    args: Value,
    doc: &Document,
    editor: &mut Editor,
    view: &mut ClipView,
    arrangement: &ArrangementView,
) -> Result<Value, EditFailure> {
    match id {
        ids::ADD_EXPRESSION_BREAKPOINT => {
            let AddExpressionBreakpointArgs {
                clip,
                notes,
                envelope,
                at,
                value,
                bend,
            } = parse(args)?;
            let (clip, midi) = clip_and_data(doc, view, arrangement, clip)?;
            let ordered = notes_in_order(&midi);
            let places = places_or_selected(view, clip, notes, ordered.len())?;
            let (low, high) = envelope.range();
            if !value.is_finite() || value < low || value > high {
                return Err(invalid(format!(
                    "`value` must be from {low} to {high} for {envelope:?}"
                )));
            }
            if bend.is_some_and(|bend| !(-1.0..=1.0).contains(&bend)) {
                return Err(invalid("`bend` must be from -1 to 1"));
            }
            if let Some(place) = places
                .iter()
                .find(|place| at < Ticks::ZERO || at > ordered[**place as usize].length)
            {
                return Err(invalid(format!(
                    "`at` must be from 0 to the note's length ({}) for the note at place {place}",
                    ordered[*place as usize].length
                )));
            }
            edit(
                doc,
                editor,
                view,
                clip,
                &midi,
                ordered,
                &places,
                id,
                |notes| {
                    for place in &places {
                        let points = notes[*place as usize].envelope_mut(envelope);
                        note_expression::put(points, at, value, bend);
                    }
                    Ok(())
                },
            )
        }
        ids::MOVE_EXPRESSION_BREAKPOINTS => {
            let MoveExpressionBreakpointsArgs {
                clip,
                envelope,
                breakpoints,
                time,
                value,
                fill,
            } = parse(args)?;
            if !value.is_finite() {
                return Err(invalid("`value` must be a number"));
            }
            let (clip, midi) = clip_and_data(doc, view, arrangement, clip)?;
            let ordered = notes_in_order(&midi);
            let named = named_breakpoints(view, clip, envelope, breakpoints, &ordered, true)?;
            let notes: Vec<u32> = named.keys().copied().collect();
            let mut moved_refs: BTreeSet<BreakpointRef> = BTreeSet::new();
            let (low, high) = envelope.range();
            let value = held_value(&ordered, &named, fill, envelope, value);
            let result = edit(
                doc,
                editor,
                view,
                clip,
                &midi,
                ordered.clone(),
                &notes,
                id,
                |edited| {
                    for (place, ats) in &named {
                        let note = &mut edited[*place as usize];
                        let length = note.length;
                        let moved = match fill {
                            Some(range) => {
                                let (from, to) = fill_window(note, range);
                                let points = note.envelope_mut(envelope);
                                // Breakpoints at the stretch's edges, holding what
                                // the envelope has there, so it moves whole.
                                for edge in [from, to] {
                                    if let Some(held) = envelope_value(points, edge.0 as f64) {
                                        note_expression::put(points, edge, held, None);
                                    }
                                }
                                points
                                    .iter()
                                    .filter(|p| p.at >= from && p.at <= to)
                                    .map(|p| p.at)
                                    .collect()
                            }
                            None => ats.clone(),
                        };
                        let dt = clamp_time(&moved, time, length);
                        let points = note.envelope_mut(envelope);
                        note_expression::shift(points, &moved, dt, value);
                        for point in points.iter_mut() {
                            point.value = point.value.clamp(low, high);
                        }
                        moved_refs.extend(moved.iter().map(|at| BreakpointRef {
                            note: *place,
                            at: *at + dt,
                        }));
                    }
                    Ok(())
                },
            )?;
            // The moved breakpoints stay selected, where they landed.
            view.select_breakpoints(clip, envelope, moved_refs);
            Ok(result)
        }
        ids::DELETE_EXPRESSION_BREAKPOINTS => {
            let DeleteExpressionBreakpointsArgs {
                clip,
                envelope,
                breakpoints,
            } = parse(args)?;
            let (clip, midi) = clip_and_data(doc, view, arrangement, clip)?;
            let ordered = notes_in_order(&midi);
            let named = named_breakpoints(view, clip, envelope, breakpoints, &ordered, true)?;
            let notes: Vec<u32> = named.keys().copied().collect();
            let result = edit(
                doc,
                editor,
                view,
                clip,
                &midi,
                ordered,
                &notes,
                id,
                |edited| {
                    for (place, ats) in &named {
                        edited[*place as usize]
                            .envelope_mut(envelope)
                            .retain(|point| !ats.contains(&point.at));
                    }
                    Ok(())
                },
            )?;
            view.select_breakpoints(clip, envelope, BTreeSet::new());
            Ok(result)
        }
        ids::SET_EXPRESSION_CURVE => {
            let SetExpressionCurveArgs {
                clip,
                note,
                envelope,
                at,
                bend,
            } = parse(args)?;
            if !(-1.0..=1.0).contains(&bend) {
                return Err(invalid("`bend` must be from -1 to 1"));
            }
            let (clip, midi) = clip_and_data(doc, view, arrangement, clip)?;
            let ordered = notes_in_order(&midi);
            let places = places_or_selected(view, clip, Some(vec![note]), ordered.len())?;
            if !ordered[note as usize]
                .envelope(envelope)
                .iter()
                .any(|point| point.at == at)
            {
                return Err(invalid(format!(
                    "the note at place {note} has no {envelope:?} breakpoint at {}",
                    at.0
                )));
            }
            edit(
                doc,
                editor,
                view,
                clip,
                &midi,
                ordered,
                &places,
                id,
                |edited| {
                    for point in edited[note as usize]
                        .envelope_mut(envelope)
                        .iter_mut()
                        .filter(|point| point.at == at)
                    {
                        point.bend = bend;
                    }
                    Ok(())
                },
            )
        }
        ids::SCALE_EXPRESSION => {
            let ScaleExpressionArgs {
                clip,
                notes,
                lane,
                factor,
            } = parse(args)?;
            if !factor.is_finite() || factor < 0.0 {
                return Err(invalid("`factor` must be 0 or more"));
            }
            let envelope = match lane {
                MpeLane::PitchBend => return Err(invalid("Pitch doesn't scale")),
                MpeLane::Slide => Some(Expression::Slide),
                MpeLane::Pressure => Some(Expression::Pressure),
                MpeLane::Velocity | MpeLane::ReleaseVelocity => None,
            };
            let (clip, midi) = clip_and_data(doc, view, arrangement, clip)?;
            let ordered = notes_in_order(&midi);
            let places = places_or_selected(view, clip, notes, ordered.len())?;
            edit(
                doc,
                editor,
                view,
                clip,
                &midi,
                ordered,
                &places,
                id,
                |edited| {
                    for place in &places {
                        let note = &mut edited[*place as usize];
                        match (envelope, lane) {
                            (Some(envelope), _) => {
                                note_expression::scale(
                                    note.envelope_mut(envelope),
                                    factor,
                                    envelope,
                                );
                            }
                            (None, MpeLane::Velocity) => {
                                note.velocity = scaled(note.velocity, factor)
                            }
                            (None, _) => {
                                note.release_velocity = scaled(note.release_velocity, factor);
                            }
                        }
                    }
                    Ok(())
                },
            )
        }
        ids::DRAW_EXPRESSION => {
            let DrawExpressionArgs {
                clip,
                notes,
                envelope,
                steps,
                absolute,
            } = parse(args)?;
            if steps.is_empty() {
                return Err(invalid("`steps` is empty"));
            }
            if steps
                .iter()
                .any(|step| !step.value.is_finite() || step.length < Ticks(1))
            {
                return Err(invalid(
                    "each step needs a number for `value` and a `length` of 1 tick or more",
                ));
            }
            if steps
                .iter()
                .any(|step| step.at.0.checked_add(step.length.0).is_none())
            {
                return Err(invalid("a step ends past the last tick"));
            }
            if steps
                .windows(2)
                .any(|pair| pair[1].at < pair[0].at + pair[0].length)
            {
                return Err(invalid("`steps` must be in order of `at` and not overlap"));
            }
            let (clip, midi) = clip_and_data(doc, view, arrangement, clip)?;
            let ordered = notes_in_order(&midi);
            let places = places_or_selected(view, clip, notes, ordered.len())?;
            edit(
                doc,
                editor,
                view,
                clip,
                &midi,
                ordered,
                &places,
                id,
                |edited| {
                    for place in &places {
                        let note = &mut edited[*place as usize];
                        let (key, start, length) = (f64::from(note.key), note.start, note.length);
                        let drawn: Vec<(Ticks, Ticks, f64)> = steps
                            .iter()
                            .filter_map(|step| {
                                let from =
                                    Ticks(step.at.0.saturating_sub(start.0)).max(Ticks::ZERO);
                                let to = Ticks((step.at + step.length).0.saturating_sub(start.0))
                                    .min(length);
                                let value = if absolute && envelope == Expression::Pitch {
                                    step.value - key
                                } else {
                                    step.value
                                };
                                (from < to).then_some((from, to, value))
                            })
                            .collect();
                        note_expression::draw(note.envelope_mut(envelope), &drawn, envelope);
                    }
                    Ok(())
                },
            )
        }
        ids::CLEAR_EXPRESSIONS => {
            let ClearExpressionsArgs { clip, notes } = parse(args)?;
            let (clip, midi) = clip_and_data(doc, view, arrangement, clip)?;
            let ordered = notes_in_order(&midi);
            let places = places_or_selected(view, clip, notes, ordered.len())?;
            let result = edit(
                doc,
                editor,
                view,
                clip,
                &midi,
                ordered,
                &places,
                id,
                |edited| {
                    for place in &places {
                        for envelope in Expression::ALL {
                            edited[*place as usize].envelope_mut(envelope).clear();
                        }
                    }
                    Ok(())
                },
            )?;
            view.select_breakpoints(clip, view_envelope(view, clip), BTreeSet::new());
            Ok(result)
        }
        ids::SELECT_EXPRESSION_BREAKPOINTS => {
            let SelectExpressionBreakpointsArgs {
                clip,
                envelope,
                breakpoints,
                mode,
                all,
            } = parse(args)?;
            if all && breakpoints.is_some() {
                return Err(invalid("give `breakpoints` or `all`, not both"));
            }
            let (clip, midi) = clip_and_data(doc, view, arrangement, clip)?;
            let ordered = notes_in_order(&midi);
            let held = view.selected_breakpoints(clip);
            let envelope = envelope
                .or(held.as_ref().map(|(envelope, _)| *envelope))
                .ok_or_else(|| unavailable("no envelope has the keyboard: give `envelope`"))?;
            let mut chosen: BTreeSet<BreakpointRef> = if all {
                view.selection_of(clip)
                    .iter()
                    .filter_map(|place| Some((*place, ordered.get(*place as usize)?)))
                    .flat_map(|(place, note)| {
                        note.envelope(envelope)
                            .iter()
                            .map(move |point| BreakpointRef {
                                note: place,
                                at: point.at,
                            })
                    })
                    .collect()
            } else {
                existing(&ordered, envelope, breakpoints.unwrap_or_default())?
                    .into_iter()
                    .collect()
            };
            if mode == NoteSelectMode::Toggle && !all {
                let now: BTreeSet<BreakpointRef> = held
                    .filter(|(held, _)| *held == envelope)
                    .map(|(_, refs)| refs.into_iter().collect())
                    .unwrap_or_default();
                chosen = now.symmetric_difference(&chosen).copied().collect();
            }
            view.select_breakpoints(clip, envelope, chosen);
            Ok(to_value(&view.state(doc, arrangement)))
        }
        ids::MPE_ZOOM => {
            let (clip, midi) = clip_and_data(doc, view, arrangement, None)?;
            let ordered = notes_in_order(&midi);
            let window = view.time_selection(doc, arrangement);
            let selected = view.selection_of(clip);
            let reached: Vec<(f64, f64)> = ordered
                .iter()
                .enumerate()
                .filter(|(place, _)| selected.is_empty() || selected.contains(&(*place as u32)))
                .filter_map(|(_, note)| pitch_rows(note, window))
                .collect();
            let low = reached
                .iter()
                .map(|rows| rows.0)
                .fold(f64::INFINITY, f64::min);
            let high = reached
                .iter()
                .map(|rows| rows.1)
                .fold(f64::NEG_INFINITY, f64::max);
            if !low.is_finite() {
                return Err(unavailable("no note lies in the time selection"));
            }
            view.zoom_mpe(clip, low, high);
            Ok(to_value(&view.state(doc, arrangement)))
        }
        other => Err(invalid(format!("`{other}` isn't an MPE tab command"))),
    }
}

/// The envelope that has the keyboard in `clip`, or Pitch.
fn view_envelope(view: &ClipView, clip: bl_model::ClipId) -> Expression {
    view.selected_breakpoints(clip)
        .map_or(Expression::Pitch, |(envelope, _)| envelope)
}

/// A velocity multiplied by `factor`, held from 1 to 127.
fn scaled(velocity: u8, factor: f64) -> u8 {
    (f64::from(velocity) * factor).round().clamp(1.0, 127.0) as u8
}

/// The key rows a note's Pitch envelope reaches inside `window` (or the whole
/// note), as semitones above key 0: the note's own row when it bends nowhere
/// there. `None` when the note lies outside the window.
fn pitch_rows(note: &Note, window: Option<(Ticks, Ticks)>) -> Option<(f64, f64)> {
    let (from, to) = match window {
        Some((from, to)) => (
            (from - note.start).max(Ticks::ZERO),
            (to - note.start).min(note.length),
        ),
        None => (Ticks::ZERO, note.length),
    };
    if from > to {
        return None;
    }
    let key = f64::from(note.key);
    let mut values: Vec<f64> = note
        .pitch
        .iter()
        .filter(|point| point.at > from && point.at < to)
        .map(|point| point.value)
        .collect();
    values.extend(
        [from, to]
            .into_iter()
            .filter_map(|at| envelope_value(&note.pitch, at.0 as f64)),
    );
    values.push(0.0);
    let low = values.iter().copied().fold(f64::INFINITY, f64::min);
    let high = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    Some((key + low, key + high))
}

/// The breakpoints named, or else those selected in the tab, as each note's
/// positions; each must exist.
fn named_breakpoints(
    view: &ClipView,
    clip: bl_model::ClipId,
    envelope: Expression,
    given: Option<Vec<BreakpointRef>>,
    ordered: &[Note],
    needed: bool,
) -> Result<BTreeMap<u32, Vec<Ticks>>, EditFailure> {
    let refs = match given {
        Some(refs) => refs,
        None => view
            .selected_breakpoints(clip)
            .filter(|(held, _)| *held == envelope)
            .map(|(_, refs)| refs)
            .unwrap_or_default(),
    };
    if refs.is_empty() && needed {
        return Err(unavailable("no breakpoints are named or selected"));
    }
    let mut named: BTreeMap<u32, Vec<Ticks>> = BTreeMap::new();
    for point in existing(ordered, envelope, refs)? {
        let ats = named.entry(point.note).or_default();
        if !ats.contains(&point.at) {
            ats.push(point.at);
        }
    }
    Ok(named)
}

/// `refs`, each checked to name a breakpoint of `envelope` that exists.
fn existing(
    ordered: &[Note],
    envelope: Expression,
    refs: Vec<BreakpointRef>,
) -> Result<Vec<BreakpointRef>, EditFailure> {
    for point in &refs {
        let note = ordered
            .get(point.note as usize)
            .ok_or_else(|| invalid(format!("the clip has no note at place {}", point.note)))?;
        if !note.envelope(envelope).iter().any(|p| p.at == point.at) {
            return Err(invalid(format!(
                "the note at place {} has no {envelope:?} breakpoint at {}",
                point.note, point.at.0
            )));
        }
    }
    Ok(refs)
}

/// The stretch `fill` covers in a note, counted from its start and held
/// inside it.
fn fill_window(note: &Note, fill: bl_shell_spec::notes::TickRange) -> (Ticks, Ticks) {
    let (a, b) = (fill.start.min(fill.end), fill.start.max(fill.end));
    (
        Ticks(a.0.saturating_sub(note.start.0)).clamp(Ticks::ZERO, note.length),
        Ticks(b.0.saturating_sub(note.start.0)).clamp(Ticks::ZERO, note.length),
    )
}

/// `value`, held so that every breakpoint that moves stays in the
/// envelope's range (R26). The time is held for each note when it is
/// applied, since it depends on the note's length.
fn held_value(
    ordered: &[Note],
    named: &BTreeMap<u32, Vec<Ticks>>,
    fill: Option<bl_shell_spec::notes::TickRange>,
    envelope: Expression,
    value: f64,
) -> f64 {
    let (low, high) = envelope.range();
    let mut least = f64::INFINITY;
    let mut most = f64::NEG_INFINITY;
    for (place, ats) in named {
        let note = &ordered[*place as usize];
        let window = fill.map(|range| fill_window(note, range));
        for point in note.envelope(envelope) {
            let moves = match window {
                Some((from, to)) => point.at >= from && point.at <= to,
                None => ats.contains(&point.at),
            };
            if moves {
                least = least.min(point.value);
                most = most.max(point.value);
            }
        }
    }
    if !least.is_finite() {
        return value;
    }
    value.clamp(low - least, high - most)
}

/// `time`, held so that the breakpoints at `moved` stay from 0 to `length`.
fn clamp_time(moved: &[Ticks], time: Ticks, length: Ticks) -> Ticks {
    let (Some(first), Some(last)) = (moved.iter().min(), moved.iter().max()) else {
        return Ticks::ZERO;
    };
    time.clamp(Ticks::ZERO - *first, length - *last)
}

/// Applies `change` to a copy of the notes as one undo step and checks that
/// every envelope still obeys MPE R1; the notes stay selected as they were.
#[expect(clippy::too_many_arguments, reason = "one edit's whole context")]
fn edit(
    _doc: &Document,
    editor: &mut Editor,
    view: &ClipView,
    clip: bl_model::ClipId,
    midi: &bl_model::MidiData,
    ordered: Vec<Note>,
    places: &[u32],
    id: &str,
    change: impl FnOnce(&mut Vec<Note>) -> Result<(), EditFailure>,
) -> Result<Value, EditFailure> {
    let (_, recorded) = transaction(editor, label(id), |editor| {
        let mut edited = ordered;
        change(&mut edited)?;
        for place in places {
            let note = &edited[*place as usize];
            if !note.expressions_fit() {
                return Err(refused(
                    "the edit would leave a breakpoint outside its note or its range",
                ));
            }
        }
        set_notes(editor, clip, midi, edited)
    })?;
    Ok(to_value(&NoteEditResult {
        notes: places.to_vec(),
        selected: view.selection_of(clip).to_vec(),
        undo_step: recorded,
        clips: Vec::new(),
    }))
}

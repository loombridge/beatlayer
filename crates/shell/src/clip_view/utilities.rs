//! The Pitch and Time Utilities (WP-6.16; the pitch and time utilities spec,
//! R1 to R15): Transpose, Fit to Scale, Invert, Add Interval, Stretch by a
//! factor, Set Length, Humanize, Reverse and Legato on the notes of the clip
//! the editor shows, and the panel's own values.
//!
//! Each one acts on the notes named, or the selected notes, or the notes that
//! start in the time selection, or, for the buttons, every note in the clip
//! (R2); it is one undo step, and one that changes nothing records none
//! (R3). A drag runs the same command on every frame from where it began, so
//! the notes a frame acts on are those the drag began with.

use std::collections::BTreeSet;

use bl_core::time::Ticks;
use bl_midi_tools::{Rng, ToolInput};
use bl_model::note_ops::{self, Mapping};
use bl_model::{ClipId, Document, Editor, MidiData, Note};
use bl_shell_spec::notes::{
    self as ids, AddIntervalArgs, DURATIONS, HumanizeArgs, NoteEditResult, PitchUnit,
    SetLengthArgs, StretchNotesArgs, TransposeArgs, UtilitiesArgs, UtilitiesState,
    UtilityNotesArgs, stretch_factor,
};
use serde_json::Value;

use super::{
    Carried, ClipView, clip_and_data, invalid, note_key, notes_in_order, parse, places_or_selected,
    refused, regroup, set_notes_in, span_of, stretch_envelopes, to_value, unavailable,
};
use crate::arrangement::ArrangementView;
use crate::arrangement::editing::{EditFailure, transaction};
use crate::midi_tools::{default_range, plain_input};

/// The furthest Interval Size goes either way (R7).
const LONGEST_INTERVAL: i32 = 36;
/// The Stretch knob's middle, ×1 (R8).
const STRETCH_MIDDLE: f64 = 0.5;
/// The factor a stretch by a factor may have (R15).
const FACTORS: (f64, f64) = (0.1, 10.0);

/// The panel's values: view state, no undo step, never saved (R14).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Utilities {
    interval: i32,
    stretch: f64,
    duration: &'static str,
    humanize: u8,
}

impl Default for Utilities {
    /// What a new Clip View has: Interval Size 0, Stretch at ×1, Duration
    /// Grid and Humanize Amount 10 % (as Live's panel opens).
    fn default() -> Self {
        Self {
            interval: 0,
            stretch: STRETCH_MIDDLE,
            duration: "grid",
            humanize: 10,
        }
    }
}

/// Whether `id` is one of the utilities, or the panel's own command.
#[must_use]
pub(super) fn handles(id: &str) -> bool {
    matches!(
        id,
        ids::TRANSPOSE
            | ids::FIT_TO_SCALE
            | ids::INVERT
            | ids::ADD_INTERVAL
            | ids::SET_LENGTH
            | ids::HUMANIZE
            | ids::REVERSE
            | ids::LEGATO
            | ids::UTILITIES
    )
}

/// The name of the undo step a utility makes, for the commands whose name
/// doesn't depend on their arguments.
#[must_use]
pub(super) fn label(id: &str) -> Option<&'static str> {
    Some(match id {
        ids::TRANSPOSE => "Transpose",
        ids::FIT_TO_SCALE => "Fit to Scale",
        ids::INVERT => "Invert",
        ids::ADD_INTERVAL => "Interval",
        ids::SET_LENGTH => "Set Length",
        ids::HUMANIZE => "Humanize",
        ids::REVERSE => "Reverse",
        ids::LEGATO => "Legato",
        _ => return None,
    })
}

/// The name of the undo step a stretch by `factor` makes (R3).
fn stretch_label(factor: f64) -> &'static str {
    // The ×2 and ÷2 buttons send exactly these.
    if factor == 2.0 {
        "Stretch Time by Double"
    } else if factor == 0.5 {
        "Shrink Time by Half"
    } else {
        "Stretch"
    }
}

/// The name of the step `note.stretch` makes with these arguments.
#[must_use]
pub(super) fn stretch_label_of(args: &Value) -> &'static str {
    args.get("factor")
        .and_then(Value::as_f64)
        .map_or("Stretch Notes", stretch_label)
}

/// What Set Length gives every note.
#[derive(Clone, Copy)]
enum Length {
    /// This length.
    Of(Ticks),
    /// An end at this tick.
    EndingAt(Ticks),
}

/// What a utility acts on: the notes' places, and the time selection when
/// the clip's editor has one.
struct Target {
    places: Vec<u32>,
    time: Option<(Ticks, Ticks)>,
    /// Whether it is every note in the clip, as no selection leaves it.
    whole: bool,
}

/// The notes a utility acts on (R2): those named, else the selected ones,
/// else those that start in the time selection, else with `whole` every note.
fn target(
    doc: &Document,
    view: &ClipView,
    arrangement: &ArrangementView,
    clip: ClipId,
    ordered: &[Note],
    notes: Option<Vec<u32>>,
    whole: bool,
) -> Result<Target, EditFailure> {
    let shown = view.shown_clip(doc, arrangement) == Some(clip);
    let time = shown
        .then(|| view.time_selection(doc, arrangement))
        .flatten();
    let selected: Vec<u32> = view
        .selection_of(clip)
        .iter()
        .copied()
        .filter(|place| (*place as usize) < ordered.len())
        .collect();
    let all = || (0..u32::try_from(ordered.len()).unwrap_or(u32::MAX)).collect();
    let (places, whole) = if notes.is_some() {
        (places_or_selected(view, clip, notes, ordered.len())?, false)
    } else if !selected.is_empty() {
        (selected, false)
    } else if let Some((from, to)) = time {
        let starting = ordered
            .iter()
            .enumerate()
            .filter(|(_, note)| note.start >= from && note.start < to)
            .map(|(place, _)| u32::try_from(place).unwrap_or(u32::MAX))
            .collect();
        (starting, false)
    } else if whole {
        (all(), true)
    } else {
        return Err(unavailable("no notes are selected"));
    };
    Ok(Target {
        places,
        time,
        whole,
    })
}

/// The notes with each target note replaced by `edit`'s answer, as a note on
/// its way through the edit, in the clip's order; and whether any changed.
fn carried(
    ordered: &[Note],
    selected: &[u32],
    places: &[u32],
    mut edit: impl FnMut(&Note) -> Result<Note, EditFailure>,
) -> Result<(Vec<Carried>, bool), EditFailure> {
    let (selected, places): (BTreeSet<u32>, BTreeSet<u32>) = (
        selected.iter().copied().collect(),
        places.iter().copied().collect(),
    );
    let mut changed = false;
    let mut out = Vec::with_capacity(ordered.len());
    for (place, note) in ordered.iter().enumerate() {
        let at = u32::try_from(place).unwrap_or(u32::MAX);
        let was = selected.contains(&at);
        if places.contains(&at) {
            let made = edit(note)?;
            if made == *note {
                out.push(Carried::kept(note, was));
            } else {
                changed = true;
                out.push(Carried::made(made, was));
            }
        } else {
            out.push(Carried::kept(note, was));
        }
    }
    Ok((out, changed))
}

/// Writes `carried` (with `data` for the clip's other data) as one undo step
/// named `label`, with `inside` run in the same step, or nothing when
/// `changed` is false, and returns the result: the places of the notes made
/// and of the ones selected.
fn finish_with(
    editor: &mut Editor,
    view: &mut ClipView,
    clip: ClipId,
    label: &'static str,
    data: MidiData,
    (mut carried, changed): (Vec<Carried>, bool),
    inside: impl FnOnce(&mut Editor) -> Result<(), EditFailure>,
) -> Result<NoteEditResult, EditFailure> {
    if !changed {
        return Ok(NoteEditResult {
            notes: Vec::new(),
            selected: view.selection_of(clip).to_vec(),
            undo_step: false,
            clips: Vec::new(),
        });
    }
    carried.sort_by_key(|carried| (carried.note.start, carried.note.key));
    let places = |wanted: fn(&Carried) -> bool| -> Vec<u32> {
        carried
            .iter()
            .enumerate()
            .filter(|(_, carried)| wanted(carried))
            .map(|(place, _)| u32::try_from(place).unwrap_or(u32::MAX))
            .collect()
    };
    let made = places(|carried| carried.touched);
    let selected = places(|carried| carried.selected);
    let notes = carried.into_iter().map(|carried| carried.note).collect();
    let ((), recorded) = transaction(editor, label, |editor| {
        inside(editor)?;
        set_notes_in(editor, clip, data, notes)
    })?;
    view.select(clip, selected.clone());
    Ok(NoteEditResult {
        notes: made,
        selected,
        undo_step: recorded,
        clips: Vec::new(),
    })
}

/// [`finish_with`] with nothing else in the step, as the result's JSON.
fn finish(
    editor: &mut Editor,
    view: &mut ClipView,
    clip: ClipId,
    label: &'static str,
    data: MidiData,
    done: (Vec<Carried>, bool),
) -> Result<Value, EditFailure> {
    finish_with(editor, view, clip, label, data, done, |_| Ok(())).map(|result| to_value(&result))
}

/// The key `by` steps from `note`'s, in the unit given (R2): `None` where it
/// would leave the keyboard or a Drum Rack's keys.
fn moved_key(
    input: &ToolInput,
    note: &Note,
    by: i32,
    unit: Option<PitchUnit>,
) -> Result<u8, EditFailure> {
    match unit.unwrap_or_default() {
        PitchUnit::Semitones => note_key(note, by),
        PitchUnit::Steps => input
            .step_key(note.key, by)
            .ok_or_else(|| refused("a note would leave the range of MIDI keys 0 to 127")),
    }
}

/// Runs a utility, or the panel's command.
///
/// # Errors
///
/// [`EditFailure`] saying why nothing changed.
#[expect(
    clippy::too_many_lines,
    reason = "one arm per command keeps each one's behaviour in sight"
)]
pub(super) fn run(
    id: &str,
    args: Value,
    doc: &Document,
    editor: &mut Editor,
    view: &mut ClipView,
    arrangement: &ArrangementView,
) -> Result<Value, EditFailure> {
    match id {
        ids::UTILITIES => {
            let args: UtilitiesArgs = parse(args)?;
            set_values(view, args)?;
            Ok(to_value(&state(doc, view, arrangement)))
        }
        ids::TRANSPOSE => {
            let TransposeArgs {
                clip,
                notes,
                by,
                unit,
            } = parse(args)?;
            let (clip, midi) = clip_and_data(doc, view, arrangement, clip)?;
            let ordered = notes_in_order(&midi);
            let target = target(doc, view, arrangement, clip, &ordered, notes, false)?;
            let input = plain_input(doc, clip);
            let selected = view.selection_of(clip).to_vec();
            let done = carried(&ordered, &selected, &target.places, |note| {
                let mut moved = note.clone();
                moved.key = moved_key(&input, note, by, unit)?;
                Ok(moved)
            })?;
            finish(editor, view, clip, "Transpose", midi, done)
        }
        ids::FIT_TO_SCALE => {
            let UtilityNotesArgs { clip, notes } = parse(args)?;
            let (clip, midi) = clip_and_data(doc, view, arrangement, clip)?;
            let scale = doc
                .clip(clip)
                .map(|clip| clip.scale)
                .filter(|scale| scale.enabled)
                .ok_or_else(|| unavailable("the clip's Scale Mode is off"))?;
            let ordered = notes_in_order(&midi);
            let target = target(doc, view, arrangement, clip, &ordered, notes, true)?;
            let selected = view.selection_of(clip).to_vec();
            let done = carried(&ordered, &selected, &target.places, |note| {
                let mut fitted = note.clone();
                fitted.key = scale.nearest(note.key);
                Ok(fitted)
            })?;
            finish(editor, view, clip, "Fit to Scale", midi, done)
        }
        ids::INVERT => {
            let UtilityNotesArgs { clip, notes } = parse(args)?;
            let (clip, midi) = clip_and_data(doc, view, arrangement, clip)?;
            let ordered = notes_in_order(&midi);
            let target = target(doc, view, arrangement, clip, &ordered, notes, true)?;
            let keys: Vec<u8> = target
                .places
                .iter()
                .map(|place| ordered[*place as usize].key)
                .collect();
            let (Some(&low), Some(&high)) = (keys.iter().min(), keys.iter().max()) else {
                return finish(editor, view, clip, "Invert", midi, (Vec::new(), false));
            };
            let input = plain_input(doc, clip);
            // R6: by the scale's (or a Drum Rack's) steps when both ends are on
            // it and the note is, else by semitones.
            let by_steps = input.allows(low) && input.allows(high);
            let selected = view.selection_of(clip).to_vec();
            let done = carried(&ordered, &selected, &target.places, |note| {
                let mut turned = note.clone();
                let mirrored = u8::try_from(i32::from(low) + i32::from(high) - i32::from(note.key))
                    .unwrap_or(note.key);
                turned.key = if by_steps && input.allows(note.key) {
                    let above = input.steps_between(low, note.key);
                    input.step_key(high, -above).unwrap_or(mirrored)
                } else {
                    mirrored
                };
                Ok(turned)
            })?;
            finish(editor, view, clip, "Invert", midi, done)
        }
        ids::ADD_INTERVAL => {
            let AddIntervalArgs {
                clip,
                notes,
                by,
                unit,
            } = parse(args)?;
            let by = by.unwrap_or(view.utilities.interval);
            if by.abs() > LONGEST_INTERVAL {
                return Err(invalid(format!(
                    "`by` must be from -{LONGEST_INTERVAL} to {LONGEST_INTERVAL}"
                )));
            }
            let (clip, midi) = clip_and_data(doc, view, arrangement, clip)?;
            let ordered = notes_in_order(&midi);
            let target = target(doc, view, arrangement, clip, &ordered, notes, true)?;
            let input = plain_input(doc, clip);
            let mut copies: Vec<Note> = target
                .places
                .iter()
                .filter_map(|place| {
                    let note = &ordered[*place as usize];
                    // R7: left out where it leaves the keyboard, or the pads.
                    let key = moved_key(&input, note, by, unit).ok()?;
                    input
                        .drum_keys
                        .as_ref()
                        .is_none_or(|keys| keys.contains(&key))
                        .then(|| Note {
                            key,
                            ..note.clone()
                        })
                })
                .collect();
            let mut data = midi.clone();
            regroup(&mut data, &mut copies, &midi.groups);
            let changed = !copies.is_empty();
            let mut all: Vec<Carried> = ordered
                .iter()
                .map(|note| Carried::kept(note, false))
                .collect();
            all.extend(copies.into_iter().map(|copy| Carried::made(copy, true)));
            finish(editor, view, clip, "Interval", data, (all, changed))
        }
        ids::SET_LENGTH => {
            let SetLengthArgs {
                clip,
                notes,
                length,
                to_time_range,
            } = parse(args)?;
            let (clip, midi) = clip_and_data(doc, view, arrangement, clip)?;
            let ordered = notes_in_order(&midi);
            let target = target(doc, view, arrangement, clip, &ordered, notes, true)?;
            let how = match (length, to_time_range.unwrap_or(false)) {
                (Some(length), false) if length > Ticks::ZERO => Length::Of(length),
                (Some(_), false) => return Err(invalid("`length` must be longer than 0")),
                (None, true) => Length::EndingAt(
                    target
                        .time
                        .ok_or_else(|| unavailable("there is no time selection to fit to"))?
                        .1,
                ),
                _ => {
                    return Err(invalid(
                        "give `length` or `to_time_range`, not both or neither",
                    ));
                }
            };
            let selected = view.selection_of(clip).to_vec();
            let done = carried(&ordered, &selected, &target.places, |note| {
                Ok(match how {
                    Length::Of(length) => note.resized(length),
                    // R10: a note that starts at or after the end keeps its length.
                    Length::EndingAt(end) if note.start < end => note.resized(end - note.start),
                    Length::EndingAt(_) => note.clone(),
                })
            })?;
            finish(editor, view, clip, "Set Length", midi, done)
        }
        ids::HUMANIZE => {
            let HumanizeArgs {
                clip,
                notes,
                amount,
                seed,
            } = parse(args)?;
            let amount = amount.unwrap_or(view.utilities.humanize);
            if amount > 100 {
                return Err(invalid("`amount` must be from 0 to 100"));
            }
            let (clip, midi) = clip_and_data(doc, view, arrangement, clip)?;
            let ordered = notes_in_order(&midi);
            let target = target(doc, view, arrangement, clip, &ordered, notes, true)?;
            let seed = seed.unwrap_or_else(|| view.tools.draw_seed());
            // R11: up to the amount of a quarter of the grid step, either way.
            let most = i64::from(amount) * arrangement.grid_ticks(doc).get() / 400;
            let mut rng = Rng::new(seed);
            let selected = view.selection_of(clip).to_vec();
            let done = carried(&ordered, &selected, &target.places, |note| {
                let spread = u32::try_from(2 * most + 1).unwrap_or(u32::MAX);
                let by = i64::from(rng.below(spread)) - most;
                let mut moved = note.clone();
                moved.start = (note.start + Ticks(by)).max(Ticks::ZERO);
                Ok(moved)
            })?;
            finish(editor, view, clip, "Humanize", midi, done)
        }
        ids::REVERSE => {
            let UtilityNotesArgs { clip, notes } = parse(args)?;
            let (clip, midi) = clip_and_data(doc, view, arrangement, clip)?;
            let length = doc.clip(clip).map_or(Ticks::ZERO, |clip| clip.length);
            let ordered = notes_in_order(&midi);
            let target = target(doc, view, arrangement, clip, &ordered, notes, true)?;
            // R12: around the middle of the time selection; with none, of the
            // loop brace while Loop is on, else the Start to the End marker.
            let (from, to) = target.time.unwrap_or_else(|| default_range(&midi, length));
            let selected = view.selection_of(clip).to_vec();
            let done = carried(&ordered, &selected, &target.places, |note| {
                note_ops::remap(note, |at| from + to - at).map_err(refused)
            })?;
            finish(editor, view, clip, "Reverse", midi, done)
        }
        ids::LEGATO => {
            let UtilityNotesArgs { clip, notes } = parse(args)?;
            let (clip, midi) = clip_and_data(doc, view, arrangement, clip)?;
            let length = doc.clip(clip).map_or(Ticks::ZERO, |clip| clip.length);
            let ordered = notes_in_order(&midi);
            let target = target(doc, view, arrangement, clip, &ordered, notes, true)?;
            let mut starts: Vec<Ticks> = target
                .places
                .iter()
                .map(|place| ordered[*place as usize].start)
                .collect();
            starts.sort_unstable();
            let limit = default_range(&midi, length).1;
            let selected = view.selection_of(clip).to_vec();
            let done = carried(&ordered, &selected, &target.places, |note| {
                // R13: the next later start among the notes; the latest start
                // reaches the loop's (or the clip's) end, unless it is past it.
                let next = starts
                    .get(starts.partition_point(|start| *start <= note.start))
                    .copied();
                Ok(
                    match next.or(Some(limit).filter(|limit| *limit > note.start)) {
                        Some(end) => note.resized(end - note.start),
                        None => note.clone(),
                    },
                )
            })?;
            finish(editor, view, clip, "Legato", midi, done)
        }
        other => Err(invalid(format!("`{other}` isn't a Pitch and Time Utility"))),
    }
}

/// Stretches notes by a factor (`note.stretch` with `factor`; R8, R9).
///
/// # Errors
///
/// [`EditFailure`] saying why nothing changed.
pub(super) fn stretch_by(
    args: StretchNotesArgs,
    doc: &Document,
    editor: &mut Editor,
    view: &mut ClipView,
    arrangement: &ArrangementView,
) -> Result<Value, EditFailure> {
    let StretchNotesArgs {
        clip,
        notes,
        from,
        to: _,
        point,
        factor,
        markers,
    } = args;
    let factor = factor.unwrap_or(1.0);
    if !(FACTORS.0..=FACTORS.1).contains(&factor) {
        return Err(invalid("`factor` must be from 0.1 to 10"));
    }
    let (clip, midi) = clip_and_data(doc, view, arrangement, clip)?;
    let ordered = notes_in_order(&midi);
    let leaves_it_to_the_range = notes.is_none() && view.selection_of(clip).is_empty();
    let mut target = target(doc, view, arrangement, clip, &ordered, notes, true)?;
    if let Some(range) = from {
        target.time = Some((range.start, range.end));
        // With no notes named or selected, the notes that start inside it.
        if leaves_it_to_the_range {
            target.whole = false;
            target.places = ordered
                .iter()
                .enumerate()
                .filter(|(_, note)| note.start >= range.start && note.start < range.end)
                .map(|(place, _)| u32::try_from(place).unwrap_or(u32::MAX))
                .collect();
        }
    }
    let selected = view.selection_of(clip).to_vec();
    // R8: around the time selection's start; with none, the selected notes'
    // first start, or with every note the clip's contents' start.
    let pivot = match point {
        Some(point) if point.from != point.to => {
            return Err(invalid(
                "with `factor`, a `point`'s `from` and `to` must be the same tick",
            ));
        }
        Some(point) => point.from,
        None => target
            .time
            .map(|(from, _)| from)
            .or_else(|| {
                (!target.whole)
                    .then(|| {
                        target
                            .places
                            .iter()
                            .map(|place| ordered[*place as usize].start)
                            .min()
                    })
                    .flatten()
            })
            .unwrap_or(Ticks::ZERO),
    };
    // A map from the pivot that scales by the factor, to the nearest tick.
    let base = Ticks::WHOLE;
    let scaled = Ticks((base.get() as f64 * factor).round() as i64);
    let map =
        Mapping::new((pivot, pivot + base), (pivot, pivot + scaled), None).map_err(invalid)?;
    let mut data = midi.clone();
    let markers = markers.unwrap_or(false) && target.whole;
    if markers {
        data.start = map.at(data.start);
        data.end = data.end.map(|end| map.at(end));
        data.loop_start = map.at(data.loop_start);
        data.loop_end = data.loop_end.map(|end| map.at(end));
    }
    let time = target.time.map(|(from, to)| (map.at(from), map.at(to)));
    let mut done = carried(&ordered, &selected, &target.places, |note| {
        note_ops::remap(note, |at| map.at(at)).map_err(refused)
    })?;
    done.1 |= markers && data != midi;
    let changed = done.1;
    let range = target.time;
    let result = finish_with(
        editor,
        view,
        clip,
        stretch_label(factor),
        data,
        done,
        |editor| match range {
            Some(range) => stretch_envelopes(editor, clip, range, &map),
            None => Ok(()),
        },
    )?;
    if changed {
        // R3, R8: the time selection stretches with the notes, or follows them.
        let follows = time.or_else(|| span_of(editor.document(), clip, &result.selected));
        view.set_time_selection(clip, follows);
    }
    Ok(to_value(&result))
}

/// Sets the panel's values that `args` gives (R14).
fn set_values(view: &mut ClipView, args: UtilitiesArgs) -> Result<(), EditFailure> {
    let UtilitiesArgs {
        interval,
        stretch,
        duration,
        humanize_amount,
    } = args;
    if interval.is_some_and(|interval| interval.abs() > LONGEST_INTERVAL) {
        return Err(invalid(format!(
            "`interval` must be from -{LONGEST_INTERVAL} to {LONGEST_INTERVAL}"
        )));
    }
    if stretch.is_some_and(|stretch| !(0.0..=1.0).contains(&stretch)) {
        return Err(invalid("`stretch` must be from 0 to 1"));
    }
    let duration = match duration {
        Some(name) => Some(
            DURATIONS
                .iter()
                .find(|known| **known == name)
                .copied()
                .ok_or_else(|| invalid(format!("`{name}` isn't one of the durations")))?,
        ),
        None => None,
    };
    if humanize_amount.is_some_and(|amount| amount > 100) {
        return Err(invalid("`humanize_amount` must be from 0 to 100"));
    }
    let values = &mut view.utilities;
    values.interval = interval.unwrap_or(values.interval);
    values.stretch = stretch.unwrap_or(values.stretch);
    values.duration = duration.unwrap_or(values.duration);
    values.humanize = humanize_amount.unwrap_or(values.humanize);
    Ok(())
}

/// The panel's values and what it shows now.
#[must_use]
pub fn state(doc: &Document, view: &ClipView, arrangement: &ArrangementView) -> UtilitiesState {
    let values = &view.utilities;
    let mut state = UtilitiesState {
        interval: values.interval,
        interval_range: (-LONGEST_INTERVAL, LONGEST_INTERVAL),
        stretch: values.stretch,
        stretch_factor: stretch_factor(values.stretch),
        duration: values.duration.to_owned(),
        humanize_amount: values.humanize,
        ..UtilitiesState::default()
    };
    let Ok((clip, midi)) = clip_and_data(doc, view, arrangement, None) else {
        return state;
    };
    let scale = doc
        .clip(clip)
        .map(|clip| clip.scale)
        .filter(|scale| scale.enabled);
    state.fit_to_scale = scale.is_some();
    state.scale = scale.map(|scale| {
        format!(
            "{} {}",
            bl_model::Scale::root_name(scale.root),
            scale.name.label()
        )
    });
    state.time_range = view.time_selection(doc, arrangement).is_some();
    let ordered = notes_in_order(&midi);
    let Ok(target) = target(doc, view, arrangement, clip, &ordered, None, true) else {
        return state;
    };
    let keys: Vec<u8> = target
        .places
        .iter()
        .map(|place| ordered[*place as usize].key)
        .collect();
    state.lowest = keys.iter().copied().min();
    state.highest = keys.iter().copied().max();
    if let (Some(low), Some(high)) = (state.lowest, state.highest) {
        // The most Interval Size can be, up or down, with every copy kept.
        let input = plain_input(doc, clip);
        let furthest = |key: u8, sign: i32| {
            (0..=LONGEST_INTERVAL)
                .take_while(|by| input.step_key(key, sign * by).is_some())
                .last()
                .unwrap_or(0)
        };
        state.interval_range = (-furthest(low, -1), furthest(high, 1));
    }
    state
}

//! Find and Select Notes (WP-6.4): which notes the editor's note filters let
//! through, and the changes `clip_view.note_filters` makes to their settings.
//!
//! The matching is a function of one clip's notes, so the multi-clip editor
//! (WP-6.6) can run it over each clip. The rules are in the find-and-select
//! notes spec.

use bl_core::time::{Ticks, TimeSignature};
use bl_model::{Note, Scale, ScaleName};
use bl_shell_spec::note_filters::{NoteFilter, NoteFilters, NoteFiltersArgs};

use crate::arrangement::editing::EditFailure;

/// What a clip contributes to its notes' filtering.
#[derive(Clone, Copy, Debug)]
pub struct Frame {
    /// The clip's span of its contents, in ticks (piano-roll R4).
    pub span: (Ticks, Ticks),
    /// The clip's time signature, whose beats Time and Duration count in.
    pub signature: TimeSignature,
    /// The editing grid's step, which Count's Quantized counts.
    pub grid: Ticks,
    /// The clip's scale root and name, whether or not its Scale Mode is on.
    pub scale: (u8, ScaleName),
}

fn invalid(message: impl Into<String>) -> EditFailure {
    EditFailure::InvalidArgs(message.into())
}

/// Beats to hundredths of a beat, so a setting compares exactly.
fn hundredths(beats: f64) -> i64 {
    (beats * 100.0).round() as i64
}

/// The ticks in `beats` of `frame`'s time signature, to a hundredth of a
/// beat.
fn ticks(frame: &Frame, beats: f64) -> Ticks {
    Ticks(hundredths(beats) * frame.signature.beat_length().get() / 100)
}

/// The length of the clip's span, in beats.
#[must_use]
pub fn span_beats(frame: &Frame) -> f64 {
    let beat = frame.signature.beat_length().get().max(1);
    ((frame.span.1 - frame.span.0).get() * 100 / beat) as f64 / 100.0
}

/// A value checked to lie in `low..=high`.
fn within(name: &str, value: f64, low: f64, high: f64) -> Result<f64, EditFailure> {
    if value.is_finite() && (low..=high).contains(&value) {
        Ok(value)
    } else {
        Err(invalid(format!("`{name}` must be from {low} to {high}")))
    }
}

/// A pair of limits after one of them moved: the other follows when the move
/// takes one past it (Spec: find-and-select-notes R7).
fn pair<T: PartialOrd + Copy>(
    name: &str,
    (min, max): (T, T),
    given: (Option<T>, Option<T>),
) -> Result<(T, T), EditFailure> {
    Ok(match given {
        (Some(low), Some(high)) if low > high => {
            return Err(invalid(format!("`{name}` min must not be above its max")));
        }
        (Some(low), Some(high)) => (low, high),
        (Some(low), None) => (low, if max < low { low } else { max }),
        (None, Some(high)) => (if min > high { high } else { min }, high),
        (None, None) => (min, max),
    })
}

/// Applies the controls `args` sets to `filters`, switching each filter it
/// touches on, and returns whether any was touched (Spec:
/// find-and-select-notes R3). Nothing changes when a control is out of range.
///
/// # Errors
///
/// [`EditFailure`] naming the control that is out of range.
#[expect(
    clippy::too_many_lines,
    reason = "one block per filter keeps each one's ranges in sight"
)]
pub fn apply(
    filters: &mut NoteFilters,
    args: &NoteFiltersArgs,
    span_beats: f64,
) -> Result<bool, EditFailure> {
    let mut next = filters.clone();
    let mut touched = Vec::new();
    if let Some(a) = &args.pitch {
        if let Some(classes) = &a.classes {
            if classes.iter().any(|class| *class > 11) {
                return Err(invalid("`classes` are pitch classes from 0 (C) to 11 (B)"));
            }
            let mut classes = classes.clone();
            classes.sort_unstable();
            classes.dedup();
            next.pitch.classes = classes;
        }
        next.pitch.invert = a.invert.unwrap_or(next.pitch.invert);
        touched.push(NoteFilter::Pitch);
    }
    if let Some(a) = &args.time {
        if let Some(start) = a.start {
            next.time.start = within("start", start, 1.0, f64::MAX)?;
        }
        if let Some(length) = a.length {
            // A hundredth of a beat is the smallest window.
            next.time.length = within("length", length, 0.01, f64::MAX)?;
        }
        if let Some(repeat) = a.repeat {
            next.time.repeat = repeat
                .map(|repeat| within("repeat", repeat, 0.0, f64::MAX))
                .transpose()?;
        }
        next.time.invert = a.invert.unwrap_or(next.time.invert);
        touched.push(NoteFilter::Time);
    }
    if let Some(a) = &args.chance {
        let (min, max) = pair(
            "chance",
            (next.chance.min, next.chance.max),
            (
                a.min.map(|v| within("min", v, 0.0, 100.0)).transpose()?,
                a.max.map(|v| within("max", v, 0.0, 100.0)).transpose()?,
            ),
        )?;
        next.chance.min = min;
        next.chance.max = max;
        next.chance.invert = a.invert.unwrap_or(next.chance.invert);
        touched.push(NoteFilter::Chance);
    }
    if let Some(a) = &args.condition {
        next.condition.active = a.active.unwrap_or(next.condition.active);
        next.condition.chance = a.chance.unwrap_or(next.condition.chance);
        next.condition.velocity_deviation = a
            .velocity_deviation
            .unwrap_or(next.condition.velocity_deviation);
        next.condition.invert = a.invert.unwrap_or(next.condition.invert);
        touched.push(NoteFilter::Condition);
    }
    if let Some(a) = &args.count {
        if let Some(every) = a.every {
            if !(1..=16).contains(&every) {
                return Err(invalid("`every` must be from 1 to 16"));
            }
            next.count.every = every;
            // Offset stays one less than Every at most.
            next.count.offset = next.count.offset.min(every - 1);
        }
        if let Some(offset) = a.offset {
            if offset >= next.count.every {
                return Err(invalid("`offset` must be less than `every`"));
            }
            next.count.offset = offset;
        }
        next.count.quantized = a.quantized.unwrap_or(next.count.quantized);
        next.count.invert = a.invert.unwrap_or(next.count.invert);
        touched.push(NoteFilter::Count);
    }
    if let Some(a) = &args.duration {
        let max = next.duration.max.unwrap_or(span_beats);
        // `null` is the span's length, the most Max can be.
        let given_max = a
            .max
            .map(|max| within("max", max.unwrap_or(span_beats), 0.0, span_beats))
            .transpose()?;
        let (min, max) = pair(
            "duration",
            (next.duration.min, max),
            (
                a.min
                    .map(|v| within("min", v, 0.0, span_beats))
                    .transpose()?,
                given_max,
            ),
        )?;
        next.duration.min = min;
        next.duration.max = Some(max);
        next.duration.invert = a.invert.unwrap_or(next.duration.invert);
        touched.push(NoteFilter::Duration);
    }
    if let Some(a) = &args.scale {
        if a.root.is_some_and(|root| root > 11) {
            return Err(invalid("`root` must be from 0 (C) to 11 (B)"));
        }
        next.scale.use_clip_scale = a.use_clip_scale.unwrap_or(next.scale.use_clip_scale);
        next.scale.root = a.root.unwrap_or(next.scale.root);
        next.scale.name = a.name.unwrap_or(next.scale.name);
        next.scale.invert = a.invert.unwrap_or(next.scale.invert);
        touched.push(NoteFilter::Scale);
    }
    if let Some(a) = &args.velocity {
        let range = |name: &str, v: u8| {
            if (1..=127).contains(&v) {
                Ok(v)
            } else {
                Err(invalid(format!("`{name}` must be from 1 to 127")))
            }
        };
        let (min, max) = pair(
            "velocity",
            (next.velocity.min, next.velocity.max),
            (
                a.min.map(|v| range("min", v)).transpose()?,
                a.max.map(|v| range("max", v)).transpose()?,
            ),
        )?;
        next.velocity.min = min;
        next.velocity.max = max;
        next.velocity.invert = a.invert.unwrap_or(next.velocity.invert);
        touched.push(NoteFilter::Velocity);
    }
    for filter in &touched {
        next.set_on(*filter, true);
    }
    *filters = next;
    Ok(!touched.is_empty())
}

/// For each of `notes`, whether `filter` lets it through before Invert.
fn holds(filters: &NoteFilters, filter: NoteFilter, notes: &[&Note], frame: &Frame) -> Vec<bool> {
    let each = |test: &dyn Fn(&Note) -> bool| notes.iter().map(|note| test(note)).collect();
    match filter {
        NoteFilter::Pitch => each(&|note| filters.pitch.classes.contains(&(note.key % 12))),
        NoteFilter::Time => {
            let time = &filters.time;
            let start = ticks(frame, time.start - 1.0);
            let length = ticks(frame, time.length);
            let repeat = time
                .repeat
                .map_or(frame.signature.bar_length(), |beats| ticks(frame, beats));
            each(&|note| {
                let from = note.start - frame.span.0 - start;
                !from.is_negative()
                    && if repeat > Ticks::ZERO {
                        from.get().rem_euclid(repeat.get()) < length.get()
                    } else {
                        from < length
                    }
            })
        }
        NoteFilter::Chance => {
            let chance = &filters.chance;
            each(&|note| {
                let percent = note.probability * 100.0;
                // Float noise from a fine-tuned value (Spec: R7).
                percent >= chance.min - 1e-6 && percent <= chance.max + 1e-6
            })
        }
        NoteFilter::Condition => {
            let condition = &filters.condition;
            each(&|note| {
                (!condition.active || !note.mute)
                    && (!condition.chance || note.probability < 1.0)
                    && (!condition.velocity_deviation || note.velocity_deviation != 0)
            })
        }
        NoteFilter::Count => {
            let count = &filters.count;
            let every = i64::from(count.every.max(1));
            let offset = i64::from(count.offset);
            // Each note's count: the step it starts in, or the number of
            // distinct starts before it (Spec: R9).
            let mut counts = Vec::with_capacity(notes.len());
            let mut distinct = -1_i64;
            let mut last = None;
            for note in notes {
                if count.quantized {
                    counts.push(
                        (note.start - frame.span.0)
                            .get()
                            .div_euclid(frame.grid.get()),
                    );
                } else {
                    if last != Some(note.start) {
                        distinct += 1;
                        last = Some(note.start);
                    }
                    counts.push(distinct);
                }
            }
            counts
                .into_iter()
                .map(|n| n >= offset && (n - offset) % every == 0)
                .collect()
        }
        NoteFilter::Duration => {
            let duration = &filters.duration;
            let min = ticks(frame, duration.min);
            let max = ticks(frame, duration.max.unwrap_or_else(|| span_beats(frame)));
            each(&|note| note.length >= min && note.length <= max)
        }
        NoteFilter::Scale => {
            let (root, name) = if filters.scale.use_clip_scale {
                frame.scale
            } else {
                (filters.scale.root, filters.scale.name)
            };
            let scale = Scale::new(root, name);
            each(&|note| scale.holds(note.key))
        }
        NoteFilter::Velocity => {
            let velocity = &filters.velocity;
            each(&|note| note.velocity >= velocity.min && note.velocity <= velocity.max)
        }
    }
}

/// Whether `filter` has Invert on.
fn inverted(filters: &NoteFilters, filter: NoteFilter) -> bool {
    match filter {
        NoteFilter::Pitch => filters.pitch.invert,
        NoteFilter::Time => filters.time.invert,
        NoteFilter::Chance => filters.chance.invert,
        NoteFilter::Condition => filters.condition.invert,
        NoteFilter::Count => filters.count.invert,
        NoteFilter::Duration => filters.duration.invert,
        NoteFilter::Scale => filters.scale.invert,
        NoteFilter::Velocity => filters.velocity.invert,
    }
}

/// The places, in `ordered` (a clip's notes in order), of the notes in the
/// clip's span that every filter that is on lets through (Spec:
/// find-and-select-notes R3).
#[must_use]
pub fn select(filters: &NoteFilters, ordered: &[Note], frame: &Frame) -> Vec<u32> {
    let inside: Vec<(u32, &Note)> = ordered
        .iter()
        .enumerate()
        .filter(|(_, note)| note.start >= frame.span.0 && note.start < frame.span.1)
        .filter_map(|(place, note)| Some((u32::try_from(place).ok()?, note)))
        .collect();
    let notes: Vec<&Note> = inside.iter().map(|(_, note)| *note).collect();
    let mut keep = vec![true; notes.len()];
    for filter in NoteFilter::ALL {
        if !filters.is_on(filter) {
            continue;
        }
        let flip = inverted(filters, filter);
        for (keep, held) in keep.iter_mut().zip(holds(filters, filter, &notes, frame)) {
            *keep &= held != flip;
        }
    }
    inside
        .into_iter()
        .zip(keep)
        .filter_map(|((place, _), keep)| keep.then_some(place))
        .collect()
}

//! What a tool is given and what it writes (the MIDI Tools spec, R16), and
//! the step helpers that count scale degrees and drum keys (R8, R9).

use bl_core::time::{Ticks, TimeSignature};

/// One breakpoint of a note's envelope, counted from the note's start: the
/// value there and the bend of the segment starting at it, from -1 to 1
/// (the MPE spec, R1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ToolPoint {
    /// Where it sits, counted from the note's start.
    pub at: Ticks,
    /// The value there: semitones for Pitch, 0 to 127 for Slide and Pressure.
    pub value: f64,
    /// The bend of the segment starting here; 0 is the straight line.
    pub bend: f64,
}

/// A note as a tool sees it: every field a note in the document has,
/// with its Pitch, Slide and Pressure envelopes.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolNote {
    /// The key. A tool may write a key outside 0 to 127; Beatlayer moves it
    /// by octaves until it is inside (the MIDI Tools spec, R13).
    pub key: i16,
    /// Where the note starts, in ticks of the clip's contents.
    pub start: Ticks,
    /// How long it lasts.
    pub length: Ticks,
    /// The velocity, 1 to 127.
    pub velocity: u8,
    /// The release velocity, 1 to 127.
    pub release_velocity: u8,
    /// The chance the note plays, from 0 to 1.
    pub chance: f64,
    /// How far velocity may deviate randomly, -127 to 127.
    pub velocity_deviation: i8,
    /// Whether the note is deactivated.
    pub deactivated: bool,
    /// The note's pitch bend, in order of `at`.
    pub pitch: Vec<ToolPoint>,
    /// The note's slide, in order of `at`.
    pub slide: Vec<ToolPoint>,
    /// The note's pressure, in order of `at`.
    pub pressure: Vec<ToolPoint>,
}

impl ToolNote {
    /// A generated note: release velocity 64, chance 100 %, velocity
    /// deviation 0, active, with no expressions (the MIDI Tools spec, R14).
    #[must_use]
    pub fn new(key: i16, start: Ticks, length: Ticks, velocity: u8) -> Self {
        Self {
            key,
            start,
            length,
            velocity,
            release_velocity: 64,
            chance: 1.0,
            velocity_deviation: 0,
            deactivated: false,
            pitch: Vec::new(),
            slide: Vec::new(),
            pressure: Vec::new(),
        }
    }

    /// Where the note ends.
    #[must_use]
    pub fn end(&self) -> Ticks {
        self.start + self.length
    }

    /// The note with this length: a shorter note ends each envelope at its
    /// new end with the value there, a longer one lets the last value hold
    /// (the MPE spec, R4).
    #[must_use]
    pub fn with_length(&self, length: Ticks) -> Self {
        if length >= self.length {
            return Self {
                length,
                ..self.clone()
            };
        }
        self.part(Ticks::ZERO, length)
    }

    /// The note over its part from `from` to `to`, both counted from its
    /// start: each envelope's stretch over that part, with a breakpoint at
    /// each end holding the value there, as a split note's part has (the
    /// MPE spec, R4).
    #[must_use]
    pub fn part(&self, from: Ticks, to: Ticks) -> Self {
        Self {
            start: self.start + from,
            length: to - from,
            pitch: window(&self.pitch, from, to),
            slide: window(&self.slide, from, to),
            pressure: window(&self.pressure, from, to),
            ..self.clone()
        }
    }
}

/// The value `points` hold at `at`, as the document reads an envelope:
/// the first value before it, the last after, and the bent segment between.
fn value_at(points: &[ToolPoint], at: Ticks) -> Option<f64> {
    let first = points.first()?;
    let last = points.last()?;
    if at <= first.at {
        return Some(first.value);
    }
    if at >= last.at {
        return Some(last.value);
    }
    let index = points.partition_point(|point| point.at <= at) - 1;
    let (start, end) = (points[index], points[index + 1]);
    let along = (at.0 - start.at.0) as f64 / (end.at.0 - start.at.0) as f64;
    let along = if start.bend == 0.0 {
        along
    } else {
        along.powf(2f64.powf(start.bend))
    };
    Some(start.value + (end.value - start.value) * along)
}

/// The stretch of `points` from `from` to `to`, counted from `from`; none
/// for an envelope with no breakpoints.
fn window(points: &[ToolPoint], from: Ticks, to: Ticks) -> Vec<ToolPoint> {
    let (Some(start), Some(end)) = (value_at(points, from), value_at(points, to)) else {
        return Vec::new();
    };
    // The bend of the segment the part starts in carries on into the part.
    let bend = points
        .iter()
        .rev()
        .find(|point| point.at <= from)
        .map_or(0.0, |point| point.bend);
    let mut part = vec![ToolPoint {
        at: Ticks::ZERO,
        value: start,
        bend,
    }];
    part.extend(
        points
            .iter()
            .filter(|point| point.at > from && point.at < to)
            .map(|point| ToolPoint {
                at: point.at - from,
                ..*point
            }),
    );
    part.push(ToolPoint {
        at: to - from,
        value: end,
        bend: 0.0,
    });
    part
}

/// A clip's scale: a root and the steps above it, in semitones.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolScale {
    /// The root, 0 (C) to 11 (B).
    pub root: u8,
    /// The scale's steps above the root.
    pub steps: Vec<u8>,
}

/// Everything a tool's notes depend on (the MIDI Tools spec, R16).
#[derive(Clone, Debug, PartialEq)]
pub struct ToolInput {
    /// The input notes, in the order of the clip's notes.
    pub notes: Vec<ToolNote>,
    /// The range the tool acts on: from its start to its end, in ticks of
    /// the clip's contents.
    pub range: (Ticks, Ticks),
    /// The note editor's grid step at the range's start.
    pub grid_step: Ticks,
    /// The clip's time signature.
    pub signature: TimeSignature,
    /// The clip's scale; `None` while Scale Mode is off, when a degree is a
    /// semitone.
    pub scale: Option<ToolScale>,
    /// The keys the track's Drum Rack receives, in order; `None` on a track
    /// that isn't a Drum Rack. They take the scale's place (R9).
    pub drum_keys: Option<Vec<u8>>,
    /// The application's seed.
    pub seed: u32,
}

impl ToolInput {
    /// The key `degrees` degrees from `key`: through the drum keys where
    /// there are some, else through the clip's scale, else by semitones.
    /// `None` when the move leaves the keyboard (R8, R9).
    #[must_use]
    pub fn step_key(&self, key: u8, degrees: i32) -> Option<u8> {
        if let Some(keys) = &self.drum_keys {
            let below = keys.iter().rposition(|k| *k <= key).unwrap_or(0);
            let moved = *keys.get(below.checked_add_signed(isize::try_from(degrees).ok()?)?)?;
            let key = i32::from(key) + (i32::from(moved) - i32::from(*keys.get(below)?));
            return u8::try_from(key).ok().filter(|key| *key <= 127);
        }
        match &self.scale {
            Some(scale) => bl_core::scale::move_by(scale.root, &scale.steps, key, degrees),
            None => u8::try_from(i32::from(key) + degrees)
                .ok()
                .filter(|key| *key <= 127),
        }
    }

    /// The key `degrees` degrees from `key`, as [`Self::step_key`] counts
    /// them, but a move off the keyboard gives the key it would reach (the
    /// caller folds it back by octaves, R13) and a move past the end of a
    /// Drum Rack's keys goes on round the list.
    #[must_use]
    pub fn stepped(&self, key: u8, degrees: i32) -> i16 {
        let clamp =
            |key: i32| i16::try_from(key).unwrap_or(if key < 0 { i16::MIN } else { i16::MAX });
        if let Some(keys) = self.drum_keys.as_ref().filter(|keys| !keys.is_empty()) {
            let below = keys.iter().rposition(|k| *k <= key).unwrap_or(0);
            let len = keys.len() as i64;
            let at = (below as i64 + i64::from(degrees)).rem_euclid(len) as usize;
            return clamp(i32::from(key) + i32::from(keys[at]) - i32::from(keys[below]));
        }
        match &self.scale {
            Some(scale) => clamp(bl_core::scale::move_off(
                scale.root,
                &scale.steps,
                key,
                degrees,
            )),
            None => clamp(i32::from(key) + degrees),
        }
    }

    /// How many degrees it is from `from` up to `to`, as [`Self::stepped`]
    /// counts them; negative where `to` is lower.
    #[must_use]
    pub fn steps_between(&self, from: u8, to: u8) -> i32 {
        let index = |key: u8| -> i64 {
            if let Some(keys) = &self.drum_keys {
                return keys
                    .iter()
                    .rposition(|k| *k <= key)
                    .map_or(0, |at| at as i64);
            }
            match &self.scale {
                Some(scale) => {
                    bl_core::scale::index_at_or_below(scale.root, &scale.steps, i32::from(key))
                }
                None => i64::from(key),
            }
        };
        i32::try_from(index(to) - index(from)).unwrap_or(0)
    }

    /// Whether a tool may write a note on `key`: a drum key, or a key of the
    /// scale, or any key (R8, R9).
    #[must_use]
    pub fn allows(&self, key: u8) -> bool {
        if let Some(keys) = &self.drum_keys {
            return keys.contains(&key);
        }
        self.scale
            .as_ref()
            .is_none_or(|scale| bl_core::scale::holds(scale.root, &scale.steps, key))
    }

    /// The key nearest `key` that a tool may write a note on, the lower of
    /// two equally near; `key` itself where there is none.
    #[must_use]
    pub fn nearest_key(&self, key: u8) -> u8 {
        if let Some(keys) = &self.drum_keys {
            return keys
                .iter()
                .copied()
                .min_by_key(|k| (i32::from(*k).abs_diff(i32::from(key)), *k))
                .unwrap_or(key);
        }
        match &self.scale {
            Some(scale) => bl_core::scale::nearest(scale.root, &scale.steps, key),
            None => key,
        }
    }
}

/// What a tool writes: the notes that go into the range.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ToolOutput {
    /// The notes written.
    pub notes: Vec<ToolNote>,
}

/// `key` moved by octaves until it is from 0 to 127 (the MIDI Tools spec,
/// R13).
#[must_use]
pub fn fold_octaves(key: i32) -> u8 {
    let mut key = key;
    if key < 0 {
        key += 12 * ((-key + 11) / 12);
    } else if key > 127 {
        key -= 12 * ((key - 127 + 11) / 12);
    }
    key as u8
}

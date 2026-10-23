//! What a tool is given and what it writes (the MIDI Tools spec, R16), and
//! the step helpers that count scale degrees and drum keys (R8, R9).

use bl_core::time::{Ticks, TimeSignature};

/// One point of a note's pressure: from `at` after the note's start the key
/// is pressed to `value`, until the next point.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ToolPressure {
    /// Where it takes effect, counted from the note's start.
    pub at: Ticks,
    /// The pressure, from 0 to 127.
    pub value: u8,
}

/// A note as a tool sees it: every field a note in the document has. The
/// expressions are the pressure points today.
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
    /// The note's pressure, in order of `at`.
    pub pressure: Vec<ToolPressure>,
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
            pressure: Vec::new(),
        }
    }

    /// Where the note ends.
    #[must_use]
    pub fn end(&self) -> Ticks {
        self.start + self.length
    }

    /// The note with this length: a shorter note drops the pressure past its
    /// new end, a longer one lets the last value hold (the MPE spec, R4).
    #[must_use]
    pub fn with_length(&self, length: Ticks) -> Self {
        let mut note = self.clone();
        note.length = length;
        note.pressure.retain(|point| point.at < length);
        note
    }

    /// The note over its part from `from` to `to`, both counted from its
    /// start: it takes the stretch of the pressure over that part, with a
    /// point at the part's start holding the value there, as a split note's
    /// part does (the MPE spec, R4).
    #[must_use]
    pub fn part(&self, from: Ticks, to: Ticks) -> Self {
        let mut note = self.clone();
        note.start = self.start + from;
        note.length = to - from;
        note.pressure = match self
            .pressure
            .iter()
            .rev()
            .find(|point| point.at <= from)
            .or(self.pressure.first())
        {
            None => Vec::new(),
            Some(held) => std::iter::once(ToolPressure {
                at: Ticks::ZERO,
                value: held.value,
            })
            .chain(
                self.pressure
                    .iter()
                    .filter(|point| point.at > from && point.at < to)
                    .map(|point| ToolPressure {
                        at: point.at - from,
                        value: point.value,
                    }),
            )
            .collect(),
        };
        note
    }
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

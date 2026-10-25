//! Scale, the scale-map MIDI effect (WP-5.30; the MIDI scale map spec): sends
//! each note of an octave to a note of your choosing, as a grid from the
//! notes that come in to the notes that go out, then transposes the result.
//! Notes mapped to nothing play nothing.
//!
//! Its look is the framework's generic layout: Beatlayer's own.

use bl_engine::{DeviceType, NoteBuffer, NoteEvent, NoteKind, Registry};
use bl_model::{DeviceDescription, DeviceRole, ParamDescriptor, ParamId, ParamUnit};

use crate::framework::{Audio, NativeDevice, ParamBank, add};
use crate::midi_fx::{Key, Router, scale_choices, scale_named, use_current_scale, value};

/// The Scale device type's id.
pub const REMAP: &str = "beatlayer.remap";

/// The key the grid's first column is counted from, as a pitch class.
pub const BASE: ParamId = ParamId(0);
/// Moves every output note, in semitones.
pub const TRANSPOSE: ParamId = ParamId(1);
/// Keeps a mapped note within half an octave of the note that came in.
pub const FOLD: ParamId = ParamId(2);
/// The lowest key that is remapped.
pub const LOWEST: ParamId = ParamId(3);
/// How many keys above Lowest are remapped.
pub const RANGE: ParamId = ParamId(4);
/// Takes Base and Scale Name from the device's current scale (the scale-map
/// spec, R12); the title bar draws it.
pub const USE_SCALE: ParamId = ParamId(5);
/// The grid's first column: where the note one step above Base goes is the
/// second, and so on for the twelve notes of an octave. The value is the
/// output row, 0 to 12 steps above Base, or −1 for no note.
pub const MAP: ParamId = ParamId(6);

/// The twelve columns.
const COLUMNS: u32 = 12;

/// The chooser of the grid: User, the grid a person edits, then a scale from
/// the list (the scale-map spec, R10). A parameter after the columns, so a
/// device saved before it keeps its numbers.
pub const SCALE_NAME: ParamId = ParamId(MAP.0 + COLUMNS);

/// The pitch classes the Base chooser offers.
pub const NOTES: [&str; 12] = [
    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
];

/// The grid's column `column` parameter.
#[must_use]
pub const fn map(column: u32) -> ParamId {
    ParamId(MAP.0 + column)
}

/// Scale's device type.
#[must_use]
pub fn remap_type() -> DeviceType {
    let mut params = vec![
        ParamDescriptor::choice(BASE, "Base", &NOTES, 0)
            .describe("The note the grid's first column stands for."),
        ParamDescriptor::integer(TRANSPOSE, "Transpose", (-36, 36), 0, ParamUnit::Semitones)
            .describe("Moves every note that comes out, after it is mapped."),
        ParamDescriptor::toggle(FOLD, "Fold", false).describe(
            "Moves a mapped note by an octave when that brings it nearer the note that came in.",
        ),
        ParamDescriptor::integer(LOWEST, "Lowest", (0, 127), 0, ParamUnit::None)
            .describe("The lowest key that is remapped; lower keys pass through."),
        ParamDescriptor::integer(RANGE, "Range", (0, 127), 127, ParamUnit::None)
            .describe("How many keys above Lowest are remapped; higher keys pass through."),
        ParamDescriptor::toggle(USE_SCALE, "Use Current Scale", false).describe(
            "Takes Base and Scale Name from the current scale, and shows them. Every note stays \
             as it came while the current scale has Scale Mode off.",
        ),
    ];
    params.extend((0..COLUMNS).map(|c| {
        ParamDescriptor::integer(
            map(c),
            &format!("Map {}", c + 1),
            (-1, 12),
            i64::from(c),
            ParamUnit::None,
        )
        .describe("Where this note of the octave goes: its row in the grid, or -1 for nowhere.")
    }));
    params.push(
        ParamDescriptor::choice(SCALE_NAME, "Scale Name", &scale_choices("User"), 0).describe(
            "User sends the notes by the grid you edit. A scale of the list sends each note to \
             the scale note nearest it, counted from Base.",
        ),
    );
    DeviceType::native(REMAP, "Scale", DeviceRole::MidiEffect)
        .with_description(DeviceDescription::new(params).with_title_switch(USE_SCALE))
}

/// Adds Scale to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, remap_type(), |_, _| Remap {
        router: Router::new(),
    });
}

struct Remap {
    router: Router,
}

/// What a block of Scale's notes is mapped by.
struct Grid {
    base: i32,
    rows: [i32; COLUMNS as usize],
    transpose: i32,
    fold: bool,
    lowest: i32,
    highest: i32,
}

impl Grid {
    /// The key `key` plays as, or `None` for no note.
    fn apply(&self, key: u8) -> Option<u8> {
        let from = i32::from(key);
        if !(self.lowest..=self.highest).contains(&from) {
            return Some(key);
        }
        let octave = (from - self.base).div_euclid(12);
        let column = (from - self.base).rem_euclid(12) as usize;
        let row = self.rows[column];
        if row < 0 {
            return None;
        }
        let mut to = self.base + octave * 12 + row;
        if self.fold {
            if to - from > 6 {
                to -= 12;
            } else if from - to > 6 {
                to += 12;
            }
        }
        to += self.transpose;
        (0..=127).contains(&to).then_some(to as u8)
    }
}

impl Remap {
    /// The grid for a note `offset` frames into the stretch, or `None` where
    /// the note stays as it came: Use Current Scale on with the current
    /// scale's Scale Mode off (the scale-map spec, R12).
    fn grid(audio: &Audio<'_, '_>, params: &ParamBank, offset: u32) -> Option<Grid> {
        let lowest = value(params, LOWEST) as i32;
        let use_scale = use_current_scale(audio, params, USE_SCALE);
        let (base, named) = if use_scale {
            let scale = audio.scale_at(offset);
            (i32::from(scale.root), scale.enabled.then_some(scale.name))
        } else {
            (
                value(params, BASE) as i32,
                scale_named(value(params, SCALE_NAME) as usize),
            )
        };
        if use_scale && named.is_none() {
            return None;
        }
        Some(Grid {
            base,
            rows: match named {
                // Spec: midi-scale-map R10. Each column goes to the scale
                // note nearest it, the lower of two.
                Some(name) => nearest_rows(name.steps()),
                None => core::array::from_fn(|c| value(params, map(c as u32)) as i32),
            },
            transpose: value(params, TRANSPOSE) as i32,
            // Spec: midi-scale-map R11. Fold works only for User.
            fold: named.is_none() && value(params, FOLD) >= 0.5,
            lowest,
            highest: (lowest + value(params, RANGE) as i32).min(127),
        })
    }
}

/// The row each column of a named scale sends its note to: the step of
/// `steps`, or the octave above, nearest the column, the lower of two.
fn nearest_rows(steps: &[u8]) -> [i32; COLUMNS as usize] {
    core::array::from_fn(|column| {
        let column = column as i32;
        let steps = || steps.iter().map(|step| i32::from(*step)).chain([12]);
        let below = steps().filter(|step| *step <= column).max().unwrap_or(0);
        let above = steps().filter(|step| *step >= column).min().unwrap_or(12);
        if column - below <= above - column {
            below
        } else {
            above
        }
    })
}

impl NativeDevice for Remap {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        for note in audio.notes() {
            match note.kind {
                NoteKind::On => {
                    let to = match Self::grid(audio, params, note.offset) {
                        Some(grid) => grid.apply(note.key),
                        None => Some(note.key),
                    };
                    let made = to.map(|key| Key {
                        key,
                        ..Key::of(&note)
                    });
                    if self.router.track(&note, made)
                        && let Some(key) = to
                    {
                        let moved = NoteEvent { key, ..note };
                        self.router.send_now(audio, moved, Key::of(&note));
                    }
                }
                NoteKind::Off | NoteKind::Choke => self.router.end(audio, &note, note.velocity),
                NoteKind::Expression { .. } => self.router.expression(audio, &note),
                // Spec: device-framework R39. A channel message passes through.
                _ => audio.send(note),
            }
        }
        self.router.finish(audio);
    }

    fn release(&mut self, out: &mut NoteBuffer) {
        self.router.release(out);
    }

    fn dropped_notes(&self) -> u64 {
        self.router.dropped()
    }
}

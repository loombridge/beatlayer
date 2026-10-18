//! Remap, the scale-map MIDI effect (WP-5.30; the MIDI scale map spec): sends
//! each note of an octave to a note of your choosing, as a grid from the
//! notes that come in to the notes that go out, then transposes the result.
//! Notes mapped to nothing play nothing.
//!
//! Its look is the framework's generic layout: Beatlayer's own.

use bl_engine::{DeviceType, NoteBuffer, NoteEvent, NoteKind, Registry};
use bl_model::{DeviceDescription, DeviceRole, ParamDescriptor, ParamId, ParamUnit};

use crate::framework::{Audio, NativeDevice, ParamBank, add};
use crate::midi_fx::{Key, Router, value};

/// The Remap device type's id.
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
/// Reads the clip's scale (Phase 6).
pub const USE_SCALE: ParamId = ParamId(5);
/// The grid's first column: where the note one step above Base goes is the
/// second, and so on for the twelve notes of an octave. The value is the
/// output row, 0 to 12 steps above Base, or −1 for no note.
pub const MAP: ParamId = ParamId(6);

/// The twelve columns.
const COLUMNS: u32 = 12;

/// The pitch classes the Base chooser offers.
pub const NOTES: [&str; 12] = [
    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
];

/// The grid's column `column` parameter.
#[must_use]
pub const fn map(column: u32) -> ParamId {
    ParamId(MAP.0 + column)
}

/// Remap's device type.
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
            "Takes the base and scale from the clip. It does nothing until scales arrive in \
             Phase 6.",
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
    DeviceType::native(REMAP, "Remap", DeviceRole::MidiEffect)
        .with_description(DeviceDescription::new(params))
}

/// Adds Remap to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, remap_type(), |_, _| Remap {
        router: Router::new(),
    });
}

struct Remap {
    router: Router,
}

/// What a block of Remap's notes is mapped by.
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

impl NativeDevice for Remap {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        let lowest = value(params, LOWEST) as i32;
        let grid = Grid {
            base: value(params, BASE) as i32,
            rows: core::array::from_fn(|c| value(params, map(c as u32)) as i32),
            transpose: value(params, TRANSPOSE) as i32,
            fold: value(params, FOLD) >= 0.5,
            lowest,
            highest: (lowest + value(params, RANGE) as i32).min(127),
        };
        for note in audio.notes() {
            match note.kind {
                NoteKind::On => {
                    let to = grid.apply(note.key);
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

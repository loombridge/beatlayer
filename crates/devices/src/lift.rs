//! Lift, the transposing MIDI effect (WP-5.30; the MIDI transpose spec): moves
//! every note by a number of semitones, and keeps the result inside a window
//! of keys by blocking, folding or limiting what falls outside it.
//!
//! Its look is the framework's generic layout: Beatlayer's own.

use bl_engine::{DeviceType, NoteBuffer, NoteEvent, NoteKind, Registry};
use bl_model::{DeviceDescription, DeviceRole, ParamDescriptor, ParamId, ParamUnit};

use crate::framework::{Audio, NativeDevice, ParamBank, add};
use crate::midi_fx::{Key, Router, value};

/// The Lift device type's id.
pub const LIFT: &str = "beatlayer.lift";

/// The transposition, in semitones.
pub const PITCH: ParamId = ParamId(0);
/// How far Step Up and Step Down move the transposition, in semitones.
pub const STEP_WIDTH: ParamId = ParamId(1);
/// The lowest key of the window.
pub const LOWEST: ParamId = ParamId(2);
/// How many keys the window spans above its lowest.
pub const RANGE: ParamId = ParamId(3);
/// What happens to a note outside the window.
pub const MODE: ParamId = ParamId(4);
/// Reads Pitch and Step Width in scale degrees (Phase 6).
pub const USE_SCALE: ParamId = ParamId(5);

/// The Mode choices.
pub const MODES: [&str; 3] = ["Block", "Fold", "Limit"];

/// Lift's device type.
#[must_use]
pub fn lift_type() -> DeviceType {
    DeviceType::native(LIFT, "Lift", DeviceRole::MidiEffect).with_description(
        DeviceDescription::new(vec![
            ParamDescriptor::integer(PITCH, "Pitch", (-128, 128), 0, ParamUnit::Semitones)
                .describe("Moves every note by this many semitones."),
            ParamDescriptor::integer(STEP_WIDTH, "Step Width", (1, 48), 1, ParamUnit::Semitones)
                .describe("How far the Step Up and Step Down buttons move Pitch."),
            ParamDescriptor::integer(LOWEST, "Lowest", (0, 127), 0, ParamUnit::None)
                .describe("The lowest key a note may sound on after it is moved."),
            ParamDescriptor::integer(RANGE, "Range", (0, 127), 127, ParamUnit::None)
                .describe("How many keys above Lowest a note may sound on."),
            ParamDescriptor::choice(MODE, "Mode", &MODES, 0).describe(
                "What happens to a note outside the window: Block drops it, Fold moves it by \
                 octaves into the window, Limit moves it to the window's nearest edge.",
            ),
            ParamDescriptor::toggle(USE_SCALE, "Use Current Scale", false).describe(
                "Reads Pitch in scale degrees. It does nothing until scales arrive in Phase 6.",
            ),
        ]),
    )
}

/// Adds Lift to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, lift_type(), |_, _| Lift {
        router: Router::new(),
    });
}

/// Where Lift sends `key`: moved by `pitch` into the window from `lowest`
/// up to `highest`, or `None` if the note is blocked.
fn lifted(key: u8, pitch: i32, lowest: i32, highest: i32, mode: usize) -> Option<u8> {
    let moved = i32::from(key) + pitch;
    if (lowest..=highest).contains(&moved) {
        return Some(moved as u8);
    }
    match mode {
        1 => {
            // Fold by octaves; a window narrower than an octave may have no
            // place for the note.
            let mut at = moved;
            while at > highest {
                at -= 12;
            }
            while at < lowest {
                at += 12;
            }
            (at <= highest).then_some(at as u8)
        }
        2 => Some(moved.clamp(lowest, highest) as u8),
        _ => None,
    }
}

struct Lift {
    router: Router,
}

impl NativeDevice for Lift {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        let pitch = value(params, PITCH) as i32;
        let lowest = value(params, LOWEST) as i32;
        let highest = (lowest + value(params, RANGE) as i32).min(127);
        let mode = value(params, MODE) as usize;
        for note in audio.notes() {
            match note.kind {
                NoteKind::On => {
                    let made = lifted(note.key, pitch, lowest, highest, mode).map(|key| Key {
                        key,
                        ..Key::of(&note)
                    });
                    if self.router.track(&note, made)
                        && let Some(made) = made
                    {
                        let moved = NoteEvent {
                            key: made.key,
                            ..note
                        };
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

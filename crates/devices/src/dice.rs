//! Dice, the random-pitch MIDI effect (WP-5.30; the MIDI random pitch spec):
//! moves some notes by a random number of steps up, down or both, chosen
//! from a fixed set, or cycling through the set in order.
//!
//! Its look is the framework's generic layout: Beatlayer's own.

use bl_engine::midi_clips::Rng;
use bl_engine::{DeviceType, NoteBuffer, NoteEvent, NoteKind, Registry};
use bl_model::{DeviceDescription, DeviceRole, ParamDescriptor, ParamId, ParamUnit};

use crate::framework::{Audio, NativeDevice, ParamBank, add};
use crate::midi_fx::{Key, Router, seed, value};

/// The Dice device type's id.
pub const DICE: &str = "beatlayer.dice";

/// How likely each note is to move, in per cent.
pub const CHANCE: ParamId = ParamId(0);
/// How many different distances a note may move by.
pub const CHOICES: ParamId = ParamId(1);
/// The step between those distances, in semitones.
pub const INTERVAL: ParamId = ParamId(2);
/// How the distance is picked.
pub const MODE: ParamId = ParamId(3);
/// Which way notes move.
pub const SIGN: ParamId = ParamId(4);
/// Keeps moved notes in the clip's scale (Phase 6).
pub const USE_SCALE: ParamId = ParamId(5);

/// The Mode choices.
pub const MODES: [&str; 2] = ["Random", "Alt"];
/// The Sign choices.
pub const SIGNS: [&str; 3] = ["Add", "Sub", "Bi"];

/// Dice's device type.
#[must_use]
pub fn dice_type() -> DeviceType {
    DeviceType::native(DICE, "Dice", DeviceRole::MidiEffect).with_description(
        DeviceDescription::new(vec![
            ParamDescriptor::continuous(CHANCE, "Chance", (0.0, 100.0), 100.0, ParamUnit::Percent)
                .decimals(0)
                .describe("How likely each note is to move: 0 % leaves every note alone."),
            ParamDescriptor::integer(CHOICES, "Choices", (1, 24), 24, ParamUnit::None).describe(
                "How many different distances a note may move by; with Interval, they set \
                 how far notes can go.",
            ),
            ParamDescriptor::integer(INTERVAL, "Interval", (1, 24), 1, ParamUnit::Semitones)
                .describe("The step between the distances a note may move by."),
            ParamDescriptor::choice(MODE, "Mode", &MODES, 0).describe(
                "Random picks a distance without a fixed order; Alt goes through the \
                 distances in turn.",
            ),
            ParamDescriptor::choice(SIGN, "Sign", &SIGNS, 0)
                .describe("Add moves notes up, Sub moves them down, Bi moves them either way."),
            ParamDescriptor::toggle(USE_SCALE, "Use Current Scale", false).describe(
                "Keeps moved notes in the clip's scale. It does nothing until scales arrive \
                 in Phase 6.",
            ),
        ]),
    )
}

/// Adds Dice to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, dice_type(), |device, _| Dice {
        router: Router::new(),
        rng: Rng::new(seed(device)),
        step: 0,
    });
}

/// `key` moved by `semitones`, folded by octaves back onto the keyboard.
fn folded(key: u8, semitones: i32) -> u8 {
    let mut at = i32::from(key) + semitones;
    while at > 127 {
        at -= 12;
    }
    while at < 0 {
        at += 12;
    }
    at as u8
}

struct Dice {
    router: Router,
    rng: Rng,
    /// How far Alt has gone through its distances.
    step: u32,
}

impl Dice {
    /// How far to move the next note, in semitones; 0 leaves it alone.
    fn distance(&mut self, params: &ParamBank) -> i32 {
        let chance = value(params, CHANCE);
        if self.rng.next_f64() * 100.0 >= chance {
            return 0;
        }
        let choices = value(params, CHOICES) as u32;
        let interval = value(params, INTERVAL) as i32;
        let sign = value(params, SIGN) as usize;
        let (k, up) = if value(params, MODE) as usize == 1 {
            // Alt: Bi alternates up and down through the distances.
            let n = self.step;
            self.step = self.step.wrapping_add(1);
            match sign {
                2 => ((n % (2 * choices)) / 2 + 1, n.is_multiple_of(2)),
                _ => (n % choices + 1, sign == 0),
            }
        } else {
            let k = self.rng.next_in_range(1, choices as i32) as u32;
            let up = match sign {
                2 => self.rng.next_u64() & 1 == 0,
                _ => sign == 0,
            };
            (k, up)
        };
        let moved = k as i32 * interval;
        if up { moved } else { -moved }
    }
}

impl NativeDevice for Dice {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        for note in audio.notes() {
            match note.kind {
                NoteKind::On => {
                    let key = folded(note.key, self.distance(params));
                    let made = Key {
                        key,
                        ..Key::of(&note)
                    };
                    if self.router.track(&note, Some(made)) {
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

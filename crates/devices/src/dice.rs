//! Random, the random-pitch MIDI effect (WP-5.30; the MIDI random pitch spec):
//! moves some notes by a random number of steps up, down or both, chosen
//! from a fixed set, or cycling through the set in order.
//!
//! Its look is the framework's generic layout: Beatlayer's own.

use bl_engine::midi_clips::Rng;
use bl_engine::{DeviceType, NoteBuffer, NoteEvent, NoteKind, Registry};
use bl_model::{DeviceDescription, DeviceRole, ParamDescriptor, ParamId, ParamUnit};

use crate::framework::{Audio, NativeDevice, ParamBank, add};
use crate::midi_fx::{Key, Router, scale_in_use, seed, value};

/// The Random device type's id.
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
/// Counts the distances in degrees of the device's current scale, so moved
/// notes land in it (the random-pitch spec, R10); the title bar draws it.
pub const USE_SCALE: ParamId = ParamId(5);

/// The Mode choices.
pub const MODES: [&str; 2] = ["Random", "Alt"];
/// The Sign choices.
pub const SIGNS: [&str; 3] = ["Add", "Sub", "Bi"];

/// Random's device type.
#[must_use]
pub fn dice_type() -> DeviceType {
    DeviceType::native(DICE, "Random", DeviceRole::MidiEffect).with_description(
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
                "Counts the distances in degrees of the current scale instead of semitones, so \
                 the notes that move land in the scale.",
            ),
        ])
        .with_title_switch(USE_SCALE),
    )
}

/// Adds Random to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, dice_type(), |device, _| Dice {
        router: Router::new(),
        rng: Rng::new(seed(device)),
        step: 0,
    });
}

/// The key `at`, folded by octaves back onto the keyboard.
fn folded(at: i32) -> u8 {
    match at {
        0..=127 => at as u8,
        128.. => ((at - 116).rem_euclid(12) + 116) as u8,
        _ => at.rem_euclid(12) as u8,
    }
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
                    // Spec: midi-random-pitch R10. A note that moves counts
                    // degrees from the scale note at or below it; one that
                    // stays is as it came.
                    let distance = self.distance(params);
                    let scale = scale_in_use(audio, params, USE_SCALE, note.offset);
                    let key = match (distance, scale.enabled) {
                        (0, _) => note.key,
                        (_, true) => folded(scale.land_off(note.key, distance)),
                        _ => folded(i32::from(note.key) + distance),
                    };
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
                // Spec: midi-random-pitch R11
                NoteKind::Expression { .. } => {
                    let scale = scale_in_use(audio, params, USE_SCALE, note.offset);
                    self.router
                        .expression_by(audio, &note, |from, to, bend| scale.bend(from, to, bend));
                }
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

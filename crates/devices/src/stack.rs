//! Stack, the chord MIDI effect (WP-5.30; the MIDI chord spec): adds up to six
//! notes above or below each note that comes in, each with its own velocity
//! and chance, and can strum them.
//!
//! Its look is the framework's generic layout: Beatlayer's own.

use bl_engine::midi_clips::Rng;
use bl_engine::{DeviceType, NoteBuffer, NoteEvent, NoteKind, Registry};
use bl_model::{DeviceDescription, DeviceRole, ParamDescriptor, ParamId, ParamUnit};

use crate::framework::{Audio, NativeDevice, ParamBank, add};
use crate::midi_fx::{Key, Router, seed, value};

/// The Stack device type's id.
pub const STACK: &str = "beatlayer.stack";

/// How many added notes there are.
pub const SHIFTS: u32 = 6;

/// Shift `n`'s distance from the note that came in (n from 0), in
/// semitones.
#[must_use]
pub const fn shift(n: u32) -> ParamId {
    ParamId(n)
}
/// Shift `n`'s velocity, as a share of the note's.
#[must_use]
pub const fn velocity(n: u32) -> ParamId {
    ParamId(SHIFTS + n)
}
/// Shift `n`'s chance of sounding.
#[must_use]
pub const fn chance(n: u32) -> ParamId {
    ParamId(2 * SHIFTS + n)
}
/// The time from the first note of the chord to the last, in milliseconds;
/// negative plays the chord from the top down.
pub const STRUM: ParamId = ParamId(3 * SHIFTS);
/// How the strum's speed changes along the chord.
pub const TENSION: ParamId = ParamId(3 * SHIFTS + 1);
/// How the velocity changes along the strummed chord.
pub const CRESCENDO: ParamId = ParamId(3 * SHIFTS + 2);
/// Sets the shifts from a chord played while it is on.
pub const LEARN: ParamId = ParamId(3 * SHIFTS + 3);
/// Reads the shifts in scale degrees (Phase 6).
pub const USE_SCALE: ParamId = ParamId(3 * SHIFTS + 4);
/// Lets a strum sound a key that is already sounding.
pub const DUPLICATES: ParamId = ParamId(3 * SHIFTS + 5);
/// Gives each added note its own ID and its source's expressions.
pub const PER_NOTE: ParamId = ParamId(3 * SHIFTS + 6);

/// Stack's device type.
#[must_use]
pub fn stack_type() -> DeviceType {
    let mut params = Vec::new();
    for n in 0..SHIFTS {
        let default = [4, 7, 0, 0, 0, 0][n as usize];
        params.push(
            ParamDescriptor::integer(
                shift(n),
                &format!("Shift {}", n + 1),
                (-36, 36),
                default,
                ParamUnit::Semitones,
            )
            .describe(
                "Adds a note this far above (or below) each note that comes in. 0, or a \
                 distance an earlier shift already uses, adds nothing.",
            ),
        );
    }
    for n in 0..SHIFTS {
        params.push(
            ParamDescriptor::continuous(
                velocity(n),
                &format!("Velocity {}", n + 1),
                (1.0, 200.0),
                100.0,
                ParamUnit::Percent,
            )
            .decimals(0)
            .describe("The velocity of this shift's note, as a share of the note that came in."),
        );
    }
    for n in 0..SHIFTS {
        params.push(
            ParamDescriptor::continuous(
                chance(n),
                &format!("Chance {}", n + 1),
                (0.0, 100.0),
                100.0,
                ParamUnit::Percent,
            )
            .decimals(0)
            .describe("How likely this shift's note is to sound."),
        );
    }
    params.extend([
        ParamDescriptor::continuous(
            STRUM,
            "Strum",
            (-400.0, 400.0),
            0.0,
            ParamUnit::Milliseconds,
        )
        .decimals(0)
        .describe(
            "Spreads the chord over this time, the note that came in first and then the \
                 shifts in order; negative plays them in the opposite order.",
        ),
        ParamDescriptor::continuous(TENSION, "Tension", (-100.0, 100.0), 0.0, ParamUnit::Percent)
            .decimals(0)
            .describe(
                "Positive starts the strum slowly and speeds up; negative does the opposite.",
            ),
        ParamDescriptor::continuous(
            CRESCENDO,
            "Crescendo",
            (-100.0, 100.0),
            0.0,
            ParamUnit::Percent,
        )
        .decimals(0)
        .describe("Ramps the velocity up (positive) or down (negative) along the strummed chord."),
        ParamDescriptor::toggle(LEARN, "Learn", false).describe(
            "While on, the first key of a chord you play sets the root and the other keys set \
             the shifts.",
        ),
        ParamDescriptor::toggle(USE_SCALE, "Use Current Scale", false).describe(
            "Reads the shifts in scale degrees. It does nothing until scales arrive in Phase 6.",
        ),
        ParamDescriptor::toggle(DUPLICATES, "Play Duplicate Notes", false)
            .describe("Lets a strum sound a key that is already sounding or waiting to sound."),
        ParamDescriptor::toggle(PER_NOTE, "Per-Note Events", false).describe(
            "Gives each added note its own ID, and sends it the expressions of the note that \
             made it.",
        ),
    ]);
    DeviceType::native(STACK, "Stack", DeviceRole::MidiEffect)
        .with_description(DeviceDescription::new(params))
}

/// Adds Stack to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, stack_type(), |device, _| Stack {
        router: Router::new(),
        rng: Rng::new(seed(device)),
    });
}

/// How far into a strum of `n` notes note `j` sounds, from 0 to 1, under
/// `tension` (−1 to 1): positive starts slowly and speeds up.
#[must_use]
pub fn strum_position(j: usize, n: usize, tension: f64) -> f64 {
    if n < 2 {
        return 0.0;
    }
    let u = j as f64 / (n - 1) as f64;
    1.0 - (1.0 - u).powf(2f64.powf(2.0 * tension))
}

/// The shifts a chord sets while Learn is on: `held` is the keys down in the
/// order they went down; the first is the root and the rest, up to six,
/// become the shifts in order, each as its distance from the root in
/// semitones. Shifts past the chord are 0.
#[must_use]
pub fn learn(held: &[u8]) -> [i32; SHIFTS as usize] {
    let mut shifts = [0; SHIFTS as usize];
    if let Some((&root, rest)) = held.split_first() {
        for (shift, &key) in shifts.iter_mut().zip(rest) {
            *shift = (i32::from(key) - i32::from(root)).clamp(-36, 36);
        }
    }
    shifts
}

struct Stack {
    router: Router,
    rng: Rng,
}

impl NativeDevice for Stack {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        let shifts: [i32; SHIFTS as usize] =
            core::array::from_fn(|n| value(params, shift(n as u32)) as i32);
        // A shift of 0, or a distance an earlier shift has, adds nothing.
        let used = |n: usize| shifts[n] != 0 && !shifts[..n].contains(&shifts[n]);
        let strum = value(params, STRUM);
        let tension = value(params, TENSION) / 100.0;
        let crescendo = value(params, CRESCENDO) / 100.0;
        let duplicates = value(params, DUPLICATES) >= 0.5;
        let per_note = value(params, PER_NOTE) >= 0.5;
        let rate = audio.sample_rate().as_f64();
        for note in audio.notes() {
            match note.kind {
                NoteKind::On => {
                    // The note that came in, then the shifts that sound.
                    let mut order = [None::<usize>; SHIFTS as usize + 1];
                    let mut count = 1;
                    for (n, &distance) in shifts.iter().enumerate() {
                        let sounds = used(n)
                            && self.rng.next_f64() * 100.0 < value(params, chance(n as u32))
                            && (0..=127).contains(&(i32::from(note.key) + distance));
                        if sounds {
                            order[count] = Some(n);
                            count += 1;
                        }
                    }
                    let slots = &mut order[..count];
                    if strum < 0.0 {
                        slots.reverse();
                    }
                    let source = Key::of(&note);
                    for (j, slot) in slots.iter().enumerate() {
                        let (key, share, made_id, follows) = match slot {
                            None => (note.key, 1.0, note.note_id, true),
                            Some(n) => (
                                (i32::from(note.key) + shifts[*n]) as u8,
                                value(params, velocity(*n as u32)) / 100.0,
                                per_note.then(|| self.router.fresh_id()),
                                per_note,
                            ),
                        };
                        // The note that came in always sounds; a shift may not
                        // double a key that is already sounding or waiting.
                        if strum != 0.0
                            && !duplicates
                            && slot.is_some()
                            && self.router.active(note.channel, key)
                        {
                            continue;
                        }
                        let ramp = if count > 1 {
                            1.0 + crescendo * (j as f64 / (count - 1) as f64 - 0.5)
                        } else {
                            1.0
                        };
                        let velocity = (f64::from(note.velocity) * share * ramp)
                            .clamp(1.0 / 127.0, 1.0) as f32;
                        let made = Key {
                            channel: note.channel,
                            key,
                            id: made_id,
                        };
                        if !self.router.track_with(&note, Some(made), follows) {
                            continue;
                        }
                        let sent = NoteEvent {
                            key,
                            velocity,
                            note_id: made_id,
                            ..note
                        };
                        let delay = (strum.abs() * strum_position(j, count, tension) * rate
                            / 1000.0)
                            .round() as i64;
                        if delay == 0 {
                            self.router.send_now(audio, sent, source);
                        } else {
                            let at = self.router.now() + i64::from(note.offset) + delay;
                            self.router.hold(at, sent, source);
                        }
                    }
                }
                NoteKind::Off | NoteKind::Choke => self.router.end(audio, &note, note.velocity),
                NoteKind::Expression { .. } => self.router.expression(audio, &note),
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

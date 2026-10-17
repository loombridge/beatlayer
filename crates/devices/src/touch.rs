//! Touch, the velocity MIDI effect (WP-5.30; the MIDI velocity spec): reshapes
//! the velocity of each note, or its release velocity, along a curve from an
//! input window to an output range.
//!
//! Its look is the framework's generic layout: Beatlayer's own.

use bl_engine::midi_clips::Rng;
use bl_engine::{DeviceType, NoteBuffer, NoteEvent, NoteKind, Registry};
use bl_model::{DeviceDescription, DeviceRole, ParamDescriptor, ParamId, ParamUnit};

use crate::framework::{Audio, NativeDevice, ParamBank, add};
use crate::midi_fx::{Key, Router, seed, value};

/// The Touch device type's id.
pub const TOUCH: &str = "beatlayer.touch";

/// The lowest input velocity of the curve.
pub const LOWEST: ParamId = ParamId(0);
/// How far the input window reaches above its lowest velocity.
pub const RANGE: ParamId = ParamId(1);
/// The output velocity at the bottom of the curve.
pub const OUT_LOW: ParamId = ParamId(2);
/// The output velocity at the top of the curve.
pub const OUT_HIGH: ParamId = ParamId(3);
/// Which velocities the curve is applied to.
pub const OPERATION: ParamId = ParamId(4);
/// What happens to a velocity outside the input window.
pub const MODE: ParamId = ParamId(5);
/// How far each velocity may move at random.
pub const RANDOM: ParamId = ParamId(6);
/// Bends the curve toward loud (positive) or soft (negative).
pub const DRIVE: ParamId = ParamId(7);
/// Stretches the curve toward its extremes (positive) or squeezes it toward
/// the middle (negative).
pub const COMPAND: ParamId = ParamId(8);

/// The Operation choices.
pub const OPERATIONS: [&str; 3] = ["Note On", "Release", "Both"];
/// The Mode choices.
pub const MODES: [&str; 3] = ["Clip", "Gate", "Fixed"];

/// Touch's device type.
#[must_use]
pub fn touch_type() -> DeviceType {
    DeviceType::native(TOUCH, "Touch", DeviceRole::MidiEffect).with_description(
        DeviceDescription::new(vec![
            ParamDescriptor::integer(LOWEST, "Lowest", (0, 127), 0, ParamUnit::None)
                .describe("The softest input velocity the curve starts from."),
            ParamDescriptor::integer(RANGE, "Range", (0, 127), 127, ParamUnit::None)
                .describe("How many velocity steps above Lowest the curve covers."),
            ParamDescriptor::integer(OUT_LOW, "Out Low", (0, 127), 0, ParamUnit::None)
                .describe("The output velocity at the bottom of the curve."),
            ParamDescriptor::integer(OUT_HIGH, "Out High", (0, 127), 127, ParamUnit::None)
                .describe("The output velocity at the top of the curve."),
            ParamDescriptor::choice(OPERATION, "Operation", &OPERATIONS, 0).describe(
                "Applies the curve to note-on velocity, to release velocity, or to both.",
            ),
            ParamDescriptor::choice(MODE, "Mode", &MODES, 0).describe(
                "Clip holds velocities outside the input window at its edges, Gate drops \
                 the notes outside it, and Fixed gives every note the Out High velocity.",
            ),
            ParamDescriptor::integer(RANDOM, "Random", (0, 127), 0, ParamUnit::None)
                .describe("How far each velocity may move at random, inside the output range."),
            ParamDescriptor::continuous(DRIVE, "Drive", (-100.0, 100.0), 0.0, ParamUnit::Percent)
                .decimals(0)
                .describe("Bends the curve so more notes come out loud (positive) or soft."),
            ParamDescriptor::continuous(
                COMPAND,
                "Compand",
                (-100.0, 100.0),
                0.0,
                ParamUnit::Percent,
            )
            .decimals(0)
            .describe(
                "Pushes outputs toward the top and bottom of the output range (positive) or \
                 toward its middle (negative).",
            ),
        ]),
    )
}

/// Adds Touch to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, touch_type(), |device, _| Touch {
        router: Router::new(),
        rng: Rng::new(seed(device)),
    });
}

/// The curve's shape: where an input at `x` (0 to 1 across the window)
/// lands (0 to 1 across the output range), under Drive and Compand (each
/// from −1 to 1).
#[must_use]
pub fn shape(x: f64, drive: f64, compand: f64) -> f64 {
    let driven = x.clamp(0.0, 1.0).powf(2f64.powf(-2.0 * drive));
    if compand >= 0.0 {
        driven + compand * (driven * driven * (3.0 - 2.0 * driven) - driven)
    } else {
        driven + -compand * (0.5 - driven)
    }
}

struct Touch {
    router: Router,
    rng: Rng,
}

/// One block's settings.
struct Curve {
    lowest: f64,
    highest: f64,
    out_low: f64,
    out_high: f64,
    mode: usize,
    random: f64,
    drive: f64,
    compand: f64,
}

impl Curve {
    /// The velocity (1 to 127) `velocity` (0 to 1) comes out as, or `None`
    /// if Gate drops it.
    fn apply(&self, velocity: f32, rng: &mut Rng) -> Option<f32> {
        // Whole steps, so a velocity of 64 / 127 isn't judged 63.99… by
        // Gate and Clip.
        let input = (f64::from(velocity) * 127.0).round();
        let out = if self.mode == 2 {
            self.out_high
        } else {
            if self.mode == 1 && (input < self.lowest || input > self.highest) {
                return None;
            }
            let span = self.highest - self.lowest;
            let x = if span > 0.0 {
                (input - self.lowest) / span
            } else {
                f64::from(input >= self.lowest)
            };
            let y = shape(x, self.drive, self.compand);
            self.out_low + y * (self.out_high - self.out_low)
        };
        let out = if self.random > 0.0 {
            let (low, high) = (
                self.out_low.min(self.out_high),
                self.out_low.max(self.out_high),
            );
            (out + (rng.next_f64() * 2.0 - 1.0) * self.random).clamp(low, high)
        } else {
            out
        };
        Some((out.round().clamp(1.0, 127.0) / 127.0) as f32)
    }
}

impl NativeDevice for Touch {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        let lowest = value(params, LOWEST);
        let curve = Curve {
            lowest,
            highest: (lowest + value(params, RANGE)).min(127.0),
            out_low: value(params, OUT_LOW),
            out_high: value(params, OUT_HIGH),
            mode: value(params, MODE) as usize,
            random: value(params, RANDOM),
            drive: value(params, DRIVE) / 100.0,
            compand: value(params, COMPAND) / 100.0,
        };
        let operation = value(params, OPERATION) as usize;
        for note in audio.notes() {
            match note.kind {
                NoteKind::On => {
                    let velocity = if operation == 1 {
                        Some(note.velocity)
                    } else {
                        curve.apply(note.velocity, &mut self.rng)
                    };
                    let made = velocity.map(|_| Key::of(&note));
                    if self.router.track(&note, made)
                        && let Some(velocity) = velocity
                    {
                        let shaped = NoteEvent { velocity, ..note };
                        self.router.send_now(audio, shaped, Key::of(&note));
                    }
                }
                NoteKind::Off | NoteKind::Choke => {
                    let release = if operation == 0 {
                        note.velocity
                    } else {
                        // Gate can't drop a release: it holds it at an edge.
                        let held = Curve { mode: 0, ..curve };
                        held.apply(note.velocity, &mut self.rng)
                            .unwrap_or(note.velocity)
                    };
                    self.router.end(audio, &note, release);
                }
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

//! Euclidean (the MIDI generators spec, R14 to R16): up to four voices, each
//! a pattern of hits spread as evenly as they go, rotated on its own.

use bl_core::time::Ticks;

use crate::descriptor::{
    ParamKind, ParamSpec, ParamValue, ToolDescriptor, ToolKind, division_ticks,
};
use crate::input::{ToolInput, ToolNote, ToolOutput};
use crate::random::Rng;
use crate::registry::{MAX_NOTES, MidiTool, ToolError};
use crate::value::{division, hold, pitch, switch, whole};

/// How many voices Euclidean has.
pub const VOICES: usize = 4;
/// The most steps a pattern has.
pub const MAX_STEPS: i64 = 32;
/// Where the voices' parameters start: Steps, Density and Division come
/// first, then a switch, a pitch, a Velocity and a Rotation for each voice.
const FIRST_VOICE: usize = 3;
const PER_VOICE: usize = 4;
/// The General MIDI kick, snare, closed and open hi-hat (R14).
const PITCHES: [u8; VOICES] = [36, 38, 42, 46];

pub(crate) struct Euclidean(ToolDescriptor);

/// The pattern of `steps` steps with `density` hits spread as evenly as they
/// go, moved `rotation` steps later (R15): step i holds a hit when
/// ((i − rotation) × density) modulo steps is less than density.
#[must_use]
pub fn euclid(steps: usize, density: usize, rotation: usize) -> Vec<bool> {
    let (n, d, r) = (steps as i64, density as i64, rotation as i64);
    (0..n).map(|i| ((i - r) * d).rem_euclid(n) < d).collect()
}

/// Every voice's Rotation for the randomise button (R16): a value from 0 to
/// `steps` − 1 for each, drawn from `seed`.
#[must_use]
pub fn random_rotations(steps: usize, seed: u32) -> [i64; VOICES] {
    let mut rng = Rng::new(seed);
    std::array::from_fn(|_| i64::from(rng.below(u32::try_from(steps).unwrap_or(1))))
}

/// The ID of voice `voice`'s (from 0) rotation parameter.
#[must_use]
pub fn rotation_id(voice: usize) -> String {
    format!("rotation_{}", voice + 1)
}

impl Euclidean {
    pub(crate) fn new() -> Self {
        let mut parameters = vec![
            ParamSpec::new(
                "steps",
                "Steps",
                ParamKind::Whole {
                    min: 1,
                    max: MAX_STEPS,
                },
                ParamValue::Whole(16),
                "How many steps the pattern has; it repeats across the selection.",
            ),
            ParamSpec::new(
                "density",
                "Density",
                ParamKind::Whole {
                    min: 0,
                    max: MAX_STEPS,
                },
                ParamValue::Whole(4),
                "How many of the steps hold a hit, spread as evenly as they go.",
            ),
            ParamSpec::new(
                "division",
                "Division",
                ParamKind::Division,
                ParamValue::Division(7),
                "How long a step is.",
            ),
        ];
        for (voice, pitch) in PITCHES.iter().enumerate() {
            let n = voice + 1;
            parameters.push(ParamSpec::new(
                &format!("voice_{n}"),
                &format!("Voice {n}"),
                ParamKind::Switch,
                ParamValue::Switch(voice == 0),
                "Whether the voice writes notes.",
            ));
            parameters.push(ParamSpec::new(
                &format!("pitch_{n}"),
                &format!("Voice {n} Pitch"),
                ParamKind::Pitch,
                ParamValue::Pitch(*pitch),
                "The key the voice writes on, or the Drum Rack pad.",
            ));
            parameters.push(ParamSpec::new(
                &format!("velocity_{n}"),
                &format!("Voice {n} Velocity"),
                ParamKind::Whole { min: 1, max: 127 },
                ParamValue::Whole(100),
                "The velocity of the voice's notes.",
            ));
            parameters.push(ParamSpec::new(
                &rotation_id(voice),
                &format!("Voice {n} Rotation"),
                ParamKind::Whole {
                    min: 0,
                    max: MAX_STEPS - 1,
                },
                ParamValue::Whole(0),
                "How many steps later the voice's pattern starts.",
            ));
        }
        Self(ToolDescriptor {
            id: "euclidean".to_owned(),
            name: "Euclidean".to_owned(),
            kind: ToolKind::Generator,
            parameters,
        })
    }
}

impl MidiTool for Euclidean {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.0
    }

    /// Density can't pass Steps, nor a Rotation pass the last step (R14).
    fn settle(
        &self,
        _before: &[ParamValue],
        after: &mut [ParamValue],
        changed: &[usize],
    ) -> Result<(), String> {
        let steps = whole(after, 0);
        hold(after, 1, (0, steps), changed, "density")?;
        for voice in 0..VOICES {
            let at = FIRST_VOICE + voice * PER_VOICE + 3;
            hold(after, at, (0, steps - 1), changed, &rotation_id(voice))?;
        }
        Ok(())
    }

    fn run(&self, input: &ToolInput, values: &[ParamValue]) -> Result<ToolOutput, ToolError> {
        let steps = usize::try_from(whole(values, 0)).unwrap_or(1).max(1);
        let density = usize::try_from(whole(values, 1)).unwrap_or(0).min(steps);
        let step = division_ticks(division(values, 2)).0;
        let (from, to) = (input.range.0.0, input.range.1.0);
        let count = usize::try_from((to - from + step - 1).max(0) / step).unwrap_or(0);
        let mut notes = Vec::new();
        for voice in 0..VOICES {
            let at = FIRST_VOICE + voice * PER_VOICE;
            if !switch(values, at) {
                continue;
            }
            let rotation = usize::try_from(whole(values, at + 3)).unwrap_or(0) % steps;
            let pattern = euclid(steps, density, rotation);
            let key = i16::from(input.nearest_key(pitch(values, at + 1)));
            let velocity = u8::try_from(whole(values, at + 2).clamp(1, 127)).unwrap_or(100);
            for n in (0..count).filter(|n| pattern[n % steps]) {
                notes.push(ToolNote::new(
                    key,
                    Ticks(from + n as i64 * step),
                    Ticks(step),
                    velocity,
                ));
                if notes.len() > MAX_NOTES {
                    return Err(ToolError::TooManyNotes(notes.len()));
                }
            }
        }
        Ok(ToolOutput { notes })
    }
}

//! Seed (the MIDI generators spec, R8 to R10): notes scattered at random
//! within ranges of pitch, length and velocity, never more at once than its
//! Voices.

use bl_core::time::Ticks;

use crate::descriptor::{ParamKind, ParamSpec, ParamValue, ToolDescriptor, ToolKind};
use crate::input::{ToolInput, ToolNote, ToolOutput};
use crate::random::Rng;
use crate::registry::{MAX_NOTES, MidiTool, ToolError};
use crate::value::{pitch_range, whole};

/// How many times a note that doesn't fit draws its start again (R10).
const REDRAWS: usize = 16;
/// The shortest and longest a note lasts, in 1/128 notes.
const SHORTEST: i64 = 1;
const LONGEST: i64 = 128;
/// Where the parameters are.
const DURATION: usize = 1;
const VELOCITY: usize = 3;
const VOICES: usize = 5;
const DENSITY: usize = 6;

pub(crate) struct Seed(ToolDescriptor);

/// A range's two ends as parameters, the lowest at most the highest (R8).
fn range(
    id: &str,
    name: &str,
    (min, max): (i64, i64),
    (low, high): (i64, i64),
    what: &str,
) -> [ParamSpec; 2] {
    let kind = ParamKind::Whole { min, max };
    [
        ParamSpec::new(
            &format!("{id}_min"),
            &format!("Minimum {name}"),
            kind.clone(),
            ParamValue::Whole(low),
            &format!("The least {what}."),
        ),
        ParamSpec::new(
            &format!("{id}_max"),
            &format!("Maximum {name}"),
            kind,
            ParamValue::Whole(high),
            &format!("The greatest {what}."),
        ),
    ]
}

impl Seed {
    pub(crate) fn new() -> Self {
        let mut parameters = vec![ParamSpec::new(
            "pitch_range",
            "Pitch Range",
            ParamKind::PitchRange,
            ParamValue::PitchRange { low: 48, high: 72 },
            "The lowest and highest key the notes are written on. The handles meet in one pitch.",
        )];
        parameters.extend(range(
            "duration",
            "Duration",
            (SHORTEST, LONGEST),
            (8, 32),
            "length of a note, in 1/128 notes",
        ));
        parameters.extend(range(
            "velocity",
            "Velocity",
            (1, 127),
            (64, 127),
            "velocity of a note",
        ));
        parameters.push(ParamSpec::new(
            "voices",
            "Voices",
            ParamKind::Whole { min: 1, max: 16 },
            ParamValue::Whole(4),
            "How many notes may sound at once.",
        ));
        parameters.push(
            ParamSpec::new(
                "density",
                "Density",
                ParamKind::Whole { min: 0, max: 100 },
                ParamValue::Whole(50),
                "How many notes are made, as a share of the pitch range filled.",
            )
            .unit("%"),
        );
        Self(ToolDescriptor {
            id: "seed".to_owned(),
            name: "Seed".to_owned(),
            kind: ToolKind::Generator,
            parameters,
        })
    }
}

/// A note kept so far: its start, end and key.
type Kept = (i64, i64, u8);

/// Whether a note from `start` to `end` on `key` fits among `kept`, which is
/// in order of start: no note of its key overlaps it, and no more than
/// `voices` sound at once with it (R10).
fn fits(kept: &[Kept], (start, end, key): Kept, voices: usize) -> bool {
    // A kept note lasts a whole note at most.
    let first = kept.partition_point(|k| k.0 <= start - Ticks::WHOLE.0);
    let last = kept.partition_point(|k| k.0 < end);
    let near: Vec<&Kept> = kept[first..last].iter().filter(|k| k.1 > start).collect();
    if near.iter().any(|k| k.2 == key) {
        return false;
    }
    if near.len() < voices {
        return true;
    }
    // The count of sounding notes only rises where a note starts.
    let mut edges: Vec<(i64, i32)> = near
        .iter()
        .flat_map(|k| [(k.0.max(start), 1), (k.1.min(end), -1)])
        .collect();
    edges.sort_unstable_by_key(|(at, change)| (*at, *change));
    let mut sounding = 0;
    edges.iter().all(|(_, change)| {
        sounding += change;
        sounding < i32::try_from(voices).unwrap_or(i32::MAX)
    })
}

impl MidiTool for Seed {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.0
    }

    /// A range's lowest stays at most its highest: the end that wasn't
    /// changed moves out of the way (R8).
    fn settle(
        &self,
        _before: &[ParamValue],
        after: &mut [ParamValue],
        changed: &[usize],
    ) -> Result<(), String> {
        for at in [DURATION, VELOCITY] {
            let (low, high) = (whole(after, at), whole(after, at + 1));
            if low > high {
                let (moved, kept) = if changed.contains(&at) {
                    (at + 1, at)
                } else {
                    (at, at + 1)
                };
                after[moved] = after[kept].clone();
            }
        }
        Ok(())
    }

    fn run(&self, input: &ToolInput, values: &[ParamValue]) -> Result<ToolOutput, ToolError> {
        let (from, to) = (input.range.0.0, input.range.1.0);
        let (low, high) = pitch_range(values, 0);
        let keys: Vec<u8> = (low..=high).filter(|key| input.allows(*key)).collect();
        let whole_note = Ticks::WHOLE.0;
        // Density times the keys it may write, for each whole note (R9).
        let count = (whole(values, DENSITY) * keys.len() as i64 * (to - from).max(0)
            + whole_note * 50)
            .div_euclid(whole_note * 100);
        let count = if keys.is_empty() { 0 } else { count };
        if count > MAX_NOTES as i64 {
            return Err(ToolError::TooManyNotes(count as usize));
        }
        let grid = input.grid_step.0.max(1);
        let starts = u32::try_from((to - from + grid - 1) / grid).unwrap_or(u32::MAX);
        let (shortest, longest) = (whole(values, DURATION), whole(values, DURATION + 1));
        let (quietest, loudest) = (whole(values, VELOCITY), whole(values, VELOCITY + 1));
        let voices = usize::try_from(whole(values, VOICES)).unwrap_or(1);
        let draw = |rng: &mut Rng, low: i64, high: i64| {
            low + i64::from(rng.below((high - low + 1) as u32))
        };
        let mut rng = Rng::new(input.seed);
        let mut kept: Vec<Kept> = Vec::new();
        let mut notes = Vec::new();
        for _ in 0..count {
            let key = keys[rng.below(keys.len() as u32) as usize];
            let length = draw(&mut rng, shortest, longest) * whole_note / 128;
            let velocity = draw(&mut rng, quietest, loudest) as u8;
            for _ in 0..=REDRAWS {
                let start = from + i64::from(rng.below(starts)) * grid;
                let span = (start, (start + length).min(to), key);
                if fits(&kept, span, voices) {
                    kept.insert(kept.partition_point(|k| k.0 <= start), span);
                    notes.push(ToolNote::new(
                        i16::from(key),
                        Ticks(start),
                        Ticks(span.1 - start),
                        velocity,
                    ));
                    break;
                }
            }
        }
        Ok(ToolOutput { notes })
    }
}

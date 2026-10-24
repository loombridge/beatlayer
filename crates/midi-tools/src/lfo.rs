//! LFO (the MIDI transformations spec, R39 to R42): writes a low-frequency
//! wave into each note's Pitch, Slide or Pressure envelope.

use std::f64::consts::TAU;

use bl_core::time::Ticks;

use crate::descriptor::{
    Breakpoint, ParamKind, ParamSpec, ParamValue, ToolDescriptor, ToolKind, division_ticks,
};
use crate::input::{ToolInput, ToolNote, ToolOutput, ToolPoint};
use crate::random::Rng;
use crate::registry::{MidiTool, ToolError};
use crate::value::{choice, options};

/// A note's per-note pitch range, in semitones (the MPE spec, R1).
const RANGE: f64 = 48.0;

/// Breakpoints in one period of the wave, and the most in one note (R42).
const PER_PERIOD: i64 = 32;
const MAX_POINTS: i64 = 512;

/// Where each Rate choice sits in the note divisions: 1 to 1/128, no triplets.
const RATES: [usize; 8] = [0, 1, 3, 5, 7, 9, 11, 12];

/// The parameters' places.
const TARGET: usize = 0;
const SHAPE_TYPE: usize = 1;
const SHAPE: usize = 2;
const RATE: usize = 3;
const TIME_SHIFT: usize = 4;
const AMOUNT: usize = 5;
const BASE: usize = 6;
const ATTACK: usize = 7;
const DECAY: usize = 8;

/// The Random wave.
const RANDOM: usize = 3;

pub(crate) struct Lfo(ToolDescriptor);

impl Lfo {
    pub(crate) fn new() -> Self {
        let percent = |id, name, default, description| {
            ParamSpec::new(
                id,
                name,
                ParamKind::Whole {
                    min: -100,
                    max: 100,
                },
                ParamValue::Whole(default),
                description,
            )
            .unit("%")
        };
        let length = |id, name, description| {
            ParamSpec::new(
                id,
                name,
                ParamKind::Whole { min: 0, max: 100 },
                ParamValue::Whole(0),
                description,
            )
            .unit("%")
        };
        let semitones_or_value = |id, name, description| {
            ParamSpec::new(
                id,
                name,
                ParamKind::Number {
                    min: -127.0,
                    max: 127.0,
                    decimals: 1,
                },
                ParamValue::Number(0.0),
                description,
            )
        };
        Self(ToolDescriptor {
            id: "lfo".to_owned(),
            name: "LFO".to_owned(),
            kind: ToolKind::Transformation,
            parameters: vec![
                ParamSpec::new(
                    "target",
                    "Target",
                    ParamKind::Choice {
                        options: options(&[
                            ("pitch_bend", "Pitch Bend"),
                            ("slide", "Slide"),
                            ("pressure", "Pressure"),
                        ]),
                    },
                    ParamValue::Choice(0),
                    "The envelope of each note the wave replaces.",
                ),
                ParamSpec::new(
                    "shape_type",
                    "Shape Type",
                    ParamKind::Choice {
                        options: options(&[
                            ("sine", "Sine"),
                            ("square", "Square"),
                            ("triangle", "Triangle"),
                            ("random", "Random"),
                        ]),
                    },
                    ParamValue::Choice(0),
                    "The wave: sine, square, triangle, or a random value for each cycle.",
                ),
                percent(
                    "shape",
                    "Shape",
                    0,
                    "Moves the wave's turning point earlier or later in each cycle.",
                ),
                ParamSpec::new(
                    "rate",
                    "Rate",
                    ParamKind::Choice {
                        options: options(&[
                            ("1", "1"),
                            ("1_2", "1/2"),
                            ("1_4", "1/4"),
                            ("1_8", "1/8"),
                            ("1_16", "1/16"),
                            ("1_32", "1/32"),
                            ("1_64", "1/64"),
                            ("1_128", "1/128"),
                        ]),
                    },
                    ParamValue::Choice(3),
                    "The length of one cycle of the wave.",
                ),
                percent(
                    "time_shift",
                    "Time Shift",
                    0,
                    "Delays the wave's start by this share of a cycle; a negative value starts it part of the way through.",
                ),
                semitones_or_value(
                    "amount",
                    "Amount",
                    "How far the wave swings: in semitones for Pitch Bend, in values from 0 to 127 for Slide and Pressure.",
                ),
                semitones_or_value(
                    "amplitude_base",
                    "Amplitude Base",
                    "The value the wave swings around.",
                ),
                length(
                    "attack",
                    "Attack",
                    "How much of the note's start the wave takes to rise to its full amount.",
                ),
                length(
                    "decay",
                    "Decay",
                    "How much of the note's end the wave takes to fall to nothing.",
                ),
            ],
        })
    }
}

/// How far Amount and Amplitude Base reach with this Target (R39).
fn limits(target: usize) -> (f64, f64) {
    if target == 0 {
        (2.0 * RANGE, RANGE)
    } else {
        (127.0, 127.0)
    }
}

/// The values the target's envelope holds (R40).
fn range(target: usize) -> (f64, f64) {
    if target == 0 {
        (-RANGE, RANGE)
    } else {
        (0.0, 127.0)
    }
}

fn number(values: &[ParamValue], at: usize) -> f64 {
    match values.get(at) {
        Some(ParamValue::Number(n)) => *n,
        _ => 0.0,
    }
}

fn percent_of(values: &[ParamValue], at: usize) -> f64 {
    match values.get(at) {
        Some(ParamValue::Whole(n)) => *n as f64 / 100.0,
        _ => 0.0,
    }
}

impl MidiTool for Lfo {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.0
    }

    fn settle(
        &self,
        _before: &[ParamValue],
        after: &mut [ParamValue],
        changed: &[usize],
    ) -> Result<(), String> {
        let (amount, base) = limits(choice(after, TARGET));
        let (new_amount, new_base) = (number(after, AMOUNT), number(after, BASE));
        if changed.contains(&TARGET) {
            // A new Target keeps the numbers, held within its ranges.
            after[AMOUNT] = ParamValue::Number(new_amount.clamp(-amount, amount));
            after[BASE] = ParamValue::Number(new_base.clamp(-base, base));
        } else if new_amount.abs() > amount || new_base.abs() > base {
            return Err(format!(
                "with this Target, Amount takes -{amount} to {amount} and Amplitude Base -{base} to {base}"
            ));
        }
        // Attack and Decay together are at most 100 %: the one just set
        // lowers the other.
        let sum = |after: &[ParamValue]| percent_of(after, ATTACK) + percent_of(after, DECAY);
        if sum(after) > 1.0 {
            let (set, other) = match (changed.contains(&ATTACK), changed.contains(&DECAY)) {
                (true, false) => (ATTACK, DECAY),
                (false, true) => (DECAY, ATTACK),
                _ => return Err("Attack and Decay together are at most 100 %".to_owned()),
            };
            after[other] = ParamValue::Whole(100 - (percent_of(after, set) * 100.0).round() as i64);
        }
        Ok(())
    }

    fn run(&self, input: &ToolInput, values: &[ParamValue]) -> Result<ToolOutput, ToolError> {
        let target = choice(values, TARGET);
        let (amount_limit, base_limit) = limits(target);
        let (amount, base) = (number(values, AMOUNT), number(values, BASE));
        if amount.abs() > amount_limit || base.abs() > base_limit {
            return Err(ToolError::Values(
                "this Target's Amount and Amplitude Base are out of range".to_owned(),
            ));
        }
        let wave = Wave {
            kind: choice(values, SHAPE_TYPE),
            // R41: m, the cycle's turning point.
            turn: ((1.0 + percent_of(values, SHAPE)) / 2.0).clamp(0.05, 0.95),
            period: division_ticks(RATES[choice(values, RATE).min(RATES.len() - 1)]).0 as f64,
            shift: percent_of(values, TIME_SHIFT),
        };
        let (low, high) = range(target);
        let mut rng = Rng::new(input.seed);
        let notes = input
            .notes
            .iter()
            .map(|note| {
                let length = note.length.0;
                let attack = percent_of(values, ATTACK) * length as f64;
                let decay = percent_of(values, DECAY) * length as f64;
                let randoms = wave.randoms(length, &mut rng);
                // R42: 32 breakpoints a period, at most 512, from the start to the end.
                let period = wave.period as i64;
                let segments = ((length * PER_PERIOD + period - 1) / period)
                    .min(MAX_POINTS - 1)
                    .min(length)
                    .max(1);
                let mut points: Vec<ToolPoint> = Vec::new();
                for k in 0..=segments {
                    let at = (length * k + segments / 2) / segments;
                    let t = at as f64;
                    let mut fade = 1.0;
                    if attack > 0.0 && t < attack {
                        fade = t / attack;
                    }
                    if decay > 0.0 && t > length as f64 - decay {
                        fade = fade.min((length as f64 - t) / decay);
                    }
                    let value = (base + amount * fade * wave.at(t, &randoms)).clamp(low, high);
                    if points.last().is_none_or(|last| last.at.0 < at) {
                        points.push(ToolPoint {
                            at: Ticks(at),
                            value,
                            bend: 0.0,
                        });
                    }
                }
                write(note, target, points)
            })
            .collect();
        Ok(ToolOutput { notes })
    }
}

/// What LFO's curve display draws for these settings (R43): the envelope
/// the tool writes over a note of two periods, with `x` from 0 to 1 along
/// the note and `y` from 0 to 1 across the target's range. Random draws
/// from seed 0.
#[must_use]
pub fn lfo_curve(values: &[ParamValue]) -> Vec<Breakpoint> {
    let target = choice(values, TARGET);
    let length = 2 * division_ticks(RATES[choice(values, RATE).min(RATES.len() - 1)]).0;
    let input = ToolInput {
        notes: vec![ToolNote::new(60, Ticks::ZERO, Ticks(length), 100)],
        range: (Ticks::ZERO, Ticks(length)),
        grid_step: Ticks(length),
        signature: bl_core::time::TimeSignature::FOUR_FOUR,
        scale: None,
        drum_keys: None,
        seed: 0,
    };
    let Ok(out) = Lfo::new().run(&input, values) else {
        return Vec::new();
    };
    let (low, high) = range(target);
    let note = &out.notes[0];
    let envelope = match target {
        0 => &note.pitch,
        1 => &note.slide,
        _ => &note.pressure,
    };
    envelope
        .iter()
        .map(|p| Breakpoint {
            x: p.at.0 as f64 / length as f64,
            y: (p.value - low) / (high - low),
        })
        .collect()
}

/// `note` with `points` as the envelope `target` names (R42).
fn write(note: &ToolNote, target: usize, points: Vec<ToolPoint>) -> ToolNote {
    let mut note = note.clone();
    match target {
        0 => note.pitch = points,
        1 => note.slide = points,
        _ => note.pressure = points,
    }
    note
}

/// The wave of R40 and R41.
struct Wave {
    kind: usize,
    turn: f64,
    period: f64,
    shift: f64,
}

impl Wave {
    /// Where in the wave's cycles a time `t` from the note's start falls: the
    /// cycle's number and the way through it, from 0 to 1; none while a
    /// positive Time Shift holds the wave back (R40).
    fn cycle(&self, t: f64) -> Option<(usize, f64)> {
        let cycles = if self.shift >= 0.0 {
            if t < self.shift * self.period {
                return None;
            }
            (t - self.shift * self.period) / self.period
        } else {
            t / self.period - self.shift
        };
        Some((cycles.floor() as usize, cycles.fract()))
    }

    /// Random's value for each cycle a note of `length` ticks reaches, and
    /// the one after (R41).
    fn randoms(&self, length: i64, rng: &mut Rng) -> Vec<f64> {
        if self.kind != RANDOM {
            return Vec::new();
        }
        let last = self.cycle(length as f64).map_or(0, |(n, _)| n);
        (0..=last + 1)
            .map(|_| rng.unit().mul_add(2.0, -1.0))
            .collect()
    }

    /// The wave at a time `t` from the note's start, from -1 to 1.
    fn at(&self, t: f64, randoms: &[f64]) -> f64 {
        let Some((n, x)) = self.cycle(t) else {
            return 0.0;
        };
        let m = self.turn;
        match self.kind {
            1 => {
                if x < m {
                    1.0
                } else {
                    -1.0
                }
            }
            RANDOM => {
                let here = randoms.get(n).copied().unwrap_or(0.0);
                if x < m {
                    here
                } else {
                    let next = randoms.get(n + 1).copied().unwrap_or(here);
                    here + (next - here) * (x - m) / (1.0 - m)
                }
            }
            kind => {
                let y = if x < m {
                    x / (2.0 * m)
                } else {
                    0.5 + (x - m) / (2.0 * (1.0 - m))
                };
                if kind == 0 {
                    (TAU * y).sin()
                } else if y <= 0.5 {
                    4.0 * y - 1.0
                } else {
                    3.0 - 4.0 * y
                }
            }
        }
    }
}

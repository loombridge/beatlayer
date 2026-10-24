//! Arpeggiate (the MIDI transformations spec, R3 to R6): turns held notes
//! into arpeggios in Arpeggiator's styles.

use bl_core::time::Ticks;

use crate::descriptor::{ParamKind, ParamSpec, ParamValue, ToolDescriptor, ToolKind, division_ticks};
use crate::input::{ToolInput, ToolNote, ToolOutput};
use crate::random::Rng;
use crate::registry::{MAX_NOTES, MidiTool, ToolError};
use crate::style::{
    CHORD_TRIGGER, PLAY_ORDER, RANDOM, RANDOM_ONCE, RANDOM_OTHER, STYLE_NAMES, style_order,
};
use crate::value::{choice, division, options, percent, whole};

pub(crate) struct Arpeggiate(ToolDescriptor);

/// The ID of a style: its name in lower case, words joined by underscores.
fn style_id(name: &str) -> String {
    name.replace(" & ", " and ")
        .replace(' ', "_")
        .to_lowercase()
}

impl Arpeggiate {
    pub(crate) fn new() -> Self {
        let styles: Vec<(String, &str)> = STYLE_NAMES
            .iter()
            .map(|name| (style_id(name), *name))
            .collect();
        let pairs: Vec<(&str, &str)> = styles.iter().map(|(id, n)| (id.as_str(), *n)).collect();
        Self(ToolDescriptor {
            id: "arpeggiate".to_owned(),
            name: "Arpeggiate".to_owned(),
            kind: ToolKind::Transformation,
            parameters: vec![
                ParamSpec::new(
                    "style",
                    "Style",
                    ParamKind::Choice {
                        options: options(&pairs),
                    },
                    ParamValue::Choice(0),
                    "The order the held notes are played in, as Arpeggiator's Style chooses it.",
                ),
                ParamSpec::new(
                    "distance",
                    "Distance",
                    ParamKind::Whole { min: -48, max: 48 },
                    ParamValue::Whole(12),
                    "How far each repeat of the pattern is transposed, in scale degrees where the clip has a scale, else in semitones.",
                )
                .scale_aware(),
                ParamSpec::new(
                    "steps",
                    "Steps",
                    ParamKind::Whole { min: 0, max: 8 },
                    ParamValue::Whole(0),
                    "How many transposed repeats the pattern goes through before starting again.",
                ),
                ParamSpec::new(
                    "rate",
                    "Rate",
                    ParamKind::Division,
                    ParamValue::Division(7),
                    "The time between the arpeggio's notes.",
                ),
                ParamSpec::new(
                    "gate",
                    "Gate",
                    ParamKind::Whole { min: 1, max: 200 },
                    ParamValue::Whole(100),
                    "How long each note lasts as a share of Rate; above 100 % a note overlaps the next.",
                )
                .unit("%"),
            ],
        })
    }
}

/// The notes of a group that sound at `at`, in the order the style counts
/// them: lowest key first, or earliest start first for Play Order.
fn sounding(notes: &[ToolNote], group: &[usize], at: Ticks, play_order: bool) -> Vec<usize> {
    let mut held: Vec<usize> = group
        .iter()
        .copied()
        .filter(|i| notes[*i].start <= at && notes[*i].end() > at)
        .collect();
    if play_order {
        held.sort_by_key(|i| (notes[*i].start, notes[*i].key));
    } else {
        held.sort_by_key(|i| (notes[*i].key, notes[*i].start));
    }
    held
}

impl MidiTool for Arpeggiate {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.0
    }

    fn run(&self, input: &ToolInput, values: &[ParamValue]) -> Result<ToolOutput, ToolError> {
        let style = choice(values, 0);
        let distance = i32::try_from(whole(values, 1)).unwrap_or(0);
        let steps = whole(values, 2);
        let rate = division_ticks(division(values, 3));
        let length = Ticks(percent(rate.0, whole(values, 4)));
        let mut rng = Rng::new(input.seed);
        let notes = &input.notes;
        // R4: groups of notes that overlap one another in the clip's order.
        let mut groups: Vec<(Vec<usize>, Ticks)> = Vec::new();
        for (at, note) in notes.iter().enumerate() {
            match groups.last_mut() {
                Some((group, end)) if note.start < *end => {
                    group.push(at);
                    *end = (*end).max(note.end());
                }
                _ => groups.push((vec![at], note.end())),
            }
        }
        let mut written = Vec::new();
        let (mut order, mut pool, mut once) = (Vec::new(), Vec::new(), Vec::new());
        for (group, end) in &groups {
            let first = group.iter().map(|i| notes[*i].start).min().unwrap_or(*end);
            let (mut place, mut cycle, mut last) = (0usize, 0i64, None::<usize>);
            once.clear();
            let mut at = first;
            while at < *end && written.len() <= MAX_NOTES {
                let held = sounding(notes, group, at, style == PLAY_ORDER);
                let k = held.len();
                if k > 0 {
                    let mut chosen: &[usize] = &held;
                    let mut single = [0usize];
                    let mut places = k;
                    if style == CHORD_TRIGGER {
                        places = 1;
                    } else {
                        if style == RANDOM {
                            single[0] = held[rng.below(k as u32) as usize];
                        } else if style == RANDOM_OTHER {
                            // Any held note but the step before's, while more than one is held.
                            let skip = last.and_then(|l| held.iter().position(|h| *h == l));
                            let mut pick = rng.below((k - usize::from(skip.is_some() && k > 1)) as u32) as usize;
                            if skip.is_some_and(|s| k > 1 && pick >= s) {
                                pick += 1;
                            }
                            single[0] = held[pick];
                        } else {
                            if style == RANDOM_ONCE {
                                if once.len() != k {
                                    style_order(style, k, &mut once, &mut pool, &mut |n| {
                                        rng.below(n as u32) as usize
                                    });
                                }
                                order.clone_from(&once);
                            } else {
                                style_order(style, k, &mut order, &mut pool, &mut |_| 0);
                            }
                            if place >= order.len() {
                                place %= order.len();
                            }
                            single[0] = held[order[place]];
                        }
                        chosen = &single;
                        if style == RANDOM || style == RANDOM_OTHER {
                            last = Some(single[0]);
                        } else {
                            places = order.len();
                        }
                    }
                    let moved = i32::try_from(cycle).unwrap_or(0).saturating_mul(distance);
                    for i in chosen {
                        let source = &notes[*i];
                        let key = u8::try_from(source.key.clamp(0, 127)).unwrap_or(0);
                        let from = at - source.start;
                        let to = (at + length).min(source.end()) - source.start;
                        // R6: the step's note over its held note's span carries that stretch of its envelopes.
                        let mut out = source.part(from, to.max(from + Ticks(1)));
                        out.key = input.stepped(key, moved);
                        out.start = at;
                        out.length = length;
                        written.push(out);
                    }
                    place += 1;
                    if place >= places {
                        place = 0;
                        cycle = if cycle + 1 > steps { 0 } else { cycle + 1 };
                    }
                }
                at += rate;
            }
        }
        Ok(ToolOutput { notes: written })
    }
}

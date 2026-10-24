//! Stacks (the MIDI generators spec, R17 to R23): a progression of chords
//! from a chord bank, one after another across the range.
//!
//! The progression is a structured value, a JSON list of chords; the
//! selected chord's settings are also parameters of their own, which
//! [`MidiTool::settle`] keeps in step with it, so a control or an agent sets
//! the selected chord's root with a plain `midi_tool.set`.

use bl_core::time::Ticks;
use serde_json::Value;

use crate::bank::{self, ChordBank, ChordEntry};
use crate::descriptor::{ParamKind, ParamSpec, ParamValue, ToolDescriptor, ToolKind};
use crate::input::{ToolInput, ToolNote, ToolOutput, fold_octaves};
use crate::registry::{MidiTool, ToolError};
use crate::value::{pitch, whole};

/// The most chords a progression holds (R17).
pub const MAX_CHORDS: usize = 16;
/// The notes of the major scale on the root, counted by a degree entry while
/// Scale Mode is off (R19).
const MAJOR: [u8; 7] = [0, 2, 4, 5, 7, 9, 11];
const ROOT_KEY: u8 = 60;

// Where the parameters sit.
const PROGRESSION: usize = 0;
const SELECTED: usize = 1;
const CHORD: usize = 2;
const ROOT: usize = 3;
const INVERSION: usize = 4;
const DURATION: usize = 5;
const OFFSET: usize = 6;
const BANK: usize = 7;

/// One chord of the progression (R17).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Chord {
    /// Which entry of the bank.
    pub chord: usize,
    /// The root key.
    pub root: u8,
    /// The inversion, −(n − 1) to n − 1 for a chord of n notes.
    pub inversion: i64,
    /// How many eighths of its slot the chord lasts, 1 to 8.
    pub duration: i64,
    /// How many eighths of its slot after the slot's start the chord
    /// starts, 0 to 7.
    pub offset: i64,
}

impl Default for Chord {
    fn default() -> Self {
        Self {
            chord: 0,
            root: ROOT_KEY,
            inversion: 0,
            duration: 8,
            offset: 0,
        }
    }
}

/// Stacks' progression, its selected chord and its bank, as the panel and
/// `midi_tool.state` show them (R28).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StackView {
    /// The progression, 1 to [`MAX_CHORDS`] chords.
    pub chords: Vec<Chord>,
    /// The selected chord's place.
    pub selected: usize,
    /// The bank.
    pub bank: ChordBank,
    /// Whether the bank is Beatlayer's built-in one.
    pub built_in: bool,
}

pub(crate) struct Stacks(ToolDescriptor);

impl Stacks {
    pub(crate) fn new() -> Self {
        let chord = Chord::default();
        let whole_of = |id: &str, name: &str, min, max, default, text: &str| {
            ParamSpec::new(
                id,
                name,
                ParamKind::Whole { min, max },
                ParamValue::Whole(default),
                text,
            )
        };
        Self(ToolDescriptor {
            id: "stacks".to_owned(),
            name: "Stacks".to_owned(),
            kind: ToolKind::Generator,
            parameters: vec![
                ParamSpec::new(
                    "progression",
                    "Progression",
                    ParamKind::Structured,
                    ParamValue::Structured(write_progression(&[chord])),
                    "The chords written one after another, 1 to 16: a list of objects with \
                     `chord` (the bank entry's place), `root`, `inversion`, `duration` and \
                     `offset`. The selected chord's settings are the parameters below.",
                ),
                whole_of(
                    "selected",
                    "Selected Chord",
                    0,
                    MAX_CHORDS as i64 - 1,
                    0,
                    "The place of the chord the controls below show.",
                ),
                whole_of(
                    "chord",
                    "Chord",
                    0,
                    bank::MAX_ENTRIES as i64 - 1,
                    0,
                    "The selected chord's entry in the bank, by its place.",
                ),
                ParamSpec::new(
                    "chord_root",
                    "Chord Root",
                    ParamKind::Pitch,
                    ParamValue::Pitch(ROOT_KEY),
                    "The selected chord's root key; with a clip scale, a key of the scale.",
                ),
                whole_of(
                    "chord_inversion",
                    "Chord Inversion",
                    -(bank::MAX_CHORD_NOTES as i64 - 1),
                    bank::MAX_CHORD_NOTES as i64 - 1,
                    0,
                    "Steps through the selected chord's inversions; a negative value is the \
                     same inversion an octave lower.",
                ),
                whole_of(
                    "chord_duration",
                    "Chord Duration",
                    1,
                    8,
                    8,
                    "How long the selected chord lasts, in eighths of its slot.",
                )
                .unit("/8"),
                whole_of(
                    "chord_offset",
                    "Chord Offset",
                    0,
                    7,
                    0,
                    "How late the selected chord starts, in eighths of its slot.",
                )
                .unit("/8"),
                ParamSpec::new(
                    "bank",
                    "Chord Bank",
                    ParamKind::Structured,
                    ParamValue::Structured(String::new()),
                    "The chord bank, as a chord bank file holds it; empty for Beatlayer's \
                     built-in bank. `midi_tool.load_bank` loads one from a file.",
                ),
            ],
        })
    }
}

// The progression as text.

fn write_progression(chords: &[Chord]) -> String {
    let list: Vec<String> = chords
        .iter()
        .map(|c| {
            format!(
                "{{\"chord\":{},\"root\":{},\"inversion\":{},\"duration\":{},\"offset\":{}}}",
                c.chord, c.root, c.inversion, c.duration, c.offset
            )
        })
        .collect();
    format!("[{}]", list.join(","))
}

fn parse_progression(text: &str) -> Result<Vec<Chord>, String> {
    let value: Value =
        serde_json::from_str(text).map_err(|error| format!("`progression` isn't JSON: {error}"))?;
    let list = value
        .as_array()
        .ok_or("`progression` must be a list of chords")?;
    if list.is_empty() || list.len() > MAX_CHORDS {
        return Err(format!(
            "`progression` holds {} chords; it takes 1 to {MAX_CHORDS}",
            list.len()
        ));
    }
    let default = Chord::default();
    list.iter()
        .enumerate()
        .map(|(at, chord)| {
            let field = |name: &str, default: i64, min: i64, max: i64| -> Result<i64, String> {
                let Some(found) = chord.get(name) else {
                    return Ok(default);
                };
                found
                    .as_i64()
                    .filter(|n| (min..=max).contains(n))
                    .ok_or_else(|| {
                        format!("chord {at} of `progression`: `{name}` goes from {min} to {max}")
                    })
            };
            if !chord.is_object() {
                return Err(format!("chord {at} of `progression` must be an object"));
            }
            Ok(Chord {
                chord: usize::try_from(field("chord", 0, 0, bank::MAX_ENTRIES as i64 - 1)?)
                    .unwrap_or(0),
                root: u8::try_from(field("root", i64::from(default.root), 0, 127)?).unwrap_or(0),
                inversion: field("inversion", 0, -7, 7)?,
                duration: field("duration", default.duration, 1, 8)?,
                offset: field("offset", 0, 0, 7)?,
            })
        })
        .collect()
}

fn text(values: &[ParamValue], at: usize) -> &str {
    match values.get(at) {
        Some(ParamValue::Structured(text)) => text,
        _ => "",
    }
}

/// The bank the values hold: an empty text is the built-in bank.
fn bank_of(values: &[ParamValue]) -> Result<ChordBank, String> {
    match text(values, BANK) {
        "" => Ok(bank::built_in().clone()),
        text => ChordBank::parse(text.as_bytes()).map_err(|error| format!("`bank`: {error}")),
    }
}

// The settings as a whole.

struct State {
    chords: Vec<Chord>,
    selected: usize,
    bank: ChordBank,
}

impl State {
    fn read(values: &[ParamValue]) -> Result<Self, String> {
        let chords = parse_progression(text(values, PROGRESSION))?;
        let selected = usize::try_from(whole(values, SELECTED)).unwrap_or(0);
        if selected >= chords.len() {
            return Err(format!(
                "`selected` goes from 0 to {}, the last chord",
                chords.len() - 1
            ));
        }
        Ok(Self {
            chords,
            selected,
            bank: bank_of(values)?,
        })
    }

    /// Checks every chord against the bank.
    fn check(&self) -> Result<(), String> {
        for (at, chord) in self.chords.iter().enumerate() {
            let entry = self.entry(chord).ok_or_else(|| {
                format!(
                    "chord {at} names bank entry {}; the bank holds {}",
                    chord.chord,
                    self.bank.chords.len()
                )
            })?;
            let most = entry.intervals.len() as i64 - 1;
            if chord.inversion.abs() > most {
                return Err(format!(
                    "chord {at}: `inversion` goes from {} to {most} for {}",
                    -most, entry.name
                ));
            }
        }
        Ok(())
    }

    fn entry(&self, chord: &Chord) -> Option<&ChordEntry> {
        self.bank.chords.get(chord.chord)
    }

    /// Writes the progression, the selection and the selected chord's
    /// settings into `values`.
    fn write(&self, values: &mut [ParamValue]) {
        let chord = self.chords[self.selected];
        values[PROGRESSION] = ParamValue::Structured(write_progression(&self.chords));
        values[SELECTED] = ParamValue::Whole(self.selected as i64);
        values[CHORD] = ParamValue::Whole(chord.chord as i64);
        values[ROOT] = ParamValue::Pitch(chord.root);
        values[INVERSION] = ParamValue::Whole(chord.inversion);
        values[DURATION] = ParamValue::Whole(chord.duration);
        values[OFFSET] = ParamValue::Whole(chord.offset);
    }

    /// Takes a chord's entry and inversion back inside the bank: a place past
    /// its end takes the first entry (R23), and an inversion past the
    /// chord's notes the last it has.
    fn fit_bank(&mut self) {
        let bank = &self.bank;
        for chord in &mut self.chords {
            if chord.chord >= bank.chords.len() {
                chord.chord = 0;
            }
            let most = bank.chords[chord.chord].intervals.len() as i64 - 1;
            chord.inversion = chord.inversion.clamp(-most, most);
        }
    }
}

/// What the values hold for Stacks' panel and `midi_tool.state` (R28).
///
/// # Errors
///
/// Why the values aren't a progression and a bank.
pub fn stack_view(values: &[ParamValue]) -> Result<StackView, String> {
    let state = State::read(values)?;
    Ok(StackView {
        built_in: text(values, BANK).is_empty(),
        chords: state.chords,
        selected: state.selected,
        bank: state.bank,
    })
}

/// Add Chord (R17): a chord after the selected one, the bank's first entry
/// with the selected chord's root, selected.
///
/// # Errors
///
/// Why the values aren't Stacks', or the progression is full.
pub fn add_chord(values: &mut [ParamValue]) -> Result<(), String> {
    let mut state = State::read(values)?;
    if state.chords.len() >= MAX_CHORDS {
        return Err(format!("a progression holds at most {MAX_CHORDS} chords"));
    }
    let root = state.chords[state.selected].root;
    state.selected += 1;
    state.chords.insert(
        state.selected,
        Chord {
            root,
            ..Chord::default()
        },
    );
    state.write(values);
    Ok(())
}

/// Delete Chord (R17): the selected chord, unless it is the only one; the
/// chord that took its place, or the last, is selected.
///
/// # Errors
///
/// Why the values aren't Stacks', or the progression has one chord.
pub fn delete_chord(values: &mut [ParamValue]) -> Result<(), String> {
    let mut state = State::read(values)?;
    if state.chords.len() == 1 {
        return Err("the only chord of a progression can't be deleted".to_owned());
    }
    state.chords.remove(state.selected);
    state.selected = state.selected.min(state.chords.len() - 1);
    state.write(values);
    Ok(())
}

/// Loads a bank (R23, R26), or the built-in one for `None`: the
/// progression's chords stay by their place in the bank, a place past the
/// new bank's end taking its first entry.
///
/// # Errors
///
/// Why the values aren't Stacks'.
pub fn load_bank(values: &mut [ParamValue], bank: Option<ChordBank>) -> Result<(), String> {
    let mut state = State::read(values)?;
    let bank = bank.filter(|bank| !bank.is_built_in());
    values[BANK] = ParamValue::Structured(bank.as_ref().map(ChordBank::write).unwrap_or_default());
    state.bank = bank.unwrap_or_else(|| bank::built_in().clone());
    state.fit_bank();
    state.write(values);
    Ok(())
}

/// The keys of `entry`'s notes on `root` with `inversion` (R19), each a key
/// the tool may write, in rising order.
#[must_use]
pub fn entry_keys(input: &ToolInput, entry: &ChordEntry, root: u8, inversion: i64) -> Vec<u8> {
    let mut keys: Vec<i32> = if entry.degrees {
        let base = input.nearest_key(root);
        entry
            .intervals
            .iter()
            .filter_map(|degrees| {
                let degrees = i32::from(*degrees);
                if input.drum_keys.is_some() {
                    // The pads count as the degrees; past the last there is none.
                    input.step_key(base, degrees).map(i32::from)
                } else if input.scale.is_some() {
                    Some(i32::from(input.stepped(base, degrees)))
                } else {
                    Some(major_move(root, degrees))
                }
            })
            .collect()
    } else {
        entry
            .intervals
            .iter()
            .map(|semitones| i32::from(root) + i32::from(*semitones))
            .collect()
    };
    for _ in 0..inversion.max(0) {
        if let Some(lowest) = keys.iter_mut().min() {
            *lowest += 12;
        }
    }
    for _ in 0..(-inversion).max(0) {
        if let Some(highest) = keys.iter_mut().max() {
            *highest -= 12;
        }
    }
    let mut keys: Vec<u8> = keys
        .into_iter()
        .map(|key| input.nearest_key(fold_octaves(key)))
        .collect();
    keys.sort_unstable();
    keys.dedup();
    keys
}

/// The key `degrees` degrees of the major scale on `root` above it.
fn major_move(root: u8, degrees: i32) -> i32 {
    bl_core::scale::move_off(root % 12, &MAJOR, root, degrees)
}

/// `slot` times `eighths` ÷ 8, rounded to the nearest tick, half up.
fn eighths(slot: i64, eighths: i64) -> i64 {
    (slot * eighths + 4).div_euclid(8)
}

impl MidiTool for Stacks {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.0
    }

    fn validate(&self, values: &[ParamValue]) -> Result<(), ToolError> {
        State::read(values)
            .and_then(|state| state.check())
            .map_err(ToolError::Values)
    }

    /// Keeps the selected chord's parameters and the progression in step: a
    /// parameter changed sets the selected chord's field, and every other
    /// change reads the selected chord back from the progression. A bank
    /// change keeps the chords by place (R23).
    fn settle(
        &self,
        _before: &[ParamValue],
        after: &mut [ParamValue],
        changed: &[usize],
    ) -> Result<(), String> {
        let given = changed.contains(&PROGRESSION);
        let bank = bank_of(after)?;
        let chords = parse_progression(text(after, PROGRESSION))?;
        let mut selected = usize::try_from(whole(after, SELECTED)).unwrap_or(0);
        if selected >= chords.len() {
            if changed.contains(&SELECTED) {
                return Err(format!(
                    "`selected` goes from 0 to {}, the last chord",
                    chords.len() - 1
                ));
            }
            selected = chords.len() - 1;
        }
        let mut state = State {
            chords,
            selected,
            bank,
        };
        let chord = &mut state.chords[selected];
        if changed.contains(&CHORD) {
            chord.chord = usize::try_from(whole(after, CHORD)).unwrap_or(0);
        }
        if changed.contains(&ROOT) {
            chord.root = pitch(after, ROOT);
        }
        if changed.contains(&INVERSION) {
            chord.inversion = whole(after, INVERSION);
        }
        if changed.contains(&DURATION) {
            chord.duration = whole(after, DURATION);
        }
        if changed.contains(&OFFSET) {
            chord.offset = whole(after, OFFSET);
        }
        // What the change gave is checked as given; what only the bank's
        // change left outside it is fitted (R23).
        let explicit =
            |at: usize, parameter: usize| given || (at == selected && changed.contains(&parameter));
        for (at, chord) in state.chords.iter().enumerate() {
            let entry_len = state
                .bank
                .chords
                .get(chord.chord)
                .map(|e| e.intervals.len());
            let Some(entry_len) = entry_len else {
                if explicit(at, CHORD) {
                    return Err(format!(
                        "chord {at} names bank entry {}; the bank holds {}",
                        chord.chord,
                        state.bank.chords.len()
                    ));
                }
                continue;
            };
            let most = entry_len as i64 - 1;
            if chord.inversion.abs() > most && explicit(at, INVERSION) {
                return Err(format!(
                    "chord {at}: `inversion` goes from {} to {most} for a chord of {entry_len} notes",
                    -most
                ));
            }
        }
        state.fit_bank();
        state.write(after);
        Ok(())
    }

    fn changes_notes(&self, changed: &[usize]) -> bool {
        changed.iter().any(|at| *at != SELECTED)
    }

    /// In Scale Mode, a new Stacks starts on the scale's root in the octave
    /// from 60 up (R20). A root that is set stays as it is, and is written on
    /// the nearest key the tool may write (R1, R19).
    fn start(&self, values: &mut [ParamValue], input: &ToolInput) {
        let Some(scale) = &input.scale else {
            return;
        };
        if values != self.0.defaults().as_slice() {
            return;
        }
        let Ok(mut state) = State::read(values) else {
            return;
        };
        state.chords[0].root = 60 + scale.root % 12;
        state.write(values);
    }

    fn run(&self, input: &ToolInput, values: &[ParamValue]) -> Result<ToolOutput, ToolError> {
        let state = State::read(values).map_err(ToolError::Values)?;
        state.check().map_err(ToolError::Values)?;
        let (from, to) = (input.range.0.0, input.range.1.0);
        let span = to - from;
        let count = state.chords.len() as i64;
        let mut notes = Vec::new();
        if span <= 0 {
            return Ok(ToolOutput { notes });
        }
        for (at, chord) in state.chords.iter().enumerate() {
            let at = at as i64;
            let slot_start = from + span * at / count;
            let slot_end = from + span * (at + 1) / count;
            let slot = slot_end - slot_start;
            let start = slot_start + eighths(slot, chord.offset);
            let end = (slot_start + eighths(slot, chord.offset + chord.duration)).min(slot_end);
            if end <= start {
                continue;
            }
            let Some(entry) = state.entry(chord) else {
                continue;
            };
            for key in entry_keys(input, entry, chord.root, chord.inversion) {
                notes.push(ToolNote::new(
                    i16::from(key),
                    Ticks(start),
                    Ticks(end - start),
                    100,
                ));
            }
        }
        Ok(ToolOutput { notes })
    }
}

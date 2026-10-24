//! Reading a tool's checked values (the registry's `run` has checked their
//! kinds and ranges, so a wrong one reads as zero, off or the first choice).

use crate::descriptor::{ChoiceOption, ParamValue};

pub(crate) fn whole(values: &[ParamValue], at: usize) -> i64 {
    match values.get(at) {
        Some(ParamValue::Whole(n)) => *n,
        _ => 0,
    }
}

pub(crate) fn switch(values: &[ParamValue], at: usize) -> bool {
    matches!(values.get(at), Some(ParamValue::Switch(true)))
}

pub(crate) fn choice(values: &[ParamValue], at: usize) -> usize {
    match values.get(at) {
        Some(ParamValue::Choice(n)) => *n,
        _ => 0,
    }
}

pub(crate) fn division(values: &[ParamValue], at: usize) -> usize {
    match values.get(at) {
        Some(ParamValue::Division(n)) => *n,
        _ => 0,
    }
}

pub(crate) fn pitch(values: &[ParamValue], at: usize) -> u8 {
    match values.get(at) {
        Some(ParamValue::Pitch(key)) => *key,
        _ => 0,
    }
}

pub(crate) fn pitch_range(values: &[ParamValue], at: usize) -> (u8, u8) {
    match values.get(at) {
        Some(ParamValue::PitchRange { low, high }) => (*low, *high),
        _ => (0, 0),
    }
}

pub(crate) fn levels(values: &[ParamValue], at: usize) -> &[f64] {
    match values.get(at) {
        Some(ParamValue::Levels(levels)) => levels,
        _ => &[],
    }
}

/// Holds the whole number at `at` within `min` to `max`, which depend on the
/// other parameters: a value the change itself set outside them is refused,
/// one the change left alone is moved inside.
pub(crate) fn hold(
    after: &mut [ParamValue],
    at: usize,
    (min, max): (i64, i64),
    changed: &[usize],
    name: &str,
) -> Result<(), String> {
    let n = whole(after, at);
    if (min..=max).contains(&n) {
        return Ok(());
    }
    if changed.contains(&at) {
        return Err(format!("`{name}` goes from {min} to {max} here"));
    }
    after[at] = ParamValue::Whole(n.clamp(min, max));
    Ok(())
}

/// The key a note name or number stands for: `C3` is key 60, and `C#3`, `Db3`
/// and `60` are the keys they read as.
#[must_use]
pub fn parse_key(text: &str) -> Option<u8> {
    let text = text.trim();
    if let Ok(key) = text.parse::<u8>() {
        return (key <= 127).then_some(key);
    }
    let mut chars = text.chars();
    let letter = chars.next()?.to_ascii_uppercase();
    let base: i32 = match letter {
        'C' => 0,
        'D' => 2,
        'E' => 4,
        'F' => 5,
        'G' => 7,
        'A' => 9,
        'B' => 11,
        _ => return None,
    };
    let rest = chars.as_str();
    let (shift, octave) = match rest.chars().next()? {
        '#' => (1, &rest[1..]),
        'b' if rest.len() > 1 => (-1, &rest[1..]),
        _ => (0, rest),
    };
    let key = (octave.parse::<i32>().ok()? + 2) * 12 + base + shift;
    u8::try_from(key).ok().filter(|key| *key <= 127)
}

/// A key's name as Live writes it: C3 is middle C (key 60).
#[must_use]
pub fn key_name(key: u8) -> String {
    const NAMES: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    format!(
        "{}{}",
        NAMES[usize::from(key) % 12],
        i32::from(key) / 12 - 2
    )
}

pub(crate) fn toggles(values: &[ParamValue], at: usize) -> &[bool] {
    match values.get(at) {
        Some(ParamValue::Toggles(toggles)) => toggles,
        _ => &[],
    }
}

/// Options from `(id, name)` pairs.
pub(crate) fn options(pairs: &[(&str, &str)]) -> Vec<ChoiceOption> {
    pairs
        .iter()
        .map(|(id, name)| ChoiceOption::new(id, name))
        .collect()
}

/// `n` × `percent` ÷ 100, rounded to the nearest tick, half up (the MIDI
/// transformations spec, R2).
pub(crate) fn percent(n: i64, percent: i64) -> i64 {
    (n * percent + 50).div_euclid(100)
}

/// A tick count worked out from a fraction, rounded to the nearest tick,
/// half up (R2).
pub(crate) fn round_ticks(ticks: f64) -> bl_core::time::Ticks {
    bl_core::time::Ticks((ticks + 0.5).floor() as i64)
}

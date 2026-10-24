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

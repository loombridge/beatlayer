//! Reading a tool's values by place, and the rounding the transformations
//! share (the MIDI transformations spec, R2).

use bl_core::time::Ticks;

use crate::descriptor::ParamValue;

/// The whole number at `at`.
pub(crate) fn whole(values: &[ParamValue], at: usize) -> i64 {
    match values.get(at) {
        Some(ParamValue::Whole(n)) => *n,
        _ => 0,
    }
}

/// The switch at `at`.
pub(crate) fn switch(values: &[ParamValue], at: usize) -> bool {
    matches!(values.get(at), Some(ParamValue::Switch(true)))
}

/// The choice at `at`.
pub(crate) fn choice(values: &[ParamValue], at: usize) -> usize {
    match values.get(at) {
        Some(ParamValue::Choice(n)) => *n,
        _ => 0,
    }
}

/// A tick count worked out from a fraction, rounded to the nearest tick,
/// half up (R2).
pub(crate) fn round_ticks(ticks: f64) -> Ticks {
    Ticks((ticks + 0.5).floor() as i64)
}

/// `percent` of `ticks`, rounded as [`round_ticks`] does.
pub(crate) fn percent_of(ticks: Ticks, percent: i64) -> Ticks {
    round_ticks(ticks.0 as f64 * percent as f64 / 100.0)
}

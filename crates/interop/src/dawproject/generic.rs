//! The mapping tables of DAWproject's four generic devices (the DAWproject
//! spec, R51 to R55): which Beatlayer parameter each of a generic device's
//! values becomes, and how its unit converts. They are kept in one place so
//! that writing a file (WP-6.37) turns them round.

use bl_devices::{compressor, eq, gate, limiter};
use bl_engine::DeviceType;
use bl_model::param::{db_to_gain, gain_to_db};
use bl_model::{DeviceKind, ParamId, ParamKind};

use super::model::Unit;

/// What a Beatlayer parameter holds, which says how a value converts to it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quantity {
    /// A level in decibels (a threshold).
    Decibels,
    /// A linear gain, which the file gives in decibels (an output level).
    Gain,
    /// A time in milliseconds, which the file gives in seconds.
    Milliseconds,
    /// A frequency in hertz.
    Hertz,
    /// A plain number the file gives as it is (a Q).
    Plain,
    /// A ratio: 100 % is the largest.
    Ratio,
    /// A switch.
    Toggle,
}

/// One value of a generic device and the parameter it becomes.
#[derive(Clone, Copy, Debug)]
pub struct Entry {
    /// The element that holds the value in the generic device.
    pub element: &'static str,
    /// The Beatlayer parameter.
    pub param: ParamId,
    /// What the parameter holds.
    pub quantity: Quantity,
}

/// The Compressor's values (R51).
pub const COMPRESSOR: &[Entry] = &[
    Entry {
        element: "Threshold",
        param: compressor::THRESHOLD,
        quantity: Quantity::Decibels,
    },
    Entry {
        element: "Ratio",
        param: compressor::RATIO,
        quantity: Quantity::Ratio,
    },
    Entry {
        element: "Attack",
        param: compressor::ATTACK,
        quantity: Quantity::Milliseconds,
    },
    Entry {
        element: "Release",
        param: compressor::RELEASE,
        quantity: Quantity::Milliseconds,
    },
    Entry {
        element: "OutputGain",
        param: compressor::OUT,
        quantity: Quantity::Gain,
    },
    Entry {
        element: "AutoMakeup",
        param: compressor::MAKEUP,
        quantity: Quantity::Toggle,
    },
];

/// The Gate's values (R52).
pub const GATE: &[Entry] = &[
    Entry {
        element: "Threshold",
        param: gate::THRESHOLD,
        quantity: Quantity::Decibels,
    },
    Entry {
        element: "Attack",
        param: gate::ATTACK,
        quantity: Quantity::Milliseconds,
    },
    Entry {
        element: "Release",
        param: gate::RELEASE,
        quantity: Quantity::Milliseconds,
    },
    Entry {
        element: "Range",
        param: gate::FLOOR,
        quantity: Quantity::Gain,
    },
];

/// The Limiter's values that become a parameter of their own (R53): its
/// Ceiling and Gain also take the output gain, which `limiter` adds.
pub const LIMITER: &[Entry] = &[Entry {
    element: "Release",
    param: limiter::RELEASE,
    quantity: Quantity::Milliseconds,
}];

/// The Limiter's Ceiling, which is `Threshold` plus `OutputGain`, in
/// decibels (R53).
pub const LIMITER_CEILING: Entry = Entry {
    element: "Threshold",
    param: limiter::CEILING,
    quantity: Quantity::Decibels,
};

/// The Limiter's Gain, which is `InputGain` plus `OutputGain`, in decibels
/// (R53).
pub const LIMITER_GAIN: Entry = Entry {
    element: "InputGain",
    param: limiter::GAIN,
    quantity: Quantity::Gain,
};

/// The EQ's own Gain, which is the generic equaliser's `OutputGain` (R54).
pub const EQ_GAIN: Entry = Entry {
    element: "OutputGain",
    param: eq::GAIN,
    quantity: Quantity::Gain,
};

/// How many bands the EQ takes of a generic equaliser (R54).
pub const EQ_BANDS: usize = eq::BANDS;

/// The EQ's band types, by the choice index each generic band type becomes
/// (R54): `None` for a type the EQ doesn't have.
#[must_use]
pub fn eq_band_type(band_type: &str) -> Option<usize> {
    Some(match band_type {
        "highPass" => 1,
        "lowShelf" => 2,
        "bell" => 3,
        "notch" => 4,
        "highShelf" => 5,
        "lowPass" => 6,
        _ => return None,
    })
}

/// The parameter of the EQ's first curve for a band's value (R54).
#[must_use]
pub const fn eq_param(band: usize, slot: usize) -> ParamId {
    eq::band_param(0, band, slot)
}

/// The EQ's band slots a generic band's values become.
pub const EQ_ON: usize = eq::ON;
/// The band's type choice.
pub const EQ_TYPE: usize = eq::TYPE;
/// The band's frequency.
pub const EQ_FREQ: usize = eq::FREQ;
/// The band's gain.
pub const EQ_GAIN_SLOT: usize = eq::GAIN_SLOT;
/// The band's Q.
pub const EQ_Q: usize = eq::Q;

/// The range of values a Beatlayer parameter holds, in its own units.
#[must_use]
pub fn range_of(types: &[DeviceType], device_type: &str, param: ParamId) -> Option<(f64, f64)> {
    let device = types
        .iter()
        .find(|t| matches!(&t.kind, DeviceKind::Native { device_type: id } if id == device_type))?;
    let descriptor = device.description.param(param)?;
    Some(match &descriptor.kind {
        ParamKind::Continuous { min, max, .. } => (*min, *max),
        ParamKind::Gain { max_db, .. } => (0.0, db_to_gain(*max_db)),
        ParamKind::Toggle => (0.0, 1.0),
        ParamKind::Choice { choices } => (0.0, choices.len().saturating_sub(1) as f64),
        ParamKind::Integer { min, max, .. } => (*min as f64, *max as f64),
    })
}

/// A file's value as the Beatlayer parameter holds it (R55): converted from
/// `unit` (or the file's own unit for the quantity when it gives none) and
/// kept within `range`. `None` for a unit that doesn't convert.
#[must_use]
pub fn convert(
    quantity: Quantity,
    unit: Option<Unit>,
    value: f64,
    min: Option<f64>,
    max: Option<f64>,
    range: (f64, f64),
) -> Option<f64> {
    if value.is_nan() {
        return None;
    }
    let unit = unit.unwrap_or(match quantity {
        Quantity::Decibels | Quantity::Gain => Unit::Decibel,
        Quantity::Milliseconds => Unit::Seconds,
        Quantity::Hertz => Unit::Hertz,
        Quantity::Plain | Quantity::Ratio | Quantity::Toggle => Unit::Linear,
    });
    let converted = match (quantity, unit) {
        (_, Unit::Normalized) => {
            let (lo, hi) = (min.unwrap_or(0.0), max.unwrap_or(1.0));
            let share = if hi > lo {
                (value - lo) / (hi - lo)
            } else {
                0.0
            };
            range.0 + share * (range.1 - range.0)
        }
        (Quantity::Decibels, Unit::Decibel) | (Quantity::Hertz, Unit::Hertz) => value,
        (Quantity::Decibels, Unit::Linear) => gain_to_db(value),
        (Quantity::Gain, Unit::Decibel) => db_to_gain(value),
        (Quantity::Gain, Unit::Linear) => value,
        (Quantity::Milliseconds, Unit::Seconds) => value * 1000.0,
        (Quantity::Plain | Quantity::Toggle, Unit::Linear) => value,
        (Quantity::Ratio, Unit::Percent) => {
            // 100 % is the largest ratio.
            if value >= 100.0 {
                range.1
            } else {
                1.0 / (1.0 - value / 100.0)
            }
        }
        (Quantity::Ratio, Unit::Linear) => value,
        _ => return None,
    };
    if converted.is_nan() {
        return None;
    }
    Some(converted.clamp(range.0, range.1))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Spec: dawproject R55.
    #[test]
    fn values_convert_from_their_units_and_stay_in_range() {
        let range = (-60.0, 0.0);
        let db = |unit, value| convert(Quantity::Decibels, unit, value, None, None, range);
        assert_eq!(db(Some(Unit::Decibel), -12.0), Some(-12.0));
        assert_eq!(db(Some(Unit::Decibel), -90.0), Some(-60.0));
        assert_eq!(db(Some(Unit::Linear), 1.0), Some(0.0));
        assert_eq!(db(Some(Unit::Normalized), 0.5), Some(-30.0));
        assert_eq!(db(Some(Unit::Seconds), 1.0), None);
        let ms = convert(
            Quantity::Milliseconds,
            Some(Unit::Seconds),
            0.01,
            None,
            None,
            (0.0, 50.0),
        );
        assert_eq!(ms, Some(10.0));
        let gain = convert(
            Quantity::Gain,
            Some(Unit::Decibel),
            0.0,
            None,
            None,
            (0.0, 8.0),
        );
        assert_eq!(gain, Some(1.0));
        let gain = convert(
            Quantity::Gain,
            Some(Unit::Decibel),
            f64::NEG_INFINITY,
            None,
            None,
            (0.0, 8.0),
        );
        assert_eq!(gain, Some(0.0));
    }

    // Spec: dawproject R51. A percent is the ratio 1 / (1 − p / 100), and
    // 100 % the largest ratio.
    #[test]
    fn a_ratio_in_percent_is_one_over_the_rest() {
        let ratio = |value| {
            convert(
                Quantity::Ratio,
                Some(Unit::Percent),
                value,
                None,
                None,
                (1.0, 100.0),
            )
        };
        assert_eq!(ratio(75.0), Some(4.0));
        assert_eq!(ratio(100.0), Some(100.0));
        assert_eq!(ratio(0.0), Some(1.0));
    }
}

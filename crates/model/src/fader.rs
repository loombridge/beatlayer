//! The fader's and the send knob's travel (the mixer view spec, R6 to R10):
//! where a gain sits on the control's scale, from 0 (−∞ dB) to 1 (the top),
//! and the gain at a place on it. The mixer's controls draw on it, and
//! relative modulation (the modulation spec, R4) takes its share of it.

/// The fader's top, in dB (observed: +6.0 dB).
pub const TOP_DB: f64 = 6.0;

/// The lowest finite volume, in dB (observed: −69.6 dB stays; −69.7 or
/// lower is −∞).
pub const LOWEST_DB: f64 = -69.6;

/// Below this a volume is −∞ dB: between the lowest finite step and the
/// next tenth down.
const SILENCE_BELOW_DB: f64 = -69.65;

/// The fader's and the meter's shared scale, as dB and the height in a
/// strip, in points from the strip section's top, where the handle sits
/// (observed: the handle sits on the meter's 0, 12, 24, 36, 48, and 60
/// labels at those values, +6 dB 10.5 points above 0, and −∞ about 7.5
/// points below −60). From −60 dB the scale runs straight to the bottom,
/// reached at −70 dB.
pub const FADER_SCALE: [(f64, f32); 8] = [
    (6.0, 13.0),
    (0.0, 23.5),
    (-12.0, 44.5),
    (-24.0, 64.5),
    (-36.0, 84.5),
    (-48.0, 99.0),
    (-60.0, 111.0),
    (-70.0, 118.5),
];

/// The fader's travel in a strip, top and bottom, in points from the strip
/// section's top: from +6 dB to −∞.
pub const FADER_TRAVEL: (f32, f32) = (13.0, 118.5);

/// The lowest finite send level, in dB: below it a send is at −∞. Live's
/// send knob's range isn't observed; Beatlayer's runs from −∞ to 0 dB
/// (the mixer spec, R7), its finite part down to the fader's lowest.
pub const SEND_LOWEST_DB: f64 = LOWEST_DB;

/// A gain in dB, from a linear gain (−∞ for 0).
#[must_use]
pub fn gain_db(gain: f64) -> f64 {
    if gain <= 0.0 {
        f64::NEG_INFINITY
    } else {
        20.0 * gain.log10()
    }
}

/// A linear gain from dB.
#[must_use]
pub fn db_gain(db: f64) -> f64 {
    if db == f64::NEG_INFINITY {
        0.0
    } else {
        10f64.powf(db / 20.0)
    }
}

/// The gain of a volume of `db`, kept within the fader's range: no higher
/// than its top, and −∞ below its lowest finite step.
#[must_use]
pub fn volume_gain(db: f64) -> f64 {
    if db.is_nan() || db < SILENCE_BELOW_DB {
        0.0
    } else {
        db_gain(db.min(TOP_DB))
    }
}

/// [`FADER_SCALE`] as pairs of `f64`, ordered by `pair`, on the stack: the
/// audio thread reads the fader's law, so it allocates nothing.
fn scale(pair: impl Fn(f64, f64) -> (f64, f64)) -> [(f64, f64); FADER_SCALE.len()] {
    let mut points = [(0.0, 0.0); FADER_SCALE.len()];
    for (point, &(db, y)) in points.iter_mut().zip(&FADER_SCALE) {
        *point = pair(db, f64::from(y));
    }
    points
}

/// Straight-line interpolation through `points`, sorted by their first
/// value in either direction, clamped to the ends.
fn through(points: &[(f64, f64)], x: f64) -> f64 {
    let ascending = points[0].0 < points[points.len() - 1].0;
    for pair in points.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let inside = if ascending {
            x >= a.0 && x <= b.0
        } else {
            x <= a.0 && x >= b.0
        };
        if inside {
            let t = (x - a.0) / (b.0 - a.0);
            return a.1 + (b.1 - a.1) * t;
        }
    }
    let before_first = if ascending {
        x < points[0].0
    } else {
        x > points[0].0
    };
    if before_first {
        points[0].1
    } else {
        points[points.len() - 1].1
    }
}

/// Where a gain sits on the fader's (and the meter's) scale, from 0 (−∞ dB,
/// the bottom) to 1 (+6 dB, the top). Gains above the top sit at the top.
#[must_use]
pub fn fader_position(gain: f64) -> f64 {
    let (top, bottom) = FADER_TRAVEL;
    let y = if gain <= 0.0 || gain.is_nan() {
        f64::from(bottom)
    } else {
        through(&scale(|db, y| (db, y)), gain_db(gain))
    };
    ((f64::from(bottom) - y) / f64::from(bottom - top)).clamp(0.0, 1.0)
}

/// The gain at a place on the fader's scale, from 0 (−∞ dB, the bottom) to
/// 1 (+6 dB, the top): the inverse of [`fader_position`].
#[must_use]
pub fn fader_gain(position: f64) -> f64 {
    let (top, bottom) = FADER_TRAVEL;
    let position = position.clamp(0.0, 1.0);
    if position <= 0.0 {
        return 0.0;
    }
    let y = f64::from(bottom) - position * f64::from(bottom - top);
    volume_gain(through(&scale(|db, y| (y, db)), y))
}

/// Where a send's knob points, from 0 (−∞) to 1 (0 dB): straight in dB
/// from the lowest finite level.
#[must_use]
pub fn send_position(level: f64) -> f64 {
    if level <= 0.0 {
        return 0.0;
    }
    ((gain_db(level) - SEND_LOWEST_DB) / -SEND_LOWEST_DB).clamp(0.0, 1.0)
}

/// The send level a knob position gives, the inverse of
/// [`send_position`]: 0 (−∞) at the bottom.
#[must_use]
pub fn send_level(position: f64) -> f64 {
    if position <= 0.0 {
        return 0.0;
    }
    db_gain((SEND_LOWEST_DB * (1.0 - position.min(1.0)) * 100.0).round() / 100.0)
}

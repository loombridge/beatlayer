//! The mixer's controls (WP-1.9): what a strip's or a track header's
//! volume, pan, activator, Solo, and Arm show, and the registered commands
//! each change runs (`mixer.*`, WP-1.6), worked out from the set and the
//! selection so the window and its tests share them. The rules are in the
//! mixer view spec (R6 to R10).
//!
//! The fader's range, its scale, how far a drag moves it, and its key steps
//! were observed in Live 12.4.6 on macOS on 11 October 2026 (the mixer view
//! spec, sources). Pan's drag distance and how much finer Shift makes a drag
//! weren't, and are listed there to observe.

use bl_model::{Document, TrackId};
use serde_json::json;

use crate::Invocation;

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

/// How far a drag moves the volume, as points of drag down from the top
/// and the dB there (observed, sampling a drag from +6 dB every 9 points,
/// then every 4.5). A drag is relative: it moves the value by the pointer's
/// movement, not to the pointer.
pub const DRAG_SCALE: [(f64, f64); 7] = [
    (0.0, 6.0),
    (9.0, 4.3),
    (18.0, -2.6),
    (27.0, -14.6),
    (31.5, -29.4),
    (36.0, -52.5),
    (40.0, LOWEST_DB),
];

/// Points of drag from the top at which the volume is −∞ (observed: about
/// 40 points for the whole range; −∞ at 40.5).
pub const DRAG_RANGE: f64 = 40.5;

/// How many pan steps there are to each side: pan reads 1L to 50L and 1R to
/// 50R.
pub const PAN_STEPS: f64 = 50.0;

/// Points of vertical drag that move pan over half its range. Not observed
/// (the mixer view spec, "To observe in Live").
pub const PAN_DRAG_TRAVEL: f32 = 100.0;

/// How much finer a drag is with Shift held. Not observed.
pub const FINE: f32 = 10.0;

/// The undo step a volume drag makes, named as Live names it (tracks and
/// undo, item 10).
pub const VOLUME_GESTURE: &str = "Change \"Track Volume\"";

/// The undo step a pan drag makes.
pub const PAN_GESTURE: &str = "Change \"Track Panning\"";

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
        let scale: Vec<(f64, f64)> = FADER_SCALE
            .iter()
            .map(|(db, y)| (*db, f64::from(*y)))
            .collect();
        through(&scale, gain_db(gain))
    };
    ((f64::from(bottom) - y) / f64::from(bottom - top)).clamp(0.0, 1.0)
}

/// Where a gain is along a drag: points of drag down from +6 dB.
#[must_use]
pub fn drag_points(gain: f64) -> f64 {
    let db = gain_db(gain);
    if db < SILENCE_BELOW_DB {
        return DRAG_RANGE;
    }
    let inverse: Vec<(f64, f64)> = DRAG_SCALE.iter().map(|(p, db)| (*db, *p)).collect();
    through(&inverse, db.min(TOP_DB))
}

/// The gain at `points` of drag down from +6 dB: −∞ from [`DRAG_RANGE`] on.
#[must_use]
pub fn drag_gain(points: f64) -> f64 {
    if points >= DRAG_RANGE {
        return 0.0;
    }
    volume_gain(through(&DRAG_SCALE, points.max(0.0)))
}

/// Volume as the mixer shows it, in dB to one decimal: `0.0`, `-6.0`,
/// `-inf`.
#[must_use]
pub fn volume_text(gain: f64) -> String {
    if gain <= 0.0 {
        "-inf".to_owned()
    } else {
        let db = gain_db(gain);
        // -0.0 reads 0.0.
        let shown = (db * 10.0).round() / 10.0;
        format!("{:.1}", if shown == 0.0 { 0.0 } else { shown })
    }
}

/// Volume as a screen reader reads it: `0.0 dB`, `minus infinity dB`.
#[must_use]
pub fn volume_spoken(gain: f64) -> String {
    if gain <= 0.0 {
        "minus infinity dB".to_owned()
    } else {
        format!("{} dB", volume_text(gain))
    }
}

/// What the Status Bar shows while a fader is dragged (observed:
/// `Track Volume: -21.0 dB`).
#[must_use]
pub fn volume_readout(gain: f64) -> String {
    format!("Track Volume: {} dB", volume_text(gain))
}

/// Pan as the mixer shows it: `C`, or a number from 1 to 50 and `L` or `R`.
#[must_use]
pub fn pan_text(pan: f64) -> String {
    let amount = (pan.abs() * PAN_STEPS).round() as i64;
    match amount {
        0 => "C".to_owned(),
        _ if pan < 0.0 => format!("{amount}L"),
        _ => format!("{amount}R"),
    }
}

/// A gain `steps` dB away (tenths of a dB with `fine`), as Up and Down step
/// it (observed: from −68.5, Down gives −69.5 then −∞; from −∞, Up gives
/// −69.0 then −68.0; from 5.5, Up gives 6.0).
#[must_use]
pub fn step_volume(gain: f64, steps: i32, fine: bool) -> f64 {
    if steps == 0 {
        return gain;
    }
    let step = if fine { 0.1 } else { 1.0 };
    let db = gain_db(gain);
    if db == f64::NEG_INFINITY {
        return if steps > 0 {
            volume_gain(-69.0 + step * f64::from(steps - 1))
        } else {
            0.0
        };
    }
    // Rounded to a hundredth, so tenths stay tenths.
    volume_gain(((db + step * f64::from(steps)) * 100.0).round() / 100.0)
}

/// A gain moved by `delta` dB, kept within the fader's range: several
/// selected tracks move by the same dB (observed). A silent track stays
/// silent.
#[must_use]
pub fn shift_volume(gain: f64, delta: f64) -> f64 {
    if gain <= 0.0 {
        return 0.0;
    }
    volume_gain(gain_db(gain) + delta)
}

/// A pan `steps` steps of 50 to a side away, within −1 to 1.
#[must_use]
pub fn step_pan(pan: f64, steps: i32) -> f64 {
    let at = (pan * PAN_STEPS).round() + f64::from(steps);
    (at / PAN_STEPS).clamp(-1.0, 1.0)
}

/// Where a meter of `amplitude` reaches, from 0 to 1, on the fader's scale,
/// which the meter shares (observed).
#[must_use]
pub fn meter_position(amplitude: f32) -> f32 {
    fader_position(f64::from(amplitude)) as f32
}

/// A continuous control in a strip or a header.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Control {
    /// The volume fader and field.
    Volume,
    /// The pan knob.
    Pan,
}

impl Control {
    /// The control's value on a track: the volume's gain, or the pan.
    #[must_use]
    pub fn value(self, doc: &Document, track: TrackId) -> Option<f64> {
        let mixer = doc.track(track)?.mixer();
        Some(match self {
            Self::Volume => mixer.volume,
            Self::Pan => mixer.pan,
        })
    }

    /// The command that sets it to a value.
    #[must_use]
    pub fn set(self, track: TrackId, value: f64) -> Invocation {
        match self {
            Self::Volume => Invocation::new(
                "mixer.set_volume",
                json!({ "track": track, "volume": value.max(0.0) }),
            ),
            Self::Pan => Invocation::new(
                "mixer.set_pan",
                json!({ "track": track, "pan": value.clamp(-1.0, 1.0) }),
            ),
        }
    }

    /// Points of vertical drag per unit a drag moves: volume moves by
    /// points of drag along [`DRAG_SCALE`], pan by [`PAN_DRAG_TRAVEL`]
    /// points per unit.
    #[must_use]
    pub fn drag_travel(self) -> f32 {
        match self {
            Self::Volume => 1.0,
            Self::Pan => PAN_DRAG_TRAVEL,
        }
    }

    /// The undo step a drag of it makes.
    #[must_use]
    pub fn gesture(self) -> &'static str {
        match self {
            Self::Volume => VOLUME_GESTURE,
            Self::Pan => PAN_GESTURE,
        }
    }

    /// Its default: 0 dB (a double-click's reset, observed), or the centre.
    #[must_use]
    pub fn default_value(self) -> f64 {
        match self {
            Self::Volume => 1.0,
            Self::Pan => 0.0,
        }
    }
}

/// A switch in a strip or a header.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Switch {
    /// The Track Activator: on while the track plays.
    Activator,
    /// Solo.
    Solo,
    /// Arm Recording.
    Arm,
}

impl Switch {
    /// Whether a track has this switch: every track but Main has the
    /// activator and Solo; only audio and MIDI tracks arm.
    #[must_use]
    pub fn on_track(self, doc: &Document, track: TrackId) -> bool {
        doc.track(track).is_some_and(|t| match self {
            Self::Activator | Self::Solo => t.kind().has_mute_and_solo(),
            Self::Arm => t.kind().has_arm(),
        })
    }

    /// Whether it's lit: the activator while the track isn't muted.
    #[must_use]
    pub fn is_on(self, doc: &Document, track: TrackId) -> bool {
        doc.track(track).is_some_and(|t| {
            let mixer = t.mixer();
            match self {
                Self::Activator => !mixer.mute,
                Self::Solo => mixer.solo,
                Self::Arm => mixer.arm,
            }
        })
    }

    /// The command that sets it on a track, without touching the others.
    #[must_use]
    pub fn set(self, track: TrackId, on: bool) -> Invocation {
        match self {
            Self::Activator => {
                Invocation::new("mixer.set_mute", json!({ "track": track, "mute": !on }))
            }
            Self::Solo => Invocation::new("mixer.set_solo", json!({ "track": track, "solo": on })),
            Self::Arm => Invocation::new("mixer.set_arm", json!({ "track": track, "arm": on })),
        }
    }

    /// The command that sets it on a track and releases it on every other,
    /// for Solo and Arm.
    #[must_use]
    pub fn exclusive(self, track: TrackId) -> Option<Invocation> {
        match self {
            Self::Activator => None,
            Self::Solo => Some(Invocation::new(
                "mixer.solo_exclusive",
                json!({ "track": track }),
            )),
            Self::Arm => Some(Invocation::new(
                "mixer.arm_exclusive",
                json!({ "track": track }),
            )),
        }
    }

    /// The undo step a click on it makes when it changes several tracks.
    #[must_use]
    pub fn gesture(self) -> &'static str {
        match self {
            Self::Activator => "Activate/Deactivate Track",
            Self::Solo => "Solo Track",
            Self::Arm => "Arm Track",
        }
    }

    /// The switch a toggling mixer command sets, and the argument it
    /// toggles.
    #[must_use]
    pub fn of_command(id: &str) -> Option<Self> {
        match id {
            "mixer.set_mute" => Some(Self::Activator),
            "mixer.set_solo" => Some(Self::Solo),
            "mixer.set_arm" => Some(Self::Arm),
            _ => None,
        }
    }
}

/// The tracks a change to `track` reaches: every selected track if it's
/// selected (R8), or only it; each must have a mixer (`has` says).
#[must_use]
pub fn targets(
    track: TrackId,
    selected: &[TrackId],
    has: impl Fn(TrackId) -> bool,
) -> Vec<TrackId> {
    if selected.contains(&track) {
        let mut out: Vec<TrackId> = vec![track];
        out.extend(
            selected
                .iter()
                .copied()
                .filter(|other| *other != track && has(*other)),
        );
        out
    } else {
        vec![track]
    }
}

/// The commands that move a control on `starts` (each track with its value
/// when the change began, the changed track first) by `delta`, which a drag
/// has moved since it began: for volume, points of drag up, which move the
/// first track along [`DRAG_SCALE`] and every other by the same dB; for
/// pan, the same amount on each (R6, R8). Every value stays within range,
/// so a drag past an end must come back before the value moves again.
#[must_use]
pub fn moved(control: Control, starts: &[(TrackId, f64)], delta: f64) -> Vec<Invocation> {
    let Some(&(_, first)) = starts.first() else {
        return Vec::new();
    };
    match control {
        Control::Volume => {
            let now = drag_gain((drag_points(first) - delta).clamp(0.0, DRAG_RANGE));
            let shift = if now <= 0.0 {
                f64::NEG_INFINITY
            } else {
                gain_db(now) - gain_db(first).max(LOWEST_DB)
            };
            starts
                .iter()
                .enumerate()
                .map(|(i, (track, start))| {
                    let value = if i == 0 {
                        now
                    } else if shift == f64::NEG_INFINITY {
                        0.0
                    } else {
                        shift_volume(*start, shift)
                    };
                    control.set(*track, value)
                })
                .collect()
        }
        Control::Pan => starts
            .iter()
            .map(|(track, start)| control.set(*track, (start + delta).clamp(-1.0, 1.0)))
            .collect(),
    }
}

/// The commands a click on a switch runs (R7, R8): `add` is Cmd (Ctrl)
/// held. Every target goes to the clicked track's new state; an exclusive
/// Solo or Arm click releases every other track.
#[must_use]
pub fn clicked(
    doc: &Document,
    switch: Switch,
    track: TrackId,
    selected: &[TrackId],
    add: bool,
) -> Vec<Invocation> {
    if !switch.on_track(doc, track) {
        return Vec::new();
    }
    let tracks = targets(track, selected, |t| switch.on_track(doc, t));
    let on = switch.is_on(doc, track);
    match switch {
        Switch::Activator => tracks.iter().map(|t| switch.set(*t, !on)).collect(),
        Switch::Solo | Switch::Arm if add => tracks.iter().map(|t| switch.set(*t, !on)).collect(),
        Switch::Solo | Switch::Arm => {
            let others_on = doc
                .tracks()
                .any(|t| !tracks.contains(&t.id()) && switch.is_on(doc, t.id()));
            if on && !others_on {
                // The only ones lit: an exclusive click turns them off.
                tracks.iter().map(|t| switch.set(*t, false)).collect()
            } else {
                let mut out: Vec<Invocation> = switch.exclusive(track).into_iter().collect();
                out.extend(tracks.iter().skip(1).map(|t| switch.set(*t, true)));
                out
            }
        }
    }
}

/// The commands a title-bar key toggling `switch` runs (R9): every track in
/// `tracks` that has it, set to the opposite of the first one's state.
#[must_use]
pub fn toggled(doc: &Document, switch: Switch, tracks: &[TrackId]) -> Vec<Invocation> {
    let tracks: Vec<TrackId> = tracks
        .iter()
        .copied()
        .filter(|t| switch.on_track(doc, *t))
        .collect();
    let Some(first) = tracks.first() else {
        return Vec::new();
    };
    let on = !switch.is_on(doc, *first);
    tracks.iter().map(|t| switch.set(*t, on)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Spec: mixer-view R6.
    #[test]
    fn values_read_as_live_s_mixer_shows_them() {
        assert_eq!(volume_text(1.0), "0.0");
        assert_eq!(volume_text(0.5), "-6.0");
        assert_eq!(volume_text(0.0), "-inf");
        assert_eq!(volume_text(0.999_99), "0.0");
        assert_eq!(volume_readout(db_gain(-21.0)), "Track Volume: -21.0 dB");
        assert_eq!(volume_spoken(0.0), "minus infinity dB");
        assert_eq!(pan_text(0.0), "C");
        assert_eq!(pan_text(-0.5), "25L");
        assert_eq!(pan_text(1.0), "50R");
        assert_eq!(pan_text(0.004), "C");
    }

    fn db(gain: f64) -> f64 {
        (gain_db(gain) * 10.0).round() / 10.0
    }

    // Spec: mixer-view R6.
    #[test]
    fn the_fader_s_range_and_scale_are_live_s() {
        // +6 dB at the top, −∞ at the bottom, −69.6 dB the lowest finite.
        assert_eq!(fader_position(db_gain(6.0)), 1.0);
        assert_eq!(fader_position(db_gain(12.0)), 1.0);
        assert_eq!(fader_position(0.0), 0.0);
        assert_eq!(db(volume_gain(-69.6)), -69.6);
        assert_eq!(volume_gain(-69.7), 0.0);
        assert_eq!(db(volume_gain(9.0)), 6.0);
        // The handle sits on the meter's labels: 0 dB at 23.5 points down
        // the strip, −12 at 44.5, ... −60 at 111.
        let (top, bottom) = FADER_TRAVEL;
        let y = |gain: f64| bottom - (bottom - top) * fader_position(gain) as f32;
        for (label, at) in [
            (0.0, 23.5),
            (-12.0, 44.5),
            (-24.0, 64.5),
            (-36.0, 84.5),
            (-48.0, 99.0),
            (-60.0, 111.0),
        ] {
            assert!((y(db_gain(label)) - at).abs() < 1e-3, "{label}");
        }
        // 0 dB about 88% of the way up.
        assert!((fader_position(1.0) - 0.9).abs() < 0.03);
        assert!((meter_position(1.0) - fader_position(1.0) as f32).abs() < 1e-6);
    }

    // Spec: mixer-view R6.
    #[test]
    fn a_drag_covers_the_range_in_about_40_points() {
        // Sampled from +6 dB every 9 points, then every 4.5.
        let samples = [
            (0.0, 6.0),
            (9.0, 4.3),
            (18.0, -2.6),
            (27.0, -14.6),
            (31.5, -29.4),
            (36.0, -52.5),
        ];
        for (points, expected) in samples {
            assert_eq!(db(drag_gain(points)), expected, "{points}");
            assert!(
                (drag_points(db_gain(expected)) - points).abs() < 1e-6,
                "{expected}"
            );
        }
        assert_eq!(drag_gain(40.5), 0.0);
        assert_eq!(drag_points(0.0), DRAG_RANGE);
        // Relative and past the ends: down 100 points from 0 dB is −∞, and
        // back up 10 points is still −∞ until the overshoot is undone.
        let t = TrackId(bl_core::id::Id::new(9).unwrap());
        let volume = |delta: f64| {
            moved(Control::Volume, &[(t, 1.0)], delta)[0].args["volume"]
                .as_f64()
                .unwrap()
        };
        assert_eq!(volume(-100.0), 0.0);
        assert_eq!(volume(-90.0), 0.0);
        assert_eq!(db(volume(100.0)), 6.0);
        assert!((db(volume(-9.0)) - db(drag_gain(drag_points(1.0) + 9.0))).abs() < 1e-9);
    }

    // Spec: mixer-view R6.
    #[test]
    fn keys_step_volume_as_live_does() {
        assert_eq!(db(step_volume(1.0, 1, false)), 1.0);
        assert_eq!(db(step_volume(1.0, -3, false)), -3.0);
        assert_eq!(db(step_volume(1.0, 1, true)), 0.1);
        // From −68.5, Down gives −69.5 then −∞; from −∞ Up gives −69.0
        // then −68.0; from 5.5, Up stops at 6.0.
        let a = step_volume(db_gain(-68.5), -1, false);
        assert_eq!(db(a), -69.5);
        assert_eq!(step_volume(a, -1, false), 0.0);
        let b = step_volume(0.0, 1, false);
        assert_eq!(db(b), -69.0);
        assert_eq!(db(step_volume(b, 1, false)), -68.0);
        assert_eq!(db(step_volume(db_gain(5.5), 1, false)), 6.0);
        assert_eq!(step_volume(0.0, -1, false), 0.0);
        assert_eq!(step_pan(0.0, 1), 0.02);
        assert_eq!(step_pan(0.99, 5), 1.0);
        assert_eq!(step_pan(-1.0, -1), -1.0);
    }

    // Spec: mixer-view R8.
    #[test]
    fn selected_tracks_move_by_the_same_db() {
        // Observed: −53 → −46 moved the other −3 → +4.
        let (a, b) = (
            TrackId(bl_core::id::Id::new(1).unwrap()),
            TrackId(bl_core::id::Id::new(2).unwrap()),
        );
        let start = db_gain(-53.0);
        let up = drag_points(start) - drag_points(db_gain(-46.0));
        let changes = moved(Control::Volume, &[(a, start), (b, db_gain(-3.0))], up);
        let get = |i: usize| changes[i].args["volume"].as_f64().unwrap();
        assert_eq!(db(get(0)), -46.0);
        assert_eq!(db(get(1)), 4.0);
        assert_eq!(db(shift_volume(db_gain(5.0), 3.0)), 6.0);
        assert_eq!(shift_volume(0.0, 3.0), 0.0);
        assert_eq!(shift_volume(db_gain(-69.0), -1.0), 0.0);
    }
}

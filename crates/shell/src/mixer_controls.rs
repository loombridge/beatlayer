//! The mixer's controls (WP-1.9): what a strip's or a track header's
//! volume, pan, activator, Solo, and Arm show, and the registered commands
//! each change runs (`mixer.*`, WP-1.6), worked out from the set and the
//! selection so the window and its tests share them. The rules are in the
//! mixer view spec (R6 to R10).
//!
//! The fader's law and range, and the steps of drags and keys, are
//! Beatlayer's until Live is observed (the mixer view spec, "To observe in
//! Live", items 1 and 2).

use bl_model::{Document, TrackId};
use serde_json::json;

use crate::Invocation;

/// The fader's top, in dB: Beatlayer's until Live's is observed (the mixer
/// spec's item 1).
pub const TOP_DB: f64 = 6.0;

/// The fader's bottom step above silence, in dB: Down from here, or a
/// fader dragged lower, reaches −∞ dB.
pub const BOTTOM_DB: f64 = -70.0;

/// The fader law's exponent: the gain is the top's gain times the fader's
/// position to this power.
const LAW: f64 = 4.0;

/// How many pan steps there are to each side: pan reads 1L to 50L and 1R to
/// 50R.
pub const PAN_STEPS: f64 = 50.0;

/// Points of vertical drag that move a volume field over the fader's whole
/// travel, or a pan control over half its range.
pub const DRAG_TRAVEL: f32 = 100.0;

/// How much finer a drag is with Shift held.
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

/// Where a gain sits on the fader, from 0 (−∞ dB) to 1 (the top). Gains
/// above the top sit at the top.
#[must_use]
pub fn fader_position(gain: f64) -> f64 {
    if gain <= 0.0 || !gain.is_finite() {
        return if gain.is_infinite() { 1.0 } else { 0.0 };
    }
    (gain / db_gain(TOP_DB)).powf(1.0 / LAW).clamp(0.0, 1.0)
}

/// The gain at a place on the fader. Below the bottom step it's silence.
#[must_use]
pub fn fader_gain(position: f64) -> f64 {
    let position = position.clamp(0.0, 1.0);
    let gain = db_gain(TOP_DB) * position.powf(LAW);
    if gain_db(gain) < BOTTOM_DB { 0.0 } else { gain }
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

/// A gain `steps` dB steps away (tenths of a dB with `fine`), from −∞ dB to
/// the top: up from silence goes to the bottom step, down past it to
/// silence (the mixer view spec, R6).
#[must_use]
pub fn step_volume(gain: f64, steps: i32, fine: bool) -> f64 {
    if steps == 0 {
        return gain;
    }
    let step = if fine { 0.1 } else { 1.0 };
    let db = gain_db(gain);
    let next = if db == f64::NEG_INFINITY {
        if steps > 0 {
            BOTTOM_DB + step * f64::from(steps - 1)
        } else {
            return 0.0;
        }
    } else {
        // Round to the step first, so steps land on whole values.
        let snapped = (db / step).round() * step;
        snapped + step * f64::from(steps)
    };
    if next < BOTTOM_DB - 1e-9 {
        0.0
    } else {
        db_gain(next.min(TOP_DB))
    }
}

/// A pan `steps` steps of 50 to a side away, within −1 to 1.
#[must_use]
pub fn step_pan(pan: f64, steps: i32) -> f64 {
    let at = (pan * PAN_STEPS).round() + f64::from(steps);
    (at / PAN_STEPS).clamp(-1.0, 1.0)
}

/// Where a meter of `amplitude` reaches, from 0 to 1, on the fader's scale,
/// so a level and a fader setting at the same height match (R10).
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
    /// The control's value on a track, in the units a drag moves: the
    /// fader's position for volume, the pan for pan.
    #[must_use]
    pub fn position(self, doc: &Document, track: TrackId) -> Option<f64> {
        let mixer = doc.track(track)?.mixer();
        Some(match self {
            Self::Volume => fader_position(mixer.volume),
            Self::Pan => mixer.pan,
        })
    }

    /// The command that sets it to a position.
    #[must_use]
    pub fn set(self, track: TrackId, position: f64) -> Invocation {
        match self {
            Self::Volume => Invocation::new(
                "mixer.set_volume",
                json!({ "track": track, "volume": fader_gain(position) }),
            ),
            Self::Pan => Invocation::new(
                "mixer.set_pan",
                json!({ "track": track, "pan": position.clamp(-1.0, 1.0) }),
            ),
        }
    }

    /// The position range.
    #[must_use]
    pub fn range(self) -> (f64, f64) {
        match self {
            Self::Volume => (0.0, 1.0),
            Self::Pan => (-1.0, 1.0),
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

    /// Its default: 0 dB, or the centre.
    #[must_use]
    pub fn default_position(self) -> f64 {
        match self {
            Self::Volume => fader_position(1.0),
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

/// The commands that move a control on `starts` (each track with its
/// position when the change began) by `delta`, each kept within the
/// control's range, so the differences between them stay (R6, R8).
#[must_use]
pub fn moved(control: Control, starts: &[(TrackId, f64)], delta: f64) -> Vec<Invocation> {
    let (low, high) = control.range();
    starts
        .iter()
        .map(|(track, start)| control.set(*track, (start + delta).clamp(low, high)))
        .collect()
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
        assert_eq!(volume_spoken(0.0), "minus infinity dB");
        assert_eq!(pan_text(0.0), "C");
        assert_eq!(pan_text(-0.5), "25L");
        assert_eq!(pan_text(1.0), "50R");
        assert_eq!(pan_text(0.004), "C");
    }

    // Spec: mixer-view R6.
    #[test]
    fn the_fader_law_runs_from_silence_to_its_top_and_back() {
        assert_eq!(fader_position(0.0), 0.0);
        assert!((fader_position(db_gain(TOP_DB)) - 1.0).abs() < 1e-12);
        assert_eq!(fader_position(100.0), 1.0);
        assert_eq!(fader_gain(0.0), 0.0);
        assert!((gain_db(fader_gain(1.0)) - TOP_DB).abs() < 1e-9);
        // 0 dB at 84% of the travel, -6 dB at 71%, -20 dB at 47%.
        assert!((fader_position(1.0) - 0.8409).abs() < 1e-3);
        assert!((fader_position(db_gain(-6.0)) - 0.7079).abs() < 1e-3);
        assert!((fader_position(db_gain(-20.0)) - 0.4729).abs() < 1e-3);
        // A round trip lands where it began, and the bottom step is silence's
        // neighbour.
        for db in [-60.0, -30.0, -6.0, 0.0, 3.0, 6.0] {
            let gain = db_gain(db);
            assert!(
                (fader_gain(fader_position(gain)) - gain).abs() < 1e-9,
                "{db}"
            );
        }
        assert_eq!(fader_gain(fader_position(db_gain(BOTTOM_DB - 1.0))), 0.0);
        // Monotonic.
        let mut last = -1.0;
        for i in 0..=1000 {
            let g = fader_gain(f64::from(i) / 1000.0);
            assert!(g >= last);
            last = g;
        }
    }

    // Spec: mixer-view R6.
    #[test]
    fn keys_step_volume_by_decibels_and_pan_by_units() {
        assert!((gain_db(step_volume(1.0, 1, false)) - 1.0).abs() < 1e-9);
        assert!((gain_db(step_volume(1.0, -3, false)) + 3.0).abs() < 1e-9);
        assert!((gain_db(step_volume(1.0, 1, true)) - 0.1).abs() < 1e-9);
        assert!((gain_db(step_volume(db_gain(5.5), 2, false)) - TOP_DB).abs() < 1e-9);
        assert!((gain_db(step_volume(0.0, 1, false)) - BOTTOM_DB).abs() < 1e-9);
        assert_eq!(step_volume(0.0, -1, false), 0.0);
        assert_eq!(step_volume(db_gain(BOTTOM_DB), -1, false), 0.0);
        assert_eq!(step_pan(0.0, 1), 0.02);
        assert_eq!(step_pan(0.99, 5), 1.0);
        assert_eq!(step_pan(-1.0, -1), -1.0);
    }

    // Spec: mixer-view R10.
    #[test]
    fn the_meter_scale_is_the_fader_s() {
        assert_eq!(meter_position(0.0), 0.0);
        assert!((f64::from(meter_position(1.0)) - fader_position(1.0)).abs() < 1e-6);
        assert_eq!(meter_position(10.0), 1.0);
    }
}

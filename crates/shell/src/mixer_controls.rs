//! The mixer's controls (WP-1.9): what a strip's or a track header's
//! volume, pan, activator, Solo, and Arm show, and the registered commands
//! each change runs (`mixer.*`, WP-1.6), worked out from the set and the
//! selection so the window and its tests share them. The rules are in the
//! mixer view spec (R6 to R10).
//!
//! The fader's range, its scale, how a drag moves it, its key steps, how a
//! drag moves pan, and how much finer Shift makes a pan drag were observed
//! in Live 12.4.6 on macOS on 11 October 2026 (the mixer view spec,
//! sources). Shift with a fader drag is assumed to match pan's until it's
//! observed.

use bl_model::{CrossfadeAssign, Document, TrackId};
use serde_json::json;

use crate::Invocation;

pub use bl_model::fader::{
    FADER_SCALE, FADER_TRAVEL, LOWEST_DB, SEND_LOWEST_DB, TOP_DB, db_gain, fader_gain,
    fader_position, gain_db, send_level, send_position, volume_gain,
};

/// The fader's travel in points, from −∞ to +6 dB: a drag moves the
/// handle one point per point of vertical drag along [`FADER_SCALE`]
/// (observed).
pub const FADER_TRAVEL_POINTS: f64 = 105.5;

/// How many pan steps there are to each side: pan reads 1L to 50L and 1R to
/// 50R.
pub const PAN_STEPS: f64 = 50.0;

/// Points of vertical drag that move pan from the centre to either end
/// (observed: linear, 0.8 steps of 50 per point, so 62.5 points from C to
/// 50R or 50L; up moves right).
pub const PAN_DRAG_TRAVEL: f32 = 62.5;

/// How much finer a drag is with Shift held (observed by hand on a pan
/// knob: about 1 cm up from C gave 9R with Shift and 29R without, one drag
/// each, so about three times; taken as 3). A fader drag is assumed to
/// match until it's observed.
pub const FINE: f32 = 3.0;

/// The undo step a volume drag makes, named as Live names it (tracks and
/// undo, item 10).
pub const VOLUME_GESTURE: &str = "Change \"Track Volume\"";

/// The undo step a pan drag makes.
pub const PAN_GESTURE: &str = "Change \"Track Panning\"";

/// The undo step a send drag makes. (Live's name for it isn't observed.)
pub const SEND_GESTURE: &str = "Change \"Send\"";

/// The undo step a crossfader drag makes. (Live's name for it isn't
/// observed; it matches the command's own.)
pub const CROSSFADER_GESTURE: &str = "Change crossfader";

/// The undo step a Cue Volume drag makes, named as Live names it (observed,
/// 18 October 2026).
pub const CUE_VOLUME_GESTURE: &str = "Change \"Preview Volume\"";

/// Points of vertical drag that turn a send from −∞ to 0 dB: the pan
/// knob's travel, as Live's send knob's isn't observed.
pub const SEND_DRAG_TRAVEL: f32 = PAN_DRAG_TRAVEL;

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

/// Volume as a screen reader reads it: the value as the Status Bar shows it
/// while the fader moves, `0.0 dB`, `-6.0 dB`, `-inf dB` (the
/// accessibility spec, R1; what Live's own accessibility tree reads is to
/// observe).
#[must_use]
pub fn volume_spoken(gain: f64) -> String {
    format!("{} dB", volume_text(gain))
}

/// What the Status Bar shows while a fader is dragged (observed:
/// `Track Volume: -21.0 dB`).
#[must_use]
pub fn volume_readout(gain: f64) -> String {
    format!("Track Volume: {} dB", volume_text(gain))
}

/// What the Status Bar shows while the Cue Volume knob is dragged (observed:
/// the knob is named Preview Volume in both modes).
#[must_use]
pub fn cue_volume_readout(gain: f64) -> String {
    format!("Preview Volume: {} dB", volume_text(gain))
}

/// The command that sets the Cue Volume to `gain`, kept within the knob's
/// range, −∞ to +6 dB.
#[must_use]
pub fn set_cue_volume(gain: f64) -> Invocation {
    Invocation::new(
        "mixer.set_cue_volume",
        json!({ "volume": gain.clamp(0.0, bl_model::MAX_CUE_VOLUME) }),
    )
}

/// The Cue Volume after a drag of `delta` points up from `start`, along the
/// fader's scale as the volume fader's drag moves it.
#[must_use]
pub fn cue_volume_moved(start: f64, delta: f64) -> Invocation {
    set_cue_volume(fader_gain(
        fader_position(start) + delta / FADER_TRAVEL_POINTS,
    ))
}

/// The Cue Volume after Up or Down: `steps` dB, tenths with `fine`, as the
/// volume fader's keys step.
#[must_use]
pub fn cue_volume_stepped(gain: f64, steps: i32, fine: bool) -> Invocation {
    set_cue_volume(step_volume(gain, steps, fine))
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

/// A send level `steps` dB away (tenths with `fine`), within −∞ to 0 dB,
/// stepping as volume does at the bottom.
#[must_use]
pub fn step_send(level: f64, steps: i32, fine: bool) -> f64 {
    step_volume(level, steps, fine).min(1.0)
}

/// A send level as the mixer reads it: dB to one decimal, or `-inf`.
#[must_use]
pub fn send_text(level: f64) -> String {
    volume_text(level)
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
    /// The send knob to this return track.
    Send(TrackId),
}

impl Control {
    /// The control's value on a track: the volume's gain, or the pan.
    #[must_use]
    pub fn value(self, doc: &Document, track: TrackId) -> Option<f64> {
        let t = doc.track(track)?;
        let mixer = t.mixer();
        Some(match self {
            Self::Volume => mixer.volume,
            Self::Pan => mixer.pan,
            Self::Send(to) => {
                let has = t.kind().has_sends()
                    && doc.track(to).map(bl_model::Track::kind)
                        == Some(bl_model::TrackKind::Return);
                if !has {
                    return None;
                }
                t.send(to)
            }
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
            Self::Send(to) => Invocation::new(
                "mixer.set_send",
                json!({ "track": track, "to": to, "level": value.clamp(0.0, 1.0) }),
            ),
        }
    }

    /// Points of vertical drag per unit a drag moves: volume moves by
    /// points of drag along [`FADER_SCALE`], pan by [`PAN_DRAG_TRAVEL`]
    /// points per unit.
    #[must_use]
    pub fn drag_travel(self) -> f32 {
        match self {
            Self::Volume => 1.0,
            Self::Pan => PAN_DRAG_TRAVEL,
            Self::Send(_) => SEND_DRAG_TRAVEL,
        }
    }

    /// The undo step a drag of it makes.
    #[must_use]
    pub fn gesture(self) -> &'static str {
        match self {
            Self::Volume => VOLUME_GESTURE,
            Self::Pan => PAN_GESTURE,
            Self::Send(_) => SEND_GESTURE,
        }
    }

    /// Its default: 0 dB (a double-click's reset, observed), the centre,
    /// or a send's −∞, where every send starts.
    #[must_use]
    pub fn default_value(self) -> f64 {
        match self {
            Self::Volume => 1.0,
            Self::Pan | Self::Send(_) => 0.0,
        }
    }

    /// `value` stepped by `steps` as Up and Down step it (tenths of a dB
    /// with `fine` for the dB controls).
    #[must_use]
    pub fn step(self, value: f64, steps: i32, fine: bool) -> f64 {
        match self {
            Self::Volume => step_volume(value, steps, fine),
            Self::Pan => step_pan(value, steps),
            Self::Send(_) => step_send(value, steps, fine),
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
/// first track's handle as far along [`FADER_SCALE`] and every other track
/// by the same dB; for pan, the same amount on each; for a send, knob
/// travel (0 to 1) on the first and the same dB on the others (R6, R8).
/// Every value stays within range.
#[must_use]
pub fn moved(control: Control, starts: &[(TrackId, f64)], delta: f64) -> Vec<Invocation> {
    let Some(&(_, first)) = starts.first() else {
        return Vec::new();
    };
    match control {
        Control::Volume => {
            let now = fader_gain(fader_position(first) + delta / FADER_TRAVEL_POINTS);
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
        Control::Send(_) => {
            let now = send_level(send_position(first) + delta);
            let shift = if now <= 0.0 {
                f64::NEG_INFINITY
            } else {
                gain_db(now) - gain_db(first).max(SEND_LOWEST_DB)
            };
            starts
                .iter()
                .enumerate()
                .map(|(i, (track, start))| {
                    let value = if i == 0 {
                        now
                    } else if shift == f64::NEG_INFINITY || *start <= 0.0 {
                        0.0
                    } else {
                        db_gain(gain_db(*start) + shift).min(1.0)
                    };
                    control.set(*track, value)
                })
                .collect()
        }
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

/// The crossfader's position as the Status Bar reads it while it's dragged
/// (observed: `Crossfade: 25A`), from 50A (0, the A end) through the centre
/// to 50B (1, the B end), counting steps from the centre. Live's own text
/// for the centre isn't observed; Beatlayer shows `C`, as pan's.
#[must_use]
pub fn crossfader_text(position: f64) -> String {
    let steps = (position.clamp(0.0, 1.0) * 100.0).round() as i64;
    match steps {
        50 => "C".to_owned(),
        n if n < 50 => format!("{}A", 50 - n),
        n => format!("{}B", n - 50),
    }
}

/// What the Status Bar shows while the crossfader is dragged (observed:
/// `Crossfade: 25A`).
#[must_use]
pub fn crossfader_readout(position: f64) -> String {
    format!("Crossfade: {}", crossfader_text(position))
}

/// The command a click on a track's crossfader A or B button runs (the
/// mixer spec, R23): the clicked side, or neither when that side is already
/// assigned, as the button toggles. The two sides are exclusive, so
/// assigning one releases the other. Nothing for a track without the
/// buttons, Main.
#[must_use]
pub fn crossfade_clicked(
    doc: &Document,
    track: TrackId,
    side: CrossfadeAssign,
) -> Option<Invocation> {
    let t = doc.track(track)?;
    if !t.kind().has_crossfade_assign() {
        return None;
    }
    let assign = if t.mixer().crossfade == side {
        CrossfadeAssign::None
    } else {
        side
    };
    Some(Invocation::new(
        "mixer.set_crossfade_assign",
        json!({ "track": track, "assign": assign }),
    ))
}

/// The crossfader's position after a drag of `dx` points on a fader `width`
/// points wide, from `start`: one point of drag per point of travel, within
/// 0 to 1. (Live's own crossfader drag isn't observed; this follows the
/// mixer's other faders, which drag relative to the pointer.)
#[must_use]
pub fn crossfader_moved(start: f64, dx: f32, width: f32) -> f64 {
    (start + f64::from(dx) / f64::from(width.max(1.0))).clamp(0.0, 1.0)
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
        assert_eq!(volume_spoken(0.0), "-inf dB");
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
    fn a_drag_moves_the_handle_a_point_per_point_along_the_scale() {
        let t = TrackId(bl_core::id::Id::new(9).unwrap());
        let from = |start: f64, up: f64| {
            moved(Control::Volume, &[(t, start)], up)[0].args["volume"]
                .as_f64()
                .unwrap()
        };
        // Observed from 0 dB: 6 points up +3.0 dB, 15 down −9.0, 30 down
        // −17.4, 45 down −26.0; from −9.0, 15 up 0 dB. Each landed on the
        // scale's own place for its value within a screenshot pixel (1.5
        // points), so within 0.5 dB here.
        for (up, expected) in [(6.0, 3.0), (-15.0, -9.0), (-30.0, -17.4), (-45.0, -26.0)] {
            let got = gain_db(from(1.0, up));
            assert!((got - expected).abs() <= 0.5, "{up}: {got}");
        }
        let got = gain_db(from(db_gain(-9.0), 15.0));
        assert!(got.abs() <= 0.5, "{got}");
        // One point of drag is one point of handle travel.
        let p0 = fader_position(1.0);
        assert!(
            (fader_position(from(1.0, -10.0)) - (p0 - 10.0 / FADER_TRAVEL_POINTS)).abs() < 1e-9
        );
        // The ends hold.
        assert_eq!(db(from(1.0, 500.0)), 6.0);
        assert_eq!(from(1.0, -500.0), 0.0);
        // The inverse lands where it began.
        for db_ in [6.0, 0.0, -12.0, -30.0, -60.0, -69.0] {
            let gain = db_gain(db_);
            assert!(
                (gain_db(fader_gain(fader_position(gain))) - db_).abs() < 1e-6,
                "{db_}"
            );
        }
        assert_eq!(fader_gain(0.0), 0.0);
    }

    // Spec: mixer-view R6.
    #[test]
    fn pan_drags_at_0_8_steps_a_point() {
        let t = TrackId(bl_core::id::Id::new(9).unwrap());
        let pan = |up: f32| {
            moved(
                Control::Pan,
                &[(t, 0.0)],
                f64::from(up / Control::Pan.drag_travel()),
            )[0]
            .args["pan"]
                .as_f64()
                .unwrap()
        };
        // Observed from C: 15 points up 12R, 30 up 24R, 60 up 48R, 30 down
        // 24L.
        for (up, expected) in [(15.0, "12R"), (30.0, "24R"), (60.0, "48R"), (-30.0, "24L")] {
            assert_eq!(pan_text(pan(up)), expected, "{up}");
        }
        assert_eq!(pan(62.5), 1.0);
        assert_eq!(pan(100.0), 1.0);
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

    // Spec: mixer-view R6, R23.
    #[test]
    fn the_crossfader_reads_as_live_writes_it_and_a_click_toggles_a_side() {
        assert_eq!(crossfader_text(0.0), "50A");
        assert_eq!(crossfader_text(1.0), "50B");
        assert_eq!(crossfader_text(0.5), "C");
        assert_eq!(crossfader_text(0.75), "25B");
        assert_eq!(crossfader_text(0.22), "28A");
        assert_eq!(crossfader_readout(0.25), "Crossfade: 25A");
        // One point of drag is one step of 50, and the ends hold.
        let moved = crossfader_moved(0.0, 20.0, 100.0);
        assert!((moved - 0.2).abs() < 1e-9);
        assert_eq!(crossfader_moved(0.5, 1e9, 100.0), 1.0);
        assert_eq!(crossfader_moved(0.5, -1e9, 100.0), 0.0);

        // A click on a side assigns it; on the lit side it releases the
        // track; the sides are exclusive, so the assignment is one value.
        let mut editor = bl_model::Editor::new();
        let id: TrackId = editor.new_id();
        editor
            .apply(bl_model::Command::CreateTrack {
                id,
                kind: bl_model::TrackKind::Audio,
                name: String::new(),
                parent: None,
                index: 0,
            })
            .unwrap();
        let assign =
            |doc: &Document, side| crossfade_clicked(doc, id, side).unwrap().args["assign"].clone();
        assert_eq!(assign(editor.document(), CrossfadeAssign::A), json!("a"));
        editor
            .apply(bl_model::Command::SetCrossfadeAssign {
                track: id,
                assign: CrossfadeAssign::A,
            })
            .unwrap();
        assert_eq!(
            assign(editor.document(), CrossfadeAssign::A),
            json!("none"),
            "the lit side releases the track"
        );
        assert_eq!(assign(editor.document(), CrossfadeAssign::B), json!("b"));
        // Main has no buttons.
        let main = editor.document().main_id();
        assert!(crossfade_clicked(editor.document(), main, CrossfadeAssign::A).is_none());
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
        let up = (fader_position(db_gain(-46.0)) - fader_position(start)) * FADER_TRAVEL_POINTS;
        let changes = moved(Control::Volume, &[(a, start), (b, db_gain(-3.0))], up);
        let get = |i: usize| changes[i].args["volume"].as_f64().unwrap();
        assert_eq!(db(get(0)), -46.0);
        assert_eq!(db(get(1)), 4.0);
        assert_eq!(db(shift_volume(db_gain(5.0), 3.0)), 6.0);
        assert_eq!(shift_volume(0.0, 3.0), 0.0);
        assert_eq!(shift_volume(db_gain(-69.0), -1.0), 0.0);
    }
}

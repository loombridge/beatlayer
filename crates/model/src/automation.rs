//! Automation: a track's envelopes, one per automatable parameter (WP-2.12).
//!
//! An envelope is a list of breakpoints joined by straight or curved
//! segments, at musical positions: the value between two of them follows the
//! segment's shape, and before the first and after the last the nearest one's
//! value holds. Every control of a track's mixer, every send, and every
//! parameter of the devices in its chain can have one, and one only per
//! parameter: no two breakpoints share a position.
//!
//! A segment is curved by a signed bend, from −1 to 1, stored on the
//! breakpoint at the segment's start; 0 is the straight line. Between the two
//! breakpoints the value follows a power curve, so it stays between theirs.
//! The law is Beatlayer's own: Live's shape isn't measured. The rules, and
//! the Live sources behind them, are in the automation spec.
//!
//! A parameter whose envelope is on follows it while the song plays. Moving
//! the control by hand turns its envelope off, so the control's own value
//! overrides it until Re-Enable Automation runs; the envelope itself stays.

use bl_core::time::Ticks;
use imbl::OrdMap;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::ModelError;
use crate::ids::{ChainId, DeviceId, ParamId, TrackId};

/// One of a track's mixer controls.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
#[schemars(
    description = "A track's mixer control: `volume` (linear gain, 1 is 0 dB), `pan` (−1 \
                   left to 1 right), or `mute` (the Track Activator: 1 muted, 0 not)."
)]
pub enum MixerParam {
    /// The track's volume, as linear gain.
    Volume,
    /// The track's pan, from −1 (left) to 1 (right).
    Pan,
    /// The Track Activator: 1 muted, 0 not.
    Mute,
}

/// A parameter of a track that automation can drive: a mixer control, a send
/// to a return track, or a parameter of a device in the track's chain.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(tag = "type", rename_all = "snake_case")]
#[schemars(
    description = "Which parameter of the track an envelope drives: `mixer` with a `param`, \
                   `send` with the return track it feeds (`to`), `device` with the device \
                   and its `param`, or `chain` with a rack's chain and its mixer `param`."
)]
pub enum AutomationTarget {
    /// A control of the track's mixer.
    Mixer {
        /// The control.
        param: MixerParam,
    },
    /// The level of the track's send to a return track.
    Send {
        /// The return track the send feeds.
        to: TrackId,
    },
    /// A parameter of a device in the track's chain.
    Device {
        /// The device.
        device: DeviceId,
        /// Its parameter.
        param: ParamId,
    },
    /// A control of a rack chain's mixer (WP-5.1, G2): the chain's Volume,
    /// Pan, or Chain Activator (`mute`: 1 muted, 0 not).
    Chain {
        /// The chain.
        chain: ChainId,
        /// The control.
        param: MixerParam,
    },
}

/// One breakpoint of an envelope: a position, the value there, and the bend
/// of the segment starting at it. The last breakpoint has no segment after
/// it, so its bend shapes nothing.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Breakpoint {
    /// Where it sits.
    pub at: Ticks,
    /// The value there, in the parameter's own units.
    pub value: f64,
    /// The bend of the segment starting here, from −1 to 1; 0 is the
    /// straight line.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub bend: f64,
}

/// For serde's `skip_serializing_if`, which passes a reference.
fn is_zero(value: &f64) -> bool {
    *value == 0.0
}

/// The value `along` of the way from `from` to `to`, on a segment whose bend
/// is `bend`: the straight line when the bend is 0, and otherwise the power
/// curve `t^(2^bend)`, `t` running 0 to 1 along the segment. The value stays
/// between the two, so a sign chooses which side the segment bows to. The
/// engine's read path calls this frame by frame, so it allocates nothing.
#[must_use]
pub fn curve_value(from: f64, to: f64, along: f64, bend: f64) -> f64 {
    let along = if bend == 0.0 {
        along
    } else {
        along.powf(2f64.powf(bend))
    };
    from + (to - from) * along
}

/// Why a bend isn't valid: it must be a number from −1 to 1. What a loaded
/// breakpoint's bend and `automation.set_curve` must pass.
pub(crate) fn check_bend(bend: f64) -> Result<(), ModelError> {
    if (-1.0..=1.0).contains(&bend) {
        Ok(())
    } else {
        Err(ModelError::InvalidValue {
            what: "automation bend",
            value: bend,
        })
    }
}

/// Why a list of breakpoints isn't valid: one before zero, one that isn't
/// after the one before it, a value or a bend that isn't in range. What a
/// loaded envelope must pass.
pub(crate) fn check_points(points: &[Breakpoint]) -> Result<(), ModelError> {
    for (index, point) in points.iter().enumerate() {
        if point.at < Ticks::ZERO {
            return Err(ModelError::NegativePosition(point.at));
        }
        if index > 0 && point.at <= points[index - 1].at {
            return Err(ModelError::ChangeInUse {
                what: "breakpoint",
                at: point.at,
            });
        }
        if !point.value.is_finite() {
            return Err(ModelError::InvalidValue {
                what: "automation value",
                value: point.value,
            });
        }
        check_bend(point.bend)?;
    }
    Ok(())
}

/// A parameter's envelope: breakpoints joined by straight or curved
/// segments, in position order.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Envelope {
    /// Its breakpoints, in order; no two share a position.
    points: Vec<Breakpoint>,
}

/// Why an edit to an envelope was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PointError {
    /// No breakpoint is at this position.
    Missing(Ticks),
    /// Another breakpoint is already at this position.
    Taken(Ticks),
    /// No segment starts at this position: it's the last breakpoint.
    NoSegment(Ticks),
}

/// The value a list of breakpoints holds at `at`: the shape of the segment
/// between the breakpoints around it, the first one's value before it, and
/// the last one's after it. `None` for no breakpoints. The position is a
/// number of ticks, so a clip envelope (WP-3.15) can be read at a position
/// between two ticks; the read path calls this frame by frame, so it
/// allocates nothing.
#[must_use]
pub fn envelope_value(points: &[Breakpoint], at: f64) -> Option<f64> {
    let first = *points.first()?;
    let last = *points.last().expect("checked");
    if at <= first.at.0 as f64 {
        return Some(first.value);
    }
    if at >= last.at.0 as f64 {
        return Some(last.value);
    }
    let index = points.partition_point(|point| (point.at.0 as f64) <= at) - 1;
    let start = points[index];
    let end = points[index + 1];
    let along = (at - start.at.0 as f64) / (end.at.0 - start.at.0) as f64;
    Some(curve_value(start.value, end.value, along, start.bend))
}

impl Envelope {
    /// Replaces every breakpoint's value by `f` of it, leaving positions and
    /// bends alone.
    pub(crate) fn map_values(&mut self, f: impl Fn(f64) -> f64) {
        for point in &mut self.points {
            point.value = f(point.value);
        }
    }

    /// Its breakpoints, in position order.
    pub fn points(&self) -> impl ExactSizeIterator<Item = Breakpoint> + '_ {
        self.points.iter().copied()
    }

    /// Whether it holds no breakpoints.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    /// The breakpoint at `at`, if there is one.
    #[must_use]
    pub fn point(&self, at: Ticks) -> Option<Breakpoint> {
        let index = self
            .points
            .binary_search_by_key(&at, |point| point.at)
            .ok()?;
        Some(self.points[index])
    }

    /// The value it holds at `at`: the shape of the segment between the
    /// breakpoints around it, the first one's value before it, and the last
    /// one's after it. `None` for an empty envelope.
    #[must_use]
    pub fn value_at(&self, at: Ticks) -> Option<f64> {
        envelope_value(&self.points, at.0 as f64)
    }

    /// What recording a touch changes (WP-2.12): `written` is what the
    /// control was moved to, as positions and values in position order, and
    /// `end` is where it was released. Returns the positions of the
    /// breakpoints the touch replaces, which are the envelope's from the
    /// first written position up to `end`, and the breakpoints to set. When
    /// the envelope has points and `end` is past the last written position,
    /// a breakpoint at `end` takes the envelope back to the value it had
    /// there, so the old envelope resumes; with none, the last value written
    /// holds. An `end` before the last written position (the playhead
    /// wrapped round the loop before recording stopped) ends the touch at
    /// its last move.
    #[must_use]
    pub fn touch(&self, written: &[(Ticks, f64)], end: Ticks) -> (Vec<Ticks>, Vec<(Ticks, f64)>) {
        let (Some(first), Some(last)) = (written.first(), written.last()) else {
            return (Vec::new(), Vec::new());
        };
        let end = end.max(last.0);
        let removed = self
            .points()
            .map(|point| point.at)
            .filter(|at| *at >= first.0 && *at < end)
            .collect();
        let mut set = written.to_vec();
        if end > last.0
            && self.point(end).is_none()
            && let Some(value) = self.value_at(end)
        {
            set.push((end, value));
        }
        (removed, set)
    }

    /// The envelope `points` make, or why they don't: they must be in
    /// position order, no two at one position, each value finite, and each
    /// bend from −1 to 1. What a loaded envelope must pass.
    pub fn from_points(points: Vec<Breakpoint>) -> Result<Self, ModelError> {
        check_points(&points)?;
        Ok(Self { points })
    }

    /// Why its breakpoints aren't valid: one before zero, one that isn't
    /// after the one before it, a value or a bend that isn't in range. What
    /// a loaded envelope must pass (WP-3.15).
    pub(crate) fn check(&self) -> Result<(), ModelError> {
        check_points(&self.points)
    }

    /// Adds a breakpoint at `at`, or replaces the value of the one already
    /// there. A new breakpoint starts a straight segment.
    pub(crate) fn set(&mut self, at: Ticks, value: f64) {
        match self.points.binary_search_by_key(&at, |point| point.at) {
            Ok(index) => self.points[index].value = value,
            Err(index) => self.points.insert(
                index,
                Breakpoint {
                    at,
                    value,
                    bend: 0.0,
                },
            ),
        }
    }

    /// Bends the segment starting at the breakpoint at `at`, leaving the
    /// breakpoints where they are.
    pub(crate) fn set_bend(&mut self, at: Ticks, bend: f64) -> Result<(), PointError> {
        let index = self
            .points
            .binary_search_by_key(&at, |point| point.at)
            .map_err(|_| PointError::Missing(at))?;
        if index + 1 >= self.points.len() {
            return Err(PointError::NoSegment(at));
        }
        self.points[index].bend = bend;
        Ok(())
    }

    /// Moves the breakpoint at `at` to `to`, keeping its value and its bend.
    pub(crate) fn move_point(&mut self, at: Ticks, to: Ticks) -> Result<(), PointError> {
        let index = self
            .points
            .binary_search_by_key(&at, |point| point.at)
            .map_err(|_| PointError::Missing(at))?;
        if to != at
            && self
                .points
                .binary_search_by_key(&to, |point| point.at)
                .is_ok()
        {
            return Err(PointError::Taken(to));
        }
        let mut point = self.points.remove(index);
        point.at = to;
        match self.points.binary_search_by_key(&to, |point| point.at) {
            Ok(index) => self.points[index] = point,
            Err(index) => self.points.insert(index, point),
        }
        Ok(())
    }

    /// Removes the breakpoint at `at`.
    pub(crate) fn delete(&mut self, at: Ticks) -> Result<(), PointError> {
        let index = self
            .points
            .binary_search_by_key(&at, |point| point.at)
            .map_err(|_| PointError::Missing(at))?;
        self.points.remove(index);
        Ok(())
    }
}

/// A parameter's automation: its envelope, and whether the envelope drives
/// the parameter.
#[derive(Clone, Debug, PartialEq)]
pub struct Automation {
    /// The envelope.
    pub envelope: Envelope,
    /// Whether the envelope drives the parameter. A hand move turns it off,
    /// so the control's own value overrides the envelope, until Re-Enable
    /// Automation turns it on again.
    pub enabled: bool,
}

impl Automation {
    /// An envelope driving its parameter.
    #[must_use]
    pub fn new(envelope: Envelope) -> Self {
        Self {
            envelope,
            enabled: true,
        }
    }
}

/// A track's automation, one entry per parameter, in target order.
pub type TrackAutomation = OrdMap<AutomationTarget, Automation>;

/// The entry at `target` in `automation`, adding an empty one if there's
/// none: for an edit that gives a parameter its first breakpoint.
pub(crate) fn entry(automation: &mut TrackAutomation, target: AutomationTarget) -> &mut Automation {
    automation
        .entry(target)
        .or_insert_with(|| Automation::new(Envelope::default()))
}

/// Turns `target`'s envelope off in `automation`, if it's on: what moving a
/// control by hand does. Nothing if the parameter has no envelope.
pub(crate) fn override_target(automation: &mut TrackAutomation, target: &AutomationTarget) {
    if let Some(automation) = automation.get_mut(target) {
        automation.enabled = false;
    }
}

/// Whether the parameter at `target` has an envelope that's turned off.
#[must_use]
pub fn is_overridden(automation: &TrackAutomation, target: &AutomationTarget) -> bool {
    automation
        .get(target)
        .is_some_and(|automation| !automation.enabled)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_envelope_holds_nothing() {
        let envelope = Envelope::default();
        assert!(envelope.is_empty());
        assert_eq!(envelope.value_at(Ticks::ZERO), None);
        assert_eq!(envelope.points().len(), 0);
    }

    #[test]
    fn a_touch_replaces_the_old_points_it_covers_and_the_old_envelope_resumes_at_its_end() {
        let mut old = Envelope::default();
        old.set(Ticks::ZERO, 0.0);
        old.set(Ticks::WHOLE * 2, 1.0);
        old.set(Ticks::WHOLE * 4, 0.0);
        let written = [(Ticks::WHOLE, 0.9), (Ticks::WHOLE + Ticks::QUARTER, 0.8)];
        let (removed, set) = old.touch(&written, Ticks::WHOLE * 3);
        // The point at 2 wholes is inside the touch; the ones outside stay.
        assert_eq!(removed, vec![Ticks::WHOLE * 2]);
        // The release goes back to what the old envelope had there: half way
        // down the segment from 1.0 at 2 wholes to 0.0 at 4.
        assert_eq!(set, [written[0], written[1], (Ticks::WHOLE * 3, 0.5)]);
        // With no old envelope the last value written holds: no release point.
        let (removed, set) = Envelope::default().touch(&written, Ticks::WHOLE * 3);
        assert!(removed.is_empty());
        assert_eq!(set, written);
        assert_eq!(old.touch(&[], Ticks::WHOLE), (vec![], vec![]));
        // An end before the last move (the loop wrapped) ends the touch at
        // its last move: the old points it covers still go.
        let written = [(Ticks::WHOLE, 0.9), (Ticks::WHOLE * 3, 0.8)];
        let (removed, set) = old.touch(&written, Ticks::ZERO);
        assert_eq!(removed, vec![Ticks::WHOLE * 2]);
        assert_eq!(set, written);
    }

    #[test]
    fn a_breakpoint_holds_before_it_and_after_it_and_the_line_between() {
        let mut envelope = Envelope::default();
        envelope.set(Ticks::WHOLE, 1.0);
        assert_eq!(envelope.value_at(Ticks::ZERO), Some(1.0));
        assert_eq!(envelope.value_at(Ticks::WHOLE * 4), Some(1.0));
        envelope.set(Ticks::WHOLE * 2, 0.0);
        assert_eq!(envelope.value_at(Ticks::ZERO), Some(1.0));
        assert_eq!(envelope.value_at(Ticks::WHOLE), Some(1.0));
        assert_eq!(
            envelope.value_at(Ticks::WHOLE + Ticks::WHOLE / 2),
            Some(0.5)
        );
        assert_eq!(envelope.value_at(Ticks::WHOLE * 2), Some(0.0));
        assert_eq!(envelope.value_at(Ticks::WHOLE * 3), Some(0.0));
        // The exact value at each breakpoint, whatever the line's slope.
        let mut exact = Envelope::default();
        exact.set(Ticks::ZERO, 0.1);
        exact.set(Ticks(3), 0.3);
        assert_eq!(exact.value_at(Ticks(3)), Some(0.3));
    }

    #[test]
    fn setting_a_breakpoint_again_replaces_its_value() {
        let mut envelope = Envelope::default();
        envelope.set(Ticks::WHOLE, 1.0);
        envelope.set(Ticks::WHOLE, 0.25);
        assert_eq!(
            envelope.points().collect::<Vec<_>>(),
            vec![Breakpoint {
                at: Ticks::WHOLE,
                value: 0.25,
                bend: 0.0
            }]
        );
    }

    #[test]
    fn moving_a_breakpoint_keeps_its_value_its_bend_and_the_order() {
        let mut envelope = Envelope::default();
        envelope.set(Ticks::ZERO, 0.0);
        envelope.set(Ticks::WHOLE, 1.0);
        envelope.set(Ticks::WHOLE * 2, 0.5);
        envelope.set_bend(Ticks::ZERO, -0.5).unwrap();
        envelope.move_point(Ticks::WHOLE, Ticks::WHOLE * 3).unwrap();
        assert_eq!(
            envelope.points().collect::<Vec<_>>(),
            vec![
                Breakpoint {
                    at: Ticks::ZERO,
                    value: 0.0,
                    bend: -0.5
                },
                Breakpoint {
                    at: Ticks::WHOLE * 2,
                    value: 0.5,
                    bend: 0.0
                },
                Breakpoint {
                    at: Ticks::WHOLE * 3,
                    value: 1.0,
                    bend: 0.0
                }
            ]
        );
        assert_eq!(
            envelope.move_point(Ticks::WHOLE, Ticks(1)),
            Err(PointError::Missing(Ticks::WHOLE))
        );
        assert_eq!(
            envelope.move_point(Ticks::ZERO, Ticks::WHOLE * 2),
            Err(PointError::Taken(Ticks::WHOLE * 2))
        );
        // Moving a breakpoint where it already is changes nothing.
        assert_eq!(envelope.move_point(Ticks::ZERO, Ticks::ZERO), Ok(()));
    }

    // Spec: automation R12
    #[test]
    fn a_curved_segment_follows_its_bend() {
        let mut envelope = Envelope::default();
        envelope.set(Ticks::ZERO, 0.0);
        envelope.set(Ticks::WHOLE, 1.0);
        assert_eq!(
            envelope.value_at(Ticks::WHOLE / 2),
            Some(0.5),
            "straight with no bend"
        );
        // A bend of −1 bows the segment up: half way along, the power curve
        // √t leads the straight line.
        envelope.set_bend(Ticks::ZERO, -1.0).unwrap();
        let half = envelope.value_at(Ticks::WHOLE / 2).unwrap();
        assert!((half - 0.5f64.sqrt()).abs() < 1e-12, "{half}");
        assert!(half > 0.5);
        // The bend of 1 mirrors it, and 0 straightens the segment again.
        envelope.set_bend(Ticks::ZERO, 1.0).unwrap();
        let half = envelope.value_at(Ticks::WHOLE / 2).unwrap();
        assert!((half - 0.25).abs() < 1e-12, "{half}");
        envelope.set_bend(Ticks::ZERO, 0.0).unwrap();
        assert_eq!(envelope.value_at(Ticks::WHOLE / 2), Some(0.5));
        // Each breakpoint's own value holds whatever the bend.
        assert_eq!(envelope.value_at(Ticks::ZERO), Some(0.0));
        assert_eq!(envelope.value_at(Ticks::WHOLE), Some(1.0));
        // A curve stays between the two values: it's the same power curve
        // whichever way round they are, so it never overshoots.
        let mut falling = Envelope::default();
        falling.set(Ticks::ZERO, 1.0);
        falling.set(Ticks::WHOLE, 0.0);
        falling.set_bend(Ticks::ZERO, -1.0).unwrap();
        for at in 0..=Ticks::WHOLE.0 {
            let value = falling.value_at(Ticks(at)).unwrap();
            assert!((0.0..=1.0).contains(&value), "at {at}: {value}");
        }
    }

    #[test]
    fn a_bend_needs_a_breakpoint_and_a_segment_after_it() {
        let mut envelope = Envelope::default();
        envelope.set(Ticks::ZERO, 0.0);
        envelope.set(Ticks::WHOLE, 1.0);
        assert_eq!(
            envelope.set_bend(Ticks::QUARTER, 0.5),
            Err(PointError::Missing(Ticks::QUARTER)),
            "no breakpoint there"
        );
        assert_eq!(
            envelope.set_bend(Ticks::WHOLE, 0.5),
            Err(PointError::NoSegment(Ticks::WHOLE)),
            "the last breakpoint has no segment after it"
        );
    }

    #[test]
    fn bad_points_and_bends_dont_load() {
        let point = |at, value, bend| Breakpoint { at, value, bend };
        assert_eq!(
            Envelope::from_points(vec![point(Ticks::ZERO, 0.0, 0.0)])
                .unwrap()
                .point(Ticks::ZERO),
            Some(point(Ticks::ZERO, 0.0, 0.0))
        );
        assert_eq!(
            Envelope::from_points(vec![point(Ticks::ZERO, 1.0, 2.0)]),
            Err(ModelError::InvalidValue {
                what: "automation bend",
                value: 2.0,
            })
        );
        assert!(Envelope::from_points(vec![point(Ticks::ZERO, 1.0, f64::NAN)]).is_err());
    }

    #[test]
    fn deleting_a_breakpoint_joins_its_neighbours() {
        let mut envelope = Envelope::default();
        envelope.set(Ticks::ZERO, 0.0);
        envelope.set(Ticks::WHOLE, 0.5);
        envelope.set(Ticks::WHOLE * 2, 1.0);
        // The segment the deletion makes starts at the breakpoint before the
        // one removed, so its bend is that breakpoint's.
        envelope.set_bend(Ticks::ZERO, -1.0).unwrap();
        envelope.delete(Ticks::WHOLE).unwrap();
        assert_eq!(
            envelope.points().collect::<Vec<_>>(),
            vec![
                Breakpoint {
                    at: Ticks::ZERO,
                    value: 0.0,
                    bend: -1.0
                },
                Breakpoint {
                    at: Ticks::WHOLE * 2,
                    value: 1.0,
                    bend: 0.0
                }
            ]
        );
        let half = envelope.value_at(Ticks::WHOLE).unwrap();
        assert!((half - 0.5f64.sqrt()).abs() < 1e-12, "{half}");
        assert_eq!(
            envelope.delete(Ticks::WHOLE),
            Err(PointError::Missing(Ticks::WHOLE))
        );
    }
}

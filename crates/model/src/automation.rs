//! Automation: a track's envelopes, one per automatable parameter (WP-2.12).
//!
//! An envelope is a list of breakpoints joined by straight segments, at
//! musical positions: the value between two of them follows the line, and
//! before the first and after the last the nearest one's value holds. Every
//! control of a track's mixer, every send, and every parameter of the devices
//! in its chain can have one, and one only per parameter: no two breakpoints
//! share a position.
//!
//! A parameter whose envelope is on follows it while the song plays. Moving
//! the control by hand turns its envelope off, so the control's own value
//! overrides it until Re-Enable Automation runs; the envelope itself stays.
//! The rules, and the Live sources behind them, are in the automation spec.

use bl_core::time::Ticks;
use imbl::OrdMap;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::ModelError;
use crate::ids::{DeviceId, ParamId, TrackId};

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
                   `send` with the return track it feeds (`to`), or `device` with the device \
                   and its `param`."
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
}

/// Why a list of breakpoints isn't valid: one before zero, one that isn't
/// after the one before it, or a value that isn't finite. What a loaded
/// envelope must pass.
pub(crate) fn check_points(points: &[(Ticks, f64)]) -> Result<(), ModelError> {
    for (index, &(at, value)) in points.iter().enumerate() {
        if at < Ticks::ZERO {
            return Err(ModelError::NegativePosition(at));
        }
        if index > 0 && at <= points[index - 1].0 {
            return Err(ModelError::ChangeInUse {
                what: "breakpoint",
                at,
            });
        }
        if !value.is_finite() {
            return Err(ModelError::InvalidValue {
                what: "automation value",
                value,
            });
        }
    }
    Ok(())
}

/// A parameter's envelope: breakpoints joined by straight segments, in
/// position order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Envelope {
    /// Its breakpoints, in order; no two share a position.
    points: Vec<(Ticks, f64)>,
}

/// Why an edit to an envelope was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PointError {
    /// No breakpoint is at this position.
    Missing(Ticks),
    /// Another breakpoint is already at this position.
    Taken(Ticks),
}

impl Envelope {
    /// Its breakpoints, in position order.
    pub fn points(&self) -> impl ExactSizeIterator<Item = (Ticks, f64)> + '_ {
        self.points.iter().copied()
    }

    /// Whether it holds no breakpoints.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    /// The breakpoint at `at`, if there is one.
    #[must_use]
    pub fn point(&self, at: Ticks) -> Option<(Ticks, f64)> {
        let index = self.points.binary_search_by_key(&at, |&(t, _)| t).ok()?;
        Some(self.points[index])
    }

    /// The value it holds at `at`: the line between the breakpoints around
    /// it, the first one's value before it, and the last one's after it.
    /// `None` for an empty envelope.
    #[must_use]
    pub fn value_at(&self, at: Ticks) -> Option<f64> {
        let first = *self.points.first()?;
        let last = *self.points.last().expect("checked");
        if at <= first.0 {
            return Some(first.1);
        }
        if at >= last.0 {
            return Some(last.1);
        }
        let index = self.points.partition_point(|&(t, _)| t <= at) - 1;
        let (start, from) = self.points[index];
        let (end, to) = self.points[index + 1];
        let along = (at.0 - start.0) as f64 / (end.0 - start.0) as f64;
        Some(from + (to - from) * along)
    }

    /// The envelope `points` make, or why they don't: they must be in
    /// position order, no two at one position, and each value finite. What
    /// a loaded envelope must pass.
    pub fn from_points(points: Vec<(Ticks, f64)>) -> Result<Self, ModelError> {
        check_points(&points)?;
        Ok(Self { points })
    }

    /// Adds a breakpoint at `at`, or replaces the one already there.
    pub(crate) fn set(&mut self, at: Ticks, value: f64) {
        match self.points.binary_search_by_key(&at, |&(t, _)| t) {
            Ok(index) => self.points[index].1 = value,
            Err(index) => self.points.insert(index, (at, value)),
        }
    }

    /// Moves the breakpoint at `at` to `to`, keeping its value.
    pub(crate) fn move_point(&mut self, at: Ticks, to: Ticks) -> Result<(), PointError> {
        let index = self
            .points
            .binary_search_by_key(&at, |&(t, _)| t)
            .map_err(|_| PointError::Missing(at))?;
        if to != at && self.points.binary_search_by_key(&to, |&(t, _)| t).is_ok() {
            return Err(PointError::Taken(to));
        }
        let point = self.points.remove(index);
        match self.points.binary_search_by_key(&to, |&(t, _)| t) {
            Ok(index) => self.points[index].1 = point.1,
            Err(index) => self.points.insert(index, (to, point.1)),
        }
        Ok(())
    }

    /// Removes the breakpoint at `at`.
    pub(crate) fn delete(&mut self, at: Ticks) -> Result<(), PointError> {
        let index = self
            .points
            .binary_search_by_key(&at, |&(t, _)| t)
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

/// Drops every envelope of `device`'s parameters from `automation`: what
/// deleting the device, or moving it to another track, does. A device's
/// automation belongs to the track it's in.
pub(crate) fn drop_device(automation: &mut TrackAutomation, device: DeviceId) {
    let targets: Vec<AutomationTarget> = automation
        .keys()
        .filter(
            |target| matches!(target, AutomationTarget::Device { device: d, .. } if *d == device),
        )
        .copied()
        .collect();
    for target in targets {
        automation.remove(&target);
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
            vec![(Ticks::WHOLE, 0.25)]
        );
    }

    #[test]
    fn moving_a_breakpoint_keeps_its_value_and_the_order() {
        let mut envelope = Envelope::default();
        envelope.set(Ticks::ZERO, 0.0);
        envelope.set(Ticks::WHOLE, 1.0);
        envelope.set(Ticks::WHOLE * 2, 0.5);
        envelope.move_point(Ticks::WHOLE, Ticks::WHOLE * 3).unwrap();
        assert_eq!(
            envelope.points().collect::<Vec<_>>(),
            vec![
                (Ticks::ZERO, 0.0),
                (Ticks::WHOLE * 2, 0.5),
                (Ticks::WHOLE * 3, 1.0)
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

    #[test]
    fn deleting_a_breakpoint_joins_its_neighbours() {
        let mut envelope = Envelope::default();
        envelope.set(Ticks::ZERO, 0.0);
        envelope.set(Ticks::WHOLE, 1.0);
        envelope.set(Ticks::WHOLE * 2, 0.0);
        envelope.delete(Ticks::WHOLE).unwrap();
        assert_eq!(
            envelope.points().collect::<Vec<_>>(),
            vec![(Ticks::ZERO, 0.0), (Ticks::WHOLE * 2, 0.0)]
        );
        assert_eq!(
            envelope.delete(Ticks::WHOLE),
            Err(PointError::Missing(Ticks::WHOLE))
        );
    }
}

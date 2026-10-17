//! Relative modulation's laws (the modulation spec, R3 and R4): what a
//! parameter's travel is, what a clip's modulation envelope contributes to
//! it, and the value it plays when the contributions are added to its base.
//!
//! Everything here is plain arithmetic on numbers, so the audio thread reads
//! it frame by frame without allocating.

use crate::fader::{LOWEST_DB, db_gain, fader_gain, fader_position, gain_db};
use crate::param::ParamDescriptor;

/// The travel a parameter's control moves along, from 0 to 1.
#[derive(Clone, Copy, Debug)]
pub enum Travel<'a> {
    /// A track's volume: the fader's scale.
    Fader,
    /// A send's level: the knob's scale, straight in dB from the lowest
    /// finite level to 0 dB.
    Send,
    /// A track's pan, from −1 (left) at 0 to 1 (right) at 1.
    Pan,
    /// A switch with no descriptor, such as a track's activator: off at 0,
    /// on at 1.
    Switch,
    /// A parameter a descriptor describes, on its own taper.
    Param(&'a ParamDescriptor),
}

impl Travel<'_> {
    /// Where `value` sits on the travel.
    #[must_use]
    pub fn position(self, value: f64) -> f64 {
        match self {
            Self::Fader => fader_position(value),
            Self::Send => {
                if value <= 0.0 {
                    0.0
                } else {
                    ((gain_db(value) - LOWEST_DB) / -LOWEST_DB).clamp(0.0, 1.0)
                }
            }
            Self::Pan => ((value.clamp(-1.0, 1.0)) + 1.0) / 2.0,
            Self::Switch => value.clamp(0.0, 1.0),
            Self::Param(descriptor) => descriptor.normalize(value),
        }
    }

    /// The value at `position` on the travel: the inverse of
    /// [`position`](Self::position), on the nearest step for a stepped
    /// parameter, and silence at the bottom of a level's travel.
    #[must_use]
    pub fn value(self, position: f64) -> f64 {
        let position = position.clamp(0.0, 1.0);
        match self {
            Self::Fader => fader_gain(position),
            Self::Send => {
                if position <= 0.0 {
                    0.0
                } else {
                    db_gain(LOWEST_DB * (1.0 - position))
                }
            }
            Self::Pan => position * 2.0 - 1.0,
            Self::Switch => position.round(),
            Self::Param(descriptor) => descriptor.denormalize(position),
        }
    }

    /// The range of the values a clip's modulation envelope holds for this
    /// travel (R4): 0 to 1 (0 to 100 %) for a level, −1 to 1 for the rest.
    #[must_use]
    pub fn clip_range(self) -> (f64, f64) {
        match self {
            Self::Fader | Self::Send => (0.0, 1.0),
            Self::Pan | Self::Switch | Self::Param(_) => (-1.0, 1.0),
        }
    }

    /// What a clip's modulation envelope at `share` contributes to a base of
    /// `base`, as a share of the travel (R4): for a level, the drop that
    /// brings its position to `share` of the base's; for pan, `share` of the
    /// distance from the base to the nearer end; for the rest, `share` of the
    /// travel itself. A share outside the envelope's range is clamped to it.
    // Spec: modulation R4
    #[must_use]
    pub fn clip_contribution(self, base: f64, share: f64) -> f64 {
        let (low, high) = self.clip_range();
        let share = if share.is_nan() {
            0.0
        } else {
            share.clamp(low, high)
        };
        match self {
            Self::Fader | Self::Send => (share - 1.0) * self.position(base),
            Self::Pan => share * (1.0 - base.clamp(-1.0, 1.0).abs()) / 2.0,
            Self::Switch | Self::Param(_) => share,
        }
    }

    /// The value played with `sum`, the contributions added, moving the
    /// base's position on the travel, clamped to the travel's ends (R3). No
    /// contribution gives the base itself, to the bit.
    // Spec: modulation R3
    #[must_use]
    pub fn played(self, base: f64, sum: f64) -> f64 {
        if sum == 0.0 {
            return base;
        }
        self.value(self.position(base) + sum)
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::ids::ParamId;
    use crate::param::ParamUnit;

    fn cutoff() -> ParamDescriptor {
        ParamDescriptor::continuous(
            ParamId(0),
            "Cutoff",
            (20.0, 20_000.0),
            1_000.0,
            ParamUnit::Hertz,
        )
        .logarithmic()
    }

    // Spec: modulation R3
    #[test]
    fn contributions_add_in_the_travel_and_clamp() {
        let descriptor = ParamDescriptor::continuous(ParamId(0), "Mix", (0.0, 1.0), 0.5, ParamUnit::None);
        let travel = Travel::Param(&descriptor);
        assert_eq!(travel.played(0.5, 0.25 + 0.125), 0.875);
        assert_eq!(travel.played(0.5, 0.25 - 0.25), 0.5, "they cancel");
        assert_eq!(travel.played(0.5, 4.0), 1.0, "clamped at the top");
        assert_eq!(travel.played(0.5, -4.0), 0.0, "and at the bottom");
        // A logarithmic taper adds in its own travel: a quarter of the
        // travel up from 1 kHz is 1 kHz · 1000^0.25.
        let log = cutoff();
        let up = Travel::Param(&log).played(1_000.0, 0.25);
        assert!((up - 1_000.0 * 1000f64.powf(0.25)).abs() < 1e-6, "{up}");
    }

    // Spec: modulation R3
    #[test]
    fn a_stepped_parameter_plays_the_nearest_step() {
        let mode = ParamDescriptor::choice(ParamId(0), "Mode", &["A", "B", "C", "D", "E"], 0);
        let travel = Travel::Param(&mode);
        assert_eq!(travel.played(0.0, 0.3), 1.0, "1.2 steps is step 1");
        assert_eq!(travel.played(0.0, 0.4), 2.0, "1.6 steps is step 2");
        assert_eq!(travel.played(2.0, -1.0), 0.0);
    }

    // Spec: modulation R4
    #[test]
    fn a_level_is_only_brought_down_to_silence() {
        for travel in [Travel::Fader, Travel::Send] {
            let base = travel.value(0.8);
            let at = |share: f64| travel.played(base, travel.clip_contribution(base, share));
            assert_eq!(at(1.0), base, "100 % leaves the level");
            assert_eq!(at(0.0), 0.0, "0 % is silence (−∞ dB)");
            assert!(at(0.5) < base && at(0.5) > 0.0);
            assert_eq!(at(7.0), base, "a share past 100 % never raises it");
            let position = travel.position(at(0.5));
            assert!((position - 0.4).abs() < 0.01, "half the position: {position}");
        }
    }

    // Spec: modulation R4
    #[test]
    fn pan_reach_shrinks_towards_the_ends() {
        let at = |base: f64, share: f64| {
            Travel::Pan.played(base, Travel::Pan.clip_contribution(base, share))
        };
        assert_eq!(at(0.0, 1.0), 1.0, "from the centre it reaches hard right");
        assert_eq!(at(0.0, -1.0), -1.0, "and hard left");
        assert!((at(0.5, 1.0) - 1.0).abs() < 1e-12);
        assert!((at(0.5, 0.5) - 0.75).abs() < 1e-12, "half the way to the end");
        assert_eq!(at(1.0, 1.0), 1.0, "at the end it doesn't move");
        assert_eq!(at(-1.0, -1.0), -1.0);
    }

    // Spec: modulation R4
    #[test]
    fn a_device_control_moves_relative_to_its_base() {
        let descriptor = cutoff();
        let travel = Travel::Param(&descriptor);
        let base = 1_000.0;
        assert_eq!(travel.played(base, travel.clip_contribution(base, 0.0)), base);
        let up = travel.played(base, travel.clip_contribution(base, 1.0));
        assert!(up > base);
        assert_eq!(up, 20_000.0, "a full share clamps at the top");
        assert_eq!(travel.clip_contribution(base, 3.0), 1.0, "clamped to 100 %");
    }

    proptest! {
        // Spec: modulation R3, R4
        #[test]
        fn a_level_never_rises_above_its_base(position in 0.0f64..=1.0, share in -2.0f64..=3.0) {
            for travel in [Travel::Fader, Travel::Send] {
                let base = travel.value(position);
                let played = travel.played(base, travel.clip_contribution(base, share));
                prop_assert!(played <= base + 1e-12 && played >= 0.0, "{played} vs {base}");
            }
        }

        #[test]
        fn pan_stays_inside_its_range_and_an_end_pan_stays_put(base in -1.0f64..=1.0, share in -2.0f64..=2.0) {
            let played = Travel::Pan.played(base, Travel::Pan.clip_contribution(base, share));
            prop_assert!((-1.0..=1.0).contains(&played));
            if base.abs() == 1.0 {
                prop_assert!((played - base).abs() < 1e-12);
            }
        }
    }
}

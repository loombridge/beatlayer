//! Relative modulation's laws (the modulation spec, R3 and R4): what a
//! parameter's travel is, what a clip's modulation envelope contributes to
//! it, and the value it plays when the contributions are added to its base.
//!
//! Everything here is plain arithmetic on numbers, so the audio thread reads
//! it frame by frame without allocating.

use crate::automation::AutomationTarget;
use crate::device::Device;
use crate::document::Document;
use crate::fader::{LOWEST_DB, db_gain, fader_gain, fader_position, gain_db};
use crate::ids::{ParamId, TrackId};
use crate::param::ParamDescriptor;
use crate::shared::Shared;

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

    /// The share a clip's modulation envelope holds (R4) that plays, over
    /// `control`, the value an envelope of `old` played before relative
    /// modulation, which added `old` to the control's value in the
    /// parameter's own units and clamped (R5). The share is clamped to the
    /// envelope's range.
    // Spec: modulation R5
    #[must_use]
    pub fn legacy_share(self, control: f64, old: f64) -> f64 {
        let (low, high) = self.clip_range();
        let played = self.position(control + old);
        let at = self.position(control);
        let share = match self {
            Self::Fader | Self::Send => {
                if at > 0.0 {
                    played / at
                } else {
                    0.0
                }
            }
            Self::Pan => {
                let base = control.clamp(-1.0, 1.0);
                if base.abs() < 1.0 {
                    ((control + old).clamp(-1.0, 1.0) - base) / (1.0 - base.abs())
                } else {
                    0.0
                }
            }
            Self::Switch | Self::Param(_) => played - at,
        };
        if share.is_nan() {
            0.0
        } else {
            share.clamp(low, high)
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

impl Document {
    /// Turns the clip envelopes of a set saved before relative modulation
    /// into the modulation that plays what they played, at the control
    /// values the set holds (R5). `descriptor` gives a device's parameter
    /// its description; a parameter it doesn't know keeps its values,
    /// clamped to the envelope's range.
    pub fn relative_clip_envelopes(
        &mut self,
        descriptor: &dyn Fn(&Device, ParamId) -> Option<ParamDescriptor>,
    ) {
        let ids: Vec<TrackId> = self.tracks.keys().copied().collect();
        for id in ids {
            let Some(track) = self.tracks.get_mut(&id).map(Shared::make_mut) else {
                continue;
            };
            let clips: Vec<_> = track
                .clips()
                .chain(track.slots().map(|(_, clip)| clip))
                .filter(|clip| clip.envelopes().len() > 0)
                .map(|clip| clip.id)
                .collect();
            for clip in clips {
                let Some(mut envelopes) = track.clip(clip).map(|clip| clip.envelopes.clone())
                else {
                    continue;
                };
                for envelope in &mut envelopes {
                    envelope.envelope.map_values(|old| match envelope.target {
                        AutomationTarget::Mixer { param } => {
                            let mixer = &track.mixer;
                            match param {
                                crate::MixerParam::Volume => {
                                    Travel::Fader.legacy_share(mixer.volume, old)
                                }
                                crate::MixerParam::Pan => Travel::Pan.legacy_share(mixer.pan, old),
                                crate::MixerParam::Mute => Travel::Switch
                                    .legacy_share(f64::from(u8::from(mixer.mute)), old),
                            }
                        }
                        AutomationTarget::Send { to } => {
                            Travel::Send.legacy_share(track.send(to), old)
                        }
                        AutomationTarget::Device { device, param } => track
                            .devices()
                            .find(|d| d.id == device)
                            .and_then(|d| Some((d, descriptor(d, param)?)))
                            .map_or_else(
                                || old.clamp(-1.0, 1.0),
                                |(d, description)| {
                                    let control = d
                                        .params
                                        .get(&param)
                                        .copied()
                                        .unwrap_or(description.default);
                                    Travel::Param(&description).legacy_share(control, old)
                                },
                            ),
                    });
                }
                if let Some(clip) = track.clip_mut(clip) {
                    clip.envelopes = envelopes;
                }
            }
        }
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
        let descriptor =
            ParamDescriptor::continuous(ParamId(0), "Mix", (0.0, 1.0), 0.5, ParamUnit::None);
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
            assert!(
                (position - 0.4).abs() < 0.01,
                "half the position: {position}"
            );
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
        assert!(
            (at(0.5, 0.5) - 0.75).abs() < 1e-12,
            "half the way to the end"
        );
        assert_eq!(at(1.0, 1.0), 1.0, "at the end it doesn't move");
        assert_eq!(at(-1.0, -1.0), -1.0);
    }

    // Spec: modulation R4
    #[test]
    fn a_device_control_moves_relative_to_its_base() {
        let descriptor = cutoff();
        let travel = Travel::Param(&descriptor);
        let base = 1_000.0;
        assert_eq!(
            travel.played(base, travel.clip_contribution(base, 0.0)),
            base
        );
        let up = travel.played(base, travel.clip_contribution(base, 1.0));
        assert!(up > base);
        assert_eq!(up, 20_000.0, "a full share clamps at the top");
        assert_eq!(travel.clip_contribution(base, 3.0), 1.0, "clamped to 100 %");
    }

    // Spec: modulation R5
    #[test]
    fn clip_envelopes_from_before_relative_modulation_play_as_they_did() {
        let played = |travel: Travel<'_>, control: f64, old: f64| {
            let share = travel.legacy_share(control, old);
            travel.played(control, travel.clip_contribution(control, share))
        };
        let close = |a: f64, b: f64| (a - b).abs() < 1e-3;
        assert!(
            close(played(Travel::Fader, 0.5, -0.25), 0.25),
            "a fader lowered"
        );
        assert!(close(played(Travel::Send, 0.5, -0.25), 0.25));
        assert!(close(played(Travel::Pan, 0.2, 0.5), 0.7));
        assert!(close(played(Travel::Pan, -0.4, -0.3), -0.7));
        let descriptor = cutoff();
        let travel = Travel::Param(&descriptor);
        assert!((played(travel, 1_000.0, 500.0) - 1_500.0).abs() < 1e-6);
        assert!((played(travel, 1_000.0, -500.0) - 500.0).abs() < 1e-6);
        // What the new range can't hold is clamped to it: a level can only
        // be brought down, so an old raise plays the control.
        assert_eq!(Travel::Fader.legacy_share(0.5, 0.0), 1.0);
        assert_eq!(
            Travel::Fader.legacy_share(0.5, 3.0),
            1.0,
            "a raise plays the control"
        );
        assert_eq!(
            Travel::Fader.legacy_share(0.0, 0.5),
            0.0,
            "a closed fader stays closed"
        );
        assert_eq!(
            Travel::Pan.legacy_share(1.0, -0.5),
            0.0,
            "an end pan doesn't move"
        );
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

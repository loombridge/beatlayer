//! A device's sidechain in the engine (WP-2.19b; the routing spec): what
//! its detector hears.
//!
//! The tracks routed into a device's sidechain input, and the track its
//! sidechain panel takes as Audio From, are summed into a buffer of the
//! device's own, each through a route. Just before the device runs, its
//! [`SidechainKey`] turns that sum into what the detector hears: while the
//! Sidechain switch is on, the sum scaled by the sidechain's gain and mixed
//! with the device's own input by its mix; while it's off, nothing, and the
//! device listens to its own input. With Listen on (and the switch on), the
//! device's output is replaced by what its detector heard.
//!
//! Gain and mix glide over [`SMOOTHING_SECONDS`](crate::mixer::SMOOTHING_SECONDS),
//! as the mixer's gains do; the switch and Listen take effect at once.
//! Nothing here allocates.

use bl_model::ParamId;

use crate::mixer::{Ramp, smoothing_frames};
use crate::node::{NodeConfig, NodeEvent, ParamValue, segments};

/// The Sidechain switch: 1 on, 0 off (the default).
pub const ENABLED: ParamId = ParamId(0);
/// The sidechain's gain, linear: 1 by default.
pub const GAIN: ParamId = ParamId(1);
/// How much of the detector's signal is the sidechain, from 0 to 1: 1 by
/// default.
pub const MIX: ParamId = ParamId(2);
/// Sidechain Listen: 1 on, 0 off (the default).
pub const LISTEN: ParamId = ParamId(3);

/// A device's sidechain settings in a program: what turns the sum of its
/// sidechain's sources into what its detector hears.
#[derive(Clone, Debug)]
pub struct SidechainKey {
    enabled: bool,
    listen: bool,
    gain: Ramp,
    mix: Ramp,
    gain_target: f64,
    mix_target: f64,
    smoothing: u32,
}

impl SidechainKey {
    /// A sidechain set as `sidechain` says, resting there.
    #[must_use]
    pub fn new(sidechain: &bl_model::Sidechain, config: &NodeConfig) -> Self {
        let gain = clean(sidechain.gain, 1.0);
        let mix = clean(sidechain.mix, 1.0).min(1.0);
        Self {
            enabled: sidechain.enabled,
            listen: sidechain.listen,
            gain: Ramp::at(gain),
            mix: Ramp::at(mix),
            gain_target: gain,
            mix_target: mix,
            smoothing: smoothing_frames(config.sample_rate),
        }
    }

    /// Whether the Sidechain switch is on: the device's detector hears the
    /// key.
    #[must_use]
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Whether the device's output is what its detector hears.
    #[must_use]
    pub fn listening(&self) -> bool {
        self.enabled && self.listen
    }

    fn set(&mut self, param: ParamId, value: ParamValue) {
        let switch = |value| matches!(value, ParamValue::Set(v) if v != 0.0);
        match param {
            ENABLED => self.enabled = switch(value),
            LISTEN => self.listen = switch(value),
            GAIN => {
                self.gain_target = level(value, f64::MAX);
                self.gain.toward(self.gain_target, self.smoothing);
            }
            MIX => {
                self.mix_target = level(value, 1.0);
                self.mix.toward(self.mix_target, self.smoothing);
            }
            _ => {}
        }
    }

    /// Takes parameter changes without making a key, for a device that's
    /// switched off.
    pub(crate) fn flush(&mut self, events: &[NodeEvent]) {
        for event in events {
            self.set(event.param, event.value);
        }
        self.gain = Ramp::at(self.gain_target);
        self.mix = Ramp::at(self.mix_target);
    }

    /// Turns `key`, holding the sum of the sidechain's sources, into what
    /// the detector hears: `gain · mix · key + (1 − mix) · own`, where `own`
    /// is the device's input. Both are planar, `channels` channels `stride`
    /// apart; `events` take effect on their frames. Real-time safe.
    pub(crate) fn make(
        &mut self,
        key: &mut [f32],
        own: &[f32],
        channels: usize,
        stride: usize,
        frames: usize,
        events: &[NodeEvent],
    ) {
        for segment in segments(events, frames) {
            for event in segment.events {
                self.set(event.param, event.value);
            }
            let (mut gain, mut mix) = (self.gain, self.mix);
            for c in 0..channels {
                (gain, mix) = (self.gain, self.mix);
                let range = c * stride + segment.range.start..c * stride + segment.range.end;
                for (k, o) in key[range.clone()].iter_mut().zip(&own[range]) {
                    let (g, m) = (gain.next(), mix.next());
                    *k = (g * m) as f32 * *k + (1.0 - m) as f32 * *o;
                }
            }
            self.gain = gain;
            self.mix = mix;
        }
    }
}

/// A finite value at least 0, or `default`.
fn clean(value: f64, default: f64) -> f64 {
    if value.is_finite() && value >= 0.0 {
        value
    } else {
        default
    }
}

/// A parameter's value as a level from 0 to `max`, 1 by default.
fn level(value: ParamValue, max: f64) -> f64 {
    match value {
        ParamValue::Set(v) => clean(v, 1.0).min(max),
        ParamValue::Default => 1.0,
    }
}

#[cfg(test)]
mod tests {
    use bl_core::time::SampleRate;

    use super::*;

    fn config() -> NodeConfig {
        NodeConfig {
            sample_rate: SampleRate::HZ_48000,
            max_frames: 8,
            channels: 2,
        }
    }

    #[test]
    fn the_key_is_the_sidechain_scaled_and_mixed_with_the_devices_input() {
        let settings = bl_model::Sidechain {
            enabled: true,
            gain: 2.0,
            mix: 0.25,
            ..bl_model::Sidechain::default()
        };
        let mut key = SidechainKey::new(&settings, &config());
        assert!(key.enabled() && !key.listening());
        let mut sum = vec![1.0f32; 16];
        let own = vec![4.0f32; 16];
        key.make(&mut sum, &own, 2, 8, 8, &[]);
        // 2 · 0.25 · 1 + 0.75 · 4
        assert!(sum.iter().all(|&s| s == 3.5), "{sum:?}");
    }
}

//! Routing to external hardware (WP-5.34): an external instrument's output
//! port, channel, audio inputs and Hardware Latency
//! (`docs/specs/external-devices.md`, R9, R12).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::track::Channel;

/// MIDI To in a change to an external device's routing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
#[schemars(
    description = "An external device's MIDI To: `none` (No Output), or `port` with the MIDI \
                   output port's `name` as the MIDI settings list it."
)]
pub enum ExternalPort {
    /// No Output.
    None,
    /// An output port, by name.
    Port {
        /// The port's name.
        name: String,
    },
}

/// The most Hardware Latency in milliseconds (`docs/specs/external-devices.md`,
/// R12).
pub const MAX_HARDWARE_LATENCY_MS: f64 = 1000.0;

/// The most Hardware Latency in samples.
pub const MAX_HARDWARE_LATENCY_SAMPLES: u32 = 192_000;

/// How much later than the audio interface's own round trip a device's
/// sound comes back, in the unit its unit switch is on.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "unit", rename_all = "snake_case")]
#[schemars(
    description = "Hardware Latency: `{ \"unit\": \"milliseconds\", \"value\": 12.5 }` (0 to \
                   1000, two decimals; kept in milliseconds when the sample rate changes) or \
                   `{ \"unit\": \"samples\", \"value\": 600 }` (0 to 192000; kept in samples)."
)]
pub enum HardwareLatency {
    /// Milliseconds, 0 to 1000: for analogue connections.
    Milliseconds {
        /// The delay.
        value: f64,
    },
    /// Samples, 0 to 192,000: for digital connections.
    Samples {
        /// The delay.
        value: u32,
    },
}

impl Default for HardwareLatency {
    /// A new device's: 0 ms.
    fn default() -> Self {
        Self::Milliseconds { value: 0.0 }
    }
}

impl HardwareLatency {
    /// Whether it is the default, 0 ms.
    #[must_use]
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// Whether the value is in range (and finite).
    #[must_use]
    pub fn is_valid(&self) -> bool {
        match *self {
            Self::Milliseconds { value } => (0.0..=MAX_HARDWARE_LATENCY_MS).contains(&value),
            Self::Samples { value } => value <= MAX_HARDWARE_LATENCY_SAMPLES,
        }
    }

    /// The delay in whole frames at `sample_rate` hertz (R12).
    #[must_use]
    pub fn frames(&self, sample_rate: u32) -> u32 {
        match *self {
            Self::Milliseconds { value } => {
                (value * f64::from(sample_rate) / 1000.0).round().max(0.0) as u32
            }
            Self::Samples { value } => value,
        }
    }

    /// The same delay in the other unit at `sample_rate`, to whole samples
    /// when it becomes samples and to two decimals when it becomes
    /// milliseconds (R12: switching the unit converts the value).
    #[must_use]
    pub fn switched(&self, sample_rate: u32) -> Self {
        match *self {
            Self::Milliseconds { .. } => Self::Samples {
                value: self.frames(sample_rate),
            },
            Self::Samples { value } => Self::Milliseconds {
                value: (f64::from(value) * 1000.0 / f64::from(sample_rate.max(1)) * 100.0).round()
                    / 100.0,
            },
        }
    }
}

/// A device's routing to external hardware: where an external instrument
/// sends its notes and where its sound comes back from
/// (`docs/specs/external-devices.md`, R9, R12).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(
    description = "An external device's routing: `port` (MIDI To, the MIDI output port's name \
                   as the MIDI settings list it; absent for No Output), `channel` (the MIDI \
                   channel, 1 to 16), `audio_from` (the audio interface's inputs it comes back \
                   on, a channel or a pair), and `hardware_latency`."
)]
pub struct External {
    /// MIDI To: the output port, by name, or `None` for No Output (a new
    /// device's). The device keeps the name while the port is away.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<String>,
    /// The MIDI channel, 1 to 16: 1 on a new device.
    #[serde(default = "first_channel", skip_serializing_if = "is_first_channel")]
    pub channel: u8,
    /// Audio From: the interface's inputs the sound comes back on, the first
    /// pair on a new device.
    #[serde(default, skip_serializing_if = "Channel::is_first_pair")]
    pub audio_from: Channel,
    /// Hardware Latency: 0 ms on a new device.
    #[serde(default, skip_serializing_if = "HardwareLatency::is_default")]
    pub hardware_latency: HardwareLatency,
}

fn first_channel() -> u8 {
    1
}

fn is_first_channel(channel: &u8) -> bool {
    *channel == 1
}

impl Default for External {
    /// A new device's: No Output, channel 1, the first input pair, 0 ms.
    fn default() -> Self {
        Self {
            port: None,
            channel: 1,
            audio_from: Channel::default(),
            hardware_latency: HardwareLatency::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Spec: external-devices R12
    #[test]
    fn hardware_latency_keeps_its_unit_across_a_rate_change() {
        let ms = HardwareLatency::Milliseconds { value: 10.0 };
        assert_eq!(ms.frames(48_000), 480);
        assert_eq!(ms.frames(96_000), 960, "milliseconds follow the rate");
        let samples = HardwareLatency::Samples { value: 480 };
        assert_eq!(samples.frames(48_000), 480);
        assert_eq!(samples.frames(96_000), 480, "samples keep their count");
        // Switching the unit converts at the current rate, to whole samples.
        assert_eq!(ms.switched(44_100), HardwareLatency::Samples { value: 441 });
        assert_eq!(
            HardwareLatency::Milliseconds { value: 0.333 }.switched(48_000),
            HardwareLatency::Samples { value: 16 }
        );
        assert_eq!(
            samples.switched(48_000),
            HardwareLatency::Milliseconds { value: 10.0 }
        );
    }

    #[test]
    fn hardware_latency_ranges_are_checked() {
        assert!(HardwareLatency::Milliseconds { value: 1000.0 }.is_valid());
        assert!(!HardwareLatency::Milliseconds { value: 1000.01 }.is_valid());
        assert!(!HardwareLatency::Milliseconds { value: -0.5 }.is_valid());
        assert!(!HardwareLatency::Milliseconds { value: f64::NAN }.is_valid());
        assert!(HardwareLatency::Samples { value: 192_000 }.is_valid());
        assert!(!HardwareLatency::Samples { value: 192_001 }.is_valid());
    }

    #[test]
    fn a_new_devices_routing_is_left_out_of_the_saved_set() {
        assert_eq!(
            serde_json::to_value(External::default()).unwrap(),
            serde_json::json!({})
        );
        let kept = External {
            port: Some("Synth".to_owned()),
            channel: 4,
            audio_from: Channel::Mono(3),
            hardware_latency: HardwareLatency::Samples { value: 9 },
        };
        let saved = serde_json::to_value(&kept).unwrap();
        assert_eq!(serde_json::from_value::<External>(saved).unwrap(), kept);
    }
}

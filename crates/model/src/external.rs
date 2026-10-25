//! Routing to external hardware (WP-5.34): an external instrument's output
//! port, channel, audio inputs and Hardware Latency
//! (`docs/specs/external-devices.md`, R9, R12), and the interface channels a
//! CV device's control voltages use (`docs/specs/cv-tools.md`, R3, R4, R14).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::mpe_settings::MpeSettings;
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

/// The most CV outputs or inputs one device has (`docs/specs/cv-tools.md`,
/// R3): CV Triggers has eight.
pub const MAX_CV_PORTS: usize = 8;

/// One CV output or input of a device and the interface channel it is set
/// to (`docs/specs/cv-tools.md`, R3, R4).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[schemars(
    description = "A CV output or input by its `name` in the device (Pitch, Gate, Mod 1 and so \
                   on) and the interface's `channel` it is set to, counted from 1; absent for \
                   No Output or No Input."
)]
pub struct CvPort {
    /// The name the device gives it.
    pub name: String,
    /// The interface channel, from 1, or `None` for No Output or No Input.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<u16>,
}

impl CvPort {
    /// A port with no channel chosen, as a new device has.
    #[must_use]
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            channel: None,
        }
    }
}

/// One step of a CV Instrument's calibration: the voltage written and the
/// pitch the oscillator gave back (`docs/specs/cv-tools.md`, R14).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CvStep {
    /// The voltage written on Pitch.
    pub volts: f64,
    /// The pitch measured, in hertz.
    pub hz: f64,
    /// How far the measured pitch is from the fitted line, in cents.
    pub error_cents: f64,
}

/// A CV Instrument's calibration of the oscillator patched to Pitch
/// (`docs/specs/cv-tools.md`, R14): the line fitted through the steps on
/// the logarithm of frequency, and the steps, whose remaining error the
/// pitch law corrects.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(
    description = "A calibration: `octaves_per_volt` and `hz_at_zero_volts` of the fitted line \
                   and the `steps` it was fitted to, each with the pitch it gave and its error \
                   in cents."
)]
pub struct CvCalibration {
    /// The octaves the oscillator rises per volt.
    pub octaves_per_volt: f64,
    /// The frequency the fitted line gives at 0 V.
    pub hz_at_zero_volts: f64,
    /// The steps, by rising voltage.
    pub steps: Vec<CvStep>,
}

impl CvCalibration {
    /// The line through `measured`, as `(volts, hertz)` pairs by rising
    /// voltage, fitted by least squares on the logarithm of frequency. At
    /// least two pairs with distinct voltages and positive frequencies.
    #[must_use]
    pub fn fit(measured: &[(f64, f64)]) -> Option<Self> {
        let n = measured.len() as f64;
        if measured.len() < 2 || measured.iter().any(|&(_, hz)| hz.is_nan() || hz <= 0.0) {
            return None;
        }
        let (sv, sl) = measured.iter().fold((0.0, 0.0), |(v, l), &(volts, hz)| {
            (v + volts, l + hz.log2())
        });
        let (mv, ml) = (sv / n, sl / n);
        let (cov, var) = measured.iter().fold((0.0, 0.0), |(c, v), &(volts, hz)| {
            (
                c + (volts - mv) * (hz.log2() - ml),
                v + (volts - mv) * (volts - mv),
            )
        });
        if var <= 0.0 {
            return None;
        }
        let slope = cov / var;
        let intercept = ml - slope * mv;
        Some(Self {
            octaves_per_volt: slope,
            hz_at_zero_volts: intercept.exp2(),
            steps: measured
                .iter()
                .map(|&(volts, hz)| CvStep {
                    volts,
                    hz,
                    error_cents: 1200.0 * (hz.log2() - (intercept + slope * volts)),
                })
                .collect(),
        })
    }

    /// The calibration of the steps `pitches` measured, in hertz from the
    /// voltage `first` in whole volts, 0 for a step that gave no steady pitch
    /// (`docs/specs/cv-tools.md`, R14). It fits the longest run of steps,
    /// three or more, that each give a pitch rising by half an octave to one
    /// and a half over the step before it as an oscillator's does: the
    /// detector reads only 41 Hz to 2 kHz, and mistakes what lies above for
    /// a lower octave, so the steps outside what it reads are left out.
    ///
    /// # Errors
    ///
    /// The first step that gave no pitch, or the first step if there are
    /// pitches but no run of three.
    pub fn from_steps(first: f64, pitches: &[f64]) -> Result<Self, usize> {
        let rising = |at: usize| {
            let ratio = pitches[at] / pitches[at - 1];
            pitches[at - 1] > 0.0 && (2f64.powf(0.5)..=2f64.powf(1.5)).contains(&ratio)
        };
        let (mut best, mut start) = ((0, 0), 0);
        for (at, &pitch) in pitches.iter().enumerate() {
            if pitch <= 0.0 {
                start = at + 1;
            } else if at > start && !rising(at) {
                start = at;
            }
            if at + 1 - start > best.1 && pitch > 0.0 {
                best = (start, at + 1 - start);
            }
        }
        let (start, len) = best;
        let measured: Vec<(f64, f64)> = (start..start + len)
            .map(|at| (first + at as f64, pitches[at]))
            .collect();
        if len < 3 {
            return Err(pitches.iter().position(|&p| p <= 0.0).unwrap_or(0));
        }
        Self::fit(&measured).ok_or(0)
    }

    /// How far above the fitted line, in octaves, the oscillator sounds at
    /// `volts`: straight between the steps' errors, and none past them.
    fn residue(&self, volts: f64) -> f64 {
        let (Some(first), Some(last)) = (self.steps.first(), self.steps.last()) else {
            return 0.0;
        };
        if volts < first.volts || volts > last.volts {
            return 0.0;
        }
        self.steps
            .windows(2)
            .find(|pair| volts <= pair[1].volts)
            .map_or(0.0, |pair| {
                let share = (volts - pair[0].volts) / (pair[1].volts - pair[0].volts);
                (pair[0].error_cents + share * (pair[1].error_cents - pair[0].error_cents)) / 1200.0
            })
    }

    /// The voltage that makes the oscillator sound `hz` (R12).
    #[must_use]
    pub fn volts_for(&self, hz: f64) -> f64 {
        let target = hz.max(f64::MIN_POSITIVE).log2() - self.hz_at_zero_volts.log2();
        let mut volts = target / self.octaves_per_volt;
        // The residue is a few cents against a line of octaves, so the
        // correction settles in a few turns.
        for _ in 0..4 {
            volts = (target - self.residue(volts)) / self.octaves_per_volt;
        }
        volts
    }

    /// The numbers that identify the calibration, so a node made for another
    /// one is made again.
    #[must_use]
    pub fn key(&self) -> Vec<u64> {
        [self.octaves_per_volt, self.hz_at_zero_volts]
            .into_iter()
            .chain(self.steps.iter().flat_map(|s| [s.volts, s.hz]))
            .map(f64::to_bits)
            .collect()
    }
}

/// A device's routing to external hardware: where an external instrument
/// sends its notes and where its sound comes back from
/// (`docs/specs/external-devices.md`, R9, R12).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(
    description = "An external device's routing: `port` (MIDI To, the MIDI output port's name \
                   as the MIDI settings list it; absent for No Output), `channel` (the MIDI \
                   channel, 1 to 16), `mpe` (the MPE/Multi-channel Settings when MPE is chosen \
                   in place of a channel), `audio_from` (the audio interface's inputs it comes back \
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
    /// MPE, chosen after the 16 channels in the channel chooser, with the
    /// MPE/Multi-channel Settings the notes go out on (the MPE spec, R19).
    /// `None` sends on `channel`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mpe: Option<MpeSettings>,
    /// Audio From: the interface's inputs the sound comes back on, the first
    /// pair on a new device.
    #[serde(default, skip_serializing_if = "Channel::is_first_pair")]
    pub audio_from: Channel,
    /// Audio To: the interface's outputs an external effect sends its signal
    /// to, the first pair on a new effect. `None` for an external
    /// instrument: it sends notes, not audio.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_to: Option<Channel>,
    /// Hardware Latency: 0 ms on a new device.
    #[serde(default, skip_serializing_if = "HardwareLatency::is_default")]
    pub hardware_latency: HardwareLatency,
    /// The CV outputs the device declares, in its order, and the interface
    /// channel each is set to (`docs/specs/cv-tools.md`, R3, R4). Empty for a
    /// device without CV, and then left out of the saved set.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cv_outputs: Vec<CvPort>,
    /// The CV inputs the device declares, as `cv_outputs`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cv_inputs: Vec<CvPort>,
    /// The calibration a CV Instrument holds (R14), if it has been
    /// calibrated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cv_calibration: Option<CvCalibration>,
}

impl External {
    /// Audio From set to No Input, as a new CV device has it: channel 0, which
    /// names no interface channel (`docs/specs/cv-tools.md`, R10).
    pub const NO_INPUT: Channel = Channel::Mono(0);

    /// Whether the device is a CV device: it declares CV outputs or inputs
    /// and has no MIDI To.
    #[must_use]
    pub fn is_cv(&self) -> bool {
        !self.cv_outputs.is_empty() || !self.cv_inputs.is_empty()
    }

    /// Whether a CV device reaches the interface: a CV output or input is set
    /// to a channel, or its Audio From to an input (`docs/specs/cv-tools.md`,
    /// R7).
    #[must_use]
    pub fn cv_routed(&self) -> bool {
        self.is_cv()
            && (self
                .cv_outputs
                .iter()
                .chain(&self.cv_inputs)
                .any(|port| port.channel.is_some())
                || self.audio_from != Self::NO_INPUT)
    }
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
            mpe: None,
            audio_from: Channel::default(),
            audio_to: None,
            hardware_latency: HardwareLatency::default(),
            cv_outputs: Vec::new(),
            cv_inputs: Vec::new(),
            cv_calibration: None,
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
            mpe: Some(crate::MpeSettings::default()),
            audio_from: Channel::Mono(3),
            audio_to: Some(Channel::Pair(2)),
            hardware_latency: HardwareLatency::Samples { value: 9 },
            cv_outputs: vec![CvPort {
                name: "Pitch".to_owned(),
                channel: Some(3),
            }],
            cv_inputs: vec![CvPort::new("CV")],
            cv_calibration: CvCalibration::fit(&[(0.0, 100.0), (1.0, 200.0)]),
        };
        let saved = serde_json::to_value(&kept).unwrap();
        assert_eq!(serde_json::from_value::<External>(saved).unwrap(), kept);
    }

    // Spec: cv-tools R14
    #[test]
    fn a_calibration_fits_the_line_and_the_pitch_law_corrects_the_steps() {
        // 0.97 octaves a volt from 110 Hz at 0 V, with a slight bow.
        let hz = |v: f64| 110.0 * (0.97 * v + 0.002 * (v * v - 3.0 * v)).exp2();
        let measured: Vec<(f64, f64)> =
            (-2..=5).map(|v| (f64::from(v), hz(f64::from(v)))).collect();
        let calibration = CvCalibration::fit(&measured).unwrap();
        assert!((calibration.octaves_per_volt - 0.97).abs() < 0.02);
        // The voltage it picks makes the oscillator sound the frequency
        // within a cent, at the steps and between them.
        for target in [60.0, 130.8, 261.6, 440.0, 880.0, 1500.0] {
            let volts = calibration.volts_for(target);
            let cents = 1200.0 * (hz(volts) / target).log2();
            assert!(cents.abs() < 1.0, "{target} Hz is {cents} cents off");
        }
        assert!(CvCalibration::fit(&[(0.0, 100.0)]).is_none());
    }

    // Spec: cv-tools R14
    #[test]
    fn steps_the_detector_cannot_read_are_left_out_of_the_fit() {
        // Eight steps of 0.97 octaves: the ends are out of the detector's
        // range, one reads an octave low (a rising run ends there), and
        // silence reads none.
        let hz = |v: f64| 164.0 * (0.97 * (v + 0.13)).exp2();
        let mut pitches: Vec<f64> = (-2..=5).map(|v| hz(f64::from(v))).collect();
        pitches[0] = 0.0;
        pitches[6] /= 2.0;
        pitches[7] = 0.0;
        let calibration = CvCalibration::from_steps(-2.0, &pitches).unwrap();
        assert_eq!(calibration.steps.len(), 5, "steps -1 V to 3 V");
        assert_eq!(calibration.steps[0].volts, -1.0);
        assert!((calibration.octaves_per_volt - 0.97).abs() < 1e-9);
        assert_eq!(CvCalibration::from_steps(-2.0, &[0.0; 8]), Err(0));
        assert_eq!(
            CvCalibration::from_steps(-2.0, &[0.0, 0.0, 100.0, 200.0, 0.0, 0.0, 0.0, 0.0]),
            Err(0)
        );
    }
}

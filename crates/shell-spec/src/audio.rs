//! The audio settings and reports the shell's audio commands take and
//! return (L-SET-012): pure data, so their schemas can be read without
//! linking an audio host. `bl-shell`'s audio host fills them in.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The test tone's level on a new install, in dBFS. Live's default isn't in
/// the UI parity reference, so this is unconfirmed (layout.md, L-SET-012).
pub const TONE_DEFAULT_DBFS: f64 = -36.0;
/// The quietest level the Tone Volume control offers, in dBFS.
pub const TONE_MIN_DBFS: f64 = -60.0;
/// The loudest level the Tone Volume control offers, in dBFS.
pub const TONE_MAX_DBFS: f64 = 0.0;
/// The test tone's frequency on a new install, in hertz (unconfirmed, as
/// [`TONE_DEFAULT_DBFS`]).
pub const TONE_DEFAULT_HZ: f64 = 440.0;
/// The lowest frequency the Tone Frequency control offers, in hertz.
pub const TONE_MIN_HZ: f64 = 20.0;
/// The highest frequency the Tone Frequency control offers, in hertz.
pub const TONE_MAX_HZ: f64 = 20_000.0;

/// The test tone's level and frequency, as the Audio page's Tone Volume and
/// Tone Frequency set them (D23) and the preferences keep them.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct ToneSettings {
    /// The level in dBFS, from -60 to 0.
    pub volume_db: f64,
    /// The frequency in hertz, from 20 to 20,000.
    pub frequency_hz: f64,
}

impl Default for ToneSettings {
    fn default() -> Self {
        Self {
            volume_db: TONE_DEFAULT_DBFS,
            frequency_hz: TONE_DEFAULT_HZ,
        }
    }
}

impl ToneSettings {
    /// The same settings brought within the controls' ranges. A value that
    /// isn't a number becomes the default.
    #[must_use]
    pub fn clamped(self) -> Self {
        let within = |value: f64, min: f64, max: f64, default: f64| {
            if value.is_nan() {
                default
            } else {
                value.clamp(min, max)
            }
        };
        Self {
            volume_db: within(
                self.volume_db,
                TONE_MIN_DBFS,
                TONE_MAX_DBFS,
                TONE_DEFAULT_DBFS,
            ),
            frequency_hz: within(self.frequency_hz, TONE_MIN_HZ, TONE_MAX_HZ, TONE_DEFAULT_HZ),
        }
    }

    /// The tone's peak amplitude: the level as a linear gain.
    #[must_use]
    pub fn amplitude(self) -> f32 {
        10f64.powf(self.clamped().volume_db / 20.0) as f32
    }
}

/// The audio device settings, as the Settings window's Audio page sets them
/// and the preferences file keeps them. `None` means the platform's or the
/// device's own choice.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct AudioSettings {
    /// The driver type's ID (`coreaudio`, `wasapi`, `asio`, `jack`, `alsa`,
    /// or `null`), or nothing for the platform's default.
    pub driver: Option<String>,
    /// The output device's ID, or nothing for the system's default output
    /// device, which the stream then follows where the driver can.
    pub output_device: Option<String>,
    /// The input device's ID, or nothing for no input.
    pub input_device: Option<String>,
    /// The sample rate in hertz, or nothing for the device's current rate,
    /// which leaves the device's own setting alone.
    pub sample_rate: Option<u32>,
    /// The buffer size in frames, or nothing for the device's default.
    pub buffer_size: Option<u32>,
    /// Output channels to open, or nothing for all the device has.
    pub output_channels: Option<u16>,
}

/// Arguments of `audio.configure`: the settings to change. Fields left out
/// keep their values.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConfigureAudio {
    /// The driver type: an ID from `audio.devices`, such as `coreaudio`,
    /// `wasapi`, `asio`, `jack`, `alsa`, or `null`. Changing it resets the
    /// devices to the driver's defaults unless they're given too.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub driver: Option<String>,
    /// The output device: an ID from `audio.devices`, or `default` for the
    /// system's default output device.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_device: Option<String>,
    /// The input device: an ID from `audio.devices`, or `none` for no input.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_device: Option<String>,
    /// The sample rate in hertz, or 0 for the device's current rate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample_rate: Option<u32>,
    /// The buffer size in frames, or 0 for the device's default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub buffer_size: Option<u32>,
    /// Output channels to open, or 0 for all the device has.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_channels: Option<u16>,
}

/// A device in use, by ID and name.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DeviceRef {
    /// The device's ID, as `audio.configure` takes it.
    pub id: String,
    /// The name the system gives it.
    pub name: String,
}

/// What `audio.status` returns.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AudioStatus {
    /// Whether the audio engine is on.
    pub engine_on: bool,
    /// The settings in force.
    pub settings: AudioSettings,
    /// The driver type in use (its ID).
    pub driver: String,
    /// The output device the stream opened, while one is open.
    pub output_device: Option<DeviceRef>,
    /// The input device the stream opened, if any.
    pub input_device: Option<DeviceRef>,
    /// Whether the stream follows the system's default output device.
    pub follows_default: bool,
    /// The sample rate the device runs at, in hertz, while a stream is open.
    pub sample_rate: Option<u32>,
    /// The buffer size the settings asked for, in frames.
    pub requested_buffer_size: Option<u32>,
    /// The buffer size the device actually delivers, in frames, once it has
    /// called back, or what the driver says it set until then.
    pub buffer_size: Option<u32>,
    /// Output channels open.
    pub output_channels: u16,
    /// Input channels open.
    pub input_channels: u16,
    /// The output buffer's length in milliseconds.
    pub buffer_ms: Option<f64>,
    /// The share of each buffer's time spent processing audio, in percent,
    /// averaged over the last measurement; nothing while the engine is off.
    pub cpu_load: Option<f32>,
    /// Dropouts since the stream opened.
    pub dropouts: u64,
    /// Whether the test tone plays.
    pub test_tone: bool,
    /// The test tone's level and frequency.
    pub tone: ToneSettings,
    /// The last error from the device, if the stream couldn't open or failed.
    pub error: Option<String>,
}

/// A driver type this build and machine offer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DriverInfo {
    /// Its ID, as `audio.configure` takes it.
    pub id: String,
    /// Its name.
    pub name: String,
}

/// One device, as `audio.devices` lists it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DeviceListing {
    /// The device's ID, as `audio.configure` takes it.
    pub id: String,
    /// The name the system gives it.
    pub name: String,
    /// Output channels it has.
    pub output_channels: u16,
    /// Input channels it has.
    pub input_channels: u16,
    /// Whether the system uses it for output by default.
    pub default_output: bool,
    /// Whether the system uses it for input by default.
    pub default_input: bool,
    /// The sample rates it accepts, in hertz.
    pub sample_rates: Vec<u32>,
    /// The rate it runs at now, if known.
    pub current_sample_rate: Option<u32>,
    /// The buffer sizes it accepts, in frames: every size from the first to
    /// the second number when `buffer_range` is true, otherwise exactly these.
    /// Empty if the driver can't say before a stream runs.
    pub buffer_sizes: Vec<u32>,
    /// Whether `buffer_sizes` is a range (its first and last values).
    pub buffer_range: bool,
    /// The buffer size it prefers, if it says.
    pub default_buffer_size: Option<u32>,
}

impl DeviceListing {
    /// The buffer sizes to offer in a chooser: powers of two within the
    /// device's range, or its own list.
    #[must_use]
    pub fn buffer_choices(&self) -> Vec<u32> {
        if self.buffer_range {
            let (min, max) = (self.buffer_sizes[0], self.buffer_sizes[1]);
            let mut sizes: Vec<u32> = (4..=13)
                .map(|shift| 1u32 << shift)
                .filter(|size| (min..=max).contains(size))
                .collect();
            if sizes.is_empty() {
                sizes.push(min);
            }
            sizes
        } else if self.buffer_sizes.is_empty() {
            vec![64, 128, 256, 512, 1024, 2048]
        } else {
            self.buffer_sizes.clone()
        }
    }
}

/// What `audio.devices` returns.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AudioDevices {
    /// The driver types on offer, best first.
    pub drivers: Vec<DriverInfo>,
    /// The driver whose devices are listed: the one in use.
    pub driver: String,
    /// Its devices.
    pub devices: Vec<DeviceListing>,
    /// Why the devices couldn't be listed, if they couldn't.
    pub error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffer_choices_are_powers_of_two_in_range() {
        let listing = DeviceListing {
            id: "null:null".into(),
            name: "Null".into(),
            output_channels: 2,
            input_channels: 2,
            default_output: true,
            default_input: true,
            sample_rates: vec![48_000],
            current_sample_rate: Some(48_000),
            buffer_sizes: vec![30, 3000],
            buffer_range: true,
            default_buffer_size: Some(256),
        };
        assert_eq!(
            listing.buffer_choices(),
            [32, 64, 128, 256, 512, 1024, 2048]
        );
    }

    #[test]
    fn tone_settings_stay_within_the_controls_ranges() {
        // Spec: app-shell R13.
        assert_eq!(ToneSettings::default().volume_db, -36.0);
        assert_eq!(ToneSettings::default().frequency_hz, 440.0);
        let wild = ToneSettings {
            volume_db: 12.0,
            frequency_hz: 5.0,
        }
        .clamped();
        assert_eq!((wild.volume_db, wild.frequency_hz), (0.0, 20.0));
        let nan = ToneSettings {
            volume_db: f64::NAN,
            frequency_hz: f64::NAN,
        }
        .clamped();
        assert_eq!(nan, ToneSettings::default());
        let quiet = ToneSettings {
            volume_db: -60.0,
            ..ToneSettings::default()
        };
        assert!((quiet.amplitude() - 0.001).abs() < 1e-7);
        assert!(
            ToneSettings {
                volume_db: 6.0,
                ..ToneSettings::default()
            }
            .amplitude()
                <= 1.0
        );
    }
}

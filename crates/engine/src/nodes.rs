//! The engine's own nodes: the test tone and the stand-in fader.

use std::f64::consts::TAU;

use bl_model::{Device, Mixer, ParamId};

use crate::node::{Block, Node, NodeConfig, NodeEvent, ParamValue, segments};

/// The device type of the test tone, a native device that adds a sine to its
/// track.
pub const TEST_TONE: &str = "beatlayer.test_tone";

/// The test tone's parameters.
pub mod test_tone {
    use bl_model::ParamId;

    /// The frequency in hertz, from 0 to half the sample rate.
    pub const FREQUENCY: ParamId = ParamId(0);
    /// The level in dBFS, 0 or lower.
    pub const LEVEL: ParamId = ParamId(1);
    /// The frequency unless the document sets one.
    pub const DEFAULT_FREQUENCY: f64 = 440.0;
    /// The level unless the document sets one: quiet enough to leave running.
    pub const DEFAULT_LEVEL: f64 = -60.0;
}

/// The test tone: a sine at a set frequency and level, added to every
/// channel. It plays whether or not the transport runs.
#[derive(Debug)]
pub struct TestTone {
    /// Where the sine is in its cycle, from 0 to 1.
    phase: f64,
    /// Cycles per frame.
    step: f64,
    amplitude: f32,
    sample_rate: f64,
}

impl TestTone {
    /// A tone set to `device`'s parameters.
    #[must_use]
    pub fn new(device: &Device, config: &NodeConfig) -> Self {
        let mut tone = Self {
            phase: 0.0,
            step: 0.0,
            amplitude: 0.0,
            sample_rate: config.sample_rate.as_f64(),
        };
        tone.set(test_tone::FREQUENCY, ParamValue::Default);
        tone.set(test_tone::LEVEL, ParamValue::Default);
        for (&param, &value) in &device.params {
            tone.set(param, ParamValue::Set(value));
        }
        tone
    }

    fn set(&mut self, param: ParamId, value: ParamValue) {
        match param {
            test_tone::FREQUENCY => {
                let hz = match value {
                    ParamValue::Set(hz) if hz.is_finite() => hz,
                    _ => test_tone::DEFAULT_FREQUENCY,
                };
                self.step = hz.clamp(0.0, self.sample_rate / 2.0) / self.sample_rate;
            }
            test_tone::LEVEL => {
                let db = match value {
                    ParamValue::Set(db) if !db.is_nan() => db.min(0.0),
                    _ => test_tone::DEFAULT_LEVEL,
                };
                self.amplitude = 10f64.powf(db / 20.0) as f32;
            }
            _ => {}
        }
    }
}

impl Node for TestTone {
    fn process(&mut self, block: &mut Block<'_>) {
        for segment in segments(block.events(), block.frames()) {
            for event in segment.events {
                self.set(event.param, event.value);
            }
            let start = self.phase;
            let mut end = start;
            for channel in block.channels_mut() {
                let mut phase = start;
                for sample in &mut channel[segment.range.clone()] {
                    *sample += (phase * TAU).sin() as f32 * self.amplitude;
                    phase += self.step;
                    if phase >= 1.0 {
                        phase -= 1.0;
                    }
                }
                end = phase;
            }
            self.phase = end;
        }
    }

    fn flush(&mut self, events: &[NodeEvent]) {
        for event in events {
            self.set(event.param, event.value);
        }
    }
}

/// The fader's parameters, which take the track's mixer settings.
pub mod fader {
    use bl_model::ParamId;

    /// Linear gain: 1 is unity and 0 is silence.
    pub const VOLUME: ParamId = ParamId(0);
    /// Stereo position from −1 to 1. Kept, not yet applied (WP-1.6).
    pub const PAN: ParamId = ParamId(1);
    /// Muted when not 0.
    pub const MUTE: ParamId = ParamId(2);
    /// Soloed when not 0. Kept, not yet applied (WP-1.6).
    pub const SOLO: ParamId = ParamId(3);
}

/// A track's fader: a stand-in that applies volume and mute, unsmoothed,
/// until the mixer (WP-1.6) brings pan laws, solo, and smoothing. It exists
/// so mixer changes travel the parameter path end to end.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fader {
    volume: f32,
    pan: f64,
    mute: bool,
    solo: bool,
}

impl Fader {
    /// A fader set to a track's mixer.
    #[must_use]
    pub fn new(mixer: &Mixer) -> Self {
        Self {
            volume: mixer.volume as f32,
            pan: mixer.pan,
            mute: mixer.mute,
            solo: mixer.solo,
        }
    }

    /// The gain the fader applies now.
    #[must_use]
    pub fn gain(&self) -> f32 {
        if self.mute { 0.0 } else { self.volume }
    }

    fn set(&mut self, param: ParamId, value: ParamValue) {
        let defaults = Mixer::default();
        match (param, value) {
            (fader::VOLUME, ParamValue::Set(volume)) if volume.is_finite() => {
                self.volume = volume.max(0.0) as f32;
            }
            (fader::VOLUME, _) => self.volume = defaults.volume as f32,
            (fader::PAN, ParamValue::Set(pan)) if pan.is_finite() => {
                self.pan = pan.clamp(-1.0, 1.0);
            }
            (fader::PAN, _) => self.pan = defaults.pan,
            (fader::MUTE, ParamValue::Set(mute)) => self.mute = mute != 0.0,
            (fader::MUTE, ParamValue::Default) => self.mute = defaults.mute,
            (fader::SOLO, ParamValue::Set(solo)) => self.solo = solo != 0.0,
            (fader::SOLO, ParamValue::Default) => self.solo = defaults.solo,
            _ => {}
        }
    }
}

impl Node for Fader {
    fn process(&mut self, block: &mut Block<'_>) {
        for segment in segments(block.events(), block.frames()) {
            for event in segment.events {
                self.set(event.param, event.value);
            }
            let gain = self.gain();
            if gain == 1.0 {
                continue;
            }
            for channel in block.channels_mut() {
                for sample in &mut channel[segment.range.clone()] {
                    *sample *= gain;
                }
            }
        }
    }

    fn flush(&mut self, events: &[NodeEvent]) {
        for event in events {
            self.set(event.param, event.value);
        }
    }
}

#[cfg(test)]
mod tests {
    use bl_core::time::SampleRate;
    use bl_model::{DeviceId, DeviceKind};

    use super::*;
    use crate::node::TransportInfo;

    fn config() -> NodeConfig {
        NodeConfig {
            sample_rate: SampleRate::HZ_48000,
            max_frames: 4096,
            channels: 2,
        }
    }

    fn render(node: &mut dyn Node, frames: usize, events: &[NodeEvent]) -> Vec<f32> {
        let mut data = vec![0.0f32; 2 * frames];
        let mut block = Block::new(
            &mut data,
            2,
            frames,
            frames,
            events,
            TransportInfo::default(),
            SampleRate::HZ_48000,
        );
        node.process(&mut block);
        data
    }

    fn tone(params: &[(ParamId, f64)]) -> TestTone {
        let mut device = Device::new(
            DeviceId(bl_core::id::Id::new(1).unwrap()),
            "",
            DeviceKind::Native {
                device_type: TEST_TONE.to_owned(),
            },
        );
        for &(param, value) in params {
            device.params.insert(param, value);
        }
        TestTone::new(&device, &config())
    }

    fn peak_db(samples: &[f32]) -> f64 {
        let peak = samples.iter().fold(0.0f32, |p, s| p.max(s.abs()));
        20.0 * f64::from(peak).log10()
    }

    #[test]
    fn the_test_tone_defaults_to_440_hz_at_minus_60_dbfs() {
        let data = render(&mut tone(&[]), 4800, &[]);
        assert!((peak_db(&data) + 60.0).abs() < 0.01);
        // 440 Hz over 4,800 frames at 48 kHz is 44 cycles: 88 sign changes.
        let crossings = data[..4800]
            .windows(2)
            .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
            .count();
        assert!((87..=89).contains(&crossings), "{crossings} crossings");
        assert_eq!(&data[..4800], &data[4800..], "both channels carry the tone");
    }

    #[test]
    fn the_test_tone_takes_its_level_from_the_document_and_never_exceeds_0_dbfs() {
        let data = render(&mut tone(&[(test_tone::LEVEL, -20.0)]), 4800, &[]);
        assert!((peak_db(&data) + 20.0).abs() < 0.01);
        let data = render(&mut tone(&[(test_tone::LEVEL, 12.0)]), 4800, &[]);
        assert!(peak_db(&data) <= 0.0);
    }

    #[test]
    fn the_test_tone_changes_level_on_the_events_frame() {
        let events = [NodeEvent {
            offset: 1000,
            param: test_tone::LEVEL,
            value: ParamValue::Set(-120.0),
        }];
        let data = render(&mut tone(&[]), 2000, &events);
        assert!(peak_db(&data[..1000]) > -60.1);
        assert!(peak_db(&data[1000..2000]) < -119.0);
    }

    #[test]
    fn the_fader_applies_volume_and_mute_from_their_frame() {
        let mut fader = Fader::new(&Mixer::default());
        let mut data = vec![1.0f32; 2 * 8];
        let events = [
            NodeEvent {
                offset: 2,
                param: fader::VOLUME,
                value: ParamValue::Set(0.5),
            },
            NodeEvent {
                offset: 5,
                param: fader::MUTE,
                value: ParamValue::Set(1.0),
            },
        ];
        let mut block = Block::new(
            &mut data,
            2,
            8,
            8,
            &events,
            TransportInfo::default(),
            SampleRate::HZ_48000,
        );
        fader.process(&mut block);
        let expected = [1.0, 1.0, 0.5, 0.5, 0.5, 0.0, 0.0, 0.0];
        assert_eq!(&data[..8], &expected);
        assert_eq!(&data[8..], &expected);
        fader.set(fader::MUTE, ParamValue::Default);
        fader.set(fader::VOLUME, ParamValue::Default);
        assert_eq!(fader.gain(), 1.0);
    }
}

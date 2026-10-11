//! The engine's own devices: the test tone and the beat tone. Each track's
//! mixer is in [`crate::mixer`].

use std::f64::consts::TAU;

use bl_core::time::{Frames, SampleRate, TICKS_PER_QUARTER, TempoMap};
use bl_model::{Device, ParamId};

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
///
/// Its sine and its decibel conversion come from `libm`, a pure-Rust maths
/// library, rather than the platform's, whose last bit can differ between
/// platforms and between versions of one operating system. So the tone is
/// the same to the bit everywhere, and the golden renders built on it are
/// bit-exact on every platform (WP-0.10).
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
                self.amplitude = libm::pow(10.0, db / 20.0) as f32;
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
                    *sample += libm::sin(phase * TAU) as f32 * self.amplitude;
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

/// The device type of the beat tone, a native device that sounds the
/// transport: while the song plays, it adds a sine locked to the song's
/// beat.
pub const BEAT_TONE: &str = "beatlayer.test_beat_tone";

/// The beat tone's parameters.
pub mod beat_tone {
    use bl_model::ParamId;

    /// Sine cycles per quarter note, a whole number from 1 to 10,000. The
    /// pitch follows the tempo: 220 cycles a quarter note is 440 Hz at
    /// 120 BPM and 660 Hz at 180.
    pub const CYCLES_PER_BEAT: ParamId = ParamId(0);
    /// The level in dBFS at the start of each quarter note, 0 or lower.
    pub const LEVEL: ParamId = ParamId(1);
    /// The cycles per quarter note unless the document sets them.
    pub const DEFAULT_CYCLES_PER_BEAT: i64 = 220;
    /// The level unless the document sets one: quiet enough to leave
    /// running.
    pub const DEFAULT_LEVEL: f64 = -60.0;
}

/// The beat tone: while the song plays, a sine whose every sample is a
/// function of the song position in ticks, so a render shows exactly where
/// the transport played. Its phase is locked to the beat (a whole number of
/// cycles to each quarter note), so its pitch follows the tempo map, ramps
/// included; and each quarter note starts at its level and falls in a
/// straight line to a quarter of it, so beats, starts, and loops show in the
/// waveform. It's silent while the song is stopped.
///
/// Like the test tone, it uses `libm` for its sine and level, integer
/// arithmetic for whole ticks, and the tempo map's own conversions, so it
/// renders to the same bits on every platform, whatever the block size: a
/// sample depends only on its song position and the tempo map
/// ([`BeatTone::sample_at`]). A golden test's sound source for the
/// transport (WP-1.5).
#[derive(Debug)]
pub struct BeatTone {
    cycles: i64,
    amplitude: f32,
}

impl BeatTone {
    /// A beat tone set to `device`'s parameters.
    #[must_use]
    pub fn new(device: &Device) -> Self {
        let mut tone = Self {
            cycles: 0,
            amplitude: 0.0,
        };
        tone.set(beat_tone::CYCLES_PER_BEAT, ParamValue::Default);
        tone.set(beat_tone::LEVEL, ParamValue::Default);
        for (&param, &value) in &device.params {
            tone.set(param, ParamValue::Set(value));
        }
        tone
    }

    fn set(&mut self, param: ParamId, value: ParamValue) {
        match param {
            beat_tone::CYCLES_PER_BEAT => {
                self.cycles = match value {
                    ParamValue::Set(cycles) if cycles.is_finite() => {
                        cycles.round().clamp(1.0, 10_000.0) as i64
                    }
                    _ => beat_tone::DEFAULT_CYCLES_PER_BEAT,
                };
            }
            beat_tone::LEVEL => {
                let db = match value {
                    ParamValue::Set(db) if !db.is_nan() => db.min(0.0),
                    _ => beat_tone::DEFAULT_LEVEL,
                };
                self.amplitude = libm::pow(10.0, db / 20.0) as f32;
            }
            _ => {}
        }
    }

    /// What the tone adds to the frame at song position `position` while
    /// the song plays, under `tempo` at `rate`.
    ///
    /// A frame spans many ticks (about 29 at 120 BPM and 48 kHz), so the
    /// tone works out where the frame falls between two ticks: the nearest
    /// tick, plus the time from that tick to the frame at the tempo there.
    /// Rounding to whole ticks instead would jitter the sine's phase and
    /// add noise around −90 dBFS.
    #[must_use]
    pub fn sample_at(&self, tempo: &TempoMap, position: Frames, rate: SampleRate) -> f32 {
        let tick = tempo.frames_to_ticks(position, rate);
        let past = rate.frames_to_seconds(position) - tempo.ticks_to_seconds(tick);
        let ticks_per_second = tempo.bpm_at(tick).get() / 60.0 * TICKS_PER_QUARTER as f64;
        // Under a tick either way, so the whole ticks below carry the
        // position and this only the fraction.
        let fraction = past * ticks_per_second;
        let quarter = i128::from(TICKS_PER_QUARTER);
        let whole = i128::from(tick.get());
        let cycles = i128::from(self.cycles);
        let phase = ((whole * cycles).rem_euclid(quarter) as f64 + fraction * self.cycles as f64)
            / TICKS_PER_QUARTER as f64;
        let into_beat = (whole.rem_euclid(quarter) as f64 + fraction)
            .rem_euclid(TICKS_PER_QUARTER as f64)
            / TICKS_PER_QUARTER as f64;
        let shape = libm::sin(phase * TAU) * (1.0 - 0.75 * into_beat);
        (shape * f64::from(self.amplitude)) as f32
    }
}

impl Node for BeatTone {
    fn process(&mut self, block: &mut Block<'_>) {
        let transport = block.transport();
        let (tempo, rate) = (block.tempo(), block.sample_rate());
        for segment in segments(block.events(), block.frames()) {
            for event in segment.events {
                self.set(event.param, event.value);
            }
            if !transport.playing {
                continue;
            }
            for i in segment.range {
                let sample = self.sample_at(tempo, transport.position + Frames(i as i64), rate);
                for channel in block.channels_mut() {
                    channel[i] += sample;
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
    use bl_core::time::Ticks;
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

    fn beat(cycles: f64) -> BeatTone {
        let mut device = Device::new(
            DeviceId(bl_core::id::Id::new(1).unwrap()),
            "",
            DeviceKind::Native {
                device_type: BEAT_TONE.to_owned(),
            },
        );
        device.params.insert(beat_tone::CYCLES_PER_BEAT, cycles);
        device.params.insert(beat_tone::LEVEL, -20.0);
        BeatTone::new(&device)
    }

    /// The beat tone over `frames` frames from song position `from`, in
    /// blocks of `block`, under `tempo`, playing or not.
    fn beats(
        tone: &mut BeatTone,
        tempo: &TempoMap,
        from: i64,
        frames: usize,
        block: usize,
        playing: bool,
    ) -> Vec<f32> {
        let mut out = Vec::with_capacity(frames);
        let mut data = vec![0.0f32; 2 * block];
        while out.len() < frames {
            let n = block.min(frames - out.len());
            data.fill(0.0);
            let transport = TransportInfo {
                playing,
                position: Frames(from + out.len() as i64),
            };
            let mut b = Block::new(&mut data, 2, n, block, &[], transport, SampleRate::HZ_48000)
                .with_tempo(tempo);
            tone.process(&mut b);
            assert_eq!(data[..n], data[block..block + n], "both channels");
            out.extend_from_slice(&data[..n]);
        }
        out
    }

    fn crossings(samples: &[f32]) -> usize {
        samples
            .windows(2)
            .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
            .count()
    }

    #[test]
    fn the_beat_tone_is_silent_while_stopped_and_its_pitch_follows_the_tempo() {
        let mut tone = beat(220.0);
        let at = |bpm| TempoMap::constant(bl_core::time::Bpm::new(bpm).unwrap());
        assert!(
            beats(&mut tone, &at(120.0), 0, 4800, 4800, false)
                .iter()
                .all(|&s| s == 0.0)
        );
        // 220 cycles a beat: 440 Hz at 120 BPM, 660 Hz at 180, for a second.
        let slow = beats(&mut tone, &at(120.0), 0, 48_000, 4800, true);
        let fast = beats(&mut tone, &at(180.0), 0, 48_000, 4800, true);
        assert!(
            (879..=881).contains(&crossings(&slow)),
            "{}",
            crossings(&slow)
        );
        assert!(
            (1319..=1321).contains(&crossings(&fast)),
            "{}",
            crossings(&fast)
        );
        // Each beat starts at the tone's level, -20 dBFS.
        let peak = slow[..2400].iter().fold(0.0f32, |p, s| p.max(s.abs()));
        assert!(
            (20.0 * f64::from(peak).log10() + 20.0).abs() < 0.1,
            "{peak}"
        );
    }

    #[test]
    fn the_beat_tone_depends_only_on_the_song_position() {
        let tempo = TempoMap::from_breakpoints([
            (Ticks::ZERO, bl_core::time::Bpm::new(90.0).unwrap()),
            (Ticks::WHOLE, bl_core::time::Bpm::new(150.0).unwrap()),
        ])
        .unwrap();
        let mut tone = beat(110.0);
        let whole = beats(&mut tone, &tempo, 1_000, 20_000, 4096, true);
        let pieces = beats(&mut tone, &tempo, 1_000, 20_000, 77, true);
        assert_eq!(whole, pieces);
        let later = beats(&mut tone, &tempo, 11_000, 10_000, 512, true);
        assert_eq!(whole[10_000..], later[..]);
        for (i, &sample) in whole.iter().enumerate() {
            let position = Frames(1_000 + i as i64);
            assert_eq!(
                sample,
                tone.sample_at(&tempo, position, SampleRate::HZ_48000)
            );
        }
    }
}

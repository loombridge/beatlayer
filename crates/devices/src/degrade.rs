//! Erosion, the native digital-degradation effect (WP-5.24; the digital
//! degradation spec): a short delay moved by a sine and by band-filtered
//! noise, which smears a signal from a soft warble to grit.
//!
//! The delay sits at a fixed centre of [`LATENCY_SECONDS`], which is the
//! device's reported latency, and the modulation swings it by up to
//! [`DEPTH_SECONDS`] either side. The noise follows the device's seed, so a
//! render repeats run after run, and every value is worked out frame by
//! frame, so it is the same at any block size.

use std::f64::consts::TAU;

use bl_dsp::Random;
use bl_engine::{DeviceType, NodeConfig, Registry};
use bl_model::device_layout::{BODY_HEIGHT, ControlBand, DisplayCell, LayoutNode, Widget};
use bl_model::{
    Device, DeviceDescription, DeviceDisplay, DeviceId, DeviceLayout, DeviceRole, ParamDescriptor,
    ParamId, ParamUnit,
};

use crate::delay::BandPass;
use crate::framework::{Audio, NativeDevice, ParamBank, add};
use crate::sections::{ANALYSER_BANDS, Analyser, spectrum_series};

/// Erosion's device type.
pub const DEGRADE: &str = "beatlayer.degrade";

/// Freq: the sine's frequency and the noise band's centre.
pub const FREQ: ParamId = ParamId(0);
/// Amount: how far the modulation moves the delay.
pub const AMOUNT: ParamId = ParamId(1);
/// Noise Blend: 0 % is the sine alone, 100 % the noise alone.
pub const NOISE_BLEND: ParamId = ParamId(2);
/// Filter Width: the noise band's width.
pub const FILTER_WIDTH: ParamId = ParamId(3);
/// Stereo Width: 0 % modulates both channels alike, 100 % independently.
pub const STEREO_WIDTH: ParamId = ParamId(4);

const FREQ_INDEX: usize = 0;
const AMOUNT_INDEX: usize = 1;
const BLEND_INDEX: usize = 2;
const WIDTH_INDEX: usize = 3;
const STEREO_INDEX: usize = 4;

/// The delay's centre, which is the device's latency. The 12.4 release
/// notes give about 2 ms for Live's.
pub const LATENCY_SECONDS: f64 = 0.002;
/// The most the modulation moves the delay either way at Amount 100 %.
const DEPTH_SECONDS: f64 = 0.0015;
/// The noise band's width in octaves at Filter Width 0 % and at 100 %.
const NARROWEST_OCTAVES: f64 = 0.25;
const WIDEST_OCTAVES: f64 = 4.0;
/// The RMS of the noise carrier, against the sine's peak of 1.
const NOISE_LEVEL: f64 = 0.5;
/// How often the noise band's coefficients are worked out, in frames.
const REFRESH_FRAMES: u64 = 16;

/// The Erosion device type.
#[must_use]
pub fn degrade_type() -> DeviceType {
    let params = vec![
        ParamDescriptor::continuous(FREQ, "Freq", (20.0, 20_000.0), 1_000.0, ParamUnit::Hertz)
            .logarithmic()
            .describe("The modulation's frequency: the sine's, and the centre of the noise band."),
        ParamDescriptor::continuous(AMOUNT, "Amount", (0.0, 100.0), 50.0, ParamUnit::Percent)
            .describe("How far the modulation moves the delay."),
        ParamDescriptor::continuous(
            NOISE_BLEND,
            "Noise Blend",
            (0.0, 100.0),
            0.0,
            ParamUnit::Percent,
        )
        .describe("The balance between sine and noise modulation: 0 % is sine only."),
        ParamDescriptor::continuous(
            FILTER_WIDTH,
            "Filter Width",
            (0.0, 100.0),
            50.0,
            ParamUnit::Percent,
        )
        .describe("The width of the noise band. It does nothing at a Noise Blend of 0 %."),
        ParamDescriptor::continuous(
            STEREO_WIDTH,
            "Stereo Width",
            (0.0, 100.0),
            0.0,
            ParamUnit::Percent,
        )
        .describe("From the same modulation in both channels (0 %) to a different one in each."),
    ];
    let layout = DeviceLayout::new(LayoutNode::Row(vec![
        LayoutNode::Cell(DisplayCell::new("Spectrum", 150.0, BODY_HEIGHT, 1.0)),
        LayoutNode::Band(ControlBand::stacked(
            BODY_HEIGHT,
            &[
                (FREQ, Widget::Knob),
                (AMOUNT, Widget::Knob),
                (NOISE_BLEND, Widget::Knob),
                (FILTER_WIDTH, Widget::Knob),
                (STEREO_WIDTH, Widget::Knob),
            ],
        )),
    ]));
    DeviceType::native(DEGRADE, "Erosion", DeviceRole::AudioEffect).with_description(
        DeviceDescription::new(params)
            .with_layout(layout)
            .with_banks(&[(
                "Degrade",
                &[
                    "Freq",
                    "Amount",
                    "Noise Blend",
                    "Filter Width",
                    "Stereo Width",
                ],
            )]),
    )
}

/// Adds Erosion to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, degrade_type(), Degrade::new);
}

/// The Q of a band-pass that is `octaves` wide between its half-power
/// points.
pub(crate) fn q_for_octaves(octaves: f64) -> f64 {
    1.0 / (2.0 * (std::f64::consts::LN_2 / 2.0 * octaves).sinh())
}

/// The noise band's width in octaves for Filter Width `percent`.
fn width_octaves(percent: f64) -> f64 {
    NARROWEST_OCTAVES * (WIDEST_OCTAVES / NARROWEST_OCTAVES).powf(percent / 100.0)
}

/// One channel's delay line: written a sample a frame, read between two.
struct Line {
    buffer: Vec<f32>,
    write: usize,
}

impl Line {
    /// Writes `sample` and returns the line `delay` frames back.
    fn tap(&mut self, sample: f32, delay: f64) -> f32 {
        let len = self.buffer.len();
        self.buffer[self.write] = sample;
        let position = (self.write as f64 - delay).rem_euclid(len as f64);
        let low = position.floor();
        let (i, fraction) = (low as usize, (position - low) as f32);
        let (a, b) = (self.buffer[i], self.buffer[(i + 1) % len]);
        self.write = (self.write + 1) % len;
        a + (b - a) * fraction
    }
}

/// Erosion's audio-thread half.
pub struct Degrade {
    rate: f64,
    /// The delay's centre, in frames.
    latency: usize,
    /// The deepest swing, in frames.
    depth: f64,
    lines: Vec<Line>,
    /// The sine's phase in cycles, and the frames run.
    phase: f64,
    frames_run: u64,
    /// Each channel's noise: its source, band and the scale that makes its
    /// variance 1.
    noise: [Random; 2],
    band: [BandPass; 2],
    noise_scale: f64,
    /// The modulation for each channel over the stretch, from −1 to 1.
    modulation: [Vec<f32>; 2],
    analyser: Analyser,
}

impl Degrade {
    fn new(device: &Device, config: &NodeConfig) -> Self {
        let rate = config.sample_rate.as_f64();
        let latency = (LATENCY_SECONDS * rate).round() as usize;
        let depth = DEPTH_SECONDS * rate;
        let len = latency + depth.ceil() as usize + 4;
        let seed = device.id.0.get();
        let frames = config.max_frames.max(1);
        Self {
            rate,
            latency,
            depth,
            lines: (0..config.channels)
                .map(|_| Line {
                    buffer: vec![0.0; len],
                    write: 0,
                })
                .collect(),
            phase: 0.0,
            frames_run: 0,
            noise: [Random::seeded(seed), Random::seeded(seed ^ 0x5EED)],
            band: [BandPass::default(), BandPass::default()],
            noise_scale: 1.0,
            modulation: [vec![0.0; frames], vec![0.0; frames]],
            analyser: Analyser::new(rate),
        }
    }
}

impl NativeDevice for Degrade {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        let frames = audio.frames();
        let mut channels = audio.channels_mut();
        match (channels.next(), channels.next()) {
            (Some(left), Some(right)) => {
                for (l, r) in left.iter().zip(right.iter()) {
                    self.analyser.push((l + r) * 0.5);
                }
            }
            (Some(only), None) => only.iter().for_each(|s| self.analyser.push(*s)),
            _ => {}
        }
        drop(channels);

        let (freq, blend, width, stereo) = (
            params.ramp(FREQ_INDEX),
            params.ramp(BLEND_INDEX),
            params.ramp(WIDTH_INDEX),
            params.ramp(STEREO_INDEX),
        );
        for i in 0..frames {
            if self.frames_run.is_multiple_of(REFRESH_FRAMES) {
                let q = q_for_octaves(width_octaves(f64::from(width[i])));
                for band in &mut self.band {
                    band.set(f64::from(freq[i]).min(self.rate * 0.45), q, self.rate);
                }
                self.noise_scale = 1.0 / self.band[0].noise_gain();
            }
            self.frames_run += 1;
            let (mix, w) = (f64::from(blend[i]) / 100.0, f64::from(stereo[i]) / 100.0);
            self.phase = (self.phase + f64::from(freq[i]) / self.rate).fract();
            // Unit-variance noise through each channel's band.
            let mut noise = [0.0f64; 2];
            for ((n, source), band) in noise.iter_mut().zip(&mut self.noise).zip(&mut self.band) {
                let white = source.next_bipolar() * 3f64.sqrt();
                *n = f64::from(band.process(white as f32)) * self.noise_scale;
            }
            // The right channel's noise leans from the left's to its own,
            // and its sine from in phase to opposed.
            let right_noise =
                ((1.0 - w) * noise[0] + w * noise[1]) / ((1.0 - w).powi(2) + w * w).sqrt();
            let sines = [
                (TAU * self.phase).sin(),
                (TAU * (self.phase + 0.5 * w)).sin(),
            ];
            let noises = [noise[0], right_noise];
            for channel in 0..2 {
                let carrier = (1.0 - mix) * sines[channel]
                    + mix * (noises[channel] * NOISE_LEVEL).clamp(-1.0, 1.0);
                self.modulation[channel][i] = carrier as f32;
            }
        }

        let amount = params.ramp(AMOUNT_INDEX);
        for channel in 0..audio.channels().min(self.lines.len()) {
            let modulation = &self.modulation[channel.min(1)];
            let line = &mut self.lines[channel];
            for ((sample, m), a) in audio
                .channel_mut(channel)
                .iter_mut()
                .zip(modulation)
                .zip(amount)
            {
                let swing = f64::from(*a) / 100.0 * self.depth * f64::from(*m);
                *sample = line.tap(*sample, self.latency as f64 + swing);
            }
        }
    }

    fn latency(&self) -> u32 {
        self.latency as u32
    }

    fn display_len(&self) -> usize {
        ANALYSER_BANDS
    }

    fn display(&mut self, out: &mut [f32]) {
        out.copy_from_slice(self.analyser.bands());
    }
}

/// The device's display (the digital degradation spec, R8): the input's
/// spectrum, once the engine has published it.
#[must_use]
pub fn display(device: DeviceId, data: Option<&[f32]>) -> DeviceDisplay {
    DeviceDisplay {
        device,
        live: data.is_some(),
        view: None,
        series: data.map(spectrum_series).into_iter().collect(),
        markers: Vec::new(),
        readings: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Spec: digital-degradation R4
    #[test]
    fn filter_width_runs_from_a_quarter_octave_to_four() {
        assert!((width_octaves(0.0) - 0.25).abs() < 1e-12);
        assert!((width_octaves(100.0) - 4.0).abs() < 1e-12);
        // An octave-wide band-pass has a Q of 1.414.
        assert!((q_for_octaves(1.0) - 2f64.sqrt()).abs() < 1e-9);
    }
}

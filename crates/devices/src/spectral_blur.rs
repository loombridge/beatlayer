//! Spectral Blur, a frequency range smeared into a cloud (WP-8.49; the
//! Spectral Blur spec): the input goes through [`bl_dsp::stft`], and the
//! bins inside Low to High feed decaying accumulators that Halo times, their
//! phases drawn afresh every hop from the device's seed. The bins outside
//! pass as they came, and the dry path is delayed by the STFT's latency.
//!
//! The random phase is drawn in the hop callback, in bin order, so it
//! depends on the seed and the hop count, never on the block size.

use core::f32::consts::PI;

use bl_dsp::Random;
use bl_dsp::stft::{Complex32, Stft, Window, hz_to_bin, row_bins};
use bl_engine::{DeviceType, NodeConfig, Registry};
use bl_model::device_layout::{BODY_HEIGHT, ControlBand, DisplayCell, LayoutNode, Widget};
use bl_model::{
    Device, DeviceDescription, DeviceDisplay, DeviceId, DeviceLayout, DeviceRole, DisplayHandle,
    DisplayMarker, DisplaySeries, OrdMap, ParamDescriptor, ParamId, ParamUnit,
};

use crate::framework::{Audio, DelayLine, NativeDevice, ParamBank, add};

/// Spectral Blur's device type.
pub const SPECTRAL_BLUR: &str = "beatlayer.spectral_blur";

/// Low: the bottom of the blurred range.
pub const LOW: ParamId = ParamId(0);
/// High: the top of the range, kept at or above Low.
pub const HIGH: ParamId = ParamId(1);
/// Halo: the time the cloud takes to fall 60 dB.
pub const HALO: ParamId = ParamId(2);
/// Residual: the share of the range's own signal kept beside the cloud.
pub const RESIDUAL: ParamId = ParamId(3);
/// Freeze: holds the cloud.
pub const FREEZE: ParamId = ParamId(4);
/// Wet Gain: the cloud's level.
pub const WET_GAIN: ParamId = ParamId(5);
/// Dry/Wet: the balance between the delayed input and the result.
pub const DRY_WET: ParamId = ParamId(6);

/// How many parameters the device has.
const PARAMS: usize = 7;
/// How many points the display's spectrum has.
pub const DISPLAY_LEN: usize = 96;
/// The display's spectrum floor, in dB.
const FLOOR_DB: f32 = -90.0;
/// The window's size at 48 kHz, scaled with the rate to a power of two.
const SIZE_AT_48K: f64 = 2_048.0;
/// The STFT's overlap.
const OVERLAP: usize = 4;
/// What the cloud's magnitudes are multiplied by: frames of random phase
/// add as power, and four Hann frames' worth of that is half the power the
/// same magnitudes make added in phase.
const CLOUD_GAIN: f32 = core::f32::consts::SQRT_2;

/// Spectral Blur's type: Beatlayer's own look, with Live's name.
#[must_use]
pub fn spectral_blur_type() -> DeviceType {
    let hertz = |id, name: &str, default, text: &str| {
        ParamDescriptor::continuous(id, name, (20.0, 20_000.0), default, ParamUnit::Hertz)
            .logarithmic()
            .describe(text)
    };
    let params = vec![
        hertz(
            LOW,
            "Low",
            200.0,
            "The bottom of the range that is blurred.",
        ),
        hertz(
            HIGH,
            "High",
            5_000.0,
            "The top of the range that is blurred; never below Low.",
        ),
        ParamDescriptor::continuous(
            HALO,
            "Halo",
            (20.0, 10_000.0),
            1_000.0,
            ParamUnit::Milliseconds,
        )
        .logarithmic()
        .describe("How long the blurred grains are: the time the cloud takes to fall 60 dB."),
        ParamDescriptor::continuous(RESIDUAL, "Residual", (0.0, 100.0), 0.0, ParamUnit::Percent)
            .describe("How much of the range's own, unblurred signal is kept beside the cloud."),
        ParamDescriptor::toggle(FREEZE, "Freeze", false)
            .describe("Holds the cloud as it is while the rest of the sound passes."),
        ParamDescriptor::gain(WET_GAIN, "Wet Gain", 12.0, 0.0)
            .describe("The cloud's level, down to silence."),
        ParamDescriptor::continuous(DRY_WET, "Dry/Wet", (0.0, 100.0), 100.0, ParamUnit::Percent)
            .describe("The balance between the dry signal and the result."),
    ];
    let layout = DeviceLayout::new(LayoutNode::Row(vec![
        LayoutNode::Band(ControlBand::stacked(
            BODY_HEIGHT,
            &[
                (LOW, Widget::Knob),
                (HIGH, Widget::Knob),
                (HALO, Widget::Knob),
            ],
        )),
        LayoutNode::Cell(DisplayCell::new("Range", 240.0, BODY_HEIGHT, 1.0)),
        LayoutNode::Band(ControlBand::stacked(
            BODY_HEIGHT,
            &[
                (RESIDUAL, Widget::Knob),
                (FREEZE, Widget::Toggle),
                (WET_GAIN, Widget::Knob),
                (DRY_WET, Widget::Knob),
            ],
        )),
    ]));
    DeviceType::native(SPECTRAL_BLUR, "Spectral Blur", DeviceRole::AudioEffect).with_description(
        DeviceDescription::new(params)
            .with_layout(layout)
            .with_banks(&[(
                "Main",
                &[
                    "Low", "High", "Halo", "Residual", "Freeze", "Wet Gain", "Dry/Wet",
                ],
            )]),
    )
}

/// Adds Spectral Blur to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, spectral_blur_type(), SpectralBlur::new);
}

/// The parameters one hop reads.
struct Settings {
    low: f64,
    high: f64,
    /// What each accumulator is multiplied by per hop, to fall 60 dB in Halo.
    decay: f32,
    residual: f32,
    wet_gain: f32,
    freeze: bool,
}

/// One channel's accumulators and random phase.
struct Cloud {
    /// Each bin's accumulated magnitude.
    level: Vec<f32>,
    random: Random,
}

impl Cloud {
    /// Blurs the bins from `low` to `high` Hz of `spectrum`, a frame of
    /// `size` samples at `rate` Hz.
    fn hop(&mut self, spectrum: &mut [Complex32], s: &Settings, size: usize, rate: f64) {
        let top = spectrum.len() - 1;
        // The bins whose centres lie in the range, never DC or Nyquist.
        let from = (hz_to_bin(s.low as f32, size, rate).ceil().max(1.0)) as usize;
        let to = (hz_to_bin(s.high as f32, size, rate).floor() as usize).min(top - 1);
        for (k, bin) in spectrum.iter_mut().enumerate() {
            if k < from || k > to {
                self.level[k] = 0.0;
                continue;
            }
            if !s.freeze {
                self.level[k] = bin.norm().max(self.level[k] * s.decay);
            }
            let phase = self.random.next_bipolar() as f32 * PI;
            *bin = *bin * s.residual
                + Complex32::from_polar(self.level[k] * CLOUD_GAIN * s.wet_gain, phase);
        }
    }
}

/// One channel: the STFT, the cloud and the dry path.
struct Lane {
    stft: Stft,
    cloud: Cloud,
    dry: DelayLine,
}

struct SpectralBlur {
    lanes: Vec<Lane>,
    size: usize,
    /// The bins each point of the display's spectrum covers.
    rows: Vec<(usize, usize)>,
    /// What a bin's magnitude is multiplied by to read a sine's amplitude.
    scale: f32,
    shown: [f32; DISPLAY_LEN],
}

impl SpectralBlur {
    fn new(device: &Device, config: &NodeConfig) -> Self {
        let rate = config.sample_rate.as_f64();
        let size = ((SIZE_AT_48K * rate / 48_000.0).round() as usize).next_power_of_two();
        let seed = device.id.0.get();
        let lanes: Vec<Lane> = (0..config.channels)
            .map(|channel| {
                let stft = Stft::new(size, OVERLAP, Window::Hann).expect("a valid frame size");
                Lane {
                    cloud: Cloud {
                        level: vec![0.0; stft.bins()],
                        random: Random::seeded(seed.wrapping_add(channel as u64)),
                    },
                    dry: DelayLine::new(stft.latency()),
                    stft,
                }
            })
            .collect();
        Self {
            size,
            rows: row_bins(DISPLAY_LEN, 20.0, 20_000.0, true, size, rate),
            scale: lanes[0].stft.amplitude_scale(),
            lanes,
            shown: [FLOOR_DB; DISPLAY_LEN],
        }
    }
}

impl NativeDevice for SpectralBlur {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        let rate = audio.sample_rate().as_f64();
        let ix: [usize; PARAMS] = core::array::from_fn(|id| {
            params
                .index(ParamId(id as u32))
                .expect("the description lists every parameter")
        });
        let ramp = |id: ParamId| params.ramp(ix[id.0 as usize]);
        let (low, high, halo, residual) = (ramp(LOW), ramp(HIGH), ramp(HALO), ramp(RESIDUAL));
        let (freeze, wet_gain, dry_wet) = (ramp(FREEZE), ramp(WET_GAIN), ramp(DRY_WET));
        let size = self.size;
        let hop_seconds = (size / OVERLAP) as f64 / rate;
        let settings = |i: usize| {
            let low = f64::from(low[i]);
            Settings {
                low,
                high: f64::from(high[i]).max(low),
                decay: 10f64.powf(-3.0 * hop_seconds / (f64::from(halo[i]) / 1_000.0)) as f32,
                residual: residual[i] / 100.0,
                wet_gain: wet_gain[i],
                freeze: freeze[i] >= 0.5,
            }
        };
        let latency = size - 1;
        let channels = self.lanes.len().min(audio.channels());
        for (i, percent) in dry_wet.iter().enumerate().take(audio.frames()) {
            let mix = percent / 100.0;
            for channel in 0..channels {
                let lane = &mut self.lanes[channel];
                let input = audio.channel(channel)[i];
                let dry = lane.dry.read(latency);
                lane.dry.write(input);
                let Lane { stft, cloud, .. } = lane;
                let (rows, scale, shown) = (&self.rows, self.scale, &mut self.shown);
                let wet = stft.process_sample(input, |spectrum| {
                    if channel == 0 {
                        for (point, &(a, b)) in shown.iter_mut().zip(rows) {
                            let peak = spectrum[a..=b].iter().map(|c| c.norm()).fold(0.0, f32::max);
                            *point = (20.0 * (peak * scale).max(1e-6).log10()).max(FLOOR_DB);
                        }
                    }
                    cloud.hop(spectrum, &settings(i), size, rate);
                });
                audio.channel_mut(channel)[i] = dry * (1.0 - mix) + wet * mix;
            }
        }
    }

    fn latency(&self) -> u32 {
        (self.size - 1) as u32
    }

    fn display_len(&self) -> usize {
        DISPLAY_LEN
    }

    fn display(&mut self, out: &mut [f32]) {
        out.copy_from_slice(&self.shown);
    }
}

/// What the display shows of Spectral Blur: the input's spectrum, when an
/// engine runs, with a marker on each edge of the range that drags Low or
/// High. `values` are the device's set values and `data` what its node
/// published.
#[must_use]
pub fn display(
    device: DeviceId,
    description: &DeviceDescription,
    values: &OrdMap<ParamId, f64>,
    data: Option<&[f32]>,
) -> DeviceDisplay {
    let value = |id: ParamId| description.value(values, id).unwrap_or(0.0);
    let low = value(LOW);
    let high = value(HIGH).max(low);
    let spectrum = data.filter(|d| d.len() == DISPLAY_LEN);
    let marker = |name: &str, hz: f64, param: ParamId| DisplayMarker {
        name: name.to_owned(),
        series: "Spectrum".to_owned(),
        x: hz as f32,
        y: -10.0,
        text: format!("{name} {hz:.0} Hz"),
        handle: Some(DisplayHandle { x: param, y: None }),
    };
    DeviceDisplay {
        device,
        live: spectrum.is_some(),
        view: None,
        series: vec![DisplaySeries {
            name: "Spectrum".to_owned(),
            unit: "dB".to_owned(),
            x_unit: "Hz".to_owned(),
            x_range: [20.0, 20_000.0],
            log_x: true,
            y_range: [FLOOR_DB, 0.0],
            detail: false,
            values: spectrum.map(<[f32]>::to_vec).unwrap_or_default(),
        }],
        markers: vec![marker("Low", low, LOW), marker("High", high, HIGH)],
        readings: Vec::new(),
        spectrograms: Vec::new(),
    }
}

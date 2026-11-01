//! Glue Compressor, the native bus compressor (WP-5.21; the bus-compressor spec):
//! Threshold, Ratio (the knee sharpens as the ratio rises, with no knee
//! control of its own), Attack in ms, Release in seconds or Auto, Range,
//! Makeup, Soft Clip, Dry/Wet, the gain reduction and clip readings,
//! Oversampling as a Show Options item, and the sidechain section: the
//! framework's External, Source, Mix, SC Gain and Sidechain Listen, and the
//! device's own SC EQ.
//!
//! The detector hears the key (`Audio::sidechain`, or the device's own
//! input) through the SC EQ when it's on, as the larger of the two
//! channels' magnitudes. The static curve turns its level in dB into a
//! reduction in dB (Compressor's gain computer, with a knee that narrows with
//! the ratio), capped by Range; the reduction rises with Attack and falls
//! with Release, both time constants of the reduction in dB. The gain is the
//! reduction taken off, plus Makeup, then Soft Clip, then the dry signal
//! blended in. With Oversampling on, the gain and Soft Clip run at twice the
//! rate, and the trip's latency is the device's.
//!
//! Its ranges and defaults, the knee law and Auto's two times are
//! Beatlayer's own; the manual gives none (see `docs/specs/bus-compressor.md`).

use bl_dsp::Oversampler;
use bl_engine::{DeviceType, NodeConfig, Registry};
use bl_model::Device;
use bl_model::device_layout::{BODY_HEIGHT, ControlBand, DisplayCell, LayoutNode, Widget};
use bl_model::param::db_to_gain;
use bl_model::{
    DeviceDescription, DeviceDisplay, DeviceId, DeviceLayout, DeviceRole, DisplayReading, OrdMap,
    ParamDescriptor, ParamId, ParamUnit,
};

use crate::dynamics::{SC_TYPES, ScFilter, curve_db, level_db, step_of};
use crate::framework::{Audio, DelayLine, NativeDevice, ParamBank, add};
use crate::sections::REFRESH_FRAMES;

/// Glue Compressor's device type.
pub const BUS_COMPRESSOR: &str = "beatlayer.bond";

/// Threshold: the level above which the gain turns down, in dB.
pub const THRESHOLD: ParamId = ParamId(0);
/// Ratio: the input's dB per the output's dB above the threshold.
pub const RATIO: ParamId = ParamId(1);
/// Attack: how long the reduction takes to rise, in ms.
pub const ATTACK: ParamId = ParamId(2);
/// Release: how long the reduction takes to fall, in seconds.
pub const RELEASE: ParamId = ParamId(3);
/// Auto Release: the release follows the signal.
pub const AUTO_RELEASE: ParamId = ParamId(4);
/// Range: the deepest reduction, in dB; 0 dB is no compression.
pub const RANGE: ParamId = ParamId(5);
/// Makeup: the gain after the compression, reaching silence.
pub const MAKEUP: ParamId = ParamId(6);
/// Soft Clip: a waveshaper holds the wet peaks under -0.5 dB.
pub const SOFT_CLIP: ParamId = ParamId(7);
/// Dry/Wet: the balance between the compressed and dry signals, in per cent.
pub const DRY_WET: ParamId = ParamId(8);
/// SC EQ: whether only a band of the detector's signal triggers it.
pub const SC_EQ: ParamId = ParamId(9);
/// Type: the SC EQ's shape.
pub const SC_TYPE: ParamId = ParamId(10);
/// Freq: the SC EQ's cutoff or center frequency, in Hz.
pub const SC_FREQ: ParamId = ParamId(11);
/// Res: the SC EQ's resonance.
pub const SC_RES: ParamId = ParamId(12);
/// Oversampling: the gain stage runs at twice the rate (a Show Options item).
pub const OVERSAMPLING: ParamId = ParamId(13);

/// The ratio range.
pub const RATIO_RANGE: (f64, f64) = (1.0, 20.0);
/// The knee's width at a ratio of 1, in dB; it falls as `KNEE_DB / ratio`.
pub const KNEE_DB: f64 = 12.0;
/// Auto Release: a reduction falls half with a fast time constant and half
/// with a slow one, in ms.
pub const AUTO_FAST_MS: f64 = 100.0;
pub const AUTO_SLOW_MS: f64 = 1_000.0;
/// Soft Clip's ceiling, in dB.
pub const SOFT_CLIP_DB: f64 = -0.5;

/// The clip light's states, as published.
const LIGHT_OFF: f32 = 0.0;
const LIGHT_CLIP: f32 = 1.0;
const LIGHT_SOFT_CLIP: f32 = 2.0;

/// What the node publishes: the gain reduction in dB (0 or below) and the
/// clip light.
const DISPLAY_VALUES: usize = 2;

/// The knee's width at `ratio`, in dB.
#[must_use]
pub fn knee_db(ratio: f64) -> f64 {
    KNEE_DB / ratio
}

/// The reduction in dB (0 or more) the static curve asks for at a level of
/// `level_db`, before Range caps it.
#[must_use]
pub fn reduction_db(level_db: f64, threshold: f64, ratio: f64) -> f64 {
    -curve_db(
        level_db,
        threshold,
        1.0 - 1.0 / ratio,
        knee_db(ratio),
        false,
    )
}

/// Glue Compressor's device type.
#[must_use]
pub fn bus_compressor_type() -> DeviceType {
    let params = vec![
        ParamDescriptor::continuous(
            THRESHOLD,
            "Threshold",
            (-60.0, 0.0),
            0.0,
            ParamUnit::Decibels,
        )
        .describe("The level above which the compressor turns the gain down."),
        ParamDescriptor::continuous(RATIO, "Ratio", RATIO_RANGE, 4.0, ParamUnit::Ratio)
            .logarithmic()
            .describe(
                "How much input above the threshold it takes to raise the output by one dB; \
                 the knee narrows as the ratio rises.",
            ),
        ParamDescriptor::continuous(
            ATTACK,
            "Attack",
            (0.01, 100.0),
            10.0,
            ParamUnit::Milliseconds,
        )
        .logarithmic()
        .describe("How long the compressor takes to reach full compression."),
        ParamDescriptor::continuous(RELEASE, "Release", (0.05, 4.0), 0.2, ParamUnit::Seconds)
            .logarithmic()
            .describe("How long the compressor takes to let go once the signal has fallen."),
        ParamDescriptor::toggle(AUTO_RELEASE, "Auto", false).describe(
            "Lets the release follow the audio: a fast part for transients and a slow one.",
        ),
        ParamDescriptor::continuous(RANGE, "Range", (-70.0, 0.0), -60.0, ParamUnit::Decibels)
            .describe("The most the compressor can turn the gain down; 0 dB is no compression."),
        ParamDescriptor::gain(MAKEUP, "Makeup", 24.0, 0.0)
            .describe("Adds gain to offset the compression."),
        ParamDescriptor::toggle(SOFT_CLIP, "Soft Clip", false)
            .describe("Rounds loud peaks with a waveshaper that holds the output under -0.5 dB."),
        ParamDescriptor::continuous(DRY_WET, "Dry/Wet", (0.0, 100.0), 100.0, ParamUnit::Percent)
            .describe("The balance between the compressed and the dry signal."),
        ParamDescriptor::toggle(SC_EQ, "SC EQ", false).describe(
            "Listens through a filter, so only a band of the detector's signal triggers it.",
        ),
        ParamDescriptor::choice(SC_TYPE, "Type", &SC_TYPES, 0)
            .describe("The sidechain EQ's shape."),
        ParamDescriptor::continuous(SC_FREQ, "Freq", (20.0, 20_000.0), 500.0, ParamUnit::Hertz)
            .logarithmic()
            .describe("The sidechain EQ's cutoff or center frequency."),
        ParamDescriptor::continuous(SC_RES, "Res", (0.1, 10.0), 0.71, ParamUnit::None)
            .logarithmic()
            .describe("The sidechain EQ's resonance."),
        ParamDescriptor::toggle(OVERSAMPLING, "Oversampling", false).describe(
            "Runs the gain stage at twice the sample rate, for less aliasing from fast \
             compression and Soft Clip; it costs a little latency and processing.",
        ),
    ];
    DeviceType::native(BUS_COMPRESSOR, "Glue Compressor", DeviceRole::AudioEffect)
        .with_sidechain()
        .with_description(
            DeviceDescription::new(params)
                .with_layout(layout())
                .with_menu_options(&[OVERSAMPLING])
                .with_banks(&[
                    (
                        "Compressor",
                        &[
                            "Threshold",
                            "Ratio",
                            "Attack",
                            "Release",
                            "Auto",
                            "Range",
                            "Makeup",
                            "Dry/Wet",
                        ],
                    ),
                    ("Sidechain", &["SC EQ", "Type", "Freq", "Res"]),
                    ("Output", &["Soft Clip"]),
                ]),
        )
}

/// Threshold and Ratio, the times, the gain reduction's display, the
/// output stage, and the SC EQ.
fn layout() -> DeviceLayout {
    DeviceLayout::new(LayoutNode::Row(vec![
        LayoutNode::Band(ControlBand::stacked(
            BODY_HEIGHT,
            &[(THRESHOLD, Widget::Knob), (RATIO, Widget::Knob)],
        )),
        LayoutNode::Band(ControlBand::stacked(
            BODY_HEIGHT,
            &[
                (ATTACK, Widget::Knob),
                (RELEASE, Widget::Knob),
                (AUTO_RELEASE, Widget::Toggle),
            ],
        )),
        LayoutNode::Cell(DisplayCell::new("Gain Reduction", 60.0, BODY_HEIGHT, 1.0)),
        LayoutNode::Band(ControlBand::stacked(
            BODY_HEIGHT,
            &[
                (RANGE, Widget::Knob),
                (MAKEUP, Widget::Knob),
                (SOFT_CLIP, Widget::Toggle),
                (DRY_WET, Widget::Knob),
            ],
        )),
        LayoutNode::Band(ControlBand::stacked(
            BODY_HEIGHT,
            &[
                (SC_EQ, Widget::Toggle),
                (SC_TYPE, Widget::Chooser),
                (SC_FREQ, Widget::Knob),
                (SC_RES, Widget::Knob),
            ],
        )),
    ]))
}

/// Adds Glue Compressor to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, bus_compressor_type(), |device, config| {
        Bond::new(device, config)
    });
}

/// The ceiling Soft Clip rounds toward, as a linear amplitude.
fn clip_ceiling() -> f32 {
    db_to_gain(SOFT_CLIP_DB) as f32
}

/// Soft Clip's waveshaper: a tanh that leaves small signals alone and never
/// passes the ceiling.
#[inline]
fn soft_clip(x: f32, ceiling: f32) -> f32 {
    ceiling * (x / ceiling).tanh()
}

/// Glue Compressor's audio-thread half.
pub struct Bond {
    rate: f64,
    /// Each frame's gain, Makeup included.
    gains: Vec<f32>,
    filter: [ScFilter; 2],
    /// The reduction in dB now, as two followers for Auto Release (both are
    /// the one reduction when Auto is off).
    fast: f32,
    slow: f32,
    oversamplers: Oversampler,
    oversampled: bool,
    /// The dry signal delayed by the oversampler's trip.
    dry: [DelayLine; 2],
    latency: u32,
    /// The gain reduction in dB now, and the loudest wet sample, before and
    /// after the clip, since the display last published.
    reduction: f32,
    /// The deepest reduction the last block reached, in dB (the mixer view
    /// spec, R40).
    block_reduction: f32,
    peak_wet: f32,
    peak_out: f32,
    soft_clipping: bool,
    /// The dry signal of the stretch being processed.
    scratch: Vec<f32>,
    /// What the continuous parameters came to at the last refresh, and the
    /// frames since the device was made, so the refreshes fall on the same
    /// frames at any block size.
    settings: Settings,
    tick: usize,
}

/// The continuous parameters' values the detector works from, taken every
/// [`REFRESH_FRAMES`] frames.
#[derive(Clone, Copy, Default)]
struct Settings {
    threshold: f64,
    ratio: f64,
    /// The deepest reduction, in dB.
    cap: f64,
    attack: f32,
    release: f32,
    fast: f32,
    slow: f32,
}

impl Bond {
    fn new(device: &Device, config: &NodeConfig) -> Self {
        let frames = config.max_frames.max(1);
        let oversamplers = Oversampler::new(1, 2, frames);
        let on = device.params.get(&OVERSAMPLING).is_some_and(|v| *v >= 0.5);
        let latency = oversamplers.latency();
        Self {
            rate: config.sample_rate.as_f64(),
            gains: vec![1.0; frames],
            filter: [ScFilter::new(), ScFilter::new()],
            fast: 0.0,
            slow: 0.0,
            oversamplers,
            oversampled: on,
            dry: [
                DelayLine::new(latency as usize),
                DelayLine::new(latency as usize),
            ],
            latency: if on { latency } else { 0 },
            reduction: 0.0,
            block_reduction: 0.0,
            peak_wet: 0.0,
            peak_out: 0.0,
            soft_clipping: false,
            scratch: Vec::with_capacity(frames),
            settings: Settings::default(),
            tick: 0,
        }
    }

    /// The gain reduction now, in dB (0 or below), for a meter.
    #[must_use]
    pub fn gain_reduction_db(&self) -> f32 {
        self.reduction
    }
}

impl NativeDevice for Bond {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        let frames = audio.frames();
        let channels = audio.channels().min(2);
        if frames == 0 || channels == 0 {
            return;
        }
        let at = |id| params.index(id).expect("the description holds it");
        let ramp = |id| params.ramp(at(id));
        let (threshold, ratio, range) = (ramp(THRESHOLD), ramp(RATIO), ramp(RANGE));
        let (attack, release) = (ramp(ATTACK), ramp(RELEASE));
        let (sc_freq, sc_res) = (ramp(SC_FREQ), ramp(SC_RES));
        let (auto, makeup) = (ramp(AUTO_RELEASE), ramp(MAKEUP));
        let sc_on = params.value(at(SC_EQ)) >= 0.5;
        let sc_kind = params.value(at(SC_TYPE)) as usize;
        let oversample = params.value(at(OVERSAMPLING)) >= 0.5;
        if oversample != self.oversampled {
            self.oversampled = oversample;
            self.oversamplers.reset();
        }
        self.latency = if oversample {
            self.oversamplers.latency()
        } else {
            0
        };

        // 1. The reduction, frame by frame.
        self.block_reduction = 0.0;
        for i in 0..frames {
            if self.tick.is_multiple_of(REFRESH_FRAMES) {
                self.settings = Settings {
                    threshold: f64::from(threshold[i]),
                    ratio: f64::from(ratio[i]).clamp(RATIO_RANGE.0, RATIO_RANGE.1),
                    cap: -f64::from(range[i]).min(0.0),
                    attack: step_of(f64::from(attack[i]), self.rate),
                    release: step_of(f64::from(release[i]) * 1_000.0, self.rate),
                    fast: step_of(AUTO_FAST_MS, self.rate),
                    slow: step_of(AUTO_SLOW_MS, self.rate),
                };
                for filter in &mut self.filter {
                    filter.set(
                        sc_kind,
                        f64::from(sc_freq[i]),
                        f64::from(sc_res[i]),
                        self.rate,
                    );
                }
            }
            self.tick += 1;
            let s = self.settings;
            let (fast_step, slow_step) = if auto[i] >= 0.5 {
                (s.fast, s.slow)
            } else {
                (s.release, s.release)
            };
            let mut key = 0.0f32;
            for channel in 0..channels {
                let keyed = audio.sidechain(channel).map(|key| key[i]);
                let sample = keyed.unwrap_or_else(|| audio.channel_mut(channel)[i]);
                let heard = if sc_on {
                    self.filter[channel].process(sample)
                } else {
                    sample
                };
                key = key.max(heard.abs());
            }
            let wanted =
                reduction_db(level_db(f64::from(key)), s.threshold, s.ratio).min(s.cap) as f32;
            for (state, release) in [(&mut self.fast, fast_step), (&mut self.slow, slow_step)] {
                let step = if wanted > *state { s.attack } else { release };
                *state += (wanted - *state) * step;
            }
            self.reduction = -0.5 * (self.fast + self.slow);
            self.block_reduction = self.block_reduction.min(self.reduction);
            self.gains[i] = db_to_gain(f64::from(self.reduction)) as f32 * makeup[i];
        }

        // 2. The gain, Soft Clip, and the dry signal, at twice the rate with
        // Oversampling on.
        let soft = params.value(at(SOFT_CLIP)) >= 0.5;
        let ceiling = clip_ceiling();
        let wet = params.ramp(at(DRY_WET));
        let mut louder = 0.0f32;
        let mut clipped = 0.0f32;
        let latency = self.latency as usize;
        let mut dry_samples = std::mem::take(&mut self.scratch);
        for channel in 0..channels {
            let samples = audio.channel_mut(channel);
            // The dry signal, from the trip ago when the wet one makes it.
            let delay = &mut self.dry[channel];
            dry_samples.clear();
            dry_samples.extend(samples.iter().map(|sample| {
                let dry = if latency == 0 {
                    *sample
                } else {
                    delay.read(latency)
                };
                delay.write(*sample);
                dry
            }));
            let gains = &self.gains[..frames];
            let (mut peak_wet, mut peak_out) = (0.0f32, 0.0f32);
            if oversample {
                let mut at_frame = 0usize;
                self.oversamplers.process(channel, samples, |high| {
                    for sample in high {
                        let x = *sample * gains[at_frame / 2];
                        at_frame += 1;
                        peak_wet = peak_wet.max(x.abs());
                        *sample = if soft { soft_clip(x, ceiling) } else { x };
                    }
                });
            } else {
                for (sample, g) in samples.iter_mut().zip(gains) {
                    let x = *sample * g;
                    peak_wet = peak_wet.max(x.abs());
                    *sample = if soft { soft_clip(x, ceiling) } else { x };
                }
            }
            for ((sample, dry), mix) in samples.iter_mut().zip(&dry_samples).zip(wet) {
                let mix = mix * 0.01;
                *sample = *sample * mix + dry * (1.0 - mix);
                peak_out = peak_out.max(sample.abs());
            }
            louder = louder.max(peak_wet);
            clipped = clipped.max(peak_out);
        }
        self.scratch = dry_samples;
        self.peak_wet = self.peak_wet.max(louder);
        self.peak_out = self.peak_out.max(clipped);
        self.soft_clipping = soft;
    }

    fn latency(&self) -> u32 {
        self.latency
    }

    /// The deepest reduction the last block reached, in dB (the mixer view
    /// spec, R40).
    fn gain_reduction(&self) -> f32 {
        self.block_reduction
    }

    fn display_len(&self) -> usize {
        DISPLAY_VALUES
    }

    fn display(&mut self, out: &mut [f32]) {
        out[0] = self.reduction;
        out[1] = if self.soft_clipping && self.peak_wet > clip_ceiling() {
            LIGHT_SOFT_CLIP
        } else if self.peak_out > 1.0 {
            LIGHT_CLIP
        } else {
            LIGHT_OFF
        };
        self.peak_wet = 0.0;
        self.peak_out = 0.0;
    }
}

/// The device's display (the bus-compressor spec): once an engine
/// publishes them, the gain reduction in dB and the clip light. `data` is
/// what the node published, if an engine runs.
#[must_use]
pub fn display(
    device: DeviceId,
    _description: &DeviceDescription,
    _values: &OrdMap<ParamId, f64>,
    data: Option<&[f32]>,
) -> DeviceDisplay {
    let readings = data
        .and_then(|d| <[f32; DISPLAY_VALUES]>::try_from(d).ok())
        .map(|[reduction, light]| {
            vec![
                DisplayReading {
                    name: "Gain Reduction".to_owned(),
                    value: reduction,
                    unit: "dB".to_owned(),
                    text: format!("{reduction:.1} dB"),
                },
                DisplayReading {
                    name: "Clip".to_owned(),
                    value: light,
                    unit: String::new(),
                    text: if light == LIGHT_SOFT_CLIP {
                        "soft clipping"
                    } else if light == LIGHT_CLIP {
                        "clipping"
                    } else {
                        "off"
                    }
                    .to_owned(),
                },
            ]
        })
        .unwrap_or_default();
    DeviceDisplay {
        spectrograms: Vec::new(),
        device,
        live: data.is_some(),
        view: None,
        series: Vec::new(),
        markers: Vec::new(),
        readings,
    }
}

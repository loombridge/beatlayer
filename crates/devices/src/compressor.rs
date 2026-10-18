//! Squeeze, the native compressor (WP-2.18a, part 2; the compressor spec):
//! threshold, ratio to infinity, attack, release with Auto Release, knee,
//! the Peak, RMS and Expand modes, the Lin and Log envelope shapes, look
//! ahead, Makeup, Out, Dry/Wet, and the sidechain section: the framework's
//! External, Source, Mix, SC Gain and Sidechain Listen, and the device's
//! own SC Filter.
//!
//! The detector listens to what the engine hands it on the device's
//! sidechain (`Block::sidechain`, WP-2.19b: the source track through the
//! sidechain's gain and mix, or the device's own input), through the SC
//! Filter when it's on. The static curve turns the detector's level into a
//! gain above the threshold: compression reduces it by the ratio over the
//! knee, expansion raises it. The gain falls with the Attack time constant
//! and recovers with the Release one (or a signal-dependent time with
//! Auto), and the signal is delayed by the look-ahead, so the gain has
//! already begun to fall when a peak comes out; that delay is the device's
//! latency for delay compensation (WP-2.11).
//!
//! Its parameters, ranges and defaults are Beatlayer's own reading of Live
//! 12's Compressor (see `docs/specs/compressor.md`); the manual records no
//! ranges or defaults, so those are Beatlayer's and are listed to observe.

use bl_engine::{DeviceType, NodeConfig, Registry};
use bl_model::Device;
use bl_model::device_layout::{BODY_HEIGHT, ControlBand, DisplayCell, LayoutNode, Widget};
use bl_model::param::db_to_gain;
use bl_model::{
    DeviceDescription, DeviceDisplay, DeviceId, DeviceLayout, DeviceRole, DisplayMarker,
    DisplayReading, DisplaySeries, OrdMap, ParamDescriptor, ParamId, ParamUnit,
};

use crate::dynamics::{
    LOOKAHEAD_SECONDS, SC_TYPES, ScFilter, curve_db, level_db, lookahead_frames, step_of,
};
use crate::framework::{Audio, DelayLine, NativeDevice, ParamBank, add};

/// Squeeze's device type.
pub const COMPRESSOR: &str = "beatlayer.squeeze";

/// Threshold: the level above which the gain turns down, in dB.
pub const THRESHOLD: ParamId = ParamId(0);
/// Ratio: the input's dB per the output's dB above the threshold.
pub const RATIO: ParamId = ParamId(1);
/// Attack: how long the gain takes to fall to a new reduction, in ms.
pub const ATTACK: ParamId = ParamId(2);
/// Release: how long the gain takes to recover, in milliseconds.
pub const RELEASE: ParamId = ParamId(3);
/// Auto Release: whether the release time follows the signal.
pub const AUTO_RELEASE: ParamId = ParamId(4);
/// Knee: the width of the gradual turn above and below the threshold, in dB.
pub const KNEE: ParamId = ParamId(5);
/// Mode: Peak, RMS, or Expand.
pub const MODE: ParamId = ParamId(6);
/// Env: the Lin or Log envelope shape.
pub const ENVELOPE: ParamId = ParamId(7);
/// Look: zero, one, or ten milliseconds of look-ahead.
pub const LOOKAHEAD: ParamId = ParamId(8);
/// Out: the output level, a gain reaching silence.
pub const OUT: ParamId = ParamId(9);
/// Makeup: automatic output compensation for the threshold and ratio.
pub const MAKEUP: ParamId = ParamId(10);
/// Dry/Wet: the balance between the compressed and dry signals, in per cent.
pub const DRY_WET: ParamId = ParamId(11);
/// SC Filter: whether only a band of the detector's signal triggers it.
pub const SC_FILTER: ParamId = ParamId(12);
/// Type: the SC Filter's shape.
pub const SC_TYPE: ParamId = ParamId(13);
/// Freq: the SC Filter's cutoff or center frequency, in Hz.
pub const SC_FREQ: ParamId = ParamId(14);
/// Res: the SC Filter's resonance.
pub const SC_RES: ParamId = ParamId(15);

/// The Ratio's top of range, which means an infinite ratio: nothing above
/// the threshold gets through. The framework's ratio values are finite, so
/// the top is 100; what Live's range is and what its control reads at the
/// top are to observe.
const RATIO_MAX: f64 = 100.0;

/// The Lookahead choice a device without a saved value starts at.
const DEFAULT_LOOKAHEAD: usize = 0;

/// The Mode choices, by their indices.
const MODE_RMS: usize = 1;
const MODE_EXPAND: usize = 2;

/// The Env choices: Lin, then Log.
const ENV_LOG: usize = 1;

/// The RMS detector's averaging time, in milliseconds: a longer window than
/// any peak, so short peaks barely register in RMS mode. Beatlayer's own.
const RMS_MS: f64 = 10.0;

/// Auto Release: the quickest and slowest release times it moves between,
/// in milliseconds, and how long a sustained reduction takes to reach the
/// slow one. Beatlayer's own; Live doesn't record its algorithm.
const AUTO_FAST_MS: f64 = 50.0;
const AUTO_SLOW_MS: f64 = 1000.0;
const AUTO_SUSTAIN_SECONDS: f64 = 0.3;

/// Log: at this many dB of reduction the release step is twice Lin's.
/// Beatlayer's own; Live's shape is to observe.
const LOG_DEPTH_DB: f32 = 6.0;

/// The Squeeze device type.
#[must_use]
pub fn compressor_type() -> DeviceType {
    let params = vec![
        ParamDescriptor::continuous(
            THRESHOLD,
            "Threshold",
            (-60.0, 0.0),
            0.0,
            ParamUnit::Decibels,
        )
        .describe("The level above which the compressor turns the gain down."),
        ParamDescriptor::continuous(RATIO, "Ratio", (1.0, RATIO_MAX), 4.0, ParamUnit::Ratio)
            .logarithmic()
            .describe(
                "How much input above the threshold it takes to raise the output by one dB; \
                 the top of the range is an infinite ratio.",
            ),
        ParamDescriptor::continuous(
            ATTACK,
            "Attack",
            (0.01, 1000.0),
            1.0,
            ParamUnit::Milliseconds,
        )
        .logarithmic()
        .describe("How long the gain takes to fall to a new reduction."),
        ParamDescriptor::continuous(
            RELEASE,
            "Release",
            (1.0, 1500.0),
            30.0,
            ParamUnit::Milliseconds,
        )
        .logarithmic()
        .describe("How long the gain takes to recover once the input falls below the threshold."),
        ParamDescriptor::toggle(AUTO_RELEASE, "Auto Release", false)
            .describe("Lets the compressor pick the release time from the signal."),
        ParamDescriptor::continuous(KNEE, "Knee", (0.0, 24.0), 6.0, ParamUnit::Decibels)
            .describe("The width of the gradual turn around the threshold."),
        ParamDescriptor::choice(MODE, "Mode", &["Peak", "RMS", "Expand"], 0).describe(
            "Whether the detector follows short peaks, an RMS level, or expands above \
                 the threshold.",
        ),
        ParamDescriptor::choice(ENVELOPE, "Env", &["Lin", "Log"], 0)
            .describe("The envelope's shape: linear, or releasing faster from deeper reductions."),
        ParamDescriptor::choice(
            LOOKAHEAD,
            "Look",
            &["0 ms", "1 ms", "10 ms"],
            DEFAULT_LOOKAHEAD,
        )
        .describe("How far ahead the detector sees a peak; it delays the output as long."),
        ParamDescriptor::gain(OUT, "Out", 24.0, 0.0).describe("The output level, down to silence."),
        ParamDescriptor::toggle(MAKEUP, "Makeup", false).describe(
            "Compensates the output for the threshold and ratio, unless an external \
             sidechain is chosen.",
        ),
        ParamDescriptor::continuous(DRY_WET, "Dry/Wet", (0.0, 100.0), 100.0, ParamUnit::Percent)
            .describe("The balance between the compressed and the dry signal."),
        ParamDescriptor::toggle(SC_FILTER, "SC Filter", false).describe(
            "Listens through a filter, so only a band of the detector's signal triggers it.",
        ),
        ParamDescriptor::choice(SC_TYPE, "Type", &SC_TYPES, 0)
            .describe("The sidechain filter's shape."),
        ParamDescriptor::continuous(SC_FREQ, "Freq", (20.0, 20_000.0), 500.0, ParamUnit::Hertz)
            .logarithmic()
            .describe("The sidechain filter's cutoff or center frequency."),
        ParamDescriptor::continuous(SC_RES, "Res", (0.1, 10.0), 0.71, ParamUnit::None)
            .logarithmic()
            .describe("The sidechain filter's resonance."),
    ];
    DeviceType::native(COMPRESSOR, "Squeeze", DeviceRole::AudioEffect)
        .with_sidechain()
        .with_description(
            DeviceDescription::new(params)
                .with_layout(layout())
                .with_views(&VIEWS),
        )
}

/// The device's layout: the ratio, attack's and release's controls on the
/// left, the gain reduction's display between them with the threshold and
/// Out on its top edge and the knee, Makeup, Look, Dry/Wet and Mode on its
/// bottom one, and the SC Filter's controls on the right.
fn layout() -> DeviceLayout {
    DeviceLayout::new(LayoutNode::Row(vec![
        LayoutNode::Band(ControlBand::stacked(
            BODY_HEIGHT,
            &[
                (RATIO, Widget::Knob),
                (ATTACK, Widget::Knob),
                (RELEASE, Widget::Knob),
                (AUTO_RELEASE, Widget::Toggle),
                (ENVELOPE, Widget::Chooser),
            ],
        )),
        LayoutNode::Cell(
            DisplayCell::new("Gain Reduction", 170.0, BODY_HEIGHT, 1.0)
                .with_top(ControlBand::stacked(
                    60.0,
                    &[(THRESHOLD, Widget::Knob), (OUT, Widget::Knob)],
                ))
                .with_bottom(ControlBand::stacked(
                    96.0,
                    &[
                        (KNEE, Widget::Knob),
                        (MAKEUP, Widget::Toggle),
                        (LOOKAHEAD, Widget::Chooser),
                        (DRY_WET, Widget::Knob),
                        (MODE, Widget::Chooser),
                    ],
                )),
        ),
        LayoutNode::Band(ControlBand::stacked(
            BODY_HEIGHT,
            &[
                (SC_FILTER, Widget::Toggle),
                (SC_TYPE, Widget::Chooser),
                (SC_FREQ, Widget::Knob),
                (SC_RES, Widget::Knob),
            ],
        )),
    ]))
}

/// Adds the Squeeze compressor to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, compressor_type(), |device, config| {
        Squeeze::new(device, config)
    });
}

/// The dB the static curve moves per dB over the threshold. The top of the
/// ratio's range is an infinite ratio: the slope is 1 (expansion's stays
/// finite).
fn curve_slope(ratio: f64, expand: bool) -> f64 {
    if expand {
        ratio - 1.0
    } else if ratio >= RATIO_MAX {
        1.0
    } else {
        1.0 - 1.0 / ratio
    }
}

/// Squeeze's audio-thread half.
pub struct Squeeze {
    rate: f64,
    /// The most samples of look-ahead any choice asks for.
    max_lookahead: usize,
    /// The look-ahead in force, in samples: the output's delay.
    lookahead: u32,
    /// Each frame's least gain across the channels, and the gain each frame
    /// takes after the envelope.
    targets: Vec<f32>,
    gains: Vec<f32>,
    delay: [DelayLine; 2],
    filter: [ScFilter; 2],
    /// Each channel's mean square, for the RMS detector.
    rms: [f32; 2],
    /// The gain now, and how long a reduction has been held (Auto Release).
    gain: f32,
    hold_frames: f32,
    /// The loudest input and output samples since the display last
    /// published, as linear amplitude.
    peak_in: f32,
    peak_out: f32,
}

impl Squeeze {
    fn new(device: &Device, config: &NodeConfig) -> Self {
        let rate = config.sample_rate.as_f64();
        let max_lookahead =
            (LOOKAHEAD_SECONDS[LOOKAHEAD_SECONDS.len() - 1] * rate).ceil() as usize + 1;
        let choice = device
            .params
            .get(&LOOKAHEAD)
            .copied()
            .unwrap_or(DEFAULT_LOOKAHEAD as f64);
        let frames = config.max_frames.max(1);
        Self {
            rate,
            max_lookahead,
            lookahead: lookahead_frames(choice, rate, max_lookahead) as u32,
            targets: vec![0.0; frames],
            gains: vec![0.0; frames],
            delay: [DelayLine::new(max_lookahead), DelayLine::new(max_lookahead)],
            filter: [ScFilter::new(), ScFilter::new()],
            rms: [0.0; 2],
            gain: 1.0,
            hold_frames: 0.0,
            peak_in: 0.0,
            peak_out: 0.0,
        }
    }
}

impl NativeDevice for Squeeze {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        let frames = audio.frames();
        let channels = audio.channels().min(2);
        if frames == 0 || channels == 0 {
            return;
        }
        // Where each parameter sits in the bank, which the description
        // always holds.
        let at = |id| params.index(id).expect("the description holds it");

        let threshold = params.value(at(THRESHOLD));
        let ratio = params.value(at(RATIO)).clamp(1.0, RATIO_MAX);
        let mode = (params.value(at(MODE)) as usize).min(MODE_EXPAND);
        let expand = mode == MODE_EXPAND;
        let slope = curve_slope(ratio, expand);
        let knee = params.value(at(KNEE));
        let auto = params.value(at(AUTO_RELEASE)) >= 0.5;
        let log_envelope = params.value(at(ENVELOPE)) as usize == ENV_LOG;
        let attack_step = step_of(params.value(at(ATTACK)), self.rate);
        // Auto Release moves between a fast time just after a peak and a
        // slow one while a reduction is held.
        let release_ms = if auto {
            let sustained =
                (f64::from(self.hold_frames) / (AUTO_SUSTAIN_SECONDS * self.rate)).clamp(0.0, 1.0);
            AUTO_FAST_MS + (AUTO_SLOW_MS - AUTO_FAST_MS) * sustained
        } else {
            params.value(at(RELEASE))
        };
        let release_step = step_of(release_ms, self.rate);
        let rms_step = step_of(RMS_MS, self.rate);
        let lookahead =
            lookahead_frames(params.value(at(LOOKAHEAD)), self.rate, self.max_lookahead);
        self.lookahead = lookahead as u32;
        // An external sidechain rules Makeup out, as Live's manual says.
        let external = audio.sidechain(0).is_some();
        let makeup = if !expand && !external && params.value(at(MAKEUP)) >= 0.5 {
            db_to_gain(slope * (-threshold).max(0.0)) as f32
        } else {
            1.0
        };
        let sc_on = params.value(at(SC_FILTER)) >= 0.5;
        if sc_on {
            let kind = params.value(at(SC_TYPE)) as usize;
            let hz = params.value(at(SC_FREQ));
            let q = params.value(at(SC_RES));
            for filter in &mut self.filter {
                filter.set(kind, hz, q, self.rate);
            }
        }

        // 1. The detector's level per frame, through the SC Filter when
        // it's on: what the sidechain hands the device, or its own input.
        // Each channel's gain is worked out on its own; the frame takes the
        // least of them, so both channels share one gain reduction (whether
        // Live's shares one is to observe).
        self.targets[..frames].fill(f32::MAX);
        for channel in 0..channels {
            for (i, target) in self.targets[..frames].iter_mut().enumerate() {
                let keyed = audio.sidechain(channel).map(|key| key[i]);
                let sample = keyed.unwrap_or_else(|| audio.channel_mut(channel)[i]);
                self.peak_in = self.peak_in.max(sample.abs());
                let filtered = if sc_on {
                    self.filter[channel].process(sample)
                } else {
                    sample
                };
                let level = if mode == MODE_RMS {
                    let square = f64::from(filtered) * f64::from(filtered);
                    self.rms[channel] += (square as f32 - self.rms[channel]) * rms_step;
                    f64::from(self.rms[channel]).sqrt()
                } else {
                    f64::from(filtered.abs())
                };
                let db = level_db(level);
                let gain = db_to_gain(curve_db(db, threshold, slope, knee, expand)) as f32;
                *target = target.min(gain);
            }
        }

        // 2. The envelope: the gain falls to a new reduction with Attack
        // and recovers with Release (Log scales the release step with the
        // depth of the reduction; Auto reads how long one has been held).
        let mut gain = self.gain;
        for i in 0..frames {
            let target = self.targets[i];
            let step = if target < gain {
                attack_step
            } else if log_envelope {
                // Beatlayer's own Log shape: deeper reductions release
                // faster, so sharply compressed peaks let go sooner.
                let depth = -20.0 * f64::from(gain.max(1e-6)).log10();
                (f64::from(release_step) * (1.0 + depth / f64::from(LOG_DEPTH_DB))).min(1.0) as f32
            } else {
                release_step
            };
            gain += (target - gain) * step;
            self.gains[i] = gain;
            if target < 1.0 {
                self.hold_frames += 1.0;
            } else {
                self.hold_frames = 0.0;
            }
        }
        self.gain = gain;

        // 3. Out: the signal from the look-ahead ago, through the envelope
        // and Out, mixed with the dry signal by Dry/Wet.
        let wet = params.ramp(at(DRY_WET));
        let out = params.ramp(at(OUT));
        for channel in 0..channels {
            for i in 0..frames {
                let sample = audio.channel_mut(channel)[i];
                let delayed = if lookahead == 0 {
                    sample
                } else {
                    self.delay[channel].read(lookahead)
                };
                self.delay[channel].write(sample);
                let mix = wet[i] * 0.01;
                let level = self.gains[i] * makeup * out[i];
                let output = delayed * (mix * level + (1.0 - mix));
                self.peak_out = self.peak_out.max(output.abs());
                audio.channel_mut(channel)[i] = output;
            }
        }
    }

    fn latency(&self) -> u32 {
        self.lookahead
    }

    fn display_len(&self) -> usize {
        DISPLAY_VALUES
    }

    fn display(&mut self, out: &mut [f32]) {
        let db = |level: f32| level_db(f64::from(level)) as f32;
        out[0] = db(self.gain);
        out[1] = db(self.peak_in);
        out[2] = db(self.peak_out);
        self.peak_in = 0.0;
        self.peak_out = 0.0;
    }
}

/// What the node publishes: the gain now, and the input's and the output's
/// peaks since the last block, all in dB.
const DISPLAY_VALUES: usize = 3;

/// The views the display offers (the compressor spec, R16), the first the
/// one it starts in.
pub const VIEWS: [&str; 3] = ["Transfer Curve", "Activity", "Collapsed"];

/// The transfer curve's input levels, in dB, one point per dB.
const CURVE_FROM_DB: f32 = -60.0;
const CURVE_POINTS: usize = 61;

/// The device's display (the compressor spec, R15): the gain reduction and
/// the input's and output's levels, the transfer curve, and the threshold
/// and the level now as markers on it. `data` is what the node published,
/// if an engine runs.
#[must_use]
pub fn display(
    device: DeviceId,
    description: &DeviceDescription,
    values: &OrdMap<ParamId, f64>,
    data: Option<&[f32]>,
) -> DeviceDisplay {
    let value = |id: ParamId| description.value(values, id).unwrap_or(0.0);
    let expand = (value(MODE) as usize).min(MODE_EXPAND) == MODE_EXPAND;
    let slope = curve_slope(value(RATIO).clamp(1.0, RATIO_MAX), expand);
    let (threshold, knee) = (value(THRESHOLD), value(KNEE));
    let through = |db: f64| db + curve_db(db, threshold, slope, knee, expand);
    let reading = |name: &str, level: f32| DisplayReading {
        name: name.to_owned(),
        value: level,
        unit: "dB".to_owned(),
        text: format!("{level:.1} dB"),
    };
    let mut out = DeviceDisplay {
        device,
        live: data.is_some(),
        view: None,
        series: vec![DisplaySeries {
            name: "Transfer Curve".to_owned(),
            unit: "dB".to_owned(),
            x_unit: "dB".to_owned(),
            x_range: [CURVE_FROM_DB, 0.0],
            log_x: false,
            y_range: [CURVE_FROM_DB, 0.0],
            detail: false,
            values: (0..CURVE_POINTS)
                .map(|i| through(f64::from(CURVE_FROM_DB) + i as f64) as f32)
                .collect(),
        }],
        markers: vec![DisplayMarker {
            name: "Threshold".to_owned(),
            series: "Transfer Curve".to_owned(),
            x: threshold as f32,
            y: through(threshold) as f32,
            text: format!(
                "Threshold {}",
                description
                    .param(THRESHOLD)
                    .map_or_else(String::new, |p| p.format(threshold))
            ),
        }],
        readings: Vec::new(),
    };
    if let Some([gain, input, output]) =
        data.and_then(|d| <[f32; DISPLAY_VALUES]>::try_from(d).ok())
    {
        out.readings = vec![
            reading("Gain Reduction", gain),
            reading("Input", input),
            reading("Output", output),
        ];
        if input > CURVE_FROM_DB {
            out.markers.push(DisplayMarker {
                name: "Level".to_owned(),
                series: "Transfer Curve".to_owned(),
                x: input,
                y: through(f64::from(input)) as f32,
                text: format!("Level {input:.1} dB"),
            });
        }
    }
    out
}

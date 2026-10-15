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
use bl_model::{DeviceDescription, DeviceLayout, DeviceRole, ParamDescriptor, ParamId, ParamUnit};

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

/// The look-ahead times the chooser offers, in seconds (the manual: zero,
/// one and ten ms).
const LOOKAHEAD_SECONDS: [f64; 3] = [0.0, 0.001, 0.010];

/// The Lookahead choice a device without a saved value starts at.
const DEFAULT_LOOKAHEAD: usize = 0;

/// The Mode choices, by their indices.
const MODE_RMS: usize = 1;
const MODE_EXPAND: usize = 2;

/// The Env choices: Lin, then Log.
const ENV_LOG: usize = 1;

/// The SC Filter's Type choices.
const SC_TYPES: [&str; 3] = ["Low Pass", "Band Pass", "High Pass"];

/// The lowest level the static curve reads, in dB: silence is the floor,
/// not minus infinity.
const FLOOR_DB: f64 = -120.0;

/// The deepest reduction the curve asks for, and the highest expansion, in
/// dB. The expansion's cap is Beatlayer's own; Live's limit is to observe.
const MIN_GAIN_DB: f64 = -120.0;
const MAX_EXPAND_DB: f64 = 24.0;

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
        .with_description(DeviceDescription::new(params).with_layout(layout()))
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

/// A second-order filter (an RBJ biquad), for the sidechain's SC Filter.
/// Coefficients are worked out off the audio thread's hot path, once per
/// stretch, from the parameters' values.
struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
}

impl Biquad {
    fn new() -> Self {
        Self {
            b0: 1.0,
            b1: 0.0,
            b2: 0.0,
            a1: 0.0,
            a2: 0.0,
            x1: 0.0,
            x2: 0.0,
            y1: 0.0,
            y2: 0.0,
        }
    }

    /// Sets its coefficients for `kind` at `hz` with resonance `q`, at
    /// `rate`. The cutoff is held just below Nyquist, so any value is safe.
    fn set(&mut self, kind: usize, hz: f64, q: f64, rate: f64) {
        let w = core::f64::consts::TAU * hz.min(rate * 0.49) / rate;
        let (sin, cos) = w.sin_cos();
        let alpha = sin / (2.0 * q.max(0.05));
        let (b0, b1, b2, a0, a1, a2) = match kind {
            // A band pass with a 0 dB peak at the center.
            1 => (alpha, 0.0, -alpha, 1.0 + alpha, -2.0 * cos, 1.0 - alpha),
            // A high pass.
            2 => {
                let k = 0.5 * (1.0 + cos);
                (k, -2.0 * k, k, 1.0 + alpha, -2.0 * cos, 1.0 - alpha)
            }
            // A low pass.
            _ => {
                let k = 0.5 * (1.0 - cos);
                (k, 1.0 - cos, k, 1.0 + alpha, -2.0 * cos, 1.0 - alpha)
            }
        };
        self.b0 = (b0 / a0) as f32;
        self.b1 = (b1 / a0) as f32;
        self.b2 = (b2 / a0) as f32;
        self.a1 = (a1 / a0) as f32;
        self.a2 = (a2 / a0) as f32;
    }

    /// One sample through it.
    fn process(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.b1 * self.x1 + self.b2 * self.x2
            - self.a1 * self.y1
            - self.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }
}

/// The look-ahead in samples for the Look chooser's `choice`.
fn lookahead_frames(choice: f64, rate: f64, max: usize) -> usize {
    let choice = (choice.max(0.0) as usize).min(LOOKAHEAD_SECONDS.len() - 1);
    ((LOOKAHEAD_SECONDS[choice] * rate).ceil() as usize).min(max)
}

/// The per-sample step of a one-pole whose time constant is `ms`
/// milliseconds: after `ms` it has closed the gap by 1 - 1/e.
fn step_of(ms: f64, rate: f64) -> f32 {
    let samples = (ms * 0.001 * rate).max(1.0);
    (1.0 - (-1.0 / samples).exp()) as f32
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
    filter: [Biquad; 2],
    /// Each channel's mean square, for the RMS detector.
    rms: [f32; 2],
    /// The gain now, and how long a reduction has been held (Auto Release).
    gain: f32,
    hold_frames: f32,
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
            filter: [Biquad::new(), Biquad::new()],
            rms: [0.0; 2],
            gain: 1.0,
            hold_frames: 0.0,
        }
    }

    /// The detector's level in dB for `level`, floored at [`FLOOR_DB`].
    fn level_db(level: f64) -> f64 {
        20.0 * level.max(db_to_gain(FLOOR_DB)).log10()
    }

    /// The static curve's gain in dB for a level `db`: compression turns
    /// the gain down above the threshold over the knee, expansion turns it
    /// up. `slope` is the dB the curve moves per dB over the threshold.
    fn curve_db(db: f64, threshold: f64, slope: f64, knee: f64, expand: bool) -> f64 {
        let over = db - threshold;
        let half = 0.5 * knee;
        let shape = if knee > 0.0 && over < half && over > -half {
            (over + half) * (over + half) / (2.0 * knee)
        } else {
            over.max(0.0)
        };
        if expand {
            (slope * shape).min(MAX_EXPAND_DB)
        } else {
            (-slope * shape).max(MIN_GAIN_DB)
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
        // The top of the ratio's range is an infinite ratio: the curve's
        // slope is 1 (expansion's stays finite).
        let slope = if expand {
            ratio - 1.0
        } else if ratio >= RATIO_MAX {
            1.0
        } else {
            1.0 - 1.0 / ratio
        };
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
                let db = Self::level_db(level);
                let gain = db_to_gain(Self::curve_db(db, threshold, slope, knee, expand)) as f32;
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
                audio.channel_mut(channel)[i] = delayed * (mix * level + (1.0 - mix));
            }
        }
    }

    fn latency(&self) -> u32 {
        self.lookahead
    }
}

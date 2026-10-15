//! Brickwall, the native look-ahead limiter (WP-2.18a, part 3; the limiter
//! spec): keeps a signal's peaks at or below a ceiling, with an optional
//! true-peak stage that holds inter-sample peaks too.
//!
//! It is a brickwall limiter with a look-ahead stage. The signal is
//! delayed by the look-ahead time while a sliding minimum of the gain the
//! signal needs is taken over that same window, so the gain has already
//! fallen to what a peak needs by the time the peak comes out. The gain
//! falls to that minimum at once (so no sample reaches the ceiling) and
//! recovers towards it over the Release time, or over a signal-dependent
//! time with Auto. Two channels share their gain reduction by Link, either
//! left/right or mid/side.
//!
//! Its parameters, ranges, defaults and look-ahead choices are Beatlayer's
//! own reading of Live 12.1's Limiter (see `docs/specs/limiter.md`); the
//! manual doesn't record the ranges or defaults, so those are Beatlayer's
//! and are listed to observe.

use bl_dsp::Oversampler;
use bl_engine::{DeviceType, NodeConfig, Registry};
use bl_model::Device;
use bl_model::device_layout::{BODY_HEIGHT, ControlBand, DisplayCell, LayoutNode, Widget};
use bl_model::param::db_to_gain;
use bl_model::{DeviceDescription, DeviceLayout, DeviceRole, ParamDescriptor, ParamId, ParamUnit};

use crate::framework::{Audio, DelayLine, NativeDevice, ParamBank, add};

/// Brickwall's device type.
pub const LIMITER: &str = "beatlayer.brickwall";

/// Ceiling: the highest level the output reaches, in dB.
pub const CEILING: ParamId = ParamId(0);
/// Gain: the input gain, a linear amplitude factor up to +18 dB.
pub const GAIN: ParamId = ParamId(1);
/// Release: how quickly the gain recovers, in milliseconds.
pub const RELEASE: ParamId = ParamId(2);
/// Auto: whether the limiter chooses the release time from the signal.
pub const AUTO: ParamId = ParamId(3);
/// Lookahead: 1.5, 3, or 6 milliseconds.
pub const LOOKAHEAD: ParamId = ParamId(4);
/// Ceiling Mode: Standard, Soft Clip, or True Peak.
pub const CEILING_MODE: ParamId = ParamId(5);
/// Routing Mode: L/R or M/S.
pub const ROUTING: ParamId = ParamId(6);
/// Link: how much of the gain reduction both channels share, per cent.
pub const LINK: ParamId = ParamId(7);

/// The parameters', descriptors', and bank's order.
const CEILING_INDEX: usize = 0;
const GAIN_INDEX: usize = 1;
const RELEASE_INDEX: usize = 2;
const AUTO_INDEX: usize = 3;
const LOOKAHEAD_INDEX: usize = 4;
const MODE_INDEX: usize = 5;
const ROUTING_INDEX: usize = 6;
const LINK_INDEX: usize = 7;

/// The look-ahead times the chooser offers, in seconds.
const LOOKAHEAD_SECONDS: [f64; 3] = [0.0015, 0.003, 0.006];

/// The Lookahead choice a device without a saved value starts at: 3 ms.
const DEFAULT_LOOKAHEAD: usize = 1;

/// The Ceiling Mode choices, by their indices.
const MODE_SOFT_CLIP: usize = 1;
const MODE_TRUE_PEAK: usize = 2;

/// Oversampling for true-peak detection: 4 times the rate.
const TRUE_PEAK_STAGES: u32 = 2;

/// The headroom True Peak mode leaves, so the reconstruction filters'
/// overshoot can't pass the ceiling. About -0.2 dB.
const TRUE_PEAK_HEADROOM: f32 = 0.977;

/// Auto Release: the quickest and slowest release times it moves between,
/// in milliseconds, and how long a sustained reduction takes to reach the
/// slow one, in seconds. Beatlayer's own; Live doesn't record its algorithm.
const AUTO_FAST_MS: f64 = 20.0;
const AUTO_SLOW_MS: f64 = 500.0;
const AUTO_SUSTAIN_SECONDS: f64 = 0.1;

/// The Brickwall device type.
#[must_use]
pub fn limiter_type() -> DeviceType {
    let params = vec![
        ParamDescriptor::continuous(CEILING, "Ceiling", (-18.0, 0.0), -0.3, ParamUnit::Decibels)
            .describe("The highest level the limiter lets through."),
        ParamDescriptor::gain(GAIN, "Gain", 18.0, 0.0)
            .describe("Boosts the signal before the limiter, down to silence."),
        ParamDescriptor::continuous(
            RELEASE,
            "Release",
            (1.0, 1000.0),
            100.0,
            ParamUnit::Milliseconds,
        )
        .logarithmic()
        .describe("How quickly the gain recovers once a peak has passed."),
        ParamDescriptor::toggle(AUTO, "Auto", false)
            .describe("Lets the limiter pick the release time from the signal."),
        ParamDescriptor::choice(LOOKAHEAD, "Lookahead", &["1.5 ms", "3 ms", "6 ms"], 1)
            .describe("How far ahead the limiter sees a peak; it delays the output as long."),
        ParamDescriptor::choice(
            CEILING_MODE,
            "Ceiling Mode",
            &["Standard", "Soft Clip", "True Peak"],
            0,
        )
        .describe(
            "Standard limits the samples; Soft Clip rounds peaks off near the ceiling; \
             True Peak holds inter-sample peaks too.",
        ),
        ParamDescriptor::choice(ROUTING, "Routing Mode", &["L/R", "M/S"], 0)
            .describe("Whether the channels are left/right or mid/side."),
        ParamDescriptor::continuous(LINK, "Link", (0.0, 100.0), 100.0, ParamUnit::Percent)
            .describe("How much of the gain reduction both channels share."),
    ];
    let layout = DeviceLayout::new(LayoutNode::Row(vec![
        LayoutNode::Band(ControlBand::stacked(BODY_HEIGHT, &[(GAIN, Widget::Knob)])),
        LayoutNode::Cell(
            DisplayCell::new("Gain Reduction", 150.0, BODY_HEIGHT, 1.0)
                .with_bottom(ControlBand::stacked(60.0, &[(CEILING, Widget::Knob)])),
        ),
        LayoutNode::Band(ControlBand::stacked(
            BODY_HEIGHT,
            &[
                (RELEASE, Widget::Knob),
                (LINK, Widget::Knob),
                (AUTO, Widget::Toggle),
                (LOOKAHEAD, Widget::Chooser),
                (CEILING_MODE, Widget::Chooser),
                (ROUTING, Widget::Chooser),
            ],
        )),
    ]));
    DeviceType::native(LIMITER, "Brickwall", DeviceRole::AudioEffect)
        .with_description(DeviceDescription::new(params).with_layout(layout))
}

/// Adds the Brickwall limiter to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, limiter_type(), make);
}

/// Makes a Brickwall for `device`'s values and `config`'s rate, channels,
/// and block size. Every buffer it needs is made here, off the audio
/// thread, and its look-ahead comes from the set's value, so it reports
/// its latency before it first runs.
#[must_use]
pub fn make(device: &Device, config: &NodeConfig) -> Brickwall {
    Brickwall::new(device, config)
}

/// The least of the last `window + 1` values pushed, in a monotonic queue
/// of fixed capacity, so each push and read is O(1) amortised and nothing
/// allocates.
struct MinQueue {
    index: Vec<usize>,
    value: Vec<f32>,
    capacity: usize,
    head: usize,
    len: usize,
}

impl MinQueue {
    fn new(capacity: usize) -> Self {
        Self {
            index: vec![0; capacity],
            value: vec![0.0; capacity],
            capacity,
            head: 0,
            len: 0,
        }
    }

    fn push(&mut self, index: usize, value: f32, window: usize) {
        while self.len > 0 {
            let back = (self.head + self.len - 1) % self.capacity;
            if self.value[back] >= value {
                self.len -= 1;
            } else {
                break;
            }
        }
        let slot = (self.head + self.len) % self.capacity;
        self.index[slot] = index;
        self.value[slot] = value;
        self.len += 1;
        while self.len > 0 && self.index[self.head] + window < index {
            self.head = (self.head + 1) % self.capacity;
            self.len -= 1;
        }
    }

    fn least(&self) -> f32 {
        self.value[self.head]
    }
}

/// Brickwall's audio-thread half.
pub struct Brickwall {
    rate: f64,
    /// The most samples of look-ahead any choice asks for.
    max_lookahead: usize,
    /// The look-ahead in force, in samples: the output's delay.
    lookahead: u32,
    /// The post-gain input, in the routing's channels.
    work: [Vec<f32>; 2],
    /// Each channel's peak per frame: the sample peak, and the 4-times
    /// oversampled peak in True Peak mode.
    peaks: [Vec<f32>; 2],
    /// Each channel's gain before the Link is applied.
    targets: [Vec<f32>; 2],
    /// Each channel's limited output.
    out: [Vec<f32>; 2],
    /// A copy of the input, for the detector to run on.
    scratch: [Vec<f32>; 2],
    /// A 4-times oversampler per channel for true-peak detection.
    detectors: [Oversampler; 2],
    delay: [DelayLine; 2],
    queue: [MinQueue; 2],
    /// Each channel's gain now, and the frame count since it started.
    gain: [f32; 2],
    position: [usize; 2],
    /// How long a reduction has been held, which Auto Release reads.
    hold_frames: f32,
}

impl Brickwall {
    fn new(device: &Device, config: &NodeConfig) -> Self {
        let rate = config.sample_rate.as_f64();
        let max_lookahead = (LOOKAHEAD_SECONDS[2] * rate).ceil() as usize + 1;
        let frames = config.max_frames.max(1);
        let buffer = || vec![0.0f32; frames];
        let detector = || Oversampler::new(TRUE_PEAK_STAGES, 1, frames);
        let choice = device
            .params
            .get(&LOOKAHEAD)
            .copied()
            .unwrap_or(DEFAULT_LOOKAHEAD as f64);
        let choice = (choice.max(0.0) as usize).min(LOOKAHEAD_SECONDS.len() - 1);
        let lookahead = ((LOOKAHEAD_SECONDS[choice] * rate).ceil() as usize).min(max_lookahead);
        Self {
            rate,
            max_lookahead,
            lookahead: lookahead as u32,
            work: [buffer(), buffer()],
            peaks: [buffer(), buffer()],
            targets: [buffer(), buffer()],
            out: [buffer(), buffer()],
            scratch: [buffer(), buffer()],
            detectors: [detector(), detector()],
            delay: [DelayLine::new(max_lookahead), DelayLine::new(max_lookahead)],
            queue: [
                MinQueue::new(max_lookahead + 2),
                MinQueue::new(max_lookahead + 2),
            ],
            gain: [1.0; 2],
            position: [0; 2],
            hold_frames: 0.0,
        }
    }

    /// The look-ahead in samples for the chooser's `choice`.
    fn lookahead_frames(&self, choice: usize) -> usize {
        let seconds = LOOKAHEAD_SECONDS[choice.min(LOOKAHEAD_SECONDS.len() - 1)];
        ((seconds * self.rate).ceil() as usize).min(self.max_lookahead)
    }

    /// The release's one-pole step: the fraction of the gap it closes each
    /// sample. With Auto on it is the fast time just after a peak and the
    /// slow one once a reduction has been held.
    fn release_step(&self, release_ms: f64, auto: bool) -> f32 {
        let ms = if auto {
            let sustained =
                (f64::from(self.hold_frames) / (AUTO_SUSTAIN_SECONDS * self.rate)).clamp(0.0, 1.0);
            AUTO_FAST_MS + (AUTO_SLOW_MS - AUTO_FAST_MS) * sustained
        } else {
            release_ms
        };
        let samples = (ms * 0.001 * self.rate).max(1.0);
        (1.0 - (-1.0 / samples).exp()) as f32
    }
}

/// Rounds a sample's peaks off near `ceiling`: the identity up to half of
/// it, and a `tanh` curve above, which reaches `ceiling` as a limit and
/// never passes it.
fn soft_clip(sample: f32, ceiling: f32) -> f32 {
    let knee = ceiling * 0.5;
    let magnitude = sample.abs();
    if magnitude <= knee {
        return sample;
    }
    let over = (magnitude - knee) / (ceiling - knee);
    sample.signum() * (knee + (ceiling - knee) * over.tanh())
}

impl NativeDevice for Brickwall {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        let frames = audio.frames();
        let channels = audio.channels().min(2);
        if frames == 0 || channels == 0 {
            return;
        }

        let ceiling = db_to_gain(params.value(CEILING_INDEX)).max(1e-6) as f32;
        let mode = (params.value(MODE_INDEX) as usize).min(MODE_TRUE_PEAK);
        // True Peak leaves a little headroom for the reconstruction filters.
        let limit = if mode == MODE_TRUE_PEAK {
            ceiling * TRUE_PEAK_HEADROOM
        } else {
            ceiling
        };
        let routing = params.value(ROUTING_INDEX) as usize;
        let link = (params.value(LINK_INDEX) / 100.0).clamp(0.0, 1.0) as f32;
        let auto = params.value(AUTO_INDEX) >= 0.5;
        let lookahead = self.lookahead_frames(params.value(LOOKAHEAD_INDEX) as usize);
        self.lookahead = lookahead as u32;

        // 1. The input gain, into the working buffers.
        let gain = params.ramp(GAIN_INDEX);
        for channel in 0..channels {
            let src = audio.channel_mut(channel);
            let work = &mut self.work[channel][..frames];
            for ((sample, out), g) in src.iter().zip(work.iter_mut()).zip(gain) {
                *out = *sample * *g;
            }
        }

        // 2. Mid/side, if the routing asks for it.
        let mid_side = routing == 1 && channels == 2;
        if mid_side {
            let (first, second) = self.work.split_at_mut(1);
            let left = &mut first[0][..frames];
            let right = &mut second[0][..frames];
            for (l, r) in left.iter_mut().zip(right.iter_mut()) {
                let (a, b) = (*l, *r);
                *l = (a + b) * 0.5;
                *r = (a - b) * 0.5;
            }
        }

        // 3. Each channel's peak and the gain it needs.
        for channel in 0..channels {
            if mode == MODE_TRUE_PEAK {
                self.scratch[channel][..frames].copy_from_slice(&self.work[channel][..frames]);
                self.peaks[channel][..frames].fill(0.0);
                let factor = self.detectors[channel].factor();
                let peaks = &mut self.peaks[channel];
                let scratch = &mut self.scratch[channel][..frames];
                self.detectors[channel].process(0, scratch, |high| {
                    for (i, sample) in high.iter().enumerate() {
                        let magnitude = sample.abs();
                        let at = i / factor;
                        if magnitude > peaks[at] {
                            peaks[at] = magnitude;
                        }
                    }
                });
            } else {
                let work = &self.work[channel];
                let peaks = &mut self.peaks[channel][..frames];
                for (peak, sample) in peaks.iter_mut().zip(work) {
                    *peak = sample.abs();
                }
            }
            let work = &self.work[channel];
            let peaks = &self.peaks[channel];
            let targets = &mut self.targets[channel][..frames];
            for ((target, sample), oversampled) in targets.iter_mut().zip(work).zip(peaks) {
                let peak = sample.abs().max(*oversampled);
                *target = if peak > 0.0 {
                    (limit / peak).min(1.0)
                } else {
                    1.0
                };
            }
        }

        // 4. The Link: share the reduction across the channels.
        if channels == 2 {
            let (first, second) = self.targets.split_at_mut(1);
            let a = &mut first[0][..frames];
            let b = &mut second[0][..frames];
            for (x, y) in a.iter_mut().zip(b.iter_mut()) {
                let shared = x.min(*y);
                *x += (shared - *x) * link;
                *y += (shared - *y) * link;
            }
        }

        // 5. Look ahead, delay, and release, channel by channel.
        let mut reduction = 0.0f32;
        for channel in 0..channels {
            for target in &self.targets[channel][..frames] {
                reduction = reduction.max(1.0 - target);
            }
        }
        if reduction > 1e-4 {
            self.hold_frames += frames as f32;
        } else {
            self.hold_frames = 0.0;
        }
        let release = self.release_step(params.value(RELEASE_INDEX), auto);
        for channel in 0..channels {
            let work = &self.work[channel][..frames];
            let targets = &self.targets[channel][..frames];
            let out = &mut self.out[channel][..frames];
            let queue = &mut self.queue[channel];
            let delay = &mut self.delay[channel];
            let mut gain = self.gain[channel];
            let mut position = self.position[channel];
            for ((i, sample), target) in work.iter().enumerate().zip(targets) {
                queue.push(position, *target, lookahead);
                let least = queue.least();
                if least < gain {
                    gain = least;
                } else {
                    gain += (least - gain) * release;
                }
                let delayed = delay.read(lookahead);
                delay.write(*sample);
                out[i] = delayed * gain;
                position += 1;
            }
            self.gain[channel] = gain;
            self.position[channel] = position;
        }

        // 6. Back from mid/side, and out.
        if mid_side {
            let (first, second) = self.out.split_at_mut(1);
            let mid = &first[0][..frames];
            let side = &second[0][..frames];
            let mut channels = audio.channels_mut();
            let left = channels.next().expect("two channels");
            let right = channels.next().expect("two channels");
            for ((l, r), (m, s)) in left
                .iter_mut()
                .zip(right.iter_mut())
                .zip(mid.iter().zip(side))
            {
                *l = m + s;
                *r = m - s;
            }
        } else {
            for channel in 0..channels {
                audio.channel_mut(channel)[..frames].copy_from_slice(&self.out[channel][..frames]);
            }
        }

        // 7. Soft clipping at the ceiling, if asked for.
        if mode == MODE_SOFT_CLIP {
            for channel in 0..channels {
                for sample in audio.channel_mut(channel) {
                    *sample = soft_clip(*sample, ceiling);
                }
            }
        }
    }

    fn latency(&self) -> u32 {
        self.lookahead
    }
}

//! The mixer: each track's pan, volume, and activator; the routes that carry
//! tracks into their groups, Main, and the returns; solo; and the meters
//! (WP-1.6). The rules, and the Live sources behind them, are in
//! the mixer spec.
//!
//! A track's signal runs through its task in the program (see
//! [`EngineProgram`](crate::EngineProgram)):
//!
//! 1. its inputs, summed: the outputs of the tracks in a group, the top-level
//!    tracks and returns for Main, or the sends to a return. Each input is a
//!    [`Route`], a gain that solo and the send's level set;
//! 2. its device chain;
//! 3. its [`TrackMixer`]: pan, volume, the Track Activator, and its
//!    crossfade assignment, then the track's meters.
//!
//! A send taps its track's signal after step 3 (post-fader, the default) or
//! after step 2 (pre-fader), as the return's Pre/Post switch says. A track's
//! output to its group or Main is its signal after step 3, through its
//! output's route, which solo closes.
//!
//! Every gain glides to a new value along a straight line over
//! [`SMOOTHING_SECONDS`] from the frame the change takes effect, so no move
//! steps: no zipper noise and no clicks. At unity, centred, and unsoloed, the
//! mixer passes audio through to the bit.

use std::collections::BTreeSet;
use std::f64::consts::{FRAC_PI_2, FRAC_PI_4, SQRT_2};

use bl_core::time::SampleRate;
use bl_model::modulation::Travel;
use bl_model::{CrossfadeAssign, Document, Mixer, PanMode, ParamId, Tap, TrackId, TrackKind};

use crate::automation::{AutomationList, SessionNow, Song};
use crate::modbus::Bus;
use crate::node::{Block, Node, NodeConfig, NodeData, NodeEvent, ParamValue, segments};
use crate::routing::{Edge, Graph, Via};

/// How long a mixer gain takes to reach a new value: 20 ms, along a straight
/// line from the gain it had (the mixer spec, R18).
pub const SMOOTHING_SECONDS: f64 = 0.02;

/// How fast a peak meter falls after a peak, in decibels per second: 20 dB
/// in 1.7 seconds, as a broadcast peak programme meter falls back
/// (the mixer spec, R19).
pub const METER_RELEASE_DB_PER_SECOND: f64 = 20.0 / 1.7;

/// The time constant of the RMS meters' average of the squared signal: 300
/// ms (the mixer spec, R19).
pub const RMS_SECONDS: f64 = 0.3;

/// A track mixer's parameters ([`NodeKey::Mixer`](crate::NodeKey::Mixer)),
/// which take the track's mixer settings.
pub mod track {
    use bl_model::ParamId;

    /// Linear gain: 1 is unity (0 dB) and 0 is silence (−∞ dB).
    pub const VOLUME: ParamId = ParamId(0);
    /// Stereo position from −1 (left) to 1 (right).
    pub const PAN: ParamId = ParamId(1);
    /// The Track Activator: muted (deactivated) when not 0.
    pub const MUTE: ParamId = ParamId(2);
    /// The crossfade assignment: 0 neither, 1 A, 2 B
    /// ([`crossfade_value`](super::crossfade_value) builds it).
    pub const CROSSFADE: ParamId = ParamId(3);
    /// The crossfader's position, from 0 (the A end) to 1 (the B end).
    pub const CROSSFADER: ParamId = ParamId(4);
    /// In Split Stereo Pan Mode, the left input channel's position, from −1
    /// (left) to 1 (right).
    pub const SPLIT_LEFT: ParamId = ParamId(5);
    /// In Split Stereo Pan Mode, the right input channel's position.
    pub const SPLIT_RIGHT: ParamId = ParamId(6);
    /// The pan mode: 0 Stereo Pan Mode (the default), 1 Split Stereo Pan
    /// Mode.
    pub const PAN_MODE: ParamId = ParamId(7);
}

/// A route's parameters: a track's output into its group or Main
/// ([`NodeKey::Output`](crate::NodeKey::Output)), or a send
/// ([`NodeKey::Send`](crate::NodeKey::Send)).
pub mod route {
    use bl_model::ParamId;

    /// A send's level, as linear gain from 0 to 1. An output's is always 1.
    pub const LEVEL: ParamId = ParamId(0);
    /// Silenced by solo when not 0: another track is soloed and this route
    /// isn't part of what's heard (see [`Solo`](super::Solo)).
    pub const SOLO_MUTE: ParamId = ParamId(1);
    /// Closed when not 0: the route feeds a track's input, and the track
    /// isn't monitoring it (the routing spec).
    pub const CLOSED: ParamId = ParamId(2);
    /// Whether the track whose input this route feeds monitors with Auto
    /// (the routing spec, R11): while its clips play, the route closes,
    /// which the audio thread sets each block.
    pub const AUTO: ParamId = ParamId(3);
}

/// The gains of Live's constant-power pan law for `pan` from −1 (left) to 1
/// (right): `[left, right]`. Both are 1 (0 dB) at the centre; fully to one
/// side, that side is √2 (+3 dB) and the other 0, and the power, the sum of
/// the squares, is 2 everywhere. In Stereo Pan Mode the two gains scale the
/// left and right channels, so pan balances a stereo signal and places a
/// mono one.
///
/// The gains are `√2·sin((1 − pan)·π/4)` and `√2·sin((1 + pan)·π/4)`, through
/// `libm`'s pure-Rust sine, so they're the same to the bit on every
/// platform; mirrored pans give mirrored gains exactly, and the centre gives
/// exactly 1, so a centred track passes audio unchanged. A pan outside −1 to
/// 1 is taken as the nearest end, and one that isn't a number as the centre.
#[must_use]
pub fn pan_gains(pan: f64) -> [f64; 2] {
    let pan = if pan.is_nan() {
        0.0
    } else {
        pan.clamp(-1.0, 1.0)
    };
    if pan == 0.0 {
        return [1.0, 1.0];
    }
    [
        SQRT_2 * libm::sin((1.0 - pan) * FRAC_PI_4),
        SQRT_2 * libm::sin((1.0 + pan) * FRAC_PI_4),
    ]
}

/// The gains of Split Stereo Pan Mode for a left control at `left` and a
/// right control at `right`, each from −1 (left) to 1 (right): a 2×2 matrix
/// `[[a, b], [c, d]]` where the left output is `left_in·a + right_in·b` and
/// the right output `left_in·c + right_in·d`. Each input channel is placed on
/// the constant-power law ([`pan_gains`]) with its own control: the left
/// channel at `left`, the right at `right`. Both centred, each channel is
/// centred and the pair sums to mono; a control fully to one side puts its
/// channel on that side at the law's gain (√2). Beatlayer's own reading of
/// manual 18.1, which leaves the mono case and the controls' travel open.
#[must_use]
pub fn split_pan_gains(left: f64, right: f64) -> [[f64; 2]; 2] {
    let [ll, lr] = pan_gains(left);
    let [rl, rr] = pan_gains(right);
    [[ll, rl], [lr, rr]]
}

/// A level in decibels as linear gain: 0 dB is 1, −6.02 dB about 0.5, and
/// −∞ dB 0.
#[must_use]
pub fn gain_from_db(db: f64) -> f64 {
    if db == f64::NEG_INFINITY {
        0.0
    } else {
        libm::pow(10.0, db / 20.0)
    }
}

/// A linear gain in decibels: 1 is 0 dB and 0 is −∞ dB.
#[must_use]
pub fn db_from_gain(gain: f64) -> f64 {
    20.0 * libm::log10(gain)
}

/// Where the Constant Power curve's cosine ends and its measured tail
/// begins (the mixer spec, R23).
const CROSSFADE_JOIN: f64 = 0.75;

/// Where the crossfade curve reaches silence: at and past it, a track on
/// the far side is silent (measured at 48 of 50) (the mixer spec, R23).
const CROSSFADE_SILENCE: f64 = 0.96;

/// The measured points of the Constant Power curve's tail, as (position,
/// decibels); between them the curve is a straight line in decibels (the
/// mixer spec, R23). The point at 0.78 is the mirror of the measurement
/// with the fader at 28A on a track assigned B.
const CROSSFADE_TAIL_DB: [(f64, f64); 4] =
    [(0.78, -9.9), (0.83, -14.9), (0.92, -37.0), (0.95, -52.7)];

/// The default crossfade curve, Constant Power, fitted to the measurements
/// in the mixer spec (R23): `cos(π/2·p)` up to 0.75, then through the
/// measured points, a straight line in decibels between them, and a
/// straight line in amplitude to silence at 0.96, where it stays. `position`
/// runs from 0 (the A end) to 1 (the B end); one outside that is taken as
/// the nearest end, and one that isn't a number as the centre.
#[must_use]
pub fn constant_power(position: f64) -> f64 {
    let position = if position.is_nan() {
        0.5
    } else {
        position.clamp(0.0, 1.0)
    };
    if position <= CROSSFADE_JOIN {
        return libm::cos(FRAC_PI_2 * position);
    }
    if position >= CROSSFADE_SILENCE {
        return 0.0;
    }
    let join = (
        CROSSFADE_JOIN,
        db_from_gain(libm::cos(FRAC_PI_2 * CROSSFADE_JOIN)),
    );
    let points = [
        join,
        CROSSFADE_TAIL_DB[0],
        CROSSFADE_TAIL_DB[1],
        CROSSFADE_TAIL_DB[2],
        CROSSFADE_TAIL_DB[3],
    ];
    for pair in points.windows(2) {
        let (from, to) = (pair[0], pair[1]);
        if position <= to.0 {
            let t = (position - from.0) / (to.0 - from.0);
            return gain_from_db(from.1 + (to.1 - from.1) * t);
        }
    }
    let last = points[points.len() - 1];
    let fade = (CROSSFADE_SILENCE - position) / (CROSSFADE_SILENCE - last.0);
    gain_from_db(last.1) * fade
}

/// The crossfade's gain for a track assigned `assign` with the crossfader
/// at `position` (0 at the A end, 1 at the B end): a track on A takes the
/// curve's own value, a track on B its mirror, and one on neither 1, so it
/// plays through at every position (the mixer spec, R23).
#[must_use]
pub fn crossfade_gain(position: f64, assign: CrossfadeAssign) -> f64 {
    match assign {
        CrossfadeAssign::None => 1.0,
        CrossfadeAssign::A => constant_power(position),
        CrossfadeAssign::B => constant_power(1.0 - position),
    }
}

/// The [`track::CROSSFADE`] parameter value that names `assign`: 0 neither,
/// 1 A, 2 B.
#[must_use]
pub fn crossfade_value(assign: CrossfadeAssign) -> f64 {
    match assign {
        CrossfadeAssign::None => 0.0,
        CrossfadeAssign::A => 1.0,
        CrossfadeAssign::B => 2.0,
    }
}

/// Frames a gain takes to reach a new value at `rate`.
pub(crate) fn smoothing_frames(rate: SampleRate) -> u32 {
    (SMOOTHING_SECONDS * rate.as_f64()).round().max(1.0) as u32
}

/// A gain that moves to each new value along a straight line, a frame at a
/// time, so it never steps.
///
/// Its arithmetic runs frame by frame in the same order whatever the block
/// size, so a render is the same to the bit at any size; and it lands on
/// its target exactly.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Ramp {
    value: f64,
    target: f64,
    step: f64,
    /// Frames until it reaches `target`.
    left: u32,
}

impl Ramp {
    /// A gain resting at `value`.
    pub(crate) fn at(value: f64) -> Self {
        Self {
            value,
            target: value,
            step: 0.0,
            left: 0,
        }
    }

    /// Starts moving to `target` from where the gain is now, to reach it in
    /// `frames` frames. A gain already heading for `target` carries on; one
    /// that automation moved off it starts moving again.
    pub(crate) fn toward(&mut self, target: f64, frames: u32) {
        if target == self.target && (self.left > 0 || self.value == self.target) {
            return;
        }
        self.target = target;
        if frames == 0 || target == self.value {
            self.value = target;
            self.left = 0;
        } else {
            self.step = (target - self.value) / f64::from(frames);
            self.left = frames;
        }
    }

    /// The gain now.
    pub(crate) fn value(&self) -> f64 {
        self.value
    }

    /// Whether the gain has reached its target.
    pub(crate) fn is_settled(&self) -> bool {
        self.left == 0
    }

    /// The gain for the next frame.
    #[inline]
    pub(crate) fn next(&mut self) -> f64 {
        if self.left > 0 {
            self.left -= 1;
            self.value = if self.left == 0 {
                self.target
            } else {
                self.value + self.step
            };
        }
        self.value
    }

    /// Moves the gain on by `frames` frames.
    fn skip(&mut self, frames: usize) {
        for _ in 0..self.moving(frames) {
            self.next();
        }
    }

    /// How many of `frames` frames the gain moves through.
    fn moving(&self, frames: usize) -> usize {
        (self.left as usize).min(frames)
    }
}

/// Scales `samples` in place by the gain, frame by frame.
pub(crate) fn scale(samples: &mut [f32], ramp: &mut Ramp) {
    let (moving, still) = samples.split_at_mut(ramp.moving(samples.len()));
    for sample in moving {
        *sample *= ramp.next() as f32;
    }
    let gain = ramp.value() as f32;
    if gain != 1.0 {
        for sample in still {
            *sample *= gain;
        }
    }
}

/// Adds `from` into `to`, scaled by the gain, frame by frame. A gain resting
/// at 0 adds nothing, and one resting at 1 adds the samples as they are.
fn add_scaled(to: &mut [f32], from: &[f32], ramp: &mut Ramp) {
    let moving = ramp.moving(to.len());
    let (to_moving, to_still) = to.split_at_mut(moving);
    let (from_moving, from_still) = from.split_at(moving);
    for (sum, sample) in to_moving.iter_mut().zip(from_moving) {
        *sum += *sample * ramp.next() as f32;
    }
    let gain = ramp.value() as f32;
    if gain == 1.0 {
        for (sum, sample) in to_still.iter_mut().zip(from_still) {
            *sum += *sample;
        }
    } else if gain != 0.0 {
        for (sum, sample) in to_still.iter_mut().zip(from_still) {
            *sum += *sample * gain;
        }
    }
}

/// A track's meters as the audio thread last left them: per channel, the
/// first two (left and right), as linear amplitude.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MeterReading {
    /// The peak level: a peak shows at once and falls at
    /// [`METER_RELEASE_DB_PER_SECOND`].
    pub peak: [f32; 2],
    /// The RMS level: the square root of an average of the squared signal
    /// whose time constant is [`RMS_SECONDS`]. A steady sine reads its
    /// amplitude over √2 (3 dB below its peak).
    pub rms: [f32; 2],
}

/// Peak and RMS meters for two channels, sample by sample.
#[derive(Clone, Copy, Debug)]
struct Meters {
    peak: [f64; 2],
    square: [f64; 2],
    /// What the peak is multiplied by each frame.
    fall: f64,
    /// How far the average of the square moves towards each new square.
    smooth: f64,
    /// Whether only channel 0 is metered (a mono engine), so both readings
    /// show it.
    mono: bool,
}

impl Meters {
    fn new(config: &NodeConfig) -> Self {
        let rate = config.sample_rate.as_f64();
        Self {
            peak: [0.0; 2],
            square: [0.0; 2],
            fall: libm::pow(10.0, -METER_RELEASE_DB_PER_SECOND / 20.0 / rate),
            smooth: 1.0 - libm::exp(-1.0 / (RMS_SECONDS * rate)),
            mono: config.channels == 1,
        }
    }

    fn measure_one(&mut self, channel: usize, sample: f32) {
        let sample = f64::from(sample);
        self.peak[channel] = (self.peak[channel] * self.fall).max(sample.abs());
        self.square[channel] += (sample * sample - self.square[channel]) * self.smooth;
    }

    /// Drops every reading to silence at once.
    fn silence(&mut self) {
        self.peak = [0.0; 2];
        self.square = [0.0; 2];
    }

    fn reading(&self) -> MeterReading {
        let peak = |c: usize| self.peak[c] as f32;
        let rms = |c: usize| self.square[c].max(0.0).sqrt() as f32;
        if self.mono {
            MeterReading {
                peak: [peak(0); 2],
                rms: [rms(0); 2],
            }
        } else {
            MeterReading {
                peak: [peak(0), peak(1)],
                rms: [rms(0), rms(1)],
            }
        }
    }
}

/// A track's mixer: pan, volume, the Track Activator, and the crossfade
/// assignment, applied in place after the track's device chain, then the
/// track's meters. It takes the [`track`] parameters.
///
/// The left channel takes the pan law's left gain ([`pan_gains`]) and the
/// right channel its right gain; in an engine with other than two channels,
/// channels past the first two (or a mono engine's one) take the volume
/// alone. Each channel's gain glides to every new value
/// ([`SMOOTHING_SECONDS`]), and the crossfade's position glides the same
/// way, the curve ([`crossfade_gain`]) following it frame by frame. The
/// meters measure the result: after pan, volume, the activator, and the
/// crossfade, before solo.
#[derive(Debug, Clone)]
pub struct TrackMixer {
    volume: f64,
    pan: f64,
    mute: bool,
    crossfade: CrossfadeAssign,
    /// Whether the track is in Split Stereo Pan Mode (the mixer spec,
    /// manual 18.1).
    split: bool,
    /// In split mode, the left input channel's position, from −1 to 1.
    split_left: f64,
    /// In split mode, the right input channel's position.
    split_right: f64,
    /// The crossfader's position the node heads for: 0 (the A end) to 1
    /// (the B end).
    position: f64,
    /// The position now, gliding like the other gains.
    crossfader: Ramp,
    /// The left gain, the right gain, and the gain of any other channel.
    gains: [Ramp; 3],
    /// In split mode, the two controls' gliding values.
    split_ramps: [Ramp; 2],
    /// In split mode, the gliding level (volume, silenced by the activator).
    split_level: Ramp,
    /// The gliding volume, pan and activator (1 for on), which a control the
    /// envelopes don't drive plays through while others are automated, so a
    /// move of it glides there as everywhere (the mixer spec, R18).
    controls: [Ramp; 3],
    smoothing: u32,
    meters: Meters,
    /// The envelopes driving its parameters while the song plays (WP-2.12).
    automation: Option<NodeData>,
    /// The base and the played value, at the last block's last frame, of the
    /// volume, pan, the activator, and the split controls, while a clip's
    /// modulation acts on them.
    modulated: [Option<(f64, f64)>; 5],
}

impl TrackMixer {
    /// A mixer set to a track's mixer settings and to the Main track's
    /// crossfader at `crossfader`, resting at them.
    #[must_use]
    pub fn new(mixer: &Mixer, crossfader: f64, config: &NodeConfig) -> Self {
        let mut node = Self {
            volume: 1.0,
            pan: 0.0,
            mute: false,
            crossfade: CrossfadeAssign::None,
            split: false,
            split_left: 0.0,
            split_right: 0.0,
            position: 0.5,
            crossfader: Ramp::at(0.5),
            gains: [Ramp::at(1.0); 3],
            split_ramps: [Ramp::at(0.0); 2],
            split_level: Ramp::at(1.0),
            controls: [Ramp::at(1.0), Ramp::at(0.0), Ramp::at(1.0)],
            smoothing: smoothing_frames(config.sample_rate),
            meters: Meters::new(config),
            automation: None,
            modulated: [None; 5],
        };
        node.set(track::VOLUME, ParamValue::Set(mixer.volume));
        node.set(track::PAN, ParamValue::Set(mixer.pan));
        node.set(
            track::MUTE,
            ParamValue::Set(f64::from(u8::from(mixer.mute))),
        );
        node.set(track::SPLIT_LEFT, ParamValue::Set(mixer.split_left));
        node.set(track::SPLIT_RIGHT, ParamValue::Set(mixer.split_right));
        node.set(
            track::PAN_MODE,
            ParamValue::Set(if mixer.pan_mode == PanMode::Split {
                1.0
            } else {
                0.0
            }),
        );
        node.set(
            track::CROSSFADE,
            ParamValue::Set(crossfade_value(mixer.crossfade)),
        );
        node.set(track::CROSSFADER, ParamValue::Set(crossfader));
        node.gains = node.targets().map(Ramp::at);
        node.split_ramps = [Ramp::at(node.split_left), Ramp::at(node.split_right)];
        node.split_level = Ramp::at(node.level());
        node.controls = node.control_targets().map(Ramp::at);
        node.crossfader = Ramp::at(node.position);
        node
    }

    /// The volume, pan and activator (1 for on) the controls glide to.
    fn control_targets(&self) -> [f64; 3] {
        [self.volume, self.pan, f64::from(u8::from(!self.mute))]
    }

    /// The gains the mixer is heading for: left, right, and other channels.
    /// The crossfade is applied frame by frame, from its gliding position.
    fn targets(&self) -> [f64; 3] {
        let level = self.level();
        let [left, right] = pan_gains(self.pan);
        [level * left, level * right, level]
    }

    /// The level the track plays at: the volume, or silence while its
    /// activator is off.
    fn level(&self) -> f64 {
        if self.mute { 0.0 } else { self.volume }
    }

    /// Whether the mixer is muted: a track's Track Activator is off, or a
    /// rack chain's is off or another chain of its rack is soloed.
    #[must_use]
    pub(crate) fn muted(&self) -> bool {
        self.mute
    }

    /// The left and right gains now.
    #[must_use]
    pub fn gains(&self) -> [f64; 2] {
        [self.gains[0].value(), self.gains[1].value()]
    }

    /// Drops the track's meters to silence at once, for a processor that
    /// has stopped making sound. Real-time safe.
    pub(crate) fn silence_meters(&mut self) {
        self.meters.silence();
    }

    /// The track's meters.
    #[must_use]
    pub fn meters(&self) -> MeterReading {
        self.meters.reading()
    }

    fn set(&mut self, param: ParamId, value: ParamValue) {
        let defaults = Mixer::default();
        match (param, value) {
            (track::VOLUME, ParamValue::Set(volume)) if volume.is_finite() => {
                self.volume = volume.max(0.0);
            }
            (track::VOLUME, _) => self.volume = defaults.volume,
            (track::PAN, ParamValue::Set(pan)) if pan.is_finite() => {
                self.pan = pan.clamp(-1.0, 1.0);
            }
            (track::PAN, _) => self.pan = defaults.pan,
            (track::SPLIT_LEFT, ParamValue::Set(pan)) if pan.is_finite() => {
                self.split_left = pan.clamp(-1.0, 1.0);
            }
            (track::SPLIT_LEFT, _) => self.split_left = defaults.split_left,
            (track::SPLIT_RIGHT, ParamValue::Set(pan)) if pan.is_finite() => {
                self.split_right = pan.clamp(-1.0, 1.0);
            }
            (track::SPLIT_RIGHT, _) => self.split_right = defaults.split_right,
            (track::PAN_MODE, ParamValue::Set(mode)) => self.split = mode != 0.0,
            (track::PAN_MODE, _) => self.split = defaults.pan_mode == PanMode::Split,
            (track::MUTE, ParamValue::Set(mute)) => self.mute = mute != 0.0,
            (track::MUTE, ParamValue::Default) => self.mute = defaults.mute,
            (track::CROSSFADE, ParamValue::Set(assign)) => {
                self.crossfade = match assign {
                    1.0 => CrossfadeAssign::A,
                    2.0 => CrossfadeAssign::B,
                    _ => CrossfadeAssign::None,
                };
            }
            (track::CROSSFADE, ParamValue::Default) => self.crossfade = defaults.crossfade,
            (track::CROSSFADER, ParamValue::Set(position)) if position.is_finite() => {
                self.position = position.clamp(0.0, 1.0);
            }
            (track::CROSSFADER, _) => self.position = 0.5,
            _ => {}
        }
    }

    fn apply(&mut self, events: &[NodeEvent]) {
        if events.is_empty() {
            return;
        }
        for event in events {
            self.set(event.param, event.value);
        }
        let (targets, smoothing) = (self.targets(), self.smoothing);
        for (ramp, target) in self.gains.iter_mut().zip(targets) {
            ramp.toward(target, smoothing);
        }
        for (ramp, target) in self
            .split_ramps
            .iter_mut()
            .zip([self.split_left, self.split_right])
        {
            ramp.toward(target, smoothing);
        }
        self.split_level.toward(self.level(), smoothing);
        let controls = self.control_targets();
        for (ramp, target) in self.controls.iter_mut().zip(controls) {
            ramp.toward(target, smoothing);
        }
        self.crossfader.toward(self.position, smoothing);
    }
}

/// Which of a track mixer's gains channel `c` of `channels` takes.
fn gain_index(c: usize, channels: usize) -> usize {
    if channels >= 2 && c < 2 { c } else { 2 }
}

impl Node for TrackMixer {
    fn process(&mut self, block: &mut Block<'_>) {
        let channels = block.channels();
        let transport = block.transport();
        let bus = block.modulation();
        // Spec: automation R2. While the song plays, an automated parameter
        // follows its envelope exactly; a change the song's playhead doesn't
        // drive (a control move) glides as the mixer spec says. A control a
        // modulator maps follows it whether the song plays or not
        // (modulation R20).
        let automation = self.automation.clone();
        let automation = automation
            .as_ref()
            .and_then(|d| d.downcast_ref::<AutomationList>())
            .filter(|list| !list.is_empty());
        let song = transport.playing.then(|| transport.position.get());
        let driven = automation.filter(|list| song.is_some() || list.has_mapped());
        if driven.is_none() {
            self.modulated = [None; 5];
        }
        // A control whose envelopes are gone heads back to what it asks for
        // from where the automation left it.
        if automation.is_none() {
            let (targets, smoothing) = (self.targets(), self.smoothing);
            for (ramp, target) in self.gains.iter_mut().zip(targets) {
                ramp.toward(target, smoothing);
            }
            for (ramp, target) in self
                .split_ramps
                .iter_mut()
                .zip([self.split_left, self.split_right])
            {
                ramp.toward(target, smoothing);
            }
            self.split_level.toward(self.level(), smoothing);
        }
        for segment in segments(block.events(), block.frames()) {
            self.apply(segment.events);
            match driven {
                Some(list) => {
                    let now = block.session_now(list);
                    self.scale_envelopes(block, segment.range.clone(), song, list, &now, bus);
                }
                None => {
                    let (mut after, mut crossfader) = (self.gains, self.crossfader);
                    let (mut split, mut split_level) = (self.split_ramps, self.split_level);
                    for frame in segment.range.clone() {
                        let fade = crossfade_gain(crossfader.next(), self.crossfade) as f32;
                        if self.split {
                            let [sl, sr] = [split[0].next(), split[1].next()];
                            let level = split_level.next();
                            let [[a, b], [c, d]] = split_pan_gains(sl, sr);
                            if let Some((left, right)) = block.channels_pair_mut() {
                                let l = f64::from(left[frame]);
                                let r = f64::from(right[frame]);
                                let fade = f64::from(fade);
                                left[frame] = ((l * a + r * b) * level * fade) as f32;
                                right[frame] = ((l * c + r * d) * level * fade) as f32;
                                self.meters.measure_one(0, left[frame]);
                                self.meters.measure_one(1, right[frame]);
                            }
                            for c in 2..channels {
                                let sample = &mut block.channel_mut(c)[frame];
                                *sample = (f64::from(*sample) * level * f64::from(fade)) as f32;
                            }
                            if channels < 2 {
                                for c in 0..channels {
                                    let sample = &mut block.channel_mut(c)[frame];
                                    *sample = (f64::from(*sample) * level * f64::from(fade)) as f32;
                                }
                            }
                        } else {
                            for c in 0..channels {
                                let index = gain_index(c, channels);
                                let mut ramp = after[index];
                                let sample = &mut block.channel_mut(c)[frame];
                                *sample *= ramp.next() as f32 * fade;
                                after[index] = ramp;
                                if c < 2 {
                                    self.meters.measure_one(c, *sample);
                                }
                            }
                        }
                    }
                    self.gains = after;
                    self.split_ramps = split;
                    self.split_level = split_level;
                    self.crossfader = crossfader;
                    for ramp in &mut self.controls {
                        ramp.skip(segment.range.len());
                    }
                }
            }
        }
    }

    fn flush(&mut self, events: &[NodeEvent]) {
        self.apply(events);
    }

    fn receive_data(&mut self, data: NodeData) {
        if data.downcast_ref::<AutomationList>().is_some() {
            self.automation = Some(data);
        }
    }

    fn modulated(&self, report: &mut dyn FnMut(ParamId, f64, f64)) {
        for (param, entry) in [
            track::VOLUME,
            track::PAN,
            track::MUTE,
            track::SPLIT_LEFT,
            track::SPLIT_RIGHT,
        ]
        .into_iter()
        .zip(self.modulated)
        {
            if let Some((base, played)) = entry {
                report(param, base, played);
            }
        }
    }
}

impl TrackMixer {
    /// Scales `range`, a frame at a time, by the gains the envelopes and the
    /// modulators ask for: an automated parameter's own value on the exact
    /// frame, at the pan law, and the controls where they are for the rest.
    /// `from` is the song frame of the block's first frame, none while the
    /// song is stopped. The mixer comes to rest at the last frame's gains, so
    /// a stop carries on from where the automation left off, and a control
    /// whose envelopes are gone heads back to what it asks for from there.
    fn scale_envelopes(
        &mut self,
        block: &mut Block<'_>,
        range: core::ops::Range<usize>,
        from: Option<i64>,
        list: &AutomationList,
        now: &SessionNow<'_>,
        bus: Bus<'_>,
    ) {
        let channels = block.channels();
        if self.split {
            self.scale_split(block, range, from, list, now, bus);
            return;
        }
        // A control the envelopes and modulators leave alone plays through
        // its gliding value, so a move of it glides (the mixer spec, R18).
        let free_mute = !list.drives(track::MUTE) && !list.modulates(track::MUTE);
        let free_pan = !list.drives(track::PAN) && !list.modulates(track::PAN);
        let mute = f64::from(u8::from(self.mute && !free_mute));
        // A pan the envelopes don't move keeps its gains while it rests; only
        // an automated, modulated or gliding pan works out the law frame by
        // frame.
        let fixed_pan = |pan: &Ramp| (free_pan && pan.is_settled()).then(|| pan_gains(pan.value()));
        // The crossfade isn't automated: it glides on top of the envelopes,
        // the same gliding position for every channel (the mixer spec, R23).
        let (crossfade, start) = (self.crossfade, self.crossfader);
        let rest = self.controls;
        let fixed = fixed_pan(&rest[1]);
        for c in 0..channels {
            let index = gain_index(c, channels);
            let mut crossfader = start;
            let [mut volume, mut pan, mut gate] = rest;
            let meter = c < 2;
            let samples = &mut block.channel_mut(c)[range.clone()];
            for (i, sample) in samples.iter_mut().enumerate() {
                let at = range.start + i;
                let song = from.map(|from| from + at as i64);
                let fade = crossfade_gain(crossfader.next(), crossfade);
                let controls = [volume.next(), pan.next(), mute];
                let gate = if free_mute { gate.next() } else { 1.0 };
                *sample = (f64::from(*sample)
                    * gain_at(list, now, song, bus, at, index, controls, fixed)
                    * gate
                    * fade) as f32;
                if meter {
                    self.meters.measure_one(c, *sample);
                }
            }
        }
        // Leave the crossfader and the controls where their glides reached
        // over the range, once, whatever the channel count.
        let mut crossfader = start;
        for _ in range.clone() {
            crossfader.next();
        }
        self.crossfader = crossfader;
        self.controls
            .iter_mut()
            .for_each(|ramp| ramp.skip(range.len()));
        let controls = [self.controls[0].value(), self.controls[1].value(), mute];
        let fixed = fixed_pan(&self.controls[1]);
        let unmuted = if free_mute {
            self.controls[2].value()
        } else {
            1.0
        };
        let (last, last_song) = (range.end - 1, from.map(|from| from + range.end as i64 - 1));
        for (index, ramp) in self.gains.iter_mut().enumerate() {
            *ramp = Ramp::at(
                gain_at(list, now, last_song, bus, last, index, controls, fixed) * unmuted,
            );
        }
        for ((entry, param), control) in self
            .modulated
            .iter_mut()
            .zip([track::VOLUME, track::PAN, track::MUTE])
            .zip(controls)
        {
            *entry = list
                .modulates(param)
                .then(|| played(list, now, last_song, bus, last, param, control))
                .filter(|(base, played, held)| *held || played != base)
                .map(|(base, played, _)| (base, played));
        }
    }

    /// In Split Stereo Pan Mode, scales `range` a frame at a time by the
    /// split controls and the level, at the pan law of each input channel:
    /// the left output takes the placed left and right channels, and the
    /// right output the same, as [`split_pan_gains`] says (the mixer spec).
    fn scale_split(
        &mut self,
        block: &mut Block<'_>,
        range: core::ops::Range<usize>,
        from: Option<i64>,
        list: &AutomationList,
        now: &SessionNow<'_>,
        bus: Bus<'_>,
    ) {
        let channels = block.channels();
        // As in `scale_envelopes`, the controls the envelopes leave alone
        // glide.
        let free_mute = !list.drives(track::MUTE) && !list.modulates(track::MUTE);
        let mute = f64::from(u8::from(self.mute && !free_mute));
        let mut controls = [0.0; 4];
        let mut gate = 1.0;
        let (crossfade, start) = (self.crossfade, self.crossfader);
        let mut crossfader = start;
        for at in range.clone() {
            let song = from.map(|from| from + at as i64);
            let fade = crossfade_gain(crossfader.next(), crossfade);
            controls = [
                self.controls[0].next(),
                mute,
                self.split_ramps[0].next(),
                self.split_ramps[1].next(),
            ];
            gate = if free_mute {
                self.controls[2].next()
            } else {
                1.0
            };
            let level = split_level_at(list, now, song, bus, at, controls) * gate;
            let sl = played(list, now, song, bus, at, track::SPLIT_LEFT, controls[2])
                .1
                .clamp(-1.0, 1.0);
            let sr = played(list, now, song, bus, at, track::SPLIT_RIGHT, controls[3])
                .1
                .clamp(-1.0, 1.0);
            let [[a, b], [c, d]] = split_pan_gains(sl, sr);
            if let Some((left, right)) = block.channels_pair_mut() {
                let l = f64::from(left[at]);
                let r = f64::from(right[at]);
                left[at] = ((l * a + r * b) * level * fade) as f32;
                right[at] = ((l * c + r * d) * level * fade) as f32;
                self.meters.measure_one(0, left[at]);
                self.meters.measure_one(1, right[at]);
            }
            for ch in 2..channels {
                let sample = &mut block.channel_mut(ch)[at];
                *sample = (f64::from(*sample) * level * fade) as f32;
            }
            if channels < 2 {
                for ch in 0..channels {
                    let sample = &mut block.channel_mut(ch)[at];
                    *sample = (f64::from(*sample) * level * fade) as f32;
                }
            }
        }
        self.crossfader = crossfader;
        self.controls[1].skip(range.len());
        let (last, last_song) = (range.end - 1, from.map(|from| from + range.end as i64 - 1));
        self.split_level =
            Ramp::at(split_level_at(list, now, last_song, bus, last, controls) * gate);
        self.modulated[0] =
            modulation_entry(list, now, last_song, bus, last, track::VOLUME, controls[0]);
        self.modulated[1] = None;
        self.modulated[2] =
            modulation_entry(list, now, last_song, bus, last, track::MUTE, controls[1]);
        self.modulated[3] = modulation_entry(
            list,
            now,
            last_song,
            bus,
            last,
            track::SPLIT_LEFT,
            controls[2],
        );
        self.modulated[4] = modulation_entry(
            list,
            now,
            last_song,
            bus,
            last,
            track::SPLIT_RIGHT,
            controls[3],
        );
    }
}

/// The level a split-mode frame plays at: the volume, or silence while the
/// activator is off.
fn split_level_at(
    list: &AutomationList,
    now: &SessionNow<'_>,
    song: Option<i64>,
    bus: Bus<'_>,
    at: usize,
    controls: [f64; 4],
) -> f64 {
    if played(list, now, song, bus, at, track::MUTE, controls[1]).1 != 0.0 {
        0.0
    } else {
        played(list, now, song, bus, at, track::VOLUME, controls[0])
            .1
            .max(0.0)
    }
}

/// The base and the played value of a mixer control at frame `at`, kept for
/// the modulation readback when a modulator moves it.
fn modulation_entry(
    list: &AutomationList,
    now: &SessionNow<'_>,
    song: Option<i64>,
    bus: Bus<'_>,
    at: usize,
    param: ParamId,
    control: f64,
) -> Option<(f64, f64)> {
    list.modulates(param)
        .then(|| played(list, now, song, bus, at, param, control))
        .filter(|(base, played, held)| *held || played != base)
        .map(|(base, played, _)| (base, played))
}

/// The gain of channel `index` at frame `at` of the block, song frame `song`:
/// an automated parameter from its envelope, a modulator's hold or a control
/// where it is, with the clips' and the modulators' modulation on top (the
/// modulation spec, R2 to R4, R14).
#[allow(clippy::too_many_arguments)]
fn gain_at(
    list: &AutomationList,
    now: &SessionNow<'_>,
    song: Option<i64>,
    bus: Bus<'_>,
    at: usize,
    index: usize,
    controls: [f64; 3],
    fixed_pan: Option<[f64; 2]>,
) -> f64 {
    let [volume, pan, mute] = controls;
    if played(list, now, song, bus, at, track::MUTE, mute).1 != 0.0 {
        return 0.0;
    }
    let level = played(list, now, song, bus, at, track::VOLUME, volume)
        .1
        .max(0.0);
    let [left, right] = fixed_pan.unwrap_or_else(|| {
        pan_gains(
            played(list, now, song, bus, at, track::PAN, pan)
                .1
                .clamp(-1.0, 1.0),
        )
    });
    match index {
        0 => level * left,
        1 => level * right,
        _ => level,
    }
}

/// The base, the played value and whether a modulator holds the mixer
/// control `param` at frame `at` of the block: its hold, else its envelope's
/// value where it has one and the song plays, else `control`; then moved by
/// the clips' and the modulators' modulation.
fn played(
    list: &AutomationList,
    now: &SessionNow<'_>,
    song: Option<i64>,
    bus: Bus<'_>,
    at: usize,
    param: ParamId,
    control: f64,
) -> (f64, f64, bool) {
    let travel = match param {
        track::VOLUME => Travel::Fader,
        track::PAN => Travel::Pan,
        _ => Travel::Switch,
    };
    let held = list.held(param, bus, at).is_some();
    let base = list.base(travel, param, song, now, bus, at, |_| control);
    (base, list.played(travel, param, song, base, bus, at), held)
}

/// The shape of the planar buffers a route mixes: `channels` channels
/// `stride` apart, of which the block uses `frames`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Planar {
    pub(crate) channels: usize,
    pub(crate) stride: usize,
    pub(crate) frames: usize,
}

/// A route into a track: another track's output into its group, Main, or
/// another track's input; a send into a return; or a track's own input
/// (the routing spec). It adds the other track's signal, scaled by its
/// gain, and takes the [`route`] parameters: the gain is the level (a
/// send's; any other route's is 1) unless solo silences the route or the
/// track it feeds isn't monitoring it, and it glides to every new value
/// ([`SMOOTHING_SECONDS`]).
#[derive(Debug, Clone)]
pub struct Route {
    level: f64,
    solo_mute: bool,
    closed: bool,
    /// The track this route feeds monitors with Auto: while its clips play,
    /// the route closes (the routing spec, R11).
    auto: bool,
    /// Whether the track's clips played in the last block, set by the
    /// program before it sums the track's inputs.
    playing_clip: bool,
    gain: Ramp,
    smoothing: u32,
    /// The envelope driving its level while the song plays (WP-2.12).
    automation: Option<NodeData>,
    /// The level's base and the value it played at the last block's last
    /// frame, while a clip's modulation acts on it.
    modulated: Option<(f64, f64)>,
}

impl Route {
    /// A route at `level`, silenced by solo or not, resting there.
    #[must_use]
    pub fn new(level: f64, solo_mute: bool, config: &NodeConfig) -> Self {
        let mut route = Self {
            level: 0.0,
            solo_mute,
            closed: false,
            auto: false,
            playing_clip: false,
            gain: Ramp::at(0.0),
            smoothing: smoothing_frames(config.sample_rate),
            automation: None,
            modulated: None,
        };
        route.set(route::LEVEL, ParamValue::Set(level));
        route.gain = Ramp::at(route.target());
        route
    }

    /// The same route, closed or open by its track's monitoring, resting
    /// there.
    #[must_use]
    pub fn closed(mut self, closed: bool) -> Self {
        self.closed = closed;
        self.gain = Ramp::at(self.target());
        self
    }

    /// The same route, closing while its track's clips play or not,
    /// resting there.
    #[must_use]
    pub fn auto(mut self, auto: bool) -> Self {
        self.auto = auto;
        self.gain = Ramp::at(self.target());
        self
    }

    /// Sets whether the track's clips played in the last block: under
    /// Monitor Auto the route closes while they do (the routing spec, R11).
    /// The gain glides like any other change; the program calls it once per
    /// block, before it sums the track's inputs.
    pub(crate) fn set_playing_clip(&mut self, playing: bool) {
        if self.playing_clip != playing {
            self.playing_clip = playing;
            self.gain.toward(self.target(), self.smoothing);
        }
    }

    /// The gain now.
    #[must_use]
    pub fn gain(&self) -> f64 {
        self.gain.value()
    }

    /// Whether the route adds nothing and will go on adding nothing until a
    /// parameter changes.
    #[must_use]
    pub fn is_silent(&self) -> bool {
        self.gain.is_settled() && self.gain.value() == 0.0
    }

    fn target(&self) -> f64 {
        if self.solo_mute || self.closed || (self.auto && self.playing_clip) {
            0.0
        } else {
            self.level
        }
    }

    fn set(&mut self, param: ParamId, value: ParamValue) {
        match (param, value) {
            (route::LEVEL, ParamValue::Set(level)) if level.is_finite() => {
                self.level = level.max(0.0);
            }
            (route::LEVEL, _) => self.level = 1.0,
            (route::SOLO_MUTE, ParamValue::Set(mute)) => self.solo_mute = mute != 0.0,
            (route::SOLO_MUTE, ParamValue::Default) => self.solo_mute = false,
            (route::CLOSED, ParamValue::Set(closed)) => self.closed = closed != 0.0,
            (route::CLOSED, ParamValue::Default) => self.closed = false,
            (route::AUTO, ParamValue::Set(auto)) => self.auto = auto != 0.0,
            (route::AUTO, ParamValue::Default) => self.auto = false,
            _ => {}
        }
    }

    /// Takes the parameter changes of a block in which the route mixes
    /// nothing, such as the send of a chain in a rack that is off, so it
    /// comes back with what was set meanwhile.
    pub(crate) fn flush(&mut self, events: &[NodeEvent]) {
        for event in events {
            self.set(event.param, event.value);
        }
        self.gain = Ramp::at(self.target());
    }

    /// Adds `frames` frames of `from` into `to`, both planar with
    /// `channels` channels `stride` apart, scaled by the route's gain, with
    /// `events` taking effect on their frames. `song` is the song frame of
    /// the first frame while the song plays, and `bus` holds what the
    /// modulators wrote. Real-time safe.
    pub(crate) fn mix(
        &mut self,
        to: &mut [f32],
        from: &[f32],
        shape: Planar,
        song: Option<Song<'_>>,
        bus: Bus<'_>,
        events: &[NodeEvent],
    ) {
        let Planar {
            channels,
            stride,
            frames,
        } = shape;
        // Spec: automation R2; clip envelopes R3. While the song plays, a
        // send with an envelope takes its level from it on each frame, and
        // the clips' modulation is added on top; a send a modulator maps
        // follows it whether the song plays or not. Solo and a closed route
        // still silence it, and a hand move still glides.
        let automation = self.automation.clone();
        let list = automation
            .as_ref()
            .and_then(|data| data.downcast_ref::<AutomationList>())
            .filter(|list| !list.is_empty());
        // The Session clip that sets the level while the song plays (WP-3.12).
        let now = match (list, song) {
            (Some(list), Some(song)) => list.now(song.session, song.tempo, song.rate),
            _ => SessionNow::NONE,
        };
        let song = song.map(|song| song.frame);
        let driven = song.is_some() && list.is_some_and(|list| list.drives(route::LEVEL));
        let modulated = list.is_some_and(|list| list.modulates(route::LEVEL));
        let follows = list.is_some()
            && (list.is_some_and(|list| list.maps(route::LEVEL))
                || (song.is_some() && (driven || modulated)));
        self.modulated = None;
        if !follows && events.is_empty() && self.is_silent() {
            return;
        }
        // A send whose envelope is gone heads back to its control's level
        // from where the automation left it.
        if !follows {
            self.gain.toward(self.target(), self.smoothing);
        }
        for segment in segments(events, frames) {
            if !segment.events.is_empty() {
                for event in segment.events {
                    self.set(event.param, event.value);
                }
                self.gain.toward(self.target(), self.smoothing);
            }
            if follows {
                let list = list.expect("a route that follows has a list");
                for c in 0..channels {
                    let at = c * stride + segment.range.start..c * stride + segment.range.end;
                    for (i, sample) in to[at.clone()].iter_mut().enumerate() {
                        let frame = segment.range.start + i;
                        let at_song = song.map(|song| song + frame as i64);
                        let gain = self.envelope_gain(list, &now, at_song, bus, frame);
                        *sample += (f64::from(from[at.start + i]) * gain) as f32;
                    }
                }
                let last = segment.range.end - 1;
                let at_song = song.map(|song| song + last as i64);
                self.gain = Ramp::at(self.envelope_gain(list, &now, at_song, bus, last));
                let (base, played, held) = self.level_at(list, &now, at_song, bus, last);
                self.modulated = (modulated || list.maps(route::LEVEL))
                    .then_some((base, played))
                    .filter(|(base, played)| held || played != base);
            } else {
                let mut after = self.gain;
                for c in 0..channels {
                    let at = c * stride + segment.range.start..c * stride + segment.range.end;
                    let mut ramp = self.gain;
                    add_scaled(&mut to[at.clone()], &from[at], &mut ramp);
                    after = ramp;
                }
                self.gain = after;
            }
        }
    }

    /// Takes the envelope driving its level (WP-2.12), if that's what the
    /// data holds; anything else is let go of.
    pub(crate) fn receive_data(&mut self, data: NodeData) {
        if data.downcast_ref::<AutomationList>().is_some() {
            self.automation = Some(data);
        }
    }

    /// The route's gain at frame `at` of the block, song frame `song`: its
    /// envelope's level, a modulator's hold, or its control's where the
    /// level isn't automated, with the clips' and the modulators' modulation
    /// added on top (WP-3.15, WP-5.10) and clamped, unless solo silences it
    /// or it's closed.
    fn envelope_gain(
        &self,
        list: &AutomationList,
        now: &SessionNow<'_>,
        song: Option<i64>,
        bus: Bus<'_>,
        at: usize,
    ) -> f64 {
        if self.solo_mute || self.closed {
            return 0.0;
        }
        self.level_at(list, now, song, bus, at).1.max(0.0)
    }

    /// The level's base, played value and whether a modulator holds it at
    /// frame `at` (the modulation spec, R3): its hold, its envelope's or its
    /// control's, moved by the clips' and the modulators' modulation on the
    /// send's travel.
    fn level_at(
        &self,
        list: &AutomationList,
        now: &SessionNow<'_>,
        song: Option<i64>,
        bus: Bus<'_>,
        at: usize,
    ) -> (f64, f64, bool) {
        let held = list.held(route::LEVEL, bus, at).is_some();
        let base = list.base(Travel::Send, route::LEVEL, song, now, bus, at, |_| {
            self.level
        });
        (
            base,
            list.played(Travel::Send, route::LEVEL, song, base, bus, at),
            held,
        )
    }

    /// Reports the level when the last block played it under modulation
    /// (WP-5.9).
    pub(crate) fn modulated(&self, report: &mut dyn FnMut(ParamId, f64, f64)) {
        if let Some((base, played)) = self.modulated {
            report(route::LEVEL, base, played);
        }
    }
}

/// Who solo silences, for a document's solo switches, its routing, and its
/// Solo in Place setting (the mixer spec, R10 to R14; the routing spec).
///
/// While no track is soloed, solo silences nothing. Otherwise the *solo
/// path* is every soloed track; every track a soloed track's output
/// reaches, through groups and other tracks' inputs, short of Main; and
/// every track whose output reaches a soloed track that way (a soloed
/// group's tracks, and tracks routed into a soloed track). Then:
///
/// - an audio, MIDI, or group track outside the path is silent: its output
///   and its sends;
/// - a return track that isn't soloed is heard only if Solo in Place is on
///   and no return is soloed;
/// - a send from a track outside the path still feeds a soloed return, so a
///   soloed return hears everything sent to it;
/// - a track's input taken from another track before its mixer (Pre FX or
///   Post FX) is never silenced, and one taken after it (Post Mixer) is
///   silenced with the other track's output;
/// - a route into a device's sidechain is never silenced;
/// - Main is always heard.
///
/// In Cue mode the Solo switches are Cue switches and silence nothing: Main
/// plays what the activators leave (the mixer spec, S5).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Solo {
    cue: bool,
    main: TrackId,
    soloed: BTreeSet<TrackId>,
    path: BTreeSet<TrackId>,
    /// Groups that hold a soloed track at any depth.
    holding: BTreeSet<TrackId>,
    returns: BTreeSet<TrackId>,
    return_soloed: bool,
    solo_in_place: bool,
}

impl Solo {
    /// Works out who solo silences in `document`.
    #[must_use]
    pub fn of(document: &Document) -> Self {
        Self::with(document, &Graph::of(document))
    }

    /// Works out who solo silences in `document`, whose routing makes
    /// `graph`.
    #[must_use]
    pub fn with(document: &Document, graph: &Graph) -> Self {
        let returns: BTreeSet<TrackId> = document.returns().collect();
        let main = document.main_id();
        let mut solo = Self {
            cue: document.cue_active(),
            main,
            soloed: BTreeSet::new(),
            path: BTreeSet::new(),
            holding: BTreeSet::new(),
            return_soloed: false,
            returns,
            solo_in_place: document.settings().solo_in_place,
        };
        for track in document.tracks() {
            if track.kind() == TrackKind::Main || !track.mixer().solo {
                continue;
            }
            let id = track.id();
            solo.soloed.insert(id);
            solo.path.insert(id);
            if track.kind() == TrackKind::Return {
                solo.return_soloed = true;
            }
            // Where its output goes, short of Main.
            let mut at = graph.output_target(id);
            while let Some(to) = at.filter(|&to| to != main) {
                if !solo.path.insert(to) {
                    break;
                }
                at = graph.output_target(to);
            }
            // What feeds it through outputs.
            let mut feeding: Vec<TrackId> = graph.output_sources(id).collect();
            while let Some(from) = feeding.pop() {
                if solo.path.insert(from) {
                    feeding.extend(graph.output_sources(from));
                }
            }
        }
        // Groups shown half lit hold a soloed track at any depth.
        for &id in &solo.soloed {
            let mut parent = document.parent(id);
            while let Some(group) = parent {
                solo.holding.insert(group);
                parent = document.parent(group);
            }
        }
        solo
    }

    /// Whether solo silences anything: a track is soloed and the set isn't
    /// in Cue mode.
    #[must_use]
    pub fn is_active(&self) -> bool {
        !self.cue && !self.soloed.is_empty()
    }

    /// Whether `track`'s Solo switch is on.
    #[must_use]
    pub fn is_soloed(&self, track: TrackId) -> bool {
        self.soloed.contains(&track)
    }

    /// Whether `track` is a group that holds a soloed track, at any depth:
    /// Live draws such a group's Solo switch half lit.
    #[must_use]
    pub fn holds_solo(&self, track: TrackId) -> bool {
        self.holding.contains(&track)
    }

    /// Whether solo silences `track`'s output.
    #[must_use]
    pub fn output_muted(&self, track: TrackId) -> bool {
        if !self.is_active() || track == self.main || self.path.contains(&track) {
            return false;
        }
        if self.returns.contains(&track) {
            return !(self.solo_in_place && !self.return_soloed);
        }
        true
    }

    /// Whether solo silences `track`'s send to the return track `to`.
    #[must_use]
    pub fn send_muted(&self, track: TrackId, to: TrackId) -> bool {
        self.is_active() && !self.path.contains(&track) && !self.soloed.contains(&to)
    }

    /// Whether solo silences a route of `graph`.
    #[must_use]
    pub fn edge_muted(&self, edge: &Edge) -> bool {
        match edge.via {
            Via::Output => self.output_muted(edge.from),
            Via::Send | Via::Chain(_) => self.send_muted(edge.from, edge.to),
            // A chain's taps all come before the track's mixer, so solo never
            // silences them (the racks spec, R40).
            // An instrument output, and a chain's taps, all come before the
            // track's mixer, so solo never silences them (the routing spec,
            // R15).
            Via::Input => {
                edge.chain.is_none()
                    && edge.output.is_none()
                    && edge.tap == Tap::PostMixer
                    && self.output_muted(edge.from)
            }
            // A device's sidechain hears its sources whatever is soloed, so
            // a soloed track still ducks as it does in the mix (to observe
            // in Live; the routing spec).
            Via::Sidechain(_) => false,
        }
    }

    /// Whether `track` would be heard on Main with every gain above 0: its
    /// own output and, for tracks routed into groups or other tracks, each
    /// one's output up to Main. A track that's silent here can still be
    /// heard through its sends.
    #[must_use]
    pub fn heard_directly(&self, document: &Document, track: TrackId) -> bool {
        let mut at = Some(track);
        let mut seen = BTreeSet::new();
        while let Some(id) = at.filter(|&id| id != self.main) {
            if self.output_muted(id) || !seen.insert(id) {
                return false;
            }
            at = document.output_of(id);
        }
        at.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A buffer shape for the tests.
    fn planar(channels: usize, stride: usize, frames: usize) -> Planar {
        Planar {
            channels,
            stride,
            frames,
        }
    }
    use crate::node::TransportInfo;

    fn config() -> NodeConfig {
        NodeConfig {
            round_trip_latency: 0,
            sample_rate: SampleRate::HZ_48000,
            max_frames: 4096,
            channels: 2,
        }
    }

    // Spec: mixer R3
    #[test]
    fn the_pan_law_is_constant_power_with_0_db_at_the_centre_and_3_db_at_the_sides() {
        assert_eq!(pan_gains(0.0), [1.0, 1.0]);
        assert_eq!(pan_gains(-1.0), [SQRT_2, 0.0]);
        assert_eq!(pan_gains(1.0), [0.0, SQRT_2]);
        assert!((db_from_gain(SQRT_2) - 3.0103).abs() < 1e-4);
        for i in -100..=100 {
            let pan = f64::from(i) / 100.0;
            let [left, right] = pan_gains(pan);
            assert!((left * left + right * right - 2.0).abs() < 1e-12, "{pan}");
            let [mirror_left, mirror_right] = pan_gains(-pan);
            assert_eq!((left, right), (mirror_right, mirror_left), "{pan}");
            if i > 0 {
                assert!(right > left);
            }
        }
        assert_eq!(pan_gains(f64::NAN), [1.0, 1.0]);
        assert_eq!(pan_gains(7.0), pan_gains(1.0));
    }

    // Spec: mixer R23
    #[test]
    fn the_crossfade_curve_hits_the_measured_points() {
        // The default Constant Power curve's measured points (the mixer
        // spec, R23): the crossfader's position from 0 (50A) to 1 (50B),
        // and the gain of a track assigned A, in decibels.
        let measured = [
            (0.00, 0.0),
            (0.22, -0.5),
            (0.30, -0.9),
            (0.48, -2.7),
            (0.50, -3.0),
            (0.57, -4.1),
            (0.75, -8.7),
            (0.83, -14.9),
            (0.92, -37.0),
            (0.95, -52.7),
        ];
        for (position, db) in measured {
            let got = db_from_gain(constant_power(position));
            // Below -30 dB the readout was coarser, so the tolerance is
            // wider there.
            let tolerance = if db < -30.0 { 1.5 } else { 0.5 };
            assert!((got - db).abs() <= tolerance, "{position}: {got} for {db}");
        }
        // The B side's own measurements, through the mirror.
        for (position, db) in [(0.61, -1.7), (0.22, -9.9)] {
            let got = db_from_gain(crossfade_gain(position, CrossfadeAssign::B));
            let tolerance = if db < -30.0 { 1.5 } else { 0.5 };
            assert!((got - db).abs() <= tolerance, "{position}: {got} for {db}");
        }
        // Continuous at the join between the cosine and the tail.
        let step = 1e-9;
        assert!(
            (constant_power(CROSSFADE_JOIN - step) - constant_power(CROSSFADE_JOIN + step)).abs()
                < 1e-6
        );
        // Unity at the near end, silence from 0.96 to the far end.
        assert_eq!(constant_power(0.0), 1.0);
        assert_eq!(constant_power(0.96), 0.0);
        assert_eq!(constant_power(1.0), 0.0);
        // A track assigned to neither passes at every position.
        for position in [0.0, 0.3, 0.5, 0.75, 0.9, 1.0] {
            assert_eq!(crossfade_gain(position, CrossfadeAssign::None), 1.0);
        }
        // Mirrored positions give mirrored gains.
        for i in 0..=8 {
            let p = f64::from(i) / 8.0;
            assert_eq!(
                crossfade_gain(p, CrossfadeAssign::A),
                crossfade_gain(1.0 - p, CrossfadeAssign::B),
                "{p}"
            );
        }
        // Ends hold, and a position that isn't a number takes the centre.
        assert_eq!(constant_power(-1.0), 1.0);
        assert_eq!(constant_power(7.0), 0.0);
        assert_eq!(constant_power(f64::NAN), constant_power(0.5));
    }

    // Spec: mixer R18, R23
    #[test]
    fn the_crossfader_glides_over_20_ms_and_the_gain_follows_the_curve() {
        let mixer = Mixer {
            crossfade: CrossfadeAssign::A,
            ..Mixer::default()
        };
        let mut node = TrackMixer::new(&mixer, 0.0, &config());
        // The A end: the assigned track at unity.
        assert_eq!(node.gains(), [1.0, 1.0]);
        let events = [NodeEvent {
            offset: 0,
            param: track::CROSSFADER,
            value: ParamValue::Set(0.5),
        }];
        let out = render(&mut node, [1.0, 1.0], 2000, &events);
        let left = &out[..2000];
        // 960 frames at 48 kHz: the position moves in a straight line to
        // the centre, the gain follows the curve frame by frame, and it
        // lands exactly on the law's value there.
        for (frame, &sample) in left[..960].iter().enumerate() {
            let position = (frame + 1) as f64 / 960.0 * 0.5;
            let expected = f64::from(constant_power(position) as f32);
            assert!(
                (f64::from(sample) - expected).abs() < 1e-5,
                "{frame}: {sample} for {expected}"
            );
        }
        let centre = constant_power(0.5) as f32;
        assert_eq!(left[959], centre);
        assert!(left[960..].iter().all(|&s| s == centre));
        assert_eq!(&out[..2000], &out[2000..]);
    }

    #[test]
    fn decibels_convert_both_ways() {
        assert_eq!(gain_from_db(0.0), 1.0);
        assert_eq!(gain_from_db(f64::NEG_INFINITY), 0.0);
        assert!((gain_from_db(-6.02) - 0.5).abs() < 0.0005);
        assert_eq!(db_from_gain(0.0), f64::NEG_INFINITY);
        assert!((db_from_gain(gain_from_db(-17.5)) + 17.5).abs() < 1e-12);
    }

    // Spec: mixer R18
    #[test]
    fn a_ramp_glides_in_a_straight_line_and_lands_exactly() {
        let mut ramp = Ramp::at(1.0);
        ramp.toward(0.5, 4);
        let values: Vec<f64> = (0..6).map(|_| ramp.next()).collect();
        assert_eq!(values, [0.875, 0.75, 0.625, 0.5, 0.5, 0.5]);
        assert!(ramp.is_settled());
        // A new target mid-glide starts from where the gain is.
        ramp.toward(1.0, 4);
        ramp.next();
        ramp.toward(0.0, 2);
        assert_eq!([ramp.next(), ramp.next()], [0.3125, 0.0]);
        // The same target again carries on rather than starting over.
        ramp.toward(1.0, 4);
        ramp.next();
        ramp.toward(1.0, 4);
        assert_eq!([ramp.next(), ramp.next(), ramp.next()], [0.5, 0.75, 1.0]);
    }

    fn render(
        node: &mut TrackMixer,
        input: [f32; 2],
        frames: usize,
        events: &[NodeEvent],
    ) -> Vec<f32> {
        let mut data = vec![0.0f32; 2 * frames];
        data[..frames].fill(input[0]);
        data[frames..].fill(input[1]);
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

    // Spec: mixer R4
    #[test]
    fn a_track_mixer_at_rest_passes_audio_through_to_the_bit() {
        let mut node = TrackMixer::new(&Mixer::default(), 0.5, &config());
        let mut data: Vec<f32> = (0..256).map(|i| (i as f32 * 0.37).sin() * 0.3).collect();
        let original = data.clone();
        let mut block = Block::new(
            &mut data,
            2,
            128,
            128,
            &[],
            TransportInfo::default(),
            SampleRate::HZ_48000,
        );
        node.process(&mut block);
        assert_eq!(data, original);
    }

    // Spec: mixer R18
    #[test]
    fn a_volume_change_glides_over_20_ms_from_its_frame() {
        let mut node = TrackMixer::new(&Mixer::default(), 0.5, &config());
        let events = [NodeEvent {
            offset: 100,
            param: track::VOLUME,
            value: ParamValue::Set(0.5),
        }];
        let out = render(&mut node, [1.0, 1.0], 2000, &events);
        let left = &out[..2000];
        assert!(left[..100].iter().all(|&s| s == 1.0));
        // 960 frames at 48 kHz, each a step of 0.5 / 960.
        for (i, &sample) in left[100..1060].iter().enumerate() {
            let expected = 1.0 - 0.5 * (i + 1) as f64 / 960.0;
            assert!((f64::from(sample) - expected).abs() < 1e-6, "{i}: {sample}");
        }
        assert!(left[1059..].iter().all(|&s| s == 0.5));
        assert_eq!(&out[..2000], &out[2000..]);
    }

    // Spec: mixer R3, R5
    #[test]
    fn pan_and_the_activator_set_each_channels_gain() {
        let mut mixer = Mixer {
            pan: -1.0,
            volume: 0.5,
            ..Mixer::default()
        };
        let mut node = TrackMixer::new(&mixer, 0.5, &config());
        assert_eq!(node.gains(), [0.5 * SQRT_2, 0.0]);
        let out = render(&mut node, [1.0, 1.0], 64, &[]);
        assert!(out[..64].iter().all(|&s| s == (0.5 * SQRT_2) as f32));
        assert!(out[64..].iter().all(|&s| s == 0.0));
        mixer.mute = true;
        let node = TrackMixer::new(&mixer, 0.5, &config());
        assert_eq!(node.gains(), [0.0, 0.0]);
    }

    // Spec: mixer R29
    #[test]
    fn split_mode_places_each_input_channel_on_its_own_control() {
        // The left control fully right: the left channel is on the right at
        // the law's gain (√2) and gone from the left.
        let mixer = Mixer {
            pan_mode: PanMode::Split,
            split_left: 1.0,
            ..Mixer::default()
        };
        let mut node = TrackMixer::new(&mixer, 0.5, &config());
        let out = render(&mut node, [1.0, 0.0], 64, &[]);
        assert!(out[..64].iter().all(|&s| s == 0.0), "the left is silent");
        assert!(
            out[64..].iter().all(|&s| (s - SQRT_2 as f32).abs() < 1e-6),
            "the left channel is on the right at √2"
        );

        // The right control fully left puts the right channel on the left.
        let mixer = Mixer {
            pan_mode: PanMode::Split,
            split_right: -1.0,
            ..Mixer::default()
        };
        let mut node = TrackMixer::new(&mixer, 0.5, &config());
        let out = render(&mut node, [0.0, 1.0], 64, &[]);
        assert!(
            out[..64].iter().all(|&s| (s - SQRT_2 as f32).abs() < 1e-6),
            "the right channel is on the left at √2"
        );
        assert!(out[64..].iter().all(|&s| s == 0.0), "the right is silent");

        // A centred control leaves its channel at 0 dB in both outputs.
        let mixer = Mixer {
            pan_mode: PanMode::Split,
            ..Mixer::default()
        };
        let mut node = TrackMixer::new(&mixer, 0.5, &config());
        let out = render(&mut node, [1.0, 0.0], 64, &[]);
        assert!(out[..64].iter().all(|&s| s == 1.0));
        assert!(out[64..].iter().all(|&s| s == 1.0));
    }

    // Spec: mixer R19
    #[test]
    fn meters_show_peaks_at_once_and_rms_as_a_slow_average() {
        let mut node = TrackMixer::new(&Mixer::default(), 0.5, &config());
        render(&mut node, [0.5, 0.25], 64, &[]);
        let reading = node.meters();
        assert_eq!(reading.peak, [0.5, 0.25]);
        assert!(reading.rms[0] < 0.1, "the average has barely moved");
        // After a second of a steady level, the RMS meter is within 0.3 dB
        // of it (its average has come 1 − e^(−1/0.3) of the way), and after
        // three it reads it.
        for _ in 0..12 {
            render(&mut node, [0.5, 0.25], 4000, &[]);
        }
        let short = db_from_gain(f64::from(node.meters().rms[0])) - db_from_gain(0.5);
        assert!((-0.3..-0.1).contains(&short), "{short} dB");
        for _ in 0..24 {
            render(&mut node, [0.5, 0.25], 4000, &[]);
        }
        let reading = node.meters();
        assert!((reading.rms[0] - 0.5).abs() < 0.001, "{:?}", reading.rms);
        assert!((reading.rms[1] - 0.25).abs() < 0.001, "{:?}", reading.rms);
        // A second of silence: the peak falls by its release rate.
        for _ in 0..12 {
            render(&mut node, [0.0, 0.0], 4000, &[]);
        }
        let fallen = db_from_gain(f64::from(node.meters().peak[0])) - db_from_gain(0.5);
        assert!(
            (fallen + METER_RELEASE_DB_PER_SECOND).abs() < 0.01,
            "fell {fallen} dB in a second"
        );
    }

    // Spec: mixer R10, R18
    #[test]
    fn a_route_adds_its_input_scaled_and_glides_when_solo_closes_it() {
        let config = config();
        let mut route = Route::new(1.0, false, &config);
        let from = vec![0.25f32; 2 * 1000];
        let mut to = vec![0.5f32; 2 * 1000];
        route.mix(&mut to, &from, planar(2, 1000, 1000), None, Bus::EMPTY, &[]);
        assert!(to.iter().all(|&s| s == 0.75));

        let events = [NodeEvent {
            offset: 10,
            param: route::SOLO_MUTE,
            value: ParamValue::Set(1.0),
        }];
        let mut to = vec![0.0f32; 2 * 1000];
        route.mix(
            &mut to,
            &from,
            planar(2, 1000, 1000),
            None,
            Bus::EMPTY,
            &events,
        );
        assert!(to[..10].iter().all(|&s| s == 0.25));
        assert!(to[10..970].windows(2).all(|w| w[1] < w[0]), "a glide down");
        assert!(to[969..1000].iter().all(|&s| s == 0.0));
        assert_eq!(to[..1000], to[1000..]);
        assert!(route.is_silent());

        // A silent send adds nothing, and doesn't look at its input.
        let mut send = Route::new(0.0, false, &config);
        let mut to = vec![0.5f32; 2 * 8];
        send.mix(&mut to, &[1.0; 16], planar(2, 8, 8), None, Bus::EMPTY, &[]);
        assert!(to.iter().all(|&s| s == 0.5));
    }
}

//! The mixer: each track's pan, volume, and activator; the routes that carry
//! tracks into their groups, Main, and the returns; solo; and the meters
//! (WP-1.6). The rules, and the Live sources behind them, are in
//! `docs/specs/mixer.md`.
//!
//! A track's signal runs through its task in the program (see
//! [`EngineProgram`](crate::EngineProgram)):
//!
//! 1. its inputs, summed: the outputs of the tracks in a group, the top-level
//!    tracks and returns for Main, or the sends to a return. Each input is a
//!    [`Route`], a gain that solo and the send's level set;
//! 2. its device chain;
//! 3. its [`TrackMixer`]: pan, volume, and the Track Activator, then the
//!    track's meters.
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
use std::f64::consts::{FRAC_PI_4, SQRT_2};

use bl_core::time::SampleRate;
use bl_model::{Document, Mixer, ParamId, TrackId, TrackKind};

use crate::node::{Block, Node, NodeConfig, NodeEvent, ParamValue, segments};

/// How long a mixer gain takes to reach a new value: 20 ms, along a straight
/// line from the gain it had (`docs/specs/mixer.md`, R18).
pub const SMOOTHING_SECONDS: f64 = 0.02;

/// How fast a peak meter falls after a peak, in decibels per second: 20 dB
/// in 1.7 seconds, as a broadcast peak programme meter falls back
/// (`docs/specs/mixer.md`, R19).
pub const METER_RELEASE_DB_PER_SECOND: f64 = 20.0 / 1.7;

/// The time constant of the RMS meters' average of the squared signal: 300
/// ms (`docs/specs/mixer.md`, R19).
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

/// Frames a gain takes to reach a new value at `rate`.
fn smoothing_frames(rate: SampleRate) -> u32 {
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
    /// `frames` frames. A gain already heading for `target` carries on.
    pub(crate) fn toward(&mut self, target: f64, frames: u32) {
        if target == self.target {
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
    fn next(&mut self) -> f64 {
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

    /// How many of `frames` frames the gain moves through.
    fn moving(&self, frames: usize) -> usize {
        (self.left as usize).min(frames)
    }
}

/// Scales `samples` in place by the gain, frame by frame.
fn scale(samples: &mut [f32], ramp: &mut Ramp) {
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

    fn measure(&mut self, channel: usize, samples: &[f32]) {
        let (mut peak, mut square) = (self.peak[channel], self.square[channel]);
        for &sample in samples {
            let sample = f64::from(sample);
            peak = (peak * self.fall).max(sample.abs());
            square += (sample * sample - square) * self.smooth;
        }
        self.peak[channel] = peak;
        self.square[channel] = square;
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

/// A track's mixer: pan, volume, and the Track Activator, applied in place
/// after the track's device chain, then the track's meters. It takes the
/// [`track`] parameters.
///
/// The left channel takes the pan law's left gain ([`pan_gains`]) and the
/// right channel its right gain; in an engine with other than two channels,
/// channels past the first two (or a mono engine's one) take the volume
/// alone. Each channel's gain glides to every new value
/// ([`SMOOTHING_SECONDS`]). The meters measure the result: after pan,
/// volume, and the activator, before solo.
#[derive(Debug, Clone)]
pub struct TrackMixer {
    volume: f64,
    pan: f64,
    mute: bool,
    /// The left gain, the right gain, and the gain of any other channel.
    gains: [Ramp; 3],
    smoothing: u32,
    meters: Meters,
}

impl TrackMixer {
    /// A mixer set to a track's mixer settings, resting at them.
    #[must_use]
    pub fn new(mixer: &Mixer, config: &NodeConfig) -> Self {
        let mut node = Self {
            volume: 1.0,
            pan: 0.0,
            mute: false,
            gains: [Ramp::at(1.0); 3],
            smoothing: smoothing_frames(config.sample_rate),
            meters: Meters::new(config),
        };
        node.set(track::VOLUME, ParamValue::Set(mixer.volume));
        node.set(track::PAN, ParamValue::Set(mixer.pan));
        node.set(
            track::MUTE,
            ParamValue::Set(f64::from(u8::from(mixer.mute))),
        );
        node.gains = node.targets().map(Ramp::at);
        node
    }

    /// The gains the mixer is heading for: left, right, and other channels.
    fn targets(&self) -> [f64; 3] {
        let level = if self.mute { 0.0 } else { self.volume };
        let [left, right] = pan_gains(self.pan);
        [level * left, level * right, level]
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
            (track::MUTE, ParamValue::Set(mute)) => self.mute = mute != 0.0,
            (track::MUTE, ParamValue::Default) => self.mute = defaults.mute,
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
    }
}

/// Which of a track mixer's gains channel `c` of `channels` takes.
fn gain_index(c: usize, channels: usize) -> usize {
    if channels >= 2 && c < 2 { c } else { 2 }
}

impl Node for TrackMixer {
    fn process(&mut self, block: &mut Block<'_>) {
        let channels = block.channels();
        for segment in segments(block.events(), block.frames()) {
            self.apply(segment.events);
            let mut after = self.gains;
            for c in 0..channels {
                let index = gain_index(c, channels);
                let mut ramp = self.gains[index];
                let samples = &mut block.channel_mut(c)[segment.range.clone()];
                scale(samples, &mut ramp);
                after[index] = ramp;
                if c < 2 {
                    self.meters.measure(c, samples);
                }
            }
            self.gains = after;
        }
    }

    fn flush(&mut self, events: &[NodeEvent]) {
        self.apply(events);
    }
}

/// A route into a track's input: another track's output into its group or
/// Main, or a send into a return. It adds the other track's signal, scaled
/// by its gain, and takes the [`route`] parameters: the gain is the level
/// (a send's; an output's is 1) unless solo silences the route, and it
/// glides to every new value ([`SMOOTHING_SECONDS`]).
#[derive(Debug, Clone)]
pub struct Route {
    level: f64,
    solo_mute: bool,
    gain: Ramp,
    smoothing: u32,
}

impl Route {
    /// A route at `level`, silenced by solo or not, resting there.
    #[must_use]
    pub fn new(level: f64, solo_mute: bool, config: &NodeConfig) -> Self {
        let mut route = Self {
            level: 0.0,
            solo_mute,
            gain: Ramp::at(0.0),
            smoothing: smoothing_frames(config.sample_rate),
        };
        route.set(route::LEVEL, ParamValue::Set(level));
        route.gain = Ramp::at(route.target());
        route
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
        if self.solo_mute { 0.0 } else { self.level }
    }

    fn set(&mut self, param: ParamId, value: ParamValue) {
        match (param, value) {
            (route::LEVEL, ParamValue::Set(level)) if level.is_finite() => {
                self.level = level.max(0.0);
            }
            (route::LEVEL, _) => self.level = 1.0,
            (route::SOLO_MUTE, ParamValue::Set(mute)) => self.solo_mute = mute != 0.0,
            (route::SOLO_MUTE, ParamValue::Default) => self.solo_mute = false,
            _ => {}
        }
    }

    /// Adds `frames` frames of `from` into `to`, both planar with
    /// `channels` channels `stride` apart, scaled by the route's gain, with
    /// `events` taking effect on their frames. Real-time safe.
    pub(crate) fn mix(
        &mut self,
        to: &mut [f32],
        from: &[f32],
        channels: usize,
        stride: usize,
        frames: usize,
        events: &[NodeEvent],
    ) {
        if events.is_empty() && self.is_silent() {
            return;
        }
        for segment in segments(events, frames) {
            if !segment.events.is_empty() {
                for event in segment.events {
                    self.set(event.param, event.value);
                }
                self.gain.toward(self.target(), self.smoothing);
            }
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

/// Who solo silences, for a document's solo switches and its Solo in Place
/// setting (`docs/specs/mixer.md`, R10 to R14).
///
/// While no track is soloed, solo silences nothing. Otherwise the *solo
/// path* is every soloed track, every group that holds one (at any depth),
/// and every track inside a soloed group: the tracks that feed a soloed
/// track or that it feeds. Then:
///
/// - an audio, MIDI, or group track outside the path is silent: its output
///   and its sends;
/// - a return track that isn't soloed is heard only if Solo in Place is on
///   and no return is soloed;
/// - a send from a track outside the path still feeds a soloed return, so a
///   soloed return hears everything sent to it;
/// - Main is always heard.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Solo {
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
        let returns: BTreeSet<TrackId> = document.returns().collect();
        let mut solo = Self {
            main: document.main_id(),
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
            let mut parent = document.parent(id);
            while let Some(group) = parent {
                solo.path.insert(group);
                solo.holding.insert(group);
                parent = document.parent(group);
            }
            let mut inside: Vec<TrackId> = track.children().collect();
            while let Some(child) = inside.pop() {
                solo.path.insert(child);
                if let Some(child) = document.track(child) {
                    inside.extend(child.children());
                }
            }
        }
        solo
    }

    /// Whether any track is soloed.
    #[must_use]
    pub fn is_active(&self) -> bool {
        !self.soloed.is_empty()
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

    /// Whether solo silences `track`'s output to its group or Main.
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

    /// Whether `track` would be heard on Main with every gain above 0: its
    /// own output and, for tracks in groups, each group's output up to Main.
    /// A track that's silent here can still be heard through its sends.
    #[must_use]
    pub fn heard_directly(&self, document: &Document, track: TrackId) -> bool {
        let mut at = Some(track);
        while let Some(id) = at {
            if self.output_muted(id) {
                return false;
            }
            at = document.parent(id);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::TransportInfo;

    fn config() -> NodeConfig {
        NodeConfig {
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
        let mut node = TrackMixer::new(&Mixer::default(), &config());
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
        let mut node = TrackMixer::new(&Mixer::default(), &config());
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
        let mut node = TrackMixer::new(&mixer, &config());
        assert_eq!(node.gains(), [0.5 * SQRT_2, 0.0]);
        let out = render(&mut node, [1.0, 1.0], 64, &[]);
        assert!(out[..64].iter().all(|&s| s == (0.5 * SQRT_2) as f32));
        assert!(out[64..].iter().all(|&s| s == 0.0));
        mixer.mute = true;
        let node = TrackMixer::new(&mixer, &config());
        assert_eq!(node.gains(), [0.0, 0.0]);
    }

    // Spec: mixer R19
    #[test]
    fn meters_show_peaks_at_once_and_rms_as_a_slow_average() {
        let mut node = TrackMixer::new(&Mixer::default(), &config());
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
        route.mix(&mut to, &from, 2, 1000, 1000, &[]);
        assert!(to.iter().all(|&s| s == 0.75));

        let events = [NodeEvent {
            offset: 10,
            param: route::SOLO_MUTE,
            value: ParamValue::Set(1.0),
        }];
        let mut to = vec![0.0f32; 2 * 1000];
        route.mix(&mut to, &from, 2, 1000, 1000, &events);
        assert!(to[..10].iter().all(|&s| s == 0.25));
        assert!(to[10..970].windows(2).all(|w| w[1] < w[0]), "a glide down");
        assert!(to[969..1000].iter().all(|&s| s == 0.0));
        assert_eq!(to[..1000], to[1000..]);
        assert!(route.is_silent());

        // A silent send adds nothing, and doesn't look at its input.
        let mut send = Route::new(0.0, false, &config);
        let mut to = vec![0.5f32; 2 * 8];
        send.mix(&mut to, &[1.0; 16], 2, 8, 8, &[]);
        assert!(to.iter().all(|&s| s == 0.5));
    }
}

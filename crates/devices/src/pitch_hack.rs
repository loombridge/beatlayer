//! Pitch Hack (WP-8.51; the pitch-hack spec): a delay whose echoes leave
//! transposed, each echo reversed or given a random interval by chance, with
//! the transposed echoes fed back into the line.
//!
//! The line is Shifter's channel ([`Chan`]): its input history is the line,
//! and its delay-line pitch shifting reads the echo at the delay time, so
//! the shifting is the Shifter's, called. A reversed echo reads the same
//! history backwards instead. Echoes are counted from the song's start, one
//! every delay time, and an echo's draws are a function of the seed and its
//! count, not a stream, so a render repeats run after run, at any block size
//! and from any start point.

use bl_core::time::{Frames, TICKS_PER_QUARTER, Ticks};
use bl_dsp::Random;
use bl_engine::{DeviceType, NodeConfig, Registry};
use bl_model::device_layout::{BODY_HEIGHT, ControlBand, LayoutNode, Widget};
use bl_model::{
    Device, DeviceDescription, DeviceLayout, DeviceRole, ParamDescriptor, ParamId, ParamUnit,
};

use crate::framework::{Audio, NativeDevice, ParamBank, add};
use crate::shifter::{Chan, DELAY_DIVISIONS};

/// Pitch Hack's device type.
pub const PITCH_HACK: &str = "beatlayer.pitch_hack";

/// Time: the free delay time, in milliseconds.
pub const TIME: ParamId = ParamId(0);
/// Sync: beat divisions on, free milliseconds off.
pub const SYNC: ParamId = ParamId(1);
/// Transpose: each echo's shift, in semitones.
pub const TRANSPOSE: ParamId = ParamId(2);
/// Cents: the shift's fine part.
pub const CENTS: ParamId = ParamId(3);
/// Reverse: the chance an echo plays backwards.
pub const REVERSE: ParamId = ParamId(4);
/// Random: the chance an echo takes a random interval.
pub const RANDOM: ParamId = ParamId(5);
/// Feedback: how much of the echoes returns to the line.
pub const FEEDBACK: ParamId = ParamId(6);
/// Dry/Wet: the dry signal against the echoes.
pub const DRY_WET: ParamId = ParamId(7);
/// Division: the synced delay time, one of [`DELAY_DIVISIONS`].
pub const DIVISION: ParamId = ParamId(8);
/// Input: the gain of the signal entering the line, down to silence.
pub const INPUT: ParamId = ParamId(9);

/// How many parameters it has.
const PARAMS: usize = 10;

/// The intervals Random draws from, in semitones.
pub const INTERVALS: [f64; 7] = [-12.0, -7.0, -5.0, 0.0, 5.0, 7.0, 12.0];
/// The shifting window, in seconds.
const WINDOW_SECONDS: f64 = 0.030;
/// How long a reversed echo takes to fade in and out, in seconds.
const FADE_SECONDS: f64 = 0.005;
/// How long the shifting takes to move to the plain echo as the shift
/// reaches nothing, in seconds.
const BYPASS_SECONDS: f64 = 0.010;
/// The longest free time, in seconds.
const MAX_FREE_SECONDS: f64 = 2.0;
/// The longest synced time, in seconds: a bar of four quarter notes at 20
/// BPM, the slowest tempo the transport allows.
const MAX_SYNC_SECONDS: f64 = 12.0;
/// The loop's gain at Feedback 100 %: the loop stays below one.
const MAX_LOOP_GAIN: f64 = 0.98;
/// The shortest time the line is read at, in frames.
const MIN_FRAMES: f64 = 16.0;

/// Pitch Hack's type: Live's name and parameters, Beatlayer's own layout
/// (the pitch-hack spec).
#[must_use]
pub fn pitch_hack_type() -> DeviceType {
    let names: Vec<&str> = DELAY_DIVISIONS.iter().map(|(name, _)| *name).collect();
    let params = vec![
        ParamDescriptor::continuous(
            TIME,
            "Time",
            (10.0, 2_000.0),
            250.0,
            ParamUnit::Milliseconds,
        )
        .logarithmic()
        .describe("The delay time in milliseconds while Sync is off."),
        ParamDescriptor::toggle(SYNC, "Sync", true)
            .describe("Sets the delay time in beat divisions rather than milliseconds."),
        ParamDescriptor::integer(TRANSPOSE, "Transpose", (-24, 24), 7, ParamUnit::Semitones)
            .describe("How far each echo is transposed, in semitones."),
        ParamDescriptor::continuous(CENTS, "Cents", (-50.0, 50.0), 0.0, ParamUnit::Cents)
            .describe("Fine-tunes the transposition, in cents."),
        ParamDescriptor::continuous(REVERSE, "Reverse", (0.0, 100.0), 0.0, ParamUnit::Percent)
            .describe("The chance an echo plays backwards."),
        ParamDescriptor::continuous(RANDOM, "Random", (0.0, 100.0), 0.0, ParamUnit::Percent)
            .describe("The chance an echo takes a random interval instead of Transpose."),
        ParamDescriptor::continuous(FEEDBACK, "Feedback", (0.0, 100.0), 30.0, ParamUnit::Percent)
            .describe(
                "How much of the transposed echoes returns to the line, to be transposed again.",
            ),
        ParamDescriptor::continuous(DRY_WET, "Dry/Wet", (0.0, 100.0), 50.0, ParamUnit::Percent)
            .describe("The balance between the dry signal and the echoes."),
        ParamDescriptor::choice(DIVISION, "Division", &names, 5)
            .describe("The delay time in beat divisions while Sync is on."),
        ParamDescriptor::gain(INPUT, "Input", 12.0, 0.0)
            .describe("The level of the signal entering the line, down to silence."),
    ];
    DeviceType::native(PITCH_HACK, "Pitch Hack", DeviceRole::AudioEffect).with_description(
        DeviceDescription::new(params)
            .with_layout(layout())
            .with_banks(&[
                (
                    "Main",
                    &[
                        "Time",
                        "Sync",
                        "Transpose",
                        "Cents",
                        "Reverse",
                        "Random",
                        "Feedback",
                        "Dry/Wet",
                    ],
                ),
                ("Input", &["Division", "Input"]),
            ]),
    )
}

fn layout() -> DeviceLayout {
    use Widget::{Chooser, Knob, Number, Toggle};
    let band = |controls: &[(ParamId, Widget)]| {
        LayoutNode::Band(ControlBand::stacked(BODY_HEIGHT, controls))
    };
    DeviceLayout::new(LayoutNode::Row(vec![
        band(&[(SYNC, Toggle), (DIVISION, Chooser), (TIME, Knob)]),
        band(&[(TRANSPOSE, Number), (CENTS, Knob)]),
        band(&[(REVERSE, Knob), (RANDOM, Knob)]),
        band(&[(INPUT, Knob), (FEEDBACK, Knob), (DRY_WET, Knob)]),
    ]))
}

/// Adds Pitch Hack to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, pitch_hack_type(), Hack::new);
}

/// What one echo does, drawn when it begins.
#[derive(Clone, Copy)]
struct Echo {
    reversed: bool,
    /// The interval it takes in place of Transpose and Cents, in semitones.
    interval: Option<f64>,
}

/// Echo `echo`'s three draws from 0 to 1, from `seed`: whether it is
/// reversed, whether it takes a random interval, and which.
#[must_use]
pub fn draws(seed: u64, echo: i64) -> [f64; 3] {
    let mut random = Random::seeded(
        seed.wrapping_mul(0x9E37_79B9_7F4A_7C15)
            ^ (echo as u64).wrapping_mul(0xBF58_476D_1CE4_E5B9),
    );
    core::array::from_fn(|_| (random.next_bipolar() + 1.0) / 2.0)
}

/// Pitch Hack: a shifting line a channel, and the echoes' draws.
struct Hack {
    lines: Vec<Chan>,
    seed: u64,
    /// The echo playing, counted from the song's start.
    count: i64,
    /// The song frame the next block should start on if the song plays on.
    expected: i64,
    /// Frames played while the song is stopped, which carry the clock on.
    idle: i64,
    /// The longest the line is read at, in frames.
    longest: f64,
    /// The echo playing, how far into it the device is, and its length.
    echo: Echo,
    phase: f64,
    length: f64,
}

impl Hack {
    fn new(device: &Device, config: &NodeConfig) -> Self {
        let rate = config.sample_rate.as_f64();
        let longest = (MAX_SYNC_SECONDS.max(MAX_FREE_SECONDS) * rate).ceil();
        // A channel's history is six times its latency: it holds a reversed
        // echo of the longest time even at +24 semitones, which reads
        // through it five times as fast as it is written.
        Self {
            lines: (0..config.channels)
                .map(|_| Chan::new(rate, longest as usize, 4))
                .collect(),
            seed: device.id.0.get(),
            count: 0,
            expected: i64::MIN,
            idle: 0,
            longest,
            echo: Echo {
                reversed: false,
                interval: None,
            },
            phase: 0.0,
            length: 0.0,
        }
    }

    /// What the echo counted `self.count` does.
    fn draw(&self, reverse: f64, random: f64) -> Echo {
        let [reversed, takes, pick] = draws(self.seed, self.count);
        Echo {
            reversed: reversed * 100.0 < reverse,
            interval: (takes * 100.0 < random).then(|| {
                INTERVALS[((pick * INTERVALS.len() as f64) as usize).min(INTERVALS.len() - 1)]
            }),
        }
    }
}

impl NativeDevice for Hack {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        let frames = audio.frames();
        if frames == 0 {
            return;
        }
        let rate = audio.sample_rate().as_f64();
        let ix: [usize; PARAMS] = core::array::from_fn(|id| {
            params
                .index(ParamId(id as u32))
                .expect("the description lists every parameter")
        });
        let ramp = |id: ParamId| params.ramp(ix[id.0 as usize]);
        let value = |id: ParamId| params.value(ix[id.0 as usize]);
        let synced = value(SYNC) >= 0.5;
        let beats = DELAY_DIVISIONS[(value(DIVISION) as usize).min(DELAY_DIVISIONS.len() - 1)].1;
        let (time, input) = (ramp(TIME), ramp(INPUT));
        let (transpose, cents) = (ramp(TRANSPOSE), ramp(CENTS));
        let (reverse, random) = (ramp(REVERSE), ramp(RANDOM));
        let (feedback, dry_wet) = (ramp(FEEDBACK), ramp(DRY_WET));
        let fade = 1.0 / (BYPASS_SECONDS * rate);
        let longest = self.longest;
        let delay_at = |i: usize, bpm: f64| {
            if synced {
                beats * 60.0 / bpm * rate
            } else {
                f64::from(time[i]) * rate / 1_000.0
            }
            .clamp(MIN_FRAMES, longest)
        };
        let playing = audio.playing();
        let base = audio.song_position().get() + if playing { 0 } else { self.idle };
        if base != self.expected {
            // The song started or jumped: find the echo that holds `base`,
            // on the song's grid while synced.
            let delay = delay_at(0, audio.bpm_at(0));
            let start = if synced {
                let span = (beats * TICKS_PER_QUARTER as f64).round().max(1.0) as i64;
                let ticks = audio
                    .tempo()
                    .frames_to_ticks(Frames(base), audio.sample_rate());
                self.count = ticks.get().div_euclid(span);
                let from = Ticks(self.count * span);
                audio
                    .tempo()
                    .ticks_to_frames(from, audio.sample_rate())
                    .get() as f64
            } else {
                self.count = (base as f64 / delay).floor() as i64;
                self.count as f64 * delay
            };
            self.phase = base as f64 - start;
            self.length = delay;
            self.echo = self.draw(f64::from(reverse[0]), f64::from(random[0]));
        }
        self.expected = base + frames as i64;
        self.idle = if playing {
            0
        } else {
            self.idle + frames as i64
        };
        for i in 0..frames {
            let delay = delay_at(i, audio.bpm_at(i));
            while self.phase >= self.length {
                self.phase -= self.length;
                self.length = delay;
                self.count += 1;
                self.echo = self.draw(f64::from(reverse[i]), f64::from(random[i]));
            }
            let (echo, phase, length) = (self.echo, self.phase, self.length);
            self.phase += 1.0;
            let semitones = echo
                .interval
                .unwrap_or_else(|| f64::from(transpose[i]) + f64::from(cents[i]) / 100.0);
            // One frame less: the echo is read before this frame's input
            // goes in. shortcut: the time is whole frames, so a tempo ramp steps
            // it by a frame now and then; upgrade with a fractional read.
            let latency = (delay - 1.0).round() as usize;
            let window = (WINDOW_SECONDS * rate).min(2.0 * latency as f64 - 8.0);
            // A reversed echo fades in and out at its ends, through the
            // forward echo, so neither jump is heard.
            let edge = (FADE_SECONDS * rate).min(length / 2.0);
            let weight = if echo.reversed {
                let span = (phase / edge).min((length - 1.0 - phase) / edge);
                (std::f64::consts::FRAC_PI_2 * span.clamp(0.0, 1.0))
                    .sin()
                    .powi(2)
            } else {
                0.0
            };
            let backwards = (1.0 + (semitones / 12.0).exp2()) * phase;
            let loop_gain = f64::from(feedback[i]) / 100.0 * MAX_LOOP_GAIN;
            let wet = f64::from(dry_wet[i]) / 100.0;
            for (channel, line) in self.lines.iter_mut().enumerate().take(audio.channels()) {
                let dry = f64::from(audio.channel_mut(channel)[i]);
                let forward = line.pitch(semitones, window, latency, fade);
                let echoed = if weight > 0.0 {
                    forward * (1.0 - weight) + line.read(backwards) * weight
                } else {
                    forward
                };
                line.push((f64::from(input[i]) * dry + loop_gain * echoed) as f32);
                audio.channel_mut(channel)[i] = (dry * (1.0 - wet) + echoed * wet) as f32;
            }
        }
    }
}

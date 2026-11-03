//! Vector Delay, a multi-tap delay whose taps move as particles (WP-8.52;
//! the Vector Delay spec): up to eight particles move in a unit square under
//! gravity, a magnet and a flowing field, and each one's place sets its
//! tap: its horizontal place the delay time, its vertical place the level.
//!
//! The forces are worked out every 32 frames from the frame the device last
//! restarted on, never from where the blocks fall, so a render is the same
//! at any block size. A tap's delay time and level move in a straight line
//! from one grid point's value to the next, so a moving tap's echo is
//! pitched by the Doppler law (`1 − r`), read through a windowed sinc.

use core::f64::consts::TAU;

use bl_dsp::Random;
use bl_engine::{DeviceType, NodeConfig, Registry};
use bl_model::device_layout::{
    BODY_HEIGHT, ControlBand, DisplayCell, LayoutNode, Page, PageStyle, PagesNode, TAB_HEIGHT,
    Widget,
};
use bl_model::{
    Device, DeviceDescription, DeviceDisplay, DeviceId, DeviceLayout, DeviceRole, DisplayMarker,
    DisplayReading, DisplaySeries, OrdMap, ParamDescriptor, ParamId, ParamUnit,
};

use crate::auto_filter::DIVISIONS;
use crate::delay::sixteenth_frames_at;
use crate::framework::{Audio, NativeDevice, ParamBank, add};
use crate::ring::{Kernel, MIN_DELAY, Ring, limit};

/// Vector Delay's device type.
pub const VECTOR_DELAY: &str = "beatlayer.vector_delay";

/// Taps: how many particles sound, 1 to 8.
pub const TAPS: ParamId = ParamId(0);
/// Sync: Max Time in beat divisions on, free milliseconds off.
pub const SYNC: ParamId = ParamId(1);
/// Max Time: the delay a particle at the right edge gives, in
/// milliseconds, while Sync is off.
pub const MAX_TIME: ParamId = ParamId(2);
/// Division: Max Time while Sync is on, one of [`division_names`].
pub const DIVISION: ParamId = ParamId(3);
/// Gravity: pulls the particles down, or up when negative, in per cent.
pub const GRAVITY: ParamId = ParamId(4);
/// Magnet: pulls them toward the centre, or pushes them away.
pub const MAGNET: ParamId = ParamId(5);
/// Flow: moves them along a slowly turning field of noise.
pub const FLOW: ParamId = ParamId(6);
/// Speed: scales time for the motion, in per cent.
pub const SPEED: ParamId = ParamId(7);
/// Friction: damps the particles' velocity.
pub const FRICTION: ParamId = ParamId(8);
/// Spread: how far the taps pan apart, alternately left and right.
pub const SPREAD: ParamId = ParamId(9);
/// Feedback: how much of the taps' sum returns to the line's input.
pub const FEEDBACK: ParamId = ParamId(10);
/// Input: the gain of what the line records.
pub const INPUT: ParamId = ParamId(11);
/// Dry/Wet: the balance between the dry signal and the taps.
pub const DRY_WET: ParamId = ParamId(12);
/// Reset: a rising edge puts the particles back to their starting places.
pub const RESET: ParamId = ParamId(13);

/// How many particles there are.
pub const PARTICLES: usize = 8;

/// Tap `n`'s starting place, the horizontal one: parameter `14 + 2n`
/// (`n` from 0).
#[must_use]
pub const fn tap_x(n: usize) -> ParamId {
    ParamId(14 + 2 * n as u32)
}

/// Tap `n`'s starting place, the vertical one.
#[must_use]
pub const fn tap_y(n: usize) -> ParamId {
    ParamId(15 + 2 * n as u32)
}

/// How many parameters the device has.
const PARAMS: usize = 14 + 2 * PARTICLES;

/// The frames between two grid points, where the forces are worked out.
pub const GRID: u64 = 32;
/// The longest line, in seconds.
const LINE_SECONDS: f64 = 4.0;
/// How long a tap takes to fade in or out as Taps changes, in seconds.
const FADE_SECONDS: f64 = 0.005;
/// What each force does at 100 %: Gravity and Flow accelerate a particle in
/// places per second per second, Magnet in places per second per second per
/// place from the centre, and Friction damps by this many per second.
const GRAVITY_ACCEL: f64 = 2.0;
const MAGNET_ACCEL: f64 = 6.0;
const FLOW_ACCEL: f64 = 3.0;
const FRICTION_RATE: f64 = 5.0;

/// How many floats the display data holds: the number of taps, then for each
/// particle its place, its velocity, its tap's delay time in seconds and its
/// level.
pub const DISPLAY_LEN: usize = 1 + 6 * PARTICLES;

/// Max Time's divisions while Sync is on, 1/16 up to 2 bars.
fn divisions() -> &'static [(&'static str, f64)] {
    &DIVISIONS[2..=14]
}

/// The names of Max Time's divisions, in the chooser's order.
#[must_use]
pub fn division_names() -> Vec<&'static str> {
    divisions().iter().map(|d| d.0).collect()
}

/// Where particle `n` starts, evenly spaced along the falling diagonal.
fn default_place(n: usize) -> f64 {
    (n + 1) as f64 / (PARTICLES + 1) as f64
}

/// Vector Delay's type: Beatlayer's own look, with Live's name.
#[must_use]
pub fn vector_delay_type() -> DeviceType {
    let percent = |id, name: &str, range, default, text: &str| {
        ParamDescriptor::continuous(id, name, range, default, ParamUnit::Percent).describe(text)
    };
    let mut params = vec![
        ParamDescriptor::integer(TAPS, "Taps", (1, PARTICLES as i64), 4, ParamUnit::None)
            .describe("How many particles, each one a tap of the delay."),
        ParamDescriptor::toggle(SYNC, "Sync", false)
            .describe("Sets Max Time as a beat division rather than milliseconds."),
        ParamDescriptor::continuous(
            MAX_TIME,
            "Max Time",
            (50.0, LINE_SECONDS * 1_000.0),
            1_000.0,
            ParamUnit::Milliseconds,
        )
        .logarithmic()
        .describe("The delay of a particle at the right edge; a particle's delay is this times its place across."),
        ParamDescriptor::choice(DIVISION, "Division", &division_names(), 5)
            .describe("Max Time as a beat division while Sync is on."),
        percent(
            GRAVITY,
            "Gravity",
            (-100.0, 100.0),
            0.0,
            "Pulls the particles down, which lowers their levels, or up when negative.",
        ),
        percent(
            MAGNET,
            "Magnet",
            (-100.0, 100.0),
            0.0,
            "Pulls the particles toward the centre, or pushes them away when negative.",
        ),
        percent(
            FLOW,
            "Flow",
            (0.0, 100.0),
            0.0,
            "Moves the particles along a slowly turning field of noise.",
        ),
        percent(
            SPEED,
            "Speed",
            (0.0, 400.0),
            100.0,
            "How fast time runs for the particles' motion.",
        ),
        percent(
            FRICTION,
            "Friction",
            (0.0, 100.0),
            20.0,
            "Slows the particles down.",
        ),
        percent(
            SPREAD,
            "Spread",
            (0.0, 100.0),
            50.0,
            "How far apart the taps pan, alternately left and right.",
        ),
        percent(
            FEEDBACK,
            "Feedback",
            (0.0, 100.0),
            30.0,
            "How much of the taps' sum returns to the line's input.",
        ),
        ParamDescriptor::gain(INPUT, "Input", 12.0, 0.0)
            .describe("The level of what the line records, down to silence."),
        percent(
            DRY_WET,
            "Dry/Wet",
            (0.0, 100.0),
            50.0,
            "The balance between the dry signal and the taps.",
        ),
        ParamDescriptor::toggle(RESET, "Reset", false)
            .describe("Puts the particles back to their starting places, at rest."),
    ];
    for n in 0..PARTICLES {
        let tap = n + 1;
        for (id, axis, what) in [(tap_x(n), "X", "across"), (tap_y(n), "Y", "up")] {
            params.push(
                ParamDescriptor::continuous(
                    id,
                    &format!("Tap {tap} {axis}"),
                    (0.0, 1.0),
                    if axis == "X" {
                        default_place(n)
                    } else {
                        1.0 - default_place(n)
                    },
                    ParamUnit::None,
                )
                .decimals(3)
                .describe(&format!(
                    "Where particle {tap} starts, {what} the square: across sets its delay, up its level."
                )),
            );
        }
    }
    DeviceType::native(VECTOR_DELAY, "Vector Delay", DeviceRole::AudioEffect).with_description(
        DeviceDescription::new(params)
            .with_layout(layout())
            .with_banks(&[
                (
                    "Main",
                    &[
                        "Taps", "Max Time", "Gravity", "Magnet", "Flow", "Speed", "Feedback",
                        "Dry/Wet",
                    ],
                ),
                (
                    "Motion",
                    &["Friction", "Spread", "Input", "Reset", "Sync", "Division"],
                ),
                (
                    "Places",
                    &[
                        "Tap 1 X", "Tap 1 Y", "Tap 2 X", "Tap 2 Y", "Tap 3 X", "Tap 3 Y",
                        "Tap 4 X", "Tap 4 Y",
                    ],
                ),
                (
                    "Places 2",
                    &[
                        "Tap 5 X", "Tap 5 Y", "Tap 6 X", "Tap 6 Y", "Tap 7 X", "Tap 7 Y",
                        "Tap 8 X", "Tap 8 Y",
                    ],
                ),
            ]),
    )
}

fn layout() -> DeviceLayout {
    let height = BODY_HEIGHT - TAB_HEIGHT;
    let left = ControlBand::stacked(
        height,
        &[
            (TAPS, Widget::Number),
            (SYNC, Widget::Toggle),
            (DIVISION, Widget::Chooser),
            (MAX_TIME, Widget::Knob),
            (SPEED, Widget::Knob),
            (FRICTION, Widget::Knob),
        ],
    );
    let right = ControlBand::stacked(
        height,
        &[
            (GRAVITY, Widget::Knob),
            (MAGNET, Widget::Knob),
            (FLOW, Widget::Knob),
            (SPREAD, Widget::Knob),
            (FEEDBACK, Widget::Knob),
            (INPUT, Widget::Knob),
            (DRY_WET, Widget::Knob),
            (RESET, Widget::Toggle),
        ],
    );
    let places: Vec<(ParamId, Widget)> = (0..PARTICLES)
        .flat_map(|n| [(tap_x(n), Widget::Number), (tap_y(n), Widget::Number)])
        .collect();
    DeviceLayout::new(LayoutNode::Pages(PagesNode::new(
        "Pages",
        PageStyle::Tabs,
        vec![
            Page::new(
                "Particles",
                LayoutNode::Row(vec![
                    LayoutNode::Band(left),
                    LayoutNode::Cell(DisplayCell::new("Particle Field", 160.0, height, 1.0)),
                    LayoutNode::Band(right),
                ]),
            ),
            Page::new(
                "Places",
                LayoutNode::Band(ControlBand::stacked(height, &places)),
            ),
        ],
    )))
}

/// Adds Vector Delay to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, vector_delay_type(), VectorDelay::new);
}

/// One particle: where it is and how fast it moves, in places and places
/// per second.
#[derive(Clone, Copy, Default)]
struct Particle {
    place: [f64; 2],
    velocity: [f64; 2],
}

struct VectorDelay {
    kernel: Kernel,
    lines: Vec<Ring>,
    particles: [Particle; PARTICLES],
    /// The starting places the particles were last put at, so a change in
    /// the parameters puts a particle at its new place.
    applied: [[f64; 2]; PARTICLES],
    /// Each tap's delay in frames and level, at the last grid point and at
    /// the next, between which they move in a straight line.
    delay: [[f64; 2]; PARTICLES],
    level: [[f64; 2]; PARTICLES],
    /// How much each tap sounds, from 0 to 1, fading as Taps changes.
    heard: [f64; PARTICLES],
    /// Frames since the particles were last put back, which the grid counts
    /// from, and the seconds of motion since, which the flow follows.
    since: u64,
    time: f64,
    /// The flow field's phases, from the device's seed.
    flow: [f64; 4],
    began: bool,
    /// Whether the next grid point starts every tap where it stands, rather
    /// than moving it there.
    fresh: bool,
    was_reset: bool,
    was_playing: bool,
    shown: [f32; DISPLAY_LEN],
}

impl VectorDelay {
    fn new(device: &Device, config: &NodeConfig) -> Self {
        let mut random = Random::seeded(device.id.0.get());
        let len = (LINE_SECONDS * config.sample_rate.as_f64()).ceil() as usize + 64;
        Self {
            kernel: Kernel::new(),
            lines: (0..config.channels.min(2))
                .map(|_| Ring::new(len))
                .collect(),
            particles: [Particle::default(); PARTICLES],
            applied: [[0.0; 2]; PARTICLES],
            delay: [[MIN_DELAY; 2]; PARTICLES],
            level: [[0.0; 2]; PARTICLES],
            heard: [0.0; PARTICLES],
            since: 0,
            time: 0.0,
            flow: core::array::from_fn(|_| (random.next_bipolar() + 1.0) * TAU / 2.0),
            began: false,
            fresh: false,
            was_reset: false,
            was_playing: false,
            shown: [0.0; DISPLAY_LEN],
        }
    }

    /// The flow field's acceleration at `place`, from 0 at a Flow of 0 to
    /// [`FLOW_ACCEL`] at 1.
    fn flow_at(&self, place: [f64; 2], flow: f64) -> [f64; 2] {
        let [x, y] = place;
        let t = self.time;
        let angle = (2.3 * x + 1.7 * y + 0.35 * t + self.flow[0]).sin()
            + (1.9 * x - 2.9 * y - 0.27 * t + self.flow[1]).sin()
            + (1.3 * y + 0.19 * t + self.flow[2]).sin();
        let angle = angle * TAU / 3.0 + self.flow[3];
        [
            flow * FLOW_ACCEL * angle.cos(),
            flow * FLOW_ACCEL * angle.sin(),
        ]
    }

    /// Moves every particle by one grid step of `dt` seconds.
    fn step(&mut self, dt: f64, (gravity, magnet, flow, friction): (f64, f64, f64, f64)) {
        let damping = (-friction * FRICTION_RATE * dt).exp();
        for n in 0..PARTICLES {
            let Particle {
                mut place,
                mut velocity,
            } = self.particles[n];
            let field = self.flow_at(place, flow);
            let acceleration = [
                magnet * MAGNET_ACCEL * (0.5 - place[0]) + field[0],
                -gravity * GRAVITY_ACCEL + magnet * MAGNET_ACCEL * (0.5 - place[1]) + field[1],
            ];
            for axis in 0..2 {
                velocity[axis] = (velocity[axis] + acceleration[axis] * dt) * damping;
                place[axis] += velocity[axis] * dt;
                // Bouncing off the walls of the square.
                if place[axis] < 0.0 {
                    place[axis] = -place[axis];
                    velocity[axis] = -velocity[axis];
                } else if place[axis] > 1.0 {
                    place[axis] = 2.0 - place[axis];
                    velocity[axis] = -velocity[axis];
                }
                place[axis] = place[axis].clamp(0.0, 1.0);
            }
            self.particles[n] = Particle { place, velocity };
        }
        self.time += dt;
    }

    /// Publishes the particles' places, velocities and taps for the display.
    fn publish(&mut self, taps: usize, rate: f64) {
        self.shown[0] = taps as f32;
        for n in 0..PARTICLES {
            let p = self.particles[n];
            self.shown[1 + 6 * n..7 + 6 * n].copy_from_slice(&[
                p.place[0] as f32,
                p.place[1] as f32,
                p.velocity[0] as f32,
                p.velocity[1] as f32,
                (self.delay[n][1] / rate) as f32,
                self.level[n][1] as f32,
            ]);
        }
    }
}

impl NativeDevice for VectorDelay {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        let rate = audio.sample_rate().as_f64();
        let ix: [usize; PARAMS] = core::array::from_fn(|id| {
            params
                .index(ParamId(id as u32))
                .expect("the description lists every parameter")
        });
        let ramp = |id: ParamId| params.ramp(ix[id.0 as usize]);
        let value = |id: ParamId| params.value(ix[id.0 as usize]);
        let (gravity, magnet, flow) = (ramp(GRAVITY), ramp(MAGNET), ramp(FLOW));
        let (speed, friction, spread) = (ramp(SPEED), ramp(FRICTION), ramp(SPREAD));
        let (feedback, input, dry_wet, reset) =
            (ramp(FEEDBACK), ramp(INPUT), ramp(DRY_WET), ramp(RESET));
        let (synced, division) = (value(SYNC) >= 0.5, value(DIVISION) as usize);
        let max_time = ramp(MAX_TIME);
        let taps = (value(TAPS) as usize).clamp(1, PARTICLES);
        let channels = self.lines.len().min(audio.channels());
        let line_max = self.lines[0].max_delay();
        let fade = 1.0 / (FADE_SECONDS * rate);
        let playing = audio.playing();
        let started = playing && !self.was_playing;
        self.was_playing = playing;
        for i in 0..audio.frames() {
            let pressed = reset[i] >= 0.5;
            let restart = (pressed && !self.was_reset) || (started && i == 0) || !self.began;
            self.was_reset = pressed;
            if restart {
                if !self.began {
                    self.began = true;
                    self.fresh = true;
                    self.heard = core::array::from_fn(|n| f64::from(u8::from(n < taps)));
                }
                self.since = 0;
                self.time = 0.0;
                for n in 0..PARTICLES {
                    let start = [f64::from(ramp(tap_x(n))[i]), f64::from(ramp(tap_y(n))[i])];
                    self.particles[n] = Particle {
                        place: start,
                        velocity: [0.0; 2],
                    };
                    self.applied[n] = start;
                }
            }
            if self.since.is_multiple_of(GRID) {
                if self.since > 0 {
                    let dt = GRID as f64 / rate * f64::from(speed[i]) / 100.0;
                    self.step(
                        dt,
                        (
                            f64::from(gravity[i]) / 100.0,
                            f64::from(magnet[i]) / 100.0,
                            f64::from(flow[i]) / 100.0,
                            f64::from(friction[i]) / 100.0,
                        ),
                    );
                }
                // A starting place set by hand or by automation puts its
                // particle there, at rest.
                for n in 0..PARTICLES {
                    let start = [f64::from(ramp(tap_x(n))[i]), f64::from(ramp(tap_y(n))[i])];
                    if start != self.applied[n] {
                        self.applied[n] = start;
                        self.particles[n] = Particle {
                            place: start,
                            velocity: [0.0; 2],
                        };
                    }
                }
                let longest = if synced {
                    divisions()[division.min(divisions().len() - 1)].1
                        * 4.0
                        * sixteenth_frames_at(audio, i)
                } else {
                    f64::from(max_time[i]) * rate / 1_000.0
                }
                .min(line_max);
                let fresh = core::mem::take(&mut self.fresh);
                for n in 0..PARTICLES {
                    let [x, y] = self.particles[n].place;
                    let to = (x * longest).clamp(MIN_DELAY, line_max);
                    let from = if fresh {
                        (to, y)
                    } else {
                        (self.delay[n][1], self.level[n][1])
                    };
                    self.delay[n] = [from.0, to];
                    self.level[n] = [from.1, y];
                }
                self.publish(taps, rate);
            }
            let progress = (self.since % GRID) as f64 / GRID as f64;
            let mix = dry_wet[i] / 100.0;
            let mut gains = [0.0f64; PARTICLES];
            for (n, gain) in gains.iter_mut().enumerate() {
                let target = f64::from(u8::from(n < taps));
                self.heard[n] += (target - self.heard[n]).clamp(-fade, fade);
                let level = self.level[n][0] + (self.level[n][1] - self.level[n][0]) * progress;
                *gain = self.heard[n] * level.clamp(0.0, 1.0);
            }
            for channel in 0..channels {
                let dry = audio.channel(channel)[i];
                let mut wet = 0.0f32;
                for (n, gain) in gains.iter().enumerate().filter(|(_, g)| **g != 0.0) {
                    let delay = self.delay[n][0] + (self.delay[n][1] - self.delay[n][0]) * progress;
                    // Taps pan alternately left and right by Spread.
                    let pan = f64::from(spread[i]) / 100.0 * if n % 2 == 0 { -1.0 } else { 1.0 };
                    let balance = if channels < 2 {
                        1.0
                    } else if channel == 0 {
                        (1.0 - pan).min(1.0)
                    } else {
                        (1.0 + pan).min(1.0)
                    };
                    wet += (gain * balance) as f32 * self.lines[channel].read(&self.kernel, delay);
                }
                let returned = limit(wet * feedback[i] / 100.0);
                self.lines[channel].write(dry * input[i] + returned);
                audio.channel_mut(channel)[i] = dry * (1.0 - mix) + wet * mix;
            }
            self.since += 1;
        }
    }

    fn display_len(&self) -> usize {
        DISPLAY_LEN
    }

    fn display(&mut self, out: &mut [f32]) {
        out.copy_from_slice(&self.shown);
    }
}

/// What the display shows of Vector Delay: each sounding particle at its
/// place, with its tap's delay time and level. `values` are the device's set
/// values and `data` what its node published, if an engine runs; without
/// it the particles sit at their starting places, at rest.
#[must_use]
pub fn display(
    device: DeviceId,
    description: &DeviceDescription,
    values: &OrdMap<ParamId, f64>,
    data: Option<&[f32]>,
) -> DeviceDisplay {
    let value = |id: ParamId| description.value(values, id).unwrap_or(0.0);
    // The zeros the node starts with, before its first block, hold no taps.
    let (taps, from) = match data.filter(|d| d.len() == DISPLAY_LEN && d[0] >= 1.0) {
        Some(data) => (data[0] as usize, Some(data)),
        None => (value(TAPS) as usize, None),
    };
    let (mut markers, mut readings) = (Vec::new(), Vec::new());
    for n in 0..taps.min(PARTICLES) {
        let (x, y, time, level) = match from {
            Some(data) => {
                let tap = &data[1 + 6 * n..7 + 6 * n];
                (
                    tap[0],
                    tap[1],
                    f64::from(tap[4]) * 1_000.0,
                    f64::from(tap[5]),
                )
            }
            None => {
                let (x, y) = (value(tap_x(n)), value(tap_y(n)));
                (x as f32, y as f32, x * value(MAX_TIME), y)
            }
        };
        let db = 20.0 * level.log10();
        let name = format!("Tap {}", n + 1);
        let (time_text, level_text) = (format!("{time:.0} ms"), format!("{db:.1} dB"));
        markers.push(DisplayMarker {
            name: name.clone(),
            series: "Particles".to_owned(),
            x,
            y,
            text: format!("{name}, {time_text}, {level_text}"),
            handle: None,
        });
        readings.extend([
            DisplayReading {
                name: format!("{name} Time"),
                value: time as f32,
                unit: "ms".to_owned(),
                text: time_text,
            },
            DisplayReading {
                name: format!("{name} Level"),
                value: db as f32,
                unit: "dB".to_owned(),
                text: level_text,
            },
        ]);
    }
    DeviceDisplay {
        device,
        live: from.is_some(),
        view: None,
        series: vec![DisplaySeries {
            name: "Particles".to_owned(),
            unit: String::new(),
            x_unit: String::new(),
            x_range: [0.0, 1.0],
            log_x: false,
            y_range: [0.0, 1.0],
            detail: false,
            values: Vec::new(),
        }],
        markers,
        readings,
        spectrograms: Vec::new(),
    }
}

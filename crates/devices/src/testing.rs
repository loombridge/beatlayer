//! Devices that prove the framework, for tests only. Not for songs: the
//! browser never offers them, and Beatlayer ships none of them.
//!
//! | Type | Name | What it does | Layout |
//! |---|---|---|---|
//! | [`UTILITY`] | Framework Test Utility | gain to −∞ dB, mute, polarity, balance | generic |
//! | [`DRIVE`] | Framework Test Drive | a `tanh` saturator, oversampled 1 to 8 times | generic |
//! | [`SCOPE`] | Framework Test Scope | passes audio through; a display cell between control bands, with a row inside its bottom edge | its own |
//! | [`EFFECTS`] | Framework Test Effects | the effects' shared parts: an LFO tremolo, or the LFO's or the envelope follower's value as the output; a menu option and an expanded part | its own |
//! | [`PAGES`] | Framework Test Pages | passes audio through; tabs of three pages in the body beside a shell of three sections and its display | its own |
//!
//! Between them they have a parameter of every kind: gains, continuous
//! values with linear and logarithmic tapers, toggles, choices, and whole
//! numbers.

use bl_dsp::{EnvelopeFollower, Oversampler};
use bl_engine::{DeviceType, NodeConfig, Registry};
use bl_model::device_layout::{
    BODY_HEIGHT, ControlBand, DisplayCell, LayoutNode, Page, PageStyle, PagesNode, TAB_HEIGHT,
    Widget,
};
use bl_model::{
    Device, DeviceDescription, DeviceLayout, DeviceRole, ParamDescriptor, ParamId, ParamUnit,
};

use crate::framework::{Audio, NativeDevice, ParamBank, add};
use crate::lfo::{Lfo, Rate, Shape, Stereo, Wave};

/// The test utility's device type.
pub const UTILITY: &str = "test.framework.utility";
/// The test drive's device type.
pub const DRIVE: &str = "test.framework.drive";
/// The test scope's device type.
pub const SCOPE: &str = "test.framework.scope";

/// The test effects' device type.
pub const EFFECTS: &str = "test.framework.effects";
/// The test pages' device type.
pub const PAGES: &str = "test.framework.pages";

/// The utility's gain: −∞ to +12 dB, 0 dB by default.
pub const UTILITY_GAIN: ParamId = ParamId(0);
/// The utility's balance, −50 (left) to 50 (right).
pub const UTILITY_BALANCE: ParamId = ParamId(1);
/// The utility's mute.
pub const UTILITY_MUTE: ParamId = ParamId(2);
/// The utility's polarity: Normal or Inverted.
pub const UTILITY_POLARITY: ParamId = ParamId(3);

/// The drive's input gain: −∞ to +36 dB.
pub const DRIVE_DRIVE: ParamId = ParamId(0);
/// The drive's output gain: −∞ to +6 dB.
pub const DRIVE_OUTPUT: ParamId = ParamId(1);
/// The drive's oversampling: Off, 2x, 4x, or 8x.
pub const DRIVE_OVERSAMPLING: ParamId = ParamId(2);

/// The scope's time window, 1 ms to 1 s.
pub const SCOPE_TIME: ParamId = ParamId(0);
/// The scope's display gain.
pub const SCOPE_GAIN: ParamId = ParamId(1);
/// The scope's freeze.
pub const SCOPE_FREEZE: ParamId = ParamId(2);
/// The scope's grid divisions, 1 to 16.
pub const SCOPE_DIVISIONS: ParamId = ParamId(3);
/// The scope's trigger level, −100 to 100 %.
pub const SCOPE_TRIGGER: ParamId = ParamId(4);

/// The effects' output: Audio (a tremolo), LFO (the LFO's value on each
/// channel), or Envelope (the envelope follower's level of the input).
pub const EFFECTS_OUTPUT: ParamId = ParamId(0);
/// The effects' depth, 0 to 100 %.
pub const EFFECTS_DEPTH: ParamId = ParamId(1);
/// The LFO's waveform, one of [`Wave::ALL`].
pub const EFFECTS_WAVE: ParamId = ParamId(2);
/// How the LFO's rate is given: Hz, Seconds or Synced.
pub const EFFECTS_RATE_MODE: ParamId = ParamId(3);
/// The LFO's rate in Hz.
pub const EFFECTS_RATE: ParamId = ParamId(4);
/// The LFO's period in seconds.
pub const EFFECTS_PERIOD: ParamId = ParamId(5);
/// The LFO's synced division, in [`DIVISIONS`].
pub const EFFECTS_DIVISION: ParamId = ParamId(6);
/// How the channels differ: Phase or Spin.
pub const EFFECTS_STEREO: ParamId = ParamId(7);
/// The right LFO's phase against the left, in degrees.
pub const EFFECTS_PHASE: ParamId = ParamId(8);
/// The right LFO's rate against the left, in per cent.
pub const EFFECTS_SPIN: ParamId = ParamId(9);
/// The LFO's phase offset against the beat grid, in degrees.
pub const EFFECTS_OFFSET: ParamId = ParamId(10);
/// The LFO's duty cycle, −100 to 100 %.
pub const EFFECTS_DUTY: ParamId = ParamId(11);
/// How far the LFO's wave is morphed toward the next, 0 to 100 %.
pub const EFFECTS_MORPH: ParamId = ParamId(12);
/// The envelope follower's attack, in milliseconds.
pub const EFFECTS_ATTACK: ParamId = ParamId(13);
/// The envelope follower's release, in milliseconds.
pub const EFFECTS_RELEASE: ParamId = ParamId(14);
/// The envelope follower's hold.
pub const EFFECTS_HOLD: ParamId = ParamId(15);
/// A menu option: the LFO upside down.
pub const EFFECTS_INVERT: ParamId = ParamId(16);

/// The synced divisions' lengths, in quarter notes.
pub const DIVISIONS: [f64; 6] = [0.25, 0.5, 1.0, 2.0, 4.0, 8.0];

/// The test utility's type.
#[must_use]
pub fn utility_type() -> DeviceType {
    DeviceType::native(UTILITY, "Framework Test Utility", DeviceRole::AudioEffect).with_description(
        DeviceDescription::new(vec![
            ParamDescriptor::gain(UTILITY_GAIN, "Gain", 12.0, 0.0)
                .describe("Scales the signal, down to silence."),
            ParamDescriptor::continuous(
                UTILITY_BALANCE,
                "Balance",
                (-50.0, 50.0),
                0.0,
                ParamUnit::None,
            )
            .decimals(0)
            .describe("Moves the signal left or right of the middle."),
            ParamDescriptor::toggle(UTILITY_MUTE, "Mute", false)
                .describe("Turns the signal off without changing the other settings."),
            ParamDescriptor::choice(UTILITY_POLARITY, "Polarity", &["Normal", "Inverted"], 0)
                .describe("Inverts the signal's phase, or leaves it."),
        ]),
    )
}

/// The test drive's type.
#[must_use]
pub fn drive_type() -> DeviceType {
    DeviceType::native(DRIVE, "Framework Test Drive", DeviceRole::AudioEffect).with_description(
        DeviceDescription::new(vec![
            ParamDescriptor::gain(DRIVE_DRIVE, "Drive", 36.0, 0.0)
                .describe("How much the signal is driven into the saturator."),
            ParamDescriptor::gain(DRIVE_OUTPUT, "Output", 6.0, 0.0)
                .describe("Brings the saturated signal back to level."),
            ParamDescriptor::choice(
                DRIVE_OVERSAMPLING,
                "Oversampling",
                &["Off", "2x", "4x", "8x"],
                2,
            )
            .describe("How many times the rate the saturator runs at."),
        ]),
    )
}

/// The test scope's type: a display cell between two control bands, with
/// a row of controls inside the cell's bottom edge.
#[must_use]
pub fn scope_type() -> DeviceType {
    let params = vec![
        ParamDescriptor::continuous(
            SCOPE_TIME,
            "Time",
            (1.0, 1000.0),
            100.0,
            ParamUnit::Milliseconds,
        )
        .logarithmic()
        .describe("How much of the signal the display shows, in milliseconds."),
        ParamDescriptor::gain(SCOPE_GAIN, "Gain", 24.0, 0.0)
            .describe("Raises or lowers what the display shows, without changing the signal."),
        ParamDescriptor::toggle(SCOPE_FREEZE, "Freeze", false)
            .describe("Holds the display still while the audio keeps going."),
        ParamDescriptor::integer(SCOPE_DIVISIONS, "Divisions", (1, 16), 4, ParamUnit::None)
            .describe("How many divisions the display's grid has."),
        ParamDescriptor::continuous(
            SCOPE_TRIGGER,
            "Trigger",
            (-100.0, 100.0),
            0.0,
            ParamUnit::Percent,
        )
        .describe("The level at which the display's window begins."),
    ];
    let layout = DeviceLayout::new(LayoutNode::Row(vec![
        LayoutNode::Band(ControlBand::stacked(
            BODY_HEIGHT,
            &[(SCOPE_TIME, Widget::Knob), (SCOPE_GAIN, Widget::Knob)],
        )),
        LayoutNode::Cell(
            DisplayCell::new("Scope", 160.0, BODY_HEIGHT, 1.0).with_bottom(ControlBand::stacked(
                40.0,
                &[(SCOPE_DIVISIONS, Widget::Number)],
            )),
        ),
        LayoutNode::Band(ControlBand::stacked(
            BODY_HEIGHT,
            &[
                (SCOPE_FREEZE, Widget::Toggle),
                (SCOPE_TRIGGER, Widget::Knob),
            ],
        )),
    ]));
    DeviceType::native(SCOPE, "Framework Test Scope", DeviceRole::AudioEffect)
        .with_description(DeviceDescription::new(params).with_layout(layout))
}

/// The pages node of the test pages' body, in the tab style.
pub const PAGES_TABS: &str = "Body";
/// The pages node of the test pages' shell.
pub const PAGES_SHELL: &str = "Shell";
/// The pages node of the test pages' title-bar tabs.
pub const PAGES_TITLE: &str = "Title";

/// The test pages' type (WP-8.1): three tabs in the body (Osc with
/// parameters 0 and 1, Filter with 2 and 3, Amp with 4 and a display cell),
/// beside a shell of three sections (parameters 5 to 7), each with a page
/// in the display area (parameters 8 to 10), and two pages in tabs in the
/// title bar (parameters 11 and 12).
#[must_use]
pub fn pages_type() -> DeviceType {
    let params: Vec<ParamDescriptor> = (0..13)
        .map(|n| {
            ParamDescriptor::continuous(
                ParamId(n),
                &format!("P{n}"),
                (0.0, 100.0),
                0.0,
                ParamUnit::Percent,
            )
            .describe("A parameter whose control sits on one page.")
        })
        .collect();
    let band = |height, first: u32, count: u32| {
        LayoutNode::Band(ControlBand::stacked(
            height,
            &(first..first + count)
                .map(|n| (ParamId(n), Widget::Knob))
                .collect::<Vec<_>>(),
        ))
    };
    let tab_height = BODY_HEIGHT - TAB_HEIGHT;
    let page = |name, node| Page::new(name, node);
    let section = |name, param, display| {
        page(name, band(BODY_HEIGHT, display, 1)).with_section(band(60.0, param, 1))
    };
    let layout = DeviceLayout::new(LayoutNode::Row(vec![
        LayoutNode::Pages(PagesNode::new(
            PAGES_TABS,
            PageStyle::Tabs,
            vec![
                page("Osc", band(tab_height, 0, 2)),
                page("Filter", band(tab_height, 2, 2)),
                page(
                    "Amp",
                    LayoutNode::Row(vec![
                        band(tab_height, 4, 1),
                        LayoutNode::Cell(DisplayCell::new("Amp Graph", 80.0, tab_height, 1.0)),
                    ]),
                ),
            ],
        )),
        LayoutNode::Pages(PagesNode::new(
            PAGES_SHELL,
            PageStyle::Shell,
            vec![
                section("Env", 5, 8),
                section("LFO", 6, 9),
                section("Out", 7, 10),
            ],
        )),
        LayoutNode::Pages(PagesNode::new(
            PAGES_TITLE,
            PageStyle::TitleBar,
            vec![
                page("Main", band(BODY_HEIGHT, 11, 1)),
                page("Aux", band(BODY_HEIGHT, 12, 1)),
            ],
        )),
    ]));
    DeviceType::native(PAGES, "Framework Test Pages", DeviceRole::AudioEffect)
        .with_description(DeviceDescription::new(params).with_layout(layout))
}

/// The test effects' type: every shared part of the effects (WP-5.42), a
/// menu option and an expanded part.
#[must_use]
pub fn effects_type() -> DeviceType {
    let wave_names: Vec<&str> = Wave::ALL.iter().map(|w| w.name()).collect();
    let params = vec![
        ParamDescriptor::choice(EFFECTS_OUTPUT, "Output", &["Audio", "LFO", "Envelope"], 0)
            .describe("What the device sends out: its input under the LFO, the LFO, or the input's level."),
        ParamDescriptor::continuous(EFFECTS_DEPTH, "Depth", (0.0, 100.0), 100.0, ParamUnit::Percent)
            .describe("How far the LFO moves the audio."),
        ParamDescriptor::choice(EFFECTS_WAVE, "Wave", &wave_names, 0)
            .describe("The LFO's waveform."),
        ParamDescriptor::choice(
            EFFECTS_RATE_MODE,
            "Rate Mode",
            &["Hz", "Seconds", "Synced"],
            0,
        )
        .describe("Whether the LFO's rate is in Hz, in seconds per cycle, or synced to the tempo."),
        ParamDescriptor::continuous(EFFECTS_RATE, "Rate", (0.01, 40.0), 1.0, ParamUnit::Hertz)
            .logarithmic()
            .describe("The LFO's rate in Hz."),
        ParamDescriptor::continuous(EFFECTS_PERIOD, "Period", (0.025, 200.0), 1.0, ParamUnit::Seconds)
            .logarithmic()
            .describe("The LFO's cycle in seconds."),
        ParamDescriptor::choice(
            EFFECTS_DIVISION,
            "Division",
            &["1/16", "1/8", "1/4", "1/2", "1 bar", "2 bars"],
            2,
        )
        .describe("The LFO's cycle, synced to the tempo."),
        ParamDescriptor::choice(EFFECTS_STEREO, "Stereo", &["Phase", "Spin"], 0)
            .describe("Whether the channels' LFOs differ in phase or in rate."),
        ParamDescriptor::continuous(EFFECTS_PHASE, "Phase", (0.0, 360.0), 0.0, ParamUnit::None)
            .decimals(0)
            .describe("How many degrees the right channel's LFO is ahead of the left's."),
        ParamDescriptor::continuous(EFFECTS_SPIN, "Spin", (-50.0, 50.0), 0.0, ParamUnit::Percent)
            .describe("How much faster the right channel's LFO runs than the left's."),
        ParamDescriptor::continuous(EFFECTS_OFFSET, "Offset", (-180.0, 180.0), 0.0, ParamUnit::None)
            .decimals(0)
            .describe("Moves the synced LFO's cycle against the beat grid, in degrees."),
        ParamDescriptor::continuous(EFFECTS_DUTY, "Duty", (-100.0, 100.0), 0.0, ParamUnit::Percent)
            .describe("Squeezes the LFO's wave toward the front or the back of its cycle."),
        ParamDescriptor::continuous(EFFECTS_MORPH, "Morph", (0.0, 100.0), 0.0, ParamUnit::Percent)
            .describe("Blends the LFO's wave toward the next one."),
        ParamDescriptor::continuous(
            EFFECTS_ATTACK,
            "Attack",
            (0.1, 1_000.0),
            10.0,
            ParamUnit::Milliseconds,
        )
        .logarithmic()
        .describe("How fast the envelope follower rises."),
        ParamDescriptor::continuous(
            EFFECTS_RELEASE,
            "Release",
            (1.0, 5_000.0),
            100.0,
            ParamUnit::Milliseconds,
        )
        .logarithmic()
        .describe("How fast the envelope follower falls."),
        ParamDescriptor::toggle(EFFECTS_HOLD, "Hold", false)
            .describe("Lets the envelope follower finish its attack before it releases."),
        ParamDescriptor::toggle(EFFECTS_INVERT, "Invert LFO", false)
            .describe("Turns the LFO upside down."),
    ];
    let layout = DeviceLayout::new(LayoutNode::Band(ControlBand::stacked(
        BODY_HEIGHT,
        &[
            (EFFECTS_OUTPUT, Widget::Chooser),
            (EFFECTS_DEPTH, Widget::Knob),
            (EFFECTS_WAVE, Widget::Chooser),
            (EFFECTS_RATE_MODE, Widget::Chooser),
            (EFFECTS_RATE, Widget::Knob),
            (EFFECTS_PERIOD, Widget::Knob),
            (EFFECTS_DIVISION, Widget::Chooser),
        ],
    )))
    .with_expanded(LayoutNode::Band(ControlBand::stacked(
        BODY_HEIGHT,
        &[
            (EFFECTS_STEREO, Widget::Chooser),
            (EFFECTS_PHASE, Widget::Knob),
            (EFFECTS_SPIN, Widget::Knob),
            (EFFECTS_OFFSET, Widget::Knob),
            (EFFECTS_DUTY, Widget::Knob),
            (EFFECTS_MORPH, Widget::Knob),
            (EFFECTS_ATTACK, Widget::Knob),
            (EFFECTS_RELEASE, Widget::Knob),
            (EFFECTS_HOLD, Widget::Toggle),
        ],
    )));
    DeviceType::native(EFFECTS, "Framework Test Effects", DeviceRole::AudioEffect).with_description(
        DeviceDescription::new(params)
            .with_layout(layout)
            .with_menu_options(&[EFFECTS_INVERT]),
    )
}

/// Every test device type.
#[must_use]
pub fn device_types() -> Vec<DeviceType> {
    vec![
        utility_type(),
        drive_type(),
        scope_type(),
        effects_type(),
        pages_type(),
    ]
}

/// Adds the test devices to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, utility_type(), |_, _| Utility);
    add(registry, drive_type(), |_, config| Drive::new(config));
    add(registry, scope_type(), |_, _| Scope);
    add(registry, effects_type(), Effects::new);
    add(registry, pages_type(), |_, _| Scope);
}

/// The engine's own devices and the test devices.
#[must_use]
pub fn registry() -> Registry {
    let mut registry = Registry::new();
    register(&mut registry);
    registry
}

/// Gain, balance, mute, and polarity.
struct Utility;

impl NativeDevice for Utility {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        let mute = params.value(2) >= 0.5;
        let sign = if params.value(3) >= 0.5 { -1.0 } else { 1.0 };
        for channel in 0..audio.channels() {
            let (gain, balance) = (params.ramp(0), params.ramp(1));
            let left = channel % 2 == 0;
            for (i, sample) in audio.channel_mut(channel).iter_mut().enumerate() {
                let b = balance[i] / 50.0;
                let side = if left {
                    (1.0 - b).min(1.0)
                } else {
                    (1.0 + b).min(1.0)
                };
                *sample *= if mute { 0.0 } else { sign * gain[i] * side };
            }
        }
    }
}

/// A `tanh` saturator with its own oversampler for each factor.
struct Drive {
    oversamplers: [Oversampler; 4],
    /// The oversampling in use: doublings.
    stages: usize,
}

impl Drive {
    fn new(config: &NodeConfig) -> Self {
        let make = |stages| Oversampler::new(stages, config.channels, config.max_frames);
        Self {
            oversamplers: [make(0), make(1), make(2), make(3)],
            stages: 2,
        }
    }
}

impl NativeDevice for Drive {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        self.stages = (params.value(2) as usize).min(3);
        let oversampler = &mut self.oversamplers[self.stages];
        let factor = oversampler.factor();
        let (drive, output) = (params.ramp(0), params.ramp(1));
        for channel in 0..audio.channels() {
            let samples = audio.channel_mut(channel);
            // Pieces the oversampler takes may be shorter than the
            // stretch; count frames across them.
            let mut frame = 0;
            oversampler.process(channel, samples, |high| {
                for (i, sample) in high.iter_mut().enumerate() {
                    let d = drive[(frame + i / factor).min(drive.len() - 1)];
                    *sample = (*sample * d).tanh();
                }
                frame += high.len() / factor;
            });
            for (sample, gain) in samples.iter_mut().zip(output) {
                *sample *= gain;
            }
        }
    }

    fn latency(&self) -> u32 {
        // The latency of the factor in use; a change reaches delay
        // compensation with WP-2.11.
        self.oversamplers[self.stages].latency()
    }
}

/// Passes audio through: its parameters only change its display.
struct Scope;

impl NativeDevice for Scope {
    fn process(&mut self, _audio: &mut Audio<'_, '_>, _params: &ParamBank) {}
}

/// The effects' shared parts, each used: the LFO moves the audio or is the
/// output; the envelope follower is the output or not; the LFO's random
/// shapes follow the device's own seed.
struct Effects {
    lfo: Lfo,
    followers: Vec<EnvelopeFollower>,
}

impl Effects {
    fn new(device: &Device, config: &NodeConfig) -> Self {
        Self {
            lfo: Lfo::new(config.sample_rate, device.id.0.get()),
            followers: (0..config.channels)
                .map(|_| EnvelopeFollower::new(config.sample_rate.as_f64()))
                .collect(),
        }
    }
}

impl NativeDevice for Effects {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        let at = |id: ParamId| params.index(id).expect("the description lists it");
        let (output, mode) = (
            params.value(at(EFFECTS_OUTPUT)) as usize,
            params.value(at(EFFECTS_RATE_MODE)) as usize,
        );
        let rate = match mode {
            0 => Rate::Hertz(params.target(at(EFFECTS_RATE))),
            1 => Rate::Seconds(params.target(at(EFFECTS_PERIOD))),
            _ => Rate::Beats(DIVISIONS[(params.value(at(EFFECTS_DIVISION)) as usize).min(5)]),
        };
        let stereo = if params.value(at(EFFECTS_STEREO)) >= 0.5 {
            Stereo::Spin(params.target(at(EFFECTS_SPIN)))
        } else {
            Stereo::Phase(params.target(at(EFFECTS_PHASE)))
        };
        self.lfo
            .set(audio, rate, stereo, params.target(at(EFFECTS_OFFSET)));
        for follower in &mut self.followers {
            follower.set(
                params.target(at(EFFECTS_ATTACK)) / 1_000.0,
                params.target(at(EFFECTS_RELEASE)) / 1_000.0,
                params.value(at(EFFECTS_HOLD)) >= 0.5,
            );
        }
        let wave = params.value(at(EFFECTS_WAVE)) as usize;
        let sign = if params.value(at(EFFECTS_INVERT)) >= 0.5 {
            -1.0
        } else {
            1.0
        };
        let (depth, duty, morph) = (
            params.ramp(at(EFFECTS_DEPTH)),
            params.ramp(at(EFFECTS_DUTY)),
            params.ramp(at(EFFECTS_MORPH)),
        );
        let tempo = audio.tempo();
        let channels = audio.channels();
        for i in 0..audio.frames() {
            let shape = Shape {
                wave: Wave::from_index(wave),
                morph: Some((Wave::from_index(wave + 1), f64::from(morph[i]) / 100.0)),
                duty: f64::from(duty[i]) / 100.0,
            };
            let lfo = self.lfo.step(tempo, i, shape);
            for channel in 0..channels {
                let value = sign * lfo[channel % 2];
                let sample = &mut audio.channel_mut(channel)[i];
                let level = self.followers[channel].process(*sample);
                *sample = match output {
                    0 => *sample * (1.0 - f64::from(depth[i]) / 200.0 * (1.0 - value)) as f32,
                    1 => value as f32,
                    _ => level as f32,
                };
            }
        }
    }
}

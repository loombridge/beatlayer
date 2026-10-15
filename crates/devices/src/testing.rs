//! Devices that prove the framework, for tests only. Not for songs: the
//! browser never offers them, and Beatlayer ships none of them.
//!
//! | Type | Name | What it does | Layout |
//! |---|---|---|---|
//! | [`UTILITY`] | Framework Test Utility | gain to −∞ dB, mute, polarity, balance | generic |
//! | [`DRIVE`] | Framework Test Drive | a `tanh` saturator, oversampled 1 to 8 times | generic |
//! | [`SCOPE`] | Framework Test Scope | passes audio through; a display cell between control bands, with a row inside its bottom edge | its own |
//!
//! Between them they have a parameter of every kind: gains, continuous
//! values with linear and logarithmic tapers, toggles, choices, and whole
//! numbers.

use bl_dsp::Oversampler;
use bl_engine::{DeviceType, NodeConfig, Registry};
use bl_model::device_layout::{BODY_HEIGHT, ControlBand, DisplayCell, LayoutNode, Widget};
use bl_model::{DeviceDescription, DeviceLayout, DeviceRole, ParamDescriptor, ParamId, ParamUnit};

use crate::framework::{Audio, NativeDevice, ParamBank, add};

/// The test utility's device type.
pub const UTILITY: &str = "test.framework.utility";
/// The test drive's device type.
pub const DRIVE: &str = "test.framework.drive";
/// The test scope's device type.
pub const SCOPE: &str = "test.framework.scope";

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
            .decimals(0),
            ParamDescriptor::toggle(UTILITY_MUTE, "Mute", false),
            ParamDescriptor::choice(UTILITY_POLARITY, "Polarity", &["Normal", "Inverted"], 0),
        ]),
    )
}

/// The test drive's type.
#[must_use]
pub fn drive_type() -> DeviceType {
    DeviceType::native(DRIVE, "Framework Test Drive", DeviceRole::AudioEffect).with_description(
        DeviceDescription::new(vec![
            ParamDescriptor::gain(DRIVE_DRIVE, "Drive", 36.0, 0.0),
            ParamDescriptor::gain(DRIVE_OUTPUT, "Output", 6.0, 0.0),
            ParamDescriptor::choice(
                DRIVE_OVERSAMPLING,
                "Oversampling",
                &["Off", "2x", "4x", "8x"],
                2,
            ),
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
        .logarithmic(),
        ParamDescriptor::gain(SCOPE_GAIN, "Gain", 24.0, 0.0),
        ParamDescriptor::toggle(SCOPE_FREEZE, "Freeze", false),
        ParamDescriptor::integer(SCOPE_DIVISIONS, "Divisions", (1, 16), 4, ParamUnit::None),
        ParamDescriptor::continuous(
            SCOPE_TRIGGER,
            "Trigger",
            (-100.0, 100.0),
            0.0,
            ParamUnit::Percent,
        ),
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

/// Every test device type.
#[must_use]
pub fn device_types() -> Vec<DeviceType> {
    vec![utility_type(), drive_type(), scope_type()]
}

/// Adds the test devices to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, utility_type(), |_, _| Utility);
    add(registry, drive_type(), |_, config| Drive::new(config));
    add(registry, scope_type(), |_, _| Scope);
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

//! CV In (WP-7.18; the CV tools spec, R26, R27, R30): a modulator whose
//! output is the voltage on its CV input. It maps its targets as the LFO
//! does (the modulation spec, R13 to R19), and its output, the voltage as a
//! share between Low and High followed by Smooth, is worked out before any
//! track runs, from the interface's input for the block.
//!
//! The input is taken on the 32-frame grid of the modulation spec's R20, and
//! the output runs in a straight line from each grid point's value to the
//! next. A line to the next point needs an input that has not arrived when
//! the line begins, at a block of one frame, so each line is played over the
//! 32 frames after the point it ends at: a fixed delay of 32 frames, the same
//! at every block size.

use bl_engine::{DeviceType, Registry};
use bl_model::device_layout::{BODY_HEIGHT, ControlBand, DeviceLayout, LayoutNode, Widget};
use bl_model::modulator::{CV_IN, DEPTH, MODE, POLARITY, SLOTS};
use bl_model::{
    DeviceDescription, DeviceDisplay, DeviceId, DeviceRole, ParamDescriptor, ParamId, ParamUnit,
};

use crate::cv::{VOLTS_PER_SAMPLE, volts_display};
use crate::framework::{Modulation, NativeDevice, ParamBank, add};
use crate::modulator::{GRID, Mapper, mapping_cells, mapping_params};

/// Low: the voltage that gives the output 0, from −10 to +10 V.
pub const LOW: ParamId = ParamId(0);
/// High: the voltage that gives the output 1, from −10 to +10 V.
pub const HIGH: ParamId = ParamId(1);
/// Smooth: the time a one-pole smoother takes to follow, from 0 to 1,000 ms.
pub const SMOOTH: ParamId = ParamId(2);

/// The input's name in the device's routing.
pub const INPUT_NAMES: [&str; 1] = ["CV"];

/// CV In's type.
#[must_use]
pub fn cv_in_type() -> DeviceType {
    let mut params = vec![
        ParamDescriptor::continuous(LOW, "Low", (-10.0, 10.0), -5.0, ParamUnit::None)
            .decimals(2)
            .describe(
                "The voltage that gives the output 0, in volts. Above High, it inverts the output.",
            ),
        ParamDescriptor::continuous(HIGH, "High", (-10.0, 10.0), 5.0, ParamUnit::None)
            .decimals(2)
            .describe("The voltage that gives the output 1, in volts."),
        ParamDescriptor::continuous(
            SMOOTH,
            "Smooth",
            (0.0, 1_000.0),
            0.0,
            ParamUnit::Milliseconds,
        )
        .decimals(0)
        .describe("Slows the output's changes: the time it takes to follow the voltage."),
    ];
    params.extend(mapping_params(
        SLOTS,
        (
            "Depth",
            "How far the modulator reaches: 0 % none, 100 % full.",
        ),
    ));
    DeviceType::native(CV_IN, "CV In", DeviceRole::AudioEffect)
        .with_cv(&[], &INPUT_NAMES)
        .with_description(
            DeviceDescription::new(params)
                .with_layout(layout())
                .with_banks(&[
                    ("Input", &["Low", "High", "Smooth"]),
                    ("Mapping", &["Mod", "Polarity", "Depth"]),
                    (
                        "Targets 1-2",
                        &["Amount 1", "Min 1", "Max 1", "Amount 2", "Min 2", "Max 2"],
                    ),
                    (
                        "Targets 3-4",
                        &["Amount 3", "Min 3", "Max 3", "Amount 4", "Min 4", "Max 4"],
                    ),
                    (
                        "Targets 5-6",
                        &["Amount 5", "Min 5", "Max 5", "Amount 6", "Min 6", "Max 6"],
                    ),
                    (
                        "Targets 7-8",
                        &["Amount 7", "Min 7", "Max 7", "Amount 8", "Min 8", "Max 8"],
                    ),
                ]),
        )
}

fn layout() -> DeviceLayout {
    let input = ControlBand::stacked(
        BODY_HEIGHT,
        &[
            (LOW, Widget::Knob),
            (HIGH, Widget::Knob),
            (SMOOTH, Widget::Knob),
        ],
    );
    let output = ControlBand::stacked(
        BODY_HEIGHT,
        &[
            (MODE, Widget::Chooser),
            (POLARITY, Widget::Chooser),
            (DEPTH, Widget::Knob),
        ],
    );
    let (first, rest) = mapping_cells(SLOTS);
    DeviceLayout::new(LayoutNode::Row(vec![
        LayoutNode::Band(input),
        first,
        LayoutNode::Band(output),
    ]))
    .with_expanded(rest)
}

/// Adds CV In to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, cv_in_type(), |_, config| {
        CvIn::new(config.sample_rate.as_f64())
    });
}

/// CV In's display (R9, R30): the voltage its input last read.
#[must_use]
pub fn display(device: DeviceId, data: Option<&[f32]>) -> DeviceDisplay {
    volts_display(device, INPUT_NAMES.map(str::to_owned), data)
}

struct CvIn {
    rate: f64,
    /// Frames since the transport last started, or since the engine's first
    /// frame before it has.
    clock: u64,
    was_playing: bool,
    primed: bool,
    /// The output at the last grid point and at the one before.
    from: f64,
    to: f64,
    smoothed: f64,
    shown: f32,
}

impl CvIn {
    fn new(rate: f64) -> Self {
        Self {
            rate,
            clock: 0,
            was_playing: false,
            primed: false,
            from: 0.0,
            to: 0.0,
            smoothed: 0.0,
            shown: 0.0,
        }
    }

    /// The share of the voltage `volts` between Low and High, within 0 to 1
    /// (R26).
    fn share(volts: f64, low: f64, high: f64) -> f64 {
        if (high - low).abs() < 1e-9 {
            return 0.0;
        }
        ((volts - low) / (high - low)).clamp(0.0, 1.0)
    }
}

impl NativeDevice for CvIn {
    fn process(&mut self, _: &mut crate::Audio<'_, '_>, _: &ParamBank) {}

    fn is_modulator(&self) -> bool {
        true
    }

    fn control(&mut self, modulation: &mut Modulation<'_, '_>, params: &ParamBank) {
        let playing = modulation.playing();
        if playing && !self.was_playing {
            // Spec: cv-tools R27. The grid is counted from the start.
            self.clock = 0;
            self.primed = false;
        }
        self.was_playing = playing;
        let ramp = |id| params.ramp(params.index(id).expect("the description holds it"));
        let (low, high, smooth) = (ramp(LOW), ramp(HIGH), ramp(SMOOTH));
        let mapper = Mapper::new(params);
        let input = modulation.cv_in();
        for f in 0..modulation.frames() {
            let volts = input.map_or(0.0, |data| f64::from(data[f]) * VOLTS_PER_SAMPLE);
            self.shown = volts as f32;
            if self.clock.is_multiple_of(GRID) {
                let u = Self::share(volts, f64::from(low[f]), f64::from(high[f]));
                let follow = if smooth[f] > 0.0 {
                    1.0 - (-(GRID as f64) / (f64::from(smooth[f]) / 1000.0 * self.rate)).exp()
                } else {
                    1.0
                };
                self.smoothed = if self.primed {
                    self.smoothed + (u - self.smoothed) * follow
                } else {
                    u
                };
                self.from = if self.primed { self.to } else { self.smoothed };
                self.to = self.smoothed;
                self.primed = true;
            }
            let t = (self.clock % GRID) as f64 / GRID as f64;
            mapper.write(modulation, params, f, self.from + (self.to - self.from) * t);
            self.clock += 1;
        }
    }

    fn display_len(&self) -> usize {
        INPUT_NAMES.len()
    }

    fn display(&mut self, out: &mut [f32]) {
        out[0] = self.shown;
    }
}

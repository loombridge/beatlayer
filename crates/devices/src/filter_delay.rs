//! Filter Delay, the three-line filter delay (WP-5.25; the filter delay spec):
//! three delay lines, each with its band in its loop, its own time (synced
//! or free), Pan and Volume, and one Feedback and one Dry for the device.

use std::f64::consts::{FRAC_1_SQRT_2, FRAC_PI_4};

use bl_engine::{DeviceType, NodeConfig, Registry};
use bl_model::device_layout::{
    BODY_HEIGHT, BOX_HEIGHT, CELL_WIDTH, Caption, Control, ControlBand, DisplayCell, LayoutNode,
    Widget,
};
use bl_model::{
    Device, DeviceDescription, DeviceLayout, DeviceRole, ParamDescriptor, ParamId, ParamUnit,
};

use crate::delay::{Line, MAX_DELAY_SECONDS, Mode, TRANSITION_SECONDS, sixteenth_frames};
use crate::eq::{Type, coeffs};
use crate::framework::{Audio, NativeDevice, ParamBank, add};
use crate::sections::Biquad;

/// Filter Delay's device type.
pub const FILTER_DELAY: &str = "bl.device.trident";

/// A line's On switch, as its offset among the line's parameters.
pub const ON: u32 = 0;
/// The line's Filter switch: its band is in the loop.
pub const FILTER: u32 = 1;
/// The band's centre frequency.
pub const FREQ: u32 = 2;
/// The band's width, in octaves.
pub const WIDTH: u32 = 3;
/// Sync: beat divisions on, free milliseconds off.
pub const SYNC: u32 = 4;
/// The synced time, in sixteenth notes.
pub const DIVISION: u32 = 5;
/// How much the synced time is shortened or lengthened, in per cent of it.
pub const PERCENT: u32 = 6;
/// The free time, in milliseconds.
pub const TIME: u32 = 7;
/// Where the line leaves, from hard left (−100 %) to hard right (+100 %).
pub const PAN: u32 = 8;
/// The line's output level.
pub const VOLUME: u32 = 9;

/// Parameters in each line.
const FIELDS: u32 = 10;
/// Lines.
const LINES: usize = 3;
/// How much of the first line's output feeds back into its input: the one
/// feedback of all three lines before each had its own.
pub const FEEDBACK: ParamId = ParamId(FIELDS * LINES as u32);
/// The unprocessed signal's level.
pub const DRY: ParamId = ParamId(FIELDS * LINES as u32 + 1);
/// The number of the second line's Feedback; the third's follows it.
const MORE_FEEDBACK: u32 = FIELDS * LINES as u32 + 2;
/// How many parameters the device has.
const PARAMS: usize = FIELDS as usize * LINES + 2 + (LINES - 1);

/// The lowest a band's edge goes, in hertz.
const LOWEST_EDGE: f64 = 20.0;
/// The highest the band's upper edge goes, as a share of the sample rate.
const HIGHEST_EDGE: f64 = 0.45;

/// Parameter `field` of line `line` (0 to 2).
#[must_use]
pub const fn line_param(line: usize, field: u32) -> ParamId {
    ParamId(line as u32 * FIELDS + field)
}

/// Line `line`'s Feedback: how much of its output feeds back into its own
/// input.
#[must_use]
pub const fn feedback_param(line: usize) -> ParamId {
    if line == 0 {
        FEEDBACK
    } else {
        ParamId(MORE_FEEDBACK + line as u32 - 1)
    }
}

/// A line's descriptors, named for the line.
/// Each line's defaults as Live's panel reads them: its band's centre, its
/// time in sixteenths, its pan in per cent, its volume in dB and its feedback.
const DEFAULTS: [(f64, i64, f64, f64, f64); LINES] = [
    (704.0, 3, -50.0, 4.0, 21.0),
    (449.0, 1, 0.0, -4.0, 46.0),
    (918.0, 5, 50.0, 6.0, 24.0),
];

fn line_descriptors(line: usize) -> Vec<ParamDescriptor> {
    let n = line + 1;
    let (centre, sixteenths, pan, volume, _) = DEFAULTS[line];
    let id = |field| line_param(line, field);
    vec![
        ParamDescriptor::toggle(id(ON), &format!("{n} On"), true)
            .describe("Switches the delay line on and off."),
        ParamDescriptor::toggle(id(FILTER), &format!("{n} Filter"), true)
            .describe("Puts the line's band in its loop."),
        ParamDescriptor::continuous(
            id(FREQ),
            &format!("{n} Freq"),
            (20.0, 20_000.0),
            centre,
            ParamUnit::Hertz,
        )
        .logarithmic()
        .describe("The centre frequency of the band the line lets through."),
        ParamDescriptor::continuous(
            id(WIDTH),
            &format!("{n} Width"),
            (1.0, 8.0),
            4.0,
            ParamUnit::None,
        )
        .describe("The width of the band, in octaves."),
        ParamDescriptor::toggle(id(SYNC), &format!("{n} Sync"), true)
            .describe("Sets the line's time in beat divisions rather than milliseconds."),
        ParamDescriptor::integer(
            id(DIVISION),
            &format!("{n} Division"),
            (1, 16),
            sixteenths,
            ParamUnit::None,
        )
        .describe("The line's time in sixteenth notes while Sync is on."),
        ParamDescriptor::continuous(
            id(PERCENT),
            &format!("{n} Time %"),
            (-50.0, 50.0),
            0.0,
            ParamUnit::Percent,
        )
        .describe("Shortens or lengthens the synced time, for a swing."),
        ParamDescriptor::continuous(
            id(TIME),
            &format!("{n} Time"),
            (1.0, 5_000.0),
            100.0,
            ParamUnit::Milliseconds,
        )
        .logarithmic()
        .describe("The line's time in milliseconds while Sync is off."),
        ParamDescriptor::continuous(
            id(PAN),
            &format!("{n} Pan"),
            (-100.0, 100.0),
            pan,
            ParamUnit::Percent,
        )
        .describe("Where the line leaves: by default the side it hears."),
        ParamDescriptor::gain(id(VOLUME), &format!("{n} Volume"), 6.0, volume)
            .describe("The line's output level, up to 6 dB."),
    ]
}

/// Filter Delay's type: Beatlayer's own look, Live's parameters (the
/// filter delay spec).
#[must_use]
pub fn filter_delay_type() -> DeviceType {
    let mut params: Vec<ParamDescriptor> = (0..LINES).flat_map(line_descriptors).collect();
    for (line, defaults) in DEFAULTS.iter().enumerate() {
        params.push(
            ParamDescriptor::continuous(
                feedback_param(line),
                &format!("{} Feedback", line + 1),
                (0.0, 100.0),
                defaults.4,
                ParamUnit::Percent,
            )
            .describe("How much of the line's output feeds back into its own input."),
        );
    }
    params.push(
        ParamDescriptor::gain(DRY, "Dry", 0.0, -5.0)
            .describe("The level of the unprocessed signal."),
    );
    DeviceType::native(FILTER_DELAY, "Filter Delay", DeviceRole::AudioEffect).with_description(
        DeviceDescription::new(params)
            .with_layout(layout())
            .with_banks(&[
                (
                    "Output",
                    &[
                        "1 Feedback",
                        "2 Feedback",
                        "3 Feedback",
                        "Dry",
                        "1 Volume",
                        "2 Volume",
                        "3 Volume",
                    ],
                ),
                (
                    "Delay 1",
                    &[
                        "1 On",
                        "1 Sync",
                        "1 Division",
                        "1 Time %",
                        "1 Time",
                        "1 Pan",
                        "1 Volume",
                    ],
                ),
                ("Filter 1", &["1 Filter", "1 Freq", "1 Width"]),
                (
                    "Delay 2",
                    &[
                        "2 On",
                        "2 Sync",
                        "2 Division",
                        "2 Time %",
                        "2 Time",
                        "2 Pan",
                        "2 Volume",
                    ],
                ),
                ("Filter 2", &["2 Filter", "2 Freq", "2 Width"]),
                (
                    "Delay 3",
                    &[
                        "3 On",
                        "3 Sync",
                        "3 Division",
                        "3 Time %",
                        "3 Time",
                        "3 Pan",
                        "3 Volume",
                    ],
                ),
                ("Filter 3", &["3 Filter", "3 Freq", "3 Width"]),
            ]),
    )
}

/// The first row's top and the pitch of the three rows, in points: Live's
/// rows are L, L+R and R.
const ROW_TOP: f32 = 16.0;
const ROW_PITCH: f32 = 48.0;

/// The sixteenths a line's grid of buttons sets, as Live's 1 2 3 4 over 5 6 8 16.
const STEPS: [i32; 8] = [1, 2, 3, 4, 5, 6, 8, 16];

/// Filter Delay as Live draws it: three rows, one for each line (L, L+R and
/// R), each with its input button, Filter switch with its frequency and width
/// over a display of the band, its grid of sixteenths over Sync and the time's
/// per cent, and its Feedback, Pan and Volume; at the right the output's
/// meter over Dry.
fn layout() -> DeviceLayout {
    let at = Control::new;
    let row = |line: usize| ROW_TOP + ROW_PITCH * line as f32;
    let (mut inputs, mut times) = (Vec::new(), Vec::new());
    for (line, name) in ["L", "L+R", "R"].into_iter().enumerate() {
        let id = |field| line_param(line, field);
        let y = row(line);
        inputs.extend([
            at(id(ON), Widget::Toggle, 4.0, y)
                .sized(CELL_WIDTH, BOX_HEIGHT)
                .labelled(name),
            at(id(FILTER), Widget::Toggle, 29.0, y)
                .sized(44.0, BOX_HEIGHT)
                .labelled("On"),
            at(id(FREQ), Widget::Slider, 29.0, y + 15.0)
                .unnamed()
                .sized(44.0, BOX_HEIGHT),
            at(id(WIDTH), Widget::Slider, 29.0, y + 29.0)
                .unnamed()
                .sized(44.0, BOX_HEIGHT),
        ]);
        times.extend([
            at(
                id(DIVISION),
                Widget::Grid {
                    columns: 4,
                    rows: 2,
                },
                4.0,
                y + 1.0,
            )
            .with_steps(&STEPS)
            .hidden_while(id(SYNC), 0),
            at(id(TIME), Widget::Number, 4.0, y + 1.0)
                .unnamed()
                .sized(97.0, BOX_HEIGHT)
                .shown_only_while(id(SYNC), 0),
            at(id(SYNC), Widget::Toggle, 4.0, y + 31.0)
                .sized(CELL_WIDTH, BOX_HEIGHT)
                .labelled("Sync"),
            at(id(PERCENT), Widget::Number, 51.0, y + 31.0)
                .unnamed()
                .sized(47.0, BOX_HEIGHT),
            at(feedback_param(line), Widget::SmallKnob, 108.0, y + 9.0).unnamed(),
            at(id(PAN), Widget::SmallKnob, 144.0, y + 9.0).unnamed(),
            at(id(VOLUME), Widget::SmallKnob, 181.0, y + 9.0).unnamed(),
        ]);
    }
    let inputs = ControlBand::free(77.0, BODY_HEIGHT, inputs).with_captions(vec![
        Caption::new("Input", 2.0, 4.0, 30.0),
        Caption::new("Filter", 29.0, 4.0, 44.0),
    ]);
    let times = ControlBand::free(218.0, BODY_HEIGHT, times).with_captions(vec![
        Caption::new("Delay Time", 4.0, 4.0, 60.0),
        Caption::new("Feedback", 100.0, 4.0, 46.0),
        Caption::new("Pan", 150.0, 4.0, 20.0),
        Caption::new("Volume", 178.0, 4.0, 36.0),
    ]);
    let space = |height| LayoutNode::Band(ControlBand::free(91.0, height, Vec::new()));
    let mut displays = vec![space(ROW_TOP)];
    for line in 0..LINES {
        displays.push(LayoutNode::Cell(DisplayCell::new(
            &format!("Filter {}", line + 1),
            91.0,
            40.0,
            1.0,
        )));
        displays.push(space(if line + 1 < LINES { 8.0 } else { 6.0 }));
    }
    let out = ControlBand::free(
        51.0,
        BODY_HEIGHT - ROW_TOP - 88.0,
        vec![at(DRY, Widget::SmallKnob, 9.0, 8.0)],
    );
    DeviceLayout::new(LayoutNode::Row(vec![
        LayoutNode::Band(inputs),
        LayoutNode::Column(displays),
        LayoutNode::Band(times),
        LayoutNode::Column(vec![
            LayoutNode::Band(
                ControlBand::free(51.0, ROW_TOP, Vec::new())
                    .with_captions(vec![Caption::new("Out", 16.0, 4.0, 24.0)]),
            ),
            LayoutNode::Cell(DisplayCell::new("Out", 20.0, 88.0, 1.0)).inset(16.0, 0.0, 15.0, 0.0),
            LayoutNode::Band(out),
        ]),
    ]))
}

pub fn register(registry: &mut Registry) {
    add(registry, filter_delay_type(), Trident::new);
}

/// One line's band: a high-pass and a low-pass, and the setting they were
/// last designed at.
#[derive(Clone, Copy, Default)]
struct Band {
    high_pass: Biquad,
    low_pass: Biquad,
    designed: (f32, f32),
}

impl Band {
    /// Designs the band for `freq` and `width` unless it already is, and
    /// returns `x` through it.
    fn process(&mut self, x: f32, freq: f32, width: f32, rate: f64) -> f32 {
        if self.designed != (freq, width) {
            self.designed = (freq, width);
            let half = f64::from(width) / 2.0;
            let low = (f64::from(freq) * (-half).exp2()).clamp(LOWEST_EDGE, rate * HIGHEST_EDGE);
            let high = (f64::from(freq) * half.exp2()).clamp(LOWEST_EDGE, rate * HIGHEST_EDGE);
            self.high_pass.c = coeffs(Type::LowCut12, low, 0.0, FRAC_1_SQRT_2, rate);
            self.low_pass.c = coeffs(Type::HighCut12, high, 0.0, FRAC_1_SQRT_2, rate);
        }
        self.low_pass.process(self.high_pass.process(x))
    }
}

/// Filter Delay: three lines and their bands.
struct Trident {
    lines: [Line; LINES],
    bands: [Band; LINES],
    rate: f64,
    glide: f64,
}

impl Trident {
    fn new(_: &Device, config: &NodeConfig) -> Self {
        let rate = config.sample_rate.as_f64();
        let len = ((MAX_DELAY_SECONDS + 0.1) * rate).ceil() as usize + 2;
        Self {
            lines: core::array::from_fn(|_| Line::new(len)),
            bands: [Band::default(); LINES],
            rate,
            glide: (TRANSITION_SECONDS * rate).max(1.0),
        }
    }
}

impl NativeDevice for Trident {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        if audio.frames() == 0 {
            return;
        }
        let ix: [usize; PARAMS] = core::array::from_fn(|id| {
            params
                .index(ParamId(id as u32))
                .expect("the description lists every parameter")
        });
        let sixteenth = sixteenth_frames(audio);
        let switch =
            |line: usize, field| params.value(ix[line_param(line, field).0 as usize]) >= 0.5;
        let ramp = |line: usize, field| params.ramp(ix[line_param(line, field).0 as usize]);
        let on: [bool; LINES] = core::array::from_fn(|l| switch(l, ON));
        let filtered: [bool; LINES] = core::array::from_fn(|l| switch(l, FILTER));
        // The time parameters' targets, not their smoothed values: the line
        // glides between times itself (the delay spec's details).
        let base: [f64; LINES] = core::array::from_fn(|l| {
            let target = |field| params.target(ix[line_param(l, field).0 as usize]);
            if switch(l, SYNC) {
                target(DIVISION) * sixteenth * (1.0 + target(PERCENT) / 100.0)
            } else {
                target(TIME) * self.rate / 1_000.0
            }
        });
        let freq: [&[f32]; LINES] = core::array::from_fn(|l| ramp(l, FREQ));
        let width: [&[f32]; LINES] = core::array::from_fn(|l| ramp(l, WIDTH));
        let pan: [&[f32]; LINES] = core::array::from_fn(|l| ramp(l, PAN));
        let volume: [&[f32]; LINES] = core::array::from_fn(|l| ramp(l, VOLUME));
        let feedback: [&[f32]; LINES] =
            core::array::from_fn(|l| params.ramp(ix[feedback_param(l).0 as usize]));
        let dry = params.ramp(ix[DRY.0 as usize]);

        let mut channels = audio.channels_mut();
        // shortcut: the filter delay is stereo; a mono track passes through.
        // Upgrade with a surround layout.
        let (Some(left), Some(right)) = (channels.next(), channels.next()) else {
            return;
        };
        for i in 0..left.len() {
            let (l, r) = (f64::from(left[i]), f64::from(right[i]));
            // The first line hears the left, the third the right, the second
            // both, at the level of one.
            let heard = [l, (l + r) / 2.0, r];
            let (mut wet_left, mut wet_right) = (0.0, 0.0);
            for k in 0..LINES {
                let tap = self.lines[k].tap(base[k], 0.0, Mode::Repitch, self.glide);
                if !on[k] {
                    self.lines[k].write(0.0);
                    continue;
                }
                let input = (heard[k] + f64::from(feedback[k][i]) / 100.0 * tap) as f32;
                let input = if filtered[k] {
                    self.bands[k].process(input, freq[k][i], width[k][i], self.rate)
                } else {
                    input
                };
                self.lines[k].write(input);
                // Constant power: hard left leaves on the left alone.
                let angle = (f64::from(pan[k][i]) / 100.0 + 1.0) * FRAC_PI_4;
                let out = tap * f64::from(volume[k][i]);
                wet_left += out * angle.cos();
                wet_right += out * angle.sin();
            }
            let dry = f64::from(dry[i]);
            left[i] = (dry * l + wet_left) as f32;
            right[i] = (dry * r + wet_right) as f32;
        }
    }
}

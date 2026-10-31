//! Gated Delay (WP-8.51; the gated-delay spec): the input enters a delay
//! only on the active steps of a gate sequencer that runs on the song's
//! grid, so the echoes come in rhythmic patterns.
//!
//! Step 1 begins on each bar line. A step opens on the frame the tempo map
//! gives its tick, so it lands there at any block size, and the sequencer's
//! place is worked out again from the song position whenever the song
//! jumps or a setting that moves the steps changes. Random's draws are a
//! function of the seed, the bar and the pass, not a stream, so the same
//! bar draws the same steps in every render.

use bl_core::time::{Frames, SampleRate, TICKS_PER_QUARTER, TempoMap, Ticks, TimeSignatureMap};
use bl_dsp::Random;
use bl_engine::{DeviceType, NodeConfig, Registry};
use bl_model::device_layout::{
    BODY_HEIGHT, Control, ControlBand, DisplayCell, LayoutNode, PADDING, Widget,
};
use bl_model::{
    Device, DeviceDescription, DeviceDisplay, DeviceId, DeviceLayout, DeviceRole, DisplayReading,
    DisplaySeries, OrdMap, ParamDescriptor, ParamId, ParamUnit,
};

use crate::delay::Line;
use crate::framework::{Audio, NativeDevice, ParamBank, add};

/// Gated Delay's device type.
pub const GATED_DELAY: &str = "beatlayer.gated_delay";

/// The number of steps in the row.
pub const STEP_COUNT: usize = 16;
/// The parameter of step `n` (1 to 16): a switch, on while the step sends.
#[must_use]
pub const fn step(n: usize) -> ParamId {
    ParamId(n as u32 - 1)
}
/// Steps: how many steps play before the row repeats.
pub const STEPS: ParamId = ParamId(16);
/// Rate: the length of a step, one of [`RATES`].
pub const RATE: ParamId = ParamId(17);
/// Delay Steps: the delay time, in steps.
pub const DELAY_STEPS: ParamId = ParamId(18);
/// Feedback: how much of the line's output returns to its input.
pub const FEEDBACK: ParamId = ParamId(19);
/// Random: the chance each step is turned over for a pass.
pub const RANDOM: ParamId = ParamId(20);
/// Dry on Rests: the dry signal plays on the off steps only.
pub const DRY_ON_RESTS: ParamId = ParamId(21);
/// Input: the gain of the signal entering the gate, down to silence.
pub const INPUT: ParamId = ParamId(22);
/// Dry/Wet: the dry signal against the echoes.
pub const DRY_WET: ParamId = ParamId(23);

/// How many parameters it has.
const PARAMS: usize = 24;

/// The steps a new device has on, counting from 1.
const ON: [usize; 5] = [1, 4, 7, 11, 14];

/// The Rate chooser's entries: their names and lengths in ticks, from 1/32
/// to a dotted quarter note with triplet and dotted forms.
pub const RATES: [(&str, i64); 12] = [
    ("1/32T", TICKS_PER_QUARTER / 12),
    ("1/32", TICKS_PER_QUARTER / 8),
    ("1/32.", TICKS_PER_QUARTER * 3 / 16),
    ("1/16T", TICKS_PER_QUARTER / 6),
    ("1/16", TICKS_PER_QUARTER / 4),
    ("1/16.", TICKS_PER_QUARTER * 3 / 8),
    ("1/8T", TICKS_PER_QUARTER / 3),
    ("1/8", TICKS_PER_QUARTER / 2),
    ("1/8.", TICKS_PER_QUARTER * 3 / 4),
    ("1/4T", TICKS_PER_QUARTER * 2 / 3),
    ("1/4", TICKS_PER_QUARTER),
    ("1/4.", TICKS_PER_QUARTER * 3 / 2),
];
/// The Rate a new device has: 1/16.
const DEFAULT_RATE: usize = 4;
/// How long the gate takes to open or close, in seconds.
const RAMP_SECONDS: f64 = 0.002;
/// The slowest tempo the transport allows, in quarter notes a minute.
const SLOWEST_BPM: f64 = 20.0;
/// The values the device publishes: the steps playing this pass, which
/// Random turned over, and the step playing.
const DISPLAY_LEN: usize = 2 * STEP_COUNT + 1;
/// The step names, for banks.
const STEP_NAMES: [&str; STEP_COUNT] = [
    "Step 1", "Step 2", "Step 3", "Step 4", "Step 5", "Step 6", "Step 7", "Step 8", "Step 9",
    "Step 10", "Step 11", "Step 12", "Step 13", "Step 14", "Step 15", "Step 16",
];
/// The step row's cell name, which the generic UI draws from the display.
pub const STEP_ROW: &str = "Step Row";
/// The display's series of the steps' switches, and its reading of the step
/// playing.
pub const STEPS_SERIES: &str = "Steps";
const PASS_SERIES: &str = "Pass";
pub const TURNED_SERIES: &str = "Turned Over";
pub const PLAYING_STEP: &str = "Playing Step";
/// The columns of the step row's switches, in two rows.
pub const ROW_COLUMNS: usize = 8;

/// Gated Delay's type: Live's name, Beatlayer's own layout (the
/// gated-delay spec).
#[must_use]
pub fn gated_delay_type() -> DeviceType {
    let names: Vec<&str> = RATES.iter().map(|(name, _)| *name).collect();
    let mut params: Vec<ParamDescriptor> = (1..=STEP_COUNT)
        .map(|n| {
            ParamDescriptor::toggle(step(n), STEP_NAMES[n - 1], ON.contains(&n)).describe(&format!(
                "Whether step {n} of the row sends the signal into the delay."
            ))
        })
        .collect();
    params.extend([
        ParamDescriptor::integer(STEPS, "Steps", (1, 16), 16, ParamUnit::None)
            .describe("How many steps play before the row repeats."),
        ParamDescriptor::choice(RATE, "Rate", &names, DEFAULT_RATE)
            .describe("The length of a step, on the song's grid."),
        ParamDescriptor::integer(DELAY_STEPS, "Delay Steps", (1, 16), 3, ParamUnit::None)
            .describe("The delay time, in steps: an echo lands this many steps after its step."),
        ParamDescriptor::continuous(FEEDBACK, "Feedback", (0.0, 100.0), 40.0, ParamUnit::Percent)
            .describe("How much of the delay's output returns to its input."),
        ParamDescriptor::continuous(RANDOM, "Random", (0.0, 100.0), 0.0, ParamUnit::Percent)
            .describe("The chance each step is turned over for one pass of the row."),
        ParamDescriptor::toggle(DRY_ON_RESTS, "Dry on Rests", true)
            .describe("Plays the dry signal on the off steps only; off, it plays throughout."),
        ParamDescriptor::gain(INPUT, "Input", 12.0, 0.0)
            .describe("The level of the signal entering the gate, down to silence."),
        ParamDescriptor::continuous(DRY_WET, "Dry/Wet", (0.0, 100.0), 50.0, ParamUnit::Percent)
            .describe("The balance between the dry signal and the echoes."),
    ]);
    DeviceType::native(GATED_DELAY, "Gated Delay", DeviceRole::AudioEffect).with_description(
        DeviceDescription::new(params)
            .with_layout(layout())
            .with_banks(&[
                (
                    "Main",
                    &[
                        "Steps",
                        "Rate",
                        "Delay Steps",
                        "Feedback",
                        "Random",
                        "Dry on Rests",
                        "Input",
                        "Dry/Wet",
                    ],
                ),
                ("Steps 1–8", &STEP_NAMES[..8]),
                ("Steps 9–16", &STEP_NAMES[8..]),
            ]),
    )
}

/// The step row's cell holds the 16 switches, in two rows of eight, and
/// draws the playing step under them.
fn layout() -> DeviceLayout {
    let size = Widget::Toggle.size();
    let controls: Vec<Control> = (1..=STEP_COUNT)
        .map(|n| Control {
            param: step(n),
            widget: Widget::Toggle,
            x: PADDING + ((n - 1) % ROW_COLUMNS) as f32 * (size.width + PADDING),
            y: PADDING + ((n - 1) / ROW_COLUMNS) as f32 * (size.height + PADDING),
        })
        .collect();
    let row = ControlBand {
        width: PADDING + ROW_COLUMNS as f32 * (size.width + PADDING),
        height: PADDING + 2.0 * (size.height + PADDING),
        controls,
        slots: None,
    };
    let cell = DisplayCell::new(STEP_ROW, row.width, BODY_HEIGHT, 1.0).with_top(row);
    let band = |controls: &[(ParamId, Widget)]| {
        LayoutNode::Band(ControlBand::stacked(BODY_HEIGHT, controls))
    };
    DeviceLayout::new(LayoutNode::Row(vec![
        band(&[
            (STEPS, Widget::Number),
            (RATE, Widget::Chooser),
            (DELAY_STEPS, Widget::Number),
        ]),
        LayoutNode::Cell(cell),
        band(&[
            (FEEDBACK, Widget::Knob),
            (RANDOM, Widget::Knob),
            (DRY_ON_RESTS, Widget::Toggle),
            (INPUT, Widget::Knob),
            (DRY_WET, Widget::Knob),
        ]),
    ]))
}

/// Adds Gated Delay to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, gated_delay_type(), Gated::new);
}

/// Where the sequencer is: the bar and the step of it, counted from the bar
/// line, and the song frames the step spans.
#[derive(Clone, Copy, Default)]
struct Place {
    bar: i64,
    count: i64,
    start: i64,
    end: i64,
}

/// The steps' ticks and the sequencer's whole view of the song's grid.
struct Grid<'a> {
    tempo: &'a TempoMap,
    signatures: &'a TimeSignatureMap,
    rate: SampleRate,
    /// A step's length in ticks.
    ticks: i64,
}

impl Grid<'_> {
    fn frame(&self, ticks: i64) -> i64 {
        self.tempo.ticks_to_frames(Ticks(ticks), self.rate).get()
    }

    /// The step `count` of `bar`: the song frames it spans, from its tick
    /// to the next step's or the next bar line, whichever comes first.
    fn place(&self, bar: i64, count: i64) -> Place {
        let start = self.signatures.bar_start(bar).get();
        let next_bar = self.signatures.bar_start(bar + 1).get();
        let from = start + count * self.ticks;
        Place {
            bar,
            count,
            start: self.frame(from),
            end: self.frame((from + self.ticks).min(next_bar)),
        }
    }

    /// The last step of `bar`.
    fn last(&self, bar: i64) -> i64 {
        let length =
            self.signatures.bar_start(bar + 1).get() - self.signatures.bar_start(bar).get();
        (length - 1) / self.ticks
    }

    /// The step after `at`.
    fn next(&self, at: Place) -> Place {
        if at.count < self.last(at.bar) {
            self.place(at.bar, at.count + 1)
        } else {
            self.place(at.bar + 1, 0)
        }
    }

    /// The step that holds song frame `frame`.
    fn find(&self, frame: i64) -> Place {
        let ticks = self.tempo.frames_to_ticks(Frames(frame), self.rate);
        let (bar, into) = self.signatures.bar_at(ticks);
        let mut at = self.place(bar, (into.get() / self.ticks).min(self.last(bar)));
        // The frame a tick lands on is rounded, so the estimate can be a
        // step out either way.
        while frame < at.start {
            at = if at.count > 0 {
                self.place(at.bar, at.count - 1)
            } else {
                self.place(at.bar - 1, self.last(at.bar - 1))
            };
        }
        while frame >= at.end {
            at = self.next(at);
        }
        at
    }
}

/// Gated Delay: a delay line a channel, the gate's state and the
/// sequencer's place.
struct Gated {
    rate: SampleRate,
    lines: Vec<Line>,
    seed: u64,
    place: Place,
    /// The steps and rate the place was worked out for.
    shape: (i64, i64),
    /// The song frame the next block should start on if the song plays on.
    expected: i64,
    /// Frames played while the song is stopped, which carry the clock on.
    idle: i64,
    /// The pass the draws are for, and its 16 draws from 0 to 1.
    pass: Option<(i64, i64)>,
    draws: [f64; STEP_COUNT],
    /// The gate's level, 0 to 1.
    gate: f64,
    /// What the last block ended on, for the display.
    shown: Shown,
}

#[derive(Clone, Copy, Default)]
struct Shown {
    switches: [bool; STEP_COUNT],
    random: f64,
    step: usize,
}

impl Gated {
    fn new(device: &Device, config: &NodeConfig) -> Self {
        let rate = config.sample_rate;
        // 16 steps of a dotted quarter note at the slowest tempo.
        let seconds = 16.0 * 1.5 * 60.0 / SLOWEST_BPM;
        let len = (seconds * rate.as_f64()).ceil() as usize + 2;
        Self {
            rate,
            lines: (0..config.channels).map(|_| Line::new(len)).collect(),
            seed: device.id.0.get(),
            place: Place::default(),
            shape: (0, 0),
            expected: i64::MIN,
            idle: 0,
            pass: None,
            draws: [0.0; STEP_COUNT],
            gate: 0.0,
            shown: Shown::default(),
        }
    }

    /// The 16 draws for `pass` of `bar`, from the seed.
    fn draw(&mut self, bar: i64, pass: i64) {
        if self.pass == Some((bar, pass)) {
            return;
        }
        self.pass = Some((bar, pass));
        let key = self.seed.wrapping_mul(0x9E37_79B9_7F4A_7C15)
            ^ (bar as u64).wrapping_mul(0xBF58_476D_1CE4_E5B9)
            ^ (pass as u64).wrapping_mul(0x94D0_49BB_1331_11EB);
        let mut random = Random::seeded(key);
        for draw in &mut self.draws {
            *draw = (random.next_bipolar() + 1.0) / 2.0;
        }
    }
}

impl NativeDevice for Gated {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        let frames = audio.frames();
        if frames == 0 {
            return;
        }
        let rate = self.rate.as_f64();
        let ix: [usize; PARAMS] = core::array::from_fn(|id| {
            params
                .index(ParamId(id as u32))
                .expect("the description lists every parameter")
        });
        let ramp = |id: ParamId| params.ramp(ix[id.0 as usize]);
        let value = |id: ParamId| params.value(ix[id.0 as usize]);
        let steps = value(STEPS) as i64;
        let ticks = RATES[(value(RATE) as usize).min(RATES.len() - 1)].1;
        let grid = Grid {
            tempo: audio.tempo(),
            signatures: audio.signatures(),
            rate: self.rate,
            ticks,
        };
        let playing = audio.playing();
        let song = audio.song_position().get();
        let base = if playing { song } else { song + self.idle };
        if base != self.expected || self.shape != (steps, ticks) {
            self.place = grid.find(base);
            self.shape = (steps, ticks);
        }
        let dry_on_rests = value(DRY_ON_RESTS) >= 0.5;
        let delay_quarters = |i: usize| {
            f64::from(ramp(DELAY_STEPS)[i]).round() * ticks as f64 / TICKS_PER_QUARTER as f64
        };
        let slew = 1.0 / (RAMP_SECONDS * rate);
        let (input, feedback, random, dry_wet) =
            (ramp(INPUT), ramp(FEEDBACK), ramp(RANDOM), ramp(DRY_WET));
        let mut level = 0.0;
        for i in 0..frames {
            let clock = base + i as i64;
            while clock >= self.place.end {
                self.place = grid.next(self.place);
            }
            let (bar, pass, at) = (
                self.place.bar,
                self.place.count / steps,
                (self.place.count % steps) as usize,
            );
            self.draw(bar, pass);
            let switch = params.ramp(ix[at])[i] >= 0.5;
            level = f64::from(random[i]);
            let on = switch != (self.draws[at] * 100.0 < level);
            self.gate += (f64::from(u8::from(on)) - self.gate).clamp(-slew, slew);
            let delay = delay_quarters(i) * 60.0 / audio.bpm_at(i) * rate;
            let (feedback, wet) = (
                f64::from(feedback[i]) / 100.0,
                f64::from(dry_wet[i]) / 100.0,
            );
            let heard = if dry_on_rests { 1.0 - self.gate } else { 1.0 };
            for (channel, line) in self.lines.iter_mut().enumerate().take(audio.channels()) {
                let dry = f64::from(audio.channel_mut(channel)[i]);
                let echo = line.read(delay);
                audio.channel_mut(channel)[i] = (dry * heard * (1.0 - wet) + echo * wet) as f32;
                line.write((f64::from(input[i]) * self.gate * dry + feedback * echo) as f32);
            }
        }
        self.expected = base + frames as i64;
        self.idle = if playing {
            0
        } else {
            self.idle + frames as i64
        };
        let at = (self.place.count % steps) as usize;
        self.shown = Shown {
            switches: core::array::from_fn(|n| params.value(ix[n]) >= 0.5),
            random: level,
            step: at,
        };
    }

    fn display_len(&self) -> usize {
        DISPLAY_LEN
    }

    fn display(&mut self, out: &mut [f32]) {
        let steps = self.shape.0.max(1) as usize;
        for n in 0..STEP_COUNT {
            let turned = self.draws[n] * 100.0 < self.shown.random && n < steps;
            out[n] = f32::from(u8::from(self.shown.switches[n] != turned));
            out[STEP_COUNT + n] = f32::from(u8::from(turned));
        }
        out[2 * STEP_COUNT] = (self.shown.step + 1) as f32;
    }
}

/// The step row as `device.display` reports it (the gated-delay spec, R9):
/// the switches from the parameters; once the engine has published, the
/// steps playing this pass, the ones Random turned over and the step
/// playing.
#[must_use]
pub fn display(
    device: DeviceId,
    description: &DeviceDescription,
    values: &OrdMap<ParamId, f64>,
    data: Option<&[f32]>,
) -> DeviceDisplay {
    let series = |name: &str, values: Vec<f32>, detail: bool| DisplaySeries {
        name: name.to_owned(),
        unit: String::new(),
        x_unit: "step".to_owned(),
        x_range: [1.0, STEP_COUNT as f32],
        log_x: false,
        y_range: [0.0, 1.0],
        detail,
        values,
    };
    let switches = (1..=STEP_COUNT)
        .map(|n| {
            let id = step(n);
            let on = values
                .get(&id)
                .copied()
                .or_else(|| description.param(id).map(|p| p.default))
                .unwrap_or(0.0);
            f32::from(u8::from(on >= 0.5))
        })
        .collect();
    let mut all = vec![series(STEPS_SERIES, switches, false)];
    let mut readings = Vec::new();
    let data = data.filter(|d| d.len() >= DISPLAY_LEN);
    if let Some(data) = data {
        all.push(series(PASS_SERIES, data[..STEP_COUNT].to_vec(), true));
        all.push(series(
            TURNED_SERIES,
            data[STEP_COUNT..2 * STEP_COUNT].to_vec(),
            true,
        ));
        readings.push(DisplayReading {
            name: PLAYING_STEP.to_owned(),
            value: data[2 * STEP_COUNT],
            unit: String::new(),
            text: format!("Step {}", data[2 * STEP_COUNT] as usize),
        });
    }
    DeviceDisplay {
        device,
        live: data.is_some(),
        view: None,
        series: all,
        markers: Vec::new(),
        readings,
    }
}

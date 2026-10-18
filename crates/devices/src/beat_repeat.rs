//! Beat Repeat (WP-5.26; the beat-repeat spec):
//! at points on the song's beat grid it captures a slice of its input and
//! repeats it for a while, with a pitch, a filter and a fade on the
//! repeats.

use std::f64::consts::{SQRT_2, TAU};

use bl_core::time::{Frames, SampleRate, TICKS_PER_QUARTER, TempoMap, Ticks};
use bl_dsp::Random;
use bl_engine::{DeviceType, NodeConfig, Registry};
use bl_model::device_layout::{BODY_HEIGHT, ControlBand, LayoutNode, Widget};
use bl_model::{
    Device, DeviceDescription, DeviceLayout, DeviceRole, ParamDescriptor, ParamId, ParamUnit,
};

use crate::framework::{Audio, NativeDevice, ParamBank, add};

/// Beat Repeat's device type.
pub const BEAT_REPEAT: &str = "bl.device.stutter";

/// How often a capture can start: one of [`INTERVALS`].
pub const INTERVAL: ParamId = ParamId(0);
/// Where in the interval it starts, in sixteenth notes.
pub const OFFSET: ParamId = ParamId(1);
/// The probability that a capture starts at a point of the interval.
pub const CHANCE: ParamId = ParamId(2);
/// The repeats' total length, in sixteenth notes.
pub const GATE: ParamId = ParamId(3);
/// Captures and repeats at once while it is on.
pub const REPEAT: ParamId = ParamId(4);
/// The size of a slice: one of [`GRIDS`].
pub const GRID: ParamId = ParamId(5);
/// Keeps the grid binary.
pub const NO_TRIPLETS: ParamId = ParamId(6);
/// How far the grid fluctuates around its setting.
pub const VARIATION: ParamId = ParamId(7);
/// When the grid is varied.
pub const VARIATION_MODE: ParamId = ParamId(8);
/// How far the repeats are resampled down, in semitones.
pub const PITCH: ParamId = ParamId(9);
/// How much lower each repeat plays than the one before.
pub const PITCH_DECAY: ParamId = ParamId(10);
/// The filter on the repeats.
pub const FILTER: ParamId = ParamId(11);
/// The filter's centre frequency.
pub const FREQ: ParamId = ParamId(12);
/// The filter's width, in octaves.
pub const WIDTH: ParamId = ParamId(13);
/// How the input and the repeats leave: Mix, Insert or Gate.
pub const MODE: ParamId = ParamId(14);
/// The device's output level.
pub const VOLUME: ParamId = ParamId(15);
/// How much quieter each repeat is than the one before.
pub const DECAY: ParamId = ParamId(16);

/// How many parameters it has.
const PARAMS: usize = 17;

/// The Interval chooser's entries, in ticks. A bar is four quarter notes.
pub const INTERVALS: [i64; 8] = [
    TICKS_PER_QUARTER / 8,
    TICKS_PER_QUARTER / 4,
    TICKS_PER_QUARTER / 2,
    TICKS_PER_QUARTER,
    TICKS_PER_QUARTER * 2,
    TICKS_PER_QUARTER * 4,
    TICKS_PER_QUARTER * 8,
    TICKS_PER_QUARTER * 16,
];
/// The Grid chooser's entries, in ticks: every second one is a triplet.
pub const GRIDS: [i64; 7] = [
    TICKS_PER_QUARTER,
    TICKS_PER_QUARTER * 2 / 3,
    TICKS_PER_QUARTER / 2,
    TICKS_PER_QUARTER / 3,
    TICKS_PER_QUARTER / 4,
    TICKS_PER_QUARTER / 6,
    TICKS_PER_QUARTER / 8,
];
/// The beat-time modes of Variation, in ticks, after Trigger.
const VARIATION_LINES: [i64; 3] = [
    TICKS_PER_QUARTER,
    TICKS_PER_QUARTER / 2,
    TICKS_PER_QUARTER / 4,
];

/// The longest slice, in seconds: the capture buffer is made to hold it.
const MAX_SLICE_SECONDS: f64 = 8.0;
/// The most grid entries Variation at 100 % moves the grid by.
const VARIATION_STEPS: f64 = 3.0;
/// The pitch each repeat adds at Pitch Decay 100 %, in semitones.
const PITCH_DECAY_SEMITONES: f64 = 2.0;
/// The most the repeats play down, in semitones.
const MAX_PITCH: f64 = 60.0;
/// The fade each repeat adds at Decay 100 %, in decibels.
const DECAY_DB: f64 = 6.0;

/// Beat Repeat's type: Beatlayer's own layout, Live's parameters (the
/// beat-repeat spec).
#[must_use]
pub fn beat_repeat_type() -> DeviceType {
    let params = vec![
        ParamDescriptor::choice(
            INTERVAL,
            "Interval",
            &[
                "1/32", "1/16", "1/8", "1/4", "1/2", "1 Bar", "2 Bars", "4 Bars",
            ],
            5,
        )
        .describe("How often a capture can start, on the song's beat grid."),
        ParamDescriptor::integer(OFFSET, "Offset", (0, 15), 0, ParamUnit::None)
            .describe("Moves the points of the interval later, in sixteenth notes."),
        ParamDescriptor::continuous(CHANCE, "Chance", (0.0, 100.0), 100.0, ParamUnit::Percent)
            .describe("The probability that a capture starts at a point of the interval."),
        ParamDescriptor::integer(GATE, "Gate", (1, 32), 4, ParamUnit::None)
            .describe("The total length of the repeats, in sixteenth notes."),
        ParamDescriptor::toggle(REPEAT, "Repeat", false)
            .describe("Captures and repeats at once, ignoring the interval, until it is off."),
        ParamDescriptor::choice(
            GRID,
            "Grid",
            &["1/4", "1/6", "1/8", "1/12", "1/16", "1/24", "1/32"],
            4,
        )
        .describe("The size of each repeated slice."),
        ParamDescriptor::toggle(NO_TRIPLETS, "No Triplets", false)
            .describe("Keeps the grid binary: a triplet grid plays as the next shorter one."),
        ParamDescriptor::continuous(
            VARIATION,
            "Variation",
            (0.0, 100.0),
            0.0,
            ParamUnit::Percent,
        )
        .describe("How far the grid size fluctuates around its setting."),
        ParamDescriptor::choice(
            VARIATION_MODE,
            "Variation Mode",
            &["Trigger", "1/4", "1/8", "1/16", "Auto"],
            0,
        )
        .describe("When the grid is varied: at each capture, on a beat division, or per repeat."),
        ParamDescriptor::continuous(PITCH, "Pitch", (0.0, 36.0), 0.0, ParamUnit::Semitones)
            .describe("Resamples the repeats lower, so the slices play longer."),
        ParamDescriptor::continuous(
            PITCH_DECAY,
            "Pitch Decay",
            (0.0, 100.0),
            0.0,
            ParamUnit::Percent,
        )
        .describe("Makes each repeat play lower than the one before."),
        ParamDescriptor::toggle(FILTER, "Filter", false)
            .describe("Passes only a band of the repeats."),
        ParamDescriptor::continuous(FREQ, "Freq", (20.0, 20_000.0), 1_000.0, ParamUnit::Hertz)
            .logarithmic()
            .describe("The filter's centre frequency."),
        ParamDescriptor::continuous(WIDTH, "Width", (0.1, 8.0), 4.0, ParamUnit::None)
            .describe("The filter's width in octaves."),
        ParamDescriptor::choice(MODE, "Mode", &["Mix", "Insert", "Gate"], 0).describe(
            "Mix adds the repeats to the input, Insert replaces it, Gate plays only them.",
        ),
        ParamDescriptor::gain(VOLUME, "Volume", 6.0, 0.0)
            .describe("The device's output level, down to silence."),
        ParamDescriptor::continuous(DECAY, "Decay", (0.0, 100.0), 0.0, ParamUnit::Percent)
            .describe("Makes each repeat quieter than the one before."),
    ];
    DeviceType::native(BEAT_REPEAT, "Beat Repeat", DeviceRole::AudioEffect)
        .with_description(DeviceDescription::new(params).with_layout(layout()))
}

fn layout() -> DeviceLayout {
    use Widget::{Chooser, Knob, Number, Toggle};
    let band = |controls: &[(ParamId, Widget)]| {
        LayoutNode::Band(ControlBand::stacked(BODY_HEIGHT, controls))
    };
    DeviceLayout::new(LayoutNode::Row(vec![
        band(&[
            (INTERVAL, Chooser),
            (OFFSET, Number),
            (CHANCE, Knob),
            (GATE, Number),
            (REPEAT, Toggle),
        ]),
        band(&[
            (GRID, Chooser),
            (NO_TRIPLETS, Toggle),
            (VARIATION, Knob),
            (VARIATION_MODE, Chooser),
        ]),
        band(&[
            (PITCH, Knob),
            (PITCH_DECAY, Knob),
            (FILTER, Toggle),
            (FREQ, Knob),
            (WIDTH, Knob),
        ]),
        band(&[(MODE, Chooser), (VOLUME, Knob), (DECAY, Knob)]),
    ]))
}

/// Adds Beat Repeat to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, beat_repeat_type(), Stutter::new);
}

/// A capture and its repeats.
struct Span {
    /// The song frame the span ends on, or `i64::MAX` while Repeat holds it.
    end: i64,
    held: bool,
    /// Frames in a quarter note at the span's start.
    quarter: f64,
    /// The Grid entry the next slice follows.
    grid: usize,
    /// Frames still to capture, and frames of the slice captured.
    capture_left: usize,
    captured: usize,
    /// The slice repeating now: its length in frames of the capture, how far
    /// it has played, and its speed, gain and number.
    slice: usize,
    position: f64,
    speed: f64,
    gain: f64,
    repeat: u32,
}

/// Beat Repeat: a capture buffer and a filter per channel, and one state
/// machine for them all.
struct Stutter {
    rate: SampleRate,
    buffers: Vec<Vec<f32>>,
    filters: Vec<[Biquad; 2]>,
    /// The filter's setting the coefficients were made for.
    tuned: Option<(f32, f32)>,
    random: Random,
    span: Option<Span>,
    /// Whether Repeat was on at the last stretch.
    held: bool,
    /// Where the song frame after the last stretch was, to notice a jump.
    expected: i64,
    /// Frames played while the song is stopped, which carry the clock on.
    idle: i64,
}

impl Stutter {
    fn new(device: &Device, config: &NodeConfig) -> Self {
        let length = (MAX_SLICE_SECONDS * config.sample_rate.as_f64()).ceil() as usize;
        Self {
            rate: config.sample_rate,
            buffers: (0..config.channels).map(|_| vec![0.0; length]).collect(),
            filters: vec![[Biquad::default(); 2]; config.channels],
            tuned: None,
            random: Random::seeded(device.id.0.get()),
            span: None,
            held: false,
            expected: 0,
            idle: 0,
        }
    }
}

/// The first point of the grid `offset + k × period` (k from 0) on song
/// frame `from` or later: its tick and its frame.
fn next_on_grid(
    tempo: &TempoMap,
    rate: SampleRate,
    from: i64,
    period: i64,
    offset: i64,
) -> (Ticks, i64) {
    let now = tempo.frames_to_ticks(Frames(from), rate).get();
    let mut k = ((now - offset).div_euclid(period) - 1).max(0);
    loop {
        let tick = Ticks(k * period + offset);
        let frame = tempo.ticks_to_frames(tick, rate).get();
        if frame >= from {
            return (tick, frame);
        }
        k += 1;
    }
}

/// Frames in a slice of `ticks` when a quarter note takes `quarter`, kept to
/// the `longest` the buffer holds.
fn slice_frames(quarter: f64, ticks: i64, longest: usize) -> usize {
    ((quarter * ticks as f64 / TICKS_PER_QUARTER as f64).round() as usize).clamp(1, longest)
}

/// One stretch's settings: where each parameter is in the bank, and the
/// ones that hold for the whole stretch.
struct Settings<'a> {
    params: &'a ParamBank,
    index: [usize; PARAMS],
    interval: i64,
    offset: i64,
    gate: i64,
    repeat: bool,
    no_triplets: bool,
    variation_mode: usize,
    filter: bool,
    mode: usize,
}

impl<'a> Settings<'a> {
    fn new(params: &'a ParamBank) -> Self {
        let index = core::array::from_fn(|id| {
            params
                .index(ParamId(id as u32))
                .expect("the description lists every parameter")
        });
        let at = |id: ParamId| params.ramp(index[id.0 as usize])[0];
        let interval = INTERVALS[at(INTERVAL) as usize % INTERVALS.len()];
        Self {
            params,
            index,
            interval,
            offset: (at(OFFSET) as i64 * (TICKS_PER_QUARTER / 4)) % interval,
            gate: at(GATE) as i64 * (TICKS_PER_QUARTER / 4),
            repeat: at(REPEAT) >= 0.5,
            no_triplets: at(NO_TRIPLETS) >= 0.5,
            variation_mode: at(VARIATION_MODE) as usize,
            filter: at(FILTER) >= 0.5,
            mode: at(MODE) as usize,
        }
    }

    /// A parameter's value at frame `i` of the stretch.
    fn at(&self, id: ParamId, i: usize) -> f64 {
        f64::from(self.params.ramp(self.index[id.0 as usize])[i])
    }

    /// The Grid entry a slice follows, varied by `amount` (0 to 1) of
    /// `draw` (−1 to 1).
    fn varied(&self, i: usize, draw: f64) -> usize {
        let base = self.at(GRID, i);
        let amount = self.at(VARIATION, i) / 100.0;
        (base + (draw * amount * VARIATION_STEPS).round()).clamp(0.0, (GRIDS.len() - 1) as f64)
            as usize
    }

    /// A slice of Grid entry `grid` in ticks, binary if No Triplets says.
    fn grid_ticks(&self, grid: usize) -> i64 {
        GRIDS[if self.no_triplets && grid % 2 == 1 {
            grid + 1
        } else {
            grid
        }]
    }
}

impl NativeDevice for Stutter {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        let frames = audio.frames();
        if frames == 0 {
            return;
        }
        let settings = Settings::new(params);
        let tempo = audio.tempo();
        let playing = audio.playing();
        let song = audio.song_position().get();
        let base = if playing { song } else { song + self.idle };
        // A held span ends with Repeat; a span of the interval ends when the
        // song stops or jumps.
        if self.held && !settings.repeat {
            self.span = None;
        }
        if self.span.as_ref().is_some_and(|s| !s.held) && (!playing || base != self.expected) {
            self.span = None;
        }
        let mut press = settings.repeat && !self.held;
        self.held = settings.repeat;

        let mut next_trigger = (playing && !settings.repeat)
            .then(|| next_on_grid(tempo, self.rate, base, settings.interval, settings.offset));
        let line = VARIATION_LINES
            .get(settings.variation_mode.wrapping_sub(1))
            .copied();
        let mut next_line = line.map(|period| next_on_grid(tempo, self.rate, base, period, 0).1);
        for i in 0..frames {
            let clock = base + i as i64;
            if self.span.as_ref().is_some_and(|s| s.end == clock) {
                self.span = None;
            }
            if next_line == Some(clock) {
                if self.span.is_some() {
                    let draw = self.draw(&settings, i);
                    let grid = settings.varied(i, draw);
                    self.span.as_mut().expect("a span").grid = grid;
                }
                next_line =
                    line.map(|period| next_on_grid(tempo, self.rate, clock + 1, period, 0).1);
            }
            if press {
                press = false;
                let tick = tempo.frames_to_ticks(Frames(clock), self.rate);
                self.start(&settings, tempo, tick, true, i);
            } else if let Some((tick, _)) = next_trigger.filter(|(_, frame)| *frame == clock) {
                let chance = (self.random.next_bipolar() + 1.0) / 2.0 * 100.0;
                if self.span.is_none() && chance < settings.at(CHANCE, i) {
                    self.start(&settings, tempo, tick, false, i);
                }
                next_trigger = Some(next_on_grid(
                    tempo,
                    self.rate,
                    clock + 1,
                    settings.interval,
                    settings.offset,
                ));
            }
            self.tune_filter(&settings, i);
            self.frame(audio, &settings, i);
        }
        self.expected = base + frames as i64;
        self.idle = if playing {
            0
        } else {
            self.idle + frames as i64
        };
    }
}

impl Stutter {
    /// The next draw of the source, for Variation.
    fn draw(&mut self, settings: &Settings<'_>, i: usize) -> f64 {
        if settings.at(VARIATION, i) > 0.0 {
            self.random.next_bipolar()
        } else {
            0.0
        }
    }

    /// Starts a capture at `tick`; `held` if Repeat asked.
    fn start(
        &mut self,
        settings: &Settings<'_>,
        tempo: &TempoMap,
        tick: Ticks,
        held: bool,
        i: usize,
    ) {
        let rate = self.rate;
        let frame_of = |ticks: Ticks| tempo.ticks_to_frames(ticks, rate).get();
        let quarter = (frame_of(tick + Ticks::QUARTER) - frame_of(tick)).max(1) as f64;
        let mut grid = settings.at(GRID, i) as usize;
        if settings.variation_mode == 0 {
            let draw = self.draw(settings, i);
            grid = settings.varied(i, draw);
        }
        let capture = slice_frames(quarter, settings.grid_ticks(grid), self.buffers[0].len());
        for filter in self.filters.iter_mut().flatten() {
            filter.reset();
        }
        self.span = Some(Span {
            end: if held {
                i64::MAX
            } else {
                frame_of(tick + Ticks(settings.gate))
            },
            held,
            quarter,
            grid,
            capture_left: capture,
            captured: capture,
            slice: capture,
            position: 0.0,
            speed: 1.0,
            gain: 1.0,
            repeat: 0,
        });
    }

    /// Makes the filter's coefficients for frame `i`, if its setting moved.
    fn tune_filter(&mut self, settings: &Settings<'_>, i: usize) {
        if !settings.filter || self.span.is_none() {
            return;
        }
        let setting = (settings.at(FREQ, i) as f32, settings.at(WIDTH, i) as f32);
        if self.tuned == Some(setting) {
            return;
        }
        self.tuned = Some(setting);
        let rate = self.rate.as_f64();
        let (centre, width) = (f64::from(setting.0), f64::from(setting.1));
        let high = (centre * (width / 2.0).exp2()).min(rate * 0.45);
        let low = (centre * (-width / 2.0).exp2()).clamp(20.0, high * 0.99);
        for [highpass, lowpass] in &mut self.filters {
            highpass.set(low, rate, true);
            lowpass.set(high, rate, false);
        }
    }

    /// Plays frame `i` of the stretch: the input, the capture, the repeats.
    fn frame(&mut self, audio: &mut Audio<'_, '_>, settings: &Settings<'_>, i: usize) {
        let volume = settings.at(VOLUME, i) as f32;
        let channels = audio.channels().min(self.buffers.len());
        let span = self.span.as_ref();
        let repeating = span.is_some_and(|s| s.capture_left == 0);
        for channel in 0..channels {
            let dry = audio.channel_mut(channel)[i];
            let mut repeat = 0.0;
            if let Some(span) = span {
                let buffer = &mut self.buffers[channel];
                if span.capture_left > 0 {
                    buffer[span.captured - span.capture_left] = dry;
                } else {
                    let (low, fraction) = (span.position.floor(), span.position.fract() as f32);
                    let low = low as usize % span.captured;
                    let a = buffer[low];
                    let b = buffer[(low + 1).min(span.captured - 1)];
                    repeat = a + (b - a) * fraction;
                    if settings.filter {
                        let [highpass, lowpass] = &mut self.filters[channel];
                        repeat = lowpass.process(highpass.process(f64::from(repeat))) as f32;
                    }
                    repeat *= span.gain as f32;
                }
            }
            audio.channel_mut(channel)[i] = volume
                * match settings.mode {
                    1 if repeating => repeat,
                    1 => dry,
                    2 => repeat,
                    _ => dry + repeat,
                };
        }
        if let Some(span) = self.span.as_mut() {
            if span.capture_left > 0 {
                span.capture_left -= 1;
                if span.capture_left == 0 {
                    self.next_slice(settings, i);
                }
            } else {
                span.position += span.speed;
                if span.position >= span.slice as f64 {
                    span.position -= span.slice as f64;
                    span.repeat += 1;
                    if settings.variation_mode == 4 {
                        let draw = self.draw(settings, i);
                        self.span.as_mut().expect("a span").grid = settings.varied(i, draw);
                    }
                    self.next_slice(settings, i);
                }
            }
        }
    }

    /// Sets the slice the span repeats next: its length, speed and gain.
    fn next_slice(&mut self, settings: &Settings<'_>, i: usize) {
        let Some(span) = self.span.as_mut() else {
            return;
        };
        let k = f64::from(span.repeat);
        let semitones = (settings.at(PITCH, i)
            + k * settings.at(PITCH_DECAY, i) / 100.0 * PITCH_DECAY_SEMITONES)
            .min(MAX_PITCH);
        span.speed = (-semitones / 12.0).exp2();
        span.gain =
            (-k * settings.at(DECAY, i) / 100.0 * DECAY_DB / 20.0 * std::f64::consts::LN_10).exp();
        let ticks = settings.grid_ticks(span.grid);
        span.slice = slice_frames(span.quarter, ticks, self.buffers[0].len());
    }
}

/// A second-order Butterworth filter, high-pass or low-pass.
#[derive(Clone, Copy, Default)]
struct Biquad {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    z1: f64,
    z2: f64,
}

impl Biquad {
    /// Sets it to cut at `hz`.
    fn set(&mut self, hz: f64, rate: f64, high: bool) {
        let w = TAU * hz / rate;
        let (sin, cos) = w.sin_cos();
        let alpha = sin / SQRT_2;
        let a0 = 1.0 + alpha;
        let sign = if high { 1.0 } else { -1.0 };
        self.b0 = (1.0 + sign * cos) / 2.0 / a0;
        self.b1 = -sign * (1.0 + sign * cos) / a0;
        self.b2 = self.b0;
        self.a1 = -2.0 * cos / a0;
        self.a2 = (1.0 - alpha) / a0;
    }

    fn reset(&mut self) {
        self.z1 = 0.0;
        self.z2 = 0.0;
    }

    fn process(&mut self, x: f64) -> f64 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }
}

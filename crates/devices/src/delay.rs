//! The tempo-synced delay device (WP-2.18b, part 2; the delay spec): two
//! independent delay lines, a band-pass filter in each loop, one LFO, and
//! the three time-change modes.

use std::f64::consts::TAU;

use bl_core::time::TempoMap;
use bl_engine::{DeviceType, NodeConfig, Registry};
use bl_model::device_layout::{BODY_HEIGHT, ControlBand, DisplayCell, LayoutNode, Widget};
use bl_model::{
    Device, DeviceDescription, DeviceLayout, DeviceRole, ParamDescriptor, ParamId, ParamUnit,
};

use crate::framework::{Audio, NativeDevice, ParamBank, add};
use crate::lfo::{Lfo, Rate, Shape, Stereo, Wave};

/// The delay's device type.
pub const DELAY: &str = "bl.device.reprise";

/// Sync: beat divisions on, free milliseconds off.
pub const SYNC: ParamId = ParamId(0);
/// Link: one delay time for both channels.
pub const LINK: ParamId = ParamId(1);
/// Freeze: cycle the buffers, ignoring new input.
pub const FREEZE: ParamId = ParamId(2);
/// Filter: the band-pass in the delay loops.
pub const FILTER: ParamId = ParamId(3);
/// The filter's center frequency.
pub const FREQ: ParamId = ParamId(4);
/// The filter's bandwidth.
pub const WIDTH: ParamId = ParamId(5);
/// How much of each channel's output feeds back.
pub const FEEDBACK: ParamId = ParamId(6);
/// How a change of delay time is handled: Repitch, Fade, or Jump.
pub const TIME_MODE: ParamId = ParamId(7);
/// Ping Pong: the echoes alternate between the channels.
pub const PING_PONG: ParamId = ParamId(8);
/// The dry/wet balance.
pub const DRY_WET: ParamId = ParamId(9);
/// The left channel's sync time, in sixteenth notes.
pub const L_SYNC: ParamId = ParamId(10);
/// The right channel's sync time, in sixteenth notes.
pub const R_SYNC: ParamId = ParamId(11);
/// The left channel's free time, in milliseconds.
pub const L_TIME: ParamId = ParamId(12);
/// The right channel's free time, in milliseconds.
pub const R_TIME: ParamId = ParamId(13);
/// The left channel's offset from its division, in per cent of it.
pub const L_OFFSET: ParamId = ParamId(14);
/// The right channel's offset from its division, in per cent of it.
pub const R_OFFSET: ParamId = ParamId(15);
/// The LFO's rate.
pub const LFO_RATE: ParamId = ParamId(16);
/// The LFO's waveform.
pub const LFO_WAVE: ParamId = ParamId(17);
/// How far the LFO's waveform is morphed toward the next one.
pub const LFO_MORPH: ParamId = ParamId(18);
/// How much the LFO moves the delay time.
pub const LFO_DELAY: ParamId = ParamId(19);
/// How much the LFO moves the filter's frequency.
pub const LFO_FILTER: ParamId = ParamId(20);

/// How many parameters the delay has.
const PARAMS: usize = 21;

/// The longest free delay the manual records, in seconds.
pub(crate) const MAX_DELAY_SECONDS: f64 = 5.0;
/// The LFO's deepest delay-time movement, in seconds.
const MODULATION_SECONDS: f64 = 0.02;
/// How long a change of delay time takes in Repitch and Fade, in seconds.
pub(crate) const TRANSITION_SECONDS: f64 = 0.02;
/// The LFO's highest filter movement, in octaves.
const FILTER_MOD_OCTAVES: f64 = 2.0;

/// The delay's type: Beatlayer's own name and look, Live's parameters
/// (the delay spec, "What Live does").
#[must_use]
pub fn delay_type() -> DeviceType {
    let params = vec![
        ParamDescriptor::toggle(SYNC, "Sync", true)
            .describe("Sets the delay time in beat divisions rather than milliseconds."),
        ParamDescriptor::toggle(LINK, "Link", false)
            .describe("Gives both channels the left channel's delay time."),
        ParamDescriptor::toggle(FREEZE, "Freeze", false)
            .describe("Cycles what is in the delay lines and ignores new input."),
        ParamDescriptor::toggle(FILTER, "Filter", false)
            .describe("Shapes each repeat with a band-pass filter."),
        ParamDescriptor::continuous(FREQ, "Freq", (20.0, 20_000.0), 1_000.0, ParamUnit::Hertz)
            .logarithmic()
            .describe("The band-pass filter's center frequency."),
        ParamDescriptor::continuous(WIDTH, "Width", (0.5, 9.0), 2.5, ParamUnit::None)
            .describe("The band-pass filter's bandwidth."),
        ParamDescriptor::continuous(FEEDBACK, "Feedback", (0.0, 100.0), 50.0, ParamUnit::Percent)
            .describe("How much of the delayed signal feeds back into the delay line."),
        ParamDescriptor::choice(TIME_MODE, "Time Mode", &["Repitch", "Fade", "Jump"], 0)
            .describe("How a change of delay time sounds: a pitch glide, a crossfade, or a cut."),
        ParamDescriptor::toggle(PING_PONG, "Ping Pong", false)
            .describe("Bounces the echoes back and forth between the channels."),
        ParamDescriptor::continuous(DRY_WET, "Dry/Wet", (0.0, 100.0), 100.0, ParamUnit::Percent)
            .describe("The balance between the dry and the delayed signal."),
        ParamDescriptor::integer(L_SYNC, "L Sync", (1, 16), 4, ParamUnit::None)
            .describe("The left channel's delay time in sixteenth notes."),
        ParamDescriptor::integer(R_SYNC, "R Sync", (1, 16), 4, ParamUnit::None)
            .describe("The right channel's delay time in sixteenth notes."),
        ParamDescriptor::continuous(
            L_TIME,
            "L Time",
            (1.0, 5_000.0),
            100.0,
            ParamUnit::Milliseconds,
        )
        .logarithmic()
        .describe("The left channel's delay time in milliseconds."),
        ParamDescriptor::continuous(
            R_TIME,
            "R Time",
            (1.0, 5_000.0),
            100.0,
            ParamUnit::Milliseconds,
        )
        .logarithmic()
        .describe("The right channel's delay time in milliseconds."),
        ParamDescriptor::continuous(L_OFFSET, "L Offset", (-50.0, 50.0), 0.0, ParamUnit::Percent)
            .describe("Moves the left channel's repeat earlier or later within its division."),
        ParamDescriptor::continuous(R_OFFSET, "R Offset", (-50.0, 50.0), 0.0, ParamUnit::Percent)
            .describe("Moves the right channel's repeat earlier or later within its division."),
        ParamDescriptor::continuous(LFO_RATE, "Rate", (0.01, 40.0), 1.0, ParamUnit::Hertz)
            .logarithmic()
            .describe("The LFO's rate."),
        ParamDescriptor::choice(
            LFO_WAVE,
            "Wave",
            &[
                "Sine",
                "Triangle",
                "Ramp Up",
                "Ramp Down",
                "Square",
                "S&H",
                "Wander",
            ],
            0,
        )
        .describe("The LFO's waveform."),
        ParamDescriptor::continuous(LFO_MORPH, "Morph", (0.0, 100.0), 0.0, ParamUnit::Percent)
            .describe("Shapes the LFO's waveform toward the next one."),
        ParamDescriptor::continuous(LFO_DELAY, "Delay", (0.0, 100.0), 0.0, ParamUnit::Percent)
            .describe("How much the LFO moves the delay time."),
        ParamDescriptor::continuous(LFO_FILTER, "Filter", (0.0, 100.0), 0.0, ParamUnit::Percent)
            .describe("How much the LFO moves the filter's frequency."),
    ];
    DeviceType::native(DELAY, "Reprise", DeviceRole::AudioEffect)
        .with_description(DeviceDescription::new(params).with_layout(layout()))
}

/// A layout of fixed control bands and the delay's display, which grows.
fn layout() -> DeviceLayout {
    let toggles = ControlBand::stacked(
        BODY_HEIGHT,
        &[
            (SYNC, Widget::Toggle),
            (LINK, Widget::Toggle),
            (FREEZE, Widget::Toggle),
            (FILTER, Widget::Toggle),
            (PING_PONG, Widget::Toggle),
        ],
    );
    let main = ControlBand::stacked(
        BODY_HEIGHT,
        &[
            (FEEDBACK, Widget::Knob),
            (DRY_WET, Widget::Knob),
            (FREQ, Widget::Knob),
            (WIDTH, Widget::Knob),
        ],
    );
    let times = ControlBand::stacked(
        BODY_HEIGHT,
        &[
            (L_SYNC, Widget::Number),
            (L_TIME, Widget::Knob),
            (R_SYNC, Widget::Number),
            (R_TIME, Widget::Knob),
            (L_OFFSET, Widget::Knob),
            (R_OFFSET, Widget::Knob),
        ],
    );
    let lfo = ControlBand::stacked(
        BODY_HEIGHT,
        &[
            (LFO_RATE, Widget::Knob),
            (LFO_MORPH, Widget::Knob),
            (LFO_DELAY, Widget::Knob),
            (LFO_FILTER, Widget::Knob),
        ],
    );
    let display = DisplayCell::new("Delay", 160.0, BODY_HEIGHT, 1.0)
        .with_top(ControlBand::stacked(40.0, &[(TIME_MODE, Widget::Chooser)]))
        .with_bottom(ControlBand::stacked(40.0, &[(LFO_WAVE, Widget::Chooser)]));
    DeviceLayout::new(LayoutNode::Row(vec![
        LayoutNode::Band(toggles),
        LayoutNode::Band(main),
        LayoutNode::Cell(display),
        LayoutNode::Band(times),
        LayoutNode::Band(lfo),
    ]))
}

/// Adds the delay to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, delay_type(), Delay::new);
}

/// How a change of delay time is handled.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    Repitch,
    Fade,
    Jump,
}

impl Mode {
    fn of(index: usize) -> Self {
        match index {
            1 => Self::Fade,
            2 => Self::Jump,
            _ => Self::Repitch,
        }
    }
}

/// The LFO's waveforms, in the order of the Wave chooser. A waveform's
/// morph blends toward the next one, except for the random ones.
const WAVES: [Wave; 7] = [
    Wave::Sine,
    Wave::Triangle,
    Wave::SawUp,
    Wave::SawDown,
    Wave::Rectangle,
    Wave::SampleHold,
    Wave::Wander,
];

/// The LFO's shape for the Wave chooser's `index`: its waveform, morphed
/// toward the next one by `morph` (0 to 1) except for the random waveforms.
fn lfo_shape(index: usize, morph: f64) -> Shape {
    let index = index.min(WAVES.len() - 1);
    Shape {
        wave: WAVES[index],
        morph: WAVES
            .get(index + 1)
            .filter(|_| index < 5)
            .map(|to| (*to, morph)),
        duty: 0.0,
    }
}

/// The delay: one line per channel, one LFO, and its settings.
struct Delay {
    lines: Vec<Line>,
    lfo: Lfo,
    sample_rate: f64,
    glide: f64,
    mod_depth: f64,
}

impl Delay {
    fn new(device: &Device, config: &NodeConfig) -> Self {
        let rate = config.sample_rate.as_f64();
        let len = ((MAX_DELAY_SECONDS + MODULATION_SECONDS + 0.1) * rate).ceil() as usize + 2;
        Self {
            lines: (0..config.channels).map(|_| Line::new(len)).collect(),
            lfo: Lfo::new(config.sample_rate, device.id.0.get()),
            sample_rate: rate,
            glide: (TRANSITION_SECONDS * rate).max(1.0),
            mod_depth: MODULATION_SECONDS * rate,
        }
    }
}

impl NativeDevice for Delay {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        if audio.frames() == 0 {
            return;
        }
        let settings = Settings::new(audio, params, self.sample_rate, self.glide, self.mod_depth);
        self.lfo.set(
            audio,
            Rate::Hertz(settings.lfo_rate),
            Stereo::Phase(0.0),
            0.0,
        );
        let tempo = audio.tempo();
        let mut channels = audio.channels_mut();
        let Some(first) = channels.next() else {
            return;
        };
        // shortcut: the delay is stereo, as the manual's is; any channel
        // past the second passes through. Upgrade with a surround layout.
        let second = channels.next();
        self.render(first, second, &settings, params, tempo);
    }
}

impl Delay {
    /// Renders the first channel and, when there is one, the second,
    /// bouncing between them while Ping Pong is on.
    fn render(
        &mut self,
        first: &mut [f32],
        mut second: Option<&mut [f32]>,
        settings: &Settings,
        params: &ParamBank,
        tempo: &TempoMap,
    ) {
        let frames = first.len();
        let has_second = second.is_some();
        for i in 0..frames {
            let morph = f64::from(settings.ramp(params, LFO_MORPH)[i]) / 100.0;
            let lfo = self.lfo.step(tempo, i, lfo_shape(settings.wave, morph))[0];
            let modulation =
                lfo * settings.ramp(params, LFO_DELAY)[i] as f64 / 100.0 * settings.mod_depth;
            let frequency = settings.filter_frequency(params, i, lfo);
            let bandwidth = settings.ramp(params, WIDTH)[i] as f64;
            let feedback = settings.ramp(params, FEEDBACK)[i] as f64 / 100.0;
            let dry_wet = settings.ramp(params, DRY_WET)[i] as f64 / 100.0;
            let base_left = settings.base_delay(params, false);
            let base_right = settings.base_delay(params, true);
            let tap_left = self.lines[0].tap(base_left, modulation, settings.mode, settings.glide);
            let tap_right = has_second
                .then(|| self.lines[1].tap(base_right, modulation, settings.mode, settings.glide));
            let input_left = if settings.freeze {
                0.0
            } else {
                f64::from(first[i])
            };
            let output_left = tap_left;
            first[i] = (input_left * (1.0 - dry_wet) + dry_wet * output_left) as f32;
            let write_left = if settings.ping_pong && has_second {
                input_left + feedback * tap_right.unwrap_or(0.0)
            } else {
                input_left + feedback * tap_left
            };
            self.lines[0].push(
                write_left,
                settings.filter_on,
                frequency,
                bandwidth,
                settings.rate,
            );
            if let Some(right) = second.as_deref_mut() {
                let input = if settings.freeze {
                    0.0
                } else {
                    f64::from(right[i])
                };
                let output = tap_right.unwrap_or(0.0);
                let write = if settings.ping_pong {
                    input + feedback * tap_left
                } else {
                    input + feedback * output
                };
                right[i] = (input * (1.0 - dry_wet) + dry_wet * output) as f32;
                self.lines[1].push(
                    write,
                    settings.filter_on,
                    frequency,
                    bandwidth,
                    settings.rate,
                );
            }
        }
    }
}

/// Frames in a sixteenth note at the tempo of the stretch's first frame: the
/// synced delays' unit, shared by the delays (the delay spec, R1).
pub(crate) fn sixteenth_frames(audio: &Audio<'_, '_>) -> f64 {
    let tempo = audio.tempo();
    let ticks = tempo.frames_to_ticks(audio.song_position(), audio.sample_rate());
    audio.sample_rate().as_f64() * 60.0 / tempo.bpm_at(ticks).get() / 4.0
}

/// One stretch's settings, worked out once: the parameters the engine
/// doesn't smooth, each parameter's place in the bank, and the tempo.
struct Settings {
    sync: bool,
    link: bool,
    freeze: bool,
    filter_on: bool,
    ping_pong: bool,
    mode: Mode,
    wave: usize,
    /// Frames in a sixteenth note at the stretch's tempo.
    sixteenth: f64,
    rate: f64,
    glide: f64,
    mod_depth: f64,
    lfo_rate: f64,
    index: [usize; PARAMS],
}

impl Settings {
    fn new(
        audio: &Audio<'_, '_>,
        params: &ParamBank,
        rate: f64,
        glide: f64,
        mod_depth: f64,
    ) -> Self {
        let index = core::array::from_fn(|id| {
            params
                .index(ParamId(id as u32))
                .expect("the description lists every parameter")
        });
        Self {
            sync: params.value(index[SYNC.0 as usize]) >= 0.5,
            link: params.value(index[LINK.0 as usize]) >= 0.5,
            freeze: params.value(index[FREEZE.0 as usize]) >= 0.5,
            filter_on: params.value(index[FILTER.0 as usize]) >= 0.5,
            ping_pong: params.value(index[PING_PONG.0 as usize]) >= 0.5,
            mode: Mode::of(params.value(index[TIME_MODE.0 as usize]) as usize),
            wave: params.value(index[LFO_WAVE.0 as usize]) as usize,
            sixteenth: sixteenth_frames(audio),
            rate,
            glide,
            mod_depth,
            lfo_rate: params.target(index[LFO_RATE.0 as usize]),
            index,
        }
    }

    /// A parameter's value at each frame of the stretch.
    fn ramp<'a>(&self, params: &'a ParamBank, id: ParamId) -> &'a [f32] {
        params.ramp(self.index[id.0 as usize])
    }

    /// The parameter's value now.
    fn value(&self, params: &ParamBank, id: ParamId) -> f64 {
        params.value(self.index[id.0 as usize])
    }

    /// The value the parameter is heading for.
    fn target(&self, params: &ParamBank, id: ParamId) -> f64 {
        params.target(self.index[id.0 as usize])
    }

    /// One channel's delay before modulation, in frames. The delay time is
    /// the parameter's target rather than its smoothed value: the time
    /// change modes (Repitch, Fade, Jump) do the smoothing themselves.
    fn base_delay(&self, params: &ParamBank, right: bool) -> f64 {
        if self.sync {
            let division = if right && !self.link {
                self.value(params, R_SYNC)
            } else {
                self.value(params, L_SYNC)
            };
            let offset = if right {
                self.target(params, R_OFFSET)
            } else {
                self.target(params, L_OFFSET)
            };
            division * self.sixteenth * (1.0 + offset / 100.0)
        } else {
            let milliseconds = if right && !self.link {
                self.target(params, R_TIME)
            } else {
                self.target(params, L_TIME)
            };
            milliseconds * self.rate / 1_000.0
        }
    }

    /// The filter's frequency for this frame, with the LFO's movement.
    fn filter_frequency(&self, params: &ParamBank, frame: usize, lfo: f64) -> f64 {
        let frequency = f64::from(self.ramp(params, FREQ)[frame]);
        let amount = f64::from(self.ramp(params, LFO_FILTER)[frame]) / 100.0;
        (frequency * (lfo * amount * FILTER_MOD_OCTAVES).exp2()).clamp(20.0, self.rate * 0.45)
    }
}

/// One channel's delay line, with its filter and its time change.
pub(crate) struct Line {
    buffer: Vec<f32>,
    write: usize,
    /// The delay in frames the line is reading at.
    delay: f64,
    /// A time change's start and end, and how far it has come.
    from: f64,
    to: f64,
    progress: f64,
    started: bool,
    filter: BandPass,
}

impl Line {
    pub(crate) fn new(len: usize) -> Self {
        Self {
            buffer: vec![0.0; len],
            write: 0,
            delay: 0.0,
            from: 0.0,
            to: 0.0,
            progress: 1.0,
            started: false,
            filter: BandPass::default(),
        }
    }

    /// The delayed sample for this frame, moving to `base` as `mode` says.
    pub(crate) fn tap(&mut self, base: f64, modulation: f64, mode: Mode, glide: f64) -> f64 {
        if !self.started {
            self.started = true;
            self.delay = base;
            self.from = base;
            self.to = base;
            self.progress = 1.0;
        } else if (base - self.to).abs() > 1e-9 {
            self.from = self.delay;
            self.to = base;
            self.progress = 0.0;
        }
        match mode {
            Mode::Jump => {
                self.progress = 1.0;
                self.delay = self.to;
                self.read(self.to + modulation)
            }
            Mode::Repitch => {
                if self.progress < 1.0 {
                    self.progress = (self.progress + 1.0 / glide).min(1.0);
                    self.delay = self.from + (self.to - self.from) * self.progress;
                }
                self.read(self.delay + modulation)
            }
            Mode::Fade => {
                if self.progress >= 1.0 {
                    self.delay = self.to;
                    self.read(self.to + modulation)
                } else {
                    let old = self.read(self.from + modulation);
                    let new = self.read(self.to + modulation);
                    let value = old + (new - old) * self.progress;
                    self.progress = (self.progress + 1.0 / glide).min(1.0);
                    self.delay = self.to;
                    value
                }
            }
        }
    }

    /// Reads the line `delay` frames back, interpolating between samples.
    pub(crate) fn read(&self, delay: f64) -> f64 {
        let len = self.buffer.len();
        let delay = delay.clamp(1.0, (len - 2) as f64);
        let position = (self.write as f64 - delay).rem_euclid(len as f64);
        let low = position.floor();
        let i = low as usize;
        let fraction = (position - low) as f32;
        let a = self.buffer[i];
        let b = self.buffer[(i + 1) % len];
        f64::from(a + (b - a) * fraction)
    }

    /// Writes `value` into the line, through the band-pass when it's on,
    /// and moves the write head on.
    fn push(&mut self, value: f64, filter_on: bool, freq: f64, bandwidth: f64, rate: f64) {
        let sample = if filter_on {
            self.filter.set(freq, bandwidth, rate);
            self.filter.process(value as f32)
        } else {
            value as f32
        };
        self.write(sample);
    }

    /// Writes `sample` into the line and moves the write head on.
    pub(crate) fn write(&mut self, sample: f32) {
        let write = self.write;
        self.buffer[write] = sample;
        self.write = (write + 1) % self.buffer.len();
    }
}

/// A band-pass filter for the delay loops, the manual's Freq and Width.
#[derive(Clone, Copy, Default)]
struct BandPass {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
}

impl BandPass {
    /// Sets its coefficients for `freq` and `bandwidth`.
    fn set(&mut self, freq: f64, bandwidth: f64, rate: f64) {
        let w0 = TAU * freq / rate;
        let (sin, cos) = w0.sin_cos();
        let alpha = sin / (2.0 * bandwidth.max(0.1));
        let a0 = 1.0 + alpha;
        self.b0 = (alpha / a0) as f32;
        self.b1 = 0.0;
        self.b2 = (-alpha / a0) as f32;
        self.a1 = (-2.0 * cos / a0) as f32;
        self.a2 = ((1.0 - alpha) / a0) as f32;
    }

    /// One sample through it.
    fn process(&mut self, sample: f32) -> f32 {
        let out = self.b0 * sample + self.z1;
        self.z1 = self.b1 * sample - self.a1 * out + self.z2;
        self.z2 = self.b2 * sample - self.a2 * out;
        out
    }
}

#[cfg(test)]
mod tests {
    use bl_core::time::{Bpm, TempoMap};

    use super::*;

    // Spec: delay R8
    #[test]
    fn every_lfo_wave_stays_between_minus_one_and_one() {
        let tempo = TempoMap::constant(Bpm::DEFAULT);
        let sample_rate = bl_core::time::SampleRate::HZ_48000;
        for wave in 0..WAVES.len() {
            for morph in [0.0, 0.5, 1.0] {
                let mut lfo = Lfo::new(sample_rate, 3);
                let shape = lfo_shape(wave, morph);
                for frame in 0..4_800 {
                    let value = lfo.step(&tempo, frame, shape)[0];
                    assert!((-1.0..=1.0).contains(&value), "wave {wave}: {value}");
                }
            }
        }
        // Morphing half-way between the ramp up and the ramp down is flat.
        let mut lfo = Lfo::new(sample_rate, 3);
        let shape = lfo_shape(2, 0.5);
        assert_eq!(shape.morph, Some((Wave::SawDown, 0.5)));
        assert!(lfo.step(&tempo, 0, shape)[0].abs() < 1e-9, "morphed");
        // The random waves ignore the morph.
        assert_eq!(lfo_shape(5, 0.5).morph, None);
        assert_eq!(lfo_shape(6, 0.5).morph, None);
    }
}

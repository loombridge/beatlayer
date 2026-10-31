//! CV Envelope Follower, CV LFO and CV Shaper (WP-7.18b; the CV tools spec,
//! R32 to R36): audio effects that leave their audio alone and send the
//! signal of the Envelope Follower, the LFO or the Shaper to one CV output
//! as Min + Depth · u · (Max − Min) volts.
//!
//! The signal is the modulators' own: the LFO's and the Shaper's [`Signal`],
//! the Envelope Follower's [`Follower`], so the laws can't drift. Where a
//! modulator works its output out every [`GRID`] frames and runs a straight
//! line between, these work it out on every frame, so a square wave's edge, a
//! step or a shape's jump lands on its frame (R33).

use bl_core::time::SampleRate;
use bl_dsp::Random;
use bl_engine::{DeviceType, NodeData, Registry};
use bl_model::device_layout::{BODY_HEIGHT, ControlBand, DeviceLayout, LayoutNode, Widget};
use bl_model::modulator::{CV_CURVE, CV_LEVEL, CV_WAVE, DEPTH, default_shape};
use bl_model::{
    DeviceDescription, DeviceDisplay, DeviceId, DeviceRole, DisplayReading, OrdMap,
    ParamDescriptor, ParamId, ParamUnit,
};

use crate::cv::{VOLTS_PER_SAMPLE, sample, volts_display};
use crate::framework::{Audio, NativeDevice, ParamBank, add};
use crate::level_mod::{
    self, Controls, DISPLAY_VALUES, Follower, LEVEL_BANK, level_params, level_parts,
};
use crate::modulator::{
    CURVE_BANKS, CurveSource, Eval, GRID, Signal, Source, WAVE_BANKS, WaveSource, curve_params,
    curve_parts, wave_params, wave_parts,
};

/// Min: the voltage a signal of 0 gives, from −10 to +10 V.
pub const MIN: ParamId = ParamId(20);
/// Max: the voltage a signal of 1 gives, from −10 to +10 V.
pub const MAX: ParamId = ParamId(21);

/// The output's name in the devices' routing.
pub const OUTPUT_NAMES: [&str; 1] = ["CV"];

/// The parameters of the output stage (R32): Depth, Min and Max, with
/// `min` and `max` their defaults in volts.
fn output_params(min: f64, max: f64) -> Vec<ParamDescriptor> {
    let volts = |id, name: &str, default, what: &str| {
        ParamDescriptor::continuous(id, name, (-10.0, 10.0), default, ParamUnit::None)
            .decimals(2)
            .describe(what)
    };
    vec![
        ParamDescriptor::continuous(DEPTH, "Depth", (0.0, 100.0), 100.0, ParamUnit::Percent)
            .decimals(0)
            .describe(
                "How much of the span from Min to Max the signal reaches: 0 % none, 100 % all.",
            ),
        volts(
            MIN,
            "Min",
            min,
            "The voltage a signal of 0 gives, in volts. Above Max it inverts the output.",
        ),
        volts(
            MAX,
            "Max",
            max,
            "The voltage a signal of 1 gives, in volts.",
        ),
    ]
}

/// The band of the output stage's controls.
fn output_band() -> LayoutNode {
    LayoutNode::Band(ControlBand::stacked(
        BODY_HEIGHT,
        &[
            (DEPTH, Widget::Knob),
            (MIN, Widget::Knob),
            (MAX, Widget::Knob),
        ],
    ))
}

const OUTPUT_BANK: (&str, &[&str]) = ("Output", &["Depth", "Min", "Max"]);

/// CV LFO's type.
#[must_use]
pub fn cv_wave_type() -> DeviceType {
    let mut params = wave_params();
    params.extend(output_params(-5.0, 5.0));
    let mut nodes = wave_parts().to_vec();
    nodes.push(output_band());
    let banks = [WAVE_BANKS[0], WAVE_BANKS[1], OUTPUT_BANK];
    DeviceType::native(CV_WAVE, "CV LFO", DeviceRole::AudioEffect)
        .with_cv(&OUTPUT_NAMES, &[])
        .with_description(
            DeviceDescription::new(params)
                .with_layout(DeviceLayout::new(LayoutNode::Row(nodes)))
                .with_banks(&banks),
        )
}

/// CV Shaper's type.
#[must_use]
pub fn cv_curve_type() -> DeviceType {
    let mut params = curve_params();
    params.extend(output_params(0.0, 5.0));
    let mut nodes = curve_parts().to_vec();
    nodes.push(output_band());
    let banks = [CURVE_BANKS[0], CURVE_BANKS[1], OUTPUT_BANK];
    DeviceType::native(CV_CURVE, "CV Shaper", DeviceRole::AudioEffect)
        .with_cv(&OUTPUT_NAMES, &[])
        .with_description(
            DeviceDescription::new(params)
                .with_layout(DeviceLayout::new(LayoutNode::Row(nodes)))
                .with_banks(&banks),
        )
}

/// CV Envelope Follower's type.
#[must_use]
pub fn cv_level_type() -> DeviceType {
    let mut params = level_params();
    params.extend(output_params(0.0, 5.0));
    let mut nodes = level_parts().to_vec();
    nodes.push(output_band());
    let banks = [LEVEL_BANK, OUTPUT_BANK];
    DeviceType::native(CV_LEVEL, "CV Envelope Follower", DeviceRole::AudioEffect)
        .with_sidechain()
        .with_cv(&OUTPUT_NAMES, &[])
        .with_description(
            DeviceDescription::new(params)
                .with_layout(DeviceLayout::new(LayoutNode::Row(nodes)))
                .with_banks(&banks),
        )
}

/// Adds the three devices to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, cv_wave_type(), |device, config| {
        CvSignal::new(WaveSource::new(), config.sample_rate, device.id.0.get())
    });
    add(registry, cv_curve_type(), |device, config| {
        CvSignal::new(
            CurveSource::new(device.shape.clone().unwrap_or_else(default_shape)),
            config.sample_rate,
            device.id.0.get(),
        )
    });
    add(registry, cv_level_type(), |_, config| CvLevel {
        follower: Follower::new(config.sample_rate.as_f64(), config.max_frames),
        shown: 0.0,
    });
}

/// The display of CV LFO (R9, R22): the waveform as the controls set it,
/// and the voltage the output last wrote.
#[must_use]
pub fn wave_display(
    device: DeviceId,
    description: &DeviceDescription,
    values: &OrdMap<ParamId, f64>,
    data: Option<&[f32]>,
) -> DeviceDisplay {
    let mut out = crate::modulator::wave_display(device, description, values);
    out.live = data.is_some();
    out.readings = curve_display(device, data).readings;
    out
}

/// The display of CV Shaper (R9): the voltage the output last wrote.
#[must_use]
pub fn curve_display(device: DeviceId, data: Option<&[f32]>) -> DeviceDisplay {
    volts_display(device, OUTPUT_NAMES.map(str::to_owned), data)
}

/// The display of CV Envelope Follower (R9, R36): Direct and S.C. in dB, the
/// level, and the voltage the output last wrote.
#[must_use]
pub fn level_display(
    device: DeviceId,
    description: &DeviceDescription,
    values: &OrdMap<ParamId, f64>,
    data: Option<&[f32]>,
) -> DeviceDisplay {
    let mut out = level_mod::display(
        device,
        description,
        values,
        data.map(|data| &data[..DISPLAY_VALUES.min(data.len())]),
    );
    let volts = data.and_then(|data| data.get(DISPLAY_VALUES)).copied();
    out.readings.push(DisplayReading {
        name: OUTPUT_NAMES[0].to_owned(),
        value: volts.unwrap_or(0.0),
        unit: "V".to_owned(),
        text: format!("{:.2} V", volts.unwrap_or(0.0)),
    });
    out
}

/// The output stage the three share (R32): Min + Depth · u · (Max − Min)
/// volts for a signal `u` from 0 to 1, written to the CV output.
struct Stage {
    depth: usize,
    min: usize,
    max: usize,
}

impl Stage {
    fn new(params: &ParamBank) -> Self {
        let index = |id: ParamId| params.index(id).expect("the description holds it");
        Self {
            depth: index(DEPTH),
            min: index(MIN),
            max: index(MAX),
        }
    }

    /// Writes frame `f`'s voltage for the signal `u` and returns it, clamped
    /// to ±10 V (cv-tools R1).
    fn write(&self, audio: &mut Audio<'_, '_>, params: &ParamBank, f: usize, u: f64) -> f32 {
        let value = |index: usize| f64::from(params.ramp(index)[f]);
        let (min, max) = (value(self.min), value(self.max));
        let volts = min + value(self.depth) / 100.0 * u * (max - min);
        if let Some(out) = audio.cv_out(0) {
            out[f] = sample(volts);
        }
        volts.clamp(-VOLTS_PER_SAMPLE, VOLTS_PER_SAMPLE) as f32
    }
}

/// Jitter's draws, which the modulators take once every [`GRID`] frames and
/// run a straight line between: the same draws, so the signal on a grid
/// frame is the modulator's.
#[derive(Default)]
struct Noise {
    from: f64,
    to: f64,
    primed: bool,
}

impl Noise {
    /// The noise `t` of the way from the draw before the last grid frame to
    /// the one after it; `aligned` on a grid frame, where a new draw is made.
    fn at(&mut self, random: &mut Random, aligned: bool, t: f64) -> f64 {
        if !self.primed {
            self.from = random.next_bipolar();
            self.to = random.next_bipolar();
            self.primed = true;
        } else if aligned {
            self.from = self.to;
            self.to = random.next_bipolar();
        }
        self.from + (self.to - self.from) * t
    }
}

/// CV LFO and CV Shaper: a source's signal worked out on every frame (R33).
struct CvSignal<S> {
    signal: Signal<S>,
    rate: SampleRate,
    /// Frames since the transport last started, or since the first frame,
    /// which place the grid Jitter's draws are taken on.
    clock: u64,
    was_playing: bool,
    noise: Noise,
    shown: f32,
}

impl<S: Source> CvSignal<S> {
    fn new(source: S, rate: SampleRate, seed: u64) -> Self {
        Self {
            signal: Signal::new(source, seed),
            rate,
            clock: 0,
            was_playing: false,
            noise: Noise::default(),
            shown: 0.0,
        }
    }
}

impl<S: Source> NativeDevice for CvSignal<S> {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        let playing = audio.playing();
        if playing && !self.was_playing {
            // Spec: modulation R20, R21. A free cycle starts again from its
            // Phase, and the random parts from the seed.
            self.clock = 0;
            self.noise = Noise::default();
            self.signal.restart();
        }
        self.was_playing = playing;
        let stage = Stage::new(params);
        let (tempo, speed, position) = (audio.tempo(), audio.speed(), audio.song_position().0);
        for f in 0..audio.frames() {
            // The song position stands still while the song is stopped.
            let song = if playing {
                position + (f as f64 * speed).round() as i64
            } else {
                position
            };
            let e = Eval {
                rate: self.rate,
                tempo,
                song,
                speed,
                f,
                step: 1,
            };
            let aligned = self.clock.is_multiple_of(GRID);
            let t = (self.clock % GRID) as f64 / GRID as f64;
            let noise = &mut self.noise;
            let u = self
                .signal
                .value(&e, params, |random| noise.at(random, aligned, t));
            self.shown = stage.write(audio, params, f, u);
            self.clock += 1;
        }
    }

    fn receive_data(&mut self, data: NodeData) {
        self.signal.receive(&data);
    }

    fn display_len(&self) -> usize {
        OUTPUT_NAMES.len()
    }

    fn display(&mut self, out: &mut [f32]) {
        out[0] = self.shown;
    }
}

/// CV Envelope Follower: the follower's level on every frame (R36).
struct CvLevel {
    follower: Follower,
    shown: f32,
}

impl NativeDevice for CvLevel {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        self.follower.hear(audio);
        let controls = Controls::new(params, audio.bpm());
        let stage = Stage::new(params);
        for f in 0..audio.frames() {
            let level = self.follower.frame(&controls, f);
            self.shown = stage.write(audio, params, f, level);
        }
    }

    fn display_len(&self) -> usize {
        DISPLAY_VALUES + 1
    }

    fn display(&mut self, out: &mut [f32]) {
        self.follower.meters(out);
        out[DISPLAY_VALUES] = self.shown;
    }
}

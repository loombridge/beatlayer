//! Auto Pan-Tremolo (WP-5.20c; the auto-pan-tremolo spec): LFO modulation of stereo
//! position or amplitude, nine waveforms, six time modes (Hz, Time, Synced, Dotted,
//! Triplet, 16th), Phase and Spin stereo offsets, Phase Offset grid shift and random
//! width, Invert, Tremolo Harmonic (600 Hz crossover) and Vintage, Amount, Shape,
//! Attack Time, and Dynamic Frequency Modulation.
//!
//! Processing runs on the audio thread without allocating, locking, or blocking.
//! Display cells publish current pan and tremolo level for headless reading.

use core::f64::consts::{FRAC_PI_4, PI, SQRT_2};

use bl_engine::{DeviceType, NodeConfig, Registry};
use bl_model::device_layout::{BODY_HEIGHT, ControlBand, DisplayCell, LayoutNode, Widget};
use bl_model::{
    Device, DeviceDescription, DeviceDisplay, DeviceId, DeviceLayout, DeviceRole, DisplayReading,
    OrdMap, ParamDescriptor, ParamId, ParamUnit,
};

use crate::framework::{Audio, NativeDevice, ParamBank, add};
use crate::lfo::{Lfo, Rate, Shape, Stereo, Wave};

/// Auto Pan-Tremolo device type.
pub const AUTO_PAN_TREMOLO: &str = "beatlayer.auto_pan_tremolo";

/// Mode: Panning or Tremolo.
pub const MODE: ParamId = ParamId(0);
/// Waveform: one of [`WAVES`].
pub const WAVE: ParamId = ParamId(1);
/// Time Mode: one of [`TIME_MODES`].
pub const TIME_MODE: ParamId = ParamId(2);
/// Rate in Hz.
pub const RATE_HZ: ParamId = ParamId(3);
/// Time in seconds.
pub const TIME_SECS: ParamId = ParamId(4);
/// Synced division: one of [`SYNCED_DIVISIONS`].
pub const SYNCED_DIVISION: ParamId = ParamId(5);
/// Dotted division: one of [`DOTTED_DIVISIONS`].
pub const DOTTED_DIVISION: ParamId = ParamId(6);
/// Triplet division: one of [`TRIPLET_DIVISIONS`].
pub const TRIPLET_DIVISION: ParamId = ParamId(7);
/// Sixteenth notes per cycle.
pub const SIXTEENTHS: ParamId = ParamId(8);
/// Stereo mode: Phase or Spin.
pub const STEREO_MODE: ParamId = ParamId(9);
/// Phase in degrees.
pub const PHASE: ParamId = ParamId(10);
/// Spin percentage.
pub const SPIN: ParamId = ParamId(11);
/// Phase Offset in degrees.
pub const PHASE_OFFSET: ParamId = ParamId(12);
/// Invert polarity.
pub const INVERT: ParamId = ParamId(13);
/// Harmonic mode toggle (Tremolo only).
pub const HARMONIC: ParamId = ParamId(14);
/// Vintage mode toggle (Tremolo only).
pub const VINTAGE: ParamId = ParamId(15);
/// Modulation Amount in percent.
pub const AMOUNT: ParamId = ParamId(16);
/// Shape in percent.
pub const SHAPE: ParamId = ParamId(17);
/// Attack Time in milliseconds.
pub const ATTACK_TIME: ParamId = ParamId(18);
/// Dynamic Frequency Modulation in percent.
pub const DYNAMIC_FM: ParamId = ParamId(19);

/// The modes in the chooser's order.
pub const MODES: [&str; 2] = ["Panning", "Tremolo"];

/// The LFO's waveforms in the chooser's order.
pub const WAVES: [&str; 9] = [
    "Sine",
    "Triangle",
    "Shark Tooth",
    "Saw Up",
    "Saw Down",
    "Square",
    "Random",
    "Wander",
    "Sample & Hold",
];

const WAVE_OF: [Wave; 9] = [
    Wave::Sine,
    Wave::Triangle,
    Wave::SharkTooth,
    Wave::SawUp,
    Wave::SawDown,
    Wave::Rectangle,
    Wave::Random,
    Wave::Wander,
    Wave::SampleHold,
];

/// The time modes in the chooser's order.
pub const TIME_MODES: [&str; 6] = ["Rate", "Time", "Synced", "Dotted", "Triplet", "16th"];

/// Straight beat divisions.
pub const SYNCED_DIVISIONS: [(&str, f64); 9] = [
    ("1/32", 0.125),
    ("1/16", 0.25),
    ("1/8", 0.5),
    ("1/4", 1.0),
    ("1/2", 2.0),
    ("1 bar", 4.0),
    ("2 bars", 8.0),
    ("4 bars", 16.0),
    ("8 bars", 32.0),
];

/// Dotted beat divisions.
pub const DOTTED_DIVISIONS: [(&str, f64); 7] = [
    ("1/16.", 0.375),
    ("1/8.", 0.75),
    ("1/4.", 1.5),
    ("1/2.", 3.0),
    ("1 bar.", 6.0),
    ("2 bars.", 12.0),
    ("4 bars.", 24.0),
];

/// Triplet beat divisions.
pub const TRIPLET_DIVISIONS: [(&str, f64); 6] = [
    ("1/16T", 1.0 / 6.0),
    ("1/8T", 1.0 / 3.0),
    ("1/4T", 2.0 / 3.0),
    ("1/2T", 4.0 / 3.0),
    ("1 bar T", 8.0 / 3.0),
    ("2 bars T", 16.0 / 3.0),
];

/// 600 Hz crossover filter for Harmonic tremolo mode.
#[derive(Clone, Debug)]
struct Crossover {
    b0: f64,
    b1: f64,
    a1: f64,
    x_prev: [f64; 2],
    y_prev: [f64; 2],
}

impl Crossover {
    fn new(sample_rate: f64) -> Self {
        let k = (PI * 600.0 / sample_rate).tan();
        let b0 = k / (1.0 + k);
        let b1 = b0;
        let a1 = -(1.0 - k) / (1.0 + k);
        Self {
            b0,
            b1,
            a1,
            x_prev: [0.0; 2],
            y_prev: [0.0; 2],
        }
    }

    /// Splits input `x` on channel `ch` into low-pass and high-pass bands.
    /// Their sum is identically `x`.
    fn split(&mut self, ch: usize, x: f32) -> (f32, f32) {
        let x_f64 = f64::from(x);
        let y_lp = self.b0 * x_f64 + self.b1 * self.x_prev[ch] - self.a1 * self.y_prev[ch];
        self.x_prev[ch] = x_f64;
        self.y_prev[ch] = y_lp;
        let y_hp = x_f64 - y_lp;
        (y_lp as f32, y_hp as f32)
    }
}

/// Detects signal onsets/transients and ramps modulation depth over Attack Time.
#[derive(Clone, Debug)]
struct OnsetDetector {
    sample_rate: f64,
    fast_env: f64,
    slow_env: f64,
    depth: f64,
    holdoff: usize,
}

impl OnsetDetector {
    fn new(sample_rate: f64) -> Self {
        Self {
            sample_rate,
            fast_env: 0.0,
            slow_env: 0.0,
            depth: 1.0,
            holdoff: 0,
        }
    }

    fn step(&mut self, input_mag: f64, attack_time_ms: f64) -> f64 {
        if attack_time_ms <= 0.01 {
            self.depth = 1.0;
            return 1.0;
        }

        let alpha_fast = if input_mag > self.fast_env {
            1.0 - (-1.0 / (0.002 * self.sample_rate)).exp()
        } else {
            1.0 - (-1.0 / (0.020 * self.sample_rate)).exp()
        };
        self.fast_env += alpha_fast * (input_mag - self.fast_env);

        let alpha_slow = if input_mag > self.slow_env {
            1.0 - (-1.0 / (0.050 * self.sample_rate)).exp()
        } else {
            1.0 - (-1.0 / (0.200 * self.sample_rate)).exp()
        };
        self.slow_env += alpha_slow * (input_mag - self.slow_env);

        if self.holdoff > 0 {
            self.holdoff -= 1;
        } else {
            let is_onset = (self.fast_env > self.slow_env * 1.8 + 0.01)
                || (self.slow_env < 0.005 && input_mag > 0.02);
            if is_onset {
                self.depth = 0.0;
                self.holdoff = (0.020 * self.sample_rate) as usize;
            }
        }

        let attack_frames = (attack_time_ms / 1000.0 * self.sample_rate).max(1.0);
        let attack_step = 1.0 / attack_frames;
        self.depth = (self.depth + attack_step).min(1.0);
        self.depth
    }
}

/// Tracks signal level for Dynamic Frequency Modulation.
#[derive(Clone, Debug)]
struct LevelTracker {
    sample_rate: f64,
    level: f64,
}

impl LevelTracker {
    fn new(sample_rate: f64) -> Self {
        Self {
            sample_rate,
            level: 0.0,
        }
    }

    fn step(&mut self, input_mag: f64) -> f64 {
        let alpha = 1.0 - (-1.0 / (0.015 * self.sample_rate)).exp();
        self.level += alpha * (input_mag - self.level);
        self.level
    }
}

/// The Auto Pan-Tremolo audio-thread processor.
pub struct AutoPanTremolo {
    lfo: Lfo,
    lfo_right_random: Lfo,
    crossover: Crossover,
    onset: OnsetDetector,
    tracker: LevelTracker,
    last_pan: f32,
    last_tremolo: f32,
}

impl AutoPanTremolo {
    /// Creates a new device instance for `config`.
    #[must_use]
    pub fn new(device: &Device, config: &NodeConfig) -> Self {
        let sample_rate = config.sample_rate.as_f64();
        let seed = device.id.0.get();
        Self {
            lfo: Lfo::new(config.sample_rate, seed),
            lfo_right_random: Lfo::new(config.sample_rate, seed.wrapping_add(1)),
            crossover: Crossover::new(sample_rate),
            onset: OnsetDetector::new(sample_rate),
            tracker: LevelTracker::new(sample_rate),
            last_pan: 0.0,
            last_tremolo: 1.0,
        }
    }
}

impl NativeDevice for AutoPanTremolo {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        let frames = audio.frames();
        let channels = audio.channels().min(2);
        if frames == 0 || channels == 0 {
            return;
        }

        let at = |id: ParamId| params.index(id).expect("the description holds it");
        let on = |id: ParamId| params.value(at(id)) >= 0.5;
        let choice = |id: ParamId| params.value(at(id)) as usize;
        let target = |id: ParamId| params.target(at(id));
        let ramp = |id: ParamId| params.ramp(at(id));

        let mode = choice(MODE);
        let wave_idx = choice(WAVE).min(WAVES.len() - 1);
        let wave = WAVE_OF[wave_idx];
        let is_random = wave.is_random();

        let time_mode = choice(TIME_MODE);
        let rate = match time_mode {
            0 => Rate::Hertz(target(RATE_HZ)),
            1 => Rate::Seconds(target(TIME_SECS)),
            2 => Rate::Beats(
                SYNCED_DIVISIONS[choice(SYNCED_DIVISION).min(SYNCED_DIVISIONS.len() - 1)].1,
            ),
            3 => Rate::Beats(
                DOTTED_DIVISIONS[choice(DOTTED_DIVISION).min(DOTTED_DIVISIONS.len() - 1)].1,
            ),
            4 => Rate::Beats(
                TRIPLET_DIVISIONS[choice(TRIPLET_DIVISION).min(TRIPLET_DIVISIONS.len() - 1)].1,
            ),
            _ => Rate::Beats(0.25 * target(SIXTEENTHS).round().max(1.0)),
        };

        let stereo = if choice(STEREO_MODE) == 1 && !is_random {
            Stereo::Spin(target(SPIN))
        } else if is_random {
            Stereo::Phase(0.0)
        } else {
            Stereo::Phase(target(PHASE))
        };

        let phase_offset = target(PHASE_OFFSET);
        let offset_for_lfo = if is_random { 0.0 } else { phase_offset };

        self.lfo.set(audio, rate, stereo, offset_for_lfo);
        if is_random {
            self.lfo_right_random.set(audio, rate, stereo, 0.0);
        }

        let invert = on(INVERT);
        let harmonic = on(HARMONIC);
        let vintage = on(VINTAGE);
        let shape_val = target(SHAPE) / 100.0;
        let attack_ms = target(ATTACK_TIME);
        let dfm = target(DYNAMIC_FM) / 100.0;
        let amount_ramp = ramp(AMOUNT);

        let shape = Shape::of(wave);

        for (i, &amt_val) in (0..frames).zip(amount_ramp) {
            let in_l = audio.channel_mut(0)[i];
            let in_r = if channels > 1 {
                audio.channel_mut(1)[i]
            } else {
                in_l
            };
            let input_mag = (f64::from(in_l.abs()).max(f64::from(in_r.abs()))).min(2.0);

            let depth = self.onset.step(input_mag, attack_ms);
            let level = self.tracker.step(input_mag);
            let rate_mult = 2.0f64.powf(2.0 * dfm * level);

            let mut lfo_out = self
                .lfo
                .step_with_multiplier(audio.tempo(), i, shape, rate_mult);

            if is_random {
                let width = (phase_offset.abs() / 180.0).clamp(0.0, 1.0);
                let r1 =
                    self.lfo_right_random
                        .step_with_multiplier(audio.tempo(), i, shape, rate_mult)[0];
                lfo_out[1] = lfo_out[0] + width * (r1 - lfo_out[0]);
            }

            if invert {
                lfo_out[0] = -lfo_out[0];
                lfo_out[1] = -lfo_out[1];
            }

            let amount = (f64::from(amt_val) / 100.0).clamp(0.0, 1.0);
            let eff_amount = amount * depth;

            if mode == 0 {
                // Panning mode
                let shape_pan = |v: f64| {
                    if shape_val > 0.0 {
                        let k = 1.0 + 3.0 * shape_val;
                        (k * v).tanh() / k.tanh()
                    } else if shape_val < 0.0 {
                        let k = 1.0 + shape_val;
                        v * k
                    } else {
                        v
                    }
                };
                let s0 = shape_pan(lfo_out[0]);
                let s1 = shape_pan(lfo_out[1]);

                let pan = (0.5 * (s1 - s0) * eff_amount).clamp(-1.0, 1.0);
                self.last_pan = pan as f32;

                let gl = SQRT_2 * ((1.0 - pan) * FRAC_PI_4).sin();
                let gr = SQRT_2 * ((1.0 + pan) * FRAC_PI_4).sin();

                audio.channel_mut(0)[i] = (f64::from(in_l) * gl) as f32;
                if channels > 1 {
                    audio.channel_mut(1)[i] = (f64::from(in_r) * gr) as f32;
                }
            } else {
                // Tremolo mode
                let shape_tremolo = |v: f64| {
                    let u = (v + 1.0) * 0.5;
                    let u_shaped = if shape_val > 0.0 {
                        u.powf(1.0 - 0.75 * shape_val)
                    } else if shape_val < 0.0 {
                        u.powf(1.0 + 3.0 * shape_val.abs())
                    } else {
                        u
                    };
                    u_shaped.clamp(0.0, 1.0)
                };

                let u0 = shape_tremolo(lfo_out[0]);
                let u1 = shape_tremolo(lfo_out[1]);
                self.last_pan = 0.0;
                self.last_tremolo = u0 as f32;

                if harmonic {
                    let (lp_l, hp_l) = self.crossover.split(0, in_l);
                    let (lp_r, hp_r) = if channels > 1 {
                        self.crossover.split(1, in_r)
                    } else {
                        (lp_l, hp_l)
                    };

                    let g_low_l = 1.0 - eff_amount * (1.0 - u0);
                    let g_high_l = 1.0 - eff_amount * u0;
                    let g_low_r = 1.0 - eff_amount * (1.0 - u1);
                    let g_high_r = 1.0 - eff_amount * u1;

                    let mut out_l = (f64::from(lp_l) * g_low_l + f64::from(hp_l) * g_high_l) as f32;
                    let mut out_r = (f64::from(lp_r) * g_low_r + f64::from(hp_r) * g_high_r) as f32;

                    if vintage {
                        out_l = (1.2 * f64::from(out_l)).tanh() as f32 / 1.2;
                        out_r = (1.2 * f64::from(out_r)).tanh() as f32 / 1.2;
                    }

                    audio.channel_mut(0)[i] = out_l;
                    if channels > 1 {
                        audio.channel_mut(1)[i] = out_r;
                    }
                } else {
                    let g_l = 1.0 - eff_amount * (1.0 - u0);
                    let g_r = 1.0 - eff_amount * (1.0 - u1);
                    let mut out_l = (f64::from(in_l) * g_l) as f32;
                    let mut out_r = (f64::from(in_r) * g_r) as f32;

                    if vintage {
                        out_l = (1.2 * f64::from(out_l)).tanh() as f32 / 1.2;
                        out_r = (1.2 * f64::from(out_r)).tanh() as f32 / 1.2;
                    }

                    audio.channel_mut(0)[i] = out_l;
                    if channels > 1 {
                        audio.channel_mut(1)[i] = out_r;
                    }
                }
            }
        }
    }

    fn display_len(&self) -> usize {
        2
    }

    fn display(&mut self, out: &mut [f32]) {
        out[0] = self.last_pan;
        out[1] = self.last_tremolo;
    }
}

/// The device description and type registration.
#[must_use]
pub fn auto_pan_tremolo_type() -> DeviceType {
    let synced_names: Vec<&str> = SYNCED_DIVISIONS.iter().map(|(n, _)| *n).collect();
    let dotted_names: Vec<&str> = DOTTED_DIVISIONS.iter().map(|(n, _)| *n).collect();
    let triplet_names: Vec<&str> = TRIPLET_DIVISIONS.iter().map(|(n, _)| *n).collect();

    let params = vec![
        ParamDescriptor::choice(MODE, "Mode", &MODES, 0)
            .describe("Selects Panning mode or Tremolo mode."),
        ParamDescriptor::choice(WAVE, "Waveform", &WAVES, 0).describe("Selects the LFO waveform."),
        ParamDescriptor::choice(TIME_MODE, "Time Mode", &TIME_MODES, 0)
            .describe("Selects Rate in Hz, Time, or tempo-synced divisions."),
        ParamDescriptor::continuous(RATE_HZ, "Rate", (0.01, 40.0), 1.0, ParamUnit::Hertz)
            .logarithmic()
            .describe("LFO frequency in Hertz."),
        ParamDescriptor::continuous(TIME_SECS, "Time", (0.1, 200.0), 1.0, ParamUnit::Seconds)
            .logarithmic()
            .describe("LFO period in seconds (100 ms to 200 s)."),
        ParamDescriptor::choice(SYNCED_DIVISION, "Synced", &synced_names, 3)
            .describe("Tempo-synced straight beat division."),
        ParamDescriptor::choice(DOTTED_DIVISION, "Dotted", &dotted_names, 2)
            .describe("Tempo-synced dotted beat division."),
        ParamDescriptor::choice(TRIPLET_DIVISION, "Triplet", &triplet_names, 2)
            .describe("Tempo-synced triplet beat division."),
        ParamDescriptor::integer(SIXTEENTHS, "16th", (1, 64), 4, ParamUnit::None)
            .describe("Tempo-synced sixteenth notes per cycle."),
        ParamDescriptor::choice(STEREO_MODE, "Stereo Offset", &["Phase", "Spin"], 0)
            .describe("Selects Phase or Spin stereo offset between channels."),
        ParamDescriptor::continuous(PHASE, "Phase", (0.0, 360.0), 180.0, ParamUnit::None)
            .decimals(0)
            .describe(
                "Phase offset in degrees between left and right channels (180° is antiphase).",
            ),
        ParamDescriptor::continuous(SPIN, "Spin", (-50.0, 50.0), 0.0, ParamUnit::Percent)
            .describe("Rate difference percentage between channels in Spin mode."),
        ParamDescriptor::continuous(
            PHASE_OFFSET,
            "Phase Offset",
            (-180.0, 180.0),
            0.0,
            ParamUnit::None,
        )
        .decimals(0)
        .describe("Grid shift in synced modes, or stereo width for random shapes."),
        ParamDescriptor::toggle(INVERT, "Invert", false)
            .describe("Inverts the polarity of the LFO waveform."),
        ParamDescriptor::toggle(HARMONIC, "Harmonic", false).describe(
            "Modulates high and low frequencies in antiphase about a 600 Hz crossover (Tremolo).",
        ),
        ParamDescriptor::toggle(VINTAGE, "Vintage", false)
            .describe("Applies warm non-linear shaping to tremolo modulation."),
        ParamDescriptor::continuous(AMOUNT, "Amount", (0.0, 100.0), 100.0, ParamUnit::Percent)
            .describe("Modulation depth applied to the signal."),
        ParamDescriptor::continuous(SHAPE, "Shape", (-100.0, 100.0), 0.0, ParamUnit::Percent)
            .describe("Reshapes the modulation waveform per mode."),
        ParamDescriptor::continuous(
            ATTACK_TIME,
            "Attack Time",
            (0.0, 1000.0),
            0.0,
            ParamUnit::Milliseconds,
        )
        .describe("Delays full modulation depth after an onset transient is detected."),
        ParamDescriptor::continuous(
            DYNAMIC_FM,
            "Dynamic FM",
            (-100.0, 100.0),
            0.0,
            ParamUnit::Percent,
        )
        .describe("Modulates LFO rate based on input signal level."),
    ];

    DeviceType::native(
        AUTO_PAN_TREMOLO,
        "Auto Pan-Tremolo",
        DeviceRole::AudioEffect,
    )
    .with_description(
        DeviceDescription::new(params)
            .with_layout(layout())
            .with_banks(&[
                (
                    "Main",
                    &[
                        "Mode",
                        "Waveform",
                        "Amount",
                        "Shape",
                        "Time Mode",
                        "Rate",
                        "Time",
                    ],
                ),
                ("Sync", &["Synced", "Dotted", "Triplet", "16th"]),
                (
                    "Stereo",
                    &["Stereo Offset", "Phase", "Spin", "Phase Offset", "Invert"],
                ),
                (
                    "Character",
                    &["Harmonic", "Vintage", "Attack Time", "Dynamic FM"],
                ),
            ]),
    )
}

fn layout() -> DeviceLayout {
    use Widget::{Chooser, Knob, Number, Toggle};
    let band = |controls: &[(ParamId, Widget)]| {
        LayoutNode::Band(ControlBand::stacked(BODY_HEIGHT, controls))
    };
    DeviceLayout::new(LayoutNode::Row(vec![
        band(&[(MODE, Chooser), (WAVE, Chooser), (TIME_MODE, Chooser)]),
        LayoutNode::Cell(DisplayCell::new("Pan", 150.0, BODY_HEIGHT, 1.0)),
        band(&[
            (RATE_HZ, Knob),
            (TIME_SECS, Knob),
            (SYNCED_DIVISION, Chooser),
            (DOTTED_DIVISION, Chooser),
            (TRIPLET_DIVISION, Chooser),
            (SIXTEENTHS, Number),
        ]),
        band(&[
            (STEREO_MODE, Chooser),
            (PHASE, Knob),
            (SPIN, Knob),
            (PHASE_OFFSET, Knob),
            (INVERT, Toggle),
            (HARMONIC, Toggle),
            (VINTAGE, Toggle),
        ]),
        band(&[
            (AMOUNT, Knob),
            (SHAPE, Knob),
            (ATTACK_TIME, Knob),
            (DYNAMIC_FM, Knob),
        ]),
    ]))
}

/// Registers the Auto Pan-Tremolo device in `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, auto_pan_tremolo_type(), AutoPanTremolo::new);
}

/// Returns device display information for headless inspection.
#[must_use]
pub fn display(
    device: DeviceId,
    _description: &DeviceDescription,
    _values: &OrdMap<ParamId, f64>,
    data: Option<&[f32]>,
) -> DeviceDisplay {
    let (pan, tremolo) = data.map_or((0.0f32, 1.0f32), |d| {
        (
            d.first().copied().unwrap_or(0.0),
            d.get(1).copied().unwrap_or(1.0),
        )
    });
    DeviceDisplay {
        device,
        live: data.is_some(),
        view: None,
        series: Vec::new(),
        markers: Vec::new(),
        readings: vec![
            DisplayReading {
                name: "Pan Position".to_owned(),
                value: pan,
                unit: String::new(),
                text: format!("{pan:.2}"),
            },
            DisplayReading {
                name: "Tremolo Level".to_owned(),
                value: tremolo,
                unit: String::new(),
                text: format!("{tremolo:.2}"),
            },
        ],
    }
}

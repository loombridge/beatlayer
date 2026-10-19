//! Chorus-Ensemble, the native chorus, ensemble and vibrato effect (WP-5.20b;
//! the chorus-ensemble spec): three modes with separate controls, modulated
//! delay lines, width, warmth, high-pass filter, and 12.4's Time and Taps.

use core::f64::consts::{PI, TAU};

use bl_core::time::SampleRate;
use bl_engine::{DeviceType, NodeConfig, Registry};
use bl_model::device_layout::{BODY_HEIGHT, ControlBand, DisplayCell, LayoutNode, Widget};
use bl_model::{
    Device, DeviceDescription, DeviceDisplay, DeviceId, DeviceLayout, DeviceRole, DisplaySeries,
    OrdMap, ParamDescriptor, ParamId, ParamUnit,
};

use crate::framework::{Audio, NativeDevice, ParamBank, add};
use crate::sections::{Biquad, Coeffs};

/// Chorus-Ensemble's device type.
pub const CHORUS_ENSEMBLE: &str = "beatlayer.chorus_ensemble";

/// Mode: Chorus, Ensemble, or Vibrato.
pub const MODE: ParamId = ParamId(0);
/// Rate: modulation rate in Hertz.
pub const RATE: ParamId = ParamId(1);
/// Amount: depth of modulation in per cent.
pub const AMOUNT: ParamId = ParamId(2);
/// Feedback: amount of output fed back to input in per cent.
pub const FEEDBACK: ParamId = ParamId(3);
/// Feedback Invert: inverts the feedback signal.
pub const FEEDBACK_INVERT: ParamId = ParamId(4);
/// High-Pass: cutoff frequency of high-pass filter on wet path in Hz.
pub const HP_FREQ: ParamId = ParamId(5);
/// Width: stereo width from mono (0%) to sides-boosted (200%).
pub const WIDTH: ParamId = ParamId(6);
/// Warmth: saturation and tone warming in per cent.
pub const WARMTH: ParamId = ParamId(7);
/// Output: output level, a gain reaching silence.
pub const OUTPUT: ParamId = ParamId(8);
/// Dry/Wet: balance between dry and wet signals in per cent.
pub const DRY_WET: ParamId = ParamId(9);

/// Chorus Time: Auto or fixed delay time.
pub const CHORUS_TIME: ParamId = ParamId(10);
/// Chorus Taps: 1 Tap or 2 Taps.
pub const CHORUS_TAPS: ParamId = ParamId(11);

/// Vibrato Offset: channel phase offset in degrees.
pub const VIBRATO_OFFSET: ParamId = ParamId(12);
/// Vibrato Shape: morphs modulation waveform from Sine (0%) to Triangle (100%).
pub const VIBRATO_SHAPE: ParamId = ParamId(13);

pub const MODES: [&str; 3] = ["Chorus", "Ensemble", "Vibrato"];
pub const CHORUS_TIMES: [&str; 6] = ["Auto", "7 ms", "10 ms", "20 ms", "35 ms", "50 ms"];
pub const TAPS: [&str; 2] = ["1 Tap", "2 Taps"];

const MAX_DELAY_SECS: f64 = 0.5;

/// Chorus-Ensemble's device type.
#[must_use]
pub fn chorus_ensemble_type() -> DeviceType {
    let params = vec![
        ParamDescriptor::choice(MODE, "Mode", &MODES, 0)
            .describe("The effect algorithm: Chorus, Ensemble, or Vibrato."),
        ParamDescriptor::continuous(RATE, "Rate", (0.01, 20.0), 1.0, ParamUnit::Hertz)
            .logarithmic()
            .describe("The LFO modulation rate in Hertz."),
        ParamDescriptor::continuous(AMOUNT, "Amount", (0.0, 100.0), 50.0, ParamUnit::Percent)
            .describe("The amplitude of the delay modulation."),
        ParamDescriptor::continuous(FEEDBACK, "Feedback", (0.0, 100.0), 0.0, ParamUnit::Percent)
            .describe("How much of the wet signal feeds back into the input."),
        ParamDescriptor::toggle(FEEDBACK_INVERT, "Feedback Invert", false)
            .describe("Inverts the polarity of the feedback signal."),
        ParamDescriptor::continuous(HP_FREQ, "High-Pass", (20.0, 2000.0), 20.0, ParamUnit::Hertz)
            .logarithmic()
            .describe("High-pass filter cutoff on the wet effect signal."),
        ParamDescriptor::continuous(WIDTH, "Width", (0.0, 200.0), 100.0, ParamUnit::Percent)
            .describe("Stereo width of the chorus signal, from mono (0%) to sides-boosted (200%)."),
        ParamDescriptor::continuous(WARMTH, "Warmth", (0.0, 100.0), 0.0, ParamUnit::Percent)
            .describe("Adds subtle saturation and gentle high-frequency rolloff."),
        ParamDescriptor::gain(OUTPUT, "Output", 12.0, 0.0)
            .describe("The output level, reaching silence."),
        ParamDescriptor::continuous(DRY_WET, "Dry/Wet", (0.0, 100.0), 50.0, ParamUnit::Percent)
            .describe("The balance between unprocessed and processed signals."),
        ParamDescriptor::choice(CHORUS_TIME, "Time", &CHORUS_TIMES, 0)
            .describe("Auto scaling or a fixed delay time in Chorus mode."),
        ParamDescriptor::choice(CHORUS_TAPS, "Taps", &TAPS, 1)
            .describe("Single or dual tap delay in Chorus mode."),
        ParamDescriptor::continuous(
            VIBRATO_OFFSET,
            "Offset",
            (0.0, 180.0),
            180.0,
            ParamUnit::None,
        )
        .decimals(0)
        .describe("Phase offset between left and right channels in Vibrato mode."),
        ParamDescriptor::continuous(
            VIBRATO_SHAPE,
            "Shape",
            (0.0, 100.0),
            0.0,
            ParamUnit::Percent,
        )
        .describe("Waveform shape in Vibrato mode, from Sine (0%) to Triangle (100%)."),
    ];

    DeviceType::native(CHORUS_ENSEMBLE, "Chorus-Ensemble", DeviceRole::AudioEffect)
        .with_description(DeviceDescription::new(params).with_layout(layout()))
}

fn layout() -> DeviceLayout {
    use Widget::{Chooser, Knob, Toggle};
    let band = |controls: &[(ParamId, Widget)]| {
        LayoutNode::Band(ControlBand::stacked(BODY_HEIGHT, controls))
    };

    let collapsed = LayoutNode::Row(vec![
        band(&[(MODE, Chooser), (RATE, Knob), (AMOUNT, Knob)]),
        LayoutNode::Cell(DisplayCell::new("Chorus-Ensemble", 160.0, BODY_HEIGHT, 1.0)),
        band(&[(CHORUS_TIME, Chooser), (CHORUS_TAPS, Chooser)]),
        band(&[(VIBRATO_OFFSET, Knob), (VIBRATO_SHAPE, Knob)]),
        band(&[
            (HP_FREQ, Knob),
            (WIDTH, Knob),
            (FEEDBACK, Knob),
            (FEEDBACK_INVERT, Toggle),
            (WARMTH, Knob),
            (OUTPUT, Knob),
            (DRY_WET, Knob),
        ]),
    ]);

    DeviceLayout::new(collapsed)
}

/// Adds Chorus-Ensemble to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, chorus_ensemble_type(), make);
}

#[must_use]
pub fn make(device: &Device, config: &NodeConfig) -> ChorusEnsemble {
    ChorusEnsemble::new(device, config)
}

struct ModDelayLine {
    buffer: Vec<f32>,
    write: usize,
}

impl ModDelayLine {
    fn new(len: usize) -> Self {
        Self {
            buffer: vec![0.0; len.max(4)],
            write: 0,
        }
    }

    fn write(&mut self, sample: f32) {
        let len = self.buffer.len();
        self.buffer[self.write] = sample;
        self.write = (self.write + 1) % len;
    }

    fn read(&self, delay_frames: f64) -> f32 {
        let len = self.buffer.len();
        let delay = delay_frames.clamp(0.0, (len - 2) as f64);
        let pos = (self.write as f64 - delay).rem_euclid(len as f64);
        let idx = pos.floor() as usize;
        let frac = (pos - pos.floor()) as f32;
        let s0 = self.buffer[idx % len];
        let s1 = self.buffer[(idx + 1) % len];
        s0 + (s1 - s0) * frac
    }
}

pub struct ChorusEnsemble {
    sample_rate: SampleRate,
    phase: f64,
    delays: [[ModDelayLine; 3]; 2],
    hp_filter: [Biquad; 2],
    warmth_filter: [Biquad; 2],
    feedback_state: [f32; 2],
    input: [Vec<f32>; 2],
    output: [Vec<f32>; 2],
}

impl ChorusEnsemble {
    #[must_use]
    pub fn new(_device: &Device, config: &NodeConfig) -> Self {
        let rate = config.sample_rate.as_f64();
        let max_len = (MAX_DELAY_SECS * rate) as usize + 32;
        let frames = config.max_frames.max(1);

        Self {
            sample_rate: config.sample_rate,
            phase: 0.0,
            delays: [
                [
                    ModDelayLine::new(max_len),
                    ModDelayLine::new(max_len),
                    ModDelayLine::new(max_len),
                ],
                [
                    ModDelayLine::new(max_len),
                    ModDelayLine::new(max_len),
                    ModDelayLine::new(max_len),
                ],
            ],
            hp_filter: [Biquad::default(); 2],
            warmth_filter: [Biquad::default(); 2],
            feedback_state: [0.0; 2],
            input: [vec![0.0; frames], vec![0.0; frames]],
            output: [vec![0.0; frames], vec![0.0; frames]],
        }
    }
}

impl NativeDevice for ChorusEnsemble {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        let frames = audio.frames();
        let channels = audio.channels().min(2);
        if frames == 0 || channels == 0 {
            return;
        }

        let at = |id: ParamId| params.index(id).expect("the description holds it");
        let val = |id: ParamId| params.value(at(id));
        let ramp = |id: ParamId| params.ramp(at(id));

        let rate_f64 = self.sample_rate.as_f64();

        let mode_idx = val(MODE).round() as usize;
        let lfo_rate = val(RATE).clamp(0.01, 20.0);
        let amount = (val(AMOUNT) / 100.0).clamp(0.0, 1.0);
        let feedback = (val(FEEDBACK) / 100.0).clamp(0.0, 0.99) as f32;
        let fb_invert = val(FEEDBACK_INVERT) >= 0.5;
        let fb_coeff = if fb_invert { -feedback } else { feedback };

        let hp_freq_val = val(HP_FREQ).clamp(20.0, 2000.0);
        let width_val = (val(WIDTH) / 100.0).clamp(0.0, 2.0) as f32;
        let warmth_val = (val(WARMTH) / 100.0).clamp(0.0, 1.0) as f32;
        let dry_wet_val = (val(DRY_WET) / 100.0).clamp(0.0, 1.0) as f32;

        let chorus_time_idx = val(CHORUS_TIME).round() as usize;
        let chorus_taps_idx = val(CHORUS_TAPS).round() as usize;

        let vibrato_offset_deg = val(VIBRATO_OFFSET).clamp(0.0, 180.0);
        let vibrato_shape_val = (val(VIBRATO_SHAPE) / 100.0).clamp(0.0, 1.0);

        let output_ramp = ramp(OUTPUT);

        // High-pass filter coefficients
        let hp_coeffs = highpass_coeffs(hp_freq_val, core::f64::consts::FRAC_1_SQRT_2, rate_f64);
        for hp in &mut self.hp_filter {
            hp.set(hp_coeffs);
        }

        // Warmth filter coefficients
        let warmth_cutoff = (20_000.0 - warmth_val as f64 * 12_000.0).clamp(2000.0, 20_000.0);
        let warmth_coeffs = lowpass_coeffs(warmth_cutoff, 0.707, rate_f64);
        for wf in &mut self.warmth_filter {
            wf.set(warmth_coeffs);
        }

        let lfo_step = lfo_rate / rate_f64;

        // Base delay time for Chorus mode in ms
        let chorus_base_ms = match chorus_time_idx {
            1 => 7.0,
            2 => 10.0,
            3 => 20.0,
            4 => 35.0,
            5 => 50.0,
            _ => 5.0 + 15.0 * amount, // Auto: scales dynamically with amount
        };

        for ch in 0..channels {
            self.input[ch][..frames].copy_from_slice(audio.channel_mut(ch));
        }

        for frame in 0..frames {
            self.phase = (self.phase + lfo_step).rem_euclid(1.0);
            let current_phase = self.phase;

            let in_l = self.input[0][frame];
            let in_r = if channels > 1 {
                self.input[1][frame]
            } else {
                in_l
            };

            let in_samples = [in_l, in_r];
            let mut wet_channels = [0.0f32; 2];

            match mode_idx {
                0 => {
                    // Chorus mode: 1 or 2 taps
                    let tap_count = if chorus_taps_idx == 0 { 1 } else { 2 };
                    let mod_depth_ms = amount * 3.0;

                    for ch in 0..2 {
                        let ch_phase = if ch == 0 {
                            current_phase
                        } else {
                            (current_phase + 0.25).rem_euclid(1.0)
                        };

                        let fb_in = in_samples[ch] + fb_coeff * self.feedback_state[ch];
                        self.delays[ch][0].write(fb_in);
                        if tap_count == 2 {
                            self.delays[ch][1].write(fb_in);
                        }

                        let lfo1 = (TAU * ch_phase).sin();
                        let delay1_ms = (chorus_base_ms + lfo1 * mod_depth_ms).max(0.5);
                        let s1 = self.delays[ch][0].read(delay1_ms * rate_f64 / 1000.0);

                        let wet_sample = if tap_count == 2 {
                            let lfo2 = (TAU * (ch_phase + 0.5)).sin();
                            let delay2_ms = (chorus_base_ms + lfo2 * mod_depth_ms).max(0.5);
                            let s2 = self.delays[ch][1].read(delay2_ms * rate_f64 / 1000.0);
                            0.5 * (s1 + s2)
                        } else {
                            s1
                        };

                        wet_channels[ch] = wet_sample;
                    }
                }
                1 => {
                    // Ensemble mode: 3 delay lines with phases 120 deg apart
                    let base_ms = 10.0;
                    let mod_depth_ms = amount * 3.5;

                    for ch in 0..2 {
                        let ch_offset = if ch == 0 { 0.0 } else { 0.1 };
                        let fb_in = in_samples[ch] + fb_coeff * self.feedback_state[ch];

                        for tap in 0..3 {
                            self.delays[ch][tap].write(fb_in);
                        }

                        let mut sum = 0.0f32;
                        for tap in 0..3 {
                            let tap_phase =
                                (current_phase + ch_offset + (tap as f64) / 3.0).rem_euclid(1.0);
                            let lfo_val = (TAU * tap_phase).sin();
                            let delay_ms = (base_ms + lfo_val * mod_depth_ms).max(0.5);
                            sum += self.delays[ch][tap].read(delay_ms * rate_f64 / 1000.0);
                        }

                        wet_channels[ch] = sum / 3.0;
                    }
                }
                _ => {
                    // Vibrato mode: pitch modulation, no dry layer, ignores feedback & dry_wet
                    let base_ms = 5.0;
                    let mod_depth_ms = amount * 4.0;
                    let offset_frac = vibrato_offset_deg / 360.0;

                    for ch in 0..2 {
                        let ch_phase = if ch == 0 {
                            current_phase
                        } else {
                            (current_phase + offset_frac).rem_euclid(1.0)
                        };

                        // Shape: sine (0%) to triangle (100%)
                        let sine_val = (TAU * ch_phase).sin();
                        let tri_val = 1.0 - 4.0 * (ch_phase - 0.5).abs();
                        let mod_val =
                            (1.0 - vibrato_shape_val) * sine_val + vibrato_shape_val * tri_val;

                        self.delays[ch][0].write(in_samples[ch]);
                        let delay_ms = (base_ms + mod_val * mod_depth_ms).max(0.5);
                        wet_channels[ch] = self.delays[ch][0].read(delay_ms * rate_f64 / 1000.0);
                    }
                }
            }

            // Warmth saturation
            for ch in 0..2 {
                let w = wet_channels[ch];
                let warmed = if warmth_val > 0.0 {
                    let driven = w * (1.0 + warmth_val * 1.5);
                    let sat = driven - (warmth_val * 0.3) * (driven * driven * driven) / 3.0;
                    self.warmth_filter[ch].process(sat)
                } else {
                    w
                };
                wet_channels[ch] = warmed;
                self.feedback_state[ch] = warmed;
            }

            // High-pass filter on wet channels
            for ch in 0..2 {
                wet_channels[ch] = self.hp_filter[ch].process(wet_channels[ch]);
            }

            // Stereo Width control in Chorus and Ensemble modes
            if mode_idx < 2 {
                let mid = 0.5 * (wet_channels[0] + wet_channels[1]);
                let side = 0.5 * (wet_channels[0] - wet_channels[1]);
                wet_channels[0] = mid + width_val * side;
                wet_channels[1] = mid - width_val * side;
            }

            let out_gain = output_ramp[frame];

            if mode_idx == 2 {
                // Vibrato mode: wet only (passes no dry layer and ignores dry/wet and feedback)
                self.output[0][frame] = wet_channels[0] * out_gain;
                if channels > 1 {
                    self.output[1][frame] = wet_channels[1] * out_gain;
                }
            } else {
                for ch in 0..channels {
                    let mixed =
                        (1.0 - dry_wet_val) * in_samples[ch] + dry_wet_val * wet_channels[ch];
                    self.output[ch][frame] = mixed * out_gain;
                }
            }
        }

        for ch in 0..channels {
            audio
                .channel_mut(ch)
                .copy_from_slice(&self.output[ch][..frames]);
        }
    }
}

fn highpass_coeffs(freq: f64, q: f64, sample_rate: f64) -> Coeffs {
    let w0 = (TAU * freq / sample_rate).clamp(1e-5, PI * 0.98);
    let alpha = (w0.sin() / (2.0 * q)).max(1e-6);
    let a0 = 1.0 + alpha;
    Coeffs {
        b0: (((1.0 + w0.cos()) / 2.0) / a0) as f32,
        b1: (-(1.0 + w0.cos()) / a0) as f32,
        b2: (((1.0 + w0.cos()) / 2.0) / a0) as f32,
        a1: ((-2.0 * w0.cos()) / a0) as f32,
        a2: ((1.0 - alpha) / a0) as f32,
    }
}

fn lowpass_coeffs(freq: f64, q: f64, sample_rate: f64) -> Coeffs {
    let w0 = (TAU * freq / sample_rate).clamp(1e-5, PI * 0.98);
    let alpha = (w0.sin() / (2.0 * q)).max(1e-6);
    let a0 = 1.0 + alpha;
    Coeffs {
        b0: (((1.0 - w0.cos()) / 2.0) / a0) as f32,
        b1: ((1.0 - w0.cos()) / a0) as f32,
        b2: (((1.0 - w0.cos()) / 2.0) / a0) as f32,
        a1: ((-2.0 * w0.cos()) / a0) as f32,
        a2: ((1.0 - alpha) / a0) as f32,
    }
}

/// Headless display data for Chorus-Ensemble.
#[must_use]
pub fn display(
    device: DeviceId,
    description: &DeviceDescription,
    values: &OrdMap<ParamId, f64>,
    data: Option<&[f32]>,
) -> DeviceDisplay {
    let value = |id: ParamId| description.value(values, id).unwrap_or(0.0) as f32;
    let mode = value(MODE).round() as usize;
    let name = match mode {
        0 => "Chorus LFO",
        1 => "Ensemble LFOs",
        _ => "Vibrato LFO",
    };

    DeviceDisplay {
        device,
        live: data.is_some(),
        view: None,
        series: vec![DisplaySeries {
            name: name.to_owned(),
            unit: String::new(),
            x_unit: "Hz".to_owned(),
            x_range: [20.0, 20_000.0],
            log_x: true,
            y_range: [-30.0, 6.0],
            detail: false,
            values: Vec::new(),
        }],
        readings: Vec::new(),
        markers: Vec::new(),
    }
}

//! Phaser-Flanger, the native phaser, flanger and doubler effect (WP-5.20b;
//! the phaser-flanger spec): three modes with separate controls, two LFOs
//! and an envelope follower, Safe Bass, and an expanded view.

use core::f64::consts::{PI, TAU};

use bl_core::time::SampleRate;
use bl_dsp::EnvelopeFollower;
use bl_engine::{DeviceType, NodeConfig, Registry};
use bl_model::device_layout::{BODY_HEIGHT, ControlBand, DisplayCell, LayoutNode, Widget};
use bl_model::{
    Device, DeviceDescription, DeviceDisplay, DeviceId, DeviceLayout, DeviceRole, DisplaySeries,
    OrdMap, ParamDescriptor, ParamId, ParamUnit,
};

use crate::framework::{Audio, NativeDevice, ParamBank, add};
use crate::lfo::{Lfo, Rate, Shape, Stereo, Wave};
use crate::sections::{Biquad, Coeffs};

/// Phaser-Flanger's device type.
pub const PHASER_FLANGER: &str = "beatlayer.phaser_flanger";

/// Mode: Phaser, Flanger, or Doubler.
pub const MODE: ParamId = ParamId(0);
/// Amount: depth of modulation in per cent.
pub const AMOUNT: ParamId = ParamId(1);
/// Feedback: amount of output fed back to input in per cent.
pub const FEEDBACK: ParamId = ParamId(2);
/// Feedback Invert: inverts the feedback signal.
pub const FEEDBACK_INVERT: ParamId = ParamId(3);
/// Output: output level, a gain reaching silence.
pub const OUTPUT: ParamId = ParamId(4);
/// Warmth: saturation and tone warming in per cent.
pub const WARMTH: ParamId = ParamId(5);
/// Dry/Wet: balance between dry and wet signals in per cent.
pub const DRY_WET: ParamId = ParamId(6);

/// Phaser Notches: count of notch filters.
pub const NOTCHES: ParamId = ParamId(7);
/// Phaser Center: center frequency of the notches in Hz.
pub const CENTER: ParamId = ParamId(8);
/// Phaser Spread: distance between notches in per cent.
pub const SPREAD: ParamId = ParamId(9);
/// Phaser Blend: modulation routing between Center (0.0) and Spread (1.0).
pub const BLEND: ParamId = ParamId(10);

/// Flanger Time: base delay time in milliseconds.
pub const FLANGER_TIME: ParamId = ParamId(11);
/// Doubler Time: base delay time in milliseconds.
pub const DOUBLER_TIME: ParamId = ParamId(12);

/// LFO Wave: one of [`WAVES`].
pub const LFO_WAVE: ParamId = ParamId(13);
/// LFO Mode: Hz or Sync.
pub const LFO_MODE: ParamId = ParamId(14);
/// LFO Rate in Hz.
pub const LFO_HZ: ParamId = ParamId(15);
/// LFO Sync division.
pub const LFO_DIVISION: ParamId = ParamId(16);
/// LFO Stereo: Phase or Spin.
pub const LFO_STEREO: ParamId = ParamId(17);
/// LFO Phase: degrees right channel leads left.
pub const LFO_PHASE: ParamId = ParamId(18);
/// LFO Spin: per cent right rate exceeds left.
pub const LFO_SPIN: ParamId = ParamId(19);
/// LFO Duty Cycle: time compression of waveform in per cent.
pub const LFO_DUTY: ParamId = ParamId(20);

/// LFO2 Mix: balance between main LFO (0%) and LFO2 (100%).
pub const LFO2_MIX: ParamId = ParamId(21);
/// LFO2 Mode: Hz or Sync.
pub const LFO2_MODE: ParamId = ParamId(22);
/// LFO2 Rate in Hz.
pub const LFO2_HZ: ParamId = ParamId(23);
/// LFO2 Sync division.
pub const LFO2_DIVISION: ParamId = ParamId(24);

/// Env Fol: enables the envelope follower.
pub const ENV_ON: ParamId = ParamId(25);
/// Env Amount: intensity and direction of envelope modulation in per cent.
pub const ENV_AMOUNT: ParamId = ParamId(26);
/// Env Attack: attack time in milliseconds.
pub const ENV_ATTACK: ParamId = ParamId(27);
/// Env Release: release time in milliseconds.
pub const ENV_RELEASE: ParamId = ParamId(28);

/// Safe Bass: cutoff frequency of high-pass filter on wet path in Hz.
pub const SAFE_BASS: ParamId = ParamId(29);

/// The main LFO's waves, in the chooser's order, under the manual's names.
pub const WAVES: [&str; 10] = [
    "Sine",
    "Triangle",
    "Saw Up",
    "Saw Down",
    "Rectangle",
    "Random",
    "Random S&H",
    "Triangle Analog",
    "Triangle 8",
    "Triangle 16",
];

/// The effects LFO's waves the chooser's [`WAVES`] play, in order.
const WAVE_OF: [Wave; 10] = [
    Wave::Sine,
    Wave::Triangle,
    Wave::SawUp,
    Wave::SawDown,
    Wave::Rectangle,
    Wave::Random,
    Wave::SampleHold,
    Wave::TriangleAnalog,
    Wave::Triangle8,
    Wave::Triangle16,
];

pub const MODES: [&str; 3] = ["Phaser", "Flanger", "Doubler"];
pub const RATE_MODES: [&str; 2] = ["Hz", "Sync"];
pub const STEREO_MODES: [&str; 2] = ["Phase", "Spin"];

pub const DIVISIONS: [(&str, f64); 17] = [
    ("1/64", 1.0 / 16.0),
    ("1/32", 1.0 / 8.0),
    ("1/16", 0.25),
    ("1/8", 0.5),
    ("1/4", 1.0),
    ("1/2", 2.0),
    ("1 bar", 4.0),
    ("2 bars", 8.0),
    ("4 bars", 16.0),
    ("8 bars", 32.0),
    ("1/16T", 1.0 / 6.0),
    ("1/8T", 1.0 / 3.0),
    ("1/4T", 2.0 / 3.0),
    ("1/2T", 4.0 / 3.0),
    ("1/16D", 0.375),
    ("1/8D", 0.75),
    ("1/4D", 1.5),
];

const MAX_NOTCHES: usize = 16;
const MAX_DELAY_SECS: f64 = 0.5;

/// Phaser-Flanger's device type.
#[must_use]
pub fn phaser_flanger_type() -> DeviceType {
    let divisions: Vec<&str> = DIVISIONS.iter().map(|d| d.0).collect();

    let params = vec![
        ParamDescriptor::choice(MODE, "Mode", &MODES, 0)
            .describe("The effect algorithm: Phaser, Flanger, or Doubler."),
        ParamDescriptor::continuous(AMOUNT, "Amount", (0.0, 100.0), 50.0, ParamUnit::Percent)
            .describe("How much modulation moves the notches or delay time."),
        ParamDescriptor::continuous(FEEDBACK, "Feedback", (0.0, 100.0), 0.0, ParamUnit::Percent)
            .describe("How much of the wet signal feeds back into the input."),
        ParamDescriptor::toggle(FEEDBACK_INVERT, "Feedback Invert", false)
            .describe("Inverts the polarity of the feedback signal."),
        ParamDescriptor::gain(OUTPUT, "Output", 12.0, 0.0)
            .describe("The output level, reaching silence."),
        ParamDescriptor::continuous(WARMTH, "Warmth", (0.0, 100.0), 0.0, ParamUnit::Percent)
            .describe("Adds subtle saturation and gentle high-frequency rolloff."),
        ParamDescriptor::continuous(DRY_WET, "Dry/Wet", (0.0, 100.0), 50.0, ParamUnit::Percent)
            .describe("The balance between unprocessed and processed signals."),
        ParamDescriptor::integer(NOTCHES, "Notches", (1, 16), 4, ParamUnit::None)
            .describe("The number of notch filter stages in Phaser mode."),
        ParamDescriptor::continuous(
            CENTER,
            "Center",
            (20.0, 20_000.0),
            1_000.0,
            ParamUnit::Hertz,
        )
        .logarithmic()
        .describe("Center frequency of the notch filters."),
        ParamDescriptor::continuous(SPREAD, "Spread", (0.0, 100.0), 50.0, ParamUnit::Percent)
            .describe("Distance between notches in Phaser mode."),
        ParamDescriptor::continuous(BLEND, "Blend", (0.0, 1.0), 0.0, ParamUnit::None)
            .describe("Modulation balance between Center (0.0) and Spread (1.0)."),
        ParamDescriptor::continuous(
            FLANGER_TIME,
            "Flanger Time",
            (0.05, 20.0),
            1.0,
            ParamUnit::Milliseconds,
        )
        .describe("Delay time in Flanger mode."),
        ParamDescriptor::continuous(
            DOUBLER_TIME,
            "Doubler Time",
            (5.0, 100.0),
            20.0,
            ParamUnit::Milliseconds,
        )
        .describe("Base delay time in Doubler mode."),
        ParamDescriptor::choice(LFO_WAVE, "LFO Wave", &WAVES, 1)
            .describe("The waveform of the main LFO."),
        ParamDescriptor::choice(LFO_MODE, "LFO Mode", &RATE_MODES, 0)
            .describe("Whether the main LFO rate is in Hertz or tempo-synced."),
        ParamDescriptor::continuous(LFO_HZ, "LFO Freq", (0.01, 40.0), 0.5, ParamUnit::Hertz)
            .logarithmic()
            .describe("The main LFO rate in Hertz."),
        ParamDescriptor::choice(LFO_DIVISION, "LFO Rate", &divisions, 4)
            .describe("The tempo-synced beat division for the main LFO."),
        ParamDescriptor::choice(LFO_STEREO, "LFO Stereo", &STEREO_MODES, 0)
            .describe("Stereo modulation relationship: Phase or Spin."),
        ParamDescriptor::continuous(LFO_PHASE, "LFO Phase", (0.0, 360.0), 180.0, ParamUnit::None)
            .decimals(0)
            .describe("Phase difference between left and right channels."),
        ParamDescriptor::continuous(
            LFO_SPIN,
            "LFO Spin",
            (-100.0, 100.0),
            0.0,
            ParamUnit::Percent,
        )
        .describe("Detunes right channel LFO rate relative to left."),
        ParamDescriptor::continuous(
            LFO_DUTY,
            "Duty Cycle",
            (-100.0, 100.0),
            0.0,
            ParamUnit::Percent,
        )
        .describe("Waveform time compression towards front or back."),
        ParamDescriptor::continuous(LFO2_MIX, "LFO2 Mix", (0.0, 100.0), 0.0, ParamUnit::Percent)
            .describe("Mix between main LFO (0%) and LFO2 (100%)."),
        ParamDescriptor::choice(LFO2_MODE, "LFO2 Mode", &RATE_MODES, 0)
            .describe("Whether LFO2 rate is in Hertz or tempo-synced."),
        ParamDescriptor::continuous(LFO2_HZ, "LFO2 Freq", (0.01, 40.0), 1.0, ParamUnit::Hertz)
            .logarithmic()
            .describe("LFO2 rate in Hertz."),
        ParamDescriptor::choice(LFO2_DIVISION, "LFO2 Rate", &divisions, 3)
            .describe("The tempo-synced beat division for LFO2."),
        ParamDescriptor::toggle(ENV_ON, "Env Fol", false)
            .describe("Enables envelope follower modulation."),
        ParamDescriptor::continuous(
            ENV_AMOUNT,
            "Env Amount",
            (-100.0, 100.0),
            0.0,
            ParamUnit::Percent,
        )
        .describe("Intensity and direction of envelope modulation."),
        ParamDescriptor::continuous(
            ENV_ATTACK,
            "Attack",
            (0.1, 500.0),
            10.0,
            ParamUnit::Milliseconds,
        )
        .describe("Envelope follower attack time."),
        ParamDescriptor::continuous(
            ENV_RELEASE,
            "Release",
            (1.0, 1000.0),
            100.0,
            ParamUnit::Milliseconds,
        )
        .describe("Envelope follower release time."),
        ParamDescriptor::continuous(
            SAFE_BASS,
            "Safe Bass",
            (5.0, 3000.0),
            100.0,
            ParamUnit::Hertz,
        )
        .logarithmic()
        .describe("High-pass filter cutoff on the wet effect signal."),
    ];

    DeviceType::native(PHASER_FLANGER, "Phaser-Flanger", DeviceRole::AudioEffect).with_description(
        DeviceDescription::new(params)
            .with_layout(layout())
            .with_banks(&[
                (
                    "Main",
                    &[
                        "Mode",
                        "Amount",
                        "Feedback",
                        "Feedback Invert",
                        "Output",
                        "Warmth",
                        "Dry/Wet",
                        "Safe Bass",
                    ],
                ),
                (
                    "Character",
                    &[
                        "Notches",
                        "Center",
                        "Spread",
                        "Blend",
                        "Flanger Time",
                        "Doubler Time",
                    ],
                ),
                (
                    "LFO",
                    &[
                        "LFO Wave",
                        "LFO Mode",
                        "LFO Freq",
                        "LFO Rate",
                        "LFO Stereo",
                        "LFO Phase",
                        "LFO Spin",
                        "Duty Cycle",
                    ],
                ),
                (
                    "LFO 2",
                    &["LFO2 Mix", "LFO2 Mode", "LFO2 Freq", "LFO2 Rate"],
                ),
                ("Envelope", &["Env Fol", "Env Amount", "Attack", "Release"]),
            ]),
    )
}

fn layout() -> DeviceLayout {
    use Widget::{Chooser, Knob, Toggle};
    let band = |controls: &[(ParamId, Widget)]| {
        LayoutNode::Band(ControlBand::stacked(BODY_HEIGHT, controls))
    };

    let collapsed = LayoutNode::Row(vec![
        band(&[(MODE, Chooser), (AMOUNT, Knob)]),
        LayoutNode::Cell(DisplayCell::new("Phaser-Flanger", 160.0, BODY_HEIGHT, 1.0)),
        band(&[
            (NOTCHES, Knob),
            (CENTER, Knob),
            (SPREAD, Knob),
            (BLEND, Knob),
        ]),
        band(&[(FLANGER_TIME, Knob), (DOUBLER_TIME, Knob)]),
        band(&[
            (FEEDBACK, Knob),
            (FEEDBACK_INVERT, Toggle),
            (WARMTH, Knob),
            (OUTPUT, Knob),
            (DRY_WET, Knob),
        ]),
    ]);

    let expanded = LayoutNode::Row(vec![
        band(&[
            (LFO_WAVE, Chooser),
            (LFO_MODE, Chooser),
            (LFO_HZ, Knob),
            (LFO_DIVISION, Chooser),
            (LFO_STEREO, Chooser),
            (LFO_PHASE, Knob),
            (LFO_SPIN, Knob),
            (LFO_DUTY, Knob),
        ]),
        band(&[
            (LFO2_MIX, Knob),
            (LFO2_MODE, Chooser),
            (LFO2_HZ, Knob),
            (LFO2_DIVISION, Chooser),
        ]),
        band(&[
            (ENV_ON, Toggle),
            (ENV_AMOUNT, Knob),
            (ENV_ATTACK, Knob),
            (ENV_RELEASE, Knob),
        ]),
        band(&[(SAFE_BASS, Knob)]),
    ]);

    DeviceLayout::new(collapsed).with_expanded(expanded)
}

/// Adds Phaser-Flanger to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, phaser_flanger_type(), make);
}

#[must_use]
pub fn make(device: &Device, config: &NodeConfig) -> PhaserFlanger {
    PhaserFlanger::new(device, config)
}

/// Computes the mapped notch frequency for index `k` of `notches` stages.
#[must_use]
pub fn phaser_notch_freq(center: f64, notches: usize, spread: f64, k: usize) -> f64 {
    let n = notches.max(1);
    let spacing = 0.5 * (0.2 + 1.8 * spread / 100.0);
    center * 2.0_f64.powf((k as f64 - (n - 1) as f64 / 2.0) * spacing)
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

pub struct PhaserFlanger {
    sample_rate: SampleRate,
    lfo: Lfo,
    lfo2_phase: f64,
    followers: [EnvelopeFollower; 2],
    allpasses: [[Biquad; MAX_NOTCHES]; 2],
    safe_bass_hp: [Biquad; 2],
    safe_bass_lp: [Biquad; 2],
    warmth_filter: [Biquad; 2],
    delays: [ModDelayLine; 2],
    feedback_state: [f32; 2],
    input: [Vec<f32>; 2],
    output: [Vec<f32>; 2],
}

impl PhaserFlanger {
    #[must_use]
    pub fn new(device: &Device, config: &NodeConfig) -> Self {
        let rate = config.sample_rate.as_f64();
        let max_len = (MAX_DELAY_SECS * rate) as usize + 32;
        let frames = config.max_frames.max(1);
        let seed = device.id.0.get() ^ 0x5a5a_5a5a_a5a5_a5a5;

        Self {
            sample_rate: config.sample_rate,
            lfo: Lfo::new(config.sample_rate, seed),
            lfo2_phase: 0.0,
            followers: [EnvelopeFollower::new(rate), EnvelopeFollower::new(rate)],
            allpasses: [[Biquad::default(); MAX_NOTCHES]; 2],
            safe_bass_hp: [Biquad::default(); 2],
            safe_bass_lp: [Biquad::default(); 2],
            warmth_filter: [Biquad::default(); 2],
            delays: [ModDelayLine::new(max_len), ModDelayLine::new(max_len)],
            feedback_state: [0.0; 2],
            input: [vec![0.0; frames], vec![0.0; frames]],
            output: [vec![0.0; frames], vec![0.0; frames]],
        }
    }
}

impl NativeDevice for PhaserFlanger {
    #[expect(
        clippy::needless_range_loop,
        reason = "the frame and the channel index several arrays at once, the input and output, the ramps and the per-channel filters, not one slice"
    )]
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
        let amount = (val(AMOUNT) / 100.0).clamp(0.0, 1.0);
        let feedback = (val(FEEDBACK) / 100.0).clamp(0.0, 0.99) as f32;
        let fb_invert = val(FEEDBACK_INVERT) >= 0.5;
        let fb_coeff = if fb_invert { -feedback } else { feedback };
        let warmth = (val(WARMTH) / 100.0).clamp(0.0, 1.0) as f32;
        let dry_wet = (val(DRY_WET) / 100.0).clamp(0.0, 1.0) as f32;

        let notches = (val(NOTCHES).round() as usize).clamp(1, MAX_NOTCHES);
        let center = val(CENTER).clamp(20.0, 20_000.0);
        let spread = val(SPREAD).clamp(0.0, 100.0);
        let blend = val(BLEND).clamp(0.0, 1.0);

        let flanger_time_ms = val(FLANGER_TIME).clamp(0.05, 20.0);
        let doubler_time_ms = val(DOUBLER_TIME).clamp(5.0, 100.0);

        let lfo_wave = WAVE_OF[(val(LFO_WAVE).round() as usize).min(WAVE_OF.len() - 1)];
        let lfo_mode_idx = val(LFO_MODE).round() as usize;
        let lfo_hz = val(LFO_HZ).clamp(0.01, 40.0);
        let lfo_div_idx = (val(LFO_DIVISION).round() as usize).min(DIVISIONS.len() - 1);
        let lfo_rate = if lfo_mode_idx == 0 {
            Rate::Hertz(lfo_hz)
        } else {
            Rate::Beats(DIVISIONS[lfo_div_idx].1)
        };

        let lfo_stereo_idx = val(LFO_STEREO).round() as usize;
        let lfo_phase = val(LFO_PHASE);
        let lfo_spin = val(LFO_SPIN);
        let lfo_stereo = if lfo_stereo_idx == 0 {
            Stereo::Phase(lfo_phase)
        } else {
            Stereo::Spin(lfo_spin)
        };
        let lfo_duty = (val(LFO_DUTY) / 100.0).clamp(-1.0, 1.0);

        let lfo2_mix = (val(LFO2_MIX) / 100.0).clamp(0.0, 1.0);
        let lfo2_mode_idx = val(LFO2_MODE).round() as usize;
        let lfo2_hz = val(LFO2_HZ).clamp(0.01, 40.0);
        let lfo2_div_idx = (val(LFO2_DIVISION).round() as usize).min(DIVISIONS.len() - 1);
        let lfo2_effective_hz = if lfo2_mode_idx == 0 {
            lfo2_hz
        } else {
            let bpm = audio.tempo().bpm_at(bl_core::time::Ticks(0)).get();
            bpm / 60.0 / DIVISIONS[lfo2_div_idx].1.max(1e-9)
        };

        let env_on = val(ENV_ON) >= 0.5;
        let env_amount = (val(ENV_AMOUNT) / 100.0).clamp(-1.0, 1.0);
        let env_attack = val(ENV_ATTACK).clamp(0.1, 500.0) / 1000.0;
        let env_release = val(ENV_RELEASE).clamp(1.0, 1000.0) / 1000.0;
        for follower in &mut self.followers {
            follower.set(env_attack, env_release, false);
        }

        let safe_bass_hz = val(SAFE_BASS).clamp(5.0, 3000.0);

        self.lfo.set(audio, lfo_rate, lfo_stereo, 0.0);
        let shape = Shape {
            wave: lfo_wave,
            morph: None,
            duty: lfo_duty,
        };

        let output_ramp = ramp(OUTPUT);

        // Update the Safe Bass crossover's two halves.
        let sb_hp_coeffs = safe_bass_hp_coeffs(safe_bass_hz, rate_f64);
        let sb_lp_coeffs = safe_bass_lp_coeffs(safe_bass_hz, rate_f64);
        for sb in &mut self.safe_bass_hp {
            sb.set(sb_hp_coeffs);
        }
        for sb in &mut self.safe_bass_lp {
            sb.set(sb_lp_coeffs);
        }

        // Update warmth gentle low-pass filter
        let warmth_cutoff = (20_000.0 - warmth as f64 * 12_000.0).clamp(2000.0, 20_000.0);
        let warmth_coeffs = lowpass_coeffs(warmth_cutoff, 0.707, rate_f64);
        for wf in &mut self.warmth_filter {
            wf.set(warmth_coeffs);
        }

        let lfo2_step = lfo2_effective_hz / rate_f64;

        for ch in 0..channels {
            self.input[ch][..frames].copy_from_slice(audio.channel_mut(ch));
        }

        for frame in 0..frames {
            let lfo_vals = self.lfo.step(audio.tempo(), frame, shape);
            self.lfo2_phase = (self.lfo2_phase + lfo2_step).rem_euclid(1.0);
            let tri_val = 1.0 - 4.0 * (self.lfo2_phase - 0.5).abs();

            for ch in 0..channels {
                let in_sample = self.input[ch][frame];

                let env_val = if env_on {
                    self.followers[ch].process(in_sample) * env_amount
                } else {
                    0.0
                };

                let lfo1_val = lfo_vals[ch];
                let combined_lfo = (1.0 - lfo2_mix) * lfo1_val + lfo2_mix * tri_val;
                let total_mod = amount * combined_lfo + env_val;

                let fb_in = in_sample + fb_coeff * self.feedback_state[ch];

                let wet_out = match mode_idx {
                    0 => {
                        // Phaser mode
                        // Blend mixes modulation routing between Center (0.0) and Spread (1.0)
                        let center_mod = total_mod * (1.0 - blend);
                        let spread_mod = total_mod * blend;

                        let mod_center =
                            (center * 2.0_f64.powf(center_mod * 2.0)).clamp(20.0, rate_f64 * 0.45);
                        let mod_spread = (spread + spread_mod * 50.0).clamp(0.0, 100.0);

                        let mut s = fb_in;
                        for k in 0..notches {
                            let fk = phaser_notch_freq(mod_center, notches, mod_spread, k)
                                .clamp(20.0, rate_f64 * 0.45);
                            let ap_c = allpass_coeffs(fk, 10.0, rate_f64);
                            self.allpasses[ch][k].set(ap_c);
                            s = self.allpasses[ch][k].process(s);
                        }
                        // All-pass output forms the wet signal; mixing with input at 50% dry/wet cancels at notches
                        s
                    }
                    1 => {
                        // Flanger mode
                        // As modulation moves right, delay time decreases
                        let mod_time_ms =
                            (flanger_time_ms * (1.0 - total_mod * 0.95)).clamp(0.05, 25.0);
                        let delay_frames = mod_time_ms * rate_f64 / 1000.0;
                        let s = self.delays[ch].read(delay_frames);
                        self.delays[ch].write(in_sample + fb_coeff * s);
                        if fb_invert { -s } else { s }
                    }
                    _ => {
                        // Doubler mode
                        // Bipolar modulation about base delay
                        let tap_factor = if ch == 0 { 1.0 } else { 1.5 };
                        let base_ms = doubler_time_ms * tap_factor;
                        let mod_time_ms =
                            (base_ms + total_mod * (doubler_time_ms * 0.5)).clamp(1.0, 250.0);
                        let delay_frames = mod_time_ms * rate_f64 / 1000.0;
                        let s = self.delays[ch].read(delay_frames);
                        self.delays[ch].write(in_sample + fb_coeff * s);
                        s
                    }
                };

                // Warmth saturation
                let warmed = if warmth > 0.0 {
                    let driven = wet_out * (1.0 + warmth * 1.5);
                    let sat = driven - (warmth * 0.3) * (driven * driven * driven) / 3.0;
                    self.warmth_filter[ch].process(sat)
                } else {
                    wet_out
                };

                // Update feedback state from warmed wet signal
                self.feedback_state[ch] = warmed;

                // Safe Bass crossover: passes dry low frequencies and modulated high frequencies
                let safe_wet = self.safe_bass_lp[ch].process(in_sample)
                    + self.safe_bass_hp[ch].process(warmed);

                // Mix dry and wet
                let mixed = (1.0 - dry_wet) * in_sample + dry_wet * safe_wet;

                let out_gain = output_ramp[frame];
                self.output[ch][frame] = mixed * out_gain;
            }
        }

        for ch in 0..channels {
            audio
                .channel_mut(ch)
                .copy_from_slice(&self.output[ch][..frames]);
        }
    }
}

fn allpass_coeffs(freq: f64, q: f64, sample_rate: f64) -> Coeffs {
    let w0 = (TAU * freq / sample_rate).clamp(1e-5, PI * 0.98);
    let alpha = (w0.sin() / (2.0 * q)).max(1e-6);
    let a0 = 1.0 + alpha;
    Coeffs {
        b0: ((1.0 - alpha) / a0) as f32,
        b1: ((-2.0 * w0.cos()) / a0) as f32,
        b2: 1.0,
        a1: ((-2.0 * w0.cos()) / a0) as f32,
        a2: ((1.0 - alpha) / a0) as f32,
    }
}

/// The Safe Bass crossover's two halves, a first-order pair at the cutoff:
/// the low band takes the input and the high band the wet signal, and the
/// two sum to the signal again wherever the effect leaves it alone, so the
/// dry/wet mix moves the low band neither out of phase nor out of the mix.
/// (Beatlayer's own: the manual gives Safe Bass no slope.)
fn safe_bass_lp_coeffs(freq: f64, sample_rate: f64) -> Coeffs {
    let k = one_pole_k(freq, sample_rate);
    let a = 1.0 / (1.0 + k);
    Coeffs {
        b0: (k * a) as f32,
        b1: (k * a) as f32,
        b2: 0.0,
        a1: ((k - 1.0) * a) as f32,
        a2: 0.0,
    }
}

fn safe_bass_hp_coeffs(freq: f64, sample_rate: f64) -> Coeffs {
    let k = one_pole_k(freq, sample_rate);
    let a = 1.0 / (1.0 + k);
    Coeffs {
        b0: a as f32,
        b1: (-a) as f32,
        b2: 0.0,
        a1: ((k - 1.0) * a) as f32,
        a2: 0.0,
    }
}

/// The bilinear prewarp of a first-order section at `freq`.
fn one_pole_k(freq: f64, sample_rate: f64) -> f64 {
    (PI * freq / sample_rate).clamp(1e-5, PI * 0.49).tan()
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

/// Headless display data for Phaser-Flanger.
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
        0 => "Phaser Notches",
        1 => "Flanger Delay",
        _ => "Doubler Taps",
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

//! PitchLoop89, a two-voice pitch-shifting looping delay (WP-8.52; the
//! PitchLoop89 spec): each voice records the input into a short circular
//! buffer and reads it back from a moving place, at a speed and in a
//! direction that set its pitch, so short snippets loop, transposed and
//! reversed.
//!
//! A voice's read place sits Delay behind the write place and drifts from
//! there by `1 − Speed` frames a frame (Hold stops the write, so it drifts
//! by `−Speed`). When it leaves the buffer it is crossfaded over 5 ms to
//! where Delay puts it, so the snippet loops. Reads go through a
//! windowed sinc so a moving place doesn't alias.

use bl_dsp::Random;
use bl_engine::{DeviceType, NodeConfig, Registry};
use bl_model::device_layout::{BODY_HEIGHT, ControlBand, LayoutNode, Widget};
use bl_model::{
    Device, DeviceDescription, DeviceLayout, DeviceRole, ParamDescriptor, ParamId, ParamUnit,
};

use crate::chorus_ensemble::{highpass_coeffs, lowpass_coeffs};
use crate::framework::{Audio, NativeDevice, ParamBank, add};
use crate::lfo::{Lfo, Rate, Shape, Stereo, Wave};
use crate::ring::{Kernel, MIN_DELAY, Ring, limit};
use crate::sections::Biquad;

/// PitchLoop89's device type.
pub const PITCHLOOP89: &str = "beatlayer.pitchloop89";

/// A voice's parameters follow each other from its first: voice A's are
/// 0 to 7 and voice B's 8 to 15.
const VOICE_PARAMS: u32 = 8;
/// Bandwidth: the buffer's length, one of [`BANDWIDTHS_MS`].
pub const BANDWIDTH_A: ParamId = ParamId(0);
/// Delay: where the read place starts, in per cent of the buffer behind
/// the write place.
pub const DELAY_A: ParamId = ParamId(1);
/// Speed: the read place's speed against the write place's, in per cent;
/// below 0 it reads backwards.
pub const SPEED_A: ParamId = ParamId(2);
/// Low-pass: the 12 dB per octave low-pass's corner, in Hz.
pub const LOW_PASS_A: ParamId = ParamId(3);
/// High-pass: the 12 dB per octave high-pass's corner, in Hz.
pub const HIGH_PASS_A: ParamId = ParamId(4);
/// Feedback: how much of the voice's output returns to its buffer.
pub const FEEDBACK_A: ParamId = ParamId(5);
/// Level: the voice's output gain.
pub const LEVEL_A: ParamId = ParamId(6);
/// Hold: stops the voice writing, so it loops what its buffer holds.
pub const HOLD_A: ParamId = ParamId(7);
/// Voice B's, in the same order.
pub const BANDWIDTH_B: ParamId = ParamId(8);
pub const DELAY_B: ParamId = ParamId(9);
pub const SPEED_B: ParamId = ParamId(10);
pub const LOW_PASS_B: ParamId = ParamId(11);
pub const HIGH_PASS_B: ParamId = ParamId(12);
pub const FEEDBACK_B: ParamId = ParamId(13);
pub const LEVEL_B: ParamId = ParamId(14);
pub const HOLD_B: ParamId = ParamId(15);

/// Routing: one of [`ROUTINGS`].
pub const ROUTING: ParamId = ParamId(16);
/// The vibrato LFO's wave, one of [`VIBRATO_WAVES`].
pub const VIBRATO_WAVE: ParamId = ParamId(17);
/// The vibrato LFO's rate, in Hz.
pub const VIBRATO_RATE: ParamId = ParamId(18);
/// How far the vibrato LFO moves both voices' speed, in cents.
pub const VIBRATO_DEPTH: ParamId = ParamId(19);
/// The position LFO's wave, one of [`POSITION_WAVES`].
pub const POSITION_WAVE: ParamId = ParamId(20);
/// The position LFO's rate, in Hz.
pub const POSITION_RATE: ParamId = ParamId(21);
/// How far the position LFO moves both voices' Delay, in per cent of the
/// buffer.
pub const POSITION_DEPTH: ParamId = ParamId(22);
/// Random: a rising edge draws a new Delay offset for each voice.
pub const RANDOM: ParamId = ParamId(23);
/// Input: the gain of what the voices record.
pub const INPUT: ParamId = ParamId(24);
/// Dry/Wet: the balance between the dry signal and the voices.
pub const DRY_WET: ParamId = ParamId(25);

/// How many parameters the device has.
const PARAMS: usize = 26;

/// The buffer lengths Bandwidth chooses, in milliseconds.
pub const BANDWIDTHS_MS: [f64; 6] = [25.0, 50.0, 100.0, 200.0, 400.0, 800.0];
const BANDWIDTH_NAMES: [&str; 6] = ["25 ms", "50 ms", "100 ms", "200 ms", "400 ms", "800 ms"];
/// The routings, in the chooser's order.
pub const ROUTINGS: [&str; 3] = ["L/R", "Sum", "Cross"];
/// The vibrato LFO's waves and the effects LFO's they play.
pub const VIBRATO_WAVES: [&str; 2] = ["Sine", "Square"];
const VIBRATO_WAVE_OF: [Wave; 2] = [Wave::Sine, Wave::Rectangle];
/// The position LFO's waves and the effects LFO's they play.
pub const POSITION_WAVES: [&str; 7] = [
    "Sine",
    "Triangle",
    "Saw Up",
    "Saw Down",
    "Rectangle",
    "Sample & Hold",
    "Wander",
];
const POSITION_WAVE_OF: [Wave; 7] = [
    Wave::Sine,
    Wave::Triangle,
    Wave::SawUp,
    Wave::SawDown,
    Wave::Rectangle,
    Wave::SampleHold,
    Wave::Wander,
];

/// How long a voice takes to crossfade to where Delay puts it, in seconds.
const CROSSFADE_SECONDS: f64 = 0.005;
/// How often the filters' coefficients are worked out, in frames, counted
/// from the device's first, so the result doesn't depend on the blocks.
const REFRESH_FRAMES: u64 = 16;
/// How far Delay may move the read place in one frame without a crossfade.
const JUMP_FRAMES: f64 = 64.0;
/// The filters' Q: Butterworth.
const Q: f64 = core::f64::consts::FRAC_1_SQRT_2;

/// PitchLoop89's type: Beatlayer's own look, with Live's name.
#[must_use]
pub fn pitchloop89_type() -> DeviceType {
    let mut params = Vec::with_capacity(PARAMS);
    for (voice, name) in ["A", "B"].into_iter().enumerate() {
        let id = |first: ParamId| ParamId(first.0 + voice as u32 * VOICE_PARAMS);
        let named = |field: &str| format!("{field} {name}");
        params.extend([
            ParamDescriptor::choice(id(BANDWIDTH_A), &named("Bandwidth"), &BANDWIDTH_NAMES, 2)
                .describe(
                    "The length of the voice's buffer. A shorter buffer loops a shorter snippet, \
                     an octave higher for each halving, and sounds grittier.",
                ),
            ParamDescriptor::continuous(
                id(DELAY_A),
                &named("Delay"),
                (0.0, 100.0),
                50.0,
                ParamUnit::Percent,
            )
            .describe("How far behind the write place the voice reads, in per cent of its buffer."),
            ParamDescriptor::continuous(
                id(SPEED_A),
                &named("Speed"),
                (-200.0, 200.0),
                100.0,
                ParamUnit::Percent,
            )
            .describe(
                "How fast the voice reads its buffer. 100 % leaves the pitch alone, 200 % raises \
                 it an octave, and below 0 the voice plays backwards.",
            ),
            ParamDescriptor::continuous(
                id(LOW_PASS_A),
                &named("Low-pass"),
                (200.0, 20_000.0),
                20_000.0,
                ParamUnit::Hertz,
            )
            .logarithmic()
            .describe("The corner of the voice's low-pass filter, 12 dB per octave."),
            ParamDescriptor::continuous(
                id(HIGH_PASS_A),
                &named("High-pass"),
                (20.0, 5_000.0),
                20.0,
                ParamUnit::Hertz,
            )
            .logarithmic()
            .describe("The corner of the voice's high-pass filter, 12 dB per octave."),
            ParamDescriptor::continuous(
                id(FEEDBACK_A),
                &named("Feedback"),
                (0.0, 100.0),
                0.0,
                ParamUnit::Percent,
            )
            .describe("How much of the voice's output returns to its buffer."),
            ParamDescriptor::gain(id(LEVEL_A), &named("Level"), 0.0, 0.0)
                .describe("The voice's output level, down to silence."),
            ParamDescriptor::toggle(id(HOLD_A), &named("Hold"), false)
                .describe("Freezes the voice's buffer, so it loops what it holds."),
        ]);
    }
    params.extend([
        ParamDescriptor::choice(ROUTING, "Routing", &ROUTINGS, 0).describe(
            "L/R keeps the voices apart, one to each side. Sum feeds both from the input's sum \
             and mixes both to both outputs. Cross feeds each voice's feedback into the other \
             voice's buffer.",
        ),
        ParamDescriptor::choice(VIBRATO_WAVE, "Vibrato Wave", &VIBRATO_WAVES, 0)
            .describe("The vibrato LFO's waveform."),
        ParamDescriptor::continuous(
            VIBRATO_RATE,
            "Vibrato Rate",
            (0.1, 20.0),
            5.0,
            ParamUnit::Hertz,
        )
        .logarithmic()
        .describe("The vibrato LFO's rate."),
        ParamDescriptor::continuous(
            VIBRATO_DEPTH,
            "Vibrato Depth",
            (0.0, 100.0),
            0.0,
            ParamUnit::Cents,
        )
        .describe("How far the vibrato LFO moves both voices' pitch."),
        ParamDescriptor::choice(POSITION_WAVE, "Position Wave", &POSITION_WAVES, 0)
            .describe("The position LFO's waveform. Wander is a smooth random one."),
        ParamDescriptor::continuous(
            POSITION_RATE,
            "Position Rate",
            (0.01, 20.0),
            0.5,
            ParamUnit::Hertz,
        )
        .logarithmic()
        .describe("The position LFO's rate."),
        ParamDescriptor::continuous(
            POSITION_DEPTH,
            "Position Depth",
            (0.0, 100.0),
            0.0,
            ParamUnit::Percent,
        )
        .describe("How far the position LFO moves both voices' Delay, in per cent of the buffer."),
        ParamDescriptor::toggle(RANDOM, "Random", false).describe(
            "Moves each voice's Delay to a new random place within the position LFO's depth.",
        ),
        ParamDescriptor::gain(INPUT, "Input", 12.0, 0.0)
            .describe("The level of what the voices record, down to silence."),
        ParamDescriptor::continuous(DRY_WET, "Dry/Wet", (0.0, 100.0), 50.0, ParamUnit::Percent)
            .describe("The balance between the dry signal and the voices."),
    ]);
    DeviceType::native(PITCHLOOP89, "PitchLoop89", DeviceRole::AudioEffect).with_description(
        DeviceDescription::new(params)
            .with_layout(layout())
            .with_banks(&[
                (
                    "Main",
                    &[
                        "Speed A",
                        "Delay A",
                        "Speed B",
                        "Delay B",
                        "Routing",
                        "Feedback A",
                        "Feedback B",
                        "Dry/Wet",
                    ],
                ),
                (
                    "Voice A",
                    &[
                        "Bandwidth A",
                        "Delay A",
                        "Speed A",
                        "Feedback A",
                        "Level A",
                        "Hold A",
                    ],
                ),
                (
                    "Voice B",
                    &[
                        "Bandwidth B",
                        "Delay B",
                        "Speed B",
                        "Feedback B",
                        "Level B",
                        "Hold B",
                    ],
                ),
                (
                    "Filters",
                    &["Low-pass A", "High-pass A", "Low-pass B", "High-pass B"],
                ),
                (
                    "Modulation",
                    &[
                        "Vibrato Wave",
                        "Vibrato Rate",
                        "Vibrato Depth",
                        "Position Wave",
                        "Position Rate",
                        "Position Depth",
                        "Random",
                    ],
                ),
                ("Global", &["Routing", "Input", "Dry/Wet"]),
            ]),
    )
}

fn layout() -> DeviceLayout {
    let voice = |first: ParamId| {
        let id = |field: ParamId| ParamId(field.0 - BANDWIDTH_A.0 + first.0);
        LayoutNode::Band(ControlBand::stacked(
            BODY_HEIGHT,
            &[
                (id(BANDWIDTH_A), Widget::Chooser),
                (id(HOLD_A), Widget::Toggle),
                (id(SPEED_A), Widget::Knob),
                (id(DELAY_A), Widget::Knob),
                (id(LEVEL_A), Widget::Knob),
                (id(FEEDBACK_A), Widget::Knob),
                (id(LOW_PASS_A), Widget::Knob),
                (id(HIGH_PASS_A), Widget::Knob),
            ],
        ))
    };
    let shared = LayoutNode::Band(ControlBand::stacked(
        BODY_HEIGHT,
        &[
            (ROUTING, Widget::Chooser),
            (RANDOM, Widget::Toggle),
            (INPUT, Widget::Knob),
            (DRY_WET, Widget::Knob),
            (VIBRATO_WAVE, Widget::Chooser),
            (VIBRATO_RATE, Widget::Knob),
            (VIBRATO_DEPTH, Widget::Knob),
            (POSITION_WAVE, Widget::Chooser),
            (POSITION_RATE, Widget::Knob),
            (POSITION_DEPTH, Widget::Knob),
        ],
    ));
    DeviceLayout::new(LayoutNode::Row(vec![
        voice(BANDWIDTH_A),
        voice(BANDWIDTH_B),
        shared,
    ]))
}

/// Adds PitchLoop89 to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, pitchloop89_type(), PitchLoop89::new);
}

/// What a voice reads its parameters as for one frame.
struct Now {
    /// The buffer's length, in frames.
    length: f64,
    /// Where Delay puts the read place, from 0 to 1.
    position: f64,
    /// The read place's speed against the write place's.
    speed: f64,
    hold: bool,
}

/// One voice: its buffer, where it reads, and its filters.
struct Voice {
    ring: Ring,
    /// How far the read place has drifted from where Delay puts it, in
    /// frames.
    drift: f64,
    /// Where the read place and where Delay put it were last, in frames
    /// behind the write place.
    last: f64,
    last_home: f64,
    /// The read place the voice is leaving, and how far through the
    /// crossfade from it to the new one, from 0 to 1.
    old: f64,
    fade: f64,
    /// Random's last draw for the voice, from −1 to 1.
    offset: f64,
    low: Biquad,
    high: Biquad,
}

impl Voice {
    fn new(len: usize) -> Self {
        Self {
            ring: Ring::new(len),
            drift: 0.0,
            last: MIN_DELAY,
            // Not a place yet, so the first frame crossfades from nothing.
            last_home: f64::NAN,
            old: MIN_DELAY,
            fade: 1.0,
            offset: 0.0,
            low: Biquad::default(),
            high: Biquad::default(),
        }
    }

    /// The voice's output for one frame, through its filters.
    fn read(&mut self, kernel: &Kernel, now: &Now, crossfade: f64) -> f32 {
        let length = now.length.min(self.ring.max_delay() - MIN_DELAY);
        let home = MIN_DELAY + now.position.clamp(0.0, 1.0) * length;
        // The distance behind the write place grows by one a frame while it
        // writes, and shrinks by the speed.
        let gain = if now.hold { 0.0 } else { 1.0 } - now.speed;
        self.drift += gain;
        let mut at = home + self.drift;
        if self.fade < 1.0 {
            self.old += gain;
        }
        // The read place leaves the buffer, or Delay moves it far in a
        // frame (Random, a square wave, automation): it goes to where Delay
        // puts it, crossfaded from where it was.
        if at < MIN_DELAY || at > MIN_DELAY + length || (home - self.last_home).abs() > JUMP_FRAMES
        {
            self.old = self.last + gain;
            self.fade = 0.0;
            self.drift = 0.0;
            at = home;
        }
        self.last_home = home;
        let mut sample = self.ring.read(kernel, at);
        if self.fade < 1.0 {
            let old = self.ring.read(kernel, self.old);
            sample = old + (sample - old) * self.fade as f32;
            self.fade = (self.fade + crossfade).min(1.0);
        }
        self.last = at;
        self.high.process(self.low.process(sample))
    }

    /// Records `sample` and what the voice feeds back, unless it holds.
    fn write(&mut self, sample: f32, hold: bool) {
        if !hold {
            self.ring.write(sample);
        }
    }
}

struct PitchLoop89 {
    kernel: Kernel,
    voices: [Voice; 2],
    vibrato: Lfo,
    position: Lfo,
    random: Random,
    was_random: bool,
    /// Frames the device has run.
    frame: u64,
}

impl PitchLoop89 {
    fn new(device: &Device, config: &NodeConfig) -> Self {
        let rate = config.sample_rate;
        let len = (BANDWIDTHS_MS[5] / 1_000.0 * rate.as_f64()).ceil() as usize + 64;
        let seed = device.id.0.get();
        Self {
            kernel: Kernel::new(),
            voices: [Voice::new(len), Voice::new(len)],
            vibrato: Lfo::new(rate, seed),
            position: Lfo::new(rate, seed ^ 0x5DEE_CE66),
            random: Random::seeded(seed),
            was_random: false,
            frame: 0,
        }
    }
}

impl NativeDevice for PitchLoop89 {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        let rate = audio.sample_rate().as_f64();
        let ix: [usize; PARAMS] = core::array::from_fn(|id| {
            params
                .index(ParamId(id as u32))
                .expect("the description lists every parameter")
        });
        let ramp = |id: ParamId| params.ramp(ix[id.0 as usize]);
        let field =
            |voice: usize, first: ParamId| ramp(ParamId(first.0 + voice as u32 * VOICE_PARAMS));
        let routing = params.value(ix[ROUTING.0 as usize]) as usize;
        let (vibrato_wave, vibrato_rate, vibrato_depth) =
            (ramp(VIBRATO_WAVE), ramp(VIBRATO_RATE), ramp(VIBRATO_DEPTH));
        let (position_wave, position_rate, position_depth) = (
            ramp(POSITION_WAVE),
            ramp(POSITION_RATE),
            ramp(POSITION_DEPTH),
        );
        let (random, input, dry_wet) = (ramp(RANDOM), ramp(INPUT), ramp(DRY_WET));
        let crossfade = 1.0 / (CROSSFADE_SECONDS * rate);
        let tempo = audio.tempo();
        for lfo in [&mut self.vibrato, &mut self.position] {
            lfo.set(audio, Rate::Hertz(1.0), Stereo::Phase(0.0), 0.0);
        }
        let stereo = audio.channels() > 1;
        for i in 0..audio.frames() {
            let wave = VIBRATO_WAVE_OF[(vibrato_wave[i] as usize).min(VIBRATO_WAVES.len() - 1)];
            let cents = self.vibrato.step_with_multiplier(
                tempo,
                i,
                Shape::of(wave),
                f64::from(vibrato_rate[i]),
            )[0] * f64::from(vibrato_depth[i]);
            let vibrato = (cents / 1_200.0).exp2();
            let wave = POSITION_WAVE_OF[(position_wave[i] as usize).min(POSITION_WAVES.len() - 1)];
            let depth = f64::from(position_depth[i]) / 100.0;
            let moved = self.position.step_with_multiplier(
                tempo,
                i,
                Shape::of(wave),
                f64::from(position_rate[i]),
            )[0] * depth;
            let pressed = random[i] >= 0.5;
            if pressed && !self.was_random {
                for voice in &mut self.voices {
                    voice.offset = self.random.next_bipolar();
                }
            }
            self.was_random = pressed;
            // Frames are rounded to filter coefficients on a fixed grid.
            let refresh = self.frame.is_multiple_of(REFRESH_FRAMES);
            let mut wet = [0.0f32; 2];
            let mut holds = [false; 2];
            let mut feedback = [0.0f32; 2];
            for (v, voice) in self.voices.iter_mut().enumerate() {
                if refresh {
                    let low = f64::from(field(v, LOW_PASS_A)[i]);
                    let high = f64::from(field(v, HIGH_PASS_A)[i]);
                    voice.low.set(lowpass_coeffs(low, Q, rate));
                    voice.high.set(highpass_coeffs(high, Q, rate));
                }
                let bandwidth = (field(v, BANDWIDTH_A)[i] as usize).min(BANDWIDTHS_MS.len() - 1);
                holds[v] = field(v, HOLD_A)[i] >= 0.5;
                let now = Now {
                    length: BANDWIDTHS_MS[bandwidth] * rate / 1_000.0,
                    position: f64::from(field(v, DELAY_A)[i]) / 100.0
                        + moved
                        + voice.offset * depth,
                    speed: f64::from(field(v, SPEED_A)[i]) / 100.0 * vibrato,
                    hold: holds[v],
                };
                let sample = voice.read(&self.kernel, &now, crossfade);
                feedback[v] = limit(sample * field(v, FEEDBACK_A)[i] / 100.0);
                wet[v] = sample * field(v, LEVEL_A)[i];
            }
            let left = audio.channel(0)[i];
            let right = if stereo { audio.channel(1)[i] } else { left };
            let (heard, sent) = if routing == 1 {
                let sum = 0.5 * (left + right);
                ([sum, sum], [0.5 * (wet[0] + wet[1]); 2])
            } else {
                ([left, right], wet)
            };
            let cross = routing == 2;
            for (v, voice) in self.voices.iter_mut().enumerate() {
                let returned = feedback[if cross { 1 - v } else { v }];
                voice.write(heard[v] * input[i] + returned, holds[v]);
            }
            let mix = dry_wet[i] / 100.0;
            audio.channel_mut(0)[i] = left * (1.0 - mix) + sent[0] * mix;
            if stereo {
                audio.channel_mut(1)[i] = right * (1.0 - mix) + sent[1] * mix;
            }
            self.frame += 1;
        }
    }
}

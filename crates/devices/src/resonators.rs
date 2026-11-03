//! Resonators, five tuned resonators (WP-8.43; the resonators spec): an input
//! filter, then five parallel feedback combs, each ringing at a pitch the
//! device counts in notes, in degrees of the current scale or in the set's
//! tuning, with Decay, Const, Color and Mode, Width, Gain and Dry/Wet.
//!
//! The manual gives Resonators' controls but no ranges, laws or defaults, so
//! the combs, the laws of Decay, Const and Color, Mode B and the placement
//! are Beatlayer's own (`docs/specs/resonators.md`).

use core::f64::consts::{FRAC_1_SQRT_2, FRAC_PI_4, PI, TAU};

use bl_dsp::LinearSmoother;
use bl_dsp::smooth::DEFAULT_SMOOTHING_SECONDS;
use bl_engine::{DeviceType, KeyPitches, NodeConfig, NodeData, Registry};
use bl_model::device_layout::{BODY_HEIGHT, ControlBand, LayoutNode, Widget};
use bl_model::{
    Device, DeviceDescription, DeviceLayout, DeviceRole, ParamDescriptor, ParamId, ParamUnit, Scale,
};

use crate::eq::{Type, coeffs};
use crate::framework::{Audio, DelayLine, NativeDevice, ParamBank, add};
use crate::midi_fx::use_current_scale;
use crate::sections::{Biquad, Coeffs};

/// Resonators' device type.
pub const RESONATORS: &str = "beatlayer.resonators";

/// The input filter's type: low-pass, band-pass, high-pass or notch (R2).
pub const FILTER_TYPE: ParamId = ParamId(0);
/// The input filter's frequency.
pub const FILTER_FREQ: ParamId = ParamId(1);
/// Mode A or B (R9).
pub const MODE: ParamId = ParamId(2);
/// How long the resonators ring, in seconds (R7).
pub const DECAY: ParamId = ParamId(3);
/// Const: every resonator rings for Decay, whatever its pitch.
pub const CONST: ParamId = ParamId(4);
/// Color: the loops' low-pass and the high shelf after them (R8).
pub const COLOR: ParamId = ParamId(5);
/// Reads the pitches in degrees of the current scale (R4); the title bar
/// draws it.
pub const USE_SCALE: ParamId = ParamId(6);
/// How far the resonators spread between the channels (R10).
pub const WIDTH: ParamId = ParamId(7);
/// The resonators' sum's gain.
pub const GAIN: ParamId = ParamId(8);
/// The balance between the dry signal and the resonators.
pub const DRY_WET: ParamId = ParamId(9);

/// A resonator's switch, as its offset among the resonator's parameters.
pub const ON: u32 = 0;
/// The resonator's gain.
pub const LEVEL: u32 = 1;
/// Resonator I's Note, II to V's Pitch relative to I's.
pub const PITCH: u32 = 2;
/// The resonator's Fine, in cents.
pub const FINE: u32 = 3;

/// Parameters in each resonator.
const FIELDS: u32 = 4;
/// Resonators.
const VOICES: usize = 5;
/// The first resonator's parameters follow the global ones.
const FIRST: u32 = 10;
/// How many parameters the device has.
const PARAMS: usize = FIRST as usize + VOICES * FIELDS as usize;

/// Parameter `field` of resonator `voice` (0 for I to 4 for V).
#[must_use]
pub const fn voice_param(voice: usize, field: u32) -> ParamId {
    ParamId(FIRST + voice as u32 * FIELDS + field)
}

/// The Filter Type choices.
pub const FILTER_TYPES: [&str; 4] = ["Low-pass", "Band-pass", "High-pass", "Notch"];
/// The Mode choices.
pub const MODES: [&str; 2] = ["A", "B"];

/// The resonators' names.
const NAMES: [&str; VOICES] = ["I", "II", "III", "IV", "V"];
/// The Pitch each of II to V starts at.
const PITCHES: [i64; VOICES - 1] = [12, 19, 24, -12];
/// The resonators that start switched on.
const ON_AT_START: [bool; VOICES] = [true, true, true, false, false];
/// Where each resonator sits at Width 100 %, from −1 (left) to +1 (right).
const PLACES: [f64; VOICES] = [0.0, -1.0, 1.0, -1.0, 1.0];

/// The highest key Note reaches: C5, with C3 at key 60.
const HIGHEST_NOTE: i64 = 84;
/// The most degrees Pitch counts with Use Current Scale on.
const MOST_DEGREES: i32 = 21;
/// Mode B's second comb: a fifth above, and 12 dB lower.
const FIFTH: f64 = 1.5;
const FIFTH_GAIN: f64 = 0.251_188_643_150_958;
/// The input filter's Q.
const FILTER_Q: f64 = FRAC_1_SQRT_2;
/// The lowest pitch a comb rings at, in hertz: C-1 and a little below.
const LOWEST_HZ: f64 = 8.0;
/// A comb's pitch stays below this share of the sample rate, so its loop is
/// long enough for the fractional delay.
const HIGHEST_SHARE: f64 = 1.0 / 7.0;
/// `ln(1000)`: the fall of 60 dB in amplitude.
const LN_1000: f64 = 6.907_755_278_982_137;
/// The Color shelf's boost at 10 times the pitch, at +100 %, in dB, and where
/// its corner is, in times the pitch.
const SHELF_DB: f64 = 6.0;
const SHELF_CORNER: f64 = 5.0;
/// Frames of the longest comb line, for the lowest pitch at the highest rate
/// the device is sized for (R11).
const SIZED_FOR_RATE: f64 = 192_000.0;

/// Resonators' type: Beatlayer's own look, Live's parameters (the
/// resonators spec).
#[must_use]
pub fn resonators_type() -> DeviceType {
    let mut params = vec![
        ParamDescriptor::choice(FILTER_TYPE, "Filter Type", &FILTER_TYPES, 1)
            .describe("The input filter: low-pass, band-pass, high-pass or notch."),
        ParamDescriptor::continuous(
            FILTER_FREQ,
            "Filter Frequency",
            (50.0, 18_000.0),
            1_000.0,
            ParamUnit::Hertz,
        )
        .logarithmic()
        .describe("The input filter's frequency."),
        ParamDescriptor::choice(MODE, "Mode", &MODES, 0).describe(
            "A rings each resonator as one comb; B adds a second a fifth above, for a clearer \
             pitch from a low Resonator I.",
        ),
        ParamDescriptor::continuous(DECAY, "Decay", (0.01, 30.0), 1.0, ParamUnit::Seconds)
            .logarithmic()
            .describe("How long the resonators take to fall 60 dB."),
        ParamDescriptor::toggle(CONST, "Const", false).describe(
            "Keeps the decay time the same at every pitch; off, higher resonators ring shorter.",
        ),
        ParamDescriptor::continuous(COLOR, "Color", (-100.0, 100.0), 0.0, ParamUnit::Percent)
            .describe("Darkens the resonators below zero and brightens them above."),
        ParamDescriptor::toggle(USE_SCALE, "Use Current Scale", false).describe(
            "Counts Note and Pitch in degrees of the current scale instead of semitones.",
        ),
        ParamDescriptor::continuous(WIDTH, "Width", (0.0, 100.0), 100.0, ParamUnit::Percent)
            .describe("Spreads the resonators between the left and the right channel."),
        ParamDescriptor::gain(GAIN, "Gain", 12.0, 0.0).describe("The resonators' output level."),
        ParamDescriptor::continuous(DRY_WET, "Dry/Wet", (0.0, 100.0), 100.0, ParamUnit::Percent)
            .describe("The balance between the unprocessed signal and the resonators."),
    ];
    for (voice, name) in NAMES.iter().enumerate() {
        let id = |field| voice_param(voice, field);
        params.push(
            ParamDescriptor::toggle(id(ON), &format!("{name} On"), ON_AT_START[voice])
                .describe("Switches the resonator on and off."),
        );
        params.push(
            ParamDescriptor::gain(id(LEVEL), &format!("{name} Gain"), 0.0, 0.0)
                .describe("The resonator's level."),
        );
        params.push(if voice == 0 {
            ParamDescriptor::integer(id(PITCH), "I Note", (0, HIGHEST_NOTE), 60, ParamUnit::None)
                .describe("Resonator I's note, from C-1 to C5 (C3 is key 60).")
        } else {
            ParamDescriptor::integer(
                id(PITCH),
                &format!("{name} Pitch"),
                (-24, 24),
                PITCHES[voice - 1],
                ParamUnit::Semitones,
            )
            .describe("The resonator's pitch above or below Resonator I's.")
        });
        params.push(
            ParamDescriptor::continuous(
                id(FINE),
                &format!("{name} Fine"),
                (-50.0, 50.0),
                0.0,
                ParamUnit::Cents,
            )
            .describe("Tunes the resonator in cents."),
        );
    }
    DeviceType::native(RESONATORS, "Resonators", DeviceRole::AudioEffect).with_description(
        DeviceDescription::new(params)
            .with_layout(layout())
            .with_title_switch(USE_SCALE)
            .with_banks(&[
                (
                    "Main",
                    &[
                        "Filter Frequency",
                        "I Note",
                        "Decay",
                        "Color",
                        "Width",
                        "Gain",
                        "Dry/Wet",
                        "Mode",
                    ],
                ),
                (
                    "Pitches",
                    &[
                        "II Pitch",
                        "II Gain",
                        "III Pitch",
                        "III Gain",
                        "IV Pitch",
                        "IV Gain",
                        "V Pitch",
                        "V Gain",
                    ],
                ),
                (
                    "Detail",
                    &[
                        "Filter Type",
                        "Const",
                        "I On",
                        "II On",
                        "III On",
                        "IV On",
                        "V On",
                        "I Gain",
                    ],
                ),
                (
                    "Fines",
                    &["I Fine", "II Fine", "III Fine", "IV Fine", "V Fine"],
                ),
            ]),
    )
}

/// The filter, the global controls, a column for each resonator, and the
/// output.
fn layout() -> DeviceLayout {
    let mut parts = vec![
        LayoutNode::Band(ControlBand::stacked(
            BODY_HEIGHT,
            &[(FILTER_TYPE, Widget::Chooser), (FILTER_FREQ, Widget::Knob)],
        )),
        LayoutNode::Band(ControlBand::stacked(
            BODY_HEIGHT,
            &[
                (MODE, Widget::Chooser),
                (CONST, Widget::Toggle),
                (DECAY, Widget::Knob),
                (COLOR, Widget::Knob),
            ],
        )),
    ];
    for voice in 0..VOICES {
        let id = |field| voice_param(voice, field);
        parts.push(LayoutNode::Band(ControlBand::stacked(
            BODY_HEIGHT,
            &[
                (id(ON), Widget::Toggle),
                (id(PITCH), Widget::Number),
                (id(FINE), Widget::Knob),
                (id(LEVEL), Widget::Knob),
            ],
        )));
    }
    parts.push(LayoutNode::Band(ControlBand::stacked(
        BODY_HEIGHT,
        &[
            (WIDTH, Widget::Knob),
            (GAIN, Widget::Knob),
            (DRY_WET, Widget::Knob),
        ],
    )));
    DeviceLayout::new(LayoutNode::Row(parts))
}

/// Adds Resonators to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, resonators_type(), Resonators::new);
}

/// The input filter's coefficients for `kind` (an index into
/// [`FILTER_TYPES`]) at `freq`: a second-order section, 12 dB per octave.
fn filter_coeffs(kind: usize, freq: f64, rate: f64) -> Coeffs {
    match kind {
        0 => coeffs(Type::HighCut12, freq, 0.0, FILTER_Q, rate),
        2 => coeffs(Type::LowCut12, freq, 0.0, FILTER_Q, rate),
        3 => coeffs(Type::Notch, freq, 0.0, FILTER_Q, rate),
        _ => {
            let w0 = TAU * (freq / rate).clamp(1.0e-6, 0.49);
            let (sin, cos) = w0.sin_cos();
            let alpha = sin / (2.0 * FILTER_Q);
            let a0 = 1.0 + alpha;
            Coeffs {
                b0: (alpha / a0) as f32,
                b1: 0.0,
                b2: (-alpha / a0) as f32,
                a1: (-2.0 * cos / a0) as f32,
                a2: ((1.0 - alpha) / a0) as f32,
            }
        }
    }
}

/// The coefficients `a_0` to `a_4` of a fourth-order Thiran allpass whose
/// delay is `d` samples (a value near 4 keeps it stable).
fn thiran(d: f64) -> [f64; 5] {
    const N: usize = 4;
    let mut a = [1.0; N + 1];
    let mut binomial = 1.0;
    for (k, coefficient) in a.iter_mut().enumerate().skip(1) {
        binomial *= (N - k + 1) as f64 / k as f64;
        let product: f64 = (0..=N)
            .map(|n| (d - N as f64 + n as f64) / (d - N as f64 + k as f64 + n as f64))
            .product();
        *coefficient = if k % 2 == 1 { -1.0 } else { 1.0 } * binomial * product;
    }
    a
}

/// One feedback comb: a delay line holding its output, the fractional delay
/// that makes the loop exactly one period long, and Color's low-pass.
struct Comb {
    line: DelayLine,
    rate: f64,
    /// The whole samples of the delay, then the allpass's coefficients.
    whole: usize,
    a: [f64; 5],
    /// The allpass's past inputs and outputs, newest first.
    input: [f64; 4],
    output: [f64; 4],
    /// The loop low-pass: its pole and state.
    pole: f64,
    held: f64,
    /// The feedback, and what the input is scaled by to keep the comb's
    /// level whatever its decay.
    feedback: f64,
    scale: f64,
    /// What it was last designed for.
    designed: Option<(f64, f64, f64)>,
}

impl Comb {
    fn new(rate: f64) -> Self {
        Self {
            line: DelayLine::new((rate.max(SIZED_FOR_RATE) / LOWEST_HZ).ceil() as usize + 8),
            rate,
            whole: 1,
            a: [1.0; 5],
            input: [0.0; 4],
            output: [0.0; 4],
            pole: 0.0,
            held: 0.0,
            feedback: 0.0,
            scale: 1.0,
            designed: None,
        }
    }

    /// Forgets what rings.
    fn clear(&mut self) {
        self.line.clear();
        self.input = [0.0; 4];
        self.output = [0.0; 4];
        self.held = 0.0;
    }

    /// Designs the loop for a pitch of `hz`, a feedback of `feedback` and
    /// `dark` from 0 (open) to 1 (cutoff at twice the pitch), unless it
    /// already is.
    fn tune(&mut self, hz: f64, feedback: f64, dark: f64) {
        if self.designed == Some((hz, feedback, dark)) {
            return;
        }
        self.designed = Some((hz, feedback, dark));
        let w = TAU * hz / self.rate;
        self.pole = if dark > 0.0 {
            (-TAU * 2.0 * hz / dark / self.rate).exp()
        } else {
            0.0
        };
        // Spec: resonators R6. The low-pass delays the loop by its phase
        // delay at the pitch, which the fractional delay leaves out.
        let low_pass_delay = (self.pole * w.sin()).atan2(1.0 - self.pole * w.cos()) / w;
        let total = self.rate / hz - low_pass_delay;
        self.whole = ((total - 4.0).round() as usize).max(1);
        self.a = thiran(total - self.whole as f64);
        self.feedback = feedback.min(MOST_FEEDBACK);
        self.scale = (1.0 - self.feedback.powi(2)).sqrt();
    }

    /// The comb's output for `x`.
    fn run(&mut self, x: f64) -> f64 {
        let delayed = f64::from(self.line.read(self.whole));
        let a = &self.a;
        let mut shifted = a[4] * delayed;
        for k in 0..4 {
            shifted += a[3 - k] * self.input[k] - a[k + 1] * self.output[k];
        }
        self.input.rotate_right(1);
        self.input[0] = delayed;
        self.output.rotate_right(1);
        self.output[0] = shifted;
        self.held = (1.0 - self.pole) * shifted + self.pole * self.held;
        let y = x * self.scale + self.feedback * self.held;
        // Keeps what has died away from slowing the loop as denormals.
        let y = if y.abs() < 1.0e-20 { 0.0 } else { y };
        self.line.write(y as f32);
        y
    }
}

/// The feedback a loop never reaches.
const MOST_FEEDBACK: f64 = 0.9999;

/// One resonator: its combs, the glide of its pitch and its switch, and the
/// shelf after the combs.
struct Voice {
    combs: [Comb; 2],
    /// Its pitch in semitones from A3 = 440 Hz at 69, gliding over 20 ms.
    pitch: LinearSmoother,
    on: LinearSmoother,
    /// The shelf: a one-pole low-pass whose complement is added, and the
    /// gain of that.
    shelf: f64,
    shelf_pole: f64,
    shelf_gain: f64,
    shelf_designed: Option<(f64, f64)>,
    silent: bool,
    second_silent: bool,
    rate: f64,
}

impl Voice {
    fn new(rate: f64, pitch: f64, on: bool) -> Self {
        let glide = LinearSmoother::frames_for(DEFAULT_SMOOTHING_SECONDS, rate);
        Self {
            combs: [Comb::new(rate), Comb::new(rate)],
            pitch: LinearSmoother::new(pitch, glide),
            on: LinearSmoother::new(f64::from(u8::from(on)), glide),
            shelf: 0.0,
            shelf_pole: 0.0,
            shelf_gain: 0.0,
            shelf_designed: None,
            silent: false,
            second_silent: false,
            rate,
        }
    }

    /// The pitch in hertz, held inside what a comb can ring at.
    fn hz(&self) -> f64 {
        hz_of(self.pitch.value(), self.rate)
    }

    /// Designs the shelf for a pitch of `hz` and a boost of `boost` (0 to 1)
    /// of [`SHELF_DB`] at 10 times the pitch, unless it already is.
    fn tune_shelf(&mut self, hz: f64, boost: f64) {
        if self.shelf_designed == Some((hz, boost)) {
            return;
        }
        self.shelf_designed = Some((hz, boost));
        self.shelf_pole = (-TAU * SHELF_CORNER * hz / self.rate).exp();
        self.shelf_gain = 0.0;
        if boost <= 0.0 {
            return;
        }
        // Spec: resonators R8. The shelf is the signal plus a share of what
        // the low-pass leaves out; the share is solved for the boost at 10
        // times the pitch, on the filter's own response there.
        let w = (10.0 * TAU * hz / self.rate).min(PI);
        let p = self.shelf_pole;
        let (re, im) = (1.0 - p * w.cos(), p * w.sin());
        let low = (
            (1.0 - p) * re / (re * re + im * im),
            -(1.0 - p) * im / (re * re + im * im),
        );
        let (a, b) = (1.0 - low.0, -low.1);
        let target = 10f64.powf(SHELF_DB * boost / 20.0);
        let size = a * a + b * b;
        self.shelf_gain = (-a + (a * a + size * (target * target - 1.0)).sqrt()) / size;
    }
}

/// The frequency of `semitones` from A3 = 440 Hz at 69, kept inside what a
/// comb at `rate` can ring at.
fn hz_of(semitones: f64, rate: f64) -> f64 {
    (440.0 * ((semitones - 69.0) / 12.0).exp2()).clamp(LOWEST_HZ, rate * HIGHEST_SHARE)
}

/// What the five pitches are worked out from.
#[derive(Clone, Copy, PartialEq)]
struct Pitches {
    scale: Option<Scale>,
    note: f32,
    steps: [f32; VOICES - 1],
    fines: [f32; VOICES],
}

/// Resonators: the input filter, five voices and the placement.
struct Resonators {
    voices: [Voice; VOICES],
    filter: Biquad,
    filter_designed: Option<(usize, f32)>,
    rate: f64,
    /// The pitch of each key under the set's tuning, and the highest key
    /// that has one.
    keys: KeyPitches,
    top: i32,
    /// What the pitches were last worked out from, and what they came to.
    worked: Option<(Pitches, [f64; VOICES])>,
    fresh: bool,
}

/// The highest key `keys` gives a pitch.
fn highest_key(keys: &KeyPitches) -> i32 {
    (0..=127)
        .rev()
        .find(|&k| keys.pitch(k as u8).is_some())
        .unwrap_or(0)
}

impl Resonators {
    fn new(_: &Device, config: &NodeConfig) -> Self {
        let rate = config.sample_rate.as_f64();
        let keys = KeyPitches::twelve();
        Self {
            voices: core::array::from_fn(|v| {
                let pitch = if v == 0 {
                    60.0
                } else {
                    60.0 + PITCHES[v - 1] as f64
                };
                Voice::new(rate, pitch, ON_AT_START[v])
            }),
            filter: Biquad::default(),
            filter_designed: None,
            rate,
            top: highest_key(&keys),
            keys,
            worked: None,
            fresh: true,
        }
    }

    /// The pitch of each resonator in semitones from A3 = 440 Hz at 69.
    fn semitones(&mut self, p: Pitches) -> [f64; VOICES] {
        if let Some((was, semitones)) = self.worked
            && was == p
        {
            return semitones;
        }
        let top = self.top;
        // Spec: resonators R4. With a scale, Note counts degrees from its
        // root in the octave of C-1, and Pitch from Resonator I's note.
        let note = p.note as i32;
        let first = match p.scale {
            Some(scale) => scale.land_off(scale.root, note),
            None => note,
        }
        .clamp(0, top);
        let key = |steps: i32| {
            match p.scale {
                Some(scale) => {
                    scale.move_off(first as u8, steps.clamp(-MOST_DEGREES, MOST_DEGREES))
                }
                None => first + steps,
            }
            .clamp(0, top)
        };
        let steps = [0.0, p.steps[0], p.steps[1], p.steps[2], p.steps[3]];
        let semitones = core::array::from_fn(|v| {
            // Spec: resonators R5. Each key's pitch is the tuning's.
            let at = self.keys.pitch(key(steps[v] as i32) as u8).unwrap_or(0.0);
            at + f64::from(p.fines[v]) / 100.0
        });
        self.worked = Some((p, semitones));
        semitones
    }
}

impl NativeDevice for Resonators {
    fn receive_data(&mut self, data: NodeData) {
        if let Some(keys) = data.downcast_ref::<KeyPitches>() {
            self.keys = *keys;
            self.top = highest_key(keys);
            self.worked = None;
        }
    }

    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        if audio.frames() == 0 {
            return;
        }
        let ix: [usize; PARAMS] = core::array::from_fn(|id| {
            params
                .index(ParamId(id as u32))
                .expect("the description lists every parameter")
        });
        let ramp = |id: ParamId| params.ramp(ix[id.0 as usize]);
        let voice_ramp = |v: usize, field| ramp(voice_param(v, field));
        let filter_type = params.value(ix[FILTER_TYPE.0 as usize]) as usize;
        let mode_b = params.value(ix[MODE.0 as usize]) >= 0.5;
        let constant = params.value(ix[CONST.0 as usize]) >= 0.5;
        let use_scale = use_current_scale(audio, params, USE_SCALE);
        let (filter_freq, decay, color) = (ramp(FILTER_FREQ), ramp(DECAY), ramp(COLOR));
        let (width, gain, dry_wet) = (ramp(WIDTH), ramp(GAIN), ramp(DRY_WET));
        let on: [&[f32]; VOICES] = core::array::from_fn(|v| voice_ramp(v, ON));
        let level: [&[f32]; VOICES] = core::array::from_fn(|v| voice_ramp(v, LEVEL));
        let note = voice_ramp(0, PITCH);
        let steps: [&[f32]; VOICES - 1] = core::array::from_fn(|v| voice_ramp(v + 1, PITCH));
        let fines: [&[f32]; VOICES] = core::array::from_fn(|v| voice_ramp(v, FINE));

        // shortcut: the resonators are stereo; a mono track passes through.
        // Upgrade with a surround layout.
        if audio.channels() < 2 {
            return;
        }
        for i in 0..audio.frames() {
            let pitches = Pitches {
                scale: use_scale.then(|| audio.scale_at(i as u32)),
                note: note[i],
                steps: core::array::from_fn(|v| steps[v][i]),
                fines: core::array::from_fn(|v| fines[v][i]),
            };
            let semitones = self.semitones(pitches);
            if self.fresh {
                self.fresh = false;
                for (v, voice) in self.voices.iter_mut().enumerate() {
                    voice.pitch.snap(semitones[v]);
                    voice.on.snap(f64::from(u8::from(on[v][i] >= 0.5)));
                }
            }
            let (l, r) = (
                f64::from(audio.channel(0)[i]),
                f64::from(audio.channel(1)[i]),
            );
            let kind = (filter_type, filter_freq[i]);
            if self.filter_designed != Some(kind) {
                self.filter_designed = Some(kind);
                self.filter
                    .set(filter_coeffs(kind.0, f64::from(kind.1), self.rate));
            }
            let heard = f64::from(self.filter.process(((l + r) / 2.0) as f32));
            let (dark, boost) = {
                let c = f64::from(color[i]) / 100.0;
                ((-c).max(0.0), c.max(0.0))
            };
            let (mut wet_left, mut wet_right) = (0.0, 0.0);
            let reference = hz_of(semitones[0], self.rate);
            for v in 0..VOICES {
                let voice = &mut self.voices[v];
                voice.pitch.set_target(semitones[v]);
                voice.pitch.step();
                voice.on.set_target(f64::from(u8::from(on[v][i] >= 0.5)));
                let present = voice.on.step();
                if present == 0.0 {
                    if !voice.silent {
                        voice.silent = true;
                        voice.combs.iter_mut().for_each(Comb::clear);
                        voice.shelf = 0.0;
                    }
                    continue;
                }
                voice.silent = false;
                let hz = voice.hz();
                // Spec: resonators R7. The loop's feedback makes the ring
                // fall 60 dB in Decay at the pitch it is set for: its own
                // with Const on, Resonator I's with it off.
                let fall = |at: f64| {
                    (-LN_1000 / (f64::from(decay[i]) * if constant { at } else { reference })).exp()
                };
                voice.combs[0].tune(hz, fall(hz), dark);
                let mut out = voice.combs[0].run(heard);
                // Spec: resonators R9
                if mode_b {
                    voice.second_silent = false;
                    let second = (hz * FIFTH).min(self.rate * HIGHEST_SHARE);
                    voice.combs[1].tune(second, fall(second), dark);
                    out += FIFTH_GAIN * voice.combs[1].run(heard);
                } else if !voice.second_silent {
                    voice.second_silent = true;
                    voice.combs[1].clear();
                }
                voice.tune_shelf(hz, boost);
                voice.shelf = (1.0 - voice.shelf_pole) * out + voice.shelf_pole * voice.shelf;
                out += voice.shelf_gain * (out - voice.shelf);
                // Spec: resonators R10. Constant power, placed by Width.
                let angle = (PLACES[v] * f64::from(width[i]) / 100.0 + 1.0) * FRAC_PI_4;
                let out = out * f64::from(level[v][i]) * present;
                wet_left += out * angle.cos();
                wet_right += out * angle.sin();
            }
            let wet = f64::from(gain[i]) * f64::from(dry_wet[i]) / 100.0;
            let dry = 1.0 - f64::from(dry_wet[i]) / 100.0;
            audio.channel_mut(0)[i] = (dry * l + wet * wet_left) as f32;
            audio.channel_mut(1)[i] = (dry * r + wet * wet_right) as f32;
        }
    }
}

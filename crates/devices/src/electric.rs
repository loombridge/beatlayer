//! Electric, the native electric piano (WP-8.23; spec: electric). A hammer
//! throws a raised-cosine force pulse at a tine, the physical models'
//! Cantilever resonator; a tone bar, a Beam resonator, is driven by the tine
//! and rings on at its fundamental; a pickup beside the tine turns their
//! displacement into the sound through a nonlinearity that runs at four
//! times the rate, and a damper stops the fork when the key is let go.
//!
//! Every law here is Beatlayer's own reading of the manual's controls on a
//! modal model of the fork, and every range and default but Detune's is
//! Beatlayer's own too (see the spec's list of what to observe).

use core::f64::consts::TAU;
use core::ops::Range;

use bl_dsp::Oversampler;
use bl_dsp::physical::{ModalBank, ModalParams, Quality, Shape};
use bl_dsp::random::Random;
use bl_engine::{DeviceType, NodeConfig, NodeData, NodeEvent, Registry};
use bl_model::device_layout::{
    BODY_HEIGHT, ControlBand, LayoutNode, Page, PageStyle, PagesNode, Widget,
};
use bl_model::param::db_to_gain;
use bl_model::{DeviceDescription, DeviceLayout, DeviceRole, ParamDescriptor, ParamId, ParamUnit};
use lspk_voice::{MAX_VOICES, VoiceBlock, VoiceEvent, VoiceEventKind};

use crate::framework::{Audio, NativeDevice, ParamBank, add};
use crate::instrument::{self, Instrument, NoteRange, Parts, Section, Sources, State};

/// Electric's device type.
pub const ELECTRIC: &str = "beatlayer.electric";

/// The Hammer section: Stiffness, and how velocity and the key move it.
pub const STIFFNESS: ParamId = ParamId(0);
pub const STIFFNESS_VEL: ParamId = ParamId(1);
pub const STIFFNESS_KEY: ParamId = ParamId(2);
/// The hammer's noise: its level, the centre of its band, its decay and how
/// the key moves its level.
pub const NOISE: ParamId = ParamId(3);
pub const NOISE_PITCH: ParamId = ParamId(4);
pub const NOISE_DECAY: ParamId = ParamId(5);
pub const NOISE_KEY: ParamId = ParamId(6);
/// The hammer's force, and how velocity and the key move it.
pub const FORCE: ParamId = ParamId(7);
pub const FORCE_VEL: ParamId = ParamId(8);
pub const FORCE_KEY: ParamId = ParamId(9);
/// The Fork section: the tine's Color, Decay and Key, the tone bar's Decay
/// and the fork's Release.
pub const TINE_COLOR: ParamId = ParamId(10);
pub const TINE_DECAY: ParamId = ParamId(11);
pub const TINE_KEY: ParamId = ParamId(12);
pub const TONE_DECAY: ParamId = ParamId(13);
pub const RELEASE: ParamId = ParamId(14);
/// The Damper's Tone, Level and Att/Rel.
pub const DAMPER_TONE: ParamId = ParamId(15);
pub const DAMPER_LEVEL: ParamId = ParamId(16);
pub const DAMPER_ATT_REL: ParamId = ParamId(17);
/// The Pickup's Symmetry, Distance, Type (R or W), Input, Output and the
/// key's move of its Output.
pub const SYMMETRY: ParamId = ParamId(18);
pub const DISTANCE: ParamId = ParamId(19);
pub const PICKUP_TYPE: ParamId = ParamId(20);
pub const INPUT: ParamId = ParamId(21);
pub const OUTPUT: ParamId = ParamId(22);
pub const OUTPUT_KEY: ParamId = ParamId(23);
/// The Global section: Volume, Voices, Semi, Detune, Stretch, Pitch Bend and
/// Note PB.
pub const VOLUME: ParamId = ParamId(24);
pub const VOICES: ParamId = ParamId(25);
pub const SEMI: ParamId = ParamId(26);
pub const DETUNE: ParamId = ParamId(27);
pub const STRETCH: ParamId = ParamId(28);
pub const PB_RANGE: ParamId = ParamId(29);
pub const NOTE_PB: ParamId = ParamId(30);

/// The names of the pickup types: electrodynamic and electrostatic.
pub const PICKUP_TYPES: [&str; 2] = ["R", "W"];

/// Spec: electric R2. A hammer's pulse is this long at Stiffness 0 and at
/// 100 %.
const SOFT_SECONDS: f64 = 0.005;
const HARD_SECONDS: f64 = 0.0002;

/// Spec: electric R2, R3, R7. A key moves a level by this many dB over four
/// octaves from C3.
const KEY_DB: f64 = 12.0;

/// Frames a modal bank and the pickup run in one go: the physical models'
/// control grid, so every block size gives the same render.
const CHUNK: usize = bl_dsp::physical::CONTROL;

/// The oversampler's stages: four times the rate (R8).
const STAGES: u32 = 2;

/// Natural log of 1,000 times the modal bank's input scale of 4: what an
/// impulse of a given area needs, per second of decay and per frame of
/// rate, to ring the fundamental at that area's height.
const IMPULSE_SCALE: f64 = 27.631_021_115_928_547;

/// The tine's Material and the tone bar's, in the physical models' sense
/// (R5): how much shorter than the fundamental the upper partials last.
const TINE_MATERIAL: f32 = 0.9;
const BAR_MATERIAL: f32 = 0.9;

/// Where the hammer strikes the tine (its tip) and where the tine and bar
/// are heard, in the physical models' positions (R7).
const TINE_HIT: f32 = 1.0;
const TINE_LISTEN: f32 = 0.5;
const BAR_HIT: f32 = 0.5;
const BAR_LISTEN: f32 = 0.5;

/// Where Color 0 leaves the tine's brightness (R6 of the physical models):
/// the resonator's impulse response favours the upper partials, so the
/// neutral colour is a little dark.
const BASE_COLOR: f32 = -0.4;

/// How much of the tine's displacement drives the tone bar (R4), per unit of
/// the tone bar's decay over the tine's: a bar that rings longer than the
/// tine takes proportionally more of a tine that dies sooner, so what it
/// takes does not shrink with the ratio.
const COUPLING: f32 = 0.1;

/// The hammer's noise, at 100 %, as a share of the tine's full swing.
const NOISE_SCALE: f32 = 0.2;

/// The damper's noise lasts this long, and falls with this time constant.
const DAMPER_SECONDS: f64 = 0.1;
const DAMPER_TAU: f64 = 0.012;

/// Where the pickup's geometry sits: the least Distance, and its reach at
/// 100 %; Symmetry's offset of the tine's rest at 0 and 100 %.
const GAP_NEAR: f32 = 1.0;
const GAP_FAR: f32 = 4.0;
const OFFSET: f32 = 0.6;

/// After the key is let go the fork is faded to nothing over the last tenth
/// of this many times Release, by which time it is 108 dB down.
const RELEASE_TAIL: f64 = 1.8;

/// Frames of zeros the pickup's filters hold before a voice ends.
const END_PADDING: usize = 128;

/// Where a modal bank's smoothing of pickup geometry settles per chunk.
const GEOMETRY_SMOOTHING: f32 = 0.3;

/// Spec: electric R1. Beatlayer's own factory presets: a name and the
/// parameters it sets, every other one at its default.
pub const PRESETS: [(&str, &[(ParamId, f64)]); 4] = [
    (
        "Soft Tine",
        &[
            (STIFFNESS, 25.0),
            (FORCE, 55.0),
            (FORCE_VEL, 40.0),
            (NOISE, 10.0),
            (TINE_COLOR, -20.0),
            (TINE_DECAY, 5_000.0),
            (TONE_DECAY, 9_000.0),
            (RELEASE, 400.0),
            (DISTANCE, 70.0),
        ],
    ),
    (
        "Bark",
        &[
            (STIFFNESS, 70.0),
            (STIFFNESS_VEL, 50.0),
            (FORCE, 75.0),
            (FORCE_VEL, 60.0),
            (NOISE, 35.0),
            (NOISE_PITCH, 2_500.0),
            (TINE_DECAY, 3_000.0),
            (TONE_DECAY, 6_000.0),
            (RELEASE, 250.0),
            (SYMMETRY, 40.0),
            (DISTANCE, 15.0),
            (PICKUP_TYPE, 0.0),
            (INPUT, 2.0),
        ],
    ),
    (
        "Glass",
        &[
            (STIFFNESS, 90.0),
            (TINE_COLOR, 50.0),
            (TINE_DECAY, 9_000.0),
            (TONE_DECAY, 4_000.0),
            (RELEASE, 900.0),
            (SYMMETRY, 55.0),
            (DISTANCE, 60.0),
            (DAMPER_LEVEL, 0.03),
        ],
    ),
    (
        "Felt",
        &[
            (STIFFNESS, 5.0),
            (FORCE, 70.0),
            (NOISE, 0.0),
            (TINE_COLOR, -60.0),
            (TINE_DECAY, 2_000.0),
            (TONE_DECAY, 3_000.0),
            (RELEASE, 150.0),
            (DISTANCE, 80.0),
            (DAMPER_TONE, 20.0),
            (DAMPER_LEVEL, 0.1),
        ],
    ),
];

fn knobs(params: &[ParamId]) -> Vec<(ParamId, Widget)> {
    params.iter().map(|&p| (p, Widget::Knob)).collect()
}

/// A page of the shell: its section button's control, and the controls the
/// display area holds.
fn page(name: &str, section: ParamId, rest: &[(ParamId, Widget)]) -> Page {
    Page::new(
        name,
        LayoutNode::Band(ControlBand::stacked(BODY_HEIGHT, rest)),
    )
    .with_section(LayoutNode::Band(ControlBand::stacked(
        60.0,
        &knobs(&[section]),
    )))
}

/// Electric's type.
#[must_use]
pub fn electric_type() -> DeviceType {
    let percent = |id, name: &str, default: f64| {
        ParamDescriptor::continuous(id, name, (0.0, 100.0), default, ParamUnit::Percent)
    };
    let amount = |id, name: &str| {
        ParamDescriptor::continuous(id, name, (-100.0, 100.0), 0.0, ParamUnit::Percent)
    };
    let time = |id, name: &str, min: f64, max: f64, default: f64| {
        ParamDescriptor::continuous(id, name, (min, max), default, ParamUnit::Milliseconds)
            .logarithmic()
    };
    let params = vec![
        percent(STIFFNESS, "Stiffness", 50.0)
            .describe("How hard the hammer is: a harder hammer strikes for less time and sounds brighter."),
        amount(STIFFNESS_VEL, "Stiffness Vel")
            .describe("How far the note's velocity moves the stiffness."),
        amount(STIFFNESS_KEY, "Stiffness Key")
            .describe("How far the key moves the stiffness: positive makes higher keys harder."),
        percent(NOISE, "Noise", 20.0)
            .describe("The level of the burst of noise the hammer adds to the strike."),
        ParamDescriptor::continuous(NOISE_PITCH, "Noise Pitch", (100.0, 10_000.0), 1_000.0, ParamUnit::Hertz)
            .logarithmic()
            .describe("The centre of the hammer noise's band."),
        time(NOISE_DECAY, "Noise Decay", 1.0, 1_000.0, 30.0)
            .describe("How long the hammer noise takes to fall 60 dB."),
        amount(NOISE_KEY, "Noise Key")
            .describe("How far the key moves the noise's level, up to 12 dB over four octaves from C3."),
        percent(FORCE, "Force", 60.0)
            .describe("How hard the hammer strikes, from soft to hard."),
        amount(FORCE_VEL, "Force Vel")
            .describe("How far the note's velocity moves the force."),
        amount(FORCE_KEY, "Force Key")
            .describe("How far the key moves the force: positive makes higher keys strike harder."),
        amount(TINE_COLOR, "Tine Color")
            .describe("The balance of the tine's high and low partials: positive brightens it."),
        time(TINE_DECAY, "Tine Decay", 100.0, 60_000.0, 4_000.0)
            .describe("How long the tine takes to fall 60 dB while the key is held."),
        amount(TINE_KEY, "Tine Key")
            .describe("How far the key moves the tine's level, up to 12 dB over four octaves from C3."),
        time(TONE_DECAY, "Tone Decay", 100.0, 60_000.0, 8_000.0)
            .describe("How long the tone bar takes to fall 60 dB while the key is held."),
        time(RELEASE, "Release", 10.0, 10_000.0, 300.0)
            .describe("How long the fork takes to fall 60 dB after the key is let go."),
        percent(DAMPER_TONE, "Damper Tone", 50.0)
            .describe("The colour of the damper's noise, from soft to bright."),
        ParamDescriptor::gain(DAMPER_LEVEL, "Damper Level", 0.0, -24.0)
            .describe("The level of the damper's noise, down to silence."),
        amount(DAMPER_ATT_REL, "Damper Att/Rel")
            .describe("Where the damper sounds: -100 % only when the note starts, 100 % only when it ends, 0 at both."),
        percent(SYMMETRY, "Pickup Symmetry", 50.0)
            .describe("The pickup's height against the tine: 50 % is straight in front, and away from it the even harmonics rise."),
        percent(DISTANCE, "Pickup Distance", 50.0)
            .describe("How far the pickup is from the tine: the closer, the more it overdrives."),
        ParamDescriptor::choice(PICKUP_TYPE, "Pickup Type", &PICKUP_TYPES, 1)
            .describe("R is an electrodynamic pickup, which responds to the change of a magnetic field; W is an electrostatic one, which responds to capacitance."),
        ParamDescriptor::gain(INPUT, "Pickup Input", 12.0, 0.0)
            .describe("How loud the fork's signal enters the pickup, which drives its distortion."),
        ParamDescriptor::gain(OUTPUT, "Pickup Output", 12.0, 0.0)
            .describe("The level of the pickup's signal."),
        amount(OUTPUT_KEY, "Pickup Out Key")
            .describe("How far the key moves the pickup's output level, up to 12 dB over four octaves from C3."),
        ParamDescriptor::gain(VOLUME, "Volume", 0.0, 0.0)
            .describe("Sets the instrument's output level, down to silence."),
        instrument::voices(VOICES, MAX_VOICES as i64, 16),
        ParamDescriptor::integer(SEMI, "Semi", (-48, 48), 0, ParamUnit::Semitones)
            .decimals(0)
            .describe("Transposes the instrument in semitones."),
        ParamDescriptor::continuous(DETUNE, "Detune", (-50.0, 50.0), 0.0, ParamUnit::Cents)
            .describe("Tunes the instrument finely, in cents."),
        instrument::stretch(STRETCH),
        ParamDescriptor::integer(PB_RANGE, "Pitch Bend", (0, 24), 2, ParamUnit::Semitones)
            .decimals(0)
            .describe("Sets the pitch bend wheel's range, in semitones."),
        ParamDescriptor::integer(NOTE_PB, "Note PB", (0, 48), 48, ParamUnit::Semitones)
            .decimals(0)
            .describe("Sets the range of a per-note pitch bend, in semitones."),
    ];
    let layout = DeviceLayout::new(LayoutNode::Row(vec![
        LayoutNode::Pages(PagesNode::new(
            "Sections",
            PageStyle::Shell,
            vec![
                page(
                    "Hammer",
                    FORCE,
                    &knobs(&[
                        STIFFNESS,
                        STIFFNESS_VEL,
                        STIFFNESS_KEY,
                        FORCE_VEL,
                        FORCE_KEY,
                        NOISE,
                        NOISE_PITCH,
                        NOISE_DECAY,
                        NOISE_KEY,
                    ]),
                ),
                page(
                    "Fork",
                    RELEASE,
                    &knobs(&[TINE_COLOR, TINE_DECAY, TINE_KEY, TONE_DECAY]),
                ),
                page(
                    "Damper/Pickup",
                    SYMMETRY,
                    &[
                        (DISTANCE, Widget::Knob),
                        (PICKUP_TYPE, Widget::Chooser),
                        (INPUT, Widget::Knob),
                        (OUTPUT, Widget::Knob),
                        (OUTPUT_KEY, Widget::Knob),
                        (DAMPER_TONE, Widget::Knob),
                        (DAMPER_LEVEL, Widget::Knob),
                        (DAMPER_ATT_REL, Widget::Knob),
                    ],
                ),
            ],
        )),
        LayoutNode::Band(ControlBand::stacked(
            BODY_HEIGHT,
            &[
                (VOLUME, Widget::Knob),
                (VOICES, Widget::Number),
                (SEMI, Widget::Number),
                (DETUNE, Widget::Knob),
                (STRETCH, Widget::Knob),
                (PB_RANGE, Widget::Number),
                (NOTE_PB, Widget::Number),
            ],
        )),
    ]));
    DeviceType::native(ELECTRIC, "Electric", DeviceRole::Instrument).with_description(
        DeviceDescription::new(params)
            .with_layout(layout)
            .with_banks(&[
                (
                    "Hammer",
                    &[
                        "Stiffness",
                        "Stiffness Vel",
                        "Stiffness Key",
                        "Force",
                        "Force Vel",
                        "Force Key",
                    ],
                ),
                (
                    "Noise",
                    &["Noise", "Noise Pitch", "Noise Decay", "Noise Key"],
                ),
                (
                    "Fork",
                    &[
                        "Tine Color",
                        "Tine Decay",
                        "Tine Key",
                        "Tone Decay",
                        "Release",
                    ],
                ),
                ("Damper", &["Damper Tone", "Damper Level", "Damper Att/Rel"]),
                (
                    "Pickup",
                    &[
                        "Pickup Symmetry",
                        "Pickup Distance",
                        "Pickup Type",
                        "Pickup Input",
                        "Pickup Output",
                        "Pickup Out Key",
                    ],
                ),
                (
                    "Global",
                    &[
                        "Volume",
                        "Voices",
                        "Semi",
                        "Detune",
                        "Stretch",
                        "Pitch Bend",
                        "Note PB",
                    ],
                ),
            ]),
    )
}

/// Adds Electric to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, electric_type(), |_, config| Electric::new(config));
}

/// Spec: electric R2. Sample `n` of the hammer's raised-cosine force pulse
/// of `width` samples, scaled so the pulse sums to 1: a harder hammer is
/// shorter and brighter with the same impulse.
#[must_use]
pub fn pulse(n: usize, width: usize) -> f32 {
    if n >= width {
        return 0.0;
    }
    let phase = TAU * (n as f64 + 0.5) / width as f64;
    ((1.0 - phase.cos()) / width as f64) as f32
}

/// Spec: electric R2. How long a hammer's pulse is at `stiffness` (0 to 1),
/// in seconds: 5 ms soft to 0.2 ms hard, evenly in octaves.
#[must_use]
pub fn pulse_seconds(stiffness: f64) -> f64 {
    SOFT_SECONDS * (HARD_SECONDS / SOFT_SECONDS).powf(stiffness.clamp(0.0, 1.0))
}

/// Spec: electric R2, R3. `base` (0 to 1) moved by velocity and the key,
/// each by its amount (−1 to 1) over half the range, and kept in 0 to 1.
fn moved(base: f64, vel_amount: f64, key_amount: f64, velocity: f64, key: f64) -> f64 {
    (base + 0.5 * (vel_amount * (2.0 * velocity - 1.0) + key_amount * key)).clamp(0.0, 1.0)
}

/// What a stretch reads from the parameters.
#[derive(Clone, Copy, Default)]
struct Settings {
    stiffness: f64,
    stiffness_vel: f64,
    stiffness_key: f64,
    noise: f64,
    noise_pitch: f64,
    noise_decay: f64,
    noise_key: f64,
    force: f64,
    force_vel: f64,
    force_key: f64,
    color: f64,
    tine_decay: f64,
    tine_key: f64,
    tone_decay: f64,
    release: f64,
    damper_tone: f64,
    damper_level: f64,
    damper_att_rel: f64,
    symmetry: f64,
    distance: f64,
    electrostatic: bool,
    input: f64,
    output: f64,
    output_key: f64,
    volume: f64,
    detune: f64,
}

impl Settings {
    fn read(params: &ParamBank) -> Self {
        let v = |id: ParamId| params.value(params.index(id).expect("the description lists it"));
        Self {
            stiffness: v(STIFFNESS) / 100.0,
            stiffness_vel: v(STIFFNESS_VEL) / 100.0,
            stiffness_key: v(STIFFNESS_KEY) / 100.0,
            noise: v(NOISE) / 100.0,
            noise_pitch: v(NOISE_PITCH),
            noise_decay: v(NOISE_DECAY) / 1000.0,
            noise_key: v(NOISE_KEY) / 100.0,
            force: v(FORCE) / 100.0,
            force_vel: v(FORCE_VEL) / 100.0,
            force_key: v(FORCE_KEY) / 100.0,
            color: v(TINE_COLOR) / 100.0,
            tine_decay: (v(TINE_DECAY) / 1000.0).clamp(0.001, 60.0),
            tine_key: v(TINE_KEY) / 100.0,
            tone_decay: (v(TONE_DECAY) / 1000.0).clamp(0.001, 60.0),
            release: (v(RELEASE) / 1000.0).clamp(0.001, 60.0),
            damper_tone: v(DAMPER_TONE) / 100.0,
            damper_level: v(DAMPER_LEVEL),
            damper_att_rel: v(DAMPER_ATT_REL) / 100.0,
            symmetry: v(SYMMETRY) / 100.0,
            distance: v(DISTANCE) / 100.0,
            electrostatic: v(PICKUP_TYPE) >= 0.5,
            input: v(INPUT),
            output: v(OUTPUT),
            output_key: v(OUTPUT_KEY) / 100.0,
            volume: v(VOLUME),
            detune: v(DETUNE),
        }
    }
}

/// Spec: electric R7. The pickup's geometry and gains, as one chunk plays
/// them.
#[derive(Clone, Copy, Default)]
struct Geometry {
    /// The distance from the pickup to the tine's neutral line, in units of
    /// the tine's full swing, and the tine's rest offset from the pickup's
    /// centre.
    gap: f32,
    offset: f32,
    input: f32,
    output: f32,
    volume: f32,
}

impl Geometry {
    fn of(settings: &Settings) -> Self {
        Self {
            gap: GAP_NEAR + (GAP_FAR - GAP_NEAR) * settings.distance as f32,
            offset: (settings.symmetry as f32 - 0.5) * 2.0 * OFFSET,
            input: settings.input as f32,
            output: settings.output as f32,
            volume: settings.volume as f32,
        }
    }

    /// Moves a fraction of the way to `target`, once per chunk on the
    /// global grid.
    fn toward(&mut self, target: &Self) {
        let step = |now: &mut f32, to: f32| *now += (to - *now) * GEOMETRY_SMOOTHING;
        step(&mut self.gap, target.gap);
        step(&mut self.offset, target.offset);
        step(&mut self.input, target.input);
        step(&mut self.output, target.output);
        step(&mut self.volume, target.volume);
    }
}

/// Spec: electric R7. How far the tine has moved from its rest `c`, when
/// it is driven to `c + drive`, kept short of the pickup at `gap` on either
/// side: the position passes unchanged up to 0.6 of the gap and bends
/// smoothly to 0.92 of it, so the pickup's 1 / (gap ∓ q) laws never divide
/// by zero. A swing that stays inside the knee is returned as it is, so a
/// tiny one keeps all its precision.
fn displacement(c: f32, drive: f32, gap: f32) -> f32 {
    let knee = 0.6 * gap;
    let q = c + drive;
    let reach = q.abs();
    if reach <= knee {
        return drive;
    }
    let span = 0.32 * gap;
    q.signum() * (knee + span * ((reach - knee) / span).tanh()) - c
}

/// The two pickup types' laws, for one sample at four times the rate.
struct Pickup {
    electrostatic: bool,
    gap: f32,
    offset: f32,
    input: f32,
    /// 1 / (d − c) and 1 / (d + c), at the tine's rest.
    near: f32,
    far: f32,
    /// The laws' slope at rest, inverted, to give unity for a small swing.
    unit: f32,
}

/// What an electrodynamic pickup remembers of the sample before: the
/// tine's displacement, and 1 / (d − q) and 1 / (d + q) for it.
#[derive(Clone, Copy)]
struct Field {
    delta: f32,
    near: f32,
    far: f32,
}

impl Field {
    /// The tine at rest.
    fn at_rest(geometry: &Geometry) -> Self {
        Self {
            delta: 0.0,
            near: 1.0 / (geometry.gap - geometry.offset),
            far: 1.0 / (geometry.gap + geometry.offset),
        }
    }
}

impl Pickup {
    fn new(electrostatic: bool, geometry: &Geometry) -> Self {
        let Geometry {
            gap, offset, input, ..
        } = *geometry;
        let (near, far) = (1.0 / (gap - offset), 1.0 / (gap + offset));
        // W's slope is 1 / (d − c)² + 1 / (d + c)², R's twice the cubes.
        let slope = if electrostatic {
            near * near + far * far
        } else {
            2.0 * (near * near * near + far * far * far)
        };
        Self {
            electrostatic,
            gap,
            offset,
            input,
            near,
            far,
            unit: 1.0 / slope,
        }
    }

    /// Spec: electric R7. The pickup's signal for the tine's displacement
    /// `x`, which is 0 at rest and `x` for a small swing. The pickup is a
    /// balanced pair of the two laws, one each side of the tine's rest, so
    /// at Symmetry 50 % the signal is odd in `x` and only odd harmonics
    /// come out: type W (the electrostatic one) goes as 1 / (d − q) −
    /// 1 / (d + q), and type R (the electrodynamic one) as the change of 1 /
    /// (d − q)² − 1 / (d + q)², scaled by `reference` to pass the note's
    /// own pitch at unity. `last` is R's previous sample. Both are written
    /// as differences of the laws, so a tiny swing isn't lost to rounding.
    #[inline]
    fn step(&self, last: &mut Field, reference: f32, x: f32) -> f32 {
        let (d, c) = (self.gap, self.offset);
        let delta = displacement(c, self.input * x, d);
        let q = c + delta;
        let (a, p) = (d - q, d + q);
        let t = 1.0 / (a * p);
        let (near, far) = (p * t, a * t);
        if self.electrostatic {
            let across = self.near * near + self.far * far;
            delta * across * self.unit
        } else {
            let across = near * last.near * (near + last.near) + far * last.far * (far + last.far);
            let change = (delta - last.delta) * across;
            *last = Field { delta, near, far };
            change * reference * self.unit
        }
    }
}

/// A band-pass filter of constant 0 dB peak gain and Q of 1 (the
/// transposed direct form II), for the hammer's noise.
#[derive(Clone, Copy, Default)]
struct Band {
    b0: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
}

impl Band {
    fn at(hz: f64, rate: f64) -> Self {
        let w = TAU * (hz / rate).min(0.45);
        let alpha = w.sin() / 2.0;
        let a0 = 1.0 + alpha;
        Self {
            b0: (alpha / a0) as f32,
            a1: (-2.0 * w.cos() / a0) as f32,
            a2: ((1.0 - alpha) / a0) as f32,
            z1: 0.0,
            z2: 0.0,
        }
    }

    fn step(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.z1;
        self.z1 = -self.a1 * y + self.z2;
        self.z2 = -self.b0 * x - self.a2 * y;
        y
    }
}

/// A burst of noise: its level now, how it falls and how many frames it has
/// left.
#[derive(Clone, Copy, Default)]
struct Burst {
    level: f32,
    fall: f32,
    left: usize,
}

impl Burst {
    fn next(&mut self) -> f32 {
        let level = self.level;
        self.level *= self.fall;
        self.left -= 1;
        level
    }
}

/// One voice's note.
#[derive(Clone)]
struct Voice {
    /// Frames since the note started, and since it was let go.
    age: usize,
    released: Option<usize>,
    /// The hammer's pulse: its width in frames and its height.
    width: usize,
    amp: f32,
    tine: ModalParams,
    bar: ModalParams,
    tine_gain: f32,
    /// How much of the tine drives the tone bar.
    drive: f32,
    /// The note's velocity, and the output's key factor.
    level: f32,
    out_key: f32,
    /// Frames after release when the fork is gone, and when the voice ends.
    gone: usize,
    end: usize,
    hammer: Burst,
    band: Band,
    hammer_gain: f32,
    rng: Random,
    damper: [Burst; 2],
    damper_lp: f32,
    damper_coefficient: f32,
    damper_rng: Random,
    /// The electrodynamic pickup's last sample, and its differentiator's
    /// scale for the note's pitch.
    field: Field,
    reference: f32,
}

impl Voice {
    fn new() -> Self {
        Self {
            age: 0,
            released: None,
            width: 2,
            amp: 0.0,
            tine: ModalParams::default(),
            bar: ModalParams::default(),
            tine_gain: 1.0,
            drive: 0.0,
            level: 0.0,
            out_key: 1.0,
            gone: 0,
            end: 0,
            hammer: Burst::default(),
            band: Band::default(),
            hammer_gain: 0.0,
            rng: Random::seeded(SEED),
            damper: [Burst::default(); 2],
            damper_lp: 0.0,
            damper_coefficient: 0.0,
            damper_rng: Random::seeded(SEED),
            field: Field {
                delta: 0.0,
                near: 1.0,
                far: 1.0,
            },
            reference: 1.0,
        }
    }
}

/// The instrument the voice section plays.
struct Fork {
    rate: f64,
    settings: Settings,
    geometry: Geometry,
    primed: bool,
    tine: ModalBank,
    bar: ModalBank,
    oversampler: Oversampler,
    voices: Vec<Voice>,
    /// How many notes have started: each note's noise is seeded from it, so
    /// a render doesn't depend on which voice a note lands on.
    notes: u64,
    stride: usize,
    exc: Vec<f32>,
    bar_in: Vec<f32>,
    tine_out: Vec<f32>,
    bar_out: Vec<f32>,
    /// The mix, one value a frame, for the stretch.
    out: Vec<f32>,
    /// The mix's DC blocker: the last input and output.
    dc: [f32; 2],
    dc_pole: f32,
}

/// Electric.
pub struct Electric {
    section: Section,
    fork: Fork,
}

const SEED: u64 = 0xE1EC;

impl Electric {
    /// Makes an Electric for `config`'s rate and block size. Every buffer
    /// and table is made here, off the audio thread.
    #[must_use]
    pub fn new(config: &NodeConfig) -> Self {
        let rate = config.sample_rate.as_f64();
        let max_frames = config.max_frames.max(1);
        let mut tine = ModalBank::new(rate as f32, MAX_VOICES);
        tine.rebuild_table(Shape::Cantilever, Quality::Low, 1.0, 0.0);
        let mut bar = ModalBank::new(rate as f32, MAX_VOICES);
        bar.rebuild_table(Shape::Beam, Quality::Eco, 1.0, 0.0);
        let stride = tine.stride();
        Self {
            section: Section::new(
                Parts {
                    voices: Some(VOICES),
                    transpose: Some(SEMI),
                    stretch: Some(STRETCH),
                    pb_range: Some(PB_RANGE),
                    note_range: NoteRange::Own(NOTE_PB),
                    ..Parts::default()
                },
                MAX_VOICES,
                1,
                rate,
                SEED,
            ),
            fork: Fork {
                rate,
                settings: Settings::default(),
                geometry: Geometry::default(),
                primed: false,
                tine,
                bar,
                oversampler: Oversampler::new(STAGES, MAX_VOICES, CHUNK),
                voices: vec![Voice::new(); MAX_VOICES],
                notes: 0,
                stride,
                exc: vec![0.0; CHUNK * stride],
                bar_in: vec![0.0; CHUNK * stride],
                tine_out: vec![0.0; CHUNK * stride * 2],
                bar_out: vec![0.0; CHUNK * stride * 2],
                out: vec![0.0; max_frames],
                dc: [0.0; 2],
                dc_pole: (-TAU * 5.0 / rate).exp() as f32,
            },
        }
    }
}

impl NativeDevice for Electric {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        let frames = audio.frames();
        if frames == 0 || audio.channels() == 0 {
            return;
        }
        let fork = &mut self.fork;
        fork.settings = Settings::read(params);
        if !fork.primed {
            fork.primed = true;
            fork.geometry = Geometry::of(&fork.settings);
        }
        self.section.process(audio, params, fork);
        for channel in 0..audio.channels() {
            audio
                .channel_mut(channel)
                .copy_from_slice(&self.fork.out[..frames]);
        }
    }

    fn flush(&mut self, _events: &[NodeEvent], _params: &ParamBank) {
        self.section.reset();
        self.fork.silence();
    }

    fn receive_data(&mut self, data: NodeData) {
        self.section.receive_data(&data);
    }

    /// Spec: electric R8. The pickup's oversampler delays the sound.
    fn latency(&self) -> u32 {
        self.fork.oversampler.latency()
    }
}

impl Fork {
    /// Forgets every note, without allocating.
    fn silence(&mut self) {
        self.notes = 0;
        for (v, voice) in self.voices.iter_mut().enumerate() {
            *voice = Voice::new();
            self.tine.reset_voice(v, &ModalParams::default());
            self.bar.reset_voice(v, &ModalParams::default());
        }
        self.oversampler.reset();
        self.dc = [0.0; 2];
    }

    /// A note starts on `v`, `time` frames into the stretch.
    fn start(&mut self, voices: &VoiceBlock, state: &State, v: usize, time: usize) {
        let s = self.settings;
        let geometry = self.geometry;
        let sources = Sources::of(voices);
        let (velocity, key) = (sources.velocity(v), sources.key(v));
        let pitch = state.pitch(voices, v, &(time..time), 0) + s.detune / 100.0;
        let hz = hz_of(pitch);
        let force = moved(s.force, s.force_vel, s.force_key, velocity, key);
        let stiffness = moved(s.stiffness, s.stiffness_vel, s.stiffness_key, velocity, key);
        let width = (pulse_seconds(stiffness) * self.rate).round().max(2.0) as usize;
        let gain = |amount: f64| db_to_gain(amount * KEY_DB * key) as f32;
        let tine = ModalParams {
            hz,
            decay: s.tine_decay as f32,
            material: TINE_MATERIAL,
            brightness: (s.color as f32 + BASE_COLOR).clamp(-1.0, 1.0),
            hit: TINE_HIT,
            pos_l: TINE_LISTEN,
            pos_r: TINE_LISTEN,
        };
        let bar = ModalParams {
            hz,
            decay: s.tone_decay as f32,
            material: BAR_MATERIAL,
            brightness: 0.0,
            hit: BAR_HIT,
            pos_l: BAR_LISTEN,
            pos_r: BAR_LISTEN,
        };
        self.tine.reset_voice(v, &tine);
        self.bar.reset_voice(v, &bar);
        let rate = self.rate;
        self.notes += 1;
        let seed = SEED ^ self.notes.wrapping_mul(0x9E37_79B9_7F4A_7C15);
        let tau = s.noise_decay / (1000f64).ln();
        let damper_attack = 1.0 - s.damper_att_rel.max(0.0);
        let damper_cutoff = 500.0 * 24f64.powf(s.damper_tone);
        let voice = &mut self.voices[v];
        voice.age = 0;
        voice.released = None;
        voice.width = width;
        voice.amp = (force * s.tine_decay * rate / IMPULSE_SCALE) as f32;
        voice.tine = tine;
        voice.bar = bar;
        voice.tine_gain = gain(s.tine_key);
        voice.drive = (f64::from(COUPLING) * s.tone_decay / s.tine_decay).min(100.0) as f32;
        voice.level = velocity as f32;
        voice.out_key = gain(s.output_key);
        voice.band = Band::at(s.noise_pitch, rate);
        voice.hammer_gain = (s.noise * f64::from(NOISE_SCALE)) as f32 * gain(s.noise_key);
        voice.hammer = Burst {
            level: 1.0,
            fall: (-1.0 / (tau * rate)).exp() as f32,
            left: if s.noise > 0.0 {
                (s.noise_decay * 1.34 * rate) as usize
            } else {
                0
            },
        };
        voice.damper_coefficient = (1.0 - (-TAU * damper_cutoff / rate).exp()) as f32;
        voice.damper[0] = Burst {
            level: (s.damper_level * damper_attack) as f32,
            fall: (-1.0 / (DAMPER_TAU * rate)).exp() as f32,
            left: (DAMPER_SECONDS * rate) as usize,
        };
        voice.damper[1].left = 0;
        voice.rng = Random::seeded(seed);
        voice.damper_rng = Random::seeded(!seed);
        voice.field = Field::at_rest(&geometry);
        voice.reference =
            (1.0 / (TAU * f64::from(hz.max(20.0)) / (rate * f64::from(1u32 << STAGES)))) as f32;
    }

    /// The key is let go on `v`: the fork falls within Release, and the
    /// damper lands.
    fn release(&mut self, v: usize) {
        let s = self.settings;
        let rate = self.rate;
        let voice = &mut self.voices[v];
        voice.released = Some(0);
        // The damper only shortens a decay, so a tine that already decays
        // faster than Release keeps its own.
        voice.tine.decay = voice.tine.decay.min(s.release as f32);
        voice.bar.decay = voice.bar.decay.min(s.release as f32);
        self.tine.set_params(v, &voice.tine);
        self.bar.set_params(v, &voice.bar);
        let tail = (s.release * RELEASE_TAIL * rate) as usize;
        voice.gone = tail;
        voice.end = tail.max((DAMPER_SECONDS * rate) as usize) + END_PADDING;
        let damper_release = 1.0 + s.damper_att_rel.min(0.0);
        voice.damper[1] = Burst {
            level: (s.damper_level * damper_release) as f32,
            fall: (-1.0 / (DAMPER_TAU * rate)).exp() as f32,
            left: (DAMPER_SECONDS * rate) as usize,
        };
    }

    /// Plays frames `at..at + n` of the stretch's `frames`: at most up to
    /// the next grid point, so a render is the same at any block size.
    fn chunk(
        &mut self,
        voices: &VoiceBlock,
        state: &State,
        frames: &Range<usize>,
        at: usize,
        n: usize,
    ) {
        let stride = self.stride;
        let mask = voices.active_mask();
        if state.clock(at).is_multiple_of(CHUNK as u64) {
            let target = Geometry::of(&self.settings);
            self.geometry.toward(&target);
            let mut bits = mask;
            while bits != 0 {
                let v = bits.trailing_zeros() as usize;
                bits &= bits - 1;
                let pitch = state.pitch(voices, v, frames, at - frames.start);
                let hz = hz_of(pitch + self.settings.detune / 100.0);
                let voice = &mut self.voices[v];
                voice.tine.hz = hz;
                voice.bar.hz = hz;
                self.tine.set_params(v, &voice.tine);
                self.bar.set_params(v, &voice.bar);
            }
        }
        // The hammer's pulse excites the tine; the tine drives the bar.
        self.exc[..n * stride].fill(0.0);
        let mut bits = mask;
        while bits != 0 {
            let v = bits.trailing_zeros() as usize;
            bits &= bits - 1;
            let voice = &self.voices[v];
            for f in 0..n.min(voice.width.saturating_sub(voice.age)) {
                self.exc[f * stride + v] = voice.amp * pulse(voice.age + f, voice.width);
            }
        }
        self.tine.process(
            &self.exc[..n * stride],
            &mut self.tine_out[..n * stride * 2],
        );
        for f in 0..n {
            for (v, voice) in self.voices.iter().enumerate() {
                self.bar_in[f * stride + v] = voice.drive * self.tine_out[f * stride * 2 + v];
            }
        }
        self.bar.process(
            &self.bar_in[..n * stride],
            &mut self.bar_out[..n * stride * 2],
        );
        let pickup = Pickup::new(self.settings.electrostatic, &self.geometry);
        let mut mix = [0.0f32; CHUNK];
        let mut bits = mask;
        while bits != 0 {
            let v = bits.trailing_zeros() as usize;
            bits &= bits - 1;
            let voice = &mut self.voices[v];
            let mut fork = [0.0f32; CHUNK];
            let mut damper = [0.0f32; CHUNK];
            for f in 0..n {
                let l = f * stride * 2 + v;
                let mut x = self.tine_out[l] * voice.tine_gain + self.bar_out[l];
                if let Some(r) = voice.released {
                    x *= fade(r + f, voice.gone);
                }
                if voice.hammer.left > 0 {
                    let level = voice.hammer.next() * voice.hammer_gain;
                    x += voice.band.step(voice.rng.next_bipolar() as f32) * level;
                }
                fork[f] = x;
                // shortcut: the damper's noise skips the oversampler, so it
                // sounds the oversampler's latency (1 ms) before the fork;
                // upgrade when a delayed copy is worth a delay line a voice.
                damper[f] = damper_noise(voice);
            }
            let (field, reference, level) = (&mut voice.field, voice.reference, voice.level);
            self.oversampler.process(v, &mut fork[..n], |high| {
                for sample in high {
                    *sample = pickup.step(field, reference, *sample);
                }
            });
            let gain = level * self.geometry.output * voice.out_key;
            for f in 0..n {
                mix[f] += fork[f] * gain + damper[f] * level;
            }
            voice.age += n;
            if let Some(r) = &mut voice.released {
                *r += n;
            }
        }
        for (out, mixed) in self.out[at..at + n].iter_mut().zip(mix) {
            let x = mixed * self.geometry.volume;
            let y = x - self.dc[0] + self.dc_pole * self.dc[1];
            self.dc = [x, y];
            *out = y;
        }
    }
}

/// Spec: electric R5. The fork's gate `since` frames after the key was let
/// go: whole until nine tenths of `gone`, then falling to nothing by it.
fn fade(since: usize, gone: usize) -> f32 {
    let start = gone as f32 * 0.9;
    ((gone as f32 - since as f32) / (gone as f32 - start).max(1.0)).clamp(0.0, 1.0)
}

/// Spec: electric R9. The next frame of the damper's noise: one noise
/// source, softened by Tone, under the bursts that are still sounding.
fn damper_noise(voice: &mut Voice) -> f32 {
    if voice.damper[0].left == 0 && voice.damper[1].left == 0 {
        return 0.0;
    }
    let noise = voice.damper_rng.next_bipolar() as f32;
    voice.damper_lp += voice.damper_coefficient * (noise - voice.damper_lp);
    let mut level = 0.0;
    for burst in &mut voice.damper {
        if burst.left > 0 {
            level += burst.next();
        }
    }
    voice.damper_lp * level
}

/// A pitch in semitones as a twelve-step key counts them, in hertz.
fn hz_of(pitch: f64) -> f32 {
    (440.0 * ((pitch - 69.0) / 12.0).exp2()) as f32
}

impl Instrument for Fork {
    fn voice_event(&mut self, voices: &mut VoiceBlock, state: &State, event: VoiceEvent) {
        let v = event.voice;
        match event.kind {
            VoiceEventKind::Start { .. } => self.start(voices, state, v, event.time as usize),
            VoiceEventKind::Release => self.release(v),
            VoiceEventKind::End { .. } => {}
        }
    }

    fn render(&mut self, voices: &mut VoiceBlock, state: &State, frames: Range<usize>) {
        let mut at = frames.start;
        while at < frames.end {
            let phase = (state.clock(at) % CHUNK as u64) as usize;
            let n = (CHUNK - phase).min(frames.end - at);
            self.chunk(voices, state, &frames, at, n);
            at += n;
        }
        let mut bits = voices.active_mask();
        while bits != 0 {
            let v = bits.trailing_zeros() as usize;
            bits &= bits - 1;
            if self.voices[v]
                .released
                .is_some_and(|r| r >= self.voices[v].end)
            {
                voices.end(v);
            }
        }
    }
}

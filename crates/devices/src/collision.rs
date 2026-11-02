//! Collision (WP-8.21; spec: collision), the native mallet instrument: a
//! mallet and a noise source excite two resonators, each one of the physical
//! models (`bl_dsp::physical`), linked in series or in parallel, with two
//! LFOs and the MIDI/MPE sources routed through the instruments' modulation
//! matrix as chosen slots.
//!
//! Voices, per-note bends and the matrix are the voice section's
//! ([`crate::instrument`]). Everything Collision adds is here: the exciters,
//! the resonators' tuning and mixers, Structure, the LFOs and the Key and Vel
//! amounts. The ranges and defaults the manual doesn't give are Beatlayer's
//! own and are listed in the spec.

use core::f64::consts::{PI, TAU};
use std::ops::Range as FrameRange;

use bl_dsp::physical::{
    Kind as GuideKind, ModalBank, ModalParams, Quality, Shape, WaveguideBank, WaveguideParams,
};
use bl_dsp::random::Random;
use bl_engine::{DeviceType, NodeConfig, NodeData, NodeEvent, Registry};
use bl_model::device_layout::{
    BODY_HEIGHT, ControlBand, LayoutNode, Page, PageStyle, PagesNode, TAB_HEIGHT, Widget,
};
use bl_model::{
    Device, DeviceDescription, DeviceLayout, DeviceRole, ParamDescriptor, ParamId, ParamUnit,
};
use lspk_dsp::envelope::{EnvelopeBank, EnvelopeParams, StealRestart, TimeUnit, Timing, Trigger};
use lspk_voice::{CONTROL_INTERVAL, MAX_VOICES, Start, VoiceBlock, VoiceEvent, VoiceEventKind};

use crate::framework::{Audio, NativeDevice, ParamBank, add};
use crate::instrument::matrix::{
    DestDef, Kind, Matrix, MatrixLayout, Pick, Range, Rate, Scope, SlotDef, SourceDef,
};
use crate::instrument::{self, Instrument, Parts, Section, Sources, State};

/// Collision's device type.
pub const COLLISION: &str = "beatlayer.collision";

// ---- Parameter numbers --------------------------------------------------

/// The Mallet section (R2).
pub const MALLET_ON: ParamId = ParamId(0);
pub const MALLET_VOLUME: ParamId = ParamId(1);
pub const MALLET_STIFFNESS: ParamId = ParamId(2);
pub const MALLET_NOISE: ParamId = ParamId(3);
pub const MALLET_COLOR: ParamId = ParamId(4);

/// The Noise section (R3).
pub const NOISE_ON: ParamId = ParamId(5);
pub const NOISE_FILTER: ParamId = ParamId(6);
pub const NOISE_FREQ: ParamId = ParamId(7);
pub const NOISE_RES: ParamId = ParamId(8);
pub const NOISE_ENV_AMT: ParamId = ParamId(9);
pub const NOISE_VOLUME: ParamId = ParamId(10);
pub const NOISE_ATTACK: ParamId = ParamId(11);
pub const NOISE_DECAY: ParamId = ParamId(12);
pub const NOISE_SUSTAIN: ParamId = ParamId(13);
pub const NOISE_RELEASE: ParamId = ParamId(14);

/// A resonator's controls, as offsets from [`res`] (R5 to R9).
pub const R_ON: usize = 0;
pub const R_TYPE: usize = 1;
pub const R_QUALITY: usize = 2;
pub const R_DECAY: usize = 3;
pub const R_MATERIAL: usize = 4;
pub const R_RADIUS: usize = 5;
pub const R_RATIO: usize = 6;
pub const R_BRIGHTNESS: usize = 7;
pub const R_INHARM: usize = 8;
pub const R_OPENING: usize = 9;
pub const R_HIT: usize = 10;
pub const R_RND: usize = 11;
pub const R_NOTE_OFF: usize = 12;
pub const R_POS_L: usize = 13;
pub const R_POS_R: usize = 14;
pub const R_TUNE: usize = 15;
pub const R_FINE: usize = 16;
pub const R_KEY: usize = 17;
pub const R_PITCH_ENV: usize = 18;
pub const R_PITCH_TIME: usize = 19;
pub const R_GAIN: usize = 20;
pub const R_PAN: usize = 21;
pub const R_BLEED: usize = 22;
/// How many parameters a resonator has: the `R_` numbers are below it.
pub const RES_COUNT: usize = 23;
const RES_BASE: usize = 15;

/// Resonator `r`'s (0 or 1) control at `offset` (one of the `R_` numbers).
#[must_use]
pub const fn res(r: usize, offset: usize) -> ParamId {
    ParamId((RES_BASE + r * RES_COUNT + offset) as u32)
}

/// An LFO's controls, as offsets from [`lfo`] (R12).
pub const L_ON: usize = 0;
pub const L_SHAPE: usize = 1;
pub const L_RATE: usize = 2;
pub const L_SYNC: usize = 3;
pub const L_DIVISION: usize = 4;
pub const L_OFFSET: usize = 5;
pub const L_RETRIGGER: usize = 6;
pub const L_AMOUNT: usize = 7;
pub const L_DEST_A: usize = 8;
pub const L_AMOUNT_A: usize = 9;
pub const L_DEST_B: usize = 10;
pub const L_AMOUNT_B: usize = 11;
const LFO_COUNT: usize = 12;
const LFO_BASE: usize = RES_BASE + 2 * RES_COUNT;

/// LFO `i`'s (0 or 1) control at `offset` (one of the `L_` numbers).
#[must_use]
pub const fn lfo(i: usize, offset: usize) -> ParamId {
    ParamId((LFO_BASE + i * LFO_COUNT + offset) as u32)
}

/// The MIDI/MPE sources, as numbers for [`midi`] (R13).
pub const PITCH_BEND: usize = 0;
pub const MOD_WHEEL: usize = 1;
pub const PRESSURE: usize = 2;
pub const SLIDE: usize = 3;
const MIDI_BASE: usize = LFO_BASE + 2 * LFO_COUNT;

/// The MIDI/MPE source `which`'s control at `offset`: 0 and 2 are its two
/// destinations, 1 and 3 their amounts.
#[must_use]
pub const fn midi(which: usize, offset: usize) -> ParamId {
    ParamId((MIDI_BASE + which * 4 + offset) as u32)
}

/// The controls the key and velocity move (R16), each with a Key amount and
/// a Vel amount after them.
const KEYED: [ParamId; 12] = [
    MALLET_VOLUME,
    MALLET_STIFFNESS,
    MALLET_NOISE,
    NOISE_FREQ,
    res(0, R_PAN),
    res(1, R_PAN),
    res(0, R_PITCH_ENV),
    res(1, R_PITCH_ENV),
    lfo(0, L_RATE),
    lfo(1, L_RATE),
    lfo(0, L_AMOUNT),
    lfo(1, L_AMOUNT),
];
const KEYVEL_BASE: usize = MIDI_BASE + 16;

/// The Key amount of the `n`th of [`KEYED`]; the Vel amount is the next.
const fn key_of(n: usize) -> ParamId {
    ParamId((KEYVEL_BASE + 2 * n) as u32)
}

/// The global section (R15).
pub const VOICES: ParamId = ParamId((KEYVEL_BASE + 24) as u32);
pub const RETRIGGER: ParamId = ParamId((KEYVEL_BASE + 25) as u32);
pub const STRUCTURE: ParamId = ParamId((KEYVEL_BASE + 26) as u32);
pub const VOLUME: ParamId = ParamId((KEYVEL_BASE + 27) as u32);
const PARAMS: usize = KEYVEL_BASE + 28;

pub const RESONATOR_TYPES: [&str; 7] = [
    "Beam", "Marimba", "String", "Membrane", "Plate", "Pipe", "Tube",
];
const QUALITIES: [&str; 4] = ["Eco", "Low", "Medium", "High"];
pub const NOISE_FILTERS: [&str; 4] = ["LP", "HP", "BP", "LP+HP"];
pub const LFO_SHAPES: [&str; 7] = [
    "Sine", "Square", "Triangle", "Saw Up", "Saw Down", "Noise 1", "Noise 2",
];
pub const DIVISIONS: [&str; 10] = [
    "1/64", "1/32", "1/16", "1/8", "1/4", "1/2", "1 Bar", "2 Bars", "4 Bars", "8 Bars",
];
/// Each division's length in sixteenth notes.
const DIVISION_SIXTEENTHS: [f64; 10] = [0.25, 0.5, 1.0, 2.0, 4.0, 8.0, 16.0, 32.0, 64.0, 128.0];
const STRUCTURES: [&str; 2] = ["Serial 1 > 2", "Parallel 1 + 2"];

const TYPE_BEAM: usize = 0;
const TYPE_MARIMBA: usize = 1;
const TYPE_STRING: usize = 2;
const TYPE_MEMBRANE: usize = 3;
const TYPE_PLATE: usize = 4;
const TYPE_PIPE: usize = 5;
const TYPE_TUBE: usize = 6;
const SERIAL: usize = 0;

const SHAPE_SQUARE: usize = 1;
const SHAPE_TRIANGLE: usize = 2;
const SHAPE_SAW_UP: usize = 3;
const SHAPE_SAW_DOWN: usize = 4;
const SHAPE_STEPPED: usize = 5;

const FILTER_LP: usize = 0;
const FILTER_HP: usize = 1;
const FILTER_BP: usize = 2;

/// The matrix's sources (R12, R13): the two LFOs, then the MIDI/MPE four.
const SRC_LFO1: usize = 0;
const SRC_LFO2: usize = 1;
const SRC_BEND: usize = 2;
const SRC_WHEEL: usize = 3;
const SRC_PRESSURE: usize = 4;
const SRC_SLIDE: usize = 5;
const SOURCE_NAMES: [&str; 6] = [
    "LFO 1",
    "LFO 2",
    "Pitch Bend",
    "Mod Wheel",
    "Pressure",
    "Slide",
];

/// Spec: collision R2. A mallet's pulse is 20 ms wide at Stiffness 0 % and
/// 0.1 ms at 100 %, in between at an even ratio.
const WIDEST_PULSE: f64 = 20e-3;
const NARROWEST_PULSE: f64 = 0.1e-3;
/// Spec: collision R3. The noise filter's envelope reaches six octaves.
const ENV_OCTAVES: f64 = 6.0;
/// Spec: collision R7. Note Off's reach, and the time it takes (the
/// resonators' own smoothing).
const NOTE_OFF_REACH: f64 = 0.99;
/// Spec: collision R8. A resonator's pitch is held between these.
const LOWEST_HZ: f64 = 20.0;
/// How long a released voice stays below [`SILENT`] before it ends, in
/// seconds.
const QUIET_SECONDS: f64 = 0.1;
const SILENT: f32 = 1e-6;
/// The excitation a resonator needs for a strike or a blown noise to sound at
/// a level that doesn't depend on its decay and pitch. The resonators scale
/// their input by their pole's distance from the unit circle (physical
/// models R13), so a struck bar gives its impulse back at a level that grows
/// with its decay and falls with its pitch, and a blown one its noise. These
/// bring them to a peak of about 0.1 and a noise level of about 0.03 at C3;
/// they are Beatlayer's own calibration, measured on the models.
const STRIKE_LEVEL: f64 = 0.5;
const BLOW_MODAL: f64 = 4.0;
const BLOW_GUIDE: f64 = 1.4;
/// The pitch the calibration is made at.
const C3_HZ: f64 = 261.63;

// ---- The device type ----------------------------------------------------

/// The controls' descriptors, in parameter order. The destination choosers
/// list Off, then every destination by its parameter's name (R14), which
/// takes a first pass to know.
fn descriptors() -> Vec<ParamDescriptor> {
    let first = build(&["Off"]);
    let mut choices = vec!["Off"];
    choices.extend(
        destination_ids()
            .iter()
            .map(|&(id, _, _)| first[id.0 as usize].name.as_str()),
    );
    build(&choices)
}

fn build(choices: &[&str]) -> Vec<ParamDescriptor> {
    let percent = |id, name: &str, default: f64| {
        ParamDescriptor::continuous(id, name, (0.0, 100.0), default, ParamUnit::Percent)
    };
    let bipolar = |id, name: &str, default: f64| {
        ParamDescriptor::continuous(id, name, (-100.0, 100.0), default, ParamUnit::Percent)
    };
    let time = |id, name: &str, max_ms: f64, default: f64| {
        ParamDescriptor::continuous(id, name, (0.0, max_ms), default, ParamUnit::Milliseconds)
            .power(3)
    };
    let mut params = vec![
        ParamDescriptor::toggle(MALLET_ON, "Mallet", true).describe("Strikes the resonators once for each note."),
        ParamDescriptor::gain(MALLET_VOLUME, "Mallet Volume", 0.0, -6.0).describe("The strike's level."),
        percent(MALLET_STIFFNESS, "Mallet Stiffness", 50.0)
            .describe("A stiffer mallet strikes for a shorter time, so it sounds brighter."),
        percent(MALLET_NOISE, "Mallet Noise", 0.0).describe("The level of the noise in the strike."),
        ParamDescriptor::continuous(MALLET_COLOR, "Mallet Color", (200.0, 16_000.0), 4_000.0, ParamUnit::Hertz)
            .logarithmic()
            .describe("The centre frequency of the strike's noise; it does nothing with Mallet Noise at 0."),
        ParamDescriptor::toggle(NOISE_ON, "Noise", false)
            .describe("Excites the resonators with filtered noise for as long as its envelope lasts."),
        ParamDescriptor::choice(NOISE_FILTER, "Noise Filter", &NOISE_FILTERS, FILTER_LP)
            .describe("The noise's filter: low-pass, high-pass, band-pass, or a band between a high-pass and a low-pass."),
        ParamDescriptor::continuous(NOISE_FREQ, "Noise Freq", (20.0, 20_000.0), 2_000.0, ParamUnit::Hertz)
            .logarithmic()
            .describe("The noise filter's frequency."),
        percent(NOISE_RES, "Noise Res", 0.0)
            .describe("The noise filter's resonance, or the width of the LP+HP band."),
        bipolar(NOISE_ENV_AMT, "Noise Env Amt", 0.0)
            .describe("How far the noise envelope moves the filter's frequency, up to six octaves."),
        ParamDescriptor::gain(NOISE_VOLUME, "Noise Volume", 0.0, -6.0).describe("The noise's level."),
        time(NOISE_ATTACK, "Noise Attack", 20_000.0, 0.0).describe("How long the noise takes to reach full level."),
        time(NOISE_DECAY, "Noise Decay", 20_000.0, 300.0)
            .describe("How long the noise takes to fall to its sustain level."),
        percent(NOISE_SUSTAIN, "Noise Sustain", 0.0)
            .describe("The level the noise holds while a note lasts; 0 removes the sustain."),
        time(NOISE_RELEASE, "Noise Release", 20_000.0, 50.0)
            .describe("How long the noise takes to fall to silence after a note ends."),
    ];
    for r in 0..2 {
        let n = r + 1;
        let name = |what: &str| format!("Res {n} {what}");
        let on = r == 0;
        params.extend([
            ParamDescriptor::toggle(res(r, R_ON), &name("On"), on)
                .describe("Turns the resonator on; with both off the device is silent."),
            ParamDescriptor::choice(res(r, R_TYPE), &name("Type"), &RESONATOR_TYPES, TYPE_BEAM)
                .describe("The object that rings: a beam, a marimba bar, a string, a membrane, a plate, a pipe or a tube."),
            ParamDescriptor::choice(res(r, R_QUALITY), &name("Quality"), &QUALITIES, 2)
                .describe("How many overtones the resonator plays, trading processing for sound; it does nothing for a string, pipe or tube."),
            ParamDescriptor::continuous(res(r, R_DECAY), &name("Decay"), (0.001, 60.0), 1.0, ParamUnit::Seconds)
                .logarithmic()
                .describe("How long the resonator rings."),
            percent(res(r, R_MATERIAL), &name("Material"), 50.0)
                .describe("How fast the high partials die against the low ones: low for wood or rubber, high for glass or metal."),
            percent(res(r, R_RADIUS), &name("Radius"), 0.0)
                .describe("A pipe's or tube's radius: larger lengthens the decay and keeps more high harmonics."),
            ParamDescriptor::continuous(res(r, R_RATIO), &name("Ratio"), (0.25, 4.0), 1.0, ParamUnit::Ratio)
                .logarithmic()
                .describe("A membrane's or plate's size along one axis against the other."),
            bipolar(res(r, R_BRIGHTNESS), &name("Brightness"), 0.0)
                .describe("Lifts or cuts the upper partials."),
            bipolar(res(r, R_INHARM), &name("Inharm"), 0.0)
                .describe("Negative values bring the partials down, positive values push them up."),
            percent(res(r, R_OPENING), &name("Opening"), 100.0)
                .describe("A pipe's far end, from closed to open."),
            percent(res(r, R_HIT), &name("Hit"), 0.0)
                .describe("Where the resonator is struck, from the centre to the edge."),
            percent(res(r, R_RND), &name("Rnd"), 0.0)
                .describe("How far each note moves the strike from Hit, at random."),
            percent(res(r, R_NOTE_OFF), &name("Note Off"), 0.0)
                .describe("How far a note's end mutes the resonator: 0 % ignores it, 100 % mutes at once."),
            percent(res(r, R_POS_L), &name("Pos. L"), 0.0)
                .describe("Where the left channel listens, from the centre to the edge."),
            percent(res(r, R_POS_R), &name("Pos. R"), 0.0)
                .describe("Where the right channel listens, from the centre to the edge."),
            ParamDescriptor::integer(res(r, R_TUNE), &name("Tune"), (-48, 48), 0, ParamUnit::Semitones)
                .decimals(0)
                .describe("Transposes the resonator in semitones."),
            ParamDescriptor::continuous(res(r, R_FINE), &name("Fine"), (-50.0, 50.0), 0.0, ParamUnit::Cents)
                .describe("Tunes the resonator in cents."),
            ParamDescriptor::continuous(res(r, R_KEY), &name("Key"), (-100.0, 200.0), 100.0, ParamUnit::Percent)
                .describe("How far the played key moves the pitch: 100 % is equal temperament, 200 % doubles the intervals and a negative value reverses them."),
            ParamDescriptor::continuous(res(r, R_PITCH_ENV), &name("Pitch Env"), (-48.0, 48.0), 0.0, ParamUnit::Semitones)
                .describe("The pitch a note starts away from its own, reaching it over Pitch Time."),
            time(res(r, R_PITCH_TIME), &name("Pitch Time"), 10_000.0, 100.0)
                .describe("How long the pitch takes to reach the note."),
            ParamDescriptor::gain(res(r, R_GAIN), &name("Gain"), 12.0, 0.0).describe("The resonator's level."),
            bipolar(res(r, R_PAN), &name("Pan"), 0.0).describe("Places the resonator between left and right."),
            percent(res(r, R_BLEED), &name("Bleed"), 0.0)
                .describe("How much of the exciters' signal is mixed into the resonator's output."),
        ]);
    }
    for i in 0..2 {
        let n = i + 1;
        let name = |what: &str| format!("LFO {n} {what}");
        params.extend([
            ParamDescriptor::toggle(lfo(i, L_ON), &format!("LFO {n}"), false).describe("Turns the LFO on."),
            ParamDescriptor::choice(lfo(i, L_SHAPE), &name("Shape"), &LFO_SHAPES, 0).describe("The LFO's waveform."),
            ParamDescriptor::continuous(lfo(i, L_RATE), &name("Rate"), (0.01, 40.0), 1.0, ParamUnit::Hertz)
                .logarithmic()
                .describe("The LFO's rate in hertz."),
            ParamDescriptor::toggle(lfo(i, L_SYNC), &name("Sync"), false)
                .describe("Plays the LFO in divisions of the song's tempo instead of hertz."),
            ParamDescriptor::choice(lfo(i, L_DIVISION), &name("Division"), &DIVISIONS, 4)
                .describe("The LFO's cycle as a division of the tempo, with Sync on."),
            ParamDescriptor::continuous(lfo(i, L_OFFSET), &name("Offset"), (0.0, 360.0), 0.0, ParamUnit::None)
                .decimals(0)
                .describe("Where in its cycle the LFO starts, in degrees."),
            ParamDescriptor::toggle(lfo(i, L_RETRIGGER), &name("Retrigger"), true)
                .describe("Restarts the LFO with every note; off runs it freely."),
            percent(lfo(i, L_AMOUNT), &name("Amount"), 100.0).describe("The LFO's overall intensity."),
            ParamDescriptor::choice(lfo(i, L_DEST_A), &name("Dest A"), choices, 0)
                .describe("The first control the LFO moves."),
            bipolar(lfo(i, L_AMOUNT_A), &name("Amount A"), 0.0)
                .describe("How far the LFO moves its first destination, in per cent of its travel."),
            ParamDescriptor::choice(lfo(i, L_DEST_B), &name("Dest B"), choices, 0)
                .describe("The second control the LFO moves."),
            bipolar(lfo(i, L_AMOUNT_B), &name("Amount B"), 0.0)
                .describe("How far the LFO moves its second destination, in per cent of its travel."),
        ]);
    }
    for (which, source) in ["Pitch Bend", "Mod Wheel", "Pressure", "Slide"].into_iter().enumerate() {
        params.extend([
            ParamDescriptor::choice(midi(which, 0), &format!("{source} Dest A"), choices, 0)
                .describe("The first control this source moves."),
            bipolar(midi(which, 1), &format!("{source} Amount A"), 0.0)
                .describe("How far this source moves its first destination, in per cent of its travel."),
            ParamDescriptor::choice(midi(which, 2), &format!("{source} Dest B"), choices, 0)
                .describe("The second control this source moves."),
            bipolar(midi(which, 3), &format!("{source} Amount B"), 0.0)
                .describe("How far this source moves its second destination, in per cent of its travel."),
        ]);
    }
    for (n, id) in KEYED.iter().enumerate() {
        let name = params[id.0 as usize].name.clone();
        params.push(
            bipolar(key_of(n), &format!("{name} Key"), 0.0)
                .describe("How far the played key moves this control, in per cent of its travel per four octaves from C3."),
        );
        params.push(
            bipolar(ParamId(key_of(n).0 + 1), &format!("{name} Vel"), 0.0)
                .describe("How far the note's velocity moves this control, in per cent of its travel."),
        );
    }
    params.extend([
        instrument::voices(VOICES, MAX_VOICES as i64, 8),
        instrument::retrigger(RETRIGGER, true),
        ParamDescriptor::choice(STRUCTURE, "Structure", &STRUCTURES, SERIAL)
            .describe("Serial sends the exciters to Resonator 1 and its sound on to Resonator 2; Parallel sends the exciters to both."),
        ParamDescriptor::gain(VOLUME, "Volume", 0.0, -6.0).describe("The instrument's output level."),
    ]);
    debug_assert!(params.iter().enumerate().all(|(i, p)| p.id.0 as usize == i));
    debug_assert_eq!(params.len(), PARAMS);
    params
}

/// The destinations (R14), in the order the choosers list them after Off: the
/// parameter, its scope and the rate its routes are worked out at.
fn destination_ids() -> Vec<(ParamId, Scope, Rate)> {
    let control = |id| (id, Scope::Voice, Rate::Control);
    let audio = |id| (id, Scope::Voice, Rate::Audio);
    let mut ids = vec![
        control(MALLET_VOLUME),
        control(MALLET_STIFFNESS),
        control(MALLET_NOISE),
        control(MALLET_COLOR),
        control(NOISE_FREQ),
        control(NOISE_RES),
        control(NOISE_ENV_AMT),
        audio(NOISE_VOLUME),
    ];
    for r in 0..2 {
        ids.extend([
            control(res(r, R_DECAY)),
            control(res(r, R_MATERIAL)),
            control(res(r, R_RADIUS)),
            control(res(r, R_BRIGHTNESS)),
            // Inharmonicity belongs to a resonator's partial table, which its
            // voices share: it reads the sources of the oldest voice.
            (res(r, R_INHARM), Scope::Global, Rate::Control),
            control(res(r, R_OPENING)),
            control(res(r, R_HIT)),
            control(res(r, R_POS_L)),
            control(res(r, R_POS_R)),
            control(res(r, R_TUNE)),
            control(res(r, R_FINE)),
            audio(res(r, R_GAIN)),
            audio(res(r, R_PAN)),
            audio(res(r, R_BLEED)),
        ]);
    }
    ids.extend([
        control(lfo(0, L_RATE)),
        control(lfo(1, L_RATE)),
        audio(lfo(0, L_AMOUNT)),
        audio(lfo(1, L_AMOUNT)),
        audio(VOLUME),
    ]);
    ids
}

/// The value of a destination chooser that picks `param` (R14): 0 is Off, and
/// the destinations follow in the order [`collision_matrix`] lists them.
///
/// # Panics
///
/// If `param` isn't a destination.
#[must_use]
pub fn destination(param: ParamId) -> f64 {
    1.0 + destination_ids()
        .iter()
        .position(|d| d.0 == param)
        .expect("the parameter is a destination") as f64
}

/// The destinations' names, made once: a layout's names are `'static`, and the
/// layout is made again each time the shell lists the device types.
fn destination_names() -> &'static [&'static str] {
    static NAMES: std::sync::OnceLock<Vec<&'static str>> = std::sync::OnceLock::new();
    NAMES.get_or_init(|| {
        let descriptors = descriptors();
        destination_ids()
            .iter()
            .map(|&(id, _, _)| {
                &*Box::leak(descriptors[id.0 as usize].name.clone().into_boxed_str())
            })
            .collect()
    })
}

/// Collision's matrix (R12 to R14): the LFOs and the MIDI/MPE sources, each
/// with two chosen slots.
#[must_use]
pub fn collision_matrix() -> MatrixLayout {
    let source = |name, range, scope| SourceDef { name, range, scope };
    let mut slots = Vec::new();
    for (src, dests) in [
        (SRC_LFO1, [(lfo(0, L_DEST_A), lfo(0, L_AMOUNT_A)), (lfo(0, L_DEST_B), lfo(0, L_AMOUNT_B))]),
        (SRC_LFO2, [(lfo(1, L_DEST_A), lfo(1, L_AMOUNT_A)), (lfo(1, L_DEST_B), lfo(1, L_AMOUNT_B))]),
        (SRC_BEND, [(midi(PITCH_BEND, 0), midi(PITCH_BEND, 1)), (midi(PITCH_BEND, 2), midi(PITCH_BEND, 3))]),
        (SRC_WHEEL, [(midi(MOD_WHEEL, 0), midi(MOD_WHEEL, 1)), (midi(MOD_WHEEL, 2), midi(MOD_WHEEL, 3))]),
        (SRC_PRESSURE, [(midi(PRESSURE, 0), midi(PRESSURE, 1)), (midi(PRESSURE, 2), midi(PRESSURE, 3))]),
        (SRC_SLIDE, [(midi(SLIDE, 0), midi(SLIDE, 1)), (midi(SLIDE, 2), midi(SLIDE, 3))]),
    ] {
        for (dest, amount) in dests {
            slots.push(SlotDef {
                source: Pick::Fixed(src as u8),
                dest: Pick::Chosen(dest),
                amount,
            });
        }
    }
    MatrixLayout {
        sources: vec![
            source(SOURCE_NAMES[SRC_LFO1], Range::Bipolar, Scope::Voice),
            source(SOURCE_NAMES[SRC_LFO2], Range::Bipolar, Scope::Voice),
            source(SOURCE_NAMES[SRC_BEND], Range::Bipolar, Scope::Global),
            source(SOURCE_NAMES[SRC_WHEEL], Range::Unipolar, Scope::Global),
            source(SOURCE_NAMES[SRC_PRESSURE], Range::Unipolar, Scope::Voice),
            source(SOURCE_NAMES[SRC_SLIDE], Range::Unipolar, Scope::Voice),
        ],
        dests: destination_ids()
            .into_iter()
            .zip(destination_names())
            .map(|((id, scope, rate), &name)| DestDef {
                name,
                param: Some(id),
                scope,
                rate,
                kind: Kind::Additive,
            })
            .collect(),
        slots,
        forbidden: Vec::new(),
    }
}

/// Collision's device type.
#[must_use]
pub fn collision_type() -> DeviceType {
    let params = descriptors();
    let band = |height: f32, controls: &[(ParamId, Widget)]| {
        LayoutNode::Band(ControlBand::stacked(height, controls))
    };
    let knob = |id| (id, Widget::Knob);
    let toggle = |id| (id, Widget::Toggle);
    let chooser = |id| (id, Widget::Chooser);
    let number = |id| (id, Widget::Number);
    let body = BODY_HEIGHT - TAB_HEIGHT;
    let resonator = |r: usize| {
        let at = |offset| res(r, offset);
        LayoutNode::Row(vec![
            band(
                body,
                &[
                    toggle(at(R_ON)),
                    chooser(at(R_TYPE)),
                    chooser(at(R_QUALITY)),
                    knob(at(R_DECAY)),
                    knob(at(R_MATERIAL)),
                    knob(at(R_RADIUS)),
                    knob(at(R_RATIO)),
                    knob(at(R_BRIGHTNESS)),
                    knob(at(R_INHARM)),
                    knob(at(R_OPENING)),
                    knob(at(R_HIT)),
                    knob(at(R_RND)),
                    knob(at(R_NOTE_OFF)),
                    knob(at(R_POS_L)),
                    knob(at(R_POS_R)),
                ],
            ),
            band(
                body,
                &[
                    number(at(R_TUNE)),
                    knob(at(R_FINE)),
                    knob(at(R_KEY)),
                    knob(at(R_PITCH_ENV)),
                    knob(at(R_PITCH_TIME)),
                    knob(at(R_GAIN)),
                    knob(at(R_PAN)),
                    knob(at(R_BLEED)),
                ],
            ),
        ])
    };
    let lfo_page = LayoutNode::Row(
        (0..2)
            .map(|i| {
                let at = |offset| lfo(i, offset);
                band(
                    body,
                    &[
                        toggle(at(L_ON)),
                        chooser(at(L_SHAPE)),
                        knob(at(L_RATE)),
                        toggle(at(L_SYNC)),
                        chooser(at(L_DIVISION)),
                        knob(at(L_OFFSET)),
                        toggle(at(L_RETRIGGER)),
                        knob(at(L_AMOUNT)),
                        chooser(at(L_DEST_A)),
                        knob(at(L_AMOUNT_A)),
                        chooser(at(L_DEST_B)),
                        knob(at(L_AMOUNT_B)),
                    ],
                )
            })
            .collect(),
    );
    let mut midi_controls: Vec<(ParamId, Widget)> = Vec::new();
    for which in 0..4 {
        midi_controls.extend([
            chooser(midi(which, 0)),
            knob(midi(which, 1)),
            chooser(midi(which, 2)),
            knob(midi(which, 3)),
        ]);
    }
    let keyed: Vec<(ParamId, Widget)> = (0..KEYED.len())
        .flat_map(|n| [knob(key_of(n)), knob(ParamId(key_of(n).0 + 1))])
        .collect();
    let layout = DeviceLayout::new(LayoutNode::Row(vec![
        LayoutNode::Pages(PagesNode::new(
            "Tabs",
            PageStyle::Tabs,
            vec![
                Page::new(
                    "Mallet",
                    band(
                        body,
                        &[
                            toggle(MALLET_ON),
                            knob(MALLET_VOLUME),
                            knob(MALLET_STIFFNESS),
                            knob(MALLET_NOISE),
                            knob(MALLET_COLOR),
                        ],
                    ),
                ),
                Page::new(
                    "Noise",
                    band(
                        body,
                        &[
                            toggle(NOISE_ON),
                            chooser(NOISE_FILTER),
                            knob(NOISE_FREQ),
                            knob(NOISE_RES),
                            knob(NOISE_ENV_AMT),
                            knob(NOISE_VOLUME),
                            knob(NOISE_ATTACK),
                            knob(NOISE_DECAY),
                            knob(NOISE_SUSTAIN),
                            knob(NOISE_RELEASE),
                        ],
                    ),
                ),
                Page::new("Resonator 1", resonator(0)),
                Page::new("Resonator 2", resonator(1)),
                Page::new("LFO", lfo_page),
                Page::new(
                    "MIDI/MPE",
                    LayoutNode::Row(vec![band(body, &midi_controls), band(body, &keyed)]),
                ),
            ],
        )),
        band(
            BODY_HEIGHT,
            &[
                number(VOICES),
                toggle(RETRIGGER),
                chooser(STRUCTURE),
                knob(VOLUME),
            ],
        ),
    ]));
    let mut banks: Vec<(String, Vec<String>)> = Vec::new();
    let mut bank = |name: &str, ids: Vec<ParamId>| {
        for (n, chunk) in ids.chunks(8).enumerate() {
            let title = if ids.len() > 8 {
                format!("{name} {}", n + 1)
            } else {
                name.to_owned()
            };
            banks.push((title, chunk.iter().map(|id| format!("#{}", id.0)).collect()));
        }
    };
    bank("Mallet", (MALLET_ON.0..=MALLET_COLOR.0).map(ParamId).collect());
    bank("Noise", (NOISE_ON.0..=NOISE_RELEASE.0).map(ParamId).collect());
    for r in 0..2 {
        bank(
            &format!("Resonator {}", r + 1),
            (0..RES_COUNT).map(|o| res(r, o)).collect(),
        );
    }
    for i in 0..2 {
        bank(&format!("LFO {}", i + 1), (0..LFO_COUNT).map(|o| lfo(i, o)).collect());
    }
    bank("MIDI/MPE", (0..16).map(|o| ParamId((MIDI_BASE + o) as u32)).collect());
    bank("Key and Vel", (0..24).map(|o| ParamId((KEYVEL_BASE + o) as u32)).collect());
    bank("Global", vec![VOICES, RETRIGGER, STRUCTURE, VOLUME]);
    let banks: Vec<(&str, Vec<&str>)> = banks
        .iter()
        .map(|(name, ids)| (name.as_str(), ids.iter().map(String::as_str).collect()))
        .collect();
    let banks: Vec<(&str, &[&str])> = banks.iter().map(|(n, p)| (*n, p.as_slice())).collect();
    DeviceType::native(COLLISION, "Collision", DeviceRole::Instrument)
        .with_description(
            DeviceDescription::new(params)
                .with_layout(layout)
                .with_banks(&banks),
        )
        .with_matrix(collision_matrix())
}

/// Spec: collision R1. Collision's factory presets of Beatlayer's own: each
/// a name and the parameters it sets, the rest staying at their defaults.
#[must_use]
pub fn factory_presets() -> Vec<(&'static str, Vec<(ParamId, f64)>)> {
    let db = bl_model::param::db_to_gain;
    vec![
        ("Init", vec![]),
        (
            "Marimba",
            vec![
                (res(0, R_TYPE), TYPE_MARIMBA as f64),
                (res(0, R_DECAY), 1.4),
                (res(0, R_MATERIAL), 25.0),
                (res(0, R_HIT), 20.0),
                (MALLET_STIFFNESS, 60.0),
                (MALLET_NOISE, 15.0),
            ],
        ),
        (
            "Glass Bar",
            vec![
                (res(0, R_TYPE), TYPE_BEAM as f64),
                (res(0, R_DECAY), 6.0),
                (res(0, R_MATERIAL), 95.0),
                (res(0, R_INHARM), 20.0),
                (MALLET_STIFFNESS, 90.0),
                (res(1, R_ON), 1.0),
                (res(1, R_TYPE), TYPE_PLATE as f64),
                (res(1, R_TUNE), 12.0),
                (res(1, R_DECAY), 3.0),
                (res(1, R_GAIN), db(-9.0)),
                (STRUCTURE, 1.0),
            ],
        ),
        (
            "Blown Pipe",
            vec![
                (MALLET_ON, 0.0),
                (NOISE_ON, 1.0),
                (NOISE_FILTER, 2.0),
                (NOISE_FREQ, 900.0),
                (NOISE_RES, 40.0),
                (NOISE_ATTACK, 60.0),
                (NOISE_SUSTAIN, 80.0),
                (res(0, R_TYPE), TYPE_PIPE as f64),
                (res(0, R_DECAY), 0.8),
                (res(0, R_OPENING), 100.0),
                (res(0, R_RADIUS), 20.0),
            ],
        ),
        (
            "Skin and String",
            vec![
                (res(0, R_TYPE), TYPE_MEMBRANE as f64),
                (res(0, R_DECAY), 0.5),
                (res(0, R_MATERIAL), 20.0),
                (res(0, R_PITCH_ENV), 7.0),
                (res(0, R_PITCH_TIME), 60.0),
                (res(1, R_ON), 1.0),
                (res(1, R_TYPE), TYPE_STRING as f64),
                (res(1, R_DECAY), 2.5),
                (res(1, R_GAIN), db(-6.0)),
                (VOLUME, db(-6.0)),
            ],
        ),
        (
            "Wobbling Tube",
            vec![
                (res(0, R_TYPE), TYPE_TUBE as f64),
                (res(0, R_DECAY), 2.0),
                (lfo(0, L_ON), 1.0),
                (lfo(0, L_RATE), 4.0),
                (lfo(0, L_DEST_A), destination(res(0, R_FINE))),
                (lfo(0, L_AMOUNT_A), 12.0),
            ],
        ),
    ]
}

/// Adds Collision to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, collision_type(), |device, config| make(device, config));
}

/// Makes a Collision for `device` and `config`'s rate and block size: every
/// buffer and table it needs is made here, off the audio thread.
#[must_use]
pub fn make(device: &Device, config: &NodeConfig) -> Collision {
    Collision::new(crate::midi_fx::seed(device), config)
}

// ---- The device ---------------------------------------------------------

/// Collision: the voice section, and the instrument its voices play.
pub struct Collision {
    section: Section,
    engine: Engine,
}

impl Collision {
    fn new(seed: u64, config: &NodeConfig) -> Self {
        let rate = config.sample_rate.as_f64();
        Self {
            section: Section::new(
                Parts {
                    voices: Some(VOICES),
                    retrigger: Some(RETRIGGER),
                    note_range: instrument::NoteRange::Mpe,
                    ..Parts::default()
                },
                MAX_VOICES,
                1,
                rate,
                seed,
            ),
            engine: Engine::new(rate, config.max_frames, seed),
        }
    }
}

impl NativeDevice for Collision {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        let frames = audio.frames();
        if frames == 0 || audio.channels() == 0 {
            return;
        }
        self.engine.snapshot(audio, params);
        self.section.process(audio, params, &mut self.engine);
        for (c, channel) in audio.channels_mut().enumerate() {
            let from = if c == 1 { &self.engine.out_r } else { &self.engine.out_l };
            channel.copy_from_slice(&from[..frames]);
        }
    }

    /// Spec: instrument modulation R12; modulation R31. What the newest
    /// voice plays for each parameter the matrix moves.
    fn matrix_played(&self, report: &mut dyn FnMut(ParamId, f64)) {
        let matrix = &self.engine.matrix;
        for (dest, def) in matrix.layout().dests.iter().enumerate() {
            if let (Some(param), Some(position)) = (def.param, matrix.shown(dest)) {
                report(param, self.engine.descs[param.0 as usize].denormalize(position));
            }
        }
    }

    fn flush(&mut self, _events: &[NodeEvent], _params: &ParamBank) {
        self.section.reset();
        self.engine.silence();
    }

    fn receive_data(&mut self, data: NodeData) {
        self.section.receive_data(&data);
    }
}

/// What a resonator is set to for one voice at one frame.
#[derive(Clone, Copy)]
struct Setting {
    kind: usize,
    hz: f64,
    decay: f64,
    material: f64,
    brightness: f64,
    radius: f64,
    opening: f64,
    hit: f64,
    pos_l: f64,
    pos_r: f64,
}

/// A mallet's strike, set up when its note starts.
#[derive(Clone, Copy, Default)]
struct Strike {
    /// The pulse's width in frames, and its peak.
    width: f32,
    peak: f32,
    /// The noise burst's level and its band-pass's coefficients.
    noise: f32,
    bp: Svf,
}

/// A state-variable filter's state, and its coefficients when one is
/// running.
#[derive(Clone, Copy, Default)]
struct Svf {
    a1: f32,
    a2: f32,
    a3: f32,
    k: f32,
    ic1: f32,
    ic2: f32,
}

impl Svf {
    /// Coefficients for `hz` at quality `q` (a zero-delay-feedback design).
    fn tune(&mut self, hz: f64, q: f64, rate: f64) {
        let g = (PI * hz.clamp(10.0, rate * 0.45) / rate).tan();
        let k = 1.0 / q;
        let a1 = 1.0 / (1.0 + g * (g + k));
        self.a1 = a1 as f32;
        self.a2 = (g * a1) as f32;
        self.a3 = (g * g * a1) as f32;
        self.k = k as f32;
    }

    fn clear(&mut self) {
        self.ic1 = 0.0;
        self.ic2 = 0.0;
    }

    /// One sample: low-pass, band-pass and high-pass.
    fn run(&mut self, x: f32) -> (f32, f32, f32) {
        let v3 = x - self.ic2;
        let v1 = self.a1 * self.ic1 + self.a2 * v3;
        let v2 = self.ic2 + self.a2 * self.ic1 + self.a3 * v3;
        self.ic1 = 2.0 * v1 - self.ic1;
        self.ic2 = 2.0 * v2 - self.ic2;
        for state in [&mut self.ic1, &mut self.ic2] {
            if state.abs() < 1e-20 {
                *state = 0.0;
            }
        }
        (v2, v1, x - self.k * v1 - v2)
    }
}

/// A resonator's table, as the bank has it: rebuilt when one of these
/// changes.
#[derive(Clone, Copy, PartialEq)]
struct Table {
    shape: usize,
    quality: usize,
    ratio: f32,
    inharm: f32,
}

/// The Collision instrument: the exciters, the resonators and their banks,
/// the LFOs and the matrix.
struct Engine {
    rate: f64,
    max_frames: usize,
    stride: usize,
    bpm: f64,
    descs: Vec<ParamDescriptor>,
    /// Each parameter's destination in the matrix, and its place among the
    /// controls the key and velocity move.
    dest_of: Vec<Option<usize>>,
    keyed: Vec<Option<usize>>,
    /// Every parameter's value for each frame of the stretch.
    ramps: Vec<f32>,
    matrix: Matrix,
    modal: [ModalBank; 2],
    guide: [WaveguideBank; 2],
    guide_kind: [usize; 2],
    table: [Option<Table>; 2],
    // Per voice.
    fresh: u32,
    released: u32,
    age: [u32; MAX_VOICES],
    strike: [Strike; MAX_VOICES],
    hit_offset: [f64; MAX_VOICES],
    noise_rng: Vec<Random>,
    noise_lp: [Svf; MAX_VOICES],
    noise_hp: [Svf; MAX_VOICES],
    quiet: [u32; MAX_VOICES],
    /// How much of a strike and of the noise each resonator takes, per voice.
    strike_scale: [[f32; MAX_VOICES]; 2],
    blow_scale: [[f32; MAX_VOICES]; 2],
    env: EnvelopeBank,
    env_params: EnvelopeParams,
    lfos: Lfos,
    // Buffers, `frames × stride` unless said.
    env_out: Vec<f32>,
    mallet: Vec<f32>,
    blown: Vec<f32>,
    feed: [Vec<f32>; 2],
    rung: [Vec<f32>; 2],
    lfo_out: [Vec<f32>; 2],
    out_l: Vec<f32>,
    out_r: Vec<f32>,
}

/// The LFOs' state: per voice and one free-running, in the last place.
struct Lfos {
    phase: [[f64; MAX_VOICES + 1]; 2],
    held: [[f64; MAX_VOICES + 1]; 2],
    drawn: [[f64; MAX_VOICES + 1]; 2],
    last: [[f32; MAX_VOICES]; 2],
    rng: [Vec<Random>; 2],
}

impl Lfos {
    fn new(seed: u64) -> Self {
        let rng = |lfo: u64| {
            (0..=MAX_VOICES as u64)
                .map(|v| Random::seeded(seed ^ (lfo + 1).wrapping_mul(0xA5A5) ^ v.wrapping_mul(0x9E37_79B9)))
                .collect::<Vec<_>>()
        };
        let mut lfos = Self {
            phase: [[0.0; MAX_VOICES + 1]; 2],
            held: [[0.0; MAX_VOICES + 1]; 2],
            drawn: [[0.0; MAX_VOICES + 1]; 2],
            last: [[0.0; MAX_VOICES]; 2],
            rng: [rng(0), rng(1)],
        };
        for l in 0..2 {
            for v in 0..=MAX_VOICES {
                lfos.held[l][v] = lfos.rng[l][v].next_bipolar();
                lfos.drawn[l][v] = lfos.rng[l][v].next_bipolar();
            }
        }
        lfos
    }

    /// Moves slot `v` of LFO `l` on by `cycles` of its cycle.
    fn advance(&mut self, l: usize, v: usize, cycles: f64) {
        self.phase[l][v] += cycles;
        if self.phase[l][v] >= 1.0 {
            self.phase[l][v] -= self.phase[l][v].floor();
            self.held[l][v] = self.drawn[l][v];
            self.drawn[l][v] = self.rng[l][v].next_bipolar();
        }
    }

    /// Starts voice `v`'s cycle over, as a note does.
    fn restart(&mut self, v: usize) {
        for l in 0..2 {
            self.phase[l][v] = 0.0;
            self.held[l][v] = self.drawn[l][v];
            self.drawn[l][v] = self.rng[l][v].next_bipolar();
        }
    }
}

/// Spec: collision R2. The mallet's pulse width in frames at `stiffness`
/// (0 to 1): 20 ms at 0 and 0.1 ms at 1, in between at an even ratio, and
/// never less than a frame.
fn pulse_width(stiffness: f64, rate: f64) -> f64 {
    (WIDEST_PULSE * (NARROWEST_PULSE / WIDEST_PULSE).powf(stiffness) * rate).max(1.0)
}

/// A raised-cosine pulse of `width` frames at frame `t`: 0 at its ends and 1
/// in its middle.
fn raised_cosine(t: f32, width: f32) -> f32 {
    0.5 * (1.0 - (TAU as f32 * t / width).cos())
}

/// The LFO waveform `shape` at `phase` (0 to 1), from −1 to 1; the noise
/// shapes take the cycle's held and next random values.
fn lfo_wave(shape: usize, phase: f64, held: f64, drawn: f64) -> f64 {
    match shape {
        SHAPE_SQUARE => {
            if phase < 0.5 {
                1.0
            } else {
                -1.0
            }
        }
        SHAPE_TRIANGLE => 1.0 - 4.0 * ((phase + 0.25).fract() - 0.5).abs(),
        SHAPE_SAW_UP => 2.0 * phase - 1.0,
        SHAPE_SAW_DOWN => 1.0 - 2.0 * phase,
        SHAPE_STEPPED => held,
        6 => held + (drawn - held) * phase,
        _ => (TAU * phase).sin(),
    }
}

impl Engine {
    fn new(rate: f64, max_frames: usize, seed: u64) -> Self {
        let max_frames = max_frames.max(1);
        let stride = MAX_VOICES.next_multiple_of(16);
        let descs = descriptors();
        let layout = collision_matrix();
        let mut dest_of = vec![None; PARAMS];
        for (d, def) in layout.dests.iter().enumerate() {
            dest_of[def.param.expect("every destination is a parameter").0 as usize] = Some(d);
        }
        let mut keyed = vec![None; PARAMS];
        for (n, id) in KEYED.iter().enumerate() {
            keyed[id.0 as usize] = Some(n);
        }
        let buffer = || vec![0.0; max_frames * stride];
        let wide = || vec![0.0; max_frames * stride * 2];
        let sample_rate = rate as f32;
        Self {
            rate,
            max_frames,
            stride,
            bpm: 120.0,
            descs,
            dest_of,
            keyed,
            ramps: vec![0.0; PARAMS * max_frames],
            matrix: Matrix::new(std::sync::Arc::new(layout)),
            modal: [
                ModalBank::new(sample_rate, MAX_VOICES),
                ModalBank::new(sample_rate, MAX_VOICES),
            ],
            guide: [
                WaveguideBank::new(sample_rate, MAX_VOICES, GuideKind::String, LOWEST_HZ as f32),
                WaveguideBank::new(sample_rate, MAX_VOICES, GuideKind::String, LOWEST_HZ as f32),
            ],
            guide_kind: [TYPE_STRING; 2],
            table: [None; 2],
            fresh: 0,
            released: 0,
            age: [u32::MAX; MAX_VOICES],
            strike: [Strike::default(); MAX_VOICES],
            hit_offset: [0.0; MAX_VOICES],
            noise_rng: (0..MAX_VOICES as u64)
                .map(|v| Random::seeded(seed ^ 0x6E01_5E ^ (v + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15)))
                .collect(),
            noise_lp: [Svf::default(); MAX_VOICES],
            noise_hp: [Svf::default(); MAX_VOICES],
            quiet: [0; MAX_VOICES],
            strike_scale: [[0.0; MAX_VOICES]; 2],
            blow_scale: [[0.0; MAX_VOICES]; 2],
            env: EnvelopeBank::new(MAX_VOICES, EnvelopeParams::AMP, Timing::new(rate)),
            env_params: EnvelopeParams::AMP,
            lfos: Lfos::new(seed),
            env_out: buffer(),
            mallet: buffer(),
            blown: buffer(),
            feed: [buffer(), buffer()],
            rung: [wide(), wide()],
            lfo_out: [buffer(), buffer()],
            out_l: vec![0.0; max_frames],
            out_r: vec![0.0; max_frames],
        }
    }

    /// Drops everything the instrument holds: for a device switched off.
    fn silence(&mut self) {
        self.fresh = 0;
        self.released = 0;
        for voice in 0..MAX_VOICES {
            self.env.reset(voice);
            self.age[voice] = u32::MAX;
        }
        self.out_l.fill(0.0);
        self.out_r.fill(0.0);
    }

    /// Reads the stretch's parameters: every one's value frame by frame, the
    /// noise envelope's settings and the matrix's slots.
    fn snapshot(&mut self, audio: &Audio<'_, '_>, params: &ParamBank) {
        let frames = audio.frames();
        for i in 0..PARAMS {
            let index = params.index(ParamId(i as u32)).expect("the description holds it");
            self.ramps[i * self.max_frames..i * self.max_frames + frames]
                .copy_from_slice(params.ramp(index));
        }
        self.bpm = audio.bpm();
        let at = |id: ParamId| params.value(params.index(id).expect("the description holds it"));
        let env = EnvelopeParams {
            unit: TimeUnit::Milliseconds,
            delay: 0.0,
            attack: at(NOISE_ATTACK) as f32,
            hold: 0.0,
            decay: at(NOISE_DECAY) as f32,
            sustain: (at(NOISE_SUSTAIN) / 100.0) as f32,
            release: at(NOISE_RELEASE) as f32,
            attack_curve: 0.5,
            decay_curve: 0.666,
            release_curve: 0.666,
            start: 0.0,
            end: 0.0,
            legato_inverted: false,
            steal_restart: StealRestart::CurrentLevel,
        };
        if env != self.env_params {
            self.env_params = env;
            self.env.set_all_params(env);
        }
        self.matrix.begin(params);
        self.out_l[..frames].fill(0.0);
        self.out_r[..frames].fill(0.0);
    }

    /// Parameter `id`'s value at `frame` of the stretch, as automation
    /// leaves it.
    fn raw(&self, id: ParamId, frame: usize) -> f64 {
        f64::from(self.ramps[id.0 as usize * self.max_frames + frame])
    }

    /// What a source reads for `voice` at `frame`.
    fn source(&self, source: usize, voice: usize, frame: usize, voices: &VoiceBlock) -> f64 {
        match source {
            SRC_LFO1 => f64::from(self.lfo_out[0][frame * self.stride + voice]),
            SRC_LFO2 => f64::from(self.lfo_out[1][frame * self.stride + voice]),
            SRC_BEND => Sources::of(voices).pitch_bend(),
            SRC_WHEEL => Sources::of(voices).mod_wheel(),
            SRC_PRESSURE => Sources::of(voices).pressure(voice),
            SRC_SLIDE => Sources::of(voices).slide(voice),
            _ => 0.0,
        }
    }

    /// `voice`'s value of parameter `id` at `frame`: automation's, moved by
    /// its Key and Vel amounts (R16) and by every matrix route into it (R12
    /// to R14), as a share of its travel.
    fn val(&self, id: ParamId, voice: usize, frame: usize, voices: &VoiceBlock, due: bool) -> f64 {
        let base = self.raw(id, frame);
        let i = id.0 as usize;
        let route = self.dest_of[i].filter(|&d| self.matrix.has(d));
        if route.is_none() && self.keyed[i].is_none() {
            return base;
        }
        let desc = &self.descs[i];
        let mut position = desc.normalize(base);
        if let Some(n) = self.keyed[i] {
            let sources = Sources::of(voices);
            position += self.raw(key_of(n), frame) / 100.0 * sources.key(voice)
                + self.raw(ParamId(key_of(n).0 + 1), frame) / 100.0 * sources.velocity(voice);
        }
        let mut position = position.clamp(0.0, 1.0);
        if let Some(dest) = route {
            position = self.matrix.position(dest, position, voice, frame, due, &|source, voice, frame| {
                self.source(source, voice, frame, voices)
            });
        }
        desc.denormalize(position)
    }

    /// Resonator `r`'s settings for `voice` at `frame`.
    fn setting(&self, r: usize, voice: usize, frame: usize, voices: &VoiceBlock, state: &State, frames: &FrameRange<usize>) -> Setting {
        let v = |offset| self.val(res(r, offset), voice, frame, voices, true);
        let kind = (self.raw(res(r, R_TYPE), frame) as usize).min(RESONATOR_TYPES.len() - 1);
        // Spec: collision R8. The note's interval from C3 times Key, then
        // Tune, Fine and the pitch envelope's straight line in semitones.
        let key = (state.pitch(voices, voice, frames, frame - frames.start) - 60.0)
            * self.raw(res(r, R_KEY), frame)
            / 100.0;
        let time = f64::from(state.frames_since_start(voice, frame) as u32) / self.rate;
        let sweep = v(R_PITCH_TIME) / 1e3;
        let envelope = if time >= sweep {
            0.0
        } else {
            v(R_PITCH_ENV) * (1.0 - time / sweep)
        };
        let semitones = 60.0 + key + v(R_TUNE) + v(R_FINE) / 100.0 + envelope;
        let hz = 440.0 * ((semitones - 69.0) / 12.0).exp2();
        // Spec: collision R7. A note's end shortens the decay by Note Off.
        let mute = if self.released & (1 << voice) != 0 {
            1.0 - v(R_NOTE_OFF) / 100.0 * NOTE_OFF_REACH
        } else {
            1.0
        };
        // Spec: collision R6. Rnd moves each note's strike from Hit.
        let hit = (v(R_HIT) / 100.0
            + self.hit_offset[voice] * v(R_RND) / 100.0)
            .clamp(0.0, 1.0);
        Setting {
            kind,
            hz,
            decay: (v(R_DECAY) * mute).max(0.001),
            material: v(R_MATERIAL) / 100.0,
            brightness: v(R_BRIGHTNESS) / 100.0,
            radius: v(R_RADIUS) / 100.0,
            opening: v(R_OPENING) / 100.0,
            hit,
            pos_l: v(R_POS_L) / 100.0,
            pos_r: v(R_POS_R) / 100.0,
        }
    }

    /// Whether resonator `r`'s type is a waveguide.
    fn is_guide(kind: usize) -> bool {
        matches!(kind, TYPE_STRING | TYPE_PIPE | TYPE_TUBE)
    }

    /// Gives resonator `r`'s bank its table and kind for `frame`: a modal
    /// bank's partials are rebuilt in place when the shape, quality, ratio or
    /// inharmonicity changed.
    fn tune_banks(&mut self, r: usize, frame: usize, voices: &VoiceBlock) {
        let kind = (self.raw(res(r, R_TYPE), frame) as usize).min(RESONATOR_TYPES.len() - 1);
        if Self::is_guide(kind) {
            if kind != self.guide_kind[r] {
                self.guide_kind[r] = kind;
                self.guide[r].set_kind(match kind {
                    TYPE_PIPE => GuideKind::Pipe,
                    TYPE_TUBE => GuideKind::Tube,
                    _ => GuideKind::String,
                });
            }
            return;
        }
        // Inharm is global: it reads the oldest voice's sources.
        let inharm = (self.val(res(r, R_INHARM), 0, frame, voices, true) / 100.0) as f32;
        let wanted = Table {
            shape: kind,
            quality: (self.raw(res(r, R_QUALITY), frame) as usize).min(QUALITIES.len() - 1),
            ratio: self.raw(res(r, R_RATIO), frame) as f32,
            inharm,
        };
        if self.table[r] != Some(wanted) {
            self.table[r] = Some(wanted);
            let shape = match kind {
                TYPE_MARIMBA => Shape::Marimba,
                TYPE_MEMBRANE => Shape::Membrane,
                TYPE_PLATE => Shape::Plate,
                _ => Shape::Beam,
            };
            let quality = [Quality::Eco, Quality::Low, Quality::Medium, Quality::High][wanted.quality];
            self.modal[r].rebuild_table(shape, quality, wanted.ratio, wanted.inharm);
        }
    }

    /// Gives `voice`'s resonator `r` its settings (or, for a note that
    /// starts, sets it there with nothing ringing), and works out how much
    /// of the strike and the noise it takes.
    fn apply(&mut self, r: usize, voice: usize, s: &Setting, frame: usize, voices: &VoiceBlock, reset: bool) {
        let inharm = self.val(res(r, R_INHARM), voice, frame, voices, true);
        let rate = self.rate;
        let (strike, blow);
        if Self::is_guide(s.kind) {
            let hz = s.hz.clamp(LOWEST_HZ, rate / 4.0);
            let params = WaveguideParams {
                hz: hz as f32,
                decay: s.decay as f32,
                // A pipe and a tube have no Material: Radius moves it.
                material: if s.kind == TYPE_STRING { s.material as f32 } else { 0.5 },
                inharm: if s.kind == TYPE_STRING { (inharm / 100.0).max(0.0) as f32 } else { 0.0 },
                opening: if s.kind == TYPE_PIPE { s.opening as f32 } else { 1.0 },
                radius: if s.kind == TYPE_STRING { 0.0 } else { s.radius as f32 },
                // A guide's positions run from an end (0) to the middle (0.5);
                // Hit and Pos. run from the centre to the edge.
                excite: (0.5 * (1.0 - s.hit)) as f32,
                pickup_l: (0.5 * (1.0 - s.pos_l)) as f32,
                pickup_r: (0.5 * (1.0 - s.pos_r)) as f32,
            };
            if reset {
                self.guide[r].reset_voice(voice, &params);
            } else {
                self.guide[r].set_params(voice, &params);
            }
            // A guide's strike comes back about as 1 / pitch (measured), and
            // its noise as the root of its decay over its period.
            let u = (s.decay * rate).max(1.0);
            strike = 0.5 * STRIKE_LEVEL * u / 27.6 * hz / C3_HZ;
            blow = BLOW_GUIDE / 0.58 * 0.038 * (u / (rate / hz)).sqrt();
        } else {
            let hz = s.hz.clamp(8.0, rate * 0.45);
            let params = ModalParams {
                hz: hz as f32,
                decay: s.decay as f32,
                material: s.material as f32,
                brightness: s.brightness as f32,
                hit: s.hit as f32,
                pos_l: s.pos_l as f32,
                pos_r: s.pos_r as f32,
            };
            if reset {
                self.modal[r].reset_voice(voice, &params);
            } else {
                self.modal[r].set_params(voice, &params);
            }
            // A partial rings an impulse back at 1 / sin(w) of its input
            // gain, w being its angular frequency.
            let u = (s.decay * rate).max(1.0);
            let w = TAU * hz / rate;
            strike = STRIKE_LEVEL * u / 27.6 * w.sin() / (TAU * C3_HZ / rate).sin();
            blow = BLOW_MODAL / 0.58 * 0.0556 * w.sin() * u.sqrt();
        }
        self.strike_scale[r][voice] = strike as f32;
        self.blow_scale[r][voice] = blow as f32;
    }

    /// Voice `voice`'s LFOs for every frame of the piece: the cycle moves by
    /// the rate (hertz, or a division of the tempo) the matrix's routes and
    /// the key leave it, and the value is scaled by Amount.
    fn render_lfos(&mut self, voices: &VoiceBlock, state: &State, frames: &FrameRange<usize>) {
        let stride = self.stride;
        let mask = voices.active_mask();
        let first = mask.trailing_zeros() as usize;
        for frame in frames.clone() {
            let due = state.control_due(frame);
            for l in 0..2 {
                let free = self.raw(lfo(l, L_RETRIGGER), frame) < 0.5;
                let on = self.raw(lfo(l, L_ON), frame) >= 0.5;
                let used = on && self.matrix.uses(l);
                // Every voice reads the other LFO's value from the frame
                // before, so the order of the two doesn't matter.
                let mut active = mask;
                while active != 0 {
                    let v = active.trailing_zeros() as usize;
                    active &= active - 1;
                    self.lfo_out[l][frame * stride + v] = self.lfos.last[l][v];
                }
                if free && on {
                    let rate = self.lfo_rate(l, first, frame, voices, due);
                    self.lfos.advance(l, MAX_VOICES, rate / self.rate);
                }
                let mut active = mask;
                while active != 0 {
                    let v = active.trailing_zeros() as usize;
                    active &= active - 1;
                    if !on {
                        self.lfo_out[l][frame * stride + v] = 0.0;
                        continue;
                    }
                    let slot = if free { MAX_VOICES } else { v };
                    if !free {
                        let rate = self.lfo_rate(l, v, frame, voices, due);
                        self.lfos.advance(l, v, rate / self.rate);
                    }
                    let value = if used {
                        let offset = self.raw(lfo(l, L_OFFSET), frame) / 360.0;
                        let phase = (self.lfos.phase[l][slot] + offset).fract();
                        let shape = self.raw(lfo(l, L_SHAPE), frame) as usize;
                        let amount = self.val(lfo(l, L_AMOUNT), v, frame, voices, due) / 100.0;
                        (lfo_wave(shape, phase, self.lfos.held[l][slot], self.lfos.drawn[l][slot]) * amount) as f32
                    } else {
                        0.0
                    };
                    self.lfo_out[l][frame * stride + v] = value;
                }
            }
            let mut active = mask;
            while active != 0 {
                let v = active.trailing_zeros() as usize;
                active &= active - 1;
                for l in 0..2 {
                    self.lfos.last[l][v] = self.lfo_out[l][frame * stride + v];
                }
            }
        }
    }

    /// LFO `l`'s speed in cycles a second for `voice` at `frame`.
    fn lfo_rate(&self, l: usize, voice: usize, frame: usize, voices: &VoiceBlock, due: bool) -> f64 {
        if self.raw(lfo(l, L_SYNC), frame) >= 0.5 {
            let division = (self.raw(lfo(l, L_DIVISION), frame) as usize).min(DIVISIONS.len() - 1);
            1.0 / (DIVISION_SIXTEENTHS[division] * 60.0 / self.bpm / 4.0)
        } else {
            self.val(lfo(l, L_RATE), voice, frame, voices, due)
        }
    }

    /// Sets up the strike and the noise filter of a note that starts at
    /// `frame`, and puts its resonators at their start.
    fn start_voice(&mut self, voice: usize, frame: usize, voices: &VoiceBlock, state: &State, frames: &FrameRange<usize>) {
        let rate = self.rate;
        let val = |id| self.val(id, voice, frame, voices, true);
        // Spec: collision R2. A raised cosine whose width Stiffness sets
        // between 20 ms and 0.1 ms, scaled to carry the same impulse at any
        // width, with a noise burst through a band-pass at Color.
        let width = pulse_width(val(MALLET_STIFFNESS) / 100.0, rate);
        let mut bp = Svf::default();
        bp.tune(val(MALLET_COLOR), 1.0, rate);
        self.strike[voice] = Strike {
            width: width as f32,
            peak: (2.0 / width * val(MALLET_VOLUME)) as f32,
            noise: (val(MALLET_NOISE) / 100.0) as f32,
            bp,
        };
        self.age[voice] = 0;
        self.noise_lp[voice].clear();
        self.noise_hp[voice].clear();
        self.hit_offset[voice] = state.random(voice) * 2.0 - 1.0;
        for r in 0..2 {
            let setting = self.setting(r, voice, frame, voices, state, frames);
            self.apply(r, voice, &setting, frame, voices, true);
        }
        self.tune_noise_filter(voice, frame, voices);
    }

    /// Tunes `voice`'s noise filter for `frame`: Freq moved by the envelope's
    /// level and Env Amt (R3), and Res.
    fn tune_noise_filter(&mut self, voice: usize, frame: usize, voices: &VoiceBlock) {
        let level = f64::from(self.env_out[frame * self.stride + voice]);
        let hz = self.val(NOISE_FREQ, voice, frame, voices, true)
            * (self.val(NOISE_ENV_AMT, voice, frame, voices, true) / 100.0 * ENV_OCTAVES * level).exp2();
        let res = self.val(NOISE_RES, voice, frame, voices, true) / 100.0;
        let filter = self.raw(NOISE_FILTER, frame) as usize;
        let (lp, hp) = (&mut self.noise_lp[voice], &mut self.noise_hp[voice]);
        match filter {
            FILTER_LP => lp.tune(hz, 0.707 + 9.0 * res, self.rate),
            FILTER_HP => hp.tune(hz, 0.707 + 9.0 * res, self.rate),
            FILTER_BP => lp.tune(hz, 0.5 + 19.5 * res, self.rate),
            _ => {
                // A band whose half-width in octaves Res narrows.
                let half = 0.05 + 3.0 * (1.0 - res);
                hp.tune(hz * (-half).exp2(), 0.707, self.rate);
                lp.tune(hz * half.exp2(), 0.707, self.rate);
            }
        }
    }

    /// One frame of `voice`'s exciters: the mallet's strike and the noise.
    fn excite(&mut self, voice: usize, frame: usize) -> (f32, f32) {
        let mut strike = 0.0;
        if self.raw(MALLET_ON, frame) >= 0.5 {
            let t = self.age[voice];
            let s = &mut self.strike[voice];
            if (t as f32) < s.width {
                let window = raised_cosine(t as f32, s.width);
                let noise = if s.noise > 0.0 {
                    let white = self.noise_rng[voice].next_bipolar() as f32;
                    s.noise * s.bp.run(white).1
                } else {
                    0.0
                };
                strike = s.peak * window * (1.0 + noise);
            }
        }
        self.age[voice] = self.age[voice].saturating_add(1);
        let mut blown = 0.0;
        if self.raw(NOISE_ON, frame) >= 0.5 {
            let white = self.noise_rng[voice].next_bipolar() as f32;
            let filtered = match self.raw(NOISE_FILTER, frame) as usize {
                FILTER_LP => self.noise_lp[voice].run(white).0,
                FILTER_HP => self.noise_hp[voice].run(white).2,
                FILTER_BP => self.noise_lp[voice].run(white).1,
                _ => {
                    let high = self.noise_hp[voice].run(white).2;
                    self.noise_lp[voice].run(high).0
                }
            };
            blown = filtered * self.env_out[frame * self.stride + voice];
        }
        (strike, blown)
    }
}

impl Instrument for Engine {
    fn voice_event(&mut self, _voices: &mut VoiceBlock, _state: &State, event: VoiceEvent) {
        let voice = event.voice;
        let bit = 1u32 << voice;
        match event.kind {
            VoiceEventKind::Start { start, .. } => {
                self.fresh |= bit;
                self.released &= !bit;
                self.quiet[voice] = 0;
                self.matrix.start(voice);
                if matches!(start, Start::Fresh | Start::Steal | Start::Retrigger) {
                    self.lfos.restart(voice);
                }
                let trigger = match start {
                    Start::Fresh => Trigger::New,
                    Start::Steal | Start::Retrigger => Trigger::Retrigger,
                    Start::Legato => Trigger::Legato,
                    Start::Join { .. } => Trigger::New,
                };
                self.env.trigger(voice, trigger);
            }
            VoiceEventKind::Release => {
                self.released |= bit;
                self.env.release(voice);
            }
            VoiceEventKind::End { .. } => {
                self.fresh &= !bit;
                self.released &= !bit;
                self.env.reset(voice);
            }
        }
    }

    fn render(&mut self, voices: &mut VoiceBlock, state: &State, frames: FrameRange<usize>) {
        let count = frames.len();
        if count == 0 || voices.active_mask() == 0 {
            return;
        }
        let stride = self.stride;
        let newest = state.newest(voices);
        self.matrix.set_newest(newest);
        self.matrix.set_oldest(state.oldest(voices));
        let (from, to) = (frames.start, frames.end);
        let lane = from * stride..to * stride;

        self.env.process(&mut self.env_out[lane.clone()]);
        for buffer in [&mut self.mallet, &mut self.blown] {
            buffer[lane.clone()].fill(0.0);
        }
        for l in 0..2 {
            self.lfo_out[l][lane.clone()].fill(0.0);
        }
        self.render_lfos(voices, state, &frames);

        // The notes that start here.
        let mut fresh = self.fresh & voices.active_mask();
        self.fresh &= !fresh;
        while fresh != 0 {
            let voice = fresh.trailing_zeros() as usize;
            fresh &= fresh - 1;
            self.start_voice(voice, from, voices, state, &frames);
        }

        // The exciters, for every frame.
        let mask = voices.active_mask();
        for frame in from..to {
            let mut active = mask;
            while active != 0 {
                let voice = active.trailing_zeros() as usize;
                active &= active - 1;
                let (strike, blown) = self.excite(voice, frame);
                self.mallet[frame * stride + voice] = strike;
                self.blown[frame * stride + voice] = blown;
            }
        }

        // The resonators, in pieces that end on the control grid so their
        // controls move on the same frames at any block size (R19).
        let grid = CONTROL_INTERVAL as usize;
        let serial = self.raw(STRUCTURE, from) < 0.5;
        let mut at = from;
        while at < to {
            let clock = state.clock(at);
            let due = clock % grid as u64 == 0;
            let end = (at + grid - (clock % grid as u64) as usize).min(to);
            for r in 0..2 {
                if self.raw(res(r, R_ON), at) < 0.5 {
                    continue;
                }
                if due {
                    self.tune_banks(r, at, voices);
                    let mut active = mask;
                    while active != 0 {
                        let voice = active.trailing_zeros() as usize;
                        active &= active - 1;
                        let setting = self.setting(r, voice, at, voices, state, &frames);
                        self.apply(r, voice, &setting, at, voices, false);
                    }
                }
                // What excites it: the strike and the noise, or in series
                // Resonator 1's sound for Resonator 2.
                let first_in_series = serial && r == 1 && self.raw(res(0, R_ON), at) >= 0.5;
                for frame in at..end {
                    for voice in 0..stride {
                        let i = frame * stride + voice;
                        self.feed[r][i] = if first_in_series {
                            0.5 * (self.rung[0][frame * 2 * stride + voice]
                                + self.rung[0][(frame * 2 + 1) * stride + voice])
                        } else if voice < MAX_VOICES {
                            self.mallet[i] * self.strike_scale[r][voice]
                                + self.blown[i] * self.blow_scale[r][voice]
                        } else {
                            0.0
                        };
                    }
                }
                let input = at * stride..end * stride;
                let output = at * 2 * stride..end * 2 * stride;
                let kind = (self.raw(res(r, R_TYPE), at) as usize).min(RESONATOR_TYPES.len() - 1);
                if Self::is_guide(kind) {
                    self.guide[r].process(&self.feed[r][input], &mut self.rung[r][output]);
                } else {
                    self.modal[r].process(&self.feed[r][input], &mut self.rung[r][output]);
                }
            }
            at = end;
        }

        // The mixers: each resonator's Gain, Pan and Bleed, then Volume.
        let mut active = mask;
        while active != 0 {
            let voice = active.trailing_zeros() as usize;
            active &= active - 1;
            for frame in from..to {
                let i = frame * stride + voice;
                let exciter = self.mallet[i] + self.blown[i];
                let (mut left, mut right) = (0.0f32, 0.0f32);
                for r in 0..2 {
                    if self.raw(res(r, R_ON), frame) < 0.5 {
                        continue;
                    }
                    let value = |offset| self.val(res(r, offset), voice, frame, voices, true);
                    let gain = value(R_GAIN) as f32;
                    let pan = (value(R_PAN) / 100.0) as f32;
                    let bleed = (value(R_BLEED) / 100.0) as f32 * exciter;
                    let l = (self.rung[r][frame * 2 * stride + voice] + bleed) * gain;
                    let rr = (self.rung[r][(frame * 2 + 1) * stride + voice] + bleed) * gain;
                    left += l * (1.0 - pan.max(0.0));
                    right += rr * (1.0 + pan.min(0.0));
                }
                let volume = self.val(VOLUME, voice, frame, voices, true) as f32;
                let (left, right) = (left * volume, right * volume);
                self.out_l[frame] += left;
                self.out_r[frame] += right;
                if left.abs().max(right.abs()) > SILENT {
                    self.quiet[voice] = 0;
                } else {
                    self.quiet[voice] = self.quiet[voice].saturating_add(1);
                }
            }
        }

        // A released voice that has been silent for a while ends.
        let after = (QUIET_SECONDS * self.rate) as u32;
        let mut active = mask;
        while active != 0 {
            let voice = active.trailing_zeros() as usize;
            active &= active - 1;
            if self.released & (1 << voice) != 0 && self.quiet[voice] >= after {
                voices.end(voice);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Spec: collision R2
    #[test]
    fn the_strike_is_a_raised_cosine_whose_width_stiffness_sets() {
        let rate = 48_000.0;
        assert!((pulse_width(0.0, rate) - 0.02 * rate).abs() < 1e-6, "20 ms soft");
        assert!((pulse_width(1.0, rate) - 0.000_1 * rate).abs() < 1e-6, "0.1 ms hard");
        assert!(pulse_width(0.25, rate) > pulse_width(0.75, rate), "stiffer is shorter");
        assert!(pulse_width(1.0, 8_000.0) >= 1.0, "never under a frame");
        let width = 960.0;
        assert_eq!(raised_cosine(0.0, width), 0.0);
        assert!((raised_cosine(width / 2.0, width) - 1.0).abs() < 1e-6);
        assert!(raised_cosine(width / 4.0, width) > 0.49);
        assert!(raised_cosine(width / 4.0, width) < 0.51);
    }

    // Spec: collision R14
    #[test]
    fn every_chooser_lists_off_then_the_destinations_by_their_parameters_names() {
        let params = descriptors();
        let destinations = destination_ids();
        let chooser = params[lfo(0, L_DEST_A).0 as usize].clone();
        let choices = match chooser.kind {
            bl_model::ParamKind::Choice { choices } => choices,
            other => panic!("a chooser, not {other:?}"),
        };
        assert_eq!(choices.len(), destinations.len() + 1);
        assert_eq!(choices[0], "Off");
        for (n, (id, _, _)) in destinations.iter().enumerate() {
            assert_eq!(choices[n + 1], params[id.0 as usize].name);
        }
    }
}

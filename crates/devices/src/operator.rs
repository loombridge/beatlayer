//! Operator, part 1 (WP-8.12; spec: operator R11 to R22): four oscillators
//! on the FM core ([`crate::fm`]) with their envelopes and the pitch
//! envelope ([`crate::operator_envelope`]), Time and Tone, the voice
//! section's Voices, Transpose, Retrigger and per-note pitch bend, and the
//! body as a shell and display (device framework R41).
//!
//! Part 2 (WP-8.13; spec: operator R23 to R30) adds the LFO
//! ([`crate::operator_lfo`]), the instruments' filter and a shaper after
//! it, the global modulation targets as chosen slots of the instruments'
//! matrix, Glide, Spread and the user waves' tables.
//!
//! The manual records few ranges or defaults, so those are Beatlayer's own
//! and are listed in `docs/specs/operator.md`.

use std::sync::Arc;

use bl_engine::{DeviceType, NodeConfig, NodeData, NodeEvent, Registry};
use bl_model::device_layout::{
    BODY_HEIGHT, ControlBand, DisplayCell, LayoutNode, Page, PageStyle, PagesNode, TAB_HEIGHT,
    Widget,
};
use bl_model::matrix::{
    DestDef, Kind as MatrixKind, MatrixLayout, Pick, Range as MatrixRange, Rate, Scope, SlotDef,
    SourceDef,
};
use bl_model::{
    Device, DeviceDescription, DeviceLayout, DeviceRole, ParamDescriptor, ParamId, ParamUnit,
};
use lspk_dsp::filter::pitch_of;
use lspk_voice::unison::pan_gains;
use lspk_voice::{MAX_VOICES, Start, VoiceBlock, VoiceEvent, VoiceEventKind};

use crate::fm::{self, Moves, OPERATORS};
use crate::framework::{Audio, NativeDevice, ParamBank, add_with_data};
use crate::instrument::matrix::Matrix;
use crate::instrument::{self, Instrument, NoteRange, Parts, Section, Sources, State};
use crate::instrument_filter::{CIRCUITS, InstrumentFilter, Kind, Per, PerFrame, Setup};
use crate::multisampler::shape;
use crate::operator_envelope::{self as envelope, Curve, Loop, Shape};
use crate::operator_lfo::{self as lfo, Lfo};
use crate::sampler::FILTER_TYPE_CHOICES;

/// Operator's device type.
pub const OPERATOR: &str = "beatlayer.operator";

/// How far apart oscillator A's, B's, C's and D's parameter numbers are.
pub const OSC_STRIDE: u32 = 32;

/// An oscillator's controls, by the offset [`osc`] adds to its first:
/// the oscillator (R11), then its envelope (R12, R14, R15).
pub const OSC_ON: u32 = 0;
pub const OSC_COARSE: u32 = 1;
pub const OSC_FINE: u32 = 2;
pub const OSC_FIXED: u32 = 3;
pub const OSC_FREQ: u32 = 4;
pub const OSC_MULTI: u32 = 5;
pub const OSC_LEVEL: u32 = 6;
pub const OSC_WAVE: u32 = 7;
pub const OSC_FEEDBACK: u32 = 8;
pub const OSC_PHASE: u32 = 9;
pub const OSC_RETRIGGER: u32 = 10;
pub const OSC_REPEAT: u32 = 11;
pub const OSC_VEL: u32 = 12;
pub const OSC_Q: u32 = 13;
pub const OSC_LEVEL_VEL: u32 = 14;
pub const OSC_LEVEL_KEY: u32 = 15;
pub const ENV_INITIAL: u32 = 16;
pub const ENV_PEAK: u32 = 17;
pub const ENV_SUSTAIN: u32 = 18;
pub const ENV_ATTACK: u32 = 19;
pub const ENV_DECAY: u32 = 20;
pub const ENV_RELEASE: u32 = 21;
pub const ENV_LOOP: u32 = 22;
pub const ENV_LOOP_TIME: u32 = 23;
pub const ENV_REPEAT: u32 = 24;
pub const ENV_TIME_VEL: u32 = 25;
/// Repeat's switch: off, a user wave's partials above its count are silent
/// (R7).
pub const OSC_REPEAT_ON: u32 = 26;

/// The parameter of oscillator `op` (0 for A to 3 for D) holding `control`.
#[must_use]
pub const fn osc(op: usize, control: u32) -> ParamId {
    ParamId(op as u32 * OSC_STRIDE + control)
}

/// The global controls: the core's, Time and Tone, the output and the voice
/// section's.
pub const ALGORITHM: ParamId = ParamId(128);
pub const ANTIALIAS: ParamId = ParamId(129);
pub const INTERPOLATION: ParamId = ParamId(130);
pub const TONE: ParamId = ParamId(131);
pub const TIME: ParamId = ParamId(132);
pub const TIME_KEY: ParamId = ParamId(133);
pub const VOLUME: ParamId = ParamId(134);
pub const PAN: ParamId = ParamId(135);
pub const PAN_KEY: ParamId = ParamId(136);
pub const PAN_RANDOM: ParamId = ParamId(137);
pub const TRANSPOSE: ParamId = ParamId(138);
pub const VOICES: ParamId = ParamId(139);
pub const RETRIGGER: ParamId = ParamId(140);
pub const PB_RANGE: ParamId = ParamId(141);
/// Enable Per-Note Pitch Bend, a menu option (R20).
pub const NOTE_PB: ParamId = ParamId(142);

/// The pitch envelope's switch and amount, its Destination A switches for
/// oscillators A to D (numbers from [`PITCH_OSC`]) and the LFO, and its
/// Destination B and amount (R16).
pub const PITCH_ON: ParamId = ParamId(143);
pub const PITCH_AMOUNT: ParamId = ParamId(144);
pub const PITCH_OSC: u32 = 145;
pub const PITCH_LFO: ParamId = ParamId(149);
pub const PITCH_DEST_B: ParamId = ParamId(150);
pub const PITCH_AMOUNT_B: ParamId = ParamId(151);

/// How far apart the filter's, the pitch's and the LFO's envelopes' first
/// parameters are, and where each begins.
pub const SHAPED_STRIDE: u32 = 16;
pub const FILTER_ENV: u32 = 160;
pub const PITCH_ENV: u32 = 176;
pub const LFO_ENV: u32 = 192;

/// A shaped envelope's controls, by the offset [`shaped`] adds to its first
/// (R13, R14, R15).
pub const SHAPED_INITIAL: u32 = 0;
pub const SHAPED_PEAK: u32 = 1;
pub const SHAPED_SUSTAIN: u32 = 2;
pub const SHAPED_END: u32 = 3;
pub const SHAPED_ATTACK: u32 = 4;
pub const SHAPED_DECAY: u32 = 5;
pub const SHAPED_RELEASE: u32 = 6;
pub const SHAPED_ATTACK_SLOPE: u32 = 7;
pub const SHAPED_DECAY_SLOPE: u32 = 8;
pub const SHAPED_RELEASE_SLOPE: u32 = 9;
pub const SHAPED_LOOP: u32 = 10;
pub const SHAPED_LOOP_TIME: u32 = 11;
pub const SHAPED_REPEAT: u32 = 12;
pub const SHAPED_TIME_VEL: u32 = 13;

/// The parameter holding `control` of the shaped envelope that begins at
/// `envelope` ([`FILTER_ENV`], [`PITCH_ENV`] or [`LFO_ENV`]).
#[must_use]
pub const fn shaped(envelope: u32, control: u32) -> ParamId {
    ParamId(envelope + control)
}

/// The LFO's controls (R23): its switch, wave, range, Rate, Rate < Key,
/// Amount and Amt < Vel, Retrigger, Destination A's switches for
/// oscillators A to D (numbers from [`LFO_OSC`]) and the filter, Amount A,
/// Destination B and Amount B.
pub const LFO_ON: ParamId = ParamId(208);
pub const LFO_WAVE: ParamId = ParamId(209);
pub const LFO_RANGE: ParamId = ParamId(210);
pub const LFO_RATE: ParamId = ParamId(211);
pub const LFO_RATE_KEY: ParamId = ParamId(212);
pub const LFO_AMOUNT: ParamId = ParamId(213);
pub const LFO_AMT_VEL: ParamId = ParamId(214);
pub const LFO_RETRIGGER: ParamId = ParamId(215);
pub const LFO_OSC: u32 = 216;
pub const LFO_FILTER: ParamId = ParamId(220);
pub const LFO_AMOUNT_A: ParamId = ParamId(221);
pub const LFO_DEST_B: ParamId = ParamId(222);
pub const LFO_AMOUNT_B: ParamId = ParamId(223);

/// The filter's controls (R24): the instruments' filter, the three
/// modulation depths and the shaper after it.
pub const FILTER_ON: ParamId = ParamId(224);
pub const FILTER_TYPE: ParamId = ParamId(225);
pub const FILTER_CIRCUIT: ParamId = ParamId(226);
pub const FILTER_FREQ: ParamId = ParamId(227);
pub const FILTER_RES: ParamId = ParamId(228);
pub const FILTER_DRIVE: ParamId = ParamId(229);
pub const FILTER_MORPH: ParamId = ParamId(230);
pub const FREQ_VEL: ParamId = ParamId(231);
pub const FREQ_KEY: ParamId = ParamId(232);
pub const FREQ_ENV: ParamId = ParamId(233);
pub const SHAPER_TYPE: ParamId = ParamId(234);
pub const SHAPER_DRIVE: ParamId = ParamId(235);
pub const SHAPER_WET: ParamId = ParamId(236);

/// Glide, its time, and Spread (R26).
pub const GLIDE_ON: ParamId = ParamId(260);
pub const GLIDE_TIME: ParamId = ParamId(261);
pub const SPREAD: ParamId = ParamId(262);

/// The global modulation sources, in the matrix's order (R25).
pub const TARGET_SOURCES: [&str; 5] = ["Velocity", "Key", "Aftertouch", "Pitch Bend", "Mod Wheel"];

/// The Destination chooser (`b` false for A, true for B) and Amount of
/// global modulation source `source` (0 to 4, [`TARGET_SOURCES`]).
#[must_use]
pub const fn target_dest(source: usize, b: bool) -> ParamId {
    ParamId(240 + source as u32 * 2 + b as u32)
}
#[must_use]
pub const fn target_amount(source: usize, b: bool) -> ParamId {
    ParamId(250 + source as u32 * 2 + b as u32)
}

/// The shaper's curves: Off, then the multisampler's (R24).
pub const SHAPERS: [&str; 5] = ["Off", "Soft", "Hard", "Sine", "4 Bit"];

/// What Multi chooses between, in hertz.
pub const MULTIS: [f64; 6] = [0.1, 1.0, 10.0, 100.0, 1_000.0, 10_000.0];

/// Destination B's choices (operator R25's list, in its order). Until the
/// filter, the LFO and the global targets arrive, only the oscillators'
/// volumes, Osc Feedback, Osc Fixed Frequency, Volume and Panorama act.
pub const DESTINATIONS: [&str; 23] = [
    "Off",
    "Osc Volume A",
    "Osc Volume B",
    "Osc Volume C",
    "Osc Volume D",
    "Osc Crossfade A/C",
    "Osc Crossfade B/D",
    "Osc Feedback",
    "Osc Fixed Frequency",
    "FM Drive",
    "Filter Frequency",
    "Filter Res",
    "Filter Morph",
    "Filter Drive",
    "Filter Envelope Amount",
    "Shaper Drive",
    "LFO Rate",
    "LFO Amount",
    "Pitch Envelope Amount",
    "Volume",
    "Panorama",
    "Tone",
    "Time",
];
const DEST_VOLUME_A: usize = 1;
const DEST_FEEDBACK: usize = 7;
const DEST_FIXED: usize = 8;
const DEST_VOLUME: usize = 19;
const DEST_PANORAMA: usize = 20;

/// Spec: operator R16. A pitch envelope level of 100 % at Pitch Env 100 %.
const PITCH_SEMITONES: f32 = 48.0;

/// Spec: operator R23, R24. The LFO's reach on the filter's cutoff, four
/// octaves; the filter envelope's at 100 %, nine; Freq < Vel's at 100 % and
/// velocity 127, six (Beatlayer's own); and a matrix movement of 1 on the
/// cutoff, its whole travel of ten octaves.
const LFO_FILTER_SEMITONES: f32 = 48.0;
const ENV_FILTER_SEMITONES: f32 = 108.0;
const VEL_FILTER_SEMITONES: f32 = 72.0;
const TRAVEL_SEMITONES: f32 = 120.0;

/// Spec: operator R25. FM Drive at a matrix movement of 1 multiplies every
/// index by 4 (Beatlayer's own).
const DRIVE_REACH: f32 = 3.0;

/// Spec: operator R15. Time's reach in octaves of time at 100 %.
const TIME_OCTAVES: f32 = 4.0;

/// The middle of the keys: C3, which Level < Key, Time < Key and Pan < Key
/// count from.
const MIDDLE_KEY: f32 = 60.0;

/// The envelopes that run: the oscillators', the pitch's, the filter's and
/// the LFO's.
const RUNNING: usize = OPERATORS + 3;
const PITCH: usize = OPERATORS;
const FILTER: usize = OPERATORS + 1;
const LFO: usize = OPERATORS + 2;

/// The times' range in milliseconds (R12).
const MAX_TIME_MS: f64 = 20_000.0;

/// Operator's descriptors, in the order of their numbers.
fn parameters() -> Vec<ParamDescriptor> {
    let time = |id, name: &str, default: f64| {
        ParamDescriptor::continuous(
            id,
            name,
            (0.0, MAX_TIME_MS),
            default,
            ParamUnit::Milliseconds,
        )
        .power(3)
    };
    let percent = |id, name: &str, range: (f64, f64), default: f64| {
        ParamDescriptor::continuous(id, name, range, default, ParamUnit::Percent)
    };
    let mut params = Vec::new();
    for (op, letter) in ["A", "B", "C", "D"].into_iter().enumerate() {
        let name = |what: &str| format!("{letter} {what}");
        let id = |control| osc(op, control);
        params.extend([
            ParamDescriptor::toggle(id(OSC_ON), &name("On"), true)
                .describe("Takes the oscillator out of the sound when off."),
            // 0 reads as a ratio of 0.5.
            ParamDescriptor::integer(id(OSC_COARSE), &name("Coarse"), (0, 48), 1, ParamUnit::None)
                .describe("The whole-number ratio of the note's frequency; 0 is a half."),
            ParamDescriptor::integer(id(OSC_FINE), &name("Fine"), (0, 1000), 0, ParamUnit::None)
                .describe("Thousandths of a ratio added to Coarse."),
            ParamDescriptor::toggle(id(OSC_FIXED), &name("Fixed"), false)
                .describe("Plays Freq times Multi whatever the key."),
            ParamDescriptor::continuous(
                id(OSC_FREQ),
                &name("Freq"),
                (1.0, 1000.0),
                100.0,
                ParamUnit::Hertz,
            )
            .describe("The fixed frequency before Multi."),
            ParamDescriptor::choice(
                id(OSC_MULTI),
                &name("Multi"),
                &["0.1", "1", "10", "100", "1000", "10000"],
                1,
            )
            .describe("What Freq is multiplied by."),
            ParamDescriptor::gain(
                id(OSC_LEVEL),
                &name("Level"),
                0.0,
                if op == 0 { 0.0 } else { f64::NEG_INFINITY },
            )
            .describe("The oscillator's output level; a modulator's sets how far it bends its carriers."),
            ParamDescriptor::choice(id(OSC_WAVE), &name("Wave"), &fm::WAVES, 0)
                .describe("The waveform the oscillator plays."),
            percent(id(OSC_FEEDBACK), &name("Feedback"), (0.0, 100.0), 0.0)
                .describe("How far the oscillator modulates itself, if nothing modulates it."),
            ParamDescriptor::continuous(
                id(OSC_PHASE),
                &name("Phase"),
                (0.0, 360.0),
                0.0,
                ParamUnit::None,
            )
            .describe("Where in its cycle the oscillator starts, in degrees."),
            ParamDescriptor::toggle(id(OSC_RETRIGGER), &name("Retrigger"), true)
                .describe("Starts the oscillator at Phase with each note."),
            percent(id(OSC_REPEAT), &name("Repeat"), (0.0, 100.0), 0.0)
                .describe("How far the drawn partials of a User wave repeat above their count, each copy fainter."),
            ParamDescriptor::toggle(id(OSC_REPEAT_ON), &name("Repeat On"), false)
                .describe("Repeats the drawn partials of a User wave above their count; off, the partials above it are silent."),
            percent(id(OSC_VEL), &name("Osc < Vel"), (-100.0, 100.0), 0.0)
                .describe("How far velocity moves the ratio, up to an octave."),
            ParamDescriptor::toggle(id(OSC_Q), &name("Q"), true)
                .describe("Rounds the velocity-shifted Coarse to a whole step."),
            percent(id(OSC_LEVEL_VEL), &name("Level < Vel"), (0.0, 100.0), 0.0)
                .describe("How far velocity moves the level: at 100 %, the softest notes are nearly silent."),
            percent(id(OSC_LEVEL_KEY), &name("Level < Key"), (-100.0, 100.0), 0.0)
                .describe("How far the key moves the level, up to 6 dB per octave from C3."),
            ParamDescriptor::gain(id(ENV_INITIAL), &name("Initial"), 0.0, f64::NEG_INFINITY)
                .describe("The envelope's level at the start of the attack."),
            ParamDescriptor::gain(id(ENV_PEAK), &name("Peak"), 0.0, 0.0)
                .describe("The envelope's level at the end of the attack."),
            ParamDescriptor::gain(id(ENV_SUSTAIN), &name("Sustain"), 0.0, 0.0)
                .describe("The level the envelope holds while the note does."),
            time(id(ENV_ATTACK), &name("Attack"), 1.0)
                .describe("How long the envelope takes from Initial to Peak."),
            time(id(ENV_DECAY), &name("Decay"), 1_000.0)
                .describe("How long the envelope takes from Peak to Sustain."),
            time(id(ENV_RELEASE), &name("Release"), 50.0)
                .describe("How long the envelope takes to fall to silence after the note ends."),
            ParamDescriptor::choice(id(ENV_LOOP), &name("Loop"), &envelope::LOOPS, 0)
                .describe("None, Loop, Beat, Sync or Trigger: how the envelope repeats or ignores the note's end."),
            time(id(ENV_LOOP_TIME), &name("Loop Time"), 100.0)
                .describe("How long Loop takes to return to Initial."),
            ParamDescriptor::choice(id(ENV_REPEAT), &name("Env Repeat"), &envelope::REPEATS, 4)
                .describe("How often Beat and Sync start the envelope again, in beats."),
            percent(id(ENV_TIME_VEL), &name("Time < Vel"), (-100.0, 100.0), 0.0)
                .describe("How far velocity shortens the envelope's times."),
        ]);
    }
    params.extend([
        ParamDescriptor::integer(ALGORITHM, "Algorithm", (1, 11), 1, ParamUnit::None)
            .describe("Which oscillators modulate which, of the eleven graphs."),
        ParamDescriptor::toggle(ANTIALIAS, "Antialias", true)
            .describe("Runs the oscillators at twice the sample rate, at the cost of a little delay."),
        ParamDescriptor::toggle(INTERPOLATION, "Interpolation", true)
            .describe("Reads the oscillators' tables with interpolation."),
        percent(TONE, "Tone", (0.0, 100.0), 70.0)
            .describe("Limits the partials every oscillator plays: lower is darker."),
        percent(TIME, "Time", (-100.0, 100.0), 0.0)
            .describe("Lengthens every envelope's times, or shortens them when negative; levels stay."),
        percent(TIME_KEY, "Time < Key", (-100.0, 100.0), 0.0)
            .describe("How far higher keys shorten every envelope's times, from C3."),
        ParamDescriptor::gain(VOLUME, "Volume", 0.0, -6.0)
            .describe("The instrument's output level."),
        percent(PAN, "Pan", (-100.0, 100.0), 0.0).describe("Places the instrument between left and right."),
        percent(PAN_KEY, "Pan < Key", (-100.0, 100.0), 0.0)
            .describe("Pans low notes left and high notes right, hard four octaves either side of C3 at 100 %."),
        percent(PAN_RANDOM, "Pan < Random", (0.0, 100.0), 0.0)
            .describe("Pans each note by a random amount up to this."),
        instrument::transpose(TRANSPOSE, 48),
        instrument::voices(VOICES, MAX_VOICES as i64, 6),
        instrument::retrigger(RETRIGGER, false),
        instrument::pb_range(PB_RANGE, 48, 5),
        {
            let mut note_pb = instrument::note_pb(NOTE_PB, true);
            "Enable Per-Note Pitch Bend".clone_into(&mut note_pb.name);
            note_pb
        },
        ParamDescriptor::toggle(PITCH_ON, "Pitch Env On", false)
            .describe("Moves the oscillators it is switched on for by the pitch envelope."),
        percent(PITCH_AMOUNT, "Pitch Env", (-100.0, 100.0), 100.0)
            .describe("How far the pitch envelope moves pitch: 100 % follows its levels, -100 % inverts them."),
    ]);
    for (op, letter) in ["A", "B", "C", "D"].into_iter().enumerate() {
        params.push(
            ParamDescriptor::toggle(
                ParamId(PITCH_OSC + op as u32),
                &format!("Pitch Osc {letter}"),
                true,
            )
            .describe("Sends the pitch envelope to this oscillator."),
        );
    }
    params.extend([
        ParamDescriptor::toggle(PITCH_LFO, "Pitch LFO", false)
            .describe("Sends the pitch envelope to the LFO's rate."),
        ParamDescriptor::choice(PITCH_DEST_B, "Pitch Dest B", &DESTINATIONS, 0)
            .describe("Sends the pitch envelope to one more destination."),
        percent(PITCH_AMOUNT_B, "Pitch Amount B", (-100.0, 100.0), 0.0)
            .describe("How far the pitch envelope moves its second destination."),
    ]);
    for (name, base, level) in [
        ("Filter", FILTER_ENV, 0.0),
        ("Pitch", PITCH_ENV, 0.0),
        ("LFO", LFO_ENV, 100.0),
    ] {
        let id = |control| shaped(base, control);
        let name = |what: &str| format!("{name} {what}");
        let slope = |control, what: &str| {
            ParamDescriptor::continuous(
                id(control),
                &name(&format!("{what} Slope")),
                (-1.0, 1.0),
                0.0,
                ParamUnit::None,
            )
            .describe("Bends the segment: positive starts fast and ends slow, negative the other way, 0 is straight.")
        };
        params.extend([
            percent(id(SHAPED_INITIAL), &name("Initial"), (-100.0, 100.0), level)
                .describe("The envelope's level at the start of the attack."),
            percent(id(SHAPED_PEAK), &name("Peak"), (-100.0, 100.0), level)
                .describe("The envelope's level at the end of the attack."),
            percent(id(SHAPED_SUSTAIN), &name("Sustain"), (-100.0, 100.0), level)
                .describe("The level the envelope holds while the note does."),
            percent(id(SHAPED_END), &name("End"), (-100.0, 100.0), level)
                .describe("The level the envelope reaches at the end of its release."),
            time(id(SHAPED_ATTACK), &name("Attack"), 0.0)
                .describe("How long the envelope takes from Initial to Peak."),
            time(id(SHAPED_DECAY), &name("Decay"), 1_000.0)
                .describe("How long the envelope takes from Peak to Sustain."),
            time(id(SHAPED_RELEASE), &name("Release"), 50.0)
                .describe("How long the envelope takes from Sustain to End after the note ends."),
            slope(SHAPED_ATTACK_SLOPE, "Attack"),
            slope(SHAPED_DECAY_SLOPE, "Decay"),
            slope(SHAPED_RELEASE_SLOPE, "Release"),
            ParamDescriptor::choice(id(SHAPED_LOOP), &name("Loop"), &envelope::LOOPS, 0)
                .describe("None, Loop, Beat, Sync or Trigger: how the envelope repeats or ignores the note's end."),
            time(id(SHAPED_LOOP_TIME), &name("Loop Time"), 100.0)
                .describe("How long Loop takes to return to Initial."),
            ParamDescriptor::choice(id(SHAPED_REPEAT), &name("Repeat"), &envelope::REPEATS, 4)
                .describe("How often Beat and Sync start the envelope again, in beats."),
            percent(id(SHAPED_TIME_VEL), &name("Time < Vel"), (-100.0, 100.0), 0.0)
                .describe("How far velocity shortens the envelope's times."),
        ]);
    }
    params.extend([
        ParamDescriptor::toggle(LFO_ON, "LFO On", true).describe("Plays the LFO."),
        ParamDescriptor::choice(LFO_WAVE, "LFO Wave", &lfo::WAVES, 0)
            .describe("The LFO's waveform: Noise is band-pass filtered noise."),
        ParamDescriptor::choice(LFO_RANGE, "LFO Range", &lfo::RANGES, 0).describe(
            "Low runs from 50 seconds a cycle to 30 Hz, Hi from 8 Hz to 12 kHz, and Sync in divisions of the song's tempo.",
        ),
        percent(LFO_RATE, "LFO Rate", (0.0, 100.0), 50.0)
            .describe("The LFO's speed within its range."),
        percent(LFO_RATE_KEY, "LFO Rate < Key", (0.0, 100.0), 0.0)
            .describe("How far higher keys speed the LFO: at 100 %, it doubles each octave."),
        percent(LFO_AMOUNT, "LFO Amount", (0.0, 100.0), 0.0)
            .describe("How far the LFO moves its destinations."),
        percent(LFO_AMT_VEL, "LFO Amt < Vel", (0.0, 100.0), 0.0)
            .describe("How far velocity scales the LFO's amount: at 100 %, the softest notes have none."),
        ParamDescriptor::toggle(LFO_RETRIGGER, "LFO Retrigger", true)
            .describe("Restarts the LFO with each note; off, every voice reads one free-running LFO."),
    ]);
    for (op, letter) in ["A", "B", "C", "D"].into_iter().enumerate() {
        params.push(
            ParamDescriptor::toggle(
                ParamId(LFO_OSC + op as u32),
                &format!("LFO Osc {letter}"),
                false,
            )
            .describe("Sends the LFO to this oscillator's level, as tremolo."),
        );
    }
    params.extend([
        ParamDescriptor::toggle(LFO_FILTER, "LFO Filter", false)
            .describe("Sends the LFO to the filter's cutoff, up to four octaves."),
        percent(LFO_AMOUNT_A, "LFO Amount A", (-100.0, 100.0), 100.0)
            .describe("How far the LFO moves the destinations switched on in A."),
        ParamDescriptor::choice(LFO_DEST_B, "LFO Dest B", &DESTINATIONS, 0)
            .describe("Sends the LFO to one more destination."),
        percent(LFO_AMOUNT_B, "LFO Amount B", (-100.0, 100.0), 0.0)
            .describe("How far the LFO moves its second destination."),
        ParamDescriptor::toggle(FILTER_ON, "Filter On", false)
            .describe("Plays every voice through the filter."),
        ParamDescriptor::choice(FILTER_TYPE, "Filter Type", &FILTER_TYPE_CHOICES, 1)
            .describe("The filter's type and slope: low-pass, high-pass, band-pass, notch or Morph, at 12 or 24 dB per octave."),
        ParamDescriptor::choice(FILTER_CIRCUIT, "Filter Circuit", &CIRCUITS, 0)
            .describe("The filter's circuit; one a type doesn't offer plays Clean."),
        ParamDescriptor::continuous(
            FILTER_FREQ,
            "Filter Freq",
            (20.0, 20_000.0),
            20_000.0,
            ParamUnit::Hertz,
        )
        .logarithmic()
        .describe("The filter's cutoff frequency."),
        percent(FILTER_RES, "Filter Res", (0.0, 100.0), 0.0)
            .describe("Raises the filter's resonance at the cutoff."),
        ParamDescriptor::continuous(
            FILTER_DRIVE,
            "Filter Drive",
            (0.0, 24.0),
            0.0,
            ParamUnit::Decibels,
        )
        .describe("Drives the signal into the filter's circuit; Clean and Morph ignore it."),
        percent(FILTER_MORPH, "Filter Morph", (0.0, 100.0), 0.0)
            .describe("Where the Morph type sits between low-pass, band-pass, high-pass and notch."),
        percent(FREQ_VEL, "Freq < Vel", (-100.0, 100.0), 0.0)
            .describe("How far velocity moves the cutoff, up to six octaves."),
        percent(FREQ_KEY, "Freq < Key", (0.0, 100.0), 0.0)
            .describe("How far the key moves the cutoff: at 100 %, it doubles each octave from C3."),
        percent(FREQ_ENV, "Freq < Envelope", (-100.0, 100.0), 0.0)
            .describe("How far the filter's envelope moves the cutoff: 100 % reaches nine octaves."),
        ParamDescriptor::choice(SHAPER_TYPE, "Shaper Type", &SHAPERS, 0)
            .describe("The shaper's curve after the filter; Off bypasses it."),
        ParamDescriptor::continuous(
            SHAPER_DRIVE,
            "Shaper Drive",
            (0.0, 24.0),
            0.0,
            ParamUnit::Decibels,
        )
        .describe("Drives the signal into the shaper's curve."),
        percent(SHAPER_WET, "Shaper Dry/Wet", (0.0, 100.0), 0.0)
            .describe("Mixes the shaped signal with the unshaped; at 0 %, the shaper is bypassed."),
    ]);
    for (source, name) in TARGET_SOURCES.into_iter().enumerate() {
        for (b, letter) in [(false, "A"), (true, "B")] {
            params.push(
                ParamDescriptor::choice(
                    target_dest(source, b),
                    &format!("{name} Dest {letter}"),
                    &DESTINATIONS,
                    0,
                )
                .describe("The destination this source moves."),
            );
        }
        for (b, letter) in [(false, "A"), (true, "B")] {
            params.push(
                percent(
                    target_amount(source, b),
                    &format!("{name} Amount {letter}"),
                    (-100.0, 100.0),
                    0.0,
                )
                .describe("How far this source moves its destination."),
            );
        }
    }
    params.extend([
        ParamDescriptor::toggle(GLIDE_ON, "Glide", false)
            .describe("Slides each new note from the last, whether or not they overlap."),
        instrument::glide_time(GLIDE_TIME, 10_000.0, 100.0),
        percent(SPREAD, "Spread", (0.0, 100.0), 0.0).describe(
            "Plays each new note as two detuned copies, left and right; the copies' mono sum starts in phase.",
        ),
    ]);
    params
}

/// A band `height` tall of the controls for `ids`, each drawn as its
/// parameter's kind says.
fn band(params: &[ParamDescriptor], height: f32, ids: &[ParamId]) -> LayoutNode {
    let controls: Vec<(ParamId, Widget)> = ids
        .iter()
        .map(|id| {
            let kind = &params
                .iter()
                .find(|p| p.id == *id)
                .expect("a parameter")
                .kind;
            (*id, Widget::for_kind(kind))
        })
        .collect();
    LayoutNode::Band(ControlBand::stacked(height, &controls))
}

/// The shell's sections in order (R18): each a page with its main controls
/// in the strip and the rest in the display. Which controls are main and
/// which fill the display is Beatlayer's own.
fn layout(params: &[ParamDescriptor]) -> DeviceLayout {
    let section = |ids: &[ParamId]| band(params, BODY_HEIGHT - TAB_HEIGHT, ids);
    let display = |ids: &[ParamId]| band(params, BODY_HEIGHT, ids);
    let range = |first: u32, count: u32| (first..first + count).map(ParamId).collect::<Vec<_>>();
    let envelope_of = |base: u32| range(base, 14);
    // Spec: operator R29. Every section's display is one cell, which the
    // section's view fills: layout is open (the displays' placement).
    let cell = || LayoutNode::Cell(DisplayCell::new("Display", 160.0, BODY_HEIGHT, 1.0));
    let mut pages = vec![
        Page::new(
            "Global",
            LayoutNode::Row(vec![
                cell(),
                display(&[
                    ANTIALIAS,
                    INTERPOLATION,
                    TONE,
                    TIME,
                    TIME_KEY,
                    PAN,
                    PAN_KEY,
                    PAN_RANDOM,
                    TRANSPOSE,
                    VOICES,
                    RETRIGGER,
                    PB_RANGE,
                    GLIDE_ON,
                    GLIDE_TIME,
                    SPREAD,
                ]),
                display(
                    &(0..TARGET_SOURCES.len())
                        .flat_map(|s| {
                            [
                                target_dest(s, false),
                                target_amount(s, false),
                                target_dest(s, true),
                                target_amount(s, true),
                            ]
                        })
                        .collect::<Vec<_>>(),
                ),
            ]),
        )
        .with_section(section(&[ALGORITHM, VOLUME])),
    ];
    for (op, letter) in ["A", "B", "C", "D"].into_iter().enumerate() {
        let id = |control| osc(op, control);
        let extra: Vec<ParamId> = [
            OSC_FINE,
            OSC_FIXED,
            OSC_FREQ,
            OSC_MULTI,
            OSC_FEEDBACK,
            OSC_PHASE,
            OSC_RETRIGGER,
            OSC_REPEAT,
            OSC_REPEAT_ON,
            OSC_VEL,
            OSC_Q,
            OSC_LEVEL_VEL,
            OSC_LEVEL_KEY,
        ]
        .into_iter()
        .map(id)
        .collect();
        let envelope: Vec<ParamId> = (ENV_INITIAL..=ENV_TIME_VEL).map(id).collect();
        pages.push(
            Page::new(
                &format!("Osc {letter}"),
                LayoutNode::Row(vec![cell(), display(&extra), display(&envelope)]),
            )
            .with_section(section(&[
                id(OSC_ON),
                id(OSC_WAVE),
                id(OSC_COARSE),
                id(OSC_LEVEL),
            ])),
        );
    }
    let mut lfo_controls = vec![LFO_RANGE, LFO_RATE_KEY, LFO_AMT_VEL, LFO_RETRIGGER];
    lfo_controls.extend((0..OPERATORS as u32).map(|op| ParamId(LFO_OSC + op)));
    lfo_controls.extend([LFO_FILTER, LFO_AMOUNT_A, LFO_DEST_B, LFO_AMOUNT_B]);
    pages.push(
        Page::new(
            "LFO",
            LayoutNode::Row(vec![
                cell(),
                display(&lfo_controls),
                display(&envelope_of(LFO_ENV)),
            ]),
        )
        .with_section(section(&[LFO_ON, LFO_WAVE, LFO_RATE, LFO_AMOUNT])),
    );
    let filter_controls = [
        FILTER_TYPE,
        FILTER_CIRCUIT,
        FILTER_DRIVE,
        FILTER_MORPH,
        FREQ_VEL,
        FREQ_KEY,
        FREQ_ENV,
        SHAPER_TYPE,
        SHAPER_DRIVE,
        SHAPER_WET,
    ];
    pages.push(
        Page::new(
            "Filter",
            LayoutNode::Row(vec![
                cell(),
                display(&filter_controls),
                display(&envelope_of(FILTER_ENV)),
            ]),
        )
        .with_section(section(&[FILTER_ON, FILTER_FREQ, FILTER_RES])),
    );
    let mut pitch = envelope_of(PITCH_ENV);
    pitch.extend((0..OPERATORS as u32).map(|op| ParamId(PITCH_OSC + op)));
    pitch.extend([PITCH_LFO, PITCH_DEST_B, PITCH_AMOUNT_B]);
    pages.push(
        Page::new("Pitch", LayoutNode::Row(vec![cell(), display(&pitch)]))
            .with_section(section(&[PITCH_ON, PITCH_AMOUNT])),
    );
    DeviceLayout::new(LayoutNode::Row(vec![LayoutNode::Pages(PagesNode::new(
        "Sections",
        PageStyle::Shell,
        pages,
    ))]))
}

/// Operator's banks of eight for control surfaces (R19, decision 4): the
/// global controls, then each oscillator's own controls and envelope, the
/// LFO's, the filter's and the pitch section's.
fn banks() -> Vec<(String, Vec<String>)> {
    let names = |prefix: &str, what: &[&str]| -> Vec<String> {
        what.iter().map(|w| format!("{prefix}{w}")).collect()
    };
    let mut banks: Vec<(String, Vec<String>)> = vec![
        (
            "Global".to_owned(),
            names(
                "",
                &[
                    "Algorithm",
                    "Antialias",
                    "Interpolation",
                    "Tone",
                    "Time",
                    "Time < Key",
                    "Volume",
                    "Pan",
                ],
            ),
        ),
        (
            "Global 2".to_owned(),
            names(
                "",
                &[
                    "Pan < Key",
                    "Pan < Random",
                    "Transpose",
                    "Voices",
                    "Retrigger",
                    "PB Range",
                    "Glide",
                    "Glide Time",
                ],
            ),
        ),
        ("Global 3".to_owned(), vec!["Spread".to_owned()]),
    ];
    for letter in ["A", "B", "C", "D"] {
        let p = format!("{letter} ");
        banks.extend([
            (
                format!("Osc {letter}"),
                names(
                    &p,
                    &[
                        "On", "Wave", "Coarse", "Fine", "Level", "Fixed", "Freq", "Multi",
                    ],
                ),
            ),
            (
                format!("Osc {letter} 2"),
                names(
                    &p,
                    &[
                        "Feedback",
                        "Phase",
                        "Retrigger",
                        "Repeat",
                        "Osc < Vel",
                        "Q",
                        "Level < Vel",
                        "Level < Key",
                    ],
                ),
            ),
            (format!("Osc {letter} 3"), names(&p, &["Repeat On"])),
            (
                format!("Env {letter}"),
                names(
                    &p,
                    &[
                        "Initial",
                        "Peak",
                        "Sustain",
                        "Attack",
                        "Decay",
                        "Release",
                        "Loop",
                        "Loop Time",
                    ],
                ),
            ),
            (
                format!("Env {letter} 2"),
                names(&p, &["Env Repeat", "Time < Vel"]),
            ),
        ]);
    }
    for (title, prefix) in [("LFO", "LFO "), ("Filter", "Filter "), ("Pitch", "Pitch ")] {
        banks.extend([
            (
                format!("{title} Env"),
                names(
                    prefix,
                    &[
                        "Initial",
                        "Peak",
                        "Sustain",
                        "End",
                        "Attack",
                        "Decay",
                        "Release",
                        "Attack Slope",
                    ],
                ),
            ),
            (
                format!("{title} Env 2"),
                names(
                    prefix,
                    &[
                        "Decay Slope",
                        "Release Slope",
                        "Loop",
                        "Loop Time",
                        "Repeat",
                        "Time < Vel",
                    ],
                ),
            ),
        ]);
    }
    let owned = |names: &[&str]| names.iter().map(|n| (*n).to_owned()).collect::<Vec<_>>();
    banks.extend([
        (
            "LFO".to_owned(),
            owned(&[
                "LFO On",
                "LFO Wave",
                "LFO Range",
                "LFO Rate",
                "LFO Rate < Key",
                "LFO Amount",
                "LFO Amt < Vel",
                "LFO Retrigger",
            ]),
        ),
        (
            "LFO Destinations".to_owned(),
            owned(&[
                "LFO Osc A",
                "LFO Osc B",
                "LFO Osc C",
                "LFO Osc D",
                "LFO Filter",
                "LFO Amount A",
                "LFO Dest B",
                "LFO Amount B",
            ]),
        ),
        (
            "Filter".to_owned(),
            owned(&[
                "Filter On",
                "Filter Type",
                "Filter Circuit",
                "Filter Freq",
                "Filter Res",
                "Filter Drive",
                "Filter Morph",
                "Freq < Vel",
            ]),
        ),
        (
            "Filter 2".to_owned(),
            owned(&[
                "Freq < Key",
                "Freq < Envelope",
                "Shaper Type",
                "Shaper Drive",
                "Shaper Dry/Wet",
            ]),
        ),
    ]);
    for source in TARGET_SOURCES {
        banks.push((
            format!("{source} Targets"),
            ["Dest A", "Amount A", "Dest B", "Amount B"]
                .iter()
                .map(|what| format!("{source} {what}"))
                .collect(),
        ));
    }
    banks.extend([
        (
            "Pitch".to_owned(),
            [
                "Pitch Env On",
                "Pitch Env",
                "Pitch Osc A",
                "Pitch Osc B",
                "Pitch Osc C",
                "Pitch Osc D",
                "Pitch LFO",
                "Pitch Dest B",
            ]
            .map(str::to_owned)
            .to_vec(),
        ),
        ("Pitch 2".to_owned(), vec!["Pitch Amount B".to_owned()]),
    ]);
    banks
}

/// Oscillator `op`'s envelope as `value` reads its parameters (R12, R14).
pub(crate) fn osc_shape(value: &impl Fn(ParamId) -> f64, op: usize) -> Shape {
    let env = |c| value(osc(op, c));
    let gain = |c| env(c) as f32;
    Shape {
        initial: gain(ENV_INITIAL),
        peak: gain(ENV_PEAK),
        sustain: gain(ENV_SUSTAIN),
        end: 0.0,
        attack: (env(ENV_ATTACK) * 1e-3) as f32,
        decay: (env(ENV_DECAY) * 1e-3) as f32,
        release: (env(ENV_RELEASE) * 1e-3) as f32,
        curve: Curve::Amplitude,
        mode: Loop::of(env(ENV_LOOP) as usize),
        loop_secs: (env(ENV_LOOP_TIME) * 1e-3) as f32,
        repeat: envelope::REPEAT_BEATS[env(ENV_REPEAT) as usize],
    }
}

/// The envelope that begins at `base` ([`FILTER_ENV`], [`PITCH_ENV`] or
/// [`LFO_ENV`]) as `value` reads its parameters (R13, R14).
pub(crate) fn shaped_shape(value: &impl Fn(ParamId) -> f64, base: u32) -> Shape {
    let get = |c| value(shaped(base, c));
    let level = |c| (get(c) / 100.0) as f32;
    Shape {
        initial: level(SHAPED_INITIAL),
        peak: level(SHAPED_PEAK),
        sustain: level(SHAPED_SUSTAIN),
        end: level(SHAPED_END),
        attack: (get(SHAPED_ATTACK) * 1e-3) as f32,
        decay: (get(SHAPED_DECAY) * 1e-3) as f32,
        release: (get(SHAPED_RELEASE) * 1e-3) as f32,
        curve: Curve::Slopes([
            get(SHAPED_ATTACK_SLOPE) as f32,
            get(SHAPED_DECAY_SLOPE) as f32,
            get(SHAPED_RELEASE_SLOPE) as f32,
        ]),
        mode: Loop::of(get(SHAPED_LOOP) as usize),
        loop_secs: (get(SHAPED_LOOP_TIME) * 1e-3) as f32,
        repeat: envelope::REPEAT_BEATS[get(SHAPED_REPEAT) as usize],
    }
}

/// Spec: operator R25. The matrix: the five global sources, each with two
/// chosen slots (Destination A and B and their amounts), and the
/// destinations of [`DESTINATIONS`] after Off, each a movement of its own
/// travel that no parameter has.
#[must_use]
pub fn operator_matrix() -> MatrixLayout {
    let source = |name, range| SourceDef {
        name,
        range,
        scope: Scope::Voice,
    };
    MatrixLayout {
        sources: vec![
            source("Velocity", MatrixRange::Unipolar),
            source("Key", MatrixRange::Bipolar),
            source("Aftertouch", MatrixRange::Unipolar),
            source("Pitch Bend", MatrixRange::Bipolar),
            source("Mod Wheel", MatrixRange::Unipolar),
        ],
        dests: DESTINATIONS[1..]
            .iter()
            .map(|&name| DestDef {
                name,
                param: None,
                scope: Scope::Voice,
                rate: Rate::Audio,
                kind: MatrixKind::Additive,
            })
            .collect(),
        slots: (0..TARGET_SOURCES.len())
            .flat_map(|source| {
                [false, true].map(|b| SlotDef {
                    source: Pick::Fixed(source as u8),
                    dest: Pick::Chosen(target_dest(source, b)),
                    amount: target_amount(source, b),
                })
            })
            .collect(),
        forbidden: Vec::new(),
    }
}

/// Operator's device type.
#[must_use]
pub fn operator_type() -> DeviceType {
    let params = parameters();
    let layout = layout(&params);
    let banks = banks();
    let banks: Vec<(&str, Vec<&str>)> = banks
        .iter()
        .map(|(name, params)| (name.as_str(), params.iter().map(String::as_str).collect()))
        .collect();
    let banks: Vec<(&str, &[&str])> = banks.iter().map(|(n, p)| (*n, p.as_slice())).collect();
    DeviceType::native(OPERATOR, "Operator", DeviceRole::Instrument)
        .with_description(
            DeviceDescription::new(params)
                .with_layout(layout)
                .with_menu_options(&[NOTE_PB])
                .with_views(&crate::operator_display::VIEWS)
                .with_banks(&banks),
        )
        .with_matrix(operator_matrix())
}

/// A factory preset: a name and the parameters it sets, the rest staying at
/// their defaults (R22).
pub struct Preset {
    pub name: &'static str,
    pub values: Vec<(ParamId, f64)>,
}

/// Operator's factory presets, Beatlayer's own.
#[must_use]
pub fn presets() -> Vec<Preset> {
    let db = bl_model::param::db_to_gain;
    let on = |op: usize, level_db: f64, coarse: f64| {
        [
            (osc(op, OSC_LEVEL), db(level_db)),
            (osc(op, OSC_COARSE), coarse),
        ]
    };
    let decay = |op: usize, sustain_db: f64, decay_ms: f64, release_ms: f64| {
        [
            (osc(op, ENV_SUSTAIN), db(sustain_db)),
            (osc(op, ENV_DECAY), decay_ms),
            (osc(op, ENV_RELEASE), release_ms),
        ]
    };
    let mut presets = vec![
        Preset {
            name: "Plain Sine",
            values: vec![(ALGORITHM, 11.0)],
        },
        // A carrier and a modulator at 1:1 whose bend dies away, a classic
        // electric-piano-like bell with a soft tail.
        Preset {
            name: "Soft Keys",
            values: [(ALGORITHM, 1.0)]
                .into_iter()
                .chain(on(0, 0.0, 1.0))
                .chain(on(1, -12.0, 1.0))
                .chain(decay(0, -60.0, 1_800.0, 400.0))
                .chain(decay(1, -60.0, 600.0, 300.0))
                .collect(),
        },
        // A modulator an octave under the carrier with feedback: a growl.
        Preset {
            name: "FM Bass",
            values: [(ALGORITHM, 1.0), (osc(1, OSC_FEEDBACK), 40.0)]
                .into_iter()
                .chain(on(0, 0.0, 1.0))
                .chain(on(1, -10.0, 0.0))
                .chain(decay(0, -3.0, 400.0, 120.0))
                .chain(decay(1, -20.0, 300.0, 120.0))
                .collect(),
        },
        // Inharmonic ratios and a long decay: a struck bell.
        Preset {
            name: "Glass Bell",
            values: [
                (ALGORITHM, 7.0),
                (osc(1, OSC_FINE), 410.0),
                (osc(2, OSC_COARSE), 7.0),
            ]
            .into_iter()
            .chain(on(0, 0.0, 1.0))
            .chain(on(1, -14.0, 3.0))
            .chain(on(2, -20.0, 7.0))
            .chain(decay(0, -80.0, 4_000.0, 1_500.0))
            .chain(decay(1, -80.0, 1_500.0, 800.0))
            .chain(decay(2, -80.0, 600.0, 400.0))
            .collect(),
        },
        // The pitch envelope drops the sine through an octave and a half.
        Preset {
            name: "Pitch Drop",
            values: vec![
                (ALGORITHM, 11.0),
                (PITCH_ON, 1.0),
                (PITCH_AMOUNT, 36.0),
                (shaped(PITCH_ENV, SHAPED_PEAK), 100.0),
                (shaped(PITCH_ENV, SHAPED_INITIAL), 100.0),
                (shaped(PITCH_ENV, SHAPED_SUSTAIN), 0.0),
                (shaped(PITCH_ENV, SHAPED_DECAY), 120.0),
                (shaped(PITCH_ENV, SHAPED_DECAY_SLOPE), 0.6),
                (osc(0, ENV_SUSTAIN), db(-60.0)),
                (osc(0, ENV_DECAY), 450.0),
            ],
        },
    ];
    for preset in &mut presets {
        preset.values.sort_by_key(|(id, _)| id.0);
        preset.values.dedup_by_key(|(id, _)| id.0);
    }
    presets
}

/// Adds Operator to `registry`.
pub fn register(registry: &mut Registry) {
    add_with_data(registry, operator_type(), make, Some(Arc::new(device_data)));
    // Repeat shapes a user wave's table (R7).
    let repeats: Vec<ParamId> = (0..OPERATORS)
        .flat_map(|op| [osc(op, OSC_REPEAT), osc(op, OSC_REPEAT_ON)])
        .collect();
    registry.rebuilds_data_on(OPERATOR, &repeats);
}

/// The user waves' tables of `device`'s four oscillators (R7, R27), built
/// from its drawn partials and Repeat, off the audio thread: a new user
/// wave is the first partial alone.
#[must_use]
pub fn device_data(device: &Device, _file: bl_engine::FileOf<'_>) -> bl_engine::DeviceData {
    let mut data = bl_engine::DeviceData::default();
    for op in 0..OPERATORS {
        let levels = device
            .partials
            .iter()
            .find(|p| usize::from(p.osc) == op)
            .map_or_else(
                || {
                    let mut first = vec![0.0; bl_model::partials::COUNTS[0]];
                    first[0] = 1.0;
                    first
                },
                |p| p.levels.clone(),
            );
        let param = |c| device.params.get(&osc(op, c)).copied().unwrap_or(0.0);
        let repeat = (param(OSC_REPEAT_ON) >= 0.5).then(|| (param(OSC_REPEAT) / 100.0) as f32);
        let table = fm::user_table(&levels, repeat);
        data.items
            .push(Box::new(move |gc| gc.data(fm::UserWave { op, table })));
    }
    data
}

/// Makes an Operator for `config`'s rate, channels and block size, with
/// the Antialias `device` has, which sets its latency. Every buffer and
/// table it needs is made here, off the audio thread.
#[must_use]
pub fn make(device: &Device, config: &NodeConfig) -> Operator {
    let mut operator = Operator::new(config);
    let antialias = device.params.get(&ANTIALIAS).copied().unwrap_or(1.0);
    operator.synth.core.settings.antialias = antialias >= 0.5;
    operator
}

/// Operator: the voice section and the sound it plays.
pub struct Operator {
    section: Section,
    synth: Synth,
}

impl Operator {
    fn new(config: &NodeConfig) -> Self {
        let rate = config.sample_rate.as_f64();
        let max_frames = config.max_frames.max(1);
        Self {
            // Spec: operator R17, R26. Voices at 1 plays Mono, legato; PB
            // Range is the wheel's; the per-note bend is the menu option's,
            // over MPE's 48 semitones; Glide slides polyphonic notes.
            section: Section::new(
                Parts {
                    voices: Some(VOICES),
                    voices_one_is_mono: true,
                    always_legato: true,
                    glide_time: Some(GLIDE_TIME),
                    glide_on: Some(GLIDE_ON),
                    poly_glide: true,
                    retrigger: Some(RETRIGGER),
                    pb_range: Some(PB_RANGE),
                    note_pb: Some(NOTE_PB),
                    note_range: NoteRange::Mpe,
                    transpose: Some(TRANSPOSE),
                    ..Parts::default()
                },
                MAX_VOICES,
                1,
                rate,
                0x0BE7,
            ),
            synth: Synth::new(rate, max_frames),
        }
    }
}

impl NativeDevice for Operator {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        let frames = audio.frames();
        if frames == 0 || audio.channels() == 0 {
            return;
        }
        self.synth.snapshot(audio, params);
        self.synth.core.begin(frames);
        self.section.process(
            audio,
            params,
            &mut Play {
                synth: &mut self.synth,
                audio,
            },
        );
        let synth = &mut self.synth;
        synth.filter_and_shape(frames);
        let [left, right] = &mut synth.out;
        synth.core.finish(frames, left, right);
        for channel in 0..audio.channels() {
            let out = &synth.out[usize::from(channel == 1)];
            for ((sample, out), gain) in audio
                .channel_mut(channel)
                .iter_mut()
                .zip(out)
                .zip(&synth.volume)
            {
                *sample = out * gain;
            }
        }
    }

    fn flush(&mut self, _events: &[NodeEvent], _params: &ParamBank) {
        // A switched-off instrument is silent and holds no notes.
        self.section.reset();
        self.synth.silence();
    }

    fn receive_data(&mut self, data: NodeData) {
        self.synth.core.receive_data(&data);
        self.section.receive_data(&data);
    }

    fn latency(&self) -> u32 {
        self.synth.core.latency()
    }
}

/// The section's [`Instrument`] for one stretch: the synth and the audio
/// that has the song's place in it.
struct Play<'p, 'a, 'b> {
    synth: &'p mut Synth,
    audio: &'p Audio<'a, 'b>,
}

impl Instrument for Play<'_, '_, '_> {
    fn voice_event(&mut self, voices: &mut VoiceBlock, state: &State, event: VoiceEvent) {
        self.synth.voice_event(voices, state, event, self.audio);
    }

    fn render(&mut self, voices: &mut VoiceBlock, state: &State, frames: core::ops::Range<usize>) {
        self.synth.render(voices, state, frames);
    }
}

/// What the stretch's frame of one voice gives the core.
#[derive(Clone, Copy)]
struct Lane {
    amp: [f32; OPERATORS],
    /// The pitch envelope's move, and Osc Fixed Frequency's, in semitones.
    pitch: f32,
    fixed: f32,
    feedback: f32,
    /// FM Drive's factor on every index, and Tone's addition.
    drive: f32,
    tone: f32,
    pan: [f32; 2],
}

impl Default for Lane {
    fn default() -> Self {
        Self {
            amp: [0.0; OPERATORS],
            pitch: 0.0,
            fixed: 0.0,
            feedback: 0.0,
            drive: 1.0,
            tone: 0.0,
            pan: [0.0; 2],
        }
    }
}

/// The most fresh starts a stretch remembers for the filter; later ones play
/// from the voice's old filter state.
const MAX_STARTS: usize = 1024;

/// The seed the LFOs' random values start from.
const LFO_SEED: u64 = 0x1F0_0000;

/// The matrix's destinations: [`DESTINATIONS`] but Off.
const DESTS: usize = DESTINATIONS.len() - 1;

/// The places in [`DESTINATIONS`] that need their number, the others
/// being found by position (R25).
const DEST_CROSSFADE_AC: usize = 5;
const DEST_CROSSFADE_BD: usize = 6;
const DEST_DRIVE: usize = 9;
const DEST_FILTER_FREQ: usize = 10;
const DEST_FILTER_RES: usize = 11;
const DEST_FILTER_MORPH: usize = 12;
const DEST_FILTER_DRIVE: usize = 13;
const DEST_FILTER_ENV: usize = 14;
const DEST_SHAPER: usize = 15;
const DEST_LFO_RATE: usize = 16;
const DEST_LFO_AMOUNT: usize = 17;
const DEST_PITCH_ENV: usize = 18;
const DEST_TONE: usize = 21;
const DEST_TIME: usize = 22;

/// The LFO's settings for a stretch (R23).
#[derive(Clone, Copy, Default)]
struct LfoSettings {
    on: bool,
    wave: usize,
    range: usize,
    rate: f32,
    rate_key: f32,
    amount: f32,
    amt_vel: f32,
    retrigger: bool,
    to: [bool; OPERATORS],
    filter: bool,
    amount_a: f32,
    dest_b: usize,
    amount_b: f32,
}

/// Everything but the voice section.
struct Synth {
    rate: f64,
    /// Where each parameter is in the bank.
    slot: Vec<usize>,
    core: fm::Core,
    /// The filter at the operators' rate, for 1 and for Antialias's 2, and
    /// how many voices a frame of its buffers holds.
    filters: [InstrumentFilter; 2],
    stride: usize,
    matrix: Matrix,
    /// Each voice's envelopes: the oscillators', the pitch's, the filter's
    /// and the LFO's.
    envelopes: [[envelope::State; RUNNING]; MAX_VOICES],
    shapes: [Shape; RUNNING],
    /// Each voice's LFO, a free-running one every voice reads while LFO
    /// Retrigger is off, and how many notes have started: each LFO draws
    /// from a seed of its note's number.
    lfos: [Lfo; MAX_VOICES],
    shared: Lfo,
    started: u64,
    /// Each voice's Level < Vel and Level < Key factors, its time scales
    /// from Time < Key and each envelope's Time < Vel, and its pan from
    /// Pan < Key and Pan < Random.
    level_scale: [[f32; OPERATORS]; MAX_VOICES],
    time_scale: [[f32; RUNNING]; MAX_VOICES],
    pan_offset: [f32; MAX_VOICES],
    /// The pan each voice's gains were last worked out for.
    pan_cache: [(f32, [f32; 2]); MAX_VOICES],
    /// Each voice's velocity and its key in octaves above C3.
    velocity: [f32; MAX_VOICES],
    octaves: [f32; MAX_VOICES],
    /// The voices whose notes are held.
    gated: u32,
    /// The song's beat at each frame of the stretch, counted at the tempo
    /// from the first frame played, and where it stands between stretches.
    beats: Vec<f64>,
    clock: f64,
    bpm: Vec<f64>,
    /// The stretch's per-frame values: each oscillator's Level, Volume,
    /// Pan, how much Time slows an envelope, the free-running LFO, and the
    /// filter's and shaper's controls.
    level: [Vec<f32>; OPERATORS],
    volume: Vec<f32>,
    pan: Vec<f32>,
    speed: Vec<f32>,
    shared_lfo: Vec<f32>,
    cutoff_base: Vec<f32>,
    res_base: Vec<f32>,
    morph_base: Vec<f32>,
    drive_base: Vec<f32>,
    shaper_base: Vec<f32>,
    wet: Vec<f32>,
    /// The voices' filter and shaper controls at the operators' rate: per
    /// frame, `stride` voices.
    cutoff: Vec<f32>,
    res: Vec<f32>,
    morph: Vec<f32>,
    drive: Vec<f32>,
    shaper: Vec<f32>,
    /// The voices that started afresh in the stretch and the frame of the
    /// operators' rate each did, in order: the filter runs in pieces that
    /// begin where they do (R24).
    starts: Vec<(usize, usize)>,
    /// The matrix's movement of each destination for each voice.
    moves: [[f32; DESTS]; MAX_VOICES],
    /// The stretch's voices' values, `STRIDE` voices to a frame.
    lanes: Vec<Lane>,
    out: [Vec<f32>; 2],
    /// The stretch's settings.
    pitch_on: bool,
    pitch_amount: f32,
    pitch_to: [bool; OPERATORS],
    pitch_lfo: bool,
    destination: usize,
    amount_b: f32,
    fixed: [bool; OPERATORS],
    audible: u8,
    time_key: f32,
    level_vel: [f32; OPERATORS],
    level_key: [f32; OPERATORS],
    time_vel: [f32; RUNNING],
    pan_key: f32,
    pan_random: f32,
    lfo: LfoSettings,
    filter_on: bool,
    setup: Setup,
    shaper_kind: usize,
    freq_vel: f32,
    freq_key: f32,
    freq_env: f32,
    spread: f32,
}

/// Voices to a frame in [`Synth::lanes`].
const STRIDE: usize = MAX_VOICES;

impl Synth {
    fn new(rate: f64, max_frames: usize) -> Self {
        let params = parameters();
        let mut slot = vec![0; 512];
        for (index, param) in params.iter().enumerate() {
            slot[param.id.0 as usize] = index;
        }
        let shape = Shape {
            initial: 0.0,
            peak: 1.0,
            sustain: 1.0,
            end: 0.0,
            attack: 0.0,
            decay: 0.0,
            release: 0.0,
            curve: Curve::Amplitude,
            mode: Loop::None,
            loop_secs: 0.0,
            repeat: 1.0,
        };
        let filters = [1usize, 2]
            .map(|k| InstrumentFilter::new((rate * k as f64) as f32, MAX_VOICES, max_frames * k));
        let stride = filters[0].stride();
        let per_frame = |n: usize| vec![0.0f32; max_frames * n];
        Self {
            rate,
            slot,
            core: fm::Core::new(rate, max_frames, 0x0BE7, stride),
            filters,
            stride,
            matrix: Matrix::new(Arc::new(operator_matrix())),
            envelopes: [[envelope::State::DONE; RUNNING]; MAX_VOICES],
            shapes: [shape; RUNNING],
            lfos: core::array::from_fn(|voice| Lfo::new(voice as u64)),
            shared: Lfo::new(LFO_SEED),
            started: 0,
            level_scale: [[1.0; OPERATORS]; MAX_VOICES],
            time_scale: [[1.0; RUNNING]; MAX_VOICES],
            pan_offset: [0.0; MAX_VOICES],
            pan_cache: [(f32::NAN, [1.0; 2]); MAX_VOICES],
            velocity: [0.0; MAX_VOICES],
            octaves: [0.0; MAX_VOICES],
            gated: 0,
            beats: vec![0.0; max_frames + 1],
            clock: 0.0,
            bpm: vec![0.0; max_frames],
            level: core::array::from_fn(|_| per_frame(1)),
            volume: vec![1.0; max_frames],
            pan: per_frame(1),
            speed: vec![1.0; max_frames],
            shared_lfo: per_frame(1),
            cutoff_base: per_frame(1),
            res_base: per_frame(1),
            morph_base: per_frame(1),
            drive_base: per_frame(1),
            shaper_base: per_frame(1),
            wet: per_frame(1),
            cutoff: per_frame(2 * stride),
            res: per_frame(2 * stride),
            morph: per_frame(2 * stride),
            drive: per_frame(2 * stride),
            shaper: per_frame(2 * stride),
            starts: Vec::with_capacity(MAX_STARTS),
            moves: [[0.0; DESTS]; MAX_VOICES],
            lanes: vec![Lane::default(); max_frames * STRIDE],
            out: [vec![0.0; max_frames], vec![0.0; max_frames]],
            pitch_on: false,
            pitch_amount: 0.0,
            pitch_to: [false; OPERATORS],
            pitch_lfo: false,
            destination: 0,
            amount_b: 0.0,
            fixed: [false; OPERATORS],
            audible: 0,
            time_key: 0.0,
            level_vel: [0.0; OPERATORS],
            level_key: [0.0; OPERATORS],
            time_vel: [0.0; RUNNING],
            pan_key: 0.0,
            pan_random: 0.0,
            lfo: LfoSettings::default(),
            filter_on: false,
            setup: Setup::new(1, false, 0),
            shaper_kind: 0,
            freq_vel: 0.0,
            freq_key: 0.0,
            freq_env: 0.0,
            spread: 0.0,
        }
    }

    fn silence(&mut self) {
        self.core.reset();
        self.envelopes = [[envelope::State::DONE; RUNNING]; MAX_VOICES];
        self.gated = 0;
        self.pan_cache = [(f32::NAN, [1.0; 2]); MAX_VOICES];
    }

    /// Reads the stretch's parameters (R11 to R17, R23 to R26).
    fn snapshot(&mut self, audio: &Audio<'_, '_>, params: &ParamBank) {
        let frames = audio.frames();
        let value = |id: ParamId| params.value(self.slot[id.0 as usize]);
        let ramp = |id: ParamId| params.ramp(self.slot[id.0 as usize]);
        let settings = &mut self.core.settings;
        settings.algorithm = value(ALGORITHM) as usize - 1;
        settings.antialias = value(ANTIALIAS) >= 0.5;
        settings.interpolation = value(INTERPOLATION) >= 0.5;
        settings.tone = (value(TONE) / 100.0) as f32;
        self.audible = 0;
        for (op, s) in settings.ops.iter_mut().enumerate() {
            let control = |c| value(osc(op, c));
            s.on = control(OSC_ON) >= 0.5;
            let coarse = control(OSC_COARSE);
            s.coarse = if coarse < 1.0 { 0.5 } else { coarse as f32 };
            s.fine = control(OSC_FINE) as f32;
            s.fixed = control(OSC_FIXED) >= 0.5;
            s.freq = control(OSC_FREQ) as f32;
            s.multi = MULTIS[control(OSC_MULTI) as usize] as f32;
            s.vel = (control(OSC_VEL) / 100.0) as f32;
            s.quantise = control(OSC_Q) >= 0.5;
            // The level is the device's: it is smoothed frame by frame.
            s.level = 1.0;
            s.wave = control(OSC_WAVE) as usize;
            s.feedback = (control(OSC_FEEDBACK) / 100.0) as f32;
            s.phase = (control(OSC_PHASE) / 360.0) as f32;
            s.retrigger = control(OSC_RETRIGGER) >= 0.5;
            self.fixed[op] = s.fixed;
            self.level_vel[op] = (control(OSC_LEVEL_VEL) / 100.0) as f32;
            self.level_key[op] = (control(OSC_LEVEL_KEY) / 100.0) as f32;
            self.level[op][..frames].copy_from_slice(ramp(osc(op, OSC_LEVEL)));
            if s.on && self.level[op][frames - 1] > 0.0 {
                self.audible |= 1 << op;
            }
        }
        for op in 0..OPERATORS {
            self.shapes[op] = osc_shape(&value, op);
            self.time_vel[op] = (value(osc(op, ENV_TIME_VEL)) / 100.0) as f32;
        }
        for (index, base) in [(PITCH, PITCH_ENV), (FILTER, FILTER_ENV), (LFO, LFO_ENV)] {
            self.shapes[index] = shaped_shape(&value, base);
            self.time_vel[index] = (value(shaped(base, SHAPED_TIME_VEL)) / 100.0) as f32;
        }
        self.pitch_on = value(PITCH_ON) >= 0.5;
        self.pitch_amount = (value(PITCH_AMOUNT) / 100.0) as f32;
        for op in 0..OPERATORS {
            self.pitch_to[op] = value(ParamId(PITCH_OSC + op as u32)) >= 0.5;
        }
        self.pitch_lfo = value(PITCH_LFO) >= 0.5;
        self.matrix.begin(params);
        self.starts.clear();
        self.destination = value(PITCH_DEST_B) as usize;
        self.amount_b = (value(PITCH_AMOUNT_B) / 100.0) as f32;
        self.time_key = (value(TIME_KEY) / 100.0) as f32;
        self.pan_key = (value(PAN_KEY) / 100.0) as f32;
        self.pan_random = (value(PAN_RANDOM) / 100.0) as f32;
        self.spread = (value(SPREAD) / 100.0) as f32;
        self.lfo = LfoSettings {
            on: value(LFO_ON) >= 0.5,
            wave: value(LFO_WAVE) as usize,
            range: value(LFO_RANGE) as usize,
            rate: value(LFO_RATE) as f32,
            rate_key: (value(LFO_RATE_KEY) / 100.0) as f32,
            amount: (value(LFO_AMOUNT) / 100.0) as f32,
            amt_vel: (value(LFO_AMT_VEL) / 100.0) as f32,
            retrigger: value(LFO_RETRIGGER) >= 0.5,
            to: core::array::from_fn(|op| value(ParamId(LFO_OSC + op as u32)) >= 0.5),
            filter: value(LFO_FILTER) >= 0.5,
            amount_a: (value(LFO_AMOUNT_A) / 100.0) as f32,
            dest_b: value(LFO_DEST_B) as usize,
            amount_b: (value(LFO_AMOUNT_B) / 100.0) as f32,
        };
        // An oscillator at Level 0 that Destination B or a global target
        // raises can sound, so its voice outlasts the note's end.
        for op in 0..OPERATORS {
            let raised = self.matrix.has(DEST_VOLUME_A - 1 + op)
                || (self.pitch_on
                    && self.amount_b != 0.0
                    && self.destination == DEST_VOLUME_A + op)
                || (self.lfo.dest_b == DEST_VOLUME_A + op && self.lfo.amount_b != 0.0);
            if raised && self.core.settings.ops[op].on {
                self.audible |= 1 << op;
            }
        }
        let entry = (value(FILTER_TYPE) as usize).min(FILTER_TYPE_CHOICES.len() - 1);
        self.setup = Setup::new(entry / 2, entry % 2 == 1, value(FILTER_CIRCUIT) as usize);
        self.filter_on = value(FILTER_ON) >= 0.5;
        self.shaper_kind = value(SHAPER_TYPE) as usize;
        self.freq_vel = (value(FREQ_VEL) / 100.0) as f32;
        self.freq_key = (value(FREQ_KEY) / 100.0) as f32;
        self.freq_env = (value(FREQ_ENV) / 100.0) as f32;
        // Per frame: Volume, Pan, and Time (R15), which slows an envelope
        // by 2^(4 Time); the filter's and the shaper's controls.
        self.volume[..frames].copy_from_slice(ramp(VOLUME));
        for (pan, p) in self.pan[..frames].iter_mut().zip(ramp(PAN)) {
            *pan = p / 100.0;
        }
        for (speed, t) in self.speed[..frames].iter_mut().zip(ramp(TIME)) {
            *speed = (-TIME_OCTAVES * t / 100.0).exp2();
        }
        for (to, hz) in self.cutoff_base[..frames].iter_mut().zip(ramp(FILTER_FREQ)) {
            *to = pitch_of(*hz, 440.0);
        }
        for (to, id, scale) in [
            (&mut self.res_base, FILTER_RES, 100.0),
            (&mut self.morph_base, FILTER_MORPH, 100.0),
            (&mut self.drive_base, FILTER_DRIVE, 24.0),
            (&mut self.shaper_base, SHAPER_DRIVE, 24.0),
            (&mut self.wet, SHAPER_WET, 100.0),
        ] {
            for (to, from) in to[..frames].iter_mut().zip(ramp(id)) {
                *to = from / scale;
            }
        }
        // The song's beat and tempo at each frame, counted at the tempo
        // (R14), and the free-running LFO (R23).
        self.beats[0] = self.clock;
        for i in 0..frames {
            self.bpm[i] = audio.bpm_at(i);
            self.beats[i + 1] = self.beats[i] + self.bpm[i] / 60.0 / self.rate;
            self.shared_lfo[i] = if self.lfo.on && !self.lfo.retrigger {
                let dt = lfo::cycles(
                    self.lfo.range,
                    f64::from(self.lfo.rate),
                    self.rate,
                    self.bpm[i],
                );
                self.shared.step(self.lfo.wave, dt)
            } else {
                0.0
            };
        }
        self.clock = self.beats[frames];
    }

    /// The beats from the note's frame to the song's next sixteenth, for
    /// Sync (R14); none while the song isn't playing.
    fn wait_for_grid(audio: &Audio<'_, '_>, frame: usize) -> Option<f64> {
        if !audio.playing() {
            return None;
        }
        let at = bl_core::time::Frames(audio.song_frame(frame));
        let sixteenths = audio
            .tempo()
            .frames_to_ticks(at, audio.sample_rate())
            .as_quarters()
            * 4.0;
        Some(((sixteenths + 1e-9).floor() + 1.0 - sixteenths) / 4.0)
    }

    fn voice_event(
        &mut self,
        voices: &mut VoiceBlock,
        state: &State,
        event: VoiceEvent,
        audio: &Audio<'_, '_>,
    ) {
        let voice = event.voice;
        let bit = 1u32 << voice;
        match event.kind {
            VoiceEventKind::Start { start, .. } => {
                // Spec: operator R26. A note keeps the Spread it starts with.
                self.core.start(voice, start, self.spread);
                let beats = self.beats[event.time as usize];
                let wait = Self::wait_for_grid(audio, event.time as usize);
                let sources = Sources::of(voices);
                let velocity = sources.velocity(voice) as f32;
                let octaves = (f32::from(voices.info(voice).key) - MIDDLE_KEY) / 12.0;
                self.velocity[voice] = velocity;
                self.octaves[voice] = octaves;
                if !matches!(start, Start::Legato) {
                    self.started += 1;
                    if self.lfo.retrigger {
                        self.lfos[voice].restart(0.0, LFO_SEED ^ self.started);
                    }
                }
                if self.filter_on && matches!(start, Start::Fresh) && self.starts.len() < MAX_STARTS
                {
                    self.starts
                        .push((voice, event.time as usize * self.core.factor()));
                }
                for op in 0..OPERATORS {
                    // Spec: operator R11. Level < Vel's law is polysynth
                    // R15's; Level < Key is up to 6 dB an octave.
                    let a = self.level_vel[op];
                    self.level_scale[voice][op] =
                        (1.0 - a + a * velocity) * (self.level_key[op] * octaves).exp2();
                }
                for e in 0..RUNNING {
                    // Spec: operator R15.
                    self.time_scale[voice][e] = (self.time_key * octaves
                        + 2.0 * self.time_vel[e] * (velocity - 0.5))
                        .exp2();
                    match start {
                        Start::Fresh => {
                            self.envelopes[voice][e].start(&self.shapes[e], beats, wait);
                        }
                        Start::Legato => {}
                        _ => self.envelopes[voice][e].restart(&self.shapes[e], beats, wait),
                    }
                }
                self.pan_offset[voice] = self.pan_key * sources.key(voice) as f32
                    + self.pan_random * (2.0 * state.random(voice) as f32 - 1.0);
                self.gated |= bit;
            }
            VoiceEventKind::Release => {
                self.gated &= !bit;
                for e in 0..RUNNING {
                    self.envelopes[voice][e].release(&self.shapes[e]);
                }
            }
            VoiceEventKind::End { .. } => {
                self.gated &= !bit;
                self.envelopes[voice] = [envelope::State::DONE; RUNNING];
                self.pan_cache[voice].0 = f32::NAN;
            }
        }
    }

    /// Works out the matrix's movement of every destination for each
    /// sounding voice, from its sources (R25).
    fn read_matrix(&mut self, voices: &VoiceBlock) {
        let sources = Sources::of(voices);
        for voice in voices.active_voices() {
            let values = [
                sources.velocity(voice),
                sources.key(voice),
                sources.pressure(voice),
                sources.pitch_bend(),
                sources.mod_wheel(),
            ];
            for dest in 0..DESTS {
                self.moves[voice][dest] = if self.matrix.has(dest) {
                    self.matrix
                        .add(dest, 0.0, voice, 0, &|source: usize, _: usize, _: usize| {
                            values[source]
                        }) as f32
                } else {
                    0.0
                };
            }
        }
    }

    fn render(&mut self, voices: &mut VoiceBlock, state: &State, frames: core::ops::Range<usize>) {
        let count = frames.len();
        if count == 0 {
            return;
        }
        let frame_secs = (1.0 / self.rate) as f32;
        let factor = self.core.factor();
        self.read_matrix(voices);
        for f in 0..count {
            let i = frames.start + f;
            let beats = self.beats[i];
            for voice in voices.active_voices() {
                let m = self.moves[voice];
                let moved = |dest: usize| m[dest - 1];
                let mut lane = Lane::default();
                let mut amp = [0.0f32; OPERATORS];
                // Time, and the matrix's movement of it, scale every
                // envelope's rate (R15, R25).
                let speed = self.speed[i] * (-TIME_OCTAVES * moved(DEST_TIME)).exp2();
                for (op, amp) in amp.iter_mut().enumerate() {
                    let tick = frame_secs * speed * self.time_scale[voice][op];
                    *amp = self.envelopes[voice][op].step(&self.shapes[op], tick, beats);
                }
                let mut shaped = [0.0f32; 3];
                for (value, index) in shaped.iter_mut().zip([PITCH, FILTER, LFO]) {
                    let tick = frame_secs * speed * self.time_scale[voice][index];
                    *value = self.envelopes[voice][index].step(&self.shapes[index], tick, beats);
                }
                let [pitch_env, filter_env, lfo_env] = shaped;
                let pitch_amount = (self.pitch_amount + moved(DEST_PITCH_ENV)).clamp(-1.0, 1.0);
                let (shift, second) = if self.pitch_on {
                    (pitch_env * pitch_amount, pitch_env * self.amount_b)
                } else {
                    (0.0, 0.0)
                };
                lane.pitch = shift * PITCH_SEMITONES;
                // Spec: operator R23. The LFO, at its rate moved by Rate <
                // Key, the pitch envelope and the matrix, scaled by its
                // Amount, Amt < Vel and envelope.
                let lfo_amount = (self.lfo.amount + moved(DEST_LFO_AMOUNT)).clamp(0.0, 1.0);
                let lfo = &self.lfo;
                let wave = if !lfo.on || lfo_amount == 0.0 {
                    0.0
                } else if lfo.retrigger {
                    let rate = (lfo.rate + 100.0 * moved(DEST_LFO_RATE)).clamp(0.0, 100.0);
                    let key = if lfo.range == lfo::SYNC {
                        1.0
                    } else {
                        (lfo.rate_key * self.octaves[voice]).exp2()
                    };
                    let pitch = if self.pitch_lfo {
                        lane.pitch / 12.0
                    } else {
                        0.0
                    };
                    let dt = lfo::cycles(lfo.range, f64::from(rate), self.rate, self.bpm[i])
                        * f64::from(key * pitch.exp2());
                    self.lfos[voice].step(lfo.wave, dt)
                } else {
                    self.shared_lfo[i]
                };
                let a = lfo.amt_vel;
                let depth = lfo_amount * (1.0 - a + a * self.velocity[voice]) * lfo_env;
                let lfo_value = wave * depth;
                // What every destination is moved by: the matrix, the pitch
                // envelope's Destination B and the LFO's.
                let mut e = [0.0f32; DESTS + 1];
                e[1..].copy_from_slice(&m);
                e[self.destination] += second;
                e[lfo.dest_b] += lfo_value * lfo.amount_b;
                let mut scale = [1.0f32; OPERATORS];
                for (scale, to) in scale.iter_mut().zip(lfo.to) {
                    if to {
                        // Tremolo: the level swings between its whole and
                        // none, by the LFO's depth and Amount A.
                        let high = (wave + 1.0) / 2.0;
                        let high = if lfo.amount_a >= 0.0 {
                            high
                        } else {
                            1.0 - high
                        };
                        *scale = 1.0 - depth.abs() * lfo.amount_a.abs() * (1.0 - high);
                    }
                }
                // Osc Crossfade moves level from the first to the second.
                for (from, to, dest) in [(0, 2, DEST_CROSSFADE_AC), (1, 3, DEST_CROSSFADE_BD)] {
                    scale[from] *= (1.0 - e[dest]).clamp(0.0, 1.0);
                    scale[to] *= (1.0 + e[dest]).clamp(0.0, 1.0);
                }
                lane.feedback = e[DEST_FEEDBACK];
                lane.fixed = e[DEST_FIXED] * PITCH_SEMITONES;
                lane.drive = (1.0 + DRIVE_REACH * e[DEST_DRIVE]).max(0.0);
                lane.tone = e[DEST_TONE];
                let volume = (1.0 + e[DEST_VOLUME]).max(0.0);
                let pan = self.pan[i] + self.pan_offset[voice] + e[DEST_PANORAMA];
                for op in 0..OPERATORS {
                    let level = (self.level[op][i] + e[DEST_VOLUME_A + op]).clamp(0.0, 1.0);
                    lane.amp[op] = level * amp[op] * self.level_scale[voice][op] * scale[op];
                }
                let pan = pan.clamp(-1.0, 1.0);
                let cache = &mut self.pan_cache[voice];
                if cache.0 != pan {
                    let (left, right) = pan_gains(pan);
                    *cache = (pan, [left, right]);
                }
                lane.pan = [cache.1[0] * volume, cache.1[1] * volume];
                self.lanes[f * STRIDE + voice] = lane;
                if self.filter_on || self.shaper_kind != 0 {
                    // Spec: operator R24. The cutoff moves with the key,
                    // velocity, the filter's envelope and the LFO.
                    let morph = self.setup.kind == Kind::Morph;
                    let reach = (self.freq_env + e[DEST_FILTER_ENV]).clamp(-1.0, 1.0);
                    let lfo_cutoff = if lfo.filter {
                        lfo_value * lfo.amount_a * LFO_FILTER_SEMITONES
                    } else {
                        0.0
                    };
                    let cutoff = (self.cutoff_base[i]
                        + self.freq_key * self.octaves[voice] * 12.0
                        + self.freq_vel * self.velocity[voice] * VEL_FILTER_SEMITONES
                        + reach * filter_env * ENV_FILTER_SEMITONES
                        + lfo_cutoff
                        + e[DEST_FILTER_FREQ] * TRAVEL_SEMITONES)
                        .clamp(-24.0, 150.0);
                    let res = (self.res_base[i] + e[DEST_FILTER_RES]).clamp(0.0, 1.0);
                    let morph_at = (self.morph_base[i]
                        + if morph { e[DEST_FILTER_MORPH] } else { 0.0 })
                    .clamp(0.0, 1.0);
                    let drive = (self.drive_base[i]
                        + if morph { 0.0 } else { e[DEST_FILTER_DRIVE] })
                    .clamp(0.0, 1.0);
                    let shaper = (self.shaper_base[i] + e[DEST_SHAPER]).clamp(0.0, 1.0);
                    for sub in 0..factor {
                        let at = (i * factor + sub) * self.stride + voice;
                        self.cutoff[at] = cutoff;
                        self.res[at] = res;
                        self.morph[at] = morph_at;
                        self.drive[at] = drive;
                        self.shaper[at] = shaper;
                    }
                }
            }
        }
        let lanes = &self.lanes;
        let (pitch_to, fixed) = (self.pitch_to, self.fixed);
        self.core.render(
            voices,
            state,
            frames,
            |voice, op, f| {
                let lane = &lanes[f * STRIDE + voice];
                Moves {
                    gain: lane.amp[op],
                    semitones: if pitch_to[op] { lane.pitch } else { 0.0 }
                        + if fixed[op] { lane.fixed } else { 0.0 },
                    feedback: lane.feedback,
                    drive: lane.drive,
                    tone: lane.tone,
                }
            },
            |voice, f| lanes[f * STRIDE + voice].pan,
        );
        // Spec: operator R12. A voice ends when its note has and every
        // carrier's envelope has reached silence.
        let carriers = fm::carriers(self.core.settings.algorithm) & self.audible;
        let mut mask = voices.active_mask();
        while mask != 0 {
            let voice = mask.trailing_zeros() as usize;
            mask &= mask - 1;
            let done = (0..OPERATORS)
                .filter(|op| carriers >> op & 1 == 1)
                .all(|op| self.envelopes[voice][op].is_done());
            if self.gated >> voice & 1 == 0 && done {
                voices.end(voice);
            }
        }
    }

    /// The filter and then the shaper over every voice's audio of the
    /// stretch (R24), before the voices are summed.
    fn filter_and_shape(&mut self, frames: usize) {
        let factor = self.core.factor();
        let n = frames * factor;
        let stride = self.stride;
        let (audio, hi) = self.core.voices_mut(frames);
        if hi == 0 {
            return;
        }
        if self.filter_on {
            let filter = &mut self.filters[factor - 1];
            filter.set_setup(self.setup);
            // A voice's filter starts afresh at the first frame of a call,
            // so each piece begins where a voice does.
            let (mut from, mut next) = (0, 0);
            while from < n {
                while next < self.starts.len() && self.starts[next].1 <= from {
                    filter.reset_voice(self.starts[next].0, 0);
                    next += 1;
                }
                let to = self.starts.get(next).map_or(n, |start| start.1.min(n));
                let span = from * stride..to * stride;
                let controls = PerFrame {
                    cutoff: &self.cutoff[span.clone()],
                    res: Per::Voice(&self.res[span.clone()]),
                    morph: Per::Voice(&self.morph[span.clone()]),
                    drive: Per::Voice(&self.drive[span]),
                };
                let audio = &mut audio[from * 2 * stride..to * 2 * stride];
                filter.process(hi, to - from, &controls, audio);
                from = to;
            }
        }
        if self.shaper_kind == 0 {
            return;
        }
        let kind = self.shaper_kind - 1;
        for at in 0..n {
            let wet = self.wet[at / factor];
            if wet <= 0.0 {
                continue;
            }
            for channel in 0..2 {
                let from = (at * 2 + channel) * stride;
                for voice in 0..hi {
                    let amount = self.shaper[at * stride + voice];
                    let x = &mut audio[from + voice];
                    *x += (shape(kind, amount, *x) - *x) * wet;
                }
            }
        }
    }
}

//! Operator, part 1 (WP-8.12; spec: operator R11 to R22): four oscillators
//! on the FM core ([`crate::fm`]) with their envelopes and the pitch
//! envelope ([`crate::operator_envelope`]), Time and Tone, the voice
//! section's Voices, Transpose, Retrigger and per-note pitch bend, and the
//! body as a shell and display (device framework R41).
//!
//! Part 2 adds the LFO, the filter and shaper, the global modulation
//! targets, Glide and Spread, and the displays; the filter's and the LFO's
//! envelopes are parameters now, so the pages that will hold their other
//! controls exist, and they run when those controls do.
//!
//! The manual records few ranges or defaults, so those are Beatlayer's own
//! and are listed in `docs/specs/operator.md`.

use bl_engine::{DeviceType, NodeConfig, NodeData, NodeEvent, Registry};
use bl_model::device_layout::{
    BODY_HEIGHT, ControlBand, LayoutNode, Page, PageStyle, PagesNode, TAB_HEIGHT, Widget,
};
use bl_model::{
    Device, DeviceDescription, DeviceLayout, DeviceRole, ParamDescriptor, ParamId, ParamUnit,
};
use lspk_voice::unison::pan_gains;
use lspk_voice::{MAX_VOICES, Start, VoiceBlock, VoiceEvent, VoiceEventKind};

use crate::fm::{self, Moves, OPERATORS};
use crate::framework::{Audio, NativeDevice, ParamBank, add};
use crate::instrument::{self, Instrument, NoteRange, Parts, Section, Sources, State};
use crate::operator_envelope::{self as envelope, Curve, Loop, Shape};

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

/// Spec: operator R15. Time's reach in octaves of time at 100 %.
const TIME_OCTAVES: f32 = 4.0;

/// The middle of the keys: C3, which Level < Key, Time < Key and Pan < Key
/// count from.
const MIDDLE_KEY: f32 = 60.0;

/// The envelopes that run in part 1: the oscillators' and the pitch's.
const RUNNING: usize = OPERATORS + 1;
const PITCH: usize = OPERATORS;

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
    let mut pages = vec![
        Page::new(
            "Global",
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
                LayoutNode::Row(vec![display(&extra), display(&envelope)]),
            )
            .with_section(section(&[
                id(OSC_ON),
                id(OSC_WAVE),
                id(OSC_COARSE),
                id(OSC_LEVEL),
            ])),
        );
    }
    pages.push(Page::new("LFO", display(&envelope_of(LFO_ENV))));
    pages.push(Page::new("Filter", display(&envelope_of(FILTER_ENV))));
    let mut pitch = envelope_of(PITCH_ENV);
    pitch.extend((0..OPERATORS as u32).map(|op| ParamId(PITCH_OSC + op)));
    pitch.extend([PITCH_LFO, PITCH_DEST_B, PITCH_AMOUNT_B]);
    pages
        .push(Page::new("Pitch", display(&pitch)).with_section(section(&[PITCH_ON, PITCH_AMOUNT])));
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
                ],
            ),
        ),
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
    DeviceType::native(OPERATOR, "Operator", DeviceRole::Instrument).with_description(
        DeviceDescription::new(params)
            .with_layout(layout)
            .with_menu_options(&[NOTE_PB])
            .with_banks(&banks),
    )
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
    add(registry, operator_type(), make);
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
        Self {
            // Spec: operator R17. Voices at 1 plays Mono, legato; PB Range
            // is the wheel's; the per-note bend is the menu option's, over
            // MPE's 48 semitones.
            section: Section::new(
                Parts {
                    voices: Some(VOICES),
                    voices_one_is_mono: true,
                    always_legato: true,
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
            synth: Synth::new(rate, config.max_frames.max(1)),
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
#[derive(Clone, Copy, Default)]
struct Lane {
    amp: [f32; OPERATORS],
    /// The pitch envelope's move, and Osc Fixed Frequency's, in semitones.
    pitch: f32,
    fixed: f32,
    feedback: f32,
    pan: [f32; 2],
}

/// Everything but the voice section.
struct Synth {
    rate: f64,
    /// Where each parameter is in the bank.
    slot: Vec<usize>,
    core: fm::Core,
    /// Each voice's envelopes: the oscillators' and the pitch's.
    envelopes: [[envelope::State; RUNNING]; MAX_VOICES],
    shapes: [Shape; RUNNING],
    /// Each voice's Level < Vel and Level < Key factors, its time scales
    /// from Time < Key and each envelope's Time < Vel, and its pan from
    /// Pan < Key and Pan < Random.
    level_scale: [[f32; OPERATORS]; MAX_VOICES],
    time_scale: [[f32; RUNNING]; MAX_VOICES],
    pan_offset: [f32; MAX_VOICES],
    /// The pan each voice's gains were last worked out for.
    pan_cache: [(f32, [f32; 2]); MAX_VOICES],
    /// The voices whose notes are held.
    gated: u32,
    /// The song's beat at each frame of the stretch, counted at the tempo
    /// from the first frame played, and where it stands between stretches.
    beats: Vec<f64>,
    clock: f64,
    /// The stretch's per-frame values: each oscillator's Level, Volume,
    /// Pan, and how much Time slows an envelope.
    level: [Vec<f32>; OPERATORS],
    volume: Vec<f32>,
    pan: Vec<f32>,
    speed: Vec<f32>,
    /// The stretch's voices' values, `STRIDE` voices to a frame.
    lanes: Vec<Lane>,
    out: [Vec<f32>; 2],
    /// The stretch's settings.
    pitch_on: bool,
    pitch_amount: f32,
    pitch_to: [bool; OPERATORS],
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
}

/// Voices to a frame in [`Synth::lanes`].
const STRIDE: usize = MAX_VOICES;

impl Synth {
    fn new(rate: f64, max_frames: usize) -> Self {
        let params = parameters();
        let mut slot = vec![0; 256];
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
        Self {
            rate,
            slot,
            core: fm::Core::new(rate, max_frames, 0x0BE7),
            envelopes: [[envelope::State::DONE; RUNNING]; MAX_VOICES],
            shapes: [shape; RUNNING],
            level_scale: [[1.0; OPERATORS]; MAX_VOICES],
            time_scale: [[1.0; RUNNING]; MAX_VOICES],
            pan_offset: [0.0; MAX_VOICES],
            pan_cache: [(f32::NAN, [1.0; 2]); MAX_VOICES],
            gated: 0,
            beats: vec![0.0; max_frames + 1],
            clock: 0.0,
            level: core::array::from_fn(|_| vec![0.0; max_frames]),
            volume: vec![1.0; max_frames],
            pan: vec![0.0; max_frames],
            speed: vec![1.0; max_frames],
            lanes: vec![Lane::default(); max_frames * STRIDE],
            out: [vec![0.0; max_frames], vec![0.0; max_frames]],
            pitch_on: false,
            pitch_amount: 0.0,
            pitch_to: [false; OPERATORS],
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
        }
    }

    fn silence(&mut self) {
        self.core.reset();
        self.envelopes = [[envelope::State::DONE; RUNNING]; MAX_VOICES];
        self.gated = 0;
        self.pan_cache = [(f32::NAN, [1.0; 2]); MAX_VOICES];
    }

    /// Reads the stretch's parameters (R11 to R17).
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
            let env = |c| value(osc(op, c));
            let gain = |c| env(c) as f32;
            self.shapes[op] = Shape {
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
            };
            self.time_vel[op] = (env(ENV_TIME_VEL) / 100.0) as f32;
        }
        let pitch = |c| value(shaped(PITCH_ENV, c));
        let level = |c| (pitch(c) / 100.0) as f32;
        self.shapes[PITCH] = Shape {
            initial: level(SHAPED_INITIAL),
            peak: level(SHAPED_PEAK),
            sustain: level(SHAPED_SUSTAIN),
            end: level(SHAPED_END),
            attack: (pitch(SHAPED_ATTACK) * 1e-3) as f32,
            decay: (pitch(SHAPED_DECAY) * 1e-3) as f32,
            release: (pitch(SHAPED_RELEASE) * 1e-3) as f32,
            curve: Curve::Slopes([
                pitch(SHAPED_ATTACK_SLOPE) as f32,
                pitch(SHAPED_DECAY_SLOPE) as f32,
                pitch(SHAPED_RELEASE_SLOPE) as f32,
            ]),
            mode: Loop::of(pitch(SHAPED_LOOP) as usize),
            loop_secs: (pitch(SHAPED_LOOP_TIME) * 1e-3) as f32,
            repeat: envelope::REPEAT_BEATS[pitch(SHAPED_REPEAT) as usize],
        };
        self.time_vel[PITCH] = (pitch(SHAPED_TIME_VEL) / 100.0) as f32;
        self.pitch_on = value(PITCH_ON) >= 0.5;
        self.pitch_amount = (value(PITCH_AMOUNT) / 100.0) as f32;
        for op in 0..OPERATORS {
            self.pitch_to[op] = value(ParamId(PITCH_OSC + op as u32)) >= 0.5;
        }
        self.destination = value(PITCH_DEST_B) as usize;
        self.amount_b = (value(PITCH_AMOUNT_B) / 100.0) as f32;
        // An oscillator at Level 0 that Destination B raises can sound, so
        // its voice outlasts the note's end.
        if self.pitch_on
            && self.amount_b != 0.0
            && (DEST_VOLUME_A..DEST_VOLUME_A + OPERATORS).contains(&self.destination)
        {
            let op = self.destination - DEST_VOLUME_A;
            if self.core.settings.ops[op].on {
                self.audible |= 1 << op;
            }
        }
        self.time_key = (value(TIME_KEY) / 100.0) as f32;
        self.pan_key = (value(PAN_KEY) / 100.0) as f32;
        self.pan_random = (value(PAN_RANDOM) / 100.0) as f32;
        // Per frame: Volume, Pan, and Time (R15), which slows an envelope
        // by 2^(4 Time).
        self.volume[..frames].copy_from_slice(ramp(VOLUME));
        for (pan, p) in self.pan[..frames].iter_mut().zip(ramp(PAN)) {
            *pan = p / 100.0;
        }
        for (speed, t) in self.speed[..frames].iter_mut().zip(ramp(TIME)) {
            *speed = (-TIME_OCTAVES * t / 100.0).exp2();
        }
        // The song's beat at each frame, counted at the tempo (R14).
        self.beats[0] = self.clock;
        for i in 0..frames {
            self.beats[i + 1] = self.beats[i] + audio.bpm_at(i) / 60.0 / self.rate;
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
                self.core.start(voice, start);
                let beats = self.beats[event.time as usize];
                let wait = Self::wait_for_grid(audio, event.time as usize);
                let sources = Sources::of(voices);
                let velocity = sources.velocity(voice) as f32;
                let octaves = (f32::from(voices.info(voice).key) - MIDDLE_KEY) / 12.0;
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

    fn render(&mut self, voices: &mut VoiceBlock, state: &State, frames: core::ops::Range<usize>) {
        let count = frames.len();
        if count == 0 {
            return;
        }
        let frame_secs = (1.0 / self.rate) as f32;
        for f in 0..count {
            let i = frames.start + f;
            let beats = self.beats[i];
            for voice in voices.active_voices() {
                let mut lane = Lane::default();
                let mut amp = [0.0f32; OPERATORS];
                for (op, amp) in amp.iter_mut().enumerate() {
                    let tick = frame_secs * self.speed[i] * self.time_scale[voice][op];
                    *amp = self.envelopes[voice][op].step(&self.shapes[op], tick, beats);
                }
                let tick = frame_secs * self.speed[i] * self.time_scale[voice][PITCH];
                let moved = self.envelopes[voice][PITCH].step(&self.shapes[PITCH], tick, beats);
                let (shift, second) = if self.pitch_on {
                    (moved * self.pitch_amount, moved * self.amount_b)
                } else {
                    (0.0, 0.0)
                };
                lane.pitch = shift * PITCH_SEMITONES;
                // Destination B: the targets part 1 has.
                let (mut volume, mut pan) = (1.0, self.pan[i] + self.pan_offset[voice]);
                let mut added = [0.0f32; OPERATORS];
                match self.destination {
                    d @ DEST_VOLUME_A..=4 => added[d - DEST_VOLUME_A] = second,
                    DEST_FEEDBACK => lane.feedback = second,
                    DEST_FIXED => lane.fixed = second * PITCH_SEMITONES,
                    DEST_VOLUME => volume = (1.0 + second).max(0.0),
                    DEST_PANORAMA => pan += second,
                    _ => {}
                }
                for op in 0..OPERATORS {
                    let level = (self.level[op][i] + added[op]).clamp(0.0, 1.0);
                    lane.amp[op] = level * amp[op] * self.level_scale[voice][op];
                }
                let pan = pan.clamp(-1.0, 1.0);
                let cache = &mut self.pan_cache[voice];
                if cache.0 != pan {
                    let (left, right) = pan_gains(pan);
                    *cache = (pan, [left, right]);
                }
                lane.pan = [cache.1[0] * volume, cache.1[1] * volume];
                self.lanes[f * STRIDE + voice] = lane;
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
}

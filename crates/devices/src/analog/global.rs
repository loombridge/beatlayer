//! Analog, part 2's parameters (WP-8.15; spec: Analog R18 to R25): the two
//! LFOs, the global section (volume, vibrato, unison, glide, the keyboard,
//! voices and priority) and the MPE section, with the modulation matrix the
//! MPE routings ride on.

use bl_model::device_layout::{
    BODY_HEIGHT, ControlBand, DisplayCell, LayoutNode, Page, TAB_HEIGHT, Widget,
};
use bl_model::{ParamDescriptor, ParamId, ParamUnit};

use crate::instrument::matrix::{
    DestDef, Kind as MatrixKind, MatrixLayout, Pick, Range as MatrixRange, Rate, Scope, SlotDef,
    SourceDef,
};
use crate::instrument::{self, NoteRange};
pub use crate::tension::{DIVISION_BEATS, DIVISIONS, LFO_WAVES};

use super::{CELL_WIDTH, PB_RANGE, SECTIONS, amp, display, filter, noise, osc};

/// An LFO's parameters, as offsets in its block (R18).
pub mod lfo {
    use bl_model::ParamId;
    pub const ON: u32 = 0;
    pub const WAVE: u32 = 1;
    pub const RATE: u32 = 2;
    pub const SYNC: u32 = 3;
    pub const DIVISION: u32 = 4;
    pub const WIDTH: u32 = 5;
    pub const DELAY: u32 = 6;
    pub const ATTACK: u32 = 7;
    pub const RETRIG: u32 = 8;
    pub const OFFSET: u32 = 9;

    /// LFO `n`'s (0 or 1) parameter at `offset`.
    #[must_use]
    pub const fn id(n: usize, offset: u32) -> ParamId {
        ParamId(super::LFO_BASE[n] + offset)
    }
}

pub(super) const LFO_BASE: [u32; 2] = [190, 200];

/// The global section's parameters (R19 to R23): the pitch wheel's range is
/// part 1's, [`super::PB_RANGE`].
pub const VOLUME: ParamId = ParamId(231);
pub const VIB_ON: ParamId = ParamId(232);
pub const VIB_AMOUNT: ParamId = ParamId(233);
pub const VIB_RATE: ParamId = ParamId(234);
pub const VIB_DELAY: ParamId = ParamId(235);
pub const VIB_ATTACK: ParamId = ParamId(236);
pub const VIB_ERROR: ParamId = ParamId(237);
pub const VIB_MOD_WHEEL: ParamId = ParamId(238);
pub const UNI_ON: ParamId = ParamId(239);
pub const UNI_VOICES: ParamId = ParamId(240);
pub const UNI_DETUNE: ParamId = ParamId(241);
pub const UNI_DELAY: ParamId = ParamId(242);
pub const GLIDE_ON: ParamId = ParamId(243);
pub const GLIDE_TIME: ParamId = ParamId(244);
pub const GLIDE_LEGATO: ParamId = ParamId(245);
pub const GLIDE_MODE: ParamId = ParamId(246);
pub const OCTAVE: ParamId = ParamId(247);
pub const SEMI: ParamId = ParamId(248);
pub const DETUNE: ParamId = ParamId(249);
pub const STRETCH: ParamId = ParamId(250);
pub const ERROR: ParamId = ParamId(251);
pub const VOICE_COUNT: ParamId = ParamId(252);
pub const PRIORITY: ParamId = ParamId(253);

/// The MPE section's parameters (R24): the switch, Pressure's and Slide's
/// two destinations and amounts, and Note PB's range.
pub const MPE_ON: ParamId = ParamId(260);
pub const MPE: [(ParamId, ParamId); 4] = [
    (ParamId(261), ParamId(262)),
    (ParamId(263), ParamId(264)),
    (ParamId(265), ParamId(266)),
    (ParamId(267), ParamId(268)),
];
pub const NOTE_PB: ParamId = ParamId(269);

/// The cell of the Global page that holds Quick Routing's four buttons
/// (R22), in the order they sit.
pub const QUICK_ROUTING: &str = "Quick Routing";
pub const QUICK_ROUTINGS: [&str; 4] = ["Parallel", "Split", "Filter 1 Only", "Serial"];

/// Unison's copies, by the chooser's index (R20).
pub const UNISON_COPIES: [&str; 2] = ["2", "4"];

/// Glide's modes (R21).
pub const GLIDE_MODES: [&str; 2] = ["Const", "Prop"];

/// The most notes sounding at once (R23), and unison's most copies a voice
/// has.
pub const MAX_VOICES: usize = 32;
pub const COPIES: usize = 4;

/// Spec: analog R24. The MPE destinations, in the choosers' order after
/// Off: the first two move a pitch, the others a parameter.
pub const DESTINATIONS: [&str; 16] = [
    "Osc 1 Pitch",
    "Osc 2 Pitch",
    "Osc 1 Level",
    "Osc 2 Level",
    "PW 1",
    "PW 2",
    "Noise Level",
    "F1 Freq",
    "F1 Reso",
    "F2 Freq",
    "F2 Reso",
    "Amp 1 Level",
    "Amp 2 Level",
    "LFO 1 Rate",
    "LFO 2 Rate",
    "Vibrato Amount",
];
pub(super) const DEST_OSC_PITCH: usize = 0;
pub(super) const DEST_LFO_RATE: usize = 13;
pub(super) const DEST_VIBRATO: usize = 15;

/// The parameter destination `dest` moves, or none for a pitch.
pub(super) fn dest_param(dest: usize) -> Option<ParamId> {
    Some(match dest {
        0 | 1 => return None,
        2 => osc::id(0, osc::LEVEL),
        3 => osc::id(1, osc::LEVEL),
        4 => osc::id(0, osc::PULSE_WIDTH),
        5 => osc::id(1, osc::PULSE_WIDTH),
        6 => noise::id(noise::LEVEL),
        7 => filter::id(0, filter::FREQ),
        8 => filter::id(0, filter::RESO),
        9 => filter::id(1, filter::FREQ),
        10 => filter::id(1, filter::RESO),
        11 => amp::id(0, amp::LEVEL),
        12 => amp::id(1, amp::LEVEL),
        13 => lfo::id(0, lfo::RATE),
        14 => lfo::id(1, lfo::RATE),
        _ => VIB_AMOUNT,
    })
}

/// Spec: analog R24. The matrix: Pressure and Slide, each to two chosen
/// destinations with an amount of their own. Pitches are worked out for
/// every frame, everything else on the control grid.
#[must_use]
pub fn matrix_layout() -> MatrixLayout {
    let source = |name| SourceDef {
        name,
        range: MatrixRange::Unipolar,
        scope: Scope::Voice,
    };
    MatrixLayout {
        sources: vec![source("Pressure"), source("Slide")],
        dests: DESTINATIONS
            .iter()
            .enumerate()
            .map(|(n, &name)| DestDef {
                name,
                param: dest_param(n),
                scope: Scope::Voice,
                rate: if n < 2 { Rate::Audio } else { Rate::Control },
                kind: MatrixKind::Additive,
            })
            .collect(),
        slots: MPE
            .iter()
            .enumerate()
            .map(|(n, &(dest, amount))| SlotDef {
                source: Pick::Fixed((n / 2) as u8),
                dest: Pick::Chosen(dest),
                amount,
            })
            .collect(),
        forbidden: Vec::new(),
    }
}

/// Part 2's descriptors: the LFOs, then the global section and the MPE
/// section.
pub(super) fn descriptors() -> Vec<ParamDescriptor> {
    let toggle =
        |id, name: &str, on, text: &str| ParamDescriptor::toggle(id, name, on).describe(text);
    let percent = |id, name: &str, default: f64, text: &str| {
        ParamDescriptor::continuous(id, name, (0.0, 100.0), default, ParamUnit::Percent)
            .describe(text)
    };
    let signed = |id, name: &str, default: f64, text: &str| {
        ParamDescriptor::continuous(id, name, (-100.0, 100.0), default, ParamUnit::Percent)
            .describe(text)
    };
    let time = |id, name: &str, max: f64, text: &str| {
        ParamDescriptor::continuous(id, name, (0.0, max), 0.0, ParamUnit::Milliseconds)
            .power(3)
            .describe(text)
    };
    let mut params = Vec::new();
    for n in 0..2 {
        let at = |offset| lfo::id(n, offset);
        let name = |what: &str| format!("LFO {} {what}", n + 1);
        params.extend([
            toggle(
                at(lfo::ON),
                &format!("LFO {}", n + 1),
                false,
                "Turns the LFO on.",
            ),
            ParamDescriptor::choice(at(lfo::WAVE), &name("Wave"), &LFO_WAVES, 0)
                .describe("The LFO's wave: S&H holds a new random value each cycle, Noise ramps between them."),
            ParamDescriptor::continuous(
                at(lfo::RATE),
                &name("Rate"),
                (0.01, 30.0),
                1.0,
                ParamUnit::Hertz,
            )
            .logarithmic()
            .describe("How fast the LFO runs, in hertz."),
            toggle(
                at(lfo::SYNC),
                &name("Sync"),
                false,
                "Runs the LFO at a division of the tempo instead of in hertz.",
            ),
            ParamDescriptor::choice(at(lfo::DIVISION), &name("Division"), &DIVISIONS, 4)
                .describe("The tempo division the LFO runs at when it is synced."),
            percent(
                at(lfo::WIDTH),
                &name("Width"),
                50.0,
                "Skews the triangle and sets the rectangle's duty cycle.",
            ),
            time(
                at(lfo::DELAY),
                &name("Delay"),
                10_000.0,
                "How long the LFO holds at 0 from the start of a note.",
            ),
            time(
                at(lfo::ATTACK),
                &name("Attack"),
                10_000.0,
                "How long the LFO takes to fade in once its delay has passed.",
            ),
            toggle(
                at(lfo::RETRIG),
                &name("Retrig"),
                false,
                "Restarts the LFO at its offset on each note; off, every note joins the LFO where it runs.",
            ),
            ParamDescriptor::continuous(
                at(lfo::OFFSET),
                &name("Offset"),
                (0.0, 360.0),
                0.0,
                ParamUnit::None,
            )
            .decimals(0)
            .describe("Where in its cycle the LFO starts, in degrees."),
        ]);
    }
    params.extend([
        ParamDescriptor::gain(VOLUME, "Volume", 0.0, 0.0)
            .describe("Sets the instrument's output level, down to silence."),
        toggle(VIB_ON, "Vibrato", false, "Turns the vibrato on."),
        percent(
            VIB_AMOUNT,
            "Vibrato Amount",
            0.0,
            "How far the vibrato moves every voice's pitch, up to a semitone.",
        ),
        ParamDescriptor::continuous(VIB_RATE, "Vibrato Rate", (0.1, 20.0), 5.0, ParamUnit::Hertz)
            .logarithmic()
            .describe("How fast the vibrato runs."),
        time(
            VIB_DELAY,
            "Vibrato Delay",
            10_000.0,
            "How long the vibrato holds at nothing from the start of a note.",
        ),
        time(
            VIB_ATTACK,
            "Vibrato Attack",
            10_000.0,
            "How long the vibrato takes to fade in once its delay has passed.",
        ),
        percent(
            VIB_ERROR,
            "Vibrato Error",
            0.0,
            "Varies each voice's vibrato rate and amount at random, by up to half at 100 %.",
        ),
        percent(
            VIB_MOD_WHEEL,
            "Vibrato Amt < MW",
            0.0,
            "How much the mod wheel adds to the vibrato's amount.",
        ),
        toggle(UNI_ON, "Unison", false, "Plays each note on several detuned copies of a voice."),
        ParamDescriptor::choice(UNI_VOICES, "Unison Voices", &UNISON_COPIES, 0)
            .describe("How many copies play each note: 2 or 4."),
        percent(
            UNI_DETUNE,
            "Unison Detune",
            0.0,
            "How far the outermost copies sit from the note, up to 50 cents at 100 %.",
        ),
        percent(
            UNI_DELAY,
            "Unison Delay",
            0.0,
            "How long after the first copy the last one starts, up to 20 ms at 100 %.",
        ),
        toggle(GLIDE_ON, "Glide", false, "Turns the glide on."),
        instrument::glide_time(GLIDE_TIME, 10_000.0, 100.0),
        instrument::glide_legato(GLIDE_LEGATO, false),
        ParamDescriptor::choice(GLIDE_MODE, "Glide Mode", &GLIDE_MODES, 0).describe(
            "Const glides in the same time for every interval; Prop scales the time with the interval, Glide Time being an octave's.",
        ),
        ParamDescriptor::integer(OCTAVE, "Keyboard Octave", (-3, 3), 0, ParamUnit::None)
            .decimals(0)
            .describe("Transposes the keyboard in octaves."),
        ParamDescriptor::integer(SEMI, "Keyboard Semi", (-12, 12), 0, ParamUnit::Semitones)
            .decimals(0)
            .describe("Transposes the keyboard in semitones."),
        ParamDescriptor::continuous(
            DETUNE,
            "Keyboard Detune",
            (-50.0, 50.0),
            0.0,
            ParamUnit::Cents,
        )
        .decimals(0)
        .describe("Detunes the keyboard, in cents."),
        ParamDescriptor::continuous(
            STRETCH,
            "Keyboard Stretch",
            (-100.0, 100.0),
            0.0,
            ParamUnit::Percent,
        )
        .describe("Widens the octaves above middle C and narrows those below it; negative does the opposite."),
        percent(
            ERROR,
            "Keyboard Error",
            0.0,
            "Detunes each note by a random amount, up to half a semitone at 100 %.",
        ),
        instrument::voices(VOICE_COUNT, MAX_VOICES as i64, 8),
        instrument::priority(PRIORITY),
        toggle(MPE_ON, "MPE", false, "Turns the pressure and slide routings and per-note pitch bend on."),
    ]);
    let mut destinations = vec!["Off"];
    destinations.extend(DESTINATIONS);
    for (n, (pair, source)) in MPE
        .into_iter()
        .zip(["Pressure", "Pressure", "Slide", "Slide"])
        .enumerate()
    {
        let number = n % 2 + 1;
        params.extend([
            ParamDescriptor::choice(pair.0, &format!("{source} Dest {number}"), &destinations, 0)
                .describe(&format!(
                    "The control {source} moves, by its amount; Off leaves it alone."
                )),
            signed(
                pair.1,
                &format!("{source} Amount {number}"),
                0.0,
                &format!("How far {source} moves its destination, in per cent of its travel."),
            ),
        ]);
    }
    params.push(
        ParamDescriptor::integer(NOTE_PB, "Note PB", (0, 48), 48, ParamUnit::Semitones)
            .decimals(0)
            .describe("Sets the range of a per-note pitch bend, in semitones."),
    );
    params
}

/// Which parts of the voice section Analog has (instrument voices R1).
pub(super) fn parts() -> instrument::Parts {
    instrument::Parts {
        voices: Some(VOICE_COUNT),
        voices_one_is_mono: true,
        priority: Some(PRIORITY),
        glide_time: Some(GLIDE_TIME),
        glide_on: Some(GLIDE_ON),
        glide_prop: Some(GLIDE_MODE),
        glide_legato: Some(GLIDE_LEGATO),
        poly_glide: true,
        unison_voices: Some(UNI_VOICES),
        unison_on: Some(UNI_ON),
        unison_choice: true,
        pb_range: Some(super::PB_RANGE),
        note_pb: Some(MPE_ON),
        note_range: NoteRange::Own(NOTE_PB),
        stretch: Some(STRETCH),
        error: Some(ERROR),
        ..instrument::Parts::default()
    }
}

/// Spec: analog R18, R19 to R24, R26. The LFOs' pages and the Global page of
/// the shell: each LFO's main controls in the strip and the rest, with its
/// wave drawn, in the display; the Global page's strip holds the output and
/// the voices, its display the vibrato, unison, glide, keyboard and MPE
/// controls beside Quick Routing's buttons.
pub(super) fn pages() -> Vec<Page> {
    const SECTION: f32 = BODY_HEIGHT - TAB_HEIGHT;
    let band = |height, controls: &[(ParamId, Widget)]| {
        LayoutNode::Band(ControlBand::stacked(height, controls))
    };
    let knob = |id| (id, Widget::Knob);
    let toggle = |id| (id, Widget::Toggle);
    let chooser = |id| (id, Widget::Chooser);
    let number = |id| (id, Widget::Number);
    let mut pages = Vec::new();
    for n in 0..2 {
        let at = |offset| lfo::id(n, offset);
        pages.push(
            Page::new(
                SECTIONS[7 + n],
                LayoutNode::Row(vec![
                    band(
                        BODY_HEIGHT,
                        &[
                            chooser(at(lfo::DIVISION)),
                            knob(at(lfo::WIDTH)),
                            knob(at(lfo::DELAY)),
                            knob(at(lfo::ATTACK)),
                            toggle(at(lfo::RETRIG)),
                            knob(at(lfo::OFFSET)),
                        ],
                    ),
                    LayoutNode::Cell(DisplayCell::new(
                        display::WAVES[n],
                        CELL_WIDTH,
                        BODY_HEIGHT,
                        1.0,
                    )),
                ]),
            )
            .with_section(band(
                SECTION,
                &[
                    toggle(at(lfo::ON)),
                    chooser(at(lfo::WAVE)),
                    knob(at(lfo::RATE)),
                    toggle(at(lfo::SYNC)),
                ],
            )),
        );
    }
    let mpe: Vec<(ParamId, Widget)> = std::iter::once(toggle(MPE_ON))
        .chain(
            MPE.iter()
                .flat_map(|&(dest, amount)| [chooser(dest), knob(amount)]),
        )
        .chain([number(NOTE_PB)])
        .collect();
    pages.push(
        Page::new(
            SECTIONS[9],
            LayoutNode::Row(vec![
                band(
                    BODY_HEIGHT,
                    &[
                        toggle(VIB_ON),
                        knob(VIB_AMOUNT),
                        knob(VIB_RATE),
                        knob(VIB_DELAY),
                        knob(VIB_ATTACK),
                        knob(VIB_ERROR),
                        knob(VIB_MOD_WHEEL),
                    ],
                ),
                band(
                    BODY_HEIGHT,
                    &[
                        toggle(UNI_ON),
                        chooser(UNI_VOICES),
                        knob(UNI_DETUNE),
                        knob(UNI_DELAY),
                    ],
                ),
                band(
                    BODY_HEIGHT,
                    &[
                        toggle(GLIDE_ON),
                        knob(GLIDE_TIME),
                        toggle(GLIDE_LEGATO),
                        chooser(GLIDE_MODE),
                    ],
                ),
                band(
                    BODY_HEIGHT,
                    &[
                        number(OCTAVE),
                        number(SEMI),
                        knob(DETUNE),
                        knob(STRETCH),
                        knob(ERROR),
                    ],
                ),
                band(BODY_HEIGHT, &mpe),
                LayoutNode::Cell(DisplayCell::new(
                    QUICK_ROUTING,
                    CELL_WIDTH,
                    BODY_HEIGHT,
                    1.0,
                )),
            ]),
        )
        .with_section(band(
            SECTION,
            &[
                knob(VOLUME),
                number(VOICE_COUNT),
                chooser(PRIORITY),
                number(PB_RANGE),
            ],
        )),
    );
    pages
}

//! Meld, parts 1 to 3 (spec: meld R1 to R58): two engines, each an oscillator
//! of one of twenty-five types ([`meld_osc`]'s tables and lanes, the FM
//! core's four FM types, [`meld_gen`]'s noise and texture types), a filter
//! of one of seventeen types ([`meld_filter`]) and an amplitude envelope,
//! mixed per voice in the Mix
//! section under the global Mixer Drive, Limit and Volume, on the voice
//! section. The envelopes' other parts, the LFOs, the matrix and the
//! panel's other tabs are Meld's next parts.
//!
//! Each engine runs its own voice section on the same notes and settings,
//! because each has its own Glide and scale awareness (R16): the two sections
//! allocate voices alike, so a voice and its stacked copies are one slot in
//! both engines, which the mix then limits together (R18). A voice ends
//! when both amplitude envelopes have ended (R13).
//!
//! Controls are worked out on the voice section's grid of 32 frames and held
//! between, and each voice's lanes render in chunks that end on it, so a
//! render is the same at any block size (R23).

use bl_dsp::random::Random;
use bl_engine::{DeviceType, NodeConfig, NodeData, NodeEvent, Registry};
use bl_model::device_layout::{
    BODY_HEIGHT, ControlBand, DisplayCell, LayoutNode, Page, PageStyle, PagesNode, TAB_HEIGHT,
    Widget,
};
use bl_model::{
    DeviceDescription, DeviceLayout, DeviceRole, ParamDescriptor, ParamId, ParamLabel, ParamUnit,
    Relabel,
};
use bl_model::{Scale, param::db_to_gain};
use lspk_dsp::filter::pitch_of;
use lspk_voice::unison::pan_gains;
use lspk_voice::{Start, VoiceBlock, VoiceEvent, VoiceEventKind};
use lspk_wavetable::osc::Onset;

use crate::analogue::note_seed;
use crate::fm;
use crate::framework::{Audio, NativeDevice, ParamBank, add};
use crate::instrument::{self, Instrument, Parts, Section, State};
use crate::instrument_filter::{Per, PerFrame};
use crate::meld_display;
use crate::meld_filter::{Controls, MeldFilter};
use crate::meld_gen::{self, Gen, Pitch};
use crate::meld_matrix::{
    self, GlobalRead, L_CENTS, L_FILTER_1, L_FILTER_2, L_FREQ, L_MACRO_1, L_MACRO_2, L_PAN,
    L_QUANT, L_SEMITONES, L_TONE, L_VOLUME, Mods, SlotRead,
};
pub use crate::meld_matrix::{dest_engine, dest_for, source_engine, source_for};
pub use crate::meld_mod::{FX_MACROS, FX_NAMES, LFO1_MACROS, LFO1_TYPES, LFO2_TYPES};
use crate::meld_osc::{
    self, Bank, CHUNK, Context, Kind, LOOPS, MACROS, MAX_LANES, SWARM_COPIES, TYPES,
};
use crate::wavetable_mod::DIVISIONS;

pub use crate::meld_osc::{shepard, shepard_width};

/// Meld's device type.
pub const MELD: &str = "beatlayer.meld";

/// The parameter `offset` of engine `engine` (0 for A, 1 for B): the
/// engines' parameters are the same set, a hundred apart, so an ID never
/// depends on a type.
#[must_use]
pub const fn param(engine: usize, offset: u32) -> ParamId {
    ParamId(100 * engine as u32 + offset)
}

/// An engine's parameters, by offset (R2, R3, R5, R12, R13, R16, R18).
pub const ON: u32 = 0;
pub const TYPE: u32 = 1;
pub const OCTAVES: u32 = 2;
pub const SEMITONES: u32 = 3;
pub const CENTS: u32 = 4;
pub const MACRO_1: u32 = 5;
pub const MACRO_2: u32 = 6;
pub const FILTER_ON: u32 = 7;
pub const FILTER_TYPE: u32 = 8;
pub const FILTER_FREQ: u32 = 9;
pub const FILTER_MACRO_1: u32 = 10;
pub const FILTER_MACRO_2: u32 = 11;
pub const ATTACK: u32 = 12;
pub const DECAY: u32 = 13;
pub const SUSTAIN: u32 = 14;
pub const RELEASE: u32 = 15;
pub const VOLUME: u32 = 16;
pub const PAN: u32 = 17;
pub const TONE: u32 = 18;
pub const KEY_TRACKING: u32 = 19;
pub const PHASE_RESET: u32 = 20;
pub const PHASE_SPREAD: u32 = 21;
pub const OSC_SCALE: u32 = 22;
pub const FILTER_SCALE: u32 = 23;
pub const GLIDE_TIME: u32 = 24;
pub const GLIDE_MODE: u32 = 25;
/// The amplitude envelope's slopes and loop mode (R59).
pub const ATTACK_SLOPE: u32 = 26;
pub const DECAY_SLOPE: u32 = 27;
pub const RELEASE_SLOPE: u32 = 28;
pub const ENV_MODE: u32 = 29;
/// The modulation envelope's times, Sustain, slopes, levels and loop mode,
/// and Link Envelopes (R59).
pub const MOD_ATTACK: u32 = 30;
pub const MOD_DECAY: u32 = 31;
pub const MOD_SUSTAIN: u32 = 32;
pub const MOD_RELEASE: u32 = 33;
pub const MOD_ATTACK_SLOPE: u32 = 34;
pub const MOD_DECAY_SLOPE: u32 = 35;
pub const MOD_RELEASE_SLOPE: u32 = 36;
pub const MOD_INITIAL: u32 = 37;
pub const MOD_PEAK: u32 = 38;
pub const MOD_FINAL: u32 = 39;
pub const MOD_MODE: u32 = 40;
pub const LINK: u32 = 41;
/// LFO 1 (R60), its two effects (R61) and LFO 2 (R60).
pub const LFO1_TYPE: u32 = 42;
pub const LFO1_SYNC: u32 = 43;
pub const LFO1_RATE: u32 = 44;
pub const LFO1_DIVISION: u32 = 45;
pub const LFO1_MACRO_1: u32 = 46;
pub const LFO1_MACRO_2: u32 = 47;
pub const LFO1_OFFSET: u32 = 48;
pub const LFO1_RETRIGGER: u32 = 49;
pub const FX1_TYPE: u32 = 50;
pub const FX1_AMOUNT: u32 = 51;
pub const FX2_TYPE: u32 = 52;
pub const FX2_AMOUNT: u32 = 53;
pub const LFO2_TYPE: u32 = 54;
pub const LFO2_SYNC: u32 = 55;
pub const LFO2_RATE: u32 = 56;
pub const LFO2_DIVISION: u32 = 57;
pub const LFO2_OFFSET: u32 = 58;
pub const LFO2_RETRIGGER: u32 = 59;

/// The global parameters (R14, R15, R17, R18).
pub const MODE: ParamId = ParamId(200);
pub const VOICES: ParamId = ParamId(201);
pub const LEGATO: ParamId = ParamId(202);
pub const STACK: ParamId = ParamId(203);
pub const SPREAD: ParamId = ParamId(204);
pub const DRIVE: ParamId = ParamId(205);
pub const LIMIT: ParamId = ParamId(206);
pub const OUTPUT: ParamId = ParamId(207);
pub const USE_SCALE: ParamId = ParamId(208);
pub const PB_RANGE: ParamId = ParamId(209);

/// The matrix's free routes (R62).
pub const ROUTES: usize = 64;

/// Route slot `slot`'s amount, from −100 % to 100 %.
#[must_use]
pub const fn route_amount(slot: usize) -> ParamId {
    ParamId(300 + slot as u32)
}

/// The most voices Poly plays and the most copies Stacked Voices stacks
/// (R14), and so the most slots: a voice's copy is a slot.
pub const MAX_VOICES: usize = 12;
pub const MAX_STACK: usize = 4;
pub(crate) const SLOTS: usize = MAX_VOICES * MAX_STACK;

/// The filter types' names, in the manual's order (R40).
const FILTERS: [&str; 17] = [
    "SVF 12dB",
    "SVF 24dB",
    "LP 12dB MS2",
    "HP 12dB MS2",
    "BP 12dB OSR",
    "LP Crunch 12dB",
    "LP Switched Res",
    "Filther",
    "Eq Peak",
    "Eq Notch",
    "Phaser",
    "Redux",
    "Vowel",
    "Comb +",
    "Comb -",
    "Plate Resonator",
    "Membrane Resonator",
];

/// A control's name as Meld's panel draws it, if `name` is one of Meld's
/// engine parameters: it drops the engine's letter, since the page shows the
/// engine, and the modulation envelope's "Mod", since its group shows it; the
/// slopes' names are short, to fit a small knob (R72).
#[must_use]
pub fn panel_name(name: &str) -> Option<String> {
    static NAMES: std::sync::OnceLock<std::collections::HashSet<String>> =
        std::sync::OnceLock::new();
    let engine = NAMES.get_or_init(|| {
        let description = meld_description();
        (0..2)
            .flat_map(|engine| (0..100).map(move |offset| param(engine, offset)))
            .filter_map(|id| description.param(id).map(|p| p.name.clone()))
            .collect()
    });
    if !engine.contains(name) {
        return None;
    }
    let name = name.get(2..).unwrap_or(name);
    let name = name.strip_prefix("Mod ").unwrap_or(name);
    let name = name.replace("Envelope Loop Mode", "Loop Mode");
    Some(
        ["Attack", "Decay", "Release"]
            .iter()
            .fold(name, |name, stage| {
                name.replace(&format!("{stage} Slope"), &format!("{} Slope", &stage[..3]))
            }),
    )
}

/// The name of filter type `index`.
#[must_use]
pub fn filter_type_name(index: usize) -> &'static str {
    FILTERS[index.min(FILTERS.len() - 1)]
}

/// Each filter type's two macros' names, as the panel shows them (R12,
/// R41).
pub const FILTER_MACROS: [(&str, &str); 17] = [
    ("Q", "L-B-H-N"),
    ("Q", "L-B-H-N"),
    ("Q", "Drive"),
    ("Q", "Drive"),
    ("Q", "Drive"),
    ("Q", "Drive"),
    ("Q", "Lofi"),
    ("Q", "Drive"),
    ("Boost", "Q"),
    ("Cut", "Q"),
    ("Feedb", "Spread"),
    ("Crush", "Lofi"),
    ("Q", "Morph"),
    ("Feedb", "Damp"),
    ("Feedb", "Damp"),
    ("Q", "Ratio"),
    ("Q", "Damp"),
];

/// The Matrix, MIDI and MPE tabs' grids (R63, R65), for each engine, and the
/// Expanded View's cell (R66).
pub const GRID_CELLS: [&str; 3] = ["Matrix", "MIDI Matrix", "MPE Matrix"];
pub const EXPANDED_CELL: &str = "Expanded View";
/// The least width of a grid in a tab and of the Expanded View, in points.
const GRID_WIDTH: f32 = 330.0;
const EXPANDED_WIDTH: f32 = 520.0;

/// The name of engine `letter`'s grid cell for tab `tab` (0 Matrix, 1 MIDI,
/// 2 MPE).
#[must_use]
pub fn grid_cell(letter: &str, tab: usize) -> String {
    format!("{letter} {}", GRID_CELLS[tab])
}

/// The height of a display above its controls, in points.
const DISPLAY_HEIGHT: f32 = 40.0;

/// An envelope's loop modes (R59).
pub const LOOP_MODES: [&str; 4] = ["None", "Trigger", "Loop", "AD Loop"];

/// The types the voice code branches on (R4).
const FOLD_FM: usize = 11;
const SHEPARDS_PI: usize = 15;
const NOISE_LOOP: usize = 18;

/// Whether type `index` is one of the four that play on the FM core.
const fn is_fm(index: usize) -> bool {
    matches!(index, 10..=13)
}

/// The per-voice limiter's release (R18).
const LIMIT_RELEASE: f32 = 0.05;

/// The device's type.
#[must_use]
pub fn meld_type() -> DeviceType {
    DeviceType::native(MELD, "Meld", DeviceRole::Instrument)
        .with_description(meld_description())
        .with_matrix(meld_matrix::layout().as_ref().clone())
}

/// Meld's parameters, panel and banks.
#[must_use]
pub fn meld_description() -> DeviceDescription {
    let mut params = Vec::new();
    for (engine, letter) in ["A", "B"].into_iter().enumerate() {
        let name = |what: &str| format!("{letter} {what}");
        let id = |offset| param(engine, offset);
        params.extend([
            ParamDescriptor::toggle(id(ON), &name("On"), engine == 0).describe(
                "Turns the engine on; off, it and its filter are silent.",
            ),
            ParamDescriptor::choice(id(TYPE), &name("Type"), &TYPES, 0)
                .describe("The oscillator's type; the arrows step through them and wrap."),
            ParamDescriptor::integer(id(OCTAVES), &name("Octaves"), (-3, 3), 0, ParamUnit::None)
                .decimals(0)
                .describe("Transposes the engine in octaves."),
            ParamDescriptor::integer(id(SEMITONES), &name("Semitones"), (-12, 12), 0, ParamUnit::Semitones)
                .decimals(0)
                .describe("Transposes the engine in semitones; with Use Current Scale on, in degrees of the scale."),
            ParamDescriptor::continuous(id(CENTS), &name("Cents"), (-50.0, 50.0), 0.0, ParamUnit::Cents)
                .decimals(0)
                .describe("Tunes the engine in cents."),
            macro_param(id(MACRO_1), &name("Macro 1"), "The oscillator's first macro."),
            macro_param(id(MACRO_2), &name("Macro 2"), "The oscillator's second macro."),
            ParamDescriptor::toggle(id(FILTER_ON), &name("Filter"), true)
                .describe("Turns the engine's filter on; off, the engine passes through."),
            ParamDescriptor::choice(id(FILTER_TYPE), &name("Filter Type"), &FILTERS, 0)
                .describe("The filter's type."),
            ParamDescriptor::continuous(
                id(FILTER_FREQ),
                &name("Filter Frequency"),
                (20.0, 20_000.0),
                20_000.0,
                ParamUnit::Hertz,
            )
            .logarithmic()
            .describe("The filter's cutoff."),
            macro_param(id(FILTER_MACRO_1), &name("Filter Macro 1"), "The filter's first macro."),
            macro_param(id(FILTER_MACRO_2), &name("Filter Macro 2"), "The filter's second macro."),
            time(id(ATTACK), &name("Attack"), 1.0, "How long the amplitude envelope takes to reach full."),
            time(id(DECAY), &name("Decay"), 600.0, "How long it takes to fall to Sustain."),
            ParamDescriptor::continuous(id(SUSTAIN), &name("Sustain"), (0.0, 100.0), 100.0, ParamUnit::Percent)
                .describe("The level the envelope holds while a note lasts."),
            time(id(RELEASE), &name("Release"), 50.0, "How long it takes to fall to silence after a note ends."),
            ParamDescriptor::gain(id(VOLUME), &name("Volume"), 0.0, -3.0)
                .describe("The engine's level in the mix."),
            ParamDescriptor::continuous(id(PAN), &name("Pan"), (-100.0, 100.0), 0.0, ParamUnit::Percent)
                .describe("Places the engine between left and right."),
            ParamDescriptor::continuous(id(TONE), &name("Tone"), (-100.0, 100.0), 0.0, ParamUnit::Percent)
                .describe("Positive values cut the lows, negative values the highs."),
            ParamDescriptor::toggle(id(KEY_TRACKING), &name("Osc Key Tracking"), true).describe(
                "On, the oscillator follows the key; off, it plays C3, or the scale's root.",
            ),
            ParamDescriptor::toggle(id(PHASE_RESET), &name("Phase Reset"), true)
                .describe("Starts the oscillator at the same phase on each note."),
            ParamDescriptor::continuous(id(PHASE_SPREAD), &name("Phase Spread"), (0.0, 100.0), 0.0, ParamUnit::Percent)
                .describe("Offsets each note's start phase by the Spread source, with Phase Reset on."),
            ParamDescriptor::toggle(id(OSC_SCALE), &name("Oscillator Scale Awareness"), true)
                .describe("Lets the oscillator types that can follow the scale do so."),
            ParamDescriptor::toggle(id(FILTER_SCALE), &name("Filter Scale Awareness"), true)
                .describe("Lets the resonator filters tune to the scale."),
            ParamDescriptor {
                name: name("Glide Time"),
                ..instrument::glide_time(id(GLIDE_TIME), 10_000.0, 0.0)
            },
            ParamDescriptor {
                name: name("Glide Mode"),
                ..instrument::glide_mode(id(GLIDE_MODE))
            },
        ]);
        params.extend(modulation_params(engine, letter));
    }
    params.extend((0..ROUTES).map(|slot| {
        ParamDescriptor::continuous(
            route_amount(slot),
            &format!("Route {} Amount", slot + 1),
            (-100.0, 100.0),
            0.0,
            ParamUnit::Percent,
        )
        .describe("How far the route's source moves its destination, in per cent of its travel.")
    }));
    params.extend([
        instrument::voice_mode(MODE, false),
        ParamDescriptor::integer(VOICES, "Voices", (2, MAX_VOICES as i64), 8, ParamUnit::None)
            .decimals(0)
            .describe("The most notes that sound at once, in Poly."),
        instrument::legato(LEGATO, false),
        ParamDescriptor::integer(STACK, "Stacked Voices", (1, MAX_STACK as i64), 1, ParamUnit::None)
            .decimals(0)
            .describe("How many copies of both engines play each note."),
        ParamDescriptor::continuous(SPREAD, "Spread", (0.0, 100.0), 0.0, ParamUnit::Percent)
            .describe("The range of the Spread source."),
        ParamDescriptor::continuous(DRIVE, "Mixer Drive", (0.0, 24.0), 0.0, ParamUnit::Decibels)
            .describe("Saturates each voice after the engines are mixed."),
        ParamDescriptor::toggle(LIMIT, "Limit", true)
            .describe("Holds each voice below 0 dBFS, after Mixer Drive."),
        ParamDescriptor::gain(OUTPUT, "Volume", 0.0, -6.0).describe("The device's output level."),
        ParamDescriptor::toggle(USE_SCALE, "Use Current Scale", false).describe(
            "Counts the pitch controls in degrees of the current scale and lets the scale-aware types follow it.",
        ),
        instrument::pb_range(PB_RANGE, 96, 7),
    ]);
    let tab = BODY_HEIGHT - 2.0 * TAB_HEIGHT;
    let band = |height: f32, controls: &[(ParamId, Widget)]| {
        LayoutNode::Band(ControlBand::stacked(height, controls))
    };
    // A display over a band of controls, together `height` tall (R67).
    let shown = |name: String, width: f32, height: f32, controls: &[(ParamId, Widget)]| {
        LayoutNode::Column(vec![
            LayoutNode::Cell(DisplayCell::new(&name, width, DISPLAY_HEIGHT, 1.0)),
            band(height - DISPLAY_HEIGHT, controls),
        ])
    };
    let engine_page = |engine: usize| {
        let id = |offset| param(engine, offset);
        let letter = ["A", "B"][engine];
        let tabs = PagesNode::new(
            &format!("{letter} Tabs"),
            PageStyle::Tabs,
            vec![
                Page::new(
                    "Envelopes",
                    LayoutNode::Row(vec![
                        shown(
                            meld_display::envelope_name(letter, false),
                            120.0,
                            tab,
                            &[
                                (id(ATTACK), Widget::Knob),
                                (id(DECAY), Widget::Knob),
                                (id(SUSTAIN), Widget::Knob),
                                (id(RELEASE), Widget::Knob),
                                (id(ATTACK_SLOPE), Widget::SmallKnob),
                                (id(DECAY_SLOPE), Widget::SmallKnob),
                                (id(RELEASE_SLOPE), Widget::SmallKnob),
                                (id(ENV_MODE), Widget::Chooser),
                            ],
                        ),
                        shown(
                            meld_display::envelope_name(letter, true),
                            120.0,
                            tab,
                            &[
                                (id(MOD_ATTACK), Widget::Knob),
                                (id(MOD_DECAY), Widget::Knob),
                                (id(MOD_SUSTAIN), Widget::Knob),
                                (id(MOD_RELEASE), Widget::Knob),
                                (id(MOD_ATTACK_SLOPE), Widget::SmallKnob),
                                (id(MOD_DECAY_SLOPE), Widget::SmallKnob),
                                (id(MOD_RELEASE_SLOPE), Widget::SmallKnob),
                                (id(MOD_INITIAL), Widget::SmallKnob),
                                (id(MOD_PEAK), Widget::SmallKnob),
                                (id(MOD_FINAL), Widget::SmallKnob),
                                (id(MOD_MODE), Widget::Chooser),
                                (id(LINK), Widget::Toggle),
                            ],
                        ),
                    ]),
                ),
                Page::new(
                    "LFOs",
                    LayoutNode::Row(vec![
                        shown(
                            meld_display::lfo_name(letter, "LFO 1"),
                            100.0,
                            tab,
                            &[
                                (id(LFO1_TYPE), Widget::Chooser),
                                (id(LFO1_SYNC), Widget::Toggle),
                                (id(LFO1_RATE), Widget::Knob),
                                (id(LFO1_DIVISION), Widget::Chooser),
                                (id(LFO1_MACRO_1), Widget::SmallKnob),
                                (id(LFO1_MACRO_2), Widget::SmallKnob),
                                (id(LFO1_OFFSET), Widget::SmallKnob),
                                (id(LFO1_RETRIGGER), Widget::Toggle),
                            ],
                        ),
                        shown(
                            meld_display::lfo_name(letter, "LFO 1 FX"),
                            100.0,
                            tab,
                            &[
                                (id(FX1_TYPE), Widget::Chooser),
                                (id(FX1_AMOUNT), Widget::SmallKnob),
                                (id(FX2_TYPE), Widget::Chooser),
                                (id(FX2_AMOUNT), Widget::SmallKnob),
                            ],
                        ),
                        shown(
                            meld_display::lfo_name(letter, "LFO 2"),
                            100.0,
                            tab,
                            &[
                                (id(LFO2_TYPE), Widget::Chooser),
                                (id(LFO2_SYNC), Widget::Toggle),
                                (id(LFO2_RATE), Widget::Knob),
                                (id(LFO2_DIVISION), Widget::Chooser),
                                (id(LFO2_OFFSET), Widget::SmallKnob),
                                (id(LFO2_RETRIGGER), Widget::Toggle),
                            ],
                        ),
                    ]),
                ),
                Page::new(
                    "Matrix",
                    LayoutNode::Cell(DisplayCell::new(
                        &grid_cell(letter, 0),
                        GRID_WIDTH,
                        tab,
                        1.0,
                    )),
                ),
                Page::new(
                    "MIDI",
                    LayoutNode::Cell(DisplayCell::new(
                        &grid_cell(letter, 1),
                        GRID_WIDTH,
                        tab,
                        1.0,
                    )),
                ),
                Page::new(
                    "MPE",
                    LayoutNode::Cell(DisplayCell::new(
                        &grid_cell(letter, 2),
                        GRID_WIDTH,
                        tab,
                        1.0,
                    )),
                ),
                Page::new(
                    "Settings",
                    band(
                        tab,
                        &[
                            (id(KEY_TRACKING), Widget::Toggle),
                            (id(PHASE_RESET), Widget::Toggle),
                            (id(PHASE_SPREAD), Widget::Knob),
                            (id(OSC_SCALE), Widget::Toggle),
                            (id(FILTER_SCALE), Widget::Toggle),
                            (id(GLIDE_TIME), Widget::Knob),
                            (id(GLIDE_MODE), Widget::Chooser),
                        ],
                    ),
                ),
            ],
        );
        let body = BODY_HEIGHT - TAB_HEIGHT;
        Page::new(
            letter,
            LayoutNode::Row(vec![
                shown(
                    meld_display::oscillator_name(letter),
                    90.0,
                    body,
                    &[
                        (id(ON), Widget::Toggle),
                        (id(TYPE), Widget::Chooser),
                        (id(OCTAVES), Widget::Number),
                        (id(SEMITONES), Widget::Number),
                        (id(CENTS), Widget::Number),
                        (id(MACRO_1), Widget::SmallKnob),
                        (id(MACRO_2), Widget::SmallKnob),
                    ],
                ),
                shown(
                    meld_display::filter_name(letter),
                    90.0,
                    body,
                    &[
                        (id(FILTER_ON), Widget::Toggle),
                        (id(FILTER_TYPE), Widget::Chooser),
                        (id(FILTER_FREQ), Widget::Knob),
                        (id(FILTER_MACRO_1), Widget::SmallKnob),
                        (id(FILTER_MACRO_2), Widget::SmallKnob),
                    ],
                ),
                LayoutNode::Pages(tabs),
            ]),
        )
    };
    // The Expanded View (R66): every source with its routes and every
    // destination with its amount, for both engines.
    let expanded = LayoutNode::Cell(DisplayCell::new(
        EXPANDED_CELL,
        EXPANDED_WIDTH,
        BODY_HEIGHT,
        1.0,
    ));
    let layout = DeviceLayout::new(LayoutNode::Row(vec![
        LayoutNode::Pages(PagesNode::new(
            "Engine",
            PageStyle::Tabs,
            vec![engine_page(0), engine_page(1)],
        )),
        band(
            BODY_HEIGHT,
            &[
                (param(0, VOLUME), Widget::Knob),
                (param(0, PAN), Widget::Knob),
                (param(0, TONE), Widget::Knob),
                (param(1, VOLUME), Widget::Knob),
                (param(1, PAN), Widget::Knob),
                (param(1, TONE), Widget::Knob),
            ],
        ),
        band(
            BODY_HEIGHT,
            &[
                (MODE, Widget::Chooser),
                (VOICES, Widget::Number),
                (LEGATO, Widget::Toggle),
                (STACK, Widget::Number),
                (SPREAD, Widget::Knob),
                (DRIVE, Widget::Knob),
                (LIMIT, Widget::Toggle),
                (OUTPUT, Widget::Knob),
                (PB_RANGE, Widget::Number),
            ],
        ),
    ]))
    .with_expanded(expanded);
    let engine_bank = |letter: &str| {
        let name = |what: &str| format!("{letter} {what}");
        vec![
            (
                name("Oscillator"),
                vec![
                    name("On"),
                    name("Type"),
                    name("Octaves"),
                    name("Semitones"),
                    name("Cents"),
                    name("Macro 1"),
                    name("Macro 2"),
                ],
            ),
            (
                name("Filter"),
                vec![
                    name("Filter"),
                    name("Filter Type"),
                    name("Filter Frequency"),
                    name("Filter Macro 1"),
                    name("Filter Macro 2"),
                ],
            ),
            (
                name("Envelope"),
                vec![
                    name("Attack"),
                    name("Decay"),
                    name("Sustain"),
                    name("Release"),
                    name("Attack Slope"),
                    name("Decay Slope"),
                    name("Release Slope"),
                    name("Envelope Loop Mode"),
                ],
            ),
            (
                name("Modulation Envelope"),
                vec![
                    name("Mod Attack"),
                    name("Mod Decay"),
                    name("Mod Sustain"),
                    name("Mod Release"),
                    name("Mod Attack Slope"),
                    name("Mod Decay Slope"),
                    name("Mod Release Slope"),
                    name("Mod Loop Mode"),
                ],
            ),
            (
                name("Modulation Envelope Levels"),
                vec![
                    name("Mod Initial"),
                    name("Mod Peak"),
                    name("Mod Final"),
                    name("Link Envelopes"),
                ],
            ),
            (
                name("LFO 1"),
                vec![
                    name("LFO 1 Type"),
                    name("LFO 1 Sync"),
                    name("LFO 1 Rate"),
                    name("LFO 1 Division"),
                    name("LFO 1 Macro 1"),
                    name("LFO 1 Macro 2"),
                    name("LFO 1 Phase Offset"),
                    name("LFO 1 Retrigger"),
                ],
            ),
            (
                name("LFO 1 FX"),
                vec![
                    name("LFO 1 FX 1"),
                    name("LFO 1 FX 1 Amount"),
                    name("LFO 1 FX 2"),
                    name("LFO 1 FX 2 Amount"),
                ],
            ),
            (
                name("LFO 2"),
                vec![
                    name("LFO 2 Type"),
                    name("LFO 2 Sync"),
                    name("LFO 2 Rate"),
                    name("LFO 2 Division"),
                    name("LFO 2 Phase Offset"),
                    name("LFO 2 Retrigger"),
                ],
            ),
            (name("Mix"), vec![name("Volume"), name("Pan"), name("Tone")]),
            (
                name("Settings"),
                vec![
                    name("Osc Key Tracking"),
                    name("Phase Reset"),
                    name("Phase Spread"),
                    name("Oscillator Scale Awareness"),
                    name("Filter Scale Awareness"),
                    name("Glide Time"),
                    name("Glide Mode"),
                ],
            ),
        ]
    };
    let mut banks = engine_bank("A");
    banks.extend(engine_bank("B"));
    banks.push((
        "Global".to_owned(),
        [
            "Voice Mode",
            "Voices",
            "Legato",
            "Stacked Voices",
            "Spread",
            "Mixer Drive",
            "Limit",
            "Volume",
        ]
        .map(str::to_owned)
        .to_vec(),
    ));
    banks.push(("Pitch Bend".to_owned(), vec!["PB Range".to_owned()]));
    for bank in 0..ROUTES / 8 {
        banks.push((
            format!("Routes {} to {}", bank * 8 + 1, bank * 8 + 8),
            (0..8)
                .map(|i| format!("#{}", route_amount(bank * 8 + i).0))
                .collect(),
        ));
    }
    let banks: Vec<(&str, Vec<&str>)> = banks
        .iter()
        .map(|(name, params)| (name.as_str(), params.iter().map(String::as_str).collect()))
        .collect();
    let banks: Vec<(&str, &[&str])> = banks.iter().map(|(n, p)| (*n, p.as_slice())).collect();
    // Spec: meld R3, R5, R12. The macros are named by their type, and
    // Semitones read in scale degrees while Use Current Scale is on.
    let relabels = (0..2)
        .flat_map(|engine| {
            let by = |param, by, names: Vec<&str>| Relabel {
                param,
                by,
                labels: names.into_iter().map(ParamLabel::named).collect(),
            };
            [
                by(
                    param(engine, MACRO_1),
                    param(engine, TYPE),
                    MACROS.iter().map(|m| m.0).collect(),
                ),
                by(
                    param(engine, MACRO_2),
                    param(engine, TYPE),
                    MACROS.iter().map(|m| m.1).collect(),
                ),
                by(
                    param(engine, FILTER_MACRO_1),
                    param(engine, FILTER_TYPE),
                    FILTER_MACROS.iter().map(|m| m.0).collect(),
                ),
                by(
                    param(engine, FILTER_MACRO_2),
                    param(engine, FILTER_TYPE),
                    FILTER_MACROS.iter().map(|m| m.1).collect(),
                ),
                by(
                    param(engine, LFO1_MACRO_1),
                    param(engine, LFO1_TYPE),
                    LFO1_MACROS.iter().map(|m| m.0).collect(),
                ),
                by(
                    param(engine, LFO1_MACRO_2),
                    param(engine, LFO1_TYPE),
                    LFO1_MACROS.iter().map(|m| m.1).collect(),
                ),
                by(
                    param(engine, FX1_AMOUNT),
                    param(engine, FX1_TYPE),
                    FX_MACROS.to_vec(),
                ),
                by(
                    param(engine, FX2_AMOUNT),
                    param(engine, FX2_TYPE),
                    FX_MACROS.to_vec(),
                ),
                Relabel {
                    param: param(engine, SEMITONES),
                    by: USE_SCALE,
                    labels: vec![
                        ParamLabel::named("Semitones"),
                        ParamLabel {
                            name: "Semitones".to_owned(),
                            unit: Some("sd".to_owned()),
                            law: None,
                        },
                    ],
                },
            ]
        })
        .collect();
    DeviceDescription::new(params)
        .with_layout(layout)
        .with_title_switch(USE_SCALE)
        .with_relabels(relabels)
        .with_banks(&banks)
        .with_grid_params((0..ROUTES).map(route_amount).collect())
}

/// Spec: meld R1. Meld's factory presets of Beatlayer's own: each a name and
/// the parameters it sets, the rest staying at their defaults.
#[must_use]
pub fn factory_presets() -> Vec<(&'static str, Vec<(ParamId, f64)>)> {
    let (a, b) = (|offset| param(0, offset), |offset| param(1, offset));
    vec![
        ("Init", vec![]),
        (
            "Warm Swarm",
            vec![
                (a(TYPE), 8.0),
                (a(MACRO_1), 15.0),
                (a(MACRO_2), 0.0),
                (a(FILTER_FREQ), 1_800.0),
                (a(FILTER_MACRO_1), 20.0),
                (a(ATTACK), 120.0),
                (a(RELEASE), 600.0),
                (a(VOLUME), db_to_gain(-6.0)),
                (b(ON), 1.0),
                (b(TYPE), 6.0),
                (b(OCTAVES), -1.0),
                (b(MACRO_1), 10.0),
                (b(VOLUME), db_to_gain(-9.0)),
                (STACK, 2.0),
            ],
        ),
        (
            "Fifth Pluck",
            vec![
                (a(TYPE), 4.0),
                (a(MACRO_1), 60.0),
                (a(MACRO_2), 50.0),
                (a(FILTER_FREQ), 3_200.0),
                (a(FILTER_MACRO_1), 35.0),
                (a(DECAY), 220.0),
                (a(SUSTAIN), 0.0),
                (a(RELEASE), 120.0),
            ],
        ),
        (
            "Sub and Sync",
            vec![
                (a(TYPE), 5.0),
                (a(MACRO_1), 30.0),
                (a(MACRO_2), 70.0),
                (a(OCTAVES), -1.0),
                (b(ON), 1.0),
                (b(TYPE), 3.0),
                (b(MACRO_1), 20.0),
                (b(MACRO_2), 55.0),
                (b(FILTER_ON), 1.0),
                (b(FILTER_FREQ), 900.0),
                (b(FILTER_MACRO_1), 40.0),
                (b(VOLUME), db_to_gain(-12.0)),
                (MODE, 1.0),
                (LEGATO, 1.0),
                (a(GLIDE_TIME), 80.0),
            ],
        ),
    ]
}

/// Engine `engine`'s envelopes, LFOs and LFO 1 FX (R59 to R61).
fn modulation_params(engine: usize, letter: &str) -> Vec<ParamDescriptor> {
    let name = |what: &str| format!("{letter} {what}");
    let id = |offset| param(engine, offset);
    let slope = |offset, what: &str, default: f64, text: &str| {
        ParamDescriptor::continuous(
            id(offset),
            &name(what),
            (-100.0, 100.0),
            default,
            ParamUnit::Percent,
        )
        .describe(text)
    };
    let level = |offset, what: &str, default: f64, text: &str| {
        ParamDescriptor::continuous(
            id(offset),
            &name(what),
            (-100.0, 100.0),
            default,
            ParamUnit::Percent,
        )
        .describe(text)
    };
    let mode = |offset, what: &str| {
        ParamDescriptor::choice(id(offset), &name(what), &LOOP_MODES, 0).describe(
            "None holds Sustain until the note ends; Trigger runs Attack, Decay and Release once; \
             Loop cycles the whole envelope until the note ends; AD Loop cycles Attack and Decay \
             while the key is held.",
        )
    };
    let hertz = |offset, what: &str, default: f64| {
        ParamDescriptor::continuous(
            id(offset),
            &name(what),
            (0.01, 40.0),
            default,
            ParamUnit::Hertz,
        )
        .logarithmic()
        .describe("How fast the LFO cycles, in hertz.")
    };
    let sync = |offset, what: &str| {
        ParamDescriptor::toggle(id(offset), &name(what), false)
            .describe("Sets the LFO's rate in divisions of the song's tempo instead of hertz.")
    };
    let division = |offset, what: &str| {
        ParamDescriptor::choice(id(offset), &name(what), &DIVISIONS.map(|(n, _)| n), 4)
            .describe("The length of one cycle with sync on, from 1/64 to 8 bars.")
    };
    let phase = |offset, what: &str| {
        ParamDescriptor::continuous(id(offset), &name(what), (0.0, 360.0), 0.0, ParamUnit::None)
            .decimals(0)
            .describe("Where in its cycle the LFO starts, in degrees, and the phase it keeps against the song.")
    };
    let retrigger = |offset, what: &str| {
        ParamDescriptor::toggle(id(offset), &name(what), false).describe(
            "Starts the LFO again at its phase offset with each note; off, it follows the song \
             position while the song plays.",
        )
    };
    vec![
        slope(
            ATTACK_SLOPE,
            "Attack Slope",
            0.0,
            "Bends the amplitude envelope's attack: positive starts fast and ends slow, negative the reverse.",
        ),
        slope(
            DECAY_SLOPE,
            "Decay Slope",
            50.0,
            "Bends the amplitude envelope's decay: positive starts fast and ends slow, negative the reverse.",
        ),
        slope(
            RELEASE_SLOPE,
            "Release Slope",
            50.0,
            "Bends the amplitude envelope's release: positive starts fast and ends slow, negative the reverse.",
        ),
        mode(ENV_MODE, "Envelope Loop Mode"),
        time(
            id(MOD_ATTACK),
            &name("Mod Attack"),
            1.0,
            "How long the modulation envelope takes to rise from Initial to Peak.",
        ),
        time(
            id(MOD_DECAY),
            &name("Mod Decay"),
            600.0,
            "How long the modulation envelope takes to fall from Peak to Sustain.",
        ),
        ParamDescriptor::continuous(
            id(MOD_SUSTAIN),
            &name("Mod Sustain"),
            (0.0, 100.0),
            0.0,
            ParamUnit::Percent,
        )
        .describe("The share of Peak the modulation envelope holds while a note lasts."),
        time(
            id(MOD_RELEASE),
            &name("Mod Release"),
            50.0,
            "How long the modulation envelope takes to fall to Final after the note ends.",
        ),
        slope(
            MOD_ATTACK_SLOPE,
            "Mod Attack Slope",
            0.0,
            "Bends the modulation envelope's attack.",
        ),
        slope(
            MOD_DECAY_SLOPE,
            "Mod Decay Slope",
            50.0,
            "Bends the modulation envelope's decay.",
        ),
        slope(
            MOD_RELEASE_SLOPE,
            "Mod Release Slope",
            50.0,
            "Bends the modulation envelope's release.",
        ),
        level(
            MOD_INITIAL,
            "Mod Initial",
            0.0,
            "The level the modulation envelope starts from when a note begins.",
        ),
        level(
            MOD_PEAK,
            "Mod Peak",
            100.0,
            "The level the modulation envelope's attack rises to.",
        ),
        level(
            MOD_FINAL,
            "Mod Final",
            0.0,
            "The level the modulation envelope's release falls to.",
        ),
        mode(MOD_MODE, "Mod Loop Mode"),
        ParamDescriptor::toggle(id(LINK), &name("Link Envelopes"), false).describe(
            "Makes the modulation envelope take the amplitude envelope's times and slopes.",
        ),
        ParamDescriptor::choice(id(LFO1_TYPE), &name("LFO 1 Type"), &LFO1_TYPES, 0)
            .describe("LFO 1's type: Basic Shapes, Ramp, Wander, Alternate, Euclid or Pulsate."),
        sync(LFO1_SYNC, "LFO 1 Sync"),
        hertz(LFO1_RATE, "LFO 1 Rate", 1.0),
        division(LFO1_DIVISION, "LFO 1 Division"),
        macro_param(
            id(LFO1_MACRO_1),
            &name("LFO 1 Macro 1"),
            "LFO 1's first macro.",
        ),
        macro_param(
            id(LFO1_MACRO_2),
            &name("LFO 1 Macro 2"),
            "LFO 1's second macro.",
        ),
        phase(LFO1_OFFSET, "LFO 1 Phase Offset"),
        retrigger(LFO1_RETRIGGER, "LFO 1 Retrigger"),
        ParamDescriptor::choice(id(FX1_TYPE), &name("LFO 1 FX 1"), &FX_NAMES, 0)
            .describe("The first effect LFO 1's output passes through."),
        macro_param(
            id(FX1_AMOUNT),
            &name("LFO 1 FX 1 Amount"),
            "The first effect's macro.",
        ),
        ParamDescriptor::choice(id(FX2_TYPE), &name("LFO 1 FX 2"), &FX_NAMES, 0)
            .describe("The second effect, after the first."),
        macro_param(
            id(FX2_AMOUNT),
            &name("LFO 1 FX 2 Amount"),
            "The second effect's macro.",
        ),
        ParamDescriptor::choice(id(LFO2_TYPE), &name("LFO 2 Type"), &LFO2_TYPES, 0)
            .describe("LFO 2's wave: Sine, Tri, Saw Up, Saw Down, Rectangle or Random S&H."),
        sync(LFO2_SYNC, "LFO 2 Sync"),
        hertz(LFO2_RATE, "LFO 2 Rate", 6.0),
        division(LFO2_DIVISION, "LFO 2 Division"),
        phase(LFO2_OFFSET, "LFO 2 Phase Offset"),
        retrigger(LFO2_RETRIGGER, "LFO 2 Retrigger"),
    ]
}

/// A macro, 0 to 100 %.
fn macro_param(id: ParamId, name: &str, text: &str) -> ParamDescriptor {
    ParamDescriptor::continuous(id, name, (0.0, 100.0), 0.0, ParamUnit::Percent).describe(text)
}

/// An envelope time, 0 ms to 20 s.
fn time(id: ParamId, name: &str, default_ms: f64, text: &str) -> ParamDescriptor {
    ParamDescriptor::continuous(
        id,
        name,
        (0.0, 20_000.0),
        default_ms,
        ParamUnit::Milliseconds,
    )
    .power(2)
    .describe(text)
}

/// Adds Meld to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, meld_type(), |device, config| {
        Meld::new(device, config)
    });
}

/// Meld's audio-thread half.
pub struct Meld {
    engines: [Engine; 2],
    rate: f32,
    /// Each slot's limiter gain, and the stretch's summed output.
    limiter: [f32; SLOTS],
    out: [Vec<f32>; 2],
}

impl Meld {
    /// Makes a Meld for `device` at `config`'s rate and block size, holding
    /// the device's routes, whose random draws follow the device's ID
    /// (device framework R29). Every buffer and table it needs is made here,
    /// off the audio thread.
    #[must_use]
    pub fn new(device: &bl_model::Device, config: &NodeConfig) -> Self {
        let rate = config.sample_rate.as_f64();
        let frames = config.max_frames.max(1);
        let seed = device.id.0.get();
        Self {
            engines: [
                Engine::new(0, rate, frames, seed, device),
                Engine::new(1, rate, frames, seed, device),
            ],
            rate: rate as f32,
            limiter: [1.0; SLOTS],
            out: [vec![0.0; frames], vec![0.0; frames]],
        }
    }
}

impl NativeDevice for Meld {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        let frames = audio.frames();
        if frames == 0 || audio.channels() == 0 {
            return;
        }
        for engine in &mut self.engines {
            let clock = engine.section.clock();
            engine.voices.begin(audio, params, clock);
            engine.section.process(audio, params, &mut engine.voices);
        }
        self.mix(frames, params);
        for channel in 0..audio.channels() {
            audio
                .channel_mut(channel)
                .copy_from_slice(&self.out[channel.min(1)][..frames]);
        }
    }

    fn flush(&mut self, _events: &[NodeEvent], _params: &ParamBank) {
        for engine in &mut self.engines {
            engine.section.reset();
            engine.voices.silence();
        }
        self.limiter = [1.0; SLOTS];
    }

    fn receive_data(&mut self, data: NodeData) {
        for engine in &mut self.engines {
            engine.section.receive_data(&data);
            if let Some(routes) = data.downcast_ref::<bl_engine::ModRoutes>() {
                engine.voices.mods.matrix.set_free(routes.0);
            }
        }
    }

    fn matrix_played(&self, report: &mut dyn FnMut(ParamId, f64)) {
        let descs = meld_matrix::descriptors();
        // Each engine's destinations, and the global ones from engine A's.
        for (engine, voices) in self.engines.iter().map(|e| &e.voices).enumerate() {
            let globals = if engine == 0 {
                meld_matrix::D_DRIVE..meld_matrix::DESTS
            } else {
                0..0
            };
            let own = engine * meld_matrix::PER_ENGINE..(engine + 1) * meld_matrix::PER_ENGINE;
            for dest in own.chain(globals) {
                if descs[dest].id.0 == u32::MAX {
                    continue;
                }
                if let Some(position) = voices.mods.matrix.shown(dest) {
                    report(descs[dest].id, descs[dest].denormalize(position));
                }
            }
        }
    }
}

impl Meld {
    /// The two engines' slots summed, driven and limited each, then summed
    /// into the output under Volume (R18).
    fn mix(&mut self, frames: usize, params: &ParamBank) {
        let value = |id: ParamId| params.value(params.index(id).expect("the description lists it"));
        let ramp = |id: ParamId| params.ramp(params.index(id).expect("the description lists it"));
        let drive_db = value(DRIVE);
        let limit = value(LIMIT) >= 0.5;
        let volume = ramp(OUTPUT);
        // A route into Mixer Drive or Volume reads the oldest voice's
        // sources, as they were at each frame (R62).
        let global = &self.engines[0].voices.mods;
        let moved = (
            global.has[meld_matrix::D_DRIVE],
            global.has[meld_matrix::D_OUTPUT],
        );
        let state = self.engines[0].section.state();
        let clock = state.clock(0);
        let used = self.engines[0]
            .voices
            .used()
            .max(self.engines[1].voices.used());
        let release = 1.0 - (-1.0 / (LIMIT_RELEASE * self.rate)).exp();
        let [a, b] = &self.engines;
        let (a, b) = (&a.voices.out, &b.voices.out);
        let [left, right] = &mut self.out;
        left[..frames].fill(0.0);
        right[..frames].fill(0.0);
        for frame in 0..frames {
            let (mut sum_l, mut sum_r) = (0.0f32, 0.0f32);
            let due = (clock + frame as u64).is_multiple_of(32);
            if moved.0 || moved.1 {
                global.matrix.set_oldest(global.oldest(frame));
            }
            let drive_db = if moved.0 {
                global.value(
                    meld_matrix::D_DRIVE,
                    drive_db,
                    0,
                    frame,
                    due,
                    &GlobalRead(global),
                )
            } else {
                drive_db
            };
            let (drive, gain) = (drive_db / 24.0, db_to_gain(drive_db) as f32);
            let level = if moved.1 {
                global.value(
                    meld_matrix::D_OUTPUT,
                    f64::from(volume[frame]),
                    0,
                    frame,
                    due,
                    &GlobalRead(global),
                ) as f32
            } else {
                volume[frame]
            };
            for slot in 0..used {
                let at = frame * SLOTS + slot;
                let (mut l, mut r) = (a[0][at] + b[0][at], a[1][at] + b[1][at]);
                if l == 0.0 && r == 0.0 {
                    self.limiter[slot] = 1.0;
                    continue;
                }
                if drive > 0.0 {
                    let drive = drive as f32;
                    l += drive * ((gain * l).tanh() - l);
                    r += drive * ((gain * r).tanh() - r);
                }
                if limit {
                    let peak = l.abs().max(r.abs());
                    let hold = &mut self.limiter[slot];
                    *hold += (1.0 - *hold) * release;
                    if peak * *hold > 1.0 {
                        *hold = 1.0 / peak;
                    }
                    l *= *hold;
                    r *= *hold;
                }
                sum_l += l;
                sum_r += r;
            }
            left[frame] = sum_l * level;
            right[frame] = sum_r * level;
        }
    }
}

/// One engine: its voice section, and the sound engine that section plays.
struct Engine {
    section: Section,
    voices: Voices,
}

impl Engine {
    fn new(which: usize, rate: f64, frames: usize, seed: u64, device: &bl_model::Device) -> Self {
        let id = |offset| Some(param(which, offset));
        Self {
            // Spec: meld R14, R16. Both engines play the same notes under
            // the same global settings, each with its own Glide and scale
            // awareness.
            section: Section::new(
                Parts {
                    voices: Some(VOICES),
                    voice_mode: Some(MODE),
                    legato: Some(LEGATO),
                    glide_time: id(GLIDE_TIME),
                    glide_mode: id(GLIDE_MODE),
                    poly_glide: true,
                    use_scale: Some(USE_SCALE),
                    scale_and: id(OSC_SCALE),
                    unison_voices: Some(STACK),
                    pb_range: Some(PB_RANGE),
                    ..Parts::default()
                },
                MAX_VOICES,
                MAX_STACK,
                rate,
                0x4D45_4C44 ^ seed,
            ),
            voices: Voices::new(which, rate, frames, seed, device),
        }
    }
}

/// An engine's controls for the stretch, read at its start.
#[derive(Clone, Copy, Default)]
struct Snapshot {
    on: bool,
    kind: usize,
    octaves: f32,
    cents: f32,
    semitones: i32,
    filter_on: bool,
    filter_type: usize,
    key_tracking: bool,
    phase_reset: bool,
    phase_spread: f32,
    osc_scale: bool,
    filter_scale: bool,
    spread: f32,
}

/// The controls worked out on the grid and held between its points.
#[derive(Clone, Copy, Default)]
struct Held {
    macro_1: f32,
    macro_2: f32,
}

/// What a slot holds: the voice and copy it plays and what it knew at its
/// note's start.
#[derive(Clone, Copy)]
struct Slot {
    voice: i8,
    copies: u8,
    key: u8,
    /// The pitch offset Semitones gives, and the key an engine without key
    /// tracking plays.
    offset: f32,
    fixed: f32,
    /// The start phase in degrees.
    phase: f32,
    gains: [f32; MAX_LANES],
    drift: [f32; MAX_LANES],
    target: [f32; MAX_LANES],
    clock: [f32; MAX_LANES],
    noise: [f32; 2],
    noise_state: u32,
    fresh: bool,
    tone: f32,
    /// The seed of the note's random draws (R30, R33, R35).
    seed: u64,
    /// The Spread source's value for the slot's copy (R15, R62).
    spread: f32,
    /// What the grid carries on for the type: Chip's arpeggio in cycles,
    /// Shepard's Pi's slide in octaves, Noise Loop's place among its loops.
    run: f32,
    /// The FM types' modulation index at the last chunk's end.
    index: f32,
    /// The note's frequency at the start of the chunk of the grid the
    /// generators and FM types are in, NaN before the slot's first chunk.
    chunk_pitch: f32,
}

impl Slot {
    const FREE: Self = Self {
        voice: -1,
        copies: 1,
        key: 0,
        offset: 0.0,
        fixed: 60.0,
        phase: 0.0,
        gains: [0.0; MAX_LANES],
        drift: [0.0; MAX_LANES],
        target: [0.0; MAX_LANES],
        clock: [0.0; MAX_LANES],
        noise: [0.0; 2],
        noise_state: 1,
        fresh: false,
        tone: 0.0,
        seed: 0,
        spread: 0.0,
        run: 0.0,
        index: 0.0,
        chunk_pitch: f32::NAN,
    };
}

/// An engine's sound: the [`Instrument`] its voice section plays.
struct Voices {
    which: usize,
    rate: f64,
    seed: u64,
    banks: [Bank; Kind::ALL.len()],
    fm: fm::Core,
    gens: Vec<Gen>,
    /// The pitch, in semitones, at which a lane's partial nears Nyquist.
    top: f32,
    /// Both engines' modulators and the matrix (R59 to R62).
    mods: Mods,
    filter: MeldFilter,
    slots: [Slot; SLOTS],
    lanes: Vec<meld_osc::Lanes>,
    snap: Snapshot,
    held: Held,
    held_valid: bool,
    current: usize,
    rng: Random,
    notes: u32,
    /// The stretch's controls, frame by frame.
    macro_1: Vec<f32>,
    macro_2: Vec<f32>,
    cutoff: Vec<f32>,
    res: Vec<f32>,
    morph: Vec<f32>,
    drive: Vec<f32>,
    volume: Vec<f32>,
    pan: Vec<f32>,
    tone: Vec<f32>,
    /// The piece's per-voice signal, in the filter's layout.
    signal: Vec<f32>,
    cutoffs: Vec<f32>,
    /// The stretch's output, slot by slot, per channel.
    out: [Vec<f32>; 2],
    filter_stride: usize,
    /// The filter's Resonance and Morph for each slot and frame, when a
    /// route moves them.
    res_each: Vec<f32>,
    morph_each: Vec<f32>,
}

impl Voices {
    fn new(which: usize, rate: f64, frames: usize, seed: u64, device: &bl_model::Device) -> Self {
        let banks = Kind::ALL.map(|kind| {
            Bank::new(
                kind,
                SLOTS,
                rate as f32,
                0x6D31 + which as u64 + (seed << 1),
            )
        });
        let filter = MeldFilter::new(rate as f32, SLOTS, frames);
        let filter_stride = filter.stride();
        let ramp = || vec![0.0; frames];
        Self {
            which,
            rate,
            seed,
            banks,
            fm: fm::Core::with_voices(rate, 1, seed, SLOTS, 0),
            gens: (0..SLOTS).map(|_| Gen::new(rate as f32)).collect(),
            top: 69.0 + 12.0 * (0.45 * rate as f32 / 440.0).log2(),
            mods: Mods::new(device, rate, frames, seed),
            filter,
            slots: [Slot::FREE; SLOTS],
            lanes: vec![meld_osc::Lanes::default(); SLOTS],
            snap: Snapshot::default(),
            held: Held::default(),
            held_valid: false,
            current: usize::MAX,
            rng: Random::seeded(0xD1F7 + which as u64 + seed),
            notes: 0,
            macro_1: ramp(),
            macro_2: ramp(),
            cutoff: ramp(),
            res: ramp(),
            morph: ramp(),
            drive: ramp(),
            volume: ramp(),
            pan: ramp(),
            tone: ramp(),
            signal: vec![0.0; frames * 2 * filter_stride],
            cutoffs: vec![0.0; frames * filter_stride],
            out: [vec![0.0; frames * SLOTS], vec![0.0; frames * SLOTS]],
            filter_stride,
            res_each: vec![0.0; frames * filter_stride],
            morph_each: vec![0.0; frames * filter_stride],
        }
    }

    /// The slots in use: one past the highest.
    fn used(&self) -> usize {
        self.slots
            .iter()
            .rposition(|s| s.voice >= 0)
            .map_or(0, |i| i + 1)
    }

    /// Forgets every note, as a switched-off device does.
    fn silence(&mut self) {
        self.mods.silence();
        self.slots = [Slot::FREE; SLOTS];
        self.held_valid = false;
    }

    /// Reads the stretch's parameters.
    fn begin(&mut self, audio: &Audio<'_, '_>, params: &ParamBank, clock: u64) {
        let frames = audio.frames();
        self.mods.begin(audio, params, clock);
        let which = self.which;
        let value = |id: ParamId| params.value(params.index(id).expect("the description lists it"));
        let ramp = |to: &mut [f32], id: ParamId, scale: f32| {
            let from = params.ramp(params.index(id).expect("the description lists it"));
            for (to, from) in to.iter_mut().zip(from) {
                *to = from * scale;
            }
        };
        let id = |offset| param(which, offset);
        ramp(&mut self.macro_1[..frames], id(MACRO_1), 0.01);
        ramp(&mut self.macro_2[..frames], id(MACRO_2), 0.01);
        ramp(&mut self.res[..frames], id(FILTER_MACRO_1), 0.01);
        ramp(&mut self.morph[..frames], id(FILTER_MACRO_2), 0.0075);
        ramp(&mut self.volume[..frames], id(VOLUME), 1.0);
        ramp(&mut self.pan[..frames], id(PAN), 0.01);
        ramp(&mut self.tone[..frames], id(TONE), 0.01);
        ramp(&mut self.cutoff[..frames], id(FILTER_FREQ), 1.0);
        ramp(&mut self.drive[..frames], id(FILTER_MACRO_2), 0.01);
        let snap = Snapshot {
            on: value(id(ON)) >= 0.5,
            kind: value(id(TYPE)) as usize,
            octaves: value(id(OCTAVES)) as f32,
            cents: value(id(CENTS)) as f32,
            semitones: value(id(SEMITONES)) as i32,
            filter_on: value(id(FILTER_ON)) >= 0.5,
            filter_type: value(id(FILTER_TYPE)) as usize,
            key_tracking: value(id(KEY_TRACKING)) >= 0.5,
            phase_reset: value(id(PHASE_RESET)) >= 0.5,
            phase_spread: (value(id(PHASE_SPREAD)) / 100.0) as f32,
            osc_scale: value(id(OSC_SCALE)) >= 0.5,
            filter_scale: value(id(FILTER_SCALE)) >= 0.5,
            spread: (value(SPREAD) / 100.0) as f32,
        };
        if snap.kind != self.snap.kind || self.current == usize::MAX {
            self.change_type(snap.kind);
        }
        self.snap = snap;
        self.filter.set_type(snap.filter_type);
        for k in 0..frames {
            self.cutoffs[k * self.filter_stride..(k + 1) * self.filter_stride]
                .fill(pitch_of(self.cutoff[k], 440.0));
        }
        for channel in &mut self.out {
            channel[..frames * SLOTS].fill(0.0);
        }
    }

    /// Moves the sounding notes to type `kind`, as a note's start would,
    /// and gives the swarms their wave and the FM types their operators.
    fn change_type(&mut self, kind: usize) {
        let first = self.current == usize::MAX;
        self.current = kind;
        if matches!(kind, 6..=9) {
            self.banks[Kind::Swarm.index()].set_shape(meld_osc::swarm_shape(kind));
        }
        self.setup_fm(kind);
        if first {
            return;
        }
        for slot in 0..SLOTS {
            if self.slots[slot].voice >= 0 {
                let s = &mut self.slots[slot];
                s.run = initial_run(kind, s.noise_state);
                s.target[0] = s.run;
                s.index = 0.0;
                s.chunk_pitch = f32::NAN;
                self.start_sound(slot, true);
            }
        }
    }

    /// Spec: meld R24 to R27. Sets the FM core's operators for FM type
    /// `kind`: Harmonic FM, Squelch and Simple FM are B into A (algorithm 8
    /// of Operator, with C and D silent), Fold FM is D into a sine A and a
    /// triangle B (algorithm 9). Every wave is a band-limited table.
    fn setup_fm(&mut self, kind: usize) {
        let settings = &mut self.fm.settings;
        settings.tone = 1.0;
        settings.antialias = false;
        let (algorithm, waves) = match kind {
            11 => (8, [0, 19, 0, 0]),
            _ => (7, [0, 0, 0, 0]),
        };
        settings.algorithm = algorithm;
        for (op, wave) in settings.ops.iter_mut().zip(waves) {
            op.wave = wave;
        }
    }

    /// Starts slot `slot`'s sound for the current type: its lanes' phase
    /// (R16) or its operators' and generators' state.
    fn start_sound(&mut self, slot: usize, reset: bool) {
        let s = self.slots[slot];
        match self.current {
            10..=13 => {
                if reset {
                    self.fm.settings.ops[0].phase = s.phase / 360.0;
                    self.fm.start(slot, Start::Fresh, 0.0);
                }
            }
            kind if meld_gen::plays(kind) => self.gens[slot].start(s.seed),
            _ => {
                if let Some(bank) = self.bank() {
                    bank.start(slot, f32::from(s.key), Onset::Fresh, reset, s.phase);
                }
            }
        }
    }

    fn bank(&mut self) -> Option<&mut Bank> {
        let kind = Kind::of(self.current)?;
        Some(&mut self.banks[kind.index()])
    }

    /// The engine's pitch for `voice`'s slot `slot` `at` frames into a
    /// render of `frames`, in semitones (R3, R16).
    fn pitch(
        &self,
        voices: &VoiceBlock,
        state: &State,
        slot: usize,
        frames: &std::ops::Range<usize>,
        at: usize,
    ) -> f32 {
        let s = &self.slots[slot];
        let mut pitch = state.pitch(voices, s.voice as usize, frames, at);
        if !self.snap.key_tracking {
            pitch += f64::from(s.fixed) - f64::from(s.key);
        }
        let moved = self.pitch_move(voices, state, slot, frames.start + at);
        (pitch + f64::from(12.0 * self.snap.octaves + self.snap.cents / 100.0 + s.offset + moved))
            as f32
    }

    /// Whether a route moves this engine's pitch.
    fn pitch_routed(&self) -> bool {
        let which = self.which;
        [L_SEMITONES, L_CENTS, L_QUANT]
            .into_iter()
            .any(|local| self.mods.has[meld_matrix::dest(which, local)])
    }

    /// What a route into this engine's Semitones, Cents or Pitch Quant adds
    /// to slot `slot`'s pitch at frame `frame` of the stretch, in semitones
    /// (R62): a Semitones route moves it in semitones (degrees of the scale
    /// when it is on) and Cents in cents; Pitch Quant moves it in whole
    /// steps, rounded.
    fn pitch_move(&self, voices: &VoiceBlock, state: &State, slot: usize, frame: usize) -> f32 {
        let which = self.which;
        let has = |local| self.mods.has[meld_matrix::dest(which, local)];
        if !(has(L_SEMITONES) || has(L_CENTS) || has(L_QUANT)) {
            return 0.0;
        }
        let s = &self.slots[slot];
        let steps = |n: f64| -> f64 {
            let scale = state.scale();
            if scale.enabled {
                let (low, high) = (n.floor(), n.ceil());
                let at = |n: f64| f64::from(scale.move_off(s.key, n as i32) - i32::from(s.key));
                at(low) + (at(high) - at(low)) * (n - low)
            } else {
                n
            }
        };
        let mut moved = 0.0;
        if has(L_SEMITONES) {
            let base = f64::from(self.snap.semitones);
            let value = self.dest_value(voices, state, L_SEMITONES, base, slot, frame);
            moved += steps(value) - f64::from(s.offset);
        }
        if has(L_CENTS) {
            let base = f64::from(self.snap.cents);
            moved += (self.dest_value(voices, state, L_CENTS, base, slot, frame) - base) / 100.0;
        }
        if has(L_QUANT) {
            moved += steps(
                self.dest_value(voices, state, L_QUANT, 0.0, slot, frame)
                    .round(),
            );
        }
        moved as f32
    }

    /// Destination `local` of this engine as slot `slot` plays it at frame
    /// `frame` of the stretch, given the parameter's own `base` value: its
    /// routes' movement, if any reaches it (R5).
    fn dest_value(
        &self,
        voices: &VoiceBlock,
        state: &State,
        local: usize,
        base: f64,
        slot: usize,
        frame: usize,
    ) -> f64 {
        let read = SlotRead {
            mods: &self.mods,
            voices,
            state,
            of: |slot: usize| {
                let s = &self.slots[slot];
                (s.voice.max(0) as usize, s.spread)
            },
        };
        self.mods.value(
            meld_matrix::dest(self.which, local),
            base,
            slot,
            frame,
            state.control_due(frame),
            &read,
        )
    }
}

impl Instrument for Voices {
    fn voice_event(&mut self, voices: &mut VoiceBlock, state: &State, event: VoiceEvent) {
        let voice = event.voice;
        match event.kind {
            VoiceEventKind::Start { start, .. } => {
                self.mods
                    .start(voice, start, state.clock(event.time as usize));
                if matches!(start, Start::Legato) {
                    self.retune_slots(voices, state, voice);
                } else {
                    self.start_slots(voices, state, voice);
                }
            }
            VoiceEventKind::Release => self.mods.release(voice),
            VoiceEventKind::End { .. } => {
                self.mods.end(voice);
                self.free_slots(voice);
            }
        }
    }

    fn render(&mut self, voices: &mut VoiceBlock, state: &State, frames: std::ops::Range<usize>) {
        let count = frames.len();
        if count == 0 {
            return;
        }
        let newest = state.newest(voices);
        let slot = newest.and_then(|voice| self.slots.iter().position(|s| s.voice == voice as i8));
        self.mods.set_newest(newest, slot);
        self.mods.run(voices, state, &frames);
        let used = self.used();
        if used > 0 {
            self.sound(voices, state, &frames, used);
        }
        // R13: a voice ends when both amplitude envelopes have.
        let mut mask = voices.active_mask();
        while mask != 0 {
            let voice = mask.trailing_zeros() as usize;
            mask &= mask - 1;
            if self.mods.idle(voice) {
                voices.end(voice);
            }
        }
    }
}

impl Voices {
    /// Gives `voice` its slots, one per stacked copy, and starts them.
    fn start_slots(&mut self, voices: &VoiceBlock, state: &State, voice: usize) {
        self.free_slots(voice);
        let copies = state.config().copies.clamp(1, MAX_STACK);
        let (key, offset, fixed) = self.tuning(voices, state, voice);
        let spread = self.snap.spread;
        let random = state.random(voice) as f32;
        self.notes = self.notes.wrapping_add(1);
        for copy in 0..copies {
            let Some(slot) = self.slots.iter().position(|s| s.voice < 0) else {
                return;
            };
            // R15: the Spread source of copy i of n, or one value a note.
            let source = if copies > 1 {
                (2.0 * copy as f32 / (copies - 1) as f32 - 1.0) * spread
            } else {
                (2.0 * random - 1.0) * spread
            };
            let phase = if self.snap.phase_reset {
                360.0 * self.snap.phase_spread * source
            } else {
                0.0
            };
            self.slots[slot] = Slot {
                voice: voice as i8,
                copies: copies as u8,
                key,
                offset,
                fixed,
                phase,
                fresh: true,
                spread: source,
                noise_state: (self.notes.wrapping_mul(0x9E37_79B9)
                    ^ (slot as u32) << 8
                    ^ self.seed as u32)
                    | 1,
                seed: note_seed(
                    self.seed ^ (self.which as u64 + 1) << 56,
                    u64::from(self.notes) * SLOTS as u64 + slot as u64,
                ),
                ..Slot::FREE
            };
            self.mods.matrix.start(slot);
            let s = &mut self.slots[slot];
            s.run = initial_run(self.current, s.noise_state);
            s.target[0] = s.run;
            self.filter.reset_voice(slot);
            self.start_sound(slot, self.snap.phase_reset);
        }
    }

    /// `voice`'s key, the pitch offset Semitones gives it, and the key an
    /// engine without key tracking plays (R3, R16).
    fn tuning(&self, voices: &VoiceBlock, state: &State, voice: usize) -> (u8, f32, f32) {
        let key = voices.info(voice).key;
        let scale = *state.scale();
        let semitones = self.snap.semitones;
        let offset = if scale.enabled {
            (scale.move_off(key, semitones) - i32::from(key)) as f32
        } else {
            semitones as f32
        };
        let fixed = 60.0
            + if scale.enabled {
                f32::from(scale.root)
            } else {
                0.0
            };
        (key, offset, fixed)
    }

    /// A legato note keeps `voice`'s slots sounding and gives them its key.
    fn retune_slots(&mut self, voices: &VoiceBlock, state: &State, voice: usize) {
        let (key, offset, fixed) = self.tuning(voices, state, voice);
        for s in &mut self.slots {
            if s.voice == voice as i8 {
                (s.key, s.offset, s.fixed) = (key, offset, fixed);
            }
        }
    }

    fn free_slots(&mut self, voice: usize) {
        for slot in &mut self.slots {
            if slot.voice == voice as i8 {
                *slot = Slot::FREE;
            }
        }
    }

    /// Renders the piece `frames` into the engine's output for the first
    /// `used` slots: oscillators in chunks on the grid, then the filter, the
    /// envelope, Tone, Pan and Volume (R2, R12, R13, R18).
    fn sound(
        &mut self,
        voices: &VoiceBlock,
        state: &State,
        frames: &std::ops::Range<usize>,
        used: usize,
    ) {
        let count = frames.len();
        let on = self.snap.on;
        self.signal[..count * 2 * self.filter_stride].fill(0.0);
        if on {
            let mut at = 0;
            while at < count {
                let clock = state.clock(frames.start + at);
                let n = (CHUNK - (clock % CHUNK as u64) as usize).min(count - at);
                if !self.held_valid || state.control_due(frames.start + at) {
                    self.held = Held {
                        macro_1: self.macro_1[frames.start + at],
                        macro_2: self.macro_2[frames.start + at],
                    };
                    self.held_valid = true;
                }
                if meld_gen::plays(self.current) || is_fm(self.current) {
                    self.voice_chunk(voices, state, frames, at, n, used);
                } else {
                    self.chunk(voices, state, frames, at, n, used);
                }
                at += n;
            }
        }
        if on && self.snap.filter_on {
            let stride = self.filter_stride;
            let (start, end) = (frames.start, frames.end);
            let (_, res, morph) = self.filter_moved(voices, state, frames, used);
            let per = PerFrame {
                cutoff: &self.cutoffs[start * stride..end * stride],
                res: if res {
                    Per::Voice(&self.res_each[start * stride..end * stride])
                } else {
                    Per::Shared(&self.res[start..end])
                },
                morph: if morph {
                    Per::Voice(&self.morph_each[start * stride..end * stride])
                } else {
                    Per::Shared(&self.morph[start..end])
                },
                drive: Per::Shared(&self.drive[start..end]),
            };
            // Spec: meld R52. The resonators snap to the current scale while
            // both Use Current Scale and Filter Scale Awareness are on.
            let scale = (self.snap.filter_scale && state.scale().enabled).then(|| *state.scale());
            let controls = Controls {
                frame: per,
                hz: &self.cutoff[start..end],
                macro_1: &self.res[start..end],
                macro_2: &self.drive[start..end],
                scale,
            };
            self.filter.process(
                used,
                count,
                &controls,
                &mut self.signal[..count * 2 * stride],
            );
        }
        self.mix(voices, state, frames, used);
    }

    /// Works out, for the first `used` slots, the filter controls a route
    /// moves over the piece `frames`: Frequency into the cutoffs, Q and the
    /// morph into their per-slot arrays. Returns which of Frequency, Q and
    /// the morph a route moves (R62).
    fn filter_moved(
        &mut self,
        voices: &VoiceBlock,
        state: &State,
        frames: &std::ops::Range<usize>,
        used: usize,
    ) -> (bool, bool, bool) {
        let which = self.which;
        let has = |local| self.mods.has[meld_matrix::dest(which, local)];
        let moved = (has(L_FREQ), has(L_FILTER_1), has(L_FILTER_2));
        if !(moved.0 || moved.1 || moved.2) {
            return moved;
        }
        let stride = self.filter_stride;
        for frame in frames.clone() {
            for slot in 0..used {
                if moved.0 {
                    let v = self.dest_value(
                        voices,
                        state,
                        L_FREQ,
                        f64::from(self.cutoff[frame]),
                        slot,
                        frame,
                    );
                    self.cutoffs[frame * stride + slot] = pitch_of(v as f32, 440.0);
                }
                if moved.1 {
                    let base = f64::from(self.res[frame]) * 100.0;
                    let v = self.dest_value(voices, state, L_FILTER_1, base, slot, frame);
                    self.res_each[frame * stride + slot] = (v * 0.01) as f32;
                }
                if moved.2 {
                    let base = f64::from(self.morph[frame]) / 0.0075;
                    let v = self.dest_value(voices, state, L_FILTER_2, base, slot, frame);
                    self.morph_each[frame * stride + slot] = (v * 0.0075) as f32;
                }
            }
        }
        moved
    }

    /// The oscillator macros slot `slot` plays at frame `frame` of the
    /// stretch: the held ones, moved by any route (R62).
    fn macros(&self, voices: &VoiceBlock, state: &State, slot: usize, frame: usize) -> (f32, f32) {
        let which = self.which;
        let has = |local| self.mods.has[meld_matrix::dest(which, local)];
        let one = |local, held: f32| {
            if has(local) {
                let v = self.dest_value(voices, state, local, f64::from(held) * 100.0, slot, frame);
                (v * 0.01) as f32
            } else {
                held
            }
        };
        (
            one(L_MACRO_1, self.held.macro_1),
            one(L_MACRO_2, self.held.macro_2),
        )
    }

    /// Renders `n` frames of every slot from `at` into the piece's signal.
    fn chunk(
        &mut self,
        voices: &VoiceBlock,
        state: &State,
        frames: &std::ops::Range<usize>,
        at: usize,
        n: usize,
        used: usize,
    ) {
        let Some(kind) = Kind::of(self.current) else {
            return;
        };
        let bank_at = kind.index();
        let scale = if self.snap.osc_scale {
            *state.scale()
        } else {
            Scale::default()
        };
        let clock = state.clock(frames.start + at);
        let on_grid = clock.is_multiple_of(CHUNK as u64);
        let noisy = self.current == 2;
        let config = state.config();
        let stepping = (config.glissando && config.glide_secs > 0.0) || self.pitch_routed();
        let modulated = stepping;
        for slot in 0..used {
            if self.slots[slot].voice < 0 {
                continue;
            }
            let (m1, m2) = self.macros(voices, state, slot, frames.start + at);
            let motion = m1 * m1 * 6.0;
            let p0 = self.pitch(voices, state, slot, frames, at);
            if on_grid {
                match kind {
                    Kind::Swarm => self.drift(slot, motion),
                    Kind::Chip | Kind::Shepard | Kind::Loop => self.advance(slot, p0, m1, m2),
                    _ => {}
                }
            }
            let s = &self.slots[slot];
            let mut lanes = meld_osc::lanes(
                self.current,
                m1,
                m2,
                &Context {
                    key: s.key,
                    scale: &scale,
                    drift: &s.drift,
                    run: s.run,
                },
            );
            if kind.fades_at_nyquist() {
                for lane in &mut lanes.lane[..lanes.count] {
                    lane.gain *= ((self.top - (p0 + lane.semitones)) * 0.5 + 1.0).clamp(0.0, 1.0);
                }
            }
            let step = (self.pitch(voices, state, slot, frames, at + n) - p0) / n as f32;
            // R29: Shepard's Pi's partials slide, an octave a second at most.
            let slide = if self.current == SHEPARDS_PI {
                12.0 * (m1 - 0.5) * 2.0 / self.rate as f32
            } else {
                0.0
            };
            // The slide moves from where the chunk of the grid began.
            let slid = slide * (clock % CHUNK as u64) as f32;
            let routed = self.pitch_routed();
            let bank = &mut self.banks[bank_at];
            for (copy, lane) in lanes.lane[..lanes.count].iter().enumerate() {
                if routed {
                    // A route moves the pitch frame by frame: each frame's
                    // pitch is worked out whole, from the frame alone, so
                    // the render is the same wherever a block cuts a chunk.
                    bank.set(slot, copy, 0.0, 0.0, lane.frame);
                } else {
                    let step = if stepping { 0.0 } else { step } + slide;
                    bank.set(slot, copy, p0 + lane.semitones + slid, step, lane.frame);
                }
            }
            if routed {
                let mut pitches = [0.0f32; CHUNK];
                for (k, pitch) in pitches[..n].iter_mut().enumerate() {
                    *pitch = self.pitch(voices, state, slot, frames, at + k);
                }
                let across = (clock % CHUNK as u64) as f32;
                for (copy, lane) in lanes.lane[..lanes.count].iter().enumerate() {
                    let mut values = [0.0f32; CHUNK];
                    for (k, value) in values[..n].iter_mut().enumerate() {
                        *value = pitches[k] + lane.semitones + slide * (across + k as f32);
                    }
                    self.banks[bank_at].set_mod(slot, copy, &values[..n]);
                }
            } else if modulated {
                // A Glissando's steps and Noisy Shapes' phase noise move the
                // pitch frame by frame; a glide's line is its own step.
                let mut values = [0.0f32; CHUNK];
                if stepping {
                    for (k, value) in values[..n].iter_mut().enumerate() {
                        *value = self.pitch(voices, state, slot, frames, at + k) - p0;
                    }
                }
                for copy in 0..lanes.count {
                    self.banks[bank_at].set_mod(slot, copy, &values[..n]);
                }
            }
            if noisy {
                let mut noise = [0.0f32; CHUNK];
                let base = (440.0 * ((f64::from(p0) - 69.0) / 12.0).exp2() / self.rate) as f32;
                self.noise(slot, m2, base, &mut noise[..n]);
                for copy in 0..lanes.count {
                    self.banks[bank_at].set_phase(slot, copy, &noise[..n]);
                }
            }
            let s = &mut self.slots[slot];
            if s.fresh {
                s.fresh = false;
                s.gains = lanes.lane.map(|lane| lane.gain);
            }
            self.lanes[slot] = lanes;
        }
        self.banks[bank_at].render(n, used, modulated, noisy);
        let stride = self.filter_stride;
        for slot in 0..used {
            if self.slots[slot].voice < 0 {
                continue;
            }
            let (lanes, s) = (&self.lanes[slot], &mut self.slots[slot]);
            // A lane's gain ramps over the grid's chunk, wherever a block
            // cuts it.
            let done = (clock % CHUNK as u64) as usize;
            for k in 0..n {
                let across = (done + k + 1) as f32 / CHUNK as f32;
                let mut sum = 0.0f32;
                for (copy, lane) in lanes.lane[..lanes.count].iter().enumerate() {
                    let gain = s.gains[copy] + (lane.gain - s.gains[copy]) * across;
                    sum += self.banks[bank_at].out(slot, copy, k) * gain;
                }
                let at = (at + k) * 2 * stride + slot;
                self.signal[at] = sum;
                self.signal[at + stride] = sum;
            }
            if done + n >= CHUNK {
                for (copy, lane) in lanes.lane[..lanes.count].iter().enumerate() {
                    s.gains[copy] = lane.gain;
                }
            }
        }
    }

    /// Moves what slot `slot` carries on a step of the grid, for Chip,
    /// Shepard's Pi and Noise Loop, at pitch `pitch` (R28, R29, R32).
    fn advance(&mut self, slot: usize, pitch: f32, m1: f32, m2: f32) {
        let rate = self.rate as f32;
        let step = CHUNK as f32 / rate;
        let s = &mut self.slots[slot];
        match self.current {
            // R28: Rate, from once to 50 times a second, cycles the arpeggio.
            14 => s.run = (s.run + 50f32.powf(m2) * step).fract(),
            // R29: Rate slides the partials, still at 50 %.
            15 => s.run = (s.run + (m1 - 0.5) * 2.0 * step).rem_euclid(10.0),
            // R32: Rate fresh loops per period; Fade ramps to the next over
            // up to half a period, as steps of the grid.
            _ => {
                let hz = 440.0 * ((pitch - 69.0) / 12.0).exp2();
                s.clock[0] += m1 * hz * step;
                if s.clock[0] >= 1.0 {
                    s.clock[0] = s.clock[0].fract();
                    let frame = (s.draw() >> 8) % LOOPS as u32;
                    s.target[0] = frame as f32;
                    let fade = m2 * 0.5 * rate / hz / CHUNK as f32;
                    if fade <= 1.0 {
                        s.run = s.target[0];
                    } else {
                        s.clock[1] = (s.target[0] - s.run).abs() / fade;
                    }
                }
                let distance = s.target[0] - s.run;
                s.run = if distance.abs() <= s.clock[1] {
                    s.target[0]
                } else {
                    s.run + distance.signum() * s.clock[1]
                };
            }
        }
    }

    /// Renders `n` frames of every slot from `at` for the types that aren't
    /// kit lanes: the FM types on the FM core (R24 to R27) and the noise and
    /// texture types' generators (R30, R33, R35).
    fn voice_chunk(
        &mut self,
        voices: &VoiceBlock,
        state: &State,
        frames: &std::ops::Range<usize>,
        at: usize,
        n: usize,
        used: usize,
    ) {
        let stride = self.filter_stride;
        let hz = |pitch: f32| 440.0 * ((pitch - 69.0) / 12.0).exp2();
        let offset = (state.clock(frames.start + at) % CHUNK as u64) as usize;
        let config = state.config();
        let stepping = (config.glissando && config.glide_secs > 0.0) || self.pitch_routed();
        for slot in 0..used {
            if self.slots[slot].voice < 0 {
                continue;
            }
            let (m1, m2) = self.macros(voices, state, slot, frames.start + at);
            // What a chunk works out follows the pitch at its start, so a
            // block that cuts the chunk changes nothing; a frame's own
            // frequency follows the glide's line, or a Glissando's steps.
            let first = self.pitch(voices, state, slot, frames, at);
            let end = self.pitch(voices, state, slot, frames, at + n);
            let s = &mut self.slots[slot];
            if offset == 0 || !s.chunk_pitch.is_finite() {
                s.chunk_pitch = hz(first);
            }
            let chunk = s.chunk_pitch;
            let mut exact = [0.0f32; CHUNK];
            if stepping {
                for (k, hz_k) in exact[..n].iter_mut().enumerate() {
                    *hz_k = hz(self.pitch(voices, state, slot, frames, at + k));
                }
            }
            let pitch = Pitch {
                chunk,
                first,
                step: (end - first) / n as f32,
                exact: stepping.then_some(&exact[..n]),
                offset,
            };
            let mut piece = [0.0f32; CHUNK];
            if is_fm(self.current) {
                self.fm_piece(slot, &mut piece[..n], &pitch, (m1, m2));
            } else {
                self.gens[slot].render(self.current, &mut piece[..n], &pitch, (m1, m2));
            }
            for (k, sample) in piece[..n].iter().enumerate() {
                let at = (at + k) * 2 * stride + slot;
                self.signal[at] = *sample;
                self.signal[at + stride] = *sample;
            }
        }
    }

    /// Spec: meld R24 to R27. `out` frames of FM slot `slot` for
    /// the note at `pitch` and macros `m`: Amount, and Ratio (Harmonic
    /// FM's whole numbers 1 to 16, Simple FM's 0.5 to 16), Shape (Fold FM)
    /// or Feedback (Squelch).
    ///
    /// The index is held to what keeps the sidebands that matter (to −80 dB)
    /// below Nyquist (R37), the fold taking its share first.
    fn fm_piece(
        &mut self,
        slot: usize,
        out: &mut [f32],
        pitch: &Pitch<'_>,
        (amount, other): (f32, f32),
    ) {
        let rate = self.rate as f32;
        let offset = pitch.offset;
        let mid = pitch.chunk;
        let (ratio, feedback) = match self.current {
            10 => (1.0 + (other * 16.0).floor().min(15.0), 0.0),
            11 => (1.0, 0.0),
            12 => (2.0, other),
            _ => (0.5 * 32f32.powf(other), 0.0),
        };
        let fold = self.current == FOLD_FM;
        let wanted = 8.0 * amount;
        let span = mid * ratio * (1.0 + 1.5 * feedback);
        let (index, bend) = if fold {
            // The triangle's kinks fold into harmonics that fall off as the
            // square of their order: 0.0005 n^2 keeps the last that Nyquist
            // allows below −80 dB.
            let order = 0.5 * rate / mid;
            let kinks = 5e-4 * order * order / other.max(1e-3);
            let bend = std::f32::consts::PI.min(kinks);
            let part = fold_share(order, bend, wanted);
            (part * wanted, part * bend)
        } else {
            (wanted.min(fm_index(mid, span, 8.0, rate)), 0.0)
        };
        // A carrier near Nyquist fades out.
        let carrier = ((0.5 - mid / rate) / 0.05).clamp(0.0, 1.0);
        // Squelch's modulator keeps its own harmonics below Nyquist, which
        // feedback widens, and its feedback speeds the phase it moves the
        // carrier by up to 1 / (1 - k) times, which the index's room covers:
        // the harmonics it can have, and what the index leaves of the room,
        // set how much feedback it gets, and up to 1 radian of the 1.5.
        let room = fm_index(mid, span, 8.0, rate).max(1e-3);
        let feedback_cap = (((0.5 * rate / (mid * ratio)) - 4.0) / 30.0)
            .min((1.0 - index / room).max(0.0) / 1.5)
            .clamp(0.0, 2.0 / 3.0);
        let before = self.slots[slot].index;
        let reach = if fold {
            triangle_reach(rate, mid, before.max(index))
        } else {
            1.0
        };
        let chunk = CHUNK as f32;
        for (k, sample) in out.iter_mut().enumerate() {
            let at = (offset + k) as f32;
            let across = (at + 1.0) / chunk;
            let f = pitch.at(k);
            let level = (before + (index - before) * across) * 0.25;
            let mut amps = [carrier, level, 0.0, 0.0];
            let mut ops = [f, f * ratio, 0.0, 0.0];
            let mut feedbacks = [0.0; fm::OPERATORS];
            if fold {
                // D modulates A (sine) and B (triangle), Shape crossfading.
                amps = [carrier * (1.0 - other), carrier * other, 0.0, level];
                ops = [f, f, 0.0, f];
            } else {
                // R26: the core's feedback grows with the operator's level,
                // so it is divided by it to keep the radians of Feedback.
                feedbacks[1] = feedback * feedback_cap / level.max(1e-3);
            }
            let mut x = self
                .fm
                .play(slot, &ops.map(f64::from), &amps, &feedbacks, reach);
            if fold && bend > 1e-3 {
                x = (bend * x).sin() / bend.min(std::f32::consts::FRAC_PI_2).sin();
            }
            *sample = x;
        }
        // The index reaches its value at the chunk's end, wherever blocks cut
        // it.
        if offset + out.len() >= CHUNK {
            self.slots[slot].index = index;
        }
    }

    /// Spec: meld R11. Moves each copy of swarm slot `slot`'s drift on a
    /// step of the grid: a smoothed random walk, faster with Motion.
    fn drift(&mut self, slot: usize, motion: f32) {
        let step = CHUNK as f32 / self.rate as f32;
        let smooth = (motion * step * 6.0).min(1.0);
        for i in 0..SWARM_COPIES {
            let s = &mut self.slots[slot];
            s.clock[i] += motion * step;
            if s.clock[i] >= 1.0 {
                s.clock[i] -= 1.0;
                s.target[i] = self.rng.next_bipolar() as f32;
            }
            let s = &mut self.slots[slot];
            s.drift[i] += (s.target[i] - s.drift[i]) * smooth;
        }
    }

    /// Spec: meld R7. Phase noise for slot `slot` over a chunk, in cycles:
    /// noise low-passed twice at half the note's frequency, up to a quarter
    /// of a cycle times `rough` at its peaks.
    fn noise(&mut self, slot: usize, rough: f32, base: f32, out: &mut [f32]) {
        let s = &mut self.slots[slot];
        let a = (std::f32::consts::PI * base).min(1.0);
        // The two poles' output has a deviation of about 0.29 √a for white
        // noise of 0.58; 3 deviations make the quarter cycle.
        let scale = 0.25 / 3.0 * rough / (0.289 * a.sqrt());
        for value in out {
            let mut x = s.noise_state;
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            s.noise_state = x;
            let white = (x >> 8) as f32 / (1u32 << 23) as f32 - 1.0;
            s.noise[0] += (white - s.noise[0]) * a;
            s.noise[1] += (s.noise[0] - s.noise[1]) * a;
            *value = (s.noise[1] * scale).clamp(-0.25, 0.25);
        }
    }

    /// Spec: meld R13, R18. Takes the filtered signal under the amplitude
    /// envelope, Tone, Pan and Volume into the stretch's output, each moved
    /// by any route into it (R62).
    fn mix(
        &mut self,
        voices: &VoiceBlock,
        state: &State,
        frames: &std::ops::Range<usize>,
        used: usize,
    ) {
        let count = frames.len();
        let stride = self.filter_stride;
        let on = self.snap.on;
        let which = self.which;
        let moved = |local| self.mods.has[meld_matrix::dest(which, local)];
        let (tone_moved, pan_moved, volume_moved) = (moved(L_TONE), moved(L_PAN), moved(L_VOLUME));
        for slot in 0..used {
            if self.slots[slot].voice < 0 {
                continue;
            }
            let voice = self.slots[slot].voice as usize;
            let copies = f32::from(self.slots[slot].copies).sqrt().recip();
            for k in 0..count {
                let frame = frames.start + k;
                let mut x = if on {
                    self.signal[k * 2 * stride + slot]
                } else {
                    0.0
                };
                let t = if tone_moved {
                    let base = f64::from(self.tone[frame]) * 100.0;
                    (self.dest_value(voices, state, L_TONE, base, slot, frame) * 0.01) as f32
                } else {
                    self.tone[frame]
                };
                let s = &mut self.slots[slot];
                if t.abs() > 1e-4 {
                    // One pole: a high-pass from 20 Hz up to 2 kHz, or a
                    // low-pass from 20 kHz down to 200 Hz (R18).
                    let cutoff = if t > 0.0 {
                        20.0 * 100f32.powf(t)
                    } else {
                        20_000.0 * 0.01f32.powf(-t)
                    };
                    let a = 1.0 - (-std::f32::consts::TAU * cutoff / self.rate as f32).exp();
                    s.tone += (x - s.tone) * a;
                    x = if t > 0.0 { x - s.tone } else { s.tone };
                }
                // The envelope is last, so a voice whose envelope has ended
                // is exactly silent whenever the section frees it.
                x *= self.mods.level[which][k * MAX_VOICES + voice];
                let pan = if pan_moved {
                    let base = f64::from(self.pan[frame]) * 100.0;
                    (self.dest_value(voices, state, L_PAN, base, slot, frame) * 0.01) as f32
                } else {
                    self.pan[frame]
                };
                let volume = if volume_moved {
                    let base = f64::from(self.volume[frame]);
                    self.dest_value(voices, state, L_VOLUME, base, slot, frame) as f32
                } else {
                    self.volume[frame]
                };
                let (left, right) = pan_gains(pan);
                let x = x * volume * copies;
                self.out[0][frame * SLOTS + slot] = x * left;
                self.out[1][frame * SLOTS + slot] = x * right;
            }
        }
    }
}

/// Spec: meld R25, R37. How much of its fold (up to `bend` radians) and
/// of its index (up to `wanted`) Fold FM keeps, from 0 to 1, for a note
/// with `order` harmonics below Nyquist. The fold's harmonics each carry the
/// modulation, so their reaches multiply: both shrink together until the
/// sidebands down to −80 dB fit.
fn fold_share(order: f32, bend: f32, wanted: f32) -> f32 {
    let fits = |part: f32| {
        let g = part * bend;
        (g + 5.0 * g.cbrt() + 3.0) * (1.0 + part * wanted) <= order
    };
    if fits(1.0) {
        return 1.0;
    }
    let (mut low, mut high) = (0.0, 1.0);
    for _ in 0..12 {
        let at = 0.5 * (low + high);
        if fits(at) {
            low = at;
        } else {
            high = at;
        }
    }
    low
}

/// How many times faster than its note a triangle that a modulation of index
/// `beta` phase-modulates is read, for the table that keeps the harmonics
/// whose sidebands still fit below Nyquist: a triangle's harmonic n carries
/// n times the index.
fn triangle_reach(rate: f32, note: f32, beta: f32) -> f64 {
    let room = 0.5 * rate - 3.0 * note;
    let mut harmonics = (room / (note + beta * note)).max(0.0);
    for _ in 0..2 {
        let tail = 5.0 * note * (harmonics * beta).cbrt();
        harmonics = ((room - tail) / (note + beta * note)).max(0.0);
    }
    if harmonics >= 1.0 {
        f64::from((0.8 * rate / note / harmonics).max(1.0))
    } else {
        1_000.0
    }
}

/// How large an FM modulation index may be, at most `limit`, for a carrier
/// at `carrier` hertz and a modulator at `modulator` hertz, so that the
/// sidebands down to −80 dB stay below Nyquist: a Bessel function of index
/// `b` is under 1e-4 past order `b + 5 b^(1/3) + 3` about.
fn fm_index(carrier: f32, modulator: f32, limit: f32, rate: f32) -> f32 {
    let room = (0.5 * rate - carrier) / modulator.max(1.0);
    let reach = |b: f32| b + 5.0 * b.cbrt() + 3.0;
    if reach(0.0) > room {
        return 0.0;
    }
    if reach(limit) <= room {
        return limit;
    }
    let (mut low, mut high) = (0.0, limit);
    for _ in 0..12 {
        let mid = 0.5 * (low + high);
        if reach(mid) <= room {
            low = mid;
        } else {
            high = mid;
        }
    }
    low
}

/// Where a note's `run` starts for type `kind`: Noise Loop's place among its
/// loops from the note's draw, else zero (R32).
fn initial_run(kind: usize, draw: u32) -> f32 {
    if kind == NOISE_LOOP {
        ((draw >> 8) % LOOPS as u32) as f32
    } else {
        0.0
    }
}

impl Slot {
    /// The next of the note's random draws, a 32-bit xorshift.
    fn draw(&mut self) -> u32 {
        let mut x = self.noise_state;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.noise_state = x;
        x
    }
}

//! Wavetable, part 1 (WP-8.9; the wavetable spec, R11 to R24): two wavetable
//! oscillators with Position and the FM, Classic and Modern effects, the sub
//! oscillator, two filters routed Serial, Parallel or Split, the amplitude
//! envelope, the voice section's controls and unison's amount, on the
//! generic panel's tabs.
//!
//! Every voice and unison copy runs in the plug-in kit's SIMD lanes
//! ([`ShapedOscillator`]); Position and the effects' controls reach the
//! oscillator at audio rate, frame by frame. The filters are the
//! instruments' filter, one lane per voice. Wavetables come from the
//! compiler as node data ([`OscTable`]), built off the audio thread.
//!
//! Part 2 (WP-8.10) adds the matrix on the instruments' modulation matrix
//! (64 free slots), Envelopes 2 and 3 and loop modes on every envelope, two
//! LFOs, the MIDI sources, unison's modes and Hi-Quality
//! ([`crate::wavetable_mod`] has the sources' laws). Every destination is
//! worked out per voice and frame where a route reaches it.

use core::f32::consts::{FRAC_1_PI, PI};
use core::ops::Range;
use std::sync::{Arc, OnceLock};

use bl_engine::{DeviceType, NodeConfig, NodeData, NodeEvent, Registry};
use bl_model::device_layout::{
    BODY_HEIGHT, ControlBand, LayoutNode, Page, PageStyle, PagesNode, TAB_HEIGHT, Widget,
};
use bl_model::{DeviceDescription, DeviceLayout, DeviceRole, ParamDescriptor, ParamId, ParamUnit};
use lspk_dsp::filter::pitch_of;
use lspk_voice::unison::pan_gains;
use lspk_voice::{Start, VoiceBlock, VoiceEvent, VoiceEventKind};
use lspk_wavetable::SharedTable;
use lspk_wavetable::osc::{Inputs, Layout, Onset, Oscillator};
use lspk_wavetable::shaper::{Mode, ShapedOscillator, ShaperInputs, Slot};
use lspk_wavetable::sub::{Inputs as SubInputs, Shape, Sub};

use crate::framework::{Audio, NativeDevice, ParamBank, add_with_data};
use crate::instrument::matrix::{
    DestDef, Kind as MatrixKind, Matrix, MatrixLayout, Pick, Range as MatrixRange, Rate, Read,
    Scope, SlotDef, SourceDef, time_factor,
};
use crate::instrument::{self, Instrument, Parts, Section, Sources, State};
use crate::instrument_filter::{CIRCUITS, InstrumentFilter, Per, PerFrame, Setup};
use crate::sampler::FILTER_TYPE_CHOICES;
use crate::wavetable_mod::{
    self as m, DIVISIONS, EnvParams, Envelope, LFO_WAVES, LOOP_MODES, Lfo, LfoParams, Loop,
    UNISON_MODES,
};
use crate::wavetables::OscTable;

/// Wavetable's device type.
pub const WAVETABLE: &str = "beatlayer.wavetable";

// Parameter ids. Each block has room for the later parts' additions: the
// oscillators 20 apiece from 0, the sub from 40, the filters 20 apiece from
// 50, the routing at 90, the amplitude envelope from 100, the global
// controls from 120; Envelopes 2 and 3 take 200 on, the LFOs 300 on, the
// MIDI tab 400 on and the matrix 1000 on.

/// The offsets of an oscillator's parameters from [`osc`]'s base.
pub const OSC_ON: u32 = 0;
pub const POSITION: u32 = 1;
pub const GAIN: u32 = 2;
pub const PAN: u32 = 3;
pub const SEMI: u32 = 4;
pub const DETUNE: u32 = 5;
pub const EFFECT: u32 = 6;
pub const FM_AMOUNT: u32 = 7;
pub const FM_TUNE: u32 = 8;
pub const PW: u32 = 9;
pub const SYNC: u32 = 10;
pub const WARP: u32 = 11;
pub const FOLD: u32 = 12;

/// Oscillator `n`'s (1 or 2) parameter at offset `what`.
#[must_use]
pub const fn osc(n: usize, what: u32) -> ParamId {
    ParamId((n as u32 - 1) * 20 + what)
}

/// The offsets of a filter's parameters from [`filter`]'s base.
pub const FILTER_ON: u32 = 0;
pub const FILTER_TYPE: u32 = 1;
pub const FILTER_CIRCUIT: u32 = 2;
pub const FILTER_FREQ: u32 = 3;
pub const FILTER_RES: u32 = 4;
pub const FILTER_DRIVE: u32 = 5;
pub const FILTER_MORPH: u32 = 6;

/// Filter `n`'s (1 or 2) parameter at offset `what`.
#[must_use]
pub const fn filter(n: usize, what: u32) -> ParamId {
    ParamId(50 + (n as u32 - 1) * 20 + what)
}

/// The sub oscillator's switch, Gain, Tone and Octave.
pub const SUB_ON: ParamId = ParamId(40);
pub const SUB_GAIN: ParamId = ParamId(41);
pub const SUB_TONE: ParamId = ParamId(42);
pub const SUB_OCTAVE: ParamId = ParamId(43);
/// How the filters are routed: Serial, Parallel or Split.
pub const ROUTING: ParamId = ParamId(90);
/// The amplitude envelope's times, sustain and slopes.
pub const AMP_ATTACK: ParamId = ParamId(100);
pub const AMP_DECAY: ParamId = ParamId(101);
pub const AMP_SUSTAIN: ParamId = ParamId(102);
pub const AMP_RELEASE: ParamId = ParamId(103);
pub const AMP_ATTACK_SLOPE: ParamId = ParamId(104);
pub const AMP_DECAY_SLOPE: ParamId = ParamId(105);
pub const AMP_RELEASE_SLOPE: ParamId = ParamId(106);
/// Poly or Mono, Poly Voices, Glide, Transpose and Volume.
pub const VOICE_MODE: ParamId = ParamId(120);
pub const POLY_VOICES: ParamId = ParamId(121);
pub const GLIDE: ParamId = ParamId(122);
pub const TRANSPOSE: ParamId = ParamId(123);
pub const VOLUME: ParamId = ParamId(124);
/// Unison's copies per oscillator and its amount.
pub const UNISON_VOICES: ParamId = ParamId(125);
pub const UNISON_AMOUNT: ParamId = ParamId(126);
/// The pitch wheel's range, in semitones.
pub const PB_RANGE: ParamId = ParamId(127);
/// Unison's mode (R30) and Hi-Quality (R31).
pub const UNISON_MODE: ParamId = ParamId(128);
pub const HI_QUALITY: ParamId = ParamId(129);

/// The offsets of an envelope's parameters from [`envelope`]'s base: Attack,
/// Decay, Sustain and Release, their slopes, Loop Mode and, on Envelopes 2
/// and 3 only, Initial, Peak and Final.
pub const ENV_ATTACK: u32 = 0;
pub const ENV_DECAY: u32 = 1;
pub const ENV_SUSTAIN: u32 = 2;
pub const ENV_RELEASE: u32 = 3;
pub const ENV_ATTACK_SLOPE: u32 = 4;
pub const ENV_DECAY_SLOPE: u32 = 5;
pub const ENV_RELEASE_SLOPE: u32 = 6;
pub const ENV_INITIAL: u32 = 7;
pub const ENV_PEAK: u32 = 8;
pub const ENV_FINAL: u32 = 9;
pub const ENV_MODE: u32 = 10;

/// Envelope `e`'s (0 the Amp envelope, 1 and 2 Envelopes 2 and 3) parameter at
/// offset `what`.
#[must_use]
pub const fn envelope(e: usize, what: u32) -> ParamId {
    ParamId(
        if e == 0 {
            100
        } else {
            200 + (e as u32 - 1) * 20
        } + what,
    )
}

/// The offsets of an LFO's parameters from [`lfo`]'s base.
pub const LFO_WAVE: u32 = 0;
pub const LFO_SHAPE: u32 = 1;
pub const LFO_SYNC: u32 = 2;
pub const LFO_RATE: u32 = 3;
pub const LFO_DIVISION: u32 = 4;
pub const LFO_AMOUNT: u32 = 5;
pub const LFO_OFFSET: u32 = 6;
pub const LFO_ATTACK: u32 = 7;
pub const LFO_RETRIGGER: u32 = 8;

/// LFO `n`'s (1 or 2) parameter at offset `what`.
#[must_use]
pub const fn lfo(n: usize, what: u32) -> ParamId {
    ParamId(300 + (n as u32 - 1) * 20 + what)
}

/// The matrix's Time and Amount (R26).
pub const MATRIX_TIME: ParamId = ParamId(1000);
pub const MATRIX_AMOUNT: ParamId = ParamId(1001);
/// The matrix's free slots (R25).
pub const SLOTS: usize = 64;

/// Slot `slot`'s amount.
#[must_use]
pub const fn slot_amount(slot: usize) -> ParamId {
    ParamId(1010 + slot as u32)
}

/// The matrix's sources by place in its list (R25).
const SRC_LFO: usize = 3;
const SRC_VELOCITY: usize = 5;
const SRC_NOTE: usize = 6;
const SRC_PITCH_BEND: usize = 7;
const SRC_AFTERTOUCH: usize = 8;
const SRC_MOD_WHEEL: usize = 9;
const SRC_RANDOM: usize = 10;
const SRC_PRESSURE: usize = 11;
const SRC_SLIDE: usize = 12;
/// The sources worked out per voice and frame: the three envelopes and two
/// LFOs.
const MODULATORS: usize = 5;
/// How many destinations each envelope has.
const ENV_DESTS: [usize; 3] = [7, 10, 10];
const SOURCES: [(&str, bool, bool); 13] = [
    // (name, bipolar, global)
    ("Amp", false, false),
    ("Env 2", true, false),
    ("Env 3", true, false),
    ("LFO 1", true, false),
    ("LFO 2", true, false),
    ("Velocity", false, false),
    ("Note", true, false),
    ("Pitch Bend", true, true),
    ("Aftertouch", false, true),
    ("Mod Wheel", false, true),
    ("Random", false, false),
    ("Pressure", false, false),
    ("Slide", false, false),
];
/// Spec: wavetable R26. The Note source's span in semitones, 1,000 to 1
/// being the travel of a filter's Frequency: at 100 % a filter follows the
/// note exactly.
const NOTE_SPAN: f64 = 119.589;

/// The destinations by place in the matrix's list (R25).
const D_SUB_GAIN: usize = 22;
const D_SUB_TONE: usize = 23;
const D_UNISON: usize = 65;
const D_VOLUME: usize = 66;
const D_TIME: usize = 67;
const D_AMOUNT: usize = 68;
const DESTS: usize = 69;
const D_OSC_SEMI: usize = 9;
const D_OSC_DETUNE: usize = 10;

/// Oscillator `n`'s (1 or 2) destination at offset `k`: Position, the six
/// effect controls, Gain, Pan, Semi, Detune.
const fn d_osc(n: usize, k: usize) -> usize {
    (n - 1) * 11 + k
}

/// Filter `n`'s destination at `k`: Frequency, Resonance, Drive, Morph.
const fn d_filter(n: usize, k: usize) -> usize {
    24 + (n - 1) * 4 + k
}

/// Envelope `e`'s destination at the offset of its parameter.
const fn d_env(e: usize, k: usize) -> usize {
    [32, 39, 49][e] + k
}

/// LFO `n`'s destination at `k`: Rate, Shape, Amount.
const fn d_lfo(n: usize, k: usize) -> usize {
    59 + (n - 1) * 3 + k
}

/// The parameter each destination moves, in order.
fn dest_params() -> Vec<ParamId> {
    let mut ids = Vec::with_capacity(DESTS);
    for n in 1..=2 {
        ids.extend(
            [
                POSITION, FM_AMOUNT, FM_TUNE, PW, SYNC, WARP, FOLD, GAIN, PAN, SEMI, DETUNE,
            ]
            .map(|what| osc(n, what)),
        );
    }
    ids.extend([SUB_GAIN, SUB_TONE]);
    for n in 1..=2 {
        ids.extend([FILTER_FREQ, FILTER_RES, FILTER_DRIVE, FILTER_MORPH].map(|w| filter(n, w)));
    }
    for (e, last) in [(0, ENV_RELEASE_SLOPE), (1, ENV_FINAL), (2, ENV_FINAL)] {
        ids.extend((ENV_ATTACK..=last).map(|what| envelope(e, what)));
    }
    for n in 1..=2 {
        ids.extend([LFO_RATE, LFO_SHAPE, LFO_AMOUNT].map(|what| lfo(n, what)));
    }
    ids.extend([UNISON_AMOUNT, VOLUME, MATRIX_TIME, MATRIX_AMOUNT]);
    debug_assert_eq!(ids.len(), DESTS);
    ids
}

/// Whether a destination is multiplicative (R25): Volume, the Amp envelope's
/// Sustain, Envelopes 2 and 3's Initial, each LFO's Amount, Unison Amount and
/// the matrix's Amount.
fn multiplicative(dest: usize) -> bool {
    dest == D_VOLUME
        || dest == D_UNISON
        || dest == D_AMOUNT
        || dest == d_env(0, ENV_SUSTAIN as usize)
        || dest == d_env(1, ENV_INITIAL as usize)
        || dest == d_env(2, ENV_INITIAL as usize)
        || dest == d_lfo(1, 2)
        || dest == d_lfo(2, 2)
}

/// Whether a destination is worked out every frame (instrument modulation
/// R1): pitch, position, the effects, amplitude and pan.
fn audio_rate(dest: usize) -> bool {
    dest < D_SUB_GAIN || dest == D_VOLUME
}

/// The matrix's layout, made once: names are the parameters', held for as long
/// as the program runs.
fn layout() -> &'static Arc<MatrixLayout> {
    static LAYOUT: OnceLock<Arc<MatrixLayout>> = OnceLock::new();
    LAYOUT.get_or_init(|| {
        let description = wavetable_description();
        let dests = dest_params()
            .into_iter()
            .enumerate()
            .map(|(dest, id)| {
                let name = description.param(id).expect("a destination is a parameter");
                DestDef {
                    name: Box::leak(name.name.clone().into_boxed_str()),
                    param: Some(id),
                    scope: if matches!(dest, D_VOLUME | D_TIME | D_AMOUNT) {
                        Scope::Global
                    } else {
                        Scope::Voice
                    },
                    rate: if audio_rate(dest) {
                        Rate::Audio
                    } else {
                        Rate::Control
                    },
                    kind: if multiplicative(dest) {
                        MatrixKind::Multiplicative
                    } else {
                        MatrixKind::Additive
                    },
                }
            })
            .collect();
        Arc::new(MatrixLayout {
            sources: SOURCES
                .iter()
                .map(|&(name, bipolar, global)| SourceDef {
                    name,
                    range: if bipolar {
                        MatrixRange::Bipolar
                    } else {
                        MatrixRange::Unipolar
                    },
                    scope: if global { Scope::Global } else { Scope::Voice },
                })
                .collect(),
            dests,
            slots: (0..SLOTS)
                .map(|slot| SlotDef {
                    source: Pick::Free,
                    dest: Pick::Free,
                    amount: slot_amount(slot),
                })
                .collect(),
            forbidden: Vec::new(),
        })
    })
}

/// Every destination's descriptor, in the matrix's order.
fn dest_descriptors() -> &'static Arc<Vec<bl_model::ParamDescriptor>> {
    static DESCRIPTORS: OnceLock<Arc<Vec<bl_model::ParamDescriptor>>> = OnceLock::new();
    DESCRIPTORS.get_or_init(|| {
        let description = wavetable_description();
        Arc::new(
            dest_params()
                .into_iter()
                .map(|id| description.param(id).expect("a parameter").clone())
                .collect(),
        )
    })
}

/// The effects an oscillator offers (R23): exactly these.
pub const EFFECTS: [&str; 4] = ["None", "FM", "Classic", "Modern"];
/// The filters' routings (R16).
pub const ROUTINGS: [&str; 3] = ["Serial", "Parallel", "Split"];
/// The pages of the body (R22).
pub const PAGES: [&str; 4] = ["Oscillator", "Matrix", "Mod Sources", "MIDI"];
/// The pages node's name.
pub const TABS: &str = "Tabs";

const FX_NONE: usize = 0;
const FX_FM: usize = 1;
const FX_CLASSIC: usize = 2;

/// The voices a Wavetable plays at most (the release notes' 16) and its
/// unison copies per voice at most (R2: 16 × 8 = 128 lanes).
const VOICES: usize = 16;
const COPIES: usize = 8;
/// The frames a stretch is rendered in: the oscillators' scratch is this
/// long.
const CHUNK: usize = 128;
/// The factor the effects render oversampled at with Hi-Quality on and off.
const HI_OVERSAMPLING: usize = 8;
const LIGHT_OVERSAMPLING: usize = 2;
/// How many frames the FM modulator's pitch holds between updates.
const GRID: usize = 32;
/// The longest envelope time, in milliseconds.
const ENVELOPE_MAX_MS: f64 = 20_000.0;

/// Spec: wavetable R14. FM's index at 100 % is 4 radians; the kit's phase
/// distortion moves the phase by this many cycles per unit of a source at its
/// full amount.
const FM_INDEX: f32 = 4.0;
/// Classic's PW reaches a tenth of the cycle at 100 %; the kit's pulse width
/// reaches a twentieth, so its amount is scaled by this.
const PW_SCALE: f32 = 0.9 / lspk_wavetable::shaper::PULSE_MAX;
/// Classic's Sync reaches 8 times the note at 100 %; the kit's reaches 16,
/// so its amount is scaled by log₁₆ 8.
const SYNC_SCALE: f32 = 0.75;

/// The ramps a stretch copies from the parameters, by index: per oscillator
/// nine (Position, Gain, Pan, then the six effect controls), per filter four
/// (Frequency, Resonance, Drive, Morph), then the sub's Gain and Tone and
/// the Volume.
const RAMP_OSC: usize = 9;
const RAMP_FILTER: usize = 2 * RAMP_OSC;
const RAMP_SUB_GAIN: usize = RAMP_FILTER + 8;
const RAMP_SUB_TONE: usize = RAMP_SUB_GAIN + 1;
const RAMP_VOLUME: usize = RAMP_SUB_TONE + 1;
const RAMPS: usize = RAMP_VOLUME + 1;

fn ramp_ids() -> [ParamId; RAMPS] {
    let mut ids = [ParamId(0); RAMPS];
    for n in 1..=2 {
        let at = (n - 1) * RAMP_OSC;
        for (i, what) in [
            POSITION, GAIN, PAN, FM_AMOUNT, FM_TUNE, PW, SYNC, WARP, FOLD,
        ]
        .into_iter()
        .enumerate()
        {
            ids[at + i] = osc(n, what);
        }
        let at = RAMP_FILTER + (n - 1) * 4;
        for (i, what) in [FILTER_FREQ, FILTER_RES, FILTER_DRIVE, FILTER_MORPH]
            .into_iter()
            .enumerate()
        {
            ids[at + i] = filter(n, what);
        }
    }
    ids[RAMP_SUB_GAIN] = SUB_GAIN;
    ids[RAMP_SUB_TONE] = SUB_TONE;
    ids[RAMP_VOLUME] = VOLUME;
    ids
}

/// The Wavetable device type.
#[must_use]
pub fn wavetable_type() -> DeviceType {
    DeviceType::native(WAVETABLE, "Wavetable", DeviceRole::Instrument)
        .with_description(wavetable_description())
        .with_matrix(layout().as_ref().clone())
}

/// Wavetable's parameters, panel and banks.
fn wavetable_description() -> DeviceDescription {
    let mut params = Vec::new();
    for n in 1..=2 {
        let name = |what: &str| format!("Osc {n} {what}");
        let effect = |what: u32, label: &str, min: f64, text: &str| {
            ParamDescriptor::continuous(
                osc(n, what),
                &name(label),
                (min, 100.0),
                0.0,
                ParamUnit::Percent,
            )
            .describe(text)
        };
        params.extend([
            ParamDescriptor::toggle(osc(n, OSC_ON), &name("On"), n == 1)
                .describe("Turns the oscillator on."),
            ParamDescriptor::continuous(
                osc(n, POSITION),
                &name("Position"),
                (0.0, 100.0),
                0.0,
                ParamUnit::Percent,
            )
            .describe("Where in the wavetable the oscillator reads: 0 % is the first frame, 100 % the last."),
            ParamDescriptor::gain(osc(n, GAIN), &name("Gain"), 0.0, 0.0)
                .describe("Sets the oscillator's level, down to silence."),
            ParamDescriptor::continuous(
                osc(n, PAN),
                &name("Pan"),
                (-50.0, 50.0),
                0.0,
                ParamUnit::None,
            )
            .decimals(0)
            .describe("Pans the oscillator, from 50 left to 50 right."),
            ParamDescriptor::integer(osc(n, SEMI), &name("Semi"), (-36, 36), 0, ParamUnit::Semitones)
                .decimals(0)
                .describe("Transposes the oscillator in semitones, on top of Transpose."),
            ParamDescriptor::continuous(
                osc(n, DETUNE),
                &name("Detune"),
                (-50.0, 50.0),
                0.0,
                ParamUnit::Cents,
            )
            .describe("Detunes the oscillator in cents."),
            ParamDescriptor::choice(osc(n, EFFECT), &name("Effect"), &EFFECTS, FX_NONE).describe(
                "The effect that reshapes the oscillator's wave: FM, Classic or Modern, each with \
                 two controls; None leaves the wave as it is.",
            ),
            effect(
                FM_AMOUNT,
                "FM Amount",
                0.0,
                "FM: how deeply a sine modulates the oscillator's phase.",
            ),
            effect(
                FM_TUNE,
                "FM Tune",
                -100.0,
                "FM: the modulating sine's frequency against the note's: an octave away at ±50 %, \
                 two at ±100 %, and inharmonic between.",
            ),
            effect(
                PW,
                "PW",
                0.0,
                "Classic: narrows the wave to a pulse, for any table.",
            ),
            effect(
                SYNC,
                "Sync",
                0.0,
                "Classic: resets the wave from a hidden oscillator, up to eight times the note.",
            ),
            effect(
                WARP,
                "Warp",
                0.0,
                "Modern: bends the wave's phase smoothly, shifting its centre.",
            ),
            effect(
                FOLD,
                "Fold",
                0.0,
                "Modern: folds the wave back on itself as it is driven, up to eight times its level.",
            ),
        ]);
    }
    params.extend([
        ParamDescriptor::toggle(SUB_ON, "Sub On", false).describe("Turns the sub oscillator on."),
        ParamDescriptor::gain(SUB_GAIN, "Sub Gain", 0.0, 0.0)
            .describe("Sets the sub oscillator's level, down to silence."),
        ParamDescriptor::continuous(SUB_TONE, "Sub Tone", (0.0, 100.0), 0.0, ParamUnit::Percent)
            .describe("Adds harmonics to the sub oscillator: a pure sine at 0 %."),
        ParamDescriptor::integer(SUB_OCTAVE, "Sub Octave", (-2, -1), -1, ParamUnit::None)
            .decimals(0)
            .describe("How many octaves below the note the sub oscillator plays."),
    ]);
    for n in 1..=2 {
        let name = |what: &str| format!("Filter {n} {what}");
        params.extend([
            ParamDescriptor::toggle(filter(n, FILTER_ON), &name("On"), n == 1)
                .describe("Turns the filter on; off, its signal passes through."),
            ParamDescriptor::choice(
                filter(n, FILTER_TYPE),
                &name("Type"),
                &FILTER_TYPE_CHOICES,
                1,
            )
            .describe("The filter's type and slope: low-pass, high-pass, band-pass, notch or Morph, at 12 or 24 dB per octave."),
            ParamDescriptor::choice(filter(n, FILTER_CIRCUIT), &name("Circuit"), &CIRCUITS, 0)
                .describe("The filter's circuit; one a type doesn't offer plays Clean."),
            ParamDescriptor::continuous(
                filter(n, FILTER_FREQ),
                &name("Freq"),
                (20.0, 20_000.0),
                20_000.0,
                ParamUnit::Hertz,
            )
            .logarithmic()
            .describe("The filter's cutoff frequency."),
            ParamDescriptor::continuous(
                filter(n, FILTER_RES),
                &name("Res"),
                (0.0, 100.0),
                0.0,
                ParamUnit::Percent,
            )
            .describe("Raises the filter's resonance at the cutoff."),
            ParamDescriptor::continuous(
                filter(n, FILTER_DRIVE),
                &name("Drive"),
                (0.0, 24.0),
                0.0,
                ParamUnit::Decibels,
            )
            .describe("Drives the signal into the filter's circuit; Clean ignores it."),
            ParamDescriptor::continuous(
                filter(n, FILTER_MORPH),
                &name("Morph"),
                (0.0, 100.0),
                0.0,
                ParamUnit::Percent,
            )
            .describe("Where the Morph type sits between low-pass, band-pass, high-pass and notch."),
        ]);
    }
    let time = |id, name: &str, ms: f64, text: &str| {
        ParamDescriptor::continuous(
            id,
            name,
            (0.0, ENVELOPE_MAX_MS),
            ms,
            ParamUnit::Milliseconds,
        )
        .power(5)
        .describe(text)
    };
    let slope = |id, name: &str, text: &str| {
        ParamDescriptor::continuous(id, name, (-1.0, 1.0), 0.0, ParamUnit::None)
            .decimals(2)
            .describe(text)
    };
    let mut poly_voices = instrument::voices(POLY_VOICES, VOICES as i64, 8);
    "Poly Voices".clone_into(&mut poly_voices.name);
    let mut glide = instrument::glide_time(GLIDE, 5_000.0, 0.0);
    "Glide".clone_into(&mut glide.name);
    params.extend([
        ParamDescriptor::choice(ROUTING, "Routing", &ROUTINGS, 0).describe(
            "Serial runs both oscillators and the sub oscillator into Filter 1 and Filter 1 into \
             Filter 2; Parallel runs Oscillator 1 through Filter 1 and Oscillator 2 through Filter 2, \
             the sub oscillator through both; Split does the same with the sub oscillator divided \
             between them. A filter that is off passes its signal through.",
        ),
        time(AMP_ATTACK, "Amp Attack", 1.0, "How long a note takes to reach full level."),
        time(
            AMP_DECAY,
            "Amp Decay",
            600.0,
            "How long the level takes to fall to Sustain.",
        ),
        ParamDescriptor::continuous(
            AMP_SUSTAIN,
            "Amp Sustain",
            (0.0, 100.0),
            100.0,
            ParamUnit::Percent,
        )
        .describe("The level held while the note lasts."),
        time(
            AMP_RELEASE,
            "Amp Release",
            100.0,
            "How long the level takes to fall to silence after the note ends.",
        ),
        slope(
            AMP_ATTACK_SLOPE,
            "Amp Attack Slope",
            "Bends the attack: positive starts fast and ends slow, negative the reverse.",
        ),
        slope(
            AMP_DECAY_SLOPE,
            "Amp Decay Slope",
            "Bends the decay: positive starts fast and ends slow, negative the reverse.",
        ),
        slope(
            AMP_RELEASE_SLOPE,
            "Amp Release Slope",
            "Bends the release: positive starts fast and ends slow, negative the reverse.",
        ),
        instrument::voice_mode(VOICE_MODE, false),
        poly_voices,
        glide,
        instrument::transpose(TRANSPOSE, 48),
        ParamDescriptor::gain(VOLUME, "Volume", 0.0, -6.0)
            .describe("Sets the instrument's output level, down to silence."),
        ParamDescriptor::integer(
            UNISON_VOICES,
            "Unison Voices",
            (2, COPIES as i64),
            4,
            ParamUnit::None,
        )
        .decimals(0)
        .describe("How many copies of each oscillator play every note when Unison Amount is above 0."),
        ParamDescriptor::continuous(
            UNISON_AMOUNT,
            "Unison Amount",
            (0.0, 100.0),
            0.0,
            ParamUnit::Percent,
        )
        .describe("How far the copies detune and pan apart; at 0 % one copy plays."),
        instrument::pb_range(PB_RANGE, 24, 2),
        ParamDescriptor::choice(UNISON_MODE, "Unison Mode", &UNISON_MODES, m::CLASSIC).describe(
            "How the copies of each oscillator spread: None plays one copy; Classic detunes them \
             evenly and pans them alternately; Shimmer and Noise jitter their pitch and position at \
             a slow and a fast rate; Phase Sync is Classic with every copy starting in phase; \
             Position Spread spreads their positions in the table; Random Note draws each copy's \
             position and detune again for every note.",
        ),
        instrument::hi_quality(HI_QUALITY),
    ]);

    // The three envelopes (R27): the Amp envelope's and Envelopes 2 and 3's
    // times, Sustain, slopes and Loop Mode; Envelopes 2 and 3's levels.
    for e in 0..3 {
        let name = |what: &str| {
            if e == 0 {
                format!("Amp {what}")
            } else {
                format!("Env {} {what}", e + 1)
            }
        };
        if e > 0 {
            params.extend([
                time(
                    envelope(e, ENV_ATTACK),
                    &name("Attack"),
                    1.0,
                    "How long the envelope takes to rise from Initial to Peak.",
                ),
                time(
                    envelope(e, ENV_DECAY),
                    &name("Decay"),
                    600.0,
                    "How long the envelope takes to fall from Peak to Sustain.",
                ),
                ParamDescriptor::continuous(
                    envelope(e, ENV_SUSTAIN),
                    &name("Sustain"),
                    (0.0, 100.0),
                    100.0,
                    ParamUnit::Percent,
                )
                .describe("The share of Peak the envelope holds while the note lasts."),
                time(
                    envelope(e, ENV_RELEASE),
                    &name("Release"),
                    100.0,
                    "How long the envelope takes to fall to Final after the note ends.",
                ),
                slope(
                    envelope(e, ENV_ATTACK_SLOPE),
                    &name("Attack Slope"),
                    "Bends the attack: positive starts fast and ends slow, negative the reverse.",
                ),
                slope(
                    envelope(e, ENV_DECAY_SLOPE),
                    &name("Decay Slope"),
                    "Bends the decay: positive starts fast and ends slow, negative the reverse.",
                ),
                slope(
                    envelope(e, ENV_RELEASE_SLOPE),
                    &name("Release Slope"),
                    "Bends the release: positive starts fast and ends slow, negative the reverse.",
                ),
            ]);
            let level = |what, label: &str, default: f64, text: &str| {
                ParamDescriptor::continuous(
                    envelope(e, what),
                    &name(label),
                    (-100.0, 100.0),
                    default,
                    ParamUnit::Percent,
                )
                .describe(text)
            };
            params.extend([
                level(
                    ENV_INITIAL,
                    "Initial",
                    0.0,
                    "The level the envelope starts from when a note begins.",
                ),
                level(ENV_PEAK, "Peak", 100.0, "The level the attack rises to."),
                level(
                    ENV_FINAL,
                    "Final",
                    0.0,
                    "The level the release falls to, and a looping envelope starts again from.",
                ),
            ]);
        }
        params.push(
            ParamDescriptor::choice(
                envelope(e, ENV_MODE),
                &name("Loop Mode"),
                &LOOP_MODES,
                0,
            )
            .describe(
                "None holds Sustain until the note ends; Trigger runs the attack, decay and release \
                 once from the note's start; Loop cycles them until the note ends.",
            ),
        );
    }

    // The two LFOs (R28).
    for n in 1..=2 {
        let name = |what: &str| format!("LFO {n} {what}");
        params.extend([
            ParamDescriptor::choice(lfo(n, LFO_WAVE), &name("Wave"), &LFO_WAVES, 0)
                .describe("The LFO's wave: Sine, Saw, Triangle, Square or Random."),
            ParamDescriptor::continuous(
                lfo(n, LFO_SHAPE),
                &name("Shape"),
                (-100.0, 100.0),
                0.0,
                ParamUnit::Percent,
            )
            .describe("Bends the wave: its slope, its peak, its pulse width or how often its random values reach the extremes."),
            ParamDescriptor::toggle(lfo(n, LFO_SYNC), &name("Sync"), false)
                .describe("Sets the rate in divisions of the song's tempo instead of hertz."),
            ParamDescriptor::continuous(
                lfo(n, LFO_RATE),
                &name("Rate"),
                (0.01, 40.0),
                1.0,
                ParamUnit::Hertz,
            )
            .logarithmic()
            .describe("How fast the LFO cycles, in hertz."),
            ParamDescriptor::choice(
                lfo(n, LFO_DIVISION),
                &name("Division"),
                &DIVISIONS.map(|(name, _)| name),
                4,
            )
            .describe("The length of one cycle with Sync on, from 1/64 to 8 bars."),
            ParamDescriptor::continuous(
                lfo(n, LFO_AMOUNT),
                &name("Amount"),
                (0.0, 100.0),
                100.0,
                ParamUnit::Percent,
            )
            .describe("Scales the LFO's output."),
            ParamDescriptor::continuous(
                lfo(n, LFO_OFFSET),
                &name("Offset"),
                (0.0, 360.0),
                0.0,
                ParamUnit::None,
            )
            .decimals(0)
            .describe("Where in its cycle the LFO starts, in degrees, and the phase it keeps against the song."),
            time(
                lfo(n, LFO_ATTACK),
                &name("Attack"),
                0.0,
                "How long the LFO takes to fade in from the start of a note.",
            ),
            ParamDescriptor::toggle(lfo(n, LFO_RETRIGGER), &name("Retrigger"), false).describe(
                "Starts the LFO again at Offset with each note; off, it follows the song position \
                 while the song plays.",
            ),
        ]);
    }

    // The matrix: Time and Amount (R26) and its free slots' amounts (R25).
    params.extend([
        ParamDescriptor::continuous(
            MATRIX_TIME,
            "Matrix Time",
            (-100.0, 100.0),
            0.0,
            ParamUnit::Percent,
        )
        .describe(
            "Scales the speed of every envelope and LFO the matrix reads; negative is faster.",
        ),
        ParamDescriptor::continuous(
            MATRIX_AMOUNT,
            "Matrix Amount",
            (0.0, 100.0),
            100.0,
            ParamUnit::Percent,
        )
        .describe("Scales every route's amount."),
    ]);
    params.extend((0..SLOTS).map(|slot| {
        ParamDescriptor::continuous(
            slot_amount(slot),
            &format!("Route {} Amount", slot + 1),
            (-100.0, 100.0),
            0.0,
            ParamUnit::Percent,
        )
        .describe("How far the slot's source moves its destination, in per cent of its travel.")
    }));

    let tab = BODY_HEIGHT - TAB_HEIGHT;
    let band =
        |controls: &[(ParamId, Widget)]| LayoutNode::Band(ControlBand::stacked(tab, controls));
    let oscillator = |n: usize| {
        band(&[
            (osc(n, OSC_ON), Widget::Toggle),
            (osc(n, POSITION), Widget::Knob),
            (osc(n, GAIN), Widget::Knob),
            (osc(n, PAN), Widget::Knob),
            (osc(n, SEMI), Widget::Number),
            (osc(n, DETUNE), Widget::Knob),
            (osc(n, EFFECT), Widget::Chooser),
            (osc(n, FM_AMOUNT), Widget::Knob),
            (osc(n, FM_TUNE), Widget::Knob),
            (osc(n, PW), Widget::Knob),
            (osc(n, SYNC), Widget::Knob),
            (osc(n, WARP), Widget::Knob),
            (osc(n, FOLD), Widget::Knob),
        ])
    };
    let filters = |n: usize| {
        band(&[
            (filter(n, FILTER_ON), Widget::Toggle),
            (filter(n, FILTER_TYPE), Widget::Chooser),
            (filter(n, FILTER_CIRCUIT), Widget::Chooser),
            (filter(n, FILTER_FREQ), Widget::Knob),
            (filter(n, FILTER_RES), Widget::Knob),
            (filter(n, FILTER_DRIVE), Widget::Knob),
            (filter(n, FILTER_MORPH), Widget::Knob),
        ])
    };
    let envelopes = |e: usize| {
        let mut controls = vec![
            (envelope(e, ENV_ATTACK), Widget::Knob),
            (envelope(e, ENV_DECAY), Widget::Knob),
            (envelope(e, ENV_SUSTAIN), Widget::Knob),
            (envelope(e, ENV_RELEASE), Widget::Knob),
            (envelope(e, ENV_ATTACK_SLOPE), Widget::Knob),
            (envelope(e, ENV_DECAY_SLOPE), Widget::Knob),
            (envelope(e, ENV_RELEASE_SLOPE), Widget::Knob),
            (envelope(e, ENV_MODE), Widget::Chooser),
        ];
        if e > 0 {
            controls.extend([
                (envelope(e, ENV_INITIAL), Widget::Knob),
                (envelope(e, ENV_PEAK), Widget::Knob),
                (envelope(e, ENV_FINAL), Widget::Knob),
            ]);
        }
        band(&controls)
    };
    let lfos = |n: usize| {
        band(&[
            (lfo(n, LFO_WAVE), Widget::Chooser),
            (lfo(n, LFO_SHAPE), Widget::Knob),
            (lfo(n, LFO_SYNC), Widget::Toggle),
            (lfo(n, LFO_RATE), Widget::Knob),
            (lfo(n, LFO_DIVISION), Widget::Chooser),
            (lfo(n, LFO_AMOUNT), Widget::Knob),
            (lfo(n, LFO_OFFSET), Widget::Knob),
            (lfo(n, LFO_ATTACK), Widget::Knob),
            (lfo(n, LFO_RETRIGGER), Widget::Toggle),
        ])
    };
    let pages = PagesNode::new(
        TABS,
        PageStyle::Tabs,
        vec![
            Page::new(
                PAGES[0],
                LayoutNode::Row(vec![
                    oscillator(1),
                    oscillator(2),
                    band(&[
                        (SUB_ON, Widget::Toggle),
                        (SUB_GAIN, Widget::Knob),
                        (SUB_TONE, Widget::Knob),
                        (SUB_OCTAVE, Widget::Number),
                    ]),
                    filters(1),
                    filters(2),
                    band(&[
                        (ROUTING, Widget::Chooser),
                        (VOICE_MODE, Widget::Chooser),
                        (POLY_VOICES, Widget::Number),
                        (GLIDE, Widget::Knob),
                        (TRANSPOSE, Widget::Number),
                        (VOLUME, Widget::Knob),
                        (UNISON_VOICES, Widget::Number),
                        (UNISON_AMOUNT, Widget::Knob),
                        (UNISON_MODE, Widget::Chooser),
                    ]),
                ]),
            ),
            Page::new(PAGES[1], {
                // The slots' amounts, eight to a band, until part 3's grid.
                let mut bands = vec![band(&[
                    (MATRIX_TIME, Widget::Knob),
                    (MATRIX_AMOUNT, Widget::Knob),
                ])];
                bands.extend((0..SLOTS / 8).map(|bank| {
                    let slots: Vec<_> = (0..8)
                        .map(|i| (slot_amount(bank * 8 + i), Widget::Knob))
                        .collect();
                    band(&slots)
                }));
                LayoutNode::Row(bands)
            }),
            Page::new(
                PAGES[2],
                LayoutNode::Row(vec![
                    envelopes(0),
                    envelopes(1),
                    envelopes(2),
                    lfos(1),
                    lfos(2),
                ]),
            ),
            Page::new(
                PAGES[3],
                LayoutNode::Row(vec![band(&[(PB_RANGE, Widget::Number)])]),
            ),
        ],
    );
    let layout = DeviceLayout::new(LayoutNode::Pages(pages));
    let named = |names: &[&str]| names.iter().map(|n| (*n).to_owned()).collect::<Vec<_>>();
    let mut banks: Vec<(String, Vec<String>)> = Vec::new();
    for n in 1..=2 {
        banks.push((
            format!("Oscillator {n}"),
            ["On", "Position", "Gain", "Pan", "Semi", "Detune", "Effect"]
                .iter()
                .map(|what| format!("Osc {n} {what}"))
                .collect(),
        ));
        banks.push((
            format!("Oscillator {n} Effect"),
            ["FM Amount", "FM Tune", "PW", "Sync", "Warp", "Fold"]
                .iter()
                .map(|what| format!("Osc {n} {what}"))
                .collect(),
        ));
    }
    banks.push((
        "Sub".to_owned(),
        named(&["Sub On", "Sub Gain", "Sub Tone", "Sub Octave"]),
    ));
    for n in 1..=2 {
        banks.push((
            format!("Filter {n}"),
            ["On", "Type", "Circuit", "Freq", "Res", "Drive", "Morph"]
                .iter()
                .map(|what| format!("Filter {n} {what}"))
                .collect(),
        ));
    }
    banks.push((
        "Amp Envelope".to_owned(),
        [
            "Attack",
            "Decay",
            "Sustain",
            "Release",
            "Attack Slope",
            "Decay Slope",
            "Release Slope",
            "Loop Mode",
        ]
        .iter()
        .map(|what| format!("Amp {what}"))
        .collect(),
    ));
    for n in 2..=3 {
        banks.push((
            format!("Env {n}"),
            [
                "Attack",
                "Decay",
                "Sustain",
                "Release",
                "Attack Slope",
                "Decay Slope",
                "Release Slope",
                "Loop Mode",
            ]
            .iter()
            .map(|what| format!("Env {n} {what}"))
            .collect(),
        ));
        banks.push((
            format!("Env {n} Levels"),
            ["Initial", "Peak", "Final"]
                .iter()
                .map(|what| format!("Env {n} {what}"))
                .collect(),
        ));
    }
    for n in 1..=2 {
        banks.push((
            format!("LFO {n}"),
            [
                "Wave", "Shape", "Sync", "Rate", "Division", "Amount", "Offset", "Attack",
            ]
            .iter()
            .map(|what| format!("LFO {n} {what}"))
            .collect(),
        ));
    }
    banks.push((
        "Global".to_owned(),
        named(&[
            "Routing",
            "Voice Mode",
            "Poly Voices",
            "Glide",
            "Transpose",
            "Volume",
            "Unison Voices",
            "Unison Amount",
        ]),
    ));
    banks.push((
        "Unison and Matrix".to_owned(),
        named(&[
            "Unison Mode",
            "Matrix Time",
            "Matrix Amount",
            "LFO 1 Retrigger",
            "LFO 2 Retrigger",
        ]),
    ));
    banks.push(("MIDI".to_owned(), named(&["PB Range"])));
    for bank in 0..SLOTS / 8 {
        banks.push((
            format!("Routes {} to {}", bank * 8 + 1, bank * 8 + 8),
            (0..8)
                .map(|i| format!("#{}", slot_amount(bank * 8 + i).0))
                .collect(),
        ));
    }
    let refs: Vec<Vec<&str>> = banks
        .iter()
        .map(|(_, names)| names.iter().map(String::as_str).collect())
        .collect();
    let banks: Vec<(&str, &[&str])> = banks
        .iter()
        .zip(&refs)
        .map(|((name, _), names)| (name.as_str(), names.as_slice()))
        .collect();
    DeviceDescription::new(params)
        .with_layout(layout)
        .with_banks(&banks)
        .with_menu_options(&[HI_QUALITY])
}

/// Adds Wavetable to `registry`.
pub fn register(registry: &mut Registry) {
    add_with_data(
        registry,
        wavetable_type(),
        Wavetable::new,
        Some(std::sync::Arc::new(|device, file, _| {
            crate::wavetables::device_data(device, file)
        })),
    );
}

/// Wavetable's audio-thread half.
pub struct Wavetable {
    section: Section,
    synth: Synth,
}

impl Wavetable {
    /// Makes a Wavetable for `config`'s rate and block size, holding
    /// `device`'s routes. Every buffer and the default tables are made here,
    /// off the audio thread.
    #[must_use]
    pub fn new(device: &bl_model::Device, config: &NodeConfig) -> Self {
        let rate = config.sample_rate.as_f64();
        Self {
            section: Section::new(
                Parts {
                    voices: Some(POLY_VOICES),
                    voice_mode: Some(VOICE_MODE),
                    always_legato: true,
                    glide_time: Some(GLIDE),
                    transpose: Some(TRANSPOSE),
                    pb_range: Some(PB_RANGE),
                    unison_voices: Some(UNISON_VOICES),
                    hi_quality: Some(HI_QUALITY),
                    ..Parts::default()
                },
                VOICES,
                COPIES,
                rate,
                0x57A7_E7AB,
            ),
            synth: Synth::new(device, rate, config.max_frames),
        }
    }
}

impl NativeDevice for Wavetable {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        let frames = audio.frames();
        let channels = audio.channels();
        if frames == 0 || channels == 0 {
            return;
        }
        self.synth.snapshot(frames, params);
        self.synth.follow_song(audio);
        self.section.process(audio, params, &mut self.synth);
        let (left, right) = (&self.synth.out[0][..frames], &self.synth.out[1][..frames]);
        if channels == 1 {
            for ((to, l), r) in audio.channel_mut(0).iter_mut().zip(left).zip(right) {
                *to = (l + r) * 0.5;
            }
            return;
        }
        audio.channel_mut(0).copy_from_slice(left);
        audio.channel_mut(1).copy_from_slice(right);
        for channel in 2..channels {
            audio.channel_mut(channel).fill(0.0);
        }
    }

    fn flush(&mut self, _events: &[NodeEvent], _params: &ParamBank) {
        self.section.reset();
        self.synth.silence();
    }

    fn matrix_played(&self, report: &mut dyn FnMut(ParamId, f64)) {
        let synth = &self.synth;
        for (dest, descriptor) in synth.descs.iter().enumerate() {
            if let Some(position) = synth.matrix.shown(dest) {
                report(descriptor.id, descriptor.denormalize(position));
            }
        }
    }

    fn receive_data(&mut self, data: NodeData) {
        self.section.receive_data(&data);
        if let Some(table) = data.downcast_ref::<OscTable>() {
            self.synth.set_table(table);
        }
        if let Some(routes) = data.downcast_ref::<bl_engine::ModRoutes>() {
            self.synth.matrix.set_free(routes.0);
        }
    }
}

/// An LFO's settings for a stretch (R28).
#[derive(Clone, Copy)]
struct LfoSet {
    wave: usize,
    sync: bool,
    /// Its rate in hertz and its division's length in quarter notes.
    hz: f64,
    beats: f64,
    /// Its Offset in cycles, its Attack in milliseconds, and Retrigger.
    offset: f64,
    attack_ms: f64,
    retrigger: bool,
}

/// What a stretch's switches and choices say, read once.
#[derive(Clone, Copy)]
struct Settings {
    on: [bool; 2],
    effect: [usize; 2],
    /// Each oscillator's pitch offset in semitones, Transpose included, and
    /// its Detune in cents.
    semitones: [f32; 2],
    cents: [f32; 2],
    sub_on: bool,
    sub_octave: i32,
    transpose: f32,
    filter_on: [bool; 2],
    setup: [Setup; 2],
    routing: usize,
    /// The three envelopes as their parameters play, with no route.
    env: [EnvParams; 3],
    lfo: [LfoSet; 2],
    /// Unison's Amount, 0 to 1, and its mode.
    amount: f32,
    mode: usize,
    /// The matrix's Time factor on the rate of envelopes and LFOs, and the
    /// song's tempo.
    speed: f64,
    bpm: f64,
}

/// One oscillator's engine and its scratch for a chunk of frames.
struct Osc {
    shaped: ShapedOscillator,
    /// The FM modulator's phase per lane, in cycles.
    fm_phase: Vec<f32>,
    out: Vec<f32>,
    frame_mod: Vec<f32>,
    fm_source: Vec<f32>,
    amount_mod: [Vec<f32>; 2],
    /// Pitch modulation in semitones, for a Semi or Detune a route reaches.
    pitch_mod: Vec<f32>,
}

impl Osc {
    fn new(table: SharedTable, rate: f32, seed: u64) -> Self {
        let osc = Oscillator::new(
            table,
            Layout {
                voices: VOICES,
                lanes_per_voice: COPIES,
            },
            rate,
            CHUNK,
            seed,
        );
        let lanes = osc.lane_count();
        let buffer = || vec![0.0; CHUNK * lanes];
        Self {
            shaped: ShapedOscillator::new(osc),
            fm_phase: vec![0.0; lanes],
            out: buffer(),
            frame_mod: buffer(),
            // One frame more than a chunk: the kit reads the source between
            // frames, so the last frame looks one ahead.
            fm_source: vec![0.0; (CHUNK + 1) * lanes],
            amount_mod: [buffer(), buffer()],
            pitch_mod: buffer(),
        }
    }
}

/// The default tables: Beatlayer's own, in the first category.
fn default_table() -> SharedTable {
    crate::wavetables::default_source()
        .build()
        .expect("the default tables are factory tables")
}

/// Everything but the voice section: the [`Instrument`] the voices play.
struct Synth {
    rate: f32,
    lanes: usize,
    stride: usize,
    osc: [Osc; 2],
    /// The sub's sine and its square, which Tone crossfades.
    sub: [Sub; 2],
    sub_out: [Vec<f32>; 2],
    sub_pitch: [f32; VOICES],
    sub_step: [f32; VOICES],
    filters: [InstrumentFilter; 2],
    ramps: Vec<Vec<f32>>,
    set: Settings,
    /// The matrix, its destinations' descriptors, and which of them a route
    /// reaches this stretch.
    matrix: Matrix,
    descs: Arc<Vec<bl_model::ParamDescriptor>>,
    has: [bool; DESTS],
    /// Whether a route reads each of the five modulators this stretch.
    used: [bool; MODULATORS],
    /// Every destination's parameter value, as of the stretch.
    base: [f64; DESTS],
    /// Each voice's three envelopes and two LFOs.
    env: [[Envelope; VOICES]; 3],
    lfo: [[Lfo; VOICES]; 2],
    /// The five modulators' values for each voice and frame of the chunk
    /// (`(source × VOICES + voice) × CHUNK + frame`) and each voice's last
    /// frame's, which the modulators' own parameters read.
    src: Vec<f32>,
    prev: [[f32; MODULATORS]; VOICES],
    /// Each LFO's cycles at each frame of the stretch while it follows the
    /// song, and whether it does.
    follow: [Vec<f64>; 2],
    following: [bool; 2],
    /// The matrix's Time factor and Amount at each frame of the chunk.
    speed_at: Vec<f64>,
    amount_at: Vec<f64>,
    /// The voice a global destination reads from at each frame of the chunk:
    /// the oldest whose Amp envelope still runs.
    oldest_at: Vec<Option<usize>>,
    amount_ready: bool,
    /// Each voice's Unison Amount for the chunk, and the gains of its copies.
    amount_of: [f32; VOICES],
    copy_gains: [[(f32, f32); COPIES]; VOICES],
    /// What each lane's note drew for Random Note (R30), the count of notes
    /// started and the device's seed.
    note_draw: Vec<[f32; 2]>,
    serial: u64,
    seed: u64,
    /// Whether a route reaches any of oscillator `o`'s controls, and the
    /// controls each voice plays: nine per voice and frame, in
    /// [`RAMP_OSC`]'s order.
    osc_routed: [bool; 2],
    played: [Vec<f32>; 2],
    /// Per chunk: each lane's pitch, unison's detune and position per lane.
    lane_pitch: Vec<f32>,
    unison: Vec<f32>,
    unison_at: Vec<f32>,
    /// The filters' inputs, the envelope, the cutoffs and the controls, for
    /// a chunk.
    sig: [Vec<f32>; 2],
    env_out: Vec<f32>,
    cutoff: [Vec<f32>; 2],
    controls: [[Vec<f32>; 3]; 2],
    /// The stretch's output, left and right.
    out: [Vec<f32>; 2],
}

/// What a route reads of everything but the modulators.
fn other(voices: &VoiceBlock, state: &State, source: usize, voice: usize) -> f64 {
    let sources = Sources::of(voices);
    match source {
        SRC_VELOCITY => sources.velocity(voice),
        SRC_NOTE => ((f64::from(voices.info(voice).key) - 60.0) / NOTE_SPAN).clamp(-1.0, 1.0),
        SRC_PITCH_BEND => sources.pitch_bend(),
        SRC_AFTERTOUCH => sources.aftertouch(),
        SRC_MOD_WHEEL => sources.mod_wheel(),
        SRC_RANDOM => state.random(voice),
        SRC_PRESSURE => sources.pressure(voice),
        SRC_SLIDE => sources.slide(voice),
        _ => 0.0,
    }
}

/// What a route reads at a frame of the chunk: the modulators' values there.
struct Now<'a> {
    src: &'a [f32],
    voices: &'a VoiceBlock,
    state: &'a State,
}

impl Read for Now<'_> {
    fn read(&self, source: usize, voice: usize, frame: usize) -> f64 {
        if source < MODULATORS {
            f64::from(self.src[(source * VOICES + voice) * CHUNK + frame])
        } else {
            other(self.voices, self.state, source, voice)
        }
    }
}

/// What a modulator's own parameters read: the modulators' values a frame
/// earlier, so no route waits on itself.
struct Before<'a> {
    prev: &'a [[f32; MODULATORS]; VOICES],
    voices: &'a VoiceBlock,
    state: &'a State,
}

impl Read for Before<'_> {
    fn read(&self, source: usize, voice: usize, _frame: usize) -> f64 {
        if source < MODULATORS {
            f64::from(self.prev[voice][source])
        } else {
            other(self.voices, self.state, source, voice)
        }
    }
}

impl Synth {
    fn new(device: &bl_model::Device, rate: f64, max_frames: usize) -> Self {
        let rate = rate as f32;
        let max_frames = max_frames.max(1);
        let filters = [
            InstrumentFilter::new(rate, VOICES, CHUNK),
            InstrumentFilter::new(rate, VOICES, CHUNK),
        ];
        let stride = filters[0].stride();
        let osc = [
            Osc::new(default_table(), rate, 0x0C45_CADE),
            Osc::new(default_table(), rate, 0x0C45_CADF),
        ];
        let lanes = osc[0].shaped.osc().lane_count();
        let sub = |shape| {
            let mut sub = Sub::new(VOICES, rate, CHUNK);
            let p = sub.params_mut();
            p.shape = shape;
            p.level = 100.0;
            p.phase = 0.0;
            sub
        };
        let signal = || vec![0.0; CHUNK * 2 * stride];
        let mono = || vec![0.0; CHUNK * stride];
        let lfo = LfoSet {
            wave: 0,
            sync: false,
            hz: 1.0,
            beats: 1.0,
            offset: 0.0,
            attack_ms: 0.0,
            retrigger: false,
        };
        let set = Settings {
            on: [true, false],
            effect: [FX_NONE; 2],
            semitones: [0.0; 2],
            cents: [0.0; 2],
            sub_on: false,
            sub_octave: -1,
            transpose: 0.0,
            filter_on: [true, false],
            setup: [Setup::new(0, true, 0); 2],
            routing: 0,
            env: [EnvParams::AMP; 3],
            lfo: [lfo; 2],
            amount: 0.0,
            mode: m::CLASSIC,
            speed: 1.0,
            bpm: 120.0,
        };
        Self {
            rate,
            lanes,
            stride,
            osc,
            sub: [sub(Shape::Sine), sub(Shape::Square)],
            sub_out: [vec![0.0; CHUNK * VOICES], vec![0.0; CHUNK * VOICES]],
            sub_pitch: [0.0; VOICES],
            sub_step: [0.0; VOICES],
            filters,
            ramps: (0..RAMPS).map(|_| vec![0.0; max_frames]).collect(),
            set,
            matrix: Matrix::of(layout().clone(), device),
            descs: dest_descriptors().clone(),
            has: [false; DESTS],
            used: [false; MODULATORS],
            base: [0.0; DESTS],
            env: [[Envelope::IDLE; VOICES]; 3],
            lfo: [[Lfo::IDLE; VOICES]; 2],
            src: vec![0.0; MODULATORS * VOICES * CHUNK],
            prev: [[0.0; MODULATORS]; VOICES],
            follow: [vec![0.0; max_frames], vec![0.0; max_frames]],
            following: [false; 2],
            speed_at: vec![1.0; CHUNK],
            amount_at: vec![1.0; CHUNK],
            oldest_at: vec![None; CHUNK],
            amount_ready: false,
            amount_of: [0.0; VOICES],
            copy_gains: [[(1.0, 1.0); COPIES]; VOICES],
            note_draw: vec![[0.0; 2]; lanes],
            serial: 0,
            seed: 0x57A7_E7AB_C0DE,
            osc_routed: [false; 2],
            played: [
                vec![0.0; VOICES * RAMP_OSC * CHUNK],
                vec![0.0; VOICES * RAMP_OSC * CHUNK],
            ],
            lane_pitch: vec![0.0; lanes],
            unison: vec![0.0; lanes],
            unison_at: vec![0.0; lanes],
            sig: [signal(), signal()],
            env_out: mono(),
            cutoff: [mono(), mono()],
            controls: [[mono(), mono(), mono()], [mono(), mono(), mono()]],
            out: [vec![0.0; max_frames], vec![0.0; max_frames]],
        }
    }

    /// A table from the compiler: the oscillator plays it from the next
    /// render (R3, R10).
    fn set_table(&mut self, table: &OscTable) {
        if let Some(osc) = self
            .osc
            .get_mut(usize::from(table.oscillator).wrapping_sub(1))
        {
            osc.shaped.osc_mut().set_table(table.table.clone());
        }
    }

    /// Reads the stretch's parameters: the ramps into buffers, the switches
    /// and choices into [`Settings`], and the matrix's slots.
    fn snapshot(&mut self, frames: usize, params: &ParamBank) {
        let at = |id: ParamId| params.index(id).expect("the description holds it");
        for (to, id) in self.ramps.iter_mut().zip(ramp_ids()) {
            to[..frames].copy_from_slice(&params.ramp(at(id))[..frames]);
        }
        let value = |id: ParamId| params.value(at(id));
        for (to, d) in self.base.iter_mut().zip(self.descs.iter()) {
            *to = value(d.id);
        }
        let transpose = value(TRANSPOSE) as f32;
        let n = |i: usize| i + 1;
        let setup = |i: usize| {
            let entry =
                (value(filter(n(i), FILTER_TYPE)) as usize).min(FILTER_TYPE_CHOICES.len() - 1);
            Setup::new(
                entry / 2,
                entry % 2 == 1,
                value(filter(n(i), FILTER_CIRCUIT)) as usize,
            )
        };
        let speed = time_factor(value(MATRIX_TIME) / 100.0);
        let lfo_set = |i: usize| LfoSet {
            wave: (value(lfo(n(i), LFO_WAVE)) as usize).min(LFO_WAVES.len() - 1),
            sync: value(lfo(n(i), LFO_SYNC)) >= 0.5,
            hz: value(lfo(n(i), LFO_RATE)),
            beats: DIVISIONS[(value(lfo(n(i), LFO_DIVISION)) as usize).min(DIVISIONS.len() - 1)].1,
            offset: value(lfo(n(i), LFO_OFFSET)) / 360.0,
            attack_ms: value(lfo(n(i), LFO_ATTACK)),
            retrigger: value(lfo(n(i), LFO_RETRIGGER)) >= 0.5,
        };
        let mut set = Settings {
            on: [0, 1].map(|i| value(osc(n(i), OSC_ON)) >= 0.5),
            effect: [0, 1].map(|i| (value(osc(n(i), EFFECT)) as usize).min(EFFECTS.len() - 1)),
            semitones: [0, 1].map(|i| value(osc(n(i), SEMI)) as f32 + transpose),
            cents: [0, 1].map(|i| value(osc(n(i), DETUNE)) as f32),
            sub_on: value(SUB_ON) >= 0.5,
            sub_octave: value(SUB_OCTAVE) as i32,
            transpose,
            filter_on: [0, 1].map(|i| value(filter(n(i), FILTER_ON)) >= 0.5),
            setup: [setup(0), setup(1)],
            routing: (value(ROUTING) as usize).min(ROUTINGS.len() - 1),
            env: self.set.env,
            lfo: [lfo_set(0), lfo_set(1)],
            amount: (value(UNISON_AMOUNT) / 100.0) as f32,
            mode: (value(UNISON_MODE) as usize).min(UNISON_MODES.len() - 1),
            speed,
            bpm: self.set.bpm,
        };
        for e in 0..3 {
            set.env[e] = self.env_params(e, speed, |k| self.base[d_env(e, k)]);
            set.env[e].mode = Loop::of(value(envelope(e, ENV_MODE)) as usize);
        }
        self.set = set;
        self.matrix.set_amount(value(MATRIX_AMOUNT) / 100.0);
        self.matrix.begin(params);
        for (dest, has) in self.has.iter_mut().enumerate() {
            *has = self.matrix.has(dest);
        }
        for (source, used) in self.used.iter_mut().enumerate() {
            *used = self.matrix.uses(source);
        }
        self.osc_routed = [1, 2].map(|n| (0..11).any(|k| self.has[d_osc(n, k)]));
        self.out[0][..frames].fill(0.0);
        self.out[1][..frames].fill(0.0);
    }

    /// The envelope `e`'s parameters as `get` gives them (by offset), Time
    /// scaling its speed by `speed`.
    fn env_params(&self, e: usize, speed: f64, get: impl Fn(usize) -> f64) -> EnvParams {
        let rate = f64::from(self.rate);
        let at = |what: u32| get(what as usize);
        let slope = |what: u32| m::shape_power(at(what) as f32);
        let level = |what: u32, default: f32| {
            if e == 0 {
                default
            } else {
                (at(what) / 100.0) as f32
            }
        };
        EnvParams {
            step: [ENV_ATTACK, ENV_DECAY, ENV_RELEASE]
                .map(|what| m::segment_step(at(what), rate, speed)),
            power: [ENV_ATTACK_SLOPE, ENV_DECAY_SLOPE, ENV_RELEASE_SLOPE].map(slope),
            sustain: (at(ENV_SUSTAIN) / 100.0) as f32,
            initial: level(ENV_INITIAL, 0.0),
            peak: level(ENV_PEAK, 1.0),
            fin: level(ENV_FINAL, 0.0),
            mode: Loop::None,
        }
    }

    /// Works out where each LFO that follows the song is at each frame of the
    /// stretch (R28): its cycles from the song's start, a function of the song
    /// position alone, so it sounds the same after the transport restarts and
    /// across a tempo change.
    fn follow_song(&mut self, audio: &Audio<'_, '_>) {
        self.set.bpm = audio.bpm();
        let frames = audio.frames();
        let rate = f64::from(self.rate);
        for l in 0..2 {
            let set = self.set.lfo[l];
            self.following[l] = audio.playing()
                && self.used[SRC_LFO + l]
                && !set.retrigger
                && !self.has[d_lfo(l + 1, 0)]
                && !self.has[D_TIME];
            if !self.following[l] {
                continue;
            }
            for k in 0..frames {
                let frame = audio.song_frame(k);
                self.follow[l][k] = if set.sync {
                    let at = bl_core::time::Frames(frame);
                    let ticks = audio.tempo().frames_to_ticks(at, audio.sample_rate());
                    ticks.as_quarters() / set.beats * self.set.speed
                } else {
                    frame as f64 / rate * set.hz * self.set.speed
                };
            }
        }
    }

    /// Drops everything the synth holds, for a device switched off.
    fn silence(&mut self) {
        self.env = [[Envelope::IDLE; VOICES]; 3];
        self.lfo = [[Lfo::IDLE; VOICES]; 2];
        self.prev = [[0.0; MODULATORS]; VOICES];
        for sub in &mut self.sub {
            sub.reset();
        }
        for osc in &mut self.osc {
            osc.fm_phase.fill(0.0);
        }
        self.out.iter_mut().for_each(|out| out.fill(0.0));
    }

    /// `dest` as `voice` plays it at frame `k` of the chunk (`at` frames into
    /// the stretch), given the parameter's own `base` value: its routes'
    /// movement, if any reaches it (R5).
    fn val(&self, c: &Ctx<'_>, dest: usize, base: f64, voice: usize, k: usize) -> f64 {
        if !self.has[dest] {
            return base;
        }
        self.scale_to(dest, k);
        let d = &self.descs[dest];
        let now = Now {
            src: &self.src,
            voices: c.voices,
            state: c.state,
        };
        let due = c.state.control_due(c.at + k);
        d.denormalize(
            self.matrix
                .position(dest, d.normalize(base), voice, k, due, &now),
        )
    }

    /// Sets the matrix's Amount to what it is at frame `k` of the chunk, once
    /// that frame's has been worked out, and a global destination's voice to
    /// the oldest still sounding then.
    fn scale_to(&self, dest: usize, k: usize) {
        if self.amount_ready {
            self.matrix.set_amount(self.amount_at[k]);
        }
        if matches!(dest, D_VOLUME | D_TIME | D_AMOUNT) {
            self.matrix.set_oldest(self.oldest_at[k]);
        }
    }

    /// [`val`](Self::val) for a modulator's own parameters, which read the
    /// modulators a frame earlier.
    fn val_before(&self, c: &Ctx<'_>, dest: usize, base: f64, voice: usize, k: usize) -> f64 {
        if !self.has[dest] {
            return base;
        }
        self.scale_to(dest, k);
        let d = &self.descs[dest];
        let before = Before {
            prev: &self.prev,
            voices: c.voices,
            state: c.state,
        };
        let due = c.state.control_due(c.at + k);
        d.denormalize(
            self.matrix
                .position(dest, d.normalize(base), voice, k, due, &before),
        )
    }
}

/// Where in the stretch a chunk is, and what its routes read.
struct Ctx<'a> {
    voices: &'a VoiceBlock,
    state: &'a State,
    /// The stretch frame the chunk starts at.
    at: usize,
}

/// A sine of `phase` cycles, to better than 10⁻⁵: a polynomial on the
/// quarter wave, which keeps the FM modulator off the libm call a frame.
fn sine(phase: f32) -> f32 {
    let x = phase - phase.round();
    let x = if x.abs() > 0.25 {
        0.5f32.copysign(x) - x
    } else {
        x
    };
    let t = 2.0 * PI * x;
    let t2 = t * t;
    t * (1.0
        + t2 * (-1.0 / 6.0 + t2 * (1.0 / 120.0 + t2 * (-1.0 / 5040.0 + t2 * (1.0 / 362_880.0)))))
}

impl Instrument for Synth {
    fn voice_event(&mut self, voices: &mut VoiceBlock, _state: &State, event: VoiceEvent) {
        let voice = event.voice;
        let lanes = voices.lanes_of(voice);
        match event.kind {
            VoiceEventKind::Start { start, .. } => {
                let key = voices.pitch()[lanes.start];
                let onset = match start {
                    Start::Fresh => Onset::Fresh,
                    Start::Steal => Onset::Steal,
                    Start::Retrigger => Onset::Retrigger,
                    Start::Legato => Onset::Legato,
                    Start::Join { .. } => Onset::Join,
                };
                // One copy, or Phase Sync's, starts at phase 0; the others at
                // random phases (R19, R30).
                let random = if self.set.amount > 0.0
                    && !matches!(self.set.mode, m::PHASE_SYNC | m::NO_UNISON)
                {
                    100.0
                } else {
                    0.0
                };
                for osc in &mut self.osc {
                    osc.shaped.osc_mut().params_mut().phase_random = random;
                    osc.shaped.start(voice, key, onset);
                }
                for sub in &mut self.sub {
                    sub.start(voice, key, onset);
                }
                match start {
                    Start::Fresh | Start::Join { .. } => {
                        for e in 0..3 {
                            self.env[e][voice].start(self.set.env[e].initial);
                        }
                        for osc in &mut self.osc {
                            osc.fm_phase[lanes.clone()].fill(0.0);
                        }
                        for (filter, on) in self.filters.iter_mut().zip(self.set.filter_on) {
                            if on {
                                filter.reset_voice(voice, 0);
                            }
                        }
                    }
                    Start::Steal | Start::Retrigger => {
                        for e in 0..3 {
                            let from = self.env[e][voice].level();
                            self.env[e][voice].start(from);
                        }
                    }
                    Start::Legato => {}
                }
                if !matches!(start, Start::Legato) {
                    self.start_note(voice, lanes);
                }
            }
            VoiceEventKind::Release => {
                for e in 0..3 {
                    self.env[e][voice].release(self.set.env[e].mode);
                }
            }
            VoiceEventKind::End { .. } => {
                for e in 0..3 {
                    self.env[e][voice] = Envelope::IDLE;
                }
                self.lfo.iter_mut().for_each(|lfo| lfo[voice] = Lfo::IDLE);
            }
        }
    }

    fn render(&mut self, voices: &mut VoiceBlock, state: &State, frames: Range<usize>) {
        let span = voices.span(16);
        if span == 0 || frames.is_empty() {
            return;
        }
        self.matrix.set_newest(state.newest(voices));
        let mut at = frames.start;
        while at < frames.end {
            let mut n = CHUNK.min(frames.end - at);
            // Shimmer and Noise move on a grid of 32 frames counted from the
            // device's first frame, and so does a route into Unison Amount,
            // so a chunk never crosses a line of it.
            if matches!(self.set.mode, m::SHIMMER | m::NOISE) || self.has[D_UNISON] {
                let grid = u64::from(lspk_voice::CONTROL_INTERVAL);
                n = n.min((grid - state.clock(at) % grid) as usize);
            }
            self.chunk(voices, state, frames.start, at, n, span);
            at += n;
        }
        // A voice whose Amp envelope has run out is silent.
        let mut mask = voices.active_mask();
        while mask != 0 {
            let voice = mask.trailing_zeros() as usize;
            mask &= mask - 1;
            if self.env[0][voice].is_idle() {
                voices.end(voice);
            }
        }
    }
}

impl Synth {
    /// A note starts on `voice`: its LFOs restart at Offset, its routes' held
    /// values go, and Random Note draws its copies' offsets (R28, R30).
    fn start_note(&mut self, voice: usize, lanes: Range<usize>) {
        self.serial += 1;
        for l in 0..2 {
            let set = self.set.lfo[l];
            let salt = if set.retrigger {
                self.seed ^ (l as u64) << 56 ^ self.serial
            } else {
                self.seed ^ (l as u64) << 56
            };
            self.lfo[l][voice].start(set.offset, salt);
            if set.attack_ms <= 0.0 {
                self.lfo[l][voice].attack = 1.0;
            }
        }
        self.matrix.start(voice);
        for lane in lanes {
            let draw = |n: u64| {
                m::unit(self.seed ^ self.serial.wrapping_mul(0x9E37) ^ (lane as u64) << 32 ^ n)
            };
            self.note_draw[lane] = [draw(1), draw(2)];
        }
    }

    /// Runs the envelopes and LFOs of the chunk's `n` frames from stretch
    /// frame `c.at` for the active voices, and fills `src` and the Amp
    /// envelope's level (R27, R28). Each modulator's own parameters read the
    /// modulators of the frame before.
    fn modulate(&mut self, c: &Ctx<'_>, n: usize, mask: u32) {
        let (rate, stride) = (f64::from(self.rate), self.stride);
        for k in 0..n {
            // The oldest voice still sounding, by the Amp envelopes as the
            // frame before left them, so it is the same at any block size.
            let mut alive = 0;
            for voice in 0..VOICES {
                if mask >> voice & 1 == 1 && !self.env[0][voice].is_idle() {
                    alive |= 1 << voice;
                }
            }
            let oldest = c.state.oldest_of(alive);
            self.oldest_at[k] = oldest;
            self.matrix.set_oldest(oldest);
            self.amount_ready = false;
            self.matrix.set_amount(self.base[D_AMOUNT] / 100.0);
            let from = oldest.unwrap_or(0);
            self.speed_at[k] = if self.has[D_TIME] {
                time_factor(self.val_before(c, D_TIME, self.base[D_TIME], from, k) / 100.0)
            } else {
                self.set.speed
            };
            self.amount_at[k] = if self.has[D_AMOUNT] {
                (self.val_before(c, D_AMOUNT, self.base[D_AMOUNT], from, k) / 100.0).clamp(0.0, 1.0)
            } else {
                self.base[D_AMOUNT] / 100.0
            };
            self.amount_ready = true;
            let mut m = mask;
            while m != 0 {
                let voice = m.trailing_zeros() as usize;
                m &= m - 1;
                let frame = c.at + k;
                let speed = self.speed_at[k];
                // The LFOs, and Envelopes 2 and 3, only while a route reads
                // them; a route added mid-note finds them where the note
                // left them.
                // shortcut: an unread modulator stands still, upgrade to run it
                // when a cheaper law makes that worth the frames.
                for l in (0..2).filter(|&l| self.used[SRC_LFO + l]) {
                    let set = self.set.lfo[l];
                    let get =
                        |kk: usize, base: f64| self.val_before(c, d_lfo(l + 1, kk), base, voice, k);
                    let lp = LfoParams {
                        wave: set.wave,
                        shape: (get(1, self.base[d_lfo(l + 1, 1)]) / 100.0) as f32,
                        amount: (get(2, self.base[d_lfo(l + 1, 2)]) / 100.0) as f32,
                    };
                    let value = if self.following[l] {
                        self.lfo[l][voice].follow(self.follow[l][frame] + set.offset);
                        self.lfo[l][voice].value(&lp)
                    } else {
                        let value = self.lfo[l][voice].value(&lp);
                        let hz = if set.sync {
                            self.set.bpm / 60.0 / set.beats
                        } else {
                            1.0
                        };
                        // Rate's route moves the hertz rate or, synced, the
                        // division's by the same share.
                        let played = get(0, self.base[d_lfo(l + 1, 0)]);
                        let ratio = played / self.base[d_lfo(l + 1, 0)].max(1e-9);
                        let hz = if set.sync { hz * ratio } else { played };
                        self.lfo[l][voice].run(hz * speed / rate);
                        value
                    };
                    let lfo = &mut self.lfo[l][voice];
                    if lfo.attack < 1.0 {
                        lfo.attack =
                            (lfo.attack + m::segment_step(set.attack_ms, rate, 1.0)).min(1.0);
                    }
                    self.src[((SRC_LFO + l) * VOICES + voice) * CHUNK + k] = value;
                    self.prev[voice][SRC_LFO + l] = value;
                }
                // The envelopes.
                for e in (0..3).filter(|&e| e == 0 || self.used[e]) {
                    let params = if (0..ENV_DESTS[e]).any(|kk| self.has[d_env(e, kk)]) {
                        let mut p = self.env_params(e, speed, |kk| {
                            self.val_before(c, d_env(e, kk), self.base[d_env(e, kk)], voice, k)
                        });
                        p.mode = self.set.env[e].mode;
                        p
                    } else {
                        let mut p = self.set.env[e];
                        for s in &mut p.step {
                            *s = (f64::from(*s) * speed / self.set.speed) as f32;
                        }
                        p
                    };
                    let level = self.env[e][voice].next(&params);
                    self.src[(e * VOICES + voice) * CHUNK + k] = level;
                    self.prev[voice][e] = level;
                }
                self.env_out[k * stride + voice] = self.env[0][voice].level();
            }
        }
    }
}

/// What an oscillator's switches and choices say for a chunk.
#[derive(Clone, Copy)]
struct OscSet {
    effect: usize,
    semitones: f32,
    cents: f32,
    /// Whether a route reaches its Semi or Detune.
    pitch_moved: bool,
    /// Unison's mode.
    mode: usize,
}

/// What one oscillator's render of a chunk reads.
struct Chunk<'a> {
    voices: &'a VoiceBlock,
    mask: u32,
    /// The chunk's length, the lanes to render and the lanes per voice's
    /// worth of them.
    n: usize,
    span: usize,
    lanes: usize,
    rate: f32,
    /// The copies each voice plays, and whether Hi-Quality is on.
    copies: usize,
    hi_quality: bool,
    /// Each lane's pitch at the chunk's first frame, its unison detune in
    /// semitones and its unison position as a share of the table.
    pitch: &'a [f32],
    unison: &'a [f32],
    unison_at: &'a [f32],
}

/// The oscillator's nine controls for each voice and frame: the voice's
/// played values where a route reaches the oscillator, else the parameters'
/// ramps.
struct Vals<'a> {
    played: Option<&'a [f32]>,
    ramps: &'a [Vec<f32>],
    at: usize,
}

impl Vals<'_> {
    #[inline]
    fn get(&self, which: usize, voice: usize, k: usize) -> f32 {
        match self.played {
            Some(played) => played[(voice * RAMP_OSC + which) * CHUNK + k],
            None => self.ramps[which][self.at + k],
        }
    }
}

/// Renders one oscillator's chunk into `o.out`: its table at Position, its
/// effect, every lane (R12 to R14, R23, R24). `v` holds its nine controls.
fn render_osc(o: &mut Osc, c: &Chunk<'_>, v: &Vals<'_>, set: OscSet) {
    let OscSet {
        effect,
        semitones,
        cents,
        pitch_moved,
        mode,
    } = set;
    let Osc {
        shaped,
        fm_phase,
        out,
        frame_mod,
        fm_source,
        amount_mod,
        pitch_mod,
    } = o;
    let (n, lanes, span) = (c.n, c.lanes, c.span);
    // Spec: wavetable R31. The effects render oversampled at a lighter factor
    // with Hi-Quality off and at the finest with it on.
    shaped.shapers_mut().set_oversampling(if c.hi_quality {
        HI_OVERSAMPLING
    } else {
        LIGHT_OVERSAMPLING
    });
    let osc = shaped.osc_mut();
    let p = osc.params_mut();
    p.octave = 0;
    p.semitone = 0;
    p.fine = cents;
    p.coarse = semitones;
    p.frame = 1.0;
    // The table's own interpolation made the frames between its drawn
    // ones; the read between frames is continuous (R9, R13).
    p.smooth = true;
    p.phase = 0.0;
    p.phase_random = if c.copies > 1 && mode != m::PHASE_SYNC {
        100.0
    } else {
        0.0
    };
    p.key_track = true;
    p.level = 100.0;
    p.reference = 440.0;
    osc.pitch_offsets_mut().copy_from_slice(c.unison);
    // Position, per frame, as the kit's frame input (R13), with unison's
    // position spread of each copy.
    let frames = osc.table().frame_count().max(1) as f32 - 1.0;
    let scale = frames / 100.0;
    let mut mask = c.mask;
    while mask != 0 {
        let voice = mask.trailing_zeros() as usize;
        mask &= mask - 1;
        let lane0 = voice * COPIES;
        for k in 0..n {
            let position = v.get(0, voice, k) * scale;
            for lane in lane0..lane0 + COPIES {
                frame_mod[k * lanes + lane] =
                    (position + c.unison_at[lane] * frames).clamp(0.0, frames);
            }
        }
    }
    // The effect's controls, per voice and frame (R14, R24).
    let mut fill = |slot: usize, which: usize, scale: f32| {
        let mut mask = c.mask;
        while mask != 0 {
            let voice = mask.trailing_zeros() as usize;
            mask &= mask - 1;
            for k in 0..n {
                let value = v.get(which, voice, k) * scale;
                amount_mod[slot][k * lanes + voice * COPIES..k * lanes + (voice + 1) * COPIES]
                    .fill(value);
            }
        }
    };
    let slots = &mut shaped.shapers_mut().slots;
    *slots = [Slot::default(); 2];
    let mut sources = [None, None];
    match effect {
        FX_FM => {
            slots[0] = Slot {
                mode: Mode::PdSub,
                amount: 100.0,
                character: 0.0,
            };
            sources[0] = Some(());
        }
        FX_CLASSIC => {
            slots[0].mode = Mode::PulseWidth;
            slots[1].mode = Mode::Sync;
            fill(0, 5, PW_SCALE);
            fill(1, 6, SYNC_SCALE);
        }
        3 => {
            slots[0].mode = Mode::LeanRight;
            slots[1].mode = Mode::Fold;
            fill(0, 7, 1.0);
            fill(1, 8, 1.0);
        }
        _ => {}
    }
    if sources[0].is_some() {
        render_fm(
            fm_phase,
            fm_source,
            c,
            v,
            semitones + cents / 100.0,
            pitch_moved.then_some(&pitch_mod[..]),
        );
    }
    let inputs = Inputs {
        lanes: span,
        pitch: c.pitch,
        pitch_step: Some(c.voices.pitch_step()),
        pitch_mod: pitch_moved.then(|| &pitch_mod[..n * lanes]),
        frame_mod: Some(&frame_mod[..n * lanes]),
    };
    let mods = ShaperInputs {
        amount_mod: [
            matches!(effect, FX_CLASSIC | 3).then(|| &amount_mod[0][..n * lanes]),
            matches!(effect, FX_CLASSIC | 3).then(|| &amount_mod[1][..n * lanes]),
        ],
        sources: [sources[0].map(|()| &fm_source[..(n + 1) * lanes]), None],
    };
    shaped.process(n, &inputs, &mods, &mut out[..n * lanes]);
}

/// The FM modulator, a sine at the note's frequency times 2^(2 Tune), for
/// every sounding lane, scaled to phase-modulate by 4 × Amount radians
/// (R14). Its frequency is worked out every [`GRID`] frames, from the pitch
/// the oscillator's Semi and Detune route move it to.
fn render_fm(
    phase: &mut [f32],
    source: &mut [f32],
    c: &Chunk<'_>,
    v: &Vals<'_>,
    semitones: f32,
    pitch_mod: Option<&[f32]>,
) {
    let steps = c.voices.pitch_step();
    let mut mask = c.mask;
    while mask != 0 {
        let voice = mask.trailing_zeros() as usize;
        mask &= mask - 1;
        for lane in voice * COPIES..voice * COPIES + c.copies {
            let mut now = phase[lane];
            let base = c.pitch[lane] + semitones + c.unison[lane];
            let mut from = 0;
            let mut last_inc = 0.0;
            while from < c.n {
                let tune = v.get(4, voice, from) / 100.0;
                let moved = pitch_mod.map_or(0.0, |moved| moved[from * c.lanes + lane]);
                let pitch = base + moved + steps[lane] * from as f32 + 24.0 * tune;
                let inc = 440.0 * ((pitch - 69.0) / 12.0).exp2() / c.rate;
                last_inc = inc;
                for k in from..(from + GRID).min(c.n) {
                    now = (now + inc).fract();
                    let index = FM_INDEX * FRAC_1_PI * v.get(3, voice, k) / 100.0;
                    source[k * c.lanes + lane] = sine(now) * index;
                }
                from += GRID;
            }
            phase[lane] = now;
            // The frame after the chunk's last, for the kit's read between
            // frames, so the chunk's end doesn't depend on where it falls.
            let index = FM_INDEX * FRAC_1_PI * v.get(3, voice, c.n - 1) / 100.0;
            source[c.n * c.lanes + lane] = sine((now + last_inc).fract()) * index;
        }
    }
}

/// The oscillator's controls in the order [`Vals`] holds them, as the
/// matrix's destinations: Position, Gain, Pan, then the six effect controls.
const OSC_DEST: [usize; RAMP_OSC] = [0, 7, 8, 1, 2, 3, 4, 5, 6];

impl Synth {
    /// Works out oscillator `o`'s nine controls for each active voice and
    /// frame of the chunk, and the pitch its Semi and Detune routes add (R25).
    fn play_osc(&mut self, c: &Ctx<'_>, o: usize, n: usize, mask: u32) {
        let moved = self.has[d_osc(o + 1, D_OSC_SEMI)] || self.has[d_osc(o + 1, D_OSC_DETUNE)];
        let (semi, detune) = (
            self.base[d_osc(o + 1, D_OSC_SEMI)],
            self.base[d_osc(o + 1, D_OSC_DETUNE)],
        );
        let mut m = mask;
        while m != 0 {
            let voice = m.trailing_zeros() as usize;
            m &= m - 1;
            for k in 0..n {
                for (which, &dest) in OSC_DEST.iter().enumerate() {
                    let base = f64::from(self.ramps[o * RAMP_OSC + which][c.at + k]);
                    let played = self.val(c, d_osc(o + 1, dest), base, voice, k) as f32;
                    self.played[o][(voice * RAMP_OSC + which) * CHUNK + k] = played;
                }
                if moved {
                    let by = (self.val(c, d_osc(o + 1, D_OSC_SEMI), semi, voice, k) - semi)
                        + (self.val(c, d_osc(o + 1, D_OSC_DETUNE), detune, voice, k) - detune)
                            / 100.0;
                    let lane = voice * COPIES;
                    for lane in lane..lane + COPIES {
                        self.osc[o].pitch_mod[k * self.lanes + lane] = by as f32;
                    }
                }
            }
        }
    }

    /// Renders `n` frames of the stretch from frame `at`, of a render call
    /// that began at `call_from`, for the first `span` lanes.
    fn chunk(
        &mut self,
        voices: &VoiceBlock,
        state: &State,
        call_from: usize,
        at: usize,
        n: usize,
        span: usize,
    ) {
        let set = self.set;
        let (lanes, stride) = (self.lanes, self.stride);
        // Spec: wavetable R19, R30. At an amount of 0, or with no mode, one
        // copy plays.
        let copies = if set.amount > 0.0 && set.mode != m::NO_UNISON {
            state.config().copies.clamp(1, COPIES)
        } else {
            1
        };
        let mask = voices.active_mask();
        let hi = span.div_ceil(COPIES).min(VOICES);
        let ahead = (at - call_from) as f32;
        let ctx = Ctx { voices, state, at };

        // 1. The envelopes and LFOs, frame by frame (R27, R28).
        self.env_out[..n * stride].fill(0.0);
        self.modulate(&ctx, n, mask);

        // 2. Each lane's pitch at the chunk's start with the tuned offset,
        //    and its unison law's detune, position and gains (R30). Shimmer
        //    and Noise read their walk at the grid line the chunk is in.
        let line = state.clock(at) & !(u64::from(lspk_voice::CONTROL_INTERVAL) - 1);
        for voice in 0..hi {
            let tuned = state.tuned_by(voice);
            let amount = if mask >> voice & 1 == 1 {
                (self.val(&ctx, D_UNISON, f64::from(set.amount) * 100.0, voice, 0) / 100.0) as f32
            } else {
                set.amount
            };
            self.amount_of[voice] = amount;
            let spread = if copies > 1 { amount } else { 0.0 };
            for i in 0..COPIES {
                let lane = voice * COPIES + i;
                self.lane_pitch[lane] =
                    voices.pitch()[lane] + voices.pitch_step()[lane] * ahead + tuned;
                let (cents, position) = if i < copies {
                    let walk = if matches!(set.mode, m::SHIMMER | m::NOISE) {
                        m::walk(self.seed, lane, line, f64::from(self.rate), set.mode)
                    } else {
                        [0.0; 2]
                    };
                    let copy = m::copy(set.mode, i, copies, amount, walk, self.note_draw[lane]);
                    (copy.cents / 100.0, copy.position)
                } else {
                    (0.0, 0.0)
                };
                self.unison[lane] = cents;
                self.unison_at[lane] = position;
                self.copy_gains[voice][i] = instrument::copy_gains(i, copies, spread);
            }
        }

        // 3. The oscillators.
        for o in 0..2 {
            if !set.on[o] {
                continue;
            }
            let routed = self.osc_routed[o];
            if routed {
                self.play_osc(&ctx, o, n, mask);
            }
            let view = Chunk {
                voices,
                mask,
                n,
                span,
                lanes,
                rate: self.rate,
                copies,
                hi_quality: state.config().hi_quality,
                pitch: &self.lane_pitch,
                unison: &self.unison,
                unison_at: &self.unison_at,
            };
            let vals = Vals {
                played: routed.then(|| &self.played[o][..]),
                ramps: &self.ramps[o * RAMP_OSC..(o + 1) * RAMP_OSC],
                at,
            };
            let pitch_moved =
                self.has[d_osc(o + 1, D_OSC_SEMI)] || self.has[d_osc(o + 1, D_OSC_DETUNE)];
            let osc_set = OscSet {
                effect: set.effect[o],
                semitones: set.semitones[o],
                cents: set.cents[o],
                pitch_moved,
                mode: set.mode,
            };
            render_osc(&mut self.osc[o], &view, &vals, osc_set);
        }

        // 4. The sub: one wave per voice, a sine and a square that Tone
        //    crossfades (R15).
        if set.sub_on {
            for voice in 0..VOICES {
                let lane = voice * COPIES;
                self.sub_pitch[voice] = voices.pitch()[lane]
                    + voices.pitch_step()[lane] * ahead
                    + state.tuned_by(voice);
                self.sub_step[voice] = voices.pitch_step()[lane];
            }
            let inputs = SubInputs {
                lanes: VOICES,
                pitch: &self.sub_pitch,
                pitch_step: Some(&self.sub_step),
                ..SubInputs::default()
            };
            for (sub, out) in self.sub.iter_mut().zip(&mut self.sub_out) {
                let p = sub.params_mut();
                p.octave = set.sub_octave;
                p.sweep = set.transpose;
                sub.process(n, &inputs, &mut out[..n * VOICES]);
            }
        }

        // 5. Each voice's oscillators and sub into the filters' inputs, as
        //    the routing says (R16), each with its gain, pan and Tone as that
        //    voice plays them.
        let used = n * 2 * stride;
        self.sig[0][..used].fill(0.0);
        self.sig[1][..used].fill(0.0);
        let mut m = mask;
        while m != 0 {
            let voice = m.trailing_zeros() as usize;
            m &= m - 1;
            for k in 0..n {
                let mut sides = [[0.0f32; 2]; 2];
                for o in (0..2).filter(|&o| set.on[o]) {
                    let from = k * lanes + voice * COPIES;
                    let (mut left, mut right) = (0.0, 0.0);
                    for (i, (l, r)) in self.copy_gains[voice].iter().enumerate().take(copies) {
                        let x = self.osc[o].out[from + i];
                        left += x * l;
                        right += x * r;
                    }
                    let gain = |which: usize| {
                        if self.osc_routed[o] {
                            self.played[o][(voice * RAMP_OSC + which) * CHUNK + k]
                        } else {
                            self.ramps[o * RAMP_OSC + which][at + k]
                        }
                    };
                    let (pan_left, pan_right) = pan_gains(gain(2) / 50.0);
                    sides[0][o] = left * gain(1) * pan_left;
                    sides[1][o] = right * gain(1) * pan_right;
                }
                let sub = if set.sub_on {
                    let tone = self.val(
                        &ctx,
                        D_SUB_TONE,
                        f64::from(self.ramps[RAMP_SUB_TONE][at + k]),
                        voice,
                        k,
                    ) as f32
                        / 100.0;
                    let level = self.val(
                        &ctx,
                        D_SUB_GAIN,
                        f64::from(self.ramps[RAMP_SUB_GAIN][at + k]),
                        voice,
                        k,
                    ) as f32;
                    let from = k * VOICES + voice;
                    let wave = (1.0 - tone) * self.sub_out[0][from] + tone * self.sub_out[1][from];
                    wave * level
                } else {
                    0.0
                };
                // Serial: both oscillators and the sub into Filter 1. Parallel:
                // each oscillator with the whole sub into its own filter.
                // Split: each with half the sub.
                let share = if set.routing == 2 { 0.5 } else { 1.0 };
                for (ch, [first, second]) in sides.into_iter().enumerate() {
                    let at = (k * 2 + ch) * stride + voice;
                    if set.routing == 0 {
                        self.sig[0][at] = first + second + sub;
                    } else {
                        self.sig[0][at] = first + sub * share;
                        self.sig[1][at] = second + sub * share;
                    }
                }
            }
        }

        // 6. The filters, each with its controls as every voice plays them; an
        //    off filter passes its signal through.
        for f in 0..2 {
            let base = RAMP_FILTER + f * 4;
            let mut m = mask;
            while m != 0 {
                let voice = m.trailing_zeros() as usize;
                m &= m - 1;
                for k in 0..n {
                    let play = |what: usize| {
                        self.val(
                            &ctx,
                            d_filter(f + 1, what),
                            f64::from(self.ramps[base + what][at + k]),
                            voice,
                            k,
                        ) as f32
                    };
                    let (freq, res, drive, morph) = (play(0), play(1), play(2), play(3));
                    let i = k * stride + voice;
                    self.cutoff[f][i] = pitch_of(freq, 440.0);
                    self.controls[f][0][i] = res / 100.0;
                    self.controls[f][1][i] = morph / 100.0;
                    self.controls[f][2][i] = drive / 24.0;
                }
            }
        }
        let run = |synth: &mut Self, f: usize, into: usize| {
            if !set.filter_on[f] {
                return;
            }
            let [res, morph, drive] = &synth.controls[f];
            let controls = PerFrame {
                cutoff: &synth.cutoff[f][..n * stride],
                res: Per::Voice(&res[..n * stride]),
                morph: Per::Voice(&morph[..n * stride]),
                drive: Per::Voice(&drive[..n * stride]),
            };
            synth.filters[f].set_setup(set.setup[f]);
            synth.filters[f].process(hi, n, &controls, &mut synth.sig[into][..used]);
        };
        run(self, 0, 0);
        if set.routing == 0 {
            // Filter 1 into Filter 2: the sub, which joined the oscillators
            // at Filter 1, passes through both.
            run(self, 1, 0);
        } else {
            run(self, 1, 1);
            for i in 0..used {
                self.sig[0][i] += self.sig[1][i];
            }
        }

        // 7. The voices under their envelopes and the Volume, into the
        //    stretch's output.
        let mut m = mask;
        while m != 0 {
            let voice = m.trailing_zeros() as usize;
            m &= m - 1;
            for k in 0..n {
                let volume = self.val(
                    &ctx,
                    D_VOLUME,
                    f64::from(self.ramps[RAMP_VOLUME][at + k]),
                    voice,
                    k,
                ) as f32;
                let level = self.env_out[k * stride + voice] * volume;
                for ch in 0..2 {
                    self.out[ch][at + k] += self.sig[0][(k * 2 + ch) * stride + voice] * level;
                }
            }
        }
    }
}

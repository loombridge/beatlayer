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
//! The tabs Matrix and Mod Sources, the MIDI sources, unison's modes and
//! Hi-Quality belong to the later parts: their parameter ids are reserved
//! below and nothing here uses them.

use core::f32::consts::{FRAC_1_PI, PI};
use core::ops::Range;

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
use crate::instrument::{self, Instrument, Parts, Section, State};
use crate::instrument_filter::{CIRCUITS, InstrumentFilter, Per, PerFrame, Setup};
use crate::sampler::FILTER_TYPE_CHOICES;
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
/// Unison's detune at an amount of 100 %, in cents (R30's Classic).
const UNISON_CENTS: f32 = 25.0;

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
    ]);

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
                    ]),
                ]),
            ),
            Page::new(PAGES[1], LayoutNode::Row(Vec::new())),
            Page::new(
                PAGES[2],
                LayoutNode::Row(vec![band(&[
                    (AMP_ATTACK, Widget::Knob),
                    (AMP_DECAY, Widget::Knob),
                    (AMP_SUSTAIN, Widget::Knob),
                    (AMP_RELEASE, Widget::Knob),
                    (AMP_ATTACK_SLOPE, Widget::Knob),
                    (AMP_DECAY_SLOPE, Widget::Knob),
                    (AMP_RELEASE_SLOPE, Widget::Knob),
                ])]),
            ),
            Page::new(
                PAGES[3],
                LayoutNode::Row(vec![band(&[(PB_RANGE, Widget::Number)])]),
            ),
        ],
    );
    let layout = DeviceLayout::new(LayoutNode::Pages(pages));
    DeviceType::native(WAVETABLE, "Wavetable", DeviceRole::Instrument).with_description(
        DeviceDescription::new(params)
            .with_layout(layout)
            .with_banks(&[
                (
                    "Oscillator 1",
                    &[
                        "Osc 1 On",
                        "Osc 1 Position",
                        "Osc 1 Gain",
                        "Osc 1 Pan",
                        "Osc 1 Semi",
                        "Osc 1 Detune",
                        "Osc 1 Effect",
                    ],
                ),
                (
                    "Oscillator 1 Effect",
                    &[
                        "Osc 1 FM Amount",
                        "Osc 1 FM Tune",
                        "Osc 1 PW",
                        "Osc 1 Sync",
                        "Osc 1 Warp",
                        "Osc 1 Fold",
                    ],
                ),
                (
                    "Oscillator 2",
                    &[
                        "Osc 2 On",
                        "Osc 2 Position",
                        "Osc 2 Gain",
                        "Osc 2 Pan",
                        "Osc 2 Semi",
                        "Osc 2 Detune",
                        "Osc 2 Effect",
                    ],
                ),
                (
                    "Oscillator 2 Effect",
                    &[
                        "Osc 2 FM Amount",
                        "Osc 2 FM Tune",
                        "Osc 2 PW",
                        "Osc 2 Sync",
                        "Osc 2 Warp",
                        "Osc 2 Fold",
                    ],
                ),
                ("Sub", &["Sub On", "Sub Gain", "Sub Tone", "Sub Octave"]),
                (
                    "Filter 1",
                    &[
                        "Filter 1 On",
                        "Filter 1 Type",
                        "Filter 1 Circuit",
                        "Filter 1 Freq",
                        "Filter 1 Res",
                        "Filter 1 Drive",
                        "Filter 1 Morph",
                    ],
                ),
                (
                    "Filter 2",
                    &[
                        "Filter 2 On",
                        "Filter 2 Type",
                        "Filter 2 Circuit",
                        "Filter 2 Freq",
                        "Filter 2 Res",
                        "Filter 2 Drive",
                        "Filter 2 Morph",
                    ],
                ),
                (
                    "Amp Envelope",
                    &[
                        "Amp Attack",
                        "Amp Decay",
                        "Amp Sustain",
                        "Amp Release",
                        "Amp Attack Slope",
                        "Amp Decay Slope",
                        "Amp Release Slope",
                    ],
                ),
                (
                    "Global",
                    &[
                        "Routing",
                        "Voice Mode",
                        "Poly Voices",
                        "Glide",
                        "Transpose",
                        "Volume",
                        "Unison Voices",
                        "Unison Amount",
                    ],
                ),
                ("MIDI", &["PB Range"]),
            ]),
    )
}

/// Adds Wavetable to `registry`.
pub fn register(registry: &mut Registry) {
    add_with_data(
        registry,
        wavetable_type(),
        |_, config| make(config),
        Some(std::sync::Arc::new(crate::wavetables::device_data)),
    );
}

/// Makes a Wavetable for `config`'s rate and block size. Every buffer and the
/// default tables are made here, off the audio thread.
#[must_use]
pub fn make(config: &NodeConfig) -> Wavetable {
    Wavetable::new(config)
}

/// Wavetable's audio-thread half.
pub struct Wavetable {
    section: Section,
    synth: Synth,
}

impl Wavetable {
    fn new(config: &NodeConfig) -> Self {
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
                    ..Parts::default()
                },
                VOICES,
                COPIES,
                rate,
                0x57A7_E7AB,
            ),
            synth: Synth::new(rate, config.max_frames),
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

    fn receive_data(&mut self, data: NodeData) {
        self.section.receive_data(&data);
        if let Some(table) = data.downcast_ref::<OscTable>() {
            self.synth.set_table(table);
        }
    }
}

/// The stages of the amplitude envelope.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    Idle,
    Attack,
    Decay,
    Sustain,
    Release,
}

/// What the envelope reads for a stretch: each segment's progress per frame
/// and its shape's exponent (R17, R27).
#[derive(Clone, Copy)]
struct Env {
    step: [f32; 3],
    power: [f32; 3],
    sustain: f32,
}

/// One voice's amplitude envelope. Spec: wavetable R27: a segment's shape is
/// x^(2^(−2s)) of its share of the way for slope s.
#[derive(Clone, Copy)]
struct Amp {
    stage: Stage,
    level: f32,
    /// How far through the segment, 0 to 1, and the level it started from.
    x: f32,
    from: f32,
}

impl Amp {
    const IDLE: Self = Self {
        stage: Stage::Idle,
        level: 0.0,
        x: 0.0,
        from: 0.0,
    };

    /// A note starts: from silence, or from the level a stolen voice had
    /// reached (instrument voices R4).
    fn start(&mut self, from_silence: bool) {
        self.from = if from_silence { 0.0 } else { self.level };
        self.level = self.from;
        self.x = 0.0;
        self.stage = Stage::Attack;
    }

    fn release(&mut self) {
        if self.stage != Stage::Idle {
            self.from = self.level;
            self.x = 0.0;
            self.stage = Stage::Release;
        }
    }

    /// The level of the next frame.
    fn next(&mut self, env: &Env) -> f32 {
        match self.stage {
            Stage::Idle => {}
            Stage::Attack => {
                self.x += env.step[0];
                if self.x >= 1.0 {
                    (self.level, self.x, self.stage) = (1.0, 0.0, Stage::Decay);
                } else {
                    self.level = self.from + (1.0 - self.from) * self.x.powf(env.power[0]);
                }
            }
            Stage::Decay => {
                self.x += env.step[1];
                if self.x >= 1.0 {
                    (self.level, self.stage) = (env.sustain, Stage::Sustain);
                } else {
                    self.level = 1.0 + (env.sustain - 1.0) * self.x.powf(env.power[1]);
                }
            }
            Stage::Sustain => self.level = env.sustain,
            Stage::Release => {
                self.x += env.step[2];
                if self.x >= 1.0 {
                    (self.level, self.stage) = (0.0, Stage::Idle);
                } else {
                    self.level = self.from * (1.0 - self.x.powf(env.power[2]));
                }
            }
        }
        self.level
    }
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
    env: Env,
    /// Unison's Amount, 0 to 1.
    amount: f32,
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
    amp: [Amp; VOICES],
    ramps: Vec<Vec<f32>>,
    set: Settings,
    /// Per chunk: each lane's pitch and Unison's detune per lane.
    lane_pitch: Vec<f32>,
    unison: Vec<f32>,
    /// Each oscillator's level and pan gains per frame, left and right.
    osc_gain: [[Vec<f32>; 2]; 2],
    /// The filters' inputs, the envelope, the cutoffs and the controls, for
    /// a chunk.
    sig: [Vec<f32>; 2],
    env_out: Vec<f32>,
    cutoff: [Vec<f32>; 2],
    controls: [[Vec<f32>; 3]; 2],
    /// The stretch's output, left and right.
    out: [Vec<f32>; 2],
}

impl Synth {
    fn new(rate: f64, max_frames: usize) -> Self {
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
        let frames = || vec![0.0; CHUNK];
        let signal = || vec![0.0; CHUNK * 2 * stride];
        let mono = || vec![0.0; CHUNK * stride];
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
            env: Env {
                step: [1.0; 3],
                power: [1.0; 3],
                sustain: 1.0,
            },
            amount: 0.0,
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
            amp: [Amp::IDLE; VOICES],
            ramps: (0..RAMPS).map(|_| vec![0.0; max_frames]).collect(),
            set,
            lane_pitch: vec![0.0; lanes],
            unison: vec![0.0; lanes],
            osc_gain: [[frames(), frames()], [frames(), frames()]],
            sig: [signal(), signal()],
            env_out: mono(),
            cutoff: [mono(), mono()],
            controls: [
                [frames(), frames(), frames()],
                [frames(), frames(), frames()],
            ],
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
    /// and choices into [`Settings`].
    fn snapshot(&mut self, frames: usize, params: &ParamBank) {
        let at = |id: ParamId| params.index(id).expect("the description holds it");
        for (to, id) in self.ramps.iter_mut().zip(ramp_ids()) {
            to[..frames].copy_from_slice(&params.ramp(at(id))[..frames]);
        }
        let value = |id: ParamId| params.value(at(id));
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
        let rate = f64::from(self.rate);
        let step = |ms: f64| (1.0 / (ms * 1e-3 * rate).max(1.0)) as f32;
        let power = |id| 2f32.powf(-2.0 * value(id) as f32);
        self.set = Settings {
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
            env: Env {
                step: [
                    step(value(AMP_ATTACK)),
                    step(value(AMP_DECAY)),
                    step(value(AMP_RELEASE)),
                ],
                power: [
                    power(AMP_ATTACK_SLOPE),
                    power(AMP_DECAY_SLOPE),
                    power(AMP_RELEASE_SLOPE),
                ],
                sustain: (value(AMP_SUSTAIN) / 100.0) as f32,
            },
            amount: (value(UNISON_AMOUNT) / 100.0) as f32,
        };
        self.out[0][..frames].fill(0.0);
        self.out[1][..frames].fill(0.0);
    }

    /// Drops everything the synth holds, for a device switched off.
    fn silence(&mut self) {
        self.amp = [Amp::IDLE; VOICES];
        for sub in &mut self.sub {
            sub.reset();
        }
        for osc in &mut self.osc {
            osc.fm_phase.fill(0.0);
        }
        self.out.iter_mut().for_each(|out| out.fill(0.0));
    }
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
                for osc in &mut self.osc {
                    osc.shaped.start(voice, key, onset);
                }
                for sub in &mut self.sub {
                    sub.start(voice, key, onset);
                }
                match start {
                    Start::Fresh | Start::Join { .. } => {
                        self.amp[voice].start(true);
                        for osc in &mut self.osc {
                            osc.fm_phase[lanes.clone()].fill(0.0);
                        }
                        for (filter, on) in self.filters.iter_mut().zip(self.set.filter_on) {
                            if on {
                                filter.reset_voice(voice, 0);
                            }
                        }
                    }
                    Start::Steal | Start::Retrigger => self.amp[voice].start(false),
                    Start::Legato => {}
                }
            }
            VoiceEventKind::Release => self.amp[voice].release(),
            VoiceEventKind::End { .. } => self.amp[voice] = Amp::IDLE,
        }
    }

    fn render(&mut self, voices: &mut VoiceBlock, state: &State, frames: Range<usize>) {
        let span = voices.span(16);
        if span == 0 || frames.is_empty() {
            return;
        }
        let mut at = frames.start;
        while at < frames.end {
            let n = CHUNK.min(frames.end - at);
            self.chunk(voices, state, frames.start, at, n, span);
            at += n;
        }
        // A voice whose envelope has run out is silent.
        let mut mask = voices.active_mask();
        while mask != 0 {
            let voice = mask.trailing_zeros() as usize;
            mask &= mask - 1;
            if self.amp[voice].stage == Stage::Idle {
                voices.end(voice);
            }
        }
    }
}

/// What one oscillator's render of a chunk reads.
struct Chunk<'a> {
    voices: &'a VoiceBlock,
    mask: u32,
    /// The stretch frame the chunk starts at, its length, the lanes to
    /// render and the lanes per voice's worth of them.
    at: usize,
    n: usize,
    span: usize,
    lanes: usize,
    rate: f32,
    /// The copies each voice plays.
    copies: usize,
    /// Each lane's pitch at the chunk's first frame, and its unison detune,
    /// in semitones.
    pitch: &'a [f32],
    unison: &'a [f32],
}

/// Renders one oscillator's chunk into `o.out`: its table at Position, its
/// effect, every lane (R12 to R14, R23, R24). `ramps` are its nine ramps.
fn render_osc(
    o: &mut Osc,
    c: &Chunk<'_>,
    ramps: &[Vec<f32>],
    effect: usize,
    semitones: f32,
    cents: f32,
) {
    let Osc {
        shaped,
        fm_phase,
        out,
        frame_mod,
        fm_source,
        amount_mod,
    } = o;
    let (n, at, lanes, span) = (c.n, c.at, c.lanes, c.span);
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
    p.phase_random = if c.copies > 1 { 100.0 } else { 0.0 };
    p.key_track = true;
    p.level = 100.0;
    p.reference = 440.0;
    osc.pitch_offsets_mut().copy_from_slice(c.unison);
    // Position, per frame, as the kit's frame input (R13).
    let scale = (osc.table().frame_count().max(1) - 1) as f32 / 100.0;
    for (k, &position) in ramps[0][at..at + n].iter().enumerate() {
        frame_mod[k * lanes..k * lanes + span].fill(position * scale);
    }
    // The effect's controls, per frame (R14, R24).
    let mut fill = |slot: usize, ramp: &[f32], scale: f32| {
        for (k, &value) in ramp[at..at + n].iter().enumerate() {
            amount_mod[slot][k * lanes..k * lanes + span].fill(value * scale);
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
            fill(0, &ramps[5], PW_SCALE);
            fill(1, &ramps[6], SYNC_SCALE);
        }
        3 => {
            slots[0].mode = Mode::LeanRight;
            slots[1].mode = Mode::Fold;
            fill(0, &ramps[7], 1.0);
            fill(1, &ramps[8], 1.0);
        }
        _ => {}
    }
    if sources[0].is_some() {
        render_fm(fm_phase, fm_source, c, ramps, semitones + cents / 100.0);
    }
    let inputs = Inputs {
        lanes: span,
        pitch: c.pitch,
        pitch_step: Some(c.voices.pitch_step()),
        pitch_mod: None,
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
/// (R14). Its frequency is worked out every [`GRID`] frames.
fn render_fm(
    phase: &mut [f32],
    source: &mut [f32],
    c: &Chunk<'_>,
    ramps: &[Vec<f32>],
    semitones: f32,
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
                let tune = ramps[4][c.at + from] / 100.0;
                let pitch = base + steps[lane] * from as f32 + 24.0 * tune;
                let inc = 440.0 * ((pitch - 69.0) / 12.0).exp2() / c.rate;
                last_inc = inc;
                for k in from..(from + GRID).min(c.n) {
                    now = (now + inc).fract();
                    let index = FM_INDEX * FRAC_1_PI * ramps[3][c.at + k] / 100.0;
                    source[k * c.lanes + lane] = sine(now) * index;
                }
                from += GRID;
            }
            phase[lane] = now;
            // The frame after the chunk's last, for the kit's read between
            // frames, so the chunk's end doesn't depend on where it falls.
            let index = FM_INDEX * FRAC_1_PI * ramps[3][c.at + c.n - 1] / 100.0;
            source[c.n * c.lanes + lane] = sine((now + last_inc).fract()) * index;
        }
    }
}

impl Synth {
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
        // Spec: wavetable R19. At an amount of 0 one copy plays.
        let copies = if set.amount > 0.0 {
            state.config().copies.clamp(1, COPIES)
        } else {
            1
        };
        let mask = voices.active_mask();
        let hi = span.div_ceil(COPIES).min(VOICES);
        let ahead = (at - call_from) as f32;

        // 1. Each lane's pitch at the chunk's start, with the tuned
        //    offset, and unison's detune (Classic's law, R30).
        for voice in 0..hi {
            let tuned = state.tuned_by(voice);
            for i in 0..COPIES {
                let lane = voice * COPIES + i;
                self.lane_pitch[lane] =
                    voices.pitch()[lane] + voices.pitch_step()[lane] * ahead + tuned;
                self.unison[lane] = if i < copies {
                    instrument::copy_cents(i, copies, UNISON_CENTS * set.amount) / 100.0
                } else {
                    0.0
                };
            }
        }

        // 2. The oscillators.
        let view = Chunk {
            voices,
            mask,
            at,
            n,
            span,
            lanes,
            rate: self.rate,
            copies,
            pitch: &self.lane_pitch,
            unison: &self.unison,
        };
        for o in 0..2 {
            if set.on[o] {
                let ramps = &self.ramps[o * RAMP_OSC..(o + 1) * RAMP_OSC];
                render_osc(
                    &mut self.osc[o],
                    &view,
                    ramps,
                    set.effect[o],
                    set.semitones[o],
                    set.cents[o],
                );
            }
        }

        // 3. The sub: one wave per voice, a sine and a square that Tone
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

        // 4. The envelope, and each oscillator's gain and pan per frame.
        let used = n * 2 * stride;
        self.env_out[..n * stride].fill(0.0);
        let mut m = mask;
        while m != 0 {
            let voice = m.trailing_zeros() as usize;
            m &= m - 1;
            for k in 0..n {
                self.env_out[k * stride + voice] = self.amp[voice].next(&set.env);
            }
        }
        for o in 0..2 {
            let base = o * RAMP_OSC;
            for k in 0..n {
                let gain = self.ramps[base + 1][at + k];
                let (left, right) = pan_gains(self.ramps[base + 2][at + k] / 50.0);
                self.osc_gain[o][0][k] = gain * left;
                self.osc_gain[o][1][k] = gain * right;
            }
        }

        // 5. Each voice's oscillators and sub into the filters' inputs, as
        //    the routing says (R16).
        self.sig[0][..used].fill(0.0);
        self.sig[1][..used].fill(0.0);
        let copy_gains: [(f32, f32); COPIES] =
            core::array::from_fn(|i| instrument::copy_gains(i, copies, set.amount));
        let mut m = mask;
        while m != 0 {
            let voice = m.trailing_zeros() as usize;
            m &= m - 1;
            for k in 0..n {
                let mut sides = [[0.0f32; 2]; 2];
                for o in (0..2).filter(|&o| set.on[o]) {
                    let from = k * lanes + voice * COPIES;
                    let (mut left, mut right) = (0.0, 0.0);
                    for (i, (l, r)) in copy_gains.iter().enumerate().take(copies) {
                        let x = self.osc[o].out[from + i];
                        left += x * l;
                        right += x * r;
                    }
                    sides[0][o] = left * self.osc_gain[o][0][k];
                    sides[1][o] = right * self.osc_gain[o][1][k];
                }
                let sub = if set.sub_on {
                    let tone = self.ramps[RAMP_SUB_TONE][at + k] / 100.0;
                    let from = k * VOICES + voice;
                    let wave = (1.0 - tone) * self.sub_out[0][from] + tone * self.sub_out[1][from];
                    wave * self.ramps[RAMP_SUB_GAIN][at + k]
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

        // 6. The filters, each with its controls per frame; an off filter
        //    passes its signal through.
        for f in 0..2 {
            let base = RAMP_FILTER + f * 4;
            for k in 0..n {
                let pitch = pitch_of(self.ramps[base][at + k], 440.0);
                self.cutoff[f][k * stride..k * stride + hi].fill(pitch);
                self.controls[f][0][k] = self.ramps[base + 1][at + k] / 100.0;
                self.controls[f][1][k] = self.ramps[base + 3][at + k] / 100.0;
                self.controls[f][2][k] = self.ramps[base + 2][at + k] / 24.0;
            }
        }
        let run = |synth: &mut Self, f: usize, into: usize| {
            if !set.filter_on[f] {
                return;
            }
            let [res, morph, drive] = &synth.controls[f];
            let controls = PerFrame {
                cutoff: &synth.cutoff[f][..n * stride],
                res: Per::Shared(&res[..n]),
                morph: Per::Shared(&morph[..n]),
                drive: Per::Shared(&drive[..n]),
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
        let sig = &self.sig[0];
        let mut m = mask;
        while m != 0 {
            let voice = m.trailing_zeros() as usize;
            m &= m - 1;
            for k in 0..n {
                let level = self.env_out[k * stride + voice] * self.ramps[RAMP_VOLUME][at + k];
                for ch in 0..2 {
                    self.out[ch][at + k] += sig[(k * 2 + ch) * stride + voice] * level;
                }
            }
        }
    }
}

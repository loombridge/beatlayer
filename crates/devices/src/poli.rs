//! Poli, the native polyphonic virtual analogue synthesiser (WP-8.16; the
//! bass and Poli spec, R1 to R3, R9 to R18): up to sixteen voices of two
//! detunable oscillators, the instruments' filter with its envelope, the
//! amplifier envelope, a per-voice LFO sent to pitch, pulse width and the
//! filter with the mod wheel scaling it, and Chorus-Ensemble's chorus after
//! the amplifier.
//!
//! It plays through the instruments' voice section and filters through the
//! instruments' filter. The pack page names no controls, so every control,
//! range and law is Beatlayer's own, as the spec says.

use core::f32::consts::TAU;
use core::ops::Range;

use bl_dsp::random::Random;
use bl_engine::{DeviceType, NodeConfig, NodeData, NodeEvent, Registry};
use bl_model::device_layout::{
    BODY_HEIGHT, ControlBand, LayoutNode, Page, PageStyle, PagesNode, TAB_HEIGHT, Widget,
};
use bl_model::param::db_to_gain;
use bl_model::{
    Device, DeviceDescription, DeviceLayout, DeviceRole, ParamDescriptor, ParamId, ParamUnit,
};
use lspk_dsp::envelope::{EnvelopeBank, EnvelopeParams};
use lspk_dsp::filter::pitch_of;
use lspk_voice::{VoiceBlock, VoiceEvent, VoiceEventKind};
use lspk_wavetable::osc::{
    Inputs, Layout as OscLayout, Oscillator, Params as OscParams, PhaseMode,
};
use lspk_wavetable::{Cache, FRAME_SIZE, SharedTable, Wavetable};

use crate::analogue::{self, Filtering, MIDDLE_C, REFERENCE};
use crate::chorus_ensemble::{Chorus, Controls};
use crate::framework::{Audio, NativeDevice, ParamBank, add};
use crate::instrument::{Instrument, Parts, Section, Sources, State};
use crate::instrument_filter::Setup;
use crate::polysynth::{DIVISION_SIXTEENTHS, LFO_DIVISIONS};

/// Poli's device type.
pub const POLI: &str = "beatlayer.poli";

/// The most notes that sound at once.
pub const VOICES: ParamId = ParamId(0);
/// The glide time, in milliseconds.
pub const GLIDE: ParamId = ParamId(1);
/// The pitch wheel's range, in semitones.
pub const PB_RANGE: ParamId = ParamId(2);
/// The first oscillator's shape.
pub const OSC1_SHAPE: ParamId = ParamId(3);
/// The first oscillator's octave.
pub const OSC1_OCTAVE: ParamId = ParamId(4);
/// The first oscillator's level.
pub const OSC1_LEVEL: ParamId = ParamId(5);
/// The second oscillator's shape.
pub const OSC2_SHAPE: ParamId = ParamId(6);
/// The second oscillator's octave.
pub const OSC2_OCTAVE: ParamId = ParamId(7);
/// The second oscillator's level.
pub const OSC2_LEVEL: ParamId = ParamId(8);
/// The second oscillator's detune, in cents.
pub const OSC2_DETUNE: ParamId = ParamId(9);
/// A pulse's width, in per cent.
pub const PW: ParamId = ParamId(10);
/// The filter's type.
pub const FILTER_TYPE: ParamId = ParamId(11);
/// The filter's slope.
pub const SLOPE: ParamId = ParamId(12);
/// The filter's circuit.
pub const CIRCUIT: ParamId = ParamId(13);
/// The filter's cutoff, in hertz.
pub const FREQ: ParamId = ParamId(14);
/// The filter's resonance, in per cent.
pub const RES: ParamId = ParamId(15);
/// How much the key moves the cutoff, in per cent.
pub const KEY: ParamId = ParamId(16);
/// How far the filter envelope moves the cutoff, in per cent of six octaves.
pub const ENV_AMOUNT: ParamId = ParamId(17);
/// The filter envelope's attack, in milliseconds.
pub const FILTER_ATTACK: ParamId = ParamId(18);
/// The filter envelope's decay, in milliseconds.
pub const FILTER_DECAY: ParamId = ParamId(19);
/// The filter envelope's sustain, in per cent.
pub const FILTER_SUSTAIN: ParamId = ParamId(20);
/// The filter envelope's release, in milliseconds.
pub const FILTER_RELEASE: ParamId = ParamId(21);
/// The amplifier envelope's attack, in milliseconds.
pub const ATTACK: ParamId = ParamId(22);
/// The amplifier envelope's decay, in milliseconds.
pub const DECAY: ParamId = ParamId(23);
/// The amplifier envelope's sustain, in per cent.
pub const SUSTAIN: ParamId = ParamId(24);
/// The amplifier envelope's release, in milliseconds.
pub const RELEASE: ParamId = ParamId(25);
/// How much the velocity moves the volume, in per cent.
pub const VEL_VOL: ParamId = ParamId(26);
/// The output level.
pub const VOLUME: ParamId = ParamId(27);
/// The LFO's shape.
pub const LFO_SHAPE: ParamId = ParamId(28);
/// Whether the LFO's rate follows the tempo.
pub const LFO_SYNC: ParamId = ParamId(29);
/// The LFO's rate, in hertz.
pub const LFO_RATE: ParamId = ParamId(30);
/// The LFO's cycle as a division of the tempo.
pub const LFO_DIVISION: ParamId = ParamId(31);
/// How long the LFO takes to fade in from a note's start, in milliseconds.
pub const LFO_DELAY: ParamId = ParamId(32);
/// How far the LFO moves the pitch, in per cent of an octave.
pub const LFO_PITCH: ParamId = ParamId(33);
/// How far the LFO moves a pulse's width, in per cent of its range.
pub const LFO_PW: ParamId = ParamId(34);
/// How far the LFO moves the cutoff, in per cent of four octaves.
pub const LFO_FILTER: ParamId = ParamId(35);
/// How much the mod wheel scales the LFO's depth, in per cent.
pub const WHEEL: ParamId = ParamId(36);
/// The chorus's switch.
pub const CHORUS: ParamId = ParamId(37);
/// The chorus's rate, in hertz.
pub const CHORUS_RATE: ParamId = ParamId(38);
/// The chorus's amount, in per cent.
pub const CHORUS_AMOUNT: ParamId = ParamId(39);
/// The chorus's dry/wet balance, in per cent.
pub const CHORUS_DRY_WET: ParamId = ParamId(40);

/// The oscillators' shapes, in the chooser's order.
pub const SHAPES: [&str; 3] = ["Saw", "Square", "Pulse"];
/// The filter's types.
pub const FILTER_TYPES: [&str; 2] = ["Low-pass", "High-pass"];
/// The filter's slopes.
pub const SLOPES: [&str; 2] = ["12 dB", "24 dB"];
/// The circuits Poli offers, in the chooser's order: Clean, MS2 and PRD,
/// which are entries 0, 2 and 4 of the instruments' filter.
pub const CIRCUITS: [&str; 3] = ["Clean", "MS2", "PRD"];
const CIRCUIT_ENTRIES: [usize; 3] = [0, 2, 4];
/// The LFO's shapes, in the chooser's order.
pub const LFO_SHAPES: [&str; 5] = ["Sine", "Triangle", "Square", "Saw", "S&H"];

const SAW: usize = 0;
const SQUARE: usize = 1;
const PULSE: usize = 2;
const SINE: usize = 0;
const TRIANGLE: usize = 1;
const LFO_SQUARE: usize = 2;
const LFO_SAW: usize = 3;

/// Spec: bass and Poli R11, R13. How far a full Env Amount moves the
/// cutoff (six octaves), a full LFO amount (four) and a full LFO amount
/// moves the pitch (an octave), in semitones.
const ENV_SEMITONES: f32 = 72.0;
const LFO_FILTER_SEMITONES: f32 = 48.0;
const LFO_PITCH_SEMITONES: f32 = 12.0;
/// Spec: bass and Poli R10, R13. A pulse's width runs from 5 to 95 %; a full
/// LFO amount swings it by half of that, 45 points, so the LFO sweeps the
/// whole range around the middle.
const PW_MIN: f32 = 0.05;
const PW_MAX: f32 = 0.95;
const LFO_PW_SPAN: f32 = 0.45;
/// The pulse table's frames: widths from 5 to 95 % in steps of 2 points.
const PULSE_FRAMES: usize = 46;
const MAX_VOICES: usize = 16;

/// A saw, a square or the pulses of every width, band-limited by the kit's
/// table.
fn shape_table(shape: usize) -> SharedTable {
    let frames = if shape == PULSE { PULSE_FRAMES } else { 1 };
    let table = Wavetable::from_fn(frames, |frame, sample| {
        let p = sample as f64 / FRAME_SIZE as f64;
        match shape {
            SAW => (2.0 * p - 1.0) as f32,
            SQUARE => {
                if p < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
            _ => {
                let width = f64::from(PW_MIN) + 0.02 * frame as f64;
                if p < width { 1.0 } else { -1.0 }
            }
        }
    })
    .expect("a table of whole frames");
    Cache::global().share(table)
}

/// Poli's device type.
#[must_use]
pub fn poli_type() -> DeviceType {
    let time = |id, name: &str, max: f64, default: f64| {
        ParamDescriptor::continuous(id, name, (0.0, max), default, ParamUnit::Milliseconds).power(3)
    };
    let percent = |id, name: &str, default: f64| {
        ParamDescriptor::continuous(id, name, (0.0, 100.0), default, ParamUnit::Percent)
    };
    let amount = |id, name: &str| {
        ParamDescriptor::continuous(id, name, (-100.0, 100.0), 0.0, ParamUnit::Percent)
    };
    let octave = |id, name: &str| {
        ParamDescriptor::integer(id, name, (-2, 2), 0, ParamUnit::None).decimals(0)
    };
    let params = vec![
        crate::instrument::voices(VOICES, MAX_VOICES as i64, 8),
        ParamDescriptor::continuous(GLIDE, "Glide", (0.0, 2_000.0), 0.0, ParamUnit::Milliseconds)
            .power(2)
            .describe("How long a note takes to glide from the pitch of the note before it."),
        ParamDescriptor::integer(PB_RANGE, "PB Range", (0, 24), 2, ParamUnit::Semitones)
            .decimals(0)
            .describe("Sets the pitch wheel's range, in semitones."),
        ParamDescriptor::choice(OSC1_SHAPE, "Osc 1 Shape", &SHAPES, SAW)
            .describe("The first oscillator's waveform."),
        octave(OSC1_OCTAVE, "Osc 1 Octave").describe("Transposes the first oscillator in octaves."),
        ParamDescriptor::gain(OSC1_LEVEL, "Osc 1 Level", 0.0, -6.0)
            .describe("Sets the first oscillator's level, down to silence."),
        ParamDescriptor::choice(OSC2_SHAPE, "Osc 2 Shape", &SHAPES, SAW)
            .describe("The second oscillator's waveform."),
        octave(OSC2_OCTAVE, "Osc 2 Octave").describe("Transposes the second oscillator in octaves."),
        ParamDescriptor::gain(OSC2_LEVEL, "Osc 2 Level", 0.0, -6.0)
            .describe("Sets the second oscillator's level, down to silence."),
        ParamDescriptor::continuous(OSC2_DETUNE, "Osc 2 Detune", (-50.0, 50.0), 7.0, ParamUnit::Cents)
            .describe("Detunes the second oscillator against the first, in cents."),
        ParamDescriptor::continuous(PW, "PW", (5.0, 95.0), 50.0, ParamUnit::Percent)
            .describe("The width of an oscillator's pulse."),
        ParamDescriptor::choice(FILTER_TYPE, "Filter Type", &FILTER_TYPES, 0)
            .describe("Low-pass or high-pass."),
        ParamDescriptor::choice(SLOPE, "Slope", &SLOPES, 1)
            .describe("The filter's slope: 12 or 24 dB per octave."),
        ParamDescriptor::choice(CIRCUIT, "Circuit", &CIRCUITS, 0)
            .describe("The filter's circuit: how it behaves when pushed."),
        ParamDescriptor::continuous(FREQ, "Freq", (20.0, 20_000.0), 2_000.0, ParamUnit::Hertz)
            .logarithmic()
            .describe("Sets the filter's cutoff."),
        percent(RES, "Res", 0.0).describe("Raises the filter's resonance."),
        percent(KEY, "Key", 0.0)
            .describe("How much the played note moves the cutoff: one octave per octave at 100 %."),
        amount(ENV_AMOUNT, "Env Amount")
            .describe("How far the filter envelope moves the cutoff, up to six octaves."),
        time(FILTER_ATTACK, "Filter Attack", 10_000.0, 1.0)
            .describe("How long the filter envelope takes to rise."),
        time(FILTER_DECAY, "Filter Decay", 10_000.0, 300.0)
            .describe("How long the filter envelope takes to fall to its sustain."),
        percent(FILTER_SUSTAIN, "Filter Sustain", 50.0)
            .describe("The level the filter envelope holds while a key is held."),
        time(FILTER_RELEASE, "Filter Release", 10_000.0, 200.0)
            .describe("How long the filter envelope takes to fall after a key is let go."),
        time(ATTACK, "Attack", 10_000.0, 5.0)
            .describe("How long the amplifier envelope takes to rise."),
        time(DECAY, "Decay", 10_000.0, 300.0)
            .describe("How long the amplifier envelope takes to fall to its sustain."),
        percent(SUSTAIN, "Sustain", 80.0)
            .describe("The level the amplifier envelope holds while a key is held."),
        time(RELEASE, "Release", 10_000.0, 300.0)
            .describe("How long the amplifier envelope takes to fall to silence."),
        percent(VEL_VOL, "Vel > Vol", 0.0).describe("How much the notes' velocity moves the volume."),
        ParamDescriptor::gain(VOLUME, "Volume", 0.0, -6.0)
            .describe("Sets the instrument's output level, down to silence."),
        ParamDescriptor::choice(LFO_SHAPE, "LFO Shape", &LFO_SHAPES, SINE)
            .describe("The LFO's waveform."),
        ParamDescriptor::toggle(LFO_SYNC, "LFO Sync", false)
            .describe("Sets the LFO's speed by the tempo instead of in hertz."),
        ParamDescriptor::continuous(LFO_RATE, "LFO Rate", (0.05, 20.0), 4.0, ParamUnit::Hertz)
            .logarithmic()
            .describe("The LFO's rate in hertz, with Sync off."),
        ParamDescriptor::choice(LFO_DIVISION, "LFO Division", &LFO_DIVISIONS, 2)
            .describe("The LFO's cycle as a division of the tempo, with Sync on."),
        time(LFO_DELAY, "LFO Delay", 5_000.0, 0.0)
            .describe("How long the LFO takes to fade in from the start of a note."),
        amount(LFO_PITCH, "LFO Pitch").describe("How far the LFO moves the pitch, up to an octave."),
        amount(LFO_PW, "LFO PW").describe("How far the LFO moves a pulse's width, over its whole range."),
        amount(LFO_FILTER, "LFO Filter").describe("How far the LFO moves the cutoff, up to four octaves."),
        percent(WHEEL, "Wheel", 0.0)
            .describe("How much the mod wheel scales the LFO's depth: at 100 % the LFO is silent with the wheel down."),
        ParamDescriptor::toggle(CHORUS, "Chorus", true).describe("Turns the chorus on."),
        ParamDescriptor::continuous(CHORUS_RATE, "Chorus Rate", (0.01, 20.0), 0.8, ParamUnit::Hertz)
            .logarithmic()
            .describe("The chorus's modulation rate."),
        percent(CHORUS_AMOUNT, "Chorus Amount", 50.0)
            .describe("The depth of the chorus's delay modulation."),
        percent(CHORUS_DRY_WET, "Chorus Dry/Wet", 50.0)
            .describe("The balance between the unprocessed and the chorused signal."),
    ];
    let height = BODY_HEIGHT - TAB_HEIGHT;
    let page = |name: &str, controls: &[(ParamId, Widget)]| {
        Page::new(
            name,
            LayoutNode::Band(ControlBand::stacked(height, controls)),
        )
    };
    let layout = DeviceLayout::new(LayoutNode::Pages(PagesNode::new(
        "Pages",
        PageStyle::Tabs,
        vec![
            page(
                "Oscillators",
                &[
                    (OSC1_SHAPE, Widget::Chooser),
                    (OSC1_OCTAVE, Widget::Number),
                    (OSC1_LEVEL, Widget::Knob),
                    (OSC2_SHAPE, Widget::Chooser),
                    (OSC2_OCTAVE, Widget::Number),
                    (OSC2_LEVEL, Widget::Knob),
                    (OSC2_DETUNE, Widget::Knob),
                    (PW, Widget::Knob),
                    (VOICES, Widget::Number),
                    (GLIDE, Widget::Knob),
                    (PB_RANGE, Widget::Number),
                ],
            ),
            page(
                "Filter",
                &[
                    (FILTER_TYPE, Widget::Chooser),
                    (SLOPE, Widget::Chooser),
                    (CIRCUIT, Widget::Chooser),
                    (FREQ, Widget::Knob),
                    (RES, Widget::Knob),
                    (KEY, Widget::Knob),
                    (ENV_AMOUNT, Widget::Knob),
                    (FILTER_ATTACK, Widget::Knob),
                    (FILTER_DECAY, Widget::Knob),
                    (FILTER_SUSTAIN, Widget::Knob),
                    (FILTER_RELEASE, Widget::Knob),
                ],
            ),
            page(
                "Amplifier",
                &[
                    (ATTACK, Widget::Knob),
                    (DECAY, Widget::Knob),
                    (SUSTAIN, Widget::Knob),
                    (RELEASE, Widget::Knob),
                    (VEL_VOL, Widget::Knob),
                    (VOLUME, Widget::Knob),
                ],
            ),
            page(
                "Modulation",
                &[
                    (LFO_SHAPE, Widget::Chooser),
                    (LFO_SYNC, Widget::Toggle),
                    (LFO_RATE, Widget::Knob),
                    (LFO_DIVISION, Widget::Chooser),
                    (LFO_DELAY, Widget::Knob),
                    (LFO_PITCH, Widget::Knob),
                    (LFO_PW, Widget::Knob),
                    (LFO_FILTER, Widget::Knob),
                    (WHEEL, Widget::Knob),
                ],
            ),
            page(
                "Chorus",
                &[
                    (CHORUS, Widget::Toggle),
                    (CHORUS_RATE, Widget::Knob),
                    (CHORUS_AMOUNT, Widget::Knob),
                    (CHORUS_DRY_WET, Widget::Knob),
                ],
            ),
        ],
    )));
    DeviceType::native(POLI, "Poli", DeviceRole::Instrument).with_description(
        DeviceDescription::new(params)
            .with_layout(layout)
            .with_banks(&[
                (
                    "Oscillator 1",
                    &[
                        "Osc 1 Shape",
                        "Osc 1 Octave",
                        "Osc 1 Level",
                        "PW",
                        "Voices",
                        "Glide",
                        "PB Range",
                    ],
                ),
                (
                    "Oscillator 2",
                    &["Osc 2 Shape", "Osc 2 Octave", "Osc 2 Level", "Osc 2 Detune"],
                ),
                (
                    "Filter",
                    &[
                        "Filter Type",
                        "Slope",
                        "Circuit",
                        "Freq",
                        "Res",
                        "Key",
                        "Env Amount",
                    ],
                ),
                (
                    "Filter Envelope",
                    &[
                        "Filter Attack",
                        "Filter Decay",
                        "Filter Sustain",
                        "Filter Release",
                    ],
                ),
                (
                    "Amplifier",
                    &[
                        "Attack",
                        "Decay",
                        "Sustain",
                        "Release",
                        "Vel > Vol",
                        "Volume",
                    ],
                ),
                (
                    "LFO",
                    &[
                        "LFO Shape",
                        "LFO Sync",
                        "LFO Rate",
                        "LFO Division",
                        "LFO Delay",
                    ],
                ),
                ("LFO Sends", &["LFO Pitch", "LFO PW", "LFO Filter", "Wheel"]),
                (
                    "Chorus",
                    &["Chorus", "Chorus Rate", "Chorus Amount", "Chorus Dry/Wet"],
                ),
            ]),
    )
}

/// A factory preset: a name and the parameters it sets, the rest staying
/// at their defaults.
pub struct Preset {
    pub name: &'static str,
    pub values: Vec<(ParamId, f64)>,
}

/// Poli's factory presets, Beatlayer's own.
#[must_use]
pub fn presets() -> Vec<Preset> {
    vec![
        Preset {
            name: "Warm Pad",
            values: vec![
                (OSC2_DETUNE, 12.0),
                (FREQ, 900.0),
                (ENV_AMOUNT, 25.0),
                (FILTER_ATTACK, 600.0),
                (ATTACK, 500.0),
                (SUSTAIN, 100.0),
                (RELEASE, 1_200.0),
                (CHORUS_AMOUNT, 70.0),
            ],
        },
        Preset {
            name: "Wide Saws",
            values: vec![
                (OSC1_OCTAVE, -1.0),
                (OSC2_OCTAVE, 1.0),
                (OSC2_DETUNE, 18.0),
                (FREQ, 3_000.0),
                (RES, 15.0),
                (RELEASE, 400.0),
                (LFO_FILTER, 10.0),
                (LFO_RATE, 0.5),
                (CHORUS_DRY_WET, 70.0),
            ],
        },
        Preset {
            name: "Pulse Pluck",
            values: vec![
                (OSC1_SHAPE, 2.0),
                (OSC2_SHAPE, 2.0),
                (PW, 30.0),
                (OSC2_DETUNE, 5.0),
                (FREQ, 700.0),
                (RES, 35.0),
                (ENV_AMOUNT, 70.0),
                (FILTER_DECAY, 250.0),
                (FILTER_SUSTAIN, 0.0),
                (DECAY, 350.0),
                (SUSTAIN, 0.0),
                (RELEASE, 200.0),
                (LFO_PW, 30.0),
                (LFO_RATE, 2.0),
                (OSC1_LEVEL, db_to_gain(-8.0)),
                (OSC2_LEVEL, db_to_gain(-8.0)),
            ],
        },
    ]
}

/// Adds Poli to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, poli_type(), |device, config| {
        Poli::new(device, config)
    });
}

/// Poli's audio-thread half.
pub struct Poli {
    section: Section,
    voice: Voice,
    chorus: Chorus,
    rate: f64,
    /// The chorus's right channel; the voices' sum is the left.
    right: Vec<f32>,
}

impl Poli {
    fn new(device: &Device, config: &NodeConfig) -> Self {
        let rate = config.sample_rate.as_f64();
        let seed = device.id.0.get();
        Self {
            // Spec: bass and Poli R9. Up to 16 voices with Glide as
            // polyphonic Portamento and the pitch wheel's range.
            section: Section::new(
                Parts {
                    voices: Some(VOICES),
                    glide_time: Some(GLIDE),
                    poly_glide: true,
                    pb_range: Some(PB_RANGE),
                    ..Parts::default()
                },
                MAX_VOICES,
                1,
                rate,
                seed,
            ),
            voice: Voice::new(rate, config.max_frames, seed),
            chorus: Chorus::new(rate),
            rate,
            right: vec![0.0; config.max_frames.max(1)],
        }
    }
}

impl NativeDevice for Poli {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        let frames = audio.frames();
        let channels = audio.channels();
        if frames == 0 || channels == 0 {
            return;
        }
        self.voice.snapshot(audio, params);
        self.section.process(audio, params, &mut self.voice);

        // Spec: bass and Poli R14. The chorus, in Chorus-Ensemble's Chorus
        // mode with two taps, after the amplifier, in stereo.
        let at = |id: ParamId| params.index(id).expect("the description holds it");
        let on = params.value(at(CHORUS)) >= 0.5;
        let (rate, amount, dry_wet) = (
            params.ramp(at(CHORUS_RATE)),
            params.ramp(at(CHORUS_AMOUNT)),
            params.ramp(at(CHORUS_DRY_WET)),
        );
        for frame in 0..frames {
            let dry = self.voice.out[frame];
            let (left, right) = if on {
                let controls = Controls {
                    mode: 0,
                    time: 0,
                    taps: 1,
                    feedback_invert: false,
                    rate: rate[frame],
                    amount: amount[frame],
                    feedback: 0.0,
                    high_pass: 20.0,
                    width: 100.0,
                    warmth: 0.0,
                    dry_wet: dry_wet[frame],
                    vibrato_offset: 180.0,
                    vibrato_shape: 0.0,
                };
                let mixed = self.chorus.frame(self.rate, &controls, [dry, dry]);
                (mixed[0], mixed[1])
            } else {
                (dry, dry)
            };
            self.voice.out[frame] = left;
            self.right[frame] = right;
        }
        audio
            .channel_mut(0)
            .copy_from_slice(&self.voice.out[..frames]);
        for channel in 1..channels {
            audio
                .channel_mut(channel)
                .copy_from_slice(&self.right[..frames]);
        }
    }

    fn flush(&mut self, _events: &[NodeEvent], _params: &ParamBank) {
        // A switched-off instrument is silent and holds no notes.
        self.section.reset();
        self.voice.silence();
    }

    fn receive_data(&mut self, data: NodeData) {
        self.section.receive_data(&data);
    }
}

/// One voice's LFO: its phase, the value a sample-and-hold holds and the
/// generator that draws it.
struct Lfos {
    /// The device's seed and the number of notes started: each note seeds
    /// its voice's generator afresh, so the values a note draws don't depend
    /// on which voice it took or where blocks fall.
    seed: u64,
    notes: u64,
    phase: [f32; MAX_VOICES],
    held: [f32; MAX_VOICES],
    random: [Random; MAX_VOICES],
}

impl Lfos {
    fn new(seed: u64) -> Self {
        Self {
            seed,
            notes: 0,
            phase: [0.0; MAX_VOICES],
            held: [0.0; MAX_VOICES],
            random: core::array::from_fn(|_| Random::seeded(seed)),
        }
    }

    /// Spec: bass and Poli R13. Starts `voice`'s LFO over with its note.
    fn restart(&mut self, voice: usize) {
        self.notes += 1;
        self.random[voice] = Random::seeded(analogue::note_seed(self.seed, self.notes));
        self.phase[voice] = 0.0;
        self.held[voice] = self.random[voice].next_bipolar() as f32;
    }

    /// Moves `voice`'s LFO on by `cycles` and gives its value, from −1 to 1.
    fn tick(&mut self, voice: usize, cycles: f32, shape: usize) -> f32 {
        let phase = &mut self.phase[voice];
        *phase += cycles;
        if *phase >= 1.0 {
            *phase -= phase.floor();
            self.held[voice] = self.random[voice].next_bipolar() as f32;
        }
        lfo_wave(shape, *phase, self.held[voice])
    }
}

/// The LFO's waveform at `phase` (0 to 1), all starting at their middle but
/// the square and the saw, which start at their top and bottom; `held` is
/// what S&H holds.
fn lfo_wave(shape: usize, phase: f32, held: f32) -> f32 {
    match shape {
        SINE => (TAU * phase).sin(),
        TRIANGLE => {
            if phase < 0.25 {
                4.0 * phase
            } else if phase < 0.75 {
                2.0 - 4.0 * phase
            } else {
                4.0 * phase - 4.0
            }
        }
        LFO_SQUARE => {
            if phase < 0.5 {
                1.0
            } else {
                -1.0
            }
        }
        LFO_SAW => 2.0 * phase - 1.0,
        _ => held,
    }
}

/// Everything but the voice section: the [`Instrument`] the voices play.
struct Voice {
    rate: f64,
    stride: usize,
    osc1: Oscillator,
    osc2: Oscillator,
    tables: [SharedTable; 3],
    shapes: [usize; 2],
    amp: EnvelopeBank,
    fenv: EnvelopeBank,
    amp_params: EnvelopeParams,
    fenv_params: EnvelopeParams,
    filtering: Filtering,
    lfos: Lfos,
    /// The stretch's parameters, frame by frame; Resonance is kept from 0 to
    /// 1, the amounts from −1 to 1 and the widths as fractions.
    level1: Vec<f32>,
    level2: Vec<f32>,
    detune: Vec<f32>,
    pw: Vec<f32>,
    freq: Vec<f32>,
    res: Vec<f32>,
    key: Vec<f32>,
    env_amount: Vec<f32>,
    vel_vol: Vec<f32>,
    volume: Vec<f32>,
    lfo_hz: Vec<f32>,
    lfo_pitch: Vec<f32>,
    lfo_pw: Vec<f32>,
    lfo_filter: Vec<f32>,
    wheel: Vec<f32>,
    /// The stretch's per-voice buffers, `frames × stride` each.
    osc1_out: Vec<f32>,
    osc2_out: Vec<f32>,
    pitch_mod1: Vec<f32>,
    pitch_mod2: Vec<f32>,
    frame_mod: Vec<f32>,
    amp_out: Vec<f32>,
    fenv_out: Vec<f32>,
    lfo_out: Vec<f32>,
    /// The stretch's mixed output, one value a frame.
    out: Vec<f32>,
    octaves: [i32; 2],
    setup: Setup,
    lfo_shape: usize,
    /// The LFO's rate in hertz when it follows the tempo, else 0.
    sync_hz: f32,
    lfo_delay_frames: f64,
}

impl Voice {
    fn new(rate: f64, max_frames: usize, seed: u64) -> Self {
        let max_frames = max_frames.max(1);
        let layout = OscLayout {
            voices: MAX_VOICES,
            lanes_per_voice: 1,
        };
        let tables = [SAW, SQUARE, PULSE].map(shape_table);
        let osc = |salt: u64| {
            Oscillator::new(
                tables[SAW].clone(),
                layout,
                rate as f32,
                max_frames,
                0x0901_1000 + salt,
            )
        };
        let (osc1, osc2) = (osc(1), osc(2));
        let stride = osc1.lane_count();
        let ramp = || vec![0.0; max_frames];
        let buffer = || vec![0.0; max_frames * stride];
        Self {
            rate,
            stride,
            osc1,
            osc2,
            tables,
            shapes: [SAW; 2],
            amp: analogue::envelopes(MAX_VOICES, rate),
            fenv: analogue::envelopes(MAX_VOICES, rate),
            amp_params: EnvelopeParams::AMP,
            fenv_params: EnvelopeParams::AMP,
            filtering: Filtering::new(rate, MAX_VOICES, max_frames),
            lfos: Lfos::new(seed),
            level1: ramp(),
            level2: ramp(),
            detune: ramp(),
            pw: ramp(),
            freq: ramp(),
            res: ramp(),
            key: ramp(),
            env_amount: ramp(),
            vel_vol: ramp(),
            volume: ramp(),
            lfo_hz: ramp(),
            lfo_pitch: ramp(),
            lfo_pw: ramp(),
            lfo_filter: ramp(),
            wheel: ramp(),
            osc1_out: buffer(),
            osc2_out: buffer(),
            pitch_mod1: buffer(),
            pitch_mod2: buffer(),
            frame_mod: buffer(),
            amp_out: buffer(),
            fenv_out: buffer(),
            lfo_out: buffer(),
            out: ramp(),
            octaves: [0; 2],
            setup: Setup::new(0, true, 0),
            lfo_shape: SINE,
            sync_hz: 0.0,
            lfo_delay_frames: 0.0,
        }
    }

    /// Reads the stretch's parameters: the ramps into the buffers, and the
    /// switches, choices and envelopes into the fields.
    fn snapshot(&mut self, audio: &Audio<'_, '_>, params: &ParamBank) {
        let frames = audio.frames();
        let at = |id: ParamId| params.index(id).expect("the description holds it");
        let ramp = |to: &mut [f32], id: ParamId, scale: f32| {
            for (to, from) in to[..frames].iter_mut().zip(params.ramp(at(id))) {
                *to = from * scale;
            }
        };
        ramp(&mut self.level1, OSC1_LEVEL, 1.0);
        ramp(&mut self.level2, OSC2_LEVEL, 1.0);
        ramp(&mut self.detune, OSC2_DETUNE, 0.01);
        ramp(&mut self.pw, PW, 0.01);
        ramp(&mut self.freq, FREQ, 1.0);
        ramp(&mut self.res, RES, 0.01);
        ramp(&mut self.key, KEY, 0.01);
        ramp(&mut self.env_amount, ENV_AMOUNT, 0.01);
        ramp(&mut self.vel_vol, VEL_VOL, 0.01);
        ramp(&mut self.volume, VOLUME, 1.0);
        ramp(&mut self.lfo_hz, LFO_RATE, 1.0);
        ramp(&mut self.lfo_pitch, LFO_PITCH, 0.01);
        ramp(&mut self.lfo_pw, LFO_PW, 0.01);
        ramp(&mut self.lfo_filter, LFO_FILTER, 0.01);
        ramp(&mut self.wheel, WHEEL, 0.01);
        self.out[..frames].fill(0.0);

        let value = |id: ParamId| params.value(at(id));
        self.octaves = [value(OSC1_OCTAVE) as i32, value(OSC2_OCTAVE) as i32];
        for (n, id) in [OSC1_SHAPE, OSC2_SHAPE].into_iter().enumerate() {
            let shape = (value(id) as usize).min(SHAPES.len() - 1);
            if shape != self.shapes[n] {
                self.shapes[n] = shape;
                let table = self.tables[shape].clone();
                [&mut self.osc1, &mut self.osc2][n].set_table(table);
            }
        }
        for (osc, octave) in [
            (&mut self.osc1, self.octaves[0]),
            (&mut self.osc2, self.octaves[1]),
        ] {
            let p: &mut OscParams = osc.params_mut();
            p.octave = octave;
            p.phase_mode = PhaseMode::NoteStart;
            p.phase_random = 0.0;
            p.phase = 0.0;
            p.level = 100.0;
            p.reference = REFERENCE;
            p.frame = 1.0;
            p.smooth = true;
        }
        // Spec: bass and Poli R11. Low-pass or high-pass, 12 or 24 dB, in
        // Clean, MS2 or PRD.
        self.setup = Setup::new(
            value(FILTER_TYPE) as usize,
            value(SLOPE) >= 0.5,
            CIRCUIT_ENTRIES[(value(CIRCUIT) as usize).min(CIRCUIT_ENTRIES.len() - 1)],
        );
        analogue::set_envelope(
            &mut self.amp,
            &mut self.amp_params,
            analogue::adsr(
                value(ATTACK),
                value(DECAY),
                value(SUSTAIN) / 100.0,
                value(RELEASE),
            ),
        );
        analogue::set_envelope(
            &mut self.fenv,
            &mut self.fenv_params,
            analogue::adsr(
                value(FILTER_ATTACK),
                value(FILTER_DECAY),
                value(FILTER_SUSTAIN) / 100.0,
                value(FILTER_RELEASE),
            ),
        );
        // Spec: bass and Poli R13. The LFO's speed in hertz or by the
        // tempo's division, and its fade-in.
        self.lfo_shape = (value(LFO_SHAPE) as usize).min(LFO_SHAPES.len() - 1);
        self.sync_hz = if value(LFO_SYNC) >= 0.5 {
            let division = (value(LFO_DIVISION) as usize).min(LFO_DIVISIONS.len() - 1);
            let sixteenth = self.rate * 60.0 / audio.bpm() / 4.0;
            (self.rate / (DIVISION_SIXTEENTHS[division] * sixteenth).max(1.0)) as f32
        } else {
            0.0
        };
        self.lfo_delay_frames = value(LFO_DELAY) * 1e-3 * self.rate;
    }

    /// Drops everything the voices hold, for a device switched off.
    fn silence(&mut self) {
        for voice in 0..MAX_VOICES {
            self.amp.reset(voice);
            self.fenv.reset(voice);
        }
        self.out.fill(0.0);
    }
}

impl Instrument for Voice {
    fn voice_event(&mut self, voices: &mut VoiceBlock, _state: &State, event: VoiceEvent) {
        let voice = event.voice;
        match event.kind {
            VoiceEventKind::Start { start, .. } => {
                let start = analogue::effective(start, self.amp.is_idle(voice));
                let key = voices.pitch()[voice];
                let onset = analogue::onset(start);
                self.osc1.start(voice, key, onset);
                self.osc2.start(voice, key, onset);
                analogue::trigger(&mut self.amp, voice, start);
                analogue::trigger(&mut self.fenv, voice, start);
                // Spec: bass and Poli R13. The LFO restarts with each note.
                self.lfos.restart(voice);
                if matches!(start, lspk_voice::Start::Fresh) {
                    self.filtering.reset_voice(voice);
                }
            }
            VoiceEventKind::Release => {
                self.amp.release(voice);
                self.fenv.release(voice);
            }
            VoiceEventKind::End { .. } => {
                self.amp.reset(voice);
                self.fenv.reset(voice);
            }
        }
    }

    fn render(&mut self, voices: &mut VoiceBlock, state: &State, frames: Range<usize>) {
        let (from, to) = (frames.start, frames.end);
        let count = to - from;
        let span = voices.span(16);
        if count == 0 || span == 0 {
            return;
        }
        let stride = self.stride;

        // 1. The envelopes.
        self.amp.process(&mut self.amp_out[..count * stride]);
        self.fenv.process(&mut self.fenv_out[..count * stride]);

        // 2. The LFO of every sounding voice: its wave, faded in from the
        //    note's start and scaled by the mod wheel.
        let wheel = Sources::of(voices).mod_wheel() as f32;
        let mut mask = voices.active_mask();
        while mask != 0 {
            let voice = mask.trailing_zeros() as usize;
            mask &= mask - 1;
            for frame in 0..count {
                let at = from + frame;
                let hz = if self.sync_hz > 0.0 {
                    self.sync_hz
                } else {
                    self.lfo_hz[at]
                };
                let wave = self.lfos.tick(voice, hz / self.rate as f32, self.lfo_shape);
                let fade = if self.lfo_delay_frames > 0.0 {
                    (state.frames_since_start(voice, from + frame) as f64 / self.lfo_delay_frames)
                        .min(1.0) as f32
                } else {
                    1.0
                };
                let w = self.wheel[at];
                self.lfo_out[frame * stride + voice] = wave * fade * (1.0 - w + w * wheel);
            }
        }

        // 3. The oscillators, band-limited: the key's tuned offset, the
        //    second one's Detune and the LFO's Pitch ride on the pitch, the
        //    pulse's width, with the LFO's PW, on the frame.
        for frame in 0..count {
            let at = from + frame;
            for voice in 0..span {
                let i = frame * stride + voice;
                let lfo = self.lfo_out[i];
                let semitones =
                    state.tuned_by(voice) + lfo * self.lfo_pitch[at] * LFO_PITCH_SEMITONES;
                self.pitch_mod1[i] = semitones;
                self.pitch_mod2[i] = semitones + self.detune[at];
                let width =
                    (self.pw[at] + lfo * self.lfo_pw[at] * LFO_PW_SPAN).clamp(PW_MIN, PW_MAX);
                self.frame_mod[i] =
                    (width - PW_MIN) / (PW_MAX - PW_MIN) * (PULSE_FRAMES - 1) as f32;
            }
        }
        let inputs = |pitch_mod, pulse: bool| Inputs {
            lanes: span,
            pitch: voices.pitch(),
            pitch_step: Some(voices.pitch_step()),
            pitch_mod: Some(pitch_mod),
            frame_mod: pulse.then(|| &self.frame_mod[..count * stride]),
        };
        let (pulse1, pulse2) = (self.shapes[0] == PULSE, self.shapes[1] == PULSE);
        let first = inputs(&self.pitch_mod1[..count * stride], pulse1);
        self.osc1
            .process(count, &first, &mut self.osc1_out[..count * stride]);
        let second = inputs(&self.pitch_mod2[..count * stride], pulse2);
        self.osc2
            .process(count, &second, &mut self.osc2_out[..count * stride]);

        // 4. The oscillators summed, and the filter's cutoff: the knob, the
        //    key, the envelope and the LFO.
        self.filtering.begin(count);
        let mut mask = voices.active_mask();
        while mask != 0 {
            let voice = mask.trailing_zeros() as usize;
            mask &= mask - 1;
            for frame in 0..count {
                let at = from + frame;
                let i = frame * stride + voice;
                self.filtering.put(
                    frame,
                    voice,
                    self.osc1_out[i] * self.level1[at] + self.osc2_out[i] * self.level2[at],
                );
                let note = voices.pitch()[voice]
                    + voices.pitch_step()[voice] * frame as f32
                    + state.tuned_by(voice)
                    - MIDDLE_C;
                self.filtering.set_cutoff(
                    frame,
                    voice,
                    pitch_of(self.freq[at], REFERENCE)
                        + self.key[at] * note
                        + self.fenv_out[i] * self.env_amount[at] * ENV_SEMITONES
                        + self.lfo_out[i] * self.lfo_filter[at] * LFO_FILTER_SEMITONES,
                );
            }
        }

        // 5. The filter.
        self.filtering
            .run(self.setup, count, &self.res[from..to], None);

        // 6. The amplifier, Vel > Vol and Volume, the voices mixed.
        let mut mask = voices.active_mask();
        while mask != 0 {
            let voice = mask.trailing_zeros() as usize;
            mask &= mask - 1;
            let velocity = voices.velocity()[voice];
            for frame in 0..count {
                let at = from + frame;
                let level = self.amp_out[frame * stride + voice];
                if level == 0.0 {
                    continue;
                }
                let vel_vol = self.vel_vol[at];
                let scale = 1.0 - vel_vol + vel_vol * velocity;
                self.out[at] += self.filtering.get(frame, voice) * level * scale * self.volume[at];
            }
        }

        // 7. A voice whose amplifier envelope has ended is silent.
        let mut mask = voices.active_mask();
        while mask != 0 {
            let voice = mask.trailing_zeros() as usize;
            mask &= mask - 1;
            if self.amp.is_idle(voice) {
                voices.end(voice);
            }
        }
    }
}

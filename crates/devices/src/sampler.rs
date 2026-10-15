//! Snapshot, the native sampler (WP-2.16; the sampler spec): Classic,
//! One-Shot and Slicing playback of one sample — the region, the loop and
//! its constant-power crossfade, Transpose, Detune, Glide and Portamento,
//! the amplitude envelope, velocity and pan — with Slicing's slices, a
//! per-voice filter with key tracking and an envelope, and a per-voice LFO
//! with a tempo-synced rate.
//!
//! The sample is decoded audio the media crate gives the engine, sent to
//! this node as node data (the sampler spec, R11): the compiler opens it
//! when the node is made, and the sampler reads it at the positions its
//! voices need, so nothing here reads a file or allocates.

use std::sync::Arc;

use bl_core::time::{Frames, SampleRate, TempoMap};
use bl_engine::{DeviceType, NodeConfig, NodeData, NoteEvent, NoteKind, Registry};
use bl_media::SampleData;
use bl_model::device_layout::{BODY_HEIGHT, ControlBand, DisplayCell, LayoutNode, Widget};
use bl_model::{DeviceDescription, DeviceLayout, DeviceRole, ParamDescriptor, ParamId, ParamUnit};
use lspk_dsp::filter::{FilterBank, FilterType, VoiceParams, pitch_of};

use crate::framework::{Audio, NativeDevice, ParamBank, add};

/// The sampler device type's id.
pub const SAMPLER: &str = "beatlayer.snapshot";

/// Classic (polyphonic, looping), One-Shot (monophonic, straight through)
/// or Slice (monophonic, one note per slice).
pub const MODE: ParamId = ParamId(0);
/// Where the played region starts, as a share of the sample.
pub const START: ParamId = ParamId(1);
/// How long the played region is, as a share of the sample.
pub const LENGTH: ParamId = ParamId(2);
/// Whether the played region's end loops into its start.
pub const LOOP: ParamId = ParamId(3);
/// How much of the played region loops, counted back from its end.
pub const LOOP_LENGTH: ParamId = ParamId(4);
/// The constant-power crossfade at the loop's wrap, as a share of the
/// loop.
pub const FADE: ParamId = ParamId(5);
/// The sample's own gain stage.
pub const GAIN: ParamId = ParamId(6);
/// Transposes the sample, in whole semitones.
pub const TRANSPOSE: ParamId = ParamId(7);
/// Fine-tunes the sample, in cents.
pub const DETUNE: ParamId = ParamId(8);
/// How many voices Classic plays at once.
pub const VOICES: ParamId = ParamId(9);
/// Whether a note already sounding on the same key is cut off by a new
/// one.
pub const RETRIG: ParamId = ParamId(10);
/// The amplitude envelope's attack time.
pub const ATTACK: ParamId = ParamId(11);
/// The amplitude envelope's decay time.
pub const DECAY: ParamId = ParamId(12);
/// The amplitude envelope's sustain level.
pub const SUSTAIN: ParamId = ParamId(13);
/// The amplitude envelope's release time.
pub const RELEASE: ParamId = ParamId(14);
/// The output level.
pub const VOLUME: ParamId = ParamId(15);
/// Where the voice sits in the stereo field.
pub const PAN: ParamId = ParamId(16);
/// How much note velocity scales the volume.
pub const VELOCITY: ParamId = ParamId(17);
/// Whether a new note slides from the last note's pitch.
pub const GLIDE: ParamId = ParamId(18);
/// Glide (monophonic) or Portamento (polyphonic).
pub const GLIDE_MODE: ParamId = ParamId(19);
/// How long a glide takes.
pub const GLIDE_TIME: ParamId = ParamId(20);
/// One-Shot: whether a note's end is ignored (on) or fades the voice out.
pub const TRIGGER: ParamId = ParamId(21);
/// One-Shot: the fade in at the region's start.
pub const FADE_IN: ParamId = ParamId(22);
/// One-Shot: the fade out ending at the region's end, and a note's end's
/// fade with Trigger off.
pub const FADE_OUT: ParamId = ParamId(23);
/// Slicing: how the slices are placed.
pub const SLICE_BY: ParamId = ParamId(24);
/// Slicing: how sensitive transient detection is; higher finds more
/// slices.
pub const SLICE_SENSITIVITY: ParamId = ParamId(25);
/// Slicing: the beat division, in sixteenth notes.
pub const SLICE_DIVISION: ParamId = ParamId(26);
/// Slicing: how many equal slices Region slicing makes.
pub const SLICE_REGIONS: ParamId = ParamId(27);
/// Whether the sample runs through the filter.
pub const FILTER: ParamId = ParamId(28);
/// The filter's type: low-pass, high-pass, band-pass or notch, at 12 or
/// 24 dB per octave.
pub const FILTER_TYPE: ParamId = ParamId(29);
/// The filter's cutoff frequency.
pub const FILTER_FREQ: ParamId = ParamId(30);
/// The filter's resonance.
pub const FILTER_RES: ParamId = ParamId(31);
/// How much the played note moves the cutoff.
pub const FILTER_KEY: ParamId = ParamId(32);
/// The filter envelope's attack time.
pub const FILTER_ENV_ATTACK: ParamId = ParamId(33);
/// The filter envelope's decay time.
pub const FILTER_ENV_DECAY: ParamId = ParamId(34);
/// The filter envelope's sustain level.
pub const FILTER_ENV_SUSTAIN: ParamId = ParamId(35);
/// The filter envelope's release time.
pub const FILTER_ENV_RELEASE: ParamId = ParamId(36);
/// How far the filter envelope moves the cutoff.
pub const FILTER_ENV_AMOUNT: ParamId = ParamId(37);
/// Whether the LFO runs.
pub const LFO: ParamId = ParamId(38);
/// The LFO's waveform.
pub const LFO_WAVE: ParamId = ParamId(39);
/// The LFO's free-running rate, in hertz.
pub const LFO_RATE: ParamId = ParamId(40);
/// Whether the LFO's rate is a beat division of the set's tempo.
pub const LFO_SYNC: ParamId = ParamId(41);
/// The LFO's synced rate, in sixteenth notes.
pub const LFO_DIVISION: ParamId = ParamId(42);
/// How long the LFO takes to reach full intensity after a note starts.
pub const LFO_ATTACK: ParamId = ParamId(43);
/// Where a retriggered voice's LFO starts in its cycle.
pub const LFO_OFFSET: ParamId = ParamId(44);
/// Whether a note restarts its voice's LFO at the offset.
pub const LFO_RETRIGGER: ParamId = ParamId(45);
/// How much the played note speeds the LFO up.
pub const LFO_KEY: ParamId = ParamId(46);
/// How far the LFO moves the voice's level.
pub const LFO_VOLUME: ParamId = ParamId(47);
/// How far the LFO bends the voice's pitch.
pub const LFO_PITCH: ParamId = ParamId(48);
/// How far the LFO sways the voice's pan.
pub const LFO_PAN: ParamId = ParamId(49);
/// How far the LFO moves the filter's cutoff.
pub const LFO_FILTER: ParamId = ParamId(50);

/// The Mode choices: Classic, One-Shot, then Slice.
pub const MODES: [&str; 3] = ["Classic", "One-Shot", "Slice"];

/// The Slice By choices.
pub const SLICE_BYS: [&str; 4] = ["Transient", "Beat", "Region", "Manual"];

/// The filter's Type choices: each family at its two slopes.
pub const FILTER_TYPES: [&str; 8] = [
    "Low-pass 12",
    "Low-pass 24",
    "High-pass 12",
    "High-pass 24",
    "Band-pass 12",
    "Band-pass 24",
    "Notch 12",
    "Notch 24",
];

/// The LFO's Wave choices.
pub const LFO_WAVES: [&str; 6] = ["Sine", "Square", "Triangle", "Saw Down", "Saw Up", "Random"];

/// The Glide Mode choices: Glide (monophonic) then Portamento
/// (polyphonic).
pub const GLIDE_MODES: [&str; 2] = ["Glide", "Portamento"];

/// How many voices Classic plays at once at most (the Voices parameter's
/// top).
pub const MAX_VOICES: usize = 16;

/// The most slices Slicing mode makes (the manual's maximum).
pub const MAX_SLICES: usize = 64;

/// The slots a voice can sound in: [`MAX_VOICES`] sounding voices and room
/// for the ones being stolen while they fade out. A note that finds every
/// slot busy takes the oldest fading one, which is nearly silent, so a
/// fade is cut short rather than a sounding voice.
const SLOTS: usize = 24;

/// How long a stolen or choked voice fades out, in seconds (Beatlayer's
/// own; the manual only says stealing is made subtle).
const STEAL_FADE_SECONDS: f64 = 0.005;

/// The envelope's longest stage, in seconds: the top of the Attack, Decay
/// and Release ranges.
const ENVELOPE_MAX_SECONDS: f64 = 20.0;

/// The one-shot fades' longest, seconds; the LFO's Attack shares it.
const FADE_MAX_SECONDS: f64 = 2.0;

/// The longest glide, in seconds: the top of its range.
const GLIDE_MAX_SECONDS: f64 = 1.0;

/// The note slice 1 plays: C3, the sampler's reference pitch (Beatlayer's
/// own; the manual says the slices play chromatically but not from which
/// note).
const SLICE_BASE: u8 = 60;

/// How long transient detection's peak-hold envelope falls between the
/// sample's peaks, in milliseconds: long enough to bridge the gaps between
/// a waveform's peaks, short enough to fall between a loop's hits
/// (Beatlayer's own detector).
const TRANSIENT_FALL_MS: f64 = 20.0;

/// The range transient detection's threshold falls over at full
/// Sensitivity, in decibels below the loudest window: at 100 % every
/// window within 40 dB of the loudest is an onset, at 0 % only the loudest
/// is.
const TRANSIENT_RANGE_DB: f64 = 40.0;

/// How far the filter envelope moves the cutoff at full amount, in
/// octaves each way (Beatlayer's own).
const FILTER_ENV_OCTAVES: f64 = 4.0;

/// How far the LFO moves the cutoff at full amount, in octaves each way
/// (Beatlayer's own).
const LFO_FILTER_OCTAVES: f64 = 4.0;

/// How far the LFO bends the pitch at full amount, in semitones each way
/// (Beatlayer's own).
const LFO_PITCH_SEMITONES: f64 = 12.0;

/// The LFO's free rate's range, in hertz (the manual's).
const LFO_MIN_HZ: f64 = 0.01;
const LFO_MAX_HZ: f64 = 30.0;

/// The kit's filters measure pitch with A4 at this frequency.
const REFERENCE_HZ: f32 = 440.0;

/// The Snapshot device type.
#[must_use]
pub fn sampler_type() -> DeviceType {
    let time = |id, name: &str, default: f64| {
        ParamDescriptor::continuous(
            id,
            name,
            (0.0, ENVELOPE_MAX_SECONDS * 1000.0),
            default,
            ParamUnit::Milliseconds,
        )
    };
    let params = vec![
        ParamDescriptor::choice(MODE, "Mode", &MODES, 0).describe(
            "Classic plays the region polyphonically with the loop and the envelope; \
                       One-Shot plays it monophonically straight through, whatever the note \
                       does; Slice plays one slice per note up the keyboard.",
        ),
        ParamDescriptor::continuous(START, "Start", (0.0, 100.0), 0.0, ParamUnit::Percent)
            .describe("Where in the sample the played region starts, as a share of it."),
        ParamDescriptor::continuous(LENGTH, "Length", (0.0, 100.0), 100.0, ParamUnit::Percent)
            .describe("How long the played region is, as a share of the sample from Start."),
        ParamDescriptor::toggle(LOOP, "Loop", false)
            .describe("Loops the region's end into its start for as long as a voice sounds."),
        ParamDescriptor::continuous(
            LOOP_LENGTH,
            "Loop Length",
            (0.0, 100.0),
            100.0,
            ParamUnit::Percent,
        )
        .describe(
            "How much of the played region the loop repeats, counted back from the \
                   region's end; the whole region at 100 %.",
        ),
        ParamDescriptor::continuous(FADE, "Fade", (0.0, 100.0), 0.0, ParamUnit::Percent).describe(
            "Crossfades the loop's end into its start over this share of the loop, so \
                       the wrap doesn't click.",
        ),
        ParamDescriptor::gain(GAIN, "Gain", 24.0, 0.0).describe(
            "The sample's own gain stage, before the filter and the volume; reaches \
                       silence at its bottom.",
        ),
        ParamDescriptor::integer(TRANSPOSE, "Transpose", (-48, 48), 0, ParamUnit::Semitones)
            .describe(
                "Transposes the sample in whole semitones; C3 plays it at its own \
                       pitch.",
            ),
        ParamDescriptor::continuous(DETUNE, "Detune", (-50.0, 50.0), 0.0, ParamUnit::Cents)
            .describe("Fine-tunes the sample in cents."),
        ParamDescriptor::integer(VOICES, "Voices", (1, MAX_VOICES as i64), 8, ParamUnit::None)
            .describe(
                "How many voices Classic plays at once; past it the oldest voice is faded \
                       out in favor of the new note.",
            ),
        ParamDescriptor::toggle(RETRIG, "Retrig", false).describe(
            "Cuts a note already sounding on the same key off when it plays again, \
                       instead of letting the two overlap.",
        ),
        time(ATTACK, "Attack", 0.0).describe(
            "The time the amplitude envelope takes to reach its peak after a note \
                   starts.",
        ),
        time(DECAY, "Decay", 0.0)
            .describe("The time the amplitude envelope takes to fall to the sustain level."),
        ParamDescriptor::continuous(SUSTAIN, "Sustain", (0.0, 100.0), 100.0, ParamUnit::Percent)
            .describe("The level the amplitude envelope holds while the note sounds."),
        time(RELEASE, "Release", 100.0).describe(
            "The time the amplitude envelope takes to fall from the sustain level to \
                   silence after the note ends.",
        ),
        ParamDescriptor::gain(VOLUME, "Volume", 24.0, 0.0)
            .describe("The instrument's output level; reaches silence at its bottom."),
        ParamDescriptor::continuous(PAN, "Pan", (-50.0, 50.0), 0.0, ParamUnit::None)
            .decimals(0)
            .describe("Where the voice sits in the stereo field."),
        ParamDescriptor::continuous(VELOCITY, "Velocity", (0.0, 100.0), 0.0, ParamUnit::Percent)
            .describe(
                "How much a note's velocity scales its volume: at 0 it doesn't, at 100 % \
                       the level follows it.",
            ),
        ParamDescriptor::toggle(GLIDE, "Glide", false).describe(
            "Starts a new note from the last note's pitch and slides it to its \
                       own.",
        ),
        ParamDescriptor::choice(GLIDE_MODE, "Glide Mode", &GLIDE_MODES, 0).describe(
            "Glide takes the sounding voice over, monophonically; Portamento gives \
                       each note a voice of its own.",
        ),
        ParamDescriptor::continuous(
            GLIDE_TIME,
            "Glide Time",
            (0.0, GLIDE_MAX_SECONDS * 1000.0),
            100.0,
            ParamUnit::Milliseconds,
        )
        .describe("How long a glide from one pitch to the next takes."),
        ParamDescriptor::toggle(TRIGGER, "Trigger", true).describe(
            "One-Shot and Slice: plays the region or the slice to its end after the \
                       note is released; off, the note's end fades the voice out.",
        ),
        ParamDescriptor::continuous(
            FADE_IN,
            "Fade In",
            (0.0, FADE_MAX_SECONDS * 1000.0),
            0.0,
            ParamUnit::Milliseconds,
        )
        .describe(
            "One-Shot and Slice: how long the region or the slice takes to reach its \
                   level after a note starts.",
        ),
        ParamDescriptor::continuous(
            FADE_OUT,
            "Fade Out",
            (0.0, FADE_MAX_SECONDS * 1000.0),
            0.0,
            ParamUnit::Milliseconds,
        )
        .describe(
            "One-Shot and Slice: fades the region or the slice out over this time, \
                   ending at its end, and with Trigger off, fades the voice out when the \
                   note ends.",
        ),
        ParamDescriptor::choice(SLICE_BY, "Slice By", &SLICE_BYS, 0).describe(
            "How Slicing's slices are placed: on the sample's transients, at beat \
                       divisions, at equal divisions, or by hand — Manual places no slices \
                       automatically.",
        ),
        ParamDescriptor::continuous(
            SLICE_SENSITIVITY,
            "Sensitivity",
            (0.0, 100.0),
            50.0,
            ParamUnit::Percent,
        )
        .describe("Transient slicing's threshold: higher finds more slices, up to 64."),
        ParamDescriptor::integer(SLICE_DIVISION, "Division", (1, 16), 4, ParamUnit::None)
            .decimals(0)
            .describe("Beat slicing's division, in sixteenth notes at the set's tempo."),
        ParamDescriptor::integer(
            SLICE_REGIONS,
            "Regions",
            (1, MAX_SLICES as i64),
            16,
            ParamUnit::None,
        )
        .decimals(0)
        .describe("Region slicing's number of equal slices."),
        ParamDescriptor::toggle(FILTER, "Filter", false)
            .describe("Runs the sample through the filter; off bypasses it."),
        ParamDescriptor::choice(FILTER_TYPE, "Type", &FILTER_TYPES, 0).describe(
            "The filter's low-pass, high-pass, band-pass or notch response, at 12 or \
                       24 dB per octave.",
        ),
        ParamDescriptor::continuous(
            FILTER_FREQ,
            "Freq",
            (20.0, 20_000.0),
            1_000.0,
            ParamUnit::Hertz,
        )
        .logarithmic()
        .describe("The filter's cutoff frequency."),
        ParamDescriptor::continuous(FILTER_RES, "Res", (0.0, 100.0), 0.0, ParamUnit::Percent)
            .describe("Raises the filter's resonance at the cutoff."),
        ParamDescriptor::continuous(FILTER_KEY, "Key", (0.0, 100.0), 0.0, ParamUnit::Percent)
            .describe(
                "How much the played note moves the cutoff: an octave per octave at \
                       100 %, C3 centered.",
            ),
        time(FILTER_ENV_ATTACK, "Filter Attack", 0.0)
            .describe("How long the filter envelope takes to reach its peak."),
        time(FILTER_ENV_DECAY, "Filter Decay", 0.0)
            .describe("How long the filter envelope takes to fall to its sustain level."),
        ParamDescriptor::continuous(
            FILTER_ENV_SUSTAIN,
            "Filter Sustain",
            (0.0, 100.0),
            100.0,
            ParamUnit::Percent,
        )
        .describe("The level the filter envelope holds while the note sounds."),
        time(FILTER_ENV_RELEASE, "Filter Release", 100.0)
            .describe("How long the filter envelope takes to fall after the note ends."),
        ParamDescriptor::continuous(
            FILTER_ENV_AMOUNT,
            "Filter Env",
            (-100.0, 100.0),
            0.0,
            ParamUnit::Percent,
        )
        .describe("How far the filter envelope moves the cutoff, up to four octaves each way."),
        ParamDescriptor::toggle(LFO, "LFO", false).describe("Turns the LFO on."),
        ParamDescriptor::choice(LFO_WAVE, "LFO Wave", &LFO_WAVES, 0)
            .describe("The LFO's waveform; Random holds a new value each cycle."),
        ParamDescriptor::continuous(
            LFO_RATE,
            "LFO Rate",
            (LFO_MIN_HZ, LFO_MAX_HZ),
            1.0,
            ParamUnit::Hertz,
        )
        .logarithmic()
        .describe("The LFO's rate in hertz; Sync replaces it with a beat division."),
        ParamDescriptor::toggle(LFO_SYNC, "LFO Sync", false)
            .describe("Sets the LFO's rate in beat divisions of the set's tempo."),
        ParamDescriptor::integer(LFO_DIVISION, "LFO Division", (1, 16), 4, ParamUnit::None)
            .decimals(0)
            .describe("The LFO's rate while synced, in sixteenth notes."),
        ParamDescriptor::continuous(
            LFO_ATTACK,
            "LFO Attack",
            (0.0, FADE_MAX_SECONDS * 1000.0),
            0.0,
            ParamUnit::Milliseconds,
        )
        .describe("How long the LFO takes to reach full intensity after a note starts."),
        ParamDescriptor::continuous(
            LFO_OFFSET,
            "LFO Offset",
            (0.0, 100.0),
            0.0,
            ParamUnit::Percent,
        )
        .describe("Where a retriggered voice's LFO starts in its cycle."),
        ParamDescriptor::toggle(LFO_RETRIGGER, "LFO Retrigger", false).describe(
            "Restarts each voice's LFO at the offset on its note; off, a voice starts at \
                       the free-running phase.",
        ),
        ParamDescriptor::continuous(LFO_KEY, "LFO Key", (0.0, 100.0), 0.0, ParamUnit::Percent)
            .describe(
                "How much the played note speeds its LFO up: at 100 % an octave above C3 \
                       runs twice as fast.",
            ),
        ParamDescriptor::continuous(
            LFO_VOLUME,
            "LFO Volume",
            (-100.0, 100.0),
            0.0,
            ParamUnit::Percent,
        )
        .describe("How far the LFO moves the voice's level."),
        ParamDescriptor::continuous(
            LFO_PITCH,
            "LFO Pitch",
            (-100.0, 100.0),
            0.0,
            ParamUnit::Percent,
        )
        .describe("How far the LFO bends the voice's pitch, up to twelve semitones each way."),
        ParamDescriptor::continuous(LFO_PAN, "LFO Pan", (-100.0, 100.0), 0.0, ParamUnit::Percent)
            .describe("How far the LFO sways the voice's pan."),
        ParamDescriptor::continuous(
            LFO_FILTER,
            "LFO Filter",
            (-100.0, 100.0),
            0.0,
            ParamUnit::Percent,
        )
        .describe("How far the LFO moves the filter's cutoff, up to four octaves each way."),
    ];
    DeviceType::native(SAMPLER, "Snapshot", DeviceRole::Instrument)
        .with_description(DeviceDescription::new(params).with_layout(layout()))
}

/// A layout of fixed control bands and the sample's display, which grows.
fn layout() -> DeviceLayout {
    let mode = ControlBand::stacked(
        BODY_HEIGHT,
        &[
            (MODE, Widget::Chooser),
            (LOOP, Widget::Toggle),
            (RETRIG, Widget::Toggle),
            (GLIDE, Widget::Toggle),
            (TRIGGER, Widget::Toggle),
        ],
    );
    let slice = ControlBand::stacked(
        BODY_HEIGHT,
        &[
            (SLICE_BY, Widget::Chooser),
            (SLICE_SENSITIVITY, Widget::Knob),
            (SLICE_DIVISION, Widget::Number),
            (SLICE_REGIONS, Widget::Number),
        ],
    );
    let sample = ControlBand::stacked(
        BODY_HEIGHT,
        &[
            (START, Widget::Knob),
            (LENGTH, Widget::Knob),
            (LOOP_LENGTH, Widget::Knob),
            (FADE, Widget::Knob),
            (GAIN, Widget::Knob),
        ],
    );
    let pitch = ControlBand::stacked(
        BODY_HEIGHT,
        &[
            (TRANSPOSE, Widget::Number),
            (DETUNE, Widget::Knob),
            (GLIDE_MODE, Widget::Chooser),
            (GLIDE_TIME, Widget::Knob),
            (VOICES, Widget::Number),
        ],
    );
    let amp = ControlBand::stacked(
        BODY_HEIGHT,
        &[
            (ATTACK, Widget::Knob),
            (DECAY, Widget::Knob),
            (SUSTAIN, Widget::Knob),
            (RELEASE, Widget::Knob),
        ],
    );
    let filter = ControlBand::stacked(
        BODY_HEIGHT,
        &[
            (FILTER, Widget::Toggle),
            (FILTER_TYPE, Widget::Chooser),
            (FILTER_FREQ, Widget::Knob),
            (FILTER_RES, Widget::Knob),
            (FILTER_KEY, Widget::Knob),
            (FILTER_ENV_ATTACK, Widget::Knob),
            (FILTER_ENV_DECAY, Widget::Knob),
            (FILTER_ENV_SUSTAIN, Widget::Knob),
            (FILTER_ENV_RELEASE, Widget::Knob),
            (FILTER_ENV_AMOUNT, Widget::Knob),
        ],
    );
    let lfo = ControlBand::stacked(
        BODY_HEIGHT,
        &[
            (LFO, Widget::Toggle),
            (LFO_WAVE, Widget::Chooser),
            (LFO_RATE, Widget::Knob),
            (LFO_SYNC, Widget::Toggle),
            (LFO_DIVISION, Widget::Number),
            (LFO_ATTACK, Widget::Knob),
            (LFO_OFFSET, Widget::Knob),
            (LFO_RETRIGGER, Widget::Toggle),
            (LFO_KEY, Widget::Knob),
            (LFO_VOLUME, Widget::Knob),
            (LFO_PITCH, Widget::Knob),
            (LFO_PAN, Widget::Knob),
            (LFO_FILTER, Widget::Knob),
        ],
    );
    let level = ControlBand::stacked(
        BODY_HEIGHT,
        &[
            (VOLUME, Widget::Knob),
            (PAN, Widget::Knob),
            (VELOCITY, Widget::Knob),
            (FADE_IN, Widget::Knob),
            (FADE_OUT, Widget::Knob),
        ],
    );
    // The sample's waveform has no drawing yet (WP-2.16): the display cell
    // is left for it.
    let display = DisplayCell::new("Sample", 160.0, BODY_HEIGHT, 1.0);
    DeviceLayout::new(LayoutNode::Row(vec![
        LayoutNode::Band(mode),
        LayoutNode::Band(slice),
        LayoutNode::Band(sample),
        LayoutNode::Cell(display),
        LayoutNode::Band(pitch),
        LayoutNode::Band(amp),
        LayoutNode::Band(filter),
        LayoutNode::Band(lfo),
        LayoutNode::Band(level),
    ]))
}

/// Adds the sampler to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, sampler_type(), |_, config| Sampler::new(config));
}

/// Which playback mode a device is in.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Polyphonic, with the loop and the envelope.
    Classic,
    /// Monophonic, straight through the region.
    OneShot,
    /// Monophonic, one slice per note.
    Slice,
}

/// How Slicing places its slices.
#[derive(Clone, Copy, PartialEq, Eq)]
enum SliceBy {
    /// On the sample's transients.
    Transient,
    /// At beat divisions of the set's tempo.
    Beat,
    /// At equal divisions of the region.
    Region,
    /// By hand: no automatic slices.
    Manual,
}

impl SliceBy {
    /// The choice the parameter's value names.
    fn of(value: f64) -> Self {
        match value as usize {
            1 => Self::Beat,
            2 => Self::Region,
            3 => Self::Manual,
            _ => Self::Transient,
        }
    }
}

/// The sampler's state between blocks: the sample and its voices.
pub struct Sampler {
    /// The decoded sample: the media crate's audio, at the engine's rate.
    data: Option<NodeData>,
    voices: [Voice; SLOTS],
    /// How many notes have been played: voices are stolen oldest first.
    counter: u64,
    /// The pitch of the last note played, in semitones from C3: where a
    /// gliding voice starts.
    last_pitch: Option<f64>,
    /// The filter, one lane per voice slot.
    filter: FilterBank,
    /// How many values a frame of the per-voice buffers holds.
    stride: usize,
    /// Each voice's sample audio for the stretch, before the filter: per
    /// frame, then per channel, one value per slot.
    audio: Vec<f32>,
    /// Each voice's cutoff for the stretch, in the filter's semitones:
    /// per frame, one value per slot.
    cutoff: Vec<f32>,
    /// Each voice's LFO value for the stretch: per frame, one value per
    /// slot.
    lfo: Vec<f32>,
    /// Slicing's slices, worked out when their inputs change.
    slices: Slices,
    /// The phase a voice's LFO starts from when Retrigger is off.
    free_phase: f64,
    /// The filter settings last handed to the bank.
    filter_params: VoiceParams,
    /// The Type choice the bank's filter type holds.
    filter_type: usize,
    /// Per-slot random state, for the LFO's Random waveform.
    rng: [u32; SLOTS],
}

impl Sampler {
    /// Makes a sampler for `config`'s rate and block size: the filter and
    /// every buffer it needs are made here, off the audio thread.
    fn new(config: &NodeConfig) -> Self {
        let mut filter = FilterBank::new(config.sample_rate.hz() as f32, SLOTS, 2);
        filter.set_type(filter_type_of(0));
        let stride = filter.stride();
        let frames = config.max_frames.max(1);
        Self {
            data: None,
            voices: [Voice::idle(); SLOTS],
            counter: 0,
            last_pitch: None,
            filter,
            stride,
            audio: vec![0.0; frames * 2 * stride],
            cutoff: vec![0.0; frames * stride],
            lfo: vec![0.0; frames * stride],
            slices: Slices::new(),
            free_phase: 0.0,
            filter_params: VoiceParams::default(),
            filter_type: 0,
            rng: core::array::from_fn(|slot| 0x9E37_79B9u32.wrapping_mul(slot as u32 + 1) | 1),
        }
    }

    /// The sample, if the media library gave one.
    fn sample(&self) -> Option<&SampleData> {
        self.data
            .as_ref()
            .and_then(|data| data.downcast_ref::<Arc<SampleData>>())
            .map(Arc::as_ref)
    }

    /// The settings one stretch of a block plays by.
    fn settings(
        &self,
        params: &ParamBank,
        rate: SampleRate,
        tempo: &TempoMap,
        position: Frames,
    ) -> Settings {
        let at = |id: ParamId| params.index(id).expect("the description holds it");
        let mode = match params.value(at(MODE)) as usize {
            1 => Mode::OneShot,
            2 => Mode::Slice,
            _ => Mode::Classic,
        };
        let sample_frames = self.sample().map_or(0, |sample| sample.frames() as u64);
        let start = params.value(at(START)) / 100.0;
        let length = params.value(at(LENGTH)) / 100.0;
        let region_start = (start * sample_frames as f64) as u64;
        let region_end = ((start + length).min(1.0) * sample_frames as f64) as u64;
        let region_end = region_end.min(sample_frames);
        let loop_share = params.value(at(LOOP_LENGTH)) / 100.0;
        let loop_start =
            region_end - ((loop_share * (region_end.saturating_sub(region_start)) as f64) as u64);
        let fade = (params.value(at(FADE)) / 100.0 * (region_end - loop_start) as f64) as u32;
        // One-Shot and Slice have Fade In and Fade Out instead of the
        // envelope: their attack is Fade In and they hold at the peak.
        let (attack_ms, decay_ms, sustain) = if mode == Mode::Classic {
            (
                params.value(at(ATTACK)),
                params.value(at(DECAY)),
                params.value(at(SUSTAIN)) / 100.0,
            )
        } else {
            (params.value(at(FADE_IN)), 0.0, 1.0)
        };
        // Frames in a sixteenth note at the stretch's tempo (the delay's
        // law): Slicing's Beat divisions and the LFO's synced rate read
        // it.
        let sixteenth =
            rate.as_f64() * 60.0 / tempo.bpm_at(tempo.frames_to_ticks(position, rate)).get() / 4.0;
        let lfo_cycle_frames = if params.value(at(LFO_SYNC)) >= 0.5 {
            params.value(at(LFO_DIVISION)).max(1.0) * sixteenth
        } else {
            rate.as_f64() / params.value(at(LFO_RATE)).clamp(LFO_MIN_HZ, LFO_MAX_HZ)
        };
        Settings {
            sample_rate: rate.as_f64(),
            mode,
            region_start,
            region_end,
            looping: mode == Mode::Classic && params.value(at(LOOP)) >= 0.5,
            loop_start,
            loop_end: region_end,
            fade,
            transpose: params.value(at(TRANSPOSE)),
            detune: params.value(at(DETUNE)),
            voices: params.value(at(VOICES)).max(1.0) as usize,
            retrig: params.value(at(RETRIG)) >= 0.5,
            glide: params.value(at(GLIDE)) >= 0.5,
            glide_portamento: params.value(at(GLIDE_MODE)) >= 0.5,
            glide_frames: frames_of(params.value(at(GLIDE_TIME)), rate.as_f64()),
            velocity: (params.value(at(VELOCITY)) / 100.0) as f32,
            attack_frames: frames_of(attack_ms, rate.as_f64()),
            decay_frames: frames_of(decay_ms, rate.as_f64()),
            sustain: sustain as f32,
            release_frames: frames_of(params.value(at(RELEASE)), rate.as_f64()),
            steal_frames: frames_of(STEAL_FADE_SECONDS * 1000.0, rate.as_f64()).max(1),
            trigger: params.value(at(TRIGGER)) >= 0.5,
            fade_out_frames: frames_of(params.value(at(FADE_OUT)), rate.as_f64()),
            slice_by: SliceBy::of(params.value(at(SLICE_BY))),
            sensitivity: params.value(at(SLICE_SENSITIVITY)) / 100.0,
            division_frames: params.value(at(SLICE_DIVISION)).max(1.0) * sixteenth,
            regions: params.value(at(SLICE_REGIONS)).max(1.0) as usize,
            filter_on: params.value(at(FILTER)) >= 0.5,
            filter_type: (params.value(at(FILTER_TYPE)) as usize).min(FILTER_TYPES.len() - 1),
            filter_freq: params.value(at(FILTER_FREQ)),
            filter_res: (params.value(at(FILTER_RES)) / 100.0) as f32,
            filter_key: (params.value(at(FILTER_KEY)) / 100.0) as f32,
            filter_attack_frames: frames_of(params.value(at(FILTER_ENV_ATTACK)), rate.as_f64()),
            filter_decay_frames: frames_of(params.value(at(FILTER_ENV_DECAY)), rate.as_f64()),
            filter_sustain: (params.value(at(FILTER_ENV_SUSTAIN)) / 100.0) as f32,
            filter_release_frames: frames_of(params.value(at(FILTER_ENV_RELEASE)), rate.as_f64()),
            filter_amount: (params.value(at(FILTER_ENV_AMOUNT)) / 100.0) as f32,
            lfo_on: params.value(at(LFO)) >= 0.5,
            lfo_wave: (params.value(at(LFO_WAVE)) as usize).min(LFO_WAVES.len() - 1),
            lfo_cycle_frames: lfo_cycle_frames.max(1.0),
            lfo_attack_frames: frames_of(params.value(at(LFO_ATTACK)), rate.as_f64()),
            lfo_offset: params.value(at(LFO_OFFSET)) / 100.0,
            lfo_retrigger: params.value(at(LFO_RETRIGGER)) >= 0.5,
            lfo_key: (params.value(at(LFO_KEY)) / 100.0) as f32,
            lfo_volume: (params.value(at(LFO_VOLUME)) / 100.0) as f32,
            lfo_pitch: (params.value(at(LFO_PITCH)) / 100.0) as f32,
            lfo_pan: (params.value(at(LFO_PAN)) / 100.0) as f32,
            lfo_filter: (params.value(at(LFO_FILTER)) / 100.0) as f32,
        }
    }

    /// Takes a note: starts, releases or chokes voices by the mode's rules.
    fn play(&mut self, note: NoteEvent, settings: &Settings) {
        match note.kind {
            NoteKind::On => self.note_on(note, settings),
            NoteKind::Off => self.note_off(note.key, settings),
            NoteKind::Choke => self.choke(note.key, settings),
        }
    }

    fn note_on(&mut self, note: NoteEvent, s: &Settings) {
        // Slicing plays the slice the note names; Classic and One-Shot the
        // whole region.
        let (region_start, region_end) = if s.mode == Mode::Slice {
            match self.slices.bounds_of(note.key) {
                Some(bounds) => bounds,
                None => return,
            }
        } else {
            (s.region_start, s.region_end)
        };
        if region_end <= region_start {
            return;
        }
        let base = s.transpose + s.detune / 100.0;
        let pitch = if s.mode == Mode::Slice {
            // A slice plays at the sample's own rate; the note names the
            // slice and doesn't transpose it.
            base
        } else {
            f64::from(note.key) - 60.0 + base
        };
        let velocity = note.velocity.clamp(0.0, 1.0);
        // Glide's monophonic mode takes the sounding voice over: the new
        // note glides from its pitch, keeping its position and envelope.
        // Slices don't glide: they play at the sample's own rate.
        if s.glide
            && !s.glide_portamento
            && s.mode != Mode::Slice
            && let Some(voice) = self.newest_playing()
        {
            voice.key = note.key;
            voice.target = pitch;
            voice.step = if s.glide_frames == 0 {
                voice.pitch = pitch;
                voice.ratio = 2f64.powf(pitch / 12.0);
                0.0
            } else {
                (pitch - voice.pitch) / f64::from(s.glide_frames)
            };
            self.last_pitch = Some(pitch);
            return;
        }
        // One-Shot and Slice are monophonic: the new note fades any
        // sounding voice out. Classic fades a voice on the same key with
        // Retrig on, and past the voice limit the oldest sounding voice.
        if s.mode != Mode::Classic {
            for voice in self.playing() {
                voice.fade(s);
            }
        } else {
            if s.retrig {
                for voice in self.playing() {
                    if voice.key == note.key {
                        voice.fade(s);
                    }
                }
            }
            let over = (self.playing().count() + 1).saturating_sub(s.voices);
            for _ in 0..over {
                if let Some(voice) = self.playing().min_by_key(|voice| voice.counter) {
                    voice.fade(s);
                }
            }
        }
        self.counter += 1;
        let counter = self.counter;
        let from = if s.glide {
            self.last_pitch.unwrap_or(pitch)
        } else {
            pitch
        };
        let Some(slot) = self.free_slot_index() else {
            return;
        };
        let mut voice = Voice::idle();
        voice.active = true;
        voice.key = note.key;
        voice.counter = counter;
        voice.position = region_start as f64;
        voice.end = region_end;
        voice.pitch = from;
        voice.target = pitch;
        voice.step = if s.glide_frames == 0 || s.mode == Mode::Slice {
            voice.pitch = pitch;
            0.0
        } else {
            (pitch - from) / f64::from(s.glide_frames)
        };
        voice.ratio = 2f64.powf(voice.pitch / 12.0);
        voice.velocity = velocity;
        voice.one_shot = s.mode != Mode::Classic;
        voice.env.trigger(s.attack_frames);
        voice.filter_env.trigger(s.filter_attack_frames);
        // A retriggered voice starts its LFO at the offset; otherwise it
        // follows the device's free-running phase (the manual: Offset does
        // nothing when Retrigger is off).
        voice.lfo_phase = if s.lfo_retrigger {
            s.lfo_offset
        } else {
            self.free_phase
        };
        voice.lfo_random = random(&mut self.rng[slot]);
        self.filter.reset_voice(slot, &self.filter_params);
        self.voices[slot] = voice;
        self.last_pitch = Some(pitch);
    }

    fn note_off(&mut self, key: u8, s: &Settings) {
        for voice in self.voices.iter_mut() {
            if !voice.active || voice.fading || voice.key != key {
                continue;
            }
            if voice.one_shot {
                // With Trigger on the note's end is ignored, and the filter
                // envelope runs its course with the amplitude envelope.
                if !s.trigger {
                    voice.env.release(s.fade_out_frames);
                    voice.filter_env.release(s.filter_release_frames);
                }
            } else {
                voice.env.release(s.release_frames);
                voice.filter_env.release(s.filter_release_frames);
            }
        }
    }

    fn choke(&mut self, key: u8, s: &Settings) {
        for voice in self.voices.iter_mut() {
            if voice.active && !voice.fading && voice.key == key {
                voice.fade(s);
            }
        }
    }

    /// The sounding voices: active and not already fading out.
    fn playing(&mut self) -> impl Iterator<Item = &mut Voice> {
        self.voices.iter_mut().filter(|v| v.active && !v.fading)
    }

    /// The newest sounding voice: the one a glide takes over.
    fn newest_playing(&mut self) -> Option<&mut Voice> {
        let counter = self.playing().map(|v| v.counter).max()?;
        self.playing().find(|v| v.counter == counter)
    }

    /// A slot for a new voice: an idle one, else the oldest fading one,
    /// which is closest to silent.
    fn free_slot_index(&self) -> Option<usize> {
        self.voices.iter().position(|v| !v.active).or_else(|| {
            let oldest = self
                .voices
                .iter()
                .filter(|v| v.fading)
                .map(|v| v.counter)
                .min()?;
            self.voices
                .iter()
                .position(|v| v.fading && v.counter == oldest)
        })
    }

    /// One frame of every voice: its sample into its lane, its cutoff and
    /// its LFO value. The voice's level and velocity scale the lane, and
    /// the output Volume and pan are applied after the filter, in
    /// [`mixed`](Self::mixed).
    fn voices_frame(&mut self, s: &Settings, sample: &SampleData, frame: usize, gain: f32) {
        // The free-running phase the voices started from: one clock for
        // the device, so voices without Retrigger stay together.
        if s.lfo_on {
            self.free_phase += 1.0 / s.lfo_cycle_frames;
            if self.free_phase >= 1.0 {
                self.free_phase -= 1.0;
            }
        }
        let stride = self.stride;
        for slot in 0..SLOTS {
            let voice = &mut self.voices[slot];
            if !voice.active {
                continue;
            }
            voice.env.step(s.sustain, s.decay_frames);
            if voice.env.stage == Stage::Idle {
                voice.active = false;
                continue;
            }
            voice
                .filter_env
                .step(s.filter_sustain, s.filter_decay_frames);
            // The LFO: as the manual describes, per voice, at the voice's
            // own rate, rising over its Attack after the note starts.
            let mut lfo_value = 0.0f32;
            if s.lfo_on {
                let cycle = voice_cycle(s, voice.key);
                voice.lfo_phase += 1.0 / cycle;
                if voice.lfo_phase >= 1.0 {
                    voice.lfo_phase -= 1.0;
                    voice.lfo_random = random(&mut self.rng[slot]);
                }
                let shape = lfo_shape(s.lfo_wave, voice.lfo_phase, voice.lfo_random);
                let attack = if s.lfo_attack_frames == 0 {
                    1.0
                } else {
                    (voice.lfo_age as f32 / s.lfo_attack_frames as f32).min(1.0)
                };
                voice.lfo_age = voice.lfo_age.saturating_add(1);
                lfo_value = shape * attack;
            }
            let ratio = voice.ratio;
            let sample_value = read(sample, voice.position);
            let mut value = sample_value;
            // The loop's wrap: a constant-power crossfade from the loop's
            // end into its start over the Fade share of it.
            if s.looping && s.fade > 0 && s.loop_end > s.loop_start {
                let fade_start = s.loop_end as f64 - f64::from(s.fade);
                if voice.position >= fade_start {
                    let t = ((voice.position - fade_start) / f64::from(s.fade)) as f32;
                    let (out, in_) = (
                        (t * core::f32::consts::FRAC_PI_2).cos(),
                        (t * core::f32::consts::FRAC_PI_2).sin(),
                    );
                    let from_start =
                        read(sample, s.loop_start as f64 + (voice.position - fade_start));
                    value = (
                        value.0 * out + from_start.0 * in_,
                        value.1 * out + from_start.1 * in_,
                    );
                }
            }
            let mut level = gain * voice.env.level * (1.0 - s.velocity * (1.0 - voice.velocity));
            // One-Shot and Slicing fade the voice out over Fade Out,
            // ending at its own end.
            if voice.one_shot && s.fade_out_frames > 0 {
                let fade_start = voice.end as f64 - f64::from(s.fade_out_frames);
                if voice.position >= fade_start {
                    let t = ((voice.end as f64 - voice.position) / f64::from(s.fade_out_frames))
                        .clamp(0.0, 1.0) as f32;
                    level *= t;
                }
            }
            let base = frame * 2 * stride + slot;
            self.audio[base] = value.0 * level;
            self.audio[base + stride] = value.1 * level;
            self.cutoff[frame * stride + slot] =
                s.cutoff_pitch(voice.key, voice.filter_env.level, lfo_value);
            self.lfo[frame * stride + slot] = lfo_value;
            // The voice moves on: its pitch first (with the LFO's bend),
            // then its position.
            if voice.step != 0.0 {
                voice.pitch += voice.step;
                if (voice.step > 0.0 && voice.pitch >= voice.target)
                    || (voice.step < 0.0 && voice.pitch <= voice.target)
                {
                    voice.pitch = voice.target;
                    voice.step = 0.0;
                }
                voice.ratio = 2f64.powf(voice.pitch / 12.0);
            }
            let bend = 2f64
                .powf(LFO_PITCH_SEMITONES * f64::from(s.lfo_pitch) * f64::from(lfo_value) / 12.0);
            voice.position += ratio * bend;
            if s.looping && s.loop_end > s.loop_start && voice.position >= s.loop_end as f64 {
                // The wrap resumes past the head the crossfade already
                // played, so the fade's end and the resumed audio meet.
                let length = (s.loop_end - s.loop_start) as f64;
                voice.position = s.loop_start as f64
                    + (voice.position + f64::from(s.fade) - s.loop_end as f64).rem_euclid(length);
            } else if voice.position >= voice.end as f64 {
                voice.active = false;
            }
        }
    }

    /// Applies the filter, per voice, to what [`voices_frame`] wrote.
    fn filtered(&mut self, s: &Settings, frames: usize) {
        if s.filter_type != self.filter_type {
            self.filter_type = s.filter_type;
            self.filter.set_type(filter_type_of(s.filter_type));
        }
        let params = VoiceParams {
            resonance: s.filter_res.clamp(0.0, 1.0),
            drive: 0.0,
            saturation: 0.0,
            character: 0.0,
            stereo_offset: 0.0,
            mix: 1.0,
            level: 1.0,
            morph_x: 0.0,
            morph_y: 0.0,
        };
        if params != self.filter_params {
            self.filter_params = params;
            for voice in 0..SLOTS {
                self.filter.set_params(voice, &params);
            }
        }
        let stride = self.stride;
        self.filter.process_voices(
            SLOTS,
            &self.cutoff[..frames * stride],
            &mut self.audio[..frames * 2 * stride],
        );
    }

    /// Mixes the filter's output into the block's channels: the output
    /// Volume, pan, and the LFO's volume and pan movements. A voice's lane
    /// holds zero after its end, so every lane can be summed.
    fn mixed(
        &self,
        s: &Settings,
        frames: usize,
        volume: &[f32],
        pan: &[f32],
        left: &mut [f32],
        right: Option<&mut [f32]>,
    ) {
        let stride = self.stride;
        let mut right = right;
        for frame in 0..frames {
            let volume = volume.get(frame).copied().unwrap_or(1.0);
            let pan = pan.get(frame).copied().unwrap_or(0.0);
            let (mut l, mut r) = (0.0f32, 0.0f32);
            for slot in 0..SLOTS {
                let lfo_value = self.lfo[frame * stride + slot];
                let lfo_volume = (1.0 + s.lfo_volume * lfo_value).max(0.0);
                let pan_units = (pan + s.lfo_pan * 50.0 * lfo_value).clamp(-50.0, 50.0);
                let b = (pan_units / 50.0).clamp(-1.0, 1.0);
                let (left_pan, right_pan) = (1.0 - b.max(0.0), 1.0 + b.min(0.0));
                let base = frame * 2 * stride + slot;
                let scale = volume * lfo_volume;
                l += self.audio[base] * scale * left_pan;
                r += self.audio[base + stride] * scale * right_pan;
            }
            left[frame] = l;
            if let Some(right) = &mut right {
                right[frame] = r;
            }
        }
    }
}

impl NativeDevice for Sampler {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        let frames = audio.frames();
        if frames == 0 {
            return;
        }
        // The decoded sample, held aside for the stretch; cloning the
        // `Arc` only moves a reference, so nothing allocates.
        let sample = match self
            .data
            .as_ref()
            .and_then(|data| data.downcast_ref::<Arc<SampleData>>())
        {
            Some(sample) if sample.frames() > 0 => Arc::clone(sample),
            _ => {
                for voice in &mut self.voices {
                    voice.active = false;
                }
                return;
            }
        };
        let at = |id: ParamId| params.index(id).expect("the description holds it");
        let rate = audio.sample_rate();
        let settings = self.settings(params, rate, audio.tempo(), audio.song_position());
        // Slices are Slicing mode's alone: working them out in Classic or
        // One-Shot would scan the region on the audio thread for nothing.
        if settings.mode == Mode::Slice {
            self.slices
                .update(&sample, &settings, Arc::as_ptr(&sample) as usize);
        }
        let gain = params.ramp(at(GAIN));
        let volume = params.ramp(at(VOLUME));
        let pan = params.ramp(at(PAN));
        let stride = self.stride;
        self.audio[..frames * 2 * stride].fill(0.0);
        self.cutoff[..frames * stride].fill(0.0);
        let mut notes = audio.notes().peekable();
        let mut channels = audio.channels_mut();
        let Some(left) = channels.next() else {
            return;
        };
        let right = channels.next();
        // 1. The notes and the voices: each voice's sample into its lane,
        //    with its cutoff and LFO.
        for frame in 0..frames {
            while notes
                .peek()
                .is_some_and(|note| note.offset as usize <= frame)
            {
                let note = notes.next().expect("peeked");
                self.play(note, &settings);
            }
            let gain = gain.get(frame).copied().unwrap_or(1.0);
            self.voices_frame(&settings, &sample, frame, gain);
        }
        // 2. The filter, per voice.
        if settings.filter_on {
            self.filtered(&settings, frames);
        }
        // 3. The amplitudes and pans, and the output.
        self.mixed(&settings, frames, volume, pan, left, right);
    }

    fn receive_data(&mut self, data: NodeData) {
        self.data = Some(data);
    }
}

/// What one stretch plays by, worked out from the parameters and the
/// sample.
#[derive(Clone, Copy)]
struct Settings {
    /// The engine's rate: transient detection's windows and the free LFO's
    /// cycle read it.
    sample_rate: f64,
    mode: Mode,
    region_start: u64,
    region_end: u64,
    looping: bool,
    loop_start: u64,
    loop_end: u64,
    /// The crossfade at the loop's wrap, in frames.
    fade: u32,
    transpose: f64,
    detune: f64,
    voices: usize,
    retrig: bool,
    glide: bool,
    glide_portamento: bool,
    glide_frames: u32,
    /// How much velocity scales the level, 0 to 1.
    velocity: f32,
    attack_frames: u32,
    decay_frames: u32,
    sustain: f32,
    release_frames: u32,
    steal_frames: u32,
    trigger: bool,
    fade_out_frames: u32,
    slice_by: SliceBy,
    /// Transient detection's sensitivity, 0 to 1.
    sensitivity: f64,
    /// Beat slicing's division, in frames at the set's tempo.
    division_frames: f64,
    /// Region slicing's number of equal slices.
    regions: usize,
    filter_on: bool,
    filter_type: usize,
    filter_freq: f64,
    filter_res: f32,
    /// How much the played note moves the cutoff, 0 to 1.
    filter_key: f32,
    filter_attack_frames: u32,
    filter_decay_frames: u32,
    filter_sustain: f32,
    filter_release_frames: u32,
    /// How far the filter envelope moves the cutoff, -1 to 1.
    filter_amount: f32,
    lfo_on: bool,
    lfo_wave: usize,
    /// The LFO's cycle, in frames at the free rate or the synced division.
    lfo_cycle_frames: f64,
    lfo_attack_frames: u32,
    lfo_offset: f64,
    lfo_retrigger: bool,
    /// How much the played note speeds the LFO up, 0 to 1.
    lfo_key: f32,
    lfo_volume: f32,
    lfo_pitch: f32,
    lfo_pan: f32,
    lfo_filter: f32,
}

impl Settings {
    /// The cutoff a voice's filter runs at this frame, in the filter's
    /// semitones: the knob's frequency, moved by the played note (Key),
    /// the filter envelope and the LFO.
    fn cutoff_pitch(&self, key: u8, envelope: f32, lfo: f32) -> f32 {
        let note = f64::from(key) - 60.0;
        let pitch = f64::from(pitch_of(self.filter_freq as f32, REFERENCE_HZ))
            + f64::from(self.filter_key) * note
            + FILTER_ENV_OCTAVES * 12.0 * f64::from(self.filter_amount) * f64::from(envelope)
            + LFO_FILTER_OCTAVES * 12.0 * f64::from(self.lfo_filter) * f64::from(lfo);
        pitch as f32
    }
}

/// The kit's filter type the Type choice names.
fn filter_type_of(entry: usize) -> FilterType {
    match entry {
        1 => FilterType::Lp24,
        2 => FilterType::Hp12,
        3 => FilterType::Hp24,
        4 => FilterType::Bp12,
        5 => FilterType::Bp24,
        6 => FilterType::Notch12,
        7 => FilterType::Notch24,
        _ => FilterType::Lp12,
    }
}

/// A voice's LFO cycle, in frames: the device's cycle at the voice's own
/// rate through Key (an octave above C3 doubles it at 100 %).
fn voice_cycle(s: &Settings, key: u8) -> f64 {
    let scale = 2f64.powf(f64::from(s.lfo_key) * (f64::from(key) - 60.0) / 12.0);
    (s.lfo_cycle_frames / scale).max(1.0)
}

/// The LFO waveform at `phase` (0 to 1): -1 to 1. `random` is the value
/// the Random waveform holds for its cycle.
fn lfo_shape(wave: usize, phase: f64, random: f32) -> f32 {
    match wave {
        1 => {
            if phase < 0.5 {
                1.0
            } else {
                -1.0
            }
        }
        2 => (1.0 - 4.0 * (phase - 0.5).abs()) as f32,
        3 => (1.0 - 2.0 * phase) as f32,
        4 => (2.0 * phase - 1.0) as f32,
        5 => random,
        _ => (core::f64::consts::TAU * phase).sin() as f32,
    }
}

/// The next value of the LFO's Random waveform: a new value in -1 to 1
/// each cycle.
fn random(state: &mut u32) -> f32 {
    *state ^= *state << 13;
    *state ^= *state >> 17;
    *state ^= *state << 5;
    (*state as f32 / u32::MAX as f32) * 2.0 - 1.0
}

/// Slicing's slice boundaries, and what they were worked out from. A
/// slice runs from its boundary to the next, so `count` slices have
/// `count + 1` boundaries.
struct Slices {
    signature: Option<SliceSignature>,
    bounds: [u64; MAX_SLICES + 1],
    count: usize,
}

/// The inputs slicing's boundaries depend on: when they all match, the
/// boundaries are up to date.
#[derive(Clone, Copy, PartialEq)]
struct SliceSignature {
    by: SliceBy,
    sensitivity: f64,
    division_frames: f64,
    region_start: u64,
    region_end: u64,
    /// The sample's address: a new sample re-slices.
    sample: usize,
    frames: u64,
}

impl Slices {
    const fn new() -> Self {
        Self {
            signature: None,
            bounds: [0; MAX_SLICES + 1],
            count: 0,
        }
    }

    /// The start and end of the slice `key` names, C3 first, or `None`
    /// outside the slices. `count` is the boundaries, one more than the
    /// slices.
    fn bounds_of(&self, key: u8) -> Option<(u64, u64)> {
        let index = key.checked_sub(SLICE_BASE)? as usize;
        (index + 1 < self.count).then(|| (self.bounds[index], self.bounds[index + 1]))
    }

    /// Works the boundaries out again when their inputs changed. The
    /// signature holds the sensitivity to the nearest per cent, so
    /// automation that sweeps it doesn't re-slice on every block.
    fn update(&mut self, sample: &SampleData, s: &Settings, address: usize) {
        let signature = SliceSignature {
            by: s.slice_by,
            sensitivity: (s.sensitivity * 100.0).round(),
            // Only Beat slicing reads the tempo through this, so only Beat
            // re-slices while it moves: a tempo ramp must not re-scan the
            // region every block.
            division_frames: if s.slice_by == SliceBy::Beat {
                s.division_frames
            } else {
                0.0
            },
            region_start: s.region_start,
            region_end: s.region_end,
            sample: address,
            frames: sample.frames() as u64,
        };
        if self.signature == Some(signature) {
            return;
        }
        self.signature = Some(signature);
        self.count = 0;
        if s.region_end <= s.region_start || sample.frames() == 0 {
            return;
        }
        match s.slice_by {
            SliceBy::Manual => {}
            SliceBy::Transient => self.detect(sample, s),
            SliceBy::Beat => self.beat(s),
            SliceBy::Region => self.regions(s),
        }
    }

    /// Pushes one boundary, keeping the list sorted and capped: duplicates
    /// and boundaries past the cap don't make empty slices.
    fn push(&mut self, at: u64) {
        if self.count < MAX_SLICES + 1 && (self.count == 0 || at > self.bounds[self.count - 1]) {
            self.bounds[self.count] = at;
            self.count += 1;
        }
    }

    /// Equal slices: the Regions parameter's count.
    fn regions(&mut self, s: &Settings) {
        let count = s.regions.clamp(1, MAX_SLICES);
        let length = s.region_end - s.region_start;
        for k in 0..=count as u64 {
            self.push(s.region_start + length * k / count as u64);
        }
        self.push(s.region_end);
    }

    /// Slices every Division sixteenth notes at the set's tempo, from the
    /// region's start: Beatlayer's reading of the manual's Beat slicing
    /// until warp and tempo detection exist, so the sample is taken to
    /// play at the set's tempo.
    fn beat(&mut self, s: &Settings) {
        let step = s.division_frames.max(1.0);
        self.push(s.region_start);
        let mut at = s.region_start as f64 + step;
        while at < s.region_end as f64 && self.count < MAX_SLICES {
            self.push(at as u64);
            at += step;
        }
        self.push(s.region_end);
    }

    /// Slices on the sample's transients (the manual's Transient slicing):
    /// a peak-hold envelope with a short fall between the sample's peaks,
    /// and an onset where it rises past the threshold Sensitivity sets.
    /// Beatlayer's own detector; up to 64 slices, the first found. The
    /// region's start and end are boundaries of their own, so the slices
    /// cover it.
    fn detect(&mut self, sample: &SampleData, s: &Settings) {
        let samples = sample.channel(0).unwrap_or(&[]);
        let (start, end) = (s.region_start, s.region_end);
        let fall = (-1.0 / (TRANSIENT_FALL_MS / 1000.0 * s.sample_rate)).exp();
        // First, the loudest the envelope gets, for the threshold.
        let mut env = 0.0f64;
        let mut peak = 0.0f64;
        for i in start..end {
            env = (env * fall).max(f64::from(samples[i as usize].abs()));
            peak = peak.max(env);
        }
        self.push(start);
        if peak > 0.0 {
            let threshold = peak * 10f64.powf(-TRANSIENT_RANGE_DB / 20.0 * s.sensitivity);
            let mut env = 0.0f64;
            let mut above = false;
            for i in start..end {
                if self.count >= MAX_SLICES {
                    break;
                }
                env = (env * fall).max(f64::from(samples[i as usize].abs()));
                if env >= threshold {
                    if !above {
                        self.push(i);
                    }
                    above = true;
                } else {
                    above = false;
                }
            }
        }
        self.push(end);
    }
}

/// Where an amplitude envelope is.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    Idle,
    Attack,
    Decay,
    Sustain,
    Release,
}

/// One voice's envelope: a level moving in a straight line through its
/// stages.
#[derive(Clone, Copy)]
struct Env {
    stage: Stage,
    level: f32,
    from: f32,
    to: f32,
    /// Frames left in the current stage; 0 means the stage is done.
    left: u32,
    /// The stage's whole length, for the level's place in it.
    total: u32,
}

impl Env {
    fn idle() -> Self {
        Self {
            stage: Stage::Idle,
            level: 0.0,
            from: 0.0,
            to: 0.0,
            left: 0,
            total: 1,
        }
    }

    /// Starts a stage from the current level.
    fn enter(&mut self, stage: Stage, to: f32, frames: u32) {
        self.stage = stage;
        self.from = self.level;
        self.to = to;
        self.left = frames;
        self.total = frames.max(1);
        if frames == 0 {
            self.level = to;
        }
    }

    /// Starts the envelope at its attack.
    fn trigger(&mut self, frames: u32) {
        self.level = 0.0;
        self.enter(Stage::Attack, 1.0, frames);
    }

    /// Releases the envelope: the level falls to silence over `frames`.
    fn release(&mut self, frames: u32) {
        self.enter(Stage::Release, 0.0, frames);
    }

    /// Moves the envelope on by one frame, through any stage it finishes.
    fn step(&mut self, sustain: f32, decay_frames: u32) {
        if self.stage == Stage::Idle {
            return;
        }
        if self.left > 0 {
            self.left -= 1;
            let place = 1.0 - self.left as f32 / self.total as f32;
            self.level = self.from + (self.to - self.from) * place;
        }
        while self.left == 0 {
            match self.stage {
                Stage::Attack => self.enter(Stage::Decay, sustain, decay_frames),
                Stage::Decay => self.enter(Stage::Sustain, sustain, 0),
                Stage::Sustain => break,
                Stage::Release => {
                    self.stage = Stage::Idle;
                    self.level = 0.0;
                    break;
                }
                Stage::Idle => break,
            }
        }
    }
}

/// One sounding sample position.
#[derive(Clone, Copy)]
struct Voice {
    active: bool,
    /// Fading out because it was stolen or choked: not counted as a
    /// sounding voice.
    fading: bool,
    key: u8,
    /// When it started: the oldest voice is stolen first.
    counter: u64,
    /// Its position in the sample, in frames.
    position: f64,
    /// The sample frame its own stretch ends at: the region's end, or the
    /// slice's.
    end: u64,
    /// Its pitch in semitones from C3, and where a glide is taking it.
    pitch: f64,
    target: f64,
    /// Semitones per frame while gliding.
    step: f64,
    /// Its playback rate: sample frames per output frame.
    ratio: f64,
    velocity: f32,
    /// Whether it plays One-Shot's or Slicing's straight-through rules.
    one_shot: bool,
    env: Env,
    /// The filter envelope, which moves the cutoff (the sampler spec's
    /// filter rule).
    filter_env: Env,
    /// Its LFO's place in its cycle.
    lfo_phase: f64,
    /// Frames since the note started, for the LFO's Attack.
    lfo_age: u32,
    /// The value the Random waveform holds for this cycle.
    lfo_random: f32,
}

impl Voice {
    fn idle() -> Self {
        Self {
            active: false,
            fading: false,
            key: 0,
            counter: 0,
            position: 0.0,
            end: 0,
            pitch: 0.0,
            target: 0.0,
            step: 0.0,
            ratio: 1.0,
            velocity: 1.0,
            one_shot: false,
            env: Env::idle(),
            filter_env: Env::idle(),
            lfo_phase: 0.0,
            lfo_age: 0,
            lfo_random: 0.0,
        }
    }

    /// Fades it out over the steal fade, keeping its sample playing.
    fn fade(&mut self, s: &Settings) {
        self.fading = true;
        self.env.release(s.steal_frames);
    }
}

/// One sample frame at `position` in each channel: linear interpolation
/// between the neighbouring frames, silence past the end, and a mono sample
/// on both sides.
fn read(sample: &SampleData, position: f64) -> (f32, f32) {
    let frames = sample.frames();
    if frames == 0 || position < 0.0 {
        return (0.0, 0.0);
    }
    let index = position.floor() as usize;
    if index >= frames {
        return (0.0, 0.0);
    }
    let next = (index + 1).min(frames - 1);
    let place = (position - index as f64) as f32;
    let channel = |c: usize| {
        let samples = sample
            .channel(c)
            .or_else(|| sample.channel(0))
            .unwrap_or(&[]);
        let a = samples.get(index).copied().unwrap_or(0.0);
        let b = samples.get(next).copied().unwrap_or(0.0);
        a + (b - a) * place
    };
    (channel(0), channel(1))
}

/// Milliseconds as whole frames at `rate`.
fn frames_of(milliseconds: f64, rate: f64) -> u32 {
    (milliseconds.max(0.0) / 1000.0 * rate).round() as u32
}

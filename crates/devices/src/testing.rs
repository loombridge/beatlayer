//! Devices that prove the framework, for tests only. Not for songs: the
//! browser never offers them, and Beatlayer ships none of them.
//!
//! | Type | Name | What it does | Layout |
//! |---|---|---|---|
//! | [`UTILITY`] | Framework Test Utility | gain to −∞ dB, mute, polarity, balance | generic |
//! | [`DRIVE`] | Framework Test Drive | a `tanh` saturator, oversampled 1 to 8 times | generic |
//! | [`SCOPE`] | Framework Test Scope | passes audio through; a display cell between control bands, with a row inside its bottom edge | its own |
//! | [`SPECTROGRAM`] | Framework Test Spectrogram | passes audio through; a display cell that draws a spectrogram of it from `bl_dsp::stft` | its own |
//! | [`EFFECTS`] | Framework Test Effects | the effects' shared parts: an LFO tremolo, or the LFO's or the envelope follower's value as the output; a menu option and an expanded part | its own |
//! | [`INSTRUMENT`] | Framework Test Instrument | the instruments' shared parts: sine voices with every part of the voice section, or a source as the output; a menu option | generic |
//! | [`PAGES`] | Framework Test Pages | passes audio through; tabs of three pages in the body beside a shell of three sections and its display | its own |
//!
//! Between them they have a parameter of every kind: gains, continuous
//! values with linear and logarithmic tapers, toggles, choices, and whole
//! numbers.

use bl_dsp::stft::{self, Stft, Window};
use bl_dsp::{EnvelopeFollower, Oversampler};
use bl_engine::{DeviceType, NodeConfig, NodeData, Registry};
use bl_model::device_layout::{
    BODY_HEIGHT, ControlBand, DisplayCell, LayoutNode, Page, PageStyle, PagesNode, TAB_HEIGHT,
    Widget,
};
use bl_model::{
    Device, DeviceDescription, DeviceDisplay, DeviceId, DeviceLayout, DeviceRole,
    DisplaySpectrogram, ParamDescriptor, ParamId, ParamUnit, SpectrogramGrid,
};

use core::f64::consts::TAU;

use lspk_dsp::envelope::{EnvelopeBank, EnvelopeParams, StealRestart, Timing};
use lspk_voice::{MAX_VOICES, Start, VoiceBlock, VoiceEvent, VoiceEventKind};

use crate::framework::{Audio, NativeDevice, ParamBank, add};
use crate::instrument::matrix::{
    DestDef, Kind as MatrixKind, Matrix, MatrixLayout, Pick, Range as MatrixRange,
    Rate as MatrixRate, Scope as MatrixScope, SlotDef, SourceDef,
};
use crate::instrument::{self, Instrument, NoteRange, Parts, Section, Sources, State};
use crate::lfo::{Lfo, Rate, Shape, Stereo, Wave};

/// The test utility's device type.
pub const UTILITY: &str = "test.framework.utility";
/// The test drive's device type.
pub const DRIVE: &str = "test.framework.drive";
/// The test scope's device type.
pub const SCOPE: &str = "test.framework.scope";

/// The test spectrogram's device type.
pub const SPECTROGRAM: &str = "test.framework.spectrogram";

/// The test effects' device type.
pub const EFFECTS: &str = "test.framework.effects";
/// The test instrument's device type.
pub const INSTRUMENT: &str = "test.framework.instrument";
/// The test pages' device type.
pub const PAGES: &str = "test.framework.pages";

/// The utility's gain: −∞ to +12 dB, 0 dB by default.
pub const UTILITY_GAIN: ParamId = ParamId(0);
/// The utility's balance, −50 (left) to 50 (right).
pub const UTILITY_BALANCE: ParamId = ParamId(1);
/// The utility's mute.
pub const UTILITY_MUTE: ParamId = ParamId(2);
/// The utility's polarity: Normal or Inverted.
pub const UTILITY_POLARITY: ParamId = ParamId(3);

/// The drive's input gain: −∞ to +36 dB.
pub const DRIVE_DRIVE: ParamId = ParamId(0);
/// The drive's output gain: −∞ to +6 dB.
pub const DRIVE_OUTPUT: ParamId = ParamId(1);
/// The drive's oversampling: Off, 2x, 4x, or 8x.
pub const DRIVE_OVERSAMPLING: ParamId = ParamId(2);

/// The scope's time window, 1 ms to 1 s.
pub const SCOPE_TIME: ParamId = ParamId(0);
/// The scope's display gain.
pub const SCOPE_GAIN: ParamId = ParamId(1);
/// The scope's freeze.
pub const SCOPE_FREEZE: ParamId = ParamId(2);
/// The scope's grid divisions, 1 to 16.
pub const SCOPE_DIVISIONS: ParamId = ParamId(3);
/// The scope's trigger level, −100 to 100 %.
pub const SCOPE_TRIGGER: ParamId = ParamId(4);

/// The spectrogram's freeze.
pub const SPECTROGRAM_FREEZE: ParamId = ParamId(0);

/// The spectrogram's grid: 64 columns of the newest frames, 48 rows from
/// 40 Hz to 16 kHz, drawn between -90 and 0 dB.
pub const SPECTROGRAM_GRID: SpectrogramGrid = SpectrogramGrid {
    columns: 64,
    rows: 48,
    frequencies: [40.0, 16_000.0],
    log_frequency: true,
    levels: [-90.0, 0.0],
};

/// The spectrogram's frame size and overlap.
const SPECTROGRAM_FRAME: usize = 4096;
const SPECTROGRAM_OVERLAP: usize = 4;

/// The effects' output: Audio (a tremolo), LFO (the LFO's value on each
/// channel), or Envelope (the envelope follower's level of the input).
pub const EFFECTS_OUTPUT: ParamId = ParamId(0);
/// The effects' depth, 0 to 100 %.
pub const EFFECTS_DEPTH: ParamId = ParamId(1);
/// The LFO's waveform, one of [`Wave::ALL`].
pub const EFFECTS_WAVE: ParamId = ParamId(2);
/// How the LFO's rate is given: Hz, Seconds or Synced.
pub const EFFECTS_RATE_MODE: ParamId = ParamId(3);
/// The LFO's rate in Hz.
pub const EFFECTS_RATE: ParamId = ParamId(4);
/// The LFO's period in seconds.
pub const EFFECTS_PERIOD: ParamId = ParamId(5);
/// The LFO's synced division, in [`DIVISIONS`].
pub const EFFECTS_DIVISION: ParamId = ParamId(6);
/// How the channels differ: Phase or Spin.
pub const EFFECTS_STEREO: ParamId = ParamId(7);
/// The right LFO's phase against the left, in degrees.
pub const EFFECTS_PHASE: ParamId = ParamId(8);
/// The right LFO's rate against the left, in per cent.
pub const EFFECTS_SPIN: ParamId = ParamId(9);
/// The LFO's phase offset against the beat grid, in degrees.
pub const EFFECTS_OFFSET: ParamId = ParamId(10);
/// The LFO's duty cycle, −100 to 100 %.
pub const EFFECTS_DUTY: ParamId = ParamId(11);
/// How far the LFO's wave is morphed toward the next, 0 to 100 %.
pub const EFFECTS_MORPH: ParamId = ParamId(12);
/// The envelope follower's attack, in milliseconds.
pub const EFFECTS_ATTACK: ParamId = ParamId(13);
/// The envelope follower's release, in milliseconds.
pub const EFFECTS_RELEASE: ParamId = ParamId(14);
/// The envelope follower's hold.
pub const EFFECTS_HOLD: ParamId = ParamId(15);
/// A menu option: the LFO upside down.
pub const EFFECTS_INVERT: ParamId = ParamId(16);

/// The test instrument's parts, one parameter each, as the voice section
/// describes them.
pub const INSTRUMENT_VOICES: ParamId = ParamId(0);
pub const INSTRUMENT_VOICE_MODE: ParamId = ParamId(1);
pub const INSTRUMENT_PRIORITY: ParamId = ParamId(2);
pub const INSTRUMENT_LEGATO: ParamId = ParamId(3);
pub const INSTRUMENT_GLIDE_TIME: ParamId = ParamId(4);
pub const INSTRUMENT_GLIDE_MODE: ParamId = ParamId(5);
pub const INSTRUMENT_GLIDE_PROP: ParamId = ParamId(6);
pub const INSTRUMENT_GLIDE_LEGATO: ParamId = ParamId(7);
pub const INSTRUMENT_USE_SCALE: ParamId = ParamId(8);
pub const INSTRUMENT_RETRIGGER: ParamId = ParamId(9);
pub const INSTRUMENT_UNISON_VOICES: ParamId = ParamId(10);
pub const INSTRUMENT_UNISON_DETUNE: ParamId = ParamId(11);
pub const INSTRUMENT_UNISON_SPREAD: ParamId = ParamId(12);
pub const INSTRUMENT_UNISON_DELAY: ParamId = ParamId(13);
pub const INSTRUMENT_PB_RANGE: ParamId = ParamId(14);
pub const INSTRUMENT_NOTE_PB: ParamId = ParamId(15);
pub const INSTRUMENT_NOTE_PB_RANGE: ParamId = ParamId(16);
pub const INSTRUMENT_TRANSPOSE: ParamId = ParamId(17);
pub const INSTRUMENT_STRETCH: ParamId = ParamId(18);
pub const INSTRUMENT_ERROR: ParamId = ParamId(19);
pub const INSTRUMENT_HI_QUALITY: ParamId = ParamId(20);
pub const INSTRUMENT_VELOCITY_CURVE: ParamId = ParamId(21);
pub const INSTRUMENT_KEY_CURVE: ParamId = ParamId(22);
/// What the instrument sends out: its sine voices, or one source's value.
pub const INSTRUMENT_OUTPUT: ParamId = ParamId(23);
/// How far the held Pressure lowers the voices' level, on the control
/// rate Hi-Quality sets.
pub const INSTRUMENT_PRESSURE_LEVEL: ParamId = ParamId(24);

/// Spec: instrument modulation R1, R2. The test instrument's matrix: the
/// level of each note (per voice, control rate, additive), a fine tuning in
/// semitones (per voice, audio rate), a master level (global, control rate,
/// multiplicative) and the matrix's Amount. Four free slots, one chosen and
/// one fixed.
pub const INSTRUMENT_NOTE_LEVEL: ParamId = ParamId(25);
pub const INSTRUMENT_FINE_TUNE: ParamId = ParamId(26);
pub const INSTRUMENT_MASTER: ParamId = ParamId(27);
pub const INSTRUMENT_MATRIX_AMOUNT: ParamId = ParamId(28);
/// The four free slots' amounts, in per cent, 0 by default.
pub const INSTRUMENT_FREE_AMOUNTS: [ParamId; 4] =
    [ParamId(29), ParamId(30), ParamId(31), ParamId(32)];
/// The chosen slot's source and destination choosers and its amount.
pub const INSTRUMENT_CHOSEN_SOURCE: ParamId = ParamId(33);
pub const INSTRUMENT_CHOSEN_DEST: ParamId = ParamId(34);
pub const INSTRUMENT_CHOSEN_AMOUNT: ParamId = ParamId(35);
/// The fixed slot's source chooser (Pressure into Fine Tune by default
/// would be a chooser's: it is fixed here, so only its amount) and amount.
pub const INSTRUMENT_FIXED_AMOUNT: ParamId = ParamId(36);
/// The matrix's sources, in the order the chosen slot's chooser lists them
/// after Off.
pub const INSTRUMENT_SOURCES: [&str; 5] =
    ["Velocity", "Key", "Pressure", "Mod Wheel", "Pitch Bend"];
/// The matrix's destinations, in the order the chosen slot's chooser lists
/// them after Off.
pub const INSTRUMENT_DESTINATIONS: [&str; 3] = ["Note Level", "Fine Tune", "Master"];
/// The fine tuning's reach: ±12 semitones.
pub const INSTRUMENT_FINE_RANGE: f64 = 12.0;

/// What [`INSTRUMENT_OUTPUT`] chooses between, after the audio: a source of
/// the lowest sounding voice, or a global one; or the pitch in semitones of
/// the lowest or the highest sounding voice, frame by frame; or the position
/// on its travel that a matrix destination plays for the lowest sounding
/// voice, after every route, frame by frame.
pub const INSTRUMENT_OUTPUTS: [&str; 16] = [
    "Audio",
    "Velocity",
    "Release Velocity",
    "Key",
    "Random",
    "Pressure",
    "Slide",
    "Note PB",
    "Mod Wheel",
    "Pitch Bend",
    "Aftertouch",
    "Pitch",
    "Pitch (Last)",
    "Note Level",
    "Fine Tune",
    "Master",
];

/// The most voices and the most unison copies the test instrument has.
pub const INSTRUMENT_MAX_VOICES: usize = MAX_VOICES;
pub const INSTRUMENT_MAX_COPIES: usize = 8;

/// A voice's level at velocity 1 and full envelope.
pub const INSTRUMENT_LEVEL: f32 = 0.1;

/// The synced divisions' lengths, in quarter notes.
pub const DIVISIONS: [f64; 6] = [0.25, 0.5, 1.0, 2.0, 4.0, 8.0];

/// The test utility's type.
#[must_use]
pub fn utility_type() -> DeviceType {
    DeviceType::native(UTILITY, "Framework Test Utility", DeviceRole::AudioEffect).with_description(
        DeviceDescription::new(vec![
            ParamDescriptor::gain(UTILITY_GAIN, "Gain", 12.0, 0.0)
                .describe("Scales the signal, down to silence."),
            ParamDescriptor::continuous(
                UTILITY_BALANCE,
                "Balance",
                (-50.0, 50.0),
                0.0,
                ParamUnit::None,
            )
            .decimals(0)
            .describe("Moves the signal left or right of the middle."),
            ParamDescriptor::toggle(UTILITY_MUTE, "Mute", false)
                .describe("Turns the signal off without changing the other settings."),
            ParamDescriptor::choice(UTILITY_POLARITY, "Polarity", &["Normal", "Inverted"], 0)
                .describe("Inverts the signal's phase, or leaves it."),
        ]),
    )
}

/// The test drive's type.
#[must_use]
pub fn drive_type() -> DeviceType {
    DeviceType::native(DRIVE, "Framework Test Drive", DeviceRole::AudioEffect).with_description(
        DeviceDescription::new(vec![
            ParamDescriptor::gain(DRIVE_DRIVE, "Drive", 36.0, 0.0)
                .describe("How much the signal is driven into the saturator."),
            ParamDescriptor::gain(DRIVE_OUTPUT, "Output", 6.0, 0.0)
                .describe("Brings the saturated signal back to level."),
            ParamDescriptor::choice(
                DRIVE_OVERSAMPLING,
                "Oversampling",
                &["Off", "2x", "4x", "8x"],
                2,
            )
            .describe("How many times the rate the saturator runs at."),
        ]),
    )
}

/// The test scope's type: a display cell between two control bands, with
/// a row of controls inside the cell's bottom edge.
#[must_use]
pub fn scope_type() -> DeviceType {
    let params = vec![
        ParamDescriptor::continuous(
            SCOPE_TIME,
            "Time",
            (1.0, 1000.0),
            100.0,
            ParamUnit::Milliseconds,
        )
        .logarithmic()
        .describe("How much of the signal the display shows, in milliseconds."),
        ParamDescriptor::gain(SCOPE_GAIN, "Gain", 24.0, 0.0)
            .describe("Raises or lowers what the display shows, without changing the signal."),
        ParamDescriptor::toggle(SCOPE_FREEZE, "Freeze", false)
            .describe("Holds the display still while the audio keeps going."),
        ParamDescriptor::integer(SCOPE_DIVISIONS, "Divisions", (1, 16), 4, ParamUnit::None)
            .describe("How many divisions the display's grid has."),
        ParamDescriptor::continuous(
            SCOPE_TRIGGER,
            "Trigger",
            (-100.0, 100.0),
            0.0,
            ParamUnit::Percent,
        )
        .describe("The level at which the display's window begins."),
    ];
    let layout = DeviceLayout::new(LayoutNode::Row(vec![
        LayoutNode::Band(ControlBand::stacked(
            BODY_HEIGHT,
            &[(SCOPE_TIME, Widget::Knob), (SCOPE_GAIN, Widget::Knob)],
        )),
        LayoutNode::Cell(
            DisplayCell::new("Scope", 160.0, BODY_HEIGHT, 1.0).with_bottom(ControlBand::stacked(
                40.0,
                &[(SCOPE_DIVISIONS, Widget::Number)],
            )),
        ),
        LayoutNode::Band(ControlBand::stacked(
            BODY_HEIGHT,
            &[
                (SCOPE_FREEZE, Widget::Toggle),
                (SCOPE_TRIGGER, Widget::Knob),
            ],
        )),
    ]));
    DeviceType::native(SCOPE, "Framework Test Scope", DeviceRole::AudioEffect)
        .with_description(DeviceDescription::new(params).with_layout(layout))
}

/// The test spectrogram's type: a display cell that draws a spectrogram.
#[must_use]
pub fn spectrogram_type() -> DeviceType {
    let params = vec![
        ParamDescriptor::toggle(SPECTROGRAM_FREEZE, "Freeze", false)
            .describe("Holds the display still while the audio keeps going."),
    ];
    let layout = DeviceLayout::new(LayoutNode::Row(vec![
        LayoutNode::Cell(DisplayCell::new("Spectrogram", 240.0, BODY_HEIGHT, 1.0)),
        LayoutNode::Band(ControlBand::stacked(
            BODY_HEIGHT,
            &[(SPECTROGRAM_FREEZE, Widget::Toggle)],
        )),
    ]));
    DeviceType::native(
        SPECTROGRAM,
        "Framework Test Spectrogram",
        DeviceRole::AudioEffect,
    )
    .with_description(
        DeviceDescription::new(params)
            .with_layout(layout)
            .with_spectrogram(SPECTROGRAM_GRID),
    )
}

/// What the test spectrogram's display shows, from the `data` its node
/// published.
#[must_use]
pub fn spectrogram_display(
    device: DeviceId,
    description: &DeviceDescription,
    data: Option<&[f32]>,
) -> DeviceDisplay {
    let spectrograms = description
        .spectrogram
        .zip(data)
        .and_then(|(grid, data)| DisplaySpectrogram::from_data("Spectrogram", grid, data))
        .into_iter()
        .collect();
    DeviceDisplay {
        device,
        live: data.is_some(),
        view: None,
        series: Vec::new(),
        spectrograms,
        markers: Vec::new(),
        readings: Vec::new(),
    }
}

/// The pages node of the test pages' body, in the tab style.
pub const PAGES_TABS: &str = "Body";
/// The pages node of the test pages' shell.
pub const PAGES_SHELL: &str = "Shell";
/// The pages node of the test pages' title-bar tabs.
pub const PAGES_TITLE: &str = "Title";

/// The test pages' type (WP-8.1): three tabs in the body (Osc with
/// parameters 0 and 1, Filter with 2 and 3, Amp with 4 and a display cell),
/// beside a shell of three sections (parameters 5 to 7), each with a page
/// in the display area (parameters 8 to 10), and two pages in tabs in the
/// title bar (parameters 11 and 12).
#[must_use]
pub fn pages_type() -> DeviceType {
    let params: Vec<ParamDescriptor> = (0..13)
        .map(|n| {
            ParamDescriptor::continuous(
                ParamId(n),
                &format!("P{n}"),
                (0.0, 100.0),
                0.0,
                ParamUnit::Percent,
            )
            .describe("A parameter whose control sits on one page.")
        })
        .collect();
    let band = |height, first: u32, count: u32| {
        LayoutNode::Band(ControlBand::stacked(
            height,
            &(first..first + count)
                .map(|n| (ParamId(n), Widget::Knob))
                .collect::<Vec<_>>(),
        ))
    };
    let tab_height = BODY_HEIGHT - TAB_HEIGHT;
    let page = |name, node| Page::new(name, node);
    let section = |name, param, display| {
        page(name, band(BODY_HEIGHT, display, 1)).with_section(band(60.0, param, 1))
    };
    let layout = DeviceLayout::new(LayoutNode::Row(vec![
        LayoutNode::Pages(PagesNode::new(
            PAGES_TABS,
            PageStyle::Tabs,
            vec![
                page("Osc", band(tab_height, 0, 2)),
                page("Filter", band(tab_height, 2, 2)),
                page(
                    "Amp",
                    LayoutNode::Row(vec![
                        band(tab_height, 4, 1),
                        LayoutNode::Cell(DisplayCell::new("Amp Graph", 80.0, tab_height, 1.0)),
                    ]),
                ),
            ],
        )),
        LayoutNode::Pages(PagesNode::new(
            PAGES_SHELL,
            PageStyle::Shell,
            vec![
                section("Env", 5, 8),
                section("LFO", 6, 9),
                section("Out", 7, 10),
            ],
        )),
        LayoutNode::Pages(PagesNode::new(
            PAGES_TITLE,
            PageStyle::TitleBar,
            vec![
                page("Main", band(BODY_HEIGHT, 11, 1)),
                page("Aux", band(BODY_HEIGHT, 12, 1)),
            ],
        )),
    ]));
    DeviceType::native(PAGES, "Framework Test Pages", DeviceRole::AudioEffect)
        .with_description(DeviceDescription::new(params).with_layout(layout))
}

/// The test effects' type: every shared part of the effects (WP-5.42), a
/// menu option and an expanded part.
#[must_use]
pub fn effects_type() -> DeviceType {
    let wave_names: Vec<&str> = Wave::ALL.iter().map(|w| w.name()).collect();
    let params = vec![
        ParamDescriptor::choice(EFFECTS_OUTPUT, "Output", &["Audio", "LFO", "Envelope"], 0)
            .describe("What the device sends out: its input under the LFO, the LFO, or the input's level."),
        ParamDescriptor::continuous(EFFECTS_DEPTH, "Depth", (0.0, 100.0), 100.0, ParamUnit::Percent)
            .describe("How far the LFO moves the audio."),
        ParamDescriptor::choice(EFFECTS_WAVE, "Wave", &wave_names, 0)
            .describe("The LFO's waveform."),
        ParamDescriptor::choice(
            EFFECTS_RATE_MODE,
            "Rate Mode",
            &["Hz", "Seconds", "Synced"],
            0,
        )
        .describe("Whether the LFO's rate is in Hz, in seconds per cycle, or synced to the tempo."),
        ParamDescriptor::continuous(EFFECTS_RATE, "Rate", (0.01, 40.0), 1.0, ParamUnit::Hertz)
            .logarithmic()
            .describe("The LFO's rate in Hz."),
        ParamDescriptor::continuous(EFFECTS_PERIOD, "Period", (0.025, 200.0), 1.0, ParamUnit::Seconds)
            .logarithmic()
            .describe("The LFO's cycle in seconds."),
        ParamDescriptor::choice(
            EFFECTS_DIVISION,
            "Division",
            &["1/16", "1/8", "1/4", "1/2", "1 bar", "2 bars"],
            2,
        )
        .describe("The LFO's cycle, synced to the tempo."),
        ParamDescriptor::choice(EFFECTS_STEREO, "Stereo", &["Phase", "Spin"], 0)
            .describe("Whether the channels' LFOs differ in phase or in rate."),
        ParamDescriptor::continuous(EFFECTS_PHASE, "Phase", (0.0, 360.0), 0.0, ParamUnit::None)
            .decimals(0)
            .describe("How many degrees the right channel's LFO is ahead of the left's."),
        ParamDescriptor::continuous(EFFECTS_SPIN, "Spin", (-50.0, 50.0), 0.0, ParamUnit::Percent)
            .describe("How much faster the right channel's LFO runs than the left's."),
        ParamDescriptor::continuous(EFFECTS_OFFSET, "Offset", (-180.0, 180.0), 0.0, ParamUnit::None)
            .decimals(0)
            .describe("Moves the synced LFO's cycle against the beat grid, in degrees."),
        ParamDescriptor::continuous(EFFECTS_DUTY, "Duty", (-100.0, 100.0), 0.0, ParamUnit::Percent)
            .describe("Squeezes the LFO's wave toward the front or the back of its cycle."),
        ParamDescriptor::continuous(EFFECTS_MORPH, "Morph", (0.0, 100.0), 0.0, ParamUnit::Percent)
            .describe("Blends the LFO's wave toward the next one."),
        ParamDescriptor::continuous(
            EFFECTS_ATTACK,
            "Attack",
            (0.1, 1_000.0),
            10.0,
            ParamUnit::Milliseconds,
        )
        .logarithmic()
        .describe("How fast the envelope follower rises."),
        ParamDescriptor::continuous(
            EFFECTS_RELEASE,
            "Release",
            (1.0, 5_000.0),
            100.0,
            ParamUnit::Milliseconds,
        )
        .logarithmic()
        .describe("How fast the envelope follower falls."),
        ParamDescriptor::toggle(EFFECTS_HOLD, "Hold", false)
            .describe("Lets the envelope follower finish its attack before it releases."),
        ParamDescriptor::toggle(EFFECTS_INVERT, "Invert LFO", false)
            .describe("Turns the LFO upside down."),
    ];
    let layout = DeviceLayout::new(LayoutNode::Band(ControlBand::stacked(
        BODY_HEIGHT,
        &[
            (EFFECTS_OUTPUT, Widget::Chooser),
            (EFFECTS_DEPTH, Widget::Knob),
            (EFFECTS_WAVE, Widget::Chooser),
            (EFFECTS_RATE_MODE, Widget::Chooser),
            (EFFECTS_RATE, Widget::Knob),
            (EFFECTS_PERIOD, Widget::Knob),
            (EFFECTS_DIVISION, Widget::Chooser),
        ],
    )))
    .with_expanded(LayoutNode::Band(ControlBand::stacked(
        BODY_HEIGHT,
        &[
            (EFFECTS_STEREO, Widget::Chooser),
            (EFFECTS_PHASE, Widget::Knob),
            (EFFECTS_SPIN, Widget::Knob),
            (EFFECTS_OFFSET, Widget::Knob),
            (EFFECTS_DUTY, Widget::Knob),
            (EFFECTS_MORPH, Widget::Knob),
            (EFFECTS_ATTACK, Widget::Knob),
            (EFFECTS_RELEASE, Widget::Knob),
            (EFFECTS_HOLD, Widget::Toggle),
        ],
    )));
    DeviceType::native(EFFECTS, "Framework Test Effects", DeviceRole::AudioEffect).with_description(
        DeviceDescription::new(params)
            .with_layout(layout)
            .with_menu_options(&[EFFECTS_INVERT]),
    )
}

/// The test instrument's type: the voice section's every part on sine
/// voices, with Hi-Quality as a menu option.
#[must_use]
pub fn instrument_type() -> DeviceType {
    let params = vec![
        instrument::voices(INSTRUMENT_VOICES, INSTRUMENT_MAX_VOICES as i64, 8),
        instrument::voice_mode(INSTRUMENT_VOICE_MODE, false),
        instrument::priority(INSTRUMENT_PRIORITY),
        instrument::legato(INSTRUMENT_LEGATO, false),
        instrument::glide_time(INSTRUMENT_GLIDE_TIME, 4_000.0, 0.0),
        instrument::glide_mode(INSTRUMENT_GLIDE_MODE),
        instrument::glide_prop(INSTRUMENT_GLIDE_PROP),
        instrument::glide_legato(INSTRUMENT_GLIDE_LEGATO, false),
        ParamDescriptor::toggle(INSTRUMENT_USE_SCALE, "Use Current Scale", false)
            .describe("Steps a Glissando through the current scale's degrees."),
        instrument::retrigger(INSTRUMENT_RETRIGGER, false),
        instrument::unison_voices(INSTRUMENT_UNISON_VOICES, INSTRUMENT_MAX_COPIES as i64),
        instrument::unison_detune(INSTRUMENT_UNISON_DETUNE, 100.0, 20.0),
        instrument::unison_spread(INSTRUMENT_UNISON_SPREAD, 0.0),
        instrument::unison_delay(INSTRUMENT_UNISON_DELAY, 200.0),
        instrument::pb_range(INSTRUMENT_PB_RANGE, 96, 2),
        instrument::note_pb(INSTRUMENT_NOTE_PB, true),
        instrument::note_pb_range(INSTRUMENT_NOTE_PB_RANGE, 96, 48),
        instrument::transpose(INSTRUMENT_TRANSPOSE, 48),
        instrument::stretch(INSTRUMENT_STRETCH),
        instrument::error(INSTRUMENT_ERROR),
        instrument::hi_quality(INSTRUMENT_HI_QUALITY),
        instrument::curve_amount(INSTRUMENT_VELOCITY_CURVE, "Velocity Curve"),
        instrument::curve_amount(INSTRUMENT_KEY_CURVE, "Key Curve"),
        ParamDescriptor::choice(INSTRUMENT_OUTPUT, "Output", &INSTRUMENT_OUTPUTS, 0)
            .describe("What the device sends out: its voices, or a modulation source's value."),
        ParamDescriptor::continuous(
            INSTRUMENT_PRESSURE_LEVEL,
            "Pressure Level",
            (0.0, 100.0),
            0.0,
            ParamUnit::Percent,
        )
        .describe("How far a note's pressure lowers its level, read on the control rate."),
        ParamDescriptor::continuous(
            INSTRUMENT_NOTE_LEVEL,
            "Note Level",
            (0.0, 100.0),
            100.0,
            ParamUnit::Percent,
        )
        .describe("Each note's level, which matrix routes move per voice."),
        ParamDescriptor::continuous(
            INSTRUMENT_FINE_TUNE,
            "Fine Tune",
            (-INSTRUMENT_FINE_RANGE, INSTRUMENT_FINE_RANGE),
            0.0,
            ParamUnit::Semitones,
        )
        .describe("Each note's pitch, which matrix routes move per voice every frame."),
        ParamDescriptor::continuous(
            INSTRUMENT_MASTER,
            "Master",
            (0.0, 100.0),
            100.0,
            ParamUnit::Percent,
        )
        .describe("The device's level, which matrix routes scale as a global destination."),
        ParamDescriptor::continuous(
            INSTRUMENT_MATRIX_AMOUNT,
            "Matrix Amount",
            (0.0, 100.0),
            100.0,
            ParamUnit::Percent,
        )
        .describe("Scales every route's amount."),
        route_amount(INSTRUMENT_FREE_AMOUNTS[0], "Slot 1 Amount"),
        route_amount(INSTRUMENT_FREE_AMOUNTS[1], "Slot 2 Amount"),
        route_amount(INSTRUMENT_FREE_AMOUNTS[2], "Slot 3 Amount"),
        route_amount(INSTRUMENT_FREE_AMOUNTS[3], "Slot 4 Amount"),
        ParamDescriptor::choice(INSTRUMENT_CHOSEN_SOURCE, "Mod Source", &CHOOSER_SOURCES, 0)
            .describe("The chosen slot's modulation source."),
        ParamDescriptor::choice(INSTRUMENT_CHOSEN_DEST, "Mod Dest", &CHOOSER_DESTINATIONS, 0)
            .describe("The chosen slot's destination."),
        route_amount(INSTRUMENT_CHOSEN_AMOUNT, "Mod Amount"),
        route_amount(INSTRUMENT_FIXED_AMOUNT, "Pressure Tune Amount"),
    ];
    DeviceType::native(
        INSTRUMENT,
        "Framework Test Instrument",
        DeviceRole::Instrument,
    )
    .with_description(DeviceDescription::new(params).with_menu_options(&[INSTRUMENT_HI_QUALITY]))
    .with_matrix(instrument_matrix())
}

const CHOOSER_SOURCES: [&str; 6] = [
    "Off",
    "Velocity",
    "Key",
    "Pressure",
    "Mod Wheel",
    "Pitch Bend",
];
const CHOOSER_DESTINATIONS: [&str; 4] = ["Off", "Note Level", "Fine Tune", "Master"];

/// A route slot's amount: −100 % to 100 %, 0 % by default.
fn route_amount(id: ParamId, name: &str) -> ParamDescriptor {
    ParamDescriptor::continuous(id, name, (-100.0, 100.0), 0.0, ParamUnit::Percent)
        .describe("How far the route's source moves its destination, in per cent of its travel.")
}

/// The test instrument's matrix (spec: instrument modulation R1, R2): the
/// sources and destinations above, slots 0 to 3 free, slot 4 chosen and
/// slot 5 fixed (Pressure into Fine Tune). Key into Master is refused.
#[must_use]
pub fn instrument_matrix() -> MatrixLayout {
    let source = |name, range, scope| SourceDef { name, range, scope };
    let dest = |name, param, scope, rate, kind| DestDef {
        name,
        param: Some(param),
        scope,
        rate,
        kind,
    };
    let mut slots: Vec<SlotDef> = INSTRUMENT_FREE_AMOUNTS
        .iter()
        .map(|&amount| SlotDef {
            source: Pick::Free,
            dest: Pick::Free,
            amount,
        })
        .collect();
    slots.push(SlotDef {
        source: Pick::Chosen(INSTRUMENT_CHOSEN_SOURCE),
        dest: Pick::Chosen(INSTRUMENT_CHOSEN_DEST),
        amount: INSTRUMENT_CHOSEN_AMOUNT,
    });
    slots.push(SlotDef {
        source: Pick::Fixed(2),
        dest: Pick::Fixed(1),
        amount: INSTRUMENT_FIXED_AMOUNT,
    });
    MatrixLayout {
        sources: vec![
            source("Velocity", MatrixRange::Unipolar, MatrixScope::Voice),
            source("Key", MatrixRange::Bipolar, MatrixScope::Voice),
            source("Pressure", MatrixRange::Unipolar, MatrixScope::Voice),
            source("Mod Wheel", MatrixRange::Unipolar, MatrixScope::Global),
            source("Pitch Bend", MatrixRange::Bipolar, MatrixScope::Global),
        ],
        dests: vec![
            dest(
                "Note Level",
                INSTRUMENT_NOTE_LEVEL,
                MatrixScope::Voice,
                MatrixRate::Control,
                MatrixKind::Additive,
            ),
            dest(
                "Fine Tune",
                INSTRUMENT_FINE_TUNE,
                MatrixScope::Voice,
                MatrixRate::Audio,
                MatrixKind::Additive,
            ),
            dest(
                "Master",
                INSTRUMENT_MASTER,
                MatrixScope::Global,
                MatrixRate::Control,
                MatrixKind::Multiplicative,
            ),
        ],
        slots,
        forbidden: vec![(1, 2)],
    }
}

/// Every test device type.
#[must_use]
pub fn device_types() -> Vec<DeviceType> {
    vec![
        utility_type(),
        drive_type(),
        scope_type(),
        spectrogram_type(),
        effects_type(),
        instrument_type(),
        pages_type(),
    ]
}

/// Adds the test devices to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, utility_type(), |_, _| Utility);
    add(registry, drive_type(), |_, config| Drive::new(config));
    add(registry, scope_type(), |_, _| Scope);
    add(registry, spectrogram_type(), |_, config| {
        Spectrogram::new(config)
    });
    add(registry, effects_type(), Effects::new);
    add(registry, instrument_type(), TestInstrument::new);
    add(registry, pages_type(), |_, _| Scope);
}

/// The engine's own devices and the test devices.
#[must_use]
pub fn registry() -> Registry {
    let mut registry = Registry::new();
    register(&mut registry);
    registry
}

/// Gain, balance, mute, and polarity.
struct Utility;

impl NativeDevice for Utility {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        let mute = params.value(2) >= 0.5;
        let sign = if params.value(3) >= 0.5 { -1.0 } else { 1.0 };
        for channel in 0..audio.channels() {
            let (gain, balance) = (params.ramp(0), params.ramp(1));
            let left = channel % 2 == 0;
            for (i, sample) in audio.channel_mut(channel).iter_mut().enumerate() {
                let b = balance[i] / 50.0;
                let side = if left {
                    (1.0 - b).min(1.0)
                } else {
                    (1.0 + b).min(1.0)
                };
                *sample *= if mute { 0.0 } else { sign * gain[i] * side };
            }
        }
    }
}

/// A `tanh` saturator with its own oversampler for each factor.
struct Drive {
    oversamplers: [Oversampler; 4],
    /// The oversampling in use: doublings.
    stages: usize,
}

impl Drive {
    fn new(config: &NodeConfig) -> Self {
        let make = |stages| Oversampler::new(stages, config.channels, config.max_frames);
        Self {
            oversamplers: [make(0), make(1), make(2), make(3)],
            stages: 2,
        }
    }
}

impl NativeDevice for Drive {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        self.stages = (params.value(2) as usize).min(3);
        let oversampler = &mut self.oversamplers[self.stages];
        let factor = oversampler.factor();
        let (drive, output) = (params.ramp(0), params.ramp(1));
        for channel in 0..audio.channels() {
            let samples = audio.channel_mut(channel);
            // Pieces the oversampler takes may be shorter than the
            // stretch; count frames across them.
            let mut frame = 0;
            oversampler.process(channel, samples, |high| {
                for (i, sample) in high.iter_mut().enumerate() {
                    let d = drive[(frame + i / factor).min(drive.len() - 1)];
                    *sample = (*sample * d).tanh();
                }
                frame += high.len() / factor;
            });
            for (sample, gain) in samples.iter_mut().zip(output) {
                *sample *= gain;
            }
        }
    }

    fn latency(&self) -> u32 {
        // The latency of the factor in use; a change reaches delay
        // compensation with WP-2.11.
        self.oversamplers[self.stages].latency()
    }
}

/// Passes audio through: its parameters only change its display.
struct Scope;

impl NativeDevice for Scope {
    fn process(&mut self, _audio: &mut Audio<'_, '_>, _params: &ParamBank) {}
}

/// Passes audio through and publishes a spectrogram of it: a column of the
/// newest frame's magnitudes in dB after each frame of the STFT.
struct Spectrogram {
    stft: Stft,
    /// The bins each row covers.
    rows: Vec<(usize, usize)>,
    /// The columns, a ring of `rows` values each; `head` is the next to write
    /// and so the oldest.
    grid: Vec<f32>,
    head: usize,
}

impl Spectrogram {
    fn new(config: &NodeConfig) -> Self {
        let grid = SPECTROGRAM_GRID;
        let stft = Stft::new(SPECTROGRAM_FRAME, SPECTROGRAM_OVERLAP, Window::Hann)
            .expect("a frame size and overlap the STFT offers");
        Self {
            rows: stft::row_bins(
                grid.rows,
                grid.frequencies[0],
                grid.frequencies[1],
                grid.log_frequency,
                SPECTROGRAM_FRAME,
                config.sample_rate.as_f64(),
            ),
            stft,
            grid: vec![grid.levels[0]; grid.columns * grid.rows],
            head: 0,
        }
    }
}

impl NativeDevice for Spectrogram {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        let frozen = params.value(SPECTROGRAM_FREEZE.0 as usize) >= 0.5;
        let Self {
            stft,
            rows,
            grid,
            head,
        } = self;
        let scale = stft.amplitude_scale();
        let (frames, channels) = (audio.frames(), audio.channels());
        for i in 0..frames {
            let sample = (0..channels).map(|c| audio.channel_mut(c)[i]).sum::<f32>()
                / channels.max(1) as f32;
            // The resynthesis isn't used: the audio passes through as it is.
            stft.process_sample(sample, |spectrum| {
                if frozen {
                    return;
                }
                let column = &mut grid[*head * rows.len()..][..rows.len()];
                for (cell, &(from, to)) in column.iter_mut().zip(rows.iter()) {
                    let loudest = spectrum[from..=to]
                        .iter()
                        .map(|bin| bin.norm())
                        .fold(0.0, f32::max);
                    *cell = 20.0 * (loudest * scale).max(1e-9).log10();
                }
                *head = (*head + 1) % SPECTROGRAM_GRID.columns;
            });
        }
    }

    fn display_len(&self) -> usize {
        self.grid.len()
    }

    fn display(&mut self, out: &mut [f32]) {
        // Oldest column first.
        let (newer, older) = self.grid.split_at(self.head * self.rows.len());
        let (first, second) = out.split_at_mut(older.len());
        first.copy_from_slice(older);
        second.copy_from_slice(newer);
    }
}

/// The effects' shared parts, each used: the LFO moves the audio or is the
/// output; the envelope follower is the output or not; the LFO's random
/// shapes follow the device's own seed.
struct Effects {
    lfo: Lfo,
    followers: Vec<EnvelopeFollower>,
}

impl Effects {
    fn new(device: &Device, config: &NodeConfig) -> Self {
        Self {
            lfo: Lfo::new(config.sample_rate, device.id.0.get()),
            followers: (0..config.channels)
                .map(|_| EnvelopeFollower::new(config.sample_rate.as_f64()))
                .collect(),
        }
    }
}

impl NativeDevice for Effects {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        let at = |id: ParamId| params.index(id).expect("the description lists it");
        let (output, mode) = (
            params.value(at(EFFECTS_OUTPUT)) as usize,
            params.value(at(EFFECTS_RATE_MODE)) as usize,
        );
        let rate = match mode {
            0 => Rate::Hertz(params.target(at(EFFECTS_RATE))),
            1 => Rate::Seconds(params.target(at(EFFECTS_PERIOD))),
            _ => Rate::Beats(DIVISIONS[(params.value(at(EFFECTS_DIVISION)) as usize).min(5)]),
        };
        let stereo = if params.value(at(EFFECTS_STEREO)) >= 0.5 {
            Stereo::Spin(params.target(at(EFFECTS_SPIN)))
        } else {
            Stereo::Phase(params.target(at(EFFECTS_PHASE)))
        };
        self.lfo
            .set(audio, rate, stereo, params.target(at(EFFECTS_OFFSET)));
        for follower in &mut self.followers {
            follower.set(
                params.target(at(EFFECTS_ATTACK)) / 1_000.0,
                params.target(at(EFFECTS_RELEASE)) / 1_000.0,
                params.value(at(EFFECTS_HOLD)) >= 0.5,
            );
        }
        let wave = params.value(at(EFFECTS_WAVE)) as usize;
        let sign = if params.value(at(EFFECTS_INVERT)) >= 0.5 {
            -1.0
        } else {
            1.0
        };
        let (depth, duty, morph) = (
            params.ramp(at(EFFECTS_DEPTH)),
            params.ramp(at(EFFECTS_DUTY)),
            params.ramp(at(EFFECTS_MORPH)),
        );
        let tempo = audio.tempo();
        let channels = audio.channels();
        for i in 0..audio.frames() {
            let shape = Shape {
                wave: Wave::from_index(wave),
                morph: Some((Wave::from_index(wave + 1), f64::from(morph[i]) / 100.0)),
                duty: f64::from(duty[i]) / 100.0,
            };
            let lfo = self.lfo.step(tempo, i, shape);
            for channel in 0..channels {
                let value = sign * lfo[channel % 2];
                let sample = &mut audio.channel_mut(channel)[i];
                let level = self.followers[channel].process(*sample);
                *sample = match output {
                    0 => *sample * (1.0 - f64::from(depth[i]) / 200.0 * (1.0 - value)) as f32,
                    1 => value as f32,
                    _ => level as f32,
                };
            }
        }
    }
}

/// Sine voices on the voice section, each with its unison copies, under an
/// envelope that restarts from its level; or one source of the voices as a
/// steady value.
struct TestInstrument {
    section: Section,
    voices: Voices,
}

/// The sound engine the section drives.
struct Voices {
    rate: f64,
    envelopes: EnvelopeBank,
    levels: Vec<f32>,
    phases: Vec<f64>,
    /// Each voice's level factor from Pressure, held between the control
    /// rate's points.
    held: [f32; MAX_VOICES],
    out: [Vec<f32>; 2],
    output: usize,
    pressure_level: f32,
    velocity_curve: f64,
    key_curve: f64,
    /// The modulation matrix, and the knobs its destinations move.
    matrix: Matrix,
    /// The values those knobs play, frame by frame (spec: instrument
    /// modulation R5): automation and modulators already applied.
    note_level: Vec<f32>,
    fine_tune: Vec<f32>,
    master: Vec<f32>,
}

/// The envelope's lanes per frame.
const STRIDE: usize = MAX_VOICES.next_multiple_of(16);

impl TestInstrument {
    fn new(device: &Device, config: &NodeConfig) -> Self {
        let rate = config.sample_rate.as_f64();
        let frames = config.max_frames.max(1);
        let params = EnvelopeParams {
            attack: 5.0,
            decay: 20.0,
            sustain: 0.5,
            release: 50.0,
            steal_restart: StealRestart::CurrentLevel,
            ..EnvelopeParams::AMP
        };
        Self {
            section: Section::new(
                Parts {
                    voices: Some(INSTRUMENT_VOICES),
                    voice_mode: Some(INSTRUMENT_VOICE_MODE),
                    priority: Some(INSTRUMENT_PRIORITY),
                    legato: Some(INSTRUMENT_LEGATO),
                    glide_time: Some(INSTRUMENT_GLIDE_TIME),
                    glide_mode: Some(INSTRUMENT_GLIDE_MODE),
                    glide_prop: Some(INSTRUMENT_GLIDE_PROP),
                    glide_legato: Some(INSTRUMENT_GLIDE_LEGATO),
                    poly_glide: true,
                    use_scale: Some(INSTRUMENT_USE_SCALE),
                    retrigger: Some(INSTRUMENT_RETRIGGER),
                    unison_voices: Some(INSTRUMENT_UNISON_VOICES),
                    unison_detune: Some(INSTRUMENT_UNISON_DETUNE),
                    unison_spread: Some(INSTRUMENT_UNISON_SPREAD),
                    unison_delay: Some(INSTRUMENT_UNISON_DELAY),
                    pb_range: Some(INSTRUMENT_PB_RANGE),
                    note_pb: Some(INSTRUMENT_NOTE_PB),
                    note_range: NoteRange::Own(INSTRUMENT_NOTE_PB_RANGE),
                    transpose: Some(INSTRUMENT_TRANSPOSE),
                    stretch: Some(INSTRUMENT_STRETCH),
                    error: Some(INSTRUMENT_ERROR),
                    hi_quality: Some(INSTRUMENT_HI_QUALITY),
                    ..Parts::default()
                },
                INSTRUMENT_MAX_VOICES,
                INSTRUMENT_MAX_COPIES,
                rate,
                0x7E57,
            ),
            voices: Voices {
                rate,
                envelopes: EnvelopeBank::new(MAX_VOICES, params, Timing::new(rate)),
                levels: vec![0.0; frames * STRIDE],
                phases: vec![0.0; MAX_VOICES * INSTRUMENT_MAX_COPIES],
                held: [1.0; MAX_VOICES],
                out: [vec![0.0; frames], vec![0.0; frames]],
                output: 0,
                pressure_level: 0.0,
                velocity_curve: 0.0,
                key_curve: 0.0,
                matrix: Matrix::of(
                    instrument_type()
                        .matrix
                        .expect("the test instrument has a matrix"),
                    device,
                ),
                note_level: vec![100.0; frames],
                fine_tune: vec![0.0; frames],
                master: vec![100.0; frames],
            },
        }
    }
}

impl NativeDevice for TestInstrument {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        let frames = audio.frames();
        let value = |id: ParamId| params.value(params.index(id).expect("the description lists it"));
        let engine = &mut self.voices;
        engine.output = value(INSTRUMENT_OUTPUT) as usize;
        engine.pressure_level = (value(INSTRUMENT_PRESSURE_LEVEL) / 100.0) as f32;
        engine.velocity_curve = value(INSTRUMENT_VELOCITY_CURVE) / 100.0;
        engine.key_curve = value(INSTRUMENT_KEY_CURVE) / 100.0;
        let ramp = |id: ParamId| params.ramp(params.index(id).expect("the description lists it"));
        engine.note_level[..frames].copy_from_slice(ramp(INSTRUMENT_NOTE_LEVEL));
        engine.fine_tune[..frames].copy_from_slice(ramp(INSTRUMENT_FINE_TUNE));
        engine.master[..frames].copy_from_slice(ramp(INSTRUMENT_MASTER));
        engine
            .matrix
            .set_amount(value(INSTRUMENT_MATRIX_AMOUNT) / 100.0);
        engine.matrix.begin(params);
        engine.out[0][..frames].fill(0.0);
        engine.out[1][..frames].fill(0.0);
        self.section.process(audio, params, engine);
        for channel in 0..audio.channels() {
            audio
                .channel_mut(channel)
                .copy_from_slice(&engine.out[channel.min(1)][..frames]);
        }
    }

    fn flush(&mut self, _events: &[bl_engine::NodeEvent], _params: &ParamBank) {
        self.section.reset();
        for voice in 0..MAX_VOICES {
            self.voices.envelopes.reset(voice);
        }
    }

    fn matrix_played(&self, report: &mut dyn FnMut(ParamId, f64)) {
        let reach = INSTRUMENT_FINE_RANGE;
        for (dest, param, low, high) in [
            (0, INSTRUMENT_NOTE_LEVEL, 0.0, 100.0),
            (1, INSTRUMENT_FINE_TUNE, -reach, reach),
            (2, INSTRUMENT_MASTER, 0.0, 100.0),
        ] {
            if let Some(position) = self.voices.matrix.shown(dest) {
                report(param, low + position * (high - low));
            }
        }
    }

    fn receive_data(&mut self, data: NodeData) {
        self.section.receive_data(&data);
        if let Some(routes) = data.downcast_ref::<bl_engine::ModRoutes>() {
            self.voices.matrix.set_free(routes.0);
        }
    }
}

impl Instrument for Voices {
    fn voice_event(&mut self, voices: &mut VoiceBlock, state: &State, event: VoiceEvent) {
        let voice = event.voice;
        match event.kind {
            VoiceEventKind::Start { start, .. } => {
                let sources = Sources::of(voices);
                self.matrix.start(voice);
                self.held[voice] = 1.0 - self.pressure_level * sources.pressure(voice) as f32;
                match start {
                    Start::Fresh => {
                        self.envelopes
                            .trigger(voice, lspk_dsp::envelope::Trigger::New);
                        let copies = INSTRUMENT_MAX_COPIES;
                        self.phases[voice * copies..(voice + 1) * copies].fill(0.0);
                    }
                    Start::Steal | Start::Retrigger => self
                        .envelopes
                        .trigger(voice, lspk_dsp::envelope::Trigger::Retrigger),
                    Start::Legato => self
                        .envelopes
                        .trigger(voice, lspk_dsp::envelope::Trigger::Legato),
                    Start::Join { leader } => self.envelopes.join(voice, leader),
                }
                let _ = state;
            }
            VoiceEventKind::Release => self.envelopes.release(voice),
            VoiceEventKind::End { .. } => self.envelopes.reset(voice),
        }
    }

    fn render(&mut self, voices: &mut VoiceBlock, state: &State, frames: core::ops::Range<usize>) {
        let count = frames.len();
        if count == 0 {
            return;
        }
        self.envelopes.process(&mut self.levels[..count * STRIDE]);
        let config = *state.config();
        let sources = Sources::of(voices);
        if self.output != 0 {
            self.read_source(voices, state, &sources, &frames);
        } else {
            self.sound(voices, state, &sources, &frames);
        }
        let mut mask = voices.active_mask();
        while mask != 0 {
            let voice = mask.trailing_zeros() as usize;
            mask &= mask - 1;
            if self.envelopes.is_idle(voice) {
                voices.end(voice);
            }
        }
        let _ = config;
    }
}

/// What the matrix's sources read: the voices' (spec: instrument voices
/// R15), in the order of [`INSTRUMENT_SOURCES`].
fn reader<'a>(
    sources: &'a Sources<'a>,
    curves: (f64, f64),
) -> impl Fn(usize, usize, usize) -> f64 + 'a {
    move |source, voice, _frame| match source {
        0 => instrument::curve(sources.velocity(voice), curves.0),
        1 => 2.0 * instrument::curve((sources.key(voice) + 1.0) / 2.0, curves.1) - 1.0,
        2 => sources.pressure(voice),
        3 => sources.mod_wheel(),
        _ => sources.pitch_bend(),
    }
}

impl Voices {
    /// Every active voice's copies as sines, mixed into the two outputs,
    /// each voice's level and pitch moved by the matrix's routes (spec:
    /// instrument modulation R3 to R7).
    fn sound(
        &mut self,
        voices: &VoiceBlock,
        state: &State,
        sources: &Sources<'_>,
        frames: &core::ops::Range<usize>,
    ) {
        let config = *state.config();
        let copies = config.copies;
        let read = reader(sources, (self.velocity_curve, self.key_curve));
        self.matrix.set_oldest(state.oldest(voices));
        self.matrix.set_newest(state.newest(voices));
        for voice in voices.active_voices() {
            let gain = INSTRUMENT_LEVEL * sources.velocity(voice) as f32;
            for frame in 0..frames.len() {
                let at = frames.start + frame;
                let due = state.control_due(at);
                if due {
                    self.held[voice] = 1.0 - self.pressure_level * sources.pressure(voice) as f32;
                }
                let travel =
                    |value: f32, low: f64, high: f64| (f64::from(value) - low) / (high - low);
                let note_level = self.matrix.position(
                    0,
                    travel(self.note_level[at], 0.0, 100.0),
                    voice,
                    at,
                    due,
                    &read,
                );
                let reach = INSTRUMENT_FINE_RANGE;
                let fine = self.matrix.position(
                    1,
                    travel(self.fine_tune[at], -reach, reach),
                    voice,
                    at,
                    due,
                    &read,
                );
                let fine = (fine * 2.0 - 1.0) * reach;
                let master = self.matrix.position(
                    2,
                    travel(self.master[at], 0.0, 100.0),
                    voice,
                    at,
                    due,
                    &read,
                );
                let level = self.levels[frame * STRIDE + voice]
                    * gain
                    * self.held[voice]
                    * note_level as f32
                    * master as f32;
                let pitch = state.pitch(voices, voice, frames, frame) + fine;
                let age = state.frames_since_start(voice, frames.start + frame);
                for copy in 0..copies {
                    let phase = &mut self.phases[voice * INSTRUMENT_MAX_COPIES + copy];
                    let cents = f64::from(instrument::copy_cents(copy, copies, config.detune));
                    let hz = 440.0 * ((pitch + cents / 100.0 - 69.0) / 12.0).exp2();
                    *phase = (*phase + hz / self.rate).fract();
                    let delay =
                        instrument::copy_delay_frames(copy, copies, config.delay_ms, self.rate);
                    if age < delay {
                        continue;
                    }
                    let (left, right) = instrument::copy_gains(copy, copies, config.spread);
                    let sample = (TAU * *phase).sin() as f32 * level;
                    self.out[0][frames.start + frame] += sample * left;
                    self.out[1][frames.start + frame] += sample * right;
                }
            }
        }
    }

    /// The chosen source as a steady value on both outputs: the lowest
    /// sounding voice's, or the global one; or a voice's pitch, frame by
    /// frame.
    fn read_source(
        &mut self,
        voices: &VoiceBlock,
        state: &State,
        sources: &Sources<'_>,
        frames: &core::ops::Range<usize>,
    ) {
        let mask = voices.active_mask();
        let (lowest, highest) = (
            (mask != 0).then(|| mask.trailing_zeros() as usize),
            (mask != 0).then(|| 31 - mask.leading_zeros() as usize),
        );
        if self.output >= 13 {
            let read = reader(sources, (self.velocity_curve, self.key_curve));
            self.matrix.set_oldest(state.oldest(voices));
            self.matrix.set_newest(state.newest(voices));
            let dest = self.output - 13;
            for frame in 0..frames.len() {
                let at = frames.start + frame;
                let value = lowest.map_or(0.0, |voice| {
                    let (knob, low, high) = match dest {
                        0 => (self.note_level[at], 0.0, 100.0),
                        1 => (
                            self.fine_tune[at],
                            -INSTRUMENT_FINE_RANGE,
                            INSTRUMENT_FINE_RANGE,
                        ),
                        _ => (self.master[at], 0.0, 100.0),
                    };
                    let base = (f64::from(knob) - low) / (high - low);
                    self.matrix
                        .position(dest, base, voice, at, state.control_due(at), &read)
                });
                self.out[0][at] = value as f32;
                self.out[1][at] = value as f32;
            }
            return;
        }
        if self.output >= 11 {
            let voice = if self.output == 11 { lowest } else { highest };
            for frame in 0..frames.len() {
                let pitch = voice.map_or(0.0, |v| state.pitch(voices, v, frames, frame));
                self.out[0][frames.start + frame] = pitch as f32;
                self.out[1][frames.start + frame] = pitch as f32;
            }
            return;
        }
        let per_voice = |read: &dyn Fn(usize) -> f64| lowest.map_or(0.0, read);
        let value = match self.output {
            1 => per_voice(&|v| instrument::curve(sources.velocity(v), self.velocity_curve)),
            2 => per_voice(&|v| sources.release_velocity(v)),
            3 => per_voice(&|v| {
                2.0 * instrument::curve((sources.key(v) + 1.0) / 2.0, self.key_curve) - 1.0
            }),
            4 => per_voice(&|v| state.random(v)),
            5 => per_voice(&|v| sources.pressure(v)),
            6 => per_voice(&|v| sources.slide(v)),
            7 => per_voice(&|v| state.note_pb(v)),
            8 => sources.mod_wheel(),
            9 => sources.pitch_bend(),
            _ => sources.aftertouch(),
        } as f32;
        self.out[0][frames.clone()].fill(value);
        self.out[1][frames.clone()].fill(value);
    }
}

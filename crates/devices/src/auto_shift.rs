//! Auto Shift (WP-8.46; the Auto Shift spec): pitch correction and voice
//! shifting. A `bl_dsp::pitch` tracker follows a monophonic voice and
//! pitch-synchronous grains move it: in the Quantizer tab to the nearest note
//! of a scale, in the MIDI tab to the notes of a MIDI track, one voice or up
//! to eight.
//!
//! The manual gives the controls and some ranges but no laws, latencies or
//! defaults, so those are Beatlayer's own (`docs/specs/auto-shift.md`).
//!
//! One tracker per Pitch Range hears the input, and each of eight shifters
//! of the range lays its grains from the tracker's readings, so every grain
//! is steered from the pitch measured where it sits ([`Grain`]): the
//! correction needs no look-ahead of its own. All of a range's tracker and
//! shifters are made with the node; only the chosen range's runs. Everything
//! else (the LFO, the vibrato, the notes, the envelopes) is worked out per
//! sample from counts of frames, so a render doesn't depend on how the host
//! cuts its blocks.
//!
//! The voice is tracked and shifted as the mean of the channels; the dry
//! signal stays in stereo, delayed to line up with the shifted one.

use core::f64::consts::{FRAC_PI_4, SQRT_2};

use bl_dsp::Random;
use bl_dsp::pitch::{Ask, Config, Grain, Tracker, VoiceShifter};
use bl_engine::{DeviceType, NodeConfig, NoteEvent, NoteExpression, NoteKind, Registry};
use bl_model::device_layout::{
    BODY_HEIGHT, ControlBand, DisplayCell, LayoutNode, Page, PageStyle, PagesNode, SEGMENT_WIDTH,
    TAB_HEIGHT, TitlePart, Widget,
};
use bl_model::{
    Device, DeviceDescription, DeviceDisplay, DeviceId, DeviceLayout, DeviceRole, DisplayMarker,
    DisplayReading, DisplaySeries, OrdMap, ParamDescriptor, ParamId, ParamUnit, Scale, ScaleName,
};

use crate::framework::{Audio, DelayLine, NativeDevice, ParamBank, add};
use crate::lfo::{Lfo, Rate, Shape, Stereo, Wave};
use crate::midi_fx::use_current_scale;

/// Auto Shift's device type.
pub const AUTO_SHIFT: &str = "beatlayer.auto_shift";

/// Use Current Scale, in the title bar (scales R16).
pub const USE_SCALE: ParamId = ParamId(0);
/// Live Mode: half the latency, with glitches for the lowest notes of the
/// range and at onsets (R3).
pub const LIVE_MODE: ParamId = ParamId(1);
/// MIDI On: the notes of the MIDI source set the pitch, and the MIDI tab
/// replaces the Quantizer tab (R9).
pub const MIDI_ON: ParamId = ParamId(2);
/// Pitch Range: one of [`PITCH_RANGES`] (R2).
pub const PITCH_RANGE: ParamId = ParamId(3);
/// Input Gain: the level into the tracker and the shifter.
pub const INPUT_GAIN: ParamId = ParamId(4);
/// Correction Strength: the share of the distance to the target that the
/// correction covers (R6).
pub const STRENGTH: ParamId = ParamId(5);
/// Smooth: moves the target over Smoothing Time.
pub const SMOOTH: ParamId = ParamId(6);
/// Smoothing Time, in milliseconds.
pub const SMOOTHING_TIME: ParamId = ParamId(7);
/// Quantizer Pitch Shift: moves each target that many degrees of the scale
/// (R7).
pub const DEGREES: ParamId = ParamId(8);
/// Root of the scale the Quantizer corrects to.
pub const ROOT: ParamId = ParamId(9);
/// Scale of the scale the Quantizer corrects to.
pub const SCALE: ParamId = ParamId(10);
/// Key C: the first of the twelve included-note keys, each leaving a note out
/// of the scale or bringing it back in (R8); the others follow in order up to
/// B, a parameter each.
pub const KEY_C: ParamId = ParamId(11);
/// Pitch Shift, in semitones, after correction (R13).
pub const PITCH_SHIFT: ParamId = ParamId(23);
/// Pitch Shift Fine, in cents.
pub const FINE: ParamId = ParamId(24);
/// Formant Shift, in per cent of an octave.
pub const FORMANT_SHIFT: ParamId = ParamId(25);
/// Formant Follow: how far the formants move with the pitch's shift and
/// correction.
pub const FORMANT_FOLLOW: ParamId = ParamId(26);
/// Vibrato Amount, in cents peak to peak (R14).
pub const VIBRATO_AMOUNT: ParamId = ParamId(27);
/// Vibrato Rate, in hertz.
pub const VIBRATO_RATE: ParamId = ParamId(28);
/// Vibrato Fade In, in milliseconds from each onset.
pub const VIBRATO_FADE: ParamId = ParamId(29);
/// Natural Vibrato: the rate and the depth wander.
pub const NATURAL_VIBRATO: ParamId = ParamId(30);
/// Dry/Wet (R15).
pub const DRY_WET: ParamId = ParamId(31);
/// Mono/Poly: one of [`VOICE_MODES`] (R9).
pub const MONO_POLY: ParamId = ParamId(32);
/// Voice Count: one of [`VOICE_COUNTS`], Poly's voices.
pub const VOICE_COUNT: ParamId = ParamId(33);
/// Glide: the time overlapping Mono notes take to slide, in milliseconds.
pub const GLIDE: ParamId = ParamId(34);
/// Attack Time, in milliseconds (R10).
pub const ATTACK: ParamId = ParamId(35);
/// Release Time, in milliseconds.
pub const RELEASE: ParamId = ParamId(36);
/// Note Latch: a voice holds its pitch after the note-off until the next
/// note.
pub const NOTE_LATCH: ParamId = ParamId(37);
/// Pitch Bend Range, in semitones.
pub const BEND_RANGE: ParamId = ParamId(38);
/// Pitch Mod Source: the first of four routings, each a source (one of
/// [`SOURCES`]) and a depth: Pitch, Formant, Volume and Pan, at the ids
/// `PITCH_SOURCE + 2 * target` and the depth one above (R11).
pub const PITCH_SOURCE: ParamId = ParamId(39);
/// Pitch Mod Depth.
pub const PITCH_DEPTH: ParamId = ParamId(40);
/// Formant Mod Source.
pub const FORMANT_SOURCE: ParamId = ParamId(41);
/// Formant Mod Depth.
pub const FORMANT_DEPTH: ParamId = ParamId(42);
/// Volume Mod Source.
pub const VOLUME_SOURCE: ParamId = ParamId(43);
/// Volume Mod Depth.
pub const VOLUME_DEPTH: ParamId = ParamId(44);
/// Pan Mod Source.
pub const PAN_SOURCE: ParamId = ParamId(45);
/// Pan Mod Depth.
pub const PAN_DEPTH: ParamId = ParamId(46);
/// LFO Waveform: one of [`WAVES`] (R12).
pub const LFO_WAVE: ParamId = ParamId(47);
/// LFO Sync: the rate is a beat division.
pub const LFO_SYNC: ParamId = ParamId(48);
/// LFO Rate, in milliseconds per cycle.
pub const LFO_RATE: ParamId = ParamId(49);
/// LFO Division: the synced rate, one of [`division_names`].
pub const LFO_DIVISION: ParamId = ParamId(50);
/// LFO Delay, in milliseconds.
pub const LFO_DELAY: ParamId = ParamId(51);
/// LFO Attack, in milliseconds.
pub const LFO_ATTACK: ParamId = ParamId(52);
/// LFO Reset: restarts the LFO at each onset.
pub const LFO_RESET: ParamId = ParamId(53);
/// LFO Pitch Depth: the first of the four depths, Pitch, Formant, Volume and
/// Pan, at consecutive ids.
pub const LFO_PITCH: ParamId = ParamId(54);
/// LFO Formant Depth.
pub const LFO_FORMANT: ParamId = ParamId(55);
/// LFO Volume Depth.
pub const LFO_VOLUME: ParamId = ParamId(56);
/// LFO Pan Depth.
pub const LFO_PAN: ParamId = ParamId(57);

/// How many parameters the device has.
const PARAMS: usize = 58;

/// The Pitch Range choices, as the manual names them, and the lowest and
/// highest pitch each looks for, in hertz (R2).
pub const PITCH_RANGES: [&str; 3] = ["High", "Mid", "Bass"];
pub const RANGE_HERTZ: [(f64, f64); 3] = [(100.0, 1_500.0), (70.0, 1_000.0), (40.0, 500.0)];
/// The Mono/Poly choices.
pub const VOICE_MODES: [&str; 2] = ["Mono", "Poly"];
/// The Voice Count choices, as numbers of voices.
pub const VOICE_COUNTS: [usize; 3] = [2, 4, 8];
/// The modulation sources of the MIDI tab.
pub const SOURCES: [&str; 6] = [
    "Velocity",
    "Pressure",
    "Mod Wheel",
    "Pitch Bend",
    "Note PB",
    "Slide",
];
/// The LFO's waves, under the manual's names.
pub const WAVES: [&str; 9] = [
    "Sine",
    "Triangle",
    "Triangle 8",
    "Triangle 16",
    "Saw Up",
    "Saw Down",
    "Rectangle",
    "Random",
    "Random S&H",
];
const WAVE_OF: [Wave; 9] = [
    Wave::Sine,
    Wave::Triangle,
    Wave::Triangle8,
    Wave::Triangle16,
    Wave::SawUp,
    Wave::SawDown,
    Wave::Rectangle,
    Wave::Random,
    Wave::SampleHold,
];

/// The display's name, its two traces and the ruler's marks.
pub const DISPLAY: &str = "Pitch";
pub const DETECTED: &str = "Detected";
pub const CORRECTED: &str = "Corrected";
/// The columns the display holds, over the last [`HISTORY_SECONDS`].
pub const COLUMNS: usize = 128;
const HISTORY_SECONDS: f64 = 4.0;
/// The notes the display shows, as keys from C-2 at 0 (so 24 is C0), and the
/// value of a column that holds no pitch: below them.
pub const NOTE_RANGE: [f32; 2] = [24.0, 96.0];
const NO_PITCH: f32 = 23.0;
/// The values after the display's two traces: voiced, detected note, target
/// note, correction in cents, latency in milliseconds, the LFO's lamp, the
/// scale's mask, MIDI On, and the three range lamps.
const TAIL: usize = 11;
const DISPLAY_LEN: usize = 2 * COLUMNS + TAIL;

/// The voices in the pool, made with the node (R20).
pub const VOICES: usize = 8;
/// How long the LFO's lamp stays lit, in seconds.
const LAMP_SECONDS: f64 = 0.05;
/// How long Natural Vibrato takes to move between two random values, in
/// seconds, and how far it moves the rate and the depth (R14).
const WANDER_SECONDS: f64 = 0.3;
const WANDER: f64 = 0.1;
/// The keys of notes held at once that Mono remembers.
const HELD_KEYS: usize = 16;
/// The most notes and messages one stretch of a block takes.
const EVENT_ROOM: usize = 4_096;

/// The synced divisions the LFO offers: 1/64 note up to 8 bars (R12).
fn division(index: usize) -> (&'static str, f64) {
    const DIVISIONS: [(&str, f64); 18] = [
        ("1/64", 0.0625),
        ("1/32", 0.125),
        ("1/16T", 1.0 / 6.0),
        ("1/16", 0.25),
        ("1/16.", 0.375),
        ("1/8T", 1.0 / 3.0),
        ("1/8", 0.5),
        ("1/8.", 0.75),
        ("1/4T", 2.0 / 3.0),
        ("1/4", 1.0),
        ("1/4.", 1.5),
        ("1/2T", 4.0 / 3.0),
        ("1/2", 2.0),
        ("1/2.", 3.0),
        ("1 bar", 4.0),
        ("2 bars", 8.0),
        ("4 bars", 16.0),
        ("8 bars", 32.0),
    ];
    DIVISIONS[index.min(DIVISIONS.len() - 1)]
}

/// The names of the LFO's divisions, in the chooser's order.
#[must_use]
pub fn division_names() -> Vec<&'static str> {
    (0..18).map(|n| division(n).0).collect()
}

/// The division a new device's LFO starts on: 1/4.
const DEFAULT_DIVISION: usize = 9;

/// The name of the key at `note` (60 is C3, as Live names it).
#[must_use]
pub fn note_name(note: i32) -> String {
    const NAMES: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    format!(
        "{}{}",
        NAMES[note.rem_euclid(12) as usize],
        note.div_euclid(12) - 2
    )
}

/// The key a pitch in hertz is on, as a number of semitones from key 0 with A
/// at 69 = 440 Hz.
fn note_of(hz: f64) -> f64 {
    69.0 + 12.0 * (hz / 440.0).log2()
}

/// The frequency of key `note`.
fn hz_of(note: f64) -> f64 {
    440.0 * ((note - 69.0) / 12.0).exp2()
}

/// Auto Shift's type.
#[must_use]
pub fn auto_shift_type() -> DeviceType {
    let counts: Vec<String> = VOICE_COUNTS.iter().map(ToString::to_string).collect();
    let counts: Vec<&str> = counts.iter().map(String::as_str).collect();
    let roots: Vec<&str> = (0..12u8).map(Scale::root_name).collect();
    let scales: Vec<&str> = ScaleName::ALL.iter().map(|name| name.label()).collect();
    let divisions = division_names();
    let mut params = vec![
        ParamDescriptor::toggle(USE_SCALE, "Use Current Scale", false).describe(
            "Corrects to the scale of the clip playing on the track instead of Root and Scale.",
        ),
        ParamDescriptor::toggle(LIVE_MODE, "Live Mode", false).describe(
            "Halves the latency, at the price of glitches at onsets and for the lowest notes of \
             the range.",
        ),
        ParamDescriptor::toggle(MIDI_ON, "MIDI On", false).describe(
            "The notes of the device's MIDI source set the pitch, and the MIDI tab replaces the \
             Quantizer tab.",
        ),
        ParamDescriptor::choice(PITCH_RANGE, "Pitch Range", &PITCH_RANGES, 1).describe(
            "The pitches the tracker looks for: High 100 Hz to 1.5 kHz, Mid 70 Hz to 1 kHz, Bass \
             40 to 500 Hz. Each has a latency of its own.",
        ),
        ParamDescriptor::gain(INPUT_GAIN, "Input Gain", 24.0, 0.0)
            .describe("The level into the tracker and the shifter, from silence to +24 dB."),
        ParamDescriptor::continuous(
            STRENGTH,
            "Correction Strength",
            (0.0, 100.0),
            100.0,
            ParamUnit::Percent,
        )
        .describe(
            "The share of the distance to the target note that the correction covers: 0 % \
             leaves the voice alone, 100 % puts it on the note.",
        ),
        ParamDescriptor::toggle(SMOOTH, "Smooth", true)
            .describe("Moves the correction to a new note over Smoothing Time."),
        ParamDescriptor::continuous(
            SMOOTHING_TIME,
            "Smoothing Time",
            (0.0, 200.0),
            50.0,
            ParamUnit::Milliseconds,
        )
        .describe("How long the correction takes to move to a new note, with Smooth on."),
        ParamDescriptor::integer(
            DEGREES,
            "Quantizer Pitch Shift",
            (-7, 7),
            0,
            ParamUnit::None,
        )
        .describe(
            "Moves every target note this many degrees along the scale, after the correction.",
        ),
        ParamDescriptor::choice(ROOT, "Root", &roots, 0)
            .describe("The root of the scale the voice is corrected to."),
        ParamDescriptor::choice(SCALE, "Scale", &scales, 0)
            .describe("The scale the voice is corrected to."),
    ];
    for (key, root) in (0..12u8).zip(roots) {
        params.push(
            ParamDescriptor::toggle(
                ParamId(KEY_C.0 + u32::from(key)),
                &format!("Key {root}"),
                false,
            )
            .describe(&format!(
                "Leaves {root} out of the scale if it is in it, and brings it in if it isn't."
            )),
        );
    }
    params.extend([
        ParamDescriptor::integer(
            PITCH_SHIFT,
            "Pitch Shift",
            (-24, 24),
            0,
            ParamUnit::Semitones,
        )
        .describe("Moves every output pitch by this many semitones, after the correction."),
        ParamDescriptor::continuous(
            FINE,
            "Pitch Shift Fine",
            (-50.0, 50.0),
            0.0,
            ParamUnit::Cents,
        )
        .describe("Moves the pitch further, in cents."),
        ParamDescriptor::continuous(
            FORMANT_SHIFT,
            "Formant Shift",
            (-100.0, 100.0),
            0.0,
            ParamUnit::Percent,
        )
        .describe("Moves the formants apart from the pitch: 100 % is an octave up."),
        ParamDescriptor::continuous(
            FORMANT_FOLLOW,
            "Formant Follow",
            (0.0, 100.0),
            0.0,
            ParamUnit::Percent,
        )
        .describe(
            "How far the formants move with the pitch's correction and shift: at 100 % they move \
             by the same interval.",
        ),
        ParamDescriptor::continuous(
            VIBRATO_AMOUNT,
            "Vibrato Amount",
            (0.0, 200.0),
            0.0,
            ParamUnit::Cents,
        )
        .describe("The vibrato's depth, in cents from its lowest pitch to its highest."),
        ParamDescriptor::continuous(
            VIBRATO_RATE,
            "Vibrato Rate",
            (2.0, 15.0),
            5.5,
            ParamUnit::Hertz,
        )
        .describe("The vibrato's speed."),
        ParamDescriptor::continuous(
            VIBRATO_FADE,
            "Vibrato Fade In",
            (0.0, 2_000.0),
            300.0,
            ParamUnit::Milliseconds,
        )
        .describe("How long the vibrato takes to reach its depth after each onset."),
        ParamDescriptor::toggle(NATURAL_VIBRATO, "Natural Vibrato", false)
            .describe("Lets the vibrato's rate and depth wander by up to 10 %."),
        ParamDescriptor::continuous(DRY_WET, "Dry/Wet", (0.0, 100.0), 100.0, ParamUnit::Percent)
            .describe("The balance between the uncorrected voice and the shifted one."),
        ParamDescriptor::choice(MONO_POLY, "Mono/Poly", &VOICE_MODES, 0).describe(
            "Mono follows the newest held note; Poly makes a copy of the voice for each held \
             note, up to Voice Count.",
        ),
        ParamDescriptor::choice(VOICE_COUNT, "Voice Count", &counts, 1).describe(
            "The most voices Poly makes: a note past them takes the oldest voice's place.",
        ),
        ParamDescriptor::continuous(GLIDE, "Glide", (0.0, 1_000.0), 0.0, ParamUnit::Milliseconds)
            .describe("The time overlapping Mono notes take to slide to the next."),
        ParamDescriptor::continuous(
            ATTACK,
            "Attack Time",
            (0.0, 1_000.0),
            10.0,
            ParamUnit::Milliseconds,
        )
        .describe("How long a voice takes to fade in after its note-on."),
        ParamDescriptor::continuous(
            RELEASE,
            "Release Time",
            (0.0, 5_000.0),
            100.0,
            ParamUnit::Milliseconds,
        )
        .describe("How long a voice takes to fade out after its note-off."),
        ParamDescriptor::toggle(NOTE_LATCH, "Note Latch", false)
            .describe("A voice holds its note's pitch after the note-off, until the next note on."),
        ParamDescriptor::integer(
            BEND_RANGE,
            "Pitch Bend Range",
            (0, 48),
            2,
            ParamUnit::Semitones,
        )
        .describe("How far the MIDI source's full pitch bend moves the pitch."),
    ]);
    let targets = [
        ("Pitch", 3, "±12 semitones"),
        ("Formant", 2, "±100 %"),
        ("Volume", 0, "down to silence"),
        ("Pan", 5, "left to right"),
    ];
    for (n, (name, source, span)) in targets.into_iter().enumerate() {
        let id = ParamId(PITCH_SOURCE.0 + 2 * n as u32);
        params.push(
            ParamDescriptor::choice(id, &format!("{name} Mod Source"), &SOURCES, source)
                .describe(&format!("What moves the {name}, by the depth below.")),
        );
        params.push(
            ParamDescriptor::continuous(
                ParamId(id.0 + 1),
                &format!("{name} Mod Depth"),
                (-100.0, 100.0),
                0.0,
                ParamUnit::Percent,
            )
            .describe(&format!(
                "How far the source moves the {name}: at 100 % over {span}; negative turns it over."
            )),
        );
    }
    params.extend([
        ParamDescriptor::choice(LFO_WAVE, "LFO Waveform", &WAVES, 0).describe("The LFO's wave."),
        ParamDescriptor::toggle(LFO_SYNC, "LFO Sync", false).describe(
            "Sets the LFO's rate in beat divisions of the tempo instead of milliseconds.",
        ),
        ParamDescriptor::continuous(
            LFO_RATE,
            "LFO Rate",
            (10.0, 10_000.0),
            500.0,
            ParamUnit::Milliseconds,
        )
        .logarithmic()
        .describe("The time of one cycle of the LFO, with LFO Sync off."),
        ParamDescriptor::choice(LFO_DIVISION, "LFO Division", &divisions, DEFAULT_DIVISION)
            .describe("The LFO's cycle as a division of the beat, with LFO Sync on."),
        ParamDescriptor::continuous(
            LFO_DELAY,
            "LFO Delay",
            (0.0, 1_500.0),
            0.0,
            ParamUnit::Milliseconds,
        )
        .describe("How long the LFO is held at zero after each reset."),
        ParamDescriptor::continuous(
            LFO_ATTACK,
            "LFO Attack",
            (0.0, 2_000.0),
            0.0,
            ParamUnit::Milliseconds,
        )
        .describe("How long the LFO takes to fade in after its delay."),
        ParamDescriptor::toggle(LFO_RESET, "LFO Reset", false)
            .describe("Restarts the LFO at each onset: of the voice, or with MIDI On of a note."),
    ]);
    for (n, (name, span)) in [
        ("Pitch", "±12 semitones"),
        ("Formant", "±100 %"),
        ("Volume", "down to silence"),
        ("Pan", "left to right"),
    ]
    .into_iter()
    .enumerate()
    {
        params.push(
            ParamDescriptor::continuous(
                ParamId(LFO_PITCH.0 + n as u32),
                &format!("LFO {name} Depth"),
                (-100.0, 100.0),
                0.0,
                ParamUnit::Percent,
            )
            .describe(&format!(
                "How far the LFO moves the {name}: at 100 % over {span}; negative turns it over."
            )),
        );
    }
    debug_assert_eq!(params.len(), PARAMS);
    let layout = layout(&params);
    DeviceType::native(AUTO_SHIFT, "Auto Shift", DeviceRole::AudioEffect)
        .with_midi_sidechain_tap()
        .with_description(
            DeviceDescription::new(params)
                .with_layout(layout)
                .with_title_switch(USE_SCALE)
                .with_banks(&[
                    (
                        "Main",
                        &[
                            "Correction Strength",
                            "Smoothing Time",
                            "Root",
                            "Scale",
                            "Pitch Shift",
                            "Formant Shift",
                            "Formant Follow",
                            "Dry/Wet",
                        ],
                    ),
                    (
                        "Input and Vibrato",
                        &[
                            "Pitch Range",
                            "Input Gain",
                            "Live Mode",
                            "Pitch Shift Fine",
                            "Vibrato Amount",
                            "Vibrato Rate",
                            "Vibrato Fade In",
                            "Natural Vibrato",
                        ],
                    ),
                    (
                        "Quantizer",
                        &[
                            "Smooth",
                            "Quantizer Pitch Shift",
                            "Key C",
                            "Key C#",
                            "Key D",
                            "Key D#",
                            "Key E",
                            "Key F",
                        ],
                    ),
                    (
                        "Included Notes",
                        &["Key F#", "Key G", "Key G#", "Key A", "Key A#", "Key B"],
                    ),
                    (
                        "MIDI",
                        &[
                            "MIDI On",
                            "Mono/Poly",
                            "Voice Count",
                            "Glide",
                            "Attack Time",
                            "Release Time",
                            "Note Latch",
                            "Pitch Bend Range",
                        ],
                    ),
                    (
                        "MIDI Routing",
                        &[
                            "Pitch Mod Source",
                            "Pitch Mod Depth",
                            "Formant Mod Source",
                            "Formant Mod Depth",
                            "Volume Mod Source",
                            "Volume Mod Depth",
                            "Pan Mod Source",
                            "Pan Mod Depth",
                        ],
                    ),
                    (
                        "LFO",
                        &[
                            "LFO Waveform",
                            "LFO Sync",
                            "LFO Rate",
                            "LFO Division",
                            "LFO Delay",
                            "LFO Attack",
                            "LFO Reset",
                        ],
                    ),
                    (
                        "LFO Routing",
                        &[
                            "LFO Pitch Depth",
                            "LFO Formant Depth",
                            "LFO Volume Depth",
                            "LFO Pan Depth",
                        ],
                    ),
                ]),
        )
}

/// The name of the pages node whose tabs sit in the title bar.
pub const TABS: &str = "Tabs";

/// A band `height` tall holding `controls` top to bottom, then in further
/// columns, each column as wide as its widest control or its longest name:
/// the names are long, and `ControlBand::stacked` leaves room for the
/// widgets alone.
fn roomy(height: f32, params: &[ParamDescriptor], controls: &[(ParamId, Widget)]) -> ControlBand {
    use bl_model::device_layout::{COLUMN_GAP, Control, PADDING, ROW_GAP};
    let width = |param: ParamId, widget: Widget| {
        let name = params
            .iter()
            .find(|p| p.id == param)
            .map_or(0, |p| p.name.len());
        widget.size().width.max(name as f32 * 5.4)
    };
    let mut placed = Vec::with_capacity(controls.len());
    let (mut x, mut y, mut column) = (PADDING, PADDING, 0.0f32);
    for &(param, widget) in controls {
        let size = widget.size();
        if y > PADDING && y + size.height + PADDING > height {
            x += column + COLUMN_GAP;
            (y, column) = (PADDING, 0.0);
        }
        let room = width(param, widget);
        // The control sits in the middle of its room.
        placed.push(Control::new(
            param,
            widget,
            x + (room - size.width) / 2.0,
            y,
        ));
        // A box above a knob leaves the knob's label room.
        let more = if matches!(widget, Widget::Knob) {
            0.0
        } else {
            5.0
        };
        y += size.height + ROW_GAP + more;
        column = column.max(room);
    }
    ControlBand::free(x + column + PADDING, height, placed)
}

/// The display, then the input section, the tabs (the Quantizer's or the
/// MIDI tab's controls, and the LFO's), the pitch section, the vibrato
/// section and Dry/Wet; MIDI In's panel in the expanded part.
fn layout(params: &[ParamDescriptor]) -> DeviceLayout {
    use Widget::{Chooser, Knob, Number, Segments, Toggle};
    let band = |controls: &[(ParamId, Widget)]| roomy(BODY_HEIGHT, params, controls);
    // Controls of the Quantizer tab grey while MIDI On is on, as the MIDI
    // tab's do while it is off: the tab that isn't in use stays reachable.
    let greyed = |mut band: ControlBand, midi: i32| {
        for control in &mut band.controls {
            control.grey = Some(bl_model::device_layout::GreyWhen {
                param: MIDI_ON,
                value: midi,
            });
        }
        LayoutNode::Band(band)
    };
    let tab = BODY_HEIGHT - TAB_HEIGHT;
    let keys =
        {
            // The twelve keys as a piano: the five black ones above the gaps
            // between the seven white ones.
            use bl_model::device_layout::Control;
            let (step, row) = (SEGMENT_WIDTH + 3.0, 21.0);
            let whites = [0u32, 2, 4, 5, 7, 9, 11];
            let blacks = [(1u32, 0.5f32), (3, 1.5), (6, 3.5), (8, 4.5), (10, 5.5)];
            let mut controls: Vec<Control> = whites
                .iter()
                .enumerate()
                .map(|(n, key)| {
                    Control::new(
                        ParamId(KEY_C.0 + key),
                        Toggle,
                        4.0 + n as f32 * step,
                        4.0 + row,
                    )
                })
                .collect();
            controls.extend(blacks.iter().map(|(key, at)| {
                Control::new(ParamId(KEY_C.0 + key), Toggle, 4.0 + at * step, 4.0)
            }));
            ControlBand::free(8.0 + 7.0 * step, tab, controls)
        };
    let sized = |controls: &[(ParamId, Widget)]| roomy(tab, params, controls);
    let quantizer = LayoutNode::Row(vec![
        greyed(
            sized(&[(STRENGTH, Knob), (SMOOTH, Toggle), (SMOOTHING_TIME, Knob)]),
            1,
        ),
        greyed(
            sized(&[(ROOT, Chooser), (SCALE, Chooser), (DEGREES, Number)]),
            1,
        ),
        greyed(keys, 1),
    ]);
    let midi = LayoutNode::Row(vec![
        greyed(
            sized(&[
                (ATTACK, Knob),
                (RELEASE, Knob),
                (NOTE_LATCH, Toggle),
                (BEND_RANGE, Number),
            ]),
            0,
        ),
        greyed(sized(&[(PITCH_SOURCE, Chooser), (PITCH_DEPTH, Knob)]), 0),
        greyed(
            sized(&[(FORMANT_SOURCE, Chooser), (FORMANT_DEPTH, Knob)]),
            0,
        ),
        greyed(sized(&[(VOLUME_SOURCE, Chooser), (VOLUME_DEPTH, Knob)]), 0),
        greyed(sized(&[(PAN_SOURCE, Chooser), (PAN_DEPTH, Knob)]), 0),
    ]);
    let lfo = LayoutNode::Row(vec![
        LayoutNode::Band(sized(&[
            (LFO_RESET, Toggle),
            (LFO_WAVE, Chooser),
            (LFO_SYNC, Toggle),
            (LFO_RATE, Knob),
            (LFO_DIVISION, Chooser),
        ])),
        LayoutNode::Band(sized(&[(LFO_DELAY, Knob), (LFO_ATTACK, Knob)])),
        LayoutNode::Band(sized(&[
            (LFO_PITCH, Knob),
            (LFO_FORMANT, Knob),
            (LFO_VOLUME, Knob),
            (LFO_PAN, Knob),
        ])),
    ]);
    DeviceLayout::new(LayoutNode::Row(vec![
        LayoutNode::Cell(DisplayCell::new(DISPLAY, 260.0, BODY_HEIGHT, 1.0)),
        LayoutNode::Band(band(&[(PITCH_RANGE, Segments(3)), (INPUT_GAIN, Knob)])),
        LayoutNode::Pages(PagesNode::new(
            TABS,
            PageStyle::TitleBar,
            vec![
                Page::new("Quantizer", quantizer),
                Page::new("MIDI", midi),
                Page::new("LFO", lfo),
            ],
        )),
        LayoutNode::Band(band(&[(PITCH_SHIFT, Number), (FINE, Knob)])),
        LayoutNode::Band(band(&[(FORMANT_SHIFT, Knob), (FORMANT_FOLLOW, Knob)])),
        LayoutNode::Band(band(&[(VIBRATO_AMOUNT, Knob), (VIBRATO_RATE, Knob)])),
        LayoutNode::Band(band(&[(VIBRATO_FADE, Knob), (NATURAL_VIBRATO, Toggle)])),
        LayoutNode::Band(band(&[(DRY_WET, Knob)])),
    ]))
    .with_title(vec![TitlePart::Button(LIVE_MODE)])
    .with_expanded(LayoutNode::Band(band(&[
        (MIDI_ON, Toggle),
        (MONO_POLY, Segments(2)),
        (VOICE_COUNT, Chooser),
        (GLIDE, Knob),
    ])))
}

/// Adds Auto Shift to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, auto_shift_type(), AutoShift::new);
}

/// The twelve pitch classes `scale` holds, as a mask of bits 0 (C) to 11 (B).
fn mask_of(scale: Scale) -> u16 {
    (0..12u8).fold(0, |mask, pc| mask | u16::from(scale.holds(60 + pc)) << pc)
}

/// Whether the note `n` (any octave) is in `mask`.
fn allowed(mask: u16, n: i32) -> bool {
    mask >> n.rem_euclid(12) & 1 == 1
}

/// The note of `mask` nearest `note`, the lower of two as near; `note` itself
/// while the mask holds none (R5).
fn nearest(mask: u16, note: f64) -> f64 {
    if mask == 0 {
        return note;
    }
    let base = note.floor() as i32;
    (base - 6..=base + 7)
        .filter(|&n| allowed(mask, n))
        .map(f64::from)
        .min_by(|a, b| (a - note).abs().total_cmp(&(b - note).abs()))
        .unwrap_or(note)
}

/// The note `degrees` degrees of `mask` from the note `n` of it (R7); semitones
/// while the mask holds none.
fn move_degrees(mask: u16, n: f64, degrees: i32) -> f64 {
    if mask == 0 {
        return n + f64::from(degrees);
    }
    let (mut at, mut left) = (n.round() as i32, degrees.abs());
    while left > 0 {
        at += degrees.signum();
        if allowed(mask, at) {
            left -= 1;
        }
    }
    f64::from(at)
}

/// What the Quantizer asks of a grain, as it was at the sample the grain is
/// centred on.
#[derive(Clone, Copy)]
struct Quant {
    mask: u16,
    degrees: i8,
    strength: f32,
    /// The smoothing time in seconds; 0 with Smooth off.
    smooth: f32,
}

impl Quant {
    const NONE: Self = Self {
        mask: 0,
        degrees: 0,
        strength: 0.0,
        smooth: 0.0,
    };
}

/// The Quantizer's state across grains: the note being corrected to, kept
/// until the next onset if the scale changes (R5), and what it moved to.
#[derive(Clone, Copy)]
struct Tune {
    have: bool,
    /// The mask the note began under.
    mask: u16,
    /// The scale note the voice is nearest, before the degrees.
    raw: f64,
    /// The target after Smoothing Time, in notes.
    smoothed: f64,
    at: i64,
    /// For the display: the voice's note, and where it comes out.
    heard: f64,
    shown: f64,
}

impl Tune {
    const NEW: Self = Self {
        have: false,
        mask: 0,
        raw: 0.0,
        smoothed: 0.0,
        at: 0,
        heard: 0.0,
        shown: 0.0,
    };
}

/// The ratios the shifter moves a grain by in the Quantizer tab (R5 to R7):
/// the pitch correction toward the nearest note of the scale, then the
/// multiplier of Pitch Shift, the LFO and the vibrato that `ask` holds.
fn quantise(tune: &mut Tune, quant: &Quant, rate: f64, grain: &Grain, ask: &Ask) -> [f32; 2] {
    let note = note_of(grain.frequency);
    let near = nearest(quant.mask, note);
    // A new note takes the nearest note of the scale; a note that goes on
    // across a change of scale keeps the target it began with.
    let fresh = grain.onset || !tune.have;
    if fresh {
        *tune = Tune {
            have: true,
            mask: quant.mask,
            raw: near,
            ..*tune
        };
    } else if quant.mask == tune.mask {
        tune.raw = near;
    }
    let target = move_degrees(tune.mask, tune.raw, i32::from(quant.degrees));
    if fresh || quant.smooth <= 0.0 {
        tune.smoothed = target;
    } else {
        let seconds = (grain.at - tune.at).max(0) as f64 / rate;
        tune.smoothed +=
            (target - tune.smoothed) * (1.0 - (-seconds / f64::from(quant.smooth)).exp());
    }
    tune.at = grain.at;
    let corrected = note + f64::from(quant.strength) * (tune.smoothed - note);
    let ratio = ((corrected - note) / 12.0).exp2() * f64::from(ask[0]);
    tune.heard = note;
    tune.shown = corrected + 12.0 * f64::from(ask[0]).log2();
    [
        ratio as f32,
        (f64::from(ask[1]) * ratio.powf(f64::from(ask[2]))) as f32,
    ]
}

/// The ratios for a grain of a MIDI voice at `ask[3]` hertz (R9).
fn follow_note(tune: &mut Tune, grain: &Grain, ask: &Ask) -> [f32; 2] {
    let target = f64::from(ask[3]) * f64::from(ask[0]);
    let ratio = target / grain.frequency;
    tune.heard = note_of(grain.frequency);
    tune.shown = note_of(target);
    [
        ratio as f32,
        (f64::from(ask[1]) * ratio.powf(f64::from(ask[2]))) as f32,
    ]
}

/// One of the node's three Pitch Ranges: its tracker, and a shifter for each
/// voice.
struct Pool {
    tracker: Tracker,
    shifters: Vec<VoiceShifter>,
    /// What the Quantizer asked at each sample, and how many samples the pool
    /// has heard.
    quant: Vec<Quant>,
    count: u64,
    live: bool,
}

impl Pool {
    fn new(rate: f64, range: usize, live: bool) -> Self {
        let (low, high) = RANGE_HERTZ[range];
        let shifters: Vec<VoiceShifter> = (0..VOICES)
            .map(|_| VoiceShifter::following(rate, low, high))
            .collect();
        let mut pool = Self {
            tracker: Tracker::new(&Config::for_range(rate, low, high)),
            quant: vec![Quant::NONE; shifters[0].span()],
            shifters,
            count: 0,
            live: false,
        };
        pool.set_live(live);
        pool
    }

    /// The latency the range has, with Live Mode off or on (R3).
    fn latency(&self) -> usize {
        let full = self.shifters[0].full_latency();
        if self.live { (full / 2).max(1) } else { full }
    }

    fn set_live(&mut self, live: bool) {
        self.live = live;
        let latency = self.latency();
        for shifter in &mut self.shifters {
            shifter.set_latency(latency);
        }
    }
}

/// A voice of the MIDI tab: a note, its expressions, and its envelope.
#[derive(Clone, Copy)]
struct Note {
    key: u8,
    channel: u8,
    id: Option<u32>,
    /// Whether the key is down, and whether Note Latch holds the note after
    /// its key came up.
    down: bool,
    latched: bool,
    age: u64,
    velocity: f32,
    pressure: Option<f32>,
    bend: f32,
    slide: f32,
    env: f32,
    /// The pitch in notes, gliding in Mono: `step` more each sample for
    /// `left` samples.
    pitch: f64,
    step: f64,
    left: u32,
}

impl Note {
    const IDLE: Self = Self {
        key: 0,
        channel: 0,
        id: None,
        down: false,
        latched: false,
        age: 0,
        velocity: 0.0,
        pressure: None,
        bend: 0.0,
        slide: 0.0,
        env: 0.0,
        pitch: 60.0,
        step: 0.0,
        left: 0,
    };

    fn active(&self) -> bool {
        self.down || self.latched
    }

    /// Starts gliding to `pitch` over `frames`.
    fn go(&mut self, pitch: f64, frames: f64) {
        let frames = frames.round();
        if frames < 1.0 {
            self.pitch = pitch;
            self.left = 0;
        } else {
            self.step = (pitch - self.pitch) / frames;
            self.left = frames as u32;
        }
    }
}

/// What one voice is for the current sample.
#[derive(Clone, Copy)]
struct Factors {
    ask: Ask,
    gain: f32,
    pan: f32,
}

/// Auto Shift's audio-thread half.
struct AutoShift {
    rate: f64,
    seed: u64,
    pools: [Pool; 3],
    range: usize,
    lfo: Lfo,
    tune: Tune,
    notes: [Note; VOICES],
    /// Mono's keys held, the newest last.
    held: [u8; HELD_KEYS],
    held_len: usize,
    next_age: u64,
    /// The channel's controls: pitch bend, pressure and the mod wheel.
    bend: f32,
    pressure: f32,
    wheel: f32,
    midi_was: bool,
    /// The dry input, per channel, and what each voice's gain and pan were.
    dry: [DelayLine; 2],
    gains: Vec<DelayLine>,
    pans: Vec<DelayLine>,
    spare: DelayLine,
    /// Frames played since the node began, since the LFO was reset and since
    /// the vibrato's onset.
    pos: u64,
    lfo_age: u64,
    vibrato_age: u64,
    vibrato_phase: f64,
    lamp: u32,
    /// The mask of the scale in use, and what it was made from.
    mask: u16,
    mask_for: Option<Scale>,
    /// The display: a column of each trace every `column_frames`.
    detected: Vec<f32>,
    target: Vec<f32>,
    head: usize,
    column_left: u64,
    column_frames: u64,
    voiced: bool,
    heard: f64,
    aimed: bool,
    /// The mask the Quantizer corrects to now, included notes and all, for
    /// the display.
    shown_mask: u16,
    /// The draws Natural Vibrato wanders between: the pair of the stretch
    /// `.0` for the rate and the pair for the depth.
    wander: (u64, [f64; 4]),
    events: Vec<NoteEvent>,
}

impl AutoShift {
    fn new(device: &Device, config: &NodeConfig) -> Self {
        let rate = config.sample_rate.as_f64();
        let value = |id: ParamId, default: f64| device.params.get(&id).copied().unwrap_or(default);
        let range = (value(PITCH_RANGE, 1.0) as usize).min(2);
        let live = value(LIVE_MODE, 0.0) >= 0.5;
        let pools = [0, 1, 2].map(|r| Pool::new(rate, r, live && r == range));
        let longest = pools
            .iter()
            .map(|p| p.shifters[0].full_latency())
            .max()
            .unwrap_or(1);
        let lines = |n: usize| (0..n).map(|_| DelayLine::new(longest)).collect::<Vec<_>>();
        Self {
            rate,
            seed: device.id.0.get(),
            pools,
            range,
            lfo: Lfo::new(config.sample_rate, device.id.0.get()),
            tune: Tune::NEW,
            notes: [Note::IDLE; VOICES],
            held: [0; HELD_KEYS],
            held_len: 0,
            next_age: 1,
            bend: 0.0,
            pressure: 0.0,
            wheel: 0.0,
            midi_was: false,
            dry: [DelayLine::new(longest), DelayLine::new(longest)],
            gains: lines(VOICES),
            pans: lines(VOICES),
            spare: DelayLine::new(longest),
            pos: 0,
            lfo_age: 0,
            vibrato_age: 0,
            vibrato_phase: 0.0,
            lamp: 0,
            mask: 0,
            mask_for: None,
            detected: vec![NO_PITCH; COLUMNS],
            target: vec![NO_PITCH; COLUMNS],
            head: 0,
            column_left: 0,
            column_frames: (rate * HISTORY_SECONDS / COLUMNS as f64).round().max(1.0) as u64,
            voiced: false,
            heard: 0.0,
            aimed: false,
            shown_mask: 0,
            wander: (u64::MAX, [0.0; 4]),
            events: Vec::with_capacity(EVENT_ROOM),
        }
    }

    /// A random value from −1 to just below 1 for the `n`th stretch of
    /// Natural Vibrato's wander, for `salt`'s use: it follows the device's
    /// seed and the count alone.
    fn draw(&self, n: u64, salt: u64) -> f64 {
        let key = self.seed ^ (salt << 56) ^ n.wrapping_mul(0x9E37_79B9_7F4A_7C15);
        Random::seeded(key).next_bipolar()
    }

    /// Takes a note or message of the MIDI source on this sample (R9, R10).
    fn note(&mut self, event: &NoteEvent, set: &Settings<'_>) {
        match event.kind {
            NoteKind::On => self.press(event, set),
            NoteKind::Off | NoteKind::Choke => self.release(event, set),
            NoteKind::Expression { expression, value } => {
                for note in self.notes.iter_mut().filter(|n| n.active()) {
                    let same = match (note.id, event.note_id) {
                        (Some(a), Some(b)) => a == b,
                        _ => note.key == event.key && note.channel == event.channel,
                    };
                    if same {
                        match expression {
                            NoteExpression::Bend => note.bend = value.clamp(-1.0, 1.0),
                            NoteExpression::Pressure => note.pressure = Some(value.clamp(0.0, 1.0)),
                            NoteExpression::Slide => note.slide = value.clamp(0.0, 1.0),
                        }
                    }
                }
            }
            NoteKind::PitchBend { value } => self.bend = value.clamp(-1.0, 1.0),
            NoteKind::ChannelPressure { value } => self.pressure = value.clamp(0.0, 1.0),
            NoteKind::Control {
                controller: 1,
                value,
            } => self.wheel = value.clamp(0.0, 1.0),
            NoteKind::Control { .. } => {}
        }
    }

    fn press(&mut self, event: &NoteEvent, set: &Settings<'_>) {
        // A note on after every key came up lets go of what Note Latch held.
        if set.latch && !self.notes.iter().any(|n| n.down) {
            for note in &mut self.notes {
                note.latched = false;
            }
        }
        let fresh = Note {
            key: event.key,
            channel: event.channel,
            id: event.note_id,
            down: true,
            latched: false,
            age: self.next_age,
            velocity: event.velocity,
            ..Note::IDLE
        };
        self.next_age += 1;
        self.restart(set.reset);
        let pitch = f64::from(event.key);
        if !set.poly {
            if self.held_len == HELD_KEYS {
                self.held.copy_within(1.., 0);
                self.held_len -= 1;
            }
            self.held[self.held_len] = event.key;
            self.held_len += 1;
            // Only a note that overlaps another glides.
            let overlaps = self.notes[0].down;
            let voice = &mut self.notes[0];
            let (env, from) = (voice.env, voice.pitch);
            *voice = Note {
                env,
                pitch: from,
                ..fresh
            };
            voice.go(pitch, if overlaps { set.glide } else { 0.0 });
            return;
        }
        // Poly: a silent voice, else the oldest.
        let count = set.voices;
        let chosen = (0..count)
            .find(|&v| !self.notes[v].active() && self.notes[v].env <= 0.0)
            .or_else(|| (0..count).find(|&v| !self.notes[v].active()))
            .or_else(|| (0..count).min_by_key(|&v| self.notes[v].age));
        if let Some(v) = chosen {
            let env = self.notes[v].env;
            self.notes[v] = Note {
                env,
                pitch,
                ..fresh
            };
        }
    }

    fn release(&mut self, event: &NoteEvent, set: &Settings<'_>) {
        let keep = |note: &mut Note| {
            note.down = false;
            note.latched = set.latch;
        };
        if set.poly {
            let oldest = (0..VOICES)
                .filter(|&v| {
                    self.notes[v].down
                        && self.notes[v].key == event.key
                        && self.notes[v].channel == event.channel
                })
                .min_by_key(|&v| self.notes[v].age);
            if let Some(v) = oldest {
                keep(&mut self.notes[v]);
            }
            return;
        }
        let Some(at) = self.held[..self.held_len]
            .iter()
            .rposition(|&k| k == event.key)
        else {
            return;
        };
        self.held.copy_within(at + 1..self.held_len, at);
        self.held_len -= 1;
        if self.held_len == 0 {
            keep(&mut self.notes[0]);
        } else if at == self.held_len {
            // The newest note still held takes over.
            let key = self.held[self.held_len - 1];
            self.notes[0].key = key;
            self.notes[0].go(f64::from(key), set.glide);
        }
    }

    /// An onset: the vibrato starts over, and the LFO with LFO Reset on (R12,
    /// R14).
    fn restart(&mut self, lfo: bool) {
        self.vibrato_age = 0;
        if lfo {
            self.lfo_age = 0;
            self.lfo.restart();
            self.lamp = (LAMP_SECONDS * self.rate) as u32;
        }
    }

    /// The mask of the notes the Quantizer corrects to at this sample.
    fn mask(&mut self, scale: Scale, flips: u16) -> u16 {
        if self.mask_for != Some(scale) {
            self.mask_for = Some(scale);
            self.mask = mask_of(scale);
        }
        self.mask ^ flips
    }
}

/// What a stretch reads of the parameters: the choices, and the values that
/// move from frame to frame.
struct Settings<'a> {
    midi: bool,
    poly: bool,
    voices: usize,
    latch: bool,
    live: bool,
    reset: bool,
    sync: bool,
    smooth: bool,
    natural: bool,
    flips: u16,
    use_scale: bool,
    chosen: Scale,
    glide: f64,
    attack: f64,
    release: f64,
    bend_range: f64,
    wave: usize,
    division: f64,
    sources: [usize; 4],
    depths: [f64; 4],
    lfo_depths: [f64; 4],
    lfo_ms: f64,
    lfo_delay: f64,
    lfo_attack: f64,
    gain: &'a [f32],
    strength: &'a [f32],
    smoothing: &'a [f32],
    degrees: &'a [f32],
    shift: &'a [f32],
    fine: &'a [f32],
    formant: &'a [f32],
    follow: &'a [f32],
    vibrato: &'a [f32],
    vibrato_rate: &'a [f32],
    vibrato_fade: &'a [f32],
    wet: &'a [f32],
}

/// A gain from 1 down to 0 as `source` (0 to 1) moves, by `depth` (−1 to 1):
/// a positive depth is silent at 0 and a negative one at 1 (R11).
fn mod_gain(depth: f64, source: f64) -> f64 {
    if depth >= 0.0 {
        1.0 - depth * (1.0 - source)
    } else {
        1.0 + depth * source
    }
}

impl AutoShift {
    /// The factors of every voice on this sample: what to ask the shifters
    /// and how loud and where to put what comes out, a latency from now.
    fn factors(&mut self, set: &Settings<'_>, i: usize, lfo: f64) -> [Factors; VOICES] {
        let mut out = [Factors {
            ask: [1.0, 1.0, 0.0, 0.0],
            gain: 0.0,
            pan: 0.0,
        }; VOICES];
        let rate = self.rate;
        // The vibrato (R14): a sine, faded in from the onset, whose rate and
        // depth wander with Natural Vibrato.
        let (mut amount, mut speed) = (f64::from(set.vibrato[i]), f64::from(set.vibrato_rate[i]));
        if set.natural {
            let position = self.pos as f64 / (WANDER_SECONDS * rate);
            let n = position.floor() as u64;
            let t = position - n as f64;
            let t = t * t * (3.0 - 2.0 * t);
            if self.wander.0 != n {
                self.wander = (
                    n,
                    [
                        self.draw(n, 1),
                        self.draw(n + 1, 1),
                        self.draw(n, 2),
                        self.draw(n + 1, 2),
                    ],
                );
            }
            let [a, b, c, d] = self.wander.1;
            speed *= 1.0 + WANDER * (a * (1.0 - t) + b * t);
            amount *= 1.0 + WANDER * (c * (1.0 - t) + d * t);
        }
        let fade = f64::from(set.vibrato_fade[i]) / 1_000.0 * rate;
        let level = if fade < 1.0 {
            1.0
        } else {
            (self.vibrato_age as f64 / fade).min(1.0)
        };
        let vibrato = (std::f64::consts::TAU * self.vibrato_phase).sin() * amount / 200.0 * level;
        self.vibrato_phase = (self.vibrato_phase + speed / rate).fract();
        self.vibrato_age += 1;
        // The LFO's level after its delay and attack.
        let lfo_level = {
            let age = self.lfo_age as f64 / rate * 1_000.0;
            if age < set.lfo_delay {
                0.0
            } else if set.lfo_attack <= 0.0 {
                1.0
            } else {
                ((age - set.lfo_delay) / set.lfo_attack).min(1.0)
            }
        };
        self.lfo_age += 1;
        let swing = lfo * lfo_level;
        let [lp, lf, lv, la] = set.lfo_depths;
        let semitones = f64::from(set.shift[i]) + f64::from(set.fine[i]) / 100.0;
        let follow = f64::from(set.follow[i]) / 100.0;
        let formant = f64::from(set.formant[i]);
        let lfo_gain = mod_gain(lv, (swing + 1.0) / 2.0);
        let voices = if set.midi && set.poly { set.voices } else { 1 };
        for (v, factors) in out.iter_mut().enumerate().take(voices) {
            let mut semis = semitones + vibrato + lp * 12.0 * swing;
            let mut shift = formant + lf * 100.0 * swing;
            let (mut gain, mut pan, mut hz) = (lfo_gain, la * swing, 0.0);
            let mut env = 1.0;
            if set.midi {
                let note = self.notes[v];
                let source = |target: usize| -> (f64, f64) {
                    let s = match set.sources[target] {
                        0 => note.velocity,
                        1 => note.pressure.unwrap_or(self.pressure),
                        2 => self.wheel,
                        3 => self.bend,
                        4 => note.bend,
                        _ => note.slide,
                    };
                    let bipolar = matches!(set.sources[target], 3 | 4);
                    let s = f64::from(s);
                    (s, if bipolar { (s + 1.0) / 2.0 } else { s })
                };
                let (s, _) = source(0);
                semis += set.depths[0] * 12.0 * s;
                let (s, _) = source(1);
                shift += set.depths[1] * 100.0 * s;
                let (_, u) = source(2);
                gain *= mod_gain(set.depths[2], u);
                let (s, _) = source(3);
                pan += set.depths[3] * s;
                let pitch = note.pitch + f64::from(self.bend) * set.bend_range;
                hz = hz_of(pitch);
                env = f64::from(note.env);
            }
            let formant_ratio = (shift.clamp(-100.0, 100.0) / 100.0).exp2();
            factors.ask = [
                (semis / 12.0).exp2() as f32,
                formant_ratio as f32,
                follow as f32,
                hz as f32,
            ];
            factors.gain = (gain * env) as f32;
            factors.pan = pan.clamp(-1.0, 1.0) as f32;
        }
        out
    }

    /// Moves every voice's envelope and glide one sample on (R10).
    fn step_notes(&mut self, set: &Settings<'_>) {
        let rise = if set.attack < 1.0 {
            1.0
        } else {
            (1.0 / set.attack) as f32
        };
        let fall = if set.release < 1.0 {
            1.0
        } else {
            (1.0 / set.release) as f32
        };
        for note in &mut self.notes {
            if note.left > 0 {
                note.pitch += note.step;
                note.left -= 1;
            }
            note.env = if note.active() {
                (note.env + rise).min(1.0)
            } else {
                (note.env - fall).max(0.0)
            };
        }
    }
}

impl NativeDevice for AutoShift {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        let frames = audio.frames();
        let channels = audio.channels().min(2);
        if frames == 0 || channels == 0 {
            return;
        }
        let ix: [usize; PARAMS] = core::array::from_fn(|id| {
            params
                .index(ParamId(id as u32))
                .expect("the description lists every parameter")
        });
        let ramp = |id: ParamId| params.ramp(ix[id.0 as usize]);
        let value = |id: ParamId| params.value(ix[id.0 as usize]);
        let on = |id: ParamId| value(id) >= 0.5;
        let rate = self.rate;
        let ms = |id: ParamId| params.target(ix[id.0 as usize]);
        let depth = |id: ParamId| params.target(ix[id.0 as usize]) / 100.0;
        let flips = (0..12).fold(0u16, |mask, key| {
            mask | u16::from(on(ParamId(KEY_C.0 + key))) << key
        });
        let set = Settings {
            midi: on(MIDI_ON),
            poly: value(MONO_POLY) >= 0.5,
            voices: VOICE_COUNTS[(value(VOICE_COUNT) as usize).min(VOICE_COUNTS.len() - 1)],
            latch: on(NOTE_LATCH),
            live: on(LIVE_MODE),
            reset: on(LFO_RESET),
            sync: on(LFO_SYNC),
            smooth: on(SMOOTH),
            natural: on(NATURAL_VIBRATO),
            flips,
            use_scale: use_current_scale(audio, params, USE_SCALE),
            chosen: Scale::new(
                (value(ROOT) as u8).min(11),
                ScaleName::ALL[(value(SCALE) as usize).min(ScaleName::ALL.len() - 1)],
            ),
            glide: ms(GLIDE) / 1_000.0 * rate,
            attack: ms(ATTACK) / 1_000.0 * rate,
            release: ms(RELEASE) / 1_000.0 * rate,
            bend_range: ms(BEND_RANGE),
            wave: (value(LFO_WAVE) as usize).min(WAVES.len() - 1),
            division: division(value(LFO_DIVISION) as usize).1,
            sources: core::array::from_fn(|n| {
                (value(ParamId(PITCH_SOURCE.0 + 2 * n as u32)) as usize).min(SOURCES.len() - 1)
            }),
            depths: core::array::from_fn(|n| depth(ParamId(PITCH_DEPTH.0 + 2 * n as u32))),
            lfo_depths: core::array::from_fn(|n| depth(ParamId(LFO_PITCH.0 + n as u32))),
            lfo_ms: ms(LFO_RATE),
            lfo_delay: ms(LFO_DELAY),
            lfo_attack: ms(LFO_ATTACK),
            gain: ramp(INPUT_GAIN),
            strength: ramp(STRENGTH),
            smoothing: ramp(SMOOTHING_TIME),
            degrees: ramp(DEGREES),
            shift: ramp(PITCH_SHIFT),
            fine: ramp(FINE),
            formant: ramp(FORMANT_SHIFT),
            follow: ramp(FORMANT_FOLLOW),
            vibrato: ramp(VIBRATO_AMOUNT),
            vibrato_rate: ramp(VIBRATO_RATE),
            vibrato_fade: ramp(VIBRATO_FADE),
            wet: ramp(DRY_WET),
        };

        // The range, Live Mode and the voices in use.
        self.range = (value(PITCH_RANGE) as usize).min(2);
        if self.pools[self.range].live != set.live {
            self.pools[self.range].set_live(set.live);
        }
        if !set.midi || !set.poly {
            // Mono, and the Quantizer, use the first voice alone.
            for note in &mut self.notes[1..] {
                note.down = false;
                note.latched = false;
            }
        } else {
            for note in &mut self.notes[set.voices..] {
                note.down = false;
                note.latched = false;
            }
        }
        if self.midi_was && !set.midi {
            for note in &mut self.notes {
                note.down = false;
                note.latched = false;
            }
            self.held_len = 0;
        }
        self.midi_was = set.midi;

        let lfo_rate = if set.sync {
            Rate::Beats(set.division)
        } else {
            Rate::Seconds(set.lfo_ms / 1_000.0)
        };
        self.lfo.set(audio, lfo_rate, Stereo::Phase(0.0), 0.0);
        let shape = Shape::of(WAVE_OF[set.wave]);
        let tempo = audio.tempo();
        self.events.clear();
        if set.midi {
            for note in audio.notes() {
                if self.events.len() < self.events.capacity() {
                    self.events.push(note);
                }
            }
        }

        let latency = self.pools[self.range].latency();
        let range = self.range;
        let mut next = 0;
        for i in 0..frames {
            while let Some(&event) = self.events.get(next).filter(|n| n.offset as usize <= i) {
                self.note(&event, &set);
                next += 1;
            }
            let scale = if set.use_scale {
                audio.scale_at(i as u32)
            } else {
                set.chosen
            };
            let mask = self.mask(scale, set.flips);
            self.shown_mask = mask;
            let (left, right) = (audio.channel(0)[i], audio.channel(channels - 1)[i]);
            let heard = (left + right) / 2.0 * set.gain[i];
            let lfo = self.lfo.step(tempo, i, shape)[0];
            self.step_notes(&set);
            let factors = self.factors(&set, i, lfo);
            let quant = Quant {
                mask,
                degrees: set.degrees[i].round().clamp(-7.0, 7.0) as i8,
                strength: (set.strength[i] / 100.0).clamp(0.0, 1.0),
                smooth: if set.smooth {
                    set.smoothing[i] / 1_000.0
                } else {
                    0.0
                },
            };

            // The shifters hear the sample, the tracker measures it, and
            // each voice's grains are laid.
            let pool = &mut self.pools[range];
            let qmask = pool.quant.len() - 1;
            pool.quant[pool.count as usize & qmask] = quant;
            pool.count += 1;
            for (shifter, f) in pool.shifters.iter_mut().zip(&factors) {
                shifter.push(heard, f.ask);
            }
            pool.tracker.feed(heard);
            let mut onset = false;
            while let Some(found) = pool.tracker.pop() {
                for shifter in &mut pool.shifters {
                    shifter.take(found);
                }
                match found.filter(|p| p.voiced) {
                    Some(p) => {
                        self.voiced = true;
                        self.heard = note_of(p.frequency);
                        onset |= p.onset;
                    }
                    None => {
                        self.voiced = false;
                        self.aimed = false;
                    }
                }
            }
            // shortcut: all eight voices run, whatever is in use, so each stays in
            // step with the tracker; the cost is a few tens of operations a sample.
            let mut wet = [0.0f32; VOICES];
            {
                let Pool {
                    shifters, quant, ..
                } = pool;
                for (v, shifter) in shifters.iter_mut().enumerate() {
                    let midi = set.midi;
                    // The display follows the first voice.
                    let mut scratch = self.tune;
                    let tune = if v == 0 { &mut self.tune } else { &mut scratch };
                    wet[v] = shifter.next(&mut |grain, ask| {
                        if midi {
                            follow_note(tune, grain, ask)
                        } else if v == 0 {
                            quantise(tune, &quant[grain.at as usize & qmask], rate, grain, ask)
                        } else {
                            [1.0, 1.0]
                        }
                    });
                }
            }
            if self.voiced && wet[0] != 0.0 {
                self.aimed = true;
            }
            if onset && !set.midi {
                self.restart(set.reset);
            }

            // What the voices make, from a latency ago: each one's level and
            // place, then the dry signal in step with them.
            let (mut out_left, mut out_right) = (0.0f32, 0.0f32);
            let dry = [self.dry[0].read(latency), self.dry[1].read(latency)];
            let mono = (dry[0] + dry[1]) / 2.0;
            for (v, shifter_out) in wet.iter().enumerate() {
                let gain = self.gains[v].read(latency);
                let pan = f64::from(self.pans[v].read(latency));
                let angle = (pan + 1.0) * FRAC_PI_4;
                let (sin, cos) = angle.sin_cos();
                let level = shifter_out * gain;
                out_left += level * (cos * SQRT_2) as f32;
                out_right += level * (sin * SQRT_2) as f32;
                self.gains[v].write(factors[v].gain);
                self.pans[v].write(factors[v].pan);
            }
            let leftover = self.spare.read(latency);
            let envs: f32 = if set.midi {
                self.notes
                    .iter()
                    .take(if set.poly { set.voices } else { 1 })
                    .map(|n| n.env)
                    .sum()
            } else {
                1.0
            };
            self.spare.write((1.0 - envs).clamp(0.0, 1.0));
            if set.midi {
                out_left += mono * leftover;
                out_right += mono * leftover;
            }
            self.dry[0].write(left);
            self.dry[1].write(right);
            let w = set.wet[i] / 100.0;
            if channels == 1 {
                let wet_mono = (out_left + out_right) / 2.0;
                audio.channel_mut(0)[i] = dry[0] * (1.0 - w) + wet_mono * w;
            } else {
                audio.channel_mut(0)[i] = dry[0] * (1.0 - w) + out_left * w;
                audio.channel_mut(1)[i] = dry[1] * (1.0 - w) + out_right * w;
            }

            self.lamp = self.lamp.saturating_sub(1);
            self.pos += 1;
            if self.column_left == 0 {
                self.column_left = self.column_frames;
                let heard = self.voiced.then_some(self.heard as f32);
                self.detected[self.head] = heard.unwrap_or(NO_PITCH);
                self.target[self.head] = if self.voiced && self.aimed {
                    self.tune.shown as f32
                } else {
                    NO_PITCH
                };
                self.head = (self.head + 1) % COLUMNS;
            }
            self.column_left -= 1;
        }
    }

    fn latency(&self) -> u32 {
        self.pools[self.range].latency() as u32
    }

    fn display_len(&self) -> usize {
        DISPLAY_LEN
    }

    fn display(&mut self, out: &mut [f32]) {
        let (newer, older) = self.detected.split_at(self.head);
        out[..older.len()].copy_from_slice(older);
        out[older.len()..COLUMNS].copy_from_slice(newer);
        let (newer, older) = self.target.split_at(self.head);
        out[COLUMNS..COLUMNS + older.len()].copy_from_slice(older);
        out[COLUMNS + older.len()..2 * COLUMNS].copy_from_slice(newer);
        let tail = &mut out[2 * COLUMNS..];
        let hz = self.voiced.then(|| hz_of(self.heard));
        tail[0] = f32::from(u8::from(self.voiced));
        tail[1] = if self.voiced { self.heard as f32 } else { 0.0 };
        tail[2] = if self.voiced && self.aimed {
            self.tune.shown as f32
        } else {
            0.0
        };
        tail[3] = if self.voiced && self.aimed {
            ((self.tune.shown - self.tune.heard) * 100.0) as f32
        } else {
            0.0
        };
        tail[4] = (self.pools[self.range].latency() as f64 / self.rate * 1_000.0) as f32;
        tail[5] = f32::from(u8::from(self.lamp > 0));
        tail[6] = f32::from(self.shown_mask);
        tail[7] = f32::from(u8::from(self.midi_was));
        for (n, (low, high)) in RANGE_HERTZ.iter().enumerate() {
            tail[8 + n] = f32::from(u8::from(hz.is_some_and(|h| (*low..=*high).contains(&h))));
        }
    }
}

/// How a pitch reads in the display: its key and its cents from it.
fn pitch_text(note: f64) -> String {
    let key = note.round();
    format!("{} {:+.0} ct", note_name(key as i32), (note - key) * 100.0)
}

/// What the display shows, from the `data` its node published and the
/// parameters' `values` (R16, R17).
#[must_use]
pub fn display(
    device: DeviceId,
    description: &DeviceDescription,
    values: &OrdMap<ParamId, f64>,
    data: Option<&[f32]>,
) -> DeviceDisplay {
    let data = data.filter(|d| d.len() == DISPLAY_LEN);
    let value = |id: ParamId| description.value(values, id).unwrap_or(0.0);
    let flips = (0..12).fold(0u16, |mask, key| {
        mask | u16::from(value(ParamId(KEY_C.0 + key)) >= 0.5) << key
    });
    let mask = data.map_or_else(
        || {
            mask_of(Scale::new(
                (value(ROOT) as u8).min(11),
                ScaleName::ALL[(value(SCALE) as usize).min(ScaleName::ALL.len() - 1)],
            )) ^ flips
        },
        |d| d[2 * COLUMNS + 6] as u16,
    );
    let series = |name: &str, values: Vec<f32>| DisplaySeries {
        name: name.to_owned(),
        unit: "notes".to_owned(),
        x_unit: "s".to_owned(),
        x_range: [-(HISTORY_SECONDS as f32), 0.0],
        log_x: false,
        y_range: NOTE_RANGE,
        detail: false,
        values,
    };
    let mut readings = Vec::new();
    let (detected, corrected) = data.map_or((Vec::new(), Vec::new()), |d| {
        (d[..COLUMNS].to_vec(), d[COLUMNS..2 * COLUMNS].to_vec())
    });
    if let Some(d) = data {
        let tail = &d[2 * COLUMNS..];
        let voiced = tail[0] >= 0.5;
        let mut reading = |name: &str, value: f32, unit: &str, text: String| {
            readings.push(DisplayReading {
                name: name.to_owned(),
                value,
                unit: unit.to_owned(),
                text,
            });
        };
        reading(
            "Voiced",
            tail[0],
            "",
            if voiced { "voiced" } else { "unvoiced" }.to_owned(),
        );
        reading(
            "Input Pitch",
            tail[1],
            "notes",
            if voiced {
                pitch_text(f64::from(tail[1]))
            } else {
                "none".to_owned()
            },
        );
        let aimed = voiced && tail[2] > 0.0;
        reading(
            "Target Pitch",
            tail[2],
            "notes",
            if aimed {
                pitch_text(f64::from(tail[2]))
            } else {
                "none".to_owned()
            },
        );
        reading("Correction", tail[3], "ct", format!("{:+.0} ct", tail[3]));
        reading("Latency", tail[4], "ms", format!("{:.1} ms", tail[4]));
        reading(
            "LFO Reset",
            tail[5],
            "",
            if tail[5] >= 0.5 { "on" } else { "off" }.to_owned(),
        );
        for (n, name) in PITCH_RANGES.iter().enumerate() {
            reading(
                &format!("{name} Range"),
                tail[8 + n],
                "",
                if tail[8 + n] >= 0.5 { "on" } else { "off" }.to_owned(),
            );
        }
    }
    // The piano ruler: a mark at the left edge for each key of the scale.
    let markers = (NOTE_RANGE[0] as i32..=NOTE_RANGE[1] as i32)
        .filter(|&n| allowed(mask, n))
        .map(|n| DisplayMarker {
            name: format!("Scale {}", note_name(n)),
            series: DETECTED.to_owned(),
            x: -(HISTORY_SECONDS as f32),
            y: n as f32,
            text: format!("{} is in the scale", note_name(n)),
            handle: None,
        })
        .collect();
    DeviceDisplay {
        device,
        live: data.is_some(),
        view: None,
        series: vec![series(DETECTED, detected), series(CORRECTED, corrected)],
        spectrograms: Vec::new(),
        markers,
        readings,
    }
}

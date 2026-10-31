//! Echo, the modulated two-line delay (WP-8.37; the echo spec): two delay
//! lines with synced or free times, three channel modes, input drive and
//! inverted feedback, a high-pass and a low-pass in the loop, an LFO and an
//! envelope that move the time and the filters, a gate, ducking, noise,
//! wobble and Repitch, a small reverb before the lines, after them or in the
//! feedback loop, and a tunnel and a filter display.
//!
//! The lines, the loop's limit, the reverb and the characters are
//! Beatlayer's own: the manual gives Echo's controls but no ranges, laws or
//! defaults (`docs/specs/echo.md`).

use core::f64::consts::{FRAC_PI_2, TAU};

use bl_core::time::TempoMap;
use bl_dsp::{EnvelopeFollower, Random};
use bl_engine::{DeviceType, NodeConfig, Registry};
use bl_model::device_layout::{
    BODY_HEIGHT, ControlBand, DisplayCell, LayoutNode, Page, PageStyle, PagesNode, TAB_HEIGHT, Widget,
};
use bl_model::{
    Device, DeviceDescription, DeviceDisplay, DeviceId, DeviceLayout, DeviceRole, DisplayHandle,
    DisplayMarker, DisplaySeries, OrdMap, ParamDescriptor, ParamId, ParamUnit,
};

use crate::framework::{Audio, NativeDevice, ParamBank, add};
use crate::lfo::{Lfo, Rate, Shape, Stereo, Wave};
use crate::reverb::{Allpass, Comb, Damping};
use crate::sections::{Biquad, Coeffs};

/// Echo's device type.
pub const ECHO: &str = "beatlayer.echo";

/// Channel Mode: Stereo, Ping Pong or Mid/Side (R4).
pub const CHANNEL_MODE: ParamId = ParamId(0);
/// Stereo Link: the right line takes the left line's time (R3).
pub const LINK: ParamId = ParamId(1);

/// A line's Sync switch, as its offset among the line's parameters.
pub const SYNC: u32 = 0;
/// Sync Mode: Notes, Triplet, Dotted or 16th.
pub const SYNC_MODE: u32 = 1;
/// The division in Notes mode.
pub const NOTES: u32 = 2;
/// The division in Triplet mode.
pub const TRIPLET: u32 = 3;
/// The division in Dotted mode.
pub const DOTTED: u32 = 4;
/// The count of sixteenth notes in 16th mode.
pub const SIXTEENTHS: u32 = 5;
/// The free time, in milliseconds.
pub const TIME: u32 = 6;
/// Delay Offset: the synced time's swing, in per cent of it.
pub const OFFSET: u32 = 7;
/// Parameters in each line.
const FIELDS: u32 = 8;

/// Parameter `field` of line `line` (0 for the left or mid line, 1 for the
/// right or side line).
#[must_use]
pub const fn line_param(line: usize, field: u32) -> ParamId {
    ParamId(2 + line as u32 * FIELDS + field)
}

/// Input: a gain on the signal entering the lines, down to −∞ dB (R5).
pub const INPUT: ParamId = ParamId(18);
/// D: the soft clip after Input.
pub const DRIVE: ParamId = ParamId(19);
/// Feedback, in per cent (R6).
pub const FEEDBACK: ParamId = ParamId(20);
/// Ø: inverts the fed-back signal.
pub const FEEDBACK_INVERT: ParamId = ParamId(21);
/// Filter: the two filters are in the loop (R7).
pub const FILTER: ParamId = ParamId(22);
/// The high-pass's cutoff.
pub const HP_FREQ: ParamId = ParamId(23);
/// The high-pass's resonance.
pub const HP_RES: ParamId = ParamId(24);
/// The low-pass's cutoff.
pub const LP_FREQ: ParamId = ParamId(25);
/// The low-pass's resonance.
pub const LP_RES: ParamId = ParamId(26);
/// The LFO's waveform (R9).
pub const WAVE: ParamId = ParamId(27);
/// LFO Sync: the rate is a division rather than a frequency.
pub const LFO_SYNC: ParamId = ParamId(28);
/// The synced LFO's rate.
pub const LFO_RATE: ParamId = ParamId(29);
/// The free LFO's frequency.
pub const LFO_FREQ: ParamId = ParamId(30);
/// Phase: how far ahead of the left line's LFO the right line's is.
pub const PHASE: ParamId = ParamId(31);
/// Mod Delay: how far the source moves the delay times (R10).
pub const MOD_DELAY: ParamId = ParamId(32);
/// Modulation ×4: multiplies Mod Delay's depth by four.
pub const MOD_X4: ParamId = ParamId(33);
/// Mod Filter: how far the source moves the filters' cutoffs.
pub const MOD_FILTER: ParamId = ParamId(34);
/// Env Mix: the LFO alone at 0 %, the envelope alone at 100 %.
pub const ENV_MIX: ParamId = ParamId(35);
/// Gate: the signal entering the lines passes only above its threshold (R11).
pub const GATE: ParamId = ParamId(36);
/// The gate's threshold, in dBFS.
pub const GATE_THRESHOLD: ParamId = ParamId(37);
/// How long the gate takes to close, in milliseconds.
pub const GATE_RELEASE: ParamId = ParamId(38);
/// Ducking: the wet signal is lowered while the input is loud (R12).
pub const DUCKING: ParamId = ParamId(39);
/// Ducking's threshold, in dBFS.
pub const DUCK_THRESHOLD: ParamId = ParamId(40);
/// How long the wet signal takes to come back, in milliseconds.
pub const DUCK_RELEASE: ParamId = ParamId(41);
/// Noise: a hiss inside the loop (R13).
pub const NOISE: ParamId = ParamId(42);
/// The hiss's level.
pub const NOISE_AMOUNT: ParamId = ParamId(43);
/// The hiss's colour, from white to a fall above 1 kHz.
pub const NOISE_MORPH: ParamId = ParamId(44);
/// Wobble: a slow irregular motion of the delay time.
pub const WOBBLE: ParamId = ParamId(45);
/// How far the delay time moves.
pub const WOBBLE_AMOUNT: ParamId = ParamId(46);
/// The balance from the slow motion to the fast one.
pub const WOBBLE_MORPH: ParamId = ParamId(47);
/// Repitch: a change of time glides the pitch (R14).
pub const REPITCH: ParamId = ParamId(48);
/// Reverb: how much of the small reverb is mixed in (R15).
pub const REVERB: ParamId = ParamId(49);
/// The reverb's location: Pre, Post or Feedback (R16).
pub const REVERB_LOCATION: ParamId = ParamId(50);
/// The reverb's reverberation time.
pub const DECAY: ParamId = ParamId(51);
/// Stereo: the wet signal's width (R17).
pub const STEREO: ParamId = ParamId(52);
/// Output: a gain on the wet signal.
pub const OUTPUT: ParamId = ParamId(53);
/// Dry/Wet.
pub const DRY_WET: ParamId = ParamId(54);
/// Equal-Loudness: Dry/Wet follows sine and cosine laws.
pub const EQUAL_LOUDNESS: ParamId = ParamId(55);

/// How many parameters Echo has.
const PARAMS: usize = 56;

/// The channel modes, in the order of the chooser.
const MODES: [&str; 3] = ["Stereo", "Ping Pong", "Mid/Side"];
/// The Sync Modes.
const SYNC_MODES: [&str; 4] = ["Notes", "Triplet", "Dotted", "16th"];
/// The divisions of Notes, Triplet and Dotted: their names and their
/// lengths in quarter notes. A bar is four quarter notes.
const DIVISIONS: [(&str, f64); 11] = [
    ("1/64", 0.0625),
    ("1/32", 0.125),
    ("1/16", 0.25),
    ("1/8", 0.5),
    ("1/4", 1.0),
    ("1/2", 2.0),
    ("1 bar", 4.0),
    ("2 bars", 8.0),
    ("4 bars", 16.0),
    ("8 bars", 32.0),
    ("16 bars", 64.0),
];
/// How many divisions the LFO's synced rate offers: 1/64 note to 8 bars.
const RATE_DIVISIONS: usize = 10;
/// The LFO's waveforms, in the order of the chooser.
const WAVES: [(&str, Wave); 6] = [
    ("Sine", Wave::Sine),
    ("Triangle", Wave::Triangle),
    ("Sawtooth Up", Wave::SawUp),
    ("Sawtooth Down", Wave::SawDown),
    ("Square", Wave::Rectangle),
    ("Noise", Wave::Random),
];
/// The reverb's locations.
const LOCATIONS: [&str; 3] = ["Pre", "Post", "Feedback"];
/// The display's views, the first the one it starts in (R8).
const VIEWS: [&str; 2] = ["Tunnel", "Filter"];

/// The longest synced time in seconds: 16 bars dotted, with a +33 % offset,
/// a ×4 modulation and wobble at their deepest, at 20 BPM, the slowest tempo
/// the transport allows (R18).
const MAX_SYNC_SECONDS: f64 = 420.0;
/// The longest free time, in seconds.
const MAX_FREE_SECONDS: f64 = 5.0;
/// Mod Delay's depth at 100 %, as a share of the time (R10).
const MOD_DEPTH: f64 = 0.02;
/// Mod Filter's depth at 100 %, in octaves.
const FILTER_OCTAVES: f64 = 2.0;
/// Wobble's depth at 100 %, as a share of the time (R13).
const WOBBLE_DEPTH: f64 = 0.005;
/// Wobble's slow and fast sines, in hertz, and its random line's rate.
const WOBBLE_HZ: [f64; 3] = [0.5, 3.3, 2.0];
/// How long a change of time takes with Repitch on and off, in seconds (R14).
const REPITCH_SECONDS: f64 = 0.2;
const CROSSFADE_SECONDS: f64 = 0.05;
/// The gate's opening time, in seconds, and the detector's attack and
/// release (R11).
const OPEN_SECONDS: f64 = 0.001;
const DETECTOR_ATTACK: f64 = 0.001;
const DETECTOR_RELEASE: f64 = 0.01;
/// The envelope's attack and release for the modulation source (R10).
const ENVELOPE_ATTACK: f64 = 0.01;
const ENVELOPE_RELEASE: f64 = 0.2;
/// The most ducking lowers the wet signal, in dB (R12).
const DUCK_DEPTH_DB: f64 = 30.0;
/// The noise's level at Amount 100 %, as a peak (R13): −40 dBFS.
const NOISE_PEAK: f64 = 0.01;
/// Where the noise's fall begins, in hertz.
const NOISE_CORNER_HZ: f64 = 1_000.0;
/// The loop's limit starts to bend at this level and holds below 1 (R6).
const KNEE: f64 = 0.5;
/// Frames between updates of the filters' coefficients.
const COEFF_GRID: u64 = 32;
/// The filter's response is drawn at this many places, at the rate the
/// display assumes (a type's display doesn't know the rate).
const CURVE_POINTS: usize = 96;
const DISPLAY_RATE: f64 = 48_000.0;
/// How many echoes the tunnel draws for each line (R8).
const ECHOES: usize = 16;
/// The tunnel's floor: an echo below it isn't drawn, in dB.
const FLOOR_DB: f64 = -60.0;

/// The names of a line's parameters.
fn line_name(line: usize, name: &str) -> String {
    format!("{} {name}", ["L", "R"][line])
}

/// A line's descriptors, named for the line.
fn line_descriptors(line: usize) -> Vec<ParamDescriptor> {
    let id = |field| line_param(line, field);
    let names: Vec<&str> = DIVISIONS.iter().map(|d| d.0).collect();
    vec![
        ParamDescriptor::toggle(id(SYNC), &line_name(line, "Sync"), true)
            .describe("Sets the line's time in beat divisions rather than milliseconds."),
        ParamDescriptor::choice(id(SYNC_MODE), &line_name(line, "Sync Mode"), &SYNC_MODES, 0)
            .describe("Counts the line's synced time in notes, triplets, dotted notes or sixteenths."),
        ParamDescriptor::choice(id(NOTES), &line_name(line, "Notes"), &names, 3)
            .describe("The line's time in notes while Sync Mode is Notes."),
        ParamDescriptor::choice(id(TRIPLET), &line_name(line, "Triplet"), &names, 3)
            .describe("The line's time in triplets while Sync Mode is Triplet."),
        ParamDescriptor::choice(id(DOTTED), &line_name(line, "Dotted"), &names, 3)
            .describe("The line's time in dotted notes while Sync Mode is Dotted."),
        ParamDescriptor::integer(
            id(SIXTEENTHS),
            &line_name(line, "16th"),
            (1, 16),
            3,
            ParamUnit::None,
        )
        .describe("The line's time in sixteenth notes while Sync Mode is 16th."),
        ParamDescriptor::continuous(
            id(TIME),
            &line_name(line, "Time"),
            (1.0, 5_000.0),
            250.0,
            ParamUnit::Milliseconds,
        )
        .logarithmic()
        .describe("The line's time in milliseconds while Sync is off."),
        ParamDescriptor::continuous(
            id(OFFSET),
            &line_name(line, "Offset"),
            (-33.0, 33.0),
            0.0,
            ParamUnit::Percent,
        )
        .describe("Moves the line's echoes later or earlier, as a swing, while Sync is on."),
    ]
}

/// Echo's device type.
#[must_use]
pub fn echo_type() -> DeviceType {
    let rates: Vec<&str> = DIVISIONS[..RATE_DIVISIONS].iter().map(|d| d.0).collect();
    let waves: Vec<&str> = WAVES.iter().map(|w| w.0).collect();
    let mut params = vec![
        ParamDescriptor::choice(CHANNEL_MODE, "Channel Mode", &MODES, 0).describe(
            "Stereo runs each channel through its line, Ping Pong bounces the echoes between the sides, Mid/Side runs the mid and the side through the lines.",
        ),
        ParamDescriptor::toggle(LINK, "Stereo Link", false)
            .describe("Gives the right line the left line's Sync, Sync Mode and time."),
    ];
    params.extend(line_descriptors(0));
    params.extend(line_descriptors(1));
    params.extend([
        ParamDescriptor::gain(INPUT, "Input", 12.0, 0.0)
            .describe("The level of the signal entering the delay lines, down to silence."),
        ParamDescriptor::toggle(DRIVE, "Input Drive", false)
            .describe("D: soft-clips the signal after Input, before the lines and on the dry path."),
        ParamDescriptor::continuous(FEEDBACK, "Feedback", (0.0, 100.0), 40.0, ParamUnit::Percent)
            .describe("How much of each line's output returns to its input."),
        ParamDescriptor::toggle(FEEDBACK_INVERT, "Feedback Invert", false)
            .describe("Ø: inverts the returned signal."),
        ParamDescriptor::toggle(FILTER, "Filter", false)
            .describe("Puts a high-pass and a low-pass in the loop."),
        ParamDescriptor::continuous(HP_FREQ, "HP Freq", (20.0, 20_000.0), 20.0, ParamUnit::Hertz)
            .logarithmic()
            .describe("The high-pass filter's cutoff."),
        ParamDescriptor::continuous(HP_RES, "HP Res", (0.0, 100.0), 0.0, ParamUnit::Percent)
            .describe("The high-pass filter's resonance."),
        ParamDescriptor::continuous(LP_FREQ, "LP Freq", (20.0, 20_000.0), 20_000.0, ParamUnit::Hertz)
            .logarithmic()
            .describe("The low-pass filter's cutoff."),
        ParamDescriptor::continuous(LP_RES, "LP Res", (0.0, 100.0), 0.0, ParamUnit::Percent)
            .describe("The low-pass filter's resonance."),
        ParamDescriptor::choice(WAVE, "Waveform", &waves, 0)
            .describe("The LFO's waveform."),
        ParamDescriptor::toggle(LFO_SYNC, "LFO Sync", false)
            .describe("Sets the LFO's rate in beat divisions rather than hertz."),
        ParamDescriptor::choice(LFO_RATE, "LFO Rate", &rates, 4)
            .describe("The LFO's rate in beat divisions while LFO Sync is on."),
        ParamDescriptor::continuous(LFO_FREQ, "LFO Freq", (0.01, 40.0), 1.0, ParamUnit::Hertz)
            .logarithmic()
            .describe("The LFO's frequency while LFO Sync is off."),
        ParamDescriptor::continuous(PHASE, "Phase", (0.0, 360.0), 0.0, ParamUnit::None)
            .decimals(0)
            .describe("How far, in degrees, the right line's LFO is ahead of the left's."),
        ParamDescriptor::continuous(MOD_DELAY, "Mod Delay", (0.0, 100.0), 0.0, ParamUnit::Percent)
            .describe("How far the modulation source moves the delay times."),
        ParamDescriptor::toggle(MOD_X4, "Modulation \u{d7}4", false)
            .describe("Multiplies Mod Delay's depth by four."),
        ParamDescriptor::continuous(MOD_FILTER, "Mod Filter", (0.0, 100.0), 0.0, ParamUnit::Percent)
            .describe("How far the modulation source moves the filters' cutoffs."),
        ParamDescriptor::continuous(ENV_MIX, "Env Mix", (0.0, 100.0), 0.0, ParamUnit::Percent)
            .describe("Mixes the modulation source from the LFO alone to the input's envelope alone."),
        ParamDescriptor::toggle(GATE, "Gate", false)
            .describe("Lets the signal into the lines only while the input is above the threshold."),
        ParamDescriptor::continuous(
            GATE_THRESHOLD,
            "Gate Threshold",
            (-60.0, 0.0),
            -30.0,
            ParamUnit::Decibels,
        )
        .describe("The level the input must reach to open the gate."),
        ParamDescriptor::continuous(
            GATE_RELEASE,
            "Gate Release",
            (1.0, 1_000.0),
            100.0,
            ParamUnit::Milliseconds,
        )
        .logarithmic()
        .describe("How long the gate takes to close."),
        ParamDescriptor::toggle(DUCKING, "Ducking", false)
            .describe("Lowers the wet signal while the input is above the threshold."),
        ParamDescriptor::continuous(
            DUCK_THRESHOLD,
            "Ducking Threshold",
            (-60.0, 0.0),
            -30.0,
            ParamUnit::Decibels,
        )
        .describe("The level above which the input ducks the wet signal."),
        ParamDescriptor::continuous(
            DUCK_RELEASE,
            "Ducking Release",
            (1.0, 1_000.0),
            200.0,
            ParamUnit::Milliseconds,
        )
        .logarithmic()
        .describe("How long the wet signal takes to come back."),
        ParamDescriptor::toggle(NOISE, "Noise", false)
            .describe("Adds a hiss inside the loop."),
        ParamDescriptor::continuous(NOISE_AMOUNT, "Noise Amount", (0.0, 100.0), 50.0, ParamUnit::Percent)
            .describe("The level of the hiss."),
        ParamDescriptor::continuous(NOISE_MORPH, "Noise Morph", (0.0, 100.0), 0.0, ParamUnit::Percent)
            .describe("The hiss's colour, from white to a fall above 1 kHz."),
        ParamDescriptor::toggle(WOBBLE, "Wobble", false)
            .describe("Moves the delay time with a slow irregular motion."),
        ParamDescriptor::continuous(WOBBLE_AMOUNT, "Wobble Amount", (0.0, 100.0), 50.0, ParamUnit::Percent)
            .describe("How far the delay time moves."),
        ParamDescriptor::continuous(WOBBLE_MORPH, "Wobble Morph", (0.0, 100.0), 0.0, ParamUnit::Percent)
            .describe("The balance from the slow motion to the fast one."),
        ParamDescriptor::toggle(REPITCH, "Repitch", false)
            .describe("Glides the pitch of the echoes when the delay time changes, as tape does."),
        ParamDescriptor::continuous(REVERB, "Reverb", (0.0, 100.0), 0.0, ParamUnit::Percent)
            .describe("How much of the small reverb is mixed in."),
        ParamDescriptor::choice(REVERB_LOCATION, "Reverb Location", &LOCATIONS, 1)
            .describe("Puts the reverb before the lines, after them, or in the feedback loop."),
        ParamDescriptor::continuous(DECAY, "Decay", (0.2, 10.0), 1.0, ParamUnit::Seconds)
            .logarithmic()
            .describe("The reverb's reverberation time."),
        ParamDescriptor::continuous(STEREO, "Stereo", (0.0, 200.0), 100.0, ParamUnit::Percent)
            .describe("The width of the wet signal: mono at 0 %, wider above 100 %."),
        ParamDescriptor::gain(OUTPUT, "Output", 12.0, 0.0)
            .describe("The level of the wet signal, down to silence."),
        ParamDescriptor::continuous(DRY_WET, "Dry/Wet", (0.0, 100.0), 100.0, ParamUnit::Percent)
            .describe("The balance between the dry and the wet signal."),
        ParamDescriptor::toggle(EQUAL_LOUDNESS, "Equal-Loudness", false)
            .describe("Makes Dry/Wet keep the loudness steady across its range."),
    ]);
    DeviceType::native(ECHO, "Echo", DeviceRole::AudioEffect).with_description(
        DeviceDescription::new(params)
            .with_layout(layout())
            .with_views(&VIEWS)
            .with_menu_options(&[EQUAL_LOUDNESS])
            .with_banks(&[
                (
                    "Main",
                    &[
                        "Channel Mode",
                        "Stereo Link",
                        "L Sync",
                        "R Sync",
                        "Input",
                        "Feedback",
                        "Reverb",
                        "Dry/Wet",
                    ],
                ),
                (
                    "Left",
                    &[
                        "L Sync Mode",
                        "L Notes",
                        "L Triplet",
                        "L Dotted",
                        "L 16th",
                        "L Time",
                        "L Offset",
                    ],
                ),
                (
                    "Right",
                    &[
                        "R Sync Mode",
                        "R Notes",
                        "R Triplet",
                        "R Dotted",
                        "R 16th",
                        "R Time",
                        "R Offset",
                    ],
                ),
                (
                    "Filter",
                    &[
                        "Input Drive",
                        "Feedback Invert",
                        "Filter",
                        "HP Freq",
                        "HP Res",
                        "LP Freq",
                        "LP Res",
                    ],
                ),
                (
                    "Modulation",
                    &[
                        "Waveform",
                        "LFO Sync",
                        "LFO Rate",
                        "LFO Freq",
                        "Phase",
                        "Env Mix",
                    ],
                ),
                (
                    "Modulation Depth",
                    &["Mod Delay", "Modulation \u{d7}4", "Mod Filter"],
                ),
                (
                    "Character",
                    &[
                        "Gate",
                        "Gate Threshold",
                        "Gate Release",
                        "Ducking",
                        "Ducking Threshold",
                        "Ducking Release",
                        "Repitch",
                    ],
                ),
                (
                    "Character 2",
                    &[
                        "Noise",
                        "Noise Amount",
                        "Noise Morph",
                        "Wobble",
                        "Wobble Amount",
                        "Wobble Morph",
                    ],
                ),
                (
                    "Global",
                    &["Reverb Location", "Decay", "Stereo", "Output"],
                ),
            ]),
    )
}

/// The name of the node that holds the three tabs, which `device.set_page`
/// and the saved page name (R1).
pub const TABS: &str = "Tabs";

/// The layout: the left section, the three tabs (Echo with the display,
/// Modulation and Character) and the global controls.
fn layout() -> DeviceLayout {
    use Widget::{Chooser, Knob, Number, Toggle};
    let band = |height, controls: &[(ParamId, Widget)]| {
        LayoutNode::Band(ControlBand::stacked(height, controls))
    };
    let line = |n: usize| {
        band(
            BODY_HEIGHT,
            &[
                (line_param(n, SYNC), Toggle),
                (line_param(n, SYNC_MODE), Chooser),
                (line_param(n, NOTES), Chooser),
                (line_param(n, TRIPLET), Chooser),
                (line_param(n, DOTTED), Chooser),
                (line_param(n, SIXTEENTHS), Number),
                (line_param(n, TIME), Knob),
                (line_param(n, OFFSET), Knob),
            ],
        )
    };
    let tab = BODY_HEIGHT - TAB_HEIGHT;
    let tabs = PagesNode::new(
        TABS,
        PageStyle::Tabs,
        vec![
            Page::new(
                "Echo",
                LayoutNode::Row(vec![
                    LayoutNode::Cell(DisplayCell::new("Echo", 220.0, tab, 1.0)),
                    band(
                        tab,
                        &[
                            (FILTER, Toggle),
                            (HP_FREQ, Knob),
                            (HP_RES, Knob),
                            (LP_FREQ, Knob),
                            (LP_RES, Knob),
                        ],
                    ),
                ]),
            ),
            Page::new(
                "Modulation",
                LayoutNode::Row(vec![
                    band(
                        tab,
                        &[
                            (WAVE, Chooser),
                            (LFO_SYNC, Toggle),
                            (LFO_RATE, Chooser),
                            (LFO_FREQ, Knob),
                            (PHASE, Knob),
                        ],
                    ),
                    band(
                        tab,
                        &[
                            (MOD_DELAY, Knob),
                            (MOD_X4, Toggle),
                            (MOD_FILTER, Knob),
                            (ENV_MIX, Knob),
                        ],
                    ),
                ]),
            ),
            Page::new(
                "Character",
                LayoutNode::Row(vec![
                    band(
                        tab,
                        &[
                            (GATE, Toggle),
                            (GATE_THRESHOLD, Knob),
                            (GATE_RELEASE, Knob),
                            (DUCKING, Toggle),
                            (DUCK_THRESHOLD, Knob),
                            (DUCK_RELEASE, Knob),
                        ],
                    ),
                    band(
                        tab,
                        &[
                            (NOISE, Toggle),
                            (NOISE_AMOUNT, Knob),
                            (NOISE_MORPH, Knob),
                            (WOBBLE, Toggle),
                            (WOBBLE_AMOUNT, Knob),
                            (WOBBLE_MORPH, Knob),
                            (REPITCH, Toggle),
                        ],
                    ),
                ]),
            ),
        ],
    );
    DeviceLayout::new(LayoutNode::Row(vec![
        band(BODY_HEIGHT, &[(CHANNEL_MODE, Chooser), (LINK, Toggle)]),
        line(0),
        line(1),
        band(
            BODY_HEIGHT,
            &[
                (INPUT, Knob),
                (DRIVE, Toggle),
                (FEEDBACK, Knob),
                (FEEDBACK_INVERT, Toggle),
            ],
        ),
        LayoutNode::Pages(tabs),
        band(
            BODY_HEIGHT,
            &[
                (REVERB, Knob),
                (REVERB_LOCATION, Chooser),
                (DECAY, Knob),
                (STEREO, Knob),
                (OUTPUT, Knob),
                (DRY_WET, Knob),
            ],
        ),
    ]))
}

/// Adds Echo to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, echo_type(), Echo::new);
}

/// The channel mode.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Channels {
    Stereo,
    PingPong,
    MidSide,
}

/// Where the reverb sits.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Location {
    Pre,
    Post,
    Feedback,
}

/// One line's time as its parameters give it.
#[derive(Clone, Copy)]
struct LineTime {
    sync: bool,
    /// The synced time in quarter notes, before the offset.
    quarters: f64,
    ms: f64,
    offset: f64,
}

impl LineTime {
    /// The time in frames, `quarter` being the frames in a quarter note
    /// here. The offset applies only while the line is synced (R3).
    fn frames(&self, quarter: f64, rate: f64) -> f64 {
        if self.sync {
            self.quarters * (1.0 + self.offset / 100.0) * quarter
        } else {
            self.ms * rate / 1_000.0
        }
    }

    /// The time in milliseconds at `bpm`.
    fn milliseconds(&self, bpm: f64) -> f64 {
        self.frames(60_000.0 / bpm, 1_000.0)
    }
}

/// The index clamped into `DIVISIONS`.
fn division(index: f64) -> f64 {
    DIVISIONS[(index.round().max(0.0) as usize).min(DIVISIONS.len() - 1)].1
}

/// The two lines' times as `get` gives their parameters (R2, R3).
fn line_times(get: &dyn Fn(ParamId) -> f64) -> [LineTime; 2] {
    let line = |n: usize| {
        let at = |field| get(line_param(n, field));
        LineTime {
            sync: at(SYNC) >= 0.5,
            quarters: match at(SYNC_MODE).round() as usize {
                1 => division(at(TRIPLET)) * 2.0 / 3.0,
                2 => division(at(DOTTED)) * 1.5,
                3 => at(SIXTEENTHS).round() / 4.0,
                _ => division(at(NOTES)),
            },
            ms: at(TIME),
            offset: at(OFFSET),
        }
    };
    let left = line(0);
    let right = if get(LINK) >= 0.5 {
        LineTime {
            offset: get(line_param(1, OFFSET)),
            ..left
        }
    } else {
        line(1)
    };
    [left, right]
}

/// One stretch's settings: the switches and choices, the lines' times, and
/// each parameter's place in the bank.
struct Settings {
    channels: Channels,
    location: Location,
    drive: bool,
    invert: bool,
    filter: bool,
    lfo_sync: bool,
    x4: bool,
    gate: bool,
    ducking: bool,
    noise: bool,
    wobble: bool,
    repitch: bool,
    equal: bool,
    wave: Wave,
    rate_quarters: f64,
    times: [LineTime; 2],
    index: [usize; PARAMS],
}

impl Settings {
    fn new(params: &ParamBank, has_second: bool) -> Self {
        let index: [usize; PARAMS] = core::array::from_fn(|id| {
            params
                .index(ParamId(id as u32))
                .expect("the description lists every parameter")
        });
        let on = |id: ParamId| params.value(index[id.0 as usize]) >= 0.5;
        let pick = |id: ParamId| params.value(index[id.0 as usize]).round().max(0.0) as usize;
        let channels = match (pick(CHANNEL_MODE), has_second) {
            (1, true) => Channels::PingPong,
            (2, true) => Channels::MidSide,
            _ => Channels::Stereo,
        };
        Self {
            channels,
            location: match pick(REVERB_LOCATION) {
                0 => Location::Pre,
                2 => Location::Feedback,
                _ => Location::Post,
            },
            drive: on(DRIVE),
            invert: on(FEEDBACK_INVERT),
            filter: on(FILTER),
            lfo_sync: on(LFO_SYNC),
            x4: on(MOD_X4),
            gate: on(GATE),
            ducking: on(DUCKING),
            noise: on(NOISE),
            wobble: on(WOBBLE),
            repitch: on(REPITCH),
            equal: on(EQUAL_LOUDNESS),
            wave: WAVES[pick(WAVE).min(WAVES.len() - 1)].1,
            rate_quarters: DIVISIONS[pick(LFO_RATE).min(RATE_DIVISIONS - 1)].1,
            // A time is the parameter's target: the time-change modes do
            // the smoothing themselves.
            times: line_times(&|id| params.target(index[id.0 as usize])),
            index,
        }
    }

    /// A parameter's value at each frame of the stretch.
    fn ramp<'a>(&self, params: &'a ParamBank, id: ParamId) -> &'a [f32] {
        params.ramp(self.index[id.0 as usize])
    }
}

/// A soft limit: linear below the knee, bending toward 1 above it with the
/// same slope where they meet (R6).
fn limit(x: f64) -> f64 {
    let over = x.abs() - KNEE;
    if over <= 0.0 {
        x
    } else {
        x.signum() * (KNEE + (1.0 - KNEE) * (over / (1.0 - KNEE)).tanh())
    }
}

/// A delay line: a ring buffer read at a fractional delay through a cubic,
/// whose time changes as Repitch says (R14).
struct Line {
    buffer: Vec<f32>,
    write: usize,
    /// The delay in frames the line is reading at.
    delay: f64,
    /// A time change's start and end, and how far it has come.
    from: f64,
    to: f64,
    progress: f64,
    started: bool,
}

impl Line {
    fn new(len: usize) -> Self {
        Self {
            buffer: vec![0.0; len],
            write: 0,
            delay: 0.0,
            from: 0.0,
            to: 0.0,
            progress: 1.0,
            started: false,
        }
    }

    /// The delayed sample for this frame, moving to `base` over `glide`
    /// frames: the read position glides with Repitch, and without it the
    /// old time crossfades into the new.
    fn tap(&mut self, base: f64, modulation: f64, repitch: bool, glide: f64) -> f64 {
        if !self.started {
            self.started = true;
            self.delay = base;
            self.from = base;
            self.to = base;
            self.progress = 1.0;
        } else if (base - self.to).abs() > 1e-9 {
            self.from = self.delay;
            self.to = base;
            self.progress = 0.0;
        }
        if repitch {
            if self.progress < 1.0 {
                self.progress = (self.progress + 1.0 / glide).min(1.0);
                self.delay = self.from + (self.to - self.from) * self.progress;
            }
            return self.read(self.delay + modulation);
        }
        if self.progress >= 1.0 {
            self.delay = self.to;
            return self.read(self.to + modulation);
        }
        let old = self.read(self.from + modulation);
        let new = self.read(self.to + modulation);
        let value = old + (new - old) * self.progress;
        self.progress = (self.progress + 1.0 / glide).min(1.0);
        self.delay = self.to;
        value
    }

    /// Reads the line `delay` frames back through a four-point cubic, which
    /// keeps the aliasing of a gliding read well below the signal (R14).
    fn read(&self, delay: f64) -> f64 {
        let len = self.buffer.len();
        // Two frames at least: the cubic reads one past the sample it
        // interpolates from.
        let delay = delay.clamp(2.0, (len - 3) as f64);
        let position = (self.write as f64 - delay).rem_euclid(len as f64);
        let low = position.floor();
        let fraction = position - low;
        let i = low as usize % len;
        let at = |k: usize| f64::from(self.buffer[(i + len + k - 1) % len]);
        let (a, b, c, d) = (at(0), at(1), at(2), at(3));
        b + 0.5
            * fraction
            * (c - a + fraction * (2.0 * a - 5.0 * b + 4.0 * c - d + fraction * (3.0 * (b - c) + d - a)))
    }

    fn write(&mut self, sample: f32) {
        self.buffer[self.write] = sample;
        self.write = (self.write + 1) % self.buffer.len();
    }
}

/// A hiss with a colour: white, or white through a one-pole low-pass at
/// 1 kHz, whose fall is 6 dB an octave above it (R13).
struct Hiss {
    random: Random,
    low: f64,
    coefficient: f64,
}

impl Hiss {
    fn new(seed: u64, rate: f64) -> Self {
        Self {
            random: Random::seeded(seed),
            low: 0.0,
            coefficient: 1.0 - (-TAU * NOISE_CORNER_HZ / rate).exp(),
        }
    }

    /// The next sample for `morph` (0 white, 1 the fall), at peak 1.
    fn next(&mut self, morph: f64) -> f64 {
        let white = self.random.next_bipolar();
        self.low += self.coefficient * (white - self.low);
        white + (self.low - white) * morph
    }
}

/// Echo's small reverb (R15): per channel two allpass stages into four
/// combs, with every comb's feedback set so the tail falls 60 dB in Decay,
/// and its output scaled to the energy of its input.
struct Verb {
    allpass: [[Allpass; 2]; 2],
    combs: [[Comb; 4]; 2],
    scale: f64,
    rate: f64,
}

/// The allpass and comb delays at 44.1 kHz, in samples, and the right
/// channel's offset on the combs.
const ALLPASS_SAMPLES: [f64; 2] = [225.0, 341.0];
const COMB_SAMPLES: [f64; 4] = [1_116.0, 1_188.0, 1_277.0, 1_356.0];
const SPREAD_SAMPLES: f64 = 23.0;

impl Verb {
    fn new(rate: f64) -> Self {
        let scale = rate / 44_100.0;
        let allpass = (ALLPASS_SAMPLES[1] * scale).ceil() as usize + 4;
        let comb = ((COMB_SAMPLES[3] + SPREAD_SAMPLES) * scale).ceil() as usize + 4;
        Self {
            allpass: core::array::from_fn(|_| core::array::from_fn(|_| Allpass::new(allpass))),
            combs: core::array::from_fn(|_| core::array::from_fn(|_| Comb::new(comb))),
            scale,
            rate,
        }
    }

    /// The pair `input` with `amount` (0 to 1) of the reverb mixed in, for
    /// a reverberation time of `decay` seconds.
    fn process(&mut self, input: [f64; 2], amount: f64, decay: f64) -> [f64; 2] {
        let mut out = [0.0; 2];
        for (channel, out) in out.iter_mut().enumerate() {
            let mut diffused = input[channel] as f32;
            for (stage, base) in ALLPASS_SAMPLES.iter().enumerate() {
                diffused = self.allpass[channel][stage].process(diffused, (base * self.scale).round(), 0.5);
            }
            let mut sum = 0.0;
            for (k, base) in COMB_SAMPLES.iter().enumerate() {
                let spread = if channel == 1 { SPREAD_SAMPLES } else { 0.0 };
                let delay = ((base + spread) * self.scale).round();
                // 60 dB down after `decay` seconds: 10^(−3 · seconds / decay).
                let feedback = (-3.0 * core::f64::consts::LN_10 * delay / self.rate / decay).exp();
                let comb = self.combs[channel][k].process(
                    diffused,
                    delay,
                    feedback as f32,
                    Damping::default(),
                );
                sum += f64::from(comb) * (1.0 - feedback * feedback).sqrt();
            }
            *out = (1.0 - amount) * input[channel] + amount * sum / 2.0;
        }
        out
    }
}

/// Echo: the two lines, the loop's filters, the modulation, the characters
/// and the reverb.
struct Echo {
    lines: [Line; 2],
    highpass: [Biquad; 2],
    lowpass: [Biquad; 2],
    lfo: Lfo,
    wobble_lfo: Lfo,
    /// Wobble's slow and fast sines' places in their cycles.
    wobble_phase: [f64; 2],
    /// Reads the input's level for the gate and ducking, and for the
    /// modulation's envelope.
    detector: EnvelopeFollower,
    envelope: EnvelopeFollower,
    /// The gate's gain, and how far ducking has lowered the wet signal, in
    /// dB.
    gate: f64,
    duck_db: f64,
    hiss: [Hiss; 2],
    verb: Verb,
    /// Frames in a quarter note at each frame of the stretch, so a synced
    /// time follows a tempo ramp the same at any block size (R2).
    quarter: Vec<f64>,
    /// Frames processed so far: the filters' coefficients follow the
    /// parameters on a grid counted from it, so a render doesn't depend on
    /// the block size.
    frames_done: u64,
    rate: f64,
    /// The lines' times in milliseconds after the last block, for the
    /// display.
    shown: [f32; 2],
}

impl Echo {
    fn new(device: &Device, config: &NodeConfig) -> Self {
        let rate = config.sample_rate.as_f64();
        let len = ((MAX_SYNC_SECONDS.max(MAX_FREE_SECONDS) + 1.0) * rate).ceil() as usize + 4;
        let seed = device.id.0.get();
        let follower = |attack, release| {
            let mut follower = EnvelopeFollower::new(rate);
            follower.set(attack, release, false);
            follower
        };
        Self {
            lines: [Line::new(len), Line::new(len)],
            highpass: [Biquad::default(); 2],
            lowpass: [Biquad::default(); 2],
            lfo: Lfo::new(config.sample_rate, seed),
            wobble_lfo: Lfo::new(config.sample_rate, seed.wrapping_add(1)),
            wobble_phase: [0.0; 2],
            detector: follower(DETECTOR_ATTACK, DETECTOR_RELEASE),
            envelope: follower(ENVELOPE_ATTACK, ENVELOPE_RELEASE),
            gate: 0.0,
            duck_db: 0.0,
            hiss: [
                Hiss::new(seed.wrapping_add(2), rate),
                Hiss::new(seed.wrapping_add(3), rate),
            ],
            verb: Verb::new(rate),
            quarter: vec![0.0; config.max_frames.max(1)],
            frames_done: 0,
            rate,
            shown: [0.0; 2],
        }
    }
}

impl NativeDevice for Echo {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        let frames = audio.frames();
        if frames == 0 {
            return;
        }
        let settings = Settings::new(params, audio.channels() > 1);
        let rate = if settings.lfo_sync {
            Rate::Beats(settings.rate_quarters)
        } else {
            Rate::Hertz(params.target(settings.index[LFO_FREQ.0 as usize]))
        };
        self.lfo.set(audio, rate, Stereo::Phase(0.0), 0.0);
        self.wobble_lfo
            .set(audio, Rate::Hertz(WOBBLE_HZ[2]), Stereo::Phase(120.0), 0.0);
        for (frame, quarter) in self.quarter[..frames].iter_mut().enumerate() {
            *quarter = self.rate * 60.0 / audio.bpm_at(frame);
        }
        let tempo = audio.tempo();
        let mut channels = audio.channels_mut();
        let Some(first) = channels.next() else {
            return;
        };
        // shortcut: Echo is stereo, as the manual's is; any channel past
        // the second passes through. Upgrade with a surround layout.
        let second = channels.next();
        self.render(first, second, &settings, params, tempo);
    }

    fn display_len(&self) -> usize {
        2
    }

    fn display(&mut self, out: &mut [f32]) {
        out.copy_from_slice(&self.shown);
    }
}

impl Echo {
    /// Renders the first channel and, when there is one, the second.
    fn render(
        &mut self,
        first: &mut [f32],
        mut second: Option<&mut [f32]>,
        s: &Settings,
        params: &ParamBank,
        tempo: &TempoMap,
    ) {
        let rate = self.rate;
        let glide = (if s.repitch { REPITCH_SECONDS } else { CROSSFADE_SECONDS } * rate).max(1.0);
        let open_step = 1.0 / (OPEN_SECONDS * rate);
        for i in 0..first.len() {
            let ramp = |id: ParamId| f64::from(s.ramp(params, id)[i]);
            let dry_left = f64::from(first[i]);
            let dry_right = second.as_deref().map_or(dry_left, |right| f64::from(right[i]));
            let peak = dry_left.abs().max(dry_right.abs()) as f32;
            let level = self.detector.process(peak);
            let envelope = self.envelope.process(peak);

            // The input: Input, then D's clip, which the dry path hears too (R5).
            let input = ramp(INPUT);
            let (mut left, mut right) = (dry_left * input, dry_right * input);
            let (mut dry_left, mut dry_right) = (dry_left, dry_right);
            if s.drive {
                (left, right) = (left.tanh(), right.tanh());
                (dry_left, dry_right) = (left, right);
            }
            let (mut in_0, mut in_1) = match s.channels {
                Channels::MidSide => ((left + right) / 2.0, (left - right) / 2.0),
                _ => (left, right),
            };

            // The gate (R11): it opens in a millisecond and closes over Release.
            if s.gate {
                let open = level > 10f64.powf(ramp(GATE_THRESHOLD) / 20.0);
                let close = 1.0 / (ramp(GATE_RELEASE) / 1_000.0 * rate).max(1.0);
                self.gate = if open {
                    (self.gate + open_step).min(1.0)
                } else {
                    (self.gate - close).max(0.0)
                };
                in_0 *= self.gate;
                in_1 *= self.gate;
            }

            let reverb = ramp(REVERB) / 100.0;
            let decay = ramp(DECAY);
            if s.location == Location::Pre {
                [in_0, in_1] = self.verb.process([in_0, in_1], reverb, decay);
            }

            // The modulation source: the LFO and the envelope's level in dB (R9, R10).
            self.lfo.set_stereo(Stereo::Phase(ramp(PHASE)));
            let lfo = self.lfo.step(tempo, i, Shape::of(s.wave));
            let envelope = ((20.0 * envelope.max(1e-9).log10() + 60.0) / 60.0).clamp(0.0, 1.0);
            let mix = ramp(ENV_MIX) / 100.0;
            let source = lfo.map(|lfo| lfo * (1.0 - mix) + envelope * mix);
            let depth = ramp(MOD_DELAY) / 100.0 * MOD_DEPTH * if s.x4 { 4.0 } else { 1.0 };
            let motion = if s.wobble {
                self.wobble(s, params, tempo, i)
            } else {
                [0.0; 2]
            };

            // The lines' times and taps.
            let base = [
                s.times[0].frames(self.quarter[i], rate),
                s.times[1].frames(self.quarter[i], rate),
            ];
            let tap: [f64; 2] = core::array::from_fn(|k| {
                let modulation = base[k] * (depth * source[k] + motion[k]);
                self.lines[k].tap(base[k], modulation, s.repitch, glide)
            });

            // What returns to the lines (R4, R6, R16).
            let feedback = ramp(FEEDBACK) / 100.0 * if s.invert { -1.0 } else { 1.0 };
            let mut returned = match s.channels {
                Channels::PingPong => [feedback * tap[1]; 2],
                _ => [feedback * tap[0], feedback * tap[1]],
            };
            if s.location == Location::Feedback {
                returned = self.verb.process(returned, reverb, decay);
            }
            let returned = returned.map(limit);
            let write = match s.channels {
                Channels::PingPong => [in_0 + in_1 + returned[0], tap[0]],
                _ => [in_0 + returned[0], in_1 + returned[1]],
            };

            // The loop's hiss and filters, and into the lines (R7, R13).
            if self.frames_done.is_multiple_of(COEFF_GRID) {
                self.design(s, params, i, source);
            }
            self.frames_done += 1;
            let amount = if s.noise { ramp(NOISE_AMOUNT) / 100.0 * NOISE_PEAK } else { 0.0 };
            let morph = ramp(NOISE_MORPH) / 100.0;
            for k in 0..2 {
                let mut sample = write[k];
                if s.noise {
                    sample += amount * self.hiss[k].next(morph);
                }
                if s.filter {
                    sample = f64::from(
                        self.lowpass[k].process(self.highpass[k].process(sample as f32)),
                    );
                }
                self.lines[k].write(sample as f32);
            }

            // The wet signal: the lines' output, the reverb after them,
            // ducking, width, Output (R12, R16, R17).
            let mut wet = match s.channels {
                Channels::MidSide => [tap[0] + tap[1], tap[0] - tap[1]],
                _ => tap,
            };
            if s.location == Location::Post {
                wet = self.verb.process(wet, reverb, decay);
            }
            if s.ducking {
                let over = (20.0 * level.max(1e-9).log10() - ramp(DUCK_THRESHOLD)).max(0.0);
                let target = over.min(DUCK_DEPTH_DB);
                let seconds = if target > self.duck_db {
                    OPEN_SECONDS
                } else {
                    ramp(DUCK_RELEASE) / 1_000.0
                };
                self.duck_db += (target - self.duck_db) * (1.0 - (-1.0 / (seconds * rate)).exp());
                let gain = 10f64.powf(-self.duck_db / 20.0);
                wet = wet.map(|w| w * gain);
            }
            let width = ramp(STEREO) / 100.0;
            let (mid, side) = ((wet[0] + wet[1]) / 2.0, (wet[0] - wet[1]) / 2.0 * width);
            let output = ramp(OUTPUT);
            let wet = [(mid + side) * output, (mid - side) * output];

            // Dry/Wet, with Equal-Loudness's sine and cosine laws (R17).
            let balance = ramp(DRY_WET) / 100.0;
            let (dry_gain, wet_gain) = if s.equal {
                ((balance * FRAC_PI_2).cos(), (balance * FRAC_PI_2).sin())
            } else {
                (1.0 - balance, balance)
            };
            first[i] = (dry_left * dry_gain + wet[0] * wet_gain) as f32;
            if let Some(right) = second.as_deref_mut() {
                right[i] = (dry_right * dry_gain + wet[1] * wet_gain) as f32;
            }
        }
        self.shown = core::array::from_fn(|k| {
            s.times[k].milliseconds(60.0 / (self.quarter[0] / rate)) as f32
        });
    }

    /// Wobble's motion of each line's time, a share of it (R13): the slow
    /// sine against a mix of the fast sine and the random line, by Morph.
    fn wobble(&mut self, s: &Settings, params: &ParamBank, tempo: &TempoMap, i: usize) -> [f64; 2] {
        let ramp = |id: ParamId| f64::from(s.ramp(params, id)[i]);
        for (phase, hz) in self.wobble_phase.iter_mut().zip(WOBBLE_HZ) {
            *phase = (*phase + hz / self.rate).rem_euclid(1.0);
        }
        let random = self.wobble_lfo.step(tempo, i, Shape::of(Wave::Wander));
        let morph = ramp(WOBBLE_MORPH) / 100.0;
        let amount = ramp(WOBBLE_AMOUNT) / 100.0 * WOBBLE_DEPTH;
        core::array::from_fn(|k| {
            let offset = 0.25 * k as f64;
            let slow = (TAU * (self.wobble_phase[0] + offset)).sin();
            let fast = 0.5 * (TAU * (self.wobble_phase[1] + offset)).sin() + 0.5 * random[k];
            amount * (slow * (1.0 - morph) + fast * morph)
        })
    }

    /// Sets the filters for the cutoffs at frame `i`, moved by Mod Filter
    /// and the modulation source (R7, R10).
    fn design(&mut self, s: &Settings, params: &ParamBank, i: usize, source: [f64; 2]) {
        let ramp = |id: ParamId| f64::from(s.ramp(params, id)[i]);
        let octaves = ramp(MOD_FILTER) / 100.0 * FILTER_OCTAVES;
        for k in 0..2 {
            let moved = |id| {
                (ramp(id) * (octaves * source[k]).exp2()).clamp(20.0, self.rate * 0.45)
            };
            let (hp, lp) = if s.filter {
                (
                    coefficients(true, moved(HP_FREQ), q_of(ramp(HP_RES)), self.rate),
                    coefficients(false, moved(LP_FREQ), q_of(ramp(LP_RES)), self.rate),
                )
            } else {
                (PASS, PASS)
            };
            self.highpass[k].set(hp);
            self.lowpass[k].set(lp);
        }
    }
}

/// A section that passes its input as it is.
const PASS: Coeffs = Coeffs {
    b0: 1.0,
    b1: 0.0,
    b2: 0.0,
    a1: 0.0,
    a2: 0.0,
};

/// The Q a resonance of `percent` gives: 0.5 at 0 %, 10 at 100 %, evenly
/// in between on a log scale (R7).
fn q_of(percent: f64) -> f64 {
    0.5 * 20f64.powf(percent / 100.0)
}

/// The cookbook's high-pass or low-pass, `[b0, b1, b2, a1, a2]`, a0 being 1.
fn design(highpass: bool, hz: f64, q: f64, rate: f64) -> [f64; 5] {
    let w0 = TAU * hz / rate;
    let (sin, cos) = w0.sin_cos();
    let alpha = sin / (2.0 * q);
    let a0 = 1.0 + alpha;
    let edge = if highpass { 1.0 + cos } else { 1.0 - cos };
    let b1 = if highpass { -edge } else { edge };
    [
        edge / 2.0 / a0,
        b1 / a0,
        edge / 2.0 / a0,
        -2.0 * cos / a0,
        (1.0 - alpha) / a0,
    ]
}

fn coefficients(highpass: bool, hz: f64, q: f64, rate: f64) -> Coeffs {
    let [b0, b1, b2, a1, a2] = design(highpass, hz, q, rate);
    Coeffs {
        b0: b0 as f32,
        b1: b1 as f32,
        b2: b2 as f32,
        a1: a1 as f32,
        a2: a2 as f32,
    }
}

/// The size in dB of the response of `coefficients` at `hz`.
fn response_db([b0, b1, b2, a1, a2]: [f64; 5], hz: f64) -> f64 {
    let w = TAU * hz / DISPLAY_RATE;
    let (c1, s1, c2, s2) = (w.cos(), w.sin(), (2.0 * w).cos(), (2.0 * w).sin());
    let numerator = (b0 + b1 * c1 + b2 * c2).hypot(b1 * s1 + b2 * s2);
    let denominator = (1.0 + a1 * c1 + a2 * c2).hypot(a1 * s1 + a2 * s2);
    20.0 * (numerator / denominator).max(1e-9).log10()
}

/// Echo's display from its parameters and the times its node published
/// (R8): the tunnel's echoes and handles, or the filters' response and
/// handles.
#[must_use]
pub fn display(
    device: DeviceId,
    description: &DeviceDescription,
    values: &OrdMap<ParamId, f64>,
    data: Option<&[f32]>,
    view: Option<&str>,
) -> DeviceDisplay {
    let get = |id: ParamId| description.value(values, id).unwrap_or(0.0);
    let filter_view = view == Some(VIEWS[1]);
    let (series, markers) = if filter_view {
        filter_display(&get)
    } else {
        tunnel_display(&get, data)
    };
    DeviceDisplay {
        device,
        live: data.is_some(),
        view: Some(VIEWS[usize::from(filter_view)].to_owned()),
        series: vec![series],
        readings: Vec::new(),
        markers,
    }
}

/// The parameter that sets line `line`'s time as its settings stand: what a
/// handle on the tunnel drags.
fn time_param(get: &dyn Fn(ParamId) -> f64, line: usize) -> ParamId {
    // With Stereo Link the right line has no time of its own.
    let line = if line == 1 && get(LINK) >= 0.5 { 0 } else { line };
    if get(line_param(line, SYNC)) < 0.5 {
        return line_param(line, TIME);
    }
    line_param(
        line,
        match get(line_param(line, SYNC_MODE)).round() as usize {
            1 => TRIPLET,
            2 => DOTTED,
            3 => SIXTEENTHS,
            _ => NOTES,
        },
    )
}

/// The tunnel: each line's first echoes as markers, a time in milliseconds
/// and a level in dB, and a handle for each line's time.
fn tunnel_display(
    get: &dyn Fn(ParamId) -> f64,
    data: Option<&[f32]>,
) -> (DisplaySeries, Vec<DisplayMarker>) {
    let times = line_times(get);
    // Without an engine the tempo is the default one, 120 BPM.
    let ms: [f64; 2] = match data {
        Some([left, right, ..]) => [f64::from(*left), f64::from(*right)],
        _ => [times[0].milliseconds(120.0), times[1].milliseconds(120.0)],
    };
    let ping_pong = get(CHANNEL_MODE).round() as usize == 1;
    let feedback = get(FEEDBACK) / 100.0;
    let names = ["Left", "Right"];
    let mut markers = Vec::new();
    for line in 0..2 {
        for n in 0..ECHOES {
            let level = 20.0 * feedback.powi(n as i32).max(1e-9).log10();
            if n > 0 && level < FLOOR_DB {
                break;
            }
            // Ping Pong's left echoes come back through the right line.
            let time = match (ping_pong, line) {
                (true, 0) => (n + 1) as f64 * ms[0] + n as f64 * ms[1],
                (true, _) => (n + 1) as f64 * (ms[0] + ms[1]),
                _ => (n + 1) as f64 * ms[line],
            };
            markers.push(DisplayMarker {
                name: format!("{} echo {}", names[line], n + 1),
                series: "Tunnel".to_owned(),
                x: time as f32,
                y: level.max(FLOOR_DB) as f32,
                text: format!("{} echo {} at {time:.0} ms, {level:.1} dB", names[line], n + 1),
                handle: None,
            });
        }
    }
    for line in 0..2 {
        markers.push(DisplayMarker {
            name: format!("{} Time", names[line]),
            series: "Tunnel".to_owned(),
            x: ms[line] as f32,
            y: 0.0,
            text: format!("{} time {:.0} ms", names[line], ms[line]),
            handle: Some(DisplayHandle {
                x: time_param(get, line),
                y: None,
            }),
        });
    }
    let span = markers.iter().map(|m| m.x).fold(1.0f32, f32::max);
    let series = DisplaySeries {
        name: "Tunnel".to_owned(),
        unit: "dB".to_owned(),
        x_unit: "ms".to_owned(),
        x_range: [0.0, span],
        log_x: false,
        y_range: [FLOOR_DB as f32, 0.0],
        detail: false,
        values: Vec::new(),
    };
    (series, markers)
}

/// The filters' response, high-pass and low-pass together, with a handle
/// on each cutoff that also sets its resonance.
fn filter_display(get: &dyn Fn(ParamId) -> f64) -> (DisplaySeries, Vec<DisplayMarker>) {
    let on = get(FILTER) >= 0.5;
    let hp = design(true, get(HP_FREQ), q_of(get(HP_RES)), DISPLAY_RATE);
    let lp = design(false, get(LP_FREQ), q_of(get(LP_RES)), DISPLAY_RATE);
    let series = DisplaySeries {
        name: "Filter".to_owned(),
        unit: "dB".to_owned(),
        x_unit: "Hz".to_owned(),
        x_range: [20.0, 20_000.0],
        log_x: true,
        y_range: [-36.0, 24.0],
        detail: false,
        values: (0..CURVE_POINTS)
            .map(|n| {
                if !on {
                    return 0.0;
                }
                let hz = 20.0 * 1_000f64.powf(n as f64 / (CURVE_POINTS - 1) as f64);
                (response_db(hp, hz) + response_db(lp, hz)) as f32
            })
            .collect(),
    };
    let marker = |name: &str, hz: ParamId, res: ParamId, coefficients: [f64; 5]| {
        let (cutoff, q) = (get(hz), q_of(get(res)));
        DisplayMarker {
            name: name.to_owned(),
            series: "Filter".to_owned(),
            x: cutoff as f32,
            y: if on { response_db(coefficients, cutoff) as f32 } else { 0.0 },
            text: format!("{name} {cutoff:.0} Hz, Q {q:.2}"),
            handle: Some(DisplayHandle {
                x: hz,
                y: Some(res),
            }),
        }
    };
    let markers = vec![
        marker("HP", HP_FREQ, HP_RES, hp),
        marker("LP", LP_FREQ, LP_RES, lp),
    ];
    (series, markers)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The reverberation time of `decay` seconds, read off the reverb's own
    /// impulse response as RT30: the time the backward-integrated energy
    /// takes to fall from −5 dB to −35 dB, doubled (R15).
    fn rt30(decay: f64) -> (f64, f64) {
        let rate = 48_000.0;
        let mut verb = Verb::new(rate);
        let frames = (decay * rate * 2.5) as usize + 4_800;
        let response: Vec<f64> = (0..frames)
            .map(|n| verb.process([if n == 0 { 1.0 } else { 0.0 }; 2], 1.0, decay)[0])
            .collect();
        let total: f64 = response.iter().map(|h| h * h).sum();
        let mut remaining = total;
        let (mut at_5, mut at_35) = (0.0, 0.0);
        for (n, h) in response.iter().enumerate() {
            remaining -= h * h;
            let level = 10.0 * (remaining.max(1e-30) / total).log10();
            if level > -5.0 {
                at_5 = n as f64 / rate;
            }
            if level > -35.0 {
                at_35 = n as f64 / rate;
            }
        }
        (2.0 * (at_35 - at_5), total)
    }

    // Spec: echo R15
    #[test]
    fn the_reverbs_rt30_is_within_ten_per_cent_of_decay() {
        for decay in [0.2, 0.5, 1.0, 3.0, 10.0] {
            let (measured, energy) = rt30(decay);
            assert!(
                (measured / decay - 1.0).abs() < 0.10,
                "Decay {decay} s measures {measured:.3} s"
            );
            // Its output has the energy of its input, so Reverb 100 % in the
            // loop doesn't change the loop's level.
            assert!((0.8..1.25).contains(&energy), "energy {energy}");
        }
    }

    // Spec: echo R6
    #[test]
    fn the_limit_is_linear_below_the_knee_and_never_passes_one() {
        assert_eq!(limit(0.3), 0.3);
        assert_eq!(limit(-0.5), -0.5);
        let mut last = 0.0;
        for n in 0..40 {
            let x = 0.5 + f64::from(n) * 0.25;
            assert!(limit(x) >= last && limit(x) <= 1.0, "{x}");
            assert_eq!(limit(-x), -limit(x));
            last = limit(x);
        }
        assert!(limit(2.0) < 1.0 && limit(2.0) > limit(1.0));
        // The slope is 1 where it bends.
        assert!((limit(0.5 + 1e-6) - 0.5 - 1e-6).abs() < 1e-9);
    }
}

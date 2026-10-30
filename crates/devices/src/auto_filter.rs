//! Auto Filter, the native Auto Filter (WP-5.20; the auto-filter spec): the filter
//! types and their slopes, each type's own control, the four circuits, Drive,
//! an LFO and an envelope follower moving the cutoff, Clip, Output and
//! Dry/Wet, and a sidechain feeding the envelope follower through the
//! device's own SC filter, with Mono Sidechain and Hi-Quality as menu
//! options.
//!
//! The filters are state-variable sections (a zero-delay-feedback design)
//! cascaded for the steeper slopes. The circuits differ in where they soften
//! the sections' state, so their small-signal responses agree (the spec's
//! R3) and only large signals tell them apart (R5). The shared parts are the
//! framework's: [`Lfo`], [`EnvelopeFollower`], and the [`Oversampler`] for
//! Hi-Quality.
//!
//! The manual records no ranges, defaults, or algorithms, so those are
//! Beatlayer's own (see `docs/specs/auto-filter.md`).

use core::f64::consts::{FRAC_1_SQRT_2, PI, TAU};

use bl_dsp::{EnvelopeFollower, Oversampler};
use bl_engine::{DeviceType, NodeConfig, Registry};
use bl_model::device_layout::{BODY_HEIGHT, ControlBand, DisplayCell, LayoutNode, Widget};
use bl_model::{
    Device, DeviceDescription, DeviceDisplay, DeviceId, DeviceLayout, DeviceRole, DisplayMarker,
    DisplayReading, DisplaySeries, OrdMap, ParamDescriptor, ParamId, ParamUnit,
};

use crate::framework::{Audio, DelayLine, NativeDevice, ParamBank, add};
use crate::lfo::{Lfo, Rate, Shape, Stereo, Wave};

/// Auto Filter's device type.
pub const AUTO_FILTER: &str = "beatlayer.sweep";

/// Type: the filter's response, one of [`TYPES`].
pub const TYPE: ParamId = ParamId(0);
/// Slope: 6, 12, 24 or 48 dB per octave, of the ones the type offers.
pub const SLOPE: ParamId = ParamId(1);
/// Circuit: SVF, DFM, MS2 or PRD, for the types they act on.
pub const CIRCUIT: ParamId = ParamId(2);
/// Freq: the cutoff, the comb's fundamental, or the resampling rate, in Hz.
pub const FREQ: ParamId = ParamId(3);
/// Res: the resonance, in per cent.
pub const RES: ParamId = ParamId(4);
/// Morph: Morph's position, Comb's peaks to notches, Vowel's gain
/// normalising, Notch + LP's notch.
pub const MORPH: ParamId = ParamId(5);
/// Control: DJ's bipolar control, in per cent.
pub const CONTROL: ParamId = ParamId(6);
/// Pitch: Vowel's formants shifted, in semitones.
pub const PITCH: ParamId = ParamId(7);
/// Formant: Vowel's position between a, e, i, o and u, in per cent.
pub const FORMANT: ParamId = ParamId(8);
/// Drive: the saturation before the filter, in per cent.
pub const DRIVE: ParamId = ParamId(9);
/// Clip: whether the output is soft-clipped.
pub const CLIP: ParamId = ParamId(10);
/// Output: the processed signal's level, a gain reaching silence.
pub const OUTPUT: ParamId = ParamId(11);
/// Dry/Wet: the balance between the filtered and dry signals, in per cent.
pub const DRY_WET: ParamId = ParamId(12);

/// LFO Amt: how far the LFO moves the cutoff, in per cent.
pub const LFO_AMOUNT: ParamId = ParamId(13);
/// LFO Wave: one of [`WAVES`].
pub const LFO_WAVE: ParamId = ParamId(14);
/// LFO Morph, or Smooth with S&H: reshapes or smooths the wave.
pub const LFO_MORPH: ParamId = ParamId(15);
/// LFO Mode: how the rate is given, one of [`RATE_MODES`].
pub const LFO_MODE: ParamId = ParamId(16);
/// LFO Rate in Hz.
pub const LFO_HZ: ParamId = ParamId(17);
/// LFO Time: seconds per cycle.
pub const LFO_TIME: ParamId = ParamId(18);
/// LFO Beats: the synced division, one of [`DIVISIONS`].
pub const LFO_DIVISION: ParamId = ParamId(19);
/// LFO Sixteenths: sixteenth notes per cycle.
pub const LFO_SIXTEENTHS: ParamId = ParamId(20);
/// LFO Stereo: Phase or Spin.
pub const LFO_STEREO: ParamId = ParamId(21);
/// LFO Phase: degrees the right channel's LFO is ahead.
pub const LFO_PHASE: ParamId = ParamId(22);
/// LFO Spin: per cent the right channel's rate is above the left's.
pub const LFO_SPIN: ParamId = ParamId(23);
/// LFO Offset: degrees against the beat grid, in the synced modes.
pub const LFO_OFFSET: ParamId = ParamId(24);
/// LFO Quantize: Off, Steps or S&H.
pub const LFO_QUANTIZE: ParamId = ParamId(25);
/// LFO Steps: the intervals to a cycle with Steps.
pub const LFO_STEPS: ParamId = ParamId(26);
/// LFO S&H: the beat-time interval with S&H, one of [`DIVISIONS`].
pub const LFO_SH: ParamId = ParamId(27);

/// Env Amt: how far, and which way, the envelope moves the cutoff, in per
/// cent.
pub const ENV_AMOUNT: ParamId = ParamId(28);
/// Env Attack, in milliseconds.
pub const ENV_ATTACK: ParamId = ParamId(29);
/// Env Release, in milliseconds.
pub const ENV_RELEASE: ParamId = ParamId(30);
/// Env Hold: finish the attack before the release.
pub const ENV_HOLD: ParamId = ParamId(31);
/// Env S&H: hold the envelope's modulation on a beat-time interval.
pub const ENV_SH: ParamId = ParamId(32);
/// Env S&H Rate: one of [`DIVISIONS`].
pub const ENV_SH_RATE: ParamId = ParamId(33);

/// SC Filter: whether the detector hears the sidechain through a filter.
pub const SC_FILTER: ParamId = ParamId(34);
/// SC Type: one of [`SC_TYPES`].
pub const SC_TYPE: ParamId = ParamId(35);
/// SC Freq: the SC filter's cutoff or centre, in Hz.
pub const SC_FREQ: ParamId = ParamId(36);
/// SC Q: the SC filter's bandwidth.
pub const SC_Q: ParamId = ParamId(37);
/// SC Shelf Gain: the shelves' gain, in dB.
pub const SC_SHELF: ParamId = ParamId(38);
/// SC Peak Gain: the peak's gain, in dB.
pub const SC_PEAK: ParamId = ParamId(39);

/// Mono Sidechain, a menu option: one detector, so both channels move
/// alike.
pub const MONO_SIDECHAIN: ParamId = ParamId(40);
/// Hi-Quality, a menu option: the filter runs at twice the rate.
pub const HI_QUALITY: ParamId = ParamId(41);

/// The filter types, in the chooser's order.
pub const TYPES: [&str; 10] = [
    "Low-pass",
    "High-pass",
    "Band-pass",
    "Notch",
    "Notch + LP",
    "Morph",
    "DJ",
    "Comb",
    "Resampling",
    "Vowel",
];

/// The slopes, in the chooser's order.
pub const SLOPES: [&str; 4] = ["6 dB", "12 dB", "24 dB", "48 dB"];

/// The circuits, in the chooser's order.
pub const CIRCUITS: [&str; 4] = ["SVF", "DFM", "MS2", "PRD"];

/// The LFO's waves, in the chooser's order, under the manual's names.
pub const WAVES: [&str; 8] = [
    "Sine",
    "Triangle",
    "Saw",
    "Square",
    "Ramp Up",
    "Ramp Down",
    "Wander",
    "S&H",
];

/// The effects LFO's waves the chooser's [`WAVES`] play, in order.
const WAVE_OF: [Wave; 8] = [
    Wave::Sine,
    Wave::Triangle,
    Wave::SharkTooth,
    Wave::Rectangle,
    Wave::SawUp,
    Wave::SawDown,
    Wave::Wander,
    Wave::SampleHold,
];

/// How the LFO's rate is given.
pub const RATE_MODES: [&str; 4] = ["Hz", "Time", "Beats", "16ths"];

/// The synced divisions: their names and their lengths in quarter notes.
pub const DIVISIONS: [(&str, f64); 17] = [
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

/// The division a new device's LFO starts on: 1/4.
const DEFAULT_DIVISION: usize = 8;

/// The division a new device's S&H intervals start on: 1/16.
const DEFAULT_SH: usize = 2;

/// The SC filter's types, in the chooser's order.
pub const SC_TYPES: [&str; 7] = [
    "Low Shelf",
    "Peak",
    "High Shelf",
    "Low Pass",
    "Band Pass",
    "High Pass",
    "Notch",
];

/// The cutoff range, in Hz.
const MIN_HZ: f64 = 20.0;
const MAX_HZ: f64 = 20_000.0;

/// Res 100 % is this Q; 0 % is the Butterworth Q, a flat top.
const Q_MAX: f64 = 10.0;

/// Notch + LP's notch has this Q, and sits this many octaves above the
/// cutoff at Morph 0 %.
const NOTCH_Q: f64 = 2.0;
const NOTCH_OCTAVES: f64 = 3.0;

/// The most feedback the comb takes, at Res 100 %.
const COMB_MAX: f64 = 0.95;

/// Vowel's formants Q at Res 0 %, and how much more Res 100 % adds.
const FORMANT_Q: f64 = 8.0;

/// Drive 100 % is this much gain into the soft clipper.
const DRIVE_MAX: f64 = 10.0;

/// How many octaves the LFO moves the cutoff at 100 % each way, and the
/// envelope at 100 % for a full-scale level (1).
const LFO_OCTAVES: f64 = 4.0;
const ENV_OCTAVES: f64 = 8.0;

/// Where the circuits soften the sections' state, in the state's own units:
/// the state is the signal times about Q, so a resonant section at a high
/// level reaches them while a −40 dBFS signal stays well inside (the
/// softening takes a share of the state on every sample).
const SOFT_LOOSE: f64 = 8.0;
const SOFT_TIGHT: f64 = 3.0;

/// Clip leaves the signal alone up to this level and rounds off from there,
/// to a peak of 1.
const CLIP_KNEE: f64 = 0.8;

/// A Vowel's formants: for a, e, i, o and u, each formant's frequency in Hz
/// and its relative level. Beatlayer's own, from published averages of
/// spoken vowels.
const FORMANTS: [[(f64, f64); 3]; 5] = [
    [(730.0, 1.0), (1090.0, 0.45), (2440.0, 0.2)],
    [(530.0, 1.0), (1840.0, 0.45), (2480.0, 0.2)],
    [(270.0, 1.0), (2290.0, 0.45), (3010.0, 0.2)],
    [(570.0, 1.0), (840.0, 0.45), (2410.0, 0.2)],
    [(300.0, 1.0), (870.0, 0.45), (2240.0, 0.2)],
];

/// The rate Hi-Quality runs the filter at, as a power of two: twice.
const HI_QUALITY_STAGES: u32 = 1;

/// Auto Filter's device type.
#[must_use]
pub fn auto_filter_type() -> DeviceType {
    let divisions: Vec<&str> = DIVISIONS.iter().map(|d| d.0).collect();
    let params = vec![
        ParamDescriptor::choice(TYPE, "Type", &TYPES, 0)
            .describe("The filter's response: how it shapes the signal around its cutoff."),
        ParamDescriptor::choice(SLOPE, "Slope", &SLOPES, 1).describe(
            "How steeply the filter cuts, in dB per octave. Types that offer fewer slopes use the \
             nearest.",
        ),
        ParamDescriptor::choice(CIRCUIT, "Circuit", &CIRCUITS, 0).describe(
            "The filter's character at high levels, for low-pass, high-pass, band-pass, notch \
             and morph.",
        ),
        ParamDescriptor::continuous(FREQ, "Freq", (MIN_HZ, MAX_HZ), 1_000.0, ParamUnit::Hertz)
            .logarithmic()
            .describe("The filter's cutoff, the comb's fundamental, or the resampling rate."),
        ParamDescriptor::continuous(RES, "Res", (0.0, 100.0), 0.0, ParamUnit::Percent)
            .describe("How much the filter rings around its cutoff. Resampling ignores it."),
        ParamDescriptor::continuous(MORPH, "Morph", (0.0, 100.0), 0.0, ParamUnit::Percent)
            .describe(
                "Morph's place between low-pass, band-pass and high-pass; the comb's move from \
                 peaks to notches; Vowel's formant levels evened out; Notch + LP's notch moving \
                 to the cutoff.",
            ),
        ParamDescriptor::continuous(CONTROL, "Control", (-100.0, 100.0), 0.0, ParamUnit::Percent)
            .describe(
                "DJ's filter: above zero it cuts the lows, below zero the highs, and the \
                 resonance grows toward either end.",
            ),
        ParamDescriptor::continuous(PITCH, "Pitch", (-24.0, 24.0), 0.0, ParamUnit::Semitones)
            .describe("Moves Vowel's formants up or down."),
        ParamDescriptor::continuous(FORMANT, "Formant", (0.0, 100.0), 0.0, ParamUnit::Percent)
            .describe("Vowel's place between a, e, i, o and u."),
        ParamDescriptor::continuous(DRIVE, "Drive", (0.0, 100.0), 0.0, ParamUnit::Percent)
            .describe("Saturates the signal before the filter. At 0 % it is untouched."),
        ParamDescriptor::toggle(CLIP, "Clip", false)
            .describe("Soft-clips the output to keep peaks, such as a high resonance's, in check."),
        ParamDescriptor::gain(OUTPUT, "Output", 12.0, 0.0)
            .describe("The filtered signal's level, down to silence."),
        ParamDescriptor::continuous(DRY_WET, "Dry/Wet", (0.0, 100.0), 100.0, ParamUnit::Percent)
            .describe("The balance between the filtered and the dry signal."),
        ParamDescriptor::continuous(LFO_AMOUNT, "LFO Amt", (0.0, 100.0), 0.0, ParamUnit::Percent)
            .describe("How far the LFO moves the cutoff."),
        ParamDescriptor::choice(LFO_WAVE, "LFO Wave", &WAVES, 0).describe("The LFO's waveform."),
        ParamDescriptor::continuous(
            LFO_MORPH,
            "LFO Morph",
            (0.0, 100.0),
            0.0,
            ParamUnit::Percent,
        )
        .describe("Reshapes the LFO's wave; with S&H it smooths the steps."),
        ParamDescriptor::choice(LFO_MODE, "LFO Mode", &RATE_MODES, 0).describe(
            "Whether the LFO's rate is in Hz, a time per cycle, beat divisions, or sixteenth \
             notes.",
        ),
        ParamDescriptor::continuous(LFO_HZ, "LFO Rate", (0.01, 40.0), 1.0, ParamUnit::Hertz)
            .logarithmic()
            .describe("The LFO's rate in Hz."),
        ParamDescriptor::continuous(
            LFO_TIME,
            "LFO Time",
            (0.025, 200.0),
            1.0,
            ParamUnit::Seconds,
        )
        .logarithmic()
        .describe("How long one cycle of the LFO takes."),
        ParamDescriptor::choice(LFO_DIVISION, "LFO Beats", &divisions, DEFAULT_DIVISION)
            .describe("The LFO's cycle as a beat division, synced to the tempo."),
        ParamDescriptor::integer(LFO_SIXTEENTHS, "LFO 16ths", (1, 64), 4, ParamUnit::None)
            .describe("The LFO's cycle in sixteenth notes, synced to the tempo."),
        ParamDescriptor::choice(LFO_STEREO, "LFO Stereo", &["Phase", "Spin"], 0)
            .describe("Whether the two channels' LFOs differ in phase or in rate."),
        ParamDescriptor::continuous(LFO_PHASE, "LFO Phase", (0.0, 360.0), 0.0, ParamUnit::None)
            .decimals(0)
            .describe("How many degrees the right channel's LFO is ahead; 180 is opposite."),
        ParamDescriptor::continuous(LFO_SPIN, "LFO Spin", (-50.0, 50.0), 0.0, ParamUnit::Percent)
            .describe("How much faster the right channel's LFO runs than the left's."),
        ParamDescriptor::continuous(
            LFO_OFFSET,
            "LFO Offset",
            (-180.0, 180.0),
            0.0,
            ParamUnit::None,
        )
        .decimals(0)
        .describe("Moves both LFOs against the beat grid, in degrees, in the synced modes."),
        ParamDescriptor::choice(LFO_QUANTIZE, "LFO Quantize", &["Off", "Steps", "S&H"], 0)
            .describe("Holds the LFO in steps within each cycle, or on a beat-time interval."),
        ParamDescriptor::integer(LFO_STEPS, "LFO Steps", (2, 64), 8, ParamUnit::None)
            .describe("How many intervals each cycle is divided into with Steps."),
        ParamDescriptor::choice(LFO_SH, "LFO S&H", &divisions, DEFAULT_SH)
            .describe("How often the LFO is sampled with S&H."),
        ParamDescriptor::continuous(
            ENV_AMOUNT,
            "Env Amt",
            (-100.0, 100.0),
            0.0,
            ParamUnit::Percent,
        )
        .describe("How far, and in which direction, the envelope follower moves the cutoff."),
        ParamDescriptor::continuous(
            ENV_ATTACK,
            "Env Attack",
            (0.1, 1_000.0),
            1.0,
            ParamUnit::Milliseconds,
        )
        .logarithmic()
        .describe("How fast the envelope follower rises with the input."),
        ParamDescriptor::continuous(
            ENV_RELEASE,
            "Env Release",
            (1.0, 5_000.0),
            100.0,
            ParamUnit::Milliseconds,
        )
        .logarithmic()
        .describe("How fast the envelope follower falls with the input."),
        ParamDescriptor::toggle(ENV_HOLD, "Env Hold", false)
            .describe("Lets the envelope finish its attack before it releases."),
        ParamDescriptor::toggle(ENV_SH, "Env S&H", false)
            .describe("Holds the envelope's modulation on a beat-time interval."),
        ParamDescriptor::choice(ENV_SH_RATE, "Env S&H Rate", &divisions, DEFAULT_SH)
            .describe("How often the envelope is sampled with Env S&H."),
        ParamDescriptor::toggle(SC_FILTER, "SC Filter", false)
            .describe("Limits which frequencies of the sidechain trigger the envelope follower."),
        ParamDescriptor::choice(SC_TYPE, "SC Type", &SC_TYPES, 4)
            .describe("The sidechain filter's shape."),
        ParamDescriptor::continuous(
            SC_FREQ,
            "SC Freq",
            (MIN_HZ, MAX_HZ),
            1_000.0,
            ParamUnit::Hertz,
        )
        .logarithmic()
        .describe("The sidechain filter's cutoff or centre frequency."),
        ParamDescriptor::continuous(SC_Q, "SC Q", (0.1, 10.0), 0.71, ParamUnit::None)
            .logarithmic()
            .describe("The sidechain filter's bandwidth."),
        ParamDescriptor::continuous(
            SC_SHELF,
            "SC Shelf Gain",
            (-24.0, 24.0),
            12.0,
            ParamUnit::Decibels,
        )
        .describe("The low and high shelves' boost or cut."),
        ParamDescriptor::continuous(
            SC_PEAK,
            "SC Peak Gain",
            (-24.0, 24.0),
            12.0,
            ParamUnit::Decibels,
        )
        .describe("The peak's boost or cut."),
        ParamDescriptor::toggle(MONO_SIDECHAIN, "Mono Sidechain", true).describe(
            "Makes the envelope follower hear one mono signal, so both channels move alike.",
        ),
        ParamDescriptor::toggle(HI_QUALITY, "Hi-Quality", false)
            .describe("Runs the filter at twice the sample rate, at the price of some latency."),
    ];
    DeviceType::native(AUTO_FILTER, "Auto Filter", DeviceRole::AudioEffect)
        .with_sidechain()
        .with_description(
            DeviceDescription::new(params)
                .with_layout(layout())
                .with_menu_options(&[MONO_SIDECHAIN, HI_QUALITY])
                .with_banks(&[
                    (
                        "Filter",
                        &[
                            "Type", "Slope", "Circuit", "Freq", "Res", "Morph", "Control", "Pitch",
                        ],
                    ),
                    ("Output", &["Formant", "Drive", "Clip", "Output", "Dry/Wet"]),
                    (
                        "LFO",
                        &[
                            "LFO Amt",
                            "LFO Wave",
                            "LFO Morph",
                            "LFO Mode",
                            "LFO Rate",
                            "LFO Time",
                            "LFO Beats",
                            "LFO 16ths",
                        ],
                    ),
                    (
                        "LFO Shape",
                        &[
                            "LFO Stereo",
                            "LFO Phase",
                            "LFO Spin",
                            "LFO Offset",
                            "LFO Quantize",
                            "LFO Steps",
                            "LFO S&H",
                        ],
                    ),
                    (
                        "Envelope",
                        &[
                            "Env Amt",
                            "Env Attack",
                            "Env Release",
                            "Env Hold",
                            "Env S&H",
                            "Env S&H Rate",
                        ],
                    ),
                    (
                        "Sidechain",
                        &[
                            "SC Filter",
                            "SC Type",
                            "SC Freq",
                            "SC Q",
                            "SC Shelf Gain",
                            "SC Peak Gain",
                        ],
                    ),
                ]),
        )
}

/// The device's layout: the filter's controls, the display of its curves,
/// the LFO's and the envelope's, and the sidechain filter's with the
/// globals. The sidechain strip down the left edge is the framework's.
fn layout() -> DeviceLayout {
    use Widget::{Chooser, Knob, Number, Toggle};
    let band = |controls: &[(ParamId, Widget)]| {
        LayoutNode::Band(ControlBand::stacked(BODY_HEIGHT, controls))
    };
    DeviceLayout::new(LayoutNode::Row(vec![
        band(&[
            (TYPE, Chooser),
            (SLOPE, Chooser),
            (CIRCUIT, Chooser),
            (FREQ, Knob),
            (RES, Knob),
            (MORPH, Knob),
            (CONTROL, Knob),
            (PITCH, Knob),
            (FORMANT, Knob),
            (DRIVE, Knob),
        ]),
        LayoutNode::Cell(DisplayCell::new("Filter", 150.0, BODY_HEIGHT, 1.0)),
        band(&[
            (LFO_WAVE, Chooser),
            (LFO_MODE, Chooser),
            (LFO_STEREO, Chooser),
            (LFO_QUANTIZE, Chooser),
            (LFO_AMOUNT, Knob),
            (LFO_MORPH, Knob),
            (LFO_HZ, Knob),
            (LFO_TIME, Knob),
            (LFO_DIVISION, Chooser),
            (LFO_SIXTEENTHS, Number),
            (LFO_STEPS, Number),
            (LFO_SH, Chooser),
            (LFO_PHASE, Knob),
            (LFO_SPIN, Knob),
            (LFO_OFFSET, Knob),
        ]),
        band(&[
            (ENV_AMOUNT, Knob),
            (ENV_ATTACK, Knob),
            (ENV_RELEASE, Knob),
            (ENV_HOLD, Toggle),
            (ENV_SH, Toggle),
            (ENV_SH_RATE, Chooser),
        ]),
        band(&[
            (SC_FILTER, Toggle),
            (SC_TYPE, Chooser),
            (SC_FREQ, Knob),
            (SC_Q, Knob),
            (SC_SHELF, Knob),
            (SC_PEAK, Knob),
            (CLIP, Toggle),
            (OUTPUT, Knob),
            (DRY_WET, Knob),
        ]),
    ]))
}

/// Adds the Auto Filter device to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, auto_filter_type(), AutoFilter::new);
}

/// A filter type, by its place in [`TYPES`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    LowPass,
    HighPass,
    BandPass,
    Notch,
    NotchLowPass,
    Morph,
    Dj,
    Comb,
    Resampling,
    Vowel,
}

impl Kind {
    fn of(index: usize) -> Self {
        [
            Self::LowPass,
            Self::HighPass,
            Self::BandPass,
            Self::Notch,
            Self::NotchLowPass,
            Self::Morph,
            Self::Dj,
            Self::Comb,
            Self::Resampling,
            Self::Vowel,
        ][index.min(TYPES.len() - 1)]
    }
}

/// A circuit, by its place in [`CIRCUITS`]: where the sections' state is
/// softened.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Circuit {
    /// Nowhere: clean, distorting only through Drive.
    Svf,
    /// In both integrators, so the distortion is fed back.
    Dfm,
    /// In the band-pass integrator, tightly, so the resonance is bounded.
    Ms2,
    /// In the low-pass integrator only, so the resonance isn't limited.
    Prd,
}

impl Circuit {
    fn of(index: usize) -> Self {
        [Self::Svf, Self::Dfm, Self::Ms2, Self::Prd][index.min(3)]
    }
}

/// A complex number, for the curves.
#[derive(Clone, Copy)]
struct C(f64, f64);

impl C {
    fn mul(self, o: C) -> C {
        C(self.0 * o.0 - self.1 * o.1, self.0 * o.1 + self.1 * o.0)
    }
    fn div(self, o: C) -> C {
        let d = o.0 * o.0 + o.1 * o.1;
        C(
            (self.0 * o.0 + self.1 * o.1) / d,
            (self.1 * o.0 - self.0 * o.1) / d,
        )
    }
    fn add(self, o: C) -> C {
        C(self.0 + o.0, self.1 + o.1)
    }
    fn scale(self, k: f64) -> C {
        C(self.0 * k, self.1 * k)
    }
    fn abs(self) -> f64 {
        self.0.hypot(self.1)
    }
}

/// What a filter does at one frame: its type and settings. The cutoffs are
/// worked out once a frame; the section coefficients follow from them.
#[derive(Clone, Copy, Debug)]
struct Plan {
    kind: Kind,
    /// Sections in the cascade (0 for the types that have none).
    stages: usize,
    /// Morph's 6 dB slope: one pole.
    one_pole: bool,
    circuit: Circuit,
    /// Freq, and the factor the LFO and envelope move it by.
    hz: f64,
    shift: f64,
    /// The first section's Q.
    q: f64,
    /// Res, Morph and Formant from 0 to 1; Control from −1 to 1.
    res: f64,
    morph: f64,
    control: f64,
    pitch: f64,
    formant: f64,
}

/// The knobs a plan takes besides the choosers: Freq in Hz, Res, Morph and
/// Formant from 0 to 1, Control from −1 to 1, and Pitch in semitones.
#[derive(Clone, Copy)]
struct Knobs {
    hz: f64,
    res: f64,
    morph: f64,
    control: f64,
    pitch: f64,
    formant: f64,
}

impl Plan {
    fn new(kind: usize, slope: usize, circuit: usize, knobs: Knobs) -> Self {
        let Knobs {
            hz,
            res,
            morph,
            control,
            pitch,
            formant,
        } = knobs;
        let kind = Kind::of(kind);
        let (stages, one_pole) = match kind {
            Kind::Morph => match slope {
                0 => (1, true),
                1 => (1, false),
                2 => (2, false),
                _ => (4, false),
            },
            Kind::LowPass | Kind::HighPass | Kind::BandPass | Kind::Notch | Kind::NotchLowPass => {
                (if slope >= 2 { 2 } else { 1 }, false)
            }
            Kind::Dj => (1, false),
            Kind::Comb | Kind::Resampling | Kind::Vowel => (0, false),
        };
        // The circuits act on these types alone.
        let circuit = match kind {
            Kind::LowPass | Kind::HighPass | Kind::BandPass | Kind::Notch | Kind::Morph => {
                Circuit::of(circuit)
            }
            _ => Circuit::Svf,
        };
        Self {
            kind,
            stages,
            one_pole,
            circuit,
            hz,
            shift: 1.0,
            q: q_of(res),
            res,
            morph,
            control,
            pitch,
            formant,
        }
    }

    /// The cutoff the type plays at, after the modulation.
    fn cutoff(&self) -> f64 {
        self.hz * self.shift
    }

    /// DJ's cutoff and Q: above zero a high-pass sweeping up from 20 Hz,
    /// below zero a low-pass sweeping down from 20 kHz, the resonance
    /// growing with the distance from zero.
    fn dj(&self) -> (bool, f64, f64) {
        let c = self.control.abs();
        let q = FRAC_1_SQRT_2 + (self.q - FRAC_1_SQRT_2) * c;
        let hz = if self.control >= 0.0 {
            MIN_HZ * (MAX_HZ / MIN_HZ).powf(c)
        } else {
            MAX_HZ * (MIN_HZ / MAX_HZ).powf(c)
        };
        (self.control >= 0.0, hz * self.shift, q)
    }

    /// Vowel's three formants at the position and pitch: frequency, level
    /// and Q.
    fn formants(&self) -> [(f64, f64, f64); 3] {
        let at = (self.formant * 4.0).min(4.0);
        let from = (at.floor() as usize).min(3);
        let t = at - from as f64;
        let shift = self.shift * (self.pitch / 12.0).exp2();
        let q = FORMANT_Q * (1.0 + self.res);
        core::array::from_fn(|i| {
            let (a, b) = (FORMANTS[from][i], FORMANTS[from + 1][i]);
            let hz = a.0 * (b.0 / a.0).powf(t) * shift;
            let level = a.1 + (b.1 - a.1) * t;
            (hz, level + (1.0 - level) * self.morph, q)
        })
    }

    /// The comb's feedback, from Res.
    fn feedback(&self) -> f64 {
        COMB_MAX * self.res
    }

    /// Notch + LP's notch cutoff: the cutoff's own at Morph 100 %.
    fn notch_hz(&self) -> f64 {
        self.cutoff() * (NOTCH_OCTAVES * (1.0 - self.morph)).exp2()
    }

    /// The small-signal response at `hz` at sample rate `rate`: the
    /// spec's curve, which the filters play. Drive, the circuits'
    /// softening, Clip and the mix are not in it.
    fn response(&self, hz: f64, rate: f64) -> C {
        let w = (PI * hz / rate).tan();
        // A section's lp, normalised band-pass, high-pass and notch at its
        // cutoff and Q.
        let section = |fc: f64, q: f64| {
            let g = (PI * fc.clamp(1.0, rate * 0.49) / rate).tan();
            let (x, k) = (w / g, 1.0 / q);
            let den = C(1.0 - x * x, k * x);
            let one = C(1.0, 0.0);
            [
                one.div(den),
                C(0.0, k * x).div(den),
                C(-x * x, 0.0).div(den),
                C(1.0 - x * x, 0.0).div(den),
            ]
        };
        let q_of = |s: usize| if s == 0 { self.q } else { FRAC_1_SQRT_2 };
        let fc = self.cutoff();
        match self.kind {
            Kind::LowPass | Kind::HighPass | Kind::BandPass | Kind::Notch => {
                let pick = match self.kind {
                    Kind::LowPass => 0,
                    Kind::BandPass => 1,
                    Kind::HighPass => 2,
                    _ => 3,
                };
                (0..self.stages).fold(C(1.0, 0.0), |acc, s| acc.mul(section(fc, q_of(s))[pick]))
            }
            Kind::NotchLowPass => (0..self.stages)
                .fold(section(self.notch_hz(), NOTCH_Q)[3], |acc, s| {
                    acc.mul(section(fc, q_of(s))[0])
                }),
            Kind::Morph if self.one_pole => {
                let g = (PI * fc.clamp(1.0, rate * 0.49) / rate).tan();
                let x = w / g;
                let lp = C(1.0, 0.0).div(C(1.0, x));
                let hp = C(0.0, x).div(C(1.0, x));
                lp.scale(1.0 - self.morph).add(hp.scale(self.morph))
            }
            Kind::Morph => (0..self.stages).fold(C(1.0, 0.0), |acc, s| {
                let [lp, bp, hp, _] = section(fc, q_of(s));
                acc.mul(blend(self.morph, lp, bp, hp))
            }),
            Kind::Dj => {
                let (high, hz, q) = self.dj();
                section(hz, q)[if high { 2 } else { 0 }]
            }
            Kind::Comb => {
                let c = self.feedback();
                let delay = rate / fc.max(1.0);
                let z = C(
                    (TAU * hz * delay / rate).cos(),
                    -(TAU * hz * delay / rate).sin(),
                );
                let one = C(1.0, 0.0);
                let peaks = one.scale(1.0 - c).div(one.add(z.scale(-c)));
                let notches = one.add(z.scale(-c)).scale(1.0 / (1.0 + c));
                peaks.scale(1.0 - self.morph).add(notches.scale(self.morph))
            }
            Kind::Resampling => C(1.0, 0.0),
            Kind::Vowel => self
                .formants()
                .iter()
                .fold(C(0.0, 0.0), |acc, &(f, level, q)| {
                    acc.add(section(f, q)[1].scale(level))
                }),
        }
    }
}

/// Res from 0 to 1 as a Q, exponentially from Butterworth to [`Q_MAX`].
fn q_of(res: f64) -> f64 {
    FRAC_1_SQRT_2 * (Q_MAX / FRAC_1_SQRT_2).powf(res.clamp(0.0, 1.0))
}

/// Morph's blend of low-pass, band-pass and high-pass: low-pass to
/// band-pass over the first half, band-pass to high-pass over the second.
fn blend<T>(morph: f64, lp: T, bp: T, hp: T) -> T
where
    T: Mix,
{
    if morph < 0.5 {
        lp.mix(bp, morph * 2.0)
    } else {
        bp.mix(hp, morph * 2.0 - 1.0)
    }
}

/// A value that crossfades, for [`blend`].
trait Mix {
    fn mix(self, to: Self, t: f64) -> Self;
}

impl Mix for f64 {
    fn mix(self, to: f64, t: f64) -> f64 {
        self + (to - self) * t
    }
}

impl Mix for C {
    fn mix(self, to: C, t: f64) -> C {
        self.scale(1.0 - t).add(to.scale(t))
    }
}

/// The small-signal response in dB of the filter `values` set up, at `hz`,
/// at sample rate `rate`: the curve the display draws and the tests hold
/// the audio to. Values it lacks are the type's defaults.
#[must_use]
pub fn response_db(values: &OrdMap<ParamId, f64>, hz: f64, rate: f64) -> f64 {
    let description = auto_filter_type().description;
    20.0 * plan_of(&description, values)
        .response(hz, rate)
        .abs()
        .max(1e-12)
        .log10()
}

/// The plan `values` set up, unmodulated.
fn plan_of(description: &DeviceDescription, values: &OrdMap<ParamId, f64>) -> Plan {
    let value = |id: ParamId| description.value(values, id).unwrap_or(0.0);
    Plan::new(
        value(TYPE) as usize,
        value(SLOPE) as usize,
        value(CIRCUIT) as usize,
        Knobs {
            hz: value(FREQ),
            res: value(RES) / 100.0,
            morph: value(MORPH) / 100.0,
            control: value(CONTROL) / 100.0,
            pitch: value(PITCH),
            formant: value(FORMANT) / 100.0,
        },
    )
}

/// Softens `x` to within about ±`a`, leaving small values alone.
fn soft(x: f64, a: f64) -> f64 {
    a * (x / a).tanh()
}

/// Drive's saturation: `tanh(d x) / d`, which is `x` itself at `d` = 0.
fn drive(x: f64, d: f64) -> f64 {
    if d < 1e-6 { x } else { (d * x).tanh() / d }
}

/// A value far below the audio's noise floor is left as zero rather than as
/// a denormal.
fn flush(x: f64) -> f64 {
    if x.abs() < 1e-20 { 0.0 } else { x }
}

/// A state-variable section's coefficients.
#[derive(Clone, Copy)]
struct Coef {
    a1: f64,
    a2: f64,
    a3: f64,
    k: f64,
}

impl Coef {
    fn new(g: f64, q: f64) -> Self {
        let k = 1.0 / q;
        let a1 = 1.0 / (1.0 + g * (g + k));
        Self {
            a1,
            a2: g * a1,
            a3: g * g * a1,
            k,
        }
    }
}

/// A state-variable section's two integrators.
#[derive(Clone, Copy, Default)]
struct Stage {
    ic1: f64,
    ic2: f64,
}

impl Stage {
    /// One sample through the section: its low-pass, raw band-pass and
    /// high-pass outputs.
    fn tick(&mut self, x: f64, c: &Coef, circuit: Circuit) -> [f64; 3] {
        let v3 = x - self.ic2;
        let v1 = c.a1 * self.ic1 + c.a2 * v3;
        let v2 = self.ic2 + c.a2 * self.ic1 + c.a3 * v3;
        self.ic1 = flush(2.0 * v1 - self.ic1);
        self.ic2 = flush(2.0 * v2 - self.ic2);
        match circuit {
            Circuit::Svf => {}
            Circuit::Dfm => {
                self.ic1 = soft(self.ic1, SOFT_LOOSE);
                self.ic2 = soft(self.ic2, SOFT_LOOSE);
            }
            Circuit::Ms2 => self.ic1 = soft(self.ic1, SOFT_TIGHT),
            Circuit::Prd => self.ic2 = soft(self.ic2, SOFT_LOOSE),
        }
        [v2, v1, x - c.k * v1 - v2]
    }
}

/// A ring of past samples, read a fractional number of samples back.
struct Ring {
    samples: Vec<f32>,
    at: usize,
}

impl Ring {
    fn new(len: usize) -> Self {
        Self {
            samples: vec![0.0; len],
            at: 0,
        }
    }

    fn write(&mut self, x: f64) {
        self.samples[self.at] = x as f32;
        self.at = (self.at + 1) % self.samples.len();
    }

    /// The sample `delay` writes ago, between the two nearest.
    fn read(&self, delay: f64) -> f64 {
        let len = self.samples.len();
        let delay = delay.clamp(1.0, (len - 2) as f64);
        let whole = delay.floor();
        let at = |n: usize| f64::from(self.samples[(self.at + len - n) % len]);
        let (a, b) = (at(whole as usize), at(whole as usize + 1));
        a + (b - a) * (delay - whole)
    }
}

/// One channel's filter: every type's state, so a change of type finds
/// them ready.
struct Chan {
    stages: [Stage; 4],
    pole: f64,
    notch: Stage,
    comb_in: Ring,
    comb_out: Ring,
    phase: f64,
    held: f64,
}

impl Chan {
    fn new(rate: f64) -> Self {
        // The longest comb delay: the lowest cutoff, at the highest rate.
        let len = (rate * 2.0 / MIN_HZ).ceil() as usize + 4;
        Self {
            stages: [Stage::default(); 4],
            pole: 0.0,
            notch: Stage::default(),
            comb_in: Ring::new(len),
            comb_out: Ring::new(len),
            phase: 1.0,
            held: 0.0,
        }
    }

    /// One sample through the filter `plan` describes, at `rate`.
    fn tick(&mut self, x: f64, plan: &Plan, rate: f64) -> f64 {
        let fc = plan.cutoff();
        let g_of = |hz: f64| (PI * hz.clamp(1.0, rate * 0.49) / rate).tan();
        let q_of = |s: usize| if s == 0 { plan.q } else { FRAC_1_SQRT_2 };
        match plan.kind {
            Kind::LowPass | Kind::HighPass | Kind::BandPass | Kind::Notch | Kind::Morph
                if plan.one_pole =>
            {
                let g = g_of(fc);
                let v = (x - self.pole) * g / (1.0 + g);
                let lp = v + self.pole;
                self.pole = flush(lp + v);
                lp.mix(x - lp, plan.morph)
            }
            Kind::LowPass | Kind::HighPass | Kind::BandPass | Kind::Notch | Kind::Morph => {
                let g = g_of(fc);
                let mut y = x;
                for s in 0..plan.stages {
                    let c = Coef::new(g, q_of(s));
                    let [lp, bp, hp] = self.stages[s].tick(y, &c, plan.circuit);
                    y = match plan.kind {
                        Kind::LowPass => lp,
                        Kind::HighPass => hp,
                        Kind::BandPass => c.k * bp,
                        Kind::Notch => lp + hp,
                        _ => blend(plan.morph, lp, c.k * bp, hp),
                    };
                }
                y
            }
            Kind::NotchLowPass => {
                let c = Coef::new(g_of(plan.notch_hz()), NOTCH_Q);
                let [lp, _, hp] = self.notch.tick(x, &c, Circuit::Svf);
                let mut y = lp + hp;
                let g = g_of(fc);
                for s in 0..plan.stages {
                    y = self.stages[s].tick(y, &Coef::new(g, q_of(s)), Circuit::Svf)[0];
                }
                y
            }
            Kind::Dj => {
                let (high, hz, q) = plan.dj();
                let [lp, _, hp] = self.stages[0].tick(x, &Coef::new(g_of(hz), q), Circuit::Svf);
                if high { hp } else { lp }
            }
            Kind::Comb => {
                let c = plan.feedback();
                let delay = rate / fc.max(1.0);
                let peaks = (1.0 - c) * x + c * self.comb_out.read(delay);
                let notches = (x - c * self.comb_in.read(delay)) / (1.0 + c);
                self.comb_in.write(x);
                self.comb_out.write(peaks);
                peaks.mix(notches, plan.morph)
            }
            Kind::Resampling => {
                self.phase += fc / rate;
                if self.phase >= 1.0 {
                    self.phase -= self.phase.floor();
                    self.held = x;
                }
                self.held
            }
            Kind::Vowel => {
                let mut y = 0.0;
                for (s, (hz, level, q)) in plan.formants().into_iter().enumerate() {
                    let c = Coef::new(g_of(hz), q);
                    y += level * c.k * self.stages[s].tick(x, &c, Circuit::Svf)[1];
                }
                y
            }
        }
    }
}

/// A biquad, for the SC filter: transposed direct form II.
#[derive(Clone, Copy, Default)]
struct Biquad {
    b: [f64; 3],
    a: [f64; 2],
    z: [f64; 2],
}

impl Biquad {
    /// Sets it to `kind` (an index of [`SC_TYPES`]) at `hz` and `q`, with
    /// the shelves' and the peak's gains in dB, as RBJ's cookbook has them.
    fn set(&mut self, kind: usize, hz: f64, q: f64, shelf_db: f64, peak_db: f64, rate: f64) {
        let w = TAU * (hz / rate).clamp(1e-6, 0.49);
        let (sin, cos) = w.sin_cos();
        let alpha = sin / (2.0 * q);
        let gain_db = match kind {
            1 => peak_db,
            0 | 2 => shelf_db,
            _ => 0.0,
        };
        let a = 10f64.powf(gain_db / 40.0);
        let root = 2.0 * a.sqrt() * alpha;
        let (b0, b1, b2, a0, a1, a2) = match kind {
            0 => (
                a * ((a + 1.0) - (a - 1.0) * cos + root),
                2.0 * a * ((a - 1.0) - (a + 1.0) * cos),
                a * ((a + 1.0) - (a - 1.0) * cos - root),
                (a + 1.0) + (a - 1.0) * cos + root,
                -2.0 * ((a - 1.0) + (a + 1.0) * cos),
                (a + 1.0) + (a - 1.0) * cos - root,
            ),
            1 => (
                1.0 + alpha * a,
                -2.0 * cos,
                1.0 - alpha * a,
                1.0 + alpha / a,
                -2.0 * cos,
                1.0 - alpha / a,
            ),
            2 => (
                a * ((a + 1.0) + (a - 1.0) * cos + root),
                -2.0 * a * ((a - 1.0) + (a + 1.0) * cos),
                a * ((a + 1.0) + (a - 1.0) * cos - root),
                (a + 1.0) - (a - 1.0) * cos + root,
                2.0 * ((a - 1.0) - (a + 1.0) * cos),
                (a + 1.0) - (a - 1.0) * cos - root,
            ),
            3 => {
                let k = (1.0 - cos) / 2.0;
                (k, 1.0 - cos, k, 1.0 + alpha, -2.0 * cos, 1.0 - alpha)
            }
            4 => (alpha, 0.0, -alpha, 1.0 + alpha, -2.0 * cos, 1.0 - alpha),
            5 => {
                let k = (1.0 + cos) / 2.0;
                (k, -(1.0 + cos), k, 1.0 + alpha, -2.0 * cos, 1.0 - alpha)
            }
            _ => (1.0, -2.0 * cos, 1.0, 1.0 + alpha, -2.0 * cos, 1.0 - alpha),
        };
        self.b = [b0 / a0, b1 / a0, b2 / a0];
        self.a = [a1 / a0, a2 / a0];
    }

    fn tick(&mut self, x: f64) -> f64 {
        let y = flush(self.b[0] * x + self.z[0]);
        self.z[0] = flush(self.b[1] * x - self.a[0] * y + self.z[1]);
        self.z[1] = flush(self.b[2] * x - self.a[1] * y);
        y
    }
}

/// Samples `x` every so often: the first call takes it, and then once the
/// interval has passed.
struct Hold {
    held: f64,
    count: f64,
}

impl Hold {
    fn new() -> Self {
        Self {
            held: 0.0,
            count: 1.0,
        }
    }

    /// `per_frame` is the fraction of an interval a frame covers.
    fn step(&mut self, x: f64, per_frame: f64) -> f64 {
        self.count += per_frame;
        if self.count >= 1.0 {
            self.count -= self.count.floor();
            self.held = x;
        }
        self.held
    }
}

/// What the filter loop reads for one channel, a frame at a time.
struct Run<'a> {
    kind: usize,
    slope: usize,
    circuit: usize,
    freq: &'a [f32],
    res: &'a [f32],
    morph: &'a [f32],
    control: &'a [f32],
    pitch: &'a [f32],
    formant: &'a [f32],
    drive: &'a [f32],
    /// The octaves the cutoff is moved by, per frame.
    octaves: &'a [f32],
}

impl Run<'_> {
    /// The plan at `frame`.
    fn plan(&self, frame: usize) -> (Plan, f64) {
        let f = |v: &[f32]| f64::from(v[frame]);
        let mut plan = Plan::new(
            self.kind,
            self.slope,
            self.circuit,
            Knobs {
                hz: f(self.freq),
                res: f(self.res) / 100.0,
                morph: f(self.morph) / 100.0,
                control: f(self.control) / 100.0,
                pitch: f(self.pitch),
                formant: f(self.formant) / 100.0,
            },
        );
        plan.shift = f(self.octaves).exp2();
        (plan, f(self.drive) / 100.0 * DRIVE_MAX)
    }

    /// Filters `samples` in place, `factor` samples to each frame of the
    /// run, from frame `*frame`. Returns the last frame's plan.
    fn filter(
        &self,
        chan: &mut Chan,
        samples: &mut [f32],
        factor: usize,
        frame: &mut usize,
        rate: f64,
    ) {
        if samples.is_empty() {
            return;
        }
        let first = *frame;
        let mut current = self.plan(first);
        for (j, sample) in samples.iter_mut().enumerate() {
            if j % factor == 0 {
                current = self.plan(first + j / factor);
            }
            let x = f64::from(*sample);
            let x = if x.is_finite() { x } else { 0.0 };
            *sample = chan.tick(drive(x, current.1), &current.0, rate * factor as f64) as f32;
        }
        *frame += samples.len() / factor;
    }
}

/// Auto Filter's audio-thread half.
pub struct AutoFilter {
    rate: f64,
    channels: Vec<Chan>,
    oversampler: Oversampler,
    /// The frames Hi-Quality delays by, now.
    latency: u32,
    /// The dry signal, delayed to meet the filtered one, and the channel's
    /// input before the filter changes it.
    dry: [DelayLine; 2],
    dry_buf: Vec<f32>,
    lfo: Lfo,
    followers: [EnvelopeFollower; 2],
    sc_filters: [Biquad; 2],
    lfo_hold: [Hold; 2],
    env_hold: [Hold; 2],
    /// The step each channel's LFO was last in, with Steps.
    step_index: [i64; 2],
    step_value: [f64; 2],
    /// What the detector hears, and the octaves the cutoff is moved by,
    /// for each channel.
    keys: [Vec<f32>; 2],
    octaves: [Vec<f32>; 2],
    /// The cutoff in Hz at the last frame of the last block, per channel,
    /// and whether Clip clipped.
    cutoffs: [f32; 2],
    clipped: bool,
}

impl AutoFilter {
    fn new(device: &Device, config: &NodeConfig) -> Self {
        let rate = config.sample_rate.as_f64();
        let frames = config.max_frames.max(1);
        let oversampler = Oversampler::new(HI_QUALITY_STAGES, 2, frames);
        let hq = device.params.get(&HI_QUALITY).is_some_and(|v| *v >= 0.5);
        let latency = if hq { oversampler.latency() } else { 0 };
        let max_latency = oversampler.latency() as usize;
        Self {
            rate,
            channels: (0..2).map(|_| Chan::new(rate)).collect(),
            oversampler,
            latency,
            dry: [DelayLine::new(max_latency), DelayLine::new(max_latency)],
            dry_buf: vec![0.0; frames],
            lfo: Lfo::new(config.sample_rate, device.id.0.get()),
            followers: [EnvelopeFollower::new(rate), EnvelopeFollower::new(rate)],
            sc_filters: [Biquad::default(); 2],
            lfo_hold: [Hold::new(), Hold::new()],
            env_hold: [Hold::new(), Hold::new()],
            step_index: [-1; 2],
            step_value: [0.0; 2],
            keys: [vec![0.0; frames], vec![0.0; frames]],
            octaves: [vec![0.0; frames], vec![0.0; frames]],
            cutoffs: [1_000.0; 2],
            clipped: false,
        }
    }
}

impl NativeDevice for AutoFilter {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        let frames = audio.frames();
        let channels = audio.channels().min(2);
        if frames == 0 || channels == 0 {
            return;
        }
        let at = |id: ParamId| params.index(id).expect("the description holds it");
        let on = |id: ParamId| params.value(at(id)) >= 0.5;
        let choice = |id: ParamId| params.value(at(id)) as usize;
        let target = |id: ParamId| params.target(at(id));
        let ramp = |id: ParamId| params.ramp(at(id));

        // 1. The LFO's rate and stereo mode, and the followers.
        let wave_index = choice(LFO_WAVE).min(WAVES.len() - 1);
        let wave = WAVE_OF[wave_index];
        let rate = match choice(LFO_MODE) {
            0 => Rate::Hertz(target(LFO_HZ)),
            1 => Rate::Seconds(target(LFO_TIME)),
            2 => Rate::Beats(DIVISIONS[choice(LFO_DIVISION).min(DIVISIONS.len() - 1)].1),
            _ => Rate::Beats(0.25 * target(LFO_SIXTEENTHS).round()),
        };
        // Wander and S&H take the same random values on both channels, and
        // Spin has no effect on them (the manual).
        let stereo = if choice(LFO_STEREO) == 1 && !wave.is_random() {
            Stereo::Spin(target(LFO_SPIN))
        } else if choice(LFO_STEREO) == 1 {
            Stereo::Phase(0.0)
        } else {
            Stereo::Phase(target(LFO_PHASE))
        };
        self.lfo.set(audio, rate, stereo, target(LFO_OFFSET));
        for follower in &mut self.followers {
            follower.set(
                target(ENV_ATTACK) / 1_000.0,
                target(ENV_RELEASE) / 1_000.0,
                on(ENV_HOLD),
            );
        }
        // How much of a beat-time interval a frame covers, at the tempo of
        // the stretch's first frame.
        let tempo = audio.tempo();
        let bpm = audio.bpm();
        let rate_hz = self.rate;
        let per_frame = |division: usize| {
            let beats = DIVISIONS[division.min(DIVISIONS.len() - 1)].1;
            bpm / 60.0 / (beats * rate_hz)
        };
        let (sh_lfo, sh_env) = (per_frame(choice(LFO_SH)), per_frame(choice(ENV_SH_RATE)));
        let quantize = choice(LFO_QUANTIZE);
        let steps = target(LFO_STEPS).round().max(2.0);
        let (env_sh, mono) = (on(ENV_SH), on(MONO_SIDECHAIN));
        let morph = ramp(LFO_MORPH);
        let (lfo_amount, env_amount) = (ramp(LFO_AMOUNT), ramp(ENV_AMOUNT));

        // 2. The detector's input: the sidechain if the device has one on,
        // else its own input; through the SC filter when it's on.
        for ch in 0..channels {
            for i in 0..frames {
                let own = audio.channel_mut(ch)[i];
                self.keys[ch][i] = audio.sidechain(ch).map_or(own, |key| key[i]);
            }
        }
        if mono && channels == 2 {
            for i in 0..frames {
                let mean = 0.5 * (self.keys[0][i] + self.keys[1][i]);
                self.keys[0][i] = mean;
                self.keys[1][i] = mean;
            }
        }
        let sc_on = on(SC_FILTER);
        if sc_on {
            for filter in &mut self.sc_filters {
                filter.set(
                    choice(SC_TYPE),
                    target(SC_FREQ),
                    target(SC_Q),
                    target(SC_SHELF),
                    target(SC_PEAK),
                    self.rate,
                );
            }
        }

        // 3. The modulation, a frame at a time: the LFO's and the
        // envelope's octaves of movement for each channel.
        for i in 0..frames {
            let shape = if wave == Wave::SampleHold {
                Shape {
                    morph: Some((Wave::Random, f64::from(morph[i]) / 100.0)),
                    ..Shape::of(wave)
                }
            } else {
                Shape {
                    duty: f64::from(morph[i]) / 100.0,
                    ..Shape::of(wave)
                }
            };
            let mut lfo = self.lfo.step(tempo, i, shape);
            let phases = self.lfo.phases();
            for ch in 0..channels {
                lfo[ch] = match quantize {
                    1 => {
                        let index = (phases[ch] * steps).floor() as i64;
                        if index != self.step_index[ch] {
                            self.step_index[ch] = index;
                            self.step_value[ch] = lfo[ch];
                        }
                        self.step_value[ch]
                    }
                    2 => self.lfo_hold[ch].step(lfo[ch], sh_lfo),
                    _ => lfo[ch],
                };
            }
            // One detector when mono: the first follower's level serves both.
            let key = |ch: usize, own: &mut Self| {
                let x = f64::from(own.keys[ch][i]);
                let x = if sc_on {
                    own.sc_filters[ch].tick(x) as f32
                } else {
                    x as f32
                };
                own.followers[ch].process(x)
            };
            let levels = if mono {
                let level = key(0, self);
                [level, level]
            } else {
                [key(0, self), if channels == 2 { key(1, self) } else { 0.0 }]
            };
            for ch in 0..channels {
                let level = levels[ch].min(1.0);
                let level = if env_sh {
                    self.env_hold[ch].step(level, sh_env)
                } else {
                    level
                };
                self.octaves[ch][i] = (f64::from(lfo_amount[i]) / 100.0 * LFO_OCTAVES * lfo[ch]
                    + f64::from(env_amount[i]) / 100.0 * ENV_OCTAVES * level)
                    as f32;
            }
        }

        // 4. The filter, then Output, Clip and the mix with the dry signal.
        let hq = on(HI_QUALITY);
        self.latency = if hq { self.oversampler.latency() } else { 0 };
        let latency = self.latency as usize;
        let (output, wet) = (ramp(OUTPUT), ramp(DRY_WET));
        let clip = on(CLIP);
        let (kind, slope, circuit) = (choice(TYPE), choice(SLOPE), choice(CIRCUIT));
        let (freq, res, morph_p) = (ramp(FREQ), ramp(RES), ramp(MORPH));
        let (control, pitch, formant, drive_p) =
            (ramp(CONTROL), ramp(PITCH), ramp(FORMANT), ramp(DRIVE));
        let Self {
            channels: chans,
            oversampler,
            dry,
            dry_buf,
            octaves,
            cutoffs,
            clipped,
            rate: sample_rate,
            ..
        } = self;
        for ch in 0..channels {
            let run = Run {
                kind,
                slope,
                circuit,
                freq,
                res,
                morph: morph_p,
                control,
                pitch,
                formant,
                drive: drive_p,
                octaves: &octaves[ch][..frames],
            };
            let samples = audio.channel_mut(ch);
            dry_buf[..frames].copy_from_slice(samples);
            let mut frame = 0;
            if hq {
                oversampler.process(ch, samples, |high| {
                    run.filter(&mut chans[ch], high, 2, &mut frame, *sample_rate);
                });
            } else {
                run.filter(&mut chans[ch], samples, 1, &mut frame, *sample_rate);
            }
            for i in 0..frames {
                let aligned = if latency == 0 {
                    dry_buf[i]
                } else {
                    dry[ch].read(latency)
                };
                dry[ch].write(dry_buf[i]);
                let mut y = f64::from(samples[i]) * f64::from(output[i]);
                if clip {
                    let (soft, hot) = soft_clip(y);
                    y = soft;
                    *clipped |= hot;
                }
                let w = f64::from(wet[i]) / 100.0;
                samples[i] = (f64::from(aligned) * (1.0 - w) + y * w) as f32;
            }
            let last = frames - 1;
            cutoffs[ch] = (f64::from(freq[last]) * f64::from(octaves[ch][last]).exp2()) as f32;
        }
    }

    fn latency(&self) -> u32 {
        self.latency
    }

    fn display_len(&self) -> usize {
        DISPLAY_VALUES
    }

    fn display(&mut self, out: &mut [f32]) {
        out[..2].copy_from_slice(&self.cutoffs);
        out[2] = f32::from(u8::from(self.clipped));
        self.clipped = false;
    }
}

/// Clip: the signal as it is up to [`CLIP_KNEE`], then rounded off to a
/// peak of 1. Also whether it went past the knee.
fn soft_clip(x: f64) -> (f64, bool) {
    let a = x.abs();
    if a <= CLIP_KNEE {
        (x, false)
    } else {
        let room = 1.0 - CLIP_KNEE;
        (
            x.signum() * (CLIP_KNEE + room * ((a - CLIP_KNEE) / room).tanh()),
            true,
        )
    }
}

/// What the node publishes: the modulated Freq of the left and the right
/// channel in Hz, and 1 if Clip clipped since the last block.
const DISPLAY_VALUES: usize = 3;

/// The curve's points, one set of them for each channel's cutoff.
const CURVE_POINTS: usize = 96;
const CURVE_RANGE_DB: [f32; 2] = [-36.0, 24.0];

/// The rate the display's curves are worked out at, as the EQ's are.
/// shortcut: 48 kHz for every set, when the engine's rate reaches the shell.
const DISPLAY_RATE: f64 = 48_000.0;

/// The device's display (the auto-filter spec, R13): the filter's curve at
/// its set cutoff and, with an engine publishing, the curves of the left and
/// right channels at their modulated cutoffs, a marker at each cutoff, and
/// the cutoffs and the Clip light as readings. `data` is what the node
/// published, if an engine runs.
#[must_use]
pub fn display(
    device: DeviceId,
    description: &DeviceDescription,
    values: &OrdMap<ParamId, f64>,
    data: Option<&[f32]>,
) -> DeviceDisplay {
    let plan = plan_of(description, values);
    let curve = |name: &str, plan: &Plan| DisplaySeries {
        name: name.to_owned(),
        unit: "dB".to_owned(),
        x_unit: "Hz".to_owned(),
        x_range: [MIN_HZ as f32, MAX_HZ as f32],
        log_x: true,
        y_range: CURVE_RANGE_DB,
        detail: false,
        values: (0..CURVE_POINTS)
            .map(|i| {
                let hz = MIN_HZ * (MAX_HZ / MIN_HZ).powf(i as f64 / (CURVE_POINTS - 1) as f64);
                let db = 20.0 * plan.response(hz, DISPLAY_RATE).abs().max(1e-12).log10();
                db.clamp(f64::from(CURVE_RANGE_DB[0]), f64::from(CURVE_RANGE_DB[1])) as f32
            })
            .collect(),
    };
    let mut out = DeviceDisplay {
        device,
        live: data.is_some(),
        view: None,
        series: vec![curve("Filter Curve", &plan)],
        markers: Vec::new(),
        readings: Vec::new(),
    };
    if let Some([left, right, clip]) = data.and_then(|d| <[f32; DISPLAY_VALUES]>::try_from(d).ok())
    {
        for (name, hz) in [("Left", left), ("Right", right)] {
            let mut moved = plan;
            moved.shift = f64::from(hz) / plan.hz;
            out.series.push(curve(&format!("{name} Curve"), &moved));
            out.markers.push(DisplayMarker {
                name: format!("{name} Cutoff"),
                series: format!("{name} Curve"),
                x: hz,
                y: 20.0
                    * moved
                        .response(f64::from(hz), DISPLAY_RATE)
                        .abs()
                        .max(1e-12)
                        .log10() as f32,
                text: format!("{name} cutoff {hz:.0} Hz"),
            });
            out.readings.push(DisplayReading {
                name: format!("{name} Cutoff"),
                value: hz,
                unit: "Hz".to_owned(),
                text: format!("{hz:.0} Hz"),
            });
        }
        out.readings.push(DisplayReading {
            name: "Clip".to_owned(),
            value: clip,
            unit: String::new(),
            text: if clip >= 0.5 { "Clipping" } else { "Clean" }.to_owned(),
        });
    }
    out
}

//! Roar, the native Roar (WP-8.38 and WP-8.39; the roar spec): an input
//! section (Drive and a Tone tilt that Color Compensation undoes afterwards),
//! three gain stages each with a shaper and a filter, seven routing modes, a
//! 4 times oversampled path, a global output and the shaper's display; and,
//! in part 2, two LFOs, an envelope follower and a noise source that move
//! the controls through a matrix, a feedback section with a Note mode a
//! MIDI track can play, a compressor and both sidechains.
//!
//! The signal goes through Drive and the Tone tilt, then, at four times the
//! rate, through the stages as the routing mode joins them, and back down
//! through the tilt's inverse, Output Gain, a hard clip and the mix with the
//! dry signal. Every curve and filter is Beatlayer's own, written in the
//! spec; the manual names them but gives no ranges or formulas. The six
//! filters Roar shares with the Auto Filter are its filters ([`crate::filters`]).

use core::f64::consts::{FRAC_PI_2, PI};

use bl_dsp::{Oversampler, Random};
use bl_engine::{DeviceType, NodeConfig, NoteEvent, NoteKind, Registry};
use bl_model::device_layout::{
    ControlBand, DisplayCell, LayoutNode, PADDING, Page, PageStyle, PagesNode, Widget,
};
use bl_model::param::db_to_gain;
use bl_model::{
    Bank, Device, DeviceDescription, DeviceDisplay, DeviceId, DeviceLayout, DeviceRole,
    DisplayMarker, DisplayReading, DisplaySeries, OrdMap, ParamDescriptor, ParamId, ParamUnit,
};

use crate::filters::{Chan, Circuit, Coef, Knobs, Plan, Stage, flush, q_of};
use crate::framework::{Audio, DelayLine, NativeDevice, ParamBank, add};
use crate::lfo::Rate;
use crate::multiband_dynamics::Split;
use crate::roar_parts::{
    Biquad, EnvSource, GRID, Gate, Glide, Law, LfoSettings, LfoSource, Noise, Squash,
};

/// Roar's device type.
pub const ROAR: &str = "beatlayer.roar";

/// Drive: the gain into every stage, in dB.
pub const DRIVE: ParamId = ParamId(0);
/// Tone Amount: the tilt's depth, per cent.
pub const TONE_AMOUNT: ParamId = ParamId(1);
/// Tone Frequency: the tilt's centre, in Hz.
pub const TONE_FREQUENCY: ParamId = ParamId(2);
/// Color Compensation: undoes the tilt after the stages.
pub const COLOR_COMPENSATION: ParamId = ParamId(3);
/// Routing Mode: one of [`MODES`].
pub const ROUTING_MODE: ParamId = ParamId(4);
/// Blend: the crossfade between a mode's two signals, per cent.
pub const BLEND: ParamId = ParamId(5);
/// Low Crossover: Multi Band's lower split, in Hz.
pub const LOW_CROSSOVER: ParamId = ParamId(6);
/// High Crossover: Multi Band's upper split, in Hz.
pub const HIGH_CROSSOVER: ParamId = ParamId(7);
/// Output Gain: the wet signal's level, before the hard clip.
pub const OUTPUT_GAIN: ParamId = ParamId(8);
/// Dry/Wet, per cent.
pub const DRY_WET: ParamId = ParamId(9);

/// A stage's switch, within its block of parameters (see [`stage_param`]).
pub const ON: u32 = 0;
/// Shaper: one of [`SHAPERS`].
pub const SHAPER: u32 = 1;
/// Shaper Amount, per cent.
pub const AMOUNT: u32 = 2;
/// Shaper Bias, per cent.
pub const BIAS: u32 = 3;
/// Shaper Level: the stage's output gain.
pub const LEVEL: u32 = 4;
/// Filter: one of [`FILTERS`].
pub const FILTER: u32 = 5;
/// The filter's frequency, in Hz.
pub const FREQUENCY: u32 = 6;
/// The filter's Resonance, per cent.
pub const RESONANCE: u32 = 7;
/// Peak Gain, in dB.
pub const PEAK_GAIN: u32 = 8;
/// Morph, per cent.
pub const MORPH: u32 = 9;
/// Pre: the filter before the shaper.
pub const PRE: u32 = 10;

const FIRST_STAGE: u32 = 10;
const PER_STAGE: u32 = 11;

/// The stages in a device.
pub const STAGES: usize = 3;

/// Feedback Mode: one of [`FEEDBACK_MODES`] (the roar spec, R24).
pub const FB_MODE: ParamId = ParamId(43);
/// Feedback Time, in ms (Time mode).
pub const FB_TIME: ParamId = ParamId(44);
/// Feedback's division in Synced mode, one of [`NOTES`] up to 4 bars.
pub const FB_SYNCED: ParamId = ParamId(45);
/// Feedback's division in Triplet mode.
pub const FB_TRIPLET: ParamId = ParamId(46);
/// Feedback's division in Dotted mode.
pub const FB_DOTTED: ParamId = ParamId(47);
/// Feedback Note: the key whose pitch the path rings at (Note mode).
pub const FB_NOTE: ParamId = ParamId(48);
/// Feedback Amount: how much returns, per cent.
pub const FB_AMOUNT: ParamId = ParamId(49);
/// Feedback Invert: flips the sign of what returns.
pub const FB_INVERT: ParamId = ParamId(50);
/// Feedback Gate: the feedback fades once the input stops.
pub const FB_GATE: ParamId = ParamId(51);
/// The feedback filter's frequency, in Hz.
pub const FB_FREQUENCY: ParamId = ParamId(52);
/// The feedback filter's width, in octaves.
pub const FB_WIDTH: ParamId = ParamId(53);
/// Compression Amount, per cent (R27).
pub const COMPRESSION: ParamId = ParamId(54);
/// Sidechain HP: the compressor's detector hears a high-pass.
pub const SIDECHAIN_HP: ParamId = ParamId(55);
/// MIDI > FB Note: notes from the MIDI sidechain set Feedback Note (R25).
pub const MIDI_FB_NOTE: ParamId = ParamId(56);
/// Global Modulation Amount, per cent (R23).
pub const GLOBAL_MODULATION: ParamId = ParamId(57);

/// An LFO's waveform, within its block of parameters (see [`lfo_param`]).
pub const LFO_WAVE: u32 = 0;
/// An LFO's Mode: one of [`LFO_MODES`].
pub const LFO_MODE: u32 = 1;
/// An LFO's rate in Free mode, in Hz.
pub const LFO_RATE: u32 = 2;
/// An LFO's division in Synced mode, one of [`NOTES`].
pub const LFO_SYNCED: u32 = 3;
/// An LFO's division in Triplet mode.
pub const LFO_TRIPLET: u32 = 4;
/// An LFO's division in Dotted mode.
pub const LFO_DOTTED: u32 = 5;
/// An LFO's length in sixteenths, in Sixteenth mode.
pub const LFO_SIXTEENTHS: u32 = 6;
/// An LFO's Morph: its duty cycle, per cent.
pub const LFO_MORPH: u32 = 7;
/// An LFO's Smooth, per cent.
pub const LFO_SMOOTH: u32 = 8;
const FIRST_LFO: u32 = 58;
const PER_LFO: u32 = 9;

/// The parameter `offset` ([`LFO_WAVE`] to [`LFO_SMOOTH`]) of LFO `lfo` (0
/// or 1).
#[must_use]
pub const fn lfo_param(lfo: usize, offset: u32) -> ParamId {
    ParamId(FIRST_LFO + PER_LFO * lfo as u32 + offset)
}

/// Env's Attack, in ms.
pub const ENV_ATTACK: ParamId = ParamId(76);
/// Env's Release, in ms.
pub const ENV_RELEASE: ParamId = ParamId(77);
/// Env's Hold.
pub const ENV_HOLD: ParamId = ParamId(78);
/// Env's Threshold, in dBFS.
pub const ENV_THRESHOLD: ParamId = ParamId(79);
/// Env's Gain, in dB.
pub const ENV_GAIN: ParamId = ParamId(80);
/// Env's detector frequency, in Hz.
pub const ENV_FREQUENCY: ParamId = ParamId(81);
/// Env's detector width, in octaves.
pub const ENV_WIDTH: ParamId = ParamId(82);
/// Input Listen: the output is what Env's detector hears.
pub const INPUT_LISTEN: ParamId = ParamId(83);
/// The noise source's type: one of [`NOISE_TYPES`].
pub const NOISE_TYPE: ParamId = ParamId(84);
/// The noise source's rate, in Hz.
pub const NOISE_RATE: ParamId = ParamId(85);

/// The matrix's sources, in its columns' order.
pub const SOURCES: [&str; 4] = ["LFO 1", "LFO 2", "Env", "Noise"];
/// The parameters the matrix can move, in its rows' order (R23).
pub const TARGET_IDS: [ParamId; TARGETS] = [
    DRIVE,
    TONE_AMOUNT,
    TONE_FREQUENCY,
    BLEND,
    LOW_CROSSOVER,
    HIGH_CROSSOVER,
    stage_param(0, AMOUNT),
    stage_param(0, BIAS),
    stage_param(0, LEVEL),
    stage_param(0, FREQUENCY),
    stage_param(0, RESONANCE),
    stage_param(0, PEAK_GAIN),
    stage_param(0, MORPH),
    stage_param(1, AMOUNT),
    stage_param(1, BIAS),
    stage_param(1, LEVEL),
    stage_param(1, FREQUENCY),
    stage_param(1, RESONANCE),
    stage_param(1, PEAK_GAIN),
    stage_param(1, MORPH),
    stage_param(2, AMOUNT),
    stage_param(2, BIAS),
    stage_param(2, LEVEL),
    stage_param(2, FREQUENCY),
    stage_param(2, RESONANCE),
    stage_param(2, PEAK_GAIN),
    stage_param(2, MORPH),
    FB_TIME,
    FB_AMOUNT,
    FB_FREQUENCY,
    FB_WIDTH,
];
/// How many parameters the matrix can move.
pub const TARGETS: usize = 31;
const FIRST_MATRIX: u32 = 86;

/// The amount of source `source` (0 to 3) on target `target` (a place in
/// [`TARGET_IDS`]).
#[must_use]
pub const fn matrix_param(source: usize, target: usize) -> ParamId {
    ParamId(FIRST_MATRIX + (source * TARGETS + target) as u32)
}

/// The feedback modes, in the chooser's order.
pub const FEEDBACK_MODES: [&str; 5] = ["Time", "Synced", "Triplet", "Dotted", "Note"];
const SYNCED_MODE: usize = 1;
const TRIPLET_MODE: usize = 2;
const DOTTED_MODE: usize = 3;
/// The LFOs' modes, in the chooser's order.
pub const LFO_MODES: [&str; 5] = ["Free", "Synced", "Triplet", "Dotted", "Sixteenth"];
/// The LFOs' waves, in the chooser's order.
pub const LFO_WAVES: [&str; 5] = ["Sine", "Triangle", "Square", "Up", "Down"];
/// The noise source's types, in the chooser's order.
pub const NOISE_TYPES: [&str; 4] = ["Simplex", "Wander", "S & H", "Brown"];
/// The note divisions, from 1/64 to 8 bars, with their length in quarter
/// notes; the feedback offers the first nine.
pub const NOTES: [(&str, f64); 10] = [
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
];
/// The feedback divisions on offer, up to 4 bars.
const FEEDBACK_NOTES: usize = 9;

/// The longest feedback loop, in seconds: Time tops out at 2 s, and a synced
/// division past this holds here.
const LOOP_MAX_SECONDS: f64 = 4.0;
/// The feedback path's reference pitch: key 69 (A3) at 440 Hz, equal
/// tuning.
const REFERENCE_KEY: f64 = 69.0;
/// How far a full pitch bend moves Note mode's pitch, in semitones.
const BEND_RANGE: f64 = 2.0;
/// The most the loop returns at Amount 100 %, which keeps its gain below 1.
const LOOP_CEILING: f64 = 0.98;
/// Points in a source's display, and the seconds it covers.
pub const TRACE_POINTS: usize = 64;
const TRACE_SECONDS: f64 = 2.0;
/// Notes a stretch can hold for the MIDI sidechain.
const EVENT_ROOM: usize = 512;
/// Keys held at once that Note mode remembers.
const HELD_KEYS: usize = 16;

/// The parameter `offset` ([`ON`] to [`PRE`]) of stage `stage` (0 to 2).
#[must_use]
pub const fn stage_param(stage: usize, offset: u32) -> ParamId {
    ParamId(FIRST_STAGE + PER_STAGE * stage as u32 + offset)
}

/// The routing modes, in the chooser's order.
pub const MODES: [&str; 7] = [
    "Single",
    "Serial",
    "Parallel",
    "Multi Band",
    "Mid Side",
    "Feedback",
    "Delay",
];
const SERIAL: usize = 1;
const PARALLEL: usize = 2;
const MULTI_BAND: usize = 3;
const MID_SIDE: usize = 4;
const FEEDBACK: usize = 5;
const DELAY: usize = 6;

/// The shaper curves, in the chooser's order.
pub const SHAPERS: [&str; 12] = [
    "Soft Sine",
    "Digital Clip",
    "Bit Crusher",
    "Diode Clipper",
    "Tube Preamp",
    "Half Wave Rectifier",
    "Full Wave Rectifier",
    "Polynomial",
    "Fractal",
    "Tri Fold",
    "Noise Injection",
    "Shards",
];
const BIT_CRUSHER: usize = 2;
const NOISE_INJECTION: usize = 10;

/// The filter types, in the chooser's order.
pub const FILTERS: [&str; 9] = [
    "LP",
    "BP",
    "HP",
    "Notch",
    "Peak",
    "Morph",
    "Comb",
    "Resampling",
    "Dispersion",
];
const PEAK: usize = 4;
const MORPH_FILTER: usize = 5;
const DISPERSION: usize = 8;

/// The Auto Filter's type for each of Roar's filters; Peak and Dispersion
/// are Roar's own.
const SHARED: [Option<usize>; 9] = [
    Some(0),
    Some(2),
    Some(1),
    Some(3),
    None,
    Some(5),
    Some(7),
    Some(8),
    None,
];

/// The name of Roar's pages node, which `device.set_page` takes.
pub const PAGES: &str = "Roar";

/// The display's views, one per stage.
const VIEWS: [&str; STAGES] = ["Stage 1", "Stage 2", "Stage 3"];

/// The stages run at four times the rate.
const OVERSAMPLE_STAGES: u32 = 2;
const FACTOR: usize = 1 << OVERSAMPLE_STAGES;

/// The shaper's gain at Amount 100 %, in dB.
const SHAPER_DB: f64 = 36.0;
/// The tilt's ends at Tone Amount 100 %, in dB.
const TONE_DB: f64 = 12.0;
/// The DC blocker's corner, in Hz.
const DC_HZ: f64 = 5.0;
/// A change of shaper or filter type crossfades over this long.
const FADE_SECONDS: f64 = 0.010;
/// Frames between the tilt's coefficient updates, and samples (at the high
/// rate) between the crossovers'. Counted from the device's first frame, so a
/// render doesn't depend on its block size.
const TONE_REFRESH: u64 = 16;
const CROSS_REFRESH: u64 = 64;
/// Dispersion's allpass sections.
const SECTIONS: usize = 8;
/// The largest value the feedback loop holds: a loop with a stage that has
/// gain in it could otherwise grow without end.
const LOOP_LIMIT: f64 = 8.0;
/// The shaper display: points across inputs from -1 to 1.
pub const CURVE_POINTS: usize = 128;
const CURVE_PEAK: f32 = 2.0;
const LEVEL_FLOOR_DB: f32 = -120.0;

/// The shaper curve `kind` at `u` (the spec's R4): `amount` from 0 to 1 and
/// `noise`, a white noise value from -1 to 1.
fn curve(kind: usize, u: f64, amount: f64, noise: f64) -> f64 {
    let c = u.clamp(-1.0, 1.0);
    match kind {
        0 => (FRAC_PI_2 * c).sin(),
        1 => c,
        BIT_CRUSHER => {
            let steps = (15.0 - 14.0 * amount).exp2();
            (c * steps).round() / steps
        }
        3 => u / (1.0 + u.abs().powf(2.5)).powf(0.4),
        4 => {
            if u >= 0.0 {
                u.tanh()
            } else {
                0.7 * (u / 0.7).tanh()
            }
        }
        5 => u.max(0.0).tanh(),
        6 => u.abs().tanh(),
        7 => 1.5 * (c - c * c * c / 3.0),
        8 => {
            (0..4)
                .map(|k| 0.5f64.powi(k) * (f64::from(1 << k) * FRAC_PI_2 * c).sin())
                .sum::<f64>()
                / 1.875
        }
        9 => 2.0 / PI * (FRAC_PI_2 * u).sin().asin(),
        NOISE_INJECTION => (u + amount * noise * u.abs()).tanh(),
        _ => {
            let s = 8.0 * c;
            (s.floor() + (s - s.floor()).powi(4)) / 8.0
        }
    }
}

/// A shaper's output for the input `x`: the curve of `g x + bias` less the
/// curve at the bias, so silence stays silence. `amount` and `bias` are
/// fractions (0 to 1, -1 to 1).
fn shape(kind: usize, amount: f64, bias: f64, noise: f64, x: f64) -> f64 {
    let gain = if kind == BIT_CRUSHER {
        1.0
    } else {
        db_to_gain(amount * SHAPER_DB)
    };
    curve(kind, gain * x + bias, amount, noise) - curve(kind, bias, amount, noise)
}

/// Roar's device type.
#[must_use]
pub fn roar_type() -> DeviceType {
    let percent = |id, name: &str, default| {
        ParamDescriptor::continuous(id, name, (0.0, 100.0), default, ParamUnit::Percent)
    };
    let mut params = vec![
        ParamDescriptor::continuous(DRIVE, "Drive", (0.0, 36.0), 0.0, ParamUnit::Decibels)
            .describe("The gain into every gain stage."),
        ParamDescriptor::continuous(
            TONE_AMOUNT,
            "Tone Amount",
            (-100.0, 100.0),
            0.0,
            ParamUnit::Percent,
        )
        .describe("Tilts the spectrum about Tone Frequency before the stages: highs up above 0, lows up below."),
        ParamDescriptor::continuous(
            TONE_FREQUENCY,
            "Tone Frequency",
            (100.0, 10_000.0),
            1_000.0,
            ParamUnit::Hertz,
        )
        .logarithmic()
        .describe("The centre of the Tone tilt."),
        ParamDescriptor::toggle(COLOR_COMPENSATION, "Color Compensation", false)
            .describe("Applies the Tone tilt the other way after the stages, so only the stages hear it."),
        ParamDescriptor::choice(ROUTING_MODE, "Routing Mode", &MODES, 0)
            .describe("How the gain stages are joined."),
        percent(BLEND, "Blend", 50.0).describe(
            "Crossfades between the routing mode's two signals; Single and Multi Band have none.",
        ),
        ParamDescriptor::continuous(
            LOW_CROSSOVER,
            "Low Crossover",
            (50.0, 2_000.0),
            250.0,
            ParamUnit::Hertz,
        )
        .logarithmic()
        .describe("Multi Band: where the low band ends."),
        ParamDescriptor::continuous(
            HIGH_CROSSOVER,
            "High Crossover",
            (500.0, 16_000.0),
            2_500.0,
            ParamUnit::Hertz,
        )
        .logarithmic()
        .describe("Multi Band: where the high band begins."),
        ParamDescriptor::gain(OUTPUT_GAIN, "Output Gain", 12.0, 0.0)
            .describe("The level of the wet signal, which then passes a hard clip at 0 dBFS."),
        percent(DRY_WET, "Dry/Wet", 100.0)
            .describe("The balance between the dry signal and the processed one."),
    ];
    for stage in 0..STAGES {
        let n = stage + 1;
        let id = |offset| stage_param(stage, offset);
        let name = |what: &str| format!("Stage {n} {what}");
        params.extend([
            ParamDescriptor::toggle(id(ON), &format!("Stage {n}"), true)
                .describe("Switches the stage on; off, it passes the signal untouched."),
            ParamDescriptor::choice(id(SHAPER), &name("Shaper"), &SHAPERS, 0)
                .describe("The curve that shapes the signal."),
            percent(id(AMOUNT), &name("Amount"), 0.0)
                .describe("How hard the shaper drives its curve."),
            ParamDescriptor::continuous(
                id(BIAS),
                &name("Bias"),
                (-100.0, 100.0),
                0.0,
                ParamUnit::Percent,
            )
            .describe("Shifts the signal on the curve, making its distortion lopsided."),
            ParamDescriptor::gain(id(LEVEL), &name("Level"), 12.0, 0.0)
                .describe("The level of the stage's output."),
            ParamDescriptor::choice(id(FILTER), &name("Filter"), &FILTERS, 0)
                .describe("The stage's filter."),
            ParamDescriptor::continuous(
                id(FREQUENCY),
                &name("Frequency"),
                (20.0, 20_000.0),
                20_000.0,
                ParamUnit::Hertz,
            )
            .logarithmic()
            .describe("The filter's frequency."),
            percent(id(RESONANCE), &name("Resonance"), 0.0).describe(
                "The filter's resonance; Comb's feedback and Dispersion's Q. Resampling has none.",
            ),
            ParamDescriptor::continuous(
                id(PEAK_GAIN),
                &name("Peak Gain"),
                (-24.0, 24.0),
                0.0,
                ParamUnit::Decibels,
            )
            .describe("Peak only: the gain at the filter's frequency."),
            percent(id(MORPH), &name("Morph"), 0.0)
                .describe("Morph only: low-pass at 0, band-pass at 50 %, high-pass at 100 %."),
            ParamDescriptor::toggle(id(PRE), &name("Pre"), false)
                .describe("Puts the filter before the shaper instead of after it."),
        ]);
    }
    params.extend(part_two_params(&params));
    // Every band sits on a page, under the tab row; a page holds four number
    // boxes a column, which a source's amounts fill.
    let page_height = 4.0 * Widget::Number.size().height + 5.0 * PADDING;
    let band = |controls: &[(ParamId, Widget)]| {
        LayoutNode::Band(ControlBand::stacked(page_height, controls))
    };
    let stage_band = |stage: usize| {
        use Widget::{Chooser, Knob, Toggle};
        let id = |offset| stage_param(stage, offset);
        band(&[
            (id(ON), Toggle),
            (id(SHAPER), Chooser),
            (id(AMOUNT), Knob),
            (id(BIAS), Knob),
            (id(LEVEL), Knob),
            (id(FILTER), Chooser),
            (id(FREQUENCY), Knob),
            (id(RESONANCE), Knob),
            (id(PEAK_GAIN), Knob),
            (id(MORPH), Knob),
            (id(PRE), Toggle),
        ])
    };
    let lfo_band = |lfo: usize| {
        use Widget::{Chooser, Knob};
        let id = |offset| lfo_param(lfo, offset);
        band(&[
            (id(LFO_WAVE), Chooser),
            (id(LFO_MODE), Chooser),
            (id(LFO_RATE), Knob),
            (id(LFO_SYNCED), Chooser),
            (id(LFO_TRIPLET), Chooser),
            (id(LFO_DOTTED), Chooser),
            (id(LFO_SIXTEENTHS), Knob),
            (id(LFO_MORPH), Knob),
            (id(LFO_SMOOTH), Knob),
        ])
    };
    // A source's page of the matrix: its amount for each target.
    let amounts = |source: usize| {
        let amounts: Vec<(ParamId, Widget)> = (0..TARGETS)
            .map(|target| (matrix_param(source, target), Widget::Number))
            .collect();
        band(&amounts)
    };
    let layout = {
        use Widget::{Chooser, Knob, Toggle};
        let device = LayoutNode::Row(vec![
            band(&[
                (DRIVE, Knob),
                (TONE_AMOUNT, Knob),
                (TONE_FREQUENCY, Knob),
                (COLOR_COMPENSATION, Toggle),
            ]),
            LayoutNode::Cell(DisplayCell::new("Shaper", 150.0, page_height, 1.0)),
            stage_band(0),
            stage_band(1),
            stage_band(2),
            band(&[
                (ROUTING_MODE, Chooser),
                (BLEND, Knob),
                (LOW_CROSSOVER, Knob),
                (HIGH_CROSSOVER, Knob),
                (COMPRESSION, Knob),
                (SIDECHAIN_HP, Toggle),
                (GLOBAL_MODULATION, Knob),
                (OUTPUT_GAIN, Knob),
                (DRY_WET, Knob),
            ]),
            band(&[
                (FB_MODE, Chooser),
                (FB_TIME, Knob),
                (FB_SYNCED, Chooser),
                (FB_TRIPLET, Chooser),
                (FB_DOTTED, Chooser),
                (FB_NOTE, Knob),
                (MIDI_FB_NOTE, Toggle),
                (FB_AMOUNT, Knob),
                (FB_INVERT, Toggle),
                (FB_GATE, Toggle),
                (FB_FREQUENCY, Knob),
                (FB_WIDTH, Knob),
            ]),
        ]);
        // A source's page: its display (R31), its controls and its amounts.
        let source_page = |name: &str, controls: LayoutNode, source: usize| {
            Page::new(
                name,
                LayoutNode::Row(vec![
                    LayoutNode::Cell(DisplayCell::new(name, 120.0, page_height, 1.0)),
                    controls,
                    amounts(source),
                ]),
            )
        };
        DeviceLayout::new(LayoutNode::Pages(PagesNode::new(
            PAGES,
            PageStyle::Tabs,
            vec![
                Page::new("Roar", device),
                source_page("LFO 1", lfo_band(0), 0),
                source_page("LFO 2", lfo_band(1), 1),
                source_page(
                    "Env",
                    band(&[
                        (ENV_ATTACK, Knob),
                        (ENV_RELEASE, Knob),
                        (ENV_HOLD, Toggle),
                        (ENV_THRESHOLD, Knob),
                        (ENV_GAIN, Knob),
                        (ENV_FREQUENCY, Knob),
                        (ENV_WIDTH, Knob),
                        (INPUT_LISTEN, Toggle),
                    ]),
                    2,
                ),
                source_page(
                    "Noise",
                    band(&[(NOISE_TYPE, Chooser), (NOISE_RATE, Knob)]),
                    3,
                ),
            ],
        )))
    };
    let shaper_drag: Vec<ParamId> = (0..STAGES).map(|s| stage_param(s, AMOUNT)).collect();
    let stage_ids = |stage: usize, offsets: &[u32]| -> Vec<ParamId> {
        offsets.iter().map(|o| stage_param(stage, *o)).collect()
    };
    let mut banks = vec![
        (
            "Main".to_owned(),
            vec![
                DRIVE,
                TONE_AMOUNT,
                ROUTING_MODE,
                BLEND,
                stage_param(0, AMOUNT),
                stage_param(0, FREQUENCY),
                OUTPUT_GAIN,
                DRY_WET,
            ],
        ),
        (
            "Input".to_owned(),
            vec![DRIVE, TONE_AMOUNT, TONE_FREQUENCY, COLOR_COMPENSATION],
        ),
    ];
    for stage in 0..STAGES {
        let n = stage + 1;
        banks.push((
            format!("Stage {n}"),
            stage_ids(stage, &[ON, SHAPER, AMOUNT, BIAS, LEVEL]),
        ));
        banks.push((
            format!("Stage {n} Filter"),
            stage_ids(
                stage,
                &[FILTER, FREQUENCY, RESONANCE, PEAK_GAIN, MORPH, PRE],
            ),
        ));
    }
    banks.push((
        "Global".to_owned(),
        vec![
            ROUTING_MODE,
            BLEND,
            LOW_CROSSOVER,
            HIGH_CROSSOVER,
            OUTPUT_GAIN,
            DRY_WET,
        ],
    ));
    banks.extend(part_two_banks());
    let mut description = DeviceDescription::new(params)
        .with_layout(layout)
        .with_views(&VIEWS)
        .with_display_drag(&shaper_drag);
    description.banks = banks
        .into_iter()
        .map(|(name, params)| Bank { name, params })
        .collect();
    DeviceType::native(ROAR, "Roar", DeviceRole::AudioEffect)
        .with_description(description)
        .with_sidechain()
        .with_midi_sidechain_tap()
}

/// The names of the divisions `NOTES[..count]` with `suffix`: "1/8", "1/8T".
fn division_names(count: usize, suffix: &str) -> Vec<String> {
    NOTES[..count]
        .iter()
        .map(|(name, _)| format!("{name}{suffix}"))
        .collect()
}

/// Part 2's parameters (WP-8.39), in descriptor order from 43: the
/// feedback section, the compressor, Global Modulation Amount, the LFOs,
/// Env, the noise source and the matrix's amounts. `existing` names the
/// matrix's targets.
fn part_two_params(existing: &[ParamDescriptor]) -> Vec<ParamDescriptor> {
    let percent = |id, name: &str, default| {
        ParamDescriptor::continuous(id, name, (0.0, 100.0), default, ParamUnit::Percent)
    };
    let choice = |id, name: &str, names: &[String], default| {
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        ParamDescriptor::choice(id, name, &refs, default)
    };
    let hertz = |id, name: &str, (min, max): (f64, f64), default| {
        ParamDescriptor::continuous(id, name, (min, max), default, ParamUnit::Hertz).logarithmic()
    };
    let millis = |id, name: &str, (min, max): (f64, f64), default| {
        ParamDescriptor::continuous(id, name, (min, max), default, ParamUnit::Milliseconds)
            .logarithmic()
    };
    let octaves = |id, name: &str, default| {
        ParamDescriptor::continuous(id, name, (0.1, 4.0), default, ParamUnit::None)
    };
    let strings = |names: &[&str]| names.iter().map(|n| (*n).to_owned()).collect::<Vec<_>>();
    let mut params = vec![
        choice(FB_MODE, "Feedback Mode", &strings(&FEEDBACK_MODES), 0)
            .describe("What sets the feedback path's delay: a time, a division of the beat or a note's pitch."),
        millis(FB_TIME, "Feedback Time", (1.0, 2_000.0), 100.0)
            .describe("Time mode: the feedback path's delay."),
        choice(
            FB_SYNCED,
            "Feedback Synced",
            &division_names(FEEDBACK_NOTES, ""),
            3,
        )
        .describe("Synced mode: the feedback path's delay, a division of the beat."),
        choice(
            FB_TRIPLET,
            "Feedback Triplet",
            &division_names(FEEDBACK_NOTES, "T"),
            3,
        )
        .describe("Triplet mode: the feedback path's delay, a triplet division."),
        choice(
            FB_DOTTED,
            "Feedback Dotted",
            &division_names(FEEDBACK_NOTES, "."),
            3,
        )
        .describe("Dotted mode: the feedback path's delay, a dotted division."),
        ParamDescriptor::integer(
            FB_NOTE,
            "Feedback Note",
            (0, 127),
            69,
            ParamUnit::None,
        )
        .describe("Note mode: the key whose pitch the feedback path rings at, from C-2 (0) to G8 (127)."),
        percent(FB_AMOUNT, "Feedback Amount", 50.0)
            .describe("How much of the signal returns round the feedback path."),
        ParamDescriptor::toggle(FB_INVERT, "Feedback Invert", false)
            .describe("Flips the sign of what returns."),
        ParamDescriptor::toggle(FB_GATE, "Feedback Gate", false)
            .describe("Fades the feedback out once the input has stopped."),
        hertz(FB_FREQUENCY, "Feedback Frequency", (20.0, 20_000.0), 1_000.0)
            .describe("The centre of the band-pass the feedback path passes."),
        octaves(FB_WIDTH, "Feedback Width", 4.0)
            .describe("The width of the feedback band-pass, in octaves."),
        percent(COMPRESSION, "Compression Amount", 0.0)
            .describe("A compressor on the output, and so on the feedback: more lowers its threshold and raises its ratio."),
        ParamDescriptor::toggle(SIDECHAIN_HP, "Sidechain HP", false)
            .describe("The compressor's detector hears a high-pass, so lows don't squash the rest."),
        ParamDescriptor::toggle(MIDI_FB_NOTE, "MIDI > FB Note", false)
            .describe("Notes from the MIDI sidechain's source set Feedback Note."),
        percent(GLOBAL_MODULATION, "Global Modulation Amount", 100.0)
            .describe("Scales every amount in the modulation matrix."),
    ];
    for lfo in 0..2 {
        let n = lfo + 1;
        let id = |offset| lfo_param(lfo, offset);
        let name = |what: &str| format!("LFO {n} {what}");
        params.extend([
            choice(id(LFO_WAVE), &name("Wave"), &strings(&LFO_WAVES), 0)
                .describe("The LFO's waveform."),
            choice(id(LFO_MODE), &name("Mode"), &strings(&LFO_MODES), 0).describe(
                "Whether the LFO runs freely or locks to the beat, and in what division.",
            ),
            hertz(id(LFO_RATE), &name("Rate"), (0.01, 40.0), 1.0)
                .describe("Free mode: the LFO's rate."),
            choice(
                id(LFO_SYNCED),
                &name("Synced"),
                &division_names(NOTES.len(), ""),
                4,
            )
            .describe("Synced mode: the length of a cycle, a division of the beat."),
            choice(
                id(LFO_TRIPLET),
                &name("Triplet"),
                &division_names(NOTES.len(), "T"),
                4,
            )
            .describe("Triplet mode: the length of a cycle, a triplet division."),
            choice(
                id(LFO_DOTTED),
                &name("Dotted"),
                &division_names(NOTES.len(), "."),
                4,
            )
            .describe("Dotted mode: the length of a cycle, a dotted division."),
            ParamDescriptor::integer(
                id(LFO_SIXTEENTHS),
                &name("Sixteenths"),
                (1, 16),
                4,
                ParamUnit::None,
            )
            .describe("Sixteenth mode: the length of a cycle in sixteenth notes."),
            percent(id(LFO_MORPH), &name("Morph"), 0.0)
                .describe("Squeezes the wave toward the front of its cycle."),
            percent(id(LFO_SMOOTH), &name("Smooth"), 0.0)
                .describe("Rounds the wave off, up to a quarter of a cycle."),
        ]);
    }
    params.extend([
        millis(ENV_ATTACK, "Env Attack", (0.1, 100.0), 5.0)
            .describe("How fast Env rises to a louder input."),
        millis(ENV_RELEASE, "Env Release", (1.0, 2_000.0), 100.0)
            .describe("How fast Env falls to a quieter input."),
        ParamDescriptor::toggle(ENV_HOLD, "Env Hold", false)
            .describe("Lets a rise finish before Env releases."),
        ParamDescriptor::continuous(
            ENV_THRESHOLD,
            "Env Threshold",
            (-60.0, 0.0),
            -40.0,
            ParamUnit::Decibels,
        )
        .describe("Env gives 0 below this level and rises with the level above it."),
        ParamDescriptor::continuous(
            ENV_GAIN,
            "Env Gain",
            (-24.0, 24.0),
            0.0,
            ParamUnit::Decibels,
        )
        .describe("Raises or lowers what Env's detector hears."),
        hertz(ENV_FREQUENCY, "Env Frequency", (20.0, 20_000.0), 1_000.0)
            .describe("The centre of the band Env's detector hears."),
        octaves(ENV_WIDTH, "Env Width", 4.0)
            .describe("The width of the band Env's detector hears, in octaves."),
        ParamDescriptor::toggle(INPUT_LISTEN, "Input Listen", false)
            .describe("Sends what Env's detector hears to the output."),
        choice(NOISE_TYPE, "Noise Type", &strings(&NOISE_TYPES), 0)
            .describe("The noise source's kind of randomness."),
        hertz(NOISE_RATE, "Noise Rate", (0.01, 40.0), 1.0).describe("How fast the noise moves."),
    ]);
    for (source, source_name) in SOURCES.iter().enumerate() {
        for (target, id) in TARGET_IDS.iter().enumerate() {
            let name = existing
                .iter()
                .find(|p| p.id == *id)
                .map_or("", |p| p.name.as_str());
            params.push(
                ParamDescriptor::continuous(
                    matrix_param(source, target),
                    &format!("{source_name} > {name}"),
                    (-100.0, 100.0),
                    0.0,
                    ParamUnit::Percent,
                )
                .describe("How far the source moves this control, as a share of its range."),
            );
        }
    }
    params
}

/// Part 2's banks of eight (R34): the feedback section and the compressor,
/// a bank or two for each source, and its column of the matrix.
fn part_two_banks() -> Vec<(String, Vec<ParamId>)> {
    let mut banks = vec![
        (
            "Feedback".to_owned(),
            vec![
                FB_MODE, FB_TIME, FB_SYNCED, FB_TRIPLET, FB_DOTTED, FB_NOTE, FB_AMOUNT, FB_INVERT,
            ],
        ),
        (
            "Feedback Filter".to_owned(),
            vec![
                FB_GATE,
                FB_FREQUENCY,
                FB_WIDTH,
                MIDI_FB_NOTE,
                COMPRESSION,
                SIDECHAIN_HP,
                GLOBAL_MODULATION,
            ],
        ),
    ];
    for lfo in 0..2 {
        let id = |offset| lfo_param(lfo, offset);
        banks.push((
            format!("LFO {}", lfo + 1),
            vec![
                id(LFO_WAVE),
                id(LFO_MODE),
                id(LFO_RATE),
                id(LFO_SYNCED),
                id(LFO_TRIPLET),
                id(LFO_DOTTED),
                id(LFO_SIXTEENTHS),
                id(LFO_MORPH),
            ],
        ));
        banks.push((format!("LFO {} Smooth", lfo + 1), vec![id(LFO_SMOOTH)]));
    }
    banks.push((
        "Env".to_owned(),
        vec![
            ENV_ATTACK,
            ENV_RELEASE,
            ENV_HOLD,
            ENV_THRESHOLD,
            ENV_GAIN,
            ENV_FREQUENCY,
            ENV_WIDTH,
            INPUT_LISTEN,
        ],
    ));
    banks.push(("Noise".to_owned(), vec![NOISE_TYPE, NOISE_RATE]));
    for (source, name) in SOURCES.iter().enumerate() {
        for (part, chunk) in (0..TARGETS).collect::<Vec<_>>().chunks(8).enumerate() {
            banks.push((
                format!("{name} Matrix {}", part + 1),
                chunk.iter().map(|t| matrix_param(source, *t)).collect(),
            ));
        }
    }
    banks
}

/// Adds Roar to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, roar_type(), make);
}

/// Makes a Roar for `device`'s values and `config`'s rate, channels and block
/// size. Every buffer is made here, off the audio thread.
#[must_use]
pub fn make(device: &Device, config: &NodeConfig) -> Roar {
    Roar::new(device, config)
}

/// A first-order section `k (s + wz) / (s + wp)` by the bilinear transform,
/// its zero and pole prewarped to `zero_hz` and `pole_hz`. Swapping the two
/// and inverting `k` gives its exact inverse.
#[derive(Clone, Copy, Default)]
struct FirstOrder {
    b0: f64,
    b1: f64,
    a1: f64,
    x1: f64,
    y1: f64,
}

impl FirstOrder {
    fn set(&mut self, zero_hz: f64, pole_hz: f64, k: f64, rate: f64) {
        let warp = |hz: f64| (PI * hz.clamp(1.0, rate * 0.49) / rate).tan();
        let (wz, wp) = (warp(zero_hz), warp(pole_hz));
        self.b0 = k * (1.0 + wz) / (1.0 + wp);
        self.b1 = k * (wz - 1.0) / (1.0 + wp);
        self.a1 = (wp - 1.0) / (1.0 + wp);
    }

    fn process(&mut self, x: f64) -> f64 {
        let y = self.b0 * x + self.b1 * self.x1 - self.a1 * self.y1;
        self.x1 = x;
        self.y1 = flush(y);
        y
    }
}

/// The Tone tilt: a low shelf and a high shelf, first order, at the same
/// corner, that cut the lows and lift the highs by `db` at the ends of the
/// band (and the reverse for a negative `db`).
#[derive(Clone, Copy, Default)]
struct Tilt {
    low: FirstOrder,
    high: FirstOrder,
}

impl Tilt {
    fn set(&mut self, db: f64, hz: f64, rate: f64) {
        let gain = db_to_gain(db);
        let (zero, pole) = (hz / gain.sqrt(), hz * gain.sqrt());
        self.low.set(zero, pole, 1.0, rate);
        self.high.set(zero, pole, gain, rate);
    }

    fn process(&mut self, x: f64) -> f64 {
        self.high.process(self.low.process(x))
    }
}

/// One stage's filter, with the states of the types Roar adds to the shared
/// ones.
struct Filter {
    chan: Chan,
    section: Stage,
    dispersion: [Stage; SECTIONS],
}

impl Filter {
    fn new(rate: f64) -> Self {
        Self {
            // The shared comb's rings are sized for twice the rate it is
            // given, and `rate` is the high one.
            chan: Chan::new(rate / 2.0),
            section: Stage::default(),
            dispersion: [Stage::default(); SECTIONS],
        }
    }

    fn reset(&mut self) {
        self.chan.reset();
        self.section = Stage::default();
        self.dispersion = [Stage::default(); SECTIONS];
    }

    /// One sample through filter `kind` as stage plan `p` sets it, at `rate`.
    fn tick(&mut self, kind: usize, x: f64, p: &StagePlan, rate: f64) -> f64 {
        let g = (PI * p.hz.clamp(1.0, rate * 0.49) / rate).tan();
        match kind {
            PEAK => {
                let c = Coef::new(g, q_of(p.res));
                let [_, band, _] = self.section.tick(x, &c, Circuit::Svf);
                x + (p.peak - 1.0) * c.k * band
            }
            DISPERSION => {
                let c = Coef::new(g, 0.5 + 7.5 * p.res);
                let mut y = x;
                for section in &mut self.dispersion {
                    let [_, band, _] = section.tick(y, &c, Circuit::Svf);
                    y -= 2.0 * c.k * band;
                }
                y
            }
            _ => {
                let plan = Plan::new(
                    SHARED[kind].unwrap_or(0),
                    1,
                    0,
                    Knobs {
                        hz: p.hz,
                        res: p.res,
                        morph: if kind == MORPH_FILTER { p.morph } else { 0.0 },
                        control: 0.0,
                        pitch: 0.0,
                        formant: 0.0,
                    },
                );
                self.chan.tick(x, &plan, rate)
            }
        }
    }
}

/// A stage's settings at one frame.
#[derive(Clone, Copy)]
struct StagePlan {
    on: bool,
    pre: bool,
    shaper: usize,
    filter: usize,
    amount: f64,
    bias: f64,
    level: f64,
    hz: f64,
    res: f64,
    morph: f64,
    /// Peak Gain as a factor.
    peak: f64,
}

/// What the routing reads at one frame.
#[derive(Clone, Copy)]
struct Frame {
    stages: [StagePlan; STAGES],
    low: f64,
    high: f64,
    /// Blend's equal-power gains for the first and the second signal.
    blend: (f64, f64),
    /// The compressor's law, or `None` at Compression Amount 0, and whether
    /// its detector hears the high-pass.
    law: Option<Law>,
    high_pass: bool,
    /// The feedback path: its length in samples at the high rate, what
    /// returns (signed), whether the gate acts, and its band-pass.
    len: f64,
    returns: f64,
    gate: bool,
    fb_hz: f64,
    fb_width: f64,
}

/// The parameters' values frame by frame: the stretch's ramps, with each
/// target of the matrix read from its modulated copy (R23), and the feedback
/// path's length worked out for the stretch.
struct Ramps<'a> {
    params: &'a ParamBank,
    modded: &'a [Vec<f32>],
    /// For each parameter, its place among the matrix's targets, or
    /// [`NO_TARGET`].
    slot: &'a [u8],
    len: &'a [f64],
}

const NO_TARGET: u8 = u8::MAX;

impl Ramps<'_> {
    fn at(&self, id: ParamId, i: usize) -> f64 {
        match self.slot[id.0 as usize] {
            NO_TARGET => f64::from(self.params.ramp(id.0 as usize)[i]),
            target => f64::from(self.modded[usize::from(target)][i]),
        }
    }

    fn frames(&self) -> usize {
        self.params.ramp(0).len()
    }
}

/// The settings at frame `i` of the stretch, from the parameters' ramps.
fn frame_at(ramps: &Ramps<'_>, i: usize) -> Frame {
    let at = |id: ParamId| ramps.at(id, i);
    let stages = core::array::from_fn(|s| {
        let at = |offset| at(stage_param(s, offset));
        StagePlan {
            on: at(ON) >= 0.5,
            pre: at(PRE) >= 0.5,
            shaper: (at(SHAPER) as usize).min(SHAPERS.len() - 1),
            filter: (at(FILTER) as usize).min(FILTERS.len() - 1),
            amount: at(AMOUNT) / 100.0,
            bias: at(BIAS) / 100.0,
            level: at(LEVEL),
            hz: at(FREQUENCY),
            res: at(RESONANCE) / 100.0,
            morph: at(MORPH) / 100.0,
            peak: db_to_gain(at(PEAK_GAIN)),
        }
    });
    let angle = at(BLEND) / 100.0 * FRAC_PI_2;
    let compression = at(COMPRESSION) / 100.0;
    let sign = if at(FB_INVERT) >= 0.5 { -1.0 } else { 1.0 };
    Frame {
        stages,
        low: at(LOW_CROSSOVER),
        high: at(HIGH_CROSSOVER),
        blend: (angle.cos(), angle.sin()),
        law: (compression > 0.0).then(|| Law::of(compression)),
        high_pass: at(SIDECHAIN_HP) >= 0.5,
        len: ramps.len[i],
        returns: sign * LOOP_CEILING * (at(FB_AMOUNT) / 100.0).clamp(0.0, 1.0),
        gate: at(FB_GATE) >= 0.5,
        fb_hz: at(FB_FREQUENCY),
        fb_width: at(FB_WIDTH),
    }
}

/// What a stretch's loop needs besides the frames: the high rate, the
/// crossfade's length in samples and the DC blocker's pole.
#[derive(Clone, Copy)]
struct Setup {
    rate: f64,
    fade: f64,
    pole: f64,
}

/// One stage on one channel: its filters, the types it is crossfading from,
/// the DC blocker and the noise.
struct Voice {
    random: Random,
    filters: [Filter; 2],
    /// The filter slot playing the filter type now.
    active: usize,
    filter: Option<usize>,
    filter_from: usize,
    filter_left: f64,
    shaper: usize,
    shaper_from: usize,
    shaper_left: f64,
    dc_in: f64,
    dc_out: f64,
    /// The largest level that has reached the shaper since the display read.
    peak: f32,
}

impl Voice {
    fn new(seed: u64, rate: f64) -> Self {
        Self {
            random: Random::seeded(seed),
            filters: [Filter::new(rate), Filter::new(rate)],
            active: 0,
            filter: None,
            filter_from: 0,
            filter_left: 0.0,
            shaper: 0,
            shaper_from: 0,
            shaper_left: 0.0,
            dc_in: 0.0,
            dc_out: 0.0,
            peak: 0.0,
        }
    }

    /// Notes the types the stretch plays: a change starts a crossfade from
    /// the type before.
    fn sync(&mut self, plan: &StagePlan, fade: f64) {
        if self.filter.is_none() {
            self.shaper = plan.shaper;
            self.filter = Some(plan.filter);
            return;
        }
        if plan.shaper != self.shaper {
            self.shaper_from = self.shaper;
            self.shaper = plan.shaper;
            self.shaper_left = fade;
        }
        if self.filter != Some(plan.filter) {
            self.filter_from = self.filter.unwrap_or(0);
            self.filter = Some(plan.filter);
            self.active ^= 1;
            self.filters[self.active].reset();
            self.filter_left = fade;
        }
    }

    fn filter_step(&mut self, x: f64, p: &StagePlan, setup: &Setup) -> f64 {
        let kind = self.filter.unwrap_or(p.filter);
        let now = self.filters[self.active].tick(kind, x, p, setup.rate);
        if self.filter_left <= 0.0 {
            return now;
        }
        let from = self.filters[self.active ^ 1].tick(self.filter_from, x, p, setup.rate);
        let t = 1.0 - self.filter_left / setup.fade;
        self.filter_left -= 1.0;
        from + (now - from) * t
    }

    fn shape_step(&mut self, x: f64, p: &StagePlan, setup: &Setup) -> f64 {
        let noisy = self.shaper == NOISE_INJECTION
            || (self.shaper_left > 0.0 && self.shaper_from == NOISE_INJECTION);
        let noise = if noisy {
            self.random.next_bipolar()
        } else {
            0.0
        };
        let now = shape(self.shaper, p.amount, p.bias, noise, x);
        if self.shaper_left <= 0.0 {
            return now;
        }
        let from = shape(self.shaper_from, p.amount, p.bias, noise, x);
        let t = 1.0 - self.shaper_left / setup.fade;
        self.shaper_left -= 1.0;
        from + (now - from) * t
    }

    /// One sample through the stage; a stage that is off passes it as it is.
    fn run(&mut self, x: f64, p: &StagePlan, setup: &Setup) -> f64 {
        if !p.on {
            return x;
        }
        let mut y = x;
        if p.pre {
            y = self.filter_step(y, p, setup);
        }
        self.peak = self.peak.max(y.abs() as f32);
        y = self.shape_step(y, p, setup);
        if !p.pre {
            y = self.filter_step(y, p, setup);
        }
        let out = y - self.dc_in + setup.pole * self.dc_out;
        self.dc_in = y;
        self.dc_out = flush(out);
        out * p.level
    }
}

/// The loop's output `len` samples back (at least 2 and 3 short of the
/// line's length), read between four samples with a Catmull-Rom spline, so
/// a high note's pitch is held to a cent.
fn tap(line: &DelayLine, len: f64) -> f64 {
    let whole = len.floor();
    let t = len - whole;
    let n = whole as usize;
    let at = |back: usize| f64::from(line.read(back));
    let (before, from, to, after) = (at(n - 1), at(n), at(n + 1), at(n + 2));
    from + 0.5
        * t
        * (to - before
            + t * (2.0 * before - 5.0 * from + 4.0 * to - after
                + t * (3.0 * (from - to) + after - before)))
}

/// One channel's stages and routing state, at the high rate.
struct Net {
    voices: [Voice; STAGES],
    /// Multi Band's crossovers: the lower on the input, the upper on what is
    /// above it, and the upper on what is below it, which brings the low band
    /// to the other two's phase.
    splits: [Split; 3],
    line: DelayLine,
    /// The feedback path's band-pass and gate, and the compressor.
    band: Biquad,
    gate: Gate,
    squash: Squash,
    /// Samples run so far, which the crossovers' refresh counts.
    ticks: u64,
}

impl Net {
    fn new(seed: u64, rate: f64, loop_cap: usize) -> Self {
        Self {
            voices: core::array::from_fn(|s| Voice::new(seed.wrapping_add(s as u64), rate)),
            splits: [Split::default(); 3],
            line: DelayLine::new(loop_cap),
            band: Biquad::default(),
            gate: Gate::default(),
            squash: Squash::new(rate),
            ticks: 0,
        }
    }

    /// Runs `block`, channel `channel`'s samples at the high rate, from frame
    /// `*frame` of the stretch, in routing mode `mode`.
    fn run(
        &mut self,
        channel: usize,
        mode: usize,
        block: &mut [f32],
        frame: &mut usize,
        ramps: &Ramps<'_>,
        setup: &Setup,
    ) {
        let first = *frame;
        let frames = ramps.frames();
        let mut at = frame_at(ramps, first.min(frames - 1));
        for (voice, plan) in self.voices.iter_mut().zip(&at.stages) {
            voice.sync(plan, setup.fade);
        }
        let looped = mode == FEEDBACK || mode == DELAY;
        for (j, sample) in block.iter_mut().enumerate() {
            if j % FACTOR == 0 {
                at = frame_at(ramps, (first + j / FACTOR).min(frames - 1));
            }
            if self.ticks.is_multiple_of(CROSS_REFRESH) {
                self.splits[0].set(at.low, setup.rate);
                self.splits[1].set(at.high, setup.rate);
                self.splits[2].set(at.high, setup.rate);
                if looped {
                    self.band.band_pass(at.fb_hz, at.fb_width, setup.rate);
                }
            }
            self.ticks += 1;
            let x = if sample.is_finite() {
                f64::from(*sample)
            } else {
                0.0
            };
            let [p0, p1, p2] = &at.stages;
            let (first_gain, second_gain) = at.blend;
            let mix = |a: f64, b: f64| first_gain * a + second_gain * b;
            let [v0, v1, v2] = &mut self.voices;
            let gate = if looped {
                self.gate.process(x, at.gate, setup.rate)
            } else {
                1.0
            };
            // The output before the compressor, and for Delay the signal
            // from stage 1 and what stage 2 heard, which it feeds back.
            let (mut y, a, heard) = match mode {
                SERIAL => {
                    let a = v0.run(x, p0, setup);
                    (mix(a, v1.run(a, p1, setup)), 0.0, 0.0)
                }
                PARALLEL => {
                    let a = v0.run(x, p0, setup);
                    (mix(a, v1.run(x, p1, setup)), 0.0, 0.0)
                }
                MULTI_BAND => {
                    let (below, above) = self.splits[0].process(x);
                    let (mid, high) = self.splits[1].process(above);
                    let (aligned_low, aligned_high) = self.splits[2].process(below);
                    let y = v0.run(aligned_low + aligned_high, p0, setup)
                        + v1.run(mid, p1, setup)
                        + v2.run(high, p2, setup);
                    (y, 0.0, 0.0)
                }
                MID_SIDE if channel == 1 => (v1.run(x, p1, setup), 0.0, 0.0),
                FEEDBACK => {
                    let a = v0.run(x, p0, setup);
                    let back = tap(&self.line, at.len) * at.returns * gate;
                    (mix(a, v1.run(back, p1, setup)), 0.0, 0.0)
                }
                DELAY => {
                    let a = v0.run(x, p0, setup);
                    let heard = v1.run(tap(&self.line, at.len), p1, setup);
                    (mix(a, heard), a, heard)
                }
                _ => (v0.run(x, p0, setup), 0.0, 0.0),
            };
            let squeeze = at
                .law
                .map_or(1.0, |law| self.squash.process(y, law, at.high_pass));
            y *= squeeze;
            // Spec: roar R24. The loop's gain stays below 1, the compressor
            // acts in it, and the written value is held to a safe size.
            match mode {
                FEEDBACK => self
                    .line
                    .write(self.band.process(y).clamp(-LOOP_LIMIT, LOOP_LIMIT) as f32),
                DELAY => {
                    let fed = a + self.band.process(heard * at.returns * gate * squeeze);
                    self.line.write(fed.clamp(-LOOP_LIMIT, LOOP_LIMIT) as f32);
                }
                _ => {}
            }
            *sample = y as f32;
        }
        *frame += block.len() / FACTOR;
    }
}

/// The MIDI sidechain's key for Note mode (R25): the note held last, the
/// newest still held taking over when it ends, and the channel's pitch
/// bend. A note-off with nothing else held leaves the key.
#[derive(Clone, Copy)]
struct MidiKey {
    held: [u8; HELD_KEYS],
    len: usize,
    key: Option<u8>,
    bend: f64,
}

impl MidiKey {
    fn new() -> Self {
        Self {
            held: [0; HELD_KEYS],
            len: 0,
            key: None,
            bend: 0.0,
        }
    }

    fn take(&mut self, note: &NoteEvent) {
        match note.kind {
            NoteKind::On => {
                if self.len == HELD_KEYS {
                    self.held.copy_within(1.., 0);
                    self.len -= 1;
                }
                self.held[self.len] = note.key;
                self.len += 1;
                self.key = Some(note.key);
            }
            NoteKind::Off | NoteKind::Choke => {
                let Some(at) = self.held[..self.len].iter().rposition(|&k| k == note.key) else {
                    return;
                };
                self.held.copy_within(at + 1..self.len, at);
                self.len -= 1;
                if at == self.len && self.len > 0 {
                    self.key = Some(self.held[self.len - 1]);
                }
            }
            NoteKind::PitchBend { value } => self.bend = f64::from(value).clamp(-1.0, 1.0),
            _ => {}
        }
    }
}

/// The delay in samples a stage's DC blocker (a zero at 1 and a pole at
/// `pole`) gives a tone at `w` radians per sample: negative, it leads.
fn blocker_delay(w: f64, pole: f64) -> f64 {
    let angle = |re: f64, im: f64| im.atan2(re);
    let phase = angle(1.0 - w.cos(), w.sin()) - angle(1.0 - pole * w.cos(), pole * w.sin());
    -phase / w
}

/// A target's place in [`TARGET_IDS`].
const fn target_of(id: ParamId) -> usize {
    let mut at = 0;
    while at < TARGETS {
        if TARGET_IDS[at].0 == id.0 {
            return at;
        }
        at += 1;
    }
    panic!("not a target of the matrix");
}

/// Roar's audio-thread half.
pub struct Roar {
    rate: f64,
    nets: [Net; 2],
    pre: [Tilt; 2],
    post: [Tilt; 2],
    oversamplers: [Oversampler; 2],
    /// The input as it came, for the dry signal, and the signal on its way
    /// through the stages.
    dry: [Vec<f32>; 2],
    work: [Vec<f32>; 2],
    delay: [DelayLine; 2],
    latency: u32,
    /// Frames processed so far, which the tilts' refresh and the grid count.
    ticks: u64,
    /// The sources: LFO 1 and 2, Env and Noise, each worked out on the grid
    /// and glided between its points.
    lfos: [LfoSource; 2],
    env: EnvSource,
    noise: Noise,
    glides: [Glide; 4],
    /// Each source's value at each frame of the stretch.
    sources: [Vec<f32>; 4],
    /// Each target's value at each frame once the matrix has moved it, and
    /// for each parameter its place among the targets.
    modded: Vec<Vec<f32>>,
    slot: Vec<u8>,
    /// Each target's range.
    bounds: [(f64, f64); TARGETS],
    /// The feedback path's length at each frame, in samples at the high
    /// rate, and what a synced division comes to on the grid, in seconds.
    loop_len: Vec<f64>,
    synced: Glide,
    /// The feedback band-pass, for the delay it adds to the loop, and its
    /// centre (radians per sample) and Q at the last grid point.
    probe: Biquad,
    centre: f64,
    q: f64,
    midi: MidiKey,
    events: Vec<NoteEvent>,
    /// What Env's detector heard, for Input Listen.
    listen: [Vec<f32>; 2],
    /// Each source's last two seconds, for its display, and how often a
    /// point is taken.
    trace: [[f32; TRACE_POINTS]; 4],
    trace_at: usize,
    trace_every: u64,
    /// The gain the compressor took off the last block, in dB.
    reduction: f32,
}

impl Roar {
    fn new(device: &Device, config: &NodeConfig) -> Self {
        let frames = config.max_frames.max(1);
        let rate = config.sample_rate.as_f64();
        let high_rate = rate * FACTOR as f64;
        let oversampler = || Oversampler::new(OVERSAMPLE_STAGES, 2, frames);
        let oversamplers = [oversampler(), oversampler()];
        let latency = oversamplers[0].latency();
        let loop_cap = (LOOP_MAX_SECONDS * high_rate) as usize;
        let seed = device.id.0.get().wrapping_mul(8);
        let buffer = || vec![0.0f32; frames];
        let description = roar_type().description;
        let mut slot = vec![NO_TARGET; description.params.len()];
        let mut bounds = [(0.0, 0.0); TARGETS];
        for (target, id) in TARGET_IDS.iter().enumerate() {
            slot[id.0 as usize] = target as u8;
            let param = description.param(*id).expect("the description holds it");
            bounds[target] = (param.min(), param.max());
        }
        Self {
            rate,
            nets: [
                Net::new(seed, high_rate, loop_cap),
                Net::new(seed.wrapping_add(4), high_rate, loop_cap),
            ],
            pre: [Tilt::default(); 2],
            post: [Tilt::default(); 2],
            oversamplers,
            dry: [buffer(), buffer()],
            work: [buffer(), buffer()],
            delay: [
                DelayLine::new(latency as usize),
                DelayLine::new(latency as usize),
            ],
            latency,
            ticks: 0,
            lfos: [
                LfoSource::new(config.sample_rate, seed.wrapping_add(100)),
                LfoSource::new(config.sample_rate, seed.wrapping_add(101)),
            ],
            env: EnvSource::new(rate),
            noise: Noise::new(config.sample_rate, seed.wrapping_add(102)),
            glides: [Glide::default(); 4],
            sources: core::array::from_fn(|_| buffer()),
            modded: (0..TARGETS).map(|_| buffer()).collect(),
            slot,
            bounds,
            loop_len: vec![FACTOR as f64; frames],
            synced: Glide::default(),
            probe: Biquad::default(),
            centre: 1.0,
            q: 1.0,
            midi: MidiKey::new(),
            events: Vec::with_capacity(EVENT_ROOM),
            listen: [buffer(), buffer()],
            trace: [[0.0; TRACE_POINTS]; 4],
            trace_at: 0,
            trace_every: ((TRACE_SECONDS * rate) / TRACE_POINTS as f64)
                .round()
                .max(1.0) as u64,
            reduction: 0.0,
        }
    }

    /// Works out the sources over the stretch (R21, R22): Env hears every
    /// frame, and each source takes a value at each grid point, which it
    /// reaches over the next 32 frames.
    fn work_out_sources(
        &mut self,
        audio: &Audio<'_, '_>,
        params: &ParamBank,
        frames: usize,
        channels: usize,
    ) {
        let index = |id: ParamId| id.0 as usize;
        let value = |id: ParamId| params.value(index(id));
        let target = |id: ParamId| params.target(index(id));
        let tempo = audio.tempo();
        let (playing, bpm) = (audio.playing(), audio.bpm());
        let rates: [Rate; 2] = core::array::from_fn(|lfo| {
            let id = |offset| lfo_param(lfo, offset);
            let division = |offset| NOTES[(value(id(offset)) as usize).min(NOTES.len() - 1)].1;
            match value(id(LFO_MODE)) as usize {
                0 => Rate::Hertz(target(id(LFO_RATE))),
                1 => Rate::Beats(division(LFO_SYNCED)),
                2 => Rate::Beats(division(LFO_TRIPLET) * 2.0 / 3.0),
                3 => Rate::Beats(division(LFO_DOTTED) * 1.5),
                _ => Rate::Beats(0.25 * target(id(LFO_SIXTEENTHS)).round().max(1.0)),
            }
        });
        for (lfo, rate) in self.lfos.iter_mut().zip(rates) {
            lfo.begin(audio, rate);
        }
        self.env.set(
            (
                target(ENV_ATTACK) / 1_000.0,
                target(ENV_RELEASE) / 1_000.0,
                value(ENV_HOLD) >= 0.5,
            ),
            (db_to_gain(target(ENV_GAIN)), target(ENV_THRESHOLD)),
            (target(ENV_FREQUENCY), target(ENV_WIDTH)),
            self.rate,
        );
        let (noise_kind, noise_hz) = (value(NOISE_TYPE) as usize, target(NOISE_RATE));
        let listen = value(INPUT_LISTEN) >= 0.5;
        for i in 0..frames {
            let tick = self.ticks + i as u64;
            let phase = tick % GRID;
            // What Env's detector hears: the sidechain with its gain and mix
            // while External is on, else the input (R29).
            let heard = |channel: usize| {
                let channel = channel.min(channels - 1);
                audio
                    .sidechain(channel)
                    .map_or_else(|| audio.channel(channel)[i], |key| key[i])
            };
            let ears = self.env.feed(heard(0), heard(1));
            if listen {
                self.listen[0][i] = ears[0];
                self.listen[1][i] = ears[1];
            }
            if phase == 0 {
                for (lfo, source) in self.lfos.iter_mut().enumerate() {
                    let id = |offset| lfo_param(lfo, offset);
                    let settings = LfoSettings {
                        wave: value(id(LFO_WAVE)) as usize,
                        rate: rates[lfo],
                        morph: f64::from(params.ramp(index(id(LFO_MORPH)))[i]) / 100.0,
                        smooth: f64::from(params.ramp(index(id(LFO_SMOOTH)))[i]) / 100.0,
                    };
                    self.glides[lfo].push(source.tick(tempo, i, playing, bpm, settings));
                }
                self.glides[2].push(self.env.value());
                self.glides[3].push(self.noise.tick(noise_kind, noise_hz));
            }
            for (source, glide) in self.sources.iter_mut().zip(&self.glides) {
                source[i] = glide.at(phase) as f32;
            }
            if tick.is_multiple_of(self.trace_every) {
                for (trace, source) in self.trace.iter_mut().zip(&self.sources) {
                    trace[self.trace_at] = source[i];
                }
                self.trace_at = (self.trace_at + 1) % TRACE_POINTS;
            }
        }
    }

    /// Moves each target by the sources (R23): its value plus, for each
    /// source, amount × source × Global Modulation Amount × the target's
    /// range, held to the range. A target no amount reaches is its
    /// parameter's own.
    fn apply_matrix(&mut self, params: &ParamBank, frames: usize) {
        let ramp = |id: ParamId| params.ramp(id.0 as usize);
        let global = ramp(GLOBAL_MODULATION);
        for (target, id) in TARGET_IDS.iter().enumerate() {
            let out = &mut self.modded[target][..frames];
            out.copy_from_slice(ramp(*id));
            let (min, max) = self.bounds[target];
            let mut moved = false;
            for (source, values) in self.sources.iter().enumerate() {
                let amount = ramp(matrix_param(source, target));
                if amount.iter().all(|a| *a == 0.0) {
                    continue;
                }
                moved = true;
                for i in 0..frames {
                    let by = f64::from(amount[i]) / 100.0
                        * f64::from(values[i])
                        * (f64::from(global[i]) / 100.0)
                        * (max - min);
                    out[i] += by as f32;
                }
            }
            if moved {
                for value in out.iter_mut() {
                    *value = value.clamp(min as f32, max as f32);
                }
            }
        }
    }

    /// Works out the feedback path's length at each frame (R24, R25), in
    /// samples at the high rate, and takes the MIDI sidechain's notes on
    /// their frames. In Feedback mode the loop's length is its time less the
    /// delay the band-pass adds: its group delay at its centre, or in Note
    /// mode its phase delay at the pitch, so the path rings at the note.
    fn work_out_loop(
        &mut self,
        audio: &Audio<'_, '_>,
        params: &ParamBank,
        frames: usize,
        routing: usize,
    ) {
        let index = |id: ParamId| id.0 as usize;
        let value = |id: ParamId| params.value(index(id));
        let high_rate = self.rate * FACTOR as f64;
        let looped = routing == FEEDBACK || routing == DELAY;
        // Delay hears stage 1's signal once at its time, before the
        // band-pass: only Feedback's single loop makes up for it.
        let make_up = routing == FEEDBACK;
        let mode = value(FB_MODE) as usize;
        let note_mode = mode == FEEDBACK_MODES.len() - 1;
        let division =
            |id: ParamId, scale: f64| NOTES[(value(id) as usize).min(FEEDBACK_NOTES - 1)].1 * scale;
        let beats = match mode {
            SYNCED_MODE => division(FB_SYNCED, 1.0),
            TRIPLET_MODE => division(FB_TRIPLET, 2.0 / 3.0),
            DOTTED_MODE => division(FB_DOTTED, 1.5),
            _ => 0.0,
        };
        let midi_on = value(MIDI_FB_NOTE) >= 0.5;
        let stage_two_on = value(stage_param(1, ON)) >= 0.5;
        let dc = 1.0 - core::f64::consts::TAU * DC_HZ / high_rate;
        let time = &self.modded[target_of(FB_TIME)];
        let (hz, width) = (
            &self.modded[target_of(FB_FREQUENCY)],
            &self.modded[target_of(FB_WIDTH)],
        );
        let key = params.ramp(index(FB_NOTE));
        let longest = LOOP_MAX_SECONDS * high_rate - 3.0;
        let mut next = 0;
        for i in 0..frames {
            while let Some(note) = self.events.get(next).filter(|n| n.offset as usize <= i) {
                self.midi.take(note);
                next += 1;
            }
            if !looped {
                continue;
            }
            let phase = (self.ticks + i as u64) % GRID;
            if phase == 0 {
                if beats > 0.0 {
                    self.synced.push(beats * 60.0 / audio.bpm_at(i));
                }
                (self.centre, self.q) =
                    self.probe
                        .band_pass(f64::from(hz[i]), f64::from(width[i]), high_rate);
            }
            let (seconds, mut delay) = if note_mode {
                let semitones = match (midi_on, self.midi.key) {
                    (true, Some(held)) => f64::from(held),
                    _ => f64::from(key[i]),
                } + BEND_RANGE * self.midi.bend;
                let pitch = 440.0 * ((semitones - REFERENCE_KEY) / 12.0).exp2();
                let w = core::f64::consts::TAU * pitch / high_rate;
                // Stage 2's DC blocker leads the phase a little at a low
                // pitch; it is in the loop while the stage is on.
                let blocker = if stage_two_on {
                    blocker_delay(w, dc)
                } else {
                    0.0
                };
                (1.0 / pitch, self.probe.phase_delay(w) + blocker)
            } else if beats > 0.0 {
                (self.synced.at(phase), 2.0 * self.q / self.centre)
            } else {
                (f64::from(time[i]) / 1_000.0, 2.0 * self.q / self.centre)
            };
            if !make_up {
                delay = 0.0;
            }
            self.loop_len[i] = (seconds * high_rate - delay).clamp(FACTOR as f64, longest);
        }
    }
}

impl NativeDevice for Roar {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        let frames = audio.frames();
        let channels = audio.channels().min(2);
        if frames == 0 || channels == 0 {
            return;
        }
        let ramp = |id: ParamId| params.ramp(id.0 as usize);
        let value = |id: ParamId| params.value(id.0 as usize);
        let mode = (value(ROUTING_MODE) as usize).min(MODES.len() - 1);
        let compensate = value(COLOR_COMPENSATION) >= 0.5;
        let listen = value(INPUT_LISTEN) >= 0.5;
        let mid_side = mode == MID_SIDE && channels == 2;
        let high_rate = self.rate * FACTOR as f64;
        let setup = Setup {
            rate: high_rate,
            fade: (FADE_SECONDS * high_rate).round(),
            pole: 1.0 - core::f64::consts::TAU * DC_HZ / high_rate,
        };

        // 0. The MIDI sidechain's notes, the sources, the matrix and the
        // feedback path's length (R21 to R25).
        self.events.clear();
        for note in audio.notes() {
            if self.events.len() < EVENT_ROOM {
                self.events.push(note);
            }
        }
        self.work_out_sources(audio, params, frames, channels);
        self.apply_matrix(params, frames);
        self.work_out_loop(audio, params, frames, mode);
        let ramps = Ramps {
            params,
            modded: &self.modded,
            slot: &self.slot,
            len: &self.loop_len,
        };
        let moved = |id: ParamId| &self.modded[target_of(id)][..frames];
        let (drive, tone, tone_hz) = (moved(DRIVE), moved(TONE_AMOUNT), moved(TONE_FREQUENCY));

        // 1. Drive and the Tone tilt, and the mid and side.
        for channel in 0..channels {
            let input = audio.channel(channel);
            self.dry[channel][..frames].copy_from_slice(input);
            let tilt = &mut self.pre[channel];
            let work = &mut self.work[channel][..frames];
            for i in 0..frames {
                if (self.ticks + i as u64).is_multiple_of(TONE_REFRESH) {
                    let db = f64::from(tone[i]) / 100.0 * TONE_DB;
                    tilt.set(db, f64::from(tone_hz[i]), self.rate);
                }
                let x = f64::from(input[i]);
                let x = if x.is_finite() { x } else { 0.0 };
                work[i] = tilt.process(x * db_to_gain(f64::from(drive[i]))) as f32;
            }
        }
        if mid_side {
            let [left, right] = &mut self.work;
            for (l, r) in left[..frames].iter_mut().zip(&mut right[..frames]) {
                (*l, *r) = (0.5 * (*l + *r), 0.5 * (*l - *r));
            }
        }

        // 2. The stages, at four times the rate.
        for channel in 0..channels {
            let net = &mut self.nets[channel];
            let mut frame = 0;
            self.oversamplers[channel].process(
                channel,
                &mut self.work[channel][..frames],
                |high| {
                    net.run(channel, mode, high, &mut frame, &ramps, &setup);
                },
            );
        }
        if mid_side {
            let [mid, side] = &mut self.work;
            for (m, s) in mid[..frames].iter_mut().zip(&mut side[..frames]) {
                (*m, *s) = (*m + *s, *m - *s);
            }
        }

        // 3. The tilt's inverse, Output Gain, the hard clip and the mix.
        let (output, mix) = (ramp(OUTPUT_GAIN), ramp(DRY_WET));
        for channel in 0..channels {
            let tilt = &mut self.post[channel];
            let work = &self.work[channel][..frames];
            let dry = &self.dry[channel][..frames];
            let delay = &mut self.delay[channel];
            let out = audio.channel_mut(channel);
            for i in 0..frames {
                if (self.ticks + i as u64).is_multiple_of(TONE_REFRESH) {
                    let db = if compensate {
                        -f64::from(tone[i]) / 100.0 * TONE_DB
                    } else {
                        0.0
                    };
                    tilt.set(db, f64::from(tone_hz[i]), self.rate);
                }
                let wet = tilt.process(f64::from(work[i])) as f32;
                let wet = (wet * output[i]).clamp(-1.0, 1.0);
                let dry_now = delay.read(self.latency as usize);
                delay.write(dry[i]);
                out[i] = dry_now + (wet - dry_now) * (mix[i] / 100.0);
            }
            if listen {
                out.copy_from_slice(&self.listen[channel][..frames]);
            }
        }
        self.ticks += frames as u64;
        self.reduction = self
            .nets
            .iter_mut()
            .map(|net| net.squash.take_reduction() as f32)
            .fold(0.0, f32::min);
    }

    fn latency(&self) -> u32 {
        self.latency
    }

    fn gain_reduction(&self) -> f32 {
        self.reduction
    }

    fn display_len(&self) -> usize {
        STAGES + SOURCES.len() * TRACE_POINTS
    }

    fn display(&mut self, out: &mut [f32]) {
        for (stage, slot) in out[..STAGES].iter_mut().enumerate() {
            *slot = self
                .nets
                .iter_mut()
                .map(|net| core::mem::take(&mut net.voices[stage].peak))
                .fold(0.0, f32::max);
        }
        // Each source's trace, the oldest point first.
        for (source, trace) in self.trace.iter().enumerate() {
            let to = &mut out[STAGES + source * TRACE_POINTS..][..TRACE_POINTS];
            for (point, slot) in to.iter_mut().enumerate() {
                *slot = trace[(self.trace_at + point) % TRACE_POINTS];
            }
        }
    }
}

/// A level in dB, with a floor.
fn level_db(level: f32) -> f32 {
    if level > 1e-6 {
        20.0 * level.log10()
    } else {
        LEVEL_FLOOR_DB
    }
}

/// The device's display (the roar spec, R12) for the view `view` (a stage):
/// the stage's curve over inputs from -1 to 1 with its bias, and, once an
/// engine publishes, the level that reached the stage as a reading and a
/// marker on the curve. `data` is what the node published.
#[must_use]
pub fn display(
    device: DeviceId,
    description: &DeviceDescription,
    values: &OrdMap<ParamId, f64>,
    data: Option<&[f32]>,
    view: Option<&str>,
) -> DeviceDisplay {
    let stage = view
        .and_then(|view| VIEWS.iter().position(|name| *name == view))
        .unwrap_or(0);
    let value = |offset| {
        description
            .value(values, stage_param(stage, offset))
            .unwrap_or(0.0)
    };
    let (kind, amount, bias) = (
        (value(SHAPER) as usize).min(SHAPERS.len() - 1),
        value(AMOUNT) / 100.0,
        value(BIAS) / 100.0,
    );
    let at = |x: f64| shape(kind, amount, bias, 0.0, x) as f32;
    let series = DisplaySeries {
        name: "Shaper Curve".to_owned(),
        unit: String::new(),
        x_unit: String::new(),
        x_range: [-1.0, 1.0],
        log_x: false,
        y_range: [-CURVE_PEAK, CURVE_PEAK],
        detail: false,
        values: (0..CURVE_POINTS)
            .map(|i| at(-1.0 + 2.0 * i as f64 / (CURVE_POINTS - 1) as f64))
            .collect(),
    };
    let mut out = DeviceDisplay {
        device,
        live: data.is_some(),
        view: None,
        series: vec![series],
        markers: Vec::new(),
        readings: Vec::new(),
    };
    // Spec: roar R31. Each source's last two seconds, once an engine
    // publishes.
    if let Some(data) = data.filter(|d| d.len() >= STAGES + SOURCES.len() * TRACE_POINTS) {
        for (source, name) in SOURCES.iter().enumerate() {
            out.series.push(DisplaySeries {
                name: (*name).to_owned(),
                unit: String::new(),
                x_unit: "s".to_owned(),
                x_range: [-TRACE_SECONDS as f32, 0.0],
                log_x: false,
                // Env runs from 0 to 1, the others from -1 to 1.
                y_range: if source == 2 { [0.0, 1.0] } else { [-1.0, 1.0] },
                detail: false,
                values: data[STAGES + source * TRACE_POINTS..][..TRACE_POINTS].to_vec(),
            });
        }
    }
    if let Some(peak) = data.and_then(|d| d.get(stage)).copied() {
        let input = level_db(peak);
        out.readings.push(DisplayReading {
            name: "Input".to_owned(),
            value: input,
            unit: "dB".to_owned(),
            text: format!("{input:.1} dB"),
        });
        if input > LEVEL_FLOOR_DB {
            let x = peak.min(1.0);
            out.markers.push(DisplayMarker {
                name: "Level".to_owned(),
                series: "Shaper Curve".to_owned(),
                x,
                y: at(f64::from(x)),
                text: format!("Level {input:.1} dB"),
                handle: None,
            });
        }
    }
    out
}

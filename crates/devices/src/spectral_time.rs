//! Spectral Time (WP-8.47; the spectral-time spec): a spectral freezer and
//! a spectral delay that work on the input's spectrum frame by frame, in
//! either order, with a spectrogram of what they make.
//!
//! One engine per Resolution is made with the node, each with an
//! [`Stft`] per channel, its freezer layers and its per-bin delay lines;
//! nothing is made on the audio thread. A change of Resolution resets the
//! engine it moves to, runs both while the new one's first output arrives,
//! and crossfades from the old one over 50 ms.

use bl_core::time::{SampleRate, TICKS_PER_QUARTER};
use bl_dsp::Random;
use bl_dsp::stft::{self, Complex32, Stft, Window, wrap_phase};
use bl_engine::{DeviceType, NodeConfig, Registry};
use bl_model::device_layout::{BODY_HEIGHT, ControlBand, DisplayCell, LayoutNode, Widget};
use bl_model::{
    Device, DeviceDescription, DeviceDisplay, DeviceId, DeviceLayout, DeviceRole,
    DisplaySpectrogram, ParamDescriptor, ParamId, ParamUnit, SpectrogramGrid,
};

use crate::beat_repeat::next_on_grid;
use crate::framework::{Audio, NativeDevice, ParamBank, add};

/// Spectral Time's device type.
pub const SPECTRAL_TIME: &str = "beatlayer.spectral_time";

/// Runs the freezer.
pub const FREEZER_ON: ParamId = ParamId(0);
/// Holds the frozen spectrum (Manual) or arms the retriggering (Retrigger).
pub const FREEZE: ParamId = ParamId(1);
/// Manual or Retrigger.
pub const FREEZER_MODE: ParamId = ParamId(2);
/// What retriggers: Onsets or Sync.
pub const TRIGGER: ParamId = ParamId(3);
/// How easily an onset is found.
pub const SENSITIVITY: ParamId = ParamId(4);
/// Whether the interval is a Time or Notes.
pub const INTERVAL_MODE: ParamId = ParamId(5);
/// The interval in milliseconds.
pub const INTERVAL_TIME: ParamId = ParamId(6);
/// The interval as a division of the song's grid, one of [`DIVISIONS`].
pub const INTERVAL_DIVISION: ParamId = ParamId(7);
/// Crossfade or Envelope.
pub const FADE_SHAPE: ParamId = ParamId(8);
/// How long a crossfade lasts, in per cent of the interval.
pub const X_FADE: ParamId = ParamId(9);
/// How long a freeze takes to fade in, in milliseconds.
pub const FADE_IN: ParamId = ParamId(10);
/// How long a freeze takes to fade out, in milliseconds.
pub const FADE_OUT: ParamId = ParamId(11);
/// Runs the delay.
pub const DELAY_ON: ParamId = ParamId(12);
/// How the delay's time is given: one of [`DELAY_MODES`].
pub const DELAY_MODE: ParamId = ParamId(13);
/// The delay time in milliseconds (Time).
pub const TIME: ParamId = ParamId(14);
/// The delay time as a division (Notes).
pub const NOTES: ParamId = ParamId(15);
/// The delay time in sixteenths (16th).
pub const SIXTEENTHS: ParamId = ParamId(16);
/// The delay time in sixteenth triplets (16th Triplet).
pub const TRIPLETS: ParamId = ParamId(17);
/// The delay time in dotted sixteenths (16th Dotted).
pub const DOTTED: ParamId = ParamId(18);
/// How much more (positive) or less the highs are delayed than the lows.
pub const TILT: ParamId = ParamId(19);
/// How far each bin's delay is scattered.
pub const SPRAY: ParamId = ParamId(20);
/// The part of the spectrum that is delayed: the highs (positive) or the
/// lows (negative).
pub const MASK: ParamId = ParamId(21);
/// How much of the delayed spectrum returns to the delay's input.
pub const FEEDBACK: ParamId = ParamId(22);
/// How far each repeat's bins move, in semitones.
pub const SHIFT: ParamId = ParamId(23);
/// How far the right channel's delays differ from the left's.
pub const STEREO: ParamId = ParamId(24);
/// The delay's own mix.
pub const DELAY_DRY_WET: ParamId = ParamId(25);
/// Which section runs first: Frz > Dly or Dly > Frz.
pub const ORDER: ParamId = ParamId(26);
/// The analysis' frame size, one of [`SIZES`].
pub const RESOLUTION: ParamId = ParamId(27);
/// The gain of what goes into the spectral processing.
pub const INPUT_SEND: ParamId = ParamId(28);
/// The device's mix.
pub const DRY_WET: ParamId = ParamId(29);
/// Passes the dry signal undelayed and reports no latency (a menu option).
pub const ZERO_DRY_LATENCY: ParamId = ParamId(30);

/// How many parameters it has.
const PARAMS: usize = 31;

/// The frame sizes Resolution chooses between at 48 kHz, scaled with the
/// sample rate to the nearest power of two.
pub const SIZES: [usize; 5] = [256, 512, 1_024, 2_048, 4_096];
/// The Resolution entry that is the default.
const DEFAULT_SIZE: usize = 3;
/// Frames begin every `size / OVERLAP` frames.
const OVERLAP: usize = 4;
/// The longest delay the per-bin delay lines hold, in seconds.
pub const CAP_SECONDS: f64 = 8.0;
/// How long a change of Resolution crossfades, in seconds.
const CROSSFADE_SECONDS: f64 = 0.05;
/// The freezes stacked at most.
pub const LAYERS: usize = 8;
/// The seconds the spectrogram shows.
const SHOWN_SECONDS: f64 = 4.0;

/// The spectrogram's grid: the last four seconds in 128 columns, 64 rows
/// from 20 Hz to 20 kHz, drawn between −90 and 0 dB.
pub const GRID: SpectrogramGrid = SpectrogramGrid {
    columns: 128,
    rows: 64,
    frequencies: [20.0, 20_000.0],
    log_frequency: true,
    levels: [-90.0, 0.0],
};

/// The Interval and Notes choosers' entries, in ticks.
pub const DIVISIONS: [i64; 9] = [
    TICKS_PER_QUARTER / 16,
    TICKS_PER_QUARTER / 8,
    TICKS_PER_QUARTER / 4,
    TICKS_PER_QUARTER / 2,
    TICKS_PER_QUARTER,
    TICKS_PER_QUARTER * 2,
    TICKS_PER_QUARTER * 4,
    TICKS_PER_QUARTER * 8,
    TICKS_PER_QUARTER * 16,
];
const DIVISION_NAMES: [&str; 9] = [
    "1/64", "1/32", "1/16", "1/8", "1/4", "1/2", "1 Bar", "2 Bars", "4 Bars",
];
/// The delay's modes.
pub const DELAY_MODES: [&str; 5] = ["Time", "Notes", "16th", "16th Triplet", "16th Dotted"];

/// The feedback loop's gain never reaches 1.
const MAX_FEEDBACK: f32 = 0.995;
/// The spectral flux an onset must exceed whatever the sound before it,
/// in amplitude, and how many frames apart two onsets are at least.
const FLUX_FLOOR: f32 = 0.01;
const ONSET_GAP_FRAMES: u64 = OVERLAP as u64;

/// Spectral Time's type.
#[must_use]
pub fn spectral_time_type() -> DeviceType {
    let division =
        |id, name: &str, default| ParamDescriptor::choice(id, name, &DIVISION_NAMES, default);
    let params =
        vec![
        ParamDescriptor::toggle(FREEZER_ON, "Freezer On", true).describe("Runs the freezer."),
        ParamDescriptor::toggle(FREEZE, "Freeze", false).describe(
            "Manual: captures the spectrum and holds it. Retrigger: lets the freezer capture.",
        ),
        ParamDescriptor::choice(FREEZER_MODE, "Mode", &["Manual", "Retrigger"], 0)
            .describe("Whether Freeze captures by hand or the freezer retriggers."),
        ParamDescriptor::choice(TRIGGER, "Trigger", &["Onsets", "Sync"], 0)
            .describe("Retrigger captures at each onset in the input or at each interval."),
        ParamDescriptor::continuous(
            SENSITIVITY,
            "Sensitivity",
            (0.0, 100.0),
            50.0,
            ParamUnit::Percent,
        )
        .describe("How easily an onset is found."),
        ParamDescriptor::choice(INTERVAL_MODE, "Interval Mode", &["Time", "Notes"], 1)
            .describe("Whether the interval is a time or a division of the song's grid."),
        ParamDescriptor::continuous(
            INTERVAL_TIME,
            "Interval Time",
            (10.0, 5_000.0),
            250.0,
            ParamUnit::Milliseconds,
        )
        .logarithmic()
        .describe("The interval, in milliseconds."),
        division(INTERVAL_DIVISION, "Interval Division", 4)
            .describe("The interval, as a division of the song's grid."),
        ParamDescriptor::choice(FADE_SHAPE, "Fade Shape", &["Crossfade", "Envelope"], 0)
            .describe("Each freeze crossfades from the last, or fades in and out on its own."),
        ParamDescriptor::continuous(X_FADE, "X-Fade", (0.0, 100.0), 20.0, ParamUnit::Percent)
            .describe("How long a crossfade lasts, in per cent of the interval."),
        ParamDescriptor::continuous(
            FADE_IN,
            "Fade In",
            (0.0, 5_000.0),
            50.0,
            ParamUnit::Milliseconds,
        )
        .describe("How long a freeze takes to fade in."),
        ParamDescriptor::continuous(
            FADE_OUT,
            "Fade Out",
            (0.0, 5_000.0),
            500.0,
            ParamUnit::Milliseconds,
        )
        .describe("How long a freeze takes to fade out."),
        ParamDescriptor::toggle(DELAY_ON, "Delay On", true).describe("Runs the delay."),
        ParamDescriptor::choice(DELAY_MODE, "Delay Mode", &DELAY_MODES, 0)
            .describe("How the delay's time is given."),
        ParamDescriptor::continuous(TIME, "Time", (10.0, 5_000.0), 250.0, ParamUnit::Milliseconds)
            .logarithmic()
            .describe("The delay time in Time mode."),
        division(NOTES, "Notes", 4).describe("The delay time in Notes mode."),
        ParamDescriptor::integer(SIXTEENTHS, "16th", (1, 16), 4, ParamUnit::None)
            .describe("The delay time in sixteenth notes."),
        ParamDescriptor::integer(TRIPLETS, "16th Triplet", (1, 16), 4, ParamUnit::None)
            .describe("The delay time in sixteenth-note triplets."),
        ParamDescriptor::integer(DOTTED, "16th Dotted", (1, 16), 4, ParamUnit::None)
            .describe("The delay time in dotted sixteenth notes."),
        ParamDescriptor::continuous(TILT, "Tilt", (-100.0, 100.0), 0.0, ParamUnit::Percent)
            .describe("Positive delays the highs more than the lows, negative the reverse."),
        ParamDescriptor::continuous(SPRAY, "Spray", (0.0, 100.0), 0.0, ParamUnit::Percent)
            .describe("Scatters each frequency band's delay time at random."),
        ParamDescriptor::continuous(MASK, "Mask", (-100.0, 100.0), 0.0, ParamUnit::Percent)
            .describe("Delays only the highs (positive) or the lows (negative)."),
        ParamDescriptor::continuous(FEEDBACK, "Feedback", (0.0, 100.0), 50.0, ParamUnit::Percent)
            .describe("How much of each repeat returns to the delay's input."),
        ParamDescriptor::continuous(SHIFT, "Shift", (-24.0, 24.0), 0.0, ParamUnit::Semitones)
            .describe("Moves each repeat's pitch by this interval."),
        ParamDescriptor::continuous(STEREO, "Stereo", (0.0, 100.0), 0.0, ParamUnit::Percent)
            .describe("Delays the right channel up to half the delay time longer than the left."),
        ParamDescriptor::continuous(
            DELAY_DRY_WET,
            "Delay Dry/Wet",
            (0.0, 100.0),
            100.0,
            ParamUnit::Percent,
        )
        .describe("Balances the delay's input against its echoes."),
        ParamDescriptor::choice(ORDER, "Order", &["Frz > Dly", "Dly > Frz"], 0)
            .describe("Which section runs first."),
        ParamDescriptor::choice(
            RESOLUTION,
            "Resolution",
            &["256", "512", "1024", "2048", "4096"],
            DEFAULT_SIZE,
        )
        .describe("The analysis' frame size at 48 kHz: lower is less latency, less accuracy."),
        ParamDescriptor::gain(INPUT_SEND, "Input Send", 12.0, 0.0)
            .describe("The gain of what goes into the spectral processing, down to silence."),
        ParamDescriptor::continuous(DRY_WET, "Dry/Wet", (0.0, 100.0), 100.0, ParamUnit::Percent)
            .describe("Balances the dry signal against the processed one."),
        ParamDescriptor::toggle(ZERO_DRY_LATENCY, "Zero Dry Signal Latency", false).describe(
            "Passes the dry signal undelayed and reports no latency; the wet signal arrives late.",
        ),
    ];
    DeviceType::native(SPECTRAL_TIME, "Spectral Time", DeviceRole::AudioEffect).with_description(
        DeviceDescription::new(params)
            .with_layout(layout())
            .with_spectrogram(GRID)
            .with_menu_options(&[ZERO_DRY_LATENCY])
            .with_banks(&[
                (
                    "Main",
                    &[
                        "Freezer On",
                        "Freeze",
                        "Delay On",
                        "Time",
                        "Feedback",
                        "Tilt",
                        "Spray",
                        "Dry/Wet",
                    ],
                ),
                (
                    "Freezer",
                    &[
                        "Mode",
                        "Trigger",
                        "Sensitivity",
                        "Interval Mode",
                        "Interval Time",
                        "Interval Division",
                        "Fade Shape",
                        "X-Fade",
                    ],
                ),
                ("Fades", &["Fade In", "Fade Out"]),
                (
                    "Delay",
                    &[
                        "Delay Mode",
                        "Notes",
                        "16th",
                        "16th Triplet",
                        "16th Dotted",
                        "Mask",
                        "Shift",
                        "Stereo",
                    ],
                ),
                ("Delay Mix", &["Delay Dry/Wet"]),
                ("Global", &["Order", "Resolution", "Input Send", "Dry/Wet"]),
            ]),
    )
}

fn layout() -> DeviceLayout {
    use Widget::{Chooser, Knob, Number, Toggle};
    let band = |controls: &[(ParamId, Widget)]| {
        LayoutNode::Band(ControlBand::stacked(BODY_HEIGHT, controls))
    };
    DeviceLayout::new(LayoutNode::Row(vec![
        band(&[
            (FREEZER_ON, Toggle),
            (FREEZE, Toggle),
            (FREEZER_MODE, Chooser),
            (TRIGGER, Chooser),
            (SENSITIVITY, Knob),
            (INTERVAL_MODE, Chooser),
            (INTERVAL_TIME, Knob),
            (INTERVAL_DIVISION, Chooser),
            (FADE_SHAPE, Chooser),
            (X_FADE, Knob),
            (FADE_IN, Knob),
            (FADE_OUT, Knob),
        ]),
        LayoutNode::Cell(DisplayCell::new("Spectrogram", 240.0, BODY_HEIGHT, 1.0)),
        band(&[
            (DELAY_ON, Toggle),
            (DELAY_MODE, Chooser),
            (TIME, Knob),
            (NOTES, Chooser),
            (SIXTEENTHS, Number),
            (TRIPLETS, Number),
            (DOTTED, Number),
            (TILT, Knob),
            (SPRAY, Knob),
            (MASK, Knob),
            (FEEDBACK, Knob),
            (SHIFT, Knob),
            (STEREO, Knob),
            (DELAY_DRY_WET, Knob),
        ]),
        band(&[
            (ORDER, Chooser),
            (RESOLUTION, Chooser),
            (INPUT_SEND, Knob),
            (DRY_WET, Knob),
        ]),
    ]))
}

/// What Spectral Time's display shows, from the `data` its node published:
/// the spectrogram.
#[must_use]
pub fn display(
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

/// Adds Spectral Time to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, spectral_time_type(), SpectralTime::new);
}

/// The frame size of Resolution entry `entry` at `rate`.
#[must_use]
pub fn frame_size(entry: usize, rate: f64) -> usize {
    let scaled = SIZES[entry.min(SIZES.len() - 1)] as f64 * rate / 48_000.0;
    (scaled.log2().round().exp2() as usize).clamp(stft::MIN_SIZE, stft::MAX_SIZE)
}

/// Frames in `ticks` of the song when a quarter note lasts `60 / bpm` s.
fn ticks_frames(ticks: i64, bpm: f64, rate: f64) -> f64 {
    60.0 / bpm * rate * ticks as f64 / TICKS_PER_QUARTER as f64
}

/// The seconds of delay the settings give at `bpm`, before Tilt, Spray and
/// Stereo (the spec's R8), for `device.parameters` to say whether the
/// delay memory holds it (R12).
#[must_use]
pub fn delay_seconds(value: impl Fn(ParamId) -> f64, bpm: f64) -> f64 {
    delay_frames(&value, bpm, 1.0)
}

fn delay_frames(value: &impl Fn(ParamId) -> f64, bpm: f64, rate: f64) -> f64 {
    let sixteenth = TICKS_PER_QUARTER / 4;
    match value(DELAY_MODE) as usize {
        0 => value(TIME) * rate / 1_000.0,
        1 => ticks_frames(
            DIVISIONS[value(NOTES) as usize % DIVISIONS.len()],
            bpm,
            rate,
        ),
        2 => ticks_frames(sixteenth, bpm, rate) * value(SIXTEENTHS).round(),
        3 => ticks_frames(TICKS_PER_QUARTER / 6, bpm, rate) * value(TRIPLETS).round(),
        _ => ticks_frames(sixteenth * 3 / 2, bpm, rate) * value(DOTTED).round(),
    }
}

/// Whether the delay time the settings give, scaled by Tilt and Spray at
/// their most, is longer than the delay memory (R12): `value` reads a
/// parameter's value.
#[must_use]
pub fn delay_is_capped(value: impl Fn(ParamId) -> f64, bpm: f64) -> bool {
    let tilt = value(TILT).abs() / 100.0;
    let spray = value(SPRAY) / 100.0;
    let stereo = 1.0 + value(STEREO) / 200.0;
    delay_seconds(value, bpm) * (1.0 + tilt) * (1.0 + spray) * stereo > CAP_SECONDS
}

/// The parameter that sets the delay time now, if the settings give a time
/// the delay memory can't hold (R12), for `device.parameters` to say so.
#[must_use]
pub fn capped_time(value: impl Fn(ParamId) -> f64, bpm: f64) -> Option<ParamId> {
    let times = [TIME, NOTES, SIXTEENTHS, TRIPLETS, DOTTED];
    let mode = value(DELAY_MODE) as usize;
    delay_is_capped(&value, bpm).then(|| times[mode.min(times.len() - 1)])
}

/// A freeze's stage.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    Idle,
    Rise,
    Hold,
    Fall,
}

/// How a layer's gain runs between its ends: straight, or the equal-power
/// quarter sine.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Law {
    Linear,
    Equal,
}

/// A captured spectrum, held and faded.
struct Layer {
    stage: Stage,
    law: Law,
    /// The gain where the stage began, and how far through it the layer is
    /// (0 to 1).
    from: f32,
    t: f32,
    /// How much of its stage each frame takes.
    rise: f32,
    fall: f32,
    /// A layer that fades out as soon as it is in.
    then_fall: bool,
    /// The frame it was captured on, to find the oldest.
    born: u64,
    mag: Vec<f32>,
    phase: Vec<f64>,
    /// The turn each bin makes in a hop: its own frequency's.
    step: Vec<f64>,
}

impl Layer {
    fn new(bins: usize) -> Self {
        Self {
            stage: Stage::Idle,
            law: Law::Linear,
            from: 0.0,
            t: 0.0,
            rise: 1.0,
            fall: 1.0,
            then_fall: false,
            born: 0,
            mag: vec![0.0; bins],
            phase: vec![0.0; bins],
            step: vec![0.0; bins],
        }
    }

    fn live(&self) -> bool {
        self.stage != Stage::Idle
    }

    fn gain(&self) -> f32 {
        let along = |t: f32| match self.law {
            Law::Linear => t,
            Law::Equal => (t * std::f32::consts::FRAC_PI_2).sin(),
        };
        match self.stage {
            Stage::Idle => 0.0,
            Stage::Rise => self.from + (1.0 - self.from) * along(self.t),
            Stage::Hold => 1.0,
            Stage::Fall => self.from * along(1.0 - self.t),
        }
    }

    /// Takes `spectrum`'s magnitudes and phases, and each bin's turn per
    /// hop from its phase against `before`, the frame before's, and starts
    /// to fade in from the gain the layer has.
    fn capture(&mut self, spectrum: &[Complex32], before: &[f32], hop: f64, size: f64, born: u64) {
        self.from = self.gain();
        for (b, bin) in spectrum.iter().enumerate() {
            let phase = f64::from(bin.arg());
            let centre = std::f64::consts::TAU * b as f64 * hop / size;
            self.mag[b] = bin.norm();
            self.phase[b] = phase;
            self.step[b] = centre + wrap_phase(phase - f64::from(before[b]) - centre);
        }
        // The bins around a peak keep the phases they have against it only if
        // they turn together, so each takes the turn of its peak.
        let mut start = 0;
        while start < self.step.len() {
            let mut peak = start;
            while peak + 1 < self.mag.len() && self.mag[peak + 1] >= self.mag[peak] {
                peak += 1;
            }
            let mut valley = peak;
            while valley + 1 < self.mag.len() && self.mag[valley + 1] < self.mag[valley] {
                valley += 1;
            }
            let turn = self.step[peak];
            self.step[start..=valley].fill(turn);
            start = valley + 1;
        }
        self.stage = Stage::Rise;
        self.t = 0.0;
        self.born = born;
    }

    /// One frame on.
    fn advance(&mut self) {
        match self.stage {
            Stage::Rise => {
                self.t += self.rise;
                if self.t >= 1.0 {
                    if self.then_fall {
                        self.stage = Stage::Fall;
                        self.from = 1.0;
                        self.t = 0.0;
                    } else {
                        self.stage = Stage::Hold;
                    }
                }
            }
            Stage::Fall => {
                self.t += self.fall;
                if self.t >= 1.0 {
                    self.stage = Stage::Idle;
                }
            }
            Stage::Idle | Stage::Hold => {}
        }
    }

    /// Starts to fade out from the gain it has, taking `fall` of the fade a
    /// frame.
    fn release(&mut self, fall: f32) {
        if matches!(self.stage, Stage::Rise | Stage::Hold) {
            self.from = self.gain();
            self.stage = Stage::Fall;
            self.t = 0.0;
            self.fall = fall;
        }
    }
}

/// What the freezer does this frame.
struct Freezer {
    on: bool,
    /// Freeze is on.
    held: bool,
    retrigger: bool,
    envelope: bool,
    /// How much of a fade a frame takes: Fade In, Fade Out, and X-Fade.
    rise: f32,
    fall: f32,
    cross: f32,
}

/// What the delay does this frame.
struct Delay {
    on: bool,
    /// The delay in samples, before Tilt, Spray and Stereo.
    samples: f64,
    tilt: f64,
    spray: f64,
    mask: f64,
    feedback: f32,
    /// How far a repeat's bins move, as a ratio of frequencies.
    ratio: f64,
    stereo: f64,
    mix: f32,
}

/// Everything a frame reads from the parameters.
struct Settings {
    freezer: Freezer,
    delay: Delay,
    delay_first: bool,
    /// Whether the freezer captures on onsets now, and how sensitively.
    onsets: bool,
    sensitivity: f32,
}

/// One channel's state in an engine.
struct Chan {
    layers: Vec<Layer>,
    /// The delay lines: `cap` frames of `bins` bins, each a pair of floats.
    line: Vec<f32>,
    head: usize,
    filled: usize,
    /// Each bin's random draw for Spray, and what it was drawn for.
    draws: Vec<f32>,
    drawn: Option<(f64, f64)>,
    /// The last frame's phases of what the freezer heard, and magnitudes of
    /// the input, for the freezer's frequencies and the flux.
    before_phase: Vec<f32>,
    before_mag: Vec<f32>,
    read: Vec<Complex32>,
    sum: Vec<Complex32>,
}

/// What the engine's channels share from frame to frame.
struct Shared {
    random: Random,
    /// The frames begun since the engine began.
    frames: u64,
    /// The sample a capture is asked for, until it is made.
    pending: Option<u64>,
    /// The flux summed over the channels so far this frame, its running
    /// average, and whether the last frame was above the threshold.
    flux: f32,
    average: f32,
    above: bool,
    last_onset: u64,
}

impl Shared {
    fn new(seed: u64) -> Self {
        Self {
            random: Random::seeded(seed),
            frames: 0,
            pending: None,
            flux: 0.0,
            average: 0.0,
            above: false,
            last_onset: 0,
        }
    }
}

/// A Resolution's analysis, freezer and delay.
struct Engine {
    size: usize,
    hop: usize,
    bins: usize,
    /// How many frames the delay lines hold.
    cap: usize,
    stfts: Vec<Stft>,
    chans: Vec<Chan>,
    shared: Shared,
    /// Where each bin is on the 20 Hz to 20 kHz axis, 0 to 1 in log
    /// frequency.
    axis: Vec<f32>,
    /// The bins each spectrogram row covers.
    rows: Vec<(usize, usize)>,
    scale: f32,
    /// Samples taken since the engine began.
    clock: u64,
    seed: u64,
    /// Each bin's delay in frames, this frame.
    delays: Vec<u32>,
    rate: f64,
}

impl Engine {
    fn new(size: usize, channels: usize, rate: f64, seed: u64) -> Self {
        let hop = size / OVERLAP;
        let bins = size / 2 + 1;
        let cap = (CAP_SECONDS * rate / hop as f64).ceil() as usize + 1;
        let stfts: Vec<Stft> = (0..channels)
            .map(|_| Stft::new(size, OVERLAP, Window::Hann).expect("a frame size the STFT offers"))
            .collect();
        let chans = (0..channels)
            .map(|_| Chan {
                layers: (0..LAYERS).map(|_| Layer::new(bins)).collect(),
                // Zeroed pages the memory doesn't touch until a frame is
                // written to them.
                line: vec![0.0; cap * bins * 2],
                head: 0,
                filled: 0,
                draws: vec![0.0; bins],
                drawn: None,
                before_phase: vec![0.0; bins],
                before_mag: vec![0.0; bins],
                read: vec![Complex32::new(0.0, 0.0); bins],
                sum: vec![Complex32::new(0.0, 0.0); bins],
            })
            .collect();
        let axis = (0..bins)
            .map(|b| {
                let hz = stft::bin_to_hz(b as f32, size, rate).max(20.0);
                ((hz / 20.0).ln() / 1_000.0f32.ln()).clamp(0.0, 1.0)
            })
            .collect();
        Self {
            size,
            hop,
            bins,
            cap,
            scale: stfts[0].amplitude_scale(),
            stfts,
            chans,
            shared: Shared::new(seed),
            axis,
            rows: stft::row_bins(
                GRID.rows,
                GRID.frequencies[0],
                GRID.frequencies[1],
                GRID.log_frequency,
                size,
                rate,
            ),
            clock: 0,
            seed,
            delays: vec![1; bins],
            rate,
        }
    }

    /// The frames its wet output lags its input by.
    fn latency(&self) -> u32 {
        self.stfts[0].latency() as u32
    }

    /// Forgets everything, as at the start.
    fn reset(&mut self) {
        for stft in &mut self.stfts {
            stft.reset();
        }
        for chan in &mut self.chans {
            for layer in &mut chan.layers {
                layer.stage = Stage::Idle;
            }
            chan.head = 0;
            chan.filled = 0;
            chan.drawn = None;
            chan.before_phase.fill(0.0);
            chan.before_mag.fill(0.0);
        }
        self.shared = Shared::new(self.seed);
        self.clock = 0;
    }

    /// Whether the sample about to be taken ends a frame.
    fn due(&self) -> bool {
        (self.clock + 1).is_multiple_of(self.hop as u64)
    }

    /// Asks for a capture of the next whole frame, if none is asked for.
    fn trigger(&mut self) {
        self.shared.pending.get_or_insert(self.clock);
    }

    /// Takes one sample of channel `c` and gives its wet output.
    /// `settings` are for the frame the sample ends, if it ends one; `meter`
    /// takes each frame's wet magnitudes if this is the engine shown.
    fn process(
        &mut self,
        c: usize,
        sample: f32,
        settings: Option<&Settings>,
        meter: Option<&mut Meter>,
    ) -> f32 {
        let Self {
            stfts,
            chans,
            shared,
            size,
            hop,
            bins,
            cap,
            axis,
            rows,
            scale,
            clock,
            delays,
            rate,
            ..
        } = self;
        let frame = Frame {
            size: *size,
            hop: *hop,
            bins: *bins,
            cap: *cap,
            axis,
            end: *clock + 1,
            channel: c,
            channels: chans.len(),
            rate: *rate,
        };
        let chan = &mut chans[c];
        let mut meter = meter;
        stfts[c].process_sample(sample, |spectrum| {
            let settings = settings.expect("a frame has its settings");
            frame.run(chan, shared, spectrum, settings, delays);
            if let Some(meter) = meter.as_deref_mut() {
                meter.take(spectrum, rows, *scale);
            }
        })
    }
}

/// The parts of an engine one frame of one channel reads.
struct Frame<'a> {
    size: usize,
    hop: usize,
    bins: usize,
    cap: usize,
    axis: &'a [f32],
    /// The sample count at the frame's end.
    end: u64,
    channel: usize,
    channels: usize,
    rate: f64,
}

impl Frame<'_> {
    fn last(&self) -> bool {
        self.channel + 1 == self.channels
    }

    /// Whether the capture asked for is of this frame: the first whole one
    /// after the asking.
    fn captures(&self, shared: &Shared) -> bool {
        shared
            .pending
            .is_some_and(|at| self.end >= self.size as u64 + at)
    }

    fn run(
        &self,
        chan: &mut Chan,
        shared: &mut Shared,
        spectrum: &mut [Complex32],
        settings: &Settings,
        delays: &mut [u32],
    ) {
        self.flux(chan, shared, spectrum);
        let capture = self.captures(shared);
        let (freezer, delay) = (&settings.freezer, &settings.delay);
        if !freezer.on {
            for layer in &mut chan.layers {
                layer.stage = Stage::Idle;
            }
        }
        if !delay.on {
            chan.filled = 0;
        }
        for section in if settings.delay_first { [1, 0] } else { [0, 1] } {
            if section == 0 && freezer.on {
                self.freeze(chan, shared, spectrum, freezer, capture);
            } else if section == 1 && delay.on {
                self.delay(chan, shared, spectrum, delay, delays);
            }
        }
        if self.last() {
            if capture {
                shared.pending = None;
            }
            self.onset(shared, settings);
            shared.frames += 1;
        }
    }

    /// Adds this channel's spectral flux to the frame's: how much louder
    /// each bin is than a frame ago.
    fn flux(&self, chan: &mut Chan, shared: &mut Shared, spectrum: &[Complex32]) {
        let scale = 2.0 / (0.5 * self.size as f32);
        let mut sum = 0.0f32;
        for (before, bin) in chan.before_mag.iter_mut().zip(spectrum) {
            let now = bin.norm() * scale;
            sum += (now - *before).max(0.0);
            *before = now;
        }
        shared.flux += sum / self.channels as f32;
    }

    /// Decides after the last channel whether this frame starts an onset: its
    /// flux crosses a threshold that rises with the flux before it and falls
    /// with Sensitivity. Asks for a capture if it does.
    fn onset(&self, shared: &mut Shared, settings: &Settings) {
        let flux = std::mem::take(&mut shared.flux);
        let k = 1.5 + 6.0 * (1.0 - settings.sensitivity);
        let threshold = shared.average * k + FLUX_FLOOR;
        let above = flux > threshold;
        if above
            && !shared.above
            && settings.onsets
            && shared.frames >= shared.last_onset + ONSET_GAP_FRAMES
        {
            shared.last_onset = shared.frames;
            shared.pending.get_or_insert(self.end);
        }
        shared.above = above;
        shared.average = 0.9 * shared.average + 0.1 * flux;
    }

    /// The freezer's section over `spectrum`.
    fn freeze(
        &self,
        chan: &mut Chan,
        shared: &Shared,
        spectrum: &mut [Complex32],
        freezer: &Freezer,
        capture: bool,
    ) {
        let (hop, size) = (self.hop as f64, self.size as f64);
        if !freezer.held {
            for layer in &mut chan.layers {
                layer.release(freezer.fall);
            }
        } else if capture && !freezer.retrigger {
            let layer = &mut chan.layers[0];
            layer.law = Law::Linear;
            layer.then_fall = false;
            layer.rise = freezer.rise;
            layer.capture(spectrum, &chan.before_phase, hop, size, shared.frames);
        } else if capture {
            let slot = free_slot(&chan.layers);
            if freezer.envelope {
                let layer = &mut chan.layers[slot];
                layer.stage = Stage::Idle;
                layer.law = Law::Linear;
                layer.then_fall = true;
                layer.rise = freezer.rise;
                layer.fall = freezer.fall;
                layer.capture(spectrum, &chan.before_phase, hop, size, shared.frames);
            } else {
                // Each freeze crossfades from the last: the last fades out
                // as this fades in.
                for layer in &mut chan.layers {
                    layer.law = Law::Equal;
                    layer.release(freezer.cross);
                }
                let layer = &mut chan.layers[slot];
                layer.stage = Stage::Idle;
                layer.law = Law::Equal;
                layer.then_fall = false;
                layer.rise = freezer.cross;
                layer.capture(spectrum, &chan.before_phase, hop, size, shared.frames);
            }
        }
        let mut level = 0.0;
        chan.sum.fill(Complex32::new(0.0, 0.0));
        for layer in chan.layers.iter_mut().filter(|l| l.live()) {
            layer.advance();
            let gain = layer.gain();
            level += gain;
            for b in 0..self.bins {
                let (sin, cos) = layer.phase[b].sin_cos();
                let magnitude = layer.mag[b] * gain;
                chan.sum[b] += Complex32::new(magnitude * cos as f32, magnitude * sin as f32);
                layer.phase[b] = wrap_phase(layer.phase[b] + layer.step[b]);
            }
        }
        // The sound passes in the measure the freeze isn't in yet; a
        // retriggered freezer with Freeze on plays only what it has frozen.
        let live = if freezer.retrigger && freezer.held {
            0.0
        } else {
            (1.0f32 - level).clamp(0.0, 1.0)
        };
        for (b, bin) in spectrum.iter_mut().enumerate() {
            let heard = bin.arg();
            *bin = *bin * live + chan.sum[b];
            chan.before_phase[b] = heard;
        }
    }

    /// The delay's section over `spectrum`.
    fn delay(
        &self,
        chan: &mut Chan,
        shared: &mut Shared,
        spectrum: &mut [Complex32],
        delay: &Delay,
        delays: &mut [u32],
    ) {
        let (bins, cap) = (self.bins, self.cap);
        // The right channel's delays run longer by up to half again.
        let hops =
            delay.samples / self.hop as f64 * (1.0 + 0.5 * delay.stereo * self.channel as f64);
        if chan.drawn != Some((delay.samples, delay.spray)) {
            chan.drawn = Some((delay.samples, delay.spray));
            for draw in &mut chan.draws {
                *draw = shared.random.next_bipolar() as f32;
            }
        }
        let (lo, hi) = self.mask(delay.mask);
        for (b, d) in delays.iter_mut().enumerate() {
            let tilt = (1.0 - delay.tilt) + 2.0 * delay.tilt * f64::from(self.axis[b]);
            let scattered = 1.0 + delay.spray * f64::from(chan.draws[b]);
            *d = (hops * tilt * scattered).round().clamp(1.0, cap as f64) as u32;
        }
        let head = chan.head;
        for (b, (read, &d)) in chan.read.iter_mut().zip(delays.iter()).enumerate() {
            let d = d as usize;
            *read = if (lo..=hi).contains(&b) && d <= chan.filled {
                let at = ((head + cap - d) % cap * bins + b) * 2;
                Complex32::new(chan.line[at], chan.line[at + 1])
            } else {
                Complex32::new(0.0, 0.0)
            };
        }
        let n = shared.frames as i64;
        for (b, bin) in spectrum.iter_mut().enumerate() {
            let inside = (lo..=hi).contains(&b);
            let fed = if (delay.ratio - 1.0).abs() < 1e-9 {
                chan.read[b]
            } else {
                // Each bin takes the one at its pitch before the shift; a
                // quarter turn per frame and bin of distance moves its
                // frequency by the distance (the frames overlap four times).
                let s = (b as f64 / delay.ratio).round() as usize;
                if s >= bins {
                    Complex32::new(0.0, 0.0)
                } else {
                    let turns = ((b as i64 - s as i64) * n).rem_euclid(OVERLAP as i64);
                    let value = chan.read[s];
                    match turns {
                        0 => value,
                        1 => Complex32::new(-value.im, value.re),
                        2 => -value,
                        _ => Complex32::new(value.im, -value.re),
                    }
                }
            };
            let input = *bin;
            let wet = if inside { chan.read[b] } else { input };
            let kept = if inside {
                input + fed * delay.feedback
            } else {
                Complex32::new(0.0, 0.0)
            };
            let at = (head * bins + b) * 2;
            chan.line[at] = kept.re;
            chan.line[at + 1] = kept.im;
            *bin = input * (1.0 - delay.mix) + wet * delay.mix;
        }
        chan.head = (head + 1) % cap;
        chan.filled = (chan.filled + 1).min(cap);
    }

    /// The first and last bin Mask lets the delay act on.
    fn mask(&self, mask: f64) -> (usize, usize) {
        let last = self.bins - 1;
        let bin = |hz: f64| f64::from(stft::hz_to_bin(hz as f32, self.size, self.rate));
        if mask > 0.0 {
            let hz = 20.0 * 1_000.0f64.powf(mask);
            ((bin(hz).ceil() as usize).min(last + 1), last)
        } else if mask < 0.0 {
            let hz = 20_000.0 * 1_000.0f64.powf(mask);
            (0, bin(hz).floor() as usize)
        } else {
            (0, last)
        }
    }
}

/// An idle layer, or the oldest if all are in use: the oldest of a ninth
/// freeze's eight is dropped.
fn free_slot(layers: &[Layer]) -> usize {
    layers.iter().position(|l| !l.live()).unwrap_or_else(|| {
        layers
            .iter()
            .enumerate()
            .min_by_key(|(_, l)| l.born)
            .map_or(0, |(i, _)| i)
    })
}

/// The spectrogram's accumulation: the loudest each row got in the column
/// being made, and the columns.
struct Meter {
    acc: Vec<f32>,
    grid: Vec<f32>,
    head: usize,
    /// Samples in a column, and how many this one has.
    column: u64,
    taken: u64,
}

impl Meter {
    fn new(rate: f64) -> Self {
        Self {
            acc: vec![0.0; GRID.rows],
            grid: vec![GRID.levels[0]; GRID.columns * GRID.rows],
            head: 0,
            column: ((SHOWN_SECONDS * rate / GRID.columns as f64).round() as u64).max(1),
            taken: 0,
        }
    }

    fn take(&mut self, spectrum: &[Complex32], rows: &[(usize, usize)], scale: f32) {
        for (acc, &(from, to)) in self.acc.iter_mut().zip(rows) {
            let loudest = spectrum[from..=to]
                .iter()
                .map(|bin| bin.norm())
                .fold(0.0, f32::max);
            *acc = acc.max(loudest * scale);
        }
    }

    /// One sample on: a full column goes into the grid.
    fn tick(&mut self) {
        self.taken += 1;
        if self.taken < self.column {
            return;
        }
        self.taken = 0;
        let rows = GRID.rows;
        let column = &mut self.grid[self.head * rows..][..rows];
        for (cell, acc) in column.iter_mut().zip(&mut self.acc) {
            *cell = (20.0 * acc.max(1e-9).log10()).max(GRID.levels[0]);
            *acc = 0.0;
        }
        self.head = (self.head + 1) % GRID.columns;
    }
}

/// Where each parameter is in the bank.
struct Index([usize; PARAMS]);

impl Index {
    fn new(params: &ParamBank) -> Self {
        Self(core::array::from_fn(|id| {
            params
                .index(ParamId(id as u32))
                .expect("the description lists every parameter")
        }))
    }

    /// Parameter `id`'s value at frame `i` of the stretch.
    fn at(&self, params: &ParamBank, id: ParamId, i: usize) -> f64 {
        f64::from(params.ramp(self.0[id.0 as usize])[i])
    }
}

/// A change of Resolution in progress: the engine it leaves, the samples
/// until the one it goes to has its first output, and how far its crossfade
/// has gone.
struct Fade {
    from: usize,
    wait: u64,
    pos: u64,
}

/// Spectral Time.
struct SpectralTime {
    /// One engine per Resolution.
    engines: Vec<Engine>,
    target: usize,
    fade: Option<Fade>,
    rate: SampleRate,
    channels: usize,
    /// The dry signal's last samples, per channel, for the latency.
    dry: Vec<Vec<f32>>,
    dry_at: usize,
    zero_dry: bool,
    /// Whether Freeze was on a sample ago.
    held: bool,
    meter: Meter,
}

impl SpectralTime {
    fn new(device: &Device, config: &NodeConfig) -> Self {
        let rate = config.sample_rate.as_f64();
        let seed = device.id.0.get();
        let engines: Vec<Engine> = (0..SIZES.len())
            .map(|entry| Engine::new(frame_size(entry, rate), config.channels, rate, seed))
            .collect();
        let value = |id: ParamId, default: f64| device.params.get(&id).copied().unwrap_or(default);
        let longest = engines.iter().map(|e| e.size).max().unwrap_or(1);
        Self {
            target: (value(RESOLUTION, DEFAULT_SIZE as f64) as usize).min(SIZES.len() - 1),
            engines,
            fade: None,
            rate: config.sample_rate,
            channels: config.channels,
            dry: vec![vec![0.0; longest]; config.channels],
            dry_at: 0,
            zero_dry: value(ZERO_DRY_LATENCY, 0.0) >= 0.5,
            held: false,
            meter: Meter::new(rate),
        }
    }

    /// The reading of the wet and dry signal's crossfade: 0 while the
    /// engine left still plays alone.
    fn crossfade(&self) -> f32 {
        match &self.fade {
            Some(fade) if fade.wait == 0 => {
                fade.pos as f32 / (CROSSFADE_SECONDS * self.rate.as_f64()) as f32
            }
            Some(_) => 0.0,
            None => 1.0,
        }
    }

    /// Moves to Resolution entry `entry`: the engine it goes to starts
    /// afresh, the one it leaves keeps playing while the new one's first
    /// output arrives and fades out after.
    fn change(&mut self, entry: usize) {
        let from = self.target;
        self.target = entry;
        self.engines[entry].reset();
        self.fade = Some(Fade {
            from,
            wait: u64::from(self.engines[entry].latency()),
            pos: 0,
        });
    }

    /// Everything a frame of `engine` reads from the parameters at frame `i`.
    fn settings(
        &self,
        params: &ParamBank,
        ix: &Index,
        audio: &Audio<'_, '_>,
        i: usize,
        engine: &Engine,
    ) -> Settings {
        let v = |id| ix.at(params, id, i);
        let rate = self.rate.as_f64();
        let bpm = audio.bpm_at(i);
        let interval = if v(INTERVAL_MODE) < 0.5 {
            v(INTERVAL_TIME) * rate / 1_000.0
        } else {
            ticks_frames(
                DIVISIONS[v(INTERVAL_DIVISION) as usize % DIVISIONS.len()],
                bpm,
                rate,
            )
        };
        // How much of a fade a frame takes, whole at once for none.
        let per_frame = |samples: f64| {
            if samples < 1.0 {
                2.0
            } else {
                (engine.hop as f64 / samples) as f32
            }
        };
        let on = v(FREEZER_ON) >= 0.5;
        let held = v(FREEZE) >= 0.5;
        let retrigger = v(FREEZER_MODE) >= 0.5;
        Settings {
            freezer: Freezer {
                on,
                held,
                retrigger,
                envelope: v(FADE_SHAPE) >= 0.5,
                rise: per_frame(v(FADE_IN) * rate / 1_000.0),
                fall: per_frame(v(FADE_OUT) * rate / 1_000.0),
                cross: per_frame(v(X_FADE) / 100.0 * interval),
            },
            delay: Delay {
                on: v(DELAY_ON) >= 0.5,
                samples: delay_frames(&v, bpm, rate),
                tilt: v(TILT) / 100.0,
                spray: v(SPRAY) / 100.0,
                mask: v(MASK) / 100.0,
                feedback: ((v(FEEDBACK) / 100.0) as f32).min(MAX_FEEDBACK),
                ratio: (v(SHIFT) / 12.0).exp2(),
                stereo: v(STEREO) / 100.0,
                mix: (v(DELAY_DRY_WET) / 100.0) as f32,
            },
            delay_first: v(ORDER) >= 0.5,
            onsets: on && held && retrigger && v(TRIGGER) < 0.5,
            sensitivity: (v(SENSITIVITY) / 100.0) as f32,
        }
    }
}

impl NativeDevice for SpectralTime {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        let frames = audio.frames();
        let channels = self.channels.min(audio.channels());
        if frames == 0 || channels == 0 {
            return;
        }
        let ix = Index::new(params);
        let rate = self.rate.as_f64();
        let tempo = audio.tempo();
        // Sync captures at the points of the song's grid while it plays.
        let sync = {
            let v = |id| ix.at(params, id, 0);
            audio.playing()
                && v(FREEZER_ON) >= 0.5
                && v(FREEZE) >= 0.5
                && v(FREEZER_MODE) >= 0.5
                && v(TRIGGER) >= 0.5
        };
        let sample_rate = self.rate;
        let period = if ix.at(params, INTERVAL_MODE, 0) < 0.5 {
            Err((ix.at(params, INTERVAL_TIME, 0) * rate / 1_000.0)
                .round()
                .max(1.0) as i64)
        } else {
            Ok(DIVISIONS[ix.at(params, INTERVAL_DIVISION, 0) as usize % DIVISIONS.len()])
        };
        let next = |from: i64| match period {
            Ok(ticks) => next_on_grid(tempo, sample_rate, from, ticks, 0).1,
            Err(frames) => (from + frames - 1).div_euclid(frames) * frames,
        };
        let mut next_sync = sync.then(|| next(audio.song_frame(0)));
        let dry_len = self.dry[0].len();
        for i in 0..frames {
            let v = |id| ix.at(params, id, i);
            let held = v(FREEZE) >= 0.5;
            if held && !self.held && v(FREEZER_ON) >= 0.5 && v(FREEZER_MODE) < 0.5 {
                for engine in &mut self.engines {
                    engine.trigger();
                }
            }
            self.held = held;
            if let Some(at) = next_sync
                && audio.song_frame(i) >= at
            {
                for engine in &mut self.engines {
                    engine.trigger();
                }
                next_sync = Some(next(audio.song_frame(i) + 1));
            }
            let entry = (v(RESOLUTION) as usize).min(SIZES.len() - 1);
            if entry != self.target {
                self.change(entry);
            }
            self.zero_dry = v(ZERO_DRY_LATENCY) >= 0.5;
            let (send, mix) = (v(INPUT_SEND) as f32, (v(DRY_WET) / 100.0) as f32);
            let mixed = self.crossfade();
            let from = self.fade.as_ref().map(|f| f.from);
            let at_target = self.settings_if_due(params, &ix, audio, i, self.target);
            let at_from = from.and_then(|f| self.settings_if_due(params, &ix, audio, i, f));
            let tap =
                |engine: &Engine, zero: bool| if zero { 0 } else { engine.latency() as usize };
            let delay_target = tap(&self.engines[self.target], self.zero_dry);
            let delay_from = from.map_or(delay_target, |f| tap(&self.engines[f], self.zero_dry));
            for c in 0..channels {
                let input = audio.channel(c)[i];
                self.dry[c][self.dry_at] = input;
                let wet_target = self.engines[self.target].process(
                    c,
                    input * send,
                    at_target.as_ref(),
                    Some(&mut self.meter),
                );
                let (wet, dry) = match from {
                    Some(f) => {
                        let wet_from =
                            self.engines[f].process(c, input * send, at_from.as_ref(), None);
                        (wet_from + (wet_target - wet_from) * mixed, {
                            let d = |n: usize| self.dry[c][(self.dry_at + dry_len - n) % dry_len];
                            d(delay_from) + (d(delay_target) - d(delay_from)) * mixed
                        })
                    }
                    None => (
                        wet_target,
                        self.dry[c][(self.dry_at + dry_len - delay_target) % dry_len],
                    ),
                };
                audio.channel_mut(c)[i] = dry * (1.0 - mix) + wet * mix;
            }
            self.dry_at = (self.dry_at + 1) % dry_len;
            self.engines[self.target].clock += 1;
            if let Some(f) = from {
                self.engines[f].clock += 1;
            }
            self.meter.tick();
            if let Some(fade) = &mut self.fade {
                if fade.wait > 0 {
                    fade.wait -= 1;
                } else {
                    fade.pos += 1;
                }
            }
            if self.crossfade() >= 1.0 {
                self.fade = None;
            }
        }
    }

    fn latency(&self) -> u32 {
        if self.zero_dry {
            0
        } else {
            self.engines[self.target].latency()
        }
    }

    fn display_len(&self) -> usize {
        GRID.columns * GRID.rows
    }

    fn display(&mut self, out: &mut [f32]) {
        // The oldest column first.
        let (newer, older) = self.meter.grid.split_at(self.meter.head * GRID.rows);
        let (first, second) = out.split_at_mut(older.len());
        first.copy_from_slice(older);
        second.copy_from_slice(newer);
    }
}

impl SpectralTime {
    fn settings_if_due(
        &self,
        params: &ParamBank,
        ix: &Index,
        audio: &Audio<'_, '_>,
        i: usize,
        engine: usize,
    ) -> Option<Settings> {
        self.engines[engine]
            .due()
            .then(|| self.settings(params, ix, audio, i, &self.engines[engine]))
    }
}

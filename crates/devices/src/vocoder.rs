//! Vocoder, the filter-bank vocoder (WP-8.45; the vocoder spec): an analysis
//! bank on the modulator (the device's input) and a synthesis bank on the
//! carrier, 4 to 40 bands spaced over Range, each band of the carrier played
//! at the level its twin in the modulator has, followed with Attack and
//! Release, shaped by Depth, Gate and Enhance. The carrier is Noise (with
//! its X–Y control), External (the sidechain), the Modulator itself, or an
//! oscillator following the modulator's pitch (Pitch Tracking); Unvoiced
//! adds noise to the upper bands while the modulator is a hiss; Formant moves
//! the synthesis bank against the analysis one.
//!
//! The banks are in [`bank`], the carriers and the unvoiced detector in
//! [`carrier`]. Every source (the input, the sidechain) is delayed by the
//! pitch tracker's latency, whatever the carrier, so the oscillator's pitch
//! is the pitch of what it plays against and the device's latency never
//! changes with the carrier (R15).
//!
//! Its ranges and defaults are Beatlayer's own reading of Live 12's Vocoder
//! (see `docs/specs/vocoder.md`); the manual records few, so those are
//! listed to observe.

pub mod bank;
pub mod carrier;

use bl_dsp::pitch::{Config, Tracker};
use bl_engine::{DeviceType, NodeConfig, Registry};
use bl_model::Device;
use bl_model::device_layout::{BODY_HEIGHT, ControlBand, DisplayCell, LayoutNode, Widget};
use bl_model::{
    DeviceDescription, DeviceDisplay, DeviceId, DeviceLayout, DeviceRole, DisplayHandle,
    DisplayMarker, DisplaySeries, OrdMap, ParamDescriptor, ParamId, ParamUnit,
};

use crate::framework::{Audio, DelayLine, NativeDevice, ParamBank, add};
use bank::{BAND_COUNTS, Bank, Coeffs, MAX_BANDS};
use carrier::{Noise, Oscillator, Unvoiced, Wave};

/// Vocoder's device type.
pub const VOCODER: &str = "beatlayer.vocoder";

/// Carrier: Noise, External, Modulator or Pitch Tracking.
pub const CARRIER: ParamId = ParamId(0);
/// Bands: how many bands each bank has.
pub const BANDS: ParamId = ParamId(1);
/// Formant: the synthesis bank's shift against the analysis bank's, in
/// semitones.
pub const FORMANT: ParamId = ParamId(2);
/// Depth: how far each band's level shapes its gain, in per cent.
pub const DEPTH: ParamId = ParamId(3);
/// Attack: the time an envelope takes to cover 63 % of a rise, in ms.
pub const ATTACK: ParamId = ParamId(4);
/// Release: the time an envelope takes to cover 63 % of a fall, in ms.
pub const RELEASE: ParamId = ParamId(5);
/// Unvoiced: the noise added to the upper bands for hissing sounds.
pub const UNVOICED: ParamId = ParamId(6);
/// Dry/Wet: the balance between the input and the vocoded signal.
pub const DRY_WET: ParamId = ParamId(7);
/// Range's low end, in hertz.
pub const RANGE_LOW: ParamId = ParamId(8);
/// Range's high end, in hertz.
pub const RANGE_HIGH: ParamId = ParamId(9);
/// BW: the bands' bandwidth against the spacing between them, in per cent.
pub const BW: ParamId = ParamId(10);
/// Precise or Retro filters.
pub const FILTER_MODE: ParamId = ParamId(11);
/// Gate: the level under which a band is silenced, in dBFS.
pub const GATE: ParamId = ParamId(12);
/// Level: the output's gain.
pub const LEVEL: ParamId = ParamId(13);
/// Enhance: normalises each band against its neighbours over 50 ms.
pub const ENHANCE: ParamId = ParamId(14);
/// Mono, Stereo or L/R.
pub const CHANNEL_MODE: ParamId = ParamId(15);
/// The noise's X, its sample-and-hold rate, in per cent.
pub const NOISE_X: ParamId = ParamId(16);
/// The noise's Y, its density, in per cent.
pub const NOISE_Y: ParamId = ParamId(17);
/// Low: the lowest pitch the oscillator plays, in hertz.
pub const PITCH_LOW: ParamId = ParamId(18);
/// High: the highest pitch the oscillator plays, in hertz.
pub const PITCH_HIGH: ParamId = ParamId(19);
/// Pitch: the oscillator's tuning against the voice, in semitones.
pub const PITCH: ParamId = ParamId(20);
/// Waveform: Saw or Pulse 1, 2 or 3.
pub const WAVEFORM: ParamId = ParamId(21);
/// Sens.: how readily a frame counts as unvoiced, in per cent.
pub const SENS: ParamId = ParamId(22);
/// Fast or Slow unvoiced frames.
pub const FAST_SLOW: ParamId = ParamId(23);

/// The Carrier choices, by their indices.
const CARRIER_NOISE: usize = 0;
const CARRIER_EXTERNAL: usize = 1;
const CARRIER_MODULATOR: usize = 2;
const CARRIER_PITCH: usize = 3;

/// The Channel Mode choices, by their indices.
const MODE_MONO: usize = 0;
const MODE_LR: usize = 2;

/// The Bands choice a new device starts at: 20 bands.
const DEFAULT_BANDS: usize = 4;

/// The lowest and highest pitch the tracker is built to find, in hertz: the
/// ends of Low's and High's ranges.
const TRACK_LOW_HZ: f64 = 40.0;
const TRACK_HIGH_HZ: f64 = 2_000.0;

/// What the envelope follower's mean of a rectified band becomes, to read as
/// the band's peak amplitude: π / 2.
const PEAK: f64 = std::f64::consts::FRAC_PI_2;

/// The periods of a band's centre its rectified output is smoothed over
/// before the follower sees it.
const RIPPLE_PERIODS: f64 = 1.5;

/// Samples between the controls' updates, over which the band gains ramp.
const TICK: usize = 16;

/// How long Enhance averages over, in seconds (R9).
const ENHANCE_SECONDS: f64 = 0.05;
/// The most Enhance raises a band by.
const ENHANCE_MAX: f64 = 16.0;

/// The gain every band's level gets before Level: the product of a carrier
/// band and a modulator band is small. Beatlayer's own.
const MAKEUP: f64 = 4.0;

/// The lowest a band shows in the display, in dB.
const FLOOR_DB: f32 = -80.0;

/// The frame of the Fast and Slow unvoiced detectors, in seconds (R12).
const FRAME_FAST: f64 = 0.005;
const FRAME_SLOW: f64 = 0.030;

/// The Vocoder device type.
#[must_use]
pub fn vocoder_type() -> DeviceType {
    let counts: Vec<String> = BAND_COUNTS.iter().map(ToString::to_string).collect();
    let counts: Vec<&str> = counts.iter().map(String::as_str).collect();
    let params = vec![
        ParamDescriptor::choice(
            CARRIER,
            "Carrier",
            &["Noise", "External", "Modulator", "Pitch Tracking"],
            CARRIER_NOISE,
        )
        .describe(
            "The sound the modulator's spectrum is put onto: noise, the sidechain, the \
             input itself, or an oscillator following the input's pitch.",
        ),
        ParamDescriptor::choice(BANDS, "Bands", &counts, DEFAULT_BANDS)
            .describe("How many bands each filter bank has."),
        ParamDescriptor::continuous(FORMANT, "Formant", (-12.0, 12.0), 0.0, ParamUnit::Semitones)
            .describe(
                "Moves the carrier's bands against the modulator's, changing the voice's size.",
            ),
        ParamDescriptor::continuous(DEPTH, "Depth", (0.0, 200.0), 100.0, ParamUnit::Percent)
            .describe(
                "How far the modulator's levels shape the carrier: none at 0 %, classic at \
                 100 %, only the peaks at 200 %.",
            ),
        ParamDescriptor::continuous(ATTACK, "Attack", (1.0, 300.0), 5.0, ParamUnit::Milliseconds)
            .logarithmic()
            .describe("How quickly the bands follow a rise in the modulator."),
        ParamDescriptor::continuous(
            RELEASE,
            "Release",
            (5.0, 3_000.0),
            100.0,
            ParamUnit::Milliseconds,
        )
        .logarithmic()
        .describe("How quickly the bands follow a fall in the modulator."),
        ParamDescriptor::continuous(UNVOICED, "Unvoiced", (0.0, 100.0), 0.0, ParamUnit::Percent)
            .describe("Adds noise to the upper bands while the modulator hisses."),
        ParamDescriptor::continuous(DRY_WET, "Dry/Wet", (0.0, 100.0), 100.0, ParamUnit::Percent)
            .describe("The balance between the input and the vocoded signal."),
        ParamDescriptor::continuous(
            RANGE_LOW,
            "Range Low",
            (20.0, 20_000.0),
            80.0,
            ParamUnit::Hertz,
        )
        .logarithmic()
        .describe("The centre of the lowest band."),
        ParamDescriptor::continuous(
            RANGE_HIGH,
            "Range High",
            (20.0, 20_000.0),
            12_000.0,
            ParamUnit::Hertz,
        )
        .logarithmic()
        .describe("The centre of the highest band."),
        ParamDescriptor::continuous(BW, "BW", (10.0, 200.0), 100.0, ParamUnit::Percent)
            .describe("The bands' bandwidth against the spacing between them."),
        ParamDescriptor::choice(FILTER_MODE, "Precise/Retro", &["Precise", "Retro"], 0)
            .describe("Precise filters, or the gentler boosted filters of older vocoders."),
        ParamDescriptor::continuous(GATE, "Gate", (-80.0, 0.0), -80.0, ParamUnit::Decibels)
            .describe("Silences a band whose modulator level is below it."),
        ParamDescriptor::gain(LEVEL, "Level", 24.0, 0.0)
            .describe("The output's level, down to silence."),
        ParamDescriptor::toggle(ENHANCE, "Enhance", false)
            .describe("Normalises each band against its neighbours over the last 50 ms."),
        ParamDescriptor::choice(CHANNEL_MODE, "Channel Mode", &["Mono", "Stereo", "L/R"], 1)
            .describe(
                "Mono: one band set for both channels. Stereo: a mono modulator, a stereo \
                 carrier. L/R: both stereo.",
            ),
        ParamDescriptor::continuous(NOISE_X, "Noise X", (0.0, 100.0), 0.0, ParamUnit::Percent)
            .describe("Lowers the noise's sample rate, from the sample rate to 1 kHz."),
        ParamDescriptor::continuous(NOISE_Y, "Noise Y", (0.0, 100.0), 0.0, ParamUnit::Percent)
            .describe("Thins the noise's impulses, from every sample to one in 64."),
        ParamDescriptor::continuous(
            PITCH_LOW,
            "Low",
            (TRACK_LOW_HZ, TRACK_HIGH_HZ),
            50.0,
            ParamUnit::Hertz,
        )
        .logarithmic()
        .describe("The lowest pitch the oscillator follows the voice down to."),
        ParamDescriptor::continuous(
            PITCH_HIGH,
            "High",
            (TRACK_LOW_HZ, TRACK_HIGH_HZ),
            1_000.0,
            ParamUnit::Hertz,
        )
        .logarithmic()
        .describe("The highest pitch the oscillator follows the voice up to."),
        ParamDescriptor::continuous(PITCH, "Pitch", (-24.0, 24.0), 0.0, ParamUnit::Semitones)
            .describe("Tunes the oscillator against the voice."),
        ParamDescriptor::choice(
            WAVEFORM,
            "Waveform",
            &["Saw", "Pulse 1", "Pulse 2", "Pulse 3"],
            0,
        )
        .describe("The oscillator's waveform: a sawtooth, or a pulse of 50, 25 or 12.5 %."),
        ParamDescriptor::continuous(SENS, "Sens.", (0.0, 100.0), 50.0, ParamUnit::Percent)
            .describe("How readily a frame counts as unvoiced."),
        ParamDescriptor::choice(FAST_SLOW, "Fast/Slow", &["Fast", "Slow"], 0)
            .describe("Whether the unvoiced detector follows the modulator in 5 ms or 30 ms."),
    ];
    DeviceType::native(VOCODER, "Vocoder", DeviceRole::AudioEffect)
        .with_sidechain()
        .with_description(
            DeviceDescription::new(params)
                .with_layout(layout())
                .with_banks(&[
                    (
                        "Main",
                        &[
                            "Carrier", "Bands", "Formant", "Depth", "Attack", "Release",
                            "Unvoiced", "Dry/Wet",
                        ],
                    ),
                    (
                        "Filters",
                        &[
                            "Range Low",
                            "Range High",
                            "BW",
                            "Precise/Retro",
                            "Gate",
                            "Level",
                            "Enhance",
                            "Channel Mode",
                        ],
                    ),
                    (
                        "Carrier",
                        &[
                            "Noise X",
                            "Noise Y",
                            "Low",
                            "High",
                            "Pitch",
                            "Waveform",
                            "Sens.",
                            "Fast/Slow",
                        ],
                    ),
                ]),
        )
}

/// The device's layout: the choices on the left, the filters' and the
/// envelopes' knobs, the band display with the level and gate beside it, and
/// the noise's and the oscillator's knobs on the right.
fn layout() -> DeviceLayout {
    let band = |controls: &[(ParamId, Widget)]| {
        LayoutNode::Band(ControlBand::stacked(BODY_HEIGHT, controls))
    };
    DeviceLayout::new(LayoutNode::Row(vec![
        band(&[
            (CARRIER, Widget::Chooser),
            (BANDS, Widget::Chooser),
            (CHANNEL_MODE, Widget::Chooser),
            (FILTER_MODE, Widget::Chooser),
            (ENHANCE, Widget::Toggle),
            (WAVEFORM, Widget::Chooser),
            (FAST_SLOW, Widget::Chooser),
        ]),
        band(&[
            (DEPTH, Widget::Knob),
            (ATTACK, Widget::Knob),
            (RELEASE, Widget::Knob),
            (FORMANT, Widget::Knob),
            (UNVOICED, Widget::Knob),
            (SENS, Widget::Knob),
        ]),
        LayoutNode::Cell(DisplayCell::new("Band Display", 300.0, BODY_HEIGHT, 1.0)),
        band(&[
            (RANGE_LOW, Widget::Knob),
            (RANGE_HIGH, Widget::Knob),
            (GATE, Widget::Knob),
            (LEVEL, Widget::Knob),
            (BW, Widget::Knob),
            (DRY_WET, Widget::Knob),
        ]),
        band(&[
            (NOISE_X, Widget::Knob),
            (NOISE_Y, Widget::Knob),
            (PITCH_LOW, Widget::Knob),
            (PITCH_HIGH, Widget::Knob),
            (PITCH, Widget::Knob),
        ]),
    ]))
}

/// Adds the Vocoder to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, vocoder_type(), |device, config| {
        Vocoder::new(device, config)
    });
}

/// The bands' place on the frequency axis for Range's two ends as set: the
/// lower one first, the two a little apart.
#[must_use]
pub fn range(low: f64, high: f64) -> (f64, f64) {
    let (a, b) = (low.min(high), low.max(high));
    (a, b.max(a * 1.05))
}

/// What a bank's design depends on, so a change redesigns it.
#[derive(Clone, Copy, PartialEq)]
struct Design {
    count: usize,
    low: f64,
    high: f64,
    bw: f64,
    formant: f64,
    retro: bool,
}

/// One analysis bank, with the followers and the gains its twin bank plays at.
struct Analysis {
    bank: Bank,
    /// Each band's rectified output with its ripple smoothed off, and the
    /// followed level it drives.
    smooth: [f64; MAX_BANDS],
    follow: [f64; MAX_BANDS],
    /// Enhance's 50 ms averages of each band's neighbourhood and of all bands.
    neighbours: [f64; MAX_BANDS],
    overall: f64,
    /// The gain each synthesis band plays at, where it is heading, and how
    /// far it moves each sample.
    gain: [f64; MAX_BANDS],
    target: [f64; MAX_BANDS],
    step: [f64; MAX_BANDS],
    unvoiced: Unvoiced,
}

impl Analysis {
    fn new() -> Self {
        Self {
            bank: Bank::new(),
            smooth: [0.0; MAX_BANDS],
            follow: [0.0; MAX_BANDS],
            neighbours: [0.0; MAX_BANDS],
            overall: 0.0,
            gain: [0.0; MAX_BANDS],
            target: [0.0; MAX_BANDS],
            step: [0.0; MAX_BANDS],
            unvoiced: Unvoiced::new(),
        }
    }

    /// Follows one sample of the analysis bank's `bands`: each band's
    /// rectified output is smoothed by its own `smoothing`, then followed
    /// with the attack or the release step as it rises or falls.
    fn follow_bands(&mut self, bands: &[f64], smoothing: &[f64], (attack, release): (f64, f64)) {
        for (k, band) in bands.iter().enumerate() {
            self.smooth[k] += (band.abs() - self.smooth[k]) * smoothing[k];
            let step = if self.smooth[k] > self.follow[k] {
                attack
            } else {
                release
            };
            self.follow[k] += (self.smooth[k] - self.follow[k]) * step;
        }
    }

    /// Works the band gains out from the followed levels (R7 to R9) and
    /// starts them moving there over the next [`TICK`] samples.
    fn plan(&mut self, count: usize, gate: f64, depth: f64, enhance: Option<f64>) {
        let mut level = [0.0; MAX_BANDS];
        for (level, follow) in level.iter_mut().zip(&self.follow).take(count) {
            *level = follow * PEAK;
        }
        let mut shaped = level;
        if let Some(coefficient) = enhance {
            let mean = level[..count].iter().sum::<f64>() / count as f64;
            self.overall += (mean - self.overall) * coefficient;
            for k in 0..count {
                let near =
                    (level[k.saturating_sub(1)] + level[k] + level[(k + 1).min(count - 1)]) / 3.0;
                self.neighbours[k] += (near - self.neighbours[k]) * coefficient;
                let ratio = if self.neighbours[k] > 1e-9 {
                    (self.overall / self.neighbours[k]).min(ENHANCE_MAX)
                } else {
                    1.0
                };
                shaped[k] = level[k] * ratio;
            }
        }
        // Depth: a gain of the mean at 0 %, the level at 100 %, the level
        // squared over the mean at 200 %.
        let mean = shaped[..count].iter().sum::<f64>() / count as f64;
        for k in 0..count {
            let gain = if mean <= 1e-12 {
                0.0
            } else {
                mean * (shaped[k] / mean).powf(depth)
            };
            self.target[k] = if level[k] < gate { 0.0 } else { gain };
            self.step[k] = (self.target[k] - self.gain[k]) / TICK as f64;
        }
    }
}

/// Vocoder's audio-thread half.
pub struct Vocoder {
    rate: f64,
    latency: usize,
    design: Option<Design>,
    analysis_coeffs: Coeffs,
    synthesis_coeffs: Coeffs,
    /// Each band's smoothing of its rectified output, per sample.
    smoothing: [f64; MAX_BANDS],
    analysis: [Analysis; 2],
    synthesis: [Bank; 2],
    /// The input's and the sidechain's channels, delayed by the latency.
    input: [DelayLine; 2],
    sidechain: [DelayLine; 2],
    noise: [Noise; 2],
    unvoiced_noise: [Noise; 2],
    tracker: Tracker,
    oscillator: Oscillator,
    /// The voice's pitch as the tracker last called it voiced.
    pitch: f64,
    /// Samples since the controls were last updated.
    tick: usize,
    /// The output of a stretch, written back when it's done.
    out: [Vec<f32>; 2],
    /// Each band's gain at the last update, for the display.
    shown: [f64; MAX_BANDS],
}

impl Vocoder {
    fn new(device: &Device, config: &NodeConfig) -> Self {
        let rate = config.sample_rate.as_f64();
        let seed = device.id.0.get();
        let tracker = Tracker::new(&Config::for_range(rate, TRACK_LOW_HZ, TRACK_HIGH_HZ));
        let latency = tracker.latency().max(1);
        let frames = config.max_frames.max(1);
        Self {
            rate,
            latency,
            design: None,
            analysis_coeffs: Coeffs::new(),
            synthesis_coeffs: Coeffs::new(),
            smoothing: [0.0; MAX_BANDS],
            analysis: [Analysis::new(), Analysis::new()],
            synthesis: [Bank::new(), Bank::new()],
            input: [DelayLine::new(latency), DelayLine::new(latency)],
            sidechain: [DelayLine::new(latency), DelayLine::new(latency)],
            noise: [Noise::new(seed), Noise::new(seed ^ 0x5EED)],
            unvoiced_noise: [Noise::new(seed ^ 0x0015E), Noise::new(seed ^ 0xB015E)],
            tracker,
            oscillator: Oscillator::new(100.0),
            pitch: 100.0,
            tick: 0,
            out: [vec![0.0; frames], vec![0.0; frames]],
            shown: [0.0; MAX_BANDS],
        }
    }
}

impl NativeDevice for Vocoder {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        let frames = audio.frames();
        let channels = audio.channels().min(2);
        if frames == 0 || channels == 0 {
            return;
        }
        let at = |id| params.index(id).expect("the description holds it");
        // The values that shape a bank are read at the stretch's end, where
        // an event puts them, so they don't depend on how blocks fall.
        let get = |id| params.target(at(id));
        let carrier = get(CARRIER) as usize;
        let mode = get(CHANNEL_MODE) as usize;
        let count = BAND_COUNTS[(get(BANDS) as usize).min(BAND_COUNTS.len() - 1)];
        let (low, high) = range(get(RANGE_LOW), get(RANGE_HIGH));
        let design = Design {
            count,
            low,
            high,
            bw: get(BW),
            formant: get(FORMANT),
            retro: get(FILTER_MODE) >= 0.5,
        };
        if self.design != Some(design) {
            let layout = (count, low, high);
            self.analysis_coeffs
                .design(layout, design.bw, 1.0, design.retro, self.rate);
            self.synthesis_coeffs.design(
                layout,
                design.bw,
                (design.formant / 12.0).exp2(),
                design.retro,
                self.rate,
            );
            for k in 0..count {
                // A band's rectified output ripples at twice its centre:
                // smoothing over 1.5 periods of the centre leaves the
                // follower a level to follow, not the ripple.
                let hz = self.analysis_coeffs.centre(k);
                self.smoothing[k] = 1.0 - (-hz / (RIPPLE_PERIODS * self.rate)).exp();
            }
            self.design = Some(design);
        }
        let step_of = |ms: f64| 1.0 - (-1.0 / (ms * 0.001 * self.rate)).exp();
        let (attack, release) = (step_of(get(ATTACK)), step_of(get(RELEASE)));
        let depth = get(DEPTH) * 0.01;
        let gate = 10f64.powf(get(GATE) / 20.0);
        let enhance = (get(ENHANCE) >= 0.5)
            .then(|| 1.0 - (-(TICK as f64) / (ENHANCE_SECONDS * self.rate)).exp());
        let noise_x = get(NOISE_X) * 0.01;
        let noise_y = get(NOISE_Y) * 0.01;
        let unvoiced = get(UNVOICED) * 0.01;
        let sens = get(SENS);
        let frame = (if get(FAST_SLOW) >= 0.5 {
            FRAME_SLOW
        } else {
            FRAME_FAST
        } * self.rate)
            .round() as usize;
        let (pitch_low, pitch_high) = {
            let (a, b) = (get(PITCH_LOW), get(PITCH_HIGH));
            (a.min(b), a.max(b))
        };
        let tune = (get(PITCH) / 12.0).exp2();
        let wave = Wave::of(get(WAVEFORM) as usize);
        let level = params.ramp(at(LEVEL));
        let wet = params.ramp(at(DRY_WET));
        let analyses = if mode == MODE_LR { 2 } else { 1 };
        let voices = if mode == MODE_MONO { 1 } else { 2 };
        let split = count / 2;
        let has_sidechain = audio.sidechain(0).is_some();

        let mut modulator_bands = [0.0; MAX_BANDS];
        let mut carrier_bands = [0.0; MAX_BANDS];
        for i in 0..frames {
            // The sources, delayed together by the latency.
            let heard = [
                audio.channel(0)[i],
                if channels > 1 {
                    audio.channel(1)[i]
                } else {
                    audio.channel(0)[i]
                },
            ];
            let keyed = [
                audio.sidechain(0).map_or(0.0, |s| s[i]),
                audio.sidechain(1.min(channels - 1)).map_or(0.0, |s| s[i]),
            ];
            let mut input = [0.0; 2];
            let mut key = [0.0; 2];
            for c in 0..2 {
                input[c] = f64::from(self.input[c].read(self.latency));
                self.input[c].write(heard[c]);
                key[c] = f64::from(self.sidechain[c].read(self.latency));
                self.sidechain[c].write(keyed[c]);
            }
            if carrier == CARRIER_PITCH {
                self.tracker.feed(0.5 * (heard[0] + heard[1]));
                while let Some(measured) = self.tracker.pop() {
                    if let Some(p) = measured.filter(|p| p.voiced) {
                        self.pitch = p.frequency;
                    }
                }
            }

            // The analysis banks and their followers.
            let mid = 0.5 * (input[0] + input[1]);
            for (a, state) in self.analysis[..analyses].iter_mut().enumerate() {
                let x = if mode == MODE_LR { input[a] } else { mid };
                state
                    .bank
                    .tick(&self.analysis_coeffs, (x, x), count, &mut modulator_bands);
                state.follow_bands(
                    &modulator_bands[..count],
                    &self.smoothing,
                    (attack, release),
                );
                state.unvoiced.hear(x, frame, sens, gate, self.rate);
                if self.tick == 0 {
                    state.plan(count, gate, depth, enhance);
                    state.bank.flush();
                }
                for (gain, step) in state.gain.iter_mut().zip(&state.step).take(count) {
                    *gain += step;
                }
            }

            // The carrier, and the synthesis banks it passes through.
            let oscillator = if carrier == CARRIER_PITCH {
                let hz = self.pitch.clamp(pitch_low, pitch_high) * tune;
                self.oscillator.next(wave, hz, self.rate)
            } else {
                0.0
            };
            let mut played = [0.0; 2];
            for v in 0..voices {
                let a = if mode == MODE_LR { v } else { 0 };
                let (modulator, external) = if mode == MODE_MONO {
                    (mid, 0.5 * (key[0] + key[1]))
                } else {
                    (input[v], key[v])
                };
                let source = match carrier {
                    CARRIER_NOISE => self.noise[v].next(noise_x, noise_y, self.rate),
                    CARRIER_EXTERNAL if has_sidechain => external,
                    CARRIER_MODULATOR => modulator,
                    CARRIER_PITCH => oscillator,
                    _ => 0.0,
                };
                let hiss =
                    self.unvoiced_noise[v].white() * unvoiced * self.analysis[a].unvoiced.amount;
                self.synthesis[v].tick(
                    &self.synthesis_coeffs,
                    (source, source + hiss),
                    split,
                    &mut carrier_bands,
                );
                let gains = &self.analysis[a].gain;
                let mut sum = 0.0;
                for k in 0..count {
                    sum += gains[k] * carrier_bands[k];
                }
                played[v] = sum * MAKEUP;
                if self.tick == 0 {
                    self.synthesis[v].flush();
                }
            }
            if mode == MODE_MONO {
                played[1] = played[0];
            }

            let mix = f64::from(wet[i]) * 0.01;
            let gain = f64::from(level[i]);
            for c in 0..2 {
                self.out[c][i] = (mix * played[c] * gain + (1.0 - mix) * input[c]) as f32;
            }
            self.tick = (self.tick + 1) % TICK;
        }
        for k in 0..count {
            self.shown[k] = (0..analyses)
                .map(|a| self.analysis[a].target[k])
                .fold(0.0, f64::max);
        }
        for c in 0..channels {
            audio.channel_mut(c).copy_from_slice(&self.out[c][..frames]);
        }
    }

    fn latency(&self) -> u32 {
        self.latency as u32
    }

    fn display_len(&self) -> usize {
        MAX_BANDS
    }

    fn display(&mut self, out: &mut [f32]) {
        for (value, &gain) in out.iter_mut().zip(&self.shown) {
            *value = (20.0 * gain.max(1e-9).log10() as f32).max(FLOOR_DB);
        }
    }
}

/// How a frequency reads: hertz below 1 kHz, else kilohertz.
fn hertz(hz: f64) -> String {
    if hz < 1_000.0 {
        format!("{hz:.0} Hz")
    } else {
        format!("{:.2} kHz", hz / 1_000.0)
    }
}

/// The device's display (the vocoder spec, R13): each band's synthesis level
/// at its centre as a marker on the frequency axis, the Gate as a line, and
/// a handle on each end of Range (Low End and High End). `data` is what the node published, if an
/// engine runs.
#[must_use]
pub fn display(
    device: DeviceId,
    description: &DeviceDescription,
    values: &OrdMap<ParamId, f64>,
    data: Option<&[f32]>,
) -> DeviceDisplay {
    let value = |id: ParamId| description.value(values, id).unwrap_or(0.0);
    let count = BAND_COUNTS[(value(BANDS) as usize).min(BAND_COUNTS.len() - 1)];
    let (low, high) = range(value(RANGE_LOW), value(RANGE_HIGH));
    let shift = (value(FORMANT) / 12.0).exp2();
    let axis = |name: &str, values: Vec<f32>| DisplaySeries {
        name: name.to_owned(),
        unit: "dB".to_owned(),
        x_unit: "Hz".to_owned(),
        x_range: [20.0, 20_000.0],
        log_x: true,
        y_range: [FLOOR_DB, 0.0],
        detail: false,
        values,
    };
    let mut markers: Vec<DisplayMarker> = (0..count)
        .map(|k| {
            let hz = bank::centre(count, low, high, k) * shift;
            let level = data.and_then(|d| d.get(k)).copied().unwrap_or(FLOOR_DB);
            DisplayMarker {
                name: format!("Band {}", k + 1),
                series: "Bands".to_owned(),
                x: hz as f32,
                y: level,
                text: format!("Band {} {} {level:.1} dB", k + 1, hertz(hz)),
                handle: None,
            }
        })
        .collect();
    for (name, id, hz) in [
        ("Low End", RANGE_LOW, value(RANGE_LOW)),
        ("High End", RANGE_HIGH, value(RANGE_HIGH)),
    ] {
        markers.push(DisplayMarker {
            name: name.to_owned(),
            series: "Bands".to_owned(),
            x: hz as f32,
            y: FLOOR_DB + 14.0,
            text: format!("{name} {}", hertz(hz)),
            handle: Some(DisplayHandle { x: id, y: None }),
        });
    }
    DeviceDisplay {
        spectrograms: Vec::new(),
        device,
        live: data.is_some(),
        view: None,
        series: vec![
            axis("Bands", Vec::new()),
            axis("Gate", vec![value(GATE) as f32; 2]),
        ],
        markers,
        readings: Vec::new(),
    }
}

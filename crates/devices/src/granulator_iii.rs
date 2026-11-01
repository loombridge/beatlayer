//! Granulator III, part 1 (WP-8.26; spec: granulator iii R1 to R17): a
//! sample played as a stream of short windowed grains in three modes,
//! Classic, Loop and Cloud, through the instruments' voice section, an
//! amplitude envelope, the instruments' filter and Volume, with the
//! waveform and grain display the framework publishes. Capturing audio in
//! real time is part 2's.
//!
//! Every voice owns a pool of [`POOL`] grains made with the device. A
//! grain's onset is a frame worked out from its note's frame or the song
//! position, never from a block's start, and every random value comes from
//! a generator seeded per device and voice and restarted with each note, so
//! a render is the same at any block size and run after run.

use core::ops::Range;
use std::sync::Arc;

use bl_core::time::{Frames, TICKS_PER_QUARTER, Ticks};
use bl_dsp::Random;
use bl_engine::{DeviceType, NodeConfig, NodeData, NodeEvent, Registry, SlotSample};
use bl_media::SampleData;
use bl_model::device_layout::{
    BODY_HEIGHT, ControlBand, DisplayCell, LayoutNode, Page, PageStyle, PagesNode, TAB_HEIGHT,
    Widget,
};
use bl_model::{
    Device, DeviceDescription, DeviceDisplay, DeviceId, DeviceLayout, DeviceRole, DisplayMarker,
    DisplayReading, DisplaySeries, OrdMap, ParamDescriptor, ParamId, ParamUnit,
};
use lspk_dsp::envelope::{EnvelopeBank, EnvelopeParams, StealRestart, TimeUnit, Timing, Trigger};
use lspk_dsp::filter::pitch_of;
use lspk_voice::{MAX_VOICES, Start, VoiceBlock, VoiceEvent, VoiceEventKind};

use crate::framework::{Audio, NativeDevice, ParamBank, add};
use crate::instrument::{self, Instrument, NoteRange, Parts, Section, Sources, State};
use crate::instrument_filter::{InstrumentFilter, Per, PerFrame, Setup};

/// Granulator III's device type.
pub const GRANULATOR_III: &str = "beatlayer.granulator_iii";

pub const MODE: ParamId = ParamId(0);
pub const POSITION: ParamId = ParamId(1);
pub const SIZE: ParamId = ParamId(2);
pub const SHAPE: ParamId = ParamId(3);
pub const SPRAY: ParamId = ParamId(4);
pub const SCAN: ParamId = ParamId(5);
pub const DENSITY: ParamId = ParamId(6);
pub const RATE: ParamId = ParamId(7);
pub const VARIATION: ParamId = ParamId(8);
pub const WIDTH: ParamId = ParamId(9);
pub const TRANSPOSE: ParamId = ParamId(10);
pub const FINE: ParamId = ParamId(11);
pub const ATTACK: ParamId = ParamId(12);
pub const DECAY: ParamId = ParamId(13);
pub const SUSTAIN: ParamId = ParamId(14);
pub const RELEASE: ParamId = ParamId(15);
pub const FILTER: ParamId = ParamId(16);
pub const FILTER_TYPE: ParamId = ParamId(17);
pub const FILTER_SLOPE: ParamId = ParamId(18);
pub const FILTER_CIRCUIT: ParamId = ParamId(19);
pub const FILTER_FREQ: ParamId = ParamId(20);
pub const FILTER_RES: ParamId = ParamId(21);
pub const VOLUME: ParamId = ParamId(22);
pub const VOICES: ParamId = ParamId(23);
pub const PRIORITY: ParamId = ParamId(24);
pub const NOTE_PB: ParamId = ParamId(25);
pub const PB_RANGE: ParamId = ParamId(26);
pub const SLIDE_AMOUNT: ParamId = ParamId(27);
pub const PRESSURE_SIZE: ParamId = ParamId(28);
pub const PRESSURE_SHAPE: ParamId = ParamId(29);

pub const MODES: [&str; 3] = ["Classic", "Loop", "Cloud"];
pub const CLASSIC: usize = 0;
pub const LOOP: usize = 1;
pub const CLOUD: usize = 2;

/// Loop's Rate choices, and each one's length in beats (quarter notes).
pub const RATES: [&str; 7] = ["1/64", "1/32", "1/16", "1/8", "1/4", "1/2", "1 Bar"];
const RATE_BEATS: [f64; 7] = [0.0625, 0.125, 0.25, 0.5, 1.0, 2.0, 4.0];
pub const FILTER_TYPES: [&str; 3] = ["Low-pass", "High-pass", "Band-pass"];
pub const SLOPES: [&str; 2] = ["12 dB", "24 dB"];
/// The circuits R11 offers: Clean and MS2, as the filter's own list numbers
/// them.
pub const CIRCUITS: [&str; 2] = ["Clean", "MS2"];
const CIRCUIT_AT: [usize; 2] = [0, 2];

/// Spec: granulator iii R4. The grains a voice's pool holds.
pub const POOL: usize = 64;
/// Spec: granulator iii R14. The waveform's peaks at a fixed resolution.
pub const PEAKS: usize = 128;
/// The floats the display data holds: the grains dropped, the grains shown,
/// the source's length in seconds, then the grains' positions and the peaks.
pub const DISPLAY_LEN: usize = 3 + POOL + PEAKS;

/// Spec: granulator iii R5. Size's range in milliseconds, and the share of
/// Spray's reach: a second.
const SIZE_MIN: f64 = 1.0;
const SIZE_MAX: f64 = 1_000.0;
/// Spec: granulator iii R5. Flat windows fade over a millisecond.
const FADE_SECONDS: f64 = 0.001;
/// Spec: granulator iii R9. Cloud's grains a second per unit of Density.
const CLOUD_PER_DENSITY: f64 = 10.0;
/// Spec: granulator iii R13. Slide moves Position by up to this share either
/// way from its centre, 0.5.
const SLIDE_REACH: f64 = 0.5;
/// The most Loop onsets one stretch holds.
const GRID: usize = 512;
/// The source frames the display's peaks scan in a stretch.
const SCAN_BUDGET: usize = 16_384;
/// The window table's resolution and the bells between Hann and Gaussian.
const TABLE: usize = 512;
const BELLS: usize = 17;
/// The Gaussian window's width as a share of the grain, at Shape 100 %.
const SIGMA: f64 = 0.12;
const ENV_STRIDE: usize = MAX_VOICES.next_multiple_of(16);
const REFERENCE_HZ: f32 = 440.0;
const MIDDLE_C: f64 = 60.0;

/// The parameters read frame by frame, and where each one's ramp is kept.
const RAMPED: [ParamId; 15] = [
    POSITION,
    SIZE,
    SHAPE,
    SPRAY,
    SCAN,
    DENSITY,
    VARIATION,
    WIDTH,
    FINE,
    SLIDE_AMOUNT,
    PRESSURE_SIZE,
    PRESSURE_SHAPE,
    VOLUME,
    FILTER_FREQ,
    FILTER_RES,
];
const R_POSITION: usize = 0;
const R_SIZE: usize = 1;
const R_SHAPE: usize = 2;
const R_SPRAY: usize = 3;
const R_SCAN: usize = 4;
const R_DENSITY: usize = 5;
const R_VARIATION: usize = 6;
const R_WIDTH: usize = 7;
const R_FINE: usize = 8;
const R_SLIDE: usize = 9;
const R_PRESS_SIZE: usize = 10;
const R_PRESS_SHAPE: usize = 11;
const R_VOLUME: usize = 12;
const R_FREQ: usize = 13;
const R_RES: usize = 14;

/// The Granulator III device type.
#[must_use]
pub fn granulator_iii_type() -> DeviceType {
    let time = |id, name: &str, default: f64| {
        ParamDescriptor::continuous(id, name, (0.0, 20_000.0), default, ParamUnit::Milliseconds)
    };
    let percent = |id, name: &str, range: (f64, f64), default: f64| {
        ParamDescriptor::continuous(id, name, range, default, ParamUnit::Percent)
    };
    let params = vec![
        ParamDescriptor::choice(MODE, "Mode", &MODES, CLASSIC).describe(
            "Classic plays a steady stream of grains; Loop plays them on the song's beat; Cloud scatters them in time, position and pitch.",
        ),
        percent(POSITION, "Position", (0.0, 100.0), 0.0)
            .describe("Where in the sample grains start, as a share of its length."),
        ParamDescriptor::continuous(SIZE, "Size", (SIZE_MIN, SIZE_MAX), 100.0, ParamUnit::Milliseconds)
            .logarithmic()
            .describe("The length of each grain."),
        percent(SHAPE, "Shape", (0.0, 100.0), 50.0).describe(
            "Each grain's window: flat with short fades at 0 %, Hann at 50 %, a narrow bell at 100 %.",
        ),
        percent(SPRAY, "Spray", (0.0, 100.0), 0.0)
            .describe("Scatters each grain's start around Position by up to a second."),
        percent(SCAN, "Scan", (-100.0, 100.0), 0.0).describe(
            "Moves Position through the sample while a note is held, forward or backward, up to the sample's own speed.",
        ),
        ParamDescriptor::integer(DENSITY, "Density", (1, 16), 4, ParamUnit::None)
            .decimals(0)
            .describe("How many grains overlap in Classic; how many per tenth of a second in Cloud."),
        ParamDescriptor::choice(RATE, "Rate", &RATES, 2)
            .describe("The length of Loop's grid, in notes of the song's beat."),
        ParamDescriptor::continuous(VARIATION, "Variation", (0.0, 12.0), 0.0, ParamUnit::Semitones)
            .describe("Scatters each grain's pitch by up to this many semitones either way."),
        percent(WIDTH, "Width", (0.0, 100.0), 50.0)
            .describe("How far Cloud's grains scatter across the stereo field."),
        instrument::transpose(TRANSPOSE, 48),
        ParamDescriptor::continuous(FINE, "Fine", (-50.0, 50.0), 0.0, ParamUnit::Cents)
            .describe("Fine-tunes the grains' pitch in cents."),
        time(ATTACK, "Attack", 5.0)
            .describe("How long the amplitude envelope takes to reach its peak."),
        time(DECAY, "Decay", 1_000.0)
            .describe("How long the amplitude envelope takes to fall to its sustain level."),
        percent(SUSTAIN, "Sustain", (0.0, 100.0), 100.0)
            .describe("The level the amplitude envelope holds while a note lasts."),
        time(RELEASE, "Release", 200.0)
            .describe("How long the amplitude envelope takes to fall to silence after a note."),
        ParamDescriptor::toggle(FILTER, "Filter", false).describe("Sends the grains through the filter."),
        ParamDescriptor::choice(FILTER_TYPE, "Filter Type", &FILTER_TYPES, 0)
            .describe("Low-pass, high-pass or band-pass."),
        ParamDescriptor::choice(FILTER_SLOPE, "Filter Slope", &SLOPES, 0)
            .describe("The filter's steepness: 12 or 24 dB per octave."),
        ParamDescriptor::choice(FILTER_CIRCUIT, "Filter Circuit", &CIRCUITS, 0)
            .describe("The filter's circuit: Clean or MS2."),
        ParamDescriptor::continuous(FILTER_FREQ, "Filter Freq", (20.0, 20_000.0), 20_000.0, ParamUnit::Hertz)
            .logarithmic()
            .describe("The filter's cutoff."),
        percent(FILTER_RES, "Filter Res", (0.0, 100.0), 0.0).describe("The filter's resonance."),
        ParamDescriptor::gain(VOLUME, "Volume", 0.0, 0.0)
            .describe("The instrument's output level, down to silence."),
        instrument::voices(VOICES, MAX_VOICES as i64, 8),
        instrument::priority(PRIORITY),
        instrument::note_pb(NOTE_PB, true),
        instrument::pb_range(PB_RANGE, 96, 2),
        percent(SLIDE_AMOUNT, "Slide Amount", (-100.0, 100.0), 50.0).describe(
            "How far an MPE note's slide moves its Position: up to half the sample at 100 %.",
        ),
        percent(PRESSURE_SIZE, "Pressure > Size", (-100.0, 100.0), 0.0)
            .describe("How far an MPE note's pressure moves its grains' Size."),
        percent(PRESSURE_SHAPE, "Pressure > Shape", (-100.0, 100.0), 0.0)
            .describe("How far an MPE note's pressure moves its grains' Shape."),
    ];
    DeviceType::native(GRANULATOR_III, "Granulator III", DeviceRole::Instrument).with_description(
        DeviceDescription::new(params)
            .with_layout(layout())
            .with_banks(&[
                (
                    "Grain",
                    &[
                        "Mode", "Position", "Size", "Shape", "Spray", "Scan", "Density", "Rate",
                    ],
                ),
                (
                    "Pitch",
                    &[
                        "Variation",
                        "Width",
                        "Transpose",
                        "Fine",
                        "Voices",
                        "Priority",
                        "Volume",
                    ],
                ),
                ("Envelope", &["Attack", "Decay", "Sustain", "Release"]),
                (
                    "Filter",
                    &[
                        "Filter",
                        "Filter Type",
                        "Filter Slope",
                        "Filter Circuit",
                        "Filter Freq",
                        "Filter Res",
                    ],
                ),
                (
                    "MPE",
                    &[
                        "Note PB",
                        "PB Range",
                        "Slide Amount",
                        "Pressure > Size",
                        "Pressure > Shape",
                    ],
                ),
            ]),
    )
}

/// The name of the node that holds the three pages, which `device.set_page`
/// and the saved page name.
pub const PAGES: &str = "Pages";

/// Spec: granulator iii R17. The display always shown, and the pages Grain,
/// Envelope and Filter, and MPE beside it.
fn layout() -> DeviceLayout {
    use Widget::{Chooser, Knob, Number, Toggle};
    let page = BODY_HEIGHT - TAB_HEIGHT;
    let band =
        |controls: &[(ParamId, Widget)]| LayoutNode::Band(ControlBand::stacked(page, controls));
    DeviceLayout::new(LayoutNode::Row(vec![
        LayoutNode::Cell(DisplayCell::new("Grains", 220.0, BODY_HEIGHT, 1.0)),
        LayoutNode::Pages(PagesNode::new(
            PAGES,
            PageStyle::Tabs,
            vec![
                Page::new(
                    "Grain",
                    LayoutNode::Row(vec![
                        band(&[
                            (MODE, Chooser),
                            (POSITION, Knob),
                            (SIZE, Knob),
                            (SHAPE, Knob),
                            (SPRAY, Knob),
                            (SCAN, Knob),
                        ]),
                        band(&[
                            (DENSITY, Number),
                            (RATE, Chooser),
                            (VARIATION, Knob),
                            (WIDTH, Knob),
                            (TRANSPOSE, Number),
                            (FINE, Knob),
                        ]),
                        band(&[(VOICES, Number), (PRIORITY, Chooser), (VOLUME, Knob)]),
                    ]),
                ),
                Page::new(
                    "Envelope and Filter",
                    LayoutNode::Row(vec![
                        band(&[
                            (ATTACK, Knob),
                            (DECAY, Knob),
                            (SUSTAIN, Knob),
                            (RELEASE, Knob),
                        ]),
                        band(&[
                            (FILTER, Toggle),
                            (FILTER_TYPE, Chooser),
                            (FILTER_SLOPE, Chooser),
                            (FILTER_CIRCUIT, Chooser),
                            (FILTER_FREQ, Knob),
                            (FILTER_RES, Knob),
                        ]),
                    ]),
                ),
                Page::new(
                    "MPE",
                    band(&[
                        (NOTE_PB, Toggle),
                        (PB_RANGE, Number),
                        (SLIDE_AMOUNT, Knob),
                        (PRESSURE_SIZE, Knob),
                        (PRESSURE_SHAPE, Knob),
                    ]),
                ),
            ],
        )),
    ]))
}

/// Adds Granulator III to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, granulator_iii_type(), |device, config| {
        GranulatorIii::new(device, config)
    });
}

/// One grain: a window of the source read at its pitch.
#[derive(Clone, Copy, Default)]
struct Grain {
    /// The read position in source frames.
    at: f64,
    /// How far it moves a frame at the note's own pitch: the grain's
    /// Variation.
    ratio: f64,
    age: u32,
    len: u32,
    /// Where the window sits between flat and bell, and which bell.
    flat: f32,
    bell: u8,
    gain: [f32; 2],
}

/// A voice's stream of grains: its generator, when the next grain starts
/// and how far Scan has moved Position.
struct Stream {
    random: Random,
    next: f64,
    scan: f64,
}

/// Granulator III's audio-thread half.
pub struct GranulatorIii {
    section: Section,
    grains: Grains,
    /// The decoded sample, kept as node data so that letting go of it never
    /// frees on the audio thread.
    data: Option<NodeData>,
}

impl GranulatorIii {
    fn new(device: &Device, config: &NodeConfig) -> Self {
        let rate = config.sample_rate.as_f64();
        Self {
            // Spec: granulator iii R12. The voices are the voice section's.
            section: Section::new(
                Parts {
                    voices: Some(VOICES),
                    priority: Some(PRIORITY),
                    transpose: Some(TRANSPOSE),
                    pb_range: Some(PB_RANGE),
                    note_pb: Some(NOTE_PB),
                    note_range: NoteRange::Mpe,
                    ..Parts::default()
                },
                MAX_VOICES,
                1,
                rate,
                0,
            ),
            grains: Grains::new(rate, config.max_frames, u64::from(device.id.0)),
            data: None,
        }
    }
}

impl NativeDevice for GranulatorIii {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        let frames = audio.frames();
        if frames == 0 || audio.channels() == 0 {
            return;
        }
        let sample = self
            .data
            .as_ref()
            .and_then(|data| data.downcast_ref::<SlotSample>())
            .filter(|slot| slot.sample.frames() > 0)
            .map(|slot| Arc::clone(&slot.sample));
        self.grains.snapshot(audio, params, sample);
        self.section.process(audio, params, &mut self.grains);
        self.grains.finish(audio, params);
    }

    fn flush(&mut self, _events: &[NodeEvent], _params: &ParamBank) {
        self.section.reset();
        self.grains.silence();
    }

    fn receive_data(&mut self, data: NodeData) {
        self.section.receive_data(&data);
        // The device has one sample slot.
        if data
            .downcast_ref::<SlotSample>()
            .is_some_and(|slot| slot.slot == 0)
        {
            self.data = Some(data);
        }
    }

    fn display_len(&self) -> usize {
        DISPLAY_LEN
    }

    fn display(&mut self, out: &mut [f32]) {
        self.grains.publish(out);
    }
}

/// The grain engine, as the voice section plays it.
struct Grains {
    rate: f64,
    seed: u64,
    max_frames: usize,
    /// The stretch's sample, taken from the node's data for its length.
    sample: Option<Arc<SampleData>>,
    pool: Box<[[Grain; POOL]; MAX_VOICES]>,
    live: [u64; MAX_VOICES],
    streams: Vec<Stream>,
    /// The parameters' ramps for the stretch, [`RAMPED`] by `max_frames`.
    ramps: Vec<f32>,
    mode: usize,
    beats: f64,
    bpm: f64,
    /// Spec: granulator iii R8. The stretch's grid onsets, in frames of the
    /// stretch, while the song plays Loop.
    grid: Vec<u32>,
    grid_on: bool,
    env: EnvelopeBank,
    env_params: EnvelopeParams,
    env_out: Vec<f32>,
    filter: InstrumentFilter,
    filter_on: bool,
    stride: usize,
    audio: Vec<f32>,
    cutoff: Vec<f32>,
    res: Vec<f32>,
    zeros: Vec<f32>,
    /// Hann-to-Gaussian windows, one for each of [`BELLS`] widths.
    bells: Vec<f32>,
    /// Grains dropped for want of room in a pool (R4).
    dropped: u64,
    /// The display's peaks, and how far through the sample they are.
    peaks: [f32; PEAKS],
    scanned: usize,
    scanning: usize,
    /// The source's length in frames, for the display.
    length: f64,
}

impl Grains {
    fn new(rate: f64, max_frames: usize, seed: u64) -> Self {
        let max_frames = max_frames.max(1);
        let filter = InstrumentFilter::new(rate as f32, MAX_VOICES, max_frames);
        let stride = filter.stride();
        let bells = (0..BELLS)
            .flat_map(|k| {
                let gauss = k as f64 / (BELLS - 1) as f64;
                (0..=TABLE).map(move |n| {
                    let t = n as f64 / TABLE as f64;
                    let hann = 0.5 - 0.5 * libm::cos(core::f64::consts::TAU * t);
                    let bell = libm::exp(-0.5 * ((t - 0.5) / SIGMA).powi(2));
                    (hann + (bell - hann) * gauss) as f32
                })
            })
            .collect();
        Self {
            rate,
            seed,
            max_frames,
            sample: None,
            pool: Box::new([[Grain::default(); POOL]; MAX_VOICES]),
            live: [0; MAX_VOICES],
            streams: (0..MAX_VOICES)
                .map(|voice| Stream {
                    random: Random::seeded(seed ^ voice as u64),
                    next: f64::MAX,
                    scan: 0.0,
                })
                .collect(),
            ramps: vec![0.0; RAMPED.len() * max_frames],
            mode: CLASSIC,
            beats: 0.25,
            bpm: 120.0,
            grid: Vec::with_capacity(GRID),
            grid_on: false,
            env: EnvelopeBank::new(MAX_VOICES, EnvelopeParams::AMP, Timing::new(rate)),
            env_params: EnvelopeParams::AMP,
            env_out: vec![0.0; max_frames * ENV_STRIDE],
            filter,
            filter_on: false,
            stride,
            audio: vec![0.0; max_frames * 2 * stride],
            cutoff: vec![0.0; max_frames * stride],
            res: vec![0.0; max_frames],
            zeros: vec![0.0; max_frames],
            bells,
            dropped: 0,
            peaks: [0.0; PEAKS],
            scanned: 0,
            scanning: 0,
            length: 0.0,
        }
    }

    fn ramp(&self, which: usize, frame: usize) -> f32 {
        self.ramps[which * self.max_frames + frame]
    }

    /// Frees every grain and voice, as a switched-off device does.
    fn silence(&mut self) {
        self.live = [0; MAX_VOICES];
        for voice in 0..MAX_VOICES {
            self.env.reset(voice);
        }
    }

    /// Reads the stretch's parameters, the sample and, for Loop under a
    /// playing song, the grid's onsets.
    fn snapshot(
        &mut self,
        audio: &Audio<'_, '_>,
        params: &ParamBank,
        sample: Option<Arc<SampleData>>,
    ) {
        let frames = audio.frames();
        let at = |id: ParamId| params.index(id).expect("the description holds it");
        for (n, id) in RAMPED.iter().enumerate() {
            let to = n * self.max_frames;
            self.ramps[to..to + frames].copy_from_slice(params.ramp(at(*id)));
        }
        self.mode = (params.value(at(MODE)) as usize).min(MODES.len() - 1);
        self.beats = RATE_BEATS[(params.value(at(RATE)) as usize).min(RATES.len() - 1)];
        self.bpm = audio.bpm();
        self.filter_on = params.value(at(FILTER)) >= 0.5;
        let envelope = EnvelopeParams {
            unit: TimeUnit::Milliseconds,
            delay: 0.0,
            attack: params.value(at(ATTACK)) as f32,
            hold: 0.0,
            decay: params.value(at(DECAY)) as f32,
            sustain: (params.value(at(SUSTAIN)) / 100.0) as f32,
            release: params.value(at(RELEASE)) as f32,
            attack_curve: 0.5,
            decay_curve: 0.666,
            release_curve: 0.666,
            start: 0.0,
            end: 0.0,
            legato_inverted: false,
            steal_restart: StealRestart::CurrentLevel,
        };
        if envelope != self.env_params {
            self.env_params = envelope;
            self.env.set_all_params(envelope);
        }
        self.audio[..frames * 2 * self.stride].fill(0.0);
        self.grid.clear();
        self.grid_on = self.mode == LOOP && audio.playing();
        if self.grid_on {
            self.grid_onsets(audio, frames);
        }
        if let Some(source) = &sample {
            self.scan_peaks(source);
        }
        self.sample = sample;
    }

    /// Spec: granulator iii R8. The frames of the stretch the song's grid of
    /// Rate falls on: each grid line's song frame, placed by the tempo map,
    /// and so the same at any block size.
    fn grid_onsets(&mut self, audio: &Audio<'_, '_>, frames: usize) {
        let (tempo, rate) = (audio.tempo(), audio.sample_rate());
        let line = (self.beats * TICKS_PER_QUARTER as f64) as i64;
        let (from, to) = (audio.song_frame(0), audio.song_frame(frames));
        let frame_of = |n: i64| tempo.ticks_to_frames(Ticks(n * line), rate).get();
        let mut n = tempo
            .frames_to_ticks(Frames(from), rate)
            .get()
            .div_euclid(line);
        while n > 0 && frame_of(n - 1) >= from {
            n -= 1;
        }
        while frame_of(n) < from {
            n += 1;
        }
        while frame_of(n) < to && self.grid.len() < GRID {
            let offset = (frame_of(n) - from) as f64 / audio.speed().max(f64::MIN_POSITIVE);
            self.grid.push((offset as usize).min(frames - 1) as u32);
            n += 1;
        }
    }

    /// Spec: granulator iii R14. Takes the peaks of the next stretch of the
    /// source, so a long sample never lands on one block.
    fn scan_peaks(&mut self, source: &Arc<SampleData>) {
        let key = Arc::as_ptr(source) as usize;
        if key != self.scanning {
            self.scanning = key;
            self.scanned = 0;
            self.peaks = [0.0; PEAKS];
        }
        let len = source.frames();
        self.length = len as f64;
        let end = (self.scanned + SCAN_BUDGET).min(len);
        for channel in source.channels().take(2) {
            for (frame, value) in channel[self.scanned..end].iter().enumerate() {
                let bin = (self.scanned + frame) * PEAKS / len;
                self.peaks[bin] = self.peaks[bin].max(value.abs());
            }
        }
        self.scanned = end;
    }

    /// The filter, then the voices summed into the channels at Volume (R11).
    fn finish(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        self.sample = None;
        let frames = audio.frames();
        let stride = self.stride;
        if self.filter_on {
            let at = |id: ParamId| params.index(id).expect("the description holds it");
            self.filter.set_setup(Setup::new(
                params.value(at(FILTER_TYPE)) as usize,
                params.value(at(FILTER_SLOPE)) >= 0.5,
                CIRCUIT_AT[(params.value(at(FILTER_CIRCUIT)) as usize).min(CIRCUITS.len() - 1)],
            ));
            for frame in 0..frames {
                let cutoff = pitch_of(self.ramp(R_FREQ, frame), REFERENCE_HZ);
                self.cutoff[frame * stride..(frame + 1) * stride].fill(cutoff);
                self.res[frame] = self.ramp(R_RES, frame) / 100.0;
            }
            let controls = PerFrame {
                cutoff: &self.cutoff[..frames * stride],
                res: Per::Shared(&self.res[..frames]),
                morph: Per::Shared(&self.zeros[..frames]),
                drive: Per::Shared(&self.zeros[..frames]),
            };
            self.filter.process(
                MAX_VOICES,
                frames,
                &controls,
                &mut self.audio[..frames * 2 * stride],
            );
        }
        let mut channels = audio.channels_mut();
        let Some(left) = channels.next() else {
            return;
        };
        let mut right = channels.next();
        for frame in 0..frames {
            let volume = self.ramp(R_VOLUME, frame);
            let at = frame * 2 * stride;
            let l: f32 = self.audio[at..at + MAX_VOICES].iter().sum();
            let r: f32 = self.audio[at + stride..at + stride + MAX_VOICES]
                .iter()
                .sum();
            match &mut right {
                Some(right) => {
                    left[frame] = l * volume;
                    right[frame] = r * volume;
                }
                None => left[frame] = (l + r) * 0.5 * volume,
            }
        }
    }

    /// Writes the display data: the grains dropped, the sounding grains'
    /// positions and the source's peaks (R14).
    fn publish(&self, out: &mut [f32]) {
        out.fill(0.0);
        out[0] = self.dropped as f32;
        let length = self.length;
        out[2] = (length / self.rate) as f32;
        let mut shown = 0;
        'voices: for (voice, pool) in self.pool.iter().enumerate() {
            let mut live = self.live[voice];
            while live != 0 {
                if shown == POOL {
                    break 'voices;
                }
                let grain = &pool[live.trailing_zeros() as usize];
                live &= live - 1;
                out[3 + shown] = (grain.at / length.max(1.0)) as f32;
                shown += 1;
            }
        }
        out[1] = shown as f32;
        out[3 + POOL..].copy_from_slice(&self.peaks);
    }
}

impl Grains {
    /// Starts a grain for `voice` at frame `frame` of the stretch and works
    /// out when its stream's next one starts (R5 to R10). The generator
    /// draws four values for every grain, whatever the mode, so a mode
    /// change never shifts what the later grains draw.
    fn start_grain(
        &mut self,
        voice: usize,
        frame: usize,
        slide: f64,
        pressure: f64,
        source: &SampleData,
    ) {
        let ramps: [f64; RAMPED.len()] = core::array::from_fn(|k| f64::from(self.ramp(k, frame)));
        let ramp = |which: usize| ramps[which];
        let stream = &mut self.streams[voice];
        let draws = [
            stream.random.next_bipolar(),
            stream.random.next_bipolar(),
            stream.random.next_bipolar(),
            stream.random.next_bipolar(),
        ];
        let scan = stream.scan;
        // Spec: granulator iii R13. Pressure moves Size along its travel
        // and Shape along its own; Slide moves Position.
        let travel = libm::log(ramp(R_SIZE) / SIZE_MIN) / libm::log(SIZE_MAX / SIZE_MIN)
            + pressure * ramp(R_PRESS_SIZE) / 100.0;
        let size_ms = SIZE_MIN * libm::pow(SIZE_MAX / SIZE_MIN, travel.clamp(0.0, 1.0));
        let size = size_ms * self.rate / 1_000.0;
        let shape =
            (ramp(R_SHAPE) / 100.0 + pressure * ramp(R_PRESS_SHAPE) / 100.0).clamp(0.0, 1.0);
        let length = source.frames() as f64;
        let share = (ramp(R_POSITION) / 100.0
            + (slide - 0.5) * 2.0 * ramp(R_SLIDE) / 100.0 * SLIDE_REACH
            + scan)
            .rem_euclid(1.0);
        let at = (share * length + draws[0] * ramp(R_SPRAY) / 100.0 * self.rate)
            .clamp(0.0, length - 1.0);
        let density = ramp(R_DENSITY).max(1.0);
        let cloud = self.mode == CLOUD;
        let pan = if cloud {
            draws[2] * ramp(R_WIDTH) / 100.0
        } else {
            0.0
        };
        // Each mode's grains overlap by its own count: scale them so the
        // stream's level doesn't climb with Density.
        let level = match self.mode {
            CLASSIC => (2.0 / density).min(1.0),
            CLOUD => {
                1.0 / (size_ms / 1_000.0 * density * CLOUD_PER_DENSITY * 0.5)
                    .max(1.0)
                    .sqrt()
            }
            _ => 1.0,
        };
        let interval = match self.mode {
            CLASSIC => size / density,
            LOOP => self.beats * 60.0 / self.bpm * self.rate,
            _ => {
                -libm::log(1.0 - (draws[3] + 1.0) / 2.0) * self.rate / (density * CLOUD_PER_DENSITY)
            }
        };
        stream.next += interval.max(1.0);
        let live = self.live[voice];
        if live == u64::MAX {
            self.dropped += 1;
            return;
        }
        let slot = (!live).trailing_zeros() as usize;
        self.live[voice] |= 1 << slot;
        self.pool[voice][slot] = Grain {
            at,
            ratio: libm::exp2(draws[1] * ramp(R_VARIATION) / 12.0),
            age: 0,
            len: size.round().max(1.0) as u32,
            flat: (shape * 2.0).min(1.0) as f32,
            bell: ((shape - 0.5).max(0.0) * 2.0 * (BELLS - 1) as f64).round() as u8,
            gain: [
                (1.0 - pan.max(0.0)) as f32 * level as f32,
                (1.0 + pan.min(0.0)) as f32 * level as f32,
            ],
        };
    }

    /// Spec: granulator iii R5. A grain's window at its `age`: a flat window
    /// with 1 ms fades at Shape 0, the Hann window at 50 % and a bell
    /// narrowing towards 100 %.
    fn window(&self, grain: &Grain) -> f32 {
        let t = (grain.age as f32 + 0.5) / grain.len as f32;
        let at = t * TABLE as f32;
        let row = &self.bells[grain.bell as usize * (TABLE + 1)..][..=TABLE];
        let (i, f) = (at as usize, at.fract());
        let bell = row[i] + (row[(i + 1).min(TABLE)] - row[i]) * f;
        let fade = ((self.rate * FADE_SECONDS) as f32)
            .min(grain.len as f32 / 2.0)
            .max(1.0);
        let edge = (grain.age as f32 + 0.5).min(grain.len as f32 - grain.age as f32 - 0.5);
        let flat = (edge / fade).min(1.0);
        // `flat` is 0 at Shape 0 and 1 from Shape 50 %: the window blends
        // from the flat one to the bell as it rises.
        flat * (1.0 - grain.flat) + bell * grain.flat
    }

    /// One grain's output at its position, read with linear interpolation;
    /// `None` once it runs off the end of the source.
    fn read(grain: &Grain, source: &SampleData) -> Option<[f32; 2]> {
        let index = grain.at as usize;
        if index + 1 >= source.frames() {
            return None;
        }
        let fraction = grain.at.fract() as f32;
        let at = |channel: usize| {
            let samples = source.channel(channel.min(source.channel_count() - 1))?;
            Some(samples[index] + (samples[index + 1] - samples[index]) * fraction)
        };
        Some([at(0)?, at(1)?])
    }

    /// One voice's frames: its stream's grains started where its mode says,
    /// each summed through its window, through the envelope, into the
    /// stretch's audio.
    fn render_voice(
        &mut self,
        voice: usize,
        voices: &VoiceBlock,
        state: &State,
        frames: &Range<usize>,
        source: &SampleData,
    ) {
        let sources = Sources::of(voices);
        let (slide, pressure) = (sources.slide(voice), sources.pressure(voice));
        let length = source.frames() as f64;
        let mut line = self
            .grid
            .partition_point(|&at| (at as usize) < frames.start);
        for i in 0..frames.len() {
            let frame = frames.start + i;
            let clock = state.clock(frame) as f64;
            let scan = f64::from(self.ramp(R_SCAN, frame)) / 100.0 / length;
            self.streams[voice].scan += scan;
            let due = if self.grid_on {
                // The stream's own clock waits, so a stopped transport
                // starts it from here and not from the note.
                self.streams[voice].next = clock + 1.0;
                let mut hit = false;
                while self.grid.get(line).is_some_and(|&at| at as usize == frame) {
                    hit = true;
                    line += 1;
                }
                hit
            } else {
                self.streams[voice].next <= clock
            };
            if due {
                self.start_grain(voice, frame, slide, pressure, source);
            }
            // The note's pitch with every bend, Transpose and Fine: the
            // sample sounds at its own pitch at C3.
            let pitch = state.pitch(voices, voice, frames, i) - MIDDLE_C
                + f64::from(self.ramp(R_FINE, frame)) / 100.0;
            let note = libm::exp2(pitch / 12.0);
            let envelope = self.env_out[i * ENV_STRIDE + voice];
            let mut sum = [0.0f32; 2];
            let mut live = self.live[voice];
            while live != 0 {
                let slot = live.trailing_zeros() as usize;
                live &= live - 1;
                let grain = self.pool[voice][slot];
                match Self::read(&grain, source) {
                    Some(out) if grain.age < grain.len => {
                        let window = self.window(&grain);
                        sum[0] += out[0] * window * grain.gain[0];
                        sum[1] += out[1] * window * grain.gain[1];
                        let grain = &mut self.pool[voice][slot];
                        grain.age += 1;
                        grain.at += note * grain.ratio;
                    }
                    _ => self.live[voice] &= !(1 << slot),
                }
            }
            let at = frame * 2 * self.stride + voice;
            self.audio[at] = sum[0] * envelope;
            self.audio[at + self.stride] = sum[1] * envelope;
        }
    }
}

impl Instrument for Grains {
    fn voice_event(&mut self, _voices: &mut VoiceBlock, state: &State, event: VoiceEvent) {
        let voice = event.voice;
        match event.kind {
            VoiceEventKind::Start { start, .. } => {
                // Spec: granulator iii R10. A note's stream starts afresh,
                // from its own frame with its generator reseeded; a legato
                // note carries the stream on.
                if !matches!(start, Start::Legato) {
                    self.streams[voice] = Stream {
                        random: Random::seeded(self.seed ^ ((voice as u64 + 1) << 32)),
                        next: state.clock(event.time as usize) as f64,
                        scan: 0.0,
                    };
                }
                match start {
                    Start::Fresh => {
                        self.live[voice] = 0;
                        self.env.trigger(voice, Trigger::New);
                        self.filter.reset_voice(voice, event.time as usize);
                    }
                    Start::Steal | Start::Retrigger => self.env.trigger(voice, Trigger::Retrigger),
                    Start::Legato => self.env.trigger(voice, Trigger::Legato),
                    Start::Join { leader } => self.env.join(voice, leader),
                }
            }
            VoiceEventKind::Release => self.env.release(voice),
            VoiceEventKind::End { .. } => {
                self.env.reset(voice);
                self.live[voice] = 0;
            }
        }
    }

    fn render(&mut self, voices: &mut VoiceBlock, state: &State, frames: Range<usize>) {
        let count = frames.len();
        if count == 0 {
            return;
        }
        self.env.process(&mut self.env_out[..count * ENV_STRIDE]);
        let source = self.sample.take();
        let mut mask = voices.active_mask();
        while mask != 0 {
            let voice = mask.trailing_zeros() as usize;
            mask &= mask - 1;
            if let Some(source) = &source {
                self.render_voice(voice, voices, state, &frames, source);
            } else {
                // Without a source the streams wait too, so a sample that
                // arrives under a held note doesn't get a burst of grains.
                self.streams[voice].next = state.clock(frames.end) as f64;
            }
        }
        self.sample = source;
        // A voice whose envelope has ended is silent.
        let mut mask = voices.active_mask();
        while mask != 0 {
            let voice = mask.trailing_zeros() as usize;
            mask &= mask - 1;
            if self.env.is_idle(voice) {
                voices.end(voice);
            }
        }
    }
}

/// What the display shows of Granulator III (R14): the source's waveform as
/// its peaks, Position and the Spray range, and a mark for every sounding
/// grain. `values` are the device's parameters and `data` what its node
/// published, if an engine runs.
#[must_use]
pub fn display(
    device: DeviceId,
    description: &DeviceDescription,
    values: &OrdMap<ParamId, f64>,
    data: Option<&[f32]>,
) -> DeviceDisplay {
    let value = |id: ParamId| description.value(values, id).unwrap_or(0.0);
    let data = data.filter(|d| d.len() == DISPLAY_LEN);
    let length = data.map_or(0.0, |d| f64::from(d[2]));
    let (dropped, shown) = data.map_or((0, 0), |d| (d[0] as u64, d[1] as usize));
    let waveform = DisplaySeries {
        name: "Waveform".to_owned(),
        unit: "".to_owned(),
        x_unit: "%".to_owned(),
        x_range: [0.0, 100.0],
        log_x: false,
        y_range: [0.0, 1.0],
        detail: false,
        values: data.map_or_else(Vec::new, |d| d[3 + POOL..].to_vec()),
    };
    let mut markers = Vec::new();
    let mut mark = |name: &str, x: f64, y: f32, text: String| {
        markers.push(DisplayMarker {
            name: name.to_owned(),
            series: "Waveform".to_owned(),
            x: x as f32,
            y,
            text,
            handle: None,
        });
    };
    let position = value(POSITION);
    mark(
        "Position",
        position,
        1.0,
        format!("Position {position:.0} %"),
    );
    if length > 0.0 {
        // Spray reaches a second either way, as a share of the source.
        let reach = value(SPRAY) / length;
        for (name, x) in [
            ("Spray Start", position - reach),
            ("Spray End", position + reach),
        ] {
            mark(
                name,
                x.clamp(0.0, 100.0),
                1.0,
                format!("{name} {:.0} %", x.clamp(0.0, 100.0)),
            );
        }
    }
    for n in 0..shown {
        let at = f64::from(data.map_or(0.0, |d| d[3 + n])) * 100.0;
        mark(
            &format!("Grain {}", n + 1),
            at,
            0.0,
            format!("Grain {} at {at:.0} %", n + 1),
        );
    }
    DeviceDisplay {
        device,
        live: data.is_some(),
        view: None,
        series: vec![waveform],
        spectrograms: Vec::new(),
        markers,
        readings: vec![
            DisplayReading {
                name: "Source Length".to_owned(),
                value: length as f32,
                unit: "s".to_owned(),
                text: format!("{length:.2} s"),
            },
            DisplayReading {
                name: "Dropped Grains".to_owned(),
                value: dropped as f32,
                unit: String::new(),
                text: dropped.to_string(),
            },
        ],
    }
}

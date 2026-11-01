//! Spectrum, the native frequency analyser (WP-5.28; the spectrum analyser spec):
//! real-time FFT measurement of incoming audio signals, Block sizes from 256
//! to 16,384, channel routing, averaging, peak hold (Max), linear/log/semitone
//! scaling, and range controls.
//!
//! Audio passes through untouched with zero latency. No allocation, lock or
//! blocking call occurs on the audio thread: all FFT buffers, twiddle factors
//! and windows are pre-allocated when the node is constructed.

use std::f64::consts::TAU;

use bl_engine::{DeviceType, NodeConfig, Registry};
use bl_model::device_layout::{BODY_HEIGHT, ControlBand, DisplayCell, LayoutNode, Widget};
use bl_model::{
    DeviceDescription, DeviceDisplay, DeviceId, DeviceLayout, DeviceRole, DisplaySeries, OrdMap,
    ParamDescriptor, ParamId, ParamUnit,
};

use crate::framework::{Audio, NativeDevice, ParamBank, add};
use crate::sections::fft;

/// Spectrum's device type.
pub const SPECTRUM: &str = "beatlayer.spectrum";

/// The block size chooser: 256 to 16,384 samples.
pub const BLOCK: ParamId = ParamId(0);
/// The channel chooser: Left, Right, or Both.
pub const CHANNEL: ParamId = ParamId(1);
/// Refresh interval in milliseconds.
pub const REFRESH: ParamId = ParamId(2);
/// Averaging count: 1 to 32 blocks.
pub const AVG: ParamId = ParamId(3);
/// Graph style: Line or Bins.
pub const GRAPH: ParamId = ParamId(4);
/// Max peak-hold toggle.
pub const MAX: ParamId = ParamId(5);
/// Scale X chooser: Linear, Logarithmic, Semitone.
pub const SCALE_X: ParamId = ParamId(6);
/// Range mode chooser: Range (manual) or Auto.
pub const RANGE_MODE: ParamId = ParamId(7);
/// Dynamic range minimum in dB.
pub const RANGE_MIN: ParamId = ParamId(8);
/// Dynamic range maximum in dB.
pub const RANGE_MAX: ParamId = ParamId(9);
/// Clear Max action trigger.
pub const CLEAR_MAX: ParamId = ParamId(10);

/// Supported FFT block sizes.
pub const BLOCK_SIZES: [usize; 7] = [256, 512, 1024, 2048, 4096, 8192, 16384];
/// Block size chooser labels.
pub const BLOCK_LABELS: [&str; 7] = ["256", "512", "1024", "2048", "4096", "8192", "16384"];
/// Channel options.
pub const CHANNELS: [&str; 3] = ["Left", "Right", "Both"];
/// Graph options.
pub const GRAPH_MODES: [&str; 2] = ["Line", "Bins"];
/// Scale X options.
pub const SCALE_X_MODES: [&str; 3] = ["Linear", "Logarithmic", "Semitone"];
/// Range mode options.
pub const RANGE_MODES: [&str; 2] = ["Range", "Auto"];

/// Maximum supported FFT size.
pub const MAX_BLOCK_SIZE: usize = 16384;
/// Maximum number of averaging history blocks.
pub const MAX_AVG_BLOCKS: usize = 32;
/// Number of display bins published to the shell / triple buffer.
pub const DISPLAY_BINS: usize = 256;
/// Floor level for silent bins.
pub const SPECTRUM_FLOOR_DB: f32 = -150.0;

/// Returns the descriptor and layout for Spectrum.
#[must_use]
pub fn spectrum_type() -> DeviceType {
    let params = vec![
        ParamDescriptor::choice(BLOCK, "Block", &BLOCK_LABELS, 3)
            .describe("The number of samples analyzed in each measurement."),
        ParamDescriptor::choice(CHANNEL, "Channel", &CHANNELS, 2)
            .describe("Which audio channel is analyzed: Left, Right, or Both."),
        ParamDescriptor::continuous(
            REFRESH,
            "Refresh",
            (10.0, 1000.0),
            50.0,
            ParamUnit::Milliseconds,
        )
        .describe("How often frequency analysis is performed."),
        ParamDescriptor::integer(AVG, "Avg", (1, 32), 1, ParamUnit::None)
            .describe("How many blocks of samples are averaged for each update."),
        ParamDescriptor::choice(GRAPH, "Graph", &GRAPH_MODES, 0)
            .describe("Displays the spectrum as an interpolated line or discrete bins."),
        ParamDescriptor::toggle(MAX, "Max", false)
            .describe("Retains the accumulated maximum amplitude on the display."),
        ParamDescriptor::choice(SCALE_X, "Scale X", &SCALE_X_MODES, 1)
            .describe("Scaling of the frequency display: Linear, Logarithmic, or Semitone."),
        ParamDescriptor::choice(RANGE_MODE, "Range", &RANGE_MODES, 0)
            .describe("Toggles between manual and automatic dynamic range scaling."),
        ParamDescriptor::continuous(
            RANGE_MIN,
            "Range Min",
            (-150.0, 0.0),
            -120.0,
            ParamUnit::Decibels,
        )
        .describe("Minimum amplitude displayed on the vertical axis."),
        ParamDescriptor::continuous(
            RANGE_MAX,
            "Range Max",
            (-60.0, 12.0),
            0.0,
            ParamUnit::Decibels,
        )
        .describe("Maximum amplitude displayed on the vertical axis."),
        ParamDescriptor::toggle(CLEAR_MAX, "Clear Max", false)
            .describe("Clears the accumulated peak-hold curve."),
    ];

    let layout = DeviceLayout::new(LayoutNode::Row(vec![
        LayoutNode::Band(ControlBand::stacked(
            BODY_HEIGHT,
            &[
                (BLOCK, Widget::Chooser),
                (CHANNEL, Widget::Chooser),
                (REFRESH, Widget::Knob),
                (AVG, Widget::Knob),
            ],
        )),
        LayoutNode::Cell(
            DisplayCell::new("Spectrum", 240.0, BODY_HEIGHT, 1.0).with_bottom(
                ControlBand::stacked(
                    40.0,
                    &[
                        (GRAPH, Widget::Chooser),
                        (MAX, Widget::Toggle),
                        (SCALE_X, Widget::Chooser),
                        (RANGE_MODE, Widget::Chooser),
                    ],
                ),
            ),
        ),
        LayoutNode::Band(ControlBand::stacked(
            BODY_HEIGHT,
            &[
                (RANGE_MIN, Widget::Knob),
                (RANGE_MAX, Widget::Knob),
                (CLEAR_MAX, Widget::Toggle),
            ],
        )),
    ]));

    DeviceType::native(SPECTRUM, "Spectrum", DeviceRole::AudioEffect).with_description(
        DeviceDescription::new(params)
            .with_layout(layout)
            .with_banks(&[
                (
                    "Display",
                    &[
                        "Block",
                        "Channel",
                        "Refresh",
                        "Avg",
                        "Graph",
                        "Max",
                        "Scale X",
                        "Clear Max",
                    ],
                ),
                ("Range", &["Range", "Range Min", "Range Max"]),
            ]),
    )
}

/// Adds Spectrum to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, spectrum_type(), |_, config| Spectrum::new(config));
}

/// Precomputed tables for a given FFT size.
struct FftPlan {
    size: usize,
    twiddle: Vec<(f32, f32)>,
    window: Vec<f32>,
    scale: f32,
}

impl FftPlan {
    fn new(size: usize) -> Self {
        let twiddle: Vec<(f32, f32)> = (0..size / 2)
            .map(|k| {
                let angle = -TAU * k as f64 / size as f64;
                (angle.cos() as f32, angle.sin() as f32)
            })
            .collect();
        let window: Vec<f32> = (0..size)
            .map(|n| {
                let x = n as f64 / size as f64;
                (0.5 - 0.5 * (TAU * x).cos()) as f32
            })
            .collect();
        let sum: f32 = window.iter().sum();
        let scale = if sum > 0.0 { 2.0 / sum } else { 0.0 };
        Self {
            size,
            twiddle,
            window,
            scale,
        }
    }
}

/// Spectrum analyser audio-thread processor.
pub struct Spectrum {
    rate: f64,
    plans: Vec<FftPlan>,
    ring_buffer: Vec<f32>,
    ring_pos: usize,
    samples_since_analysis: usize,
    re: Vec<f32>,
    im: Vec<f32>,
    current_bins: Vec<f32>,
    history_powers: Vec<Vec<f32>>,
    history_count: usize,
    history_head: usize,
    max_held: Vec<f32>,
    display_bins: Vec<f32>,
    current_block_size: usize,
    prev_clear_max: bool,
}

impl Spectrum {
    /// Constructs a new Spectrum processor for `config`.
    #[must_use]
    pub fn new(config: &NodeConfig) -> Self {
        let plans: Vec<FftPlan> = BLOCK_SIZES.iter().copied().map(FftPlan::new).collect();
        let max_half = MAX_BLOCK_SIZE / 2;
        let mut history_powers = Vec::with_capacity(MAX_AVG_BLOCKS);
        for _ in 0..MAX_AVG_BLOCKS {
            history_powers.push(vec![0.0f32; max_half]);
        }
        Self {
            rate: config.sample_rate.as_f64(),
            plans,
            ring_buffer: vec![0.0f32; MAX_BLOCK_SIZE],
            ring_pos: 0,
            samples_since_analysis: 0,
            re: vec![0.0f32; MAX_BLOCK_SIZE],
            im: vec![0.0f32; MAX_BLOCK_SIZE],
            current_bins: vec![SPECTRUM_FLOOR_DB; max_half],
            history_powers,
            history_count: 0,
            history_head: 0,
            max_held: vec![SPECTRUM_FLOOR_DB; max_half],
            display_bins: vec![SPECTRUM_FLOOR_DB; DISPLAY_BINS],
            current_block_size: BLOCK_SIZES[3], // 2048
            prev_clear_max: false,
        }
    }

    /// The latest measured spectrum bins (in dB, length = `block_size / 2`).
    #[must_use]
    pub fn bins(&self) -> &[f32] {
        let half = self.current_block_size / 2;
        &self.current_bins[..half]
    }

    /// Spacing between adjacent frequency bins in Hz (`rate / block_size`).
    #[must_use]
    pub fn bin_spacing(&self) -> f64 {
        self.rate / self.current_block_size as f64
    }

    /// Max-held peak bins (in dB, length = `block_size / 2`).
    #[must_use]
    pub fn max_held(&self) -> &[f32] {
        let half = self.current_block_size / 2;
        &self.max_held[..half]
    }

    /// Resets the peak-hold buffer.
    pub fn clear_max(&mut self) {
        self.max_held.fill(SPECTRUM_FLOOR_DB);
    }

    fn analyze(&mut self, block_idx: usize, avg_blocks: usize, max_enabled: bool) {
        let plan = &self.plans[block_idx];
        let n = plan.size;
        let half = n / 2;
        self.current_block_size = n;

        // Extract windowed samples from ring buffer ending at ring_pos
        for i in 0..n {
            let offset = (self.ring_pos + MAX_BLOCK_SIZE - n + i) % MAX_BLOCK_SIZE;
            self.re[i] = self.ring_buffer[offset] * plan.window[i];
            self.im[i] = 0.0;
        }

        fft(&mut self.re[..n], &mut self.im[..n], &plan.twiddle);

        // Store instantaneous power in history buffer
        let slot = &mut self.history_powers[self.history_head];
        for (bin, slot_val) in slot[..half].iter_mut().enumerate() {
            let amp = self.re[bin].hypot(self.im[bin]) * plan.scale;
            *slot_val = amp * amp;
        }
        self.history_head = (self.history_head + 1) % MAX_AVG_BLOCKS;
        self.history_count = (self.history_count + 1).min(avg_blocks);

        // Average over last avg_blocks measurements
        let count = self.history_count.min(avg_blocks).max(1);
        for bin in 0..half {
            let mut sum_power = 0.0f32;
            for c in 0..count {
                let idx = (self.history_head + MAX_AVG_BLOCKS - 1 - c) % MAX_AVG_BLOCKS;
                sum_power += self.history_powers[idx][bin];
            }
            let mean_amp = (sum_power / count as f32).sqrt();
            let db = if mean_amp > 1e-7 {
                20.0 * mean_amp.log10()
            } else {
                SPECTRUM_FLOOR_DB
            };
            self.current_bins[bin] = db;
            if max_enabled {
                if db > self.max_held[bin] {
                    self.max_held[bin] = db;
                }
            } else {
                self.max_held[bin] = db;
            }
        }

        // Resample/interpolate to DISPLAY_BINS for display triple-buffer
        for i in 0..DISPLAY_BINS {
            let frac = i as f64 / (DISPLAY_BINS - 1) as f64;
            let bin_f = frac * (half - 1) as f64;
            let bin_i = (bin_f as usize).min(half - 1);
            let bin_next = (bin_i + 1).min(half - 1);
            let rem = (bin_f - bin_i as f64) as f32;
            let db = self.current_bins[bin_i] * (1.0 - rem) + self.current_bins[bin_next] * rem;
            self.display_bins[i] = db;
        }
    }
}

impl NativeDevice for Spectrum {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        let block_idx = (params.value(BLOCK.0 as usize) as usize).min(BLOCK_SIZES.len() - 1);
        let channel_mode = params.value(CHANNEL.0 as usize) as usize;
        let refresh_ms = params.value(REFRESH.0 as usize).max(5.0);
        let avg_blocks = (params.value(AVG.0 as usize) as usize).clamp(1, MAX_AVG_BLOCKS);
        let max_enabled = params.value(MAX.0 as usize) >= 0.5;
        let clear_max = params.value(CLEAR_MAX.0 as usize) >= 0.5;

        if clear_max && !self.prev_clear_max {
            self.clear_max();
        }
        self.prev_clear_max = clear_max;

        let frames = audio.frames();
        let channels = audio.channels();

        // Accumulate input into ring buffer based on channel selection
        for i in 0..frames {
            let sample = match channel_mode {
                0 => audio.channel_mut(0)[i],
                1 => {
                    let ch = if channels > 1 { 1 } else { 0 };
                    audio.channel_mut(ch)[i]
                }
                _ => {
                    if channels >= 2 {
                        let l = audio.channel_mut(0)[i];
                        let r = audio.channel_mut(1)[i];
                        (l + r) * 0.5
                    } else {
                        audio.channel_mut(0)[i]
                    }
                }
            };
            self.ring_buffer[self.ring_pos] = sample;
            self.ring_pos = (self.ring_pos + 1) % MAX_BLOCK_SIZE;
        }

        self.samples_since_analysis += frames;
        let refresh_frames = ((refresh_ms / 1000.0) * self.rate).round() as usize;
        let target_block_size = BLOCK_SIZES[block_idx];
        self.current_block_size = target_block_size;

        let interval = refresh_frames.max(target_block_size / 4);
        if self.samples_since_analysis >= interval
            || (self.history_count == 0 && self.samples_since_analysis >= target_block_size)
        {
            self.samples_since_analysis = 0;
            self.analyze(block_idx, avg_blocks, max_enabled);
        }
        // Note: audio is unmodified! Bit-identical pass-through.
    }

    fn latency(&self) -> u32 {
        0
    }

    fn display_len(&self) -> usize {
        DISPLAY_BINS
    }

    fn display(&mut self, out: &mut [f32]) {
        out.copy_from_slice(&self.display_bins);
    }
}

/// Builds the device display for Spectrum from parameters and publication data.
#[must_use]
pub fn display(
    device: DeviceId,
    description: &DeviceDescription,
    values: &OrdMap<ParamId, f64>,
    data: Option<&[f32]>,
) -> DeviceDisplay {
    let value = |id: ParamId| description.value(values, id).unwrap_or(0.0);
    let scale_x = value(SCALE_X) as usize;
    let range_mode = value(RANGE_MODE) as usize;
    let mut range_min = value(RANGE_MIN) as f32;
    let mut range_max = value(RANGE_MAX) as f32;

    let (x_unit, log_x) = match scale_x {
        0 => ("Hz".to_owned(), false),
        2 => ("st".to_owned(), true),
        _ => ("Hz".to_owned(), true),
    };

    let mut series = Vec::new();
    let mut readings = Vec::new();

    if let Some(bins) = data {
        let peak_val = bins.iter().copied().reduce(f32::max);
        if range_mode == 1 {
            // Auto dynamic range mode (Spec: spectrum-analyser R10):
            // Range Max is rounded up to the nearest 6 dB step above the peak bin
            // (clamped to [-60 dB, +12 dB]), and Range Min is set 72 dB below Range Max
            // (minimum -150 dB). If no signal or peak is below -120 dB, defaults to [-120 dB, 0 dB].
            if let Some(peak) = peak_val.filter(|&p| p > -120.0) {
                range_max = ((peak / 6.0).ceil() * 6.0).clamp(-60.0, 12.0);
                range_min = (range_max - 72.0).max(-150.0);
            } else {
                range_max = 0.0;
                range_min = -120.0;
            }
        }
        series.push(DisplaySeries {
            name: "Spectrum".to_owned(),
            unit: "dB".to_owned(),
            x_unit,
            x_range: [20.0, 20_000.0],
            log_x,
            y_range: [range_min, range_max],
            detail: false,
            values: bins.to_vec(),
        });
        if let Some(peak) = peak_val {
            readings.push(bl_model::DisplayReading {
                name: "Peak".to_owned(),
                value: peak,
                unit: "dB".to_owned(),
                text: format!("{peak:.1} dB"),
            });
        }
    }

    DeviceDisplay {
        spectrograms: Vec::new(),
        device,
        live: data.is_some(),
        view: None,
        series,
        markers: Vec::new(),
        readings,
    }
}

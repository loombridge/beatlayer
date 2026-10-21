//! Oscilloscope, the native waveform display device (WP-5.28, IMP-31; the
//! oscilloscope spec): real-time waveform capture, rising-edge trigger,
//! time window, display gain, freeze, and grid divisions.
//!
//! Audio passes through untouched with zero latency. No allocation, lock or
//! blocking call occurs on the audio thread: all sample buffers are
//! pre-allocated when the node is constructed.

use bl_engine::{DeviceType, NodeConfig, Registry};
use bl_model::device_layout::{BODY_HEIGHT, ControlBand, DisplayCell, LayoutNode, Widget};
use bl_model::{
    DeviceDescription, DeviceDisplay, DeviceId, DeviceLayout, DeviceRole, DisplayReading,
    DisplaySeries, OrdMap, ParamDescriptor, ParamId, ParamUnit,
};

use crate::framework::{Audio, NativeDevice, ParamBank, add};

/// Oscilloscope's device type.
pub const OSCILLOSCOPE: &str = "beatlayer.oscilloscope";

/// Time window in milliseconds: 1.0 to 1000.0 ms.
pub const TIME: ParamId = ParamId(0);
/// Display gain: -36.0 to +36.0 dB.
pub const GAIN: ParamId = ParamId(1);
/// Freeze toggle.
pub const FREEZE: ParamId = ParamId(2);
/// Grid divisions: 1 to 16.
pub const DIVISIONS: ParamId = ParamId(3);
/// Rising-edge trigger level: -100% to +100%.
pub const TRIGGER: ParamId = ParamId(4);

/// Number of points in the published waveform display series.
pub const WAVEFORM_POINTS: usize = 256;
/// How much audio the ring holds, in seconds: a little over the longest Time
/// window (1 s), so the whole window fits at every rate the engine runs at.
const RING_SECONDS: f64 = 1.05;

/// Returns the descriptor and layout for Oscilloscope.
#[must_use]
pub fn oscilloscope_type() -> DeviceType {
    let params = vec![
        ParamDescriptor::continuous(TIME, "Time", (1.0, 1000.0), 50.0, ParamUnit::Milliseconds)
            .logarithmic()
            .describe("How much of the signal the display shows, in milliseconds."),
        ParamDescriptor::gain(GAIN, "Gain", 36.0, 0.0)
            .describe("Raises or lowers what the display shows, without changing the signal."),
        ParamDescriptor::toggle(FREEZE, "Freeze", false)
            .describe("Holds the display still while audio keeps going."),
        ParamDescriptor::integer(DIVISIONS, "Divisions", (1, 16), 4, ParamUnit::None)
            .describe("How many divisions the display's grid has."),
        ParamDescriptor::continuous(TRIGGER, "Trigger", (-100.0, 100.0), 0.0, ParamUnit::Percent)
            .describe("The level at which the display's window begins."),
    ];

    let layout = DeviceLayout::new(LayoutNode::Row(vec![
        LayoutNode::Band(ControlBand::stacked(
            BODY_HEIGHT,
            &[(TIME, Widget::Knob), (GAIN, Widget::Knob)],
        )),
        LayoutNode::Cell(
            DisplayCell::new("Waveform", 220.0, BODY_HEIGHT, 1.0)
                .with_bottom(ControlBand::stacked(40.0, &[(DIVISIONS, Widget::Number)])),
        ),
        LayoutNode::Band(ControlBand::stacked(
            BODY_HEIGHT,
            &[(FREEZE, Widget::Toggle), (TRIGGER, Widget::Knob)],
        )),
    ]));

    DeviceType::native(OSCILLOSCOPE, "Oscilloscope", DeviceRole::AudioEffect)
        .with_description(DeviceDescription::new(params).with_layout(layout))
}

/// Adds Oscilloscope to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, oscilloscope_type(), |_, config| {
        Oscilloscope::new(config)
    });
}

/// Oscilloscope audio-thread processor.
pub struct Oscilloscope {
    rate: f64,
    ring_len: usize,
    ring_buffer: Vec<f32>,
    ring_pos: usize,
    /// Samples written since the device was made; a sample's position in the
    /// ring is its number of samples written, modulo `ring_len`.
    written: u64,
    /// The sample the trigger fired on, while its window is still filling.
    pending: Option<u64>,
    captured: Vec<f32>,
    triggered: bool,
    display_buffer: [f32; WAVEFORM_POINTS],
}

impl Oscilloscope {
    /// Constructs a new Oscilloscope processor.
    #[must_use]
    pub fn new(config: &NodeConfig) -> Self {
        let ring_len = (config.sample_rate.as_f64() * RING_SECONDS).ceil() as usize + 2;
        Self {
            rate: config.sample_rate.as_f64(),
            ring_len,
            ring_buffer: vec![0.0f32; ring_len],
            ring_pos: 0,
            written: 0,
            pending: None,
            captured: vec![0.0f32; WAVEFORM_POINTS],
            triggered: false,
            display_buffer: [0.0f32; WAVEFORM_POINTS],
        }
    }

    /// The latest captured waveform points.
    #[must_use]
    pub fn captured(&self) -> &[f32] {
        &self.captured
    }

    /// Whether the trigger condition was met on the last capture.
    #[must_use]
    pub fn triggered(&self) -> bool {
        self.triggered
    }
}

impl NativeDevice for Oscilloscope {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        let time_ms = params.value(TIME.0 as usize);
        let gain_val = params.value(GAIN.0 as usize);
        let gain = if gain_val <= 0.0 {
            0.0
        } else {
            gain_val as f32
        };
        let freeze = params.value(FREEZE.0 as usize) >= 0.5;
        let trigger_pct = params.value(TRIGGER.0 as usize) as f32;
        let trigger_level = trigger_pct / 100.0;

        let frames = audio.frames();
        let channels = audio.channels();

        // Feed mono mix to ring buffer
        let start_pos = self.ring_pos;
        for i in 0..frames {
            let sample = if channels >= 2 {
                let l = audio.channel_mut(0)[i];
                let r = audio.channel_mut(1)[i];
                (l + r) * 0.5
            } else {
                audio.channel_mut(0)[i]
            };
            self.ring_buffer[self.ring_pos] = sample;
            self.ring_pos = (self.ring_pos + 1) % self.ring_len;
        }
        let block_start = self.written;
        self.written += frames as u64;

        if freeze {
            // The window waits for the next crossing once the display moves
            // again: the samples of a trigger older than the ring are gone.
            self.pending = None;
            return;
        }

        // Spec: oscilloscope R4. The window is the Time parameter, which the
        // ring holds in full at every rate.
        let window_samples = ((time_ms / 1000.0) * self.rate).round() as usize;
        let window_samples = window_samples.clamp(2, self.ring_len - 2);

        // Search the new frames for a rising crossing of the trigger level.
        if self.pending.is_none() {
            for i in 0..frames {
                let curr_idx = (start_pos + i) % self.ring_len;
                let prev_idx = (curr_idx + self.ring_len - 1) % self.ring_len;
                let s_prev = self.ring_buffer[prev_idx];
                let s_curr = self.ring_buffer[curr_idx];
                if s_prev <= trigger_level && s_curr > trigger_level {
                    self.pending = Some(block_start + i as u64);
                    break;
                }
            }
        }

        // Spec: oscilloscope R2, R3. The window is taken from the crossing
        // once the audio after it has arrived, so every point drawn is real.
        if let Some(at) = self.pending {
            if self.written >= at + window_samples as u64 {
                self.triggered = true;
                for k in 0..WAVEFORM_POINTS {
                    let frac = k as f64 / (WAVEFORM_POINTS - 1) as f64;
                    let offset = (frac * (window_samples - 1) as f64).round() as u64;
                    let sample_idx = ((at + offset) % self.ring_len as u64) as usize;
                    let val = (self.ring_buffer[sample_idx] * gain).clamp(-1.0, 1.0);
                    self.captured[k] = val;
                    self.display_buffer[k] = val;
                }
                self.pending = None;
            }
        } else if !self.triggered {
            // Free-run capture if never triggered
            let head_idx = (self.ring_pos + self.ring_len - window_samples) % self.ring_len;
            for k in 0..WAVEFORM_POINTS {
                let frac = k as f64 / (WAVEFORM_POINTS - 1) as f64;
                let offset = (frac * (window_samples - 1) as f64).round() as usize;
                let sample_idx = (head_idx + offset) % self.ring_len;
                let val = (self.ring_buffer[sample_idx] * gain).clamp(-1.0, 1.0);
                self.captured[k] = val;
                self.display_buffer[k] = val;
            }
        }
        // Audio is unmodified! Bit-identical pass-through.
    }

    fn latency(&self) -> u32 {
        0
    }

    fn display_len(&self) -> usize {
        WAVEFORM_POINTS
    }

    fn display(&mut self, out: &mut [f32]) {
        out.copy_from_slice(&self.display_buffer);
    }
}

/// Builds the device display for Oscilloscope from parameters and publication data.
#[must_use]
pub fn display(
    device: DeviceId,
    description: &DeviceDescription,
    values: &OrdMap<ParamId, f64>,
    data: Option<&[f32]>,
) -> DeviceDisplay {
    let value = |id: ParamId| description.value(values, id).unwrap_or(0.0);
    let time_ms = value(TIME) as f32;

    let mut series = Vec::new();
    let mut readings = Vec::new();

    if let Some(wave) = data {
        series.push(DisplaySeries {
            name: "Waveform".to_owned(),
            unit: "".to_owned(),
            x_unit: "ms".to_owned(),
            x_range: [0.0, time_ms],
            log_x: false,
            y_range: [-1.0, 1.0],
            detail: false,
            values: wave.to_vec(),
        });
        readings.push(DisplayReading {
            name: "Time".to_owned(),
            value: time_ms,
            unit: "ms".to_owned(),
            text: format!("{time_ms:.1} ms"),
        });
    }

    DeviceDisplay {
        device,
        live: data.is_some(),
        view: None,
        series,
        markers: Vec::new(),
        readings,
    }
}

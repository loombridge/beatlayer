//! Gate, the native gate (WP-5.21; the gate spec): Threshold and Return
//! (the hysteresis between opening and closing), Attack, Hold, Release,
//! Floor, Flip, Lookahead, and the sidechain section: the framework's
//! External, Source, Mix, SC Gain and Sidechain Listen, and the device's own
//! SC EQ.
//!
//! The detector hears the key (`Audio::sidechain`, or the device's own
//! input) through the SC EQ when it's on, as the larger of the two
//! channels' magnitudes. A key at or above Threshold opens the gate; one
//! below Threshold less Return for the Hold time closes it, and between the
//! two it stays as it is. The gate's opening moves from 0 to 1 over Attack
//! and back over Release, in a straight line, and the gain is Floor plus the
//! opening's share of the rest. The signal is delayed by the lookahead, so
//! the gate has begun to open when a peak comes out; that delay is the
//! device's latency (WP-2.11).
//!
//! Its ranges and defaults are Beatlayer's own; the manual gives none (see
//! `docs/specs/gate.md`).

use bl_engine::{DeviceType, NodeConfig, Registry};
use bl_model::Device;
use bl_model::device_layout::{BODY_HEIGHT, ControlBand, DisplayCell, LayoutNode, Widget};
use bl_model::param::db_to_gain;
use bl_model::{
    DeviceDescription, DeviceDisplay, DeviceId, DeviceLayout, DeviceRole, DisplayReading, OrdMap,
    ParamDescriptor, ParamId, ParamUnit,
};

use crate::dynamics::{LOOKAHEAD_SECONDS, SC_TYPES, ScFilter, level_db, lookahead_frames};
use crate::framework::{Audio, DelayLine, NativeDevice, ParamBank, add};
use crate::sections::REFRESH_FRAMES;

/// Gate's device type.
pub const GATE: &str = "beatlayer.shutter";

/// Threshold: the level that opens the gate, in dB.
pub const THRESHOLD: ParamId = ParamId(0);
/// Return: how far below Threshold the level must fall to close it, in dB.
pub const RETURN: ParamId = ParamId(1);
/// Attack: how long the gate takes to open, in ms.
pub const ATTACK: ParamId = ParamId(2);
/// Hold: how long the gate stays open after the level falls, in ms.
pub const HOLD: ParamId = ParamId(3);
/// Release: how long the gate takes to close, in ms.
pub const RELEASE: ParamId = ParamId(4);
/// Floor: the gain of a closed gate, reaching silence.
pub const FLOOR: ParamId = ParamId(5);
/// Flip: the gate passes the signal only while it is below the threshold.
pub const FLIP: ParamId = ParamId(6);
/// Look: zero, one, or ten milliseconds of lookahead.
pub const LOOKAHEAD: ParamId = ParamId(7);
/// SC EQ: whether only a band of the detector's signal triggers it.
pub const SC_EQ: ParamId = ParamId(8);
/// Type: the SC EQ's shape.
pub const SC_TYPE: ParamId = ParamId(9);
/// Freq: the SC EQ's cutoff or center frequency, in Hz.
pub const SC_FREQ: ParamId = ParamId(10);
/// Res: the SC EQ's resonance.
pub const SC_RES: ParamId = ParamId(11);

/// What the node publishes: the gain now, and the input's and the output's
/// peaks since the last block, all in dB.
const DISPLAY_VALUES: usize = 3;

/// Gate's device type.
#[must_use]
pub fn gate_type() -> DeviceType {
    let params = vec![
        ParamDescriptor::continuous(
            THRESHOLD,
            "Threshold",
            (-80.0, 0.0),
            -30.0,
            ParamUnit::Decibels,
        )
        .describe("The level that opens the gate."),
        ParamDescriptor::continuous(RETURN, "Return", (0.0, 24.0), 6.0, ParamUnit::Decibels)
            .describe(
                "How far below the threshold the level must fall before the gate starts to \
                 close; a larger gap stops it chattering.",
            ),
        ParamDescriptor::continuous(ATTACK, "Attack", (0.01, 50.0), 1.0, ParamUnit::Milliseconds)
            .logarithmic()
            .describe("How long the gate takes to open fully."),
        ParamDescriptor::continuous(HOLD, "Hold", (1.0, 2000.0), 20.0, ParamUnit::Milliseconds)
            .logarithmic()
            .describe("How long the gate stays open once the level has fallen."),
        ParamDescriptor::continuous(
            RELEASE,
            "Release",
            (1.0, 5000.0),
            100.0,
            ParamUnit::Milliseconds,
        )
        .logarithmic()
        .describe("How long the gate takes to close fully."),
        ParamDescriptor::gain(FLOOR, "Floor", 0.0, f64::NEG_INFINITY)
            .describe("The level of a closed gate: silence, or any level up to no change."),
        ParamDescriptor::toggle(FLIP, "Flip", false)
            .describe("Passes the signal only while it is below the threshold."),
        ParamDescriptor::choice(LOOKAHEAD, "Look", &["0 ms", "1 ms", "10 ms"], 0)
            .describe("How far ahead the detector sees a peak; it delays the output as long."),
        ParamDescriptor::toggle(SC_EQ, "SC EQ", false).describe(
            "Listens through a filter, so only a band of the detector's signal opens the gate.",
        ),
        ParamDescriptor::choice(SC_TYPE, "Type", &SC_TYPES, 0)
            .describe("The sidechain EQ's shape."),
        ParamDescriptor::continuous(SC_FREQ, "Freq", (20.0, 20_000.0), 500.0, ParamUnit::Hertz)
            .logarithmic()
            .describe("The sidechain EQ's cutoff or center frequency."),
        ParamDescriptor::continuous(SC_RES, "Res", (0.1, 10.0), 0.71, ParamUnit::None)
            .logarithmic()
            .describe("The sidechain EQ's resonance."),
    ];
    DeviceType::native(GATE, "Gate", DeviceRole::AudioEffect)
        .with_sidechain()
        .with_description(DeviceDescription::new(params).with_layout(layout()))
}

/// The detector's controls on the left, the display with the envelope's
/// controls along its bottom edge, and the SC EQ's on the right.
fn layout() -> DeviceLayout {
    DeviceLayout::new(LayoutNode::Row(vec![
        LayoutNode::Band(ControlBand::stacked(
            BODY_HEIGHT,
            &[
                (THRESHOLD, Widget::Knob),
                (RETURN, Widget::Knob),
                (FLIP, Widget::Toggle),
                (LOOKAHEAD, Widget::Chooser),
            ],
        )),
        LayoutNode::Cell(
            DisplayCell::new("Gate", 220.0, BODY_HEIGHT, 1.0).with_bottom(ControlBand::stacked(
                96.0,
                &[
                    (ATTACK, Widget::Knob),
                    (HOLD, Widget::Knob),
                    (RELEASE, Widget::Knob),
                    (FLOOR, Widget::Knob),
                ],
            )),
        ),
        LayoutNode::Band(ControlBand::stacked(
            BODY_HEIGHT,
            &[
                (SC_EQ, Widget::Toggle),
                (SC_TYPE, Widget::Chooser),
                (SC_FREQ, Widget::Knob),
                (SC_RES, Widget::Knob),
            ],
        )),
    ]))
}

/// Adds Gate to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, gate_type(), |device, config| {
        Shutter::new(device, config)
    });
}

/// Gate's audio-thread half.
pub struct Shutter {
    rate: f64,
    /// The most samples of lookahead any choice asks for.
    max_lookahead: usize,
    /// The lookahead in force, in samples: the output's delay.
    lookahead: u32,
    /// Each frame's gain.
    gains: Vec<f32>,
    delay: [DelayLine; 2],
    filter: [ScFilter; 2],
    /// Whether the detector's last word was "open", and the frames of hold
    /// left before it closes.
    detected: bool,
    hold_left: u32,
    /// The gate's opening, from 0 (closed) to 1 (open).
    opening: f32,
    /// The gain now.
    gain: f32,
    /// The loudest input and output samples since the display last
    /// published, as linear amplitude.
    peak_in: f32,
    peak_out: f32,
    /// What the continuous parameters came to at the last refresh, and the
    /// frames since the device was made, so the refreshes fall on the same
    /// frames at any block size.
    settings: Settings,
    tick: usize,
}

/// The continuous parameters' values the detector works from, taken every
/// [`REFRESH_FRAMES`] frames.
#[derive(Clone, Copy, Default)]
struct Settings {
    open_level: f32,
    close_level: f32,
    rise: f32,
    fall: f32,
    hold: u32,
}

impl Shutter {
    fn new(device: &Device, config: &NodeConfig) -> Self {
        let rate = config.sample_rate.as_f64();
        let max_lookahead =
            (LOOKAHEAD_SECONDS[LOOKAHEAD_SECONDS.len() - 1] * rate).ceil() as usize + 1;
        let choice = device.params.get(&LOOKAHEAD).copied().unwrap_or(0.0);
        Self {
            rate,
            max_lookahead,
            lookahead: lookahead_frames(choice, rate, max_lookahead) as u32,
            gains: vec![0.0; config.max_frames.max(1)],
            delay: [DelayLine::new(max_lookahead), DelayLine::new(max_lookahead)],
            filter: [ScFilter::new(), ScFilter::new()],
            detected: false,
            hold_left: 0,
            opening: 0.0,
            gain: 0.0,
            peak_in: 0.0,
            peak_out: 0.0,
            settings: Settings::default(),
            tick: 0,
        }
    }

    /// The gain in dB now (0 for an open gate), what a meter of the
    /// gate's reduction reads.
    #[must_use]
    pub fn gain_reduction_db(&self) -> f32 {
        level_db(f64::from(self.gain)) as f32
    }
}

/// Frames for a time in milliseconds, at least one.
fn frames_of(ms: f64, rate: f64) -> f32 {
    (ms * 0.001 * rate).max(1.0) as f32
}

impl NativeDevice for Shutter {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        let frames = audio.frames();
        let channels = audio.channels().min(2);
        if frames == 0 || channels == 0 {
            return;
        }
        let at = |id| params.index(id).expect("the description holds it");
        let ramp = |id| params.ramp(at(id));
        let (threshold, hysteresis) = (ramp(THRESHOLD), ramp(RETURN));
        let (attack, hold, release) = (ramp(ATTACK), ramp(HOLD), ramp(RELEASE));
        let floor = ramp(FLOOR);
        let flip = params.value(at(FLIP)) >= 0.5;
        let (sc_freq, sc_res) = (ramp(SC_FREQ), ramp(SC_RES));
        let sc_kind = params.value(at(SC_TYPE)) as usize;
        let lookahead =
            lookahead_frames(params.value(at(LOOKAHEAD)), self.rate, self.max_lookahead);
        self.lookahead = lookahead as u32;
        let sc_on = params.value(at(SC_EQ)) >= 0.5;

        // 1. The detector and the gate's opening, frame by frame.
        for i in 0..frames {
            if self.tick.is_multiple_of(REFRESH_FRAMES) {
                let (open, closing) = (f64::from(threshold[i]), f64::from(hysteresis[i]));
                self.settings = Settings {
                    open_level: db_to_gain(open) as f32,
                    close_level: db_to_gain(open - closing) as f32,
                    rise: 1.0 / frames_of(f64::from(attack[i]), self.rate),
                    fall: 1.0 / frames_of(f64::from(release[i]), self.rate),
                    hold: frames_of(f64::from(hold[i]), self.rate) as u32,
                };
                for filter in &mut self.filter {
                    filter.set(
                        sc_kind,
                        f64::from(sc_freq[i]),
                        f64::from(sc_res[i]),
                        self.rate,
                    );
                }
            }
            self.tick += 1;
            let Settings {
                open_level,
                close_level,
                rise,
                fall,
                hold,
            } = self.settings;
            let mut key = 0.0f32;
            for channel in 0..channels {
                let keyed = audio.sidechain(channel).map(|key| key[i]);
                let sample = keyed.unwrap_or_else(|| audio.channel_mut(channel)[i]);
                self.peak_in = self.peak_in.max(sample.abs());
                let heard = if sc_on {
                    self.filter[channel].process(sample)
                } else {
                    sample
                };
                key = key.max(heard.abs());
            }
            if key >= open_level {
                self.detected = true;
                self.hold_left = hold;
            } else if self.detected {
                if key >= close_level {
                    self.hold_left = hold;
                } else if self.hold_left > 0 {
                    self.hold_left -= 1;
                } else {
                    self.detected = false;
                }
            }
            self.opening = if self.detected != flip {
                (self.opening + rise).min(1.0)
            } else {
                (self.opening - fall).max(0.0)
            };
            self.gain = floor[i] + (1.0 - floor[i]) * self.opening;
            self.gains[i] = self.gain;
        }

        // 2. The signal from the lookahead ago, through the gain.
        for channel in 0..channels {
            for i in 0..frames {
                let sample = audio.channel_mut(channel)[i];
                let delayed = if lookahead == 0 {
                    sample
                } else {
                    self.delay[channel].read(lookahead)
                };
                self.delay[channel].write(sample);
                let output = delayed * self.gains[i];
                self.peak_out = self.peak_out.max(output.abs());
                audio.channel_mut(channel)[i] = output;
            }
        }
    }

    fn latency(&self) -> u32 {
        self.lookahead
    }

    fn display_len(&self) -> usize {
        DISPLAY_VALUES
    }

    fn display(&mut self, out: &mut [f32]) {
        let db = |level: f32| level_db(f64::from(level)) as f32;
        out[0] = self.gain_reduction_db();
        out[1] = db(self.peak_in);
        out[2] = db(self.peak_out);
        self.peak_in = 0.0;
        self.peak_out = 0.0;
    }
}

/// The device's display (the gate spec): Threshold and Return as readings,
/// and, once an engine publishes them, the gain reduction and the input's and
/// output's levels. `data` is what the node published, if an engine runs.
#[must_use]
pub fn display(
    device: DeviceId,
    description: &DeviceDescription,
    values: &OrdMap<ParamId, f64>,
    data: Option<&[f32]>,
) -> DeviceDisplay {
    let value = |id: ParamId| description.value(values, id).unwrap_or(0.0);
    let reading = |name: &str, level: f64| DisplayReading {
        name: name.to_owned(),
        value: level as f32,
        unit: "dB".to_owned(),
        text: format!("{level:.1} dB"),
    };
    let mut readings = vec![
        reading("Threshold", value(THRESHOLD)),
        reading("Return", value(RETURN)),
    ];
    if let Some([gain, input, output]) =
        data.and_then(|d| <[f32; DISPLAY_VALUES]>::try_from(d).ok())
    {
        readings.extend([
            reading("Gain Reduction", f64::from(gain)),
            reading("Input", f64::from(input)),
            reading("Output", f64::from(output)),
        ]);
    }
    DeviceDisplay {
        device,
        live: data.is_some(),
        view: None,
        series: Vec::new(),
        markers: Vec::new(),
        readings,
    }
}

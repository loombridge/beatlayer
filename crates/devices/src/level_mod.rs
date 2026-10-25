//! The Envelope Follower (WP-5.11; the modulation spec, R24): Beatlayer's equivalent
//! of an Envelope Follower device. An audio effect that passes its audio
//! through untouched and turns the level of the signal it hears into a
//! control signal for the parameters it maps.
//!
//! It hears its track's signal at its place in the chain or, with its
//! sidechain section on, the framework's sidechain: the source track, chosen
//! by Audio From and its tap, through the sidechain's gain, mixed with the
//! track's own signal by Mix (0 % the track's own, 100 % the sidechain's).
//!
//! The detector is Beatlayer's own: the peak of the heard signal across its
//! channels, through Gain, followed by a one-pole smoother whose time
//! constant is Rise while the level climbs and Fall while it drops (0 ms
//! follows at once); the output is that level, from 0 to 1 at full scale,
//! delayed by Delay. Direct and S.C. show the level of the track's own
//! signal and of what the sidechain hands the detector, in dB.

use bl_engine::{DeviceType, Registry};
use bl_model::device_layout::{
    BODY_HEIGHT, ControlBand, DeviceLayout, DisplayCell, LayoutNode, Widget,
};
use bl_model::modulator::{LEVEL_MOD, SLOTS};
use bl_model::param::gain_to_db;
use bl_model::{
    DeviceDescription, DeviceDisplay, DeviceId, DeviceRole, DisplayReading, OrdMap,
    ParamDescriptor, ParamId, ParamUnit,
};

use crate::framework::{Audio, DelayLine, Modulation, NativeDevice, ParamBank, add};
use crate::modulator::{DIVISIONS, mapping_cells, mapping_params};
use crate::signal_mod::{Indices, SignalOut};

/// Gain: the input's gain before the detector.
pub const GAIN: ParamId = ParamId(0);
/// Rise: the time the output takes to follow a rising level.
pub const RISE: ParamId = ParamId(1);
/// Fall: the time the output takes to follow a falling level.
pub const FALL: ParamId = ParamId(2);
/// The Delay mode switch: a time or a beat division.
pub const DELAY_MODE: ParamId = ParamId(3);
/// Delay, as a time.
pub const DELAY_TIME: ParamId = ParamId(4);
/// Delay, as a beat division of the song's tempo.
pub const DELAY_BEATS: ParamId = ParamId(5);

/// The longest delay, in seconds.
const MAX_DELAY_SECONDS: f64 = 5.0;
/// The floor of the meters, in dB.
const FLOOR_DB: f32 = -60.0;
/// The values the node publishes: Direct and S.C. in dB, and the level.
const DISPLAY_VALUES: usize = 3;

/// The Envelope Follower's type.
#[must_use]
pub fn level_mod_type() -> DeviceType {
    let names: Vec<&str> = DIVISIONS.iter().map(|d| d.0).collect();
    let mut params = vec![
        ParamDescriptor::gain(GAIN, "Gain", 40.0, 0.0)
            .describe("The gain of the signal the detector hears, down to silence."),
        ParamDescriptor::continuous(RISE, "Rise", (0.0, 1_000.0), 5.0, ParamUnit::Milliseconds)
            .decimals(1)
            .describe("How slowly the output follows a rising level; 0 follows at once."),
        ParamDescriptor::continuous(FALL, "Fall", (0.0, 5_000.0), 100.0, ParamUnit::Milliseconds)
            .decimals(1)
            .describe("How slowly the output follows a falling level; 0 follows at once."),
        ParamDescriptor::choice(DELAY_MODE, "Delay Mode", &["Time", "Beats"], 0)
            .describe("Sets the delay as a time or as a beat division of the song's tempo."),
        ParamDescriptor::continuous(
            DELAY_TIME,
            "Delay",
            (0.0, 1_000.0),
            0.0,
            ParamUnit::Milliseconds,
        )
        .decimals(1)
        .describe("Delays the output by this time."),
        ParamDescriptor::choice(DELAY_BEATS, "Delay Division", &names, 1)
            .describe("Delays the output by this beat division of the song's tempo."),
    ];
    params.extend(mapping_params(
        SLOTS,
        (
            "Depth",
            "How far the modulator reaches: 0 % none, 100 % full.",
        ),
    ));
    DeviceType::native(LEVEL_MOD, "Envelope Follower", DeviceRole::AudioEffect)
        .with_sidechain()
        .with_description(DeviceDescription::new(params).with_layout(layout()))
}

fn layout() -> DeviceLayout {
    let input = ControlBand::stacked(
        BODY_HEIGHT,
        &[
            (GAIN, Widget::Knob),
            (RISE, Widget::Knob),
            (FALL, Widget::Knob),
            (DELAY_MODE, Widget::Chooser),
            (DELAY_TIME, Widget::Knob),
            (DELAY_BEATS, Widget::Chooser),
        ],
    );
    let output = ControlBand::stacked(
        BODY_HEIGHT,
        &[
            (bl_model::modulator::MODE, Widget::Chooser),
            (bl_model::modulator::POLARITY, Widget::Chooser),
            (bl_model::modulator::DEPTH, Widget::Knob),
        ],
    );
    let (first, rest) = mapping_cells(SLOTS);
    DeviceLayout::new(LayoutNode::Row(vec![
        LayoutNode::Band(input),
        LayoutNode::Cell(DisplayCell::new("Level", 110.0, BODY_HEIGHT, 1.0)),
        first,
        LayoutNode::Band(output),
    ]))
    .with_expanded(rest)
}

/// Adds the Envelope Follower to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, level_mod_type(), |_, config| {
        LevelMod::new(config.sample_rate.as_f64(), config.max_frames)
    });
}

struct LevelMod {
    out: SignalOut,
    rate: f64,
    /// The detector's level.
    level: f64,
    /// The output's history, for Delay.
    delay: DelayLine,
    /// The track's own peak and what the detector hears, per frame of the
    /// stretch.
    direct: Vec<f32>,
    heard: Vec<f32>,
    /// The loudest samples since the display last read them.
    peak_direct: f32,
    peak_heard: f32,
}

impl LevelMod {
    fn new(rate: f64, max_frames: usize) -> Self {
        Self {
            out: SignalOut::new(),
            rate,
            level: 0.0,
            delay: DelayLine::new((MAX_DELAY_SECONDS * rate) as usize + 1),
            direct: vec![0.0; max_frames],
            heard: vec![0.0; max_frames],
            peak_direct: 0.0,
            peak_heard: 0.0,
        }
    }

    /// The one-pole coefficient of a time constant of `ms` milliseconds.
    fn coefficient(&self, ms: f64) -> f64 {
        if ms <= 0.0 {
            1.0
        } else {
            1.0 - (-1000.0 / (ms * self.rate)).exp()
        }
    }
}

impl NativeDevice for LevelMod {
    fn is_signal_modulator(&self) -> bool {
        true
    }

    fn lag(&mut self, modulation: &mut Modulation<'_, '_>) {
        self.out.lag(modulation);
    }

    fn release(&mut self, _: &mut bl_engine::NoteBuffer) {
        self.out.off();
    }

    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        let frames = audio.frames();
        let channels = audio.channels();
        if self.out.begin(audio) {
            // Spec: modulation R20. It begins as a render from here does.
            self.level = 0.0;
            self.delay.clear();
        }
        for note in audio.notes() {
            audio.send(note);
        }

        // What the track carries and what the detector hears, as peaks.
        self.direct[..frames].fill(0.0);
        for c in 0..channels {
            for (peak, &x) in self.direct.iter_mut().zip(audio.channel_mut(c).iter()) {
                *peak = peak.max(x.abs());
            }
        }
        if audio.sidechain(0).is_some() {
            self.heard[..frames].fill(0.0);
            for c in 0..channels {
                if let Some(key) = audio.sidechain(c) {
                    for (peak, &x) in self.heard.iter_mut().zip(key) {
                        *peak = peak.max(x.abs());
                    }
                }
            }
        } else {
            self.heard[..frames].copy_from_slice(&self.direct[..frames]);
        }
        for i in 0..frames {
            self.peak_direct = self.peak_direct.max(self.direct[i]);
            self.peak_heard = self.peak_heard.max(self.heard[i]);
        }

        let index = |id: ParamId| params.index(id).expect("the description holds it");
        let (gain, rise, fall) = (
            params.ramp(index(GAIN)),
            params.ramp(index(RISE)),
            params.ramp(index(FALL)),
        );
        let bpm = audio.bpm();
        let (rate, max_delay) = (self.rate, self.delay_len());
        let delay_frames = |f: usize| -> usize {
            let seconds = if params.ramp(index(DELAY_MODE))[f] >= 0.5 {
                let division = params.ramp(index(DELAY_BEATS))[f] as usize;
                DIVISIONS[division.min(DIVISIONS.len() - 1)].1 * 60.0 / bpm
            } else {
                f64::from(params.ramp(index(DELAY_TIME))[f]) / 1000.0
            };
            ((seconds * rate) as usize).min(max_delay)
        };
        let ix = Indices::new(params, SLOTS);
        for f in 0..frames {
            // Spec: modulation R24. Peak, Gain, then Rise or Fall.
            let x = f64::from(self.heard[f]) * f64::from(gain[f]);
            let coefficient =
                self.coefficient(f64::from(if x > self.level { rise[f] } else { fall[f] }));
            self.level += (x - self.level) * coefficient;
            let level = self.level.min(1.0);
            self.delay.write(level as f32);
            let delay = delay_frames(f);
            let delayed = if delay == 0 {
                level
            } else {
                f64::from(self.delay.read(delay + 1))
            };
            self.out.frame(audio, &ix, params, f, &[delayed]);
        }
    }

    fn display_len(&self) -> usize {
        DISPLAY_VALUES
    }

    fn display(&mut self, out: &mut [f32]) {
        let db = |peak: f32| gain_to_db(f64::from(peak)).max(f64::from(FLOOR_DB)) as f32;
        out[0] = db(self.peak_direct);
        out[1] = db(self.peak_heard);
        out[2] = self.level.min(1.0) as f32;
        self.peak_direct = 0.0;
        self.peak_heard = 0.0;
    }
}

impl LevelMod {
    /// The most frames the delay can reach back.
    fn delay_len(&self) -> usize {
        (MAX_DELAY_SECONDS * self.rate) as usize
    }
}

/// The Envelope Follower's display (R24): Direct and S.C. in dB, and the level it
/// outputs as a share, from what its node published.
#[must_use]
pub fn display(
    device: DeviceId,
    _description: &DeviceDescription,
    _values: &OrdMap<ParamId, f64>,
    data: Option<&[f32]>,
) -> DeviceDisplay {
    let mut out = DeviceDisplay {
        device,
        live: data.is_some(),
        view: None,
        series: Vec::new(),
        markers: Vec::new(),
        readings: Vec::new(),
    };
    if let Some([direct, heard, level]) =
        data.and_then(|d| <[f32; DISPLAY_VALUES]>::try_from(d).ok())
    {
        let db = |name: &str, value: f32| DisplayReading {
            name: name.to_owned(),
            value,
            unit: "dB".to_owned(),
            text: format!("{value:.1} dB"),
        };
        out.readings = vec![
            db("Direct", direct),
            db("S.C.", heard),
            DisplayReading {
                name: "Level".to_owned(),
                value: level * 100.0,
                unit: "%".to_owned(),
                text: format!("{:.0} %", level * 100.0),
            },
        ];
    }
    out
}

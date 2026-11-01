//! DS Tom (WP-8.24; the drum synths spec, R13): a sine whose pitch bends
//! down, four resonant band-passes at a membrane's overtones, and a
//! low-pass. The manual gives the controls and no ranges or curves, so the
//! laws here are Beatlayer's own.

use std::f64::consts::TAU;

use bl_engine::{DeviceType, Registry};
use bl_model::ParamUnit;
use bl_model::device_layout::Widget;
use bl_model::{
    DeviceDescription, DeviceDisplay, DeviceId, DeviceRole, OrdMap, ParamDescriptor, ParamId,
};

use crate::drum_synth::{
    DECAY, DrumSynth, Filter, Hit, Pass, Preset, VOLUME, decay, layout, master_at, percent, volume,
};
use crate::framework::{ParamBank, add};

/// DS Tom's device type id.
pub const DS_TOM: &str = "beatlayer.ds_tom";

/// The tom's tuning in hertz.
pub const PITCH: ParamId = ParamId(2);
/// The low-pass's cutoff and gain.
pub const COLOR: ParamId = ParamId(3);
/// The level of the membrane's resonances.
pub const TONE: ParamId = ParamId(4);
/// How far above its pitch the sine starts.
pub const BEND: ParamId = ParamId(5);

/// The next four modes of a circular membrane, as multiples of the pitch
/// (the physical models spec, R1).
const MEMBRANE: [f64; 4] = [1.594, 2.136, 2.296, 2.653];
/// The resonances' Q and level each at Tone 100 %.
const RESONANCE_Q: f64 = 20.0;
const RESONANCE_LEVEL: f64 = 0.25;
/// What a unit impulse has to be, in units of the band-pass's rate over its
/// bandwidth, to ring at half scale.
const RING_PEAK: f64 = 0.5;
/// The sine's level.
const SINE_LEVEL: f64 = 0.5;
/// The low-pass's cutoff at Color 0, the factor Color 100 % raises it by
/// (1 kHz to 10 kHz) and its gain at Color 100 %, in decibels.
const COLOR_HZ: f64 = 1_000.0;
const COLOR_RANGE: f64 = 10.0;
const COLOR_GAIN_DB: f64 = 6.0;
/// The octaves the sine starts above its pitch at Bend 100 %.
const BEND_OCTAVES: f64 = 1.0;

/// The DS Tom device type.
#[must_use]
pub fn ds_tom_type() -> DeviceType {
    let params = vec![
        decay(500.0),
        volume(),
        ParamDescriptor::continuous(PITCH, "Pitch", (40.0, 400.0), 110.0, ParamUnit::Hertz)
            .logarithmic()
            .describe("The tom's tuning in hertz, whatever key plays it."),
        percent(
            COLOR,
            "Color",
            50.0,
            "Raises the low-pass's cutoff from 1 kHz to 10 kHz and its gain by up to 6 dB.",
        ),
        percent(
            TONE,
            "Tone",
            30.0,
            "The level of four resonant band-passes at a membrane's overtones.",
        ),
        percent(
            BEND,
            "Bend",
            50.0,
            "How far above its pitch the sine starts, up to an octave, falling to the pitch \
             over half of Decay.",
        ),
    ];
    DeviceType::native(DS_TOM, "DS Tom", DeviceRole::Instrument).with_description(
        DeviceDescription::new(params)
            .with_layout(layout(&[
                (PITCH, Widget::Knob),
                (COLOR, Widget::Knob),
                (TONE, Widget::Knob),
                (BEND, Widget::Knob),
                (DECAY, Widget::Knob),
                (VOLUME, Widget::Knob),
            ]))
            .with_banks(&[
                ("Tone", &["Pitch", "Color", "Tone", "Bend"]),
                ("Level", &["Decay", "Volume"]),
            ]),
    )
}

/// DS Tom's factory presets.
pub const PRESETS: [Preset; 3] = [
    Preset {
        name: "Floor",
        params: &[
            (PITCH, 80.0),
            (COLOR, 35.0),
            (TONE, 40.0),
            (BEND, 40.0),
            (DECAY, 700.0),
        ],
    },
    Preset {
        name: "Rack",
        params: &[
            (PITCH, 160.0),
            (COLOR, 60.0),
            (TONE, 30.0),
            (BEND, 60.0),
            (DECAY, 450.0),
        ],
    },
    Preset {
        name: "Electro",
        params: &[
            (PITCH, 220.0),
            (COLOR, 85.0),
            (TONE, 0.0),
            (BEND, 100.0),
            (DECAY, 300.0),
        ],
    },
];

/// Adds DS Tom to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, ds_tom_type(), |_, config| {
        DrumSynth::new(0, config.sample_rate.as_f64(), || {
            Tom::new(config.sample_rate.as_f64())
        })
    });
}

/// DS Tom's display: the master envelope its sine follows.
#[must_use]
pub fn display(
    device: DeviceId,
    description: &DeviceDescription,
    values: &OrdMap<ParamId, f64>,
) -> DeviceDisplay {
    crate::drum_synth::display(device, description, values, |value, ms| {
        master_at(ms, value(DECAY))
    })
}

/// One tom hit.
struct Tom {
    rate: f64,
    phase: f64,
    age: u32,
    membrane: [Filter; 4],
    low_pass: Filter,
}

impl Tom {
    fn new(rate: f64) -> Self {
        Self {
            rate,
            phase: 0.0,
            age: 0,
            membrane: [Filter::default(); 4],
            low_pass: Filter::default(),
        }
    }
}

impl Hit for Tom {
    fn start(&mut self, _seed: u64) {
        self.phase = 0.0;
        self.age = 0;
        self.membrane = [Filter::default(); 4];
        self.low_pass.clear();
    }

    fn frame(&mut self, bank: &ParamBank, frame: usize, master: f32) -> (f32, f32) {
        let at = |id: ParamId| f64::from(bank.ramp(id.0 as usize)[frame]);
        let t = f64::from(self.age) / self.rate;
        let pitch = at(PITCH);
        let half_decay = at(DECAY) / 2000.0;
        // The sine starts Bend up and falls to its pitch over half of Decay.
        let octaves = BEND_OCTAVES * at(BEND) / 100.0 * (1.0 - t / half_decay).max(0.0);
        self.phase = (self.phase + pitch * 2f64.powf(octaves) / self.rate).fract();
        // The hit excites the resonances: one impulse, scaled so each rings
        // at the same level whatever its pitch.
        let mut rings = 0.0;
        for (filter, ratio) in self.membrane.iter_mut().zip(MEMBRANE) {
            let hz = ratio * pitch;
            let impulse = if self.age == 0 {
                RING_PEAK * RESONANCE_Q * self.rate / (TAU * hz)
            } else {
                0.0
            };
            rings += filter.tick(impulse, Pass::Band, hz, RESONANCE_Q, self.rate);
        }
        let sound =
            SINE_LEVEL * (TAU * self.phase).sin() + at(TONE) / 100.0 * RESONANCE_LEVEL * rings;
        let color = at(COLOR) / 100.0;
        let cutoff = COLOR_HZ * COLOR_RANGE.powf(color);
        let gain = 10f64.powf(COLOR_GAIN_DB * color / 20.0);
        let out = self
            .low_pass
            .tick(sound, Pass::Low, cutoff, 0.707, self.rate)
            * gain
            * f64::from(master);
        self.age = self.age.saturating_add(1);
        (out as f32, out as f32)
    }
}

//! DS Kick (WP-8.24; the drum synths spec, R11): a sine whose pitch falls
//! from above, with added harmonics, a soft clipper and a click. The manual
//! gives the controls and no ranges or curves, so the laws here are
//! Beatlayer's own.

use std::f64::consts::TAU;

use bl_dsp::random::Random;
use bl_engine::{DeviceType, Registry};
use bl_model::ParamUnit;
use bl_model::device_layout::Widget;
use bl_model::{
    DeviceDescription, DeviceDisplay, DeviceId, DeviceRole, OrdMap, ParamDescriptor, ParamId,
};

use crate::drum_synth::{
    DECAY, DrumSynth, Filter, Hit, Pass, Preset, decay, layout, master_at, percent, volume,
};
use crate::framework::{ParamBank, add};

/// DS Kick's device type id.
pub const DS_KICK: &str = "beatlayer.ds_kick";

/// The sine's pitch at rest, in hertz.
pub const PITCH: ParamId = ParamId(2);
/// How far above its pitch the sine starts, up to [`ENV_OCTAVES`].
pub const ENV: ParamId = ParamId(3);
/// How long the sine takes to fade in, up to [`ATTACK_MAX_MS`].
pub const ATTACK: ParamId = ParamId(4);
/// How much of the second and third harmonics is added.
pub const OT: ParamId = ParamId(5);
/// How hard the sound drives a soft clipper.
pub const DRIVE: ParamId = ParamId(6);
/// Whether a click is added.
pub const CLICK: ParamId = ParamId(7);

/// How many octaves above its pitch the sine starts at Env 100 %.
const ENV_OCTAVES: f64 = 4.0;
/// The pitch falls to its rest with this time constant, in seconds.
const SWEEP_TAU: f64 = 0.030;
/// The longest Attack, in milliseconds.
const ATTACK_MAX_MS: f64 = 10.0;
/// The second and third harmonics' levels at OT 100 %: -6 dB and -12 dB.
const HARMONIC_2: f64 = 0.5;
const HARMONIC_3: f64 = 0.25;
/// The soft clipper's gain at Drive 100 %, in decibels, and how much
/// quieter its output is at that Drive.
const DRIVE_DB: f64 = 24.0;
const DRIVE_SCALE: f64 = 0.5;
/// The click: its length, its band-pass's centre and Q, and its level.
const CLICK_MS: f64 = 1.0;
const CLICK_HZ: f64 = 4_000.0;
const CLICK_Q: f64 = 2.0;
const CLICK_GAIN: f64 = 2.0;

/// The DS Kick device type.
#[must_use]
pub fn ds_kick_type() -> DeviceType {
    let params = vec![
        decay(400.0),
        volume(),
        ParamDescriptor::continuous(PITCH, "Pitch", (30.0, 120.0), 50.0, ParamUnit::Hertz)
            .logarithmic()
            .describe("The kick's tuning in hertz, whatever key plays it."),
        percent(
            ENV,
            "Env",
            50.0,
            "How far above its pitch the sine starts: at 100 % four octaves, falling to the \
             pitch over about 30 ms.",
        ),
        percent(
            ATTACK,
            "Attack",
            0.0,
            "Fades the sine in over up to 10 ms, smoothing its start.",
        ),
        percent(
            OT,
            "OT",
            0.0,
            "Adds the sine's second and third harmonics, up to 6 and 12 dB below it.",
        ),
        percent(
            DRIVE,
            "Drive",
            0.0,
            "Drives the sound through a soft clipper by up to 24 dB, with the output scaled \
             back.",
        ),
        ParamDescriptor::toggle(CLICK, "Click", false)
            .describe("Adds a 1 ms band-passed click at 4 kHz for a sharper start."),
    ];
    DeviceType::native(DS_KICK, "DS Kick", DeviceRole::Instrument).with_description(
        DeviceDescription::new(params)
            .with_layout(layout(&[
                (PITCH, Widget::Knob),
                (ENV, Widget::Knob),
                (ATTACK, Widget::Knob),
                (OT, Widget::Knob),
                (DRIVE, Widget::Knob),
                (CLICK, Widget::Toggle),
                (DECAY, Widget::Knob),
                (crate::drum_synth::VOLUME, Widget::Knob),
            ]))
            .with_banks(&[
                ("Tone", &["Pitch", "Env", "Attack", "OT", "Drive", "Click"]),
                ("Level", &["Decay", "Volume"]),
            ]),
    )
}

/// DS Kick's factory presets.
pub const PRESETS: [Preset; 3] = [
    Preset {
        name: "Deep Sub",
        params: &[(PITCH, 40.0), (ENV, 30.0), (ATTACK, 20.0), (DECAY, 900.0)],
    },
    Preset {
        name: "Punch",
        params: &[
            (PITCH, 55.0),
            (ENV, 80.0),
            (OT, 30.0),
            (DRIVE, 25.0),
            (CLICK, 1.0),
            (DECAY, 350.0),
        ],
    },
    Preset {
        name: "Hard Drive",
        params: &[
            (PITCH, 60.0),
            (ENV, 60.0),
            (OT, 60.0),
            (DRIVE, 80.0),
            (DECAY, 250.0),
        ],
    },
];

/// Adds DS Kick to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, ds_kick_type(), |device, config| {
        DrumSynth::new(device.id.0.get(), config.sample_rate.as_f64(), || {
            Kick::new(config.sample_rate.as_f64())
        })
    });
}

/// DS Kick's display: the envelope the Attack and Decay give the sine.
#[must_use]
pub fn display(
    device: DeviceId,
    description: &DeviceDescription,
    values: &OrdMap<ParamId, f64>,
) -> DeviceDisplay {
    crate::drum_synth::display(device, description, values, |value, ms| {
        let attack = value(ATTACK) / 100.0 * ATTACK_MAX_MS;
        let fade = if attack > 0.0 {
            (ms / attack).min(1.0)
        } else {
            1.0
        };
        fade * master_at(ms, value(DECAY))
    })
}

/// One kick hit.
struct Kick {
    rate: f64,
    noise: Random,
    phase: f64,
    age: u32,
    click: Filter,
}

impl Kick {
    fn new(rate: f64) -> Self {
        Self {
            rate,
            noise: Random::seeded(0),
            phase: 0.0,
            age: 0,
            click: Filter::default(),
        }
    }
}

impl Hit for Kick {
    fn start(&mut self, seed: u64) {
        self.noise = Random::seeded(seed);
        self.phase = 0.0;
        self.age = 0;
        self.click.clear();
    }

    fn frame(&mut self, bank: &ParamBank, frame: usize, master: f32) -> (f32, f32) {
        let at = |id: ParamId| f64::from(bank.ramp(id.0 as usize)[frame]);
        let t = f64::from(self.age) / self.rate;
        // The pitch starts Env x 4 octaves up and falls exponentially.
        let octaves = ENV_OCTAVES * at(ENV) / 100.0 * (-t / SWEEP_TAU).exp();
        self.phase = (self.phase + at(PITCH) * 2f64.powf(octaves) / self.rate).fract();
        let ot = at(OT) / 100.0;
        let (h2, h3) = (HARMONIC_2 * ot, HARMONIC_3 * ot);
        let sine = (TAU * self.phase).sin()
            + h2 * (2.0 * TAU * self.phase).sin()
            + h3 * (3.0 * TAU * self.phase).sin();
        let mut sound = sine / (1.0 + h2 + h3);
        let attack = at(ATTACK) / 100.0 * ATTACK_MAX_MS / 1000.0;
        if attack > 0.0 {
            sound *= (t / attack).min(1.0);
        }
        // A soft clipper, blended in over the first tenth of Drive so that
        // it's transparent at none.
        let drive = at(DRIVE) / 100.0;
        if drive > 0.0 {
            let gain = 10f64.powf(DRIVE_DB * drive / 20.0);
            let clipped = (gain * sound).tanh() / gain.tanh() * (1.0 - DRIVE_SCALE * drive);
            let blend = (drive * 10.0).min(1.0);
            sound += (clipped - sound) * blend;
        }
        if bank.value(CLICK.0 as usize) >= 0.5 {
            let window = (1.0 - t / (CLICK_MS / 1000.0)).max(0.0);
            let hit = self.noise.next_bipolar();
            let band = self
                .click
                .tick(hit, Pass::Band, CLICK_HZ, CLICK_Q, self.rate);
            sound += CLICK_GAIN * window * band;
        }
        self.age = self.age.saturating_add(1);
        let out = (sound * f64::from(master)) as f32;
        (out, out)
    }
}

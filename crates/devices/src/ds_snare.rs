//! DS Snare (WP-8.24; the drum synths spec, R12): a body of two sines that
//! morph towards triangles, and noise through a low-pass, high-pass or
//! band-pass filter. The manual gives the controls and no ranges or
//! curves, so the laws here are Beatlayer's own.

use std::f64::consts::TAU;

use bl_dsp::random::Random;
use bl_engine::{DeviceType, Registry};
use bl_model::device_layout::Widget;
use bl_model::{
    DeviceDescription, DeviceDisplay, DeviceId, DeviceRole, OrdMap, ParamDescriptor, ParamId,
};

use crate::drum_synth::{
    DECAY, DrumSynth, Filter, Hit, Pass, Preset, VOLUME, decay, layout, master_at, percent, tune,
    volume,
};
use crate::framework::{ParamBank, add};

/// DS Snare's device type id.
pub const DS_SNARE: &str = "beatlayer.ds_snare";

/// The pitch of the body and the noise filter, in semitones.
pub const TUNE: ParamId = ParamId(2);
/// How far the body's sines are triangles, and how high the noise filter's
/// cutoff is.
pub const COLOR: ParamId = ParamId(3);
/// How much noise there is against body.
pub const TONE: ParamId = ParamId(4);
/// The noise filter's type.
pub const FILTER: ParamId = ParamId(5);

/// The noise filter's types, in the chooser's order.
pub const FILTERS: [&str; 3] = ["Low-pass", "High-pass", "Band-pass"];

/// The body's two sines, in hertz at Tune 0.
const BODY_HZ: [f64; 2] = [185.0, 330.0];
/// The body's pitch starts a semitone up and falls with this time constant,
/// in seconds.
const BODY_SWEEP: f64 = 0.020;
/// The body decays this many times faster than the noise.
const BODY_FASTER: i32 = 4;
/// The noise filter's cutoff at Color 0, and the factor Color 100 % raises
/// it by: 2 kHz to 8 kHz.
const NOISE_HZ: f64 = 2_000.0;
const NOISE_RANGE: f64 = 4.0;
/// The filter's Q, and the gain that brings each type's noise to a level
/// near the body's.
const NOISE_Q: [f64; 3] = [0.707, 0.707, 1.0];
const NOISE_GAIN: [f64; 3] = [3.0, 1.0, 2.0];

/// The DS Snare device type.
#[must_use]
pub fn ds_snare_type() -> DeviceType {
    let params = vec![
        decay(250.0),
        volume(),
        tune(TUNE, "Tune"),
        percent(
            COLOR,
            "Color",
            50.0,
            "Shapes the pitched part: from sines to triangles. It also raises the noise \
             filter's cutoff, from 2 kHz to 8 kHz.",
        ),
        percent(
            TONE,
            "Tone",
            50.0,
            "How much noise there is against the pitched part: none at 0, only noise at 100 %.",
        ),
        ParamDescriptor::choice(FILTER, "Filter", &FILTERS, 1).describe(
            "Filters the noise: a low-pass, a high-pass or a band-pass at the Color's cutoff.",
        ),
    ];
    DeviceType::native(DS_SNARE, "DS Snare", DeviceRole::Instrument).with_description(
        DeviceDescription::new(params)
            .with_layout(layout(&[
                (COLOR, Widget::Knob),
                (TONE, Widget::Knob),
                (FILTER, Widget::Chooser),
                (TUNE, Widget::Knob),
                (DECAY, Widget::Knob),
                (VOLUME, Widget::Knob),
            ]))
            .with_banks(&[
                ("Tone", &["Color", "Tone", "Filter", "Tune"]),
                ("Level", &["Decay", "Volume"]),
            ]),
    )
}

/// DS Snare's factory presets.
pub const PRESETS: [Preset; 3] = [
    Preset {
        name: "Tight",
        params: &[(TONE, 45.0), (COLOR, 30.0), (DECAY, 140.0)],
    },
    Preset {
        name: "Fat Back",
        params: &[(TUNE, -3.0), (TONE, 65.0), (COLOR, 60.0), (DECAY, 380.0)],
    },
    Preset {
        name: "Rim Crack",
        params: &[
            (TUNE, 5.0),
            (TONE, 80.0),
            (COLOR, 90.0),
            (FILTER, 2.0),
            (DECAY, 120.0),
        ],
    },
];

/// Adds DS Snare to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, ds_snare_type(), |device, config| {
        DrumSynth::new(device.id.0.get(), config.sample_rate.as_f64(), || {
            Snare::new(config.sample_rate.as_f64())
        })
    });
}

/// DS Snare's display: the body's and the noise's envelopes, the larger.
#[must_use]
pub fn display(
    device: DeviceId,
    description: &DeviceDescription,
    values: &OrdMap<ParamId, f64>,
) -> DeviceDisplay {
    crate::drum_synth::display(device, description, values, |value, ms| {
        let tone = value(TONE) / 100.0;
        let master = master_at(ms, value(DECAY));
        ((1.0 - tone) * master.powi(BODY_FASTER)).max(tone * master)
    })
}

/// A triangle from 0 up, for a phase in cycles.
fn triangle(phase: f64) -> f64 {
    1.0 - 4.0 * ((phase + 0.25).fract() - 0.5).abs()
}

/// One snare hit.
struct Snare {
    rate: f64,
    noise: Random,
    phases: [f64; 2],
    age: u32,
    filter: Filter,
}

impl Snare {
    fn new(rate: f64) -> Self {
        Self {
            rate,
            noise: Random::seeded(0),
            phases: [0.0; 2],
            age: 0,
            filter: Filter::default(),
        }
    }
}

impl Hit for Snare {
    fn start(&mut self, seed: u64) {
        self.noise = Random::seeded(seed);
        self.phases = [0.0; 2];
        self.age = 0;
        self.filter.clear();
    }

    fn frame(&mut self, bank: &ParamBank, frame: usize, master: f32) -> (f32, f32) {
        let at = |id: ParamId| f64::from(bank.ramp(id.0 as usize)[frame]);
        let t = f64::from(self.age) / self.rate;
        let master = f64::from(master);
        let color = at(COLOR) / 100.0;
        let tone = at(TONE) / 100.0;
        let tuned = 2f64.powf(at(TUNE) / 12.0);
        let sweep = 2f64.powf((-t / BODY_SWEEP).exp() / 12.0);
        let mut body = 0.0;
        for (phase, hz) in self.phases.iter_mut().zip(BODY_HZ) {
            *phase = (*phase + hz * tuned * sweep / self.rate).fract();
            body += 0.5 * ((1.0 - color) * (TAU * *phase).sin() + color * triangle(*phase));
        }
        let kind = (bank.value(FILTER.0 as usize) as usize).min(FILTERS.len() - 1);
        let pass = [Pass::Low, Pass::High, Pass::Band][kind];
        let cutoff = NOISE_HZ * NOISE_RANGE.powf(color);
        let hiss = self.noise.next_bipolar();
        let hiss = self
            .filter
            .tick(hiss, pass, cutoff, NOISE_Q[kind], self.rate)
            * NOISE_GAIN[kind];
        self.age = self.age.saturating_add(1);
        let out = (1.0 - tone) * body * master.powi(BODY_FASTER) + tone * hiss * master;
        (out as f32, out as f32)
    }
}

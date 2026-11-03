//! DS HH (WP-8.25; the drum synths spec, R15): six band-limited squares at
//! inharmonic ratios, summed with white or pink noise, through a resonant
//! high-pass of 12 or 24 dB per octave. The manual gives the controls and no
//! ranges or curves, so the laws here are Beatlayer's own. DS Cymbal plays
//! the same [`Hat`] with its own base pitch and no attack or slope.

use bl_engine::{DeviceType, Registry};
use bl_model::{
    DeviceDescription, DeviceDisplay, DeviceId, DeviceRole, OrdMap, ParamDescriptor, ParamId,
    ParamUnit,
};

use crate::drum_synth::{
    DECAY, DrumSynth, Filter, Hit, Pass, Preset, VOLUME, decay, framed, label, large, panel,
    percent, tune, tune_semitones, volume,
};
use crate::ds_metal::{Noise, Squares};
use crate::framework::{ParamBank, add};

/// DS HH's device type id.
pub const DS_HH: &str = "beatlayer.ds_hh";

/// The high-pass's cutoff, in hertz.
pub const TONE: ParamId = ParamId(2);
/// The pitch of the squares, in per cent of two octaves each way (spec R5).
pub const PITCH: ParamId = ParamId(3);
/// The noise's colour.
pub const NOISE: ParamId = ParamId(4);
/// The high-pass's slope.
pub const SLOPE: ParamId = ParamId(5);
/// How slowly the squares come in.
pub const ATTACK: ParamId = ParamId(6);

/// The noise colours and slopes, in the choosers' order.
pub const NOISES: [&str; 2] = ["White", "Pink"];
pub const SLOPES: [&str; 2] = ["12 dB", "24 dB"];

/// The six squares' ratios to the base (spec R15).
pub(crate) const RATIOS: [f64; 6] = [1.0, 1.342, 1.2312, 1.6532, 1.9523, 2.1523];
/// The base at Pitch 0.
const BASE_HZ: f64 = 400.0;
/// The longest Attack, in milliseconds.
const ATTACK_MAX_MS: f64 = 20.0;
/// The high-pass's Q for one 12 dB section, and for each of the two of 24 dB.
const HP_Q: [f64; 2] = [1.2, 0.9];
/// What the squares' mean and the noise are scaled by before the high-pass,
/// to bring a hit to a usual level without clipping.
const SQUARES_GAIN: f64 = 0.6;
const NOISE_GAIN: f64 = 0.35;

/// The squares' base pitch in hertz at `semitones` from `base`.
pub(crate) fn base_hz(base: f64, semitones: f64) -> f64 {
    base * (semitones / 12.0).exp2()
}

/// The DS HH device type.
#[must_use]
pub fn ds_hh_type() -> DeviceType {
    let params = vec![
        decay(41.5),
        volume(),
        ParamDescriptor::continuous(TONE, "Tone", (2_000.0, 16_000.0), 6_000.0, ParamUnit::Hertz)
            .logarithmic()
            .describe("The high-pass's cutoff: higher values leave a thinner, brighter hat."),
        tune(PITCH, "Pitch", 50.0),
        ParamDescriptor::choice(NOISE, "Noise", &NOISES, 0)
            .describe("The noise's colour: white, or pink with less treble."),
        ParamDescriptor::choice(SLOPE, "Slope", &SLOPES, 0)
            .describe("The high-pass's slope, 12 or 24 dB per octave."),
        percent(
            ATTACK,
            "Attack",
            0.0,
            "Fades the pitched squares in over up to 20 ms; the noise starts at once.",
        ),
    ];
    DeviceType::native(DS_HH, "DS HH", DeviceRole::Instrument).with_description(
        DeviceDescription::new(params)
            .with_layout(panel(
                (
                    vec![
                        large(TONE, 0),
                        large(ATTACK, 1),
                        framed(NOISE, 2, 0),
                        framed(SLOPE, 2, 1),
                    ],
                    vec![label("Noise/Slope", 2)],
                ),
                [PITCH, DECAY, VOLUME],
            ))
            .with_banks(&[
                ("Hat", &["Tone", "Slope", "Noise", "Attack", "Pitch"]),
                ("Level", &["Decay", "Volume"]),
            ]),
    )
}

/// DS HH's factory presets.
pub const PRESETS: [Preset; 3] = [
    Preset {
        name: "Closed",
        params: &[(TONE, 8_000.0), (DECAY, 29.9)],
    },
    Preset {
        name: "Open",
        params: &[(TONE, 5_000.0), (DECAY, 63.5), (SLOPE, 1.0)],
    },
    Preset {
        name: "Pink Shaker",
        params: &[
            (TONE, 4_000.0),
            (NOISE, 1.0),
            (ATTACK, 40.0),
            (PITCH, 39.6),
            (DECAY, 50.0),
        ],
    },
];

/// Adds DS HH to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, ds_hh_type(), |device, config| {
        DrumSynth::new(device.id.0.get(), config.sample_rate.as_f64(), || {
            Hat::new(config.sample_rate.as_f64(), BASE_HZ, true)
        })
    });
}

/// DS HH's display: the hit's envelope.
#[must_use]
pub fn display(
    device: DeviceId,
    description: &DeviceDescription,
    values: &OrdMap<ParamId, f64>,
) -> DeviceDisplay {
    crate::drum_synth::decay_display(device, description, values)
}

/// One hat or cymbal hit. A cymbal has no Noise, Slope or Attack: it plays
/// white noise through a 12 dB slope with its squares at once.
pub(crate) struct Hat {
    rate: f64,
    base: f64,
    full: bool,
    squares: Squares<6>,
    noise: Noise,
    high_pass: [Filter; 2],
    age: u32,
}

impl Hat {
    pub(crate) fn new(rate: f64, base: f64, full: bool) -> Self {
        Self {
            rate,
            base,
            full,
            squares: Squares::new(),
            noise: Noise::new(),
            high_pass: [Filter::default(); 2],
            age: 0,
        }
    }
}

impl Hit for Hat {
    fn start(&mut self, seed: u64) {
        self.squares.start();
        self.noise.start(seed);
        for filter in &mut self.high_pass {
            filter.clear();
        }
        self.age = 0;
    }

    fn frame(&mut self, bank: &ParamBank, frame: usize, master: f32) -> (f32, f32) {
        let at = |id: ParamId| f64::from(bank.ramp(id.0 as usize)[frame]);
        let base = base_hz(self.base, tune_semitones(at(PITCH)));
        let squares = self
            .squares
            .tick(RATIOS.map(|ratio| base * ratio), self.rate);
        let mut mean = squares.iter().map(|s| f64::from(*s)).sum::<f64>() / RATIOS.len() as f64;
        let (pink, slope) = if self.full {
            let index = |id: ParamId| (bank.value(id.0 as usize) as usize).min(1);
            (index(NOISE) == 1, index(SLOPE))
        } else {
            (false, 0)
        };
        if self.full {
            let attack = ATTACK_MAX_MS / 1000.0 * self.rate * at(ATTACK) / 100.0;
            if attack > 0.0 {
                mean *= (f64::from(self.age) / attack).min(1.0);
            }
        }
        let hiss = if pink {
            self.noise.pink()
        } else {
            self.noise.white()
        };
        let mix = SQUARES_GAIN * mean + NOISE_GAIN * hiss;
        let tone = at(TONE);
        let mut out = self.high_pass[0].tick(mix, Pass::High, tone, HP_Q[slope], self.rate);
        if slope == 1 {
            out = self.high_pass[1].tick(out, Pass::High, tone, HP_Q[1], self.rate);
        }
        self.age = self.age.saturating_add(1);
        let out = (out * f64::from(master)) as f32;
        (out, out)
    }
}

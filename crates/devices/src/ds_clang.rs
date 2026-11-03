//! DS Clang (WP-8.25; the drum synths spec, R18): two band-limited squares
//! with white noise through a high-pass and a band-pass, or, with Clave on,
//! a sine burst sounded Repeat times. The manual gives the controls and no
//! ranges or curves, so the laws here are Beatlayer's own.

use core::f64::consts::TAU;

use bl_engine::{DeviceType, Registry};
use bl_model::device_layout::{Control, Widget};
use bl_model::{
    DeviceDescription, DeviceDisplay, DeviceId, DeviceRole, OrdMap, ParamDescriptor, ParamId,
    ParamUnit,
};

use crate::drum_synth::{
    DECAY, DrumSynth, Filter, Hit, Pass, Preset, VOLUME, decay, decay_ms, framed, label, master_at,
    panel, percent, tune, tune_semitones, volume,
};
use crate::ds_hh::base_hz;
use crate::ds_metal::{Noise, Squares};
use crate::framework::{ParamBank, add};

/// DS Clang's device type id.
pub const DS_CLANG: &str = "beatlayer.ds_clang";

/// The level of the 540 Hz square and of the 800 Hz one.
pub const TONE_A: ParamId = ParamId(2);
pub const TONE_B: ParamId = ParamId(3);
/// The cutoff of the high-pass and of the band-pass, in hertz.
pub const FILTER: ParamId = ParamId(4);
/// The white noise's level.
pub const NOISE: ParamId = ParamId(5);
/// Whether the hit is a clave instead.
pub const CLAVE: ParamId = ParamId(6);
/// How many times the clave sounds.
pub const REPEAT: ParamId = ParamId(7);
/// The pitch, in per cent of two octaves each way (spec R5).
pub const PITCH: ParamId = ParamId(8);

/// The squares' frequencies at Pitch 0.
const SQUARES_HZ: [f64; 2] = [540.0, 800.0];
/// The clave's sine, its burst's time to fall 60 dB and the longest it
/// sounds, in milliseconds, and the gap between repeats (spec R18).
const CLAVE_HZ: f64 = 2_500.0;
const CLAVE_DECAY_MS: f64 = 20.0;
const CLAVE_LENGTH_MS: f64 = 30.0;
const REPEAT_MS: f64 = 25.0;
/// The most repeats.
const REPEAT_MAX: i64 = 8;
/// The band-pass's Q, and the high-pass's.
const BAND_Q: f64 = 1.5;
const HIGH_Q: f64 = 0.707;
/// What the squares' and the noise's levels are scaled by, and the gain that
/// brings what the filters leave to a hit's usual level.
const SQUARE_GAIN: f64 = 0.5;
const NOISE_GAIN: f64 = 0.7;
const OUT_GAIN: f64 = 2.0;

/// The DS Clang device type.
#[must_use]
pub fn ds_clang_type() -> DeviceType {
    let params = vec![
        decay(53.7),
        volume(),
        percent(
            TONE_A,
            "Tone A",
            100.0,
            "The level of the first of the two cowbell tones, a square at 540 Hz.",
        ),
        percent(
            TONE_B,
            "Tone B",
            80.0,
            "The level of the second of the two cowbell tones, a square at 800 Hz.",
        ),
        ParamDescriptor::continuous(
            FILTER,
            "Filter",
            (100.0, 10_000.0),
            1_000.0,
            ParamUnit::Hertz,
        )
        .logarithmic()
        .describe("The cutoff of the high-pass and the band-pass the sound goes through."),
        percent(NOISE, "Noise", 0.0, "Mixes in white noise."),
        ParamDescriptor::toggle(CLAVE, "Clave", false)
            .describe("Makes the hit a clave instead: a 2.5 kHz sine burst, sounded Repeat times."),
        ParamDescriptor::integer(REPEAT, "Repeat", (1, REPEAT_MAX), 1, ParamUnit::None)
            .describe("How many times the clave sounds, 25 ms apart, with Clave on."),
        tune(PITCH, "Pitch", 50.0),
    ];
    DeviceType::native(DS_CLANG, "DS Clang", DeviceRole::Instrument).with_description(
        DeviceDescription::new(params)
            .with_layout(panel(
                (
                    vec![
                        framed(TONE_A, 0, 0),
                        framed(TONE_B, 0, 1),
                        framed(FILTER, 1, 0),
                        framed(NOISE, 1, 1),
                        framed(REPEAT, 2, 0),
                        Control::new(CLAVE, Widget::Toggle, 92.0, 85.0).sized(41.0, 12.0),
                    ],
                    vec![
                        label("Tone A/B", 0),
                        label("Filter/Noise", 1),
                        label("Repeat", 2),
                    ],
                ),
                [PITCH, DECAY, VOLUME],
            ))
            .with_banks(&[
                ("Clang", &["Tone A", "Tone B", "Filter", "Noise", "Pitch"]),
                ("Clave", &["Clave", "Repeat"]),
                ("Level", &["Decay", "Volume"]),
            ]),
    )
}

/// DS Clang's factory presets.
pub const PRESETS: [Preset; 3] = [
    Preset {
        name: "Cowbell",
        params: &[(TONE_B, 100.0), (FILTER, 900.0), (DECAY, 56.8)],
    },
    Preset {
        name: "Clank",
        params: &[
            (TONE_A, 70.0),
            (FILTER, 2_500.0),
            (NOISE, 30.0),
            (PITCH, 64.6),
            (DECAY, 41.5),
        ],
    },
    Preset {
        name: "Clave Roll",
        params: &[(CLAVE, 1.0), (REPEAT, 4.0), (PITCH, 45.8)],
    },
];

/// Adds DS Clang to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, ds_clang_type(), |device, config| {
        DrumSynth::new(device.id.0.get(), config.sample_rate.as_f64(), || {
            Clang::new(config.sample_rate.as_f64())
        })
    });
}

/// DS Clang's display: the hit's envelope, or the clave's bursts.
#[must_use]
pub fn display(
    device: DeviceId,
    description: &DeviceDescription,
    values: &OrdMap<ParamId, f64>,
) -> DeviceDisplay {
    crate::drum_synth::display(
        device,
        description,
        values,
        |_| Vec::new(),
        |value, ms| {
            if value(CLAVE) < 0.5 {
                return master_at(ms, decay_ms(value(DECAY)));
            }
            (0..value(REPEAT) as usize)
                .map(|k| ms - k as f64 * REPEAT_MS)
                .filter(|into| (0.0..CLAVE_LENGTH_MS).contains(into))
                .map(|into| 10f64.powf(-3.0 * into / CLAVE_DECAY_MS))
                .fold(0.0, f64::max)
        },
    )
}

/// The frame a clave repeat `k` starts on, counted from the hit's start: a
/// whole number of 25 ms from it at any block size (spec R18).
fn repeat_start(k: usize, rate: f64) -> u32 {
    (k as f64 * REPEAT_MS / 1000.0 * rate).round() as u32
}

/// One DS Clang hit.
struct Clang {
    rate: f64,
    squares: Squares<2>,
    noise: Noise,
    high_pass: Filter,
    band_pass: Filter,
    age: u32,
    /// The frame the last repeat of the clave ends on, while Clave is on.
    clave_ends: u32,
}

impl Clang {
    fn new(rate: f64) -> Self {
        Self {
            rate,
            squares: Squares::new(),
            noise: Noise::new(),
            high_pass: Filter::default(),
            band_pass: Filter::default(),
            age: 0,
            clave_ends: 0,
        }
    }

    /// The clave: each repeat that has started and not yet died away.
    fn clave(&mut self, repeats: usize, hz: f64) -> f64 {
        let length = (CLAVE_LENGTH_MS / 1000.0 * self.rate) as u32;
        let mut sum = 0.0;
        for k in 0..repeats {
            let start = repeat_start(k, self.rate);
            if self.age >= start && self.age - start < length {
                let t = f64::from(self.age - start) / self.rate;
                sum += (TAU * hz * t).sin() * 10f64.powf(-3.0 * t * 1000.0 / CLAVE_DECAY_MS);
            }
        }
        self.clave_ends = repeat_start(repeats - 1, self.rate) + length;
        sum
    }
}

impl Hit for Clang {
    fn start(&mut self, seed: u64) {
        self.squares.start();
        self.noise.start(seed);
        self.high_pass.clear();
        self.band_pass.clear();
        self.age = 0;
        self.clave_ends = 0;
    }

    fn busy(&self) -> bool {
        self.age < self.clave_ends
    }

    fn frame(&mut self, bank: &ParamBank, frame: usize, master: f32) -> (f32, f32) {
        let at = |id: ParamId| f64::from(bank.ramp(id.0 as usize)[frame]);
        let out = if bank.value(CLAVE.0 as usize) >= 0.5 {
            let repeats = (bank.value(REPEAT.0 as usize) as i64).clamp(1, REPEAT_MAX) as usize;
            self.clave(repeats, base_hz(CLAVE_HZ, tune_semitones(at(PITCH))))
        } else {
            self.clave_ends = 0;
            let tones = self.squares.tick(
                SQUARES_HZ.map(|hz| base_hz(hz, tune_semitones(at(PITCH)))),
                self.rate,
            );
            let mix = SQUARE_GAIN
                * (at(TONE_A) / 100.0 * f64::from(tones[0])
                    + at(TONE_B) / 100.0 * f64::from(tones[1]))
                + NOISE_GAIN * at(NOISE) / 100.0 * self.noise.white();
            let high = self
                .high_pass
                .tick(mix, Pass::High, at(FILTER), HIGH_Q, self.rate);
            OUT_GAIN
                * f64::from(master)
                * self
                    .band_pass
                    .tick(high, Pass::Band, at(FILTER), BAND_Q, self.rate)
        };
        self.age = self.age.saturating_add(1);
        (out as f32, out as f32)
    }
}

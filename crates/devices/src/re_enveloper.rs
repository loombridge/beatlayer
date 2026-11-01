//! Re-Enveloper, the native three-band envelope processor (WP-8.50; the
//! Re-Enveloper spec): two crossovers split the signal into three bands,
//! and each band's C/E Factor turns the gap between a fast and a slow
//! envelope of the band into a gain, so a positive factor flattens the
//! band's envelope and a negative one exaggerates it.
//!
//! The crossovers are [Multiband Dynamics](crate::multiband_dynamics)'s,
//! called rather than copied; they sum to an all-pass and add no latency.
//! The law and the ranges are Beatlayer's own reading of Live's Creative
//! Extensions pack page, which names the controls but gives neither; see
//! `docs/specs/re-enveloper.md`.

use bl_dsp::EnvelopeFollower;
use bl_engine::{DeviceType, NodeConfig, Registry};
use bl_model::device_layout::{BODY_HEIGHT, ControlBand, DisplayCell, LayoutNode, Widget};
use bl_model::param::db_to_gain;
use bl_model::{
    Device, DeviceDescription, DeviceDisplay, DeviceId, DeviceLayout, DeviceRole, DisplayReading,
    ParamDescriptor, ParamId, ParamUnit,
};

use crate::dynamics::level_db;
use crate::framework::{Audio, NativeDevice, ParamBank, add};
use crate::multiband_dynamics::{BAND_NAMES, BANDS, Crossover};

/// Re-Enveloper's device type.
pub const RE_ENVELOPER: &str = "beatlayer.re_enveloper";

/// Low Crossover: the frequency between the low and mid bands, in Hz.
pub const LOW_CROSSOVER: ParamId = ParamId(0);
/// High Crossover: the frequency between the mid and high bands, in Hz.
pub const HIGH_CROSSOVER: ParamId = ParamId(1);
/// Output: the level of the sum of the bands.
pub const OUTPUT: ParamId = ParamId(8);

/// The bands, by their indices.
pub const LOW: usize = 0;
pub const MID: usize = 1;
pub const HIGH: usize = 2;

/// Band `band`'s C/E Factor, in per cent: positive compresses its
/// envelope, negative expands it.
#[must_use]
pub const fn factor(band: usize) -> ParamId {
    ParamId(2 + band as u32)
}

/// Band `band`'s Gain, after its envelope stage.
#[must_use]
pub const fn gain(band: usize) -> ParamId {
    ParamId(5 + band as u32)
}

/// The fast envelope's attack and release, and the slow one's, in seconds.
const FAST: (f64, f64) = (0.001, 0.030);
const SLOW: (f64, f64) = (0.050, 0.500);

/// The most gain, in dB, an envelope stage cuts or boosts by.
const LIMIT_DB: f64 = 24.0;

/// What the node publishes, for each band: the fast envelope's level and
/// the envelope stage's gain, in dB.
const DISPLAY_VALUES: usize = 2 * BANDS;

/// The Re-Enveloper device type.
#[must_use]
pub fn re_enveloper_type() -> DeviceType {
    let mut params = vec![
        ParamDescriptor::continuous(
            LOW_CROSSOVER,
            "Low Crossover",
            (50.0, 2_000.0),
            200.0,
            ParamUnit::Hertz,
        )
        .logarithmic()
        .describe("The frequency between the low and mid bands."),
        ParamDescriptor::continuous(
            HIGH_CROSSOVER,
            "High Crossover",
            (500.0, 16_000.0),
            2_500.0,
            ParamUnit::Hertz,
        )
        .logarithmic()
        .describe("The frequency between the mid and high bands; never below the low one."),
    ];
    for (band, name) in BAND_NAMES.iter().enumerate() {
        params.push(
            ParamDescriptor::continuous(
                factor(band),
                &format!("{name} C/E Factor"),
                (-100.0, 100.0),
                0.0,
                ParamUnit::Percent,
            )
            .describe(
                "Above 0 compresses this band's envelope, so attacks and tails come closer \
                 to one level; below 0 expands it, so attacks stand out and tails fall away.",
            ),
        );
    }
    for (band, name) in BAND_NAMES.iter().enumerate() {
        params.push(
            ParamDescriptor::gain(gain(band), &format!("{name} Gain"), 12.0, 0.0)
                .describe("The band's level after its envelope stage, down to silence."),
        );
    }
    params.push(
        ParamDescriptor::gain(OUTPUT, "Output", 12.0, 0.0)
            .describe("The level of the sum of the bands, down to silence."),
    );

    let band = |band: usize| {
        LayoutNode::Band(ControlBand::stacked(
            BODY_HEIGHT,
            &[(factor(band), Widget::Knob), (gain(band), Widget::Knob)],
        ))
    };
    let layout = DeviceLayout::new(LayoutNode::Row(vec![
        LayoutNode::Band(ControlBand::stacked(
            BODY_HEIGHT,
            &[
                (LOW_CROSSOVER, Widget::Knob),
                (HIGH_CROSSOVER, Widget::Knob),
            ],
        )),
        LayoutNode::Cell(DisplayCell::new("Bands", 200.0, BODY_HEIGHT, 1.0)),
        band(LOW),
        band(MID),
        band(HIGH),
        LayoutNode::Band(ControlBand::stacked(BODY_HEIGHT, &[(OUTPUT, Widget::Knob)])),
    ]));
    DeviceType::native(RE_ENVELOPER, "Re-Enveloper", DeviceRole::AudioEffect).with_description(
        DeviceDescription::new(params)
            .with_layout(layout)
            .with_banks(&[
                (
                    "Bands",
                    &[
                        "Low Crossover",
                        "High Crossover",
                        "Low C/E Factor",
                        "Mid C/E Factor",
                        "High C/E Factor",
                        "Low Gain",
                        "Mid Gain",
                        "High Gain",
                    ],
                ),
                ("Output", &["Output"]),
            ]),
    )
}

/// Adds Re-Enveloper to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, re_enveloper_type(), make);
}

/// Makes a Re-Enveloper for `config`'s rate. Every follower is made here,
/// off the audio thread.
#[must_use]
pub fn make(_: &Device, config: &NodeConfig) -> ReEnveloper {
    let rate = config.sample_rate.as_f64();
    let follower = |(attack, release): (f64, f64)| {
        let mut follower = EnvelopeFollower::new(rate);
        follower.set(attack, release, false);
        follower
    };
    ReEnveloper {
        rate,
        crossovers: (0.0, 0.0),
        cross: [Crossover::default(); 2],
        fast: [(); BANDS].map(|()| follower(FAST)),
        slow: [(); BANDS].map(|()| follower(SLOW)),
        gain_db: [0.0; BANDS],
    }
}

/// Re-Enveloper's audio-thread half.
pub struct ReEnveloper {
    rate: f64,
    /// The crossovers' frequencies now, so they're redesigned only when
    /// they move.
    crossovers: (f64, f64),
    /// Each channel's crossover: Multiband Dynamics's, with both bands on.
    cross: [Crossover; 2],
    /// Each band's fast and slow envelope of the loudest channel, so the
    /// channels share one gain.
    fast: [EnvelopeFollower; BANDS],
    slow: [EnvelopeFollower; BANDS],
    /// Each band's envelope-stage gain now, in dB.
    gain_db: [f64; BANDS],
}

impl NativeDevice for ReEnveloper {
    fn process(&mut self, audio: &mut Audio<'_, '_>, params: &ParamBank) {
        let frames = audio.frames();
        let channels = audio.channels().min(2);
        if frames == 0 || channels == 0 {
            return;
        }
        let index = |id: ParamId| id.0 as usize;
        let lower = params.value(index(LOW_CROSSOVER));
        let upper = params.value(index(HIGH_CROSSOVER)).max(lower);
        if (lower, upper) != self.crossovers {
            self.crossovers = (lower, upper);
            for cross in &mut self.cross {
                cross.set(lower, upper, self.rate);
            }
        }
        let ramp = |id: ParamId| params.ramp(index(id));
        let factors = [ramp(factor(LOW)), ramp(factor(MID)), ramp(factor(HIGH))];
        let gains = [ramp(gain(LOW)), ramp(gain(MID)), ramp(gain(HIGH))];
        let output = ramp(OUTPUT);

        for i in 0..frames {
            // Each channel's three bands.
            let mut split = [[0.0f64; BANDS]; 2];
            for (channel, (cross, bands)) in self
                .cross
                .iter_mut()
                .zip(&mut split)
                .enumerate()
                .take(channels)
            {
                *bands = cross.process(f64::from(audio.channel(channel)[i]), true, true);
            }
            let mut sum = [0.0f64; 2];
            for band in 0..BANDS {
                let loudest = split[..channels]
                    .iter()
                    .fold(0.0f64, |most, bands| most.max(bands[band].abs()));
                let fast = self.fast[band].process(loudest as f32);
                let slow = self.slow[band].process(loudest as f32);
                // The fast envelope's level over the slow one's, in dB.
                let over = level_db(fast) - level_db(slow);
                let db = (-f64::from(factors[band][i]) * 0.01 * over).clamp(-LIMIT_DB, LIMIT_DB);
                self.gain_db[band] = db;
                let g = db_to_gain(db) * f64::from(gains[band][i]);
                for (total, bands) in sum.iter_mut().zip(&split).take(channels) {
                    *total += bands[band] * g;
                }
            }
            for (channel, total) in sum.iter().enumerate().take(channels) {
                audio.channel_mut(channel)[i] = (total * f64::from(output[i])) as f32;
            }
        }
    }

    fn display_len(&self) -> usize {
        DISPLAY_VALUES
    }

    fn display(&mut self, out: &mut [f32]) {
        for band in 0..BANDS {
            out[2 * band] = level_db(self.fast[band].level()) as f32;
            out[2 * band + 1] = self.gain_db[band] as f32;
        }
    }
}

/// The device's display (the Re-Enveloper spec, R1): each band's envelope
/// and gain, once an engine publishes. `data` is what the node published.
#[must_use]
pub fn display(device: DeviceId, data: Option<&[f32]>) -> DeviceDisplay {
    let published = data.and_then(|d| <[f32; DISPLAY_VALUES]>::try_from(d).ok());
    let mut readings = Vec::new();
    if let Some(published) = published {
        for (band, name) in BAND_NAMES.iter().enumerate() {
            for (what, value) in [
                ("Envelope", published[2 * band]),
                ("Gain", published[2 * band + 1]),
            ] {
                readings.push(DisplayReading {
                    name: format!("{name} {what}"),
                    value,
                    unit: "dB".to_owned(),
                    text: format!("{value:.1} dB"),
                });
            }
        }
    }
    DeviceDisplay {
        device,
        live: data.is_some(),
        view: None,
        series: Vec::new(),
        markers: Vec::new(),
        spectrograms: Vec::new(),
        readings,
    }
}

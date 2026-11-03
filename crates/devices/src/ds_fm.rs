//! DS FM (WP-8.25; the drum synths spec, R17): two operators of the FM core
//! (a modulator into a carrier), the modulator's envelope falling twice as
//! fast as the carrier's, through a low-pass. The manual gives the controls
//! and no ranges or curves, so the laws here are Beatlayer's own.

use bl_engine::{DeviceType, Registry};
use bl_model::{
    DeviceDescription, DeviceDisplay, DeviceId, DeviceRole, OrdMap, ParamDescriptor, ParamId,
    ParamUnit,
};
use lspk_voice::Start;

use crate::drum_synth::{
    DECAY, DrumSynth, Filter, Hit, Pass, Preset, VOLUME, decay, framed, label, large, panel,
    percent, tune, tune_semitones, volume,
};
use crate::fm::{Core, OPERATORS};
use crate::framework::{ParamBank, add};

/// DS FM's device type id.
pub const DS_FM: &str = "beatlayer.ds_fm";

/// The low-pass's cutoff, in hertz.
pub const TONE: ParamId = ParamId(2);
/// The modulator's feedback.
pub const FEEDB: ParamId = ParamId(3);
/// The FM amount: the modulation index, up to 8.
pub const AMNT: ParamId = ParamId(4);
/// The modulator's ratio, blended through [`RATIOS`].
pub const MOD: ParamId = ParamId(5);
/// The carrier's pitch, in per cent of two octaves each way (spec R5).
pub const PITCH: ParamId = ParamId(6);

/// The modulator's ratios to the carrier that Mod blends through, in turn.
pub const RATIOS: [f64; 4] = [1.0, 1.41, 2.76, 3.5];
/// The carrier at Pitch 0.
const CARRIER_HZ: f64 = 200.0;
/// The FM core's algorithm 8 (index 7) has B modulating A, and B with no
/// modulator of its own, so its Feedback applies; C and D stay silent.
const ALGORITHM: usize = 7;
/// The core moves a carrier's phase by up to 4 radians per unit of
/// modulator amplitude: this many units make Amnt 100 % an index of 8.
const INDEX_UNITS: f32 = 2.0;
/// The carrier's level.
const CARRIER_LEVEL: f32 = 0.8;
/// The low-pass's Q.
const LOW_PASS_Q: f64 = 0.707;

/// The DS FM device type.
#[must_use]
pub fn ds_fm_type() -> DeviceType {
    let params = vec![
        decay(56.8),
        volume(),
        ParamDescriptor::continuous(TONE, "Tone", (200.0, 20_000.0), 12_000.0, ParamUnit::Hertz)
            .logarithmic()
            .describe("The low-pass's cutoff: lower values take the highs off the sound."),
        percent(
            FEEDB,
            "Feedb.",
            20.0,
            "The modulator's feedback in the FM algorithm: the sound gets noisier as it rises.",
        ),
        percent(
            AMNT,
            "Amnt",
            40.0,
            "The FM amount: how far the modulator moves the carrier's phase, up to an index \
             of 8. Its envelope falls twice as fast as the carrier's.",
        ),
        percent(
            MOD,
            "Mod",
            30.0,
            "Blends the modulator's frequency ratio through 1, 1.41, 2.76 and 3.5 times the \
             carrier's.",
        ),
        tune(PITCH, "Pitch", 50.0),
    ];
    DeviceType::native(DS_FM, "DS FM", DeviceRole::Instrument).with_description(
        DeviceDescription::new(params)
            .with_layout(panel(
                (
                    vec![
                        large(TONE, 0),
                        large(AMNT, 1),
                        framed(FEEDB, 2, 0),
                        framed(MOD, 2, 1),
                    ],
                    vec![label("Feedb./Mod", 2)],
                ),
                [PITCH, DECAY, VOLUME],
            ))
            .with_banks(&[
                ("FM", &["Tone", "Feedb.", "Amnt", "Mod", "Pitch"]),
                ("Level", &["Decay", "Volume"]),
            ]),
    )
}

/// DS FM's factory presets.
pub const PRESETS: [Preset; 3] = [
    Preset {
        name: "Zap",
        params: &[(AMNT, 70.0), (MOD, 60.0), (PITCH, 75.0), (DECAY, 48.2)],
    },
    Preset {
        name: "Metal Tom",
        params: &[
            (AMNT, 55.0),
            (FEEDB, 50.0),
            (MOD, 85.0),
            (PITCH, 35.4),
            (DECAY, 61.6),
        ],
    },
    Preset {
        name: "Soft Blip",
        params: &[(TONE, 3_000.0), (AMNT, 20.0), (MOD, 0.0), (DECAY, 44.0)],
    },
];

/// Adds DS FM to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, ds_fm_type(), |device, config| {
        DrumSynth::new(device.id.0.get(), config.sample_rate.as_f64(), || {
            Fm::new(config.sample_rate.as_f64())
        })
    });
}

/// DS FM's display: the hit's envelope.
#[must_use]
pub fn display(
    device: DeviceId,
    description: &DeviceDescription,
    values: &OrdMap<ParamId, f64>,
) -> DeviceDisplay {
    crate::drum_synth::decay_display(device, description, values)
}

/// The modulator's ratio at Mod `share` (0 to 1): a straight line between
/// the neighbouring [`RATIOS`].
#[must_use]
pub fn ratio_at(share: f64) -> f64 {
    let place = share.clamp(0.0, 1.0) * (RATIOS.len() - 1) as f64;
    let below = (place as usize).min(RATIOS.len() - 2);
    RATIOS[below] + (RATIOS[below + 1] - RATIOS[below]) * (place - below as f64)
}

/// One DS FM hit.
struct Fm {
    rate: f64,
    core: Core,
    filter: Filter,
}

impl Fm {
    fn new(rate: f64) -> Self {
        let mut core = Core::with_voices(rate, 1, 0, 1, 0);
        core.settings.algorithm = ALGORITHM;
        core.settings.tone = 1.0;
        Self {
            rate,
            core,
            filter: Filter::default(),
        }
    }
}

impl Hit for Fm {
    fn start(&mut self, _seed: u64) {
        self.core.start(0, Start::Fresh, 0.0);
        self.filter.clear();
    }

    fn frame(&mut self, bank: &ParamBank, frame: usize, master: f32) -> (f32, f32) {
        let at = |id: ParamId| f64::from(bank.ramp(id.0 as usize)[frame]);
        let carrier = CARRIER_HZ * (tune_semitones(at(PITCH)) / 12.0).exp2();
        let mut hz = [0.0; OPERATORS];
        hz[0] = carrier;
        hz[1] = carrier * ratio_at(at(MOD) / 100.0);
        let mut amps = [0.0; OPERATORS];
        amps[0] = CARRIER_LEVEL * master;
        // The modulator falls twice as fast: the master envelope squared.
        amps[1] = INDEX_UNITS * (at(AMNT) / 100.0) as f32 * master * master;
        let mut feedbacks = [0.0; OPERATORS];
        feedbacks[1] = (at(FEEDB) / 100.0) as f32;
        let out = self.core.play(0, &hz, &amps, &feedbacks, 1.0);
        let out = self
            .filter
            .tick(f64::from(out), Pass::Low, at(TONE), LOW_PASS_Q, self.rate)
            as f32;
        (out, out)
    }
}

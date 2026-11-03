//! DS Clap (WP-8.24; the drum synths spec, R14): four bursts of noise
//! through a band-pass, a tail of band-passed noise, and two delay lines
//! panned left and right. The manual gives the controls and no ranges or
//! curves, so the laws here are Beatlayer's own.

use bl_dsp::random::Random;
use bl_engine::{DeviceType, Registry};
use bl_model::{DeviceDescription, DeviceDisplay, DeviceId, DeviceRole, OrdMap, ParamId};

use crate::drum_synth::{
    DECAY, DrumSynth, Filter, Hit, Pass, Preset, VOLUME, decay, decay_ms, framed, label, large,
    master_at, panel, percent, tune, tune_semitones, volume,
};
use crate::filters::Ring;
use crate::framework::{ParamBank, add};

/// DS Clap's device type id.
pub const DS_CLAP: &str = "beatlayer.ds_clap";

/// The time between the two delay lines, from none at 0 % to
/// [`SLOPPY_MAX_MS`] at 100 %.
pub const SLOPPY: ParamId = ParamId(2);
/// How much filtered noise follows the bursts.
pub const TAIL: ParamId = ParamId(3);
/// The stereo width, from mono at 0 to hard left and right.
pub const SPREAD: ParamId = ParamId(4);
/// The band-pass's brightness.
pub const TONE: ParamId = ParamId(5);
/// The pitch of the band-pass: 50 % is its own, and the ends are two octaves
/// away.
pub const TUNE: ParamId = ParamId(6);

/// How many bursts there are, how far apart they start and how fast each
/// falls 60 dB, in milliseconds.
const BURSTS: u32 = 4;
const BURST_GAP_MS: f64 = 8.0;
const BURST_DECAY_MS: f64 = 6.0;
/// The longest Sloppy, in milliseconds.
const SLOPPY_MAX_MS: f64 = 30.0;
/// The band-pass's centre at Tune 0 and Tone 50 %, its Q, and how far
/// Tone moves it, in octaves each way from the middle.
const CENTRE_HZ: f64 = 1_200.0;
const BAND_Q: f64 = 2.0;
const TONE_OCTAVES: f64 = 1.0;
/// What the band-pass takes from white noise is made up by this gain.
const NOISE_GAIN: f64 = 3.0;
/// The tail starts as the last burst does, and fades in over this long.
const TAIL_FADE_MS: f64 = 2.0;
/// The hit runs at least this long, so the bursts and the delays finish
/// whatever Decay is.
const MIN_MS: f64 = BURSTS as f64 * BURST_GAP_MS + 2.0 * BURST_DECAY_MS + SLOPPY_MAX_MS + 20.0;

/// The DS Clap device type.
#[must_use]
pub fn ds_clap_type() -> DeviceType {
    let params = vec![
        decay(30.0),
        volume(),
        percent(
            SLOPPY,
            "Sloppy",
            24.0,
            "The time between the two delay lines, left and right, up to 30 ms: the claps \
             line up tightly at 0.",
        ),
        percent(
            TAIL,
            "Tail",
            65.0,
            "Adds band-passed noise to the clap's bursts, decaying over Decay.",
        ),
        percent(
            SPREAD,
            "Spread",
            76.0,
            "The stereo width: mono at 0, the two lines hard left and right at 100 %.",
        ),
        percent(
            TONE,
            "Tone",
            32.0,
            "Tilts the band-pass's brightness: higher values add high frequencies.",
        ),
        tune(TUNE, "Tune", 18.0),
    ];
    DeviceType::native(DS_CLAP, "DS Clap", DeviceRole::Instrument).with_description(
        DeviceDescription::new(params)
            .with_layout(panel(
                (
                    vec![
                        large(SLOPPY, 0),
                        large(TAIL, 1),
                        framed(SPREAD, 2, 0),
                        framed(TONE, 2, 1),
                    ],
                    vec![label("Spread/Tone", 2)],
                ),
                [TUNE, DECAY, VOLUME],
            ))
            .with_banks(&[
                ("Clap", &["Sloppy", "Tail", "Spread", "Tone", "Tune"]),
                ("Level", &["Decay", "Volume"]),
            ]),
    )
}

/// DS Clap's factory presets.
pub const PRESETS: [Preset; 3] = [
    Preset {
        name: "Tight",
        params: &[(SLOPPY, 6.7), (TAIL, 30.0), (SPREAD, 40.0), (DECAY, 50.0)],
    },
    Preset {
        name: "Room",
        params: &[
            (SLOPPY, 46.7),
            (TAIL, 70.0),
            (SPREAD, 80.0),
            (TONE, 40.0),
            (DECAY, 68.3),
        ],
    },
    Preset {
        name: "Sloppy Crowd",
        params: &[
            (SLOPPY, 93.3),
            (TAIL, 50.0),
            (SPREAD, 100.0),
            (TUNE, 43.8),
            (DECAY, 63.5),
        ],
    },
];

/// Adds DS Clap to `registry`.
pub fn register(registry: &mut Registry) {
    add(registry, ds_clap_type(), |device, config| {
        DrumSynth::new(device.id.0.get(), config.sample_rate.as_f64(), || {
            Clap::new(config.sample_rate.as_f64())
        })
    });
}

/// The bursts' envelope at `ms` into the hit.
fn bursts(ms: f64) -> f64 {
    (0..BURSTS)
        .map(|n| {
            let into = ms - f64::from(n) * BURST_GAP_MS;
            if into < 0.0 {
                0.0
            } else {
                10f64.powf(-3.0 * into / BURST_DECAY_MS)
            }
        })
        .sum()
}

/// How far the tail has come in at `ms` into the hit: it starts as the
/// last burst does.
fn tail_in(ms: f64) -> f64 {
    ((ms - f64::from(BURSTS - 1) * BURST_GAP_MS) / TAIL_FADE_MS).clamp(0.0, 1.0)
}

/// DS Clap's display: the bursts and the tail.
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
            bursts(ms) + value(TAIL) / 100.0 * tail_in(ms) * master_at(ms, decay_ms(value(DECAY)))
        },
    )
}

/// One clap hit.
struct Clap {
    rate: f64,
    noise: Random,
    age: u32,
    bursts: Filter,
    tail: Filter,
    line: Ring,
}

impl Clap {
    fn new(rate: f64) -> Self {
        Self {
            rate,
            noise: Random::seeded(0),
            age: 0,
            bursts: Filter::default(),
            tail: Filter::default(),
            line: Ring::new((SLOPPY_MAX_MS / 1000.0 * rate) as usize + 8),
        }
    }
}

impl Hit for Clap {
    fn start(&mut self, seed: u64) {
        self.noise = Random::seeded(seed);
        self.age = 0;
        self.bursts.clear();
        self.tail.clear();
        self.line.samples.fill(0.0);
    }

    fn min_ms(&self) -> f64 {
        MIN_MS
    }

    fn frame(&mut self, bank: &ParamBank, frame: usize, master: f32) -> (f32, f32) {
        let at = |id: ParamId| f64::from(bank.ramp(id.0 as usize)[frame]);
        let ms = f64::from(self.age) * 1000.0 / self.rate;
        let centre = CENTRE_HZ
            * 2f64.powf(tune_semitones(at(TUNE)) / 12.0)
            * 2f64.powf((at(TONE) / 100.0 - 0.5) * 2.0 * TONE_OCTAVES);
        let burst = self.noise.next_bipolar() * bursts(ms);
        let hiss = self.noise.next_bipolar() * at(TAIL) / 100.0 * tail_in(ms) * f64::from(master);
        let sound = NOISE_GAIN
            * (self
                .bursts
                .tick(burst, Pass::Band, centre, BAND_Q, self.rate)
                + self.tail.tick(hiss, Pass::Band, centre, BAND_Q, self.rate));
        self.line.write(sound);
        let left = self.line.read(1.0);
        let right = self
            .line
            .read(1.0 + at(SLOPPY) / 100.0 * SLOPPY_MAX_MS / 1000.0 * self.rate);
        let spread = at(SPREAD) / 100.0;
        let mid = (left + right) / 2.0;
        self.age = self.age.saturating_add(1);
        (
            (mid + (left - mid) * spread) as f32,
            (mid + (right - mid) * spread) as f32,
        )
    }
}

//! Triband, the native multiband dynamics (WP-5.22; the multiband-dynamics
//! spec): the crossovers and the High and Low switches, the four kinds of
//! processing and their static curves, the soft knee, the envelope times,
//! Time and Amount, Peak and RMS, the band gains, activators and solos, the
//! sidechain key, the display, and renders that don't depend on the block
//! size. Every block is a real-time section under the allocation trap.

bl_rt::install_trap!();

mod common;

use core::f64::consts::TAU;

use bl_devices::multiband_dynamics::{
    ABOVE_ATTACK, ABOVE_RATIO, ABOVE_RELEASE, ABOVE_THRESHOLD, ACTIVE, AMOUNT, BAND_OUTPUT,
    BELOW_ATTACK, BELOW_RATIO, BELOW_RELEASE, BELOW_THRESHOLD, DETECTOR, HIGH, HIGH_FREQ, HIGH_ON,
    INPUT, LOW, LOW_FREQ, LOW_ON, MID, MULTIBAND_DYNAMICS, OUTPUT, SOFT_KNEE, SOLO, TIME, VIEWS,
    band_param, multiband_dynamics_type,
};
use bl_engine::{Block, DeviceType, DisplayReader, Node, Registry, TransportInfo};
use bl_model::param::db_to_gain;
use bl_model::{DeviceId, OrdMap, ParamId};

use common::{RATE, device, node};

const RATE_HZ: f64 = 48_000.0;

/// A registry with Triband alone.
fn registry() -> Registry {
    let mut registry = Registry::empty();
    bl_devices::multiband_dynamics::register(&mut registry);
    registry
}

/// A Triband node of `params`, up to `max_frames` at a time, and the reader
/// of its display.
fn rig(params: &[(ParamId, f64)], max_frames: usize) -> (Box<dyn Node>, DisplayReader) {
    let device_type: DeviceType = multiband_dynamics_type();
    let mut node = node(&registry(), &device(&device_type, params), max_frames);
    let reader = node.display().expect("it has a display");
    (node, reader)
}

/// One block of `input` on both channels, with `key` (also on both) as the
/// sidechain signal when given.
fn run(node: &mut dyn Node, input: &[f32], key: Option<&[f32]>) -> Vec<f32> {
    let frames = input.len();
    let mut data: Vec<f32> = (0..2).flat_map(|_| input.iter().copied()).collect();
    let keyed: Option<Vec<f32>> = key.map(|key| (0..2).flat_map(|_| key.iter().copied()).collect());
    let mut block = Block::new(
        &mut data,
        2,
        frames,
        frames,
        &[],
        TransportInfo::default(),
        RATE,
    );
    if let Some(keyed) = &keyed {
        block = block.with_sidechain(keyed);
    }
    bl_rt::rt_test!({ node.process(&mut block) });
    data.truncate(frames);
    data
}

/// `input` through `node` in blocks of `block` frames.
fn stream(node: &mut dyn Node, input: &[f32], block: usize, key: Option<&[f32]>) -> Vec<f32> {
    let mut out = Vec::with_capacity(input.len());
    for start in (0..input.len()).step_by(block) {
        let end = (start + block).min(input.len());
        out.extend(run(node, &input[start..end], key.map(|k| &k[start..end])));
    }
    out
}

/// Both bands off: one band, no crossover.
const ONE_BAND: [(ParamId, f64); 2] = [(LOW_ON, 0.0), (HIGH_ON, 0.0)];

/// `params` with the bands switched off, for a single band.
fn one_band(params: &[(ParamId, f64)]) -> Vec<(ParamId, f64)> {
    ONE_BAND.iter().chain(params).copied().collect()
}

fn mid(field: u32) -> ParamId {
    band_param(MID, field)
}

/// The settled output level, in dB relative to the input, of a constant
/// `input_db` signal through one band of `params` (and a constant `key_db`
/// key, when given).
fn gain_db(params: &[(ParamId, f64)], input_db: f64, key_db: Option<f64>) -> f64 {
    let input = vec![db_to_gain(input_db) as f32; 24_000];
    let key = key_db.map(|db| vec![db_to_gain(db) as f32; 24_000]);
    let (mut node, _) = rig(&one_band(params), 256);
    let out = stream(node.as_mut(), &input, 256, key.as_deref());
    let level = out[20_000..]
        .iter()
        .fold(0.0f64, |m, s| m.max(f64::from(s.abs())));
    20.0 * (level / db_to_gain(input_db)).log10()
}

/// The magnitude of `signal` at `hz`, over the whole of it (a Goertzel
/// filter, on whole cycles).
fn tone_level(signal: &[f32], hz: f64) -> f64 {
    let (mut re, mut im) = (0.0f64, 0.0f64);
    for (n, s) in signal.iter().enumerate() {
        let phase = TAU * hz * n as f64 / RATE_HZ;
        re += f64::from(*s) * phase.cos();
        im += f64::from(*s) * phase.sin();
    }
    2.0 * re.hypot(im) / signal.len() as f64
}

fn db(level: f64) -> f64 {
    20.0 * level.max(1e-9).log10()
}

/// A sine of `hz` and `amplitude`, `frames` long.
fn sine(hz: f64, amplitude: f64, frames: usize) -> Vec<f32> {
    (0..frames)
        .map(|n| (amplitude * (TAU * hz * n as f64 / RATE_HZ).sin()) as f32)
        .collect()
}

fn mix(parts: &[Vec<f32>]) -> Vec<f32> {
    (0..parts[0].len())
        .map(|n| parts.iter().map(|p| p[n]).sum())
        .collect()
}

/// The published display values: for each band the gain, and the input and
/// output peaks, in dB.
fn published(reader: &mut DisplayReader) -> [f32; 9] {
    reader.read().values.clone().try_into().unwrap()
}

/// A Butterworth biquad (the bilinear transform of the analogue section),
/// in direct form I with `f64` state, apart from the device's own.
struct Reference {
    b: [f64; 3],
    a: [f64; 2],
    x: [f64; 2],
    y: [f64; 2],
}

impl Reference {
    fn new(high_pass: bool, hz: f64) -> Self {
        let k = (core::f64::consts::PI * hz / RATE_HZ).tan();
        let q = core::f64::consts::FRAC_1_SQRT_2;
        let norm = 1.0 / (1.0 + k / q + k * k);
        let (b0, b1) = if high_pass {
            (norm, -2.0 * norm)
        } else {
            (k * k * norm, 2.0 * k * k * norm)
        };
        Self {
            b: [b0, b1, b0],
            a: [2.0 * (k * k - 1.0) * norm, (1.0 - k / q + k * k) * norm],
            x: [0.0; 2],
            y: [0.0; 2],
        }
    }

    fn process(&mut self, x: f64) -> f64 {
        let y = self.b[0] * x + self.b[1] * self.x[0] + self.b[2] * self.x[1]
            - self.a[0] * self.y[0]
            - self.a[1] * self.y[1];
        self.x = [x, self.x[0]];
        self.y = [y, self.y[0]];
        y
    }
}

/// A fourth-order Linkwitz-Riley filter: two Butterworth sections.
struct Lr4([Reference; 2]);

impl Lr4 {
    fn new(high_pass: bool, hz: f64) -> Self {
        Self([Reference::new(high_pass, hz), Reference::new(high_pass, hz)])
    }

    fn process(&mut self, x: f64) -> f64 {
        let x = self.0[0].process(x);
        self.0[1].process(x)
    }
}

/// The spec's crossover-only reference: the low band through the upper
/// crossover's all-pass, plus the mid and high bands from what is above the
/// lower crossover.
fn crossover_reference(input: &[f32], lower: f64, upper: f64) -> Vec<f32> {
    let (mut low, mut high) = (Lr4::new(false, lower), Lr4::new(true, lower));
    let (mut mid_low, mut mid_high) = (Lr4::new(false, upper), Lr4::new(true, upper));
    let (mut align_low, mut align_high) = (Lr4::new(false, upper), Lr4::new(true, upper));
    input
        .iter()
        .map(|x| {
            let x = f64::from(*x);
            let (below, above) = (low.process(x), high.process(x));
            (align_low.process(below)
                + align_high.process(below)
                + mid_low.process(above)
                + mid_high.process(above)) as f32
        })
        .collect()
}

/// The error of `got` against `want`, in dB relative to `want`.
fn null_db(got: &[f32], want: &[f32]) -> f64 {
    let error: f64 = got
        .iter()
        .zip(want)
        .map(|(g, w)| (f64::from(g - w)).powi(2))
        .sum();
    let signal: f64 = want.iter().map(|w| f64::from(*w).powi(2)).sum();
    10.0 * (error / signal).log10()
}

// Spec: multiband-dynamics R1
#[test]
fn triband_is_described_with_a_sidechain_three_bands_and_no_latency() {
    let device_type = multiband_dynamics_type();
    device_type.description.check().unwrap();
    assert_eq!(device_type.name, "Triband");
    assert!(device_type.sidechain, "it has a sidechain section");
    assert_eq!(device_type.description.params.len(), 9 + 3 * 12);
    assert_eq!(device_type.description.views, VIEWS);
    assert!(matches!(
        device_type.kind,
        bl_model::DeviceKind::Native { ref device_type } if device_type == MULTIBAND_DYNAMICS
    ));
    // Every band's gains reach silence, and its names say which band.
    for band in [LOW, MID, HIGH] {
        for field in [INPUT, BAND_OUTPUT] {
            let param = device_type
                .description
                .param(band_param(band, field))
                .unwrap();
            assert!(param.is_gain(), "{}", param.name);
        }
    }
    let names: Vec<_> = device_type
        .description
        .params
        .iter()
        .map(|p| &p.name)
        .collect();
    for name in ["Low Active", "Mid Above Threshold", "High Below Release"] {
        assert!(names.iter().any(|n| *n == name), "{name}");
    }
    // The crossovers add no latency, whatever the settings.
    let (node, _) = rig(&[(LOW_FREQ, 40.0), (HIGH_FREQ, 15_000.0)], 64);
    assert_eq!(node.latency(), 0);
}

// Spec: multiband-dynamics R2
#[test]
fn with_every_ratio_at_one_the_bands_sum_flat_and_null_against_the_crossover_reference() {
    // Flat to 0.1 dB across the crossovers and far from them.
    for hz in [
        40.0, 100.0, 300.0, 500.0, 800.0, 1_500.0, 2_000.0, 3_000.0, 6_000.0, 12_000.0,
    ] {
        // Every test frequency is a multiple of 10 Hz, so the last 4,800
        // samples hold whole cycles of it.
        let (mut node, _) = rig(&[], 9_600);
        let out = run(node.as_mut(), &sine(hz, 0.5, 9_600), None);
        let level = db(tone_level(&out[4_800..], hz) / 0.5);
        assert!(level.abs() < 0.1, "{hz} Hz: {level} dB");
    }
    // And the output is the reference all-pass, at several crossovers.
    let noise = common::noise(8_192);
    for (lower, upper) in [(500.0, 2_000.0), (60.0, 9_000.0), (1_000.0, 1_000.0)] {
        let (mut node, _) = rig(&[(LOW_FREQ, lower), (HIGH_FREQ, upper)], 8_192);
        let got = run(node.as_mut(), &noise, None);
        let want = crossover_reference(&noise, lower, upper);
        let null = null_db(&got, &want);
        assert!(null < -100.0, "{lower}/{upper} Hz: {null} dB");
    }
}

// Spec: multiband-dynamics R3
#[test]
fn the_high_and_low_switches_decide_the_bands() {
    // Heavy compression on the Low and High bands (a ratio of 100 from
    // -70 dB) and none on the Mid.
    let heavy = |band| {
        [
            (band_param(band, ABOVE_THRESHOLD), -70.0),
            (band_param(band, ABOVE_RATIO), 100.0),
        ]
    };
    let tones = |hz: f64| sine(hz, 0.5, 9_600);
    let level_of = |params: &[(ParamId, f64)], hz: f64| {
        let (mut node, _) = rig(params, 9_600);
        let out = run(node.as_mut(), &tones(hz), None);
        db(tone_level(&out[4_800..], hz) / 0.5)
    };
    let both: Vec<_> = heavy(LOW).into_iter().chain(heavy(HIGH)).collect();
    // Both off: one band, the Mid, and the other bands' settings do
    // nothing. With no crossover the signal comes out as it went in.
    let off = [(LOW_ON, 0.0), (HIGH_ON, 0.0)];
    let all: Vec<_> = off.iter().chain(&both).copied().collect();
    let (mut node, _) = rig(&all, 4_800);
    let input = common::noise(4_800);
    assert_eq!(run(node.as_mut(), &input, None), input);
    // With the Mid heavy instead, the whole signal is compressed.
    let mid_heavy = heavy(MID);
    let params: Vec<_> = off.iter().chain(&mid_heavy).copied().collect();
    assert!(level_of(&params, 100.0) < -20.0);
    assert!(level_of(&params, 8_000.0) < -20.0);
    // Low on, High off: the Low band is below 500 Hz, the Mid above it, and
    // the High band's settings do nothing.
    let low_only: Vec<_> = [(HIGH_ON, 0.0)].iter().chain(&both).copied().collect();
    assert!(level_of(&low_only, 100.0) < -20.0, "the Low band");
    assert!(level_of(&low_only, 8_000.0).abs() < 0.1, "no High band");
    // High on, Low off: the High band is above 2 kHz, the Mid below it, and
    // the Low band's settings do nothing.
    let high_only: Vec<_> = [(LOW_ON, 0.0)].iter().chain(&both).copied().collect();
    assert!(level_of(&high_only, 8_000.0) < -20.0, "the High band");
    assert!(level_of(&high_only, 100.0).abs() < 0.1, "no Low band");
}

// Spec: multiband-dynamics R4
#[test]
fn a_tone_in_one_band_moves_only_that_bands_level() {
    // Three bands, tones in the Low, Mid and High bands.
    let frames = 24_000;
    let low = sine(100.0, 0.5, frames);
    let high = sine(8_000.0, 0.5, frames);
    // Each band's input level is its tone's, the others' far below.
    for (hz, band) in [(100.0, LOW), (1_000.0, MID), (8_000.0, HIGH)] {
        let (mut node, mut reader) = rig(&[], 4_800);
        stream(node.as_mut(), &sine(hz, 0.5, frames), 4_800, None);
        let values = published(&mut reader);
        for other in [LOW, MID, HIGH] {
            let input = f64::from(values[3 * other + 1]);
            if other == band {
                assert!((input - db(0.5)).abs() < 1.5, "band {band}: {input} dB");
            } else {
                assert!(
                    input < db(0.5) - 20.0,
                    "band {band} in band {other}: {input} dB"
                );
            }
        }
    }
    // Compressing the Low band moves its tone and leaves the High band's.
    let params = [
        (band_param(LOW, ABOVE_THRESHOLD), -30.0),
        (band_param(LOW, ABOVE_RATIO), 100.0),
        (band_param(LOW, ABOVE_ATTACK), 1.0),
    ];
    let (mut node, mut reader) = rig(&params, 4_800);
    let out = stream(node.as_mut(), &mix(&[low, high]), 4_800, None);
    let tail = &out[12_000..];
    assert!(
        db(tone_level(tail, 100.0) / 0.5) < -15.0,
        "the Low tone is cut"
    );
    assert!(
        db(tone_level(tail, 8_000.0) / 0.5).abs() < 0.1,
        "the High tone isn't"
    );
    let values = published(&mut reader);
    assert!(values[0] < -15.0, "the Low band's gain: {}", values[0]);
    assert_eq!((values[3], values[6]), (0.0, 0.0), "the others' gains");
}

// Spec: multiband-dynamics R5
#[test]
fn the_static_curve_of_each_kind_matches_at_three_thresholds() {
    // Each kind as (Above or Below's threshold, ratio, input in dB,
    // thresholds); the gain it calls for follows from the ratio. Downward compression: Above, ratio 4, from 0 dB; downward expansion
    // below: Below, ratio 0.5, from -60 dB; upward compression: Below,
    // ratio 2; upward expansion: Above, ratio 0.75.
    let cases: [(&str, u32, f64, f64, [f64; 3]); 4] = [
        (
            "downward compression",
            ABOVE_THRESHOLD,
            4.0,
            0.0,
            [-30.0, -20.0, -10.0],
        ),
        (
            "upward expansion",
            ABOVE_THRESHOLD,
            0.75,
            0.0,
            [-30.0, -20.0, -10.0],
        ),
        (
            "upward compression",
            BELOW_THRESHOLD,
            2.0,
            -60.0,
            [-50.0, -40.0, -30.0],
        ),
        (
            "downward expansion",
            BELOW_THRESHOLD,
            0.5,
            -60.0,
            [-50.0, -40.0, -30.0],
        ),
    ];
    for (kind, field, ratio, input_db, thresholds) in cases {
        for threshold in thresholds {
            let above = field == ABOVE_THRESHOLD;
            let over = if above {
                input_db - threshold
            } else {
                threshold - input_db
            };
            let slope = 1.0 / ratio - 1.0;
            let expected = if above { slope * over } else { -slope * over };
            let params = [
                (mid(field), threshold),
                (mid(field + 1), ratio),
                (mid(field + 2), 1.0),
                (mid(field + 3), 5.0),
            ];
            let got = gain_db(&params, input_db, None);
            assert!(
                (got - expected).abs() < 0.1,
                "{kind} at {threshold} dB: {got} dB, expected {expected} dB"
            );
        }
    }
    // Nothing happens on the far side of a threshold.
    let params = [(mid(ABOVE_THRESHOLD), -20.0), (mid(ABOVE_RATIO), 4.0)];
    assert!(gain_db(&params, -30.0, None).abs() < 0.1);
    let params = [(mid(BELOW_THRESHOLD), -40.0), (mid(BELOW_RATIO), 2.0)];
    assert!(gain_db(&params, -30.0, None).abs() < 0.1);
}

// Spec: multiband-dynamics R5
#[test]
fn the_below_threshold_never_rises_over_the_above_one() {
    // Below at -10 dB over an Above at -40 dB: the Below is held at -40 dB,
    // so a -30 dB signal sits between them and is left alone.
    let params = [
        (mid(ABOVE_THRESHOLD), -40.0),
        (mid(BELOW_THRESHOLD), -10.0),
        (mid(BELOW_RATIO), 4.0),
    ];
    assert!(gain_db(&params, -30.0, None).abs() < 0.1);
}

// Spec: multiband-dynamics R6
#[test]
fn a_soft_knee_turns_the_threshold_gradually() {
    // Above at -20 dB, ratio 4, a 6 dB knee: none at -23 dB, a quarter of
    // the knee's way at the threshold, the hard curve at -17 dB.
    for (input_db, expected) in [(-23.0, 0.0), (-20.0, -0.5625), (-17.0, -2.25)] {
        let params = [
            (mid(ABOVE_THRESHOLD), -20.0),
            (mid(ABOVE_RATIO), 4.0),
            (SOFT_KNEE, 1.0),
        ];
        let got = gain_db(&params, input_db, None);
        assert!((got - expected).abs() < 0.1, "in {input_db} dB: {got} dB");
    }
    // The Below side's knee mirrors it.
    let params = [
        (mid(BELOW_THRESHOLD), -40.0),
        (mid(BELOW_RATIO), 2.0),
        (SOFT_KNEE, 1.0),
    ];
    let got = gain_db(&params, -40.0, None);
    assert!((got - 0.375).abs() < 0.1, "{got} dB");
    // And with the knee off the threshold is a corner.
    let params = [(mid(BELOW_THRESHOLD), -40.0), (mid(BELOW_RATIO), 2.0)];
    assert!(gain_db(&params, -40.0, None).abs() < 0.1);
}

/// The time, in ms, a side's gain takes to close 1 - 1/e of the way from
/// where `before_db` leaves it to where `after_db` does, for a constant
/// input that steps between them.
fn time_constant(params: &[(ParamId, f64)], before_db: f64, after_db: f64) -> f64 {
    let settled = |input_db| gain_db(params, input_db, None);
    let (from, to) = (settled(before_db), settled(after_db));
    assert!(
        (to - from).abs() > 10.0,
        "a step to measure: {from} to {to}"
    );
    let (mut node, _) = rig(&one_band(params), 256);
    let before = vec![db_to_gain(before_db) as f32; 48_000];
    stream(node.as_mut(), &before, 256, None);
    let after = vec![db_to_gain(after_db) as f32; 24_000];
    let out = stream(node.as_mut(), &after, 256, None);
    let at = out
        .iter()
        .position(|s| {
            let now = 20.0 * (f64::from(s.abs()) / db_to_gain(after_db)).log10();
            (now - from) / (to - from) >= 1.0 - (-1.0f64).exp()
        })
        .expect("the gain gets there");
    (at + 1) as f64 / RATE_HZ * 1000.0
}

// Spec: multiband-dynamics R7
#[test]
fn attack_and_release_are_time_constants_on_each_side() {
    let near = |got: f64, want: f64, who: &str| {
        assert!(
            (got / want - 1.0).abs() < 0.05,
            "{who}: {got} ms, wanted {want} ms"
        );
    };
    // Above: loud takes the gain down (attack), quiet lets it go (release).
    let above = [
        (mid(ABOVE_THRESHOLD), -40.0),
        (mid(ABOVE_RATIO), 4.0),
        (mid(ABOVE_ATTACK), 20.0),
        (mid(ABOVE_RELEASE), 50.0),
    ];
    near(time_constant(&above, -60.0, 0.0), 20.0, "Above attack");
    near(time_constant(&above, 0.0, -60.0), 50.0, "Above release");
    // Below: quiet takes the gain down (attack), loud lets it go (release).
    let below = [
        (mid(BELOW_THRESHOLD), -40.0),
        (mid(BELOW_RATIO), 0.5),
        (mid(BELOW_ATTACK), 30.0),
        (mid(BELOW_RELEASE), 80.0),
    ];
    near(time_constant(&below, 0.0, -60.0), 30.0, "Below attack");
    near(time_constant(&below, -60.0, 0.0), 80.0, "Below release");
}

// Spec: multiband-dynamics R8
#[test]
fn time_scales_every_attack_and_release_together() {
    for (time, factor) in [(50.0, 0.5), (200.0, 2.0)] {
        let params = [
            (mid(ABOVE_THRESHOLD), -40.0),
            (mid(ABOVE_RATIO), 4.0),
            (mid(ABOVE_ATTACK), 20.0),
            (mid(ABOVE_RELEASE), 50.0),
            (TIME, time),
        ];
        let attack = time_constant(&params, -60.0, 0.0);
        let release = time_constant(&params, 0.0, -60.0);
        assert!(
            (attack / (20.0 * factor) - 1.0).abs() < 0.05,
            "{time} %: {attack} ms"
        );
        assert!(
            (release / (50.0 * factor) - 1.0).abs() < 0.05,
            "{time} %: {release} ms"
        );
    }
}

// Spec: multiband-dynamics R9
#[test]
fn amount_scales_every_ratio_toward_one() {
    // Above at -20 dB, ratio 4, a 0 dB input: 15 dB of cut at 100 %, the
    // ratio's square root (2) at 50 %, none at 0 %, 16 at 200 %.
    for (amount, expected) in [(100.0, -15.0), (50.0, -10.0), (0.0, 0.0), (200.0, -18.75)] {
        let params = [
            (mid(ABOVE_THRESHOLD), -20.0),
            (mid(ABOVE_RATIO), 4.0),
            (AMOUNT, amount),
        ];
        let got = gain_db(&params, 0.0, None);
        assert!((got - expected).abs() < 0.1, "{amount} %: {got} dB");
    }
    // At 0 % the device nulls against the crossovers alone, however its
    // ratios are set.
    let noise = common::noise(8_192);
    let mut params = vec![(AMOUNT, 0.0)];
    for band in [LOW, MID, HIGH] {
        params.extend([
            (band_param(band, ABOVE_THRESHOLD), -60.0),
            (band_param(band, ABOVE_RATIO), 8.0),
            (band_param(band, BELOW_THRESHOLD), -50.0),
            (band_param(band, BELOW_RATIO), 0.3),
        ]);
    }
    let (mut node, _) = rig(&params, 8_192);
    let got = run(node.as_mut(), &noise, None);
    let null = null_db(&got, &crossover_reference(&noise, 500.0, 2_000.0));
    assert!(null < -100.0, "{null} dB");
}

// Spec: multiband-dynamics R10
#[test]
fn rms_is_less_sensitive_to_a_short_peak() {
    // A millisecond burst at full scale over a quiet floor. The peak
    // detector follows it; the RMS one barely moves.
    let mut input = vec![db_to_gain(-60.0) as f32; 9_600];
    input[4_800..4_848].fill(1.0);
    let at_burst_end = |detector: f64| {
        let params = one_band(&[
            (mid(ABOVE_THRESHOLD), -20.0),
            (mid(ABOVE_RATIO), 100.0),
            (mid(ABOVE_ATTACK), 0.5),
            (mid(ABOVE_RELEASE), 10.0),
            (DETECTOR, detector),
        ]);
        let (mut node, _) = rig(&params, 9_600);
        // The burst's last sample, after the attack has had its 48 frames.
        run(node.as_mut(), &input, None)[4_847].abs()
    };
    let (peak, rms) = (at_burst_end(0.0), at_burst_end(1.0));
    assert!(
        db(f64::from(peak)) < -10.0,
        "the peak detector cuts the burst: {peak}"
    );
    assert!(rms > 2.0 * peak, "RMS cuts it less: {rms} against {peak}");
}

// Spec: multiband-dynamics R11
#[test]
fn a_bands_gains_act_before_and_after_its_dynamics_and_an_inactive_band_passes() {
    let level = |params: &[(ParamId, f64)]| {
        let input = vec![0.25f32; 9_600];
        let (mut node, _) = rig(&one_band(params), 4_800);
        let out = stream(node.as_mut(), &input, 4_800, None);
        f64::from(out[9_000])
    };
    assert_eq!(level(&[]), 0.25);
    assert_eq!(level(&[(mid(INPUT), 2.0), (mid(BAND_OUTPUT), 0.5)]), 0.25);
    assert_eq!(level(&[(mid(INPUT), 0.0)]), 0.0, "Input reaches -inf dB");
    assert_eq!(
        level(&[(mid(BAND_OUTPUT), 0.0)]),
        0.0,
        "Output reaches -inf dB"
    );
    assert_eq!(level(&[(mid(INPUT), 4.0)]), 1.0);
    // The detector hears the band after Input: a -26 dB signal with +12 dB
    // of Input is 6 dB over a -20 dB threshold, and at an infinite ratio
    // is cut to the threshold: out at -20 dB less the 12 dB Input's boost
    // being kept, -26 + 12 - 5.94.
    let params = [
        (mid(INPUT), db_to_gain(12.0)),
        (mid(ABOVE_THRESHOLD), -20.0),
        (mid(ABOVE_RATIO), 100.0),
    ];
    let got = gain_db(&params, -26.0, None);
    assert!((got - (12.0 - 5.94)).abs() < 0.1, "{got} dB");
    // An inactive band passes its input as it is: no Input, no Output, no
    // dynamics.
    let off = [
        (mid(ACTIVE), 0.0),
        (mid(INPUT), 0.0),
        (mid(BAND_OUTPUT), 0.0),
        (mid(ABOVE_THRESHOLD), -70.0),
        (mid(ABOVE_RATIO), 100.0),
    ];
    assert_eq!(level(&off), 0.25);
    // Switched off while it was cutting, a band reports no gain change.
    let tone = sine(1_000.0, 0.5, 4_800);
    let (mut node, mut reader) = rig(
        &[(mid(ABOVE_THRESHOLD), -30.0), (mid(ABOVE_RATIO), 100.0)],
        4_800,
    );
    run(node.as_mut(), &tone, None);
    assert!(published(&mut reader)[3] < -10.0, "cutting");
    let mut data: Vec<f32> = tone.iter().chain(&tone).copied().collect();
    let events = [common::set(0, mid(ACTIVE), 0.0)];
    let mut block = Block::new(
        &mut data,
        2,
        4_800,
        4_800,
        &events,
        TransportInfo::default(),
        RATE,
    );
    bl_rt::rt_test!({ node.process(&mut block) });
    assert_eq!(published(&mut reader)[3], 0.0, "no longer cutting");
}

// Spec: multiband-dynamics R12
#[test]
fn a_soloed_band_leaves_the_others_at_minus_infinity() {
    let frames = 9_600;
    let input = mix(&[
        sine(100.0, 0.3, frames),
        sine(1_000.0, 0.3, frames),
        sine(8_000.0, 0.3, frames),
    ]);
    let levels = |params: &[(ParamId, f64)]| {
        let (mut node, _) = rig(params, frames);
        let out = run(node.as_mut(), &input, None);
        [100.0, 1_000.0, 8_000.0].map(|hz| db(tone_level(&out[4_800..], hz) / 0.3))
    };
    let all = levels(&[]);
    assert!(all.iter().all(|l| l.abs() < 0.1), "{all:?}");
    let [low, mid_, high] = levels(&[(band_param(MID, SOLO), 1.0)]);
    assert!(low < -35.0 && high < -35.0, "{low} {high}");
    assert!(mid_ > -2.0, "the Mid band stays: {mid_}");
    let [low, mid_, high] = levels(&[(band_param(LOW, SOLO), 1.0), (band_param(HIGH, SOLO), 1.0)]);
    // The Mid tone is an octave from each crossover, so each of the other
    // two bands holds it 24 dB down.
    assert!(mid_ < -17.0, "{mid_}");
    assert!(low > -2.0 && high > -2.0, "{low} {high}");
    // A solo on a band that doesn't exist mutes nothing.
    let none = levels(&[(LOW_ON, 0.0), (band_param(LOW, SOLO), 1.0)]);
    assert!(none[1].abs() < 0.1 && none[2].abs() < 0.1, "{none:?}");
}

// Spec: multiband-dynamics R13
#[test]
fn output_is_the_whole_devices_gain() {
    let level = |output: f64| {
        let input = vec![0.25f32; 4_800];
        let (mut node, _) = rig(&one_band(&[(OUTPUT, output)]), 4_800);
        f64::from(run(node.as_mut(), &input, None)[4_000])
    };
    assert_eq!(level(1.0), 0.25);
    assert_eq!(level(2.0), 0.5);
    assert_eq!(level(0.0), 0.0, "Output reaches -inf dB");
}

// Spec: multiband-dynamics R14
#[test]
fn a_key_from_another_track_drives_every_bands_detector() {
    // Three quiet tones, one in each band, below every Above threshold.
    let frames = 24_000;
    let quiet = |amplitude: f64| {
        mix(&[
            sine(100.0, amplitude, frames),
            sine(1_000.0, amplitude, frames),
            sine(8_000.0, amplitude, frames),
        ])
    };
    let mut params = vec![];
    for band in [LOW, MID, HIGH] {
        params.extend([
            (band_param(band, ABOVE_THRESHOLD), -20.0),
            (band_param(band, ABOVE_RATIO), 100.0),
            (band_param(band, ABOVE_ATTACK), 1.0),
        ]);
    }
    let input = quiet(0.03);
    let (mut node, mut reader) = rig(&params, 4_800);
    stream(node.as_mut(), &input, 4_800, None);
    let alone = published(&mut reader);
    assert!(
        [0, 3, 6].iter().all(|i| alone[*i].abs() < 0.5),
        "alone they pass: {alone:?}"
    );
    // The same tones, loud, as the key: each band's detector hears its own
    // band of it and turns that band down.
    let key = quiet(0.5);
    let (mut node, mut reader) = rig(&params, 4_800);
    stream(node.as_mut(), &input, 4_800, Some(&key));
    let keyed = published(&mut reader);
    for (band, index) in [(LOW, 0), (MID, 3), (HIGH, 6)] {
        assert!(keyed[index] < -8.0, "band {band}: {} dB", keyed[index]);
    }
    // A key that is silent in a band leaves that band alone, whatever the
    // input is doing there.
    let key = sine(100.0, 0.5, frames);
    let (mut node, mut reader) = rig(&params, 4_800);
    stream(node.as_mut(), &quiet(0.5), 4_800, Some(&key));
    let keyed = published(&mut reader);
    assert!(
        keyed[0] < -8.0 && keyed[3].abs() < 0.5 && keyed[6].abs() < 0.5,
        "{keyed:?}"
    );
}

// Spec: multiband-dynamics R15
#[test]
fn the_display_holds_each_bands_curve_thresholds_and_readings() {
    let device_type = multiband_dynamics_type();
    let description = &device_type.description;
    let mut values: OrdMap<ParamId, f64> = OrdMap::new();
    values.insert(band_param(MID, ABOVE_THRESHOLD), -20.0);
    values.insert(band_param(MID, ABOVE_RATIO), 4.0);
    values.insert(HIGH_ON, 0.0);
    let id = DeviceId(bl_core::id::Id::new(1).unwrap());
    let shown = bl_devices::display(MULTIBAND_DYNAMICS, id, description, &values, None)
        .expect("it draws a display");
    assert!(!shown.live);
    let names: Vec<_> = shown.series.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(
        names,
        ["Low Transfer Curve", "Mid Transfer Curve"],
        "no High band"
    );
    let curve = &shown.series[1];
    assert_eq!(curve.values.len(), 71);
    // Down to -20 dB the curve is the diagonal; at 0 dB it is 15 dB lower.
    assert!((curve.values[50] + 20.0).abs() < 0.01);
    assert!((curve.values[70] + 15.0).abs() < 0.01);
    let marker = shown
        .markers
        .iter()
        .find(|m| m.name == "Mid Above Threshold")
        .expect("the threshold is marked");
    assert_eq!((marker.x, marker.y), (-20.0, -20.0));
    assert_eq!(
        shown.markers.len(),
        4,
        "two thresholds on each band's curve"
    );
    assert!(shown.readings.is_empty(), "no engine, no readings");
    // With the node's data: each existing band's gain, input and output.
    let data = [-12.0f32, -6.0, -8.0, 0.0, -3.0, -3.0, 0.0, -9.0, -9.0];
    let shown =
        bl_devices::display(MULTIBAND_DYNAMICS, id, description, &values, Some(&data)).unwrap();
    assert!(shown.live);
    let reading = |name: &str| {
        shown
            .readings
            .iter()
            .find(|r| r.name == name)
            .map(|r| r.value)
    };
    assert_eq!(reading("Low Gain Reduction"), Some(-12.0));
    assert_eq!(reading("Mid Input"), Some(-3.0));
    assert_eq!(reading("Mid Output"), Some(-3.0));
    assert_eq!(reading("High Input"), None, "no High band");
}

// Spec: multiband-dynamics R16
#[test]
fn renders_are_the_same_at_any_block_size() {
    let input = common::noise(12_288);
    let key = sine(150.0, 0.7, 12_288);
    let mut params = vec![(SOFT_KNEE, 1.0), (DETECTOR, 1.0), (TIME, 150.0)];
    for band in [LOW, MID, HIGH] {
        params.extend([
            (band_param(band, ABOVE_THRESHOLD), -24.0),
            (band_param(band, ABOVE_RATIO), 3.0),
            (band_param(band, BELOW_THRESHOLD), -45.0),
            (band_param(band, BELOW_RATIO), 0.6),
            (band_param(band, INPUT), db_to_gain(3.0)),
        ]);
    }
    let render = |block: usize, key: Option<&[f32]>| {
        let (mut node, _) = rig(&params, block);
        stream(node.as_mut(), &input, block, key)
    };
    for key in [None, Some(&key[..])] {
        let reference = render(4_096, key);
        assert!(reference.iter().any(|s| *s != 0.0));
        for block in [1, 64, 512] {
            assert_eq!(render(block, key), reference, "block size {block}");
        }
    }
    // And run after run.
    assert_eq!(render(512, None), render(512, None));
}

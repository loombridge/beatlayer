//! The FM core (WP-8.6; the Operator spec, R1 to R10): the eleven
//! algorithms, the Bessel sidebands, feedback, the frequency laws, the
//! waveforms and their fold-back, user waves, and the aliasing controls,
//! through the test FM device on the voice section.
//!
//! Every block runs as a real-time section under the allocation trap.

use crate::common::{self, CHANNELS};
use core::f64::consts::TAU;

use bl_core::time::SampleRate;
use bl_devices::fm::{self, UserWave};
use bl_devices::testing::{
    self, FM_ALGORITHM, FM_ANTIALIAS, FM_COARSE, FM_FEEDBACK, FM_FINE, FM_FIXED, FM_FREQ,
    FM_INTERPOLATION, FM_LEVEL, FM_MULTI, FM_ON, FM_PHASE, FM_Q, FM_RETRIGGER, FM_TONE, FM_VEL,
    FM_VOICES, FM_WAVE, fm_param,
};
use bl_engine::{
    Block, Collector, Node, NodeConfig, NoteBuffer, NoteEvent, NoteKind, TransportInfo,
};
use bl_model::ParamId;
use bl_model::param::db_to_gain;
use realfft::RealFftPlanner;

const HZ: f64 = 48_000.0;

/// Key 57, 220 Hz exactly.
const A3: u8 = 57;
const F0: f64 = 220.0;

fn on(offset: u32, key: u8, velocity: f32) -> NoteEvent {
    NoteEvent {
        offset,
        kind: NoteKind::On,
        channel: 0,
        key,
        velocity,
        note_id: None,
    }
}

fn off(offset: u32, key: u8) -> NoteEvent {
    NoteEvent {
        offset,
        kind: NoteKind::Off,
        channel: 0,
        key,
        velocity: 0.0,
        note_id: None,
    }
}

/// A new device's parameters, but for Antialias off and Tone at 100 %, so
/// a test sees the operators' own sound; `extra` overrides them.
fn patch(extra: &[(ParamId, f64)]) -> Vec<(ParamId, f64)> {
    let mut params = vec![(FM_ANTIALIAS, 0.0), (FM_TONE, 100.0)];
    params.extend_from_slice(extra);
    params
}

fn node_at(rate: SampleRate, params: &[(ParamId, f64)], max_frames: usize) -> Box<dyn Node> {
    let device = common::device(&testing::fm_type(), params);
    testing::registry()
        .create(
            &device,
            &NodeConfig {
                round_trip_latency: 0,
                sample_rate: rate,
                max_frames,
                channels: CHANNELS,
            },
        )
        .expect("the registry makes the device")
}

fn node(params: &[(ParamId, f64)]) -> Box<dyn Node> {
    node_at(common::RATE, params, 4096)
}

/// Runs one block through `node`, playing `notes`, and returns the left
/// channel.
fn play_block(
    node: &mut dyn Node,
    rate: SampleRate,
    frames: usize,
    notes: &[NoteEvent],
) -> Vec<f32> {
    let mut data = vec![0.0f32; CHANNELS * frames];
    let mut out = NoteBuffer::with_capacity(64);
    {
        let mut block = Block::new(
            &mut data,
            CHANNELS,
            frames,
            frames,
            &[],
            TransportInfo::default(),
            rate,
        )
        .with_notes(notes, &mut out);
        bl_rt::rt_test!({ node.process(&mut block) });
    }
    data[..frames].to_vec()
}

/// Plays `notes` for `frames`, in blocks of `block`.
fn play_blocks(
    node: &mut dyn Node,
    rate: SampleRate,
    frames: usize,
    block: usize,
    notes: &[NoteEvent],
) -> Vec<f32> {
    let mut signal = Vec::with_capacity(frames);
    let mut at = 0;
    while at < frames {
        let len = block.min(frames - at);
        let block_notes: Vec<NoteEvent> = notes
            .iter()
            .filter(|note| (note.offset as usize) >= at && (note.offset as usize) < at + len)
            .map(|note| NoteEvent {
                offset: note.offset - at as u32,
                ..*note
            })
            .collect();
        signal.extend(play_block(node, rate, len, &block_notes));
        at += len;
    }
    signal
}

/// Plays `notes` for `frames`, in blocks of the node's size.
fn play(node: &mut dyn Node, rate: SampleRate, frames: usize, notes: &[NoteEvent]) -> Vec<f32> {
    play_blocks(node, rate, frames, 4096, notes)
}

/// A note of `key` held for `frames` frames, played at 48 kHz in one block.
fn held(params: &[(ParamId, f64)], key: u8, frames: usize) -> Vec<f32> {
    play(
        node(params).as_mut(),
        common::RATE,
        frames,
        &[on(0, key, 1.0)],
    )
}

fn frequency(key: u8) -> f64 {
    440.0 * 2f64.powf((f64::from(key) - 69.0) / 12.0)
}

/// A level of `amplitude` as a parameter value.
fn level(op: usize, amplitude: f64) -> (ParamId, f64) {
    (fm_param(op, FM_LEVEL), amplitude)
}

fn silent(op: usize) -> (ParamId, f64) {
    level(op, 0.0)
}

/// The amplitude of the sine at `hz` in `signal`: a Hann-windowed DFT over
/// the whole slice.
fn amplitude_at(signal: &[f32], hz: f64) -> f64 {
    let n = signal.len() as f64;
    let (mut re, mut im, mut weights) = (0.0f64, 0.0f64, 0.0f64);
    for (i, &x) in signal.iter().enumerate() {
        let w = 0.5 - 0.5 * (TAU * i as f64 / n).cos();
        let phase = TAU * hz * i as f64 / HZ;
        re += f64::from(x) * w * phase.cos();
        im -= f64::from(x) * w * phase.sin();
        weights += w;
    }
    2.0 * re.hypot(im) / weights
}

fn db(amplitude: f64) -> f64 {
    20.0 * amplitude.max(1e-12).log10()
}

fn rms(signal: &[f32]) -> f64 {
    (signal.iter().map(|&x| f64::from(x).powi(2)).sum::<f64>() / signal.len() as f64).sqrt()
}

/// The peak of `signal`.
fn peak(signal: &[f32]) -> f32 {
    signal.iter().fold(0.0, |peak, x| peak.max(x.abs()))
}

/// The energy outside a note's harmonics over the energy in them, in dB:
/// the aliasing level. A 7-term Blackman–Harris window, 8,192-frame
/// spectra overlapping by half, and 9 bins either side of each harmonic,
/// as the kit's own measurement does.
fn aliasing_db(signal: &[f32], fundamental: f64) -> f64 {
    const N: usize = 8_192;
    const A: [f64; 7] = [
        0.271_051_400_693_42,
        0.433_297_939_234_48,
        0.218_122_999_543_11,
        0.065_925_446_388_03,
        0.010_811_742_098_37,
        0.000_776_584_825_22,
        0.000_013_887_217_35,
    ];
    let window: Vec<f64> = (0..N)
        .map(|i| {
            let x = core::f64::consts::TAU * i as f64 / (N - 1) as f64;
            A.iter()
                .enumerate()
                .map(|(k, a)| {
                    let sign = if k % 2 == 0 { 1.0 } else { -1.0 };
                    sign * a * (k as f64 * x).cos()
                })
                .sum()
        })
        .collect();
    let fft = RealFftPlanner::<f64>::new().plan_fft_forward(N);
    let mut input = fft.make_input_vec();
    let mut spectrum = fft.make_output_vec();
    let bin = HZ / N as f64;
    let margin = 9.0 * bin;
    let (mut harmonic, mut other) = (0.0, 0.0);
    for start in (0..=signal.len() - N).step_by(N / 2) {
        for ((slot, &x), &w) in input.iter_mut().zip(&signal[start..start + N]).zip(&window) {
            *slot = f64::from(x) * w;
        }
        fft.process(&mut input, &mut spectrum).unwrap();
        for (k, value) in spectrum.iter().enumerate() {
            let hz = k as f64 * bin;
            if hz < margin || hz >= HZ / 2.0 {
                continue;
            }
            let energy = value.norm_sqr();
            let lowest = ((hz - margin) / fundamental).ceil().max(1.0);
            let highest = ((hz + margin) / fundamental).floor();
            if lowest <= highest && lowest * fundamental < HZ / 2.0 {
                harmonic += energy;
            } else {
                other += energy;
            }
        }
    }
    10.0 * (other / harmonic).log10()
}

/// Bessel's function of the first kind, order `n`, at `x`: the integral
/// (1/π) ∫ cos(nτ − x sin τ) dτ over 0 to π, by Simpson's rule.
fn bessel(n: i32, x: f64) -> f64 {
    const STEPS: usize = 2_000;
    let h = core::f64::consts::PI / STEPS as f64;
    let f = |t: f64| (f64::from(n) * t - x * t.sin()).cos();
    let mut sum = f(0.0) + f(core::f64::consts::PI);
    for i in 1..STEPS {
        sum += f(i as f64 * h) * if i % 2 == 1 { 4.0 } else { 2.0 };
    }
    sum * h / 3.0 / core::f64::consts::PI
}

/// The edges of each algorithm's graph as the manual's figure draws them:
/// (modulator, modulated) with A as 0 to D as 3, and its carriers.
const EDGES: [&[(usize, usize)]; 11] = [
    &[(3, 2), (2, 1), (1, 0)],
    &[(2, 1), (3, 1), (1, 0)],
    &[(2, 1), (1, 0), (3, 0)],
    &[(3, 1), (3, 2), (1, 0), (2, 0)],
    &[(3, 2), (2, 0), (2, 1)],
    &[(3, 2), (2, 1)],
    &[(1, 0), (2, 0), (3, 0)],
    &[(1, 0), (3, 2)],
    &[(3, 0), (3, 1), (3, 2)],
    &[(3, 2)],
    &[],
];
const CARRIERS: [&[usize]; 11] = [
    &[0],
    &[0],
    &[0],
    &[0],
    &[0, 1],
    &[0, 1],
    &[0],
    &[0, 2],
    &[0, 1, 2],
    &[0, 1, 2],
    &[0, 1, 2, 3],
];

fn algorithm(number: usize) -> (ParamId, f64) {
    (FM_ALGORITHM, number as f64)
}

// Spec: operator R2
#[test]
fn switching_one_operator_on_at_a_time_reaches_the_output_only_through_a_carrier() {
    for number in 1..=11 {
        for op in 0..4 {
            let mut params = vec![algorithm(number)];
            for other in 0..4 {
                params.push(if other == op {
                    level(other, 1.0)
                } else {
                    silent(other)
                });
            }
            let signal = held(&patch(&params), A3, 4_800);
            let heard = rms(&signal[2_400..]) > 0.01;
            assert_eq!(
                heard,
                CARRIERS[number - 1].contains(&op),
                "algorithm {number}, operator {op} alone"
            );
            if !heard {
                assert_eq!(peak(&signal), 0.0, "algorithm {number}, operator {op}");
            }
        }
    }
}

// Spec: operator R1, R2, R10
#[test]
fn every_algorithm_connects_the_operators_as_the_manuals_figure_draws_it() {
    // The graph, played in plain f64: D first, each operator's phase moved
    // by four radians per unit of its modulators' outputs, the carriers
    // averaged.
    let reference = |number: usize, n: usize| {
        let mut y = [0.0f64; 4];
        for op in (0..4).rev() {
            let theta = TAU * F0 * (op + 1) as f64 * (n + 1) as f64 / HZ;
            let moved: f64 = EDGES[number - 1]
                .iter()
                .filter(|(_, to)| *to == op)
                .map(|(from, _)| y[*from])
                .sum();
            y[op] = 0.5 * (theta + 4.0 * moved).sin();
        }
        let carriers = CARRIERS[number - 1];
        carriers.iter().map(|&c| y[c]).sum::<f64>() / carriers.len() as f64
    };
    for number in 1..=11 {
        let mut params = vec![algorithm(number)];
        for op in 0..4 {
            params.push(level(op, 0.5));
            params.push((fm_param(op, FM_COARSE), (op + 1) as f64));
        }
        let signal = held(&patch(&params), A3, 4_000);
        let worst = (200..4_000)
            .map(|n| (f64::from(signal[n]) - reference(number, n)).abs())
            .fold(0.0, f64::max);
        assert!(worst < 1e-3, "algorithm {number}: off by {worst}");
    }
}

// Spec: operator R1
#[test]
fn a_modulated_carrier_has_its_sidebands_at_the_bessel_amplitudes() {
    // Algorithm 1 with C and D silent: B, at amplitude a, moves A's phase
    // by β = 4a radians, both at ratio 1: sin(θ + β sin θ). Harmonic h of
    // it has amplitude J(h−1, β) + (−1)^h J(h+1, β).
    for beta in [0.5, 1.0, 2.0, 4.0, 6.0, 8.0] {
        // A level tops out at 1, so a β past 4 takes three modulators
        // (algorithm 7: B, C and D into A), each at a third of it.
        let params = if beta <= 4.0 {
            patch(&[level(0, 1.0), level(1, beta / 4.0), silent(2), silent(3)])
        } else {
            patch(&[
                algorithm(7),
                level(0, 1.0),
                level(1, beta / 12.0),
                level(2, beta / 12.0),
                level(3, beta / 12.0),
            ])
        };
        let signal = held(&params, A3, 24_000);
        let steady = &signal[4_000..];
        for h in 1..=14 {
            let expected = (bessel(h - 1, beta) + (-1f64).powi(h) * bessel(h + 1, beta)).abs();
            if expected < 0.02 {
                continue;
            }
            let heard = amplitude_at(steady, F0 * f64::from(h));
            let error = db(heard) - db(expected);
            assert!(
                error.abs() < 0.5,
                "β {beta}, harmonic {h}: {heard:.4} against {expected:.4} ({error:+.2} dB)"
            );
        }
    }
}

// Spec: operator R3
#[test]
fn feedback_adds_harmonics_to_an_unmodulated_operator_and_never_to_a_modulated_one() {
    let on_a = |feedback: f64| {
        let params = patch(&[algorithm(11), (fm_param(0, FM_FEEDBACK), feedback)]);
        held(&params, A3, 8_000)
    };
    let (plain, fed) = (on_a(0.0), on_a(100.0));
    assert!(amplitude_at(&plain[2_000..], 2.0 * F0) < 1e-4);
    assert!(
        amplitude_at(&fed[2_000..], 2.0 * F0) > 0.01,
        "feedback makes a second harmonic"
    );
    // In algorithm 1, B modulates A and C modulates B: feedback on either
    // does nothing; on D, which nothing modulates, it changes the sound.
    let chain = |op: usize, feedback: f64| {
        let mut params = vec![algorithm(1)];
        params.extend((0..4).map(|o| level(o, 0.5)));
        params.push((fm_param(op, FM_FEEDBACK), feedback));
        held(&patch(&params), A3, 4_000)
    };
    for op in [0, 1, 2] {
        assert_eq!(
            chain(op, 0.0),
            chain(op, 100.0),
            "operator {op} is modulated"
        );
    }
    assert_ne!(chain(3, 0.0), chain(3, 100.0), "operator D is not");
}

// Spec: operator R3
#[test]
fn feedback_at_its_maximum_stays_bounded_from_44_1_to_192_khz() {
    for rate in [
        SampleRate::HZ_44100,
        SampleRate::HZ_48000,
        SampleRate::HZ_96000,
        SampleRate::HZ_192000,
    ] {
        for wave in [0.0, 9.0, 19.0] {
            // Every operator at full level and full feedback, all carriers
            // (algorithm 11), with Antialias on.
            let mut params = vec![(FM_ANTIALIAS, 1.0), algorithm(11)];
            for op in 0..4 {
                params.push(level(op, 1.0));
                params.push((fm_param(op, FM_FEEDBACK), 100.0));
                params.push((fm_param(op, FM_WAVE), wave));
            }
            let mut node = node_at(rate, &params, 4096);
            let signal = play(
                node.as_mut(),
                rate,
                rate.hz() as usize / 2,
                &[on(0, 40, 1.0), on(0, 64, 1.0), on(0, 100, 1.0)],
            );
            let top = peak(&signal);
            assert!(top.is_finite(), "{} Hz, wave {wave}", rate.hz());
            // The waves peak at about 1; the three notes sum at most 3 and
            // the carriers are averaged.
            assert!(
                top < 3.5 && top > 0.1,
                "{} Hz, wave {wave}: peak {top}",
                rate.hz()
            );
        }
    }
}

// Spec: operator R4
#[test]
fn coarse_and_fine_set_the_ratio_of_the_notes_frequency() {
    let heard = |coarse: f64, fine: f64, hz: f64| {
        let params = patch(&[
            (fm_param(0, FM_COARSE), coarse),
            (fm_param(0, FM_FINE), fine),
        ]);
        amplitude_at(&held(&params, A3, 8_000)[2_000..], hz)
    };
    assert!((heard(1.0, 0.0, F0) - 1.0).abs() < 0.01);
    assert!(
        (heard(0.0, 0.0, F0 / 2.0) - 1.0).abs() < 0.01,
        "Coarse 0 is a ratio of 0.5"
    );
    assert!((heard(3.0, 500.0, 3.5 * F0) - 1.0).abs() < 0.01);
    assert!((heard(48.0, 1000.0, 49.0 * F0) - 1.0).abs() < 0.01);
}

// Spec: operator R4
#[test]
fn fixed_frequency_ignores_the_key_and_reaches_down_to_a_tenth_of_a_hertz() {
    let fixed = |freq: f64, multi: f64| {
        patch(&[
            (fm_param(0, FM_FIXED), 1.0),
            (fm_param(0, FM_FREQ), freq),
            (fm_param(0, FM_MULTI), multi),
        ])
    };
    // Multi 1 is index 1: 440 Hz whatever the key.
    let (low, high) = (
        held(&fixed(440.0, 1.0), 30, 8_000),
        held(&fixed(440.0, 1.0), 100, 8_000),
    );
    assert_eq!(low, high);
    assert!((amplitude_at(&low[2_000..], 440.0) - 1.0).abs() < 0.01);
    // Freq 1 × Multi 0.1 (index 0): a sine of 0.1 Hz, ten seconds a cycle.
    let slow = held(&fixed(1.0, 0.0), 60, 480_000);
    let worst = (200..480_000)
        .map(|n| (f64::from(slow[n]) - (TAU * 0.1 * (n + 1) as f64 / HZ).sin()).abs())
        .fold(0.0, f64::max);
    assert!(worst < 1e-3, "off by {worst}");
    // 20 kHz sounds; 22 kHz, past 0.45 of the sample rate, doesn't.
    assert!(rms(&held(&fixed(2.0, 5.0), 60, 4_800)[2_400..]) > 0.1);
    assert_eq!(peak(&held(&fixed(2.2, 5.0), 60, 4_800)), 0.0);
}

// Spec: operator R4
#[test]
fn osc_vel_moves_the_ratio_by_up_to_an_octave_and_q_rounds_it_to_a_coarse_step() {
    let heard = |quantise: f64, hz: f64| {
        let params = patch(&[
            (fm_param(0, FM_COARSE), 3.0),
            (fm_param(0, FM_VEL), 50.0),
            (fm_param(0, FM_Q), quantise),
        ]);
        let signal = play(
            node(&params).as_mut(),
            common::RATE,
            8_000,
            &[on(0, A3, 1.0)],
        );
        amplitude_at(&signal[2_000..], hz)
    };
    // 3 × 2^0.5 = 4.243 at full velocity: 4 with Q, itself without.
    assert!((heard(1.0, 4.0 * F0) - 1.0).abs() < 0.01);
    assert!((heard(0.0, 3.0 * 2f64.sqrt() * F0) - 1.0).abs() < 0.01);
}

// Spec: operator R5
#[test]
fn level_and_the_switch_set_what_an_operator_gives() {
    let heard = |params: &[(ParamId, f64)]| peak(&held(&patch(params), A3, 4_800));
    assert!((heard(&[level(0, db_to_gain(-6.0))]) - 0.5).abs() < 0.01);
    assert_eq!(heard(&[silent(0)]), 0.0);
    assert_eq!(heard(&[(fm_param(0, FM_ON), 0.0)]), 0.0);
    // A modulator that is off modulates nothing.
    let plain = heard(&[algorithm(11)]);
    assert_eq!(
        plain,
        heard(&[level(1, 1.0), (fm_param(1, FM_ON), 0.0), algorithm(11)])
    );
}

// Spec: operator R10
#[test]
fn no_algorithm_is_louder_than_a_single_carrier_at_the_same_level() {
    for number in 1..=11 {
        let mut params = vec![algorithm(number)];
        params.extend((0..4).map(|op| level(op, 1.0)));
        let top = peak(&held(&patch(&params), A3, 4_800));
        assert!(top <= 1.001, "algorithm {number}: {top}");
    }
    // Algorithm 11 with four equal carriers sounds as one of them alone
    // does at four times its level.
    let mut four = vec![algorithm(11)];
    four.extend((0..4).map(|op| level(op, 1.0)));
    let one = held(&patch(&[algorithm(11), level(0, 1.0)]), A3, 4_800);
    let each = held(&patch(&four), A3, 4_800);
    let worst = one
        .iter()
        .zip(&each)
        .map(|(a, b)| (a * 4.0 - b).abs())
        .fold(0.0, f32::max);
    assert!(worst < 1e-6);
    assert!((peak(&each) - 1.0).abs() < 0.01);
}

// Spec: operator R8
#[test]
fn phase_is_where_a_note_starts_and_retrigger_off_runs_freely() {
    let play_twice = |retrigger: f64| {
        let params = patch(&[
            (FM_VOICES, 1.0),
            (fm_param(0, FM_PHASE), 90.0),
            (fm_param(0, FM_RETRIGGER), retrigger),
        ]);
        play_blocks(
            node(&params).as_mut(),
            common::RATE,
            16_000,
            1_024,
            &[
                on(0, 58, 1.0),
                off(3_000, 58),
                on(12_000, 58, 1.0),
                off(15_000, 58),
            ],
        )
    };
    let hz = frequency(58);
    let fixed_start = play_twice(1.0);
    // A quarter of a cycle on: the note is a cosine.
    let worst = (200..2_500)
        .map(|n| {
            (f64::from(fixed_start[n]) - (TAU * (hz * (n + 1) as f64 / HZ + 0.25)).sin()).abs()
        })
        .fold(0.0, f64::max);
    assert!(worst < 1e-3, "off by {worst}");
    let difference = |signal: &[f32]| {
        (500..2_500)
            .map(|n| (signal[n] - signal[n + 12_000]).abs())
            .fold(0.0, f32::max)
    };
    assert!(
        difference(&fixed_start) < 1e-5,
        "every note starts at the same phase"
    );
    assert!(
        difference(&play_twice(0.0)) > 0.01,
        "free-running notes don't"
    );
}

// Spec: operator R6
#[test]
fn every_wave_but_the_noises_and_the_d_forms_stays_below_minus_80_db_at_the_top_key() {
    // The saws, squares and triangle from their harmonics, and the sines
    // in 16 and 256 levels, all from the kit's band-limited tables.
    let waves: Vec<usize> = (0..fm::WAVES.len())
        .filter(|w| {
            ![
                fm::SAW_D,
                fm::SQUARE_D,
                fm::NOISE_LOOPED,
                fm::NOISE_WHITE,
                fm::USER,
            ]
            .contains(w)
        })
        .collect();
    for (antialias, tone) in [(0.0, 100.0), (1.0, 70.0)] {
        for &wave in &waves {
            for key in [127u8, 100, 60] {
                let params = patch(&[
                    (FM_ANTIALIAS, antialias),
                    (FM_TONE, tone),
                    (fm_param(0, FM_WAVE), wave as f64),
                    level(0, db_to_gain(-6.0)),
                ]);
                let signal = held(&params, key, 1 << 15);
                assert!(peak(&signal[4_096..]) > 0.05, "{} sounds", fm::WAVES[wave]);
                let worst = aliasing_db(&signal[4_096..], frequency(key));
                assert!(
                    worst < -80.0,
                    "{} at key {key}, Antialias {antialias}, Tone {tone}: {worst:.1} dB",
                    fm::WAVES[wave]
                );
            }
        }
    }
}

// Spec: operator R6
#[test]
fn the_sines_in_16_and_256_levels_are_less_pure_the_fewer_levels_they_have() {
    // The share of a sine's energy outside its fundamental, in dB.
    let impurity = |wave: usize| {
        // 48,000 frames after the attack: exactly 220 cycles.
        let signal = held(&patch(&[(fm_param(0, FM_WAVE), wave as f64)]), A3, 52_000);
        let steady = &signal[4_000..];
        let n = steady.len() as f64;
        let (mut re, mut im) = (0.0, 0.0);
        for (i, &x) in steady.iter().enumerate() {
            let angle = TAU * F0 * i as f64 / HZ;
            re += f64::from(x) * angle.cos();
            im += f64::from(x) * angle.sin();
        }
        let fundamental = 2.0 * re.hypot(im) / n;
        let total = rms(steady).powi(2);
        db(((total - fundamental * fundamental / 2.0).max(0.0) / total).sqrt())
    };
    let (pure, eight, four) = (impurity(0), impurity(2), impurity(1));
    println!("impurity: {pure:.1} dB, 8 bit {eight:.1} dB, 4 bit {four:.1} dB");
    assert!(pure < -100.0 && eight < -35.0 && four > eight + 15.0);
}

// Spec: operator R6
#[test]
fn the_noises_and_d_forms_play_and_the_noise_loop_repeats_every_second() {
    let wave = |wave: usize| (fm_param(0, FM_WAVE), wave as f64);
    let saw_d = held(&patch(&[wave(fm::SAW_D)]), A3, 4_800);
    assert!(
        (peak(&saw_d) - 1.0).abs() < 0.02,
        "a naive saw ramps up to 1"
    );
    let square_d = held(&patch(&[wave(fm::SQUARE_D)]), A3, 4_800);
    assert!(
        square_d[2_000..]
            .iter()
            .all(|x| (x.abs() - 1.0).abs() < 1e-6 || x.abs() < 1e-6)
    );
    assert!(rms(&held(&patch(&[wave(fm::NOISE_WHITE)]), A3, 4_800)[2_400..]) > 0.3);
    let looped = held(&patch(&[wave(fm::NOISE_LOOPED)]), A3, 100_000);
    assert!(rms(&looped[2_400..]) > 0.3);
    let worst = (2_000..50_000)
        .map(|n| (looped[n] - looped[n + 48_000]).abs())
        .fold(0.0, f32::max);
    assert!(worst < 1e-5, "one second later it is the same noise");
}

/// The first `count` partials' levels of the user waves below.
fn drawn(count: usize) -> Vec<f32> {
    (0..count).map(|h| 1.0 / (1.0 + h as f32 * 0.5)).collect()
}

fn user_params() -> Vec<(ParamId, f64)> {
    patch(&[(fm_param(0, FM_WAVE), fm::USER as f64)])
}

/// A node playing user wave `table` on A, `block` frames at a time.
fn user_node(table: &UserWave) -> (Box<dyn Node>, Collector) {
    let mut node = node(&user_params());
    let collector = Collector::new();
    node.receive_data(collector.handle().data(table.clone()));
    (node, collector)
}

// Spec: operator R7
#[test]
fn a_user_waves_partials_play_within_a_tenth_of_a_decibel_of_their_levels() {
    for count in [16, 32, 64] {
        let levels = drawn(count);
        let table = UserWave {
            op: 0,
            table: fm::user_table(&levels, None),
        };
        let (mut node, _collector) = user_node(&table);
        let signal = play(node.as_mut(), common::RATE, 16_000, &[on(0, A3, 1.0)]);
        for (h, level) in levels.iter().enumerate() {
            let heard = amplitude_at(&signal[4_000..], F0 * (h + 1) as f64);
            let error = db(heard) - db(f64::from(*level));
            assert!(
                error.abs() < 0.1,
                "{count} partials, partial {}: {error:+.3} dB",
                h + 1
            );
        }
        // Nothing above them without Repeat.
        let above = amplitude_at(&signal[4_000..], F0 * (count + 1) as f64);
        assert!(
            db(above) < -90.0,
            "{count} partials: {:.1} dB above",
            db(above)
        );
    }
}

// Spec: operator R7
#[test]
fn repeat_goes_on_with_the_drawn_pattern_each_copy_fading_by_its_share() {
    let levels = drawn(16);
    let table = UserWave {
        op: 0,
        table: fm::user_table(&levels, Some(0.5)),
    };
    let (mut node, _collector) = user_node(&table);
    let signal = play(node.as_mut(), common::RATE, 16_000, &[on(0, A3, 1.0)]);
    for (partial, copy) in [(1, 0), (5, 0), (17, 1), (21, 1), (33, 2), (49, 3), (65, 4)] {
        let expected = f64::from(levels[(partial - 1) % 16]) * 0.5f64.powi(copy);
        let heard = amplitude_at(&signal[4_000..], F0 * partial as f64);
        let error = db(heard) - db(expected);
        assert!(error.abs() < 0.1, "partial {partial}: {error:+.3} dB");
    }
}

// Spec: operator R7
#[test]
fn a_user_wave_swaps_in_while_notes_play_without_allocating() {
    let collector = Collector::new();
    let mut node = node(&user_params());
    let second = UserWave {
        op: 0,
        table: fm::user_table(&[0.0, 1.0], None),
    };
    // The default user wave is its first partial alone: a sine.
    let mut signal = play(node.as_mut(), common::RATE, 8_000, &[on(0, A3, 1.0)]);
    let before = signal.len();
    node.receive_data(collector.handle().data(second));
    signal.extend(play(node.as_mut(), common::RATE, 8_000, &[]));
    signal.extend(play(node.as_mut(), common::RATE, 8_000, &[]));
    assert!(amplitude_at(&signal[4_000..before], F0) > 0.99);
    assert!(
        amplitude_at(&signal[before + 4_000..], F0) < 1e-3,
        "the first partial is gone"
    );
    assert!((amplitude_at(&signal[before + 4_000..], 2.0 * F0) - 1.0).abs() < 0.01);
}

// Spec: operator R9
#[test]
fn tone_limits_the_partials_every_table_plays() {
    // A saw of 64 harmonics at 261.6 Hz: 91 below Nyquist at Tone 100 %, a
    // quarter of that at 50 %, which leaves the 16-harmonic level.
    let key = 60;
    let hz = frequency(key);
    let saw = |tone: f64| {
        held(
            &patch(&[(FM_TONE, tone), (fm_param(0, FM_WAVE), 9.0)]),
            key,
            16_000,
        )
    };
    let (full, dark) = (saw(100.0), saw(50.0));
    let at = |signal: &[f32], h: usize| amplitude_at(&signal[4_000..], hz * h as f64);
    let ideal = |h: usize| 2.0 / core::f64::consts::PI / h as f64;
    assert!(
        (at(&full, 40) - ideal(40)).abs() < 0.002,
        "{}",
        at(&full, 40)
    );
    assert!((at(&dark, 12) - ideal(12)).abs() < 0.002);
    assert!(db(at(&dark, 24)) < -70.0, "{} dB", db(at(&dark, 24)));
}

// Spec: operator R9
#[test]
fn antialias_reports_its_latency_and_cuts_the_fold_back_of_a_hard_fm_tone() {
    let hard = |antialias: f64| {
        // B at ratio 7 and full level into A: sidebands far past Nyquist.
        let params = patch(&[
            (FM_ANTIALIAS, antialias),
            level(0, 1.0),
            level(1, 1.0),
            (fm_param(1, FM_COARSE), 7.0),
            silent(2),
            silent(3),
        ]);
        let mut node = node(&params);
        let signal = play(node.as_mut(), common::RATE, 1 << 15, &[on(0, 100, 1.0)]);
        (
            node.latency(),
            aliasing_db(&signal[4_096..], frequency(100)),
        )
    };
    let ((plain_latency, plain), (latency, anti)) = (hard(0.0), hard(1.0));
    assert_eq!(plain_latency, 0);
    assert_eq!(latency, 32, "one doubling: 32 frames");
    println!("fold-back: {plain:.1} dB off, {anti:.1} dB on");
    assert!(anti < plain - 3.0, "{anti:.1} dB against {plain:.1} dB");
}

// Spec: operator R9
#[test]
fn interpolation_off_reads_the_table_without_interpolating() {
    let alias = |interpolation: f64| {
        let params = patch(&[(FM_INTERPOLATION, interpolation)]);
        aliasing_db(&held(&params, A3, 1 << 15)[4_096..], F0)
    };
    let (on, off) = (alias(1.0), alias(0.0));
    println!("interpolation: {on:.1} dB on, {off:.1} dB off");
    assert!(off > on + 20.0, "{off:.1} dB off against {on:.1} dB on");
}

// Spec: operator R10
#[test]
fn every_block_size_renders_the_same() {
    // Noise, feedback, a steal, overlapping notes, a release, and
    // Antialias on, with notes off the blocks' edges.
    let params = [
        (FM_VOICES, 2.0),
        (FM_ANTIALIAS, 1.0),
        (FM_TONE, 70.0),
        algorithm(4),
        level(0, 0.5),
        level(1, 0.5),
        level(2, 0.4),
        level(3, 0.3),
        (fm_param(0, FM_WAVE), 0.0),
        (fm_param(1, FM_WAVE), 19.0),
        (fm_param(2, FM_WAVE), 5.0),
        (fm_param(3, FM_WAVE), fm::NOISE_WHITE as f64),
        (fm_param(3, FM_FEEDBACK), 40.0),
        (fm_param(2, FM_FIXED), 1.0),
        (fm_param(1, FM_VEL), 60.0),
    ];
    let notes = [
        on(0, 50, 0.8),
        on(1_237, 62, 0.5),
        on(2_001, 71, 1.0),
        off(4_999, 62),
        off(7_000, 71),
        on(9_000, 40, 0.3),
        off(12_001, 40),
        off(13_000, 50),
    ];
    let render =
        |block: usize| play_blocks(node(&params).as_mut(), common::RATE, 20_000, block, &notes);
    let reference = render(4_096);
    assert!(peak(&reference) > 0.05);
    for block in [1, 64, 512] {
        assert!(render(block) == reference, "block size {block}");
    }
}

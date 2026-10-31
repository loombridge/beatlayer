//! The physical models' tests (spec: physical-models R1 to R18). Each
//! measures a struck resonator's output the way the rule says: by the peaks
//! of its spectrum and the sizes of its partials over time.

use std::f64::consts::PI;

use super::*;
use crate::Random;
use lspk_dsp::simd::Level;

lspk_rt::install_trap!();

const RATE: f32 = 48_000.0;

/// Hann-windowed DFT magnitude of `x` at `f` hertz, scaled so a sine of
/// amplitude 1 reads about 1.
fn dft(x: &[f32], rate: f64, f: f64) -> f64 {
    let n = x.len();
    let (sin, cos) = (2.0 * PI * f / rate).sin_cos();
    let (mut c, mut s) = (1.0_f64, 0.0_f64);
    let (mut re, mut im, mut norm) = (0.0, 0.0, 0.0);
    for (i, &v) in x.iter().enumerate() {
        let window = 0.5 - 0.5 * (2.0 * PI * i as f64 / n as f64).cos();
        re += window * f64::from(v) * c;
        im += window * f64::from(v) * s;
        norm += window;
        (c, s) = (c * cos - s * sin, s * cos + c * sin);
    }
    2.0 * re.hypot(im) / norm
}

/// The frequency of the spectrum's peak within `span` (a fraction) of `f`.
fn peak(x: &[f32], rate: f64, f: f64, span: f64) -> f64 {
    let step = 2.0 * span * f / 40.0;
    let (mut best, mut at) = (-1.0, f);
    for i in 0..=40 {
        let candidate = f * (1.0 - span) + step * f64::from(i);
        let magnitude = dft(x, rate, candidate);
        if magnitude > best {
            (best, at) = (magnitude, candidate);
        }
    }
    let (mut lo, mut hi) = (at - step, at + step);
    for _ in 0..40 {
        let (a, b) = (lo + (hi - lo) / 3.0, hi - (hi - lo) / 3.0);
        if dft(x, rate, a) < dft(x, rate, b) {
            lo = a;
        } else {
            hi = b;
        }
    }
    (lo + hi) / 2.0
}

/// How many cents `a` is above `b`.
fn cents(a: f64, b: f64) -> f64 {
    1200.0 * (a / b).log2()
}

/// A power-of-two FFT's magnitudes, for finding where to look.
fn spectrum(x: &[f32], padded: usize) -> Vec<f64> {
    let n = x.len();
    let mut re: Vec<f64> = (0..padded)
        .map(|i| {
            if i < n {
                let window = 0.5 - 0.5 * (2.0 * PI * i as f64 / n as f64).cos();
                window * f64::from(x[i])
            } else {
                0.0
            }
        })
        .collect();
    let mut im = vec![0.0; padded];
    let mut j = 0;
    for i in 1..padded {
        let mut bit = padded >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= padded {
        let angle = -2.0 * PI / len as f64;
        for start in (0..padded).step_by(len) {
            for k in 0..len / 2 {
                let (s, c) = (angle * k as f64).sin_cos();
                let (a, b) = (start + k, start + k + len / 2);
                let (tr, ti) = (re[b] * c - im[b] * s, re[b] * s + im[b] * c);
                (re[b], im[b]) = (re[a] - tr, im[a] - ti);
                (re[a], im[a]) = (re[a] + tr, im[a] + ti);
            }
        }
        len *= 2;
    }
    (0..padded / 2).map(|i| re[i].hypot(im[i])).collect()
}

/// The fundamental of `x` between `lo` and `hi` hertz, to a cent or better.
fn fundamental(x: &[f32], rate: f64, lo: f64, hi: f64) -> f64 {
    // Sixty cycles of the lowest it might be, so the window's edges don't pull the peak.
    let window = &x[..x.len().min(((60.0 * rate / lo) as usize).max(1 << 14))];
    let padded = (window.len() * 4).next_power_of_two();
    let bins = spectrum(window, padded);
    let bin = |f: f64| (f * padded as f64 / rate) as usize;
    let (a, b) = (bin(lo), bin(hi).min(bins.len() - 1));
    let at = (a..=b)
        .max_by(|&i, &j| bins[i].total_cmp(&bins[j]))
        .unwrap();
    let found = at as f64 * rate / padded as f64;
    peak(window, rate, found, 0.002)
}

/// Voice 0's left and right output for `frames` frames of an excitation.
fn modal_render(bank: &mut ModalBank, excitation: &[f32]) -> (Vec<f32>, Vec<f32>) {
    let stride = bank.stride();
    let mut input = vec![0.0; excitation.len() * stride];
    for (frame, &value) in excitation.iter().enumerate() {
        input[frame * stride] = value;
    }
    let mut output = vec![0.0; input.len() * 2];
    bank.process(&input, &mut output);
    let side = |s: usize| {
        (0..excitation.len())
            .map(|f| output[(f * 2 + s) * stride])
            .collect()
    };
    (side(0), side(1))
}

fn impulse(frames: usize, at: usize, size: f32) -> Vec<f32> {
    let mut x = vec![0.0; frames];
    x[at] = size;
    x
}

/// A bank of one voice with `table` and `params`.
fn modal(rate: f32, table: ModalTable, params: &ModalParams) -> ModalBank {
    let mut bank = ModalBank::new(rate, 1);
    drop(bank.set_table(table));
    bank.reset_voice(0, params);
    bank
}

const SHAPES: [Shape; 6] = [
    Shape::Beam,
    Shape::Marimba,
    Shape::Cantilever,
    Shape::Membrane,
    Shape::CircularMembrane,
    Shape::Plate,
];

fn sustained(hz: f32) -> ModalParams {
    ModalParams {
        hz,
        decay: 30.0,
        material: 1.0,
        hit: 0.3,
        pos_l: 0.6,
        pos_r: 0.8,
        ..ModalParams::default()
    }
}

// Spec: physical-models R1, R2
#[test]
fn partial_ratios_are_the_published_ones() {
    let ratios = |shape, aspect| {
        let table = ModalTable::new(shape, Quality::Medium, aspect, 0.0);
        (0..10).map(|n| table.ratio(n)).collect::<Vec<_>>()
    };
    let near = |got: Vec<f64>, want: &[f64]| {
        for (n, want) in want.iter().enumerate() {
            let error = cents(got[n], *want).abs();
            assert!(
                error < 1.0,
                "partial {n}: {} against {want}: {error} cents",
                got[n]
            );
        }
    };
    near(
        ratios(Shape::Beam, 1.0),
        &[1.0, 2.756, 5.404, 8.933, 13.34, 18.64],
    );
    near(ratios(Shape::Marimba, 1.0), &[1.0, 4.0, 10.0]);
    near(
        ratios(Shape::Cantilever, 1.0),
        &[1.0, 6.267, 17.55, 34.39, 56.84],
    );
    near(
        ratios(Shape::CircularMembrane, 1.0),
        &[
            1.0, 1.594, 2.136, 2.296, 2.653, 2.918, 3.156, 3.501, 3.600, 3.648,
        ],
    );
    // A square membrane: √((m² + n²) / 2) over the (1, 1) mode's.
    let root = |sum: f64| (sum / 2.0).sqrt();
    near(
        ratios(Shape::Membrane, 1.0),
        &[1.0, root(5.0), root(5.0), root(8.0), root(10.0), root(10.0)],
    );
    // A square plate: (m² + n²) / 2.
    near(ratios(Shape::Plate, 1.0), &[1.0, 2.5, 2.5, 4.0, 5.0, 5.0]);
    // R2: Ratio 2 makes a = 2b: (m / 2)² + n².
    let wide = |sum: f64| (sum / 1.25).sqrt();
    near(
        ratios(Shape::Membrane, 2.0),
        &[1.0, wide(2.0), wide(3.25), wide(4.25)],
    );
    assert_eq!(
        ModalTable::new(Shape::Plate, Quality::Eco, 100.0, 0.0).ratio(1),
        ModalTable::new(Shape::Plate, Quality::Eco, 4.0, 0.0).ratio(1)
    );
}

// Spec: physical-models R1, R2
#[test]
fn a_struck_resonator_sounds_each_shapes_partials() {
    for (shape, aspect) in SHAPES
        .into_iter()
        .map(|s| (s, 1.0))
        .chain([(Shape::Membrane, 1.7), (Shape::Plate, 0.6)])
    {
        let table = ModalTable::new(shape, Quality::Medium, aspect, 0.0);
        let params = sustained(300.0);
        let wanted: Vec<usize> = (0..8)
            .filter(|&n| {
                (table.shape(n, 0.3) * table.shape(n, 0.6)).abs() > 0.05
                    && 300.0 * table.ratio(n) < 0.4 * f64::from(RATE)
                    && [n.wrapping_sub(1), n + 1]
                        .into_iter()
                        .filter(|&m| m < 32)
                        .all(|m| (table.ratio(m) / table.ratio(n) - 1.0).abs() > 0.02)
            })
            .collect();
        assert!(wanted.len() >= 2, "{shape:?}: {wanted:?}");
        let ratios: Vec<f64> = wanted.iter().map(|&n| table.ratio(n)).collect();
        let mut bank = modal(RATE, table, &params);
        let (left, _) = modal_render(&mut bank, &impulse(1 << 16, 0, 1000.0));
        for (n, ratio) in wanted.into_iter().zip(ratios) {
            let found = peak(&left, f64::from(RATE), 300.0 * ratio, 0.006);
            let error = cents(found, 300.0 * ratio).abs();
            assert!(
                error < 1.0,
                "{shape:?} partial {n}: {found} Hz, {error} cents"
            );
        }
    }
}

// Spec: physical-models R3
#[test]
fn quality_sets_the_partials_and_none_reaches_the_nyquist_cutoff() {
    for (quality, count) in [
        (Quality::Eco, 8),
        (Quality::Low, 16),
        (Quality::Medium, 32),
        (Quality::High, 64),
    ] {
        for shape in SHAPES {
            assert_eq!(ModalTable::new(shape, quality, 1.0, 0.0).partials(), count);
        }
    }
    // A beam at 5 kHz: its third partial, at 27 kHz, would fold to 21 kHz.
    let table = ModalTable::new(Shape::Beam, Quality::High, 1.0, 0.0);
    let mut bank = modal(RATE, table, &sustained(5000.0));
    let (left, _) = modal_render(&mut bank, &impulse(1 << 15, 0, 1000.0));
    let (loud, folded) = (dft(&left, 48_000.0, 5000.0), dft(&left, 48_000.0, 21_000.0));
    assert!(folded < loud * 1e-4, "{folded} against {loud}");
}

// Spec: physical-models R4
#[test]
fn inharmonicity_moves_the_upper_partials_and_not_the_fundamental() {
    let table = |i| ModalTable::new(Shape::Beam, Quality::Medium, 1.0, i);
    let flat = table(0.0);
    for (i, power) in [(1.0, 1.5), (-1.0, 0.5), (0.5, 1.25)] {
        let moved = table(i);
        assert_eq!(moved.ratio(0), 1.0);
        for n in 1..32 {
            assert!(cents(moved.ratio(n), flat.ratio(n).powf(power)).abs() < 1e-6);
        }
    }
    // And a struck resonator plays it.
    let mut bank = modal(RATE, table(1.0), &sustained(300.0));
    let (left, _) = modal_render(&mut bank, &impulse(1 << 16, 0, 1000.0));
    let want = 300.0 * flat.ratio(2).powf(1.5);
    assert!(cents(peak(&left, 48_000.0, want, 0.006), want).abs() < 1.0);
}

/// How long partial `at` of a resonator rings, from two windows of its
/// output: the time it takes to fall 60 dB.
fn ring_time(x: &[f32], rate: f64, hz: f64, guess: f64) -> f64 {
    let window = ((0.15 * guess).min(0.1) * rate) as usize;
    let gap = (0.3 * guess * rate) as usize;
    let start = (0.002 * rate) as usize;
    let first = dft(&x[start..start + window], rate, hz);
    let second = dft(&x[start + gap..start + gap + window], rate, hz);
    60.0 * (gap as f64 / rate) / (20.0 * (first / second).log10())
}

// Spec: physical-models R5
#[test]
fn each_partial_rings_for_the_time_decay_and_material_give() {
    let table = ModalTable::new(Shape::Beam, Quality::Medium, 1.0, 0.0);
    let ratios: Vec<f64> = (0..3).map(|n| table.ratio(n)).collect();
    for material in [0.0, 0.5, 1.0] {
        let params = ModalParams {
            hz: 400.0,
            decay: 1.0,
            material,
            hit: 0.3,
            pos_l: 0.6,
            ..ModalParams::default()
        };
        let mut bank = modal(RATE, table.clone(), &params);
        let (left, _) = modal_render(&mut bank, &impulse(1 << 16, 0, 1000.0));
        for (n, &ratio) in ratios.iter().enumerate() {
            let want = ratio.powf(-2.0 * (1.0 - f64::from(material)));
            let got = ring_time(&left, 48_000.0, 400.0 * ratio, want);
            assert!(
                (got / want - 1.0).abs() < 0.05,
                "m {material}, partial {n}: {got} against {want}"
            );
        }
    }
}

// Spec: physical-models R6
#[test]
fn brightness_lifts_the_upper_partials_by_their_ratio_to_a_power() {
    let table = ModalTable::new(Shape::Beam, Quality::Medium, 1.0, 0.0);
    let top = table.ratio(2) / table.ratio(0);
    let level = |brightness| {
        let params = ModalParams {
            brightness,
            ..sustained(400.0)
        };
        let mut bank = modal(RATE, table.clone(), &params);
        let (left, _) = modal_render(&mut bank, &impulse(1 << 14, 0, 1000.0));
        let part = |n| dft(&left[..9600], 48_000.0, 400.0 * table.ratio(n));
        part(2) / part(0)
    };
    for b in [-1.0, 0.5, 1.0] {
        let got = level(b) / level(0.0);
        let want = top.powf(f64::from(b) / 2.0);
        assert!(
            (got / want - 1.0).abs() < 0.05,
            "b {b}: {got} against {want}"
        );
    }
}

// Spec: physical-models R7
#[test]
fn a_hit_at_a_nodes_leaves_its_partial_below_60_db() {
    for shape in SHAPES {
        let table = ModalTable::new(shape, Quality::Medium, 1.0, 0.0);
        let silent: Vec<usize> = (0..6)
            .filter(|&n| table.shape(n, 0.0).abs() < 1e-9)
            .collect();
        if silent.is_empty() {
            assert_eq!(shape, Shape::Cantilever);
            continue;
        }
        let ratios: Vec<f64> = (0..6).map(|n| table.ratio(n)).collect();
        // Struck at the centre and heard off it: the silent partials are the centre's nodes.
        let params = ModalParams {
            hit: 0.0,
            pos_l: 0.4,
            pos_r: 0.4,
            ..sustained(300.0)
        };
        let mut bank = modal(RATE, table, &params);
        let (left, _) = modal_render(&mut bank, &impulse(1 << 16, 0, 1000.0));
        let loudest = (0..6)
            .filter(|n| !silent.contains(n))
            .map(|n| dft(&left, 48_000.0, 300.0 * ratios[n]))
            .fold(0.0, f64::max);
        for n in silent {
            let level = dft(&left, 48_000.0, 300.0 * ratios[n]);
            assert!(
                level < loudest * 1e-3,
                "{shape:?} partial {n}: {level} against {loudest}"
            );
        }
    }
}

// Spec: physical-models R7
#[test]
fn two_listening_positions_give_two_channels() {
    let table = ModalTable::new(Shape::Beam, Quality::Medium, 1.0, 0.0);
    let params = ModalParams {
        hit: 0.4,
        pos_l: 0.0,
        pos_r: 0.6,
        ..sustained(300.0)
    };
    let mut bank = modal(RATE, table.clone(), &params);
    let (left, right) = modal_render(&mut bank, &impulse(1 << 15, 0, 1000.0));
    // Beam partial 1 has a node at the centre, where the left channel listens.
    let at = 300.0 * table.ratio(1);
    assert!(dft(&left, 48_000.0, at) < dft(&right, 48_000.0, at) * 1e-3);
}

// Spec: physical-models R1, R5 (the effects half's acceptance: every model tunes its fundamental)
#[test]
fn a_modal_fundamental_is_within_a_cent_from_40_hz_to_4_khz() {
    for rate in [44_100.0_f32, 48_000.0, 96_000.0, 192_000.0] {
        for hz in [40.0_f32, 110.0, 440.0, 1000.0, 4000.0] {
            let table = ModalTable::new(Shape::Beam, Quality::Medium, 1.0, 0.0);
            let params = ModalParams {
                decay: 60.0,
                ..sustained(hz)
            };
            let mut bank = modal(rate, table, &params);
            let frames = (rate as usize * 3).max((70.0 * rate / hz) as usize);
            let (left, _) = modal_render(&mut bank, &impulse(frames, 0, 1000.0));
            let found = fundamental(
                &left,
                f64::from(rate),
                f64::from(hz) * 0.9,
                f64::from(hz) * 1.1,
            );
            assert!(
                cents(found, f64::from(hz)).abs() < 1.0,
                "{rate} Hz rate, {hz} Hz: {found}"
            );
        }
    }
}

// Spec: physical-models R14
#[test]
fn a_decayed_resonator_holds_no_state() {
    let table = ModalTable::new(Shape::Plate, Quality::High, 1.0, 0.0);
    let params = ModalParams {
        decay: 0.01,
        material: 0.0,
        ..sustained(300.0)
    };
    let mut bank = modal(RATE, table, &params);
    let (left, _) = modal_render(&mut bank, &impulse(48_000, 0, 1.0));
    assert!(!bank.is_silent() || left.iter().all(|v| *v == 0.0 || v.is_normal()));
    modal_render(&mut bank, &vec![0.0; 48_000]);
    assert!(bank.is_silent());
    let (tail, _) = modal_render(&mut bank, &vec![0.0; 4096]);
    assert!(tail.iter().all(|v| *v == 0.0));
}

/// A long decay's state stays out of the denormals while it rings out.
#[test]
fn a_long_decay_never_reaches_the_denormals() {
    let table = ModalTable::new(Shape::Membrane, Quality::High, 1.0, 0.0);
    let params = ModalParams {
        decay: 0.3,
        ..sustained(300.0)
    };
    let mut bank = modal(RATE, table, &params);
    let (left, right) = modal_render(&mut bank, &impulse(48_000 * 6, 0, 1.0));
    assert!(
        left.iter()
            .chain(&right)
            .all(|v| *v == 0.0 || v.is_normal())
    );
}

/// The excitation for a property test: a few short bursts of noise.
fn bursts(random: &mut Random, frames: usize) -> Vec<f32> {
    let mut x = vec![0.0; frames];
    for _ in 0..1 + (random.next_u64() % 3) {
        let at = (random.next_u64() as usize) % (frames / 4);
        let len = 1 + (random.next_u64() as usize) % 48;
        for v in &mut x[at..at + len] {
            *v = random.next_bipolar() as f32 / 2.0;
        }
    }
    x
}

fn rms(x: &[f32]) -> f64 {
    (x.iter().map(|&v| f64::from(v).powi(2)).sum::<f64>() / x.len() as f64).sqrt()
}

/// Output must be finite, below +24 dBFS, and falling once the excitation has stopped.
fn assert_bounded_and_decaying(label: &str, rate: f32, out: &[f32]) {
    let seconds = |s: f32| (s * rate) as usize;
    assert!(out.iter().all(|v| v.is_finite()), "{label}: not finite");
    let peak = out.iter().fold(0.0_f32, |m, v| m.max(v.abs()));
    assert!(peak < 15.84, "{label}: peak {peak}");
    let early = rms(&out[seconds(3.0)..seconds(4.0)]);
    let late = rms(&out[seconds(9.0)..seconds(10.0)]);
    assert!(
        late <= early.max(1e-9) * 1.001,
        "{label}: {late} after {early}"
    );
}

fn random_modal(random: &mut Random, rate: f32) -> (ModalTable, ModalParams) {
    let unit = |r: &mut Random| ((r.next_bipolar() + 1.0) / 2.0) as f32;
    let shape = SHAPES[(random.next_u64() % 6) as usize];
    let quality = [Quality::Eco, Quality::Low, Quality::Medium, Quality::High]
        [(random.next_u64() % 4) as usize];
    let table = ModalTable::new(
        shape,
        quality,
        0.25 + 3.75 * unit(random),
        random.next_bipolar() as f32,
    );
    let params = ModalParams {
        hz: 20.0 * (rate / 4.0 / 20.0).powf(unit(random)),
        decay: 0.001 * 60_000.0_f32.powf(unit(random)),
        material: unit(random),
        brightness: random.next_bipolar() as f32,
        hit: unit(random),
        pos_l: unit(random),
        pos_r: unit(random),
    };
    (table, params)
}

// Spec: physical-models R13
#[test]
fn modal_resonators_stay_bounded_and_decay_alone_and_in_series() {
    let mut random = Random::seeded(510);
    for rate in [44_100.0_f32, 48_000.0, 96_000.0, 192_000.0] {
        for round in 0..2 {
            let frames = rate as usize * 10;
            let stride = 16;
            let (table_a, params_a) = random_modal(&mut random, rate);
            let (table_b, params_b) = random_modal(&mut random, rate);
            let excitation = bursts(&mut random, frames);
            let mut a = modal(rate, table_a, &params_a);
            let mut b = modal(rate, table_b, &params_b);
            let (single, _) = modal_render(&mut a, &excitation);
            assert_bounded_and_decaying(
                &format!("{rate} single {round} {params_a:?}"),
                rate,
                &single,
            );
            a.reset_voice(0, &params_a);
            let mut out = Vec::with_capacity(frames);
            // In series, block by block: the first's output excites the second.
            for block in excitation.chunks(512) {
                let (first, _) = modal_render(&mut a, block);
                let (second, _) = modal_render(&mut b, &first);
                out.extend(second);
            }
            assert_eq!(stride, a.stride());
            assert_bounded_and_decaying(
                &format!("{rate} series {round} {params_a:?} {params_b:?}"),
                rate,
                &out,
            );
        }
    }
}

// Spec: physical-models R16
#[test]
fn a_modal_render_is_the_same_at_every_block_size_and_run() {
    let table = ModalTable::new(Shape::CircularMembrane, Quality::Medium, 1.0, 0.2);
    let params = ModalParams {
        decay: 2.0,
        ..sustained(250.0)
    };
    let moved = ModalParams {
        decay: 0.5,
        material: 0.1,
        brightness: 0.6,
        hit: 0.7,
        pos_l: 0.2,
        pos_r: 0.9,
        ..params
    };
    let mut random = Random::seeded(7);
    let excitation: Vec<f32> = (0..8192)
        .map(|i| {
            if i % 3000 < 200 {
                random.next_bipolar() as f32
            } else {
                0.0
            }
        })
        .collect();
    let whole = |block: usize| {
        let mut bank = modal(RATE, table.clone(), &params);
        let mut left = Vec::new();
        for (i, chunk) in excitation.chunks(block).enumerate() {
            if i * block == 4096 {
                bank.set_params(0, &moved);
            }
            left.extend(modal_render(&mut bank, chunk).0);
        }
        left
    };
    let reference = whole(4096);
    assert!(reference.iter().any(|v| v.abs() > 1e-3));
    for block in [1, 64, 512, 4096] {
        assert_eq!(whole(block), reference, "block {block}");
    }
    assert_eq!(whole(512), whole(512));
}

/// The largest second difference in `x`, which a click makes large.
fn roughness(x: &[f32]) -> f32 {
    x.windows(3)
        .map(|w| (w[2] - 2.0 * w[1] + w[0]).abs())
        .fold(0.0, f32::max)
}

// Spec: physical-models R17
#[test]
fn modal_controls_move_without_a_click() {
    let table = ModalTable::new(Shape::Beam, Quality::Medium, 1.0, 0.0);
    let base = ModalParams {
        decay: 6.0,
        material: 0.8,
        ..sustained(220.0)
    };
    let moves = [
        ModalParams { decay: 0.4, ..base },
        ModalParams {
            material: 0.0,
            ..base
        },
        ModalParams {
            brightness: -1.0,
            ..base
        },
        ModalParams {
            hit: 0.35,
            pos_l: 0.65,
            pos_r: 0.85,
            ..base
        },
    ];
    for moved in moves {
        let mut bank = modal(RATE, table.clone(), &base);
        let mut out = modal_render(&mut bank, &impulse(12_000, 0, 1000.0)).0;
        bank.set_params(0, &moved);
        out.extend(modal_render(&mut bank, &vec![0.0; 12_000]).0);
        let (before, after) = (
            roughness(&out[6000..11_990]),
            roughness(&out[12_000..18_000]),
        );
        assert!(after <= before * 1.1, "{moved:?}: {after} after {before}");
    }
}

// Spec: physical-models R12
#[test]
fn voices_are_independent_and_a_table_swaps_without_allocating() {
    let table = ModalTable::new(Shape::Plate, Quality::Medium, 1.0, 0.0);
    let params = |hz| ModalParams {
        hz,
        ..sustained(hz)
    };
    let mut many = ModalBank::new(RATE, 5);
    drop(many.set_table(table.clone()));
    for v in 0..5 {
        many.reset_voice(v, &params(200.0 + 70.0 * v as f32));
    }
    let mut single = modal(RATE, table.clone(), &params(200.0 + 70.0 * 3.0));
    let stride = many.stride();
    let mut input = vec![0.0; 2048 * stride];
    input[3] = 1.0;
    let mut output = vec![0.0; input.len() * 2];
    lspk_rt::rt_test!({ many.process(&input, &mut output) });
    let reference = modal_render(&mut single, &impulse(2048, 0, 1.0)).0;
    let third: Vec<f32> = (0..2048).map(|f| output[f * 2 * stride + 3]).collect();
    assert_eq!(third, reference);
    // The other voices heard nothing.
    assert!((0..2048).all(|f| output[f * 2 * stride + 2] == 0.0));
    let next = ModalTable::new(Shape::Membrane, Quality::High, 1.0, 0.0);
    let old = lspk_rt::rt_test!({ many.set_table(next) });
    drop(old);
    lspk_rt::rt_test!({ many.process(&input, &mut output) });
}

// Spec: physical-models R12, R15
#[test]
fn sixteen_resonators_and_sixteen_waveguides_run_under_the_trap() {
    let mut modal_bank = ModalBank::new(RATE, 16);
    let mut guides = WaveguideBank::new(RATE, 16, Kind::String, 30.0);
    drop(modal_bank.set_table(ModalTable::new(Shape::Beam, Quality::Medium, 1.0, 0.0)));
    for v in 0..16 {
        modal_bank.reset_voice(
            v,
            &ModalParams {
                hz: 100.0 * (1.0 + v as f32 / 4.0),
                ..sustained(100.0)
            },
        );
        guides.reset_voice(
            v,
            &WaveguideParams {
                hz: 100.0 * (1.0 + v as f32 / 4.0),
                ..WaveguideParams::default()
            },
        );
    }
    let mut input = vec![0.0; 512 * 16];
    input.iter_mut().step_by(16).for_each(|v| *v = 0.5);
    let mut output = vec![0.0; input.len() * 2];
    for block in 0..8 {
        if block == 3 {
            for v in 0..16 {
                modal_bank.set_params(
                    v,
                    &ModalParams {
                        decay: 0.2,
                        ..sustained(100.0 + v as f32)
                    },
                );
                guides.set_params(
                    v,
                    &WaveguideParams {
                        decay: 0.2,
                        opening: 0.3,
                        ..WaveguideParams::default()
                    },
                );
            }
        }
        lspk_rt::rt_test!({
            modal_bank.process(&input, &mut output);
            guides.process(&input, &mut output);
        });
    }
}

// Spec: physical-models R12, R15 (every path gives the same sound)
#[test]
fn every_lane_path_gives_the_same_sound() {
    let table = ModalTable::new(Shape::Beam, Quality::Medium, 1.0, 0.0);
    let render = |level: Option<Level>| {
        let mut bank = ModalBank::new(RATE, 6);
        drop(bank.set_table(table.clone()));
        let mut guides = WaveguideBank::new(RATE, 6, Kind::Pipe, 30.0);
        for v in 0..6 {
            bank.reset_voice(
                v,
                &ModalParams {
                    hz: 150.0 + 40.0 * v as f32,
                    ..sustained(150.0)
                },
            );
            guides.reset_voice(
                v,
                &WaveguideParams {
                    hz: 150.0 + 40.0 * v as f32,
                    ..WaveguideParams::default()
                },
            );
        }
        let stride = bank.stride();
        let mut input = vec![0.0; 4096 * stride];
        input[..stride].fill(1.0);
        let mut a = vec![0.0; input.len() * 2];
        let mut b = vec![0.0; input.len() * 2];
        match level {
            Some(level) => {
                if !bank.process_at(level, &input, &mut a)
                    || !guides.process_at(level, &input, &mut b)
                {
                    return None;
                }
            }
            None => {
                bank.process(&input, &mut a);
                guides.process(&input, &mut b);
            }
        }
        Some((a, b))
    };
    let (ma, mb) = render(None).unwrap();
    let scale = ma.iter().fold(0.0_f32, |m, v| m.max(v.abs()));
    for level in Level::ALL {
        let Some((a, b)) = render(Some(level)) else {
            continue;
        };
        let worst = a
            .iter()
            .zip(&ma)
            .chain(b.iter().zip(&mb))
            .map(|(x, y)| (x - y).abs())
            .fold(0.0, f32::max);
        assert!(worst <= scale * 1e-3, "{level}: {worst} of {scale}");
    }
}

// ---- waveguides ----

fn guide(rate: f32, kind: Kind, params: &WaveguideParams) -> WaveguideBank {
    let mut bank = WaveguideBank::new(rate, 1, kind, 20.0);
    bank.reset_voice(0, params);
    bank
}

fn guide_render(bank: &mut WaveguideBank, excitation: &[f32]) -> (Vec<f32>, Vec<f32>) {
    let stride = bank.stride();
    let mut input = vec![0.0; excitation.len() * stride];
    for (frame, &value) in excitation.iter().enumerate() {
        input[frame * stride] = value;
    }
    let mut output = vec![0.0; input.len() * 2];
    bank.process(&input, &mut output);
    let side = |s: usize| {
        (0..excitation.len())
            .map(|f| output[(f * 2 + s) * stride])
            .collect()
    };
    (side(0), side(1))
}

fn ringing(hz: f32) -> WaveguideParams {
    WaveguideParams {
        hz,
        decay: 30.0,
        material: 1.0,
        excite: 0.31,
        pickup_l: 0.17,
        pickup_r: 0.43,
        ..WaveguideParams::default()
    }
}

/// The MIDI keys up to the highest below a quarter of `rate`, every third and the last.
fn keys(rate: f32) -> Vec<u32> {
    let top = (69.0 + 12.0 * (rate / 4.0 / 440.0).log2()).floor() as u32;
    (28..top).step_by(3).chain([top]).collect()
}

fn key_hz(key: u32) -> f32 {
    440.0 * 2.0_f32.powf((key as f32 - 69.0) / 12.0)
}

// Spec: physical-models R8
#[test]
fn a_waveguide_plays_its_note_within_a_cent_at_every_rate() {
    for kind in [Kind::String, Kind::Tube, Kind::Pipe] {
        for rate in [44_100.0_f32, 48_000.0, 96_000.0, 192_000.0] {
            for key in keys(rate) {
                let hz = key_hz(key);
                // A pipe closed at one end plays an octave below its note (R9).
                let openings = if kind == Kind::Pipe {
                    vec![(1.0, 1.0), (0.0, 0.5)]
                } else {
                    vec![(1.0, 1.0)]
                };
                for (opening, factor) in openings {
                    let mut bank = guide(
                        rate,
                        kind,
                        &WaveguideParams {
                            opening,
                            ..ringing(hz)
                        },
                    );
                    let want = f64::from(hz) * factor;
                    let frames =
                        ((rate * 0.7) as usize).max((70.0 * f64::from(rate) / want) as usize);
                    let (left, _) = guide_render(&mut bank, &impulse(frames, 0, 1.0));
                    let found = fundamental(&left, f64::from(rate), want * 0.9, want * 1.1);
                    let error = cents(found, want);
                    assert!(
                        error.abs() < 1.0,
                        "{kind:?} {rate} Hz rate, key {key} ({hz} Hz), opening {opening}: {found}, {error} cents"
                    );
                }
            }
        }
    }
}

// Spec: physical-models R8, and the effects half's acceptance: decay within 10 %
#[test]
fn a_waveguides_fundamental_falls_in_the_time_decay_gives() {
    for (kind, hz, decay) in [
        (Kind::String, 220.0, 1.0),
        (Kind::String, 440.0, 3.0),
        (Kind::Tube, 110.0, 2.0),
        (Kind::Pipe, 330.0, 1.5),
    ] {
        let params = WaveguideParams {
            decay,
            material: 0.3,
            opening: 1.0,
            ..ringing(hz)
        };
        let mut bank = guide(RATE, kind, &params);
        let (left, _) = guide_render(&mut bank, &impulse(48_000 * 2, 0, 1.0));
        let got = ring_time(&left, 48_000.0, f64::from(hz), f64::from(decay));
        assert!(
            (got / f64::from(decay) - 1.0).abs() < 0.1,
            "{kind:?} {hz}: {got} against {decay}"
        );
    }
}

// Spec: physical-models R8
#[test]
fn a_strings_partials_are_harmonic_and_inharm_stretches_them_as_a_stiff_string() {
    for (hz, inharm) in [
        (220.0_f32, 0.0_f32),
        (220.0, 0.5),
        (110.0, 1.0),
        (440.0, 1.0),
    ] {
        let stiffness = f64::from(inharm) * 0.001;
        let params = WaveguideParams {
            inharm,
            ..ringing(hz)
        };
        let mut bank = guide(RATE, Kind::String, &params);
        let (left, _) = guide_render(&mut bank, &impulse(1 << 16, 0, 1.0));
        for n in 2..=6 {
            let want = f64::from(hz) * f64::from(n) * (1.0 + stiffness * f64::from(n * n)).sqrt();
            let found = peak(&left, 48_000.0, want, 0.012);
            // Harmonic to half a cent; stiff to the law within 3 cents up to the sixth.
            let allowed = if inharm == 0.0 { 0.5 } else { 3.0 };
            assert!(
                cents(found, want).abs() < allowed,
                "{hz} Hz, inharm {inharm}, partial {n}: {found} against {want}"
            );
        }
    }
}

// Spec: physical-models R9
#[test]
fn a_pipe_plays_odd_harmonics_closed_and_every_harmonic_open() {
    let hz = 300.0;
    let level = |opening: f32, f: f64| {
        let params = WaveguideParams {
            opening,
            excite: 0.37,
            pickup_l: 0.13,
            ..ringing(hz)
        };
        let mut bank = guide(RATE, Kind::Pipe, &params);
        let (left, _) = guide_render(&mut bank, &impulse(1 << 15, 0, 1.0));
        dft(&left, 48_000.0, f)
    };
    let f = f64::from(hz);
    // Closed: an octave below the tube, the odd multiples of hz / 2.
    assert!(level(0.0, f / 2.0) > 100.0 * level(0.0, f));
    assert!(level(0.0, f * 1.5) > 100.0 * level(0.0, 2.0 * f));
    // Open: every multiple of hz.
    assert!(level(1.0, f) > 100.0 * level(1.0, f / 2.0));
    assert!(level(1.0, 2.0 * f) > 100.0 * level(1.0, f * 1.5));
    // Between, the even harmonics rise as the opening grows.
    let ratio = |o: f32| level(o, f) / level(o, f / 2.0);
    assert!(ratio(0.0) < ratio(0.25) && ratio(0.25) < ratio(0.75) && ratio(0.75) < ratio(1.0));
    // A Tube plays every harmonic, as a String does.
    let tube = guide_render(
        &mut guide(RATE, Kind::Tube, &ringing(hz)),
        &impulse(1 << 15, 0, 1.0),
    )
    .0;
    assert!(dft(&tube, 48_000.0, f) > 100.0 * dft(&tube, 48_000.0, f / 2.0));
}

// Spec: physical-models R10
#[test]
fn radius_lengthens_decay_and_lowers_an_open_pipe_by_up_to_a_semitone() {
    let params = |radius| WaveguideParams {
        opening: 1.0,
        radius,
        decay: 1.0,
        material: 0.5,
        ..ringing(300.0)
    };
    let note = |kind, radius| {
        let mut bank = guide(RATE, kind, &params(radius));
        let (left, _) = guide_render(&mut bank, &impulse(1 << 16, 0, 1.0));
        fundamental(&left, 48_000.0, 200.0, 400.0)
    };
    let shift = cents(note(Kind::Pipe, 1.0), note(Kind::Pipe, 0.0));
    assert!((-100.0..-95.0).contains(&shift), "{shift} cents");
    assert!(cents(note(Kind::Tube, 1.0), note(Kind::Tube, 0.0)).abs() < 1.0);
    let time = |radius| {
        let mut bank = guide(RATE, Kind::Tube, &params(radius));
        let (left, _) = guide_render(&mut bank, &impulse(48_000 * 4, 0, 1.0));
        ring_time(&left, 48_000.0, 300.0, 1.0 + f64::from(radius))
    };
    assert!(
        time(1.0) > 1.8 * time(0.0),
        "{} against {}",
        time(1.0),
        time(0.0)
    );
}

// Spec: physical-models R7, R11
#[test]
fn a_string_struck_at_its_middle_leaves_its_even_harmonics_below_60_db() {
    for (kind, hz) in [
        (Kind::String, 220.0),
        (Kind::String, 523.0),
        (Kind::Tube, 330.0),
    ] {
        let params = WaveguideParams {
            excite: 0.5,
            pickup_l: 0.2,
            pickup_r: 0.3,
            ..ringing(hz)
        };
        let mut bank = guide(RATE, kind, &params);
        let (left, _) = guide_render(&mut bank, &impulse(1 << 15, 0, 1.0));
        let f = f64::from(hz);
        let odd = [1.0, 3.0, 5.0]
            .map(|n| dft(&left, 48_000.0, f * n))
            .into_iter()
            .fold(0.0, f64::max);
        for n in [2.0, 4.0] {
            let level = dft(&left, 48_000.0, f * n);
            assert!(
                level < odd * 1e-3,
                "{kind:?} {hz} Hz harmonic {n}: {level} against {odd}"
            );
        }
    }
}

// Spec: physical-models R11
#[test]
fn a_pickup_at_a_nodes_hears_no_harmonic_there() {
    let params = WaveguideParams {
        excite: 0.31,
        pickup_l: 0.5,
        pickup_r: 0.25,
        ..ringing(220.0)
    };
    let mut bank = guide(RATE, Kind::String, &params);
    let (left, right) = guide_render(&mut bank, &impulse(1 << 15, 0, 1.0));
    let odd = dft(&left, 48_000.0, 220.0);
    assert!(dft(&left, 48_000.0, 440.0) < odd * 1e-3);
    assert!(dft(&right, 48_000.0, 440.0) > odd * 0.1);
}

// Spec: physical-models R11
#[test]
fn an_interaction_reads_the_velocity_and_drives_the_guide() {
    struct Damper {
        calls: usize,
        loudest: f32,
    }
    impl Interaction for Damper {
        fn force(&mut self, voice: usize, velocity: f32) -> f32 {
            assert_eq!(voice, 0);
            self.calls += 1;
            self.loudest = self.loudest.max(velocity.abs());
            -0.3 * velocity
        }
    }
    let run = |damped: bool| {
        let mut bank = guide(RATE, Kind::String, &ringing(220.0));
        let stride = bank.stride();
        let mut input = vec![0.0; 8192 * stride];
        input[0] = 1.0;
        let mut out = vec![0.0; input.len() * 2];
        let mut damper = Damper {
            calls: 0,
            loudest: 0.0,
        };
        if damped {
            bank.process_with(&input, &mut out, &mut damper);
        } else {
            bank.process(&input, &mut out);
        }
        let left: Vec<f32> = (0..8192).map(|f| out[f * 2 * stride]).collect();
        (rms(&left), damper)
    };
    let (free, _) = run(false);
    let (held, damper) = run(true);
    assert_eq!(damper.calls, 8192);
    assert!(damper.loudest > 0.0);
    assert!(held < free * 0.8, "{held} against {free}");
}

// Spec: physical-models R13
#[test]
fn waveguides_stay_bounded_and_decay_alone_and_in_series() {
    let mut random = Random::seeded(5101);
    for rate in [44_100.0_f32, 48_000.0, 96_000.0, 192_000.0] {
        for round in 0..2 {
            let settings = |random: &mut Random| {
                let unit = |r: &mut Random| ((r.next_bipolar() + 1.0) / 2.0) as f32;
                let kind = [Kind::String, Kind::Tube, Kind::Pipe][(random.next_u64() % 3) as usize];
                let params = WaveguideParams {
                    hz: 20.0 * (rate / 1.5 / 20.0).powf(unit(random)),
                    decay: 0.001 * 60_000.0_f32.powf(unit(random)),
                    material: unit(random),
                    inharm: unit(random),
                    opening: unit(random),
                    radius: unit(random),
                    excite: unit(random) / 2.0,
                    pickup_l: unit(random) / 2.0,
                    pickup_r: unit(random) / 2.0,
                };
                (kind, params)
            };
            let frames = rate as usize * 10;
            let ((kind_a, params_a), (kind_b, params_b)) =
                (settings(&mut random), settings(&mut random));
            let excitation = bursts(&mut random, frames);
            let mut a = guide(rate, kind_a, &params_a);
            let mut b = guide(rate, kind_b, &params_b);
            let (single, _) = guide_render(&mut a, &excitation);
            assert_bounded_and_decaying(
                &format!("{rate} single {round} {kind_a:?} {params_a:?}"),
                rate,
                &single,
            );
            a.reset_voice(0, &params_a);
            let mut out = Vec::with_capacity(frames);
            for block in excitation.chunks(512) {
                let (first, _) = guide_render(&mut a, block);
                out.extend(guide_render(&mut b, &first).0);
            }
            assert_bounded_and_decaying(
                &format!("{rate} series {round} {kind_a:?} {params_a:?} {kind_b:?} {params_b:?}"),
                rate,
                &out,
            );
        }
    }
}

// Spec: physical-models R14
#[test]
fn a_decayed_waveguide_holds_no_denormals() {
    for kind in [Kind::String, Kind::Pipe] {
        let params = WaveguideParams {
            decay: 0.3,
            material: 0.2,
            opening: 0.9,
            ..ringing(110.0)
        };
        let mut bank = guide(RATE, kind, &params);
        let (left, right) = guide_render(&mut bank, &impulse(48_000 * 8, 0, 1.0));
        // The first frames hold a few as the filters' impulse responses die away; after that, none.
        let (left, right) = (&left[48_000..], &right[48_000..]);
        let bad = left
            .iter()
            .chain(right)
            .position(|v| !(*v == 0.0 || v.is_normal()));
        assert!(
            bad.is_none(),
            "{kind:?}: {:?} at {bad:?}",
            bad.map(|i| left.get(i))
        );
        assert_eq!(left[left.len() - 1], 0.0);
    }
}

// Spec: physical-models R16
#[test]
fn a_waveguide_render_is_the_same_at_every_block_size_and_run() {
    let params = WaveguideParams {
        decay: 2.0,
        inharm: 0.4,
        opening: 0.8,
        radius: 0.3,
        ..ringing(180.0)
    };
    let moved = WaveguideParams {
        hz: 205.0,
        decay: 0.7,
        material: 0.1,
        opening: 0.2,
        excite: 0.2,
        pickup_l: 0.4,
        ..params
    };
    let mut random = Random::seeded(9);
    let excitation: Vec<f32> = (0..8192)
        .map(|i| {
            if i % 3000 < 100 {
                random.next_bipolar() as f32
            } else {
                0.0
            }
        })
        .collect();
    let render = |block: usize| {
        let mut bank = guide(RATE, Kind::Pipe, &params);
        let mut out = Vec::new();
        for (i, chunk) in excitation.chunks(block).enumerate() {
            if i * block == 4096 {
                bank.set_params(0, &moved);
            }
            let (left, right) = guide_render(&mut bank, chunk);
            out.extend(left);
            out.extend(right);
        }
        out
    };
    // Compare per frame: blocks interleave left then right.
    let reference = render(4096);
    assert!(reference.iter().any(|v| v.abs() > 1e-4));
    let per_block = |block: usize| {
        let mut bank = guide(RATE, Kind::Pipe, &params);
        let mut left = Vec::new();
        for (i, chunk) in excitation.chunks(block).enumerate() {
            if i * block == 4096 {
                bank.set_params(0, &moved);
            }
            left.extend(guide_render(&mut bank, chunk).0);
        }
        left
    };
    let first = per_block(4096);
    for block in [1, 64, 512, 4096] {
        assert_eq!(per_block(block), first, "block {block}");
    }
    assert_eq!(render(512), render(512));
}

// Spec: physical-models R17
#[test]
fn waveguide_controls_and_pitch_move_without_a_click() {
    let base = WaveguideParams {
        decay: 4.0,
        material: 0.7,
        opening: 1.0,
        ..ringing(220.0)
    };
    let moves = [
        WaveguideParams { decay: 0.5, ..base },
        WaveguideParams {
            material: 0.1,
            ..base
        },
        WaveguideParams {
            opening: 0.8,
            ..base
        },
        WaveguideParams {
            pickup_l: 0.3,
            pickup_r: 0.1,
            ..base
        },
        WaveguideParams { hz: 221.0, ..base },
    ];
    for moved in moves {
        let mut bank = guide(RATE, Kind::Pipe, &base);
        let mut out = guide_render(&mut bank, &impulse(12_000, 0, 1.0)).0;
        bank.set_params(0, &moved);
        out.extend(guide_render(&mut bank, &vec![0.0; 12_000]).0);
        let (before, after) = (
            roughness(&out[6000..11_990]),
            roughness(&out[12_000..18_000]),
        );
        assert!(after <= before * 1.5, "{moved:?}: {after} after {before}");
    }
}

// Spec: physical-models R11, R17
#[test]
fn a_tap_that_has_moved_stays_where_it_was_put() {
    let params = WaveguideParams {
        excite: 0.31,
        pickup_l: 0.17,
        pickup_r: 0.17,
        ..ringing(220.0)
    };
    let mut bank = guide(RATE, Kind::String, &params);
    guide_render(&mut bank, &impulse(1000, 0, 1.0));
    bank.set_params(
        0,
        &WaveguideParams {
            pickup_l: 0.5,
            ..params
        },
    );
    guide_render(&mut bank, &vec![0.0; 24_000]);
    let (left, _) = guide_render(&mut bank, &vec![0.0; 1 << 15]);
    // Heard at the middle, the even harmonics are gone: after a second the tap hasn't drifted off.
    assert!(dft(&left, 48_000.0, 440.0) < dft(&left, 48_000.0, 220.0) * 1e-3);
}

#[test]
fn a_tap_moved_to_an_end_never_reads_ahead_of_the_ring() {
    // The steps of a moving tap can add up to a hair under the one sample it
    // stops at; the next read must still be at least a sample back.
    let params = ringing(220.0);
    let mut bank = guide(RATE, Kind::String, &params);
    guide_render(&mut bank, &impulse(1000, 0, 1.0));
    for position in [0.31, 0.0, 0.47, 0.0, 0.123, 0.0] {
        bank.set_params(
            0,
            &WaveguideParams {
                excite: position,
                pickup_l: position,
                pickup_r: position,
                ..params
            },
        );
        let (left, right) = guide_render(&mut bank, &impulse(4_800, 0, 1.0));
        assert!(left.iter().chain(&right).all(|v| v.is_finite()));
    }
}

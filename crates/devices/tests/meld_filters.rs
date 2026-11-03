//! Meld, part 3 (WP-8.19; the meld spec, R40 to R58): the filter slot's
//! seventeen types against their prototypes, their levels at high drive and
//! resonance, the resonators' modes, filter scale awareness, type changes,
//! and block-size independence.
//!
//! Every block runs as a real-time section under the allocation trap.

use std::f64::consts::{PI, TAU};

use crate::meld::{
    BLOCKS, HZ, Params, Setting, a, assert_near, assert_same, c_major, off, on, play, play_with,
    rms,
};
use bl_devices::instrument_filter::{Per, PerFrame};
use bl_devices::meld::{
    FILTER_FREQ, FILTER_MACRO_1, FILTER_MACRO_2, FILTER_ON, FILTER_SCALE, FILTER_TYPE, MACRO_1,
    USE_SCALE,
};
use bl_devices::meld_filter::{Controls, MeldFilter, TYPES};
use bl_dsp::physical::{ModalTable, Quality, Shape};
use bl_model::Scale;
use realfft::RealFftPlanner;

type C = (f64, f64);

const SG_Q: f64 = std::f64::consts::FRAC_1_SQRT_2;

const OSR: usize = 4;
const LP_CRUNCH: usize = 5;
const LP_SWITCHED_RES: usize = 6;
const FILTHER: usize = 7;
const EQ_PEAK: usize = 8;
const EQ_NOTCH: usize = 9;
const PHASER: usize = 10;
const REDUX: usize = 11;
const VOWEL: usize = 12;
const COMB_PLUS: usize = 13;
const COMB_MINUS: usize = 14;
const PLATE: usize = 15;
const MEMBRANE: usize = 16;

/// Frequency in hertz and the two macros, each 0 to 1.
#[derive(Clone, Copy)]
struct Knobs {
    hz: f64,
    m1: f64,
    m2: f64,
}

fn knobs(hz: f64, m1: f64, m2: f64) -> Knobs {
    Knobs { hz, m1, m2 }
}

fn pitch_of(hz: f64) -> f32 {
    (69.0 + 12.0 * (hz / 440.0).log2()) as f32
}

/// What a run does besides filtering: where each voice starts (a voice runs
/// from its frame, its input silent before), and the scale from each frame.
#[derive(Default, Clone)]
struct Plan {
    starts: Vec<usize>,
    scales: Vec<(usize, Option<Scale>)>,
}

/// `input` through filter type `ty`, `block` frames at a time, every block a
/// real-time section. Voice `v` hears `input` once it has started, and the
/// result is each voice's output. A block never crosses a voice's start or a
/// change of scale, as the engine's stretches don't.
fn run(
    ty: usize,
    rate: f64,
    knobs: impl Fn(usize) -> Knobs,
    input: &[f32],
    block: usize,
    plan: &Plan,
) -> Vec<Vec<f32>> {
    let starts = if plan.starts.is_empty() {
        vec![0]
    } else {
        plan.starts.clone()
    };
    let mut filter = MeldFilter::new(rate as f32, starts.len(), block);
    filter.set_type(ty);
    let stride = filter.stride();
    let mut out = vec![Vec::with_capacity(input.len()); starts.len()];
    let mut pos = 0;
    let mut scale = None;
    while pos < input.len() {
        let mut len = block.min(input.len() - pos);
        for edge in starts.iter().chain(plan.scales.iter().map(|(at, _)| at)) {
            if *edge > pos {
                len = len.min(edge - pos);
            }
        }
        for (lane, start) in starts.iter().enumerate() {
            if *start == pos {
                filter.reset_voice(lane);
            }
        }
        for (at, s) in &plan.scales {
            if *at <= pos {
                scale = *s;
            }
        }
        let hi = starts.iter().filter(|s| **s <= pos).count();
        let mut audio = vec![0.0f32; len * 2 * stride];
        let (mut cutoff, mut hz, mut m1, mut m2, mut morph) = (
            vec![0.0f32; len * stride],
            vec![0.0f32; len],
            vec![0.0f32; len],
            vec![0.0f32; len],
            vec![0.0f32; len],
        );
        for i in 0..len {
            let k = knobs(pos + i);
            for lane in 0..hi {
                audio[i * 2 * stride + lane] = input[pos + i];
            }
            cutoff[i * stride..(i + 1) * stride].fill(pitch_of(k.hz));
            (hz[i], m1[i], m2[i], morph[i]) =
                (k.hz as f32, k.m1 as f32, k.m2 as f32, (k.m2 * 0.75) as f32);
        }
        let controls = Controls {
            frame: PerFrame {
                cutoff: &cutoff,
                res: Per::Shared(&m1),
                morph: Per::Shared(&morph),
                drive: Per::Shared(&m2),
            },
            hz: &hz,
            macro_1: &m1,
            macro_2: &m2,
            scale,
        };
        bl_rt::rt_test!({ filter.process(hi, len, &controls, &mut audio) });
        for (lane, lane_out) in out.iter_mut().enumerate() {
            lane_out.extend((0..len).map(|i| {
                if lane < hi {
                    audio[i * 2 * stride + lane]
                } else {
                    0.0
                }
            }));
        }
        pos += len;
    }
    out
}

fn voice(ty: usize, k: Knobs, input: &[f32], block: usize) -> Vec<f32> {
    run(ty, HZ, |_| k, input, block, &Plan::default()).remove(0)
}

fn impulse(amplitude: f32, frames: usize) -> Vec<f32> {
    let mut x = vec![0.0; frames];
    x[0] = amplitude;
    x
}

fn noise(frames: usize, seed: u32) -> Vec<f32> {
    let mut state = seed | 1;
    (0..frames)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            (f64::from(state) / f64::from(u32::MAX) * 2.0 - 1.0) as f32
        })
        .collect()
}

fn sine(hz: f64, amplitude: f64, frames: usize) -> Vec<f32> {
    (0..frames)
        .map(|i| (amplitude * (TAU * hz * i as f64 / HZ).sin()) as f32)
        .collect()
}

fn peak(signal: &[f32]) -> f64 {
    signal.iter().fold(0.0f64, |m, x| m.max(f64::from(x.abs())))
}

/// The complex spectrum of `signal`, bin `i` at `i * HZ / len`.
fn fft(signal: &[f32]) -> Vec<C> {
    let mut input: Vec<f64> = signal.iter().map(|x| f64::from(*x)).collect();
    let plan = RealFftPlanner::<f64>::new().plan_fft_forward(input.len());
    let mut output = plan.make_output_vec();
    plan.process(&mut input, &mut output)
        .expect("the lengths match");
    output.iter().map(|c| (c.re, c.im)).collect()
}

fn mul(x: C, y: C) -> C {
    (x.0 * y.0 - x.1 * y.1, x.0 * y.1 + x.1 * y.0)
}

fn div(x: C, y: C) -> C {
    let d = y.0 * y.0 + y.1 * y.1;
    ((x.0 * y.0 + x.1 * y.1) / d, (x.1 * y.0 - x.0 * y.1) / d)
}

fn db(z: C) -> f64 {
    20.0 * z.0.hypot(z.1).max(1e-12).log10()
}

fn q_of(q0: f64, m: f64) -> f64 {
    q0 * (10.0 / q0).powf(m)
}

/// A second-order section's low-pass, band-pass, high-pass at `x` times its
/// cutoff on the warped axis, with quality `q`.
fn section(x: f64, q: f64) -> [C; 3] {
    let den = (1.0 - x * x, x / q);
    [
        div((1.0, 0.0), den),
        div((0.0, x / q), den),
        div((-x * x, 0.0), den),
    ]
}

fn warp(hz: f64) -> f64 {
    (PI * hz / HZ).tan()
}

/// The prototypes of the linear types at `hz`, written out again here: the
/// analogue response on the axis the bilinear transform warps, or for the
/// combs the transfer function of the loop at the digital frequency.
fn prototype(ty: usize, k: Knobs, hz: f64) -> C {
    let x = warp(hz) / warp(k.hz);
    let cascade = |qs: &[f64], tap: usize| {
        qs.iter()
            .fold((1.0, 0.0), |acc, q| mul(acc, section(x, *q)[tap]))
    };
    match ty {
        0 => cascade(&[q_of(SG_Q, k.m1)], 0),
        1 => cascade(&[0.541_196_1, q_of(1.306_563, k.m1)], 0),
        2 => cascade(&[q_of(SG_Q, k.m1)], 0),
        3 => cascade(&[q_of(SG_Q, k.m1)], 2),
        OSR => cascade(&[q_of(SG_Q, k.m1)], 1),
        EQ_PEAK | EQ_NOTCH => {
            let q = 0.3 * (12.0f64 / 0.3).powf(k.m2);
            let (num, den) = if ty == EQ_PEAK {
                let amp = 10f64.powf(k.m1 * 24.0 / 40.0);
                (amp / q, 1.0 / (amp * q))
            } else {
                ((1.0 - k.m1) / q, 1.0 / q)
            };
            div((1.0 - x * x, x * num), (1.0 - x * x, x * den))
        }
        PHASER => {
            let mut chain = (1.0, 0.0);
            for stage in 0..6 {
                let spacing = 0.5 * (0.2 + 1.8 * k.m2);
                let f = k.hz * ((stage as f64 - 2.5) * spacing).exp2();
                let xs = warp(hz) / warp(f);
                chain = mul(chain, div((1.0, -xs), (1.0, xs)));
            }
            let feedback = 0.95 * k.m1;
            div(chain, (1.0 + feedback * chain.0, feedback * chain.1))
        }
        VOWEL => {
            const TABLE: [[f64; 3]; 5] = [
                [730.0, 1090.0, 2440.0],
                [530.0, 1840.0, 2480.0],
                [270.0, 2290.0, 3010.0],
                [570.0, 840.0, 2410.0],
                [300.0, 870.0, 2240.0],
            ];
            let place = 4.0 * k.m2;
            let (first, share) = (
                (place as usize).min(3),
                place - (place as usize).min(3) as f64,
            );
            let q = 2.0 * 15f64.powf(k.m1);
            let mut sum = (0.0, 0.0);
            for (j, weight) in [1.0, 0.8, 0.6].into_iter().enumerate() {
                let f = (TABLE[first][j] * (1.0 - share) + TABLE[first + 1][j] * share) * k.hz
                    / 1_000.0;
                let band = section(warp(hz) / warp(f), q)[1];
                sum = (sum.0 + weight * band.0, sum.1 + weight * band.1);
            }
            sum
        }
        _ => {
            let fb = 0.99 * k.m1;
            let sign = if ty == COMB_PLUS { 1.0 } else { -1.0 };
            let damp = k.m2 > 0.0;
            let w = TAU * hz / HZ;
            let w0 = TAU * k.hz / HZ;
            // The low-pass, the all-pass and the whole frames of the loop.
            let lp = if damp {
                1.0 - (-TAU * 20_000.0 * (500.0f64 / 20_000.0).powf(k.m2) / HZ).exp()
            } else {
                1.0
            };
            let lp_at = |w: f64| {
                let z1 = (w.cos(), -w.sin());
                div((lp, 0.0), (1.0 - (1.0 - lp) * z1.0, -(1.0 - lp) * z1.1))
            };
            let lp_delay = ((1.0 - lp) * w0.sin()).atan2(1.0 - (1.0 - lp) * w0.cos()) / w0;
            let whole = HZ / k.hz - lp_delay;
            let frames = (whole - 0.5).floor();
            let t = ((whole - frames) * w0 / 2.0).tan() / (w0 / 2.0).tan();
            let coef = (1.0 - t) / (1.0 + t);
            let z1 = (w.cos(), -w.sin());
            let ap = div((coef + z1.0, z1.1), (1.0 + coef * z1.0, coef * z1.1));
            let delay = ((-w * frames).cos(), (-w * frames).sin());
            let loop_gain = mul(mul(lp_at(w), ap), delay);
            let norm = (1.0 - fb * fb).sqrt();
            div(
                (norm, 0.0),
                (1.0 - sign * fb * loop_gain.0, -sign * fb * loop_gain.1),
            )
        }
    }
}

// Spec: meld R40 to R42, R46, R47, R49, R50, R53 (the linear types)
#[test]
fn each_linear_type_follows_its_prototype_within_half_a_db_at_small_signal() {
    let frames = 1 << 16;
    let settings: [(usize, &[Knobs]); 11] = [
        (0, &[knobs(1_000.0, 0.0, 0.0), knobs(300.0, 0.0, 0.0)]),
        (1, &[knobs(2_000.0, 0.0, 0.0), knobs(500.0, 0.0, 0.0)]),
        (2, &[knobs(1_000.0, 0.0, 0.0), knobs(5_000.0, 0.7, 0.0)]),
        (3, &[knobs(1_000.0, 0.0, 0.0), knobs(200.0, 0.7, 0.0)]),
        (OSR, &[knobs(1_000.0, 0.3, 0.0), knobs(8_000.0, 0.9, 0.0)]),
        (EQ_PEAK, &[knobs(1_000.0, 1.0, 0.5), knobs(150.0, 0.5, 0.9)]),
        (
            EQ_NOTCH,
            &[knobs(1_000.0, 0.9, 0.5), knobs(4_000.0, 1.0, 0.2)],
        ),
        (PHASER, &[knobs(1_000.0, 0.5, 0.5), knobs(300.0, 0.9, 0.1)]),
        (VOWEL, &[knobs(1_000.0, 0.3, 0.0), knobs(1_200.0, 0.5, 0.6)]),
        (
            COMB_PLUS,
            &[knobs(500.0, 0.5, 0.0), knobs(2_000.0, 0.7, 0.4)],
        ),
        (
            COMB_MINUS,
            &[knobs(500.0, 0.5, 0.0), knobs(1_000.0, 0.6, 0.3)],
        ),
    ];
    for (ty, list) in settings {
        for k in list {
            let response = fft(&voice(ty, *k, &impulse(0.01, frames), 512));
            let mut checked = 0;
            for (bin, value) in response.iter().enumerate().skip(1) {
                let hz = bin as f64 * HZ / frames as f64;
                if !(20.0..=20_000.0).contains(&hz) {
                    continue;
                }
                let want = prototype(ty, *k, hz);
                let measured = (value.0 / 0.01, value.1 / 0.01);
                if db(want) < -50.0 {
                    continue;
                }
                checked += 1;
                assert!(
                    (db(measured) - db(want)).abs() < 0.5,
                    "type {ty} at {} Hz, macros {} and {}: {:.2} dB at {hz:.1} Hz, the prototype's {:.2}",
                    k.hz,
                    k.m1,
                    k.m2,
                    db(measured),
                    db(want)
                );
            }
            assert!(
                checked > 1_000,
                "type {ty}: only {checked} bins above -50 dB"
            );
        }
    }
}

// Spec: meld R44 (the switching rate and its clock)
#[test]
fn lp_switched_res_holds_its_output_between_ticks_that_count_from_the_voices_start() {
    let hold = |m2: f64, block: usize| {
        let input = sine(300.0, 0.5, 9_600);
        voice(LP_SWITCHED_RES, knobs(10_000.0, 0.0, m2), &input, block)
    };
    let steps = |signal: &[f32]| signal.windows(2).filter(|w| w[0] != w[1]).count();
    // A fifth of a second at 500 Hz switches a hundred times; at 44 kHz it
    // moves nearly every frame.
    let crushed = steps(&hold(1.0, 512));
    assert!(
        (95..=105).contains(&crushed),
        "{crushed} steps in a fifth of a second"
    );
    assert!(steps(&hold(0.0, 512)) > 4_000);
    // The clock starts with the voice: the first tick falls the same
    // number of frames in whatever the blocks.
    for block in BLOCKS {
        assert_same(&hold(1.0, block), &hold(1.0, 512), "the switching clock");
    }
    assert!(hold(1.0, 64).iter().any(|x| *x != 0.0), "it sounds");
}

// Spec: meld R43, R45, R48, R53 (the nonlinear types)
#[test]
fn each_nonlinear_type_stays_below_plus_12_dbfs_at_any_setting() {
    let loud: Vec<Vec<f32>> = vec![
        noise(8_000, 7),
        sine(110.0, 1.0, 8_000),
        sine(1_000.0, 1.0, 8_000),
    ];
    let limit = 10f64.powf(12.0 / 20.0);
    for ty in [
        2,
        3,
        OSR,
        LP_CRUNCH,
        LP_SWITCHED_RES,
        FILTHER,
        REDUX,
        PLATE,
        MEMBRANE,
    ] {
        for hz in [60.0, 1_000.0, 15_000.0] {
            for (m1, m2) in [(0.0, 0.0), (1.0, 1.0), (1.0, 0.0), (0.0, 1.0), (0.6, 0.6)] {
                for input in &loud {
                    let out = voice(ty, knobs(hz, m1, m2), input, 512);
                    assert!(
                        peak(&out) < limit,
                        "type {ty} at {hz} Hz, macros {m1} and {m2}: peak {}",
                        peak(&out)
                    );
                }
            }
        }
    }
}

/// The share of a signal's power outside the fundamental and its harmonic
/// `harmonic`, read by DFT over whole cycles.
fn harmonic(signal: &[f32], hz: f64, n: usize) -> f64 {
    let (mut re, mut im) = (0.0, 0.0);
    for (i, s) in signal.iter().enumerate() {
        let angle = TAU * hz * n as f64 * i as f64 / HZ;
        re += f64::from(*s) * angle.cos();
        im += f64::from(*s) * angle.sin();
    }
    2.0 * re.hypot(im) / signal.len() as f64
}

// Spec: meld R43 (distortion grows with Q and Drive)
#[test]
fn lp_crunch_distorts_more_as_q_and_drive_rise() {
    let input = sine(240.0, 0.25, 24_000);
    let distortion = |m1: f64, m2: f64| {
        let out = voice(LP_CRUNCH, knobs(2_000.0, m1, m2), &input, 512);
        let tail = &out[12_000..];
        let third = harmonic(tail, 240.0, 3) + harmonic(tail, 240.0, 5);
        third / harmonic(tail, 240.0, 1)
    };
    let (clean, queued, driven, both) = (
        distortion(0.0, 0.0),
        distortion(1.0, 0.0),
        distortion(0.0, 1.0),
        distortion(1.0, 1.0),
    );
    assert!(clean < 0.01, "{clean}");
    assert!(
        queued > clean && driven > clean,
        "{clean} {queued} {driven}"
    );
    assert!(
        both > 0.05 && both > queued && both > driven,
        "{queued} {driven} {both}"
    );
}

// Spec: meld R45 (a hard clip in, tanh out)
#[test]
fn filther_clips_hard_in_and_saturates_out() {
    let input = sine(500.0, 0.5, 24_000);
    let at = |m2: f64| voice(FILTHER, knobs(15_000.0, 0.0, m2), &input, 512);
    let (flat, driven) = (at(0.0), at(1.0));
    let third = |s: &[f32]| harmonic(&s[12_000..], 500.0, 3) / harmonic(&s[12_000..], 500.0, 1);
    assert!(third(&flat) < 0.02, "{}", third(&flat));
    // 24 dB of drive turns the sine into a square, its third harmonic a third
    // of its fundamental, under a tanh that holds it below 1.
    assert!(
        (third(&driven) - 1.0 / 3.0).abs() < 0.08,
        "{}",
        third(&driven)
    );
    assert!(peak(&driven) < 1.0 && peak(&driven) > 0.7);
}

// Spec: meld R48 (Redux's rate and bit depth)
#[test]
fn redux_holds_at_the_rate_and_quantises_to_the_bit_depth() {
    let ramp: Vec<f32> = (0..9_600).map(|i| (i as f32 / 4_800.0) - 1.0).collect();
    let values = |m1: f64, hz: f64| {
        let mut seen: Vec<f32> = voice(REDUX, knobs(hz, m1, 1.0), &ramp, 512);
        seen.sort_by(f32::total_cmp);
        seen.dedup();
        seen.len()
    };
    // Crush at 100 % is one bit; at 8/15 of its travel, eight.
    assert!(values(1.0, 20_000.0) <= 2);
    let eight = values(8.0 / 15.0, 20_000.0);
    assert!((128..=256).contains(&eight), "{eight} levels");
    // A tenth of the rate holds each value ten frames.
    let held = voice(REDUX, knobs(4_800.0, 0.0, 1.0), &ramp, 512);
    let runs = held.windows(2).filter(|w| w[0] != w[1]).count();
    assert!(
        (900..=1_000).contains(&runs),
        "{runs} changes in 9,600 frames"
    );
}

/// The frequency of the peak of `spectrum` nearest `near`, by a parabola
/// through the log magnitudes round it.
fn peak_near(spectrum: &[C], frames: usize, near: f64) -> Option<(f64, f64)> {
    let bin = |hz: f64| (hz * frames as f64 / HZ).round() as usize;
    let (lo, hi) = (bin(near * 0.97), bin(near * 1.03).min(spectrum.len() - 2));
    let mag = |i: usize| spectrum[i].0.hypot(spectrum[i].1);
    let top = (lo..=hi).max_by(|x, y| mag(*x).total_cmp(&mag(*y)))?;
    if top == lo || top == hi {
        return None;
    }
    let (l, m, r) = (mag(top - 1).ln(), mag(top).ln(), mag(top + 1).ln());
    let shift = 0.5 * (l - r) / (l - 2.0 * m + r);
    Some(((top as f64 + shift) * HZ / frames as f64, mag(top)))
}

// Spec: meld R51, R54 (the resonators' modes)
#[test]
fn the_resonators_ring_at_their_modal_ratios_within_a_cent() {
    let frames = 1 << 17;
    let f0 = 440.0;
    for (ty, shape, m2) in [
        (PLATE, Shape::Plate, 0.5),
        (MEMBRANE, Shape::CircularMembrane, 0.2),
    ] {
        let k = knobs(f0, 0.77, m2);
        let mut response = voice(ty, k, &impulse(0.5, frames), 512);
        // A Hann window leaves the line's shape alone and its skirts down.
        let n = response.len() as f64;
        for (i, x) in response.iter_mut().enumerate() {
            *x *= (0.5 - 0.5 * (TAU * i as f64 / n).cos()) as f32;
        }
        let spectrum = fft(&response);
        let table = ModalTable::new(shape, Quality::Medium, 1.0, 0.0);
        let loudest = (1..spectrum.len())
            .map(|i| spectrum[i].0.hypot(spectrum[i].1))
            .fold(0.0, f64::max);
        let mut checked = 0;
        for mode in 0..8 {
            let want = f0 * table.ratio(mode);
            let Some((found, level)) = peak_near(&spectrum, frames, want) else {
                continue;
            };
            if level < loudest * 0.01 {
                continue;
            }
            checked += 1;
            let cents = 1200.0 * (found / want).log2();
            assert!(
                cents.abs() < 1.0,
                "type {ty}, mode {mode}: {found:.3} Hz, {cents:.2} cents off {want:.3}"
            );
        }
        assert!(checked >= 4, "type {ty}: only {checked} modes found");
    }
}

// Spec: meld R52 (Filter Scale Awareness), scales R28
#[test]
fn the_resonators_snap_to_the_scale_from_the_frame_it_changes() {
    let input = noise(6_000, 3);
    let k = knobs(277.18, 0.7, 0.5); // C sharp 4: C and D are equally near
    let snapped = knobs(440.0 * (-9.0f64 / 12.0).exp2(), 0.7, 0.5);
    let change = 1_010; // between the bank's grid points of 32 frames
    for ty in [PLATE, MEMBRANE] {
        let still = |k: Knobs| voice(ty, k, &input, 512);
        let (loose, tuned) = (still(k), still(snapped));
        assert_ne!(loose, tuned);
        let plan = |at: usize| Plan {
            starts: vec![],
            scales: vec![(at, Some(c_major()))],
        };
        let from_start = run(ty, HZ, |_| k, &input, 512, &plan(0)).remove(0);
        assert_same(&from_start, &tuned, "snapped from the first frame");
        let mut reference = None;
        for block in [1, 64, 512, 4096] {
            let switched = run(ty, HZ, |_| k, &input, block, &plan(change)).remove(0);
            // Nothing moves before the frame; the pitch moves on it.
            assert_same(
                &switched[..change],
                &loose[..change],
                "before the scale changes",
            );
            assert!(
                (change..change + 3).any(|i| switched[i] != loose[i]),
                "type {ty}: the resonator still rings at its old pitch at frame {change}"
            );
            match &reference {
                None => reference = Some(switched),
                Some(first) => assert_same(&switched, first, "the change at any block size"),
            }
        }
    }
}

// Spec: meld R52, scales R28 (through the device)
#[test]
fn filter_scale_awareness_snaps_the_devices_resonator_and_follows_its_switch() {
    let base = |hz: f64, snap: bool, use_scale: bool| -> Params {
        vec![
            (a(MACRO_1), 70.0),
            (a(FILTER_TYPE), f64::from(PLATE as u32)),
            (a(FILTER_FREQ), hz),
            (a(FILTER_MACRO_1), 70.0),
            (a(FILTER_MACRO_2), 50.0),
            (a(FILTER_SCALE), f64::from(u8::from(snap))),
            (USE_SCALE, f64::from(u8::from(use_scale))),
        ]
    };
    let notes = [on(0, 60), off(9_000, 60)];
    let scaled = Setting {
        scales: vec![(0, c_major())],
        ..Setting::default()
    };
    let tuned = play(
        &base(440.0 * (-9.0f64 / 12.0).exp2(), false, false),
        12_000,
        &notes,
    );
    let [snapped, _] = play_with(&base(277.18, true, true), 512, 12_000, &notes, &scaled);
    assert_near(
        &snapped,
        &tuned[0],
        1e-5,
        "277.18 Hz snapped to C4 in C major",
    );
    // With either switch off the Frequency stays where it is set.
    let loose = play(&base(277.18, false, false), 12_000, &notes);
    for (snap, use_scale) in [(false, true), (true, false)] {
        let [left, _] = play_with(&base(277.18, snap, use_scale), 512, 12_000, &notes, &scaled);
        assert_near(&left, &loose[0], 1e-5, "the switches off");
    }
    assert!(rms(&snapped) > 1e-3);
}

// Spec: meld R56 (type changes)
#[test]
fn a_change_of_filter_type_makes_no_click() {
    let input = sine(220.0, 0.3, 12_000);
    // Redux's hold and the resonators' fundamental sit out of the way at 20 kHz.
    let k = knobs(20_000.0, 0.0, 0.0);
    let change = 6_000;
    let sine_step = 0.3 * TAU * 220.0 / HZ;
    for from in 0..TYPES {
        for to in 0..TYPES {
            if from == to {
                continue;
            }
            let mut filter = MeldFilter::new(HZ as f32, 1, 512);
            filter.set_type(from);
            filter.reset_voice(0);
            let stride = filter.stride();
            let mut out = Vec::new();
            for (n, chunk) in input.chunks(500).enumerate() {
                if n * 500 == change {
                    bl_rt::rt_test!({ filter.set_type(to) });
                }
                let len = chunk.len();
                let mut audio = vec![0.0f32; len * 2 * stride];
                for (i, x) in chunk.iter().enumerate() {
                    audio[i * 2 * stride] = *x;
                }
                let cutoff = vec![pitch_of(k.hz); len * stride];
                let (hz, m1, m2) = (
                    vec![k.hz as f32; len],
                    vec![k.m1 as f32; len],
                    vec![k.m2 as f32; len],
                );
                let controls = Controls {
                    frame: PerFrame {
                        cutoff: &cutoff,
                        res: Per::Shared(&m1),
                        morph: Per::Shared(&m2),
                        drive: Per::Shared(&m2),
                    },
                    hz: &hz,
                    macro_1: &m1,
                    macro_2: &m2,
                    scale: None,
                };
                bl_rt::rt_test!({ filter.process(1, len, &controls, &mut audio) });
                out.extend((0..len).map(|i| audio[i * 2 * stride]));
            }
            let step = out[change - 2..change + 400]
                .windows(2)
                .map(|w| f64::from((w[1] - w[0]).abs()))
                .fold(0.0, f64::max);
            assert!(
                step < sine_step * 8.0 + 0.01,
                "type {from} to {to}: a step of {step} (a sine's is {sine_step})"
            );
        }
    }
}

// Spec: meld R55, R57 (per voice, per frame, any block size)
#[test]
fn every_type_renders_the_same_at_block_sizes_1_64_512_and_4096_and_run_after_run() {
    let input = noise(9_000, 11);
    let plan = Plan {
        starts: vec![0, 3_000],
        scales: vec![(5_000, Some(c_major()))],
    };
    for ty in 0..TYPES {
        // Frequency and the macros move on every frame.
        let moving = |i: usize| {
            let t = i as f64 / 9_000.0;
            knobs(200.0 * 20f64.powf(t), 0.2 + 0.6 * t, 1.0 - 0.8 * t)
        };
        let reference = run(ty, HZ, moving, &input, 512, &plan);
        assert_eq!(
            reference,
            run(ty, HZ, moving, &input, 512, &plan),
            "type {ty} run after run"
        );
        for block in BLOCKS {
            let blocked = run(ty, HZ, moving, &input, block, &plan);
            for (lane, (got, want)) in blocked.iter().zip(&reference).enumerate() {
                assert_same(
                    got,
                    want,
                    &format!("type {ty}, voice {lane}, blocks of {block}"),
                );
            }
        }
        assert!(
            reference[0].iter().any(|x| *x != 0.0),
            "type {ty} is silent"
        );
        assert!(
            reference.iter().flatten().all(|x| x.is_finite()),
            "type {ty} isn't finite"
        );
    }
}

// Spec: meld R55, R58 (each type plays in the device, under its macros)
#[test]
fn every_filter_type_plays_in_the_device_and_its_macros_and_frequency_act() {
    let notes = [on(0, 48), off(14_000, 48)];
    let with = |ty: usize, hz: f64, m1: f64, m2: f64| -> Vec<f32> {
        let params: Params = vec![
            (a(FILTER_ON), 1.0),
            (a(FILTER_TYPE), ty as f64),
            (a(FILTER_FREQ), hz),
            (a(FILTER_MACRO_1), m1),
            (a(FILTER_MACRO_2), m2),
        ];
        play(&params, 16_000, &notes)[0].clone()
    };
    for ty in 0..TYPES {
        let base = with(ty, 1_500.0, 40.0, 40.0);
        assert!(rms(&base) > 1e-4, "type {ty} is silent");
        assert!(base.iter().all(|x| x.is_finite()));
        for (what, other) in [
            ("Frequency", with(ty, 700.0, 40.0, 40.0)),
            ("macro 1", with(ty, 1_500.0, 90.0, 40.0)),
            ("macro 2", with(ty, 1_500.0, 40.0, 90.0)),
        ] {
            // The SVF types' second macro is a morph; every other control
            // of every type changes the sound.
            assert_ne!(base, other, "type {ty}: {what} does nothing");
        }
    }
}

const GOLDEN_AT: [usize; 5] = [500, 2_000, 6_000, 10_000, 13_000];
const GOLDEN_WINDOW: usize = 512;

// Spec: meld R40 to R51 (the golden renders)
#[test]
fn each_filter_type_renders_a_phrase_as_it_did_when_it_was_written() {
    let notes = [on(0, 48), on(6_000, 55), off(12_000, 48), off(14_000, 55)];
    for (ty, want) in GOLDEN.iter().enumerate() {
        let params: Params = vec![
            (a(FILTER_ON), 1.0),
            (a(FILTER_TYPE), ty as f64),
            (a(FILTER_FREQ), 1_200.0),
            (a(FILTER_MACRO_1), 45.0),
            (a(FILTER_MACRO_2), 55.0),
        ];
        let [left, _] = play(&params, 16_000, &notes);
        for (&at, &level) in GOLDEN_AT.iter().zip(want) {
            let measured = rms(&left[at..at + GOLDEN_WINDOW]);
            assert!(
                (measured - level).abs() <= 2e-3 * level + 1e-5,
                "type {ty}, frames {at}+: RMS {measured} isn't {level}"
            );
        }
    }
}

/// The RMS of each window of the golden render of each filter type in the
/// left channel, rendered when the types were written. A window's level, not
/// a sample, is compared: the platforms' maths libraries differ in the last
/// bit, and the feedback and the clocks carry that into single samples.
const GOLDEN: [[f64; 5]; TYPES] = [
    [0.0283835, 0.0273637, 0.0604274, 0.0455807, 0.0420344],
    [0.0028172, 0.0029058, 0.0087138, 0.0062261, 0.0063108],
    [1.7109762, 1.7119141, 2.9392869, 2.0559294, 1.6639500],
    [0.2909265, 0.2863743, 0.5642633, 0.4468165, 0.4135934],
    [0.1567843, 0.1516394, 0.3191172, 0.2380994, 0.2272366],
    [2.9024147, 2.9547103, 4.9753039, 3.3605168, 2.8236123],
    [0.6878125, 0.7105130, 1.1948903, 0.8133794, 0.6826039],
    [0.7323591, 0.7324382, 1.2426931, 0.8794373, 0.7094844],
    [0.6935420, 0.7193291, 1.1907585, 0.7596800, 0.6921782],
    [0.6873933, 0.7121531, 1.1840462, 0.7752253, 0.6800490],
    [0.6668990, 0.6443764, 1.2518289, 1.1113671, 0.9591913],
    [0.6598862, 0.6595942, 1.1128907, 0.8556954, 0.6003440],
    [0.0466307, 0.0452597, 0.1071014, 0.0784927, 0.0771724],
    [0.8546745, 0.8700475, 1.3980113, 0.9779267, 0.6864698],
    [0.4544988, 0.4711415, 0.7927548, 0.4912431, 0.4835772],
    [0.2317762, 0.1377338, 0.2127235, 0.1376358, 0.1231654],
    [0.1747991, 0.1314863, 0.2101734, 0.1359931, 0.1217716],
];

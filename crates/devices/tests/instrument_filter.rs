//! The instruments' filter (WP-8.3; the instrument filter spec): its types,
//! slopes and circuits against their prototypes, the circuits' levels and
//! distortion, Morph, Drive, per-frame values, type changes and stability.
//! Every block runs as a real-time section under the allocation trap.

use std::f64::consts::{PI, TAU};

use bl_devices::instrument_filter::{
    CIRCUITS, Circuit, InstrumentFilter, Kind, Per, PerFrame, Setup, offered,
};

type C = (f64, f64);

fn pitch_of(hz: f64) -> f32 {
    (69.0 + 12.0 * (hz / 440.0).log2()) as f32
}

/// The setup of type `kind` (a `Kind` index), slope and circuit.
fn setup(kind: usize, slope24: bool, circuit: Circuit) -> Setup {
    Setup::new(kind, slope24, circuit as usize)
}

/// Everything one run sets: the cutoff in hertz and the three controls,
/// each 0 to 1 (Drive of its 24 dB).
#[derive(Clone, Copy)]
struct Knobs {
    hz: f64,
    res: f32,
    morph: f32,
    drive: f32,
}

impl Knobs {
    fn at(hz: f64, res: f32) -> Self {
        Self {
            hz,
            res,
            morph: 0.0,
            drive: 0.0,
        }
    }
}

/// `input` through one voice of `setup` at `rate`, `block` frames at a time,
/// every block a real-time section; the left channel out.
fn render(setup: Setup, rate: f64, knobs: Knobs, input: &[f32], block: usize) -> Vec<f32> {
    let mut filter = InstrumentFilter::new(rate as f32, 1, block);
    filter.set_setup(setup);
    filter.reset_voice(0, 0);
    let stride = filter.stride();
    let mut out = Vec::with_capacity(input.len());
    for chunk in input.chunks(block) {
        let n = chunk.len();
        let mut audio = vec![0.0f32; n * 2 * stride];
        for (i, x) in chunk.iter().enumerate() {
            audio[i * 2 * stride] = *x;
            audio[i * 2 * stride + stride] = *x;
        }
        let cutoff = vec![pitch_of(knobs.hz); n * stride];
        let (res, morph, drive) = (
            vec![knobs.res; n],
            vec![knobs.morph; n],
            vec![knobs.drive; n],
        );
        let controls = PerFrame {
            cutoff: &cutoff,
            res: Per::Shared(&res),
            morph: Per::Shared(&morph),
            drive: Per::Shared(&drive),
        };
        bl_rt::rt_test!({ filter.process(1, n, &controls, &mut audio) });
        out.extend((0..n).map(|i| audio[i * 2 * stride]));
    }
    out
}

fn sine(rate: f64, hz: f64, amplitude: f64, frames: usize) -> Vec<f32> {
    (0..frames)
        .map(|i| (amplitude * (TAU * hz * i as f64 / rate).sin()) as f32)
        .collect()
}

fn peak(signal: &[f32]) -> f64 {
    signal.iter().fold(0.0f64, |m, x| m.max(f64::from(x.abs())))
}

fn rms(signal: &[f32]) -> f64 {
    (signal.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>() / signal.len() as f64).sqrt()
}

// ---- the prototypes, written out again here ----

fn mul(a: C, b: C) -> C {
    (a.0 * b.0 - a.1 * b.1, a.0 * b.1 + a.1 * b.0)
}

fn div(a: C, b: C) -> C {
    let d = b.0 * b.0 + b.1 * b.1;
    ((a.0 * b.0 + a.1 * b.1) / d, (a.1 * b.0 - a.0 * b.1) / d)
}

/// A second-order section's low-pass, band-pass, high-pass and notch at
/// `x` times its cutoff on the warped axis, with quality `q`.
fn section(x: f64, q: f64) -> [C; 4] {
    let den = (1.0 - x * x, x / q);
    [
        div((1.0, 0.0), den),
        div((0.0, x / q), den),
        div((-x * x, 0.0), den),
        div((1.0 - x * x, 0.0), den),
    ]
}

/// The most Q a section reaches, and a 24 dB response's sections' Qs.
const Q_MAX: f64 = 10.0;

fn q_of(q0: f64, res: f64) -> f64 {
    q0 * (Q_MAX / q0).powf(res)
}

/// The prototype's response at `hz`, for the circuits other than Clean: the
/// state-variable section for OSR and the Sallen-Key section for MS2 and
/// SMP (the same second-order response), a four-pole ladder for PRD.
fn prototype(setup: Setup, rate: f64, fc: f64, res: f64, hz: f64) -> C {
    let warp = |f: f64| (PI * f / rate).tan();
    let x = warp(hz) / warp(fc);
    if setup.circuit == Circuit::Prd {
        let k = 4.2 * res;
        let s = (0.0, x);
        let one = (1.0, 0.0);
        let low = div(one, (1.0 + s.0, s.1));
        let high = div(s, (1.0 + s.0, s.1));
        let stage = if setup.kind == Kind::HighPass {
            high
        } else {
            low
        };
        let pow = |z: C, n: usize| (0..n).fold(one, |acc, _| mul(acc, z));
        let loop_ = pow(stage, 4);
        let out = pow(stage, if setup.slope24 { 4 } else { 2 });
        let h = div(out, (1.0 + k * loop_.0, k * loop_.1));
        return (h.0 * (1.0 + 0.5 * k), h.1 * (1.0 + 0.5 * k));
    }
    let qs = if setup.slope24 {
        vec![0.541_196_1, q_of(1.306_563, res)]
    } else {
        vec![q_of(std::f64::consts::FRAC_1_SQRT_2, res)]
    };
    qs.into_iter().fold((1.0, 0.0), |acc, q| {
        let taps = section(x, q);
        let tap = match setup.kind {
            Kind::LowPass => taps[0],
            Kind::BandPass => taps[1],
            Kind::HighPass => taps[2],
            _ => taps[3],
        };
        mul(acc, tap)
    })
}

fn db(z: C) -> f64 {
    20.0 * z.0.hypot(z.1).max(1e-12).log10()
}

/// The response at `hz` of the impulse response `ir`, which was made with
/// an impulse of `amplitude`.
fn response_at(ir: &[f32], hz: f64, rate: f64, amplitude: f64) -> C {
    let w = TAU * hz / rate;
    let (mut re, mut im) = (0.0f64, 0.0f64);
    for (n, v) in ir.iter().enumerate() {
        re += f64::from(*v) * (w * n as f64).cos();
        im -= f64::from(*v) * (w * n as f64).sin();
    }
    (re / amplitude, im / amplitude)
}

fn impulse(frames: usize, amplitude: f32) -> Vec<f32> {
    let mut input = vec![0.0f32; frames];
    input[0] = amplitude;
    input
}

/// The circuits and types the response test covers.
fn circuit_setups() -> Vec<Setup> {
    let mut all = Vec::new();
    for kind in 0..4 {
        for slope24 in [false, true] {
            for circuit in [Circuit::Osr, Circuit::Ms2, Circuit::Smp, Circuit::Prd] {
                let s = setup(kind, slope24, circuit);
                if s.circuit == circuit {
                    all.push(s);
                }
            }
        }
    }
    all
}

// Spec: instrument filter R4 (every circuit follows its prototype at small
// signal)
#[test]
fn every_circuit_follows_its_prototype_within_half_a_db_at_small_signal() {
    for rate in [44_100.0, 48_000.0] {
        for s in circuit_setups() {
            for fc in [100.0, 1_000.0, 10_000.0] {
                for res in [0.0, 0.5, 0.9] {
                    let amplitude = 0.01;
                    let ir = render(
                        s,
                        rate,
                        Knobs::at(fc, res as f32),
                        &impulse(rate as usize, amplitude as f32),
                        512,
                    );
                    let mut worst = (0.0f64, 0.0f64);
                    for i in 0..48 {
                        let hz = 20.0 * 1000f64.powf(f64::from(i) / 47.0);
                        let expected = prototype(s, rate, fc, res, hz);
                        // Only where the response is far above the audio's
                        // noise floor.
                        if db(expected) < -50.0 {
                            continue;
                        }
                        let measured = db(response_at(&ir, hz, rate, amplitude));
                        let off = (measured - db(expected)).abs();
                        if off > worst.0 {
                            worst = (off, hz);
                        }
                    }
                    assert!(
                        worst.0 <= 0.5,
                        "{:?} at {rate} Hz, cutoff {fc}, res {res}: off by {:.3} dB at {:.0} Hz",
                        s,
                        worst.0,
                        worst.1
                    );
                }
            }
        }
    }
}

/// A deterministic white noise at `amplitude`.
fn noise(frames: usize, amplitude: f32) -> Vec<f32> {
    let mut state = 0x1234_5678u32;
    (0..frames)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            amplitude * (state as f32 / u32::MAX as f32 * 2.0 - 1.0)
        })
        .collect()
}

/// The amplitude of `hz` in `signal` by correlation over whole cycles.
fn amplitude_at(signal: &[f32], rate: f64, hz: f64) -> f64 {
    let cycles = (signal.len() as f64 * hz / rate).floor().max(1.0);
    let n = (cycles * rate / hz).round() as usize;
    let (mut a, mut b) = (0.0, 0.0);
    for (i, x) in signal[..n].iter().enumerate() {
        let w = TAU * hz * i as f64 / rate;
        a += f64::from(*x) * w.sin();
        b += f64::from(*x) * w.cos();
    }
    2.0 * a.hypot(b) / n as f64
}

/// Total harmonic distortion of a steady `signal` of `hz`: the harmonics' RMS
/// over the fundamental's amplitude.
fn thd(signal: &[f32], rate: f64, hz: f64) -> f64 {
    let fundamental = amplitude_at(signal, rate, hz);
    let harmonics: f64 = (2..=10)
        .map(|n| amplitude_at(signal, rate, hz * f64::from(n)).powi(2))
        .sum();
    harmonics.sqrt() / fundamental
}

#[test]
fn scratch_levels() {
    let rate = 48_000.0;
    for circuit in [Circuit::Osr, Circuit::Ms2, Circuit::Smp, Circuit::Prd] {
        for slope24 in [false, true] {
            let s = setup(0, slope24, circuit);
            // Loud noise at full resonance and drive.
            let mut k = Knobs::at(1_000.0, 1.0);
            k.drive = 1.0;
            let out = render(s, rate, k, &noise(48_000, 1.0), 512);
            // A sine at the cutoff.
            let sin = render(
                s,
                rate,
                Knobs {
                    drive: 0.0,
                    ..Knobs::at(1_000.0, 1.0)
                },
                &sine(rate, 1_000.0, 1.0, 48_000),
                512,
            );
            // A kick, then silence.
            let mut kick = vec![0.0f32; 96_000];
            kick[..64].fill(0.5);
            let ring = render(s, rate, Knobs::at(1_000.0, 1.0), &kick, 512);
            let tail = &ring[48_000..];
            let found = (900..1100)
                .map(|hz| (hz, amplitude_at(&tail[..24_000], rate, f64::from(hz))))
                .fold((0, 0.0), |m, x| if x.1 > m.1 { x } else { m });
            println!(
                "{circuit:?} 24={slope24}: noise peak {:.3} ({:.1} dBFS); sine peak {:.3}; ring peak {:.3}, tail peak {:.4} at {} Hz amp {:.4}",
                peak(&out),
                20.0 * peak(&out).log10(),
                peak(&sin),
                peak(&ring),
                peak(tail),
                found.0,
                found.1
            );
        }
    }
    // R8's distortion
    for circuit in [Circuit::Ms2, Circuit::Smp, Circuit::Prd] {
        let s = setup(0, false, circuit);
        let k = Knobs {
            drive: 0.5,
            ..Knobs::at(1_000.0, 0.7)
        };
        let out = render(s, rate, k, &sine(rate, 100.0, 0.5, 48_000), 512);
        println!(
            "{circuit:?} THD {:.4}  peak {:.3}",
            thd(&out[24_000..], rate, 100.0),
            peak(&out)
        );
    }
}

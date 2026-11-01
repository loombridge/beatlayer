//! The instruments' filter (WP-8.3; the instrument filter spec): its types,
//! slopes and circuits against their prototypes, the circuits' levels and
//! distortion, Morph, Drive, per-frame values, type changes and stability.
//! Every block runs as a real-time section under the allocation trap.

use crate::common;
use std::f64::consts::{PI, TAU};
use std::sync::Arc;
use std::time::Instant;

use bl_devices::sampler::{
    FILTER, FILTER_CIRCUIT, FILTER_DRIVE, FILTER_FREQ, FILTER_RES, FILTER_TYPE, LOOP, VOICES,
    sampler_type,
};
use bl_engine::{
    Block, Collector, DeviceType, NoteBuffer, NoteEvent, NoteKind, Registry, SlotSample,
    TransportInfo,
};

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
/// every block a real-time section; the left channel out. `knobs` gives each
/// frame's values from its frame number.
fn render_frames(
    setup: Setup,
    rate: f64,
    knobs: impl Fn(usize) -> Knobs,
    input: &[f32],
    block: usize,
) -> Vec<f32> {
    let mut filter = InstrumentFilter::new(rate as f32, 1, block);
    filter.set_setup(setup);
    filter.reset_voice(0, 0);
    run_filter(&mut filter, 0, knobs, input, block)
}

/// Runs `input` through `filter`'s voice 0, from frame `from` of the run.
fn run_filter(
    filter: &mut InstrumentFilter,
    from: usize,
    knobs: impl Fn(usize) -> Knobs,
    input: &[f32],
    block: usize,
) -> Vec<f32> {
    let stride = filter.stride();
    let mut out = Vec::with_capacity(input.len());
    for (n, chunk) in input.chunks(block).enumerate() {
        let frames = chunk.len();
        let first = from + n * block;
        let mut audio = vec![0.0f32; frames * 2 * stride];
        let (mut cutoff, mut res, mut morph, mut drive) = (
            vec![0.0f32; frames * stride],
            vec![0.0f32; frames],
            vec![0.0f32; frames],
            vec![0.0f32; frames],
        );
        for (i, x) in chunk.iter().enumerate() {
            let k = knobs(first + i);
            audio[i * 2 * stride] = *x;
            audio[i * 2 * stride + stride] = *x;
            cutoff[i * stride] = pitch_of(k.hz);
            (res[i], morph[i], drive[i]) = (k.res, k.morph, k.drive);
        }
        let controls = PerFrame {
            cutoff: &cutoff,
            res: Per::Shared(&res),
            morph: Per::Shared(&morph),
            drive: Per::Shared(&drive),
        };
        bl_rt::rt_test!({ filter.process(1, frames, &controls, &mut audio) });
        out.extend((0..frames).map(|i| audio[i * 2 * stride]));
    }
    out
}

fn render(setup: Setup, rate: f64, knobs: Knobs, input: &[f32], block: usize) -> Vec<f32> {
    render_frames(setup, rate, |_| knobs, input, block)
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

/// The gain, as output over input amplitude, of a sine of `hz` at `amplitude`
/// through `setup`, once it has settled.
fn gain_at(setup: Setup, rate: f64, knobs: Knobs, hz: f64, amplitude: f64) -> f64 {
    let frames = rate as usize;
    let out = render(setup, rate, knobs, &sine(rate, hz, amplitude, frames), 512);
    amplitude_at(&out[frames / 2..], rate, hz) / amplitude
}

/// `Setup`s of every circuit on every type that plays it, both slopes.
fn all_setups(circuits: &[Circuit]) -> Vec<Setup> {
    let mut all = Vec::new();
    for kind in 0..5 {
        for slope24 in [false, true] {
            for circuit in circuits {
                let s = setup(kind, slope24, *circuit);
                if s.circuit == *circuit {
                    all.push(s);
                }
            }
        }
    }
    all
}

const ALL_CIRCUITS: [Circuit; 5] = [
    Circuit::Clean,
    Circuit::Osr,
    Circuit::Ms2,
    Circuit::Smp,
    Circuit::Prd,
];

// Spec: instrument filter R1, R2 (the circuits each type offers)
#[test]
fn the_chooser_offers_only_the_circuits_a_type_plays() {
    assert_eq!(CIRCUITS, ["Clean", "OSR", "MS2", "SMP", "PRD"]);
    for kind in 0..5 {
        let kind = Kind::of(kind);
        let list = offered(kind);
        assert!(list.contains(&Circuit::Clean) && list.contains(&Circuit::Osr));
        let wide = matches!(kind, Kind::LowPass | Kind::HighPass);
        assert_eq!(list.len(), if wide { 5 } else { 2 }, "{kind:?}");
    }
    // A circuit the type doesn't offer plays Clean, in sound as in name.
    let rate = 48_000.0;
    let input = noise(4_800, 0.5);
    for kind in [2, 3, 4] {
        for circuit in [Circuit::Ms2, Circuit::Smp, Circuit::Prd] {
            let asked = setup(kind, false, circuit);
            assert_eq!(asked.circuit, Circuit::Clean);
            let knobs = Knobs::at(1_000.0, 0.5);
            assert_eq!(
                render(asked, rate, knobs, &input, 512),
                render(setup(kind, false, Circuit::Clean), rate, knobs, &input, 512)
            );
        }
    }
    assert_eq!(setup(0, true, Circuit::Prd).circuit, Circuit::Prd);
}

// Spec: instrument filter R3 (Clean is the kit's state-variable filter)
#[test]
fn clean_is_the_kits_filter_sample_for_sample() {
    use lspk_dsp::filter::{FilterBank, FilterType, VoiceParams};
    let rate = 48_000.0;
    let input = noise(9_600, 0.8);
    for (kind, slope24, kit) in [
        (0, false, FilterType::Lp12),
        (0, true, FilterType::Lp24),
        (1, false, FilterType::Hp12),
        (2, true, FilterType::Bp24),
        (3, false, FilterType::Notch12),
    ] {
        let mut bank = FilterBank::new(rate as f32, 1, 2);
        bank.set_type(kit);
        let stride = bank.stride();
        bank.reset_voice(
            0,
            &VoiceParams {
                resonance: 0.4,
                ..VoiceParams::default()
            },
        );
        let mut audio = vec![0.0f32; input.len() * 2 * stride];
        for (i, x) in input.iter().enumerate() {
            audio[i * 2 * stride] = *x;
            audio[i * 2 * stride + stride] = *x;
        }
        let pitch = vec![pitch_of(900.0); input.len() * stride];
        bank.process_voices(1, &pitch, &mut audio);
        let want: Vec<f32> = (0..input.len()).map(|i| audio[i * 2 * stride]).collect();
        let got = render(
            setup(kind, slope24, Circuit::Clean),
            rate,
            Knobs::at(900.0, 0.4),
            &input,
            512,
        );
        assert_eq!(got, want, "kind {kind} slope24 {slope24}");
    }
}

// Spec: instrument filter R5 (OSR's state is held by a hard clip)
#[test]
fn osr_holds_its_peak_and_stays_under_plus_six_dbfs() {
    for rate in [44_100.0, 48_000.0] {
        for s in all_setups(&[Circuit::Osr]) {
            let knobs = Knobs {
                drive: 1.0,
                ..Knobs::at(1_000.0, 1.0)
            };
            let out = render(s, rate, knobs, &noise(rate as usize, 1.0), 512);
            assert!(peak(&out) < 2.0, "{s:?}: peak {}", peak(&out));
        }
    }
    // The resonant peak stops growing: a tenfold louder sine at the cutoff
    // comes out far less than tenfold louder.
    let s = setup(0, false, Circuit::Osr);
    let knobs = Knobs::at(1_000.0, 1.0);
    let quiet = gain_at(s, 48_000.0, knobs, 1_000.0, 0.05);
    let loud = gain_at(s, 48_000.0, knobs, 1_000.0, 0.5);
    assert!(loud < quiet / 3.0, "gain {quiet} falls to only {loud}");
}

// Spec: instrument filter R6 (MS2's feedback softens, and it rings down)
#[test]
fn ms2_rounds_its_peak_off_rings_down_and_stays_under_plus_six_dbfs() {
    let rate = 48_000.0;
    for s in all_setups(&[Circuit::Ms2]) {
        let knobs = Knobs {
            drive: 1.0,
            ..Knobs::at(1_000.0, 1.0)
        };
        let out = render(s, rate, knobs, &noise(rate as usize, 1.0), 512);
        assert!(peak(&out) < 2.0, "{s:?}: peak {}", peak(&out));
        // A kick at 100 %, then silence: it rings down.
        let mut kick = vec![0.0f32; rate as usize];
        kick[..64].fill(0.5);
        let ring = render(s, rate, Knobs::at(1_000.0, 1.0), &kick, 512);
        assert!(peak(&ring) < 2.0);
        assert!(peak(&ring[24_000..]) < 1e-4, "{s:?} rings on");
    }
    // The peak rounds off gradually: each louder sine at the cutoff gains
    // less than the one before.
    let s = setup(0, false, Circuit::Ms2);
    let gains: Vec<f64> = [0.01, 0.1, 0.3, 1.0]
        .iter()
        .map(|a| gain_at(s, rate, Knobs::at(1_000.0, 0.9), 1_000.0, *a))
        .collect();
    assert!(gains.windows(2).all(|w| w[1] < w[0]), "{gains:?}");
}

/// The frequency near `hz` where `signal` rings loudest, within 100 cents,
/// as cents from `hz`.
fn cents_off(signal: &[f32], rate: f64, hz: f64) -> f64 {
    (-50..=50)
        .map(|step| f64::from(step) * 2.0)
        .map(|cents| {
            (
                cents,
                amplitude_at(signal, rate, hz * (cents / 1200.0).exp2()),
            )
        })
        .fold((0.0, 0.0), |m, x| if x.1 > m.1 { x } else { m })
        .0
}

// Spec: instrument filter R7 (PRD self-oscillates at its cutoff)
#[test]
fn prd_self_oscillates_at_its_cutoff_and_stays_under_plus_twelve_dbfs() {
    for rate in [44_100.0, 48_000.0] {
        for s in all_setups(&[Circuit::Prd]) {
            for fc in [100.0, 1_000.0, 10_000.0] {
                let mut kick = vec![0.0f32; 2 * rate as usize];
                kick[..64].fill(0.5);
                let ring = render(s, rate, Knobs::at(fc, 1.0), &kick, 512);
                let tail = &ring[rate as usize..];
                assert!(rms(tail) > 0.01, "{s:?} at {fc}: it stops");
                let off = cents_off(&tail[..rate as usize / 2], rate, fc);
                assert!(off.abs() <= 50.0, "{s:?} at {fc}: {off} cents off");
                assert!(peak(&ring) < 3.98, "{s:?} at {fc}: peak {}", peak(&ring));
            }
            let knobs = Knobs {
                drive: 1.0,
                ..Knobs::at(1_000.0, 1.0)
            };
            let out = render(s, rate, knobs, &noise(rate as usize, 1.0), 512);
            assert!(peak(&out) < 3.98, "{s:?}: peak {}", peak(&out));
        }
    }
    // Loud input pulls the resonance down less than MS2's does.
    let rate = 48_000.0;
    let pull = |circuit| {
        let s = setup(0, false, circuit);
        let knobs = Knobs::at(1_000.0, 0.9);
        gain_at(s, rate, knobs, 1_000.0, 0.1) / gain_at(s, rate, knobs, 1_000.0, 0.01)
    };
    let (prd, ms2) = (pull(Circuit::Prd), pull(Circuit::Ms2));
    assert!(prd > ms2, "PRD keeps {prd}, MS2 {ms2}");
}

// Spec: instrument filter R8 (SMP lies between MS2 and PRD)
#[test]
fn smps_distortion_lies_between_ms2s_and_prds() {
    let rate = 48_000.0;
    let knobs = Knobs {
        drive: 0.5,
        ..Knobs::at(1_000.0, 0.7)
    };
    let distortion = |circuit| {
        let out = render(
            setup(0, false, circuit),
            rate,
            knobs,
            &sine(rate, 100.0, 0.5, rate as usize),
            512,
        );
        thd(&out[24_000..], rate, 100.0)
    };
    let (ms2, smp, prd) = (
        distortion(Circuit::Ms2),
        distortion(Circuit::Smp),
        distortion(Circuit::Prd),
    );
    assert!(ms2 < smp && smp < prd, "THD {ms2}, {smp}, {prd}");
}

// Spec: instrument filter R9 (Drive is a gain into the circuit)
#[test]
fn drive_is_a_gain_for_the_circuits_on_low_pass_high_pass_and_band_pass() {
    let rate = 48_000.0;
    let level = |s: Setup, drive: f32| {
        let (fc, hz) = match s.kind {
            Kind::LowPass => (10_000.0, 100.0),
            Kind::HighPass => (100.0, 5_000.0),
            _ => (1_000.0, 1_000.0),
        };
        let knobs = Knobs {
            drive,
            ..Knobs::at(fc, 0.0)
        };
        let out = render(s, rate, knobs, &sine(rate, hz, 0.001, 24_000), 512);
        amplitude_at(&out[12_000..], rate, hz)
    };
    for kind in 0..3 {
        for circuit in [Circuit::Osr, Circuit::Ms2, Circuit::Smp, Circuit::Prd] {
            let s = setup(kind, false, circuit);
            if s.circuit != circuit {
                continue;
            }
            let rise = 20.0 * (level(s, 0.5) / level(s, 0.0)).log10();
            assert!((rise - 12.0).abs() < 0.3, "{s:?}: {rise} dB for 12 dB");
        }
    }
    // Clean, notch and Morph ignore it.
    let input = noise(9_600, 0.5);
    for s in all_setups(&ALL_CIRCUITS) {
        if s.circuit == Circuit::Clean || matches!(s.kind, Kind::Notch | Kind::Morph) {
            let with = |drive| {
                let knobs = Knobs {
                    drive,
                    ..Knobs::at(1_000.0, 0.5)
                };
                render(s, rate, knobs, &input, 512)
            };
            assert_eq!(with(0.0), with(1.0), "{s:?}");
        }
    }
}

// Spec: instrument filter R10 (Morph sweeps low-pass, band-pass, high-pass,
// notch and back)
#[test]
fn morph_passes_through_the_four_types_in_order() {
    let rate = 48_000.0;
    let input = noise(9_600, 0.5);
    for circuit in [Circuit::Clean, Circuit::Osr] {
        for slope24 in [false, true] {
            let pure = |kind: usize| {
                render(
                    setup(kind, slope24, circuit),
                    rate,
                    Knobs::at(1_000.0, 0.5),
                    &input,
                    512,
                )
            };
            let at = |morph: f32| {
                let knobs = Knobs {
                    morph,
                    ..Knobs::at(1_000.0, 0.5)
                };
                render(setup(4, slope24, circuit), rate, knobs, &input, 512)
            };
            // Each stop is the pure type: low-pass, band-pass, high-pass,
            // notch, and low-pass again.
            for (morph, kind) in [(0.0, 0), (0.25, 2), (0.5, 1), (0.75, 3), (1.0, 0)] {
                assert_eq!(
                    at(morph),
                    pure(kind),
                    "{circuit:?} slope24 {slope24} at {morph}"
                );
            }
            // Between two stops it is the crossfade of the two (a 12 dB
            // section's taps; Clean mixes the finished types).
            if !(circuit == Circuit::Osr && slope24) {
                let (a, b, mid) = (pure(0), pure(2), at(0.125));
                let worst = (0..input.len())
                    .map(|i| (mid[i] - (a[i] + b[i]) / 2.0).abs())
                    .fold(0.0f32, f32::max);
                assert!(worst < 1e-4, "{circuit:?}: off by {worst}");
            }
        }
    }
}

/// A sweep of the cutoff, resonance, Morph and Drive with the frame.
fn moving(frame: usize) -> Knobs {
    let t = frame as f64 / 48_000.0;
    Knobs {
        hz: 1_000.0 * (1.5 * (TAU * 3.0 * t).sin()).exp2(),
        res: (0.5 + 0.5 * (TAU * 5.0 * t).sin()) as f32,
        morph: (0.5 + 0.5 * (TAU * 2.0 * t).sin()) as f32,
        drive: (0.5 + 0.5 * (TAU * 7.0 * t).cos()) as f32,
    }
}

// Spec: instrument filter R11, R13 (the same output at any block size, with
// every value moving at every frame)
#[test]
fn a_voices_output_is_the_same_at_any_block_size() {
    let rate = 48_000.0;
    let input = noise(12_000, 0.7);
    for s in all_setups(&ALL_CIRCUITS) {
        let reference = render_frames(s, rate, moving, &input, 12_000);
        for block in [1, 64, 512, 4_096] {
            let got = render_frames(s, rate, moving, &input, block);
            assert_eq!(got, reference, "{s:?} at {block}");
        }
    }
}

// Spec: instrument filter R11 (the cutoff modulates at audio rate)
#[test]
fn a_cutoff_modulated_at_audio_rate_is_heard_at_audio_rate() {
    let rate = 48_000.0;
    let input = sine(rate, 5_000.0, 0.5, 24_000);
    for circuit in [Circuit::Clean, Circuit::Osr, Circuit::Ms2, Circuit::Prd] {
        let s = setup(0, false, circuit);
        let sidebands = |out: &[f32]| {
            amplitude_at(&out[12_000..], rate, 3_000.0)
                + amplitude_at(&out[12_000..], rate, 7_000.0)
        };
        let steady = render(s, rate, Knobs::at(4_000.0, 0.3), &input, 512);
        let modulated = render_frames(
            s,
            rate,
            |frame| {
                Knobs::at(
                    4_000.0 * (0.5 * (TAU * 2_000.0 * frame as f64 / rate).sin()).exp2(),
                    0.3,
                )
            },
            &input,
            512,
        );
        assert!(
            sidebands(&modulated) > 0.01 && sidebands(&steady) < 0.001,
            "{circuit:?}: {} against {}",
            sidebands(&modulated),
            sidebands(&steady)
        );
    }
}

// Spec: instrument filter R11 (a change of type, slope or circuit crossfades)
#[test]
fn a_change_of_type_slope_or_circuit_makes_no_click() {
    let rate = 48_000.0;
    let hz = 440.0;
    let before = sine(rate, hz, 0.5, 10_000);
    let after: Vec<f32> = (10_000..14_000)
        .map(|i| (0.5 * (TAU * hz * f64::from(i as u32) / rate).sin()) as f32)
        .collect();
    let steepest = |signal: &[f32]| {
        signal
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0f32, f32::max)
    };
    for (from, to) in [
        (
            setup(0, false, Circuit::Clean),
            setup(0, false, Circuit::Osr),
        ),
        (setup(0, false, Circuit::Osr), setup(0, true, Circuit::Prd)),
        (setup(0, true, Circuit::Ms2), setup(1, true, Circuit::Ms2)),
        (
            setup(0, false, Circuit::Clean),
            setup(2, false, Circuit::Clean),
        ),
    ] {
        let knobs = |_| Knobs::at(2_000.0, 0.2);
        let mut filter = InstrumentFilter::new(rate as f32, 1, 512);
        filter.set_setup(from);
        filter.reset_voice(0, 0);
        let mut out = run_filter(&mut filter, 0, knobs, &before, 512);
        filter.set_setup(to);
        out.extend(run_filter(&mut filter, 10_000, knobs, &after, 512));
        let steady = steepest(&out[8_000..10_000]);
        let across = steepest(&out[10_000 - 1..10_000 + 1_000]);
        assert!(
            across <= steady * 1.5,
            "{from:?} to {to:?}: a step of {across} against {steady}"
        );
    }
}

/// A deterministic stream of numbers, 0 to below 1.
fn lcg(state: &mut u64) -> f64 {
    *state = state
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    (*state >> 11) as f64 / (1u64 << 53) as f64
}

// Spec: instrument filter R12 (stable at every setting from 44.1 to 192 kHz)
#[test]
fn nothing_is_unbounded_at_any_setting_or_rate() {
    let mut state = 7u64;
    for case in 0..240 {
        let rate = [44_100.0, 48_000.0, 96_000.0, 192_000.0][case % 4];
        let s = Setup::new(
            (lcg(&mut state) * 5.0) as usize,
            lcg(&mut state) < 0.5,
            (lcg(&mut state) * 5.0) as usize,
        );
        let knobs = Knobs {
            hz: 20.0 * 1_200f64.powf(lcg(&mut state)),
            res: lcg(&mut state) as f32,
            morph: lcg(&mut state) as f32,
            drive: lcg(&mut state) as f32,
        };
        let mut input = noise(8_192, 1.0);
        input[100] = 1.0;
        input[101] = -1.0;
        let out = render(s, rate, knobs, &input, 256);
        assert!(
            out.iter().all(|x| x.is_finite() && x.abs() < 100.0),
            "{s:?} at {rate} with {:?}: peak {}",
            (knobs.hz, knobs.res, knobs.morph, knobs.drive),
            peak(&out)
        );
    }
    // A 20 kHz cutoff at 44.1 kHz and full resonance stays stable.
    for s in all_setups(&ALL_CIRCUITS) {
        let knobs = Knobs {
            drive: 1.0,
            ..Knobs::at(20_000.0, 1.0)
        };
        let out = render(s, 44_100.0, knobs, &noise(44_100, 1.0), 512);
        assert!(
            out.iter().all(|x| x.is_finite() && x.abs() < 100.0),
            "{s:?}"
        );
    }
}

// Spec: instrument filter R12 (denormals are flushed)
#[test]
fn a_long_silence_after_loud_input_is_exactly_silent() {
    let rate = 48_000.0;
    let mut input = noise(4_800, 1.0);
    input.resize(rate as usize * 3, 0.0);
    for s in all_setups(&[Circuit::Osr, Circuit::Ms2, Circuit::Smp, Circuit::Prd]) {
        let out = render(s, rate, Knobs::at(2_000.0, 0.5), &input, 512);
        assert!(
            out[rate as usize * 2..].iter().all(|x| *x == 0.0),
            "{s:?} holds a tail"
        );
    }
}

/// Whether the tests run at full depth: `BL_TEST_DEPTH=full`, as on pushes
/// to main, manual CI runs, and the nightly run. The device budget is judged
/// only then, so a loaded machine can't fail it.
fn full_depth() -> bool {
    match std::env::var("BL_TEST_DEPTH").as_deref() {
        Ok("full") => true,
        Ok("quick") | Err(_) => false,
        Ok(other) => {
            panic!("BL_TEST_DEPTH is `{other}`: set it to quick or full, or leave it unset")
        }
    }
}

// Spec: instrument filter R13 (sixteen voices through any circuit)
#[test]
fn sixteen_voices_render_faster_than_the_device_budget_with_no_allocation() {
    let mut registry = Registry::empty();
    bl_devices::sampler::register(&mut registry);
    let rate = common::RATE.hz() as usize;
    let noise = noise(rate, 0.5);
    let sample = Arc::new(bl_media::SampleData::new(common::RATE.hz(), vec![noise]).unwrap());
    for circuit in 0..5u8 {
        let device_type: DeviceType = sampler_type();
        let device = common::device(
            &device_type,
            &[
                (LOOP, 1.0),
                (VOICES, 16.0),
                (FILTER, 1.0),
                (FILTER_TYPE, 1.0),
                (FILTER_CIRCUIT, f64::from(circuit)),
                (FILTER_FREQ, 1_500.0),
                (FILTER_RES, 70.0),
                (FILTER_DRIVE, 12.0),
            ],
        );
        let mut node = common::node(&registry, &device, 128);
        let collector = Collector::new();
        node.receive_data(
            collector
                .handle()
                .data(SlotSample::new(0, Arc::clone(&sample))),
        );
        let chord: Vec<NoteEvent> = (0..16)
            .map(|i| NoteEvent {
                offset: 0,
                kind: NoteKind::On,
                channel: 0,
                key: 36 + i * 3,
                velocity: 0.8,
                note_id: None,
            })
            .collect();
        let seconds = 5;
        let mut data = vec![0.0f32; common::CHANNELS * 128];
        let mut out = NoteBuffer::with_capacity(16);
        let mut heard = 0.0f64;
        let start = Instant::now();
        for block in 0..seconds * rate / 128 {
            data.fill(0.0);
            let notes: &[NoteEvent] = if block == 0 { &chord } else { &[] };
            let mut b = Block::new(
                &mut data,
                common::CHANNELS,
                128,
                128,
                &[],
                TransportInfo::default(),
                common::RATE,
            )
            .with_notes(notes, &mut out);
            bl_rt::rt_test!({ node.process(&mut b) });
            heard += data.iter().map(|x| f64::from(x.abs())).sum::<f64>();
        }
        let elapsed = start.elapsed().as_secs_f64();
        let realtime = seconds as f64 / elapsed;
        println!(
            "{}: {realtime:.0}x real time (debug)",
            CIRCUITS[usize::from(circuit)]
        );
        assert!(heard > 1.0, "{} sounds", CIRCUITS[usize::from(circuit)]);
        if full_depth() {
            // A debug build on a loaded machine (the Linux CI runner, which
            // runs the SMP circuit at about 4x while the Mac runs it at
            // about 30x); 16 voices mustn't eat more than half the time
            // they play.
            assert!(
                realtime >= 2.0,
                "{}: {realtime:.1}x real time",
                CIRCUITS[usize::from(circuit)]
            );
        }
    }
}

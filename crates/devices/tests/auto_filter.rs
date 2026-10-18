//! Sweep, the native Auto Filter (WP-5.20; the auto-filter spec): the
//! filter types and slopes against the spec's curves, the circuits, Drive,
//! the LFO and envelope follower moving the cutoff, the sidechain, Clip,
//! Hi-Quality's latency, and renders that repeat at any block size. Every
//! block runs as a real-time section under the allocation trap.

bl_rt::install_trap!();

mod common;

use std::f64::consts::TAU;

use bl_core::time::{Bpm, Frames, TempoMap, TempoSegment, Ticks};
use bl_devices::auto_filter::{
    self, AUTO_FILTER, CIRCUIT, CLIP, CONTROL, DRIVE, DRY_WET, ENV_AMOUNT, ENV_ATTACK, ENV_HOLD,
    ENV_RELEASE, ENV_SH, FORMANT, FREQ, HI_QUALITY, LFO_AMOUNT, LFO_DIVISION, LFO_HZ, LFO_MODE,
    LFO_OFFSET, LFO_PHASE, LFO_QUANTIZE, LFO_SH, LFO_SPIN, LFO_STEPS, LFO_STEREO, LFO_WAVE,
    MONO_SIDECHAIN, MORPH, OUTPUT, PITCH, RES, SC_FILTER, SC_FREQ, SC_PEAK, SC_Q, SC_TYPE, SLOPE,
    TYPE, auto_filter_type, response_db,
};
use bl_engine::{Block, DeviceType, DisplayReader, Node, Registry, TransportInfo};
use bl_model::ParamId;
use common::{CHANNELS, RATE};
use realfft::RealFftPlanner;

const BLOCKS: [usize; 4] = [1, 64, 512, 4096];

fn registry() -> Registry {
    let mut registry = Registry::empty();
    auto_filter::register(&mut registry);
    registry
}

fn sweep(params: &[(ParamId, f64)], max_frames: usize) -> Box<dyn Node> {
    let device_type: DeviceType = auto_filter_type();
    common::node(
        &registry(),
        &common::device(&device_type, params),
        max_frames,
    )
}

/// The device's `values` as the response function reads them.
fn values(params: &[(ParamId, f64)]) -> bl_model::OrdMap<ParamId, f64> {
    common::device(&auto_filter_type(), params).params
}

/// `frames` of a sine at `hz`, `db` dBFS.
fn sine(hz: f64, db: f64, frames: usize) -> Vec<f32> {
    let amp = 10f64.powf(db / 20.0);
    (0..frames)
        .map(|n| (amp * (TAU * hz * n as f64 / 48_000.0).sin()) as f32)
        .collect()
}

/// Streams `input` through `node` in blocks of `block`, one channel's worth
/// on both, and returns the left channel.
fn stream(node: &mut dyn Node, input: &[f32], block: usize) -> Vec<f32> {
    let mut out = Vec::with_capacity(input.len());
    for chunk in input.chunks(block) {
        out.extend_from_slice(&common::run(node, chunk, &[])[0]);
    }
    out
}

/// The amplitude of the component of `signal` at `hz`, over whole cycles.
fn amplitude_at(signal: &[f32], hz: f64) -> f64 {
    let (mut re, mut im) = (0.0, 0.0);
    for (n, s) in signal.iter().enumerate() {
        let w = TAU * hz * n as f64 / 48_000.0;
        re += f64::from(*s) * w.cos();
        im += f64::from(*s) * w.sin();
    }
    2.0 * re.hypot(im) / signal.len() as f64
}

/// The gain in dB at `hz` of a device of `params` for a −40 dBFS sine,
/// measured over the last 4,800 frames (whole cycles at any multiple of
/// 10 Hz) after 9,600 of settling.
fn gain_db(params: &[(ParamId, f64)], hz: f64) -> f64 {
    let input = sine(hz, -40.0, 14_400);
    let mut node = sweep(params, 4800);
    let out = stream(node.as_mut(), &input, 4800);
    20.0 * (amplitude_at(&out[9_600..], hz) / amplitude_at(&input[9_600..], hz)).log10()
}

// Spec: auto-filter R1
#[test]
fn sweep_is_described_with_a_sidechain_and_two_menu_options() {
    let device_type = auto_filter_type();
    device_type.description.check().unwrap();
    assert_eq!(device_type.name, "Sweep");
    assert!(device_type.sidechain);
    assert!(matches!(
        device_type.kind,
        bl_model::DeviceKind::Native { ref device_type } if device_type == AUTO_FILTER
    ));
    assert_eq!(
        device_type.description.menu_options,
        [MONO_SIDECHAIN, HI_QUALITY]
    );
    let mono = device_type.description.param(MONO_SIDECHAIN).unwrap();
    assert_eq!(mono.default, 1.0, "Mono Sidechain is on in a new device");
    assert_eq!(
        device_type.description.param(HI_QUALITY).unwrap().default,
        0.0
    );
}

/// The low-pass, high-pass, band-pass and notch types at 12 and 24 dB, Notch
/// + LP, and Morph at all four slopes, as (type, slope, morph) cases.
fn cases() -> Vec<(f64, f64, f64)> {
    let mut cases = Vec::new();
    for kind in [0.0, 1.0, 2.0, 3.0, 4.0] {
        for slope in [1.0, 2.0] {
            cases.push((kind, slope, 50.0));
        }
    }
    for slope in [0.0, 1.0, 2.0, 3.0] {
        for morph in [0.0, 30.0, 50.0, 80.0, 100.0] {
            cases.push((5.0, slope, morph));
        }
    }
    cases
}

// Spec: auto-filter R2, R3
#[test]
fn each_types_response_matches_the_spec_at_three_cutoffs_and_three_resonances() {
    let mut compared = 0;
    for (kind, slope, morph) in cases() {
        for cutoff in [200.0, 1_000.0, 4_000.0] {
            for res in [0.0, 50.0, 90.0] {
                let params = [
                    (TYPE, kind),
                    (SLOPE, slope),
                    (FREQ, cutoff),
                    (RES, res),
                    (MORPH, morph),
                ];
                for octaves in [-2.0, -1.0, 0.0, 1.0, 2.0] {
                    let hz = (cutoff * 2f64.powf(octaves) / 10.0).round() * 10.0;
                    let measured = gain_db(&params, hz);
                    let curve = response_db(&values(&params), hz, 48_000.0);
                    if curve < -70.0 {
                        continue;
                    }
                    assert!(
                        (measured - curve).abs() < 0.5,
                        "type {kind} slope {slope} morph {morph} at {cutoff} Hz, res {res} %: \
                         {measured:.2} dB at {hz} Hz, the curve {curve:.2} dB"
                    );
                    compared += 1;
                }
            }
        }
    }
    assert!(compared > 900, "{compared} points compared");
}

/// A device's node with a reader on its display, run a block at a time at a
/// tempo, so a test reads what it did to the cutoff after each block.
struct Probe {
    node: Box<dyn Node>,
    reader: DisplayReader,
    base: f64,
    position: i64,
    tempo: TempoMap,
    playing: bool,
}

impl Probe {
    fn new(params: &[(ParamId, f64)], max_frames: usize, tempo: TempoMap, playing: bool) -> Self {
        let mut node = sweep(params, max_frames);
        let reader = node.display().expect("Sweep has a display");
        let base = auto_filter_type()
            .description
            .value(&values(params), FREQ)
            .unwrap();
        Self {
            node,
            reader,
            base,
            position: 0,
            tempo,
            playing,
        }
    }

    /// Runs `frames` frames of `left` and `right` (silence when `None`),
    /// with `key` as the sidechain (both channels) when given. Returns the
    /// octaves the left and the right cutoffs are from Freq after the block,
    /// and the clip light.
    fn run(
        &mut self,
        left: Option<&[f32]>,
        right: Option<&[f32]>,
        key: Option<(&[f32], &[f32])>,
        frames: usize,
    ) -> ([f64; 2], f32, Vec<Vec<f32>>) {
        let mut data = vec![0.0f32; CHANNELS * frames];
        if let Some(left) = left {
            data[..frames].copy_from_slice(left);
        }
        if let Some(right) = right {
            data[frames..].copy_from_slice(right);
        }
        let keyed: Option<Vec<f32>> = key.map(|(l, r)| l.iter().chain(r).copied().collect());
        let transport = TransportInfo {
            playing: self.playing,
            position: Frames(self.position),
        };
        {
            let mut block = Block::new(&mut data, CHANNELS, frames, frames, &[], transport, RATE)
                .with_tempo(&self.tempo);
            if let Some(keyed) = &keyed {
                block = block.with_sidechain(keyed);
            }
            bl_rt::rt_test!({ self.node.process(&mut block) });
        }
        self.position += frames as i64;
        let values = &self.reader.read().values;
        (
            [
                (f64::from(values[0]) / self.base).log2(),
                (f64::from(values[1]) / self.base).log2(),
            ],
            values[2],
            data.chunks(frames).map(<[f32]>::to_vec).collect(),
        )
    }

    /// The cutoffs in octaves after `frames` silent frames.
    fn skip(&mut self, frames: usize) -> [f64; 2] {
        self.run(None, None, None, frames).0
    }
}

fn steady() -> TempoMap {
    TempoMap::constant(Bpm::DEFAULT)
}

/// A magnitude response in dB at each FFT bin of 2^16 frames, from an
/// impulse of 0.01.
fn spectrum_db(params: &[(ParamId, f64)]) -> Vec<f64> {
    const N: usize = 1 << 16;
    let mut input = vec![0.0f32; N];
    input[0] = 0.01;
    let mut node = sweep(params, N);
    let out = common::run(node.as_mut(), &input, &[]).remove(0);
    let mut planner = RealFftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(N);
    let mut samples = out;
    let mut bins = fft.make_output_vec();
    fft.process(&mut samples, &mut bins).unwrap();
    bins.iter()
        .map(|b| 20.0 * (f64::from(b.norm()) / 0.01).max(1e-12).log10())
        .collect()
}

/// The bin of the highest point of `spectrum` within `tolerance` (a
/// fraction) of `hz`, as a frequency, and whether it is a peak inside that
/// window rather than at its edge.
fn peak_near(spectrum: &[f64], hz: f64, tolerance: f64) -> (f64, bool) {
    let bin = |f: f64| (f * 65_536.0 / 48_000.0).round() as usize;
    let (from, to) = (bin(hz * (1.0 - tolerance)), bin(hz * (1.0 + tolerance)));
    let top = (from..=to)
        .max_by(|a, b| spectrum[*a].total_cmp(&spectrum[*b]))
        .unwrap();
    (top as f64 * 48_000.0 / 65_536.0, top > from && top < to)
}

// Spec: auto-filter R3
#[test]
fn each_slope_falls_at_its_dbs_per_octave_within_a_decibel() {
    // Octaves apart, well past the cutoff: low-pass at 200 Hz, high-pass at
    // 4 kHz (measured below it).
    let drop = |kind: f64, slope: f64, cutoff: f64, near: f64, far: f64| {
        let params = [(TYPE, kind), (SLOPE, slope), (FREQ, cutoff), (MORPH, 0.0)];
        gain_db(&params, near) - gain_db(&params, far)
    };
    for (slope, per_octave) in [(1.0, 12.0), (2.0, 24.0)] {
        let low = drop(0.0, slope, 200.0, 800.0, 1_600.0);
        assert!(
            (low - per_octave).abs() < 1.0,
            "low-pass {per_octave}: {low}"
        );
        let high = drop(1.0, slope, 4_000.0, 1_000.0, 500.0);
        assert!(
            (high - per_octave).abs() < 1.0,
            "high-pass {per_octave}: {high}"
        );
    }
    for (slope, per_octave) in [(0.0, 6.0), (1.0, 12.0), (2.0, 24.0), (3.0, 48.0)] {
        // Morph 0 % is the low-pass, 100 % the high-pass.
        let low = drop(5.0, slope, 200.0, 800.0, 1_600.0);
        assert!((low - per_octave).abs() < 1.0, "morph {per_octave}: {low}");
        let params = [(TYPE, 5.0), (SLOPE, slope), (FREQ, 4_000.0), (MORPH, 100.0)];
        let high = gain_db(&params, 1_000.0) - gain_db(&params, 500.0);
        assert!(
            (high - per_octave).abs() < 1.0,
            "morph high {per_octave}: {high}"
        );
    }
}

// Spec: auto-filter R4
#[test]
fn dj_cuts_the_lows_above_zero_and_the_highs_below_it() {
    let at = |control: f64, hz: f64| gain_db(&[(TYPE, 6.0), (CONTROL, control)], hz);
    assert!(at(50.0, 100.0) < -15.0, "lows cut: {}", at(50.0, 100.0));
    assert!(at(50.0, 8_000.0).abs() < 1.0, "highs pass");
    assert!(
        at(-50.0, 8_000.0) < -15.0,
        "highs cut: {}",
        at(-50.0, 8_000.0)
    );
    assert!(at(-50.0, 100.0).abs() < 1.0, "lows pass");
    assert!(at(0.0, 100.0).abs() < 0.5 && at(0.0, 8_000.0).abs() < 0.5);
    // The curve says so too.
    let curve = |control: f64, hz: f64| {
        response_db(&values(&[(TYPE, 6.0), (CONTROL, control)]), hz, 48_000.0)
    };
    assert!((curve(50.0, 100.0) - at(50.0, 100.0)).abs() < 0.5);
}

// Spec: auto-filter R4
#[test]
fn the_combs_peaks_fall_on_multiples_of_the_cutoff_and_morph_turns_them_to_notches() {
    let peaks = spectrum_db(&[(TYPE, 7.0), (FREQ, 2_000.0), (RES, 80.0), (MORPH, 0.0)]);
    let notches = spectrum_db(&[(TYPE, 7.0), (FREQ, 2_000.0), (RES, 80.0), (MORPH, 100.0)]);
    for k in 1..=4 {
        let at = 2_000.0 * f64::from(k);
        let (found, inside) = peak_near(&peaks, at, 0.1);
        assert!(
            inside && (found - at).abs() < at * 0.01,
            "peak {k}: {found}"
        );
        // Midway between two multiples the response is far lower.
        let between = (at + 1_000.0) * 65_536.0 / 48_000.0;
        assert!(peaks[between as usize] < peaks[(at * 65_536.0 / 48_000.0) as usize] - 10.0);
        // And the notches sit where the peaks were.
        let bin = (at * 65_536.0 / 48_000.0).round() as usize;
        assert!(notches[bin] < notches[between as usize] - 10.0, "notch {k}");
    }
}

// Spec: auto-filter R4
#[test]
fn vowels_formant_peaks_fall_on_the_specs_table() {
    // The table's first two formants for a, e, i, o and u; Formant 0 to 100
    // % in quarters, then Pitch moving them an octave.
    let table: [[f64; 2]; 5] = [
        [730.0, 1_090.0],
        [530.0, 1_840.0],
        [270.0, 2_290.0],
        [570.0, 840.0],
        [300.0, 870.0],
    ];
    for (vowel, formants) in table.iter().enumerate() {
        let spectrum = spectrum_db(&[(TYPE, 9.0), (FORMANT, vowel as f64 * 25.0)]);
        for hz in formants {
            let (found, inside) = peak_near(&spectrum, *hz, 0.1);
            assert!(
                inside && (found - hz).abs() < hz * 0.03,
                "vowel {vowel}: {found} for {hz}"
            );
        }
    }
    let up = spectrum_db(&[(TYPE, 9.0), (FORMANT, 0.0), (PITCH, 12.0)]);
    let (found, inside) = peak_near(&up, 1_460.0, 0.1);
    assert!(
        inside && (found - 1_460.0).abs() < 1_460.0 * 0.03,
        "{found}"
    );
}

// Spec: auto-filter R4
#[test]
fn resampling_holds_the_signal_so_a_high_tone_aliases_and_a_low_one_passes() {
    let params = [(TYPE, 8.0), (FREQ, 4_800.0)];
    assert!(gain_db(&params, 240.0).abs() < 0.5, "a low tone passes");
    // 12 kHz held at 4.8 kHz comes out as a 2.4 kHz alias, louder than the
    // tone itself.
    let input = sine(12_000.0, -12.0, 14_400);
    let mut node = sweep(&params, 4800);
    let out = stream(node.as_mut(), &input, 4800);
    let alias = amplitude_at(&out[9_600..], 2_400.0);
    let tone = amplitude_at(&out[9_600..], 12_000.0);
    assert!(alias > tone * 3.0, "alias {alias}, tone {tone}");
}

// Spec: auto-filter R5
#[test]
fn drive_adds_harmonics_from_none_at_zero() {
    // A wide-open low-pass leaves a 1 kHz sine alone, so its 3 kHz is what
    // Drive makes.
    let third = |drive: f64| {
        let params = [(TYPE, 0.0), (FREQ, 20_000.0), (DRIVE, drive)];
        let input = sine(1_000.0, -12.0, 14_400);
        let mut node = sweep(&params, 4800);
        let out = stream(node.as_mut(), &input, 4800);
        amplitude_at(&out[9_600..], 3_000.0) / amplitude_at(&out[9_600..], 1_000.0)
    };
    let (none, a, b, c) = (third(0.0), third(10.0), third(30.0), third(100.0));
    assert!(none < 1e-4, "no drive, no harmonic: {none}");
    assert!(none < a && a < b && b < c, "{none} {a} {b} {c}");
}

// Spec: auto-filter R3, R5
#[test]
fn every_circuit_follows_the_same_small_signal_curve_on_the_types_it_acts_on() {
    for kind in [0.0, 1.0, 2.0, 3.0, 5.0] {
        for slope in [1.0, 2.0] {
            for circuit in [0.0, 1.0, 2.0, 3.0] {
                for res in [0.0, 90.0] {
                    let params = [
                        (TYPE, kind),
                        (SLOPE, slope),
                        (CIRCUIT, circuit),
                        (FREQ, 1_000.0),
                        (RES, res),
                        (MORPH, 50.0),
                    ];
                    for hz in [250.0, 500.0, 1_000.0, 2_000.0, 4_000.0] {
                        let measured = gain_db(&params, hz);
                        let curve = response_db(&values(&params), hz, 48_000.0);
                        if curve < -70.0 {
                            continue;
                        }
                        assert!(
                            (measured - curve).abs() < 0.5,
                            "type {kind} slope {slope} circuit {circuit} res {res} at {hz} Hz: \
                             {measured:.2} dB, the curve {curve:.2} dB"
                        );
                    }
                }
            }
        }
    }
}

// Spec: auto-filter R5
#[test]
fn the_circuits_agree_on_a_small_signal_and_part_on_a_large_one() {
    // At −40 dBFS they follow one curve (the response test holds each to
    // it); at 0 dBFS with the resonance up, the circuits' saturation tells
    // them apart.
    let loud = |circuit: f64| {
        let params = [
            (TYPE, 0.0),
            (FREQ, 1_000.0),
            (RES, 90.0),
            (CIRCUIT, circuit),
        ];
        let input = sine(1_000.0, -6.0, 14_400);
        let mut node = sweep(&params, 4800);
        let out = stream(node.as_mut(), &input, 4800);
        amplitude_at(&out[9_600..], 1_000.0)
    };
    let (svf, dfm, ms2, prd) = (loud(0.0), loud(1.0), loud(2.0), loud(3.0));
    assert!(dfm < svf * 0.9 && ms2 < svf * 0.9, "{svf} {dfm} {ms2}");
    assert!(prd < svf, "{svf} {prd}");
    assert!(
        [dfm, ms2, prd]
            .windows(2)
            .any(|p| (p[0] - p[1]).abs() > 0.01),
        "{dfm} {ms2} {prd}"
    );
}

// Spec: auto-filter R7
#[test]
fn a_synced_lfo_cycle_starts_on_the_beat_at_two_tempos_and_across_a_tempo_change() {
    let constant = |bpm| TempoMap::constant(Bpm::new(bpm).unwrap());
    let changing = TempoMap::new([
        TempoSegment::Constant {
            start: Ticks::ZERO,
            bpm: Bpm::new(120.0).unwrap(),
        },
        TempoSegment::Constant {
            start: Ticks::from_quarters(4),
            bpm: Bpm::new(90.0).unwrap(),
        },
    ])
    .unwrap();
    // Ramp Up from its bottom, a quarter-note cycle, ±4 octaves.
    let params = [
        (LFO_WAVE, 4.0),
        (LFO_MODE, 2.0),
        (LFO_DIVISION, 8.0),
        (LFO_AMOUNT, 100.0),
    ];
    for tempo in [constant(120.0), constant(75.0), changing] {
        let mut probe = Probe::new(&params, 4096, tempo.clone(), true);
        for beat in 1..8 {
            let at = tempo.ticks_to_frames(Ticks::from_quarters(beat), RATE).0;
            let half = tempo
                .ticks_to_frames(Ticks(Ticks::from_quarters(beat).0 + 352_800), RATE)
                .0;
            // A block that ends on the beat's frame reads the LFO there.
            let to_beat = (at - probe.position) as usize;
            if to_beat > 0 {
                let before = probe.skip(to_beat.min(4096));
                if to_beat > 4096 {
                    continue;
                }
                let phase = ((before[0] / 4.0) + 1.0) / 2.0;
                assert!(
                    phase.min(1.0 - phase) < 0.001,
                    "beat {beat}: the cycle is at {phase}"
                );
            }
            let to_half = (half - probe.position) as usize;
            if (1..=4096).contains(&to_half) {
                let mid = probe.skip(to_half);
                let phase = ((mid[0] / 4.0) + 1.0) / 2.0;
                assert!((phase - 0.5).abs() < 0.001, "half a beat on: {phase}");
            }
        }
    }
}

// Spec: auto-filter R7
#[test]
fn phase_puts_the_channels_in_antiphase_spin_detunes_and_steps_quantise() {
    // Sine at 5 Hz, ±4 octaves; Phase 180° makes the cutoffs mirror images.
    let mut probe = Probe::new(
        &[(LFO_AMOUNT, 100.0), (LFO_HZ, 5.0), (LFO_PHASE, 180.0)],
        64,
        steady(),
        false,
    );
    for _ in 0..400 {
        let [l, r] = probe.skip(64);
        assert!((l + r).abs() < 1e-3, "{l} {r}");
    }
    // Spin 20 %: the right runs at 6 Hz against the left's 5.
    let mut probe = Probe::new(
        &[
            (LFO_AMOUNT, 100.0),
            (LFO_HZ, 5.0),
            (LFO_STEREO, 1.0),
            (auto_filter::LFO_SPIN, 20.0),
        ],
        16,
        steady(),
        false,
    );
    let mut crossings: [Vec<f64>; 2] = [Vec::new(), Vec::new()];
    let mut last = [0.0f64; 2];
    for n in 0..(4 * 48_000 / 16) {
        let now = probe.skip(16);
        for ch in 0..2 {
            if last[ch] < 0.0 && now[ch] >= 0.0 {
                let t = f64::from(n) * 16.0 - 16.0 * now[ch] / (now[ch] - last[ch]);
                crossings[ch].push(t);
            }
            last[ch] = now[ch];
        }
    }
    let period = |c: &[f64]| (c[c.len() - 1] - c[0]) / (c.len() - 1) as f64;
    let ratio = period(&crossings[0]) / period(&crossings[1]);
    assert!((ratio - 1.2).abs() < 1.2 * 0.001, "{ratio}");
    // Steps 4 on a ramp: four levels to a cycle.
    let mut probe = Probe::new(
        &[
            (LFO_AMOUNT, 100.0),
            (LFO_WAVE, 4.0),
            (LFO_HZ, 10.0),
            (LFO_QUANTIZE, 1.0),
            (LFO_STEPS, 4.0),
        ],
        16,
        steady(),
        false,
    );
    let mut levels: Vec<i64> = (0..3_000)
        .map(|_| (probe.skip(16)[0] * 20.0).round() as i64)
        .collect();
    levels.sort_unstable();
    levels.dedup();
    assert_eq!(levels.len(), 4, "{levels:?}");
}

// Spec: auto-filter R7
#[test]
fn s_and_h_holds_the_lfo_on_a_beat_time_interval() {
    // A ramp at 1.5 Hz sampled once a beat (0.5 s at 120 BPM): the held
    // values step through the ramp at 0, 3/4, 1/2 and 1/4 of a cycle.
    let mut probe = Probe::new(
        &[
            (LFO_AMOUNT, 100.0),
            (LFO_WAVE, 4.0),
            (LFO_HZ, 1.5),
            (LFO_QUANTIZE, 2.0),
            (LFO_SH, 8.0),
        ],
        64,
        steady(),
        false,
    );
    let mut held: Vec<i64> = (0..(19 * 48_000 / 10 / 64))
        .map(|_| (probe.skip(64)[0] * 4.0).round() as i64)
        .collect();
    held.dedup();
    assert_eq!(held, [-16, 8, 0, -8], "octaves ×4 of the held values");
}

// Spec: auto-filter R8
#[test]
fn the_envelope_followers_attack_and_release_are_time_constants_and_hold_finishes_the_attack() {
    // 0.5 held for a second: the cutoff rises by 8 octaves × 0.5 = 4.
    let mut probe = Probe::new(
        &[
            (ENV_AMOUNT, 100.0),
            (ENV_ATTACK, 20.0),
            (ENV_RELEASE, 100.0),
        ],
        1,
        steady(),
        false,
    );
    let level = [0.5f32];
    let to_octaves =
        |probe: &mut Probe, input: Option<&[f32]>| probe.run(input, input, None, 1).0[0];
    let rise = (1..48_000)
        .find(|_| to_octaves(&mut probe, Some(&level)) >= 4.0 * (1.0 - (-1.0f64).exp()))
        .unwrap();
    assert!(
        (rise as f64 / 48_000.0 - 0.020).abs() < 0.020 * 0.05,
        "attack {rise}"
    );
    for _ in 0..48_000 {
        to_octaves(&mut probe, Some(&level));
    }
    let fall = (1..48_000)
        .find(|_| to_octaves(&mut probe, None) <= 4.0 * (-1.0f64).exp())
        .unwrap();
    assert!(
        (fall as f64 / 48_000.0 - 0.100).abs() < 0.100 * 0.05,
        "release {fall}"
    );

    // A pulse a hundredth of the attack long: Hold lets the attack finish.
    let peak = |hold: f64| {
        let mut probe = Probe::new(
            &[
                (ENV_AMOUNT, 100.0),
                (ENV_ATTACK, 50.0),
                (ENV_RELEASE, 50.0),
                (ENV_HOLD, hold),
            ],
            1,
            steady(),
            false,
        );
        let one = [1.0f32];
        (0..12_000)
            .map(|n| {
                let input = (n < 24).then_some(&one[..]);
                probe.run(input, input, None, 1).0[0] / 8.0
            })
            .fold(0.0f64, f64::max)
    };
    assert!(peak(0.0) < 0.02, "without Hold the release cuts it short");
    assert!(peak(1.0) > 0.9, "with Hold the attack reaches the pulse");
}

// Spec: auto-filter R9
#[test]
fn a_key_from_another_track_moves_the_cutoff_and_mono_sidechain_gives_both_channels_one() {
    let params = [(ENV_AMOUNT, 100.0), (ENV_ATTACK, 1.0), (ENV_RELEASE, 50.0)];
    let steady_input = vec![0.01f32; 4_800];
    let loud = vec![0.5f32; 4_800];
    let silent = vec![0.0f32; 4_800];
    let octaves = |params: &[(ParamId, f64)], left: &[f32], right: &[f32]| {
        let mut probe = Probe::new(params, 4_800, steady(), false);
        probe
            .run(
                Some(&steady_input),
                Some(&steady_input),
                Some((left, right)),
                4_800,
            )
            .0
    };
    // The device's own input is the same throughout; the key decides.
    let quiet = octaves(&params, &silent, &silent);
    let keyed = octaves(&params, &loud, &loud);
    assert!(quiet[0].abs() < 0.2, "{quiet:?}");
    assert!(keyed[0] > 3.0, "a loud key opens the cutoff: {keyed:?}");

    // A key on the left alone: Mono Sidechain (on in a new device) moves
    // both channels alike, to the mean; with it off only the left moves.
    let mono = octaves(&params, &loud, &silent);
    assert!(
        (mono[0] - mono[1]).abs() < 1e-9 && mono[0] > 1.0,
        "{mono:?}"
    );
    let mut stereo_params = params.to_vec();
    stereo_params.push((MONO_SIDECHAIN, 0.0));
    let stereo = octaves(&stereo_params, &loud, &silent);
    assert!(stereo[0] > 3.0 && stereo[1].abs() < 0.2, "{stereo:?}");
}

// Spec: auto-filter R9
#[test]
fn the_sc_filter_hears_only_its_band() {
    let params = [
        (ENV_AMOUNT, 100.0),
        (ENV_ATTACK, 1.0),
        (ENV_RELEASE, 50.0),
        (SC_FILTER, 1.0),
        (SC_TYPE, 4.0),
        (SC_FREQ, 5_000.0),
        (SC_Q, 4.0),
    ];
    let octaves = |key_hz: f64| {
        let key = sine(key_hz, -6.0, 9_600);
        let own = vec![0.0f32; 9_600];
        let mut probe = Probe::new(&params, 9_600, steady(), false);
        probe
            .run(Some(&own), Some(&own), Some((&key, &key)), 9_600)
            .0[0]
    };
    let (inside, outside) = (octaves(5_000.0), octaves(100.0));
    assert!(inside > 2.0, "in the band: {inside}");
    assert!(outside < 0.2, "out of it: {outside}");
    // A peak of +12 dB at the key's frequency raises what the follower
    // hears fourfold.
    let mut peaked = params.to_vec();
    peaked.extend([(SC_TYPE, 1.0), (SC_PEAK, 12.0)]);
    let key = sine(5_000.0, -30.0, 9_600);
    let own = vec![0.0f32; 9_600];
    let plain = {
        let mut probe = Probe::new(
            &[(ENV_AMOUNT, 100.0), (ENV_ATTACK, 1.0), (ENV_RELEASE, 50.0)],
            9_600,
            steady(),
            false,
        );
        probe
            .run(Some(&own), Some(&own), Some((&key, &key)), 9_600)
            .0[0]
    };
    let boosted = {
        let mut probe = Probe::new(&peaked, 9_600, steady(), false);
        probe
            .run(Some(&own), Some(&own), Some((&key, &key)), 9_600)
            .0[0]
    };
    assert!(boosted > plain * 3.0, "{plain} {boosted}");
}

// Spec: auto-filter R10
#[test]
fn clip_holds_the_peak_at_one_and_its_light_shows_it() {
    let loud = sine(1_000.0, -3.0, 4_800);
    let gain = 10f64.powf(12.0 / 20.0);
    let run = |clip: f64, input: &[f32]| {
        let mut probe = Probe::new(
            &[(TYPE, 0.0), (FREQ, 20_000.0), (OUTPUT, gain), (CLIP, clip)],
            4_800,
            steady(),
            false,
        );
        let (_, light, data) = probe.run(Some(input), Some(input), None, 4_800);
        let peak = data[0].iter().fold(0.0f32, |m, s| m.max(s.abs()));
        (peak, light)
    };
    let (open, light_open) = run(0.0, &loud);
    let (held, light) = run(1.0, &loud);
    assert!(open > 2.0 && light_open == 0.0, "{open}");
    assert!(held <= 1.0 && held > 0.95 && light == 1.0, "{held}");
    // A quiet signal passes untouched and the light stays dark.
    let quiet = sine(1_000.0, -20.0, 4_800);
    let (peak, light) = run(1.0, &quiet);
    assert!((peak - 0.1 * 3.98).abs() < 0.01 && light == 0.0, "{peak}");
}

// Spec: auto-filter R10
#[test]
fn dry_wet_zero_passes_the_dry_signal_and_output_reaches_silence() {
    let input = common::noise(4_800);
    let mut node = sweep(&[(TYPE, 0.0), (FREQ, 200.0), (DRY_WET, 0.0)], 4_800);
    assert_eq!(common::run(node.as_mut(), &input, &[])[0], input);
    let mut node = sweep(&[(OUTPUT, 0.0)], 4_800);
    assert!(
        common::run(node.as_mut(), &input, &[])[0]
            .iter()
            .all(|s| *s == 0.0)
    );
}

// Spec: auto-filter R11
#[test]
fn hi_quality_reports_the_oversamplers_latency_and_the_plain_path_none() {
    let latency = bl_dsp::Oversampler::new(1, 1, 1).latency();
    assert!(latency > 0);
    assert_eq!(sweep(&[], 64).latency(), 0);
    assert_eq!(sweep(&[(HI_QUALITY, 1.0)], 64).latency(), latency);
    // Turned on during playback, the latency follows with the next block.
    let mut node = sweep(&[], 64);
    common::run(
        node.as_mut(),
        &[0.0; 64],
        &[common::set(0, HI_QUALITY, 1.0)],
    );
    assert_eq!(node.latency(), latency);
    // An impulse comes out that many frames later, at either wet setting.
    for wet in [100.0, 0.0] {
        let mut input = vec![0.0f32; 512];
        input[0] = 1.0;
        let mut node = sweep(&[(TYPE, 6.0), (HI_QUALITY, 1.0), (DRY_WET, wet)], 512);
        let out = common::run(node.as_mut(), &input, &[]).remove(0);
        let top = (0..512)
            .max_by(|a, b| out[*a].abs().total_cmp(&out[*b].abs()))
            .unwrap();
        assert_eq!(top, latency as usize, "wet {wet}");
    }
}

/// The same input through the device at each block size, from a song that
/// plays at 100 BPM.
fn render(params: &[(ParamId, f64)], block: usize) -> Vec<Vec<f32>> {
    let input = common::noise(8_192);
    let tempo = TempoMap::constant(Bpm::new(100.0).unwrap());
    let mut node = sweep(params, block);
    let mut out = vec![Vec::new(); CHANNELS];
    for (n, chunk) in input.chunks(block).enumerate() {
        let channels = vec![chunk.to_vec(); CHANNELS];
        let transport = TransportInfo {
            playing: true,
            position: Frames((n * block) as i64),
        };
        let done = common::run_at(node.as_mut(), &channels, &[], &tempo, transport);
        for (all, part) in out.iter_mut().zip(done) {
            all.extend(part);
        }
    }
    out
}

// Spec: auto-filter R14
#[test]
fn renders_are_bit_identical_at_block_sizes_1_64_512_and_4096_and_run_after_run() {
    let sets: [&[(ParamId, f64)]; 3] = [
        &[
            (TYPE, 5.0),
            (SLOPE, 2.0),
            (CIRCUIT, 1.0),
            (FREQ, 800.0),
            (RES, 60.0),
            (MORPH, 40.0),
            (DRIVE, 30.0),
            (CLIP, 1.0),
            (LFO_AMOUNT, 80.0),
            (LFO_WAVE, 6.0),
            (LFO_HZ, 3.0),
            (ENV_AMOUNT, 40.0),
            (HI_QUALITY, 1.0),
            (MONO_SIDECHAIN, 0.0),
        ],
        &[
            (TYPE, 7.0),
            (RES, 70.0),
            (LFO_AMOUNT, 60.0),
            (LFO_WAVE, 7.0),
            (LFO_MODE, 2.0),
            (LFO_DIVISION, 5.0),
            (LFO_QUANTIZE, 2.0),
            (LFO_OFFSET, 45.0),
            (LFO_STEREO, 1.0),
            (LFO_SPIN, 10.0),
            (ENV_AMOUNT, -50.0),
            (ENV_SH, 1.0),
        ],
        &[
            (TYPE, 9.0),
            (FORMANT, 40.0),
            (PITCH, 3.0),
            (LFO_AMOUNT, 50.0),
            (LFO_WAVE, 2.0),
            (LFO_QUANTIZE, 1.0),
            (LFO_STEPS, 6.0),
            (HI_QUALITY, 1.0),
        ],
    ];
    for params in sets {
        let reference = render(params, 4096);
        assert_eq!(render(params, 4096), reference, "run after run");
        for block in BLOCKS {
            assert_eq!(render(params, block), reference, "at block size {block}");
        }
    }
}

//! Spectral Blur, the frequency range smeared into a cloud (WP-8.49; the
//! Spectral Blur spec): the range and the latency, the decay Halo sets,
//! Residual, Freeze, Wet Gain and Dry/Wet, a render that doesn't depend on
//! the block size, and what the display reads. Every block runs as a
//! real-time section under the allocation trap.

use crate::common;
use bl_core::id::Id;
use bl_core::time::{Bpm, TempoMap};
use bl_devices::spectral_blur::{
    DISPLAY_LEN, DRY_WET, FREEZE, HALO, HIGH, LOW, RESIDUAL, SPECTRAL_BLUR, WET_GAIN,
    spectral_blur_type,
};
use bl_engine::{Node, Registry};
use bl_model::{DeviceId, DeviceKind, OrdMap, ParamId};
use common::{node, run_blocks, set};
use std::f64::consts::TAU;

const RATE: f64 = 48_000.0;
/// The frames the output lags the input by: the 2,048-frame window less one.
const LATENCY: usize = 2_047;

fn registry() -> Registry {
    let mut registry = Registry::new();
    bl_devices::register(&mut registry);
    registry
}

fn blur_with(id: u64, params: &[(ParamId, f64)]) -> Box<dyn Node> {
    let mut device = spectral_blur_type().device(DeviceId(Id::new(id).unwrap()));
    for (param, value) in params {
        device.params.insert(*param, *value);
    }
    node(&registry(), &device, 4_096)
}

fn blur(params: &[(ParamId, f64)]) -> Box<dyn Node> {
    blur_with(1, params)
}

/// `input` through `node` in blocks of `block`, left channel.
fn render_blocks(
    node: &mut dyn Node,
    input: &[f32],
    block: usize,
    events: &[bl_engine::NodeEvent],
) -> Vec<f32> {
    let tempo = TempoMap::constant(Bpm::new(120.0).unwrap());
    run_blocks(
        node,
        &[input.to_vec(), input.to_vec()],
        block,
        events,
        &tempo,
    )
    .swap_remove(0)
}

fn render(params: &[(ParamId, f64)], input: &[f32]) -> Vec<f32> {
    render_blocks(blur(params).as_mut(), input, 4_096, &[])
}

/// A sum of sines, `(hz, amplitude)`, for `frames` frames.
fn tones(parts: &[(f64, f64)], frames: usize) -> Vec<f32> {
    (0..frames)
        .map(|n| {
            parts
                .iter()
                .map(|(hz, a)| a * (TAU * hz * n as f64 / RATE).sin())
                .sum::<f64>() as f32
        })
        .collect()
}

/// The amplitude of the sine at `hz` in `signal` (a Goertzel sum).
fn amplitude(signal: &[f32], hz: f64) -> f64 {
    let (mut re, mut im) = (0.0, 0.0);
    for (n, s) in signal.iter().enumerate() {
        let phase = TAU * hz * n as f64 / RATE;
        re += f64::from(*s) * phase.cos();
        im += f64::from(*s) * phase.sin();
    }
    2.0 * re.hypot(im) / signal.len() as f64
}

/// `n` seconds of frames.
fn seconds(n: usize) -> usize {
    n * 48_000
}

fn db(ratio: f64) -> f64 {
    20.0 * ratio.log10()
}

/// The mean square of `signal`.
fn power(signal: &[f32]) -> f64 {
    signal.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / signal.len() as f64
}

// Spec: spectral-blur R1, R2
#[test]
fn it_is_an_audio_effect_named_for_live_with_one_display_cell_and_reports_its_latency() {
    let t = spectral_blur_type();
    assert_eq!(t.name, "Spectral Blur");
    assert_eq!(t.role, bl_model::DeviceRole::AudioEffect);
    assert!(matches!(&t.kind, DeviceKind::Native { device_type } if device_type == SPECTRAL_BLUR));
    let layout = &t.description.layout;
    assert_eq!(layout.place(layout.docked_size()).cells.len(), 1);
    let param = |id| t.description.param(id).unwrap();
    for (id, range, default) in [
        (LOW, (20.0, 20_000.0), 200.0),
        (HIGH, (20.0, 20_000.0), 5_000.0),
        (HALO, (20.0, 10_000.0), 1_000.0),
        (RESIDUAL, (0.0, 100.0), 0.0),
        (DRY_WET, (0.0, 100.0), 100.0),
    ] {
        assert_eq!((param(id).min(), param(id).max()), range);
        assert_eq!(param(id).default, default);
    }
    assert_eq!(param(FREEZE).default, 0.0);
    assert!((db(param(WET_GAIN).max()) - 12.0).abs() < 1e-6);
    assert_eq!(blur(&[]).latency() as usize, LATENCY);
}

// Spec: spectral-blur R2
#[test]
fn what_lies_outside_the_range_passes_unchanged_after_the_latency() {
    let frames = 48_000;
    // Both tones carry the same level; only the 1 kHz one is in the range.
    let input = tones(&[(1_000.0, 0.4), (10_000.0, 0.4)], frames);
    let out = render(&[], &input);
    let steady = 4 * LATENCY..frames;
    let passed = amplitude(&out[steady.clone()], 10_000.0);
    assert!(db(passed / 0.4).abs() < 0.1, "the tone above: {passed}");
    // With Dry/Wet at half the result and the delayed input add in step. A
    // short Halo lets the tone's onset, which reaches the range, die away.
    let only_outside = tones(&[(10_000.0, 0.4)], frames);
    let half = render(&[(DRY_WET, 50.0), (HALO, 20.0)], &only_outside);
    for n in 4 * LATENCY..frames {
        let want = only_outside[n - LATENCY];
        assert!((half[n] - want).abs() < 1e-3, "frame {n}");
    }
}

// Spec: spectral-blur R2
#[test]
fn high_is_kept_at_or_above_low() {
    // Low above High: the range is Low's one bin, not nothing. With the
    // cloud and Residual off, a tone on that bin loses its centre.
    let input = tones(&[(3_000.0, 0.4)], 24_000);
    let out = render(&[(LOW, 3_000.0), (HIGH, 500.0), (WET_GAIN, 0.0)], &input);
    let left = amplitude(&out[4 * LATENCY..], 3_000.0);
    assert!(db(left / 0.4) < -3.0, "{left}");
}

/// The 60 dB decay time of the energy of `signal` after `from`, in
/// seconds, from the slope of its level in 50 ms windows between 3 and
/// 45 dB below where it began.
fn decay_time(signal: &[f32], from: usize) -> f64 {
    let window = 2_400;
    let levels: Vec<f64> = signal[from..]
        .chunks_exact(window)
        .map(|c| 10.0 * power(c).max(1e-30).log10())
        .collect();
    let top = levels[0];
    let points: Vec<(f64, f64)> = levels
        .iter()
        .enumerate()
        .filter(|(_, l)| (top - 45.0..=top - 3.0).contains(l))
        .map(|(n, l)| (n as f64 * window as f64 / RATE, *l))
        .collect();
    assert!(points.len() >= 5, "{levels:?}");
    let n = points.len() as f64;
    let (mx, my) = (
        points.iter().map(|p| p.0).sum::<f64>() / n,
        points.iter().map(|p| p.1).sum::<f64>() / n,
    );
    let slope = points.iter().map(|p| (p.0 - mx) * (p.1 - my)).sum::<f64>()
        / points.iter().map(|p| (p.0 - mx).powi(2)).sum::<f64>();
    -60.0 / slope
}

// Spec: spectral-blur R3
#[test]
fn a_tone_burst_in_the_range_decays_over_halos_time_within_ten_per_cent() {
    // Tones on every bin from 300 Hz to 4 kHz make a burst the cloud's
    // level doesn't fluctuate on.
    let burst = 24_000;
    let parts: Vec<(f64, f64)> = (7..86)
        .map(|k| (k as f64 * RATE / 2_048.0, 0.005))
        .collect();
    let mut input = tones(&parts, burst);
    input.resize(burst + 6 * 48_000, 0.0);
    for halo in [500.0, 1_000.0, 2_000.0] {
        let out = render(&[(HALO, halo)], &input);
        let time = decay_time(&out, burst + LATENCY + 2_048);
        assert!(
            (time / (halo / 1_000.0) - 1.0).abs() < 0.1,
            "Halo {halo} ms decayed in {time} s"
        );
    }
    // The cloud is about as loud as the burst was.
    let out = render(&[], &input);
    let during = power(&out[LATENCY + 2_048..burst + LATENCY]);
    let burst_power = power(&input[..burst]);
    assert!(db((during / burst_power).sqrt()).abs() < 1.5);
}

// Spec: spectral-blur R4, R6
#[test]
fn residual_keeps_the_ranges_own_signal_and_wet_gain_at_minus_infinity_leaves_only_it() {
    let frames = 48_000;
    let input = tones(&[(1_000.0, 0.4), (10_000.0, 0.4)], frames);
    let steady = 4 * LATENCY..frames;
    let at =
        |params: &[(ParamId, f64)], hz: f64| amplitude(&render(params, &input)[steady.clone()], hz);
    // Residual 100 % with the cloud off: the range's signal passes as it came.
    let kept = at(&[(RESIDUAL, 100.0), (WET_GAIN, 0.0)], 1_000.0);
    assert!(db(kept / 0.4).abs() < 0.1, "{kept}");
    // Residual 0 % with the cloud off: the range is silent, the rest isn't.
    let silent = at(&[(WET_GAIN, 0.0)], 1_000.0);
    assert!(db(silent / 0.4) < -60.0, "{silent}");
    let above = at(&[(WET_GAIN, 0.0)], 10_000.0);
    assert!(db(above / 0.4).abs() < 0.1, "{above}");
    // Half the Residual keeps half the amplitude.
    let half = at(&[(RESIDUAL, 50.0), (WET_GAIN, 0.0)], 1_000.0);
    assert!(db(half / 0.2).abs() < 0.1, "{half}");
}

// Spec: spectral-blur R6
#[test]
fn wet_gain_sets_the_clouds_level_and_dry_wet_balances_it_against_the_dry_signal() {
    let input = tones(&[(1_000.0, 0.4)], 96_000);
    let level = |params: &[(ParamId, f64)]| {
        // Gain 1.0 is 0 dB: the parameter holds the linear gain.
        power(&render(params, &input)[4 * LATENCY..])
    };
    let unity = level(&[]);
    // 6.02 dB is a gain of 2.
    let louder = level(&[(WET_GAIN, 2.0)]);
    assert!((10.0 * (louder / unity).log10() - 6.02).abs() < 0.1);
    // Fully dry, the delayed input alone.
    let dry = render(&[(DRY_WET, 0.0)], &input);
    for n in LATENCY..dry.len() {
        assert_eq!(dry[n], input[n - LATENCY], "frame {n}");
    }
}

// Spec: spectral-blur R5
#[test]
fn freeze_holds_the_cloud_within_half_a_decibel_while_the_rest_passes() {
    let burst = 24_000;
    let parts: Vec<(f64, f64)> = (7..86)
        .map(|k| (k as f64 * RATE / 2_048.0, 0.005))
        .collect();
    let total = 30 * 48_000;
    let mut input = tones(&parts, burst);
    input.resize(total, 0.0);
    // Frozen half way through the burst; Halo is short, so a cloud that
    // kept decaying would be gone in a second.
    let frozen = [set(12_000, FREEZE, 1.0)];
    let mut held = blur(&[(HALO, 200.0)]);
    let out = render_blocks(held.as_mut(), &input, 4_096, &frozen);
    let windows: Vec<f64> = out[burst + 2 * LATENCY..]
        .chunks_exact(seconds(2))
        .map(|c| 10.0 * power(c).log10())
        .collect();
    let (high, low) = windows
        .iter()
        .fold((f64::MIN, f64::MAX), |(h, l), w| (h.max(*w), l.min(*w)));
    assert!(high - low < 0.5, "{windows:?}");
    assert!(low > -60.0, "it holds a cloud: {low}");
    // Turning Freeze off lets it decay again.
    let release = [set(12_000, FREEZE, 1.0), set(96_000, FREEZE, 0.0)];
    let mut node = blur(&[(HALO, 200.0)]);
    let out = render_blocks(node.as_mut(), &input, 4_096, &release);
    assert!(power(&out[total - 48_000..]) < 1e-10);
    // A tone above the range keeps passing while the cloud is frozen.
    let mut rest = tones(&parts, burst);
    let above = tones(&[(10_000.0, 0.02)], total);
    rest.resize(total, 0.0);
    let mixed: Vec<f32> = rest.iter().zip(&above).map(|(a, b)| a + b).collect();
    let out = render_blocks(blur(&[(HALO, 200.0)]).as_mut(), &mixed, 4_096, &frozen);
    for chunk in out[burst + 2 * LATENCY..].chunks_exact(seconds(5)) {
        assert!(db(amplitude(chunk, 10_000.0) / 0.02).abs() < 0.1);
    }
}

// Spec: spectral-blur R7
#[test]
fn a_render_is_the_same_at_every_block_size_and_every_run_and_follows_the_seed() {
    let input = common::noise(30_000);
    let events = [set(9_001, HALO, 300.0), set(12_345, FREEZE, 1.0)];
    let params = [(RESIDUAL, 30.0), (DRY_WET, 80.0)];
    let at = |id: u64, block: usize| {
        let mut node = blur_with(id, &params);
        render_blocks(node.as_mut(), &input, block, &events)
    };
    let reference = at(1, 4_096);
    assert!(reference[LATENCY..].iter().any(|s| *s != 0.0));
    for block in [1, 64, 512] {
        assert!(at(1, block) == reference, "block of {block}");
    }
    assert!(at(1, 4_096) == reference, "run after run");
    assert!(at(2, 4_096) != reference, "another device, another cloud");
}

// Spec: spectral-blur R11
#[test]
fn the_display_reads_the_range_and_the_input_spectrum_headless() {
    let t = spectral_blur_type();
    let id = DeviceId(Id::new(1).unwrap());
    let mut values = OrdMap::new();
    values.insert(LOW, 300.0);
    values.insert(HIGH, 3_000.0);
    let shown = bl_devices::display(SPECTRAL_BLUR, id, &t.description, &values, None).unwrap();
    assert!(!shown.live);
    let marker = |d: &bl_model::DeviceDisplay, name: &str| {
        d.markers.iter().find(|m| m.name == name).unwrap().clone()
    };
    let (low, high) = (marker(&shown, "Low"), marker(&shown, "High"));
    assert_eq!((low.x, high.x), (300.0, 3_000.0));
    assert_eq!(low.handle.unwrap().x, LOW);
    assert_eq!(high.handle.unwrap().x, HIGH);
    // High below Low sits at Low.
    values.insert(HIGH, 100.0);
    let shown = bl_devices::display(SPECTRAL_BLUR, id, &t.description, &values, None).unwrap();
    assert_eq!(marker(&shown, "High").x, 300.0);
    // From an engine: a 1 kHz tone of 0.5 is the loudest of the spectrum, at
    // about -6 dB.
    let mut node = blur(&[]);
    let mut reader = node.display().expect("Spectral Blur has a display");
    render_blocks(node.as_mut(), &tones(&[(1_000.0, 0.5)], 8_192), 4_096, &[]);
    let data = reader.read().values.clone();
    assert_eq!(data.len(), DISPLAY_LEN);
    let live =
        bl_devices::display(SPECTRAL_BLUR, id, &t.description, &values, Some(&data)).unwrap();
    assert!(live.live);
    let curve = &live.series("Spectrum").unwrap().values;
    let peak = curve
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .unwrap();
    let hz = 20.0 * 1_000f64.powf(peak.0 as f64 / (DISPLAY_LEN - 1) as f64);
    assert!((hz / 1_000.0 - 1.0).abs() < 0.1, "{hz} Hz");
    assert!((*peak.1 + 6.0).abs() < 1.5, "{} dB", peak.1);
}

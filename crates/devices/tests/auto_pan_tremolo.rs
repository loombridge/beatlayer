//! Auto Pan-Tremolo tests (WP-5.20c; the auto-pan-tremolo spec).
//! Every block runs as a real-time section under the allocation trap.

use crate::common;
use core::f64::consts::SQRT_2;

use bl_core::time::{Bpm, Frames, TempoMap, TempoSegment, Ticks};
use bl_devices::auto_pan_tremolo::{
    self, AMOUNT, ATTACK_TIME, AUTO_PAN_TREMOLO, DOTTED_DIVISION, DYNAMIC_FM, HARMONIC, INVERT,
    MODE, PHASE, PHASE_OFFSET, RATE_HZ, SHAPE, SIXTEENTHS, SPIN, STEREO_MODE, SYNCED_DIVISION,
    TIME_MODE, TIME_SECS, TRIPLET_DIVISION, VINTAGE, WAVE, auto_pan_tremolo_type,
};
use bl_engine::{Node, Registry, TransportInfo};
use bl_model::{Command, Device, DeviceId, DeviceRole, Editor, ParamId, TrackKind};
use common::{device, node, run, run_at, run_blocks};

const BLOCKS: [usize; 4] = [1, 64, 512, 4096];

fn registry() -> Registry {
    let mut registry = Registry::empty();
    auto_pan_tremolo::register(&mut registry);
    registry
}

fn auto_pan(params: &[(ParamId, f64)], max_frames: usize) -> Box<dyn Node> {
    let device_type = auto_pan_tremolo_type();
    node(&registry(), &device(&device_type, params), max_frames)
}

fn tempo(bpm: f64) -> TempoMap {
    TempoMap::constant(Bpm::new(bpm).expect("a tempo in range"))
}

// Spec: auto-pan-tremolo R1
#[test]
fn auto_pan_tremolo_is_described() {
    let device_type = auto_pan_tremolo_type();
    assert_eq!(device_type.name, "Auto Pan-Tremolo");
    assert_eq!(
        device_type.kind,
        bl_model::DeviceKind::Native {
            device_type: AUTO_PAN_TREMOLO.to_owned()
        }
    );
    assert_eq!(device_type.role, DeviceRole::AudioEffect);
    assert_eq!(device_type.description.params.len(), 20);
}

// Spec: auto-pan-tremolo R2
#[test]
fn panning_at_amount_100_moves_mono_input_by_pan_law_reaching_silence() {
    let params = vec![
        (MODE, 0.0),        // Panning
        (WAVE, 0.0),        // Sine
        (TIME_MODE, 0.0),   // Rate Hz
        (RATE_HZ, 1.0),     // 1 Hz = 48,000 frames
        (STEREO_MODE, 0.0), // Phase
        (PHASE, 180.0),     // Antiphase
        (AMOUNT, 100.0),    // 100 %
    ];
    let mut dev = auto_pan(&params, 48_000);
    let input = vec![1.0f32; 48_000];
    let out = run(dev.as_mut(), &input, &[]);

    let mut min_left = f32::MAX;
    let mut min_right = f32::MAX;
    let mut max_left = f32::MIN;
    let mut max_right = f32::MIN;

    for (i, (&l, &r)) in out[0].iter().zip(&out[1]).enumerate() {
        min_left = min_left.min(l);
        min_right = min_right.min(r);
        max_left = max_left.max(l);
        max_right = max_right.max(r);

        // Constant power pan law: l^2 + r^2 == 2.0 (input is 1.0 mono)
        let power = (l as f64) * (l as f64) + (r as f64) * (r as f64);
        assert!(
            (power - 2.0).abs() < 1e-4,
            "constant power at frame {i}: {power}"
        );
    }

    // Far sides reach -inf dB (0.0)
    assert!(min_left.abs() < 1e-5, "left reaches silence: {min_left}");
    assert!(min_right.abs() < 1e-5, "right reaches silence: {min_right}");
    // Peaks reach sqrt(2) * 1.0
    assert!(
        (max_left as f64 - SQRT_2).abs() < 1e-3,
        "left peak reaches sqrt(2): {max_left}"
    );
    assert!(
        (max_right as f64 - SQRT_2).abs() < 1e-3,
        "right peak reaches sqrt(2): {max_right}"
    );
}

// Spec: auto-pan-tremolo R2
#[test]
fn tremolo_at_amount_100_reaches_silence_at_troughs() {
    let params = vec![
        (MODE, 1.0),      // Tremolo
        (WAVE, 0.0),      // Sine
        (TIME_MODE, 0.0), // Rate Hz
        (RATE_HZ, 1.0),   // 1 Hz
        (AMOUNT, 100.0),  // 100 %
    ];
    let mut dev = auto_pan(&params, 48_000);
    let input = vec![1.0f32; 48_000];
    let out = run(dev.as_mut(), &input, &[]);

    let mut min_level = f32::MAX;
    let mut max_level = f32::MIN;
    for &sample in &out[0] {
        min_level = min_level.min(sample);
        max_level = max_level.max(sample);
    }

    assert!(
        min_level.abs() < 1e-4,
        "tremolo trough reaches -inf dB silence: {min_level}"
    );
    assert!(
        (max_level - 1.0).abs() < 1e-3,
        "tremolo peak reaches 1.0: {max_level}"
    );
}

// Spec: auto-pan-tremolo R3
#[test]
fn every_waveform_plays_and_random_shapes_are_repeatable() {
    for wave_idx in 0..9 {
        let params = vec![
            (MODE, 1.0),
            (WAVE, wave_idx as f64),
            (TIME_MODE, 0.0),
            (RATE_HZ, 10.0),
            (AMOUNT, 100.0),
        ];
        let mut dev1 = auto_pan(&params, 24_000);
        let input = vec![1.0f32; 24_000];
        let out1 = run(dev1.as_mut(), &input, &[]);

        // Check it modulates
        let max_val = out1[0].iter().copied().fold(f32::MIN, f32::max);
        let min_val = out1[0].iter().copied().fold(f32::MAX, f32::min);
        assert!(max_val - min_val > 0.1, "wave {wave_idx} modulates signal");

        // Repeatable run after run
        let mut dev2 = auto_pan(&params, 24_000);
        let out2 = run(dev2.as_mut(), &input, &[]);
        assert_eq!(out1, out2, "wave {wave_idx} repeats run after run");
    }
}

// Spec: auto-pan-tremolo R4
#[test]
fn hz_and_time_periods_are_accurate_within_0_1_percent() {
    // Hz mode: 10 Hz => period is 4800 frames
    let params_hz = vec![
        (MODE, 1.0),
        (WAVE, 0.0),      // Sine
        (TIME_MODE, 0.0), // Rate
        (RATE_HZ, 10.0),
        (AMOUNT, 100.0),
    ];
    let mut dev = auto_pan(&params_hz, 48_000);
    let input = vec![1.0f32; 48_000];
    let out = run(dev.as_mut(), &input, &[]);
    // Find troughs (zero crossings of derivative / minima)
    let mut troughs = Vec::new();
    for i in 1..47_999 {
        if out[0][i] < out[0][i - 1] && out[0][i] <= out[0][i + 1] && out[0][i] < 0.01 {
            troughs.push(i);
        }
    }
    assert!(troughs.len() >= 9);
    for window in troughs.windows(2) {
        let period = (window[1] - window[0]) as f64;
        assert!(
            (period - 4800.0).abs() / 4800.0 < 0.001,
            "Hz period within 0.1%: {period}"
        );
    }

    // Time mode: 0.2 s (200 ms) => period is 9600 frames
    let params_time = vec![
        (MODE, 1.0),
        (WAVE, 0.0),
        (TIME_MODE, 1.0), // Time
        (TIME_SECS, 0.2),
        (AMOUNT, 100.0),
    ];
    let mut dev = auto_pan(&params_time, 48_000);
    let out = run(dev.as_mut(), &input, &[]);
    let mut troughs = Vec::new();
    for i in 1..47_999 {
        if out[0][i] < out[0][i - 1] && out[0][i] <= out[0][i + 1] && out[0][i] < 0.01 {
            troughs.push(i);
        }
    }
    assert!(troughs.len() >= 4);
    for window in troughs.windows(2) {
        let period = (window[1] - window[0]) as f64;
        assert!(
            (period - 9600.0).abs() / 9600.0 < 0.001,
            "Time period within 0.1%: {period}"
        );
    }
}

// Spec: auto-pan-tremolo R4, R6
#[test]
fn each_synced_mode_starts_cycle_on_beat_across_tempos() {
    let input = vec![vec![1.0f32; 48_000], vec![1.0f32; 48_000]];

    // Straight 1/4 note (1 beat) at 120 BPM: 1 beat = 24,000 frames
    let params_120 = vec![
        (MODE, 1.0),
        (WAVE, 0.0),            // Sine
        (TIME_MODE, 2.0),       // Synced
        (SYNCED_DIVISION, 3.0), // 1/4 note (1.0 beat)
        (AMOUNT, 100.0),
    ];
    let mut dev = auto_pan(&params_120, 48_000);
    let out = run_at(
        dev.as_mut(),
        &input,
        &[],
        &tempo(120.0),
        TransportInfo {
            playing: true,
            position: Frames(0),
            ..TransportInfo::default()
        },
    );
    // Sine starts at 0 phase (midpoint level = 0.5)
    assert!(
        (out[0][0] - 0.5).abs() < 1e-3,
        "cycle starts on beat at 120 BPM"
    );
    // Half cycle at 12,000 frames
    assert!(
        (out[0][12_000] - 0.5).abs() < 1e-3,
        "half cycle at 12,000 frames"
    );

    // At 60 BPM: 1 beat = 48,000 frames
    let mut dev = auto_pan(&params_120, 48_000);
    let out = run_at(
        dev.as_mut(),
        &input,
        &[],
        &tempo(60.0),
        TransportInfo {
            playing: true,
            position: Frames(0),
            ..TransportInfo::default()
        },
    );
    assert!(
        (out[0][0] - 0.5).abs() < 1e-3,
        "cycle starts on beat at 60 BPM"
    );
    assert!(
        (out[0][24_000] - 0.5).abs() < 1e-3,
        "half cycle at 24,000 frames"
    );

    // Phase Offset moves synced cycle by its fraction: 90° = 0.25 cycle
    let mut params_offset = params_120.clone();
    params_offset.push((PHASE_OFFSET, 90.0));
    let mut dev = auto_pan(&params_offset, 48_000);
    let out = run_at(
        dev.as_mut(),
        &input,
        &[],
        &tempo(120.0),
        TransportInfo {
            playing: true,
            position: Frames(0),
            ..TransportInfo::default()
        },
    );
    // At phase 90° (0.25 cycle), Sine is at peak (+1.0), so tremolo level is 1.0
    assert!(
        (out[0][0] - 1.0).abs() < 1e-3,
        "Phase Offset shifts start to peak: {}",
        out[0][0]
    );

    // Across tempo change
    let tempo_change = TempoMap::new([
        TempoSegment::Constant {
            start: Ticks::ZERO,
            bpm: Bpm::new(120.0).unwrap(),
        },
        TempoSegment::Constant {
            start: Ticks::from_quarters(1),
            bpm: Bpm::new(60.0).unwrap(),
        },
    ])
    .unwrap();
    let mut dev = auto_pan(&params_120, 48_000);
    let out = run_at(
        dev.as_mut(),
        &input,
        &[],
        &tempo_change,
        TransportInfo {
            playing: true,
            position: Frames(0),
            ..TransportInfo::default()
        },
    );
    assert!(
        (out[0][0] - 0.5).abs() < 1e-3,
        "starts on beat across tempo change"
    );
}

// Spec: auto-pan-tremolo R5
#[test]
fn phase_offsets_channels_and_spin_detunes() {
    let input = vec![1.0f32; 48_000];

    // Phase 180° in Panning mode puts channels in antiphase
    let params_phase = vec![
        (MODE, 0.0),
        (WAVE, 0.0),
        (TIME_MODE, 0.0),
        (RATE_HZ, 1.0),
        (STEREO_MODE, 0.0),
        (PHASE, 180.0),
        (AMOUNT, 100.0),
    ];
    let mut dev = auto_pan(&params_phase, 48_000);
    let out = run(dev.as_mut(), &input, &[]);
    // At quarter cycle (12,000 frames), one side at peak, other at silence
    assert!(out[0][12_000] > 1.4 && out[1][12_000] < 1e-4);

    // Spin detunes right channel rate
    let params_spin = vec![
        (MODE, 0.0),
        (WAVE, 0.0),
        (TIME_MODE, 0.0),
        (RATE_HZ, 1.0),
        (STEREO_MODE, 1.0), // Spin
        (SPIN, 50.0),       // Right rate is 1.5x left rate
        (AMOUNT, 100.0),
    ];
    let mut dev = auto_pan(&params_spin, 48_000);
    let out = run(dev.as_mut(), &input, &[]);
    // Channels should drift apart and not be in static relationship
    assert_ne!(out[0], out[1]);
}

// Spec: auto-pan-tremolo R6
#[test]
fn phase_offset_acts_as_width_on_random_shapes() {
    let input = vec![1.0f32; 48_000];

    // In Panning mode, Wave = Random (6), Phase Offset = 0° (0 width => mono center, left == right)
    let params_mono = vec![
        (MODE, 0.0), // Panning
        (WAVE, 6.0), // Random
        (TIME_MODE, 0.0),
        (RATE_HZ, 4.0),
        (PHASE_OFFSET, 0.0), // 0 width
        (AMOUNT, 100.0),
    ];
    let mut dev = auto_pan(&params_mono, 48_000);
    let out_mono = run(dev.as_mut(), &input, &[]);
    assert_eq!(
        out_mono[0], out_mono[1],
        "0° Phase Offset gives mono identical random channels"
    );

    // Phase Offset = 180° (full width => left != right)
    let params_stereo = vec![
        (MODE, 0.0), // Panning
        (WAVE, 6.0), // Random
        (TIME_MODE, 0.0),
        (RATE_HZ, 4.0),
        (PHASE_OFFSET, 180.0), // Full stereo width
        (AMOUNT, 100.0),
    ];
    let mut dev = auto_pan(&params_stereo, 48_000);
    let out_stereo = run(dev.as_mut(), &input, &[]);
    assert_ne!(
        out_stereo[0], out_stereo[1],
        "180° Phase Offset produces stereo variation"
    );
}

// Spec: auto-pan-tremolo R7
#[test]
fn invert_flips_the_shape() {
    let input = vec![1.0f32; 48_000];
    let params_normal = vec![
        (MODE, 1.0),
        (WAVE, 3.0), // Saw Up
        (TIME_MODE, 0.0),
        (RATE_HZ, 1.0),
        (INVERT, 0.0),
        (AMOUNT, 100.0),
    ];
    let mut dev = auto_pan(&params_normal, 48_000);
    let out_normal = run(dev.as_mut(), &input, &[]);

    let params_inverted = vec![
        (MODE, 1.0),
        (WAVE, 3.0), // Saw Up inverted -> Saw Down
        (TIME_MODE, 0.0),
        (RATE_HZ, 1.0),
        (INVERT, 1.0),
        (AMOUNT, 100.0),
    ];
    let mut dev = auto_pan(&params_inverted, 48_000);
    let out_inverted = run(dev.as_mut(), &input, &[]);

    // Troughs and peaks swap
    assert!((out_normal[0][100] - out_inverted[0][48_000 - 100]).abs() < 0.05);
}

// Spec: auto-pan-tremolo R8
#[test]
fn harmonic_bands_sum_flat_at_amount_zero_and_move_in_antiphase() {
    let input = common::noise(4800);
    // At Amount 0 %, low and high bands sum flat within 0.1 dB
    let params_flat = vec![
        (MODE, 1.0), // Tremolo
        (HARMONIC, 1.0),
        (AMOUNT, 0.0),
    ];
    let mut dev = auto_pan(&params_flat, 4800);
    let out = run(dev.as_mut(), &input, &[]);
    for i in 100..4800 {
        let diff = (out[0][i] - input[i]).abs();
        assert!(
            diff < 1e-4,
            "harmonic sums flat within 0.1 dB at frame {i}: diff {diff}"
        );
    }

    // At Amount 100 %, low and high bands move in antiphase
    let params_active = vec![
        (MODE, 1.0),
        (WAVE, 0.0),
        (TIME_MODE, 0.0),
        (RATE_HZ, 1.0),
        (HARMONIC, 1.0),
        (AMOUNT, 100.0),
    ];
    let mut dev_low = auto_pan(&params_active, 48_000);
    let low_sine = common::sine(100.0, 1.0, 48_000); // well below 600 Hz crossover
    let out_low = run(dev_low.as_mut(), &low_sine, &[]);

    let mut dev_high = auto_pan(&params_active, 48_000);
    let high_sine = common::sine(3000.0, 1.0, 48_000); // well above 600 Hz crossover
    let out_high = run(dev_high.as_mut(), &high_sine, &[]);

    let rms = |slice: &[f32]| {
        (slice.iter().map(|&x| (x as f64) * (x as f64)).sum::<f64>() / slice.len() as f64).sqrt()
    };
    let low_rms_peak = rms(&out_low[0][11_500..12_500]);
    let high_rms_at_low_peak = rms(&out_high[0][11_500..12_500]);
    let low_rms_trough = rms(&out_low[0][35_500..36_500]);
    let high_rms_at_low_trough = rms(&out_high[0][35_500..36_500]);

    assert!(
        low_rms_peak > high_rms_at_low_peak,
        "low band peak corresponds to high band trough"
    );
    assert!(
        low_rms_trough < high_rms_at_low_trough,
        "low band trough corresponds to high band peak"
    );
}

// Spec: auto-pan-tremolo R9
#[test]
fn vintage_adds_warmth_with_nonlinear_shaping() {
    let input = vec![1.0f32; 1000];
    let params_clean = vec![(MODE, 1.0), (VINTAGE, 0.0), (AMOUNT, 0.0)];
    let mut dev_clean = auto_pan(&params_clean, 1000);
    let out_clean = run(dev_clean.as_mut(), &input, &[]);

    let params_vintage = vec![(MODE, 1.0), (VINTAGE, 1.0), (AMOUNT, 0.0)];
    let mut dev_vintage = auto_pan(&params_vintage, 1000);
    let out_vintage = run(dev_vintage.as_mut(), &input, &[]);

    // Vintage applies tanh(1.2 * 1.0) / 1.2 = ~0.695
    assert_ne!(
        out_clean[0], out_vintage[0],
        "vintage non-linear shaping differs from clean"
    );
    let expected = (1.2f64.tanh() / 1.2) as f32;
    assert!(
        (out_vintage[0][0] - expected).abs() < 1e-3,
        "vintage warm saturation curve matches"
    );
}

// Spec: auto-pan-tremolo R10
#[test]
fn shape_modifies_panning_and_tremolo_waveforms() {
    let input = vec![1.0f32; 4800];
    let params_0 = vec![(MODE, 1.0), (WAVE, 0.0), (SHAPE, 0.0), (AMOUNT, 100.0)];
    let mut dev0 = auto_pan(&params_0, 4800);
    let out0 = run(dev0.as_mut(), &input, &[]);

    let params_pos = vec![(MODE, 1.0), (WAVE, 0.0), (SHAPE, 100.0), (AMOUNT, 100.0)];
    let mut dev_pos = auto_pan(&params_pos, 4800);
    let out_pos = run(dev_pos.as_mut(), &input, &[]);

    assert_ne!(out0[0], out_pos[0], "Shape modifies the waveform");
}

// Spec: auto-pan-tremolo R11
#[test]
fn attack_time_delays_modulation_depth_after_onset_within_5_percent() {
    // 100 ms Attack Time at 48 kHz = 4800 frames
    let params = vec![
        (MODE, 1.0),
        (WAVE, 0.0),
        (TIME_MODE, 0.0),
        (RATE_HZ, 20.0),      // Fast modulation
        (ATTACK_TIME, 100.0), // 100 ms
        (AMOUNT, 100.0),
    ];
    let mut dev = auto_pan(&params, 9600);

    // Initial onset: sudden jump from 0.0 to 1.0
    let input = vec![1.0f32; 9600];
    let out = run(dev.as_mut(), &input, &[]);

    // Immediately after onset (first few frames), modulation depth should be suppressed (close to 1.0 unmodulated)
    assert!(
        (out[0][0] - 1.0).abs() < 0.05,
        "depth starts suppressed after onset"
    );

    // After 4800 frames (100 ms), depth reaches full (reaches close to 0 in troughs)
    let min_after_attack = out[0][4800..9600].iter().copied().fold(f32::MAX, f32::min);
    assert!(
        min_after_attack < 0.05,
        "full depth reached after Attack Time within 5%"
    );
}

// Spec: auto-pan-tremolo R12
#[test]
fn dynamic_frequency_modulation_scales_rate_with_input_level() {
    let input_loud = vec![1.0f32; 48_000];

    // DFM +100 % with loud input
    let params_pos = vec![
        (MODE, 1.0),
        (WAVE, 0.0),
        (TIME_MODE, 0.0),
        (RATE_HZ, 2.0),
        (DYNAMIC_FM, 100.0),
        (AMOUNT, 100.0),
    ];
    let mut dev_pos = auto_pan(&params_pos, 48_000);
    let out_pos = run(dev_pos.as_mut(), &input_loud, &[]);

    // DFM -100 % with loud input
    let params_neg = vec![
        (MODE, 1.0),
        (WAVE, 0.0),
        (TIME_MODE, 0.0),
        (RATE_HZ, 2.0),
        (DYNAMIC_FM, -100.0),
        (AMOUNT, 100.0),
    ];
    let mut dev_neg = auto_pan(&params_neg, 48_000);
    let out_neg = run(dev_neg.as_mut(), &input_loud, &[]);

    // Count zero crossings / cycles
    let count_crossings = |sig: &[f32]| {
        sig.windows(2)
            .filter(|w| (w[0] - 0.5) * (w[1] - 0.5) < 0.0)
            .count()
    };
    let cycles_pos = count_crossings(&out_pos[0]);
    let cycles_neg = count_crossings(&out_neg[0]);

    assert!(
        cycles_pos > cycles_neg,
        "positive DFM raises rate ({cycles_pos}), negative DFM lowers rate ({cycles_neg})"
    );
}

// Spec: auto-pan-tremolo R13
#[test]
fn auto_pan_tremolo_reports_zero_latency() {
    let dev = auto_pan(&[], 256);
    assert_eq!(dev.latency(), 0, "Auto Pan-Tremolo reports zero latency");
}

// Spec: auto-pan-tremolo R14
#[test]
fn pan_position_is_readable_headlessly_through_display() {
    let params = vec![(MODE, 0.0), (WAVE, 0.0), (AMOUNT, 100.0)];
    let mut dev = auto_pan(&params, 256);
    let input = vec![1.0f32; 256];
    let _ = run(dev.as_mut(), &input, &[]);

    let device_type = auto_pan_tremolo_type();
    let display = bl_devices::display(
        AUTO_PAN_TREMOLO,
        DeviceId(bl_core::id::Id::new(1).unwrap()),
        &device_type.description,
        &bl_model::OrdMap::new(),
        Some(&[0.5f32, 0.8f32]),
    );
    assert!(display.is_some());
    let disp = display.unwrap();
    assert_eq!(disp.readings[0].name, "Pan Position");
    assert_eq!(disp.readings[0].value, 0.5);
    assert_eq!(disp.readings[1].name, "Tremolo Level");
    assert_eq!(disp.readings[1].value, 0.8);
}

// Spec: auto-pan-tremolo R15
#[test]
fn renders_are_bit_identical_at_various_block_sizes_and_runs() {
    let input = common::noise(4096);
    let tempo_map = tempo(120.0);

    for wave in [0.0, 1.0, 6.0] {
        let params = vec![
            (MODE, 0.0),
            (WAVE, wave),
            (TIME_MODE, 0.0),
            (RATE_HZ, 2.0),
            (AMOUNT, 100.0),
        ];

        let mut dev1 = auto_pan(&params, 4096);
        let first = run_blocks(
            dev1.as_mut(),
            &[input.clone(), input.clone()],
            4096,
            &[],
            &tempo_map,
        );

        for &block in &BLOCKS {
            let mut dev = auto_pan(&params, 4096);
            let out = run_blocks(
                dev.as_mut(),
                &[input.clone(), input.clone()],
                block,
                &[],
                &tempo_map,
            );
            assert_eq!(out, first, "wave {wave} matches at block size {block}");
        }

        // Run after run repeatability
        let mut dev2 = auto_pan(&params, 4096);
        let second = run_blocks(
            dev2.as_mut(),
            &[input.clone(), input.clone()],
            4096,
            &[],
            &tempo_map,
        );
        assert_eq!(first, second, "wave {wave} is identical run after run");
    }
}

// Spec: auto-pan-tremolo R16
#[test]
fn a_set_with_auto_pan_tremolo_saves_and_reopens() {
    let mut editor = Editor::new();
    let track = editor.new_id();
    editor
        .apply(Command::CreateTrack {
            id: track,
            kind: TrackKind::Audio,
            name: "Audio".to_owned(),
            parent: None,
            index: 0,
        })
        .unwrap();

    let mut dev = Device::new(
        editor.new_id(),
        "Auto Pan-Tremolo",
        bl_model::DeviceKind::Native {
            device_type: AUTO_PAN_TREMOLO.to_owned(),
        },
    );
    dev.params.insert(MODE, 1.0);
    dev.params.insert(WAVE, 2.0);
    dev.params.insert(TIME_MODE, 1.0);
    dev.params.insert(RATE_HZ, 5.0);
    dev.params.insert(TIME_SECS, 0.5);
    dev.params.insert(SYNCED_DIVISION, 4.0);
    dev.params.insert(DOTTED_DIVISION, 3.0);
    dev.params.insert(TRIPLET_DIVISION, 1.0);
    dev.params.insert(SIXTEENTHS, 8.0);
    dev.params.insert(STEREO_MODE, 1.0);
    dev.params.insert(PHASE, 90.0);
    dev.params.insert(SPIN, 25.0);
    dev.params.insert(PHASE_OFFSET, 45.0);
    dev.params.insert(INVERT, 1.0);
    dev.params.insert(HARMONIC, 1.0);
    dev.params.insert(VINTAGE, 1.0);
    dev.params.insert(AMOUNT, 75.0);
    dev.params.insert(SHAPE, 50.0);
    dev.params.insert(ATTACK_TIME, 50.0);
    dev.params.insert(DYNAMIC_FM, -30.0);

    let dev_id = dev.id;
    editor
        .apply(Command::InsertDevice {
            track,
            index: 0,
            device: dev,
        })
        .unwrap();

    let saved = editor.document().clone();
    let reopened = Editor::from_document(saved, bl_core::id::IdAllocator::new());
    let track_doc = reopened.document().track(track).unwrap();
    let reopened_dev = track_doc.devices().find(|d| d.id == dev_id).unwrap();

    assert_eq!(reopened_dev.params.get(&MODE), Some(&1.0));
    assert_eq!(reopened_dev.params.get(&WAVE), Some(&2.0));
    assert_eq!(reopened_dev.params.get(&TIME_MODE), Some(&1.0));
    assert_eq!(reopened_dev.params.get(&RATE_HZ), Some(&5.0));
    assert_eq!(reopened_dev.params.get(&TIME_SECS), Some(&0.5));
    assert_eq!(reopened_dev.params.get(&SYNCED_DIVISION), Some(&4.0));
    assert_eq!(reopened_dev.params.get(&DOTTED_DIVISION), Some(&3.0));
    assert_eq!(reopened_dev.params.get(&TRIPLET_DIVISION), Some(&1.0));
    assert_eq!(reopened_dev.params.get(&SIXTEENTHS), Some(&8.0));
    assert_eq!(reopened_dev.params.get(&STEREO_MODE), Some(&1.0));
    assert_eq!(reopened_dev.params.get(&PHASE), Some(&90.0));
    assert_eq!(reopened_dev.params.get(&SPIN), Some(&25.0));
    assert_eq!(reopened_dev.params.get(&PHASE_OFFSET), Some(&45.0));
    assert_eq!(reopened_dev.params.get(&INVERT), Some(&1.0));
    assert_eq!(reopened_dev.params.get(&HARMONIC), Some(&1.0));
    assert_eq!(reopened_dev.params.get(&VINTAGE), Some(&1.0));
    assert_eq!(reopened_dev.params.get(&AMOUNT), Some(&75.0));
    assert_eq!(reopened_dev.params.get(&SHAPE), Some(&50.0));
    assert_eq!(reopened_dev.params.get(&ATTACK_TIME), Some(&50.0));
    assert_eq!(reopened_dev.params.get(&DYNAMIC_FM), Some(&-30.0));
}

// Spec: transport R24
#[test]
fn a_synced_cycle_follows_the_played_tempo() {
    let input = vec![vec![1.0f32; 12_000], vec![1.0f32; 12_000]];
    let params = vec![
        (MODE, 1.0),
        (WAVE, 0.0),
        (TIME_MODE, 2.0),
        (SYNCED_DIVISION, 3.0), // 1/4 note
        (AMOUNT, 100.0),
    ];
    let run_with = |bpm: f64, speed: f64| {
        run_at(
            auto_pan(&params, 12_000).as_mut(),
            &input,
            &[],
            &tempo(bpm),
            TransportInfo {
                playing: true,
                position: Frames(0),
                speed,
                ..TransportInfo::default()
            },
        )
    };
    // A quarter note at 240 BPM is 12,000 frames, so the peak is at 3,000.
    let followed = run_with(120.0, 2.0);
    assert!(
        (followed[0][3_000] - 1.0).abs() < 1e-3,
        "the peak at the played tempo"
    );
    let same = run_with(240.0, 1.0);
    for (a, b) in followed[0].iter().zip(&same[0]) {
        assert!((a - b).abs() < 1e-4, "{a} against {b}");
    }
}

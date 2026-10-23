//! Shifter, the native Shifter (WP-5.27; the pitch-and-frequency-shifter spec):
//! the three modes against the spec's numbers, Spread and Wide, the LFO and
//! the envelope follower, the delay, the MIDI sidechain's notes, glide and
//! pitch bend, and renders that repeat at any block size. Every block runs
//! as a real-time section under the allocation trap.

use crate::common;
use bl_core::time::{Bpm, Frames, SampleRate, TempoMap, TempoSegment, Ticks};
use bl_devices::shifter::{
    self, BEND_RANGE, COARSE, DELAY_DIVISION, DELAY_HZ, DELAY_MODE, DELAY_ON, DRIVE, DRY_WET,
    ENV_ATTACK, ENV_HZ, ENV_ON, ENV_RELEASE, FEEDBACK, FINE, GLIDE, LFO_AMOUNT, LFO_DIVISION,
    LFO_HZ, LFO_MODE, LFO_WAVE, LFO_WIDTH, MODE, PITCH_SOURCE, SHIFT, SHIFTER, SPREAD, TONE, WIDE,
    WINDOW, shifter_type,
};
use bl_engine::{
    Block, DeviceType, Node, NoteBuffer, NoteEvent, NoteKind, Registry, TransportInfo,
};
use bl_model::ParamId;
use common::{CHANNELS, RATE};

const BLOCKS: [usize; 4] = [1, 64, 512, 4096];

/// The frames every mode delays by: 10 ms at 48 kHz.
const LATENCY: usize = 480;

fn registry() -> Registry {
    let mut registry = Registry::empty();
    shifter::register(&mut registry);
    registry
}

fn veer(params: &[(ParamId, f64)], max_frames: usize) -> Box<dyn Node> {
    let device_type: DeviceType = shifter_type();
    common::node(
        &registry(),
        &common::device(&device_type, params),
        max_frames,
    )
}

/// `frames` of a sine at `hz`, full scale.
fn sine(hz: f64, frames: usize) -> Vec<f32> {
    common::sine(hz, 1.0, frames)
}

/// Streams the same `input` on both channels through `node` in blocks of
/// `block`, at 120 BPM with the song stopped, and returns both channels.
fn stream(node: &mut dyn Node, input: &[f32], block: usize) -> Vec<Vec<f32>> {
    common::stream(node, &[input.to_vec(), input.to_vec()], block)
}

/// Streams `input` on both channels through `node` in blocks of `block`
/// with the song playing from frame 0 under `tempo`.
fn play(node: &mut dyn Node, input: &[f32], block: usize, tempo: &TempoMap) -> Vec<Vec<f32>> {
    let mut out: Vec<Vec<f32>> = (0..CHANNELS)
        .map(|_| Vec::with_capacity(input.len()))
        .collect();
    for start in (0..input.len()).step_by(block) {
        let end = (start + block).min(input.len());
        let frames = end - start;
        let mut data: Vec<f32> = (0..CHANNELS)
            .flat_map(|_| input[start..end].iter().copied())
            .collect();
        {
            let transport = TransportInfo {
                playing: true,
                position: Frames(start as i64),
            };
            let mut block = Block::new(&mut data, CHANNELS, frames, frames, &[], transport, RATE)
                .with_tempo(tempo);
            bl_rt::rt_test!({ node.process(&mut block) });
        }
        for (channel, values) in out.iter_mut().zip(data.chunks(frames)) {
            channel.extend_from_slice(values);
        }
    }
    out
}

/// Runs `input` on both channels through `node` as a single block holding
/// `notes`, and returns both channels.
fn run_with_notes(node: &mut dyn Node, input: &[f32], notes: &[NoteEvent]) -> Vec<Vec<f32>> {
    let frames = input.len();
    let mut data: Vec<f32> = (0..CHANNELS).flat_map(|_| input.iter().copied()).collect();
    let mut out = NoteBuffer::with_capacity(8);
    {
        let mut block = Block::new(
            &mut data,
            CHANNELS,
            frames,
            frames,
            &[],
            TransportInfo::default(),
            RATE,
        )
        .with_notes(notes, &mut out);
        bl_rt::rt_test!({ node.process(&mut block) });
    }
    data.chunks(frames).map(<[f32]>::to_vec).collect()
}

/// The frequency of the strongest component of `signal` (a second of audio
/// or more), found from a Hann-windowed spectrum by interpolating the peak.
fn peak_hz(signal: &[f32]) -> f64 {
    let spectrum = common::spectrum(signal);
    let (bin, _) = spectrum
        .iter()
        .enumerate()
        .skip(2)
        .max_by(|a, b| a.1.total_cmp(b.1))
        .expect("a spectrum");
    let level = |i: usize| spectrum[i].max(1e-12).ln();
    let (a, b, c) = (level(bin - 1), level(bin), level(bin + 1));
    let offset = 0.5 * (a - c) / (a - 2.0 * b + c);
    (bin as f64 + offset) * RATE.as_f64() / signal.len() as f64
}

/// How many cents `hz` is above `reference`.
fn cents(hz: f64, reference: f64) -> f64 {
    1_200.0 * (hz / reference).log2()
}

/// The amplitude of `signal` at `hz`, which has a whole number of cycles in
/// it.
fn amplitude_at(signal: &[f32], hz: f64) -> f64 {
    let (re, im) = common::dft_at(signal, hz);
    re.hypot(im)
}

/// The frequency of a stretch of a clean sine from its positive-going zero
/// crossings, in Hz.
fn crossing_hz(signal: &[f32]) -> f64 {
    let mut crossings = Vec::new();
    for (i, pair) in signal.windows(2).enumerate() {
        if pair[0] < 0.0 && pair[1] >= 0.0 {
            crossings.push(i as f64 + f64::from(-pair[0]) / f64::from(pair[1] - pair[0]));
        }
    }
    assert!(crossings.len() > 3, "a sine in the stretch");
    (crossings.len() - 1) as f64 * RATE.as_f64() / (crossings[crossings.len() - 1] - crossings[0])
}

fn db(ratio: f64) -> f64 {
    20.0 * ratio.max(1e-12).log10()
}

// Spec: pitch-and-frequency-shifter R1
#[test]
fn veer_is_described_with_a_midi_sidechain_an_expanded_part_and_a_latency() {
    let device_type = shifter_type();
    device_type.description.check().unwrap();
    assert_eq!(device_type.name, "Shifter");
    assert!(device_type.midi_sidechain && !device_type.sidechain);
    assert!(matches!(
        device_type.kind,
        bl_model::DeviceKind::Native { ref device_type } if device_type == SHIFTER
    ));
    assert!(device_type.description.layout.has_expanded());
    // A new device has a MIDI sidechain at No Input.
    let device = device_type.device(bl_model::DeviceId(bl_core::id::Id::new(1).unwrap()));
    assert_eq!(
        device.midi_sidechain,
        Some(bl_model::MidiSidechain::default())
    );
    // Every mode reports the same latency, so Dry/Wet lines up.
    for mode in [0.0, 1.0, 2.0] {
        assert_eq!(veer(&[(MODE, mode)], 512).latency(), LATENCY as u32);
    }
}

// Spec: pitch-and-frequency-shifter R4
#[test]
fn freq_mode_moves_a_sine_by_the_shift_and_cancels_the_other_image() {
    let mut node = veer(&[(MODE, 1.0), (SHIFT, 100.0)], 4800);
    let out = stream(node.as_mut(), &sine(1_000.0, 52_800), 4800);
    let heard = &out[0][4_800..];
    assert!((peak_hz(heard) - 1_100.0).abs() < 0.1, "{}", peak_hz(heard));
    let up = amplitude_at(heard, 1_100.0);
    let image = amplitude_at(heard, 900.0);
    assert!(
        db(up) > -0.1,
        "the shifted tone keeps its level: {}",
        db(up)
    );
    assert!(db(image / up) < -60.0, "the image is {} dB", db(image / up));
    assert!(db(amplitude_at(heard, 1_000.0) / up) < -60.0);
    // A shift down.
    let mut node = veer(&[(MODE, 1.0), (SHIFT, -250.0)], 4800);
    let out = stream(node.as_mut(), &sine(1_000.0, 52_800), 4800);
    assert!((peak_hz(&out[0][4_800..]) - 750.0).abs() < 0.1);
}

// Spec: pitch-and-frequency-shifter R5
#[test]
fn ring_mode_gives_the_sum_and_difference_and_suppresses_the_input() {
    let mut node = veer(&[(MODE, 2.0), (SHIFT, 100.0)], 4800);
    let out = stream(node.as_mut(), &sine(1_000.0, 52_800), 4800);
    let heard = &out[0][4_800..];
    let (sum, difference) = (amplitude_at(heard, 1_100.0), amplitude_at(heard, 900.0));
    assert!(db(sum / difference).abs() < 0.1, "{}", db(sum / difference));
    assert!((sum - 0.5).abs() < 0.01, "each at half the input's level");
    assert!(db(amplitude_at(heard, 1_000.0) / sum) < -60.0);

    // Drive bends the ring-modulated signal, and none leaves it clean.
    let mut driven = veer(&[(MODE, 2.0), (SHIFT, 100.0), (DRIVE, 100.0)], 4800);
    let out = stream(driven.as_mut(), &sine(1_000.0, 52_800), 4800);
    let harmonic = amplitude_at(&out[0][4_800..], 3_300.0);
    assert!(
        db(harmonic / sum) > -40.0,
        "Drive adds harmonics: {}",
        db(harmonic / sum)
    );
    assert!(db(amplitude_at(heard, 3_300.0) / sum) < -60.0);
}

// Spec: pitch-and-frequency-shifter R3
#[test]
fn pitch_mode_moves_a_sine_seven_semitones_up_within_a_cent() {
    let mut node = veer(&[(COARSE, 7.0)], 4800);
    let out = stream(node.as_mut(), &sine(1_000.0, 52_800), 4800);
    let reference = 1_000.0 * (7.0f64 / 12.0).exp2();
    let heard = peak_hz(&out[0][4_800..]);
    assert!(cents(heard, reference).abs() < 1.0, "{heard} Hz");
    // Fine adds cents: 7.50 semitones.
    let mut node = veer(&[(COARSE, 7.0), (FINE, 50.0)], 4800);
    let out = stream(node.as_mut(), &sine(1_000.0, 52_800), 4800);
    let heard = peak_hz(&out[0][4_800..]);
    assert!(cents(heard, 1_000.0 * (7.5f64 / 12.0).exp2()).abs() < 1.0);
    // A shift down, with a longer window.
    let mut node = veer(&[(COARSE, -12.0), (WINDOW, 20.0)], 4800);
    let out = stream(node.as_mut(), &sine(1_000.0, 52_800), 4800);
    let heard = peak_hz(&out[0][4_800..]);
    assert!(cents(heard, 500.0).abs() < 1.0, "{heard} Hz");
}

// Spec: pitch-and-frequency-shifter R3
#[test]
fn pitch_mode_keeps_a_steady_tones_pitch_within_a_cent_across_tones_and_windows() {
    for (hz, semitones, window) in [
        (220.0, 7.0, 10.0),
        (440.0, 7.0, 10.0),
        (440.0, -5.0, 20.0),
        (1_760.0, 12.0, 4.0),
        (3_000.0, -7.0, 10.0),
        (330.0, 19.0, 10.0),
        (500.0, -12.0, 20.0),
        (1_000.0, -24.0, 2.0),
        (6_000.0, 12.0, 2.0),
    ] {
        let mut node = veer(&[(COARSE, semitones), (WINDOW, window)], 4800);
        let out = stream(node.as_mut(), &sine(hz, 52_800), 4800);
        let heard = peak_hz(&out[0][4_800..]);
        let expected = hz * (semitones / 12.0f64).exp2();
        assert!(
            cents(heard, expected).abs() < 1.0,
            "{hz} Hz by {semitones} st, window {window} ms: {heard} Hz, {} cents",
            cents(heard, expected)
        );
    }
}

// Spec: pitch-and-frequency-shifter R3
#[test]
fn a_fast_lfo_through_a_shift_of_nothing_keeps_the_heads_inside_the_line() {
    // The LFO swings the shift across nothing at 40 Hz, so a head's delay
    // turns back inside its run; the output must stay finite and bounded.
    for window in [2.0, 10.0, 20.0] {
        let params = [
            (LFO_AMOUNT, 100.0),
            (LFO_WAVE, 7.0),
            (LFO_HZ, 40.0),
            (WINDOW, window),
        ];
        let mut node = veer(&params, 4096);
        let out = stream(node.as_mut(), &common::noise(48_000), 4096);
        assert!(
            out[0].iter().all(|s| s.is_finite() && s.abs() < 3.0),
            "window {window}"
        );
    }
}

// Spec: pitch-and-frequency-shifter R3
#[test]
fn with_no_shift_pitch_mode_is_the_input_delayed_by_its_latency() {
    let input = common::noise(9_600);
    let mut node = veer(&[], 4800);
    let out = stream(node.as_mut(), &input, 4800);
    for i in LATENCY..9_600 {
        assert_eq!(out[0][i], input[i - LATENCY], "frame {i}");
    }
    // Dry/Wet at 0 % is the same signal in every mode.
    for mode in [1.0, 2.0] {
        let mut node = veer(&[(MODE, mode), (SHIFT, 300.0), (DRY_WET, 0.0)], 4800);
        let out = stream(node.as_mut(), &input, 4800);
        for i in LATENCY..9_600 {
            assert_eq!(out[0][i], input[i - LATENCY], "mode {mode}, frame {i}");
        }
    }
}

// Spec: pitch-and-frequency-shifter R6
#[test]
fn spread_detunes_the_channels_in_opposite_directions_and_wide_turns_the_right_over() {
    let params = [(COARSE, 0.0), (SPREAD, 100.0)];
    let mut node = veer(&params, 4800);
    let out = stream(node.as_mut(), &sine(1_000.0, 52_800), 4800);
    let (left, right) = (peak_hz(&out[0][4_800..]), peak_hz(&out[1][4_800..]));
    assert!(
        cents(left, 1_000.0 * (0.5f64 / 12.0).exp2()).abs() < 1.0,
        "{left}"
    );
    assert!(
        cents(right, 1_000.0 * (-0.5f64 / 12.0).exp2()).abs() < 1.0,
        "{right}"
    );

    // Wide flips the right: both channels go up.
    let mut node = veer(&[(SPREAD, 100.0), (WIDE, 1.0)], 4800);
    let out = stream(node.as_mut(), &sine(1_000.0, 52_800), 4800);
    let (left, right) = (peak_hz(&out[0][4_800..]), peak_hz(&out[1][4_800..]));
    assert!((left - right).abs() < 0.5, "{left} and {right}");

    // With no Spread, Wide changes nothing.
    let input = common::noise(9_600);
    let mut plain = veer(&[(WIDE, 0.0)], 4800);
    let mut wide = veer(&[(WIDE, 1.0)], 4800);
    assert_eq!(
        stream(plain.as_mut(), &input, 4800),
        stream(wide.as_mut(), &input, 4800)
    );

    // In Freq mode Spread is in Hz: 10 Hz each way.
    let mut node = veer(&[(MODE, 1.0), (SPREAD, 100.0)], 4800);
    let out = stream(node.as_mut(), &sine(1_000.0, 52_800), 4800);
    assert!((peak_hz(&out[0][4_800..]) - 1_010.0).abs() < 0.1);
    assert!((peak_hz(&out[1][4_800..]) - 990.0).abs() < 0.1);
}

/// Where the frequency of a Ring-mode render of a steady input changes, in
/// frames: the places two short stretches' frequencies differ by more than
/// a factor of two. The render is of DC, so its output is the carrier.
fn frequency_edges(carrier: &[f32], low: f64, high: f64) -> Vec<usize> {
    let mid = (low * high).sqrt();
    let mut edges = Vec::new();
    let mut fast = None;
    let mut last = None;
    for i in 1..carrier.len() {
        if carrier[i - 1] < 0.0 && carrier[i] >= 0.0 {
            if let Some(before) = last {
                let hz = RATE.as_f64() / (i - before) as f64;
                let now = hz > mid;
                if fast.is_some_and(|f| f != now) {
                    edges.push(i);
                }
                fast = Some(now);
            }
            last = Some(i);
        }
    }
    edges
}

// Spec: pitch-and-frequency-shifter R8
#[test]
fn a_synced_lfos_cycle_lands_on_the_beat_at_two_tempos_and_across_a_change() {
    // Ring mode on DC is the carrier: 4,000 Hz plus or minus 1,000 Hz as a
    // Rectangle LFO swings, once to a quarter note. The edges are at each
    // half beat.
    let params = [
        (MODE, 2.0),
        (SHIFT, 4_000.0),
        (LFO_AMOUNT, 100.0),
        (LFO_WAVE, 7.0),
        (LFO_MODE, 1.0),
        (LFO_DIVISION, 8.0),
    ];
    let quarter = Ticks::from_quarters(1);
    for (bpm, change) in [(120.0, false), (90.0, false), (120.0, true)] {
        let tempo = if change {
            TempoMap::new([
                TempoSegment::Constant {
                    start: Ticks::ZERO,
                    bpm: Bpm::new(120.0).unwrap(),
                },
                TempoSegment::Constant {
                    start: Ticks::from_quarters(2),
                    bpm: Bpm::new(90.0).unwrap(),
                },
            ])
            .unwrap()
        } else {
            TempoMap::constant(Bpm::new(bpm).unwrap())
        };
        let mut node = veer(&params, 4096);
        let frames = 128_000;
        let out = play(node.as_mut(), &vec![1.0; frames], 4096, &tempo);
        let edges = frequency_edges(&out[0], 3_000.0, 5_000.0);
        assert!(edges.len() >= 6, "{bpm}: {edges:?}");
        for (n, &edge) in edges.iter().take(6).enumerate() {
            // The rectangle is high for the first half of each beat and
            // low for the second: an edge each half beat after the first.
            let ticks = Ticks(quarter.0 / 2 * (n as i64 + 1));
            let expected = tempo.ticks_to_frames(ticks, RATE).get() as usize;
            assert!(
                edge.abs_diff(expected) <= 40,
                "{bpm} (change {change}): edge {n} at {edge}, expected {expected}"
            );
        }
    }
}

// Spec: pitch-and-frequency-shifter R8
#[test]
fn random_s_and_h_width_mirrors_the_channels_at_full_width() {
    // Ring mode on DC is the carrier: 4,000 Hz plus 1,000 Hz times the held
    // random value, which changes every 480 frames at 100 Hz. At full width
    // the right channel's value is the left's turned over, so the two
    // carriers' frequencies sum to 8,000 Hz; at no width they match.
    let params = |width: f64| {
        [
            (MODE, 2.0),
            (SHIFT, 4_000.0),
            (LFO_AMOUNT, 100.0),
            (LFO_WAVE, 9.0),
            (LFO_HZ, 100.0),
            (LFO_WIDTH, width),
        ]
    };
    for (width, mirrored) in [(100.0, true), (0.0, false)] {
        let mut node = veer(&params(width), 4096);
        let out = stream(node.as_mut(), &vec![1.0; 24_000], 4096);
        let mut moved = 0;
        for cycle in 2..40 {
            let stretch = cycle * 480 + 40..(cycle + 1) * 480 - 40;
            let (left, right) = (
                crossing_hz(&out[0][stretch.clone()]),
                crossing_hz(&out[1][stretch]),
            );
            if mirrored {
                assert!((left + right - 8_000.0).abs() < 160.0, "{left} and {right}");
            } else {
                assert!((left - right).abs() < 160.0, "{left} and {right}");
            }
            moved += usize::from((left - 4_000.0).abs() > 200.0);
        }
        assert!(moved > 10, "the random values move the carrier: {moved}");
    }
}

// Spec: pitch-and-frequency-shifter R9
#[test]
fn the_envelope_followers_attack_and_release_follow_their_times() {
    // Ring mode on DC: the carrier's frequency is the envelope in Hz, so the
    // output is the sine of its integral, which a model of the follower in
    // continuous time predicts.
    let (attack, release) = (0.05, 0.08);
    let params = [
        (MODE, 2.0),
        (ENV_ON, 1.0),
        (ENV_HZ, 400.0),
        (ENV_ATTACK, attack * 1_000.0),
        (ENV_RELEASE, release * 1_000.0),
    ];
    let rate = RATE.as_f64();
    // 0.25 s at full scale, then 0.25 s at a quarter of it, then a step
    // up again.
    let input: Vec<f32> = (0..36_000)
        .map(|n| match n {
            0..12_000 => 1.0,
            12_000..24_000 => 0.25,
            _ => 0.75,
        })
        .collect();
    let mut node = veer(&params, 4096);
    let out = stream(node.as_mut(), &input, 4096);
    let mut level = 0.0f64;
    let mut phase = 0.0f64;
    let mut worst = 0.0f64;
    for n in 0..input.len() {
        let x = f64::from(input[n]);
        let seconds = if x > level { attack } else { release };
        level += (x - level) * (1.0 - (-1.0 / (seconds * rate)).exp());
        let expected = if n >= LATENCY {
            f64::from(input[n - LATENCY]) * (std::f64::consts::TAU * phase).sin()
        } else {
            0.0
        };
        phase = (phase + 400.0 * level / rate).rem_euclid(1.0);
        worst = worst.max((f64::from(out[0][n]) - expected).abs());
    }
    assert!(worst < 1e-3, "the render differs from the model by {worst}");
}

// Spec: pitch-and-frequency-shifter R10
#[test]
fn the_synced_delays_repeat_lands_on_its_frame() {
    let mut impulse = vec![0.0f32; 160_000];
    impulse[0] = 1.0;
    // Pitch mode with no shift is a plain delay of the latency.
    for (bpm, repeat) in [(120.0, 24_000), (90.0, 32_000)] {
        let params = [
            (DELAY_ON, 1.0),
            (DELAY_MODE, 1.0),
            (DELAY_DIVISION, 8.0),
            (FEEDBACK, 50.0),
        ];
        let tempo = TempoMap::constant(Bpm::new(bpm).unwrap());
        let mut node = veer(&params, 4096);
        let out = play(node.as_mut(), &impulse, 4096, &tempo);
        let at = |frame: usize| out[0][frame];
        assert_eq!(at(LATENCY), 1.0, "the dry impulse");
        assert_eq!(at(LATENCY + repeat), 1.0, "{bpm} BPM: the first repeat");
        assert_eq!(at(LATENCY + 2 * repeat), 0.5, "the second, at half");
        assert_eq!(at(LATENCY + repeat - 1), 0.0);
        assert_eq!(at(LATENCY + repeat + 1), 0.0);
    }

    // Free: 4 Hz is a quarter of a second.
    let params = [(DELAY_ON, 1.0), (DELAY_HZ, 4.0)];
    let mut node = veer(&params, 4096);
    let out = stream(node.as_mut(), &impulse[..40_000], 4096);
    assert_eq!(out[0][LATENCY + 12_000], 1.0);
}

// Spec: pitch-and-frequency-shifter R10
#[test]
fn tone_cuts_the_highs_of_the_repeats_beyond_the_first() {
    let mut burst = vec![0.0f32; 120_000];
    for (n, v) in common::noise(2_000).into_iter().enumerate() {
        burst[n] = v;
    }
    let render = |tone: f64| {
        let params = [
            (DELAY_ON, 1.0),
            (DELAY_MODE, 1.0),
            (DELAY_DIVISION, 5.0),
            (FEEDBACK, 80.0),
            (TONE, tone),
        ];
        let mut node = veer(&params, 4096);
        stream(node.as_mut(), &burst, 4096).remove(0)
    };
    let (open, dark) = (render(0.0), render(100.0));
    // 1/8 at 120 BPM is 12,000 frames. The repeats sit at that many frames
    // past the dry burst, each 2,000 long.
    let band = |signal: &[f32], repeat: usize| -> f64 {
        let from = LATENCY + repeat * 12_000;
        let stretch = &signal[from..from + 2_000];
        let (mut high, mut all) = (0.0, 1e-12);
        for pair in stretch.windows(2) {
            let rise = f64::from(pair[1] - pair[0]);
            high += rise * rise;
            all += f64::from(pair[1]) * f64::from(pair[1]);
        }
        high / all
    };
    assert!(
        (band(&open, 1) - band(&dark, 1)).abs() < 1e-9,
        "the first repeat doesn't take the feedback path"
    );
    let (second_open, second_dark) = (band(&open, 2), band(&dark, 2));
    assert!(
        second_dark < 0.5 * second_open,
        "the second repeat is darker: {second_dark} against {second_open}"
    );
}

/// A note-on of `key` on frame `offset`.
fn on(offset: u32, key: u8) -> NoteEvent {
    common::note_on(offset, key, 1.0, None)
}

fn off(offset: u32, key: u8) -> NoteEvent {
    NoteEvent {
        kind: NoteKind::Off,
        ..common::note_on(offset, key, 0.0, None)
    }
}

fn bend(offset: u32, value: f32) -> NoteEvent {
    NoteEvent {
        kind: NoteKind::PitchBend { value },
        ..common::note_on(offset, 0, 0.0, None)
    }
}

// Spec: routing R32; pitch-and-frequency-shifter R11
#[test]
fn with_midi_pitch_a_note_sets_the_shift_to_its_interval_from_c3_on_its_frame() {
    let frames = 144_000;
    let mut node = veer(&[(PITCH_SOURCE, 1.0)], frames);
    // Key 67 is seven semitones above the reference, 60.
    let out = run_with_notes(node.as_mut(), &sine(1_000.0, frames), &[on(72_000, 67)]);
    // Before the note there is no shift.
    let plain = crossing_hz(&out[0][LATENCY + 2_000..72_000]);
    assert!((plain - 1_000.0).abs() < 0.5, "no note, no shift: {plain}");
    let heard = peak_hz(&out[0][72_000 + 9_600..72_000 + 9_600 + 48_000]);
    assert!(
        cents(heard, 1_000.0 * (7.0f64 / 12.0).exp2()).abs() < 2.0,
        "{heard} Hz"
    );
    // The shift starts on the note's frame: the shifted tone's periods are
    // 32 frames, the plain tone's 48.
    let mut crossings = Vec::new();
    for i in 71_000..73_000 {
        if out[0][i - 1] < 0.0 && out[0][i] >= 0.0 {
            crossings.push(i);
        }
    }
    let spacing: Vec<usize> = crossings.windows(2).map(|w| w[1] - w[0]).collect();
    let first_short = crossings
        .windows(2)
        .find(|w| w[1] - w[0] < 40)
        .map(|w| w[0])
        .expect("the shifted tone");
    assert!(spacing.contains(&48), "plain before: {spacing:?}");
    assert!(
        first_short.abs_diff(72_000) <= 80,
        "the shift starts at {first_short}, the note's frame is 72,000"
    );
}

// Spec: pitch-and-frequency-shifter R11
#[test]
fn glide_reaches_the_new_pitch_in_its_time() {
    let frames = 96_000;
    let params = [(PITCH_SOURCE, 1.0), (GLIDE, 500.0)];
    let mut node = veer(&params, frames);
    // Up an octave from C3 at frame 24,000, over 500 ms (24,000 frames).
    let out = run_with_notes(
        node.as_mut(),
        &sine(1_000.0, frames),
        &[on(0, 60), on(24_000, 72)],
    );
    let ratio_at = |from: usize| crossing_hz(&out[0][from..from + 2_400]) / 1_000.0;
    let halfway = 24_000 + LATENCY + 12_000 - 1_200;
    let expected = (6.0f64 / 12.0).exp2();
    assert!(
        (ratio_at(halfway) - expected).abs() < 0.03 * expected,
        "halfway through the glide: {}",
        ratio_at(halfway)
    );
    // 5 % past its time it has arrived; 5 % before, it hasn't.
    let arrived = 24_000 + LATENCY + 25_200 + 2_400;
    assert!(
        (ratio_at(arrived) - 2.0).abs() < 0.03,
        "{}",
        ratio_at(arrived)
    );
    let early = 24_000 + LATENCY + 22_800 - 2_400;
    assert!(ratio_at(early) < 1.97, "{}", ratio_at(early));
}

// Spec: pitch-and-frequency-shifter R11
#[test]
fn pitch_bend_covers_its_range() {
    let frames = 48_000;
    for (range, value, semitones) in [
        (12.0, 1.0f32, 12.0f64),
        (12.0, 0.5, 6.0),
        (24.0, -1.0, -24.0),
    ] {
        let params = [(PITCH_SOURCE, 1.0), (BEND_RANGE, range)];
        let mut node = veer(&params, frames);
        let out = run_with_notes(node.as_mut(), &sine(1_000.0, frames), &[bend(0, value)]);
        let heard = peak_hz(&out[0][4_800..]);
        let expected = 1_000.0 * (semitones / 12.0f64).exp2();
        assert!(
            cents(heard, expected).abs() < 3.0,
            "{value} of {range}: {heard} Hz"
        );
    }
}

// Spec: pitch-and-frequency-shifter R11
#[test]
fn the_newest_held_note_sounds_and_releasing_it_returns_to_the_one_before() {
    let frames = 144_000;
    let mut node = veer(&[(PITCH_SOURCE, 1.0)], frames);
    let notes = [on(0, 67), on(48_000, 72), off(96_000, 72)];
    let out = run_with_notes(node.as_mut(), &sine(1_000.0, frames), &notes);
    for (from, key) in [(12_000, 67), (60_000, 72), (108_000, 67)] {
        let heard = peak_hz(&out[0][from..from + 24_000]);
        let expected = 1_000.0 * ((f64::from(key) - 60.0) / 12.0).exp2();
        assert!(cents(heard, expected).abs() < 5.0, "key {key}: {heard}");
    }
}

// Spec: pitch-and-frequency-shifter R12
#[test]
fn renders_are_the_same_at_every_block_size() {
    let input = common::noise(24_000);
    let cases: [&[(ParamId, f64)]; 4] = [
        &[
            (COARSE, 5.0),
            (FINE, 13.0),
            (SPREAD, 40.0),
            (DELAY_ON, 1.0),
            (FEEDBACK, 40.0),
            (TONE, 30.0),
            (LFO_AMOUNT, 30.0),
            (ENV_ON, 1.0),
            (ENV_HZ, 100.0),
            (DRY_WET, 70.0),
        ],
        &[
            (MODE, 1.0),
            (SHIFT, 123.0),
            (SPREAD, 40.0),
            (LFO_AMOUNT, 30.0),
            (LFO_WAVE, 9.0),
            (ENV_ON, 1.0),
            (ENV_HZ, 100.0),
        ],
        &[
            (MODE, 2.0),
            (SHIFT, 80.0),
            (DRIVE, 50.0),
            (DELAY_ON, 1.0),
            (DELAY_MODE, 1.0),
            (FEEDBACK, 50.0),
        ],
        &[(PITCH_SOURCE, 1.0), (GLIDE, 20.0), (BEND_RANGE, 7.0)],
    ];
    for params in cases {
        let mut first: Option<Vec<Vec<f32>>> = None;
        for block in BLOCKS {
            let mut node = veer(params, 4096);
            let out = stream(node.as_mut(), &input, block);
            match &first {
                None => first = Some(out),
                Some(first) => assert_eq!(&out, first, "blocks of {block}"),
            }
        }
    }
}

// Spec: pitch-and-frequency-shifter R7
#[test]
fn the_sample_rate_sets_the_latency_in_frames() {
    let device = common::device(&shifter_type(), &[]);
    for (rate, expected) in [
        (SampleRate::HZ_44100, 441),
        (SampleRate::HZ_48000, 480),
        (SampleRate::HZ_96000, 960),
    ] {
        let node = registry()
            .create(
                &device,
                &bl_engine::NodeConfig {
                    round_trip_latency: 0,
                    sample_rate: rate,
                    max_frames: 256,
                    channels: 2,
                },
            )
            .unwrap();
        assert_eq!(node.latency(), expected);
    }
}

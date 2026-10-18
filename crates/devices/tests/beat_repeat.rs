//! Stutter, the equivalent of Beat Repeat (WP-5.26; the beat-repeat spec):
//! captures land on the song's beat grid through the tempo map, the slice
//! repeats for the gate's length, the three modes mix the input and the
//! repeats, Chance and Variation draw from a seeded source, and a render is
//! the same at any block size.

bl_rt::install_trap!();

mod common;

use bl_core::time::{Bpm, Frames, TempoMap, TempoSegment, Ticks};
use bl_devices::beat_repeat::{
    CHANCE, DECAY, FILTER, FREQ, GATE, GRID, INTERVAL, MODE, NO_TRIPLETS, OFFSET, PITCH,
    PITCH_DECAY, REPEAT, VARIATION, VARIATION_MODE, VOLUME, WIDTH, beat_repeat_type,
};
use bl_engine::{NodeEvent, Registry, TransportInfo};
use bl_model::ParamId;
use common::{CHANNELS, RATE, device, node, run_at, set};

fn registry() -> Registry {
    let mut registry = Registry::new();
    bl_devices::register(&mut registry);
    registry
}

fn constant(bpm: f64) -> TempoMap {
    TempoMap::constant(Bpm::new(bpm).expect("a tempo in range"))
}

/// 120 BPM for a quarter note, then 90.
fn changing() -> TempoMap {
    TempoMap::new([
        TempoSegment::Constant {
            start: Ticks::ZERO,
            bpm: Bpm::new(120.0).unwrap(),
        },
        TempoSegment::Constant {
            start: Ticks::from_quarters(1),
            bpm: Bpm::new(90.0).unwrap(),
        },
    ])
    .unwrap()
}

/// A ramp counting from 1: the sample on frame `n` is `n + 1`, so the
/// frame a captured sample came from reads off the output.
fn ramp(frames: usize) -> Vec<f32> {
    (0..frames).map(|n| n as f32 + 1.0).collect()
}

/// A parameter change on frame `at` of a render.
type Change = (usize, ParamId, f64);

/// Renders `input` (the same on both channels) through Stutter with
/// `params` in blocks of `block` frames, while the song plays from frame 0
/// (or stands still at 0), applying each of `changes` on its frame. Returns
/// the left channel and the right.
fn render(
    params: &[(ParamId, f64)],
    changes: &[Change],
    input: &[f32],
    block: usize,
    tempo: &TempoMap,
    playing: bool,
) -> Vec<Vec<f32>> {
    let mut stutter = node(&registry(), &device(&beat_repeat_type(), params), block);
    let mut out: Vec<Vec<f32>> = (0..CHANNELS)
        .map(|_| Vec::with_capacity(input.len()))
        .collect();
    let mut done = 0;
    while done < input.len() {
        let length = block.min(input.len() - done);
        let events: Vec<NodeEvent> = changes
            .iter()
            .filter(|(at, ..)| (done..done + length).contains(at))
            .map(|(at, param, value)| set((*at - done) as u32, *param, *value))
            .collect();
        let chunk = input[done..done + length].to_vec();
        let transport = TransportInfo {
            playing,
            position: Frames(if playing { done as i64 } else { 0 }),
        };
        let blocks = run_at(
            stutter.as_mut(),
            &vec![chunk; CHANNELS],
            &events,
            tempo,
            transport,
        );
        for (channel, data) in out.iter_mut().zip(blocks) {
            channel.extend(data);
        }
        done += length;
    }
    out
}

/// The frame of song tick `ticks`.
fn frame(tempo: &TempoMap, ticks: Ticks) -> usize {
    tempo.ticks_to_frames(ticks, RATE).get() as usize
}

/// Gate mode, a sixteenth-note grid, one bar's interval, offset 8/16: a
/// capture on beat three of each bar.
fn beat_three() -> Vec<(ParamId, f64)> {
    vec![
        (INTERVAL, 5.0),
        (OFFSET, 8.0),
        (GRID, 4.0),
        (GATE, 4.0),
        (MODE, 2.0),
    ]
}

// Spec: beat-repeat R1
#[test]
fn stutter_is_described_with_every_parameter_in_its_layout_and_no_latency() {
    let device_type = beat_repeat_type();
    device_type.description.check().unwrap();
    assert_eq!(device_type.name, "Stutter");
    assert_eq!(device_type.description.params.len(), 17);
    assert!(device_type.description.menu_options.is_empty());
    assert_eq!(device_type.description.param(VOLUME).unwrap().default, 1.0);
    let stutter = node(&registry(), &device(&device_type, &[]), 64);
    assert_eq!(stutter.latency(), 0);
}

// Spec: beat-repeat R2, R3
#[test]
fn a_capture_starts_on_beat_three_at_two_tempos_and_across_a_tempo_change() {
    for tempo in [constant(120.0), constant(90.0), changing()] {
        let input = ramp(frame(&tempo, Ticks::from_quarters(7)));
        let out = render(&beat_three(), &[], &input, 4096, &tempo, true);
        for bar in 0..2 {
            let beat_three = Ticks::from_quarters(4 * bar + 2);
            let start = frame(&tempo, beat_three);
            let slice = frame(&tempo, beat_three + Ticks::SIXTEENTH) - start;
            // Silent through the capture, then the slice from the capture's
            // first frame: the sample there is the frame it was taken on.
            assert_eq!(out[0][start + slice - 1], 0.0, "bar {bar}: capture");
            assert_eq!(
                out[0][start + slice] as usize,
                start + 1,
                "bar {bar}: the capture starts on beat three's frame"
            );
            assert_eq!(out[1][start + slice], out[0][start + slice]);
        }
        // Nothing else repeats: before the first capture it is silent.
        assert!(
            out[0][..frame(&tempo, Ticks::from_quarters(2))]
                .iter()
                .all(|s| *s == 0.0)
        );
    }
}

// Spec: beat-repeat R4
#[test]
fn the_captured_slice_repeats_for_gates_length() {
    let tempo = constant(120.0);
    let start = frame(&tempo, Ticks::from_quarters(2));
    let slice = 6_000;
    let input = ramp(start + 5 * slice);
    let out = render(&beat_three(), &[], &input, 512, &tempo, true);
    let (silent, repeats) = out[0][start..].split_at(slice);
    assert!(silent.iter().all(|s| *s == 0.0), "the capture is silent");
    // Gate 4/16 is one beat: the capture and three repeats, then it ends.
    for repeat in 0..3 {
        for j in 0..slice {
            assert_eq!(
                repeats[repeat * slice + j],
                input[start + j],
                "repeat {repeat}, frame {j}"
            );
        }
    }
    assert!(
        repeats[3 * slice..].iter().all(|s| *s == 0.0),
        "silent when the gate ends"
    );
}

// Spec: beat-repeat R4
#[test]
fn a_span_ends_when_the_song_stops() {
    let tempo = constant(120.0);
    let params = beat_three();
    let mut stutter = node(&registry(), &device(&beat_repeat_type(), &params), 16_384);
    let input = ramp(16_384);
    let mut block = |position: i64, playing: bool| {
        run_at(
            stutter.as_mut(),
            &vec![input.clone(); CHANNELS],
            &[],
            &tempo,
            TransportInfo {
                playing,
                position: Frames(position),
            },
        )
    };
    // The capture starts on frame 48,000 and would repeat from 54,000.
    block(40_000, true);
    let stopped = block(56_384, false);
    assert!(stopped[0].iter().all(|s| *s == 0.0), "no repeats");
}

// Spec: beat-repeat R10
#[test]
fn insert_mutes_the_input_while_repeating_gate_never_passes_it_and_mix_passes_both() {
    let tempo = constant(120.0);
    let start = frame(&tempo, Ticks::from_quarters(2));
    let slice = 6_000;
    let input = ramp(start + 5 * slice);
    let repeating = start + slice..start + 4 * slice;
    let repeat_at = |n: usize| input[start + (n - start - slice) % slice];
    for (mode, name) in [(0.0, "Mix"), (1.0, "Insert"), (2.0, "Gate")] {
        let mut params = beat_three();
        params.retain(|(id, _)| *id != MODE);
        params.push((MODE, mode));
        let out = render(&params, &[], &input, 4096, &tempo, true);
        for n in 0..input.len() {
            let dry = input[n];
            let want = if repeating.contains(&n) {
                match mode as u32 {
                    0 => dry + repeat_at(n),
                    1 => repeat_at(n),
                    _ => repeat_at(n),
                }
            } else {
                match mode as u32 {
                    2 => 0.0,
                    _ => dry,
                }
            };
            assert_eq!(out[0][n], want, "{name}, frame {n}");
        }
    }
}

/// A constant 0.5 through Gate mode, every second sixteenth note a capture
/// of a thirty-second: the number of captures that repeated in `intervals`
/// intervals of a sixteenth note.
fn captures_in(intervals: usize, chance: f64) -> usize {
    let tempo = constant(120.0);
    let params = [
        (INTERVAL, 1.0),
        (GRID, 6.0),
        (GATE, 1.0),
        (CHANCE, chance),
        (MODE, 2.0),
    ];
    let input = vec![0.5f32; intervals * 6_000];
    let out = render(&params, &[], &input, 4_096, &tempo, true);
    out[0]
        .windows(2)
        .filter(|pair| pair[0] == 0.0 && pair[1] != 0.0)
        .count()
}

// Spec: beat-repeat R5
#[test]
fn chance_zero_never_repeats_and_fifty_per_cent_stays_within_the_binomial_bounds() {
    assert_eq!(captures_in(200, 0.0), 0);
    assert_eq!(captures_in(200, 100.0), 200, "every interval");
    // 1,000 intervals at 50 %: 500 captures, a standard deviation of 15.8,
    // and the spec allows four of them either way.
    let count = captures_in(1_000, 50.0);
    assert!((437..=563).contains(&count), "{count} captures");
    assert_eq!(count, captures_in(1_000, 50.0), "run after run");
}

// Spec: beat-repeat R7
#[test]
fn pitch_an_octave_down_plays_each_slice_at_half_speed() {
    let tempo = constant(120.0);
    let start = frame(&tempo, Ticks::from_quarters(2));
    let slice = 6_000;
    let input = ramp(start + 6 * slice);
    let mut params = beat_three();
    params.extend([(GATE, 8.0), (PITCH, 12.0)]);
    let out = render(&params, &[], &input, 4096, &tempo, true);
    let after = start + slice;
    for j in 0..2 * slice - 1 {
        // Half speed on a ramp: frame j reads the captured ramp at j / 2.
        let want = input[start] + j as f32 / 2.0;
        assert_eq!(out[0][after + j], want, "frame {j}");
    }
}

// Spec: beat-repeat R7
#[test]
fn pitch_decay_plays_each_repeat_lower_than_the_one_before() {
    let tempo = constant(120.0);
    let start = frame(&tempo, Ticks::from_quarters(2));
    let slice = 6_000;
    let input = ramp(start + 6 * slice);
    let mut params = beat_three();
    params.extend([(GATE, 8.0), (PITCH_DECAY, 100.0)]);
    let out = render(&params, &[], &input, 4096, &tempo, true);
    // The first repeat is at pitch 0, the second two semitones down, the
    // third four: each one's speed on the ramp is its slope.
    let mut edges = vec![start + slice];
    edges.extend(falls(&out[0], start + slice));
    for repeat in 0..3 {
        let middle = (edges[repeat] + edges[repeat + 1]) / 2;
        let slope = f64::from(out[0][middle + 1_000] - out[0][middle]) / 1_000.0;
        let speed = (-2.0 * repeat as f64 / 12.0).exp2();
        assert!((slope - speed).abs() < 0.001, "repeat {repeat}: {slope}");
    }
}

// Spec: beat-repeat R8
#[test]
fn decay_lowers_each_repeat_by_its_step() {
    let tempo = constant(120.0);
    let start = frame(&tempo, Ticks::from_quarters(2));
    let slice = 6_000;
    let input = vec![0.5f32; start + 8 * slice];
    let mut params = beat_three();
    // 50 % is 3 dB for each repeat.
    params.extend([(GATE, 8.0), (DECAY, 50.0)]);
    let out = render(&params, &[], &input, 4096, &tempo, true);
    let level = |repeat: usize| f64::from(out[0][start + slice * (1 + repeat) + 100]);
    for repeat in 0..4 {
        let db = 20.0 * (level(repeat + 1) / level(repeat)).log10();
        assert!((db + 3.0).abs() < 0.01, "repeat {repeat}: {db} dB");
    }
}

// Spec: beat-repeat R6
#[test]
fn no_triplets_keeps_the_grid_binary() {
    let tempo = constant(120.0);
    let start = frame(&tempo, Ticks::from_quarters(2));
    let input = ramp(start + 30_000);
    // Grid 1/12 is a third of a quarter note, 8,000 frames; without the
    // triplet it plays as 1/16, 6,000.
    for (no_triplets, slice) in [(0.0, 8_000), (1.0, 6_000)] {
        let mut params = beat_three();
        params.extend([(GRID, 3.0), (GATE, 16.0), (NO_TRIPLETS, no_triplets)]);
        let out = render(&params, &[], &input, 4096, &tempo, true);
        assert_eq!(out[0][start + slice - 1], 0.0);
        assert_eq!(out[0][start + slice], input[start], "{no_triplets}");
    }
}

/// The frames after `from` where a render of Stutter in Gate mode over a
/// ramp falls back: where a slice starts again.
fn falls(out: &[f32], from: usize) -> Vec<usize> {
    (from + 1..out.len())
        .filter(|n| out[*n] < out[*n - 1] && out[*n] != 0.0)
        .collect()
}

/// The lengths of the slices of the repeats, from the end of the capture.
fn slice_lengths(out: &[f32], from: usize) -> Vec<usize> {
    let mut last = from;
    falls(out, from)
        .into_iter()
        .map(|n| {
            let length = n - last;
            last = n;
            length
        })
        .collect()
}

// Spec: beat-repeat R9
#[test]
fn variation_zero_keeps_the_grid_and_each_mode_varies_it_at_its_moments() {
    let tempo = constant(120.0);
    let start = frame(&tempo, Ticks::from_quarters(2));
    let input = ramp(start + 200_000);
    let long = |mode: f64, variation: f64| {
        let mut params = beat_three();
        params.extend([
            (INTERVAL, 7.0),
            (OFFSET, 8.0),
            (GATE, 32.0),
            (VARIATION, variation),
            (VARIATION_MODE, mode),
        ]);
        render(&params, &[], &input, 4096, &tempo, true)
    };
    // Fixed: every repeat is a 1/16, 6,000 frames.
    let fixed = long(4.0, 0.0);
    let lengths = slice_lengths(&fixed[0], start + 6_000);
    assert!(lengths.len() > 20);
    assert!(lengths.iter().all(|l| *l == 6_000), "{lengths:?}");
    // At 100 %, Trigger varies the capture's size once: every repeat of
    // the span has the one new size; the others vary the repeats.
    let trigger = long(0.0, 100.0);
    let first = trigger[0]
        .iter()
        .position(|s| *s != 0.0)
        .expect("a capture repeated");
    let size = first - start;
    assert!(
        [24_000, 16_000, 12_000, 8_000, 6_000, 4_000, 3_000].contains(&size),
        "{size}"
    );
    let lengths = slice_lengths(&trigger[0], first);
    assert!(lengths.iter().all(|l| *l == size), "{lengths:?}");
    for mode in 1..=4 {
        let varied = long(f64::from(mode), 100.0);
        let lengths = slice_lengths(&varied[0], start + 6_000);
        let mut distinct = lengths.clone();
        distinct.sort_unstable();
        distinct.dedup();
        assert!(distinct.len() >= 2, "mode {mode}: {lengths:?}");
        assert_eq!(varied, long(f64::from(mode), 100.0), "run after run");
    }
}

// Spec: beat-repeat R11
#[test]
fn the_repeat_button_captures_and_repeats_at_once_until_it_is_off() {
    // The song is stopped, the interval would never repeat (Chance 0), and
    // Gate is one sixteenth: Repeat ignores all three.
    let tempo = constant(120.0);
    let input = ramp(40_000);
    let params = [(CHANCE, 0.0), (GATE, 1.0), (GRID, 4.0), (MODE, 2.0)];
    let changes = [(1_000, REPEAT, 1.0), (20_000, REPEAT, 0.0)];
    let out = render(&params, &changes, &input, 512, &tempo, false);
    assert!(out[0][..7_000].iter().all(|s| *s == 0.0));
    for n in 7_000..20_000 {
        assert_eq!(out[0][n], input[1_000 + (n - 7_000) % 6_000], "frame {n}");
    }
    assert!(out[0][20_000..].iter().all(|s| *s == 0.0), "off");
}

/// A sine of `hz` at 0.5.
fn sine(hz: f64, frames: usize) -> Vec<f32> {
    (0..frames)
        .map(|n| (0.5 * (std::f64::consts::TAU * hz * n as f64 / 48_000.0).sin()) as f32)
        .collect()
}

fn rms(samples: &[f32]) -> f64 {
    (samples.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / samples.len() as f64).sqrt()
}

// Spec: beat-repeat R12
#[test]
fn the_filter_passes_its_band_and_cuts_what_lies_outside_it() {
    let tempo = constant(120.0);
    let level = |hz: f64, filter: bool| {
        let params = [
            (GRID, 0.0),
            (MODE, 2.0),
            (FILTER, f64::from(u8::from(filter))),
            (FREQ, 1_000.0),
            (WIDTH, 2.0),
        ];
        let changes = [(0, REPEAT, 1.0)];
        let out = render(&params, &changes, &sine(hz, 72_000), 4096, &tempo, false);
        // The repeats begin when the first capture (a quarter note) ends.
        rms(&out[0][36_000..72_000])
    };
    let open = level(1_000.0, false);
    assert!((open - 0.5 / 2f64.sqrt()).abs() < 0.01, "{open}");
    let db = |hz| 20.0 * (level(hz, true) / open).log10();
    assert!(db(1_000.0).abs() < 1.0, "{} dB in the band", db(1_000.0));
    assert!(db(100.0) < -20.0, "{} dB a decade below", db(100.0));
    assert!(db(10_000.0) < -20.0, "{} dB a decade above", db(10_000.0));
}

// Spec: beat-repeat R13
#[test]
fn renders_are_bit_identical_at_block_sizes_1_64_512_and_4096_and_run_after_run() {
    let tempo = changing();
    let params = [
        (INTERVAL, 3.0),
        (OFFSET, 1.0),
        (CHANCE, 60.0),
        (GATE, 6.0),
        (GRID, 3.0),
        (VARIATION, 80.0),
        (VARIATION_MODE, 4.0),
        (PITCH, 5.0),
        (PITCH_DECAY, 40.0),
        (FILTER, 1.0),
        (FREQ, 2_000.0),
        (DECAY, 30.0),
        (MODE, 1.0),
    ];
    // Parameters move mid-stream: a mode, the pitch, the gain, the button.
    let changes = [
        (20_000, MODE, 0.0),
        (31_111, PITCH, 12.0),
        (47_777, VOLUME, 0.5),
        (60_000, REPEAT, 1.0),
        (70_000, REPEAT, 0.0),
        (85_000, VARIATION_MODE, 1.0),
        (90_000, FREQ, 500.0),
    ];
    let input = common::noise(120_000);
    let reference = render(&params, &changes, &input, 4096, &tempo, true);
    assert!(reference[0] != input, "the device did something");
    assert_eq!(
        reference,
        render(&params, &changes, &input, 4096, &tempo, true)
    );
    for block in [1, 64, 512] {
        assert!(
            reference == render(&params, &changes, &input, block, &tempo, true),
            "block size {block}"
        );
    }
}

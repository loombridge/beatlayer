//! Gated Delay (WP-8.51; the gated-delay spec): the gate opens on its steps'
//! frames through the tempo map at any block size, with a 2 ms ramp; echoes
//! land Delay Steps later; Random's draws follow the seed and the song's
//! bar; Dry on Rests, Input at silence, and renders that repeat at four
//! block sizes. Every block runs as a real-time section under the
//! allocation trap.

use crate::common;
use bl_core::id::Id;
use bl_core::time::{Bpm, Frames, TICKS_PER_QUARTER, TempoMap, TempoSegment, Ticks};
use bl_devices::gated_delay::{
    self, DELAY_STEPS, DRY_ON_RESTS, DRY_WET, FEEDBACK, INPUT, RANDOM, RATE, RATES, STEP_COUNT,
    STEPS, gated_delay_type, step,
};
use bl_engine::{Block, Registry, TransportInfo};
use bl_model::{DeviceId, ParamId};
use common::{CHANNELS, RATE as SAMPLE_RATE, node};

const BLOCKS: [usize; 4] = [1, 64, 512, 4096];
/// A bar of four quarter notes, in ticks.
const BAR: i64 = 4 * TICKS_PER_QUARTER;
/// The gate's ramp, in frames: 2 ms at 48 kHz.
const RAMP: f64 = 96.0;
/// The steps a new device has on, from 1.
const ON: [usize; 5] = [1, 4, 7, 11, 14];

fn registry() -> Registry {
    let mut registry = Registry::empty();
    gated_delay::register(&mut registry);
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

fn frame(tempo: &TempoMap, ticks: i64) -> usize {
    tempo.ticks_to_frames(Ticks(ticks), SAMPLE_RATE).get() as usize
}

/// `input` on both channels through Gated Delay `id` with `params`, in
/// blocks of `block`, the song playing from frame `start` under `tempo`.
fn render_as(
    id: u64,
    params: &[(ParamId, f64)],
    input: &[f32],
    block: usize,
    tempo: &TempoMap,
    (start, playing): (i64, bool),
) -> Vec<Vec<f32>> {
    let mut device = common::device(&gated_delay_type(), params);
    device.id = DeviceId(Id::new(id).expect("an ID"));
    let mut gated = node(&registry(), &device, block);
    let mut out = vec![Vec::with_capacity(input.len()); CHANNELS];
    for from in (0..input.len()).step_by(block) {
        let to = (from + block).min(input.len());
        let mut data: Vec<f32> = (0..CHANNELS)
            .flat_map(|_| input[from..to].iter().copied())
            .collect();
        {
            let transport = TransportInfo {
                playing,
                position: Frames(if playing { start + from as i64 } else { start }),
                ..TransportInfo::default()
            };
            let mut block = Block::new(
                &mut data,
                CHANNELS,
                to - from,
                to - from,
                &[],
                transport,
                SAMPLE_RATE,
            )
            .with_tempo(tempo);
            bl_rt::rt_test!({ gated.process(&mut block) });
        }
        for (channel, values) in out.iter_mut().zip(data.chunks(to - from)) {
            channel.extend_from_slice(values);
        }
    }
    out
}

fn render(
    params: &[(ParamId, f64)],
    input: &[f32],
    block: usize,
    tempo: &TempoMap,
) -> Vec<Vec<f32>> {
    render_as(1, params, input, block, tempo, (0, true))
}

/// With Dry/Wet at 0 % and Dry on Rests on, a steady input comes out as
/// one minus the gate: the gate's level, frame by frame.
fn gate_of(out: &[f32]) -> Vec<f64> {
    out.iter().map(|s| 1.0 - f64::from(*s)).collect()
}

fn steady() -> Vec<(ParamId, f64)> {
    vec![(DRY_WET, 0.0)]
}

/// Checks `gate` against the steps `on` (from 0), `steps` of them to a
/// pass, each `ticks` long, for `bars` bars under `tempo`: each step's
/// level starts moving on the step's frame and settles within the ramp.
fn check_gate(gate: &[f64], tempo: &TempoMap, on: &[bool], steps: usize, ticks: i64, bars: i64) {
    let slew = 1.0 / RAMP;
    let mut before = 0.0f64;
    for bar in 0..bars {
        let mut k = 0;
        while k * ticks < BAR {
            let from = frame(tempo, bar * BAR + k * ticks);
            let target = f64::from(u8::from(on[k as usize % steps]));
            if from >= gate.len() {
                return;
            }
            if from > 0 {
                assert!(
                    (gate[from - 1] - before).abs() < 1e-6,
                    "bar {bar} step {k}: settled at {before} before frame {from}, not {}",
                    gate[from - 1]
                );
            }
            let first = before + (target - before).clamp(-slew, slew);
            assert!(
                (gate[from] - first).abs() < 1e-6,
                "bar {bar} step {k}: the ramp begins on frame {from}: {} not {first}",
                gate[from]
            );
            before = target;
            k += 1;
        }
    }
}

fn on_pattern(overrides: &[(usize, bool)]) -> Vec<bool> {
    let mut on: Vec<bool> = (1..=STEP_COUNT).map(|n| ON.contains(&n)).collect();
    for (n, value) in overrides {
        on[*n - 1] = *value;
    }
    on
}

// Spec: gated-delay R1, R2, R11
#[test]
fn gated_delay_is_described_with_its_steps_in_the_layout_and_no_latency() {
    let device_type = gated_delay_type();
    device_type.description.check().unwrap();
    assert_eq!(device_type.name, "Gated Delay");
    assert_eq!(device_type.description.params.len(), 24);
    let default = |id: ParamId| device_type.description.param(id).unwrap().default;
    for n in 1..=STEP_COUNT {
        assert_eq!(default(step(n)) == 1.0, ON.contains(&n), "step {n}");
        assert_eq!(
            device_type.description.param(step(n)).unwrap().name,
            format!("Step {n}")
        );
    }
    assert_eq!(default(STEPS), 16.0);
    assert_eq!(RATES[default(RATE) as usize].0, "1/16");
    assert_eq!(default(DELAY_STEPS), 3.0);
    assert_eq!(default(FEEDBACK), 40.0);
    assert_eq!(default(DRY_ON_RESTS), 1.0);
    assert_eq!(default(DRY_WET), 50.0);
    let banks = device_type.description.banks();
    let names: Vec<_> = banks
        .iter()
        .map(|b| (b.name.as_str(), b.params.len()))
        .collect();
    assert_eq!(names, [("Main", 8), ("Steps 1–8", 8), ("Steps 9–16", 8)]);
    assert_eq!(
        node(&registry(), &common::device(&device_type, &[]), 64).latency(),
        0
    );
}

// Spec: gated-delay R2, R3
#[test]
fn the_gate_opens_on_its_steps_frames_through_a_tempo_change_at_four_block_sizes() {
    let input = vec![1.0; 100_000];
    let cases: [(&str, TempoMap, Vec<(ParamId, f64)>, usize, i64); 4] = [
        ("120 BPM", constant(120.0), steady(), 16, RATES[4].1),
        ("90 BPM", constant(90.0), steady(), 16, RATES[4].1),
        ("a tempo change", changing(), steady(), 16, RATES[4].1),
        (
            "1/8 triplets, 12 steps, over a tempo change",
            changing(),
            [steady(), vec![(RATE, 6.0), (STEPS, 12.0)]].concat(),
            12,
            RATES[6].1,
        ),
    ];
    for (name, tempo, params, steps, ticks) in cases {
        let on = on_pattern(&[]);
        let reference = render(&params, &input, 4096, &tempo);
        check_gate(&gate_of(&reference[0]), &tempo, &on, steps, ticks, 2);
        for block in &BLOCKS[..3] {
            assert_eq!(
                reference,
                render(&params, &input, *block, &tempo),
                "{name}, block {block}"
            );
        }
    }
}

// Spec: gated-delay R2
#[test]
fn step_one_begins_on_each_bar_line_whatever_the_rows_length() {
    // 12 steps of 1/16 leave a bar's last four to the row's second pass,
    // which begins again at step 1, and the next bar begins at step 1.
    let tempo = constant(120.0);
    let mut params = steady();
    params.extend([
        (STEPS, 12.0),
        (step(1), 1.0),
        (step(4), 0.0),
        (step(13), 1.0),
    ]);
    let out = render(&params, &vec![1.0; 100_000], 4096, &tempo);
    let gate = gate_of(&out[0]);
    // Step 4 is off, so the bar's sixteenth step (the row's fourth) is off
    // and the one before it (the row's third) follows step 3, which is off.
    let at = |k: i64, bar: i64| gate[frame(&tempo, bar * BAR + k * TICKS_PER_QUARTER / 4) + 300];
    assert!(at(0, 0) > 0.99, "step 1 opens the bar");
    assert!(at(12, 0) > 0.99, "the second pass begins at step 1");
    assert!(at(15, 0) < 0.01, "and reaches step 4, which is off");
    assert!(at(0, 1) > 0.99, "step 1 opens the next bar");
}

// Spec: gated-delay R3
#[test]
fn the_gate_ramps_over_two_milliseconds_so_no_click_is_heard() {
    let out = render(&steady(), &vec![1.0; 20_000], 4096, &constant(120.0));
    // Step 1 is on from frame 0: the dry signal ramps down over 96 frames.
    for i in 0..96 {
        let want = 1.0 - (i as f64 + 1.0) / RAMP;
        assert!((f64::from(out[0][i]) - want).abs() < 1e-5, "frame {i}");
    }
    assert!(out[0][96].abs() < 1e-6);
}

// Spec: gated-delay R2
#[test]
fn the_sequencer_runs_on_while_the_song_is_stopped() {
    let tempo = constant(120.0);
    let input = vec![1.0; 50_000];
    let playing = render(&steady(), &input, 512, &tempo);
    let stopped = render_as(1, &steady(), &input, 512, &tempo, (0, false));
    assert_eq!(playing, stopped, "counting on from where the song stands");
}

/// A click on frame `at`.
fn click(frames: usize, at: &[usize]) -> Vec<f32> {
    let mut input = vec![0.0; frames];
    for i in at {
        input[*i] = 1.0;
    }
    input
}

// Spec: gated-delay R4
#[test]
fn an_echo_lands_delay_steps_later_and_only_from_an_on_step() {
    let echoes = |bpm: f64, rate: usize| {
        let tempo = constant(bpm);
        let step_frames = frame(&tempo, RATES[rate].1) as usize;
        // A click inside step 1 (on) and one inside step 2 (off).
        let (on, off) = (step_frames / 2, step_frames * 3 / 2);
        let params = [
            (DRY_WET, 100.0),
            (FEEDBACK, 0.0),
            (RATE, rate as f64),
            (DELAY_STEPS, 3.0),
        ];
        let out = render(&params, &click(step_frames * 8, &[on, off]), 4096, &tempo);
        let heard: Vec<usize> = (0..out[0].len())
            .filter(|i| out[0][*i].abs() > 1e-6)
            .collect();
        (heard, on + 3 * step_frames, out[0][on + 3 * step_frames])
    };
    for (bpm, rate) in [(120.0, 4), (90.0, 4), (120.0, 7), (120.0, 1)] {
        let (heard, want, level) = echoes(bpm, rate);
        assert_eq!(heard, [want], "{bpm} BPM, rate {rate}");
        assert!((level - 1.0).abs() < 1e-6);
    }
}

// Spec: gated-delay R4
#[test]
fn feedback_repeats_the_echoes_on_whether_the_step_is_on_or_off() {
    let tempo = constant(120.0);
    // 6,000 frames a step: a click in step 1, echoes every three steps:
    // frames 21,000 (step 4, on), 39,000 (step 7, on), 57,000 (step 10,
    // off): the loop returns what the line holds whatever the gate does.
    let params = [(DRY_WET, 100.0), (FEEDBACK, 50.0)];
    let out = render(&params, &click(80_000, &[3_000]), 4096, &tempo);
    for (n, at) in [21_000, 39_000, 57_000, 75_000].into_iter().enumerate() {
        assert!(
            (f64::from(out[0][at]) - 0.5f64.powi(n as i32)).abs() < 1e-6,
            "echo {n}"
        );
    }
}

// Spec: gated-delay R5
#[test]
fn dry_on_rests_plays_the_dry_signal_on_the_off_steps_only() {
    let tempo = constant(120.0);
    let input = common::noise(60_000);
    let on_step = 3_000..5_900;
    let off_step = 6_300..11_900;
    let out = render(&steady(), &input, 4096, &tempo);
    assert!(
        out[0][on_step.clone()]
            .iter()
            .skip(200)
            .all(|s| s.abs() < 1e-6),
        "muted on an on step"
    );
    assert_eq!(
        out[0][off_step.clone()],
        input[off_step.clone()],
        "plays on an off step"
    );
    // Off, the dry signal plays throughout.
    let mut params = steady();
    params.push((DRY_ON_RESTS, 0.0));
    let out = render(&params, &input, 4096, &tempo);
    assert_eq!(out[0], input);
}

// Spec: gated-delay R3, R10
#[test]
fn input_reaches_silence_and_gains_up_to_twelve_decibels() {
    let tempo = constant(120.0);
    let input = common::noise(60_000);
    let echoes = [(DRY_WET, 100.0), (FEEDBACK, 0.0), (INPUT, 0.0)];
    let out = render(&echoes, &input, 4096, &tempo);
    assert!(out.iter().flatten().all(|s| *s == 0.0), "Input at -inf dB");
    let peak = |gain: f64| {
        let params = [(DRY_WET, 100.0), (FEEDBACK, 0.0), (INPUT, gain)];
        let out = render(&params, &input, 4096, &tempo);
        out[0].iter().fold(0.0f32, |m, s| m.max(s.abs()))
    };
    assert!((peak(10f64.powf(12.0 / 20.0)) / peak(1.0) - 3.98).abs() < 0.01);
}

/// The steps' levels settled in bar `bar`, one per step of 1/16, read 200
/// frames in.
fn settled(out: &[f32], tempo: &TempoMap, bar: i64) -> Vec<bool> {
    (0..16)
        .map(|k| {
            let at = frame(tempo, bar * BAR + k * TICKS_PER_QUARTER / 4) + 200;
            out[at] < 0.5
        })
        .collect()
}

// Spec: gated-delay R6
#[test]
fn random_turns_steps_over_by_the_seed_and_the_bar_so_a_render_repeats() {
    let tempo = constant(120.0);
    let bar = frame(&tempo, BAR);
    let input = vec![1.0; bar * 6];
    let mut params = steady();
    params.push((RANDOM, 50.0));
    let first = render(&params, &input, 4096, &tempo);
    let on = on_pattern(&[]);
    // Each bar turns some steps over, and not the same ones: a pass is a bar.
    let bars: Vec<Vec<bool>> = (0..5).map(|b| settled(&first[0], &tempo, b)).collect();
    let turned: Vec<usize> = bars
        .iter()
        .map(|b| b.iter().zip(&on).filter(|(a, b)| a != b).count())
        .collect();
    assert!(
        turned.iter().all(|n| *n > 0 && *n < 16),
        "turned over: {turned:?}"
    );
    assert!(
        bars.windows(2).any(|w| w[0] != w[1]),
        "each pass draws again"
    );
    // The same render again, and at other block sizes.
    for block in [64, 512] {
        assert_eq!(
            first,
            render(&params, &input, block, &tempo),
            "block {block}"
        );
    }
    // The same bar from another start draws the same steps: playing from
    // bar 3 gives bar 3's pattern, not the first bar's.
    let from_three = render_as(
        1,
        &params,
        &input[..bar * 2],
        4096,
        &tempo,
        (3 * bar as i64, true),
    );
    assert_eq!(settled(&from_three[0], &tempo, 0), bars[3]);
    // Another device's seed draws other steps.
    let other = render_as(2, &params, &input, 4096, &tempo, (0, true));
    assert!((0..5).any(|b| settled(&other[0], &tempo, b) != bars[b as usize]));
    // At 0 % none is turned over; at 100 % all are.
    params.push((RANDOM, 0.0));
    let none = render(&params, &input, 4096, &tempo);
    assert_eq!(settled(&none[0], &tempo, 2), on);
    params.push((RANDOM, 100.0));
    let all = render(&params, &input, 4096, &tempo);
    let inverse: Vec<bool> = on.iter().map(|o| !o).collect();
    assert_eq!(settled(&all[0], &tempo, 2), inverse);
}

// Spec: gated-delay R8
#[test]
fn renders_are_bit_identical_at_four_block_sizes_and_run_after_run() {
    let input = common::noise(120_000);
    let params = [
        (RATE, 3.0),
        (STEPS, 11.0),
        (DELAY_STEPS, 5.0),
        (FEEDBACK, 60.0),
        (RANDOM, 40.0),
        (DRY_WET, 70.0),
    ];
    for tempo in [constant(120.0), changing()] {
        let first = render(&params, &input, BLOCKS[3], &tempo);
        assert!(first[0].iter().any(|s| *s != 0.0));
        assert_eq!(
            first,
            render(&params, &input, BLOCKS[3], &tempo),
            "run after run"
        );
        for block in &BLOCKS[..3] {
            assert_eq!(
                first,
                render(&params, &input, *block, &tempo),
                "block {block}"
            );
        }
    }
}

// Spec: gated-delay R9
#[test]
fn the_step_row_publishes_its_steps_the_pass_and_the_step_playing() {
    let tempo = constant(120.0);
    let play = |random: f64, frames: usize| {
        let params = [(RANDOM, random)];
        let device = common::device(&gated_delay_type(), &params);
        let mut gated = node(&registry(), &device, frames);
        let mut reader = gated.display().expect("Gated Delay has a display");
        let mut data = vec![0.0f32; CHANNELS * frames];
        {
            let transport = TransportInfo {
                playing: true,
                position: Frames(0),
                ..TransportInfo::default()
            };
            let mut block = Block::new(
                &mut data,
                CHANNELS,
                frames,
                frames,
                &[],
                transport,
                SAMPLE_RATE,
            )
            .with_tempo(&tempo);
            bl_rt::rt_test!({ gated.process(&mut block) });
        }
        let shown = reader.read().values.clone();
        let description = gated_delay_type().description;
        gated_delay::display(device.id, &description, &device.params, Some(&shown))
    };
    // 7,000 frames in, step 2 of the row is playing.
    let display = play(0.0, 7_000);
    assert!(display.live);
    let switches = display.series("Steps").expect("the steps");
    let on = on_pattern(&[]);
    let want: Vec<f32> = on.iter().map(|o| f32::from(u8::from(*o))).collect();
    assert_eq!(switches.values, want);
    assert_eq!(display.series("Pass").unwrap().values, want);
    assert_eq!(display.series("Turned Over").unwrap().values, vec![0.0; 16]);
    let playing = display.reading("Playing Step").expect("the step playing");
    assert_eq!((playing.value, playing.text.as_str()), (2.0, "Step 2"));
    // At Random 100 % every step is turned over, and the pass is the
    // opposite of the switches.
    let display = play(100.0, 7_000);
    assert_eq!(display.series("Turned Over").unwrap().values, vec![1.0; 16]);
    let opposite: Vec<f32> = want.iter().map(|v| 1.0 - v).collect();
    assert_eq!(display.series("Pass").unwrap().values, opposite);
    assert_eq!(
        display.series("Steps").unwrap().values,
        want,
        "the switches stay"
    );
    // Before the engine has published, only the switches are there.
    let description = gated_delay_type().description;
    let device = common::device(&gated_delay_type(), &[(step(2), 1.0)]);
    let idle = gated_delay::display(device.id, &description, &device.params, None);
    assert!(!idle.live);
    assert_eq!(idle.series.len(), 1);
    assert_eq!(idle.series[0].values[1], 1.0);
}

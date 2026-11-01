//! Vector Delay, the multi-tap delay whose taps move as particles (WP-8.52;
//! the Vector Delay spec): the taps' places, the forces on the 32-frame
//! grid, the Doppler pitch of a moving tap, Reset and the transport start,
//! Feedback, Input and Dry/Wet, and what the display reads. Every block runs
//! as a real-time section under the allocation trap.

use crate::common;
use bl_core::id::Id;
use bl_core::time::{Bpm, Frames, TempoMap};
use bl_devices::vector_delay::{
    DISPLAY_LEN, DRY_WET, FEEDBACK, FLOW, FRICTION, GRAVITY, INPUT, MAGNET, MAX_TIME, RESET, SPEED,
    SPREAD, TAPS, VECTOR_DELAY, tap_x, tap_y, vector_delay_type,
};
use bl_engine::{DisplayReader, Node, Registry, TransportInfo};
use bl_model::{DeviceId, DeviceKind, OrdMap, ParamId};
use common::{node, noise, run_at, run_blocks, set};

const RATE: f64 = 48_000.0;
const MAX_FRAMES: usize = 200_000;

fn registry() -> Registry {
    let mut registry = Registry::new();
    bl_devices::register(&mut registry);
    registry
}

fn tempo() -> TempoMap {
    TempoMap::constant(Bpm::new(120.0).unwrap())
}

/// Vector Delay as device number `id`, fully wet, with `params`.
fn vector(id: u64, params: &[(ParamId, f64)]) -> Box<dyn Node> {
    let mut device = vector_delay_type().device(DeviceId(Id::new(id).unwrap()));
    device.params.insert(DRY_WET, 100.0);
    for (param, value) in params {
        device.params.insert(*param, *value);
    }
    node(&registry(), &device, MAX_FRAMES)
}

fn impulse(frames: usize) -> Vec<f32> {
    let mut input = vec![0.0f32; frames];
    input[0] = 1.0;
    input
}

/// A 1 kHz sine whose loudness rises.
fn tone(frames: usize) -> Vec<f32> {
    (0..frames)
        .map(|n| {
            let ramp = 0.1 + 0.8 * n as f64 / frames as f64;
            (ramp * (std::f64::consts::TAU * 1_000.0 * n as f64 / RATE).sin()) as f32
        })
        .collect()
}

/// Two taps at rest, 12,000 and 24,000 frames at the default Max Time, the
/// second at half the level, with nothing moving them.
fn two_taps() -> Vec<(ParamId, f64)> {
    vec![
        (TAPS, 2.0),
        (tap_x(0), 0.25),
        (tap_y(0), 1.0),
        (tap_x(1), 0.5),
        (tap_y(1), 0.5),
    ]
}

/// The sum of `signal` around `at`: what an echo adds up to however a
/// fraction of a frame spreads it.
fn around(signal: &[f32], at: usize) -> f32 {
    signal[at - 8..=at + 8].iter().sum()
}

/// Where the biggest sample of `signal[from..to]` is.
fn loudest(signal: &[f32], from: usize, to: usize) -> usize {
    (from..to)
        .max_by(|a, b| signal[*a].abs().total_cmp(&signal[*b].abs()))
        .unwrap()
}

// Spec: vector-delay R1, R2
#[test]
fn it_is_an_audio_effect_named_for_live_with_one_display_cell() {
    let t = vector_delay_type();
    assert_eq!(t.name, "Vector Delay");
    assert_eq!(t.role, bl_model::DeviceRole::AudioEffect);
    assert!(matches!(&t.kind, DeviceKind::Native { device_type } if device_type == VECTOR_DELAY));
    let layout = &t.description.layout;
    let cells = layout.place(layout.docked_size()).cells;
    assert_eq!(cells.len(), 1, "the particle field");
    let param = |id| t.description.param(id).unwrap();
    assert_eq!(param(TAPS).default, 4.0);
    assert_eq!(param(MAX_TIME).default, 1_000.0);
    assert_eq!(param(SPREAD).default, 50.0);
    assert_eq!(param(FEEDBACK).default, 30.0);
    assert_eq!(param(FRICTION).default, 20.0);
    assert_eq!(param(SPEED).default, 100.0);
    // Evenly spaced along the diagonal, the later ones lower.
    let places: Vec<(f64, f64)> = (0..8)
        .map(|n| (param(tap_x(n)).default, param(tap_y(n)).default))
        .collect();
    for n in 1..8 {
        assert!(places[n].0 > places[n - 1].0 && places[n].1 < places[n - 1].1);
        let step = places[n].0 - places[n - 1].0;
        assert!((step - (places[1].0 - places[0].0)).abs() < 1e-9);
    }
}

// Spec: vector-delay R2, R12
#[test]
fn taps_at_rest_land_on_max_time_times_their_place_at_their_level_and_pan() {
    let frames = 30_000;
    let mut node = vector(1, &[&two_taps()[..], &[(FEEDBACK, 0.0)]].concat());
    let out = run_at(
        node.as_mut(),
        &[impulse(frames), impulse(frames)],
        &[],
        &tempo(),
        TransportInfo::default(),
    );
    // Tap 1 pans left by Spread's 50 %, tap 2 right: the far side's share
    // is halved.
    for (at, left, right) in [(12_000, 1.0, 0.5), (24_000, 0.25, 0.5)] {
        assert!((around(&out[0], at) - left).abs() < 1e-3, "left at {at}");
        assert!((around(&out[1], at) - right).abs() < 1e-3, "right at {at}");
        assert!(loudest(&out[0], at - 100, at + 100).abs_diff(at) <= 1);
    }
    let quiet: f32 = out[0][..11_900].iter().map(|s| s.abs()).sum();
    assert_eq!(quiet, 0.0, "nothing before the first tap");
    // A tap at the far edge reaches the line's 4 seconds.
    let mut far = vector(
        1,
        &[
            (TAPS, 1.0),
            (tap_x(0), 1.0),
            (tap_y(0), 1.0),
            (MAX_TIME, 4_000.0),
            (FEEDBACK, 0.0),
            (SPREAD, 0.0),
        ],
    );
    let out = run_at(
        far.as_mut(),
        &[impulse(195_000), impulse(195_000)],
        &[],
        &tempo(),
        TransportInfo::default(),
    );
    assert!(loudest(&out[0], 1, 195_000).abs_diff(192_000) <= 1);
    assert!((around(&out[0], 192_000) - 1.0).abs() < 1e-3);
}

// Spec: vector-delay R2
#[test]
fn a_synced_max_time_follows_the_tempo() {
    use bl_devices::vector_delay::{DIVISION, SYNC, division_names};
    let quarter = division_names().iter().position(|n| *n == "1/4").unwrap();
    // A quarter at 120 BPM is 24,000 frames: tap 1 at x = 0.5 is 12,000.
    let mut node = vector(
        1,
        &[
            (TAPS, 1.0),
            (tap_x(0), 0.5),
            (tap_y(0), 1.0),
            (SYNC, 1.0),
            (DIVISION, quarter as f64),
            (FEEDBACK, 0.0),
            (SPREAD, 0.0),
        ],
    );
    let frames = 16_000;
    let out = run_at(
        node.as_mut(),
        &[impulse(frames), impulse(frames)],
        &[],
        &tempo(),
        TransportInfo::default(),
    );
    assert!(loudest(&out[0], 1, frames).abs_diff(12_000) <= 1);
}

/// What a run through blocks of the grid's length shows: the output and, after
/// each block, what the node published.
struct Run {
    out: Vec<Vec<f32>>,
    shown: Vec<Vec<f32>>,
}

/// Runs `input` through `node` in blocks of `block` frames, `playing` saying
/// for each block (by its number) whether the song plays, with `events`
/// landing in the block they fall in.
fn run_shown(
    node: &mut dyn Node,
    reader: &mut DisplayReader,
    input: &[Vec<f32>],
    block: usize,
    events: &[bl_engine::NodeEvent],
    playing: impl Fn(usize) -> bool,
) -> Run {
    let frames = input[0].len();
    let mut out = vec![Vec::with_capacity(frames); input.len()];
    let mut shown = Vec::new();
    let tempo = tempo();
    for (number, done) in (0..frames).step_by(block).enumerate() {
        let length = block.min(frames - done);
        let here: Vec<bl_engine::NodeEvent> = events
            .iter()
            .filter(|e| (e.offset as usize) >= done && (e.offset as usize) < done + length)
            .map(|e| bl_engine::NodeEvent {
                offset: e.offset - done as u32,
                ..*e
            })
            .collect();
        let part: Vec<Vec<f32>> = input
            .iter()
            .map(|c| c[done..done + length].to_vec())
            .collect();
        let transport = TransportInfo {
            playing: playing(number),
            position: Frames(done as i64),
            ..TransportInfo::default()
        };
        for (all, chunk) in out
            .iter_mut()
            .zip(run_at(node, &part, &here, &tempo, transport))
        {
            all.extend(chunk);
        }
        shown.push(reader.read().values.clone());
    }
    Run { out, shown }
}

/// Vector Delay with forces on, a node and the reader of its display.
fn moving(params: &[(ParamId, f64)]) -> (Box<dyn Node>, DisplayReader) {
    let mut node = vector(1, params);
    let reader = node.display().expect("Vector Delay has a display");
    (node, reader)
}

fn forces() -> Vec<(ParamId, f64)> {
    vec![
        (GRAVITY, 40.0),
        (MAGNET, -30.0),
        (FLOW, 60.0),
        (SPEED, 200.0),
        (FRICTION, 5.0),
        (TAPS, 6.0),
    ]
}

// Spec: vector-delay R3, R7
#[test]
fn the_motion_and_the_render_are_identical_at_four_block_sizes_and_run_after_run() {
    let frames = 16_384;
    let input = [noise(frames), noise(frames)];
    let events = [
        set(5_000, FLOW, 20.0),
        set(7_000, GRAVITY, -50.0),
        set(9_000, SPEED, 100.0),
        set(11_000, MAGNET, 40.0),
    ];
    let render = |block: usize| {
        let (mut node, mut reader) = moving(&forces());
        let out = run_blocks(node.as_mut(), &input, block, &events, &tempo());
        (out, reader.read().values.clone())
    };
    let first = render(4_096);
    assert_eq!(render(4_096), first, "run after run");
    assert!(first.0[0].iter().any(|s| *s != 0.0));
    // The particles moved, and the same way whatever the blocks.
    let start: Vec<f32> = (0..6)
        .map(|n| {
            vector_delay_type()
                .description
                .param(tap_x(n))
                .unwrap()
                .default as f32
        })
        .collect();
    assert!((0..6).any(|n| (first.1[1 + 6 * n] - start[n]).abs() > 0.01));
    for block in [1, 64, 512] {
        assert_eq!(render(block), first, "block size {block}");
    }
    assert_ne!(
        {
            let mut node = vector(2, &forces());
            let mut reader = node.display().unwrap();
            run_blocks(node.as_mut(), &input, 4_096, &events, &tempo());
            reader.read().values.clone()
        },
        first.1,
        "the flow field follows the device's seed"
    );
}

/// Where taps 1's particle is after each block, from the display's data.
fn places(run: &Run) -> Vec<[f32; 2]> {
    run.shown.iter().map(|d| [d[1], d[2]]).collect()
}

// Spec: vector-delay R3
#[test]
fn gravity_magnet_flow_speed_and_friction_move_the_particles_in_the_square() {
    // A quarter of a second.
    let silent = [vec![0.0f32; 12_000], vec![0.0f32; 12_000]];
    let place = |params: &[(ParamId, f64)]| {
        let (mut node, mut reader) =
            moving(&[&[(TAPS, 1.0), (tap_x(0), 0.5), (tap_y(0), 0.5)], params].concat());
        let run = run_shown(node.as_mut(), &mut reader, &silent, 32, &[], |_| true);
        places(&run)
    };
    let rest = place(&[]);
    assert!(
        rest.iter().all(|p| *p == [0.5, 0.5]),
        "no forces, no motion"
    );
    let down = place(&[(GRAVITY, 100.0)]);
    assert!(
        down.last().unwrap()[1] < 0.5 && down.last().unwrap()[0] == 0.5,
        "gravity pulls down"
    );
    let up = place(&[(GRAVITY, -100.0)]);
    assert!(up.last().unwrap()[1] > 0.5, "or up");
    // Off the centre, the magnet pulls toward it and a negative one pushes away.
    let off = [(tap_x(0), 0.95), (tap_y(0), 0.5)];
    let pulled = place(&[&off[..], &[(MAGNET, 100.0), (FRICTION, 0.0)]].concat());
    assert!(pulled.last().unwrap()[0] < 0.9, "toward the centre");
    let pushed = place(&[&off[..], &[(MAGNET, -100.0), (FRICTION, 0.0)]].concat());
    assert!(pushed.last().unwrap()[0] > 0.97, "away from it");
    // Flow moves a particle with nothing else on it.
    let flowing = place(&[(FLOW, 100.0)]);
    assert!(flowing.last().unwrap() != &[0.5, 0.5], "flow moves them");
    // Speed scales time: at 0 nothing moves, at 200 % it goes further.
    assert!(
        place(&[(GRAVITY, 100.0), (SPEED, 0.0)])
            .iter()
            .all(|p| *p == [0.5, 0.5])
    );
    let slow = place(&[(GRAVITY, 100.0), (SPEED, 100.0)]);
    let fast = place(&[(GRAVITY, 100.0), (SPEED, 200.0)]);
    assert!(fast.last().unwrap()[1] < slow.last().unwrap()[1]);
    // Friction slows them: 100 % falls less far than 0 %.
    let stuck = place(&[(GRAVITY, 100.0), (FRICTION, 100.0)]);
    let free = place(&[(GRAVITY, 100.0), (FRICTION, 0.0)]);
    assert!(stuck.last().unwrap()[1] > free.last().unwrap()[1]);
    // They bounce off the walls: kept in the square however long the fall.
    let long = [vec![0.0f32; 96_000], vec![0.0f32; 96_000]];
    let (mut node, mut reader) = moving(&[
        (TAPS, 4.0),
        (GRAVITY, 100.0),
        (MAGNET, -100.0),
        (FLOW, 100.0),
        (FRICTION, 0.0),
        (SPEED, 400.0),
    ]);
    let run = run_shown(node.as_mut(), &mut reader, &long, 32, &[], |_| true);
    for data in &run.shown {
        for n in 0..4 {
            assert!(
                (0.0..=1.0).contains(&data[1 + 6 * n]) && (0.0..=1.0).contains(&data[2 + 6 * n])
            );
        }
    }
    let lowest = run.shown.iter().map(|d| d[2]).fold(f32::MAX, f32::min);
    assert!(lowest < 0.05, "it reached the floor and bounced: {lowest}");
}

/// The delay in frames of tap 1 at output frame `n`, from the display data
/// after each 32-frame block: the delay is the straight line between the
/// grid points, which the data holds one block ahead.
fn delay_at(run: &Run, n: f64) -> f64 {
    let block = (n / 32.0).floor() as usize;
    let at = |k: usize| f64::from(run.shown[k][5]) * RATE;
    let (from, to) = (if block == 0 { at(0) } else { at(block - 1) }, at(block));
    from + (to - from) * (n - 32.0 * block as f64) / 32.0
}

/// The interpolated rising zero crossings of `signal` over `from..to`.
fn crossings(signal: &[f32], from: usize, to: usize) -> Vec<f64> {
    (from..to - 1)
        .filter(|n| signal[*n] <= 0.0 && signal[*n + 1] > 0.0)
        .map(|n| n as f64 + f64::from(-signal[n]) / f64::from(signal[n + 1] - signal[n]))
        .collect()
}

/// One tap pushed away from the centre: it speeds up, its delay grows, and
/// then it grows faster than time passes, until the tap hits the wall.
fn pushed_tap() -> (Box<dyn Node>, DisplayReader) {
    moving(&[
        (TAPS, 1.0),
        (tap_x(0), 0.52),
        (tap_y(0), 1.0),
        (MAX_TIME, 1_400.0),
        (MAGNET, -100.0),
        (FRICTION, 0.0),
        (FEEDBACK, 0.0),
        (SPREAD, 0.0),
    ])
}

// Spec: vector-delay R4
#[test]
fn a_moving_taps_pitch_follows_the_doppler_law() {
    let frames = 120_000;
    let (mut node, mut reader) = pushed_tap();
    let input = [tone(frames), tone(frames)];
    let run = run_shown(node.as_mut(), &mut reader, &input, 32, &[], |_| true);
    let mut checked = 0;
    let mut reversed = 0;
    for start in (30_000..110_000).step_by(2_000) {
        let found = crossings(&run.out[0], start, start + 2_000);
        if found.len() < 6 {
            continue;
        }
        let (first, last) = (found[0], found[found.len() - 1]);
        let (begins, ends) = (
            delay_at(&run, first + 1.0) - delay_at(&run, first),
            delay_at(&run, last + 1.0) - delay_at(&run, last),
        );
        if (1.0 - begins).signum() != (1.0 - ends).signum() {
            // The tap bounced off the wall inside the window.
            continue;
        }
        if delay_at(&run, start as f64) > start as f64 - 100.0 {
            // The tap reads before the tone began.
            continue;
        }
        let measured = (found.len() - 1) as f64 * RATE / (last - first);
        let rate = (delay_at(&run, last) - delay_at(&run, first)) / (last - first);
        let want = 1_000.0 * (1.0 - rate).abs();
        let cents = 1_200.0 * (measured / want).log2();
        assert!(
            cents.abs() < 5.0,
            "r = {rate} from {start}: {measured} Hz against {want} Hz"
        );
        checked += 1;
        reversed += usize::from(rate > 1.0);
    }
    assert!(checked >= 20, "windows checked: {checked}");
    assert!(reversed >= 1, "the tap reached r above 1: {reversed}");
}

/// The share of the steps from one click to the next, among the clicks of
/// `signal` over `from..to`, that fall; a jump of a factor of three or more,
/// where the pattern starts over, isn't a step. Each click is its loudest
/// sample, with the sinc's ringing about it left in the 120 frames after.
fn falling_share(signal: &[f32], from: usize, to: usize) -> f64 {
    let (mut heard, mut n) = (Vec::new(), from);
    while n < to {
        if signal[n].abs() > 0.05 {
            let end = (n + 120).min(to);
            heard.push(signal[n..end].iter().fold(0.0f32, |m, s| m.max(s.abs())));
            n = end;
        } else {
            n += 1;
        }
    }
    let steps: Vec<f32> = heard
        .windows(2)
        .map(|w| w[1] / w[0])
        .filter(|ratio| (1.0 / 3.0..3.0).contains(ratio))
        .collect();
    assert!(steps.len() >= 5, "clicks heard: {}", heard.len());
    steps.iter().filter(|ratio| **ratio < 1.0).count() as f64 / steps.len() as f64
}

// Spec: vector-delay R4
#[test]
fn a_tap_whose_delay_grows_faster_than_time_passes_plays_backwards() {
    let frames = 100_000;
    // Clicks 100 frames apart, each twice the last until a drop.
    let clicks: Vec<f32> = (0..frames)
        .map(|n| {
            if n % 100 == 0 {
                0.1 * 2f32.powi((n / 100) % 4)
            } else {
                0.0
            }
        })
        .collect();
    let (mut node, mut reader) = pushed_tap();
    let input = [clicks.clone(), clicks];
    let run = run_shown(node.as_mut(), &mut reader, &input, 32, &[], |_| true);
    // The blocks the delay grows faster than time (r above 1) and those it
    // grows slower, the tap's echo then playing the clicks forwards.
    let slope = |block: usize| {
        (delay_at(&run, 32.0 * (block + 1) as f64) - delay_at(&run, 32.0 * block as f64)) / 32.0
    };
    let blocks = 40_000 / 32..96_000 / 32;
    let fast: Vec<usize> = blocks
        .clone()
        .filter(|b| (1.1..1.5).contains(&slope(*b)))
        .collect();
    let slow: Vec<usize> = blocks.filter(|b| (0.3..0.9).contains(&slope(*b))).collect();
    // The first stretch of them, before the tap meets the wall.
    let run_of = fast
        .iter()
        .zip(fast[0]..)
        .take_while(|(b, n)| **b == *n)
        .count();
    let (start, end) = (32 * fast[0], 32 * (fast[run_of - 1] + 1));
    assert!(
        run_of > 100,
        "the delay grew faster than time for {run_of} blocks"
    );
    let backwards = falling_share(&run.out[0], start, end);
    let (from, to) = (32 * slow[0], 32 * (slow[slow.len() - 1] + 1));
    let forwards = falling_share(&run.out[0], from.max(46_000), to.min(60_000));
    assert!(backwards > 0.8, "r above 1: clicks fall {backwards}");
    assert!(forwards < 0.2, "r below 1: clicks fall {forwards}");
}

// Spec: vector-delay R5
#[test]
fn reset_and_the_transport_start_put_the_particles_back_and_repeat_the_motion() {
    let frames = 32 * 120;
    let silent = [vec![0.0f32; frames], vec![0.0f32; frames]];
    // From a fresh device: the motion after the first block.
    let (mut node, mut reader) = moving(&forces());
    let first = run_shown(node.as_mut(), &mut reader, &silent, 32, &[], |_| true);
    // Reset at block 40 repeats it, and so does the transport starting at
    // block 80 after stopping at block 60.
    let (mut node, mut reader) = moving(&forces());
    let events = [set(32 * 40, RESET, 1.0), set(32 * 41, RESET, 0.0)];
    let later = run_shown(node.as_mut(), &mut reader, &silent, 32, &events, |b| {
        !(60..80).contains(&b)
    });
    for k in 0..30 {
        assert_eq!(later.shown[40 + k], first.shown[k], "Reset, block {k}");
        assert_eq!(
            later.shown[80 + k],
            first.shown[k],
            "transport start, block {k}"
        );
    }
    assert_ne!(later.shown[59], first.shown[59], "it was elsewhere before");
}

// Spec: vector-delay R8
#[test]
fn setting_a_starting_place_puts_its_particle_there_at_rest() {
    let silent = [vec![0.0f32; 32 * 100], vec![0.0f32; 32 * 100]];
    let (mut node, mut reader) = moving(&forces());
    let events = [set(32 * 20, tap_x(1), 0.9), set(32 * 20, tap_y(1), 0.3)];
    let run = run_shown(node.as_mut(), &mut reader, &silent, 32, &events, |_| true);
    // The parameters glide, so the particle follows them to the place and
    // there it sits for a grid step, at rest.
    let at = run.shown[20..]
        .iter()
        .find(|d| (d[7] - 0.9).abs() < 1e-6 && (d[8] - 0.3).abs() < 1e-6)
        .expect("the particle reached the place set");
    assert_eq!((at[9], at[10]), (0.0, 0.0), "at rest");
}

// Spec: vector-delay R6
#[test]
fn feedback_returns_the_taps_sum_and_input_and_dry_wet_balance_the_signal() {
    let frames = 40_000;
    let one_tap = [
        (TAPS, 1.0),
        (tap_x(0), 0.25),
        (tap_y(0), 1.0),
        (SPREAD, 0.0),
    ];
    let run = |extra: &[(ParamId, f64)]| {
        let mut node = vector(1, &[&one_tap[..], extra].concat());
        run_at(
            node.as_mut(),
            &[impulse(frames), impulse(frames)],
            &[],
            &tempo(),
            TransportInfo::default(),
        )
    };
    // 50 % feedback: each echo half the last, at multiples of 12,000.
    let echoes = run(&[(FEEDBACK, 50.0)]);
    for (n, want) in [(12_000, 1.0), (24_000, 0.5), (36_000, 0.25)] {
        assert!((around(&echoes[0], n) - want).abs() < 1e-3, "echo at {n}");
    }
    let plain = run(&[(FEEDBACK, 0.0)]);
    assert!(
        plain[0][13_000..].iter().all(|s| s.abs() < 1e-6),
        "one echo"
    );
    // Dry/Wet: 0 is the input, 100 has none of it.
    assert_eq!(run(&[(DRY_WET, 0.0)])[0], impulse(frames));
    assert_eq!(plain[0][0], 0.0);
    assert!((run(&[(DRY_WET, 50.0), (FEEDBACK, 0.0)])[0][0] - 0.5).abs() < 1e-6);
    // Input at -inf dB records nothing: no echo, the dry still passes.
    let muted = run(&[(INPUT, 0.0), (DRY_WET, 50.0)]);
    assert_eq!(muted[0][0], 0.5);
    assert!(muted[0][1..].iter().all(|s| *s == 0.0));
    // The device has no latency.
    assert_eq!(vector(1, &[]).latency(), 0);
}

// Spec: vector-delay R8
#[test]
fn the_display_reads_each_particles_place_time_and_level_headless() {
    let description = vector_delay_type().description;
    let id = DeviceId(Id::new(1).unwrap());
    let mut values = OrdMap::new();
    values.insert(TAPS, 2.0);
    values.insert(tap_x(0), 0.25);
    values.insert(tap_y(0), 1.0);
    values.insert(tap_x(1), 0.5);
    values.insert(tap_y(1), 0.5);
    let shown = bl_devices::display(VECTOR_DELAY, id, &description, &values, None).unwrap();
    assert!(!shown.live);
    assert_eq!(shown.markers.len(), 2);
    let tap = &shown.markers[1];
    assert_eq!((tap.name.as_str(), tap.x, tap.y), ("Tap 2", 0.5, 0.5));
    assert_eq!(shown.reading("Tap 1 Time").unwrap().text, "250 ms");
    assert_eq!(shown.reading("Tap 2 Level").unwrap().text, "-6.0 dB");
    assert!(shown.series("Particles").is_some());
    // From an engine: the particles' places after a run, as published.
    let (mut node, mut reader) = moving(&two_taps());
    run_at(
        node.as_mut(),
        &[vec![0.0; 64], vec![0.0; 64]],
        &[],
        &tempo(),
        TransportInfo::default(),
    );
    let data = reader.read().values.clone();
    assert_eq!(data.len(), DISPLAY_LEN);
    let live = bl_devices::display(VECTOR_DELAY, id, &description, &values, Some(&data)).unwrap();
    assert!(live.live);
    assert_eq!(live.reading("Tap 1 Time").unwrap().text, "250 ms");
}

//! CV Envelope Follower, CV LFO and CV Shaper (WP-7.18b; the CV tools spec,
//! R32 to R36): the voltage each writes to its CV output, read off the CV
//! buffer a program would give it, beside what the modulator it shares its
//! law with makes of the same settings. Every block runs as a real-time
//! section under the allocation trap.

use crate::common;
use bl_core::time::{Frames, TempoMap};
use bl_devices::cv_modulators::{MAX, MIN, cv_curve_type, cv_level_type, cv_wave_type};
use bl_devices::level_mod::{
    DELAY_BEATS, DELAY_MODE, DELAY_TIME, FALL, RISE, level_mod_type,
};
use bl_devices::modulator::{
    JITTER, MANUAL, PHASE, RATE_BEATS, RATE_HZ, RESTART, SHAPE, SMOOTH, TIME_MODE, TRIGGER,
    TRIGGER_MODE, WAVE, WAVES, wave_mod_type,
};
use bl_engine::modbus::Shape;
use bl_engine::{Block, Collector, Node, NodeEvent, Registry, TransportInfo};
use bl_model::ParamId;
use bl_model::modulator::{self, DEPTH, POLARITY, ShapePoint, shape_preset, shape_value};
use common::{CHANNELS, RATE, control, device, node, set, signal};

/// The block sizes every render is checked at.
const BLOCKS: [usize; 4] = [1, 64, 512, 4096];

fn registry() -> Registry {
    let mut registry = Registry::empty();
    bl_devices::register(&mut registry);
    registry
}

/// What a device wrote on its CV output and left on its track.
struct Rendered {
    cv: Vec<f32>,
    audio: Vec<Vec<f32>>,
}

/// Runs `node` over `frames` frames in blocks of `block`, each a real-time
/// section: `input` is each channel's audio (silence past its end),
/// `events` its parameter changes (offsets from the run's start) and
/// `transport` the song's state at the start of each block, given the
/// frames run so far.
fn render(
    node: &mut dyn Node,
    frames: usize,
    block: usize,
    input: &[Vec<f32>],
    events: &[NodeEvent],
    transport: impl Fn(usize) -> TransportInfo,
) -> Rendered {
    let tempo = TempoMap::default();
    let mut cv_out = Vec::with_capacity(frames);
    let mut audio: Vec<Vec<f32>> = (0..CHANNELS).map(|_| Vec::with_capacity(frames)).collect();
    let mut done = 0;
    while done < frames {
        let length = block.min(frames - done);
        let here: Vec<NodeEvent> = events
            .iter()
            .filter(|e| (e.offset as usize) >= done && (e.offset as usize) < done + length)
            .map(|e| NodeEvent {
                offset: e.offset - done as u32,
                ..*e
            })
            .collect();
        let mut data: Vec<f32> = (0..CHANNELS)
            .flat_map(|c| {
                (done..done + length).map(move |i| {
                    input
                        .get(c)
                        .and_then(|channel| channel.get(i))
                        .copied()
                        .unwrap_or(0.0)
                })
            })
            .collect();
        let mut cv = vec![0.0f32; CHANNELS * length];
        {
            let mut block = Block::new(
                &mut data,
                CHANNELS,
                length,
                length,
                &here,
                transport(done),
                RATE,
            )
            .with_tempo(&tempo)
            .with_cv_out(&mut cv);
            let node = &mut *node;
            bl_rt::rt_test!({ node.process(&mut block) });
        }
        cv_out.extend_from_slice(&cv[..length]);
        for (channel, values) in audio.iter_mut().zip(data.chunks(length)) {
            channel.extend_from_slice(values);
        }
        done += length;
    }
    Rendered { cv: cv_out, audio }
}

fn playing(position: usize) -> TransportInfo {
    TransportInfo::at(true, Frames(position as i64))
}

/// A CV LFO with `params` set: Min 0 V and Max 10 V unless they say
/// otherwise, so a sample value is the signal itself.
fn cv_wave(params: &[(ParamId, f64)]) -> Box<dyn Node> {
    let mut all = vec![(MIN, 0.0), (MAX, 10.0)];
    all.extend_from_slice(params);
    node(&registry(), &device(&cv_wave_type(), &all), 4_096)
}

/// The LFO's signal for the same settings, from its first slot given the
/// signal itself (unipolar, full amount and depth), at its 32-frame grid.
fn lfo(params: &[(ParamId, f64)], frames: usize) -> Vec<f32> {
    let mut all = vec![(modulator::amount(0), 100.0), (POLARITY, 1.0)];
    all.extend_from_slice(params);
    let mut node = node(&registry(), &device(&wave_mod_type(), &all), 4_096);
    control(
        node.as_mut(),
        frames,
        512,
        &[],
        &TempoMap::default(),
        playing(0),
    )
    .contribution(0)
    .to_vec()
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-5
}

// Spec: cv-tools R33, R34
#[test]
fn each_waveform_equals_the_lfos_on_its_grid_points_and_follows_the_law_between() {
    const FRAMES: usize = 40_000;
    for (index, name) in WAVES.iter().enumerate() {
        // Shape bends the deterministic waves, Jitter reaches every one.
        let params = [
            (WAVE, index as f64),
            (SHAPE, 25.0),
            (RATE_HZ, 3.7),
            (JITTER, 30.0),
        ];
        let want = lfo(&params, FRAMES);
        let got = render(cv_wave(&params).as_mut(), FRAMES, 512, &[], &[], playing).cv;
        for at in (0..FRAMES).step_by(32) {
            assert!(
                close(got[at], want[at]),
                "{name}, grid frame {at}: {} isn't the LFO's {}",
                got[at],
                want[at]
            );
        }
        // Between the points it isn't the LFO's straight line: the law is
        // worked out on the frame.
        if index < 5 {
            assert!(
                (0..FRAMES).any(|at| at % 32 != 0 && !close(got[at], want[at])),
                "{name} is on the grid's line between its points"
            );
        }
    }
}

// Spec: cv-tools R33
#[test]
fn the_deterministic_waves_follow_their_law_on_every_frame() {
    // 10 Hz at 48 kHz is a cycle of 4,800 frames.
    for (index, law) in [
        (0usize, (|p: f64| 0.5 - 0.5 * (std::f64::consts::TAU * p).cos()) as fn(f64) -> f64),
        (1, |p| p),
        (2, |p| 1.0 - p),
        (3, |p| 1.0 - (2.0 * p - 1.0).abs()),
        (4, |p| f64::from(u8::from(p < 0.5))),
    ] {
        let got = render(
            cv_wave(&[(WAVE, index as f64), (RATE_HZ, 10.0)]).as_mut(),
            9_600,
            512,
            &[],
            &[],
            playing,
        )
        .cv;
        for (frame, &value) in got.iter().enumerate() {
            let want = law((frame % 4_800) as f64 / 4_800.0);
            assert!(
                (f64::from(value) - want).abs() < 1e-4,
                "{}, frame {frame}: {value} isn't {want}",
                WAVES[index]
            );
        }
    }
}

// Spec: cv-tools R33
#[test]
fn a_1_hz_square_wave_has_its_edges_on_their_frames() {
    for block in BLOCKS {
        let got = render(
            cv_wave(&[(WAVE, 4.0), (RATE_HZ, 1.0)]).as_mut(),
            96_000,
            block,
            &[],
            &[],
            playing,
        )
        .cv;
        let high = |from: usize, to: usize| got[from..to].iter().all(|&v| v == 1.0);
        let low = |from: usize, to: usize| got[from..to].iter().all(|&v| v == 0.0);
        assert!(high(0, 24_000), "block size {block}: high to frame 23,999");
        assert!(low(24_000, 48_000), "block size {block}: low from 24,000");
        assert!(high(48_000, 72_000), "block size {block}: high again");
        assert!(low(72_000, 96_000), "block size {block}: and low again");
    }
}

// Spec: cv-tools R34
#[test]
fn a_synced_cv_lfo_locks_to_the_song_position_and_a_free_one_starts_from_phase() {
    // At 120 BPM a quarter note is 24,000 frames; a 1/4 cycle on the rising
    // ramp. A song that starts a quarter of the way in begins there.
    let synced = [
        (TIME_MODE, 1.0),
        (RATE_BEATS, 3.0),
        (WAVE, 1.0),
        (PHASE, 0.0),
    ];
    let got = render(
        cv_wave(&synced).as_mut(),
        24_000,
        512,
        &[],
        &[],
        |done| playing(6_000 + done),
    )
    .cv;
    for frame in [0usize, 1, 5_999, 6_000, 17_999] {
        let want = (6_000 + frame) as f32 / 24_000.0;
        assert!(close(got[frame], want), "frame {frame}: {}", got[frame]);
    }
    // Stopped, a synced cycle holds still at the song position.
    let held = render(cv_wave(&synced).as_mut(), 2_048, 512, &[], &[], |_| {
        TransportInfo::at(false, Frames(12_000))
    })
    .cv;
    assert!(held.iter().all(|&v| close(v, 0.5)), "held at the position");

    // A free cycle runs while the song is stopped and starts again from its
    // Phase when the transport starts.
    let free = [(WAVE, 1.0), (RATE_HZ, 10.0), (PHASE, 25.0)];
    let got = render(cv_wave(&free).as_mut(), 4_096, 512, &[], &[], |done| {
        if done < 2_048 {
            TransportInfo::at(false, Frames(0))
        } else {
            playing(0)
        }
    })
    .cv;
    assert!(close(got[0], 0.25), "from Phase while stopped");
    assert!(got[2_047] > 0.25 + 0.4, "it ran while stopped");
    assert!(close(got[2_048], 0.25), "from Phase again at the start");
    assert!(close(got[2_048 + 480], 0.35), "and on from there");
}

// Spec: cv-tools R32
#[test]
fn min_max_and_depth_map_the_signal_to_volts_and_min_above_max_inverts_it() {
    // The rising ramp, 10 Hz: the signal at frame 2,400 is 0.5.
    let at = |params: &[(ParamId, f64)]| {
        let mut all = vec![(WAVE, 1.0), (RATE_HZ, 10.0)];
        all.extend_from_slice(params);
        let mut node = node(&registry(), &device(&cv_wave_type(), &all), 4_096);
        let got = render(node.as_mut(), 4_800, 512, &[], &[], playing).cv;
        (got[0] * 10.0, got[2_400] * 10.0, got[4_799] * 10.0)
    };
    // The last frame is one frame short of the top of the ramp.
    let volts = |got: (f32, f32, f32), want: (f32, f32, f32)| {
        assert!(
            (got.0 - want.0).abs() < 1e-3
                && (got.1 - want.1).abs() < 1e-3
                && (got.2 - want.2).abs() < 1e-2,
            "{got:?} isn't {want:?}"
        );
    };
    // CV LFO's defaults: −5 V to +5 V.
    volts(at(&[]), (-5.0, 0.0, 5.0));
    volts(at(&[(MIN, -2.0), (MAX, 6.0)]), (-2.0, 2.0, 6.0));
    // Depth takes a share of the span from Min.
    volts(
        at(&[(MIN, -2.0), (MAX, 6.0), (DEPTH, 50.0)]),
        (-2.0, 0.0, 2.0),
    );
    volts(at(&[(MIN, -2.0), (MAX, 6.0), (DEPTH, 0.0)]), (-2.0, -2.0, -2.0));
    // Min above Max inverts it, and the full range is reachable.
    volts(at(&[(MIN, 6.0), (MAX, -2.0)]), (6.0, 2.0, -2.0));
    volts(at(&[(MIN, -10.0), (MAX, 10.0)]), (-10.0, 0.0, 10.0));
    // The device's defaults: CV Shaper and CV Envelope Follower send 0 to 5 V.
    for (device_type, name) in [(cv_curve_type(), "CV Shaper"), (cv_level_type(), "CV Envelope Follower")] {
        let description = &device_type.description;
        let default = |id| description.param(id).unwrap().default;
        assert_eq!((default(MIN), default(MAX), default(DEPTH)), (0.0, 5.0, 100.0), "{name}");
    }
}

/// The shape the Shaper tests play: a rise to the middle, a curved fall and
/// a floor.
fn shape() -> Vec<ShapePoint> {
    let p = |x, y, bend| ShapePoint {
        x,
        y,
        bend,
        sustain: false,
    };
    vec![
        p(0.0, 0.0, 0.0),
        p(0.5, 1.0, 0.6),
        p(0.9, 0.25, 0.0),
        p(1.0, 0.25, 0.0),
    ]
}

fn cv_curve(shape: &[ShapePoint], params: &[(ParamId, f64)]) -> Box<dyn Node> {
    let mut all = vec![(MIN, 0.0), (MAX, 10.0)];
    all.extend_from_slice(params);
    let mut device = device(&cv_curve_type(), &all);
    device.shape = Some(shape.to_vec());
    node(&registry(), &device, 4_096)
}

// Spec: cv-tools R33, R35
#[test]
fn the_shaper_plays_its_shape_in_loop_on_every_frame() {
    for block in BLOCKS {
        let got = render(
            cv_curve(&shape(), &[(RATE_HZ, 10.0)]).as_mut(),
            9_600,
            block,
            &[],
            &[],
            playing,
        )
        .cv;
        for (frame, &value) in got.iter().enumerate() {
            let want = shape_value(&shape(), (frame % 4_800) as f64 / 4_800.0);
            assert!(
                (f64::from(value) - want).abs() < 1e-5,
                "block size {block}, frame {frame}: {value} isn't {want}"
            );
        }
    }
    // The pulse preset's drop lands on its frame at 1 Hz, within the
    // preset's own 1/4096 slope.
    let pulse = shape_preset(4).unwrap();
    let got = render(cv_curve(&pulse, &[(RATE_HZ, 1.0)]).as_mut(), 48_000, 512, &[], &[], playing).cv;
    assert!(got[..23_000].iter().all(|&v| v == 1.0));
    assert!(got[24_100..].iter().all(|&v| v == 0.0));
}

// Spec: cv-tools R35
#[test]
fn one_shot_plays_once_per_t_and_manual_holds_the_picked_point() {
    let rise = shape_preset(1).unwrap();
    let params = [(RATE_HZ, 10.0), (TRIGGER_MODE, 1.0)];
    let events = [
        set(1_000, TRIGGER, 1.0),
        set(8_000, TRIGGER, 0.0),
        set(10_000, TRIGGER, 1.0),
    ];
    let got = render(cv_curve(&rise, &params).as_mut(), 16_000, 700, &[], &events, playing).cv;
    assert!(got[..1_000].iter().all(|&v| v == 0.0), "rests at the start");
    // Each frame after T turns on is a step of the shape's 4,800-frame cycle.
    for n in [0usize, 100, 2_399, 4_000] {
        let want = ((n + 1) as f32 / 4_800.0).min(1.0);
        assert!(close(got[1_000 + n], want), "frame {n} after T: {}", got[1_000 + n]);
    }
    assert!(
        got[1_000 + 4_800..10_000].iter().all(|&v| v == 1.0),
        "rests at its end, not looping"
    );
    assert!(close(got[10_000 + 2_399], 0.5), "plays again at the next T");

    // Manual: the control's position picks the point of the shape.
    let triangle = shape_preset(3).unwrap();
    let events = [set(2_000, MANUAL, 75.0)];
    let got = render(
        cv_curve(&triangle, &[(TRIGGER_MODE, 2.0), (MANUAL, 25.0)]).as_mut(),
        6_000,
        400,
        &[],
        &events,
        playing,
    )
    .cv;
    assert!(close(got[1_500], 0.5), "25 % of the triangle");
    assert!(close(got[5_900], 0.5), "75 % of it");
}

// Spec: cv-tools R35
#[test]
fn a_new_shape_reaches_the_shaper_as_node_data() {
    let collector = Collector::new();
    let mut node = cv_curve(
        &[ShapePoint {
            x: 0.0,
            y: 1.0,
            bend: 0.0,
            sustain: false,
        }],
        &[(TRIGGER_MODE, 2.0), (MANUAL, 50.0)],
    );
    assert_eq!(render(node.as_mut(), 256, 256, &[], &[], playing).cv[200], 1.0);
    node.receive_data(collector.handle().data(Shape(vec![ShapePoint {
        x: 0.0,
        y: 0.25,
        bend: 0.0,
        sustain: false,
    }])));
    assert_eq!(render(node.as_mut(), 256, 256, &[], &[], playing).cv[200], 0.25);
}

// Spec: cv-tools R8, R35
#[test]
fn random_parts_repeat_from_the_transports_start() {
    // A random wave with Jitter, played, stopped and played again from the
    // start: the second play is the first.
    let params = [(WAVE, 5.0), (RATE_HZ, 10.0), (JITTER, 40.0)];
    let song = |done: usize| match done {
        ..10_240 => playing(done),
        10_240..14_336 => TransportInfo::at(false, Frames(0)),
        _ => playing(done - 14_336),
    };
    let got = render(cv_wave(&params).as_mut(), 24_576, 512, &[], &[], song).cv;
    assert_eq!(got[..9_000], got[14_336..14_336 + 9_000]);
    assert!(
        got[..9_000].windows(2).any(|w| w[0] != w[1]),
        "and it isn't a constant"
    );
    // A new device plays the same from the start (R8: seeded per device).
    let again = render(cv_wave(&params).as_mut(), 9_000, 64, &[], &[], playing).cv;
    assert_eq!(got[..9_000], again[..]);
    // Restarting R makes a free cycle start from Phase.
    let events = [set(1_000, RESTART, 1.0)];
    let got = render(
        cv_wave(&[(WAVE, 1.0), (RATE_HZ, 10.0), (PHASE, 10.0)]).as_mut(),
        2_048,
        512,
        &[],
        &events,
        playing,
    )
    .cv;
    assert!(close(got[1_000], 0.1), "R restarts from Phase: {}", got[1_000]);
}

/// A sine burst of `amplitude` at 1 kHz for `frames` frames, from phase 0.
fn burst(amplitude: f32, frames: usize) -> Vec<f32> {
    (0..frames)
        .map(|i| amplitude * (std::f32::consts::TAU * 1_000.0 * i as f32 / 48_000.0).sin())
        .collect()
}

/// Silence, a burst at −12 dBFS, silence, one at −24 dBFS, silence.
fn bursts() -> Vec<Vec<f32>> {
    let mut one = vec![0.0; 2_048];
    one.extend(burst(10f32.powf(-12.0 / 20.0), 8_192));
    one.extend(vec![0.0; 8_192]);
    one.extend(burst(10f32.powf(-24.0 / 20.0), 8_192));
    one.extend(vec![0.0; 8_192]);
    vec![one.clone(), one]
}

fn cv_level(params: &[(ParamId, f64)]) -> Box<dyn Node> {
    let mut all = vec![(MIN, 0.0), (MAX, 10.0)];
    all.extend_from_slice(params);
    node(&registry(), &device(&cv_level_type(), &all), 4_096)
}

// Spec: cv-tools R33, R36
#[test]
fn the_envelope_followers_level_comes_through_rise_fall_and_delay() {
    let input = bursts();
    let frames = input[0].len();
    // The Envelope Follower's rows follow its level one grid step behind
    // (modulation R20), so its row on a grid frame is CV Envelope Follower's
    // output on the grid frame before.
    for params in [
        vec![],
        vec![(RISE, 20.0), (FALL, 300.0)],
        vec![(RISE, 0.0), (FALL, 0.0), (DELAY_TIME, 10.0)],
        vec![(DELAY_MODE, 1.0), (DELAY_BEATS, 0.0)],
    ] {
        let mut all = vec![(modulator::amount(0), 100.0), (POLARITY, 1.0)];
        all.extend_from_slice(&params);
        let mut follower = node(&registry(), &device(&level_mod_type(), &all), 4_096);
        let want = signal(
            follower.as_mut(),
            frames,
            32,
            &input,
            None,
            &[],
            &[],
            &TempoMap::default(),
            playing,
        )
        .rows;
        let want = want.contribution(0);
        let got = render(cv_level(&params).as_mut(), frames, 512, &input, &[], playing);
        for grid in (32..frames).step_by(32) {
            assert!(
                close(got.cv[grid - 32], want[grid]),
                "{params:?}, frame {grid}: {} isn't the Envelope Follower's {}",
                got.cv[grid - 32],
                want[grid]
            );
        }
        assert_eq!(got.audio, input, "{params:?}: the audio passes untouched");
    }

    // The bursts' levels: the peak of each, to the detector's Rise and Fall.
    let got = render(cv_level(&[(RISE, 0.0)]).as_mut(), frames, 512, &input, &[], playing).cv;
    let loud = got[2_048..10_240].iter().fold(0.0f32, |m, &v| m.max(v));
    let quiet = got[26_624..34_816].iter().fold(0.0f32, |m, &v| m.max(v));
    assert!((loud - 0.251).abs() < 0.01, "−12 dBFS: {loud}");
    assert!((quiet - 0.063).abs() < 0.005, "−24 dBFS: {quiet}");
    assert!(got[..2_000].iter().all(|&v| v == 0.0), "silence gives 0 V");
}

// Spec: cv-tools R8
#[test]
fn every_render_is_the_same_at_any_block_size() {
    let input = bursts();
    let frames = 16_384;
    let events = [set(5_000, TRIGGER, 1.0)];
    let renders: Vec<(&str, Box<dyn Fn(usize) -> Vec<f32>>)> = vec![
        (
            "CV LFO",
            Box::new(move |block| {
                let params = [(WAVE, 7.0), (RATE_HZ, 12.0), (JITTER, 50.0), (SMOOTH, 40.0)];
                render(cv_wave(&params).as_mut(), frames, block, &[], &[], playing).cv
            }),
        ),
        (
            "CV Shaper",
            Box::new(move |block| {
                let params = [(RATE_HZ, 12.0), (JITTER, 50.0), (TRIGGER_MODE, 1.0)];
                render(cv_curve(&shape(), &params).as_mut(), frames, block, &[], &events, playing).cv
            }),
        ),
        (
            "CV Envelope Follower",
            Box::new(move |block| {
                let params = [(RISE, 10.0), (FALL, 80.0), (DELAY_TIME, 5.0)];
                render(cv_level(&params).as_mut(), frames, block, &input, &[], playing).cv
            }),
        ),
    ];
    for (name, make) in renders {
        let first = make(BLOCKS[0]);
        assert!(first.iter().any(|&v| v != 0.0), "{name} writes something");
        for block in &BLOCKS[1..] {
            assert!(make(*block) == first, "{name} at block size {block}");
        }
    }
}

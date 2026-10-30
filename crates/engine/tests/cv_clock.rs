//! CV Clock Out and CV Clock In (WP-7.18, `docs/specs/cv-tools.md` R21 to
//! R25): a clock locked to the song written on a CV output, and a pulse
//! train on a CV input that leads the song's tempo. The pulses are read off
//! the device channels, and the input is generated, so every number is the
//! test's own. Every block runs as a real-time section under the allocation
//! trap.

use crate::common;
use std::time::Duration;

use bl_core::clock::TestClock;
use bl_core::midi::{MidiInputEvent, MidiMessage, MidiSource, PortId, SyncInput};
use bl_core::time::{Bpm, TICKS_PER_QUARTER, TempoMap, Ticks};
use bl_devices::cv_clock_in::{self, cv_clock_in_type};
use bl_devices::cv_clock_out::{self, cv_clock_out_type};
use bl_engine::testing::{DELAY, DELAY_FRAMES};
use bl_engine::{DeviceInput, TransportCommand};
use bl_model::{Command, Device, DeviceId, ParamId, TrackId, TrackKind};
use common::{RATE, Rig, device, track};
use rtrb::RingBuffer;

/// The interface's channels, both ways.
const CHANNELS: usize = 4;

/// The block sizes the clocks run at.
const BLOCKS: [usize; 4] = [1, 64, 512, 4096];

fn rig(max_frames: usize) -> Rig {
    Rig::with_config_and_registry(
        max_frames,
        |config| {
            config.input_channels = CHANNELS;
            config.output_channels = CHANNELS;
        },
        |registry| {
            cv_clock_out::register(registry);
            cv_clock_in::register(registry);
        },
    )
}

/// Puts a CV Clock Out on `track`, its Clock to interface channel 1, with
/// `params` set.
fn clock_out(rig: &mut Rig, track: TrackId, params: &[(ParamId, f64)]) -> DeviceId {
    let id = rig.editor.new_id();
    let mut device = cv_clock_out_type().device(id);
    device.external.as_mut().unwrap().cv_outputs[0].channel = Some(1);
    for &(param, value) in params {
        device.params.insert(param, value);
    }
    insert(rig, track, device)
}

/// Puts a CV Clock In on `track`, its Clock on interface channel `channel`,
/// with `params` set.
fn clock_in(rig: &mut Rig, track: TrackId, channel: u16, params: &[(ParamId, f64)]) -> DeviceId {
    let id = rig.editor.new_id();
    let mut device = cv_clock_in_type().device(id);
    device.external.as_mut().unwrap().cv_inputs[0].channel = Some(channel);
    for &(param, value) in params {
        device.params.insert(param, value);
    }
    insert(rig, track, device)
}

fn insert(rig: &mut Rig, track: TrackId, device: Device) -> DeviceId {
    let id = device.id;
    let index = rig.editor.document().track(track).unwrap().devices().len();
    rig.apply(Command::InsertDevice {
        track,
        index,
        device,
    });
    id
}

/// Plays `blocks` blocks of `frames` frames and returns what interface
/// channel 1 carried.
fn play(rig: &mut Rig, frames: usize, blocks: usize) -> Vec<f32> {
    let mut out = Vec::new();
    for _ in 0..blocks {
        rig.block(frames);
        let output = rig.processor.output();
        if output.ext_channels() > 0 {
            out.extend_from_slice(&output.ext_channel(0)[..frames]);
        } else {
            out.extend(std::iter::repeat_n(0.0, frames));
        }
    }
    out
}

/// The frames where the clock rises and where it falls.
fn edges(out: &[f32]) -> (Vec<usize>, Vec<usize>) {
    let (mut rises, mut falls) = (Vec::new(), Vec::new());
    let mut high = false;
    for (at, &sample) in out.iter().enumerate() {
        match (high, sample > 0.0) {
            (false, true) => rises.push(at),
            (true, false) => falls.push(at),
            _ => {}
        }
        high = sample > 0.0;
    }
    (rises, falls)
}

fn tempo_of(breakpoints: &[(Ticks, f64)]) -> TempoMap {
    TempoMap::from_breakpoints(
        breakpoints
            .iter()
            .map(|&(at, bpm)| (at, Bpm::new(bpm).unwrap())),
    )
    .unwrap()
}

// Spec: cv-tools R21, R22
#[test]
fn a_clock_at_120_and_4_per_quarter_pulses_every_6000_frames_at_its_level_and_width() {
    let mut renders = Vec::new();
    for frames in BLOCKS {
        let mut rig = rig(frames);
        let a = track(&mut rig.editor, TrackKind::Audio, None);
        clock_out(&mut rig, a, &[]);
        // Stopped: nothing.
        assert!(
            play(&mut rig, frames, 4096 / frames)
                .iter()
                .all(|&s| s == 0.0),
            "stopped, the clock is 0 V"
        );
        rig.sender.transport(TransportCommand::Play);
        renders.push(play(&mut rig, frames, 20_480 / frames));
    }
    let (rises, falls) = edges(&renders[0]);
    assert_eq!(rises, [0, 6000, 12000, 18000]);
    assert_eq!(falls[..3], [3000, 9000, 15000], "50 % of each period");
    assert!(
        renders[0].iter().filter(|&&s| s != 0.0).all(|&s| s == 0.5),
        "Level 5 V is half of full scale"
    );
    for render in &renders[1..] {
        assert_eq!(render, &renders[0], "the same at any block size");
    }
}

// Spec: cv-tools R21
#[test]
fn rate_width_and_level_set_the_pulses() {
    let mut rig = rig(64);
    let a = track(&mut rig.editor, TrackKind::Audio, None);
    clock_out(
        &mut rig,
        a,
        &[
            // 48 per quarter, Width 25 %, Level 10 V.
            (cv_clock_out::RATE, 7.0),
            (cv_clock_out::WIDTH, 25.0),
            (cv_clock_out::LEVEL, 10.0),
        ],
    );
    rig.sender.transport(TransportCommand::Play);
    let out = play(&mut rig, 64, 32);
    let (rises, falls) = edges(&out);
    assert_eq!(rises[..4], [0, 500, 1000, 1500]);
    assert_eq!(falls[..2], [125, 625]);
    assert!(out.iter().all(|&s| s == 0.0 || s == 1.0));
}

// Spec: cv-tools R22
#[test]
fn a_start_between_multiples_waits_for_the_next() {
    let mut rig = rig(64);
    let a = track(&mut rig.editor, TrackKind::Audio, None);
    clock_out(&mut rig, a, &[]);
    // A tenth of a quarter note in: 2,400 frames, 3,600 short of the next
    // multiple at a sixth of a bar.
    rig.sender
        .transport(TransportCommand::Locate(Ticks(TICKS_PER_QUARTER / 10)));
    rig.sender.transport(TransportCommand::Play);
    let out = play(&mut rig, 64, 200);
    let (rises, _) = edges(&out);
    assert_eq!(rises[..2], [3600, 9600]);
}

// Spec: cv-tools R22
#[test]
fn the_pulses_fall_on_the_tempo_maps_frames_through_a_ramp() {
    let tempo = tempo_of(&[
        (Ticks::ZERO, 100.0),
        (Ticks::from_quarters(4), 140.0),
        (Ticks::from_quarters(40), 140.0),
    ]);
    let mut renders = Vec::new();
    for frames in [64, 4096] {
        let mut rig = rig(frames);
        rig.apply(Command::SetTempo {
            tempo: tempo.clone(),
        });
        let a = track(&mut rig.editor, TrackKind::Audio, None);
        clock_out(&mut rig, a, &[]);
        rig.sender.transport(TransportCommand::Play);
        renders.push(play(&mut rig, frames, 4 * 49_152 / frames));
    }
    assert_eq!(renders[0], renders[1]);
    let (rises, _) = edges(&renders[0]);
    let step = TICKS_PER_QUARTER / 4;
    for (k, &rise) in rises.iter().enumerate() {
        let want = tempo.ticks_to_frames(Ticks(k as i64 * step), RATE).get();
        assert_eq!(rise as i64, want, "pulse {k}");
    }
    assert!(rises.len() > 20, "{} pulses", rises.len());
}

// Spec: cv-tools R22
#[test]
fn a_loops_jump_puts_the_next_pulse_on_the_multiple_after_the_loops_start() {
    let mut rig = rig(64);
    let a = track(&mut rig.editor, TrackKind::Audio, None);
    clock_out(&mut rig, a, &[]);
    // The loop runs from a tenth of a quarter note to a quarter and a tenth.
    rig.apply(Command::SetLoop {
        start: Ticks(TICKS_PER_QUARTER / 10),
        length: Ticks::QUARTER,
    });
    rig.apply(Command::SetLoopEnabled { enabled: true });
    rig.sender.transport(TransportCommand::Play);
    let out = play(&mut rig, 64, 600);
    let (rises, falls) = edges(&out);
    // The song plays up to 26,400 frames and jumps back to 2,400. The pulse
    // that began at 24,000 is cut at the jump, and the next begins at the
    // multiple after the loop's start, 3,600 frames on.
    assert_eq!(rises[..5], [0, 6000, 12000, 18000, 24000]);
    assert_eq!(falls[4], 26_400, "the pulse in progress ends at the jump");
    assert_eq!(rises[5], 26_400 + 3600);
}

// Spec: cv-tools R5, R22
#[test]
fn the_pulses_are_held_back_by_the_tracks_compensation_into_main() {
    let latency = 500;
    for (compensation, shift) in [(true, latency), (false, 0)] {
        let mut rig = rig(64);
        let a = track(&mut rig.editor, TrackKind::Audio, None);
        clock_out(&mut rig, a, &[]);
        let b = track(&mut rig.editor, TrackKind::Audio, None);
        device(&mut rig.editor, b, DELAY, &[(DELAY_FRAMES, latency as f64)]);
        rig.apply(Command::SetDelayCompensation {
            enabled: compensation,
        });
        rig.sender.transport(TransportCommand::Play);
        let out = play(&mut rig, 64, 200);
        let (rises, _) = edges(&out);
        assert_eq!(
            rises[..2],
            [shift, 6000 + shift],
            "compensation {compensation}"
        );
    }
}

/// The interface's input: pulse trains on chosen channels.
struct Trains(Vec<Vec<f32>>);

/// One clock's pulses: when each rises, in frames (between frames), and how
/// long each stays up.
struct Train {
    rises: Vec<f64>,
    width: f64,
}

impl Train {
    /// A train of `count` pulses `interval` frames apart from `first`, each
    /// moved by up to `jitter` frames (from `seed`).
    fn steady(first: f64, interval: f64, count: usize, jitter: f64, seed: u64) -> Self {
        let mut random = crate::clock_in::Random(seed);
        Self {
            rises: (0..count)
                .map(|k| first + k as f64 * interval + random.next() * jitter)
                .collect(),
            width: interval / 2.0,
        }
    }

    /// The voltage at frame `n`, in sample values: rising through
    /// Threshold (0.1, 1 V) exactly at `rise`, on a line a tenth a frame.
    fn level(&self, n: usize) -> f32 {
        let n = n as f64;
        let upto = self.rises.partition_point(|&r| r <= n + 1.0);
        self.rises[upto.saturating_sub(2)..upto]
            .iter()
            .map(|&r| {
                let up = (0.1 + (n - r) * 0.1).clamp(0.0, 0.5);
                let down = (0.5 - (n - (r + self.width)) * 0.1).clamp(0.0, 0.5);
                up.min(down)
            })
            .fold(0.0, f64::max) as f32
    }
}

impl Trains {
    fn block(trains: &[(usize, &Train)], start: usize, frames: usize) -> Self {
        let mut channels = vec![vec![0.0; frames]; CHANNELS];
        for &(channel, train) in trains {
            for (i, sample) in channels[channel - 1].iter_mut().enumerate() {
                *sample = train.level(start + i);
            }
        }
        Self(channels)
    }
}

impl DeviceInput for Trains {
    fn channels(&self) -> usize {
        self.0.len()
    }

    fn channel(&self, index: usize) -> &[f32] {
        &self.0[index]
    }
}

/// A rig and the pulses fed to it, run block by block.
struct Bench {
    rig: Rig,
    frames: usize,
    at: usize,
}

impl Bench {
    fn new(frames: usize) -> Self {
        Self {
            rig: rig(frames),
            frames,
            at: 0,
        }
    }

    /// Runs to frame `until`, feeding `trains` (channel, train).
    fn run_to(&mut self, until: usize, trains: &[(usize, &Train)]) {
        while self.at < until {
            let input = Trains::block(trains, self.at, self.frames);
            self.rig.block_with_input(self.frames, &input);
            self.at += self.frames;
        }
    }

    /// The tempo the song plays at, if it follows one.
    fn following(&mut self) -> Option<f64> {
        self.rig.monitor.state().following
    }

    fn leader(&mut self) -> Option<DeviceId> {
        self.rig.monitor.state().cv_leader.map(|l| l.device)
    }
}

/// The pulses of a clock at `bpm`, 4 to the quarter, from frame `first`:
/// the interval in frames.
fn interval(bpm: f64) -> f64 {
    60.0 / bpm / 4.0 * RATE.as_f64()
}

// Spec: cv-tools R23, R24
#[test]
fn a_pulse_train_leads_the_tempo_through_a_step_and_holds_when_it_stops() {
    // 0.2 ms of jitter is 9.6 frames.
    for (frames, seed) in BLOCKS
        .into_iter()
        .flat_map(|b| [crate::clock_in::SEEDS[0], crate::clock_in::SEEDS[2]].map(|s| (b, s)))
    {
        let first = 1000.0;
        let steady = Train::steady(first, interval(100.0), 24, 9.6, seed);
        let step_at = first + 24.0 * interval(100.0);
        let quick = Train::steady(step_at, interval(130.0), 24, 9.6, seed ^ 1);
        let train = Train {
            rises: steady.rises.iter().chain(&quick.rises).copied().collect(),
            width: interval(130.0) / 2.0,
        };
        let mut bench = Bench::new(frames);
        let a = track(&mut bench.rig.editor, TrackKind::Audio, None);
        let device = clock_in(&mut bench.rig, a, 2, &[]);
        let pulses = [(2usize, &train)];

        bench.run_to((first + 3.0 * 4.0 * interval(100.0)) as usize, &pulses);
        let tempo = bench.following().expect("it follows");
        assert!(
            (tempo - 100.0).abs() < 0.3,
            "{tempo} after three quarter notes at blocks of {frames}"
        );
        assert_eq!(bench.leader(), Some(device), "it leads");
        bench.run_to(step_at as usize - 100, &pulses);
        assert!((bench.following().unwrap() - 100.0).abs() < 0.3);

        // A quarter note after the step to 130.
        bench.run_to((step_at + 4.0 * interval(130.0) + 2000.0) as usize, &pulses);
        let tempo = bench.following().unwrap();
        assert!(
            (tempo - 130.0).abs() < 1.0,
            "{tempo} a quarter note after the step, blocks of {frames}"
        );

        // The pulses stop: the last tempo holds for as long as it takes.
        let ends = *train.rises.last().unwrap() as usize;
        bench.run_to(ends + 4 * RATE.hz() as usize, &pulses);
        let held = bench.following().expect("still leads in silence");
        assert!((held - 130.0).abs() < 1.0, "held {held}");
        assert_eq!(bench.leader(), Some(device));
        let state = bench.rig.monitor.state();
        assert!(!state.playing, "a clock never starts the transport");
        assert_eq!(state.position_ticks, Ticks::ZERO, "nor locates it");
    }
}

// Spec: cv-tools R24
#[test]
fn the_beat_is_kept_to_the_pulses_counted_from_the_first() {
    for frames in [64, 512] {
        let jitter = 9.6;
        let train = Train::steady(
            2000.0,
            interval(100.0),
            60,
            jitter,
            crate::clock_in::SEEDS[1],
        );
        let mut bench = Bench::new(frames);
        let a = track(&mut bench.rig.editor, TrackKind::Audio, None);
        clock_in(&mut bench.rig, a, 2, &[]);
        bench.rig.sender.transport(TransportCommand::Play);
        let pulses = [(2usize, &train)];
        // The song's position at each pulse, from the block that follows it.
        let mut at_pulse = Vec::new();
        for &rise in &train.rises {
            bench.run_to(rise as usize + frames, &pulses);
            let state = bench.rig.monitor.state();
            // The block began after the pulse: back up by the ticks between.
            let started = (bench.at - frames) as f64;
            let ticks_per_frame = TICKS_PER_QUARTER as f64 * 100.0 / 60.0 / RATE.as_f64();
            at_pulse.push(state.position_ticks.get() as f64 + (rise - started) * ticks_per_frame);
        }
        // From the tenth pulse on, each is a pulse's length of a quarter
        // note after the one before, within a thirty-second of a quarter.
        let want = TICKS_PER_QUARTER as f64 / 4.0;
        let worst = at_pulse[10..]
            .windows(2)
            .map(|w| (w[1] - w[0] - want).abs())
            .fold(0.0, f64::max);
        assert!(
            worst < want / 8.0,
            "off by {worst} ticks at blocks of {frames}"
        );
        let drift = at_pulse[59] - at_pulse[10] - 49.0 * want;
        assert!(
            drift.abs() < want / 8.0,
            "drifted {drift} ticks at blocks of {frames}"
        );
    }
}

// Spec: cv-tools R25
#[test]
fn a_second_clock_in_follows_nothing_and_the_first_in_track_order_leads() {
    let slow = Train::steady(1000.0, interval(100.0), 48, 0.0, 1);
    let quick = Train::steady(1000.0, interval(130.0), 60, 0.0, 1);
    let pulses = [(1usize, &slow), (2usize, &quick)];
    let mut bench = Bench::new(64);
    // A new track goes to the front, so `b` is the first in track order.
    let a = track(&mut bench.rig.editor, TrackKind::Audio, None);
    let b = track(&mut bench.rig.editor, TrackKind::Audio, None);
    let second = clock_in(&mut bench.rig, a, 2, &[]);
    let first = clock_in(&mut bench.rig, b, 1, &[]);
    bench.run_to(60_000, &pulses);
    assert_eq!(bench.leader(), Some(first));
    assert!((bench.following().unwrap() - 100.0).abs() < 0.3);
    // Its Sync off, the one after takes the lead.
    bench.rig.apply(Command::SetDeviceParam {
        device: first,
        param: cv_clock_in::SYNC,
        value: 0.0,
    });
    bench.run_to(120_000, &pulses);
    assert_eq!(bench.leader(), Some(second));
    assert!((bench.following().unwrap() - 130.0).abs() < 0.5);
    // Both off: nothing leads.
    bench.rig.apply(Command::SetDeviceParam {
        device: second,
        param: cv_clock_in::SYNC,
        value: 0.0,
    });
    bench.run_to(130_000, &pulses);
    assert_eq!(bench.leader(), None);
    assert_eq!(bench.following(), None);
}

// Spec: cv-tools R25
#[test]
fn a_midi_clock_under_external_sync_leads_over_both() {
    let frames = 64;
    let mut bench = Bench::new(frames);
    let a = track(&mut bench.rig.editor, TrackKind::Audio, None);
    clock_in(&mut bench.rig, a, 1, &[]);
    bench.rig.sync();
    let port = PortId::from_name("Sequencer");
    let (mut queue, consumer) = RingBuffer::new(8192);
    assert!(bench.rig.processor.set_midi_input(consumer).is_none());
    bench.rig.sender.sync_inputs(&[SyncInput {
        port,
        delay_ms: 0,
        timecode: None,
    }]);
    bench.rig.sender.external_sync(true);
    assert!(bench.rig.sender.flush());
    // A MIDI clock at 90 BPM, and a CV clock at 120.
    let times = crate::clock_in::clock_times(10e6, 12.0, |_| 90.0);
    let train = Train::steady(1000.0, interval(120.0), 80, 0.0, 1);
    let start = TestClock::new().now();
    let mut sent = 0;
    while bench.at < 5 * RATE.hz() as usize {
        let from = bench.at as f64 * 1e9 / RATE.as_f64();
        while sent < times.len() && times[sent] < from {
            queue
                .push(MidiInputEvent {
                    source: MidiSource::Sync(port),
                    time: start + Duration::from_nanos(times[sent] as u64),
                    message: MidiMessage::CLOCK,
                })
                .unwrap();
            sent += 1;
        }
        let input = Trains::block(&[(1, &train)], bench.at, frames);
        let now = start + Duration::from_nanos(from as u64);
        let processor = &mut bench.rig.processor;
        bl_rt::rt_test!({
            processor.process_at(frames, &input, now);
        });
        bench.at += frames;
    }
    let tempo = bench.following().expect("a clock leads");
    assert!(
        (tempo - 90.0).abs() < 1.0,
        "the MIDI clock's, not the CV's: {tempo}"
    );
    assert_eq!(bench.leader(), None, "the CV Clock In follows nothing");
}

// Spec: cv-tools R23
#[test]
fn threshold_sets_where_a_rise_counts_as_a_pulse() {
    let mut bench = Bench::new(64);
    let a = track(&mut bench.rig.editor, TrackKind::Audio, None);
    let device = clock_in(&mut bench.rig, a, 1, &[(cv_clock_in::THRESHOLD, 2.0)]);
    let train = Train::steady(1000.0, interval(100.0), 12, 0.0, 1);
    bench.run_to(60_000, &[(1, &train)]);
    assert!((bench.following().unwrap() - 100.0).abs() < 0.3);
    assert_eq!(bench.leader(), Some(device));
    // Threshold above the pulse's top (5 V) finds none.
    let mut bench = Bench::new(64);
    let a = track(&mut bench.rig.editor, TrackKind::Audio, None);
    clock_in(&mut bench.rig, a, 1, &[(cv_clock_in::THRESHOLD, 6.0)]);
    bench.run_to(60_000, &[(1, &train)]);
    assert_eq!(bench.following(), None);
}

// Spec: cv-tools R24
#[test]
fn a_clock_leaves_the_transport_alone_while_the_song_plays_and_stops() {
    let train = Train::steady(1000.0, interval(100.0), 24, 0.0, 1);
    let mut bench = Bench::new(64);
    let a = track(&mut bench.rig.editor, TrackKind::Audio, None);
    clock_in(&mut bench.rig, a, 1, &[]);
    bench.run_to(30_000, &[(1, &train)]);
    assert!(!bench.rig.monitor.state().playing);
    bench.rig.sender.transport(TransportCommand::Play);
    bench.run_to(60_000, &[(1, &train)]);
    assert!(
        bench.rig.monitor.state().playing,
        "still the person's start"
    );
    bench.rig.sender.transport(TransportCommand::Stop);
    bench.run_to(90_000, &[(1, &train)]);
    assert!(!bench.rig.monitor.state().playing);
}

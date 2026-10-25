//! MIDI clock out (WP-7.11, `docs/specs/midi-clock.md` R3 to R8, R11 to R13):
//! the Timing Clock, Start, Continue, Stop and Song Position Pointer the
//! engine stamps for each output with Sync on, on the frame each falls on.
//! Every block runs as a real-time section under the allocation trap, the
//! callbacks exactly on time.

use crate::common;
use std::time::{Duration, Instant};

use bl_core::midi::{ClockType, HardwareResync, MidiOutputEvent, PortId, SyncOutput};
use bl_core::time::{Bpm, TempoMap, Ticks};
use bl_engine::{LoopRegion, NoInput, TransportCommand};
use bl_model::{Command, CountIn, MetronomeSettings};
use common::{RATE, Rig};
use rtrb::{Consumer, RingBuffer};

const BAR: Ticks = Ticks::WHOLE;
const BEAT: Ticks = Ticks::QUARTER;
const CLOCK: Ticks = Ticks(Ticks::QUARTER.0 / 24);
const CLOCK_BYTES: [u8; 3] = [0xF8, 0, 0];
const START_BYTES: [u8; 3] = [0xFA, 0, 0];
const CONTINUE_BYTES: [u8; 3] = [0xFB, 0, 0];
const STOP_BYTES: [u8; 3] = [0xFC, 0, 0];

fn port() -> PortId {
    PortId::from_name("Drum machine")
}

fn output(delay_ms: i16, clock_type: ClockType, resync: HardwareResync) -> SyncOutput {
    SyncOutput {
        port: port(),
        delay_ms,
        clock_type,
        resync,
    }
}

fn song() -> SyncOutput {
    output(0, ClockType::Song, HardwareResync::StopAndStart)
}

fn bpm(bpm: f64) -> Bpm {
    Bpm::new(bpm).unwrap()
}

/// The length of `frames` frames at 48 kHz.
fn frames(frames: u64) -> Duration {
    Duration::from_nanos(frames * 1_000_000_000 / 48_000)
}

/// A rig with a queue out, playing in blocks of `block` frames.
struct Clock {
    rig: Rig,
    out: Consumer<MidiOutputEvent>,
    start: Instant,
    block: usize,
    blocks: u64,
    latency: u32,
}

impl Clock {
    fn new(block: usize, latency: u32, queue: usize, outputs: &[SyncOutput]) -> Self {
        let mut rig = Rig::with_config(block, |config| config.output_latency = latency);
        let (producer, out) = RingBuffer::new(queue);
        assert!(rig.processor.set_midi_output(producer).is_none());
        rig.sender.sync_outputs(outputs);
        assert!(rig.sender.flush());
        Self {
            rig,
            out,
            start: Instant::now(),
            block,
            blocks: 0,
            latency,
        }
    }

    fn with_tempo(mut self, tempo: TempoMap) -> Self {
        self.rig.apply(Command::SetTempo { tempo });
        self
    }

    fn command(&mut self, command: TransportCommand) {
        self.rig.sender.transport(command);
        assert!(self.rig.sender.flush());
    }

    fn run(&mut self) {
        let now = self.start + frames(self.blocks * self.block as u64);
        let (processor, block) = (&mut self.rig.processor, self.block);
        bl_rt::rt_test!({
            processor.process_at(block, &NoInput, now);
        });
        self.blocks += 1;
    }

    /// Runs whole blocks until `total` frames have played since the start.
    fn run_to(&mut self, total: u64) {
        while self.blocks * (self.block as u64) < total {
            self.run();
        }
    }

    /// Everything sent since the last call.
    fn sent(&mut self) -> Vec<MidiOutputEvent> {
        let mut sent = Vec::new();
        while let Ok(event) = self.out.pop() {
            sent.push(event);
        }
        sent
    }

    /// The moment engine frame `frame` is heard.
    fn heard(&self, frame: i64) -> Instant {
        self.start + frames(frame as u64 + u64::from(self.latency))
    }
}

/// How far `a` is from `b`, in nanoseconds.
fn apart(a: Instant, b: Instant) -> u128 {
    a.max(b).duration_since(a.min(b)).as_nanos()
}

fn bytes(events: &[MidiOutputEvent]) -> Vec<[u8; 3]> {
    events.iter().map(|e| e.message.padded()).collect()
}

fn clocks(events: &[MidiOutputEvent]) -> Vec<&MidiOutputEvent> {
    events
        .iter()
        .filter(|e| e.message.padded() == CLOCK_BYTES)
        .collect()
}

/// Checks every clock in `sent` is the stamp of its tick's frame through
/// `tempo`, for a song that started at engine frame `started`.
fn assert_clocks_on_their_frames(
    clock: &Clock,
    sent: &[MidiOutputEvent],
    tempo: &TempoMap,
    started: i64,
    first: i64,
    what: &str,
) {
    let all = clocks(sent);
    assert!(all.len() > 40, "{what}: only {} clocks", all.len());
    for (n, event) in all.iter().enumerate() {
        let tick = Ticks((first + n as i64) * CLOCK.0);
        let frame = started + tempo.ticks_to_frames(tick, RATE).get();
        let error = apart(event.time, clock.heard(frame));
        assert!(
            error <= 1_500,
            "{what}: clock {n} is {error} ns from its frame"
        );
    }
}

// Spec: midi-clock R3, R4, R13
#[test]
fn clocks_are_stamped_for_their_frames_whatever_the_block() {
    for block in [1, 61, 512, 4096] {
        let mut clock = Clock::new(block, 300, 8192, &[song()]);
        clock.run();
        clock.command(TransportCommand::Play);
        clock.run();
        let started = (clock.blocks as i64 - 1) * block as i64;
        clock.run_to(if block == 1 { 48_000 } else { 96_000 });
        let sent = clock.sent();
        assert_eq!(
            bytes(&sent)[..2],
            [START_BYTES, CLOCK_BYTES],
            "block {block}"
        );
        let tempo = TempoMap::constant(bpm(120.0));
        assert_clocks_on_their_frames(&clock, &sent, &tempo, started, 0, &format!("block {block}"));
        // 1,000 frames a clock at 120 BPM and 48 kHz: 20.8333 ms.
        let all = clocks(&sent);
        for pair in all.windows(2) {
            let gap = pair[1].time.duration_since(pair[0].time).as_nanos();
            assert!(
                gap.abs_diff(20_833_333) <= 2_000,
                "block {block}: gap {gap} ns"
            );
        }
        let state = clock.rig.monitor.state().sync_out;
        assert!(state.sending);
        assert_eq!((state.late, state.dropped), (0, 0));
        assert_eq!(state.clocks as usize, all.len());
        assert!(state.quarters >= 2);
    }
}

// Spec: midi-clock R3
#[test]
fn clocks_follow_a_tempo_step_and_a_ramp_through_the_map() {
    let step = TempoMap::from_breakpoints([
        (Ticks::ZERO, bpm(120.0)),
        (BAR, bpm(120.0)),
        (BAR, bpm(150.0)),
    ])
    .unwrap();
    let ramp =
        TempoMap::from_breakpoints([(Ticks::ZERO, bpm(90.0)), (BAR * 2, bpm(180.0))]).unwrap();
    for (what, tempo) in [("step", step), ("ramp", ramp)] {
        for block in [61, 512, 4096] {
            let mut clock = Clock::new(block, 0, 8192, &[song()]).with_tempo(tempo.clone());
            clock.run();
            clock.command(TransportCommand::Play);
            clock.run();
            let started = (clock.blocks as i64 - 1) * block as i64;
            clock.run_to(5 * 48_000);
            let sent = clock.sent();
            assert_clocks_on_their_frames(&clock, &sent, &tempo, started, 0, what);
        }
    }
}

// Spec: midi-clock R5, R7
#[test]
fn song_sends_start_then_a_pointer_and_continue_stop() {
    let mut clock = Clock::new(512, 0, 8192, &[song()]);
    clock.run();
    // A move of the insert marker while stopped sends the pointer alone.
    clock.command(TransportCommand::Locate(BAR * 2 + BEAT));
    clock.run();
    assert_eq!(bytes(&clock.sent()), vec![[0xF2, 36, 0]]);
    clock.run();
    assert!(clock.sent().is_empty(), "nothing while stopped");

    // From 3.2.1: the pointer for 36 sixteenths, Continue, then the clock
    // on that frame.
    clock.command(TransportCommand::Play);
    clock.run();
    let sent = clock.sent();
    assert_eq!(
        bytes(&sent)[..3],
        [[0xF2, 36, 0], CONTINUE_BYTES, CLOCK_BYTES]
    );
    assert!(sent[..3].iter().all(|e| e.time == sent[0].time));

    // From 1.1.1: Start, then the clock on the first frame.
    clock.command(TransportCommand::Stop);
    clock.run();
    assert_eq!(bytes(&clock.sent())[0], STOP_BYTES);
    clock.run();
    clock.command(TransportCommand::Stop);
    clock.run();
    assert_eq!(bytes(&clock.sent()), vec![[0xF2, 0, 0]]);
    clock.command(TransportCommand::Play);
    clock.run();
    let sent = clock.sent();
    assert_eq!(bytes(&sent)[..2], [START_BYTES, CLOCK_BYTES]);
    assert_eq!(sent[0].time, sent[1].time);
}

// Spec: midi-clock R5, R7. The loop's wrap and a locate are jumps.
#[test]
fn song_sends_a_pointer_and_continue_on_the_frame_of_a_jump() {
    let mut clock = Clock::new(512, 0, 8192, &[song()]);
    clock.command(TransportCommand::SetLoop(Some(LoopRegion {
        start: Ticks::ZERO,
        end: BAR,
    })));
    clock.run();
    clock.command(TransportCommand::Play);
    clock.run_to(96_000 + 1024);
    let sent = clock.sent();
    // The loop ends on frame 96,000 of a song that began at frame 512.
    let wrap = clock.heard(512 + 96_000);
    let at_wrap: Vec<[u8; 3]> = sent
        .iter()
        .filter(|e| e.time == wrap)
        .map(|e| e.message.padded())
        .collect();
    assert_eq!(at_wrap, vec![[0xF2, 0, 0], CONTINUE_BYTES, CLOCK_BYTES]);
    assert_eq!(
        sent.iter()
            .filter(|e| e.message.padded()[0] == 0xF2)
            .count(),
        1,
        "a pointer for the wrap only"
    );

    // A locate while playing does the same at its block's start.
    clock.command(TransportCommand::Locate(BAR + BEAT * 2));
    clock.run();
    let sent = clock.sent();
    assert_eq!(
        bytes(&sent)[..3],
        [[0xF2, 24, 0], CONTINUE_BYTES, CLOCK_BYTES]
    );
}

// Spec: midi-clock R6, R7
#[test]
fn pattern_sends_start_on_the_next_bar_line_only() {
    let mut clock = Clock::new(
        256,
        0,
        8192,
        &[output(0, ClockType::Pattern, HardwareResync::Off)],
    );
    clock.run();
    clock.command(TransportCommand::Locate(BEAT));
    clock.run();
    assert!(clock.sent().is_empty(), "a moved marker sends nothing");
    clock.command(TransportCommand::Play);
    clock.run_to(2 * 96_000);
    let sent = clock.sent();
    let all = bytes(&sent);
    assert_eq!(all[0], CLOCK_BYTES, "clocks run from the start of playback");
    let starts: Vec<usize> = (0..all.len()).filter(|&i| all[i] == START_BYTES).collect();
    assert_eq!(starts.len(), 1);
    assert_eq!(all[starts[0] + 1], CLOCK_BYTES);
    assert_eq!(sent[starts[0]].time, sent[starts[0] + 1].time);
    // Beat 2 to the bar line: three beats, 72 clocks, before Start.
    assert_eq!(starts[0], 72);
    assert!(all.iter().all(|b| b[0] != 0xF2 && b[0] != 0xFB));

    // A locate sends nothing for a Pattern output but the clocks go on.
    clock.command(TransportCommand::Locate(BAR * 4));
    clock.run();
    assert!(bytes(&clock.sent()).iter().all(|b| *b == CLOCK_BYTES));
}

// Spec: midi-clock R8
#[test]
fn a_count_in_sends_nothing_until_the_transport_moves() {
    let mut clock = Clock::new(256, 0, 8192, &[song()]);
    clock.command(TransportCommand::SetMetronome(MetronomeSettings {
        enabled: true,
        count_in: CountIn::Bars1,
        ..MetronomeSettings::default()
    }));
    clock.run();
    clock.command(TransportCommand::Play);
    // A 4/4 bar at 120 BPM is 96,000 frames; the count-in takes one.
    clock.run_to(96_000);
    assert!(clock.sent().is_empty());
    assert!(!clock.rig.monitor.state().sync_out.sending);
    clock.run();
    clock.run();
    let sent = clock.sent();
    assert_eq!(bytes(&sent)[..2], [START_BYTES, CLOCK_BYTES]);
}

// Spec: midi-clock R4
#[test]
fn the_sync_delay_moves_every_stamp_by_exactly_itself() {
    let base = Instant::now();
    let play = |delay: i16, block: usize, latency: u32| {
        let mut clock = Clock::new(
            block,
            latency,
            8192,
            &[output(delay, ClockType::Song, HardwareResync::Off)],
        );
        clock.start = base;
        clock.run();
        clock.command(TransportCommand::Play);
        clock.run_to(3 * 96_000);
        let sent = clock.sent();
        let state = clock.rig.monitor.state().sync_out;
        (sent, state)
    };
    let (zero, _) = play(0, 512, 0);
    for delay in [50, -50] {
        let (moved, state) = play(delay, 512, 0);
        // The clocks written ahead for a negative delay reach the queue
        // earlier, so compare by stamp.
        let key = |e: &MidiOutputEvent| (e.time, e.message.padded());
        let mut want: Vec<_> = zero.iter().map(key).collect();
        let mut got: Vec<_> = moved.iter().map(key).collect();
        let shift = Duration::from_millis(50);
        for (time, _) in &mut want {
            *time = if delay > 0 {
                *time + shift
            } else {
                *time - shift
            };
        }
        want.sort();
        got.sort();
        let n = got.len().min(want.len());
        assert!(n > 100, "delay {delay}: {n} messages");
        for (got, want) in got[..n].iter().zip(&want[..n]) {
            // The stamps are rounded to a nanosecond.
            assert_eq!(got.1, want.1);
            assert!(
                apart(got.0, want.0) <= 2,
                "delay {delay} moves every stamp by exactly itself"
            );
        }
        // Spec: midi-clock R4. No clock is stamped for a moment already
        // past once the song runs: they were written ahead.
        assert_eq!(state.late, 0, "delay {delay}");
    }
}

// Spec: midi-clock R13
#[test]
fn a_full_queue_drops_the_clock_and_counts_it() {
    let mut clock = Clock::new(4096, 0, 8, &[song()]);
    clock.run();
    clock.command(TransportCommand::Play);
    clock.run_to(48_000);
    let state = clock.rig.monitor.state().sync_out;
    assert!(state.dropped > 0);
    assert_eq!(clock.sent().len(), 8);
}

// Spec: midi-clock R11
#[test]
fn resync_sends_each_ports_choice_before_the_next_bar_lines_clock() {
    let second = PortId::from_name("Sequencer");
    let outputs = [
        output(0, ClockType::Song, HardwareResync::StopAndStart),
        SyncOutput {
            port: second,
            delay_ms: 0,
            clock_type: ClockType::Song,
            resync: HardwareResync::StartOnly,
        },
        SyncOutput {
            port: PortId::from_name("Drone"),
            delay_ms: 0,
            clock_type: ClockType::Song,
            resync: HardwareResync::Off,
        },
    ];
    let mut clock = Clock::new(256, 0, 8192, &outputs);
    clock.run();
    // While stopped the command does nothing.
    clock.command(TransportCommand::ResyncExternalHardware);
    clock.run();
    assert!(clock.sent().is_empty());
    clock.command(TransportCommand::Play);
    clock.run_to(30_000);
    clock.sent();
    clock.command(TransportCommand::ResyncExternalHardware);
    // The next bar line is at frame 96,000 of a song started at frame 512.
    clock.run_to(97_000);
    let sent = clock.sent();
    let bar_line = clock.heard(512 + 96_000);
    let at_bar: Vec<(PortId, [u8; 3])> = sent
        .iter()
        .filter(|e| e.time == bar_line)
        .map(|e| (e.port, e.message.padded()))
        .collect();
    let drums = port();
    assert!(at_bar.contains(&(drums, STOP_BYTES)));
    assert!(at_bar.contains(&(drums, START_BYTES)));
    assert!(at_bar.contains(&(second, START_BYTES)));
    assert!(!at_bar.contains(&(second, STOP_BYTES)));
    let drone: Vec<_> = sent
        .iter()
        .filter(|e| e.port == PortId::from_name("Drone"))
        .filter(|e| e.message.padded() != CLOCK_BYTES)
        .collect();
    assert!(drone.is_empty(), "Don't Resync sends nothing");
    // Stop and Start comes before the bar line's clock for that port.
    let order: Vec<[u8; 3]> = sent
        .iter()
        .filter(|e| e.port == drums && e.time == bar_line)
        .map(|e| e.message.padded())
        .collect();
    assert_eq!(order, vec![STOP_BYTES, START_BYTES, CLOCK_BYTES]);
}

// Spec: midi-clock R13. An engine with no queue to the ports, such as an
// offline render's, sends no clock.
#[test]
fn an_engine_without_a_queue_to_the_ports_sends_no_clock() {
    let mut rig = Rig::new(512);
    rig.sender.sync_outputs(&[song()]);
    assert!(rig.sender.flush());
    rig.sender.transport(TransportCommand::Play);
    assert!(rig.sender.flush());
    rig.render(96_000, 512);
    let state = rig.monitor.state();
    assert!(state.playing);
    assert_eq!(state.sync_out.clocks, 0);
}

// Spec: midi-clock R3, R5. A new tempo map swapped in while the song plays
// moves frames under the playhead but not its beat: it is no jump, so no
// pointer or Continue goes out, and the clocks carry on at the new tempo.
#[test]
fn a_tempo_change_while_playing_is_not_a_jump() {
    let mut clock = Clock::new(512, 0, 8192, &[song()]);
    clock.run();
    clock.command(TransportCommand::Play);
    clock.run_to(30_000);
    clock.sent();
    clock.rig.apply(Command::SetTempo {
        tempo: TempoMap::constant(bpm(90.0)),
    });
    clock.run_to(120_000);
    let sent = clock.sent();
    assert!(clocks(&sent).len() > 40, "the clocks go on");
    assert!(
        sent.iter()
            .all(|e| e.message.padded()[0] != 0xF2 && e.message.padded()[0] != 0xFB),
        "no pointer or Continue for a tempo change"
    );
}

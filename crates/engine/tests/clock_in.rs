//! MIDI clock in (WP-7.12, `docs/specs/midi-clock.md` R14 to R24, R26): an
//! input's Timing Clock, Start, Continue, Stop and Song Position Pointer,
//! sent from a test port at times of the test clock with seeded jitter,
//! followed with EXT on. Every block runs as a real-time section under the
//! allocation trap.

use crate::common;
use std::time::{Duration, Instant};

use bl_core::clock::TestClock;
use bl_core::midi::{
    ClockType, HardwareResync, MidiInputEvent, MidiMessage, MidiOutputEvent, MidiSource, PortId,
    SyncInput, SyncOutput,
};
use bl_core::time::{Bpm, TICKS_PER_QUARTER, TempoMap, Ticks};
use bl_engine::{LoopRegion, NoInput, TransportCommand, testing};
use bl_model::{
    Clip, ClipData, Command, CountIn, MetronomeRhythm, MetronomeSound, MidiData, Note, TrackKind,
};
use common::{RATE, Rig, device, track};
use rtrb::{Producer, RingBuffer};

/// The block sizes every test runs at.
pub(crate) const BLOCKS: [usize; 4] = [1, 61, 512, 4096];

/// The seeds of the jitter each test runs under.
pub(crate) const SEEDS: [u64; 6] = [
    0x9E37_79B9_7F4A_7C15,
    0x1234_5678_9ABC_DEF1,
    0xDEAD_BEEF_0BAD_F00D,
    0x0123_4567_89AB_CDEF,
    0xFEDC_BA98_7654_3210,
    0xA5A5_5A5A_C3C3_3C3C,
];

const QUARTER: i64 = TICKS_PER_QUARTER;
const CLOCK: f64 = (TICKS_PER_QUARTER / 24) as f64;
const SIXTEENTH: f64 = 6.0 * CLOCK;

fn port() -> PortId {
    PortId::from_name("Sequencer")
}

/// Nanoseconds in `frames` frames.
fn frames_ns(frames: u64) -> f64 {
    frames as f64 * 1e9 / RATE.as_f64()
}

/// A small seeded generator: the same jitter every run.
pub(crate) struct Random(pub(crate) u64);

impl Random {
    /// A number from -1 to 1.
    pub(crate) fn next(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) as f64 / (1u64 << 52) as f64 - 1.0
    }
}

/// The arrival time, in nanoseconds from the start, of every Timing Clock of
/// `quarters` quarter notes, when the tempo is `bpm` at each quarter note
/// of the song, the first at `first`.
pub(crate) fn clock_times(first: f64, quarters: f64, bpm: impl Fn(f64) -> f64) -> Vec<f64> {
    let mut times = Vec::new();
    let (mut t, mut q) = (first, 0.0);
    while q < quarters {
        times.push(t);
        t += 1e9 * 60.0 / (bpm(q) * 24.0);
        q += 1.0 / 24.0;
    }
    times
}

/// A rig whose input queue a test port fills, on a test clock.
pub(crate) struct Host {
    pub(crate) rig: Rig,
    queue: Producer<MidiInputEvent>,
    start: Instant,
    pub(crate) block: usize,
    pub(crate) blocks: u64,
    /// What waits to arrive: time in nanoseconds from the start, message,
    /// and the port it comes from.
    waiting: Vec<(f64, MidiMessage, PortId)>,
}

impl Host {
    pub(crate) fn new(block: usize, tempo: f64) -> Self {
        Self::with(block, tempo, |_| {})
    }

    fn with(block: usize, tempo: f64, build: impl FnOnce(&mut Rig)) -> Self {
        let mut rig = Rig::new(block);
        rig.editor
            .apply(Command::SetTempo {
                tempo: TempoMap::constant(Bpm::new(tempo).unwrap()),
            })
            .unwrap();
        build(&mut rig);
        rig.sync();
        let (queue, consumer) = RingBuffer::new(8192);
        assert!(rig.processor.set_midi_input(consumer).is_none());
        rig.sender.sync_inputs(&[SyncInput {
            port: port(),
            delay_ms: 0,
        }]);
        rig.sender.external_sync(true);
        assert!(rig.sender.flush());
        Self {
            rig,
            queue,
            start: TestClock::new().now(),
            block,
            blocks: 0,
            waiting: Vec::new(),
        }
    }

    /// Has `message` arrive `ns` nanoseconds after the start.
    fn at(&mut self, ns: f64, message: MidiMessage) {
        self.at_port(port(), ns, message);
    }

    fn at_port(&mut self, from: PortId, ns: f64, message: MidiMessage) {
        self.waiting.push((ns, message, from));
        self.waiting.sort_by(|a, b| a.0.total_cmp(&b.0));
    }

    pub(crate) fn clocks(&mut self, times: &[f64]) {
        for &t in times {
            self.at(t, MidiMessage::CLOCK);
        }
    }

    /// The start of block `n`, in nanoseconds from the start.
    pub(crate) fn block_ns(&self, n: u64) -> f64 {
        frames_ns(n * self.block as u64)
    }

    /// Runs one block, handing over everything that arrived before it.
    pub(crate) fn run(&mut self) {
        let from = self.block_ns(self.blocks);
        while self.waiting.first().is_some_and(|w| w.0 < from) {
            let (ns, message, from_port) = self.waiting.remove(0);
            self.queue
                .push(MidiInputEvent {
                    source: MidiSource::Sync(from_port),
                    time: self.start + Duration::from_nanos(ns.max(0.0) as u64),
                    message,
                })
                .unwrap();
        }
        let now = self.start + Duration::from_nanos(from as u64);
        let processor = &mut self.rig.processor;
        let block = self.block;
        bl_rt::rt_test!({
            processor.process_at(block, &NoInput, now);
        });
        self.blocks += 1;
    }

    /// Runs blocks until the one that starts at `ns` or after has run.
    pub(crate) fn run_to(&mut self, ns: f64) {
        while self.block_ns(self.blocks) <= ns {
            self.run();
        }
    }

    fn tempo(&mut self) -> Option<f64> {
        self.rig.monitor.state().sync_in.tempo
    }

    fn playing(&mut self) -> bool {
        self.rig.monitor.state().playing
    }

    fn receiving(&mut self) -> bool {
        self.rig.monitor.state().sync_in.receiving
    }

    fn following(&mut self) -> Option<f64> {
        self.rig.monitor.state().following
    }

    /// The song position in ticks the last block started on.
    fn ticks(&mut self) -> f64 {
        self.rig.monitor.state().position_ticks.get() as f64
    }

    pub(crate) fn send(&mut self, command: TransportCommand) {
        self.rig.sender.transport(command);
        assert!(self.rig.sender.flush());
    }
}

#[test]
fn a_steady_clock_with_jitter_is_followed_within_a_tenth_of_a_bpm_and_holds() {
    // Spec: midi-clock R18. ±1 ms jitter on 120 BPM.
    for (block, seed) in BLOCKS.into_iter().flat_map(|b| SEEDS.map(move |s| (b, s))) {
        let mut host = Host::new(block, 90.0);
        let mut random = Random(seed);
        let times: Vec<f64> = clock_times(10e6, 40.0, |_| 120.0)
            .into_iter()
            .map(|t| t + random.next() * 1e6)
            .collect();
        host.clocks(&times);
        // Two quarter notes in.
        host.run_to(10e6 + 2.0 * 500e6 + 100e6);
        let tempo = host.tempo().unwrap();
        assert!(
            (tempo - 120.0).abs() < 0.1,
            "{tempo} at blocks of {block} after two quarter notes"
        );
        // Then, once the fit has settled, its two decimals move by no more
        // than 0.05 BPM in a second.
        host.run_to(10e6 + 4.0e9);
        let mut last = host.tempo().unwrap();
        let mut at = 10e6 + 5.0e9;
        while at < 10e6 + 38.0 * 500e6 {
            host.run_to(at);
            let now = host.tempo().unwrap();
            assert!(
                (now - last).abs() <= 0.05 + 1e-9,
                "{last} to {now} at {at} ns, blocks of {block}"
            );
            assert!((now - 120.0).abs() < 0.1, "{now} at {at}");
            last = now;
            at += 1e9;
        }
        let state = host.rig.monitor.state();
        assert!(
            state.following.is_some(),
            "the song plays the followed tempo"
        );
        assert_eq!(state.sync_in.glitches, 0);
    }
}

#[test]
fn a_step_to_130_is_followed_within_half_a_bpm_after_a_quarter_note() {
    // Spec: midi-clock R18.
    for (block, seed) in BLOCKS.into_iter().flat_map(|b| SEEDS.map(move |s| (b, s))) {
        let mut host = Host::new(block, 90.0);
        let mut random = Random(seed);
        let steady = clock_times(10e6, 8.0, |_| 120.0);
        let step_at = *steady.last().unwrap() + 1e9 * 60.0 / (120.0 * 24.0);
        let stepped = clock_times(step_at, 4.0, |_| 130.0);
        let times: Vec<f64> = steady
            .iter()
            .chain(&stepped)
            .map(|t| t + random.next() * 1e6)
            .collect();
        host.clocks(&times);
        host.run_to(step_at - 1e6);
        assert!((host.tempo().unwrap() - 120.0).abs() < 0.1);
        // One quarter note after the step, at 130 BPM.
        host.run_to(step_at + 1e9 * 60.0 / 130.0 + 80e6);
        let tempo = host.tempo().unwrap();
        assert!(
            (tempo - 130.0).abs() < 0.5,
            "{tempo} at blocks of {block} a quarter note after the step"
        );
    }
}

#[test]
fn a_ramp_to_140_over_eight_bars_stays_within_a_bpm_of_the_clock() {
    // Spec: midi-clock R18. 120 to 140 BPM over 32 quarter notes.
    let incoming = |q: f64| 120.0 + 20.0 * (q / 32.0).min(1.0);
    for (block, seed) in BLOCKS.into_iter().flat_map(|b| SEEDS.map(move |s| (b, s))) {
        let mut host = Host::new(block, 90.0);
        let mut random = Random(seed);
        let times: Vec<f64> = clock_times(10e6, 34.0, incoming)
            .into_iter()
            .map(|t| t + random.next() * 1e6)
            .collect();
        host.clocks(&times);
        // The song's quarter note at each check: the clocks that arrived.
        let mut worst: f64 = 0.0;
        for (i, &t) in times.iter().enumerate().skip(48) {
            if i % 12 != 0 {
                continue;
            }
            host.run_to(t + 50e6);
            let q = i as f64 / 24.0;
            let tempo = host.tempo().unwrap();
            worst = worst.max((tempo - incoming(q)).abs());
        }
        assert!(worst < 1.0, "{worst} BPM off at blocks of {block}");
    }
}

/// A rig with a click track and notes, for comparing renders.
fn set(rig: &mut Rig) {
    rig.editor
        .apply(Command::SetMetronomeEnabled { enabled: true })
        .unwrap();
    rig.editor
        .apply(Command::SetMetronomeCountIn {
            count_in: CountIn::None,
        })
        .unwrap();
    rig.editor
        .apply(Command::SetMetronomeSound {
            sound: MetronomeSound::Classic,
        })
        .unwrap();
    rig.editor
        .apply(Command::SetMetronomeRhythm {
            rhythm: MetronomeRhythm::Quarter,
        })
        .unwrap();
    let a = track(&mut rig.editor, TrackKind::Midi, None);
    device(&mut rig.editor, a, testing::NOTE_PULSE, &[]);
    let notes: Vec<Note> = [(60, 0, 2), (62, 4, 4), (64, 6, 1), (65, 8, 3), (67, 16, 4)]
        .iter()
        .map(|&(key, start, length)| {
            Note::new(
                key,
                Ticks::SIXTEENTH * start,
                Ticks::SIXTEENTH * length,
                100,
            )
        })
        .collect();
    let id = rig.editor.new_id();
    rig.editor
        .apply(Command::CreateClip {
            track: a,
            clip: Clip::new(
                id,
                Ticks::ZERO,
                Ticks(QUARTER * 16),
                ClipData::Midi(MidiData::new(notes)),
            ),
        })
        .unwrap();
}

#[test]
fn a_set_followed_at_140_plays_every_note_and_click_where_140_plays_it() {
    // Spec: midi-clock R17, R20, R22. Clocks run at 140 BPM without jitter,
    // and the host's Start comes between two of them: the next clock, the
    // song's first frame, falls on the first frame of a block.
    let period = 1e9 * 60.0 / (140.0 * 24.0);
    for block in BLOCKS {
        let total = 110_000usize;
        let mut reference = Host::with(block, 140.0, set);
        reference.send(TransportCommand::Play);
        let mut want = Vec::new();
        while want.len() < total {
            reference.run();
            want.extend_from_slice(reference.rig.processor.output().channel(0));
        }

        let mut host = Host::with(block, 120.0, set);
        // The block the song starts with: 150 ms in, or the first after.
        let begins = 7200usize.div_ceil(block) as u64;
        let first = host.block_ns(begins);
        host.clocks(
            &(-9..4000)
                .map(|k| first + f64::from(k) * period)
                .collect::<Vec<_>>(),
        );
        host.at(first - period / 2.0, MidiMessage::START);
        let mut got = Vec::new();
        while got.len() < total {
            host.run();
            if host.blocks > begins {
                got.extend_from_slice(host.rig.processor.output().channel(0));
            }
        }
        assert!(want.iter().any(|&s| s != 0.0), "the reference sounds");
        let wrong = (0..total).find(|&i| want[i] != got[i]);
        assert_eq!(
            wrong.map(|i| (i, got[i], want[i])),
            None,
            "block {block}: first difference"
        );
    }
}

/// Runs a host whose clocks run steadily at `bpm` from before `first`, for
/// as many clocks as a test needs.
fn steady(host: &mut Host, first: f64, bpm: f64, before: i32) -> f64 {
    let period = 1e9 * 60.0 / (bpm * 24.0);
    host.clocks(
        &(-before..4000)
            .map(|k| first + f64::from(k) * period)
            .collect::<Vec<_>>(),
    );
    period
}

#[test]
fn start_and_continue_play_from_the_frame_of_the_clock_after_them() {
    // Spec: midi-clock R20, R22. The song starts on the frame the first
    // clock after the message is heard on, and plays from where the message
    // says, ahead by what that frame is behind.
    let per_tick = 140.0 / 60.0 * 705_600.0 / 1e9;
    for block in [61, 512, 4096] {
        let mut host = Host::new(block, 120.0);
        let begins = 14400usize.div_ceil(block) as u64;
        let period = 1e9 * 60.0 / (140.0 * 24.0);
        // Clocks run on, a third of a period past the start of a block.
        let lattice = host.block_ns(begins) + 0.3 * period;
        steady(&mut host, lattice, 140.0, 20);
        host.at(host.block_ns(begins) - 1000.0, MidiMessage::START);
        host.run_to(host.block_ns(begins - 1));
        assert!(!host.playing(), "block {block}: not before the message");
        while !host.playing() {
            host.run();
        }
        let ticks = host.ticks();
        let want = (host.block_ns(host.blocks) - lattice) * per_tick;
        assert!(
            (ticks - want).abs() < 80.0,
            "block {block}: {ticks} ticks after Start, {want} wanted"
        );

        // Stop stops at the block it is taken in, keeping the position.
        let stop = host.block_ns(host.blocks) - 1.0;
        host.at(stop, MidiMessage::STOP);
        host.run();
        assert!(!host.playing());
        let kept = host.ticks();
        host.run();
        assert_eq!(host.ticks(), kept);

        // A pointer then Continue play on from the pointer.
        let now = host.block_ns(host.blocks);
        host.at(now - 2000.0, MidiMessage::song_position(16));
        host.run();
        assert!(!host.playing());
        assert_eq!(host.rig.monitor.state().insert_marker, Ticks(QUARTER * 4));
        let from = host.block_ns(host.blocks);
        host.at(from - 1000.0, MidiMessage::CONTINUE);
        let next = lattice + ((from - lattice) / period).ceil() * period;
        while !host.playing() {
            host.run();
        }
        let ticks = host.ticks();
        let want = (QUARTER * 4) as f64 + (host.block_ns(host.blocks) - next) * per_tick;
        assert!(
            (ticks - want).abs() < 80.0,
            "block {block}: {ticks} ticks after Continue, {want} wanted"
        );
    }
}

#[test]
fn a_start_with_no_clock_before_it_waits_for_the_first_and_plays_along() {
    // Spec: midi-clock R19, R20. The tempo is learned from the clocks that
    // follow Start; the song is within a sixteenth of the clock throughout.
    for block in BLOCKS {
        let mut host = Host::new(block, 120.0);
        let first = host.block_ns(3.max(2000 / block as u64));
        host.at(first - 1e6, MidiMessage::START);
        let period = steady(&mut host, first, 140.0, 0);
        for seconds in 1..6 {
            let at = first + f64::from(seconds) * 1e9;
            host.run_to(at);
            // The clock's position at the block's start, a quarter note per
            // 24 clocks.
            let now = host.block_ns(host.blocks - 1);
            let want = (now - first) / period * CLOCK;
            let got = host.ticks();
            assert!(
                (got - want).abs() < SIXTEENTH,
                "block {block}: {got} ticks, the clock is at {want}, {seconds} s in"
            );
        }
        let tempo = host.tempo().unwrap();
        assert!((tempo - 140.0).abs() < 0.1);
    }
}

#[test]
fn a_doubled_clock_and_a_missing_clock_are_set_aside_and_the_song_stays_in_step() {
    // Spec: midi-clock R18, R19.
    for block in BLOCKS {
        let mut host = Host::new(block, 120.0);
        let first = host.block_ns(3.max(2000 / block as u64));
        host.at(first - 1e6, MidiMessage::START);
        let period = 1e9 * 60.0 / (120.0 * 24.0);
        let mut times = Vec::new();
        for k in 0..200 {
            let t = first + f64::from(k) * period;
            match k {
                // Missing: the clock never comes.
                80 | 140 => {}
                // Doubled: it comes twice, a third of a period apart.
                110 => times.extend([t, t + period / 3.0]),
                _ => times.push(t),
            }
        }
        host.clocks(&times);
        host.run_to(first + 150.0 * period);
        let state = host.rig.monitor.state().sync_in;
        assert_eq!(state.glitches, 3, "block {block}");
        let now = host.block_ns(host.blocks - 1);
        // The position is the clock's as the host counts it: the missing
        // clocks still count.
        let want = (now - first) / period * CLOCK;
        let got = host.ticks();
        assert!(
            (got - want).abs() < SIXTEENTH,
            "block {block}: {got} ticks, the clock is at {want}"
        );
        assert!((host.tempo().unwrap() - 120.0).abs() < 0.3, "block {block}");
    }
}

#[test]
fn after_half_a_second_of_silence_the_song_plays_on_and_follows_again_from_where_it_is() {
    // Spec: midi-clock R21.
    for block in [61, 4096] {
        let mut host = Host::new(block, 90.0);
        let first = host.block_ns(3.max(2000 / block as u64));
        host.at(first - 1e6, MidiMessage::START);
        let period = 1e9 * 60.0 / (140.0 * 24.0);
        let times: Vec<f64> = (0..96).map(|k| first + f64::from(k) * period).collect();
        host.clocks(&times);
        let last = *times.last().unwrap();
        host.run_to(last + 100e6);
        assert!(host.receiving());
        assert!(host.playing());
        // 500 ms later the clock is silent, and the song plays on at 140.
        host.run_to(last + 700e6);
        assert!(!host.receiving(), "block {block}");
        assert!(host.playing());
        let before = host.ticks();
        host.run_to(last + 1700e6);
        let ticks = host.ticks() - before;
        // A second of 140 BPM is 140/60 quarter notes, within a block.
        let second = 140.0 / 60.0 * QUARTER as f64;
        assert!(
            (ticks - second * (host.block_ns(host.blocks - 1) - (last + 700e6)) / 1e9).abs()
                < 0.05 * second,
            "block {block}: {ticks} ticks"
        );
        let tempo = host.following().unwrap();
        assert!((tempo - 140.0).abs() < 0.2, "{tempo}");
        // The clock comes back, a little off the song's own position.
        let later = last + 2000e6;
        let times: Vec<f64> = (0..96).map(|k| later + f64::from(k) * period).collect();
        host.clocks(&times);
        host.run_to(later + 300e6);
        assert!(host.receiving() && host.playing());
        assert!((host.tempo().unwrap() - 140.0).abs() < 0.5);
    }
}

#[test]
fn a_pointer_at_or_past_the_loops_end_wraps_into_it() {
    // Spec: midi-clock R22. A loop of the second bar: 1.2.1 sixteenths 16
    // to 32.
    let bar = Ticks(QUARTER * 4);
    for (sixteenths, want) in [
        // Before the loop's start: as it is.
        (4, Ticks::SIXTEENTH * 4),
        // Inside it: as it is.
        (20, Ticks::SIXTEENTH * 20),
        // At the end: the start.
        (32, bar),
        // Past the end: the start plus the distance past it, modulo the
        // loop's length.
        (40, bar + Ticks::SIXTEENTH * 8),
        (16 + 16 * 3 + 5, bar + Ticks::SIXTEENTH * 5),
    ] {
        let mut host = Host::new(512, 120.0);
        host.send(TransportCommand::SetLoop(Some(LoopRegion {
            start: bar,
            end: bar * 2,
        })));
        host.run();
        host.at(
            host.block_ns(host.blocks + 1) - 1.0,
            MidiMessage::song_position(sixteenths),
        );
        host.run();
        host.run();
        assert_eq!(
            host.rig.monitor.state().insert_marker,
            want,
            "pointer {sixteenths}"
        );
    }
}

#[test]
fn a_pointer_while_playing_jumps_when_it_is_over_a_sixteenth_away() {
    // Spec: midi-clock R19, R22.
    for block in [61, 4096] {
        let mut host = Host::new(block, 120.0);
        let begins = 14400usize.div_ceil(block) as u64;
        let first = host.block_ns(begins);
        let period = steady(&mut host, first, 120.0, 40);
        host.at(first - 1000.0, MidiMessage::START);
        host.run_to(first + 1e9);
        assert!(host.playing());
        // The host jumps to bar 3: a pointer, then Continue.
        let at = host.block_ns(host.blocks) + 3.0 * period;
        let next = first + ((at - first) / period).ceil() * period;
        host.at(at - period, MidiMessage::song_position(32));
        host.run_to(next + 100e6);
        let want = (QUARTER * 8) as f64 + (host.block_ns(host.blocks) - next) / period * CLOCK;
        let got = host.ticks();
        assert!(
            (got - want).abs() < SIXTEENTH / 2.0,
            "block {block}: {got} ticks, the host is at {want}"
        );
    }
}

#[test]
fn a_sync_delay_plays_later_against_the_clock_and_a_negative_one_earlier() {
    // Spec: midi-clock R14, R20. 100 ms of 120 BPM is 0.2 of a quarter
    // note.
    let at = |delay_ms: i16| {
        let mut host = Host::new(512, 120.0);
        host.rig.sender.sync_inputs(&[SyncInput {
            port: port(),
            delay_ms,
        }]);
        assert!(host.rig.sender.flush());
        let first = host.block_ns(40);
        let period = steady(&mut host, first, 120.0, 40);
        host.at(first - 1000.0, MidiMessage::START);
        host.run_to(first + 4e9);
        let now = host.block_ns(host.blocks - 1);
        let clock = (now - first) / period * CLOCK;
        host.ticks() - clock
    };
    let late = at(100);
    let early = at(-100);
    let plain = at(0);
    let tenth = 0.2 * QUARTER as f64;
    assert!((plain - late - tenth).abs() < 0.1 * tenth, "{plain} {late}");
    assert!(
        (early - plain - tenth).abs() < 0.1 * tenth,
        "{early} {plain}"
    );
}

#[test]
fn the_first_input_with_clocks_leads_and_the_others_are_ignored() {
    // Spec: midi-clock R17.
    let mut host = Host::new(512, 120.0);
    let other = PortId::from_name("Other");
    host.rig.sender.sync_inputs(&[
        SyncInput {
            port: port(),
            delay_ms: 0,
        },
        SyncInput {
            port: other,
            delay_ms: 0,
        },
    ]);
    assert!(host.rig.sender.flush());
    let period = 1e9 * 60.0 / (100.0 * 24.0);
    // The other input sends 150 BPM clocks, later than the first's 100.
    for k in 0..96 {
        host.at(1e6 + f64::from(k) * period, MidiMessage::CLOCK);
    }
    for k in 0..96 {
        let t = 400e6 + f64::from(k) * 1e9 * 60.0 / (150.0 * 24.0);
        host.at_port(other, t, MidiMessage::CLOCK);
    }
    host.run_to(1.8e9);
    let state = host.rig.monitor.state().sync_in;
    assert_eq!(state.lead, Some(port()));
    assert!((state.tempo.unwrap() - 100.0).abs() < 0.2);
}

#[test]
fn with_ext_off_messages_only_light_the_lamp_and_ext_off_plays_the_map_again() {
    // Spec: midi-clock R15, R16, R22, R24.
    let mut host = Host::new(512, 90.0);
    host.rig.sender.external_sync(false);
    assert!(host.rig.sender.flush());
    let first = host.block_ns(10);
    let period = steady(&mut host, first, 140.0, 0);
    host.at(first - 1000.0, MidiMessage::START);
    host.run_to(first + 3e9);
    // The lamp: a quarter note of clock a time, and the tempo is read, but
    // nothing plays or follows.
    let state = host.rig.monitor.state();
    assert!(!state.playing);
    assert_eq!(state.following, None);
    assert!(state.sync_in.receiving && !state.sync_in.following);
    assert!(state.sync_in.quarters >= 5, "{}", state.sync_in.quarters);
    assert!((state.sync_in.tempo.unwrap() - 140.0).abs() < 0.1);

    // EXT on, a play of Beatlayer's own follows the clock's tempo.
    host.rig.sender.external_sync(true);
    host.send(TransportCommand::Play);
    host.run_to(first + 3.2e9);
    assert!(host.playing());
    assert!((host.following().unwrap() - 140.0).abs() < 0.2);
    let _ = period;

    // EXT off again: the map's tempo plays, from the same beat.
    let before = host.ticks();
    host.rig.sender.external_sync(false);
    assert!(host.rig.sender.flush());
    host.run();
    assert_eq!(host.following(), None);
    assert!(host.playing());
    let after = host.ticks();
    assert!(after > before && after - before < QUARTER as f64 / 8.0);
}

#[test]
fn clock_out_follows_the_incoming_tempo() {
    // Spec: midi-clock R9. A song at 120 BPM following a 140 BPM clock sends
    // its own clock to an output at 140.
    for block in [61, 512, 4096] {
        let mut host = Host::new(block, 120.0);
        let (producer, mut out) = RingBuffer::<MidiOutputEvent>::new(8192);
        assert!(host.rig.processor.set_midi_output(producer).is_none());
        host.rig.sender.sync_outputs(&[SyncOutput {
            port: PortId::from_name("Drum machine"),
            delay_ms: 0,
            clock_type: ClockType::Song,
            resync: HardwareResync::StopAndStart,
        }]);
        assert!(host.rig.sender.flush());
        let begins = 14400usize.div_ceil(block) as u64;
        let first = host.block_ns(begins);
        let period = steady(&mut host, first, 140.0, 20);
        host.at(first - 1000.0, MidiMessage::START);
        host.run_to(first + 3e9);
        let mut times = Vec::new();
        while let Ok(event) = out.pop() {
            if event.message == MidiMessage::CLOCK {
                times.push(event.time);
            }
        }
        assert!(times.len() > 100, "block {block}: {} clocks", times.len());
        let spread = times.last().unwrap().duration_since(times[0]).as_nanos() as f64
            / (times.len() - 1) as f64;
        assert!(
            (spread - period).abs() < 1000.0,
            "block {block}: {spread} ns apart"
        );
        for pair in times.windows(2) {
            let gap = pair[1].duration_since(pair[0]).as_nanos() as f64;
            assert!((gap - period).abs() < 25_000.0, "block {block}: {gap} ns");
        }
    }
}

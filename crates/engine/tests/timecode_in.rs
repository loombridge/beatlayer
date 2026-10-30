//! MIDI Timecode in (WP-7.13, `docs/specs/midi-timecode.md` R4 to R10, R12):
//! quarter frames and full frames sent from a test port at times of the
//! test clock, followed with EXT on. Every block runs as a real-time section
//! under the allocation trap.

use crate::clock_in::Host;

use bl_core::midi::{MidiMessage, PortId, SyncInput, TimecodeSettings};
use bl_core::time::{TICKS_PER_QUARTER, Ticks};
use bl_core::timecode::{FrameRate, Timecode, TimecodeRate};
use bl_engine::{LoopRegion, TransportCommand};

/// The block sizes every test runs at.
const BLOCKS: [usize; 4] = [1, 61, 512, 4096];

/// Ticks in a second at 120 BPM, the tempo every test's set has.
const PER_SECOND: f64 = 2.0 * TICKS_PER_QUARTER as f64;

fn port() -> PortId {
    PortId::from_name("Timecode")
}

fn host(block: usize, rate: TimecodeRate, offset: Timecode) -> Host {
    let input = SyncInput {
        port: port(),
        delay_ms: 0,
        timecode: Some(TimecodeSettings { rate, offset }),
    };
    Host::with_input(block, 120.0, input, |_| {})
}

/// The time of frame `n` at `fps` whole frames a second.
fn time_at(n: u32, fps: u32) -> Timecode {
    Timecode {
        hours: (n / (fps * 3600)) as u8,
        minutes: (n / (fps * 60) % 60) as u8,
        seconds: (n / fps % 60) as u8,
        frames: (n % fps) as u8,
    }
}

/// The eight quarter-frame data bytes of `time` at the rate code `code`.
fn pieces(time: Timecode, code: u8) -> [u8; 8] {
    let nibbles = [
        time.frames & 15,
        time.frames >> 4,
        time.seconds & 15,
        time.seconds >> 4,
        time.minutes & 15,
        time.minutes >> 4,
        time.hours & 15,
        (time.hours >> 4) | code << 1,
    ];
    core::array::from_fn(|i| (i as u8) << 4 | nibbles[i])
}

fn quarter(data: u8) -> MidiMessage {
    MidiMessage::from_bytes(&[0xF1, data]).unwrap()
}

impl Host {
    /// Has a host send quarter frames from `first` ns on, 25 fps from frame
    /// `from`, for `frames` frames, running `speed` times as fast as the
    /// clock: eight pieces every two frames, each describing the time of its
    /// first.
    fn timecode(&mut self, first: f64, from: u32, frames: u32, speed: f64) {
        let frame = 1e9 / 25.0 / speed;
        for pair in 0..frames / 2 {
            let at = first + f64::from(pair * 2) * frame;
            let data = pieces(time_at(from + pair * 2, 25), 1);
            for (k, &byte) in data.iter().enumerate() {
                self.at_port(port(), at + k as f64 * frame / 4.0, quarter(byte));
            }
        }
    }
}

/// Where a block boundary near the start of the run is: a block of the
/// run's length from the start.
fn begin(host: &Host) -> f64 {
    host.block_ns(2400u64.div_ceil(host.block_size()) + 2) + 370_000.0
}

#[test]
fn quarter_frames_from_ten_seconds_at_25_fps_start_the_song_on_the_eighth() {
    // Spec: midi-timecode R6. The eighth piece of 00:00:10:00 arrives 70 ms
    // after the first; the song plays from 10.08 s on the frame it is heard
    // on.
    for block in BLOCKS {
        let mut host = host(block, TimecodeRate::All, Timecode::ZERO);
        let first = begin(&host);
        host.timecode(first, 250, 100, 1.0);
        let eighth = first + 70e6;
        host.run_to(eighth - 1e6);
        assert!(
            !host.playing(),
            "block {block}: not before the eighth piece"
        );
        host.run_to(eighth + 100e6);
        assert!(host.playing(), "block {block}: started");
        let end = host.block_ns(host.blocks);
        let want = (10.08 + (end - eighth) / 1e9) * PER_SECOND;
        let got = host.ticks();
        assert!(
            (got - want).abs() < 80.0,
            "block {block}: {got} ticks after the eighth piece, {want} wanted"
        );
    }
}

#[test]
fn the_offset_is_where_the_arrangement_starts_and_a_time_before_it_is_1_1_1() {
    // Spec: midi-timecode R4. The Arrangement starts at 00:00:10:00.
    let offset = time_at(250, 25);
    for block in [61, 4096] {
        let mut host = host(block, TimecodeRate::All, offset);
        let first = begin(&host);
        // 00:00:09:00 to 00:00:11:00: the song waits at 1.1.1 while the time
        // is before the offset, then follows it.
        host.timecode(first, 225, 100, 1.0);
        host.run_to(first + 500e6);
        assert_eq!(host.ticks(), 0.0, "block {block}: before the offset");
        host.run_to(first + 1.5e9);
        let now = host.block_ns(host.blocks);
        // 1 s past the offset at the time the first piece was sent.
        let want = (now - first) / 1e9 - 1.0;
        let got = host.ticks() / PER_SECOND;
        assert!(
            (got - want).abs() < 0.05,
            "block {block}: {got} s, {want} wanted"
        );
    }
}

#[test]
fn timecode_one_percent_fast_relocates_when_the_difference_passes_a_frame() {
    // Spec: midi-timecode R7. The song keeps its own tempo between times; a
    // host 1 % fast is a frame (40 ms) ahead after 4 s.
    for block in BLOCKS {
        let mut host = host(block, TimecodeRate::All, Timecode::ZERO);
        let first = begin(&host);
        host.timecode(first, 0, 1200, 1.01);
        host.run_to(first + 100e6);
        assert!(host.playing(), "block {block}");
        let started = host.block_ns(host.blocks);
        let mut jumps = Vec::new();
        let mut before = (host.ticks(), host.block_ns(host.blocks));
        // The block's frames: the song moves on that far unless it jumps.
        while host.block_ns(host.blocks) < first + 12e9 {
            host.run();
            let now = (host.ticks(), host.block_ns(host.blocks));
            let moved = (now.1 - before.1) / 1e9 * PER_SECOND;
            if (now.0 - before.0 - moved).abs() > 20_000.0 {
                jumps.push((now.1 - started) / 1e9);
            }
            before = now;
        }
        assert_eq!(jumps.len(), 2, "block {block}: jumps at {jumps:?}");
        assert!(
            jumps[0] > 3.8 && jumps[0] < 4.3,
            "block {block}: the first jump at {} s",
            jumps[0]
        );
        assert!(jumps[1] > 7.9 && jumps[1] < 8.5, "block {block}: {jumps:?}");
    }
}

#[test]
fn a_full_frame_locates_while_stopped_and_jumps_while_playing() {
    // Spec: midi-timecode R5, R8. F0 7F id 01 01 hh mm ss ff F7: the full
    // frame names its rate in the hours byte.
    for block in [61, 4096] {
        let mut host = host(block, TimecodeRate::All, Timecode::ZERO);
        let at = host.block_ns(4) - 1000.0;
        host.at_port(
            port(),
            at,
            MidiMessage::full_frame(1 << 5, 0, 2, 0).unwrap(),
        );
        host.run_to(host.block_ns(6));
        assert!(!host.playing());
        assert_eq!(
            host.rig.monitor.state().insert_marker,
            Ticks(2 * 2 * TICKS_PER_QUARTER),
            "block {block}: located at 2 s"
        );
        // Running, then the host jumps to 00:00:30:00 with a full frame and
        // runs on from there.
        let first = host.block_ns(host.blocks + 4) + 370_000.0;
        host.timecode(first, 0, 100, 1.0);
        host.run_to(first + 300e6);
        assert!(host.playing(), "block {block}");
        let jump = first + 600e6;
        host.at_port(
            port(),
            jump,
            MidiMessage::full_frame(1 << 5, 0, 30, 0).unwrap(),
        );
        host.timecode(jump + 1e6, 750, 100, 1.0);
        host.run_to(jump + 500e6);
        let now = host.block_ns(host.blocks);
        let want = 30.0 + (now - jump) / 1e9;
        let got = host.ticks() / PER_SECOND;
        assert!(
            (got - want).abs() < 0.1,
            "block {block}: {got} s, {want} wanted"
        );
    }
}

#[test]
fn silence_for_100_ms_or_pieces_in_reverse_order_stop_the_song() {
    // Spec: midi-timecode R8.
    for block in BLOCKS {
        let mut host = host(block, TimecodeRate::All, Timecode::ZERO);
        let first = begin(&host);
        // Two sequences, 160 ms, then nothing.
        host.timecode(first, 0, 4, 1.0);
        host.run_to(first + 200e6);
        assert!(host.playing(), "block {block}: running");
        // The last piece came at 150 ms: 100 ms on is 250 ms.
        host.run_to(first + 230e6);
        assert!(
            host.playing(),
            "block {block}: not before 100 ms of silence"
        );
        host.run_to(first + 250e6 + host.block_ns(2));
        assert!(!host.playing(), "block {block}: 100 ms of silence");

        // Reverse order: pieces 7, 6, 5 after a running forward stream.
        let mut host = self::host(block, TimecodeRate::All, Timecode::ZERO);
        let first = begin(&host);
        host.timecode(first, 0, 40, 1.0);
        host.run_to(first + 400e6);
        assert!(host.playing(), "block {block}: running again");
        let at = first + 480e6;
        let data = pieces(time_at(40, 25), 1);
        for (k, piece) in [7usize, 6, 5].into_iter().enumerate() {
            host.at_port(port(), at + k as f64 * 10e6, quarter(data[piece]));
        }
        host.run_to(at + 60e6);
        assert!(!host.playing(), "block {block}: reverse order");
    }
}

#[test]
fn the_arrangement_loop_does_not_wrap_timecode_that_leads() {
    // Spec: midi-timecode R9. A loop of the first bar (2 s): the timecode's
    // position is played through its end.
    let bar = Ticks(4 * TICKS_PER_QUARTER);
    for block in [61, 4096] {
        let mut host = host(block, TimecodeRate::All, Timecode::ZERO);
        host.send(TransportCommand::SetLoop(Some(LoopRegion {
            start: Ticks::ZERO,
            end: bar,
        })));
        let first = begin(&host);
        host.timecode(first, 0, 200, 1.0);
        let mut before = host.ticks();
        while host.block_ns(host.blocks) < first + 3.5e9 {
            host.run();
            let now = host.ticks();
            assert!(
                now >= before,
                "block {block}: wrapped from {before} to {now}"
            );
            before = now;
        }
        assert!(before > 1.5 * bar.get() as f64, "block {block}: {before}");

        // Without timecode, the same song wraps.
        let mut host = self::host(block, TimecodeRate::All, Timecode::ZERO);
        host.send(TransportCommand::SetLoop(Some(LoopRegion {
            start: Ticks::ZERO,
            end: bar,
        })));
        host.send(TransportCommand::Play);
        let mut before = host.ticks();
        let mut wrapped = false;
        while host.block_ns(host.blocks) < 3.5e9 {
            host.run();
            wrapped |= host.ticks() < before;
            before = host.ticks();
        }
        assert!(wrapped, "block {block}: the loop wraps without timecode");
    }
}

#[test]
fn with_ext_off_whole_times_only_light_the_lamp_and_are_reported() {
    // Spec: midi-timecode R10, R12. A fixed rate counts the time at itself,
    // whatever the message says.
    for block in [61, 4096] {
        let mut host = host(block, TimecodeRate::Fixed(FrameRate::Fps24), Timecode::ZERO);
        host.rig.sender.external_sync(false);
        assert!(host.rig.sender.flush());
        let first = begin(&host);
        host.timecode(first, 262, 100, 1.0);
        host.run_to(first + 300e6);
        assert!(!host.playing(), "block {block}: EXT is off");
        let state = host.rig.monitor.state().sync_in;
        // Sent from 00:00:10:12 at 25 fps, the pieces name frames the 24 fps
        // count reads as its own, and the eighth piece adds two 24 fps
        // frames.
        assert!(state.quarters >= 3, "block {block}: {}", state.quarters);
        assert_eq!(state.lead, Some(port()));
        let reported = state.timecode.unwrap();
        assert_eq!(reported.rate, FrameRate::Fps24);
        assert!(reported.time.seconds >= 10);
        assert!(!reported.running);
        let seconds = reported.time.seconds(FrameRate::Fps24) + 2.0 / 24.0;
        assert!((reported.position.get() as f64 / PER_SECOND - seconds).abs() < 1e-3);
    }
}

#[test]
fn the_reported_time_runs_while_the_song_follows() {
    // Spec: midi-timecode R12.
    let mut host = host(512, TimecodeRate::All, Timecode::ZERO);
    let first = begin(&host);
    host.timecode(first, 250, 100, 1.0);
    host.run_to(first + 300e6);
    let state = host.rig.monitor.state().sync_in;
    let reported = state.timecode.unwrap();
    assert_eq!(reported.rate, FrameRate::Fps25);
    assert!(reported.running && state.receiving && state.timecode_leads);
    assert_eq!(state.tempo, None, "timecode brings no tempo");
    assert_eq!(host.rig.monitor.state().following, None);
}

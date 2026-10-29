//! MIDI input in the engine (WP-2.2, `docs/specs/midi-io.md`): messages on
//! the lock-free queue reach each MIDI track's input in the next block, on
//! the sample their timestamps give, matched by source and channel, with
//! every block a real-time section under the allocation trap.
//!
//! `cargo test -p bl-engine --test engine_tests -- midi_input:: --nocapture` prints the
//! timing measurements.

use crate::common;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering::Relaxed};
use std::thread;
use std::time::{Duration, Instant};

use bl_audio_io::{
    AudioBuffer, AudioBufferMut, AudioCallback, Backend, NullBackend, NullOptions, Pacing,
    PortRequest, ProcessContext, StreamRequest,
};
use bl_core::midi::{MidiInputEvent, MidiMessage, MidiSource, PortId};
use bl_engine::{MIDI_EVENTS_PER_TRACK, MidiEvent, NoInput, Processor};
use bl_model::{Command, Input, Monitor, TrackId, TrackKind};
use common::{RATE, Rig, track};
use rtrb::{Producer, RingBuffer};

const BLOCK: usize = 512;

/// The length of `frames` frames at 48 kHz.
fn frames(frames: u64) -> Duration {
    Duration::from_nanos(frames * 1_000_000_000 / 48_000)
}

/// A rig with a MIDI queue, the time its first block starts, and the
/// tracks' inputs.
struct Midi {
    rig: Rig,
    queue: Producer<MidiInputEvent>,
    start: Instant,
    blocks: u64,
}

impl Midi {
    fn new() -> Self {
        let mut rig = Rig::new(BLOCK);
        let (queue, consumer) = RingBuffer::new(4096);
        assert!(rig.processor.set_midi_input(consumer).is_none());
        Self {
            rig,
            queue,
            start: Instant::now(),
            blocks: 0,
        }
    }

    fn set_input(&mut self, track: TrackId, input: Input, channel: Option<u8>) {
        self.rig.apply(Command::SetInput {
            track,
            input,
            channel,
        });
    }

    /// The time the block `n` blocks after the first starts, on an exact
    /// clock.
    fn block_time(&self, n: u64) -> Instant {
        self.start + frames(n * BLOCK as u64)
    }

    /// Processes the next block, called exactly on time, as a real-time
    /// section.
    fn block(&mut self) {
        let now = self.block_time(self.blocks);
        self.block_at(now);
    }

    fn block_at(&mut self, now: Instant) {
        let processor = &mut self.rig.processor;
        bl_rt::rt_test!({
            processor.process_at(BLOCK, &NoInput, now);
        });
        self.blocks += 1;
    }

    /// Sends a message that arrived `after` the start of block `block`.
    fn send(&mut self, block: u64, after: Duration, source: MidiSource, message: MidiMessage) {
        let time = self.block_time(block) + after;
        self.queue
            .push(MidiInputEvent {
                source,
                time,
                message,
            })
            .unwrap();
    }

    fn input(&self, track: TrackId) -> (Vec<(u32, MidiMessage)>, bool) {
        let input = self.rig.processor.midi_input(track).unwrap();
        (
            input.events.iter().map(|e| (e.offset, e.message)).collect(),
            input.monitored,
        )
    }
}

fn port(name: &str) -> MidiSource {
    MidiSource::Port(PortId::from_name(name))
}

fn ms(ms: u64) -> Duration {
    Duration::from_millis(ms)
}

// Spec: midi-io R7, R9, R10, R12, R15, R23
#[test]
fn messages_reach_each_midi_tracks_input_by_source_and_channel_in_the_next_block() {
    let mut midi = Midi::new();
    let editor = &mut midi.rig.editor;
    let all = track(editor, TrackKind::Midi, None);
    let keyboard = track(editor, TrackKind::Midi, None);
    let keys_a = track(editor, TrackKind::Midi, None);
    let silent = track(editor, TrackKind::Midi, None);
    let audio = track(editor, TrackKind::Audio, None);
    midi.rig.sync();
    midi.rig.apply(Command::SetArm {
        track: all,
        arm: true,
    });
    midi.set_input(keyboard, Input::ComputerKeyboard, None);
    midi.rig.apply(Command::SetMonitor {
        track: keyboard,
        monitor: Monitor::In,
    });
    midi.set_input(
        keys_a,
        Input::Port {
            name: "Keys A".to_owned(),
        },
        Some(2),
    );
    midi.rig.apply(Command::SetMonitor {
        track: keys_a,
        monitor: Monitor::Off,
    });
    midi.set_input(silent, Input::None, None);

    // Block 0 locks the clock; what arrives during it lands in block 1.
    midi.block();
    let note = MidiMessage::note_on(1, 60, 100);
    let keyboard_note = MidiMessage::note_on(0, 72, 100);
    let other_port = MidiMessage::note_on(0, 48, 90);
    let clock = MidiMessage::from_bytes(&[0xF8]).unwrap();
    let wrong_channel = MidiMessage::note_on(5, 61, 80);
    midi.send(0, ms(1), port("Keys A"), note);
    midi.send(0, ms(2), MidiSource::ComputerKeyboard, keyboard_note);
    midi.send(0, ms(3), port("Keys B"), other_port);
    midi.send(0, ms(4), port("Keys A"), clock);
    midi.send(0, ms(5), port("Keys A"), wrong_channel);
    midi.block();

    // 1 ms is 48 frames at 48 kHz.
    let (events, monitored) = midi.input(all);
    assert_eq!(
        events,
        vec![
            (48, note),
            (96, keyboard_note),
            (144, other_port),
            (240, wrong_channel)
        ]
    );
    assert!(monitored, "armed, on Auto");
    assert_eq!(midi.input(keyboard), (vec![(96, keyboard_note)], true));
    assert_eq!(midi.input(keys_a), (vec![(48, note)], false));
    assert_eq!(midi.input(silent), (vec![], false));
    assert!(midi.rig.processor.midi_input(audio).is_none());

    let counters = midi.rig.processor.midi_counters();
    assert_eq!(counters.received, 5);
    // The Keys A note and the keyboard's reach two tracks each.
    assert_eq!(counters.delivered, 6);
    assert_eq!(counters.unrouted, 1, "the clock reaches no track");
    assert_eq!(counters.late, 0);

    // The next block is empty again.
    midi.block();
    assert_eq!(midi.input(all).0, vec![]);
}

#[test]
fn the_time_base_puts_a_moment_on_the_engine_clock_with_or_without_a_midi_queue() {
    // Spec: midi-io R16, control-bar-transport R19. A moment on the main
    // thread, such as a click, falls on the frame the block clock gives it.
    let mut midi = Midi::new();
    let mut bare = Rig::new(BLOCK);
    let start = Instant::now();
    for n in 0..4 {
        midi.block();
        bl_rt::rt_test!({
            bare.processor
                .process_at(BLOCK, &NoInput, start + frames(n * BLOCK as u64));
        });
    }
    let after = frames(3 * BLOCK as u64 + 100);
    let base = midi.rig.monitor.state().time_base.unwrap();
    assert_eq!(base.frame_at(midi.start + after), 3 * BLOCK as u64 + 100);
    let base = bare.monitor.state().time_base.unwrap();
    assert_eq!(base.frame_at(start + after), 3 * BLOCK as u64 + 100);
}

// Spec: midi-io R15
#[test]
fn a_message_stamped_after_the_block_starts_waits_and_a_late_one_lands_first() {
    let mut midi = Midi::new();
    let keys = track(&mut midi.rig.editor, TrackKind::Midi, None);
    midi.rig.sync();
    midi.block();
    midi.block();
    let on = MidiMessage::note_on(0, 60, 100);
    // Stamped 0.5 ms into block 2, but already queued when block 2 starts:
    // it waits for block 3.
    midi.send(
        2,
        Duration::from_micros(500),
        MidiSource::ComputerKeyboard,
        on,
    );
    midi.block();
    assert_eq!(midi.input(keys).0, vec![]);
    midi.block();
    assert_eq!(midi.input(keys).0, vec![(24, on)]);

    // Stamped during block 3, but only seen at block 5, after block 4 took
    // what arrived during block 3: it's late, so it lands on block 5's first
    // frame.
    midi.block();
    let off = MidiMessage::note_off(0, 60);
    midi.send(3, ms(1), MidiSource::ComputerKeyboard, off);
    midi.block();
    assert_eq!(midi.input(keys).0, vec![(0, off)]);
    assert_eq!(midi.rig.processor.midi_counters().late, 1);
}

// Spec: midi-io R14
#[test]
fn a_full_track_input_drops_what_doesnt_fit_and_counts_it() {
    let mut midi = Midi::new();
    let keys = track(&mut midi.rig.editor, TrackKind::Midi, None);
    midi.rig.sync();
    midi.block();
    for i in 0..MIDI_EVENTS_PER_TRACK + 10 {
        let key = (i % 128) as u8;
        midi.send(
            0,
            Duration::from_micros(i as u64 * 20),
            MidiSource::ComputerKeyboard,
            MidiMessage::note_on(0, key, 1),
        );
    }
    midi.block();
    let (events, _) = midi.input(keys);
    assert_eq!(events.len(), MIDI_EVENTS_PER_TRACK);
    assert!(events.windows(2).all(|w| w[0].0 <= w[1].0), "in order");
    let counters = midi.rig.processor.midi_counters();
    assert_eq!(counters.dropped, 10);
    assert_eq!(counters.delivered, MIDI_EVENTS_PER_TRACK as u64);
}

// Spec: midi-io R11, R12
#[test]
fn input_and_monitoring_edits_reach_the_engine_and_undo() {
    let mut midi = Midi::new();
    let keys = track(&mut midi.rig.editor, TrackKind::Midi, None);
    midi.rig.sync();
    midi.block();
    assert!(!midi.input(keys).1, "a new track monitors on Auto, unarmed");

    let synced = midi.rig.apply(Command::SetMonitor {
        track: keys,
        monitor: Monitor::In,
    });
    assert!(
        synced.program,
        "a MIDI track's routes travel in the program"
    );
    midi.block();
    assert!(midi.input(keys).1);

    // A port on one channel: only that channel's messages arrive, and the
    // edit undoes as one step.
    midi.set_input(
        keys,
        Input::Port {
            name: "Keys".to_owned(),
        },
        Some(3),
    );
    let keys_port = port("Keys");
    midi.send(1, ms(1), keys_port, MidiMessage::note_on(2, 64, 99));
    midi.send(1, ms(2), keys_port, MidiMessage::note_on(0, 65, 99));
    midi.block();
    assert_eq!(midi.input(keys).0.len(), 1, "only channel 3");

    // Undo puts the input back to All Ins, and the engine follows.
    midi.rig.editor.undo().unwrap();
    midi.rig.sync();
    midi.send(
        2,
        ms(1),
        MidiSource::ComputerKeyboard,
        MidiMessage::note_on(2, 64, 99),
    );
    midi.send(
        2,
        ms(2),
        MidiSource::ComputerKeyboard,
        MidiMessage::note_on(0, 65, 99),
    );
    midi.block();
    assert_eq!(midi.input(keys).0.len(), 2);
}

/// A deterministic random sequence.
struct Lcg(u64);

impl Lcg {
    /// A number from -1 to 1.
    fn next(&mut self) -> f64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 11) as f64 / (1u64 << 53) as f64) * 2.0 - 1.0
    }
}

// Spec: midi-io R15, R16
#[test]
fn messages_keep_their_spacing_to_the_sample_through_callback_jitter() {
    // The callback runs up to 0.5 ms early or late; messages arrive every
    // 7.3 ms, out of step with the 512-frame blocks. After the block clock
    // locks, each message lands within 12 frames (0.25 ms) of where its
    // timestamp puts it, where the raw callback times would put it up to 24
    // away.
    let mut midi = Midi::new();
    let keys = track(&mut midi.rig.editor, TrackKind::Midi, None);
    midi.rig.sync();
    let mut jitter = Lcg(11);
    let spacing = Duration::from_micros(7_300);
    let blocks = 2_000u64;
    let mut next = 0u64;
    let mut worst: f64 = 0.0;
    let mut placed = 0;
    for n in 0..blocks {
        // Queue the messages that arrive during block n.
        let end = midi.block_time(n + 1);
        loop {
            let time = midi.start + spacing * next as u32;
            if time >= end {
                break;
            }
            let message = MidiMessage::note_on(0, (next % 128) as u8, 100);
            midi.queue
                .push(MidiInputEvent {
                    source: MidiSource::ComputerKeyboard,
                    time,
                    message,
                })
                .unwrap();
            next += 1;
        }
        let late = jitter.next() * 500_000.0;
        let ideal = midi.block_time(n);
        let now = if late >= 0.0 {
            ideal + Duration::from_nanos(late as u64)
        } else {
            ideal - Duration::from_nanos((-late) as u64)
        };
        midi.block_at(now);
        // Block n holds what arrived during block n - 1: frame f of it is
        // song frame (n - 1) * 512 + f after the first block's start.
        if n > 1_000 {
            for event in midi.rig.processor.midi_input(keys).unwrap().events {
                let index = u64::from(event.message.data1());
                let frame = (n - 1) * BLOCK as u64 + u64::from(event.offset);
                // Which message this is: the nearest whose time fits.
                let expected = (0..next)
                    .filter(|k| k % 128 == index)
                    .map(|k| (spacing * k as u32).as_secs_f64() * 48_000.0)
                    .min_by(|a, b| {
                        (a - frame as f64)
                            .abs()
                            .total_cmp(&(b - frame as f64).abs())
                    })
                    .unwrap();
                worst = worst.max((frame as f64 - expected).abs());
                placed += 1;
            }
        }
    }
    eprintln!(
        "simulated callback jitter ±0.5 ms: {placed} messages placed within {worst:.1} frames \
         ({:.3} ms) of their timestamps",
        worst / 48.0
    );
    assert!(placed > 1_000);
    assert!(worst <= 12.0, "{worst} frames");
    assert_eq!(midi.rig.processor.midi_counters().late, 0);
}

/// The engine as a stream's callback, recording the engine frame each
/// message on one track's input landed on, without allocating.
struct Play {
    processor: Processor,
    track: TrackId,
    landed: Arc<Vec<AtomicU64>>,
    count: Arc<AtomicUsize>,
}

impl AudioCallback for Play {
    fn process(
        &mut self,
        ctx: &ProcessContext,
        _input: &AudioBuffer<'_>,
        _output: &mut AudioBufferMut<'_>,
    ) {
        let now = Instant::now();
        let clock = self.processor.clock();
        self.processor.process_at(ctx.frames, &NoInput, now);
        if let Some(input) = self.processor.midi_input(self.track) {
            for &MidiEvent { offset, .. } in input.events {
                let i = self.count.load(Relaxed);
                if i < self.landed.len() {
                    self.landed[i].store(clock + u64::from(offset), Relaxed);
                    self.count.store(i + 1, Relaxed);
                }
            }
        }
    }
}

// Spec: midi-io R13 to R16
#[test]
fn timing_jitter_with_a_real_time_audio_thread_and_a_midi_thread() {
    // The null device paces 256-frame blocks like a sound card, on a
    // real-time thread under the allocation trap. A MIDI thread sends 300
    // messages 7.3 ms apart, stamping each as the device layer does. The
    // positions they land on, against their stamps, measure the jitter
    // the whole path adds.
    const COUNT: usize = 300;
    let mut rig = Rig::new(4096);
    let keys = track(&mut rig.editor, TrackKind::Midi, None);
    rig.sync();
    let (mut queue, consumer) = RingBuffer::new(1024);
    rig.processor.set_midi_input(consumer);

    let landed: Arc<Vec<AtomicU64>> = Arc::new((0..COUNT).map(|_| AtomicU64::new(0)).collect());
    let count = Arc::new(AtomicUsize::new(0));
    let callback = Play {
        processor: rig.processor,
        track: keys,
        landed: Arc::clone(&landed),
        count: Arc::clone(&count),
    };
    let backend = NullBackend::with_options(NullOptions {
        dropout_after: None,
        pacing: Pacing::RealTime,
        ..NullOptions::default()
    });
    let request = StreamRequest {
        output: Some(PortRequest::default()),
        input: None,
        sample_rate: Some(RATE.hz()),
        buffer_size: Some(256),
    };
    let mut stream = backend.open_stream(&request, Box::new(callback)).unwrap();
    stream.start().unwrap();
    // Let the block clock lock.
    thread::sleep(Duration::from_secs(3));

    let done = Arc::new(AtomicBool::new(false));
    let sender = {
        let done = Arc::clone(&done);
        thread::spawn(move || {
            let spacing = Duration::from_micros(7_300);
            let began = Instant::now() + ms(5);
            let mut stamps = Vec::with_capacity(COUNT);
            for i in 0..COUNT {
                let due = began + spacing * i as u32;
                // Sleep most of the way, then spin to the moment.
                if let Some(wait) = due.checked_duration_since(Instant::now() + ms(1)) {
                    thread::sleep(wait);
                }
                while Instant::now() < due {
                    std::hint::spin_loop();
                }
                let time = Instant::now();
                let message = MidiMessage::note_on(0, (i % 128) as u8, 100);
                queue
                    .push(MidiInputEvent {
                        source: MidiSource::ComputerKeyboard,
                        time,
                        message,
                    })
                    .unwrap();
                stamps.push(time);
            }
            done.store(true, Relaxed);
            stamps
        })
    };
    let stamps = sender.join().unwrap();
    assert!(done.load(Relaxed));
    let waited = Instant::now();
    while count.load(Relaxed) < COUNT && waited.elapsed() < Duration::from_secs(5) {
        thread::sleep(ms(10));
    }
    stream.stop().unwrap();
    let stats = stream.stats();
    assert_eq!(count.load(Relaxed), COUNT, "every message landed");

    // Each message's landing frame against its stamp, after taking out the
    // constant latency and the device clock's rate (a least-squares line).
    let x: Vec<f64> = stamps
        .iter()
        .map(|s| s.duration_since(stamps[0]).as_secs_f64())
        .collect();
    let y: Vec<f64> = landed.iter().map(|f| f.load(Relaxed) as f64).collect();
    let n = COUNT as f64;
    let (mean_x, mean_y) = (x.iter().sum::<f64>() / n, y.iter().sum::<f64>() / n);
    let slope = x
        .iter()
        .zip(&y)
        .map(|(a, b)| (a - mean_x) * (b - mean_y))
        .sum::<f64>()
        / x.iter().map(|a| (a - mean_x).powi(2)).sum::<f64>();
    let residuals: Vec<f64> = x
        .iter()
        .zip(&y)
        .map(|(a, b)| b - (mean_y + slope * (a - mean_x)))
        .collect();
    let worst = residuals.iter().fold(0.0f64, |w, r| w.max(r.abs()));
    let rms = (residuals.iter().map(|r| r * r).sum::<f64>() / n).sqrt();
    eprintln!(
        "real-time path: {COUNT} messages 7.3 ms apart over 256-frame blocks at 48 kHz; \
         jitter {rms:.2} frames RMS ({:.3} ms), worst {worst:.1} frames ({:.3} ms); device rate \
         as measured {slope:.1} frames per second; {} dropouts, {} heap operations on the \
         audio thread",
        rms / 48.0,
        worst / 48.0,
        stats.dropouts,
        stats.heap_operations,
    );
    assert_eq!(stats.heap_operations, 0);
    assert!(stats.audio_thread.unwrap().trap_installed);
    // Generous bounds, so a busy test machine doesn't fail them: a block
    // is 256 frames, and raw callback jitter on a loaded machine reaches a
    // few milliseconds.
    assert!(rms < 48.0, "{rms} frames RMS");
    assert!(worst < 144.0, "{worst} frames");
    assert!((slope - 48_000.0).abs() < 48.0, "{slope}");
}

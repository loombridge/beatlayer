//! The whole input path with the test backend, as CI runs it: a port's
//! messages and the computer keyboard's notes reach the engine's per-track
//! MIDI input (WP-2.2, `docs/specs/midi-io.md`), on the samples their
//! timestamps give, with every engine block a real-time section under the
//! allocation trap.
//!
//! `cargo test -p bl-midi-io --test to_the_engine -- --nocapture` prints the
//! timing measurement.

bl_rt::install_trap!();

use std::thread;
use std::time::{Duration, Instant};

use bl_core::midi::{MidiInputEvent, MidiMessage, MidiSource};
use bl_core::time::SampleRate;
use bl_engine::{Collector, Compiler, EngineConfig, Processor, Registry, Sender, channel};
use bl_midi_io::{ComputerKeyboard, Direction, MidiPorts, PortSwitches, TestBackend, input_queue};
use bl_model::{Command, Editor, Input, Monitor, TrackId, TrackKind};

const BLOCK: usize = 256;

/// An engine on the test's thread, a document, and the MIDI ports.
struct Rig {
    editor: Editor,
    compiler: Compiler,
    sender: Sender,
    processor: Processor,
    _collector: Collector,
    test: TestBackend,
    ports: MidiPorts,
}

impl Rig {
    fn new() -> Self {
        let config = EngineConfig::new(SampleRate::HZ_48000, BLOCK);
        let collector = Collector::new();
        let compiler = Compiler::new(config, Registry::new(), collector.handle());
        let (sender, mut processor, _monitor) = channel(&config);
        let (input, engine_end) = input_queue(1024);
        processor.set_midi_input(engine_end);
        let test = TestBackend::new();
        let ports = MidiPorts::new(Box::new(test.clone()), input);
        Self {
            editor: Editor::new(),
            compiler,
            sender,
            processor,
            _collector: collector,
            test,
            ports,
        }
    }

    fn midi_track(&mut self) -> TrackId {
        let id = self.editor.new_id();
        self.apply(Command::CreateTrack {
            id,
            kind: TrackKind::Midi,
            name: String::new(),
            parent: None,
            index: 0,
        });
        id
    }

    fn apply(&mut self, command: Command) {
        self.editor.apply(command).unwrap();
        self.sender
            .send_update(self.compiler.sync(self.editor.document()));
        assert!(self.sender.flush());
    }

    fn block_at(&mut self, now: Instant) {
        let processor = &mut self.processor;
        bl_rt::rt_test!({
            processor.process_at(BLOCK, now);
        });
    }

    fn events(&self, track: TrackId) -> Vec<(u32, Vec<u8>)> {
        self.processor
            .midi_input(track)
            .unwrap()
            .events
            .iter()
            .map(|e| (e.offset, e.message.bytes().to_vec()))
            .collect()
    }
}

fn block_length(blocks: u32) -> Duration {
    Duration::from_nanos(u64::from(blocks) * BLOCK as u64 * 1_000_000_000 / 48_000)
}

// Spec: midi-io R5, R7, R12, R15, R21, R23
#[test]
fn a_ports_notes_and_the_computer_keyboards_reach_their_tracks_inputs() {
    let mut rig = Rig::new();
    rig.test.add_port(Direction::Input, "Keys");
    rig.test.add_port(Direction::Input, "Pads");
    rig.ports.refresh().unwrap();
    // Pads only controls Beatlayer: its notes don't reach tracks.
    rig.ports
        .set_switches(
            Direction::Input,
            "Pads",
            PortSwitches {
                remote: true,
                ..PortSwitches::OFF
            },
        )
        .unwrap();
    let all = rig.midi_track();
    let keys = rig.midi_track();
    rig.apply(Command::SetInput {
        track: keys,
        input: Input::Port {
            name: "Keys".to_owned(),
        },
        channel: None,
    });
    rig.apply(Command::SetMonitor {
        track: keys,
        monitor: Monitor::In,
    });

    let start = Instant::now();
    rig.block_at(start);
    // During the first block: a note on Keys 1 ms in, one on Pads, and the
    // computer keyboard's A 2 ms in.
    assert!(
        rig.test
            .send("Keys", start + Duration::from_millis(1), &[0x90, 64, 90])
    );
    assert!(
        rig.test
            .send("Pads", start + Duration::from_millis(1), &[0x99, 36, 127])
    );
    let mut keyboard = ComputerKeyboard::new();
    keyboard.set_on(true);
    let note = keyboard.press(0).unwrap();
    assert!(rig.ports.sender().send(MidiInputEvent {
        source: MidiSource::ComputerKeyboard,
        time: start + Duration::from_millis(2),
        message: note,
    }));
    rig.block_at(start + block_length(1));

    assert_eq!(
        rig.events(all),
        vec![(48, vec![0x90, 64, 90]), (96, vec![0x90, 60, 100])]
    );
    assert_eq!(rig.events(keys), vec![(48, vec![0x90, 64, 90])]);
    assert!(rig.processor.midi_input(keys).unwrap().monitored);
    assert!(!rig.processor.midi_input(all).unwrap().monitored);
    let stats = rig.ports.sender().stats().snapshot();
    assert_eq!((stats.to_tracks, stats.to_remote), (2, 1));
}

// Spec: midi-io R13 to R16
#[test]
fn timing_jitter_from_a_midi_thread_through_the_test_backend() {
    // A MIDI thread sends 200 notes 6.1 ms apart into the test backend,
    // which calls the port's handler on that thread, as a MIDI system calls
    // it on its own. The engine runs a 256-frame block every 5.33 ms on
    // this thread, as a device's callback would. Each note's landing frame
    // against its send time is the jitter of the path.
    const COUNT: u32 = 200;
    let mut rig = Rig::new();
    rig.test.add_port(Direction::Input, "Keys");
    rig.ports.refresh().unwrap();
    let keys = rig.midi_track();

    let test = rig.test.clone();
    let spacing = Duration::from_micros(6_100);
    let begin = Instant::now() + Duration::from_secs(2);
    let midi = thread::spawn(move || {
        let mut sent = Vec::new();
        for i in 0..COUNT {
            let due = begin + spacing * i;
            spin_until(due);
            let now = Instant::now();
            assert!(test.send("Keys", now, &[0x90, (i % 128) as u8, 100]));
            sent.push(now);
        }
        sent
    });

    let period = block_length(1);
    let first = Instant::now();
    let mut landed = Vec::new();
    let mut n = 0u32;
    while landed.len() < COUNT as usize {
        assert!(n < 20_000, "the notes never arrived");
        let due = first + period * n;
        spin_until(due);
        let clock = rig.processor.clock();
        // The callback's own time, as a device gives it: the harness thread
        // can't promise it, so the block is told when it was due.
        rig.block_at(due);
        for &(offset, _) in &rig.events(keys) {
            landed.push(clock as f64 + f64::from(offset));
        }
        n += 1;
    }
    let sent = midi.join().unwrap();

    // Each note's frame against its send time, after taking out the fixed
    // latency (one block) and fitting the engine's frame rate.
    let x: Vec<f64> = sent
        .iter()
        .map(|t| t.duration_since(sent[0]).as_secs_f64())
        .collect();
    let count = f64::from(COUNT);
    let (mean_x, mean_y) = (
        x.iter().sum::<f64>() / count,
        landed.iter().sum::<f64>() / count,
    );
    let slope = x
        .iter()
        .zip(&landed)
        .map(|(a, b)| (a - mean_x) * (b - mean_y))
        .sum::<f64>()
        / x.iter().map(|a| (a - mean_x).powi(2)).sum::<f64>();
    let residuals: Vec<f64> = x
        .iter()
        .zip(&landed)
        .map(|(a, b)| b - (mean_y + slope * (a - mean_x)))
        .collect();
    let worst = residuals.iter().fold(0.0f64, |w, r| w.max(r.abs()));
    let rms = (residuals.iter().map(|r| r * r).sum::<f64>() / count).sqrt();
    let late = rig.processor.midi_counters().late;
    eprintln!(
        "test backend to engine: {COUNT} notes 6.1 ms apart, 256-frame blocks at 48 kHz: \
         jitter {rms:.2} frames RMS ({:.3} ms), worst {worst:.1} frames ({:.3} ms); \
         {slope:.1} frames per second; {late} late",
        rms / 48.0,
        worst / 48.0
    );
    // A note stamped just before a block starts but queued just after it
    // misses that block: rare, and counted.
    assert!(late < u64::from(COUNT / 20), "{late} late");
    // Generous bounds for a busy test machine; a quiet one measures well
    // under a frame.
    assert!(rms < 24.0, "{rms}");
    assert!(worst < 96.0, "{worst}");
}

/// Sleeps most of the way to `due`, then spins to it.
fn spin_until(due: Instant) {
    if let Some(wait) = due.checked_duration_since(Instant::now() + Duration::from_millis(1)) {
        thread::sleep(wait);
    }
    while Instant::now() < due {
        std::hint::spin_loop();
    }
}

// Spec: midi-io R19, R21
#[test]
fn the_keyboard_s_note_offs_follow_the_notes_it_started() {
    let mut keyboard = ComputerKeyboard::new();
    keyboard.set_on(true);
    let on = keyboard.press(9).unwrap();
    keyboard.shift_octave(1);
    let off = keyboard.release(9).unwrap();
    assert_eq!(on, MidiMessage::note_on(0, 69, 100));
    assert_eq!(off, MidiMessage::note_off(0, 69));
}

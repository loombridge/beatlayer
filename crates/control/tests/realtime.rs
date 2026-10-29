//! A running engine and a flood of messages through a surface
//! (`docs/specs/control-surfaces.md`, R5 and R16): notes the surface plays
//! reach a track whose input is All Ins with the port's Track switch off,
//! and the audio thread allocates nothing while the host handles 10,000
//! messages. Every engine block is a real-time section under the allocation
//! trap.

use std::time::{Duration, Instant};

use bl_control::testing::{Journal, Reply};
use bl_control::{RowConfig, Surfaces};
use bl_core::clock::TestClock;
use bl_core::time::SampleRate;
use bl_engine::{Collector, Compiler, EngineConfig, NoInput, Processor, Registry, channel};
use bl_midi_io::{Direction, MidiPorts, OutputBus, PortSwitches, TestBackend, input_queue};
use bl_model::{Command, Editor, TrackId, TrackKind};

const BLOCK: usize = 256;

fn block_length() -> Duration {
    Duration::from_nanos(BLOCK as u64 * 1_000_000_000 / 48_000)
}

struct Rig {
    processor: Processor,
    _collector: Collector,
    test: TestBackend,
    _ports: MidiPorts,
    surfaces: Surfaces,
    track: TrackId,
    now: Instant,
}

impl Rig {
    fn new() -> Self {
        let config = EngineConfig::new(SampleRate::HZ_48000, BLOCK);
        let collector = Collector::new();
        let mut compiler = Compiler::new(config, Registry::new(), collector.handle());
        let (mut sender, mut processor, _monitor) = channel(&config);
        let (input, engine_end) = input_queue(1024);
        processor.set_midi_input(engine_end);
        let mut editor = Editor::new();
        let track = editor.new_id();
        editor
            .apply(Command::CreateTrack {
                id: track,
                kind: TrackKind::Midi,
                name: String::new(),
                parent: None,
                index: 0,
            })
            .unwrap();
        sender.send_update(compiler.sync(editor.document()));
        assert!(sender.flush());

        let test = TestBackend::new();
        test.add_port(Direction::Input, "Pads");
        test.add_port(Direction::Output, "Pads Out");
        let bus = OutputBus::new();
        let mut ports =
            MidiPorts::new(Box::new(test.clone()), input.clone()).with_output(bus.clone());
        // Track is off: only the surface's own notes reach the track.
        ports.remember(Direction::Input, "Pads", PortSwitches::OFF);
        ports.refresh().unwrap();
        let mut surfaces = Surfaces::new(input, bus);
        surfaces
            .set_row(
                0,
                RowConfig {
                    surface: Some("test".to_owned()),
                    input: Some("Pads".to_owned()),
                    output: Some("Pads Out".to_owned()),
                },
            )
            .unwrap();
        surfaces.reconcile(&mut ports);
        surfaces.settle();
        Self {
            processor,
            _collector: collector,
            test,
            _ports: ports,
            surfaces,
            track,
            now: TestClock::new().now(),
        }
    }

    fn journal(&self) -> Journal {
        self.surfaces
            .probe(0)
            .unwrap()
            .downcast::<Journal>()
            .map(|journal| (*journal).clone())
            .unwrap_or_else(|_| panic!("the probe is a journal"))
    }

    /// Runs one block under the allocation trap, and returns the events the
    /// track's input got.
    fn block(&mut self) -> Vec<Vec<u8>> {
        let (processor, now) = (&mut self.processor, self.now);
        bl_rt::rt_test!({
            processor.process_at(BLOCK, &NoInput, now);
        });
        self.now += block_length();
        self.processor
            .midi_input(self.track)
            .unwrap()
            .events
            .iter()
            .map(|e| e.message.bytes().to_vec())
            .collect()
    }
}

// Spec: control-surfaces R5. A note the surface plays reaches a track whose
// input is All Ins even though the port's Track switch is off.
#[test]
fn a_note_a_surface_plays_reaches_an_all_ins_track_with_track_off() {
    let mut rig = Rig::new();
    rig.journal().reply(Reply::Play);
    rig.block();
    assert!(rig.test.send("Pads", rig.now, &[0x99, 36, 100]));
    rig.surfaces.settle();
    // The note lands in one of the two blocks after it was played.
    let mut events = rig.block();
    events.extend(rig.block());
    assert_eq!(events, [vec![0x99, 36, 100]]);
}

// Spec: control-surfaces R16. The audio thread allocates nothing while the
// host takes 10,000 messages and sends its lights.
#[test]
fn the_audio_thread_allocates_nothing_while_the_host_takes_ten_thousand_messages() {
    let mut rig = Rig::new();
    rig.journal().reply(Reply::Play);
    let mut played = 0;
    for burst in 0..200u32 {
        for i in 0..50u32 {
            let key = ((burst + i) % 120) as u8;
            let on = rig.test.send("Pads", rig.now, &[0x99, key, 100]);
            assert!(on);
        }
        played += rig
            .block()
            .iter()
            .filter(|m| m.first() == Some(&0x99))
            .count();
    }
    rig.surfaces.settle();
    // On a slow machine the host may route every note only after the last
    // burst's block, so the two blocks after it count too.
    for _ in 0..2 {
        played += rig
            .block()
            .iter()
            .filter(|m| m.first() == Some(&0x99))
            .count();
    }
    let taken = rig.surfaces.traffic().0;
    assert_eq!(taken, 10_000, "the host took every message");
    assert!(played > 0, "the surface's notes reached the track");
}

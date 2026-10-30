//! Nothing for Push 2 runs on the audio thread (`docs/specs/push-2.md`, R35):
//! pads' notes reach the engine's input queue while the audio thread
//! allocates nothing.

use std::time::{Duration, Instant};

use bl_control::{ReadModel, RowConfig, Surfaces};
use bl_core::clock::TestClock;
use bl_core::time::SampleRate;
use bl_engine::{Collector, Compiler, EngineConfig, NoInput, Registry, channel};
use bl_midi_io::{MidiPorts, OutputBus, input_queue};
use bl_model::{Command, Editor, TrackId, TrackKind};
use bl_push::Shared;
use bl_push::content::{Content, Contents, Ctx, MakeContents, Pad, PadEvent};
use bl_push::sim::{LIVE, Simulated};
use std::sync::Arc;

const BLOCK: usize = 256;

/// A content whose pads play their notes.
struct Plays;

impl Content for Plays {
    fn name(&self) -> &str {
        "plays"
    }

    fn pad(&mut self, pad: Pad, event: PadEvent, ctx: &mut Ctx<'_>) {
        let note = bl_push::controls::pad_note(pad.row, pad.column);
        match event {
            PadEvent::On(velocity) => ctx
                .cx
                .play(bl_core::midi::MidiMessage::note_on(0, note, velocity)),
            PadEvent::Off => ctx.cx.play(bl_core::midi::MidiMessage::note_off(0, note)),
            _ => {}
        }
    }
}

fn block_length() -> Duration {
    Duration::from_nanos(BLOCK as u64 * 1_000_000_000 / 48_000)
}

// Spec: push-2 R35. The pads' notes reach a track on the lock-free input
// queue, and the audio thread allocates nothing while the surface takes 5,000
// pad messages and sends its lights. Every engine block is a real-time
// section under the allocation trap.
#[test]
fn pads_notes_reach_the_engine_without_allocating_on_the_audio_thread() {
    let config = EngineConfig::new(SampleRate::HZ_48000, BLOCK);
    let collector = Collector::new();
    let mut compiler = Compiler::new(config, Registry::new(), collector.handle());
    let (mut sender, mut processor, _monitor) = channel(&config);
    let (input, engine_end) = input_queue(4096);
    processor.set_midi_input(engine_end);
    let mut editor = Editor::new();
    let track: TrackId = editor.new_id();
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

    let backend = bl_midi_io::TestBackend::new();
    let clock = TestClock::new();
    let bus = OutputBus::new();
    let mut ports =
        MidiPorts::new(Box::new(backend.clone()), input.clone()).with_output(bus.clone());
    let mut surfaces = Surfaces::new(input, bus);
    surfaces.set_clock(clock.clone());
    let mut sim = Simulated::new(&backend, &clock);
    let make: MakeContents = Arc::new(|| {
        let mut contents = Contents::placeholders();
        contents.pads[0] = Box::new(Plays);
        contents
    });
    surfaces.set_natives(vec![Arc::new(bl_push::kind(
        Arc::new(Shared::default()),
        sim.frames().claim(),
        make,
    ))]);
    sim.plug_in();
    surfaces
        .set_row(
            0,
            RowConfig {
                surface: Some(bl_push::SURFACE_ID.to_owned()),
                input: Some(LIVE.to_owned()),
                output: Some(LIVE.to_owned()),
            },
        )
        .unwrap();
    ports.refresh().unwrap();
    surfaces.reconcile(&mut ports);
    loop {
        surfaces.settle();
        if sim.pump() == 0 {
            break;
        }
    }
    surfaces.publish(ReadModel::at_rest(Arc::new(editor.document().clone())));

    let mut now: Instant = clock.now();
    let mut played = 0;
    for burst in 0..100u32 {
        for i in 0..25u32 {
            let (row, column) = (((burst + i) / 8 % 8) as u8, ((burst + i) % 8) as u8);
            assert!(sim.press_pad(row, column, 100));
            assert!(sim.release_pad(row, column));
        }
        surfaces.tick();
        sim.pump();
        bl_rt::rt_test!({
            processor.process_at(BLOCK, &NoInput, now);
        });
        now += block_length();
        played += processor
            .midi_input(track)
            .unwrap()
            .events
            .iter()
            .filter(|e| e.message.is_note_on())
            .count();
    }
    surfaces.settle();
    for _ in 0..2 {
        bl_rt::rt_test!({
            processor.process_at(BLOCK, &NoInput, now);
        });
        now += block_length();
        played += processor
            .midi_input(track)
            .unwrap()
            .events
            .iter()
            .filter(|e| e.message.is_note_on())
            .count();
    }
    assert!(played > 0, "the pads' notes reached the track");
    assert!(
        surfaces.traffic().0 >= 5_000,
        "the host took every pad message"
    );
}

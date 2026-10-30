//! Feedback to controllers on its path (`docs/specs/remote-mapping.md`, R40
//! and R44): messages leave from the main thread straight to the output port
//! while the engine runs blocks under the allocation trap, and neither
//! waits for the other.

use std::thread;
use std::time::Instant;

use bl_core::midi::PortId;
use bl_core::time::SampleRate;
use bl_engine::{Collector, Compiler, EngineConfig, NoInput, Registry, channel};
use bl_midi_io::{Direction, MidiPorts, OutputBus, PortSwitches, TestBackend, input_queue};

const BLOCK: usize = 256;
const MESSAGES: usize = 5_000;

// Spec: remote-mapping R40, R44
#[test]
fn feedback_flows_while_the_audio_thread_allocates_nothing() {
    let config = EngineConfig::new(SampleRate::HZ_48000, BLOCK);
    let collector = Collector::new();
    let _compiler = Compiler::new(config, Registry::new(), collector.handle());
    let (_sender, mut processor, _monitor) = channel(&config);
    let bus = OutputBus::new();
    processor.set_midi_output(bus.attach(1024));
    let test = TestBackend::new();
    test.add_port(Direction::Output, "Lamps");
    let mut ports =
        MidiPorts::new(Box::new(test.clone()), input_queue(64).0).with_output(bus.clone());
    ports.refresh().unwrap();
    let remote = PortSwitches {
        remote: true,
        ..PortSwitches::OFF
    };
    ports
        .set_switches(Direction::Output, "Lamps", remote)
        .unwrap();

    let port = PortId::from_name("Lamps");
    let feedback = thread::spawn(move || {
        for n in 0..MESSAGES {
            bus.send_to(port, &[0xB0, 20, (n % 128) as u8], None)
                .unwrap();
        }
    });
    // Blocks run on this thread until the last message has left, each a
    // real-time section: an allocation or a lock would fail the test.
    while !feedback.is_finished() {
        bl_rt::rt_test!({
            processor.process_at(BLOCK, &NoInput, Instant::now());
        });
    }
    feedback.join().unwrap();
    assert_eq!(test.sent("Lamps").len(), MESSAGES);
}

//! The platform's MIDI system through `midir` (WP-2.2): ports are listed,
//! and on macOS and Linux a virtual port carries messages into an input
//! with a measured jitter. Where the system isn't there (a CI container
//! without the ALSA sequencer), the tests say so and pass.
//!
//! The virtual port's messages are polyphonic key pressure of 0 for note 0
//! on channel 16, which changes nothing in any program that sees the port
//! while the test runs.
//!
//! `cargo test -p bl-midi-io --test midi_io_tests -- platform:: --nocapture` prints the
//! ports and the measurement.

use bl_midi_io::{Direction, MidiBackend, MidirBackend};

// Spec: midi-io R1
#[test]
fn the_platforms_ports_are_listed() {
    let mut backend = MidirBackend::new();
    for direction in [Direction::Input, Direction::Output] {
        match backend.ports(direction) {
            Ok(ports) => eprintln!(
                "{} {} ports: {}",
                backend.name(),
                direction.label(),
                if ports.is_empty() {
                    "none".to_owned()
                } else {
                    ports.join(", ")
                }
            ),
            // Core MIDI and Windows MIDI are always there; the ALSA
            // sequencer may not be, in a container.
            Err(error) if cfg!(target_os = "linux") => {
                eprintln!("{} isn't available here: {error}", backend.name());
            }
            Err(error) => panic!("{} couldn't list ports: {error}", backend.name()),
        }
    }
}

// Spec: midi-io R1, R13
#[cfg(unix)]
#[test]
fn a_virtual_port_carries_messages_into_an_input_with_little_jitter() {
    use std::sync::mpsc;
    use std::thread;
    use std::time::{Duration, Instant};

    use midir::MidiOutput;
    use midir::os::unix::VirtualOutput;

    const COUNT: u32 = 100;
    // Inert: poly pressure 0 on note 0, channel 16.
    const MESSAGE: [u8; 3] = [0xAF, 0, 0];

    let name = format!("Beatlayer test {}", std::process::id());
    let output = match MidiOutput::new("Beatlayer test").map(|o| o.create_virtual(&name)) {
        Ok(Ok(output)) => output,
        Ok(Err(error)) => return eprintln!("no virtual port here: {error}"),
        Err(error) => return eprintln!("no MIDI system here: {error}"),
    };
    let mut output = output;
    let mut backend = MidirBackend::new();
    // The system takes a moment to show a new port.
    let began = Instant::now();
    while !backend
        .ports(Direction::Input)
        .unwrap_or_default()
        .contains(&name)
    {
        assert!(
            began.elapsed() < Duration::from_secs(5),
            "the virtual port never showed"
        );
        thread::sleep(Duration::from_millis(20));
    }
    let (tx, rx) = mpsc::channel();
    let input = backend
        .open_input(
            &name,
            Box::new(move |time, bytes| {
                if bytes == MESSAGE {
                    let _ = tx.send(time);
                }
            }),
        )
        .unwrap();

    let spacing = Duration::from_micros(4_700);
    let start = Instant::now() + Duration::from_millis(50);
    let mut sent = Vec::new();
    for i in 0..COUNT {
        let due = start + spacing * i;
        if let Some(wait) = due.checked_duration_since(Instant::now() + Duration::from_millis(1)) {
            thread::sleep(wait);
        }
        while Instant::now() < due {
            std::hint::spin_loop();
        }
        sent.push(Instant::now());
        output.send(&MESSAGE).unwrap();
    }
    let mut received = Vec::new();
    for _ in 0..COUNT {
        match rx.recv_timeout(Duration::from_secs(2)) {
            Ok(time) => received.push(time),
            Err(_) => break,
        }
    }
    drop(input);
    drop(output);
    assert_eq!(received.len(), COUNT as usize, "every message arrived");

    let delays: Vec<f64> = sent
        .iter()
        .zip(&received)
        .map(|(s, r)| r.duration_since(*s).as_secs_f64() * 1e6)
        .collect();
    let mean = delays.iter().sum::<f64>() / f64::from(COUNT);
    let jitter = (delays.iter().map(|d| (d - mean).powi(2)).sum::<f64>() / f64::from(COUNT)).sqrt();
    let worst = delays.iter().fold(0.0f64, |w, d| w.max((d - mean).abs()));
    eprintln!(
        "virtual port \"{name}\": {COUNT} messages 4.7 ms apart; stamped {mean:.0} µs after \
         sending on average, jitter {jitter:.0} µs RMS, worst {worst:.0} µs"
    );
    // A stamp is never later than the message's arrival, and the spacing
    // survives to well under a millisecond on a quiet machine.
    assert!(mean < 5_000.0, "{mean} µs");
    assert!(jitter < 1_000.0, "{jitter} µs");
}

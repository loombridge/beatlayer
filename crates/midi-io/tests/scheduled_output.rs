//! Scheduled MIDI output (`docs/specs/external-devices.md`, R5): a message
//! goes out on the moment it is stamped for, and one whose moment has gone
//! by is sent at once and counted late. Where the platform's MIDI system
//! isn't there (a CI container without the ALSA sequencer), the tests say so
//! and pass.
//!
//! The messages the system tests send are polyphonic key pressure of 0 for
//! note 0 on channel 16 (the value byte numbers them), which changes nothing
//! in any program that sees the port while the test runs.
//!
//! `cargo test -p bl-midi-io --test scheduled_output -- --nocapture` prints
//! the measurements.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bl_core::midi::{MidiMessage, MidiOutputEvent, PortId};
use bl_midi_io::{
    Direction, MidiBackend, MidiError, MidiPorts, OutputConnection, PortSwitches, TestBackend,
    TimedOutput, input_queue,
};

/// What reached an output, and when.
type Log = Arc<Mutex<Vec<(Instant, Vec<u8>)>>>;

/// An output that notes the moment each message reaches it, then passes the
/// message on.
struct Recorder {
    inner: Box<dyn OutputConnection>,
    log: Log,
}

impl OutputConnection for Recorder {
    fn send(&mut self, bytes: &[u8]) -> Result<(), MidiError> {
        let now = Instant::now();
        self.log.lock().unwrap().push((now, bytes.to_vec()));
        self.inner.send(bytes)
    }
}

/// The value below which `fraction` of `sorted` lies.
fn percentile(sorted: &[f64], fraction: f64) -> f64 {
    let last = sorted.len() - 1;
    sorted[((last as f64) * fraction).ceil() as usize]
}

// Spec: external-devices R5
#[test]
fn each_backend_sends_on_its_stamp_within_its_resolution() {
    timer_thread_sends_on_its_stamp();
    #[cfg(unix)]
    system_sends_on_its_stamp();
}

/// The timer thread that Windows uses, over a test port, on every platform.
fn timer_thread_sends_on_its_stamp() {
    const COUNT: usize = 50;

    let test = TestBackend::new();
    test.add_port(Direction::Output, "Synth");
    let log = Arc::new(Mutex::new(Vec::new()));
    let recorder = Recorder {
        inner: test.clone().open_output("Synth").unwrap(),
        log: Arc::clone(&log),
    };
    let mut output = TimedOutput::new(Box::new(recorder));

    // Moments 2 to 60 ms ahead at irregular spacing: about 1.16 ms apart
    // plus a pseudo-random part of up to 1 ms. Two pairs share a moment, to
    // see that ties leave in the order they came.
    let base = Instant::now();
    let mut seed = 0x2545_F491_4F6C_DD1D_u64;
    let mut offsets: Vec<f64> = (0..COUNT)
        .map(|i| {
            seed = seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            2.0 + i as f64 * 1.16 + ((seed >> 33) % 1_000) as f64 / 1_000.0
        })
        .collect();
    offsets[21] = offsets[20];
    offsets[40] = offsets[39];
    let stamps: Vec<Instant> = offsets
        .iter()
        .map(|ms| base + Duration::from_secs_f64(ms / 1_000.0))
        .collect();

    // Handed over out of order (the step of 7 visits every index of 50), so
    // the order they leave in is the thread's doing.
    let handed: Vec<usize> = (0..COUNT).map(|n| n * 7 % COUNT).collect();
    for &i in &handed {
        output.send_at(&[0x90, i as u8, 100], stamps[i]).unwrap();
    }
    assert!(
        stamps[0] > Instant::now(),
        "handing the messages over took longer than the first moment"
    );
    let mut expected: Vec<usize> = (0..COUNT).collect();
    expected.sort_by_key(|&i| (stamps[i], handed.iter().position(|&h| h == i)));

    let waited = Instant::now();
    while log.lock().unwrap().len() < COUNT {
        assert!(
            waited.elapsed() < Duration::from_secs(2),
            "only {} of {COUNT} messages arrived",
            log.lock().unwrap().len()
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    drop(output);

    let log = log.lock().unwrap();
    let order: Vec<usize> = log.iter().map(|(_, bytes)| bytes[1] as usize).collect();
    assert_eq!(
        order, expected,
        "messages leave in the order of their moments"
    );
    assert_eq!(test.sent("Synth").len(), COUNT);

    let mut delays: Vec<f64> = log
        .iter()
        .map(|(arrived, bytes)| {
            let stamp = stamps[bytes[1] as usize];
            assert!(*arrived >= stamp, "a message left before its moment");
            arrived.duration_since(stamp).as_secs_f64() * 1_000.0
        })
        .collect();
    delays.sort_by(f64::total_cmp);
    let (median, p95, max) = (
        percentile(&delays, 0.5),
        percentile(&delays, 0.95),
        delays[COUNT - 1],
    );
    eprintln!(
        "timer thread: {COUNT} messages, after their moment by {median:.3} ms median, \
         {p95:.3} ms at the 95th percentile, {max:.3} ms at most"
    );
    // The rule is "within 1 ms", which a quiet machine meets for nearly
    // every message. A loaded one (this test runs beside others) can
    // preempt the thread once in a while, so the median and the 95th
    // percentile must meet the rule and the worst only be bounded. As the
    // other tests of wall-clock limits do, it judges them only at full
    // depth.
    if full_depth() {
        assert!(median < 1.0, "median {median} ms");
        assert!(p95 < 1.0, "95th percentile {p95} ms");
        assert!(max < 5.0, "worst {max} ms");
    }
}

/// Whether the tests run at full depth: `BL_TEST_DEPTH=full`, as on pushes
/// to main and the nightly run.
fn full_depth() -> bool {
    match std::env::var("BL_TEST_DEPTH").as_deref() {
        Ok("full") => true,
        Err(_) | Ok("" | "quick") => false,
        Ok(other) => panic!("BL_TEST_DEPTH is `{other}`: set it to quick or full"),
    }
}

/// Core MIDI or the ALSA sequencer, over a virtual input that Beatlayer's
/// output sends to.
#[cfg(unix)]
fn system_sends_on_its_stamp() {
    use std::sync::mpsc;

    use bl_midi_io::MidirBackend;
    use midir::MidiInput;
    use midir::os::unix::VirtualInput;

    const COUNT: u8 = 30;

    let name = format!("Beatlayer scheduled test {}", std::process::id());
    let (tx, rx) = mpsc::channel();
    // A virtual input made by `midir` is an output port to Beatlayer. The
    // callback notes when each message reaches it, and the system's own
    // stamp, which is in microseconds on a clock of the system's choosing.
    let input = match MidiInput::new("Beatlayer test").map(|i| {
        i.create_virtual(
            &name,
            move |system_stamp, bytes, ()| {
                let arrived = Instant::now();
                if bytes.len() == 3 && bytes[0] == 0xAF {
                    let _ = tx.send((arrived, system_stamp, bytes[2]));
                }
            },
            (),
        )
    }) {
        Ok(Ok(input)) => input,
        Ok(Err(error)) => return eprintln!("no virtual port here: {error}"),
        Err(error) => return eprintln!("no MIDI system here: {error}"),
    };

    let mut backend = MidirBackend::new();
    // The system takes a moment to show a new port, and the ALSA sequencer
    // names it with its client and port numbers around the name.
    let began = Instant::now();
    let port = loop {
        let ports = backend.ports(Direction::Output).unwrap_or_default();
        if let Some(port) = ports.into_iter().find(|p| p.contains(&name)) {
            break port;
        }
        assert!(
            began.elapsed() < Duration::from_secs(5),
            "the virtual port never showed"
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    let mut output = backend.open_output(&port).unwrap();

    // Stamps 20 to 60 ms ahead, irregularly spaced and handed over in order,
    // as the output thread hands them.
    let base = Instant::now();
    let stamps: Vec<Instant> = (0..COUNT)
        .map(|i| {
            let ms = 20.0 + f64::from(i) * 1.3 + f64::from(i * 7 % 5) * 0.4;
            base + Duration::from_secs_f64(ms / 1_000.0)
        })
        .collect();
    for (i, &stamp) in stamps.iter().enumerate() {
        output.send_at(&[0xAF, 0, i as u8], stamp).unwrap();
    }
    let handed_over = Instant::now();
    assert!(
        stamps[0] > handed_over + Duration::from_millis(10),
        "handing the messages over took too long"
    );

    let mut arrivals = Vec::new();
    for _ in 0..COUNT {
        match rx.recv_timeout(Duration::from_secs(2)) {
            Ok(arrival) => arrivals.push(arrival),
            Err(_) => break,
        }
    }
    drop(output);
    drop(input);
    assert_eq!(arrivals.len(), usize::from(COUNT), "every message arrived");

    let order: Vec<u8> = arrivals.iter().map(|&(_, _, i)| i).collect();
    assert_eq!(order, (0..COUNT).collect::<Vec<_>>(), "in order");
    let mut delays: Vec<f64> = arrivals
        .iter()
        .map(|&(arrived, _, i)| {
            let stamp = stamps[usize::from(i)];
            if arrived >= stamp {
                arrived.duration_since(stamp).as_secs_f64() * 1_000.0
            } else {
                -stamp.duration_since(arrived).as_secs_f64() * 1_000.0
            }
        })
        .collect();
    let (first, last) = (arrivals[0].1, arrivals[usize::from(COUNT) - 1].1);
    eprintln!(
        "virtual port \"{name}\": {COUNT} messages stamped 20 to 60 ms ahead; the system's \
         stamps span {} µs where the moments span {} µs",
        last.wrapping_sub(first),
        stamps[usize::from(COUNT) - 1]
            .duration_since(stamps[0])
            .as_micros()
    );
    delays.sort_by(f64::total_cmp);
    eprintln!(
        "virtual port: arrived {:.3} ms to {:.3} ms after their moments, {:.3} ms median",
        delays[0],
        delays[delays.len() - 1],
        percentile(&delays, 0.5)
    );
    // Looser than the timer thread's bound: the system sends on the moment,
    // but the virtual input's callback runs on a thread of its own that adds
    // its wake-up time before `Instant::now` is read, and the clocks the
    // system uses for the moment and for the message's arrival are read at
    // slightly different times. So a message may be seen up to 1 ms early
    // and up to 3 ms late.
    if full_depth() {
        assert!(delays[0] >= -1.0, "arrived {} ms early", -delays[0]);
        assert!(
            delays[delays.len() - 1] <= 3.0,
            "arrived {} ms late",
            delays[delays.len() - 1]
        );
    }
}

// Spec: external-devices R5
#[test]
fn dropping_a_timed_output_drops_what_waits() {
    let test = TestBackend::new();
    test.add_port(Direction::Output, "Synth");
    let mut output = TimedOutput::new(test.clone().open_output("Synth").unwrap());
    output
        .send_at(&[0x90, 60, 100], Instant::now() + Duration::from_secs(30))
        .unwrap();
    let began = Instant::now();
    drop(output);
    assert!(began.elapsed() < Duration::from_secs(1), "dropping hung");
    assert!(test.sent("Synth").is_empty());
}

// Spec: external-devices R5
#[test]
fn a_late_message_is_sent_at_once_and_counted() {
    let test = TestBackend::new();
    test.add_port(Direction::Output, "Synth");
    test.add_port(Direction::Output, "Muted");
    let (sender, _engine) = input_queue(64);
    let mut ports = MidiPorts::new(Box::new(test.clone()), sender);
    ports.refresh().unwrap();
    ports
        .set_switches(
            Direction::Output,
            "Synth",
            PortSwitches {
                track: true,
                ..PortSwitches::OFF
            },
        )
        .unwrap();
    // Open, but with Track off: tracks' MIDI doesn't reach it.
    ports
        .set_switches(
            Direction::Output,
            "Muted",
            PortSwitches {
                sync: true,
                ..PortSwitches::OFF
            },
        )
        .unwrap();
    let mut producer = ports.output().attach(64);

    let late = MidiMessage::note_on(0, 60, 100);
    let early = MidiMessage::note_on(0, 62, 100);
    let muted = MidiMessage::note_on(0, 64, 100);
    let muted_off = MidiMessage::note_off(0, 64);
    let now = Instant::now();
    let due = now + Duration::from_millis(40);
    let synth = PortId::from_name("Synth");
    for event in [
        MidiOutputEvent {
            port: synth,
            time: now - Duration::from_secs(1),
            message: late,
        },
        MidiOutputEvent {
            port: synth,
            time: due,
            message: early,
        },
        MidiOutputEvent {
            port: PortId::from_name("Muted"),
            time: due,
            message: muted,
        },
        MidiOutputEvent {
            port: PortId::from_name("Muted"),
            time: due,
            message: muted_off,
        },
    ] {
        producer.push(event).unwrap();
    }

    let waited = Instant::now();
    loop {
        let stats = ports.output_stats().snapshot();
        if stats.sent == 3 && stats.unrouted == 1 {
            break;
        }
        assert!(
            waited.elapsed() < Duration::from_secs(2),
            "the output thread handled only {stats:?}"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    let stats = ports.output_stats().snapshot();
    assert_eq!((stats.late, stats.refused), (1, 0));

    // Both reached the port, the late one with no stamp and the other with
    // exactly its own; the muted port got only the note off (R6), so a
    // note sent before its Track switch went off doesn't hang.
    assert_eq!(
        test.sent("Synth"),
        vec![late.bytes().to_vec(), early.bytes().to_vec()]
    );
    assert_eq!(test.sent_at("Synth"), vec![(due, early.bytes().to_vec())]);
    assert_eq!(test.sent("Muted"), vec![muted_off.bytes().to_vec()]);
}

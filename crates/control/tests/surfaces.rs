//! The control-surface host on test ports (`docs/specs/control-surfaces.md`,
//! R3 to R10, R11 and R16): the test surface runs against `TestBackend`
//! ports, with the engine's input queue on the other side.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bl_control::testing::{Call, GOODBYE, HELLO, Journal, Reply, SETUP};
use bl_control::{ReadModel, RowConfig, RunState, Surfaces};
use bl_core::clock::TestClock;
use bl_core::midi::{MidiInputEvent, MidiSource, PortId};
use bl_midi_io::{
    Direction, MidiPorts, OutputBus, PortSwitches, RemoteBits, TestBackend, input_queue,
};
use bl_model::Editor;
use rtrb::Consumer;
use serde_json::json;

struct Rig {
    test: TestBackend,
    ports: MidiPorts,
    surfaces: Surfaces,
    engine: Consumer<MidiInputEvent>,
}

impl Rig {
    fn new() -> Self {
        let test = TestBackend::new();
        let (sender, engine) = input_queue(16_384);
        let bus = OutputBus::new();
        let ports = MidiPorts::new(Box::new(test.clone()), sender.clone()).with_output(bus.clone());
        Self {
            test,
            ports,
            surfaces: Surfaces::new(sender, bus),
            engine,
        }
    }

    /// A rig with the ports `Pads` and `Pads Out`, and `Other` and `Other Out`.
    fn with_ports() -> Self {
        let mut rig = Self::new();
        for name in ["Pads", "Other"] {
            rig.test.add_port(Direction::Input, name);
            rig.test.add_port(Direction::Output, &format!("{name} Out"));
        }
        rig.sync();
        rig
    }

    /// Lists the ports again and makes the surfaces follow the rows, as the
    /// shell does after a change.
    fn sync(&mut self) {
        self.ports.refresh().unwrap();
        self.surfaces.reconcile(&mut self.ports);
        self.surfaces.settle();
    }

    fn set(
        &mut self,
        row: usize,
        surface: Option<&str>,
        input: &str,
        output: &str,
    ) -> Result<(), String> {
        self.surfaces.set_row(
            row,
            RowConfig {
                surface: surface.map(str::to_owned),
                input: Some(input.to_owned()),
                output: Some(output.to_owned()),
            },
        )?;
        self.sync();
        Ok(())
    }

    fn journal(&self, row: usize) -> Journal {
        self.surfaces
            .probe(row)
            .expect("the row runs a test surface")
            .downcast::<Journal>()
            .map(|journal| (*journal).clone())
            .unwrap_or_else(|_| panic!("the probe is a journal"))
    }

    fn send(&self, port: &str, at: Instant, bytes: &[u8]) {
        assert!(self.test.send(port, at, bytes));
    }

    fn run(&self, row: usize) -> RunState {
        self.surfaces.report(&self.ports)[row].run
    }

    fn reached_tracks(&mut self) -> Vec<MidiInputEvent> {
        std::iter::from_fn(|| self.engine.pop().ok()).collect()
    }
}

fn pads() -> PortId {
    PortId::from_name("Pads")
}

// Spec: control-surfaces R6, R8, R9. The surface starts when its row and its
// input exist, gets every message with its time (a system exclusive message
// whole), sends in the order it gave, and stops with its goodbye when the
// row changes or its port goes.
#[test]
fn a_surface_starts_gets_every_message_sends_in_order_and_stops_with_its_goodbye() {
    let mut rig = Rig::new();
    rig.set(0, Some("test"), "Pads", "Pads Out").unwrap();
    assert_eq!(rig.run(0), RunState::Waiting, "neither port is there yet");

    rig.test.add_port(Direction::Input, "Pads");
    rig.test.add_port(Direction::Output, "Pads Out");
    rig.sync();
    assert_eq!(rig.run(0), RunState::Running);
    assert_eq!(rig.test.sent("Pads Out"), [HELLO]);

    let now = TestClock::new().now();
    let (first, second) = (now, now + Duration::from_millis(3));
    let sysex = [0xF0, 0x7D, 1, 2, 3, 0xF7];
    rig.send("Pads", first, &[0x90, 36, 100]);
    rig.send("Pads", second, &sysex);
    rig.send("Pads", second, &[0xB0, 7, 64]);
    rig.surfaces.settle();
    let journal = rig.journal(0);
    assert_eq!(
        journal.messages(),
        [
            (first, vec![0x90, 36, 100]),
            (second, sysex.to_vec()),
            (second, vec![0xB0, 7, 64]),
        ]
    );
    assert_eq!(
        rig.test.sent("Pads Out"),
        [HELLO, &[0x90, 36, 100][..], &sysex, &[0xB0, 7, 64]]
    );
    assert_eq!(rig.surfaces.traffic().0, 3);

    // The row changes: the surface stops and says goodbye.
    rig.surfaces.set_row(0, RowConfig::default()).unwrap();
    rig.sync();
    assert_eq!(rig.test.sent("Pads Out").last().unwrap(), GOODBYE);
    assert_eq!(rig.run(0), RunState::Empty);
    assert_eq!(journal.calls().last(), Some(&Call::Stop));

    // The port goes: it stops, and starts again when the port comes back.
    rig.set(0, Some("test"), "Pads", "Pads Out").unwrap();
    assert_eq!(rig.run(0), RunState::Running);
    let sent = rig.test.sent("Pads Out").len();
    rig.test.remove_port(Direction::Input, "Pads");
    rig.sync();
    assert_eq!(rig.run(0), RunState::Waiting);
    assert_eq!(rig.test.sent("Pads Out").len(), sent + 1);
    assert_eq!(rig.test.sent("Pads Out").last().unwrap(), GOODBYE);
    rig.test.add_port(Direction::Input, "Pads");
    rig.sync();
    assert_eq!(rig.run(0), RunState::Running);
}

// Spec: control-surfaces R4. A message a manual mapping takes never reaches
// the surface; one no mapping takes does, and reaches no track.
#[test]
fn a_message_a_mapping_takes_never_reaches_the_surface() {
    let mut rig = Rig::with_ports();
    rig.set(0, Some("test"), "Pads", "Pads Out").unwrap();
    rig.ports
        .set_switches(
            Direction::Input,
            "Pads",
            PortSwitches {
                track: true,
                remote: true,
                ..PortSwitches::OFF
            },
        )
        .unwrap();
    let mut bits = RemoteBits::default();
    bits.controller(0, 20);
    rig.ports.sender().remote().publish(&bits);

    let now = TestClock::new().now();
    rig.send("Pads", now, &[0xB0, 20, 99]);
    rig.send("Pads", now, &[0xB0, 21, 99]);
    rig.surfaces.settle();
    assert_eq!(rig.journal(0).messages(), [(now, vec![0xB0, 21, 99])]);
    let mut taken = Vec::new();
    rig.ports.sender().remote().drain(&mut taken);
    assert_eq!(taken.len(), 1);
    assert_eq!(taken[0].message.bytes(), &[0xB0, 20, 99]);
    assert!(
        rig.reached_tracks().is_empty(),
        "neither message reached a track"
    );
}

// Spec: control-surfaces R4. A message the surface takes reaches no track;
// one it hands back does while the port's Track switch is on.
#[test]
fn a_message_the_surface_takes_never_reaches_a_track_and_one_it_hands_back_does_under_track() {
    let mut rig = Rig::with_ports();
    rig.set(0, Some("test"), "Pads", "Pads Out").unwrap();
    let now = TestClock::new().now();

    rig.send("Pads", now, &[0x90, 60, 100]);
    rig.surfaces.settle();
    assert!(rig.reached_tracks().is_empty());

    rig.journal(0).reply(Reply::HandBack);
    rig.send("Pads", now, &[0x90, 61, 100]);
    rig.surfaces.settle();
    let handed = rig.reached_tracks();
    assert_eq!(handed.len(), 1);
    assert_eq!(handed[0].source, MidiSource::Port(pads()));
    assert_eq!(handed[0].time, now);
    assert_eq!(handed[0].message.bytes(), &[0x90, 61, 100]);

    // With Track off the port's switches keep it from tracks.
    rig.ports
        .set_switches(Direction::Input, "Pads", PortSwitches::OFF)
        .unwrap();
    rig.send("Pads", now, &[0x90, 62, 100]);
    rig.surfaces.settle();
    assert!(rig.reached_tracks().is_empty());
    assert_eq!(
        rig.run(0),
        RunState::Running,
        "the port stays open and held"
    );
}

// Spec: control-surfaces R5. Notes the surface plays reach the tracks as the
// port's, whatever its Track switch says.
#[test]
fn notes_a_surface_plays_reach_tracks_with_track_off() {
    let mut rig = Rig::with_ports();
    rig.ports
        .set_switches(Direction::Input, "Pads", PortSwitches::OFF)
        .unwrap();
    rig.set(0, Some("test"), "Pads", "Pads Out").unwrap();
    rig.journal(0).reply(Reply::Play);
    let now = TestClock::new().now();
    rig.send("Pads", now, &[0x99, 36, 90]);
    rig.surfaces.settle();
    let played = rig.reached_tracks();
    assert_eq!(played.len(), 1);
    assert_eq!(played[0].source, MidiSource::Port(pads()));
    assert_eq!(played[0].message.bytes(), &[0x99, 36, 90]);
}

// Spec: control-surfaces R3. A port a row names is open whatever its
// switches say, a port serves one row, and up to six surfaces run.
#[test]
fn a_port_a_row_names_is_open_and_serves_one_row() {
    let mut rig = Rig::with_ports();
    rig.ports
        .set_switches(Direction::Input, "Pads", PortSwitches::OFF)
        .unwrap();
    assert!(!rig.test.is_open(Direction::Input, "Pads"));
    rig.set(0, Some("test"), "Pads", "Pads Out").unwrap();
    assert!(rig.test.is_open(Direction::Input, "Pads"));
    assert!(rig.test.is_open(Direction::Output, "Pads Out"));

    let refused = rig.set(1, Some("test"), "Pads", "Other Out").unwrap_err();
    assert_eq!(refused, "the port \"Pads\" is already used by row 1");
    let refused = rig.set(1, Some("test"), "Other", "Pads Out").unwrap_err();
    assert_eq!(refused, "the port \"Pads Out\" is already used by row 1");
    assert_eq!(rig.surfaces.rows()[1], RowConfig::default());
    assert!(rig.set(1, Some("nothing"), "Other", "Other Out").is_err());

    // Two surfaces of one kind, each on its own ports.
    rig.set(1, Some("test"), "Other", "Other Out").unwrap();
    assert_eq!(
        (rig.run(0), rig.run(1)),
        (RunState::Running, RunState::Running)
    );

    // Clearing the row closes a port nothing else keeps open.
    rig.surfaces.set_row(0, RowConfig::default()).unwrap();
    rig.sync();
    assert!(!rig.test.is_open(Direction::Input, "Pads"));
    assert!(!rig.test.is_open(Direction::Output, "Pads Out"));
    // And two rows can swap ports.
    rig.surfaces.set_row(1, RowConfig::default()).unwrap();
    rig.set(0, Some("test"), "Pads", "Pads Out").unwrap();
    rig.surfaces.set_row(0, RowConfig::default()).unwrap();
    rig.surfaces
        .set_row(
            1,
            RowConfig {
                surface: Some("test".to_owned()),
                input: Some("Pads".to_owned()),
                output: Some("Pads Out".to_owned()),
            },
        )
        .unwrap();
    rig.sync();
    assert!(rig.test.is_open(Direction::Input, "Pads"));
    assert_eq!(rig.run(1), RunState::Running);
}

// Spec: control-surfaces R10. A surface that sleeps 200 ms in a call is
// stopped and the others run; one that panics is stopped; choosing the
// surface again restarts it.
#[test]
fn a_surface_that_sleeps_or_panics_is_stopped_alone() {
    let mut rig = Rig::with_ports();
    rig.set(0, Some("test"), "Pads", "Pads Out").unwrap();
    rig.set(1, Some("test"), "Other", "Other Out").unwrap();
    let (slow, steady) = (rig.journal(0), rig.journal(1));
    slow.sleep(Duration::from_millis(200));

    let now = TestClock::new().now();
    rig.send("Pads", now, &[0xB0, 1, 1]);
    rig.send("Other", now, &[0xB0, 2, 2]);
    // The steady surface is served although the slow one is still asleep.
    rig.surfaces.settle();
    assert_eq!(steady.messages().len(), 1);
    let report = &rig.surfaces.report(&rig.ports)[0];
    assert_eq!(report.run, RunState::Stopped);
    assert!(
        report.error.as_deref().unwrap().contains("100 ms"),
        "{report:?}"
    );
    assert_eq!(rig.run(1), RunState::Running);

    // The stopped surface gets no more calls, and the other keeps going.
    rig.send("Pads", now, &[0xB0, 1, 2]);
    rig.send("Other", now, &[0xB0, 2, 3]);
    rig.surfaces.settle();
    assert_eq!(steady.messages().len(), 2);
    assert_eq!(slow.messages().len(), 1);

    // Reconciling, as a refresh of the ports does, doesn't restart it.
    rig.sync();
    assert_eq!(rig.run(0), RunState::Stopped);
    // Choosing the surface again does.
    rig.set(0, Some("test"), "Pads", "Pads Out").unwrap();
    assert_eq!(rig.run(0), RunState::Running);
    let again = rig.journal(0);
    again.panic();
    rig.send("Pads", now, &[0xB0, 1, 3]);
    rig.surfaces.settle();
    let report = &rig.surfaces.report(&rig.ports)[0];
    assert_eq!(report.run, RunState::Stopped);
    assert!(
        report.error.as_deref().unwrap().contains("panicked"),
        "{report:?}"
    );
    assert_eq!(rig.run(1), RunState::Running);
    assert_eq!(rig.test.sent("Pads Out").last().unwrap(), GOODBYE);
}

// Spec: control-surfaces R6, R7. The tick comes with the read model, and the
// commands a surface asks for wait for the main thread in order, with their
// gesture.
#[test]
fn ticks_bring_the_read_model_and_commands_queue_in_order() {
    let mut rig = Rig::with_ports();
    rig.set(0, Some("test"), "Pads", "Pads Out").unwrap();
    let journal = rig.journal(0);
    let editor = Editor::new();
    let tracks = editor.document().tracks().count();
    rig.surfaces.tick();
    assert_eq!(journal.ticks(), 0, "no read model has been published");
    rig.surfaces
        .publish(ReadModel::at_rest(Arc::new(editor.document().clone())));
    rig.surfaces.tick();
    rig.surfaces.tick();
    assert_eq!(journal.ticks(), 2);
    assert!(journal.calls().contains(&Call::Tick { tracks }));

    journal.knob("mixer.set_volume", json!({ "volume": "$" }));
    let now = TestClock::new().now();
    rig.send("Pads", now, &[0xB0, 7, 127]);
    rig.send("Pads", now, &[0xB0, 7, 0]);
    rig.send("Pads", now, &[0x80, 7, 0]);
    rig.surfaces.settle();
    let mut requests = Vec::new();
    rig.surfaces.take_requests(&mut requests);
    let seen: Vec<_> = requests
        .iter()
        .map(|r| {
            (
                r.row,
                r.command().unwrap().args["volume"].as_f64().unwrap(),
                r.command().unwrap().gesture,
            )
        })
        .collect();
    let gesture = |released| {
        Some(bl_control::Gesture {
            control: 7,
            released,
        })
    };
    assert_eq!(
        seen,
        [
            (0, 1.0, gesture(false)),
            (0, 0.0, gesture(false)),
            (0, 0.0, gesture(true))
        ]
    );
    assert!(
        requests
            .iter()
            .all(|r| r.command().unwrap().id == "mixer.set_volume")
    );
}

// Spec: control-surfaces R11. Dump sends the setup the surface declares to
// the row's output, and only where there is one.
#[test]
fn dump_sends_the_setup_to_the_rows_output() {
    let mut rig = Rig::with_ports();
    assert!(
        rig.surfaces.dump(0, &rig.ports).is_err(),
        "an empty row has no setup"
    );
    rig.set(0, Some("test"), "Pads", "Pads Out").unwrap();
    assert!(rig.surfaces.report(&rig.ports)[0].can_dump);
    assert_eq!(rig.surfaces.dump(0, &rig.ports), Ok(1));
    assert_eq!(rig.test.sent("Pads Out").last().unwrap(), SETUP);
    rig.test.remove_port(Direction::Output, "Pads Out");
    rig.sync();
    assert!(!rig.surfaces.report(&rig.ports)[0].can_dump);
    assert!(rig.surfaces.dump(0, &rig.ports).is_err());
}

// Spec: control-surfaces R8. A surface that needs a message on a beat stamps
// it, and the port's backend gets it with that moment; the others go at once.
#[test]
fn a_stamped_message_reaches_the_port_with_its_moment() {
    let mut rig = Rig::with_ports();
    rig.set(0, Some("test"), "Pads", "Pads Out").unwrap();
    let later = Duration::from_millis(40);
    rig.journal(0).stamp(later);
    let now = TestClock::new().now();
    rig.send("Pads", now, &[0x90, 40, 99]);
    rig.surfaces.settle();
    assert_eq!(
        rig.test.sent_at("Pads Out"),
        [(now + later, vec![0x90, 40, 99])]
    );
    assert_eq!(rig.test.sent("Pads Out").len(), 2, "the hello went at once");
}

// Spec: control-surfaces R12. What a surface holds is gone when it stops.
#[test]
fn a_stopped_surface_holds_nothing() {
    let mut rig = Rig::with_ports();
    rig.set(0, Some("test"), "Pads", "Pads Out").unwrap();
    assert!(rig.surfaces.report(&rig.ports)[0].state.is_some());
    rig.surfaces.set_row(0, RowConfig::default()).unwrap();
    rig.sync();
    assert!(rig.surfaces.report(&rig.ports)[0].state.is_none());
}

// Spec: control-surfaces R13. A running surface reports its ring and mode;
// a surface that isn't offered is reported as stopped, by its ID.
#[test]
fn the_report_names_the_state_of_each_row() {
    let mut rig = Rig::with_ports();
    rig.set(0, Some("test"), "Pads", "Pads Out").unwrap();
    let report = rig.surfaces.report(&rig.ports);
    assert_eq!(report.len(), 6);
    assert_eq!(report[0].surface_name.as_deref(), Some("Test Surface"));
    let state = report[0].state.as_ref().unwrap();
    assert_eq!(state.ring.map(|r| (r.tracks, r.scenes)), Some((8, 2)));
    assert_eq!(state.mode.as_deref(), Some("Test"));
    assert_eq!(report[1].run, RunState::Empty);
    assert_eq!(
        (report[0].input_present, report[0].output_present),
        (Some(true), Some(true))
    );
}

// Spec: control-surfaces R2. A row from the preferences keeps a surface this
// build doesn't offer, and reads stopped, so the next save doesn't lose it.
#[test]
fn a_row_keeps_a_surface_that_is_not_offered() {
    let mut rig = Rig::with_ports();
    let config = RowConfig {
        surface: Some("elsewhere".to_owned()),
        input: Some("Pads".to_owned()),
        output: None,
    };
    assert!(rig.surfaces.set_row(0, config.clone()).is_err());
    rig.surfaces.restore(0, config.clone()).unwrap();
    rig.sync();
    assert_eq!(rig.surfaces.rows()[0], config);
    let report = &rig.surfaces.report(&rig.ports)[0];
    assert_eq!(report.run, RunState::Stopped);
    assert!(report.error.as_deref().unwrap().contains("elsewhere"));
}

// Spec: control-surfaces R38. The instant test surface turns each message
// into the control it is, in order, and the host hands the shell what moved
// with its row; a message that is no control asks for nothing.
#[test]
fn the_instant_test_surface_turns_messages_into_controls_in_order() {
    use bl_control::testing::{RELATIVE, role_of};
    use bl_control::{Ask, Control, Hit, RingStep, Role};
    use bl_core::midi::MidiMessage;
    use bl_model::RemoteMode;

    let mut rig = Rig::with_ports();
    rig.set(2, Some("test-instant"), "Pads", "Pads Out")
        .unwrap();
    let now = TestClock::new().now();
    for bytes in [
        [0xB0, 3, 100],
        [0xB0, 8, 1],
        [0x90, 1, 90],
        [0x80, 1, 0],
        [0x90, 3, 90],
        [0xB0, 100, 1],
    ] {
        rig.send("Pads", now, &bytes);
    }
    rig.surfaces.settle();
    let mut requests = Vec::new();
    rig.surfaces.take_requests(&mut requests);
    let controls: Vec<(usize, Control)> = requests
        .iter()
        .map(|r| match &r.ask {
            Ask::Control(control) => (r.row, *control),
            Ask::Command(_) | Ask::Rest(_) => panic!("a control, not a command"),
        })
        .collect();
    let value = |value| Hit::Value { value, full: 127 };
    assert_eq!(
        controls,
        [
            (
                2,
                Control {
                    role: Role::Device(3),
                    mode: RemoteMode::Absolute,
                    hit: value(100)
                }
            ),
            (
                2,
                Control {
                    role: Role::Device(0),
                    mode: RELATIVE,
                    hit: value(1)
                }
            ),
            (
                2,
                Control {
                    role: Role::NextBank,
                    mode: RemoteMode::Absolute,
                    hit: Hit::NoteOn {
                        key: 1,
                        velocity: 90
                    }
                }
            ),
            (
                2,
                Control {
                    role: Role::NextBank,
                    mode: RemoteMode::Absolute,
                    hit: Hit::NoteOff { key: 1 }
                }
            ),
            (
                2,
                Control {
                    role: Role::Ring(RingStep::Right),
                    mode: RemoteMode::Absolute,
                    hit: Hit::NoteOn {
                        key: 3,
                        velocity: 90
                    }
                }
            ),
        ],
        "the controller with no role asks for nothing"
    );
    assert!(role_of(&MidiMessage::from_bytes(&[0xB0, 100, 1]).unwrap()).is_none());
}

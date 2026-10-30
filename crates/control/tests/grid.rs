//! The grid surface's recognition and its simulated devices
//! (`docs/specs/supported-controllers.md`, R2, R5 and R72): a controller
//! whose ports appear is asked for its identity and fills the first free row;
//! the simulated Launchpads apply what the surface documents to a model of
//! their lights and faders.

use bl_control::grid::sim::Launchpad;
use bl_control::grid::{Layer, Light};
use bl_control::{Recognise, RowConfig, RunState, Surfaces, kind};
use bl_midi_io::{Direction, MidiBackend, MidiPorts, OutputBus, TestBackend, input_queue};

struct Rig {
    test: TestBackend,
    ports: MidiPorts,
    surfaces: Surfaces,
}

impl Rig {
    fn new() -> Self {
        let test = TestBackend::new();
        let (sender, _engine) = input_queue(1_024);
        let bus = OutputBus::new();
        let ports = MidiPorts::new(Box::new(test.clone()), sender.clone()).with_output(bus.clone());
        Self {
            test,
            ports,
            surfaces: Surfaces::new(sender, bus),
        }
    }

    /// Lists the ports and follows the rows, as the shell does after a
    /// change, with the simulated controllers answering in between.
    fn sync(&mut self, pads: &mut [&mut Launchpad]) {
        for _ in 0..3 {
            self.ports.refresh().unwrap();
            self.surfaces.reconcile(&mut self.ports);
            self.surfaces.settle();
            for pad in pads.iter_mut() {
                pad.pump();
            }
        }
    }

    fn surface(&self, row: usize) -> Option<String> {
        self.surfaces.rows()[row].surface.clone()
    }
}

// Spec: supported-controllers R72. A surface's ports are matched as text
// their name contains, in any case, so the same controller is found under
// each platform's naming.
#[test]
fn ports_are_matched_by_the_text_their_names_contain_in_any_case() {
    let x = kind("launchpad-x").unwrap().recognise.unwrap();
    for name in [
        "LPX DAW",
        "lpx daw",
        "MIDIIN2 (LPX DAW)",
        "Launchpad X lpx daw 2",
    ] {
        assert!(x.names(name), "{name}");
    }
    for name in ["LPX MIDI", "LPMiniMK3 DAW", "Pads"] {
        assert!(!x.names(name), "{name}");
    }
    let mini = kind("launchpad-mini-mk3").unwrap().recognise.unwrap();
    assert!(mini.names("LPMiniMK3 DAW") && !mini.names("LPMiniMK3 MIDI"));
    let pro: Recognise = kind("launchpad-pro-mk3").unwrap().recognise.unwrap();
    assert!(pro.names("LPProMK3 DAW") && !pro.names("LPProMK3 DIN") && !pro.names("LPProMK3 MIDI"));
    // The reply names the maker.
    let reply = [
        0xF0, 0x7E, 0, 0x06, 0x02, 0x00, 0x20, 0x29, 0x13, 0x01, 0, 0, 1, 2, 3, 4, 0xF7,
    ];
    assert!(x.replies(&reply));
    let other = [
        0xF0, 0x7E, 0, 0x06, 0x02, 0x00, 0x01, 0x05, 0x13, 0x01, 0xF7,
    ];
    assert!(!x.replies(&other));
}

// Spec: supported-controllers R2, R3. Ports that appear are asked for their
// identity and, when the reply names the maker, fill the first free row; a
// second controller takes the next; the row's surface runs on the DAW ports
// and the controller's MIDI port is left alone.
#[test]
fn a_controller_that_answers_the_inquiry_fills_the_first_free_row() {
    let mut rig = Rig::new();
    let mut x = Launchpad::x(&rig.test);
    let mut pro = Launchpad::pro(&rig.test);
    x.plug_in();
    rig.sync(&mut [&mut x]);
    assert_eq!(rig.surface(0).as_deref(), Some("launchpad-x"));
    assert_eq!(rig.surfaces.report(&rig.ports)[0].run, RunState::Running);
    assert_eq!(rig.surface(1), None);
    assert_eq!(
        rig.surfaces.rows()[0].input.as_deref(),
        Some("LPX DAW"),
        "the DAW port"
    );
    assert!(x.daw_mode());
    // The MIDI port isn't a row's: nothing holds it.
    assert!(
        rig.surfaces
            .rows()
            .iter()
            .all(|r| r.input.as_deref() != Some("LPX MIDI"))
    );

    pro.plug_in();
    rig.sync(&mut [&mut x, &mut pro]);
    assert_eq!(rig.surface(1).as_deref(), Some("launchpad-pro-mk3"));
    assert!(pro.daw_mode());
    // The DIN port is left alone too.
    assert!(
        rig.surfaces
            .rows()
            .iter()
            .all(|r| r.input.as_deref() != Some("LPProMK3 DIN"))
    );
    assert!(rig.surfaces.filled());
    assert!(!rig.surfaces.filled(), "asked once");
}

// Spec: supported-controllers R2. A row a person set to None isn't filled
// again for those ports while Beatlayer runs; a full table fills nothing.
#[test]
fn a_row_a_person_emptied_is_not_filled_again() {
    let mut rig = Rig::new();
    let mut x = Launchpad::x(&rig.test);
    x.plug_in();
    rig.sync(&mut [&mut x]);
    let config = rig.surfaces.rows()[0].clone();
    assert_eq!(config.surface.as_deref(), Some("launchpad-x"));
    rig.surfaces
        .set_row(
            0,
            RowConfig {
                surface: None,
                ..config
            },
        )
        .unwrap();
    rig.surfaces.set_row(0, RowConfig::default()).unwrap();
    rig.sync(&mut [&mut x]);
    assert_eq!(rig.surface(0), None, "it stays off");
    assert!(!x.daw_mode(), "the controller was sent back to Standalone");
    assert_eq!(x.lit_when_left(), Some(0));

    // Another controller of the same kind is another set of ports.
    let mut full = Rig::new();
    for row in 0..6 {
        full.surfaces
            .set_row(
                row,
                RowConfig {
                    surface: Some("test".to_owned()),
                    input: Some(format!("In {row}")),
                    output: Some(format!("Out {row}")),
                },
            )
            .unwrap();
    }
    let mut mini = Launchpad::mini(&full.test);
    mini.plug_in();
    full.sync(&mut [&mut mini]);
    assert!(!full.surfaces.probing());
    assert!(!mini.daw_mode());
}

fn open_output(test: &TestBackend, name: &str) -> Box<dyn bl_midi_io::OutputConnection> {
    test.clone().open_output(name).unwrap()
}

// Spec: supported-controllers R5. A simulated Launchpad has ports with its
// controller's names, answers the device inquiry, applies the documented
// light, fader and layout messages to a model, and sends a press when a
// test presses.
#[test]
fn a_simulated_launchpad_applies_the_documented_messages() {
    let test = TestBackend::new();
    let mut pad = Launchpad::x(&test);
    pad.plug_in();
    assert!(!test.is_open(Direction::Input, "LPX DAW"));
    let mut out = open_output(&test, "LPX DAW");
    let mut send = |bytes: &[u8]| out.send(bytes).unwrap();

    // DAW mode, and the lights on channels 1 to 3.
    send(&[0xF0, 0x00, 0x20, 0x29, 0x02, 0x0C, 0x10, 0x01, 0xF7]);
    send(&[0x90, 11, 5]); // static red, the lower left pad
    send(&[0x91, 12, 19]); // flashing green over static
    send(&[0x90, 12, 13]);
    send(&[0x91, 12, 19]);
    send(&[0x92, 13, 45]); // pulsing blue
    send(&[0xB0, 91, 3]); // a button
    send(&[0x98, 11, 13]); // a drum pad
    pad.pump();
    assert!(pad.daw_mode());
    assert_eq!(pad.pad(0, 7), Some(Light::Static(5)));
    assert_eq!(pad.pad(1, 7), Some(Light::Flash { a: 13, b: 19 }));
    assert_eq!(pad.pad(2, 7), Some(Light::Pulse(45)));
    assert_eq!(pad.button(91), Some(Light::Static(3)));
    assert_eq!(pad.light(Layer::Drum, 11), Some(Light::Static(13)));
    // Static again ends the flashing and pulsing; off puts a pad out.
    send(&[0x90, 12, 21]);
    send(&[0x90, 13, 0]);
    pad.pump();
    assert_eq!(pad.pad(1, 7), Some(Light::Static(21)));
    assert_eq!(pad.pad(2, 7), None);

    // A fader bank, its colours and positions.
    send(&[
        0xF0, 0x00, 0x20, 0x29, 0x02, 0x0C, 0x01, 0x00, 0x01, 0, 0, 21, 21, 1, 1, 22, 49, 0xF7,
    ]);
    send(&[0xB4, 21, 100]);
    send(&[0xF0, 0x00, 0x20, 0x29, 0x02, 0x0C, 0x00, 0x0D, 0xF7]);
    pad.pump();
    let bank = pad.bank(0);
    assert!(bank.horizontal);
    assert!(bank.faders[0].shown && bank.faders[1].bipolar);
    assert_eq!((bank.faders[0].position, bank.faders[1].colour), (100, 49));
    assert_eq!(pad.layout(), (0x0D, 0));

    // The inquiry is answered on the input.
    let (tx, rx) = std::sync::mpsc::channel();
    let _input = test
        .clone()
        .open_input(
            "LPX DAW",
            Box::new(move |_, bytes| tx.send(bytes.to_vec()).unwrap()),
        )
        .unwrap();
    send(&[0xF0, 0x7E, 0x7F, 0x06, 0x01, 0xF7]);
    pad.pump();
    let reply = rx.recv().unwrap();
    assert_eq!(
        &reply[..9],
        [0xF0, 0x7E, 0x00, 0x06, 0x02, 0x00, 0x20, 0x29, 0x13]
    );

    // A press arrives as the controller sends it.
    pad.pad_down(0, 0);
    assert_eq!(rx.recv().unwrap(), [0x90, 81, 100]);
    pad.button_down(95);
    assert_eq!(rx.recv().unwrap(), [0xB0, 95, 127]);

    // Standalone clears the DAW's state.
    send(&[0xF0, 0x00, 0x20, 0x29, 0x02, 0x0C, 0x10, 0x00, 0xF7]);
    pad.pump();
    assert!(!pad.daw_mode());
    assert!(pad.all_dark());
    assert_eq!(pad.lit_when_left(), Some(4), "what was lit when it left");
}

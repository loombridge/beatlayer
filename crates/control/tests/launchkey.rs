//! The Launchkey surfaces' recognition and their simulated devices
//! (`docs/specs/supported-controllers.md`, R2, R5 and R72): a controller
//! whose ports appear is asked for its identity and fills the first free row,
//! and the simulated Launchkeys apply what the surface documents to a model
//! of their lights, display and modes.

use bl_control::grid::Light;
use bl_control::launchkey::Layer;
use bl_control::launchkey::sim::{Launchkey, Size};
use bl_control::{Recognise, RunState, Surfaces, kind};
use bl_midi_io::{MidiBackend, MidiPorts, OutputBus, TestBackend, input_queue};

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
    fn sync(&mut self, keys: &mut [&mut Launchkey]) {
        for _ in 0..3 {
            self.ports.refresh().unwrap();
            self.surfaces.reconcile(&mut self.ports);
            self.surfaces.settle();
            for key in keys.iter_mut() {
                key.pump();
            }
        }
    }

    fn surface(&self, row: usize) -> Option<String> {
        self.surfaces.rows()[row].surface.clone()
    }
}

fn recognise(id: &str) -> Recognise {
    kind(id).unwrap().recognise.unwrap()
}

// Spec: supported-controllers R72. A surface's ports are matched as text
// their name contains, in any case, so the same controller is found under
// each platform's naming, and an MK3's ports aren't an MK4's.
#[test]
fn ports_are_matched_by_the_text_their_names_contain_in_any_case() {
    let mk3 = recognise("launchkey-mk3");
    for name in [
        "Launchkey MK3 61 LKMK3 DAW",
        "launchkey mk3 49 lkmk3 daw",
        "MIDIIN2 (LKMK3 DAW)",
        "Launchkey MK3 88 DAW Port",
    ] {
        assert!(mk3.names(name), "{name}");
    }
    for name in [
        "Launchkey MK3 61 LKMK3 MIDI",
        "Launchkey MK4 61 DAW",
        "Pads",
    ] {
        assert!(!mk3.names(name), "{name}");
    }
    let mk4 = recognise("launchkey-mk4");
    for name in [
        "Launchkey MK4 61 DAW",
        "LAUNCHKEY MK4 49 DAW PORT",
        "Launchkey Mini MK4 25 DAW",
    ] {
        assert!(mk4.names(name), "{name}");
    }
    for name in [
        "Launchkey MK4 61 MIDI",
        "Launchkey MK3 61 LKMK3 DAW",
        "LPX DAW",
    ] {
        assert!(!mk4.names(name), "{name}");
    }
    // The reply names the maker.
    let reply = [
        0xF0, 0x7E, 0, 0x06, 0x02, 0x00, 0x20, 0x29, 0x36, 0x01, 0, 0, 1, 2, 3, 4, 0xF7,
    ];
    assert!(mk3.replies(&reply) && mk4.replies(&reply));
    let other = [
        0xF0, 0x7E, 0, 0x06, 0x02, 0x00, 0x01, 0x05, 0x13, 0x01, 0xF7,
    ];
    assert!(!mk3.replies(&other));
}

// Spec: supported-controllers R2, R3. A Launchkey whose ports appear is asked
// for its identity and fills the first free row with its own surface on its
// DAW ports, an MK3 and an MK4 each its own; the MIDI ports stay unclaimed.
#[test]
fn a_launchkey_that_answers_the_inquiry_fills_the_first_free_row() {
    let mut rig = Rig::new();
    let mut mk3 = Launchkey::mk3(&rig.test, Size::K61);
    let mut mk4 = Launchkey::mk4(&rig.test, Size::Mini);
    mk3.plug_in();
    rig.sync(&mut [&mut mk3]);
    assert_eq!(rig.surface(0).as_deref(), Some("launchkey-mk3"));
    assert_eq!(rig.surfaces.report(&rig.ports)[0].run, RunState::Running);
    assert_eq!(
        rig.surfaces.rows()[0].input.as_deref(),
        Some("Launchkey MK3 61 LKMK3 DAW")
    );
    assert!(mk3.daw_mode_on());
    assert_eq!(rig.surface(1), None);

    mk4.plug_in();
    rig.sync(&mut [&mut mk3, &mut mk4]);
    assert_eq!(rig.surface(1).as_deref(), Some("launchkey-mk4"));
    assert!(mk4.daw_mode_on());
    assert!(
        rig.surfaces.rows().iter().all(|r| {
            let input = r.input.as_deref().unwrap_or_default();
            !input.contains("MIDI")
        }),
        "the MIDI ports stay ordinary ports"
    );
    assert!(rig.surfaces.filled());
}

fn open_output(test: &TestBackend, name: &str) -> Box<dyn bl_midi_io::OutputConnection> {
    test.clone().open_output(name).unwrap()
}

// Spec: supported-controllers R5. A simulated Launchkey has ports with its
// controller's names, answers the device inquiry, applies the documented
// light, mode, display and scale messages to a model, and sends a press when
// a test presses.
#[test]
fn a_simulated_launchkey_applies_the_documented_messages() {
    let test = TestBackend::new();
    let mut keys = Launchkey::mk3(&test, Size::K61);
    keys.plug_in();
    let daw = keys.daw_port();
    let mut out = open_output(&test, &daw);
    let mut send = |bytes: &[u8]| out.send(bytes).unwrap();

    // DAW mode, the modes, and lights on channels 1 to 3 and 10.
    send(&[0x9F, 0x0C, 0x7F]);
    send(&[0xBF, 0x03, 0x02]);
    send(&[0xBF, 0x09, 0x01]);
    send(&[0x90, 0x70, 5]); // static red, the lower left pad
    send(&[0x91, 0x71, 19]); // flashing green over static
    send(&[0x90, 0x71, 13]);
    send(&[0x91, 0x71, 19]);
    send(&[0x92, 0x72, 45]); // pulsing blue
    send(&[0xB0, 0x69, 3]); // a button
    send(&[0x99, 36, 13]); // a drum pad
    keys.pump();
    assert!(keys.daw_mode_on());
    assert_eq!(keys.modes(), [2, 1, 0]);
    assert_eq!(keys.pad(0, 1), Some(Light::Static(5)));
    assert_eq!(keys.pad(1, 1), Some(Light::Flash { a: 13, b: 19 }));
    assert_eq!(keys.pad(2, 1), Some(Light::Pulse(45)));
    assert_eq!(keys.button(0x69), Some(Light::Static(3)));
    assert_eq!(keys.light(Layer::Drum, 36), Some(Light::Static(13)));
    send(&[0x90, 0x71, 21]);
    send(&[0x90, 0x72, 0]);
    keys.pump();
    assert_eq!(keys.pad(1, 1), Some(Light::Static(21)));
    assert_eq!(keys.pad(2, 1), None);

    // The display: two default rows, a name and a value.
    let mut sysex = |body: &[u8]| {
        let mut bytes = vec![0xF0, 0x00, 0x20, 0x29, 0x02, 0x0F];
        bytes.extend_from_slice(body);
        bytes.push(0xF7);
        out.send(&bytes).unwrap();
    };
    sysex(&[0x04, 0x00, b'U', b'p']);
    sysex(&[0x07, 0x38, b'G', b'a', b'i', b'n']);
    sysex(&[0x08, 0x38, b'-', b'6']);
    keys.pump();
    assert_eq!(keys.row(0), "Up");
    assert_eq!((keys.name(0x38), keys.value(0x38)), ("Gain", "-6"));

    // The inquiry is answered on the input, and a press arrives as the
    // controller sends it.
    let (tx, rx) = std::sync::mpsc::channel();
    let _input = test
        .clone()
        .open_input(
            &daw,
            Box::new(move |_, bytes| tx.send(bytes.to_vec()).unwrap()),
        )
        .unwrap();
    let mut out = open_output(&test, &daw);
    out.send(&[0xF0, 0x7E, 0x7F, 0x06, 0x01, 0xF7]).unwrap();
    keys.pump();
    let reply = rx.recv().unwrap();
    assert_eq!(
        &reply[..9],
        [0xF0, 0x7E, 0x00, 0x06, 0x02, 0x00, 0x20, 0x29, 0x36]
    );
    keys.pad_down(0, 0);
    assert_eq!(rx.recv().unwrap(), [0x90, 0x60, 100]);
    keys.button_down(0x69);
    assert_eq!(rx.recv().unwrap(), [0xB0, 0x69, 127], "on channel 1");
    keys.button_down(0x73);
    assert_eq!(rx.recv().unwrap(), [0xBF, 0x73, 127], "on channel 16");
    keys.pot_to(2, 99);
    assert_eq!(rx.recv().unwrap(), [0xBF, 0x17, 99]);
    keys.fader_to(1, 8);
    assert_eq!(rx.recv().unwrap(), [0xBF, 0x36, 8]);

    // Leaving DAW mode clears the DAW's state.
    out.send(&[0x9F, 0x0C, 0x00]).unwrap();
    keys.pump();
    assert!(!keys.daw_mode_on());
    assert_eq!(keys.lit(), 0);
    assert_eq!(keys.lit_when_left(), Some(4), "what was lit when it left");
    assert_eq!(keys.row(0), "", "the display is given back");

    // An MK4 takes its feature controls and screen messages.
    let mut mk4 = Launchkey::mk4(&test, Size::K49);
    mk4.plug_in();
    let daw = mk4.daw_port();
    let mut out = open_output(&test, &daw);
    for bytes in [
        &[0xB6, 0x54, 0x01][..],
        &[0xB6, 0x4A, 0x01],
        &[0xB6, 0x3D, 0x05],
        &[0xB6, 0x3E, 0x03],
        &[0xF0, 0x00, 0x20, 0x29, 0x02, 0x14, 0x04, 0x15, 0x62, 0xF7],
        &[
            0xF0, 0x00, 0x20, 0x29, 0x02, 0x14, 0x06, 0x15, 0x01, b'W', b'i', b'd', b'e', 0xF7,
        ],
    ] {
        out.send(bytes).unwrap();
    }
    mk4.pump();
    assert!(mk4.drum_claimed());
    assert_eq!(mk4.scale(), (true, 5, 3));
    let screen = mk4.screen(0x15);
    assert_eq!(screen.config, 0x62);
    assert_eq!(screen.fields, ["", "Wide"]);
}

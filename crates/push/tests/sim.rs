//! The simulated Push 2 itself (`docs/specs/push-2.md`, R36): it answers as
//! the interface manual's examples do, models every LED, and decodes frames.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use bl_core::clock::TestClock;
use bl_midi_io::{MidiBackend, TestBackend};
use bl_push::frame;
use bl_push::sim::{LIVE, Simulated, USER};
use bl_push::sysex::{self, MidiMode};

fn device() -> (Simulated, TestBackend) {
    let backend = TestBackend::new();
    let mut sim = Simulated::new(&backend, &TestClock::new());
    sim.plug_in();
    (sim, backend)
}

/// A system exclusive message as the manual writes its examples.
fn message(bytes: &[u8]) -> Vec<u8> {
    let mut message = vec![0xF0, 0x00, 0x21, 0x1D, 0x01, 0x01];
    message.extend_from_slice(bytes);
    message.push(0xF7);
    message
}

// Spec: push-2 R36. Each Get command is answered as the interface manual's
// examples say, from what the device holds.
#[test]
fn the_responder_answers_as_the_interface_manuals_examples_do() {
    let (mut sim, _) = device();
    sim.handle(&sysex::DEVICE_INQUIRY);
    assert_eq!(
        sysex::parse(&sim.replies[0]),
        sysex::Reply::Identity {
            firmware: "1.0.47".to_owned()
        }
    );
    // Get LED Color Palette Entry 125: 0/0/255 and 126.
    sim.handle(&message(&[0x04, 0x7D]));
    assert_eq!(
        sim.replies[1],
        message(&[0x04, 0x7D, 0x00, 0x00, 0x00, 0x00, 0x7F, 0x01, 0x7E, 0x00])
    );
    // Request Statistics: the external power supply, run ID 0, uptime 959 s.
    sim.handle(&message(&[0x1A, 0x00]));
    assert_eq!(
        sim.replies[2],
        message(&[0x1A, 0x01, 0x00, 0x3F, 0x07, 0x00, 0x00, 0x00])
    );
    // What was set reads back: brightness, the strip, the curve, aftertouch.
    sim.handle(&message(&[0x06, 0x10]));
    sim.handle(&message(&[0x07]));
    assert_eq!(sim.replies[3], message(&[0x07, 0x10]));
    sim.handle(&message(&[0x17, 0x25]));
    sim.handle(&message(&[0x18]));
    assert_eq!(sim.replies[4], message(&[0x18, 0x25]));
    sim.handle(&message(&[
        0x20, 0x10, 0x3E, 0x41, 0x44, 0x46, 0x49, 0x4B, 0x4D, 0x50, 0x52, 0x54, 0x56, 0x59, 0x5B,
        0x5D, 0x5F, 0x62,
    ]));
    sim.handle(&message(&[0x21, 0x11]));
    assert_eq!(sim.replies[5], message(&[0x21, 0x11, 0x41]));
    sim.handle(&message(&[0x1E, 0x01]));
    sim.handle(&message(&[0x1F]));
    assert_eq!(sim.replies[6], message(&[0x1F, 0x01]));
    sim.handle(&message(&[0x08, 0x40, 0x00]));
    sim.handle(&message(&[0x09]));
    assert_eq!(sim.replies[7], message(&[0x09, 0x40, 0x00]));
}

// Spec: push-2 R36. Set MIDI Mode is answered on both ports.
#[test]
fn set_midi_mode_is_answered_on_both_ports() {
    let (mut sim, backend) = device();
    let heard = Arc::new(Mutex::new(Vec::new()));
    let mut open = Vec::new();
    for port in [LIVE, USER] {
        let heard = Arc::clone(&heard);
        open.push(
            backend
                .clone()
                .open_input(
                    port,
                    Box::new(move |_: Instant, bytes: &[u8]| {
                        heard.lock().unwrap().push((port, bytes.to_vec()));
                    }),
                )
                .unwrap(),
        );
    }
    sim.handle(&sysex::midi_mode(MidiMode::User));
    assert_eq!(sim.device.mode, MidiMode::User);
    let heard = heard.lock().unwrap().clone();
    assert_eq!(
        heard,
        [
            (LIVE, sysex::midi_mode(MidiMode::User)),
            (USER, sysex::midi_mode(MidiMode::User))
        ]
    );
}

// Spec: push-2 R36. The LED model applies each note-on, controller and
// animation message, and ignores the Live port's LEDs in User mode except
// User's own.
#[test]
fn the_led_model_applies_colours_and_animations() {
    let (mut sim, backend) = device();
    let out = &mut backend.clone().open_output(LIVE).unwrap();
    let mut send = |bytes: &[u8]| out.send(bytes).unwrap();
    send(&[0x90, 99, 127]); // the top right pad red
    send(&[0x90, 36, 126]); // the bottom left pad green
    send(&[0xB0, 60, 0]);
    send(&[0xBF, 119, 127]); // Undo blinks slowly: from 0 to 127 over a half note
    send(&[0xB1, 60, 125]); // Mute: a fast transition to blue
    sim.pump();
    assert_eq!(sim.led("pad_7_7").unwrap().colour, 127);
    assert_eq!(sim.led("pad_0_0").unwrap().colour, 126);
    assert_eq!(sim.led("mute").unwrap().animation, Some((125, 1)));
    assert_eq!(sim.led("undo").unwrap().animation, Some((127, 15)));
    send(&[0xB0, 60, 5]);
    sim.pump();
    assert_eq!(
        sim.led("mute").unwrap(),
        bl_push::leds::Led::steady(5),
        "channel 0 stops the animation"
    );

    sim.handle(&sysex::midi_mode(MidiMode::User));
    let out = &mut backend.clone().open_output(LIVE).unwrap();
    out.send(&[0xB0, 61, 99]).unwrap();
    out.send(&[0xB0, 59, 127]).unwrap();
    sim.pump();
    assert_eq!(
        sim.led("solo").unwrap().colour,
        0,
        "the Live port's LEDs are ignored in User mode"
    );
    assert_eq!(sim.led("user").unwrap().colour, 127, "except User's");
}

// Spec: push-2 R36. The frame sink undoes the XOR and the pixel format and keeps
// each frame as RGB; a bad frame is counted.
#[test]
fn the_frame_sink_decodes_frames() {
    let (sim, _) = device();
    let claim = sim.frames().claim();
    let mut sink = claim().unwrap();
    let mut rgb = vec![0u8; frame::RGB_BYTES];
    rgb[..3].copy_from_slice(&[248, 252, 248]);
    sink.send(&frame::encode(&rgb)).unwrap();
    sink.send(&[1, 2, 3]).unwrap();
    assert_eq!(sim.frames().count(), 1);
    assert_eq!(sim.frames().bad(), 1);
    assert_eq!(
        &sim.frames().latest().unwrap()[..3],
        &[255, 255, 255],
        "5 and 6 bits, spread to 8"
    );
    sim.frames().fail_claims(1);
    assert!(claim().is_err());
    assert!(claim().is_ok());
}

// Spec: push-2 R36. A test presses pads, turns encoders, moves the strip and
// presses buttons on the device's ports, and they reach what listens.
#[test]
fn the_controls_send_on_the_ports_the_modes_say() {
    let (mut sim, backend) = device();
    let heard = Arc::new(Mutex::new(Vec::new()));
    let mut open = Vec::new();
    for port in [LIVE, USER] {
        let heard = Arc::clone(&heard);
        open.push(
            backend
                .clone()
                .open_input(
                    port,
                    Box::new(move |_: Instant, bytes: &[u8]| {
                        heard.lock().unwrap().push((port, bytes.to_vec()));
                    }),
                )
                .unwrap(),
        );
    }
    assert!(sim.press_pad(0, 0, 100));
    assert!(sim.turn(0, -3));
    assert!(sim.turn(controls_tempo(), 5));
    assert!(sim.strip_move(0.5));
    assert!(sim.press("user"));
    sim.device.mode = MidiMode::User;
    assert!(sim.press("play"));
    let heard = heard.lock().unwrap().clone();
    assert_eq!(heard[0], (LIVE, vec![0x90, 36, 100]));
    assert_eq!(
        heard[1],
        (LIVE, vec![0xB0, 71, 125]),
        "-3 in two's complement"
    );
    assert_eq!(heard[2], (LIVE, vec![0xB0, 14, 5]));
    assert_eq!(heard[3].1[0], 0xE0, "pitch bend by default");
    assert_eq!(
        heard[4],
        (LIVE, vec![0xB0, 59, 127]),
        "User is sent on both ports"
    );
    assert_eq!(heard[5], (USER, vec![0xB0, 59, 127]));
    assert_eq!(
        heard[6],
        (USER, vec![0xB0, 85, 127]),
        "in User mode the rest goes to the User port"
    );
}

const fn controls_tempo() -> u8 {
    bl_push::controls::TEMPO
}

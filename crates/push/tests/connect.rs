//! Connecting (`docs/specs/push-2.md`, R1 to R5): the sequence at start and
//! stop, the replies it waits for, a device that isn't a Push 2, and the
//! Track switch not reaching tracks.

use bl_control::RunState;
use bl_midi_io::{Direction, PortSwitches};
use bl_push::sim::LIVE;
use bl_push::sysex::{self, DEVICE_INQUIRY, MidiMode};

use crate::rig::Rig;

/// The sysex R4 sends at the start, in order, with the palette entries the
/// device was given.
fn start_sequence(rig: &Rig) -> Vec<Vec<u8>> {
    let setup = rig.shared.setup();
    let mut expected = vec![
        DEVICE_INQUIRY.to_vec(),
        sysex::midi_mode(MidiMode::Live),
        sysex::request_statistics(),
    ];
    // The palette entries, from the first index the allocator gives out.
    let sent = rig.sim.sysex();
    let first = expected.len();
    let reapply = sent
        .iter()
        .position(|m| *m == sysex::reapply_palette())
        .expect("Reapply Color Palette is sent");
    expected.extend(sent[first..reapply].iter().cloned());
    expected.push(sysex::reapply_palette());
    expected.push(sysex::led_brightness(setup.led_value()));
    expected.push(sysex::display_brightness(setup.display_value()));
    expected.extend(bl_push::curve::messages(&setup.curve()));
    expected.push(sysex::pad_parameters(1220, 1570));
    expected.push(sysex::aftertouch_mode(false));
    expected.push(sysex::touch_strip_config(0x68));
    expected
}

// Spec: push-2 R1, R4. Starting the surface sends, in order, a Device
// Inquiry, Set MIDI Mode with Live, Request Statistics, the palette entries
// and Reapply, the two brightnesses, the velocity curve in eight messages and
// the pad parameters, the aftertouch mode, the touch strip's configuration,
// and then every LED; it reads the firmware and the power supply.
#[test]
fn starting_sends_exactly_r4s_sequence_and_reads_the_replies() {
    let mut rig = Rig::new();
    rig.start();
    assert_eq!(rig.run(), RunState::Running);
    let sysex = rig.sim.sysex();
    assert_eq!(sysex, start_sequence(&rig));
    // 14 colours were written, none of them over index 0, 16 or 122 to 127.
    let entries: Vec<&Vec<u8>> = sysex
        .iter()
        .filter(|m| m.get(6) == Some(&sysex::id::SET_PALETTE_ENTRY))
        .collect();
    assert_eq!(entries.len(), 14);
    for entry in entries {
        assert!(
            entry[7] != 0 && entry[7] != 16 && entry[7] < 122,
            "index {}",
            entry[7]
        );
    }
    // Every LED once, after the last sysex.
    let sent = rig.sim.sent();
    let last_sysex = sent.iter().rposition(|m| sysex::is_sysex(m)).unwrap();
    assert_eq!(
        sent.len() - last_sysex - 1,
        64 + bl_push::controls::BUTTONS.len()
    );
    // What the device was told took effect.
    assert_eq!(rig.sim.device.mode, MidiMode::Live);
    assert_eq!(rig.sim.device.curve, rig.shared.setup().curve());
    assert_eq!(rig.sim.device.thresholds, (1220, 1570));
    assert_eq!(rig.sim.device.led_brightness, 127);
    assert_eq!(rig.sim.device.display_brightness, 255);
    // And the surface read the replies.
    let state = rig.snapshot();
    assert_eq!(state.firmware.as_deref(), Some("1.0.47"));
    assert_eq!(state.external_power, Some(true));
    assert_eq!(state.error, None);
}

// Spec: push-2 R4, R9 of control surfaces. Stopping turns every LED off, sets
// the display's brightness to 0 and leaves Push 2 in Live mode.
#[test]
fn stopping_puts_every_led_out_darkens_the_display_and_leaves_live_mode() {
    let mut rig = Rig::new();
    rig.flags.lock().unwrap().light_all = true;
    rig.start();
    rig.tick();
    assert!(rig.sim.led("play").unwrap().colour != 0);
    rig.sim.forget_sent();
    rig.surfaces
        .set_row(0, bl_control::RowConfig::default())
        .unwrap();
    rig.sync();
    assert_eq!(rig.run(), RunState::Empty);
    let sent = rig.sim.sent().to_vec();
    let leds = 64 + bl_push::controls::BUTTONS.len();
    assert_eq!(sent.len(), leds + 2);
    assert!(
        sent[..leds].iter().all(|m| m[2] == 0 && m[0] & 0x0F == 0),
        "every LED, off, on channel 0"
    );
    assert_eq!(sent[leds], sysex::display_brightness(0));
    assert_eq!(sent[leds + 1], sysex::midi_mode(MidiMode::Live));
    assert_eq!(rig.sim.device.display_brightness, 0);
    for name in ["play", "user", "pad_0_0", "pad_7_7", "lower_3"] {
        assert_eq!(rig.sim.led(name).unwrap().colour, 0, "{name}");
    }
}

// Spec: push-2 R1. Unplugging stops the surface; plugging it in again starts
// it afresh with the whole sequence and every LED.
#[test]
fn unplugging_stops_and_plugging_in_again_starts_afresh() {
    let mut rig = Rig::new();
    rig.start();
    assert_eq!(rig.run(), RunState::Running);
    rig.sim.unplug();
    rig.sync();
    assert_eq!(rig.run(), RunState::Waiting);
    rig.sim.power_cycle();
    rig.sim.plug_in();
    rig.sync();
    assert_eq!(rig.run(), RunState::Running);
    let sent = rig.sim.sent();
    assert_eq!(
        sent[0..3],
        [
            DEVICE_INQUIRY.to_vec(),
            sysex::midi_mode(MidiMode::Live),
            sysex::request_statistics()
        ]
    );
    let leds = sent.iter().filter(|m| !sysex::is_sysex(m)).count();
    assert_eq!(leds, 64 + bl_push::controls::BUTTONS.len());
}

// Spec: push-2 R4. A reply that doesn't come: the command is sent again after
// 500 ms, and after another 500 ms the surface carries on without it.
#[test]
fn a_reply_that_never_comes_is_waited_for_twice_and_then_given_up() {
    let mut rig = Rig::new();
    rig.sim.silent = true;
    rig.start();
    assert_eq!(rig.sim.sysex(), [DEVICE_INQUIRY.to_vec()]);
    rig.advance(499);
    assert_eq!(rig.sim.sysex().len(), 1, "still waiting at 499 ms");
    rig.advance(1);
    assert_eq!(
        rig.sim.sysex(),
        [DEVICE_INQUIRY.to_vec(), DEVICE_INQUIRY.to_vec()],
        "sent again at 500 ms"
    );
    rig.advance(500);
    let sysex = rig.sim.sysex();
    assert_eq!(
        sysex[2],
        sysex::midi_mode(MidiMode::Live),
        "carried on after the retry"
    );
    // Each of the three replies is given the same two waits.
    for _ in 0..4 {
        rig.advance(500);
    }
    assert_eq!(
        rig.sim
            .sysex()
            .iter()
            .filter(|m| **m == sysex::request_statistics())
            .count(),
        2
    );
    rig.advance(500);
    assert!(
        rig.sim.sysex().contains(&sysex::touch_strip_config(0x68)),
        "the sequence ran on"
    );
    assert_eq!(rig.snapshot().firmware, None);
}

// Spec: push-2 R1. A device whose Device Inquiry reply isn't Ableton's Push 2
// isn't driven.
#[test]
fn a_device_that_is_not_a_push_2_is_left_alone() {
    let mut rig = Rig::new();
    rig.sim.impostor = true;
    rig.start();
    assert_eq!(rig.sim.sysex(), [DEVICE_INQUIRY.to_vec()]);
    let error = rig.snapshot().error.expect("it says why");
    assert!(error.contains("isn't a Push 2"), "{error}");
    rig.sim.press("play");
    rig.advance(10);
    assert!(rig.commands().is_empty());
    assert_eq!(rig.sim.sysex().len(), 1);
}

// Spec: push-2 R3. The Live port belongs to the surface: a pad's note never
// reaches a track through the port's Track switch.
#[test]
fn a_pads_note_never_reaches_a_track_through_the_ports_track_switch() {
    let mut rig = Rig::new();
    rig.sim.plug_in();
    rig.ports.refresh().unwrap();
    rig.ports
        .set_switches(
            Direction::Input,
            LIVE,
            PortSwitches {
                track: true,
                remote: true,
                ..PortSwitches::OFF
            },
        )
        .unwrap();
    rig.start();
    assert!(rig.sim.press_pad(2, 3, 100));
    assert!(rig.sim.release_pad(2, 3));
    rig.advance(10);
    assert!(rig.reached_tracks().is_empty());
    let heard = rig.heard.all();
    assert!(
        heard.contains(&"Note: pad 2,3 On(100)".to_owned()),
        "{heard:?}"
    );
}

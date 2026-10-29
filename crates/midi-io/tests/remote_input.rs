//! The remote input on the MIDI system's thread (WP-7.1, `docs/specs/remote-mapping.md`,
//! R9 and R44): judging a message against the table of mapped messages
//! allocates nothing, under the allocation trap.

use bl_core::midi::MidiMessage;
use bl_midi_io::{RemoteBits, input_queue};

// Spec: remote-mapping R9, R44
#[test]
fn judging_a_message_against_the_table_allocates_nothing() {
    let (sender, _engine) = input_queue(64);
    let remote = sender.remote();
    let mut bits = RemoteBits::default();
    bits.controller(0, 20);
    bits.note(0, 60);
    bits.pitch_bend(3);
    remote.publish(&bits);
    let messages = [
        MidiMessage::from_bytes(&[0xB0, 20, 64]).unwrap(),
        MidiMessage::from_bytes(&[0x90, 60, 100]).unwrap(),
        MidiMessage::from_bytes(&[0x80, 60, 0]).unwrap(),
        MidiMessage::from_bytes(&[0x90, 61, 100]).unwrap(),
        MidiMessage::from_bytes(&[0xE3, 0, 64]).unwrap(),
        MidiMessage::from_bytes(&[0xC0, 5]).unwrap(),
    ];

    bl_rt::trap::prepare_thread();
    let before = bl_rt::trap::thread_violations();
    let taken = bl_rt::trap::forbid(|| {
        let mut taken = 0;
        for _ in 0..1000 {
            taken += messages.iter().filter(|m| remote.takes(m)).count();
        }
        taken
    });
    assert_eq!(taken, 4000);
    assert_eq!(
        bl_rt::trap::thread_violations(),
        before,
        "{:?}",
        bl_rt::trap::last_violation()
    );
}

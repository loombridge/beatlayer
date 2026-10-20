//! CC Control (WP-5.32; the MIDI CC sender spec): each control sends its
//! controller when its value changes, Send sends them all, a controller
//! passes through, and Learn's helpers name the dial a controller goes to.
//! Every block is a real-time section under the allocation trap.

bl_rt::install_trap!();

mod common;

use bl_devices::midi_cc::{
    CC_CONTROL, CUSTOM_A, MOD_WHEEL, PITCH_BEND, PRESSURE, SEND, cc_control_type, cc_type_for,
    dial_cc, dial_value, next_unassigned,
};
use bl_engine::{NodeEvent, NoteEvent, NoteKind};
use bl_model::ParamId;
use common::chain::{BEAT_FRAMES, Chain, note};
use common::{device, node, notes_through_events, registry, set};

const FRAMES: usize = 64;

fn sent(params: &[(ParamId, f64)], events: &[NodeEvent]) -> Vec<NoteEvent> {
    let mut node = node(&registry(), &device(&cc_control_type(), params), FRAMES);
    notes_through_events(node.as_mut(), &[], events, FRAMES)
}

/// The control changes in `events`.
fn controls(events: &[NoteEvent]) -> Vec<(u32, u8, f32)> {
    events
        .iter()
        .filter_map(|event| match event.kind {
            NoteKind::Control { controller, value } => Some((event.offset, controller, value)),
            _ => None,
        })
        .collect()
}

// Spec: midi-cc-sender R2. A control sends its controller on the frame its
// value changes.
#[test]
fn a_control_sends_on_the_frame_its_value_changes() {
    let events = sent(&[], &[set(5, MOD_WHEEL, 100.0)]);
    let sent = controls(&events);
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].0, 5);
    assert_eq!(sent[0].1, 1, "the mod wheel is CC 1");
    assert!((sent[0].2 - 100.0 / 127.0).abs() < 1e-6);
}

// Spec: midi-cc-sender R3. A Custom dial sends on its own controller once it
// has one, and nothing while it is None.
#[test]
fn a_dial_sends_on_its_own_controller() {
    let nothing = sent(&[], &[set(5, dial_value(0), 40.0)]);
    assert!(controls(&nothing).is_empty(), "no controller yet");
    let events = sent(&[(dial_cc(0), 74.0)], &[set(5, dial_value(0), 40.0)]);
    let sent = controls(&events);
    assert_eq!(sent.len(), 1, "{events:?}");
    assert_eq!(sent[0].1, 74);
    assert!((sent[0].2 - 40.0 / 127.0).abs() < 1e-6);
}

// Spec: midi-cc-sender R4. Pitch Bend and Pressure send their own kinds of
// message.
#[test]
fn pitch_bend_and_pressure_send_their_kinds() {
    let events = sent(&[], &[set(3, PITCH_BEND, 4096.0), set(9, PRESSURE, 64.0)]);
    let bend = events
        .iter()
        .find(|event| matches!(event.kind, NoteKind::PitchBend { .. }))
        .expect("a pitch bend");
    assert_eq!(bend.offset, 3);
    assert!(matches!(bend.kind, NoteKind::PitchBend { value } if (value - 0.5).abs() < 1e-6));
    let pressure = events
        .iter()
        .find(|event| matches!(event.kind, NoteKind::ChannelPressure { .. }))
        .expect("a pressure");
    assert_eq!(pressure.offset, 9);
    assert!(
        matches!(pressure.kind, NoteKind::ChannelPressure { value } if (value - 64.0 / 127.0).abs() < 1e-6)
    );
}

// Spec: midi-cc-sender R5. Send sends every assigned control's value once,
// on the frame it turns on.
#[test]
fn send_sends_every_assigned_control() {
    let events = sent(
        &[(CUSTOM_A, 127.0), (dial_cc(0), 74.0), (dial_value(0), 20.0)],
        &[set(0, SEND, 1.0)],
    );
    let mut sent = controls(&events);
    sent.sort_by_key(|(_, controller, _)| *controller);
    let controllers: Vec<u8> = sent.iter().map(|(_, controller, _)| *controller).collect();
    assert_eq!(controllers, [1, 64, 74], "mod wheel, Custom A and Custom B");
    assert!(sent.iter().all(|(offset, _, _)| *offset == 0));
}

// Spec: midi-cc-sender R6. Learn's helpers name the next dial without a
// controller, and the CC Type a learned controller has.
#[test]
fn learn_names_the_next_free_dial() {
    assert_eq!(next_unassigned(&[]), Some(0));
    assert_eq!(next_unassigned(&[(dial_cc(0), 74.0)]), Some(1));
    let all: Vec<(ParamId, f64)> = (0..12).map(|k| (dial_cc(k), 74.0)).collect();
    assert_eq!(next_unassigned(&all), None);
    assert!((cc_type_for(74) - 74.0).abs() < 1e-12);
}

// Spec: midi-cc-sender R1. The device forwards what it hears unchanged.
#[test]
fn what_it_hears_passes_through_unchanged() {
    let note = NoteEvent {
        offset: 0,
        kind: NoteKind::On,
        channel: 0,
        key: 60,
        velocity: 0.5,
        note_id: None,
    };
    let message = NoteEvent {
        offset: 4,
        kind: NoteKind::Control {
            controller: 7,
            value: 0.5,
        },
        channel: 0,
        key: 0,
        velocity: 0.0,
        note_id: None,
    };
    let mut node = node(&registry(), &device(&cc_control_type(), &[]), FRAMES);
    let out = notes_through_events(node.as_mut(), &[note, message], &[], FRAMES);
    assert_eq!(out, [note, message]);
}

// Spec: midi-cc-sender R8. A dial's controller reaches the next device in
// the chain on its frame, in any block size.
#[test]
fn a_dial_reaches_the_next_device_on_its_frame() {
    for size in [1, 100, 512, 4096] {
        let mut chain = Chain::new(4096);
        let device = chain.effect(CC_CONTROL, &[(dial_cc(0), 74.0)]);
        chain.clip(2, [note(60, 0, 1)]);
        chain.sync();
        chain.play();
        chain.run(BEAT_FRAMES as usize, size);
        chain.set(device, dial_value(0), 100.0);
        chain.run(BEAT_FRAMES as usize, size);
        let heard = chain.heard();
        assert!(
            heard
                .iter()
                .any(|(_, kind, key, _)| *kind == NoteKind::On && *key == 60),
            "the note passed through at block size {size}"
        );
        let control = heard
            .iter()
            .find(|(_, kind, _, _)| matches!(kind, NoteKind::Control { controller: 74, .. }))
            .unwrap_or_else(|| panic!("the controller reached the instrument at size {size}"));
        assert!(control.0 >= BEAT_FRAMES, "after the change, at size {size}");
    }
}

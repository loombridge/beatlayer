//! MIDI Monitor (WP-5.32; the MIDI monitor spec): it passes everything it
//! hears unchanged, its display publishes the keyboard and the chord, and
//! Freeze and Clear act on the display. Every block is a real-time section
//! under the allocation trap.

bl_rt::install_trap!();

mod common;

use bl_devices::midi_monitor::{CLEAR, FREEZE, MIDI_MONITOR, chord, display, note_name};
use bl_engine::{NoteEvent, NoteExpression, NoteKind};
use bl_model::OrdMap;
use common::{device, node, note_on, notes_through, notes_through_events, registry};

const FRAMES: usize = 256;

// Spec: midi-monitor R2. The monitor changes nothing it passes.
#[test]
fn the_monitor_changes_nothing_it_passes() {
    let notes = vec![
        note_on(0, 60, 100.0 / 127.0, None),
        NoteEvent {
            offset: 4,
            kind: NoteKind::Expression {
                expression: NoteExpression::Pressure,
                value: 0.5,
            },
            channel: 0,
            key: 60,
            velocity: 0.0,
            note_id: None,
        },
        NoteEvent {
            offset: 8,
            kind: NoteKind::Control {
                controller: 7,
                value: 0.5,
            },
            channel: 0,
            key: 0,
            velocity: 0.0,
            note_id: None,
        },
    ];
    let mut node = node(
        &registry(),
        &device(&bl_devices::midi_monitor_type(), &[]),
        FRAMES,
    );
    let out = notes_through(node.as_mut(), &notes, FRAMES);
    assert_eq!(out, notes);
    assert_eq!(MIDI_MONITOR, "beatlayer.midi_monitor");
    assert_eq!(CLEAR, bl_model::ParamId(1));
}

// Spec: midi-monitor R1, R3, R5, R6. The display publishes the keyboard, the
// chord and the counts, read headless through its reader; Freeze holds it
// while the notes pass on.
#[test]
fn the_display_publishes_the_keyboard_and_the_chord() {
    let mut node = node(
        &registry(),
        &device(&bl_devices::midi_monitor_type(), &[]),
        FRAMES,
    );
    let mut reader = node.display().expect("the monitor has a display");
    let notes = vec![
        note_on(0, 60, 100.0 / 127.0, None),
        note_on(0, 64, 100.0 / 127.0, None),
        note_on(0, 67, 100.0 / 127.0, None),
    ];
    let out = notes_through(node.as_mut(), &notes, FRAMES);
    assert_eq!(out, notes, "the notes pass on");
    let data = reader.read();
    assert!(data.published >= 1, "the display published");
    let values: OrdMap<bl_model::ParamId, f64> = [(FREEZE, 0.0)].into_iter().collect();
    let shown = display(
        bl_model::DeviceId(bl_core::id::Id::new(1).unwrap()),
        &values,
        Some(&data.values),
    );
    assert_eq!(shown.reading("Root").unwrap().text, "C4");
    assert_eq!(shown.reading("Chord").unwrap().text, "Major");
    assert_eq!(shown.reading("Notes").unwrap().text, "3");
}

// Spec: midi-monitor R4. Clear empties the display.
#[test]
fn clear_empties_the_display() {
    let mut node = node(
        &registry(),
        &device(&bl_devices::midi_monitor_type(), &[]),
        FRAMES,
    );
    let mut reader = node.display().expect("the monitor has a display");
    notes_through(
        node.as_mut(),
        &[note_on(0, 60, 100.0 / 127.0, None)],
        FRAMES,
    );
    let data = reader.read();
    assert!(data.values[60] > 0.0, "the note went in");
    // A block with Clear turning on empties it.
    notes_through_events(node.as_mut(), &[], &[common::set(0, CLEAR, 1.0)], FRAMES);
    let data = reader.read();
    assert_eq!(data.values[60], 0.0, "Clear emptied the keyboard");
}

// Spec: midi-monitor R7, R8. The chord name reads root position, and a key
// is named from middle C.
#[test]
fn chords_and_keys_are_named() {
    assert_eq!(chord(&[60, 64, 67]), Some((60, "Major")));
    assert_eq!(chord(&[60]), None);
    assert_eq!(note_name(60), "C4");
    assert_eq!(note_name(69), "A4");
}

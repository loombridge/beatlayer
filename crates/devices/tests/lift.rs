//! Pitch, the transposing MIDI effect (WP-5.30; the transpose spec): notes
//! land on their frames transposed, the window blocks, folds or limits,
//! expressions follow their notes, and no note hangs when the device is
//! switched off, deleted or moved. Every block is a real-time section under
//! the allocation trap.

bl_rt::install_trap!();

mod common;

use bl_core::time::SampleRate;
use bl_devices::lift::{LIFT, LOWEST, MODE, PITCH, RANGE, USE_SCALE};
use bl_engine::scales::ScaleRuns;
use bl_engine::{Block, NoteBuffer, NoteEvent, NoteExpression, NoteKind, TransportInfo};
use bl_model::{Command, Scale, ScaleName};
use common::chain::{BEAT_FRAMES, Chain, note, nothing_hangs};
use common::{bend, bends, device, keys_on, node, off, on, registry, scaled_through};

const SIZES: [usize; 4] = [1, 100, 512, 4096];

// Spec: midi-transpose R1. Every note moves by Pitch, on its own frames, with
// its velocity and release.
#[test]
fn a_note_lands_transposed_on_its_frames() {
    for size in SIZES {
        let mut chain = Chain::new(4096);
        chain.effect(LIFT, &[(PITCH, 7.0)]);
        chain.clip(4, [note(60, 0, 1), note(64, 2, 1)]);
        chain.sync();
        chain.play();
        chain.run(4 * BEAT_FRAMES as usize, size);
        let heard = chain.heard();
        let on = |frame, key| (frame, NoteKind::On, key, 100.0 / 127.0);
        assert_eq!(heard.len(), 4, "block size {size}");
        assert_eq!(heard[0], on(0, 67));
        assert_eq!(
            (heard[1].0, heard[1].1, heard[1].2),
            (BEAT_FRAMES, NoteKind::Off, 67)
        );
        assert_eq!(heard[2], on(2 * BEAT_FRAMES, 71));
        assert_eq!(
            (heard[3].0, heard[3].1, heard[3].2),
            (3 * BEAT_FRAMES, NoteKind::Off, 71)
        );
    }
}

// Spec: midi-transpose R2. A negative Pitch moves notes down.
#[test]
fn a_negative_pitch_moves_notes_down() {
    let mut chain = Chain::new(512);
    chain.effect(LIFT, &[(PITCH, -12.0)]);
    chain.clip(2, [note(60, 0, 1)]);
    chain.sync();
    chain.play();
    chain.run(2 * BEAT_FRAMES as usize, 512);
    let keys: Vec<u8> = chain.heard().iter().map(|h| h.2).collect();
    assert_eq!(keys, [48, 48]);
}

// Spec: midi-transpose R3. Block drops a note outside the window, with its
// note-off.
#[test]
fn block_drops_notes_outside_the_window() {
    let mut chain = Chain::new(512);
    chain.effect(
        LIFT,
        &[(PITCH, 12.0), (LOWEST, 60.0), (RANGE, 12.0), (MODE, 0.0)],
    );
    // 48 + 12 = 60 is inside; 64 + 12 = 76 is outside.
    chain.clip(4, [note(48, 0, 1), note(64, 2, 1)]);
    chain.sync();
    chain.play();
    chain.run(4 * BEAT_FRAMES as usize, 512);
    let heard = chain.heard();
    assert_eq!(heard.len(), 2);
    assert_eq!((heard[0].2, heard[0].1), (60, NoteKind::On));
    assert_eq!((heard[1].2, heard[1].1), (60, NoteKind::Off));
}

// Spec: midi-transpose R4. Fold moves a note by octaves into the window.
#[test]
fn fold_moves_notes_by_octaves_into_the_window() {
    let mut chain = Chain::new(512);
    chain.effect(
        LIFT,
        &[(PITCH, 12.0), (LOWEST, 48.0), (RANGE, 23.0), (MODE, 1.0)],
    );
    chain.clip(4, [note(64, 0, 1), note(40, 2, 1)]);
    chain.sync();
    chain.play();
    chain.run(4 * BEAT_FRAMES as usize, 512);
    // 76 folds down to 64; 52 is inside.
    let ons: Vec<u8> = chain
        .heard()
        .iter()
        .filter(|h| h.1 == NoteKind::On)
        .map(|h| h.2)
        .collect();
    assert_eq!(ons, [64, 52]);
}

// Spec: midi-transpose R5. Limit moves a note to the window's nearest edge.
#[test]
fn limit_moves_notes_to_the_nearest_edge() {
    let mut chain = Chain::new(512);
    chain.effect(
        LIFT,
        &[(PITCH, 12.0), (LOWEST, 48.0), (RANGE, 23.0), (MODE, 2.0)],
    );
    chain.clip(4, [note(64, 0, 1), note(30, 2, 1)]);
    chain.sync();
    chain.play();
    chain.run(4 * BEAT_FRAMES as usize, 512);
    let ons: Vec<u8> = chain
        .heard()
        .iter()
        .filter(|h| h.1 == NoteKind::On)
        .map(|h| h.2)
        .collect();
    // 76 is limited to 71; 42 to 48.
    assert_eq!(ons, [71, 48]);
}

// Spec: midi-transpose R6. Changing Pitch while a note is held ends that note
// on the key it started on.
#[test]
fn a_held_note_ends_where_it_started() {
    for size in SIZES {
        let mut chain = Chain::new(4096);
        let lift = chain.effect(LIFT, &[(PITCH, 7.0)]);
        chain.clip(4, [note(60, 0, 2)]);
        chain.sync();
        chain.play();
        chain.run(BEAT_FRAMES as usize, size);
        chain.set(lift, PITCH, -5.0);
        chain.run(2 * BEAT_FRAMES as usize, size);
        let keys: Vec<(NoteKind, u8)> = chain.heard().iter().map(|h| (h.1, h.2)).collect();
        assert_eq!(
            keys,
            [(NoteKind::On, 67), (NoteKind::Off, 67)],
            "size {size}"
        );
    }
}

// Spec: device-framework R32. Switching the device off ends its notes on the
// frame the block starts.
#[test]
fn switching_off_while_a_note_is_held_leaves_nothing_hanging() {
    for size in SIZES {
        let mut chain = Chain::new(4096);
        let lift = chain.effect(LIFT, &[(PITCH, 2.0)]);
        chain.clip(4, [note(60, 0, 2)]);
        chain.sync();
        chain.play();
        chain.run(BEAT_FRAMES as usize, size);
        chain.apply(Command::SetDeviceEnabled {
            device: lift,
            enabled: false,
        });
        chain.run(3 * BEAT_FRAMES as usize, size);
        let heard = chain.heard();
        assert!(nothing_hangs(&heard), "size {size}: {heard:?}");
        let off = heard
            .iter()
            .find(|h| h.1 == NoteKind::Off && h.2 == 62)
            .unwrap_or_else(|| panic!("size {size}: no off for 62 in {heard:?}"));
        assert_eq!(off.0, BEAT_FRAMES, "size {size}");
    }
}

// Spec: device-framework R32. Deleting the device ends its notes.
#[test]
fn deleting_while_a_note_is_held_leaves_nothing_hanging() {
    for size in SIZES {
        let mut chain = Chain::new(4096);
        let lift = chain.effect(LIFT, &[(PITCH, 2.0)]);
        chain.clip(4, [note(60, 0, 2)]);
        chain.sync();
        chain.play();
        chain.run(BEAT_FRAMES as usize, size);
        chain.apply(Command::RemoveDevice { device: lift });
        chain.run(3 * BEAT_FRAMES as usize, size);
        let heard = chain.heard();
        assert!(nothing_hangs(&heard), "size {size}: {heard:?}");
        assert!(
            heard
                .iter()
                .any(|h| h.1 == NoteKind::Off && h.2 == 62 && h.0 == BEAT_FRAMES),
            "size {size}: {heard:?}"
        );
    }
}

// Spec: device-framework R32. Moving the device ends its notes, through the
// devices that now follow it.
#[test]
fn moving_while_a_note_is_held_leaves_nothing_hanging() {
    for size in SIZES {
        let mut chain = Chain::new(4096);
        let first = chain.effect(LIFT, &[(PITCH, 2.0)]);
        chain.effect(LIFT, &[(PITCH, 7.0)]);
        chain.clip(4, [note(60, 0, 2)]);
        chain.sync();
        chain.play();
        chain.run(BEAT_FRAMES as usize, size);
        let track = chain.track;
        chain.apply(Command::MoveDevice {
            device: first,
            track,
            index: 1,
        });
        chain.run(3 * BEAT_FRAMES as usize, size);
        let heard = chain.heard();
        assert!(nothing_hangs(&heard), "size {size}: {heard:?}");
    }
}

// Spec: midi-transpose R7. A transposed note's expressions follow it, by note ID.
#[test]
fn expressions_follow_the_transposed_note() {
    let registry = registry();
    let device = device(&bl_devices::lift_type(), &[(PITCH, 5.0)]);
    let mut lift = node(&registry, &device, 256);
    let on = |id| NoteEvent {
        offset: 0,
        kind: NoteKind::On,
        channel: 0,
        key: 60,
        velocity: 0.8,
        note_id: Some(id),
    };
    let bend = NoteEvent {
        offset: 10,
        kind: NoteKind::Expression {
            expression: NoteExpression::Bend,
            value: 0.5,
        },
        ..on(7)
    };
    let off = NoteEvent {
        offset: 20,
        kind: NoteKind::Off,
        ..on(7)
    };
    let notes = [on(7), bend, off];
    let mut out = NoteBuffer::with_capacity(16);
    let mut data = vec![0.0f32; 2 * 256];
    let mut block = Block::new(
        &mut data,
        2,
        256,
        256,
        &[],
        TransportInfo::default(),
        SampleRate::HZ_48000,
    )
    .with_notes(&notes, &mut out);
    bl_rt::rt_test!({ lift.process(&mut block) });
    let sent = out.notes();
    assert_eq!(sent.len(), 3);
    assert!(sent.iter().all(|n| n.key == 65 && n.note_id == Some(7)));
    assert_eq!(sent[1].kind, bend.kind);
}

// Spec: midi-transpose R8. A note blocked by the window takes its expressions
// with it.
#[test]
fn a_blocked_note_takes_its_expressions_with_it() {
    let registry = registry();
    let device = device(&bl_devices::lift_type(), &[(PITCH, 100.0)]);
    let mut lift = node(&registry, &device, 256);
    let on = NoteEvent {
        offset: 0,
        kind: NoteKind::On,
        channel: 0,
        key: 60,
        velocity: 0.8,
        note_id: None,
    };
    let notes = [
        on,
        NoteEvent {
            offset: 5,
            kind: NoteKind::Expression {
                expression: NoteExpression::Pressure,
                value: 0.3,
            },
            ..on
        },
        NoteEvent {
            offset: 9,
            kind: NoteKind::Off,
            ..on
        },
    ];
    let mut out = NoteBuffer::with_capacity(16);
    let mut data = vec![0.0f32; 2 * 256];
    let mut block = Block::new(
        &mut data,
        2,
        256,
        256,
        &[],
        TransportInfo::default(),
        SampleRate::HZ_48000,
    )
    .with_notes(&notes, &mut out);
    bl_rt::rt_test!({ lift.process(&mut block) });
    assert!(out.notes().is_empty(), "{:?}", out.notes());
}

// Spec: device-framework R30. Several hundred notes in a block go through a
// MIDI effect in order, on their frames, without allocating, and each ends.
#[test]
fn several_hundred_notes_in_one_block_go_through_in_order() {
    for size in SIZES {
        let mut chain = Chain::new(4096);
        chain.effect(LIFT, &[(PITCH, 1.0)]);
        // Three hundred short notes, one every 20 frames, on keys that don't
        // overlap: some 380 events land in the first 4,096-frame block.
        let ticks = |frames: i64| {
            bl_core::time::Ticks(frames * bl_core::time::TICKS_PER_QUARTER / BEAT_FRAMES)
        };
        let notes = (0..300u32).map(|i| {
            bl_model::Note::new((i % 100) as u8, ticks(i64::from(i) * 20), ticks(500), 90)
        });
        chain.clip(2, notes);
        chain.sync();
        chain.play();
        chain.run(2 * BEAT_FRAMES as usize, size);
        let heard = chain.heard();
        assert_eq!(heard.len(), 600, "size {size}");
        assert!(nothing_hangs(&heard));
        assert!(
            heard.windows(2).all(|w| w[0].0 <= w[1].0),
            "size {size}: in order of frame"
        );
        assert!(heard.iter().all(|h| h.2 >= 1 && h.2 <= 100));
    }
}

// Spec: device-framework R32. Moving the device to another track ends its
// notes on the track it left, where they sound.
#[test]
fn moving_to_another_track_ends_its_notes_on_the_track_it_left() {
    use bl_devices::touch::{MODE as TOUCH_MODE, OPERATION, OUT_HIGH, TOUCH};
    for size in SIZES {
        let mut chain = Chain::new(4096);
        let lift = chain.effect(LIFT, &[(PITCH, 2.0)]);
        chain.clip(4, [note(60, 0, 2)]);
        let first = chain.track;
        let other = chain.editor.new_id();
        chain
            .editor
            .apply(Command::CreateTrack {
                id: other,
                kind: bl_model::TrackKind::Midi,
                name: String::new(),
                parent: None,
                index: 1,
            })
            .unwrap();
        // The other track's instrument hears its note-offs at release
        // velocity 10, so the test can tell the two tracks apart.
        chain.track = other;
        chain.insert(0, bl_engine::testing::INSTRUMENT, &[]);
        chain.insert(
            0,
            TOUCH,
            &[(OPERATION, 1.0), (TOUCH_MODE, 2.0), (OUT_HIGH, 10.0)],
        );
        chain.track = first;
        chain.sync();
        chain.play();
        chain.run(BEAT_FRAMES as usize, size);
        chain.apply(Command::MoveDevice {
            device: lift,
            track: other,
            index: 0,
        });
        chain.run(3 * BEAT_FRAMES as usize, size);
        let heard = chain.heard();
        assert!(nothing_hangs(&heard), "size {size}: {heard:?}");
        let off = heard
            .iter()
            .find(|h| h.1 == NoteKind::Off && h.2 == 62)
            .unwrap_or_else(|| panic!("size {size}: no off for 62 in {heard:?}"));
        assert_eq!(off.0, BEAT_FRAMES, "size {size}");
        assert_eq!(off.3, 0.0, "size {size}: it ended on the other track");
    }
}

fn c_major() -> ScaleRuns {
    ScaleRuns::of(Scale::new(0, ScaleName::Major))
}

/// A Pitch node counting degrees, moving by `pitch`.
fn degrees(pitch: f64, window: &[(bl_model::ParamId, f64)]) -> Box<dyn bl_engine::Node> {
    let mut params = vec![(PITCH, pitch), (USE_SCALE, 1.0)];
    params.extend_from_slice(window);
    node(&registry(), &device(&bl_devices::lift_type(), &params), 256)
}

// Spec: midi-transpose R12. With Use Current Scale on, every note moves by
// degrees of the scale: a note in it lands on a scale note, one outside it
// keeps its distance above the scale note below it.
#[test]
fn use_current_scale_moves_notes_by_degrees() {
    let notes = [on(0, 60, 1), on(10, 61, 2), on(20, 64, 3)];
    let mut pitch = degrees(1.0, &[]);
    let sent = scaled_through(pitch.as_mut(), &notes, 256, c_major());
    // C3 to D3, C#3 two semitones up with C3's move, E3 to F3.
    assert_eq!(keys_on(&sent), [62, 63, 65]);
    // With the switch off the same notes move by semitones.
    let mut pitch = node(
        &registry(),
        &device(&bl_devices::lift_type(), &[(PITCH, 1.0)]),
        256,
    );
    let sent = scaled_through(pitch.as_mut(), &notes, 256, c_major());
    assert_eq!(keys_on(&sent), [61, 62, 65]);
    // A current scale with Scale Mode off is the chromatic one.
    let mut pitch = degrees(1.0, &[]);
    let off_scale = ScaleRuns::of(Scale::default());
    let sent = scaled_through(pitch.as_mut(), &notes, 256, off_scale);
    assert_eq!(keys_on(&sent), [61, 62, 65]);
}

// Spec: midi-transpose R12. A value beyond 30 acts as 30 when the switch is on.
#[test]
fn use_current_scale_holds_pitch_at_thirty_degrees() {
    let mut pitch = degrees(100.0, &[]);
    let sent = scaled_through(pitch.as_mut(), &[on(0, 36, 1)], 256, c_major());
    // Thirty degrees of C Major: four octaves and two degrees above C1.
    assert_eq!(keys_on(&sent), [36 + 48 + 4]);
    let mut pitch = degrees(-100.0, &[]);
    let sent = scaled_through(pitch.as_mut(), &[on(0, 120, 1)], 256, c_major());
    assert_eq!(keys_on(&sent), [120 - 48 - 3]);
}

// Spec: midi-transpose R13. Lowest and Range are keys, and the window takes
// the key a note's move gives.
#[test]
fn the_window_takes_the_key_the_degrees_give() {
    let mut pitch = degrees(2.0, &[(LOWEST, 60.0), (RANGE, 4.0), (MODE, 0.0)]);
    // C3 moves up two degrees to E3 (64), inside 60 to 64; D3 to F3 (65),
    // outside, and blocked.
    let notes = [on(0, 60, 1), on(10, 62, 2)];
    let sent = scaled_through(pitch.as_mut(), &notes, 256, c_major());
    assert_eq!(keys_on(&sent), [64]);
}

// Spec: midi-transpose R14. A bend is carried in degrees: the bend that takes
// C3 two semitones up, to D3, takes the E3 it moved to a degree up, to F3.
#[test]
fn a_bend_is_carried_in_degrees() {
    let mut pitch = degrees(2.0, &[]);
    let notes = [
        on(0, 60, 7),
        bend(5, 60, 7, 2.0 / 48.0),
        bend(6, 60, 7, 1.0 / 48.0),
    ];
    let sent = scaled_through(pitch.as_mut(), &notes, 256, c_major());
    let carried = bends(&sent);
    assert_eq!(carried.len(), 2);
    assert!(carried.iter().all(|(key, _)| *key == 64));
    // Up a degree from E3 is F3, one semitone; halfway to it is half that.
    assert!((carried[0].1 - 1.0 / 48.0).abs() < 1e-6, "{carried:?}");
    assert!((carried[1].1 - 0.5 / 48.0).abs() < 1e-6, "{carried:?}");
}

// Spec: midi-transpose R14, scales R17. A change of the current scale while a
// note is held doesn't move it: the note-off goes to the key the note-on did.
#[test]
fn a_scale_change_while_a_note_is_held_does_not_move_it() {
    let mut pitch = degrees(2.0, &[]);
    let mut scale = c_major();
    scale.push(100, Scale::new(2, ScaleName::Major));
    let notes = [on(0, 60, 1), off(200, 60, 1)];
    let sent = scaled_through(pitch.as_mut(), &notes, 256, scale);
    assert_eq!(
        sent.iter().map(|n| (n.kind, n.key)).collect::<Vec<_>>(),
        [(NoteKind::On, 64), (NoteKind::Off, 64)]
    );
    // And the next note on a frame after the change counts the new scale.
    let mut pitch = degrees(2.0, &[]);
    let mut scale = c_major();
    scale.push(100, Scale::new(2, ScaleName::Major));
    let notes = [on(99, 60, 1), on(100, 60, 2)];
    let sent = scaled_through(pitch.as_mut(), &notes, 256, scale);
    // C3 is outside D Major: its scale note below is B2, two degrees up from
    // there is D3, three semitones, and C3 keeps its distance above B2.
    assert_eq!(keys_on(&sent), [64, 63]);
}

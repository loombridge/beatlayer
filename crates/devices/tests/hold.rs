//! Hold, the note-length MIDI effect (WP-5.30; the MIDI note length spec): the
//! two trigger sources, Length in milliseconds or beat divisions, Gate,
//! Release Velocity, Decay Time, Key Scale and Latch. Every block is a
//! real-time section under the allocation trap.

bl_rt::install_trap!();

mod common;

use bl_core::time::Bpm;
use bl_devices::hold::{
    DECAY_TIME, GATE, HOLD, KEY_SCALE, LATCH, LENGTH_DIVISION, LENGTH_MS, RELEASE_VELOCITY, SYNC,
    TRIGGER,
};
use bl_engine::NoteKind;
use bl_model::Command;
use common::chain::{BEAT_FRAMES, Chain, note, nothing_hangs};
use common::{node, note_off, note_on, notes_through, registry};

const SIZES: [usize; 4] = [1, 100, 512, 4096];

fn play(
    params: &[(bl_model::ParamId, f64)],
    notes: impl IntoIterator<Item = bl_model::Note>,
    size: usize,
) -> Vec<(i64, NoteKind, u8, f32)> {
    let mut chain = Chain::new(4096);
    chain.effect(HOLD, params);
    chain.clip(8, notes);
    chain.sync();
    chain.play();
    chain.run(8 * BEAT_FRAMES as usize, size);
    chain.heard()
}

fn times(heard: &[(i64, NoteKind, u8, f32)], kind: NoteKind) -> Vec<i64> {
    heard.iter().filter(|h| h.1 == kind).map(|h| h.0).collect()
}

// Spec: midi-note-length R1. In Note On mode each note lasts Length, whatever
// its own length, on exact frames.
#[test]
fn a_note_lasts_the_length() {
    for size in SIZES {
        // 100 ms is 4,800 frames.
        let heard = play(&[(LENGTH_MS, 100.0)], [note(60, 0, 2)], size);
        assert_eq!(times(&heard, NoteKind::On), [0], "size {size}");
        assert_eq!(times(&heard, NoteKind::Off), [4800], "size {size}");
    }
}

// Spec: midi-note-length R2. Gate scales the length.
#[test]
fn gate_scales_the_length() {
    let half = play(&[(LENGTH_MS, 100.0), (GATE, 50.0)], [note(60, 0, 1)], 512);
    assert_eq!(times(&half, NoteKind::Off), [2400]);
    let double = play(&[(LENGTH_MS, 100.0), (GATE, 200.0)], [note(60, 0, 1)], 512);
    assert_eq!(times(&double, NoteKind::Off), [9600]);
}

// Spec: midi-note-length R3. Synced, the length is a beat division at the
// tempo of the moment, at two tempos.
#[test]
fn a_synced_length_follows_the_tempo() {
    // 1/8 is half a beat: 12,000 frames at 120 BPM, 24,000 at 60.
    let params = [(SYNC, 1.0), (LENGTH_DIVISION, 2.0)];
    let at_120 = play(&params, [note(60, 0, 2)], 512);
    assert_eq!(times(&at_120, NoteKind::Off), [12_000]);
    let mut chain = Chain::new(4096);
    chain.apply(Command::SetTempo {
        tempo: bl_core::time::TempoMap::constant(Bpm::new(60.0).unwrap()),
    });
    chain.effect(HOLD, &params);
    chain.clip(8, [note(60, 0, 2)]);
    chain.sync();
    chain.play();
    chain.run(4 * BEAT_FRAMES as usize, 512);
    assert_eq!(times(&chain.heard(), NoteKind::Off), [24_000]);
}

// Spec: midi-note-length R4. In Note Off mode a note starts where the key
// comes up, and lasts Length from there.
#[test]
fn note_off_mode_starts_notes_when_keys_come_up() {
    for size in SIZES {
        let heard = play(
            &[(TRIGGER, 1.0), (LENGTH_MS, 100.0)],
            [note(60, 0, 1)],
            size,
        );
        assert_eq!(times(&heard, NoteKind::On), [BEAT_FRAMES], "size {size}");
        assert_eq!(
            times(&heard, NoteKind::Off),
            [BEAT_FRAMES + 4800],
            "size {size}"
        );
    }
}

// Spec: midi-note-length R5. Key Scale lengthens notes below middle C and
// shortens those above (positive), and the opposite (negative).
#[test]
fn key_scale_changes_the_length_by_pitch() {
    let heard = play(
        &[(LENGTH_MS, 100.0), (KEY_SCALE, 100.0)],
        [note(48, 0, 1), note(60, 2, 1), note(72, 4, 1)],
        512,
    );
    let ons = times(&heard, NoteKind::On);
    let offs = times(&heard, NoteKind::Off);
    let lengths: Vec<i64> = ons.iter().zip(&offs).map(|(a, b)| b - a).collect();
    assert_eq!(lengths, [9600, 4800, 2400]);
    let reverse = play(
        &[(LENGTH_MS, 100.0), (KEY_SCALE, -100.0)],
        [note(48, 0, 1), note(72, 4, 1)],
        512,
    );
    let ons = times(&reverse, NoteKind::On);
    let offs = times(&reverse, NoteKind::Off);
    let lengths: Vec<i64> = ons.iter().zip(&offs).map(|(a, b)| b - a).collect();
    assert_eq!(lengths, [2400, 9600]);
}

// Spec: midi-note-length R6. A note on a key that is still sounding ends it
// first, and the earlier note's note-off doesn't cut the new one.
#[test]
fn a_retriggered_key_ends_the_note_before() {
    let heard = play(
        &[(LENGTH_MS, 1000.0)],
        [note(60, 0, 1), note(60, 1, 1)],
        512,
    );
    assert_eq!(times(&heard, NoteKind::On), [0, BEAT_FRAMES]);
    assert_eq!(
        times(&heard, NoteKind::Off),
        [BEAT_FRAMES, BEAT_FRAMES + 48_000]
    );
    let kinds: Vec<NoteKind> = heard
        .iter()
        .filter(|h| h.0 == BEAT_FRAMES)
        .map(|h| h.1)
        .collect();
    assert_eq!(kinds, [NoteKind::Off, NoteKind::On], "the off comes first");
}

// Spec: midi-note-length R7. Latch in Note On mode keeps each note until the
// next one starts.
#[test]
fn latch_keeps_a_note_until_the_next_starts() {
    let heard = play(&[(LATCH, 1.0)], [note(60, 0, 1), note(64, 2, 1)], 512);
    assert_eq!(times(&heard, NoteKind::On), [0, 2 * BEAT_FRAMES]);
    assert_eq!(times(&heard, NoteKind::Off), [2 * BEAT_FRAMES]);
    let keys: Vec<u8> = heard.iter().map(|h| h.2).collect();
    assert_eq!(keys, [60, 60, 64]);
}

// Spec: midi-note-length R8. In Note Off mode a note's velocity falls over
// Decay Time while the key is held, and Release Velocity mixes in the key's
// release velocity.
#[test]
fn decay_and_release_velocity_set_the_velocity() {
    let registry = registry();
    let params = |decay, release| {
        let device = common::device(
            &bl_devices::hold_type(),
            &[
                (TRIGGER, 1.0),
                (LENGTH_MS, 100.0),
                (DECAY_TIME, decay),
                (RELEASE_VELOCITY, release),
            ],
        );
        node(&registry, &device, 8192)
    };
    let on = note_on(0, 60, 0.8, None);
    // Held for 50 ms of a 100 ms decay: half the velocity is left.
    let off = note_off(2400, &on, 0.2);
    let velocity = |decay, release| {
        let mut hold = params(decay, release);
        let sent = notes_through(hold.as_mut(), &[on, off], 8192);
        sent.iter()
            .find(|n| n.kind == NoteKind::On)
            .unwrap()
            .velocity
    };
    assert!(
        (velocity(0.0, 0.0) - 0.8).abs() < 1e-6,
        "no decay, no release"
    );
    assert!((velocity(100.0, 0.0) - 0.4).abs() < 1e-6, "decayed to half");
    assert!(
        (velocity(100.0, 50.0) - 0.3).abs() < 1e-6,
        "half release velocity"
    );
    assert!(
        (velocity(0.0, 100.0) - 0.2).abs() < 1e-6,
        "all release velocity"
    );
}

// Spec: device-framework R28. Switching off, deleting or moving the device
// while its notes sound leaves nothing hanging.
#[test]
fn switching_off_or_deleting_with_a_note_sounding_ends_it() {
    for remove in [false, true] {
        for size in SIZES {
            let mut chain = Chain::new(4096);
            let hold = chain.effect(HOLD, &[(LENGTH_MS, 5000.0)]);
            chain.clip(4, [note(60, 0, 1)]);
            chain.sync();
            chain.play();
            chain.run(BEAT_FRAMES as usize / 2, size);
            chain.apply(if remove {
                Command::RemoveDevice { device: hold }
            } else {
                Command::SetDeviceEnabled {
                    device: hold,
                    enabled: false,
                }
            });
            chain.run(2 * BEAT_FRAMES as usize, size);
            let heard = chain.heard();
            assert!(
                nothing_hangs(&heard),
                "remove {remove} size {size}: {heard:?}"
            );
            // It ends on the frame the change lands, not when the key comes up.
            let first_off = times(&heard, NoteKind::Off)[0];
            assert_eq!(first_off, BEAT_FRAMES / 2, "remove {remove} size {size}");
        }
    }
}

// Spec: device-framework R27. Turning Latch off ends the latched note.
#[test]
fn turning_latch_off_ends_the_latched_note() {
    for size in SIZES {
        let mut chain = Chain::new(4096);
        let hold = chain.effect(HOLD, &[(LATCH, 1.0)]);
        chain.clip(4, [note(60, 0, 1)]);
        chain.sync();
        chain.play();
        chain.run(2 * BEAT_FRAMES as usize, size);
        chain.set(hold, LATCH, 0.0);
        chain.run(2 * BEAT_FRAMES as usize, size);
        let heard = chain.heard();
        assert!(nothing_hangs(&heard), "size {size}: {heard:?}");
        assert_eq!(
            times(&heard, NoteKind::Off),
            [2 * BEAT_FRAMES],
            "size {size}"
        );
    }
}

// Spec: device-framework R27. A note-off for a note that went through while
// Hold was switched off passes through once it is on again.
#[test]
fn a_note_played_while_switched_off_ends_after_switching_on() {
    for size in SIZES {
        let mut chain = Chain::new(4096);
        let hold = chain.effect(HOLD, &[]);
        chain.clip(4, [note(60, 0, 2)]);
        chain.apply(Command::SetDeviceEnabled {
            device: hold,
            enabled: false,
        });
        chain.play();
        chain.run(BEAT_FRAMES as usize, size);
        chain.apply(Command::SetDeviceEnabled {
            device: hold,
            enabled: true,
        });
        chain.run(3 * BEAT_FRAMES as usize, size);
        let heard = chain.heard();
        assert!(nothing_hangs(&heard), "size {size}: {heard:?}");
        assert_eq!(
            times(&heard, NoteKind::Off),
            [2 * BEAT_FRAMES],
            "size {size}"
        );
    }
}

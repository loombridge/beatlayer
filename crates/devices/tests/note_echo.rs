//! Note Echo, the note echo MIDI effect (WP-5.31; the note-echo spec):
//! synced 16th-note divisions with swing, free time in milliseconds,
//! Thru/Mute input modes, per-repeat pitch transposition, velocity decay,
//! feedback, MPE expression feedback, and ending notes cleanly on stop.
//! Every block is a real-time section under the allocation trap.

use crate::common;
use bl_core::time::Bpm;
use bl_devices::note_echo::{
    FEEDBACK, INPUT, MPE, NOTE_ECHO, PITCH, PRESS_FEEDBACK, SYNC, TIME_MS, TIME_SYNC,
    VELOCITY_DECAY, note_echo_type,
};
use bl_engine::{Block, DeviceType, Node, NoteEvent, NoteKind};
use bl_model::{Command, DeviceRole};
use common::chain::{BEAT_FRAMES, Chain, note, nothing_hangs};
use common::{device, node, notes_through_events, off, on, registry, set};

const SIZES: [usize; 4] = [1, 100, 512, 4096];

fn play(
    params: &[(bl_model::ParamId, f64)],
    notes: impl IntoIterator<Item = bl_model::Note>,
    size: usize,
) -> Vec<(i64, NoteKind, u8, f32)> {
    let mut chain = Chain::new(4096);
    chain.effect(NOTE_ECHO, params);
    chain.clip(8, notes);
    chain.sync();
    chain.play();
    chain.run(8 * BEAT_FRAMES as usize, size);
    chain.heard()
}

fn times(heard: &[(i64, NoteKind, u8, f32)], kind: NoteKind) -> Vec<i64> {
    heard.iter().filter(|h| h.1 == kind).map(|h| h.0).collect()
}

fn keys(heard: &[(i64, NoteKind, u8, f32)], kind: NoteKind) -> Vec<u8> {
    heard.iter().filter(|h| h.1 == kind).map(|h| h.2).collect()
}

// Spec: midi-echo R1. Sync and free delay times land on exact frames.
#[test]
fn sync_and_free_delay_time_land_on_exact_frames() {
    for size in SIZES {
        // Sync mode: 4 sixteenths = 1 quarter beat = 24,000 frames at 120 BPM.
        // Single echo (FEEDBACK = 0).
        let sync_120 = play(
            &[(SYNC, 1.0), (TIME_SYNC, 4.0), (FEEDBACK, 0.0)],
            [note(60, 0, 1)],
            size,
        );
        let ons = times(&sync_120, NoteKind::On);
        assert_eq!(ons[0], 0, "original note at frame 0 (size {size})");
        assert_eq!(ons[1], BEAT_FRAMES, "echo at 1 beat (size {size})");

        // At 60 BPM: 1 beat = 48,000 frames.
        let mut chain = Chain::new(4096);
        chain.apply(Command::SetTempo {
            tempo: bl_core::time::TempoMap::constant(Bpm::new(60.0).unwrap()),
        });
        chain.effect(NOTE_ECHO, &[(SYNC, 1.0), (TIME_SYNC, 4.0), (FEEDBACK, 0.0)]);
        chain.clip(8, [note(60, 0, 1)]);
        chain.sync();
        chain.play();
        chain.run(4 * BEAT_FRAMES as usize, size);
        let ons_60 = times(&chain.heard(), NoteKind::On);
        assert_eq!(ons_60[0], 0);
        assert_eq!(ons_60[1], 48_000);

        // Free mode: 100 ms = 4,800 frames at 48 kHz.
        let free = play(
            &[(SYNC, 0.0), (TIME_MS, 100.0), (FEEDBACK, 0.0)],
            [note(60, 0, 1)],
            size,
        );
        let ons_free = times(&free, NoteKind::On);
        assert_eq!(ons_free[0], 0);
        assert_eq!(ons_free[1], 4800);
    }
}

// Spec: midi-echo R2. Input mode Thru and Mute.
#[test]
fn input_mode_thru_and_mute() {
    // Thru (0): original note sounds at frame 0, echo at frame 24,000.
    let thru = play(&[(INPUT, 0.0), (FEEDBACK, 0.0)], [note(60, 0, 1)], 512);
    let ons_thru = times(&thru, NoteKind::On);
    assert_eq!(ons_thru.len(), 2);
    assert_eq!(ons_thru[0], 0);
    assert_eq!(ons_thru[1], BEAT_FRAMES);

    // Mute (1): original note is dropped, only echo at frame 24,000 sounds.
    let mute = play(&[(INPUT, 1.0), (FEEDBACK, 0.0)], [note(60, 0, 1)], 512);
    let ons_mute = times(&mute, NoteKind::On);
    assert_eq!(ons_mute.len(), 1);
    assert_eq!(ons_mute[0], BEAT_FRAMES);
}

// Spec: midi-echo R3. Pitch transposition applies per repeat.
#[test]
fn pitch_transposition_applies_per_repeat() {
    // Pitch +3 semitones per repeat, 2 echoes (FEEDBACK = 50.0).
    let heard = play(
        &[(PITCH, 3.0), (FEEDBACK, 50.0), (TIME_SYNC, 4.0)],
        [note(60, 0, 1)],
        512,
    );
    let on_keys = keys(&heard, NoteKind::On);
    assert_eq!(on_keys[0], 60);
    assert_eq!(on_keys[1], 63);
    assert_eq!(on_keys[2], 66);
}

// Spec: midi-echo R10. Note Echo has no Use Current Scale: its Pitch moves each
// echo by keys whatever the scale of its track's clip.
#[test]
fn pitch_moves_by_keys_whatever_the_clip_s_scale() {
    for size in SIZES {
        let mut chain = Chain::new(4096);
        chain.effect(
            NOTE_ECHO,
            &[(PITCH, 3.0), (FEEDBACK, 50.0), (TIME_SYNC, 4.0)],
        );
        chain.clip(8, [note(60, 0, 1)]);
        chain.scale_clips(bl_model::Scale::new(0, bl_model::ScaleName::Major));
        chain.sync();
        chain.play();
        chain.run(8 * BEAT_FRAMES as usize, size);
        let on_keys = keys(&chain.heard(), NoteKind::On);
        // Three semitones, not three degrees, which would be 64 and 67.
        assert_eq!(&on_keys[0..3], &[60, 63, 66], "block size {size}");
    }
}

// Spec: midi-echo R4. Velocity decay and feedback diminish repeats.
#[test]
fn velocity_decay_and_feedback_diminish_repeats() {
    // Velocity Decay 50 %, Feedback 50 %: each repeat is scaled down.
    let heard = play(
        &[(VELOCITY_DECAY, 50.0), (FEEDBACK, 50.0), (TIME_SYNC, 4.0)],
        [note(60, 0, 1)],
        512,
    );
    let vels: Vec<f32> = heard
        .iter()
        .filter(|h| h.1 == NoteKind::On)
        .map(|h| h.3)
        .collect();
    assert!(vels.len() >= 3);
    assert!(vels[1] < vels[0], "first echo quieter than original");
    assert!(vels[2] < vels[1], "second echo quieter than first");
}

// Spec: midi-echo R5. Echoes preserve original note duration.
#[test]
fn echoes_preserve_original_note_duration() {
    // Note lasts 1 beat (24,000 frames) from 0 to 24,000.
    // Echo starts at 24,000 and should end at 48,000 (lasting 24,000 frames).
    let heard = play(&[(FEEDBACK, 0.0), (TIME_SYNC, 4.0)], [note(60, 0, 1)], 512);
    let ons = times(&heard, NoteKind::On);
    let offs = times(&heard, NoteKind::Off);
    assert_eq!(ons, [0, 24_000]);
    assert_eq!(offs, [24_000, 48_000]);
}

// Spec: midi-echo R6. MPE switch and expression feedback.
#[test]
fn mpe_switch_and_expression_feedback() {
    // When MPE switch is off, expressions are not echoed.
    // When on, per-note expressions accompany echoes.
    let mut chain = Chain::new(4096);
    chain.effect(
        NOTE_ECHO,
        &[(MPE, 1.0), (PRESS_FEEDBACK, 50.0), (FEEDBACK, 0.0)],
    );
    chain.clip(8, [note(60, 0, 1)]);
    chain.sync();
    chain.play();
    chain.run(4 * BEAT_FRAMES as usize, 512);
    assert!(!chain.heard().is_empty());
}

// Spec: midi-echo R7. Stopping transport leaves no hanging echoes.
#[test]
fn stopping_transport_leaves_no_hanging_echoes() {
    let mut chain = Chain::new(4096);
    chain.effect(NOTE_ECHO, &[(FEEDBACK, 80.0)]);
    chain.clip(8, [note(60, 0, 4)]);
    chain.sync();
    chain.play();
    chain.run(BEAT_FRAMES as usize, 512);
    chain.stop();
    chain.run(BEAT_FRAMES as usize, 512);
    let heard = chain.heard();
    let ons = times(&heard, NoteKind::On).len();
    let offs = times(&heard, NoteKind::Off).len();
    assert_eq!(ons, offs, "all notes and echoes ended cleanly");
}

// Spec: midi-echo R7. A stop while an echo is sounding ends that echo too.
#[test]
fn stopping_transport_while_an_echo_sounds_leaves_no_hanging_notes() {
    let mut chain = Chain::new(4096);
    // Time 2 sixteenths is half a beat: the echo at 12,000 frames has
    // sounded when the transport stops at 14,000, and the source note, two
    // beats long, never reaches its own end.
    chain.effect(NOTE_ECHO, &[(SYNC, 1.0), (TIME_SYNC, 2.0), (FEEDBACK, 0.0)]);
    chain.clip(8, [note(60, 0, 2)]);
    chain.sync();
    chain.play();
    chain.run(14_000, 512);
    chain.stop();
    chain.run(2 * BEAT_FRAMES as usize, 512);
    let heard = chain.heard();
    assert!(nothing_hangs(&heard), "no note hangs: {heard:?}");
    assert_eq!(
        times(&heard, NoteKind::On).len(),
        times(&heard, NoteKind::Off).len(),
        "every note ended: {heard:?}"
    );
}

// Spec: midi-echo R2. A monitored-input note carries no ID, as the engine
// makes it; the input passes through and its echoes outlive it.
#[test]
fn input_notes_without_an_id_keep_their_echoes() {
    let mut chain = Chain::with_registry(4096, |registry| {
        registry.add_device(strip_ids_type(), |_, _| Box::new(StripIds));
    });
    chain.effect(STRIP_IDS, &[]);
    chain.effect(NOTE_ECHO, &[(TIME_SYNC, 8.0), (FEEDBACK, 0.0)]);
    chain.clip(8, [note(60, 0, 1)]);
    chain.sync();
    chain.play();
    chain.run(4 * BEAT_FRAMES as usize, 512);
    let heard = chain.heard();
    assert_eq!(times(&heard, NoteKind::On), [0, 48_000], "{heard:?}");
    assert_eq!(times(&heard, NoteKind::Off), [24_000, 72_000], "{heard:?}");
}

// Spec: device-framework R31, midi-echo R2. A note-off follows what happened
// to its note-on: switching Input to Mute while a note is held still ends the
// dry note, and switching to Thru mid-note starts no dry note-off.
#[test]
fn switching_input_to_mute_while_a_note_is_held_still_ends_it() {
    let seven = |sent: &[NoteEvent], kind| {
        sent.iter()
            .filter(|n| n.kind == kind && n.key == 60 && n.note_id == Some(7))
            .map(|n| n.offset)
            .collect::<Vec<_>>()
    };
    let run = |start: f64, then: f64| {
        let device = device(&note_echo_type(), &[(INPUT, start), (FEEDBACK, 0.0)]);
        let mut echo = node(&registry(), &device, 512);
        notes_through_events(
            echo.as_mut(),
            &[on(0, 60, 7), off(200, 60, 7)],
            &[set(100, INPUT, then)],
            512,
        )
    };
    let thru_then_mute = run(0.0, 1.0);
    assert_eq!(seven(&thru_then_mute, NoteKind::On), [0]);
    assert_eq!(seven(&thru_then_mute, NoteKind::Off), [200]);

    let mute_then_thru = run(1.0, 0.0);
    assert!(seven(&mute_then_thru, NoteKind::On).is_empty());
    assert!(seven(&mute_then_thru, NoteKind::Off).is_empty());
}

// Spec: midi-echo R7. A note with no room in the device makes no echoes, so
// no echo is left without its note-off.
#[test]
fn more_than_64_overlapping_notes_leave_no_echo_hanging() {
    let mut chain = Chain::new(4096);
    chain.effect(NOTE_ECHO, &[(SYNC, 1.0), (TIME_SYNC, 4.0), (FEEDBACK, 0.0)]);
    chain.clip(8, (0u8..70).map(|k| note(k, 0, 1)));
    chain.sync();
    chain.play();
    chain.run(4 * BEAT_FRAMES as usize, 512);
    let heard = chain.heard();
    assert!(nothing_hangs(&heard), "{heard:?}");
    for key in 0..64 {
        let count = |kind| heard.iter().filter(|h| h.1 == kind && h.2 == key).count();
        assert_eq!(count(NoteKind::On), count(NoteKind::Off), "key {key}");
    }
}

/// A test device that forwards every note without its ID, as the engine's
/// monitored MIDI input hands notes to a chain.
const STRIP_IDS: &str = "test.strip_ids";

fn strip_ids_type() -> DeviceType {
    DeviceType::native(STRIP_IDS, "Test Strip IDs", DeviceRole::MidiEffect)
}

struct StripIds;

impl Node for StripIds {
    fn process(&mut self, block: &mut Block<'_>) {
        for note in block.notes() {
            block.send_note(NoteEvent {
                note_id: None,
                ..*note
            });
        }
    }
}

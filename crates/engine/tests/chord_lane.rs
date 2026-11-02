//! The chord lane in the engine (WP-6.48; the chord lane spec, R6 to R9 and
//! R11): a following track's Arrangement notes move by the region they start
//! in, in degrees of the clip's scale or in semitones, before the devices;
//! each note ends on the key it started on; the region's scale is the track's
//! devices' current scale; Session clips and tracks that don't follow are
//! untouched; and an export plays as playback does. Every block runs as a
//! real-time section under the allocation trap.

use crate::common;
use bl_engine::testing::{self, RecordedNote, scale_code};
use bl_engine::{EngineConfig, Offline, SessionCommand, TransportCommand};
use bl_model::{
    Breakpoint, ChordRegion, Clip, ClipData, Command, Editor, MidiData, Note, Scale, ScaleName,
    TrackId, TrackKind,
};
use common::{RATE, Rig, device, track};

const BEAT: bl_core::time::Ticks = bl_core::time::Ticks::QUARTER;
const BEAT_FRAMES: i64 = 24_000;
const SIZES: [usize; 3] = [1, 64, 512];

fn d_dorian() -> Scale {
    Scale::new(2, ScaleName::Dorian)
}

/// A region from beat `from` to beat `to`.
fn region(editor: &mut Editor, from: i64, to: i64, scale: Scale, transpose: i8) {
    let id = editor.new_id();
    let mut region = ChordRegion::new(id, BEAT * from, BEAT * to);
    region.scale = scale;
    region.transpose = transpose;
    editor.apply(Command::AddChordRegion { region }).unwrap();
}

/// A MIDI track with the test instrument, playing a clip over four beats
/// holding `notes` (key, beat, beats) in `scale`.
fn played(editor: &mut Editor, notes: &[(u8, i64, i64)], scale: Scale, follow: bool) -> TrackId {
    let id = track(editor, TrackKind::Midi, None);
    device(editor, id, testing::INSTRUMENT, &[]);
    let notes = notes
        .iter()
        .map(|&(key, at, length)| Note::new(key, BEAT * at, BEAT * length, 100));
    let mut clip = Clip::new(
        editor.new_id(),
        bl_core::time::Ticks::ZERO,
        BEAT * 4,
        ClipData::Midi(MidiData::new(notes)),
    );
    clip.scale = scale;
    editor
        .apply(Command::CreateClip { track: id, clip })
        .unwrap();
    if follow {
        editor
            .apply(Command::SetFollowChordLane {
                track: id,
                follow: true,
            })
            .unwrap();
    }
    id
}

/// The notes the rig's instruments heard over `frames` frames: (song frame,
/// on or off, key).
fn heard(rig: &mut Rig, frames: usize, size: usize) -> Vec<(i64, bool, u8)> {
    rig.sender.transport(TransportCommand::Play);
    rig.render(frames, size);
    summary(rig.probe.recorded_notes())
}

fn summary(notes: Vec<RecordedNote>) -> Vec<(i64, bool, u8)> {
    notes
        .into_iter()
        .map(|n| {
            (
                n.frame,
                n.event.kind == bl_engine::NoteKind::On,
                n.event.key,
            )
        })
        .collect()
}

/// A note on every beat of D, in a clip of D Dorian.
const ON_EACH_BEAT: [(u8, i64, i64); 4] = [(62, 0, 1), (62, 1, 1), (62, 2, 1), (62, 3, 1)];

#[test]
fn a_following_track_s_notes_move_by_the_region_they_start_in() {
    // Spec: chord-lane R6, R11. D Dorian: D E F G A B C, so two degrees up
    // from D is F (65) and one down is C (60); where the clip's Scale Mode is
    // off the same numbers are semitones.
    for (scale, up, down) in [(d_dorian(), 65, 60), (Scale::default(), 64, 61)] {
        for size in SIZES {
            let mut rig = Rig::new(512);
            played(&mut rig.editor, &ON_EACH_BEAT, scale, true);
            region(&mut rig.editor, 1, 2, Scale::default(), 2);
            region(&mut rig.editor, 2, 3, Scale::default(), -1);
            rig.sync();
            let heard = heard(&mut rig, 5 * BEAT_FRAMES as usize, size);
            let on: Vec<(i64, u8)> = heard
                .iter()
                .filter(|n| n.1)
                .map(|n| (n.0 / BEAT_FRAMES, n.2))
                .collect();
            assert_eq!(
                on,
                [(0, 62), (1, up), (2, down), (3, 62)],
                "{scale:?}, block size {size}"
            );
            // The off of each note is on the key of its on, on the frame it
            // had: a beat later.
            for (frame, _, key) in heard.iter().filter(|n| !n.1) {
                let on = heard
                    .iter()
                    .find(|n| n.1 && n.0 == frame - BEAT_FRAMES)
                    .unwrap();
                assert_eq!(*key, on.2, "block size {size}");
            }
        }
    }
}

#[test]
fn a_note_held_across_a_region_s_edge_ends_on_its_own_key() {
    // Spec: chord-lane R7. One note starts before the region and ends inside
    // it; another starts inside it and ends after it.
    for size in SIZES {
        let mut rig = Rig::new(512);
        played(
            &mut rig.editor,
            &[(62, 0, 2), (62, 2, 2)],
            Scale::default(),
            true,
        );
        region(&mut rig.editor, 1, 3, Scale::default(), 5);
        rig.sync();
        let heard = heard(&mut rig, 5 * BEAT_FRAMES as usize, size);
        assert_eq!(
            heard,
            [
                (0, true, 62),
                (2 * BEAT_FRAMES, false, 62),
                (2 * BEAT_FRAMES, true, 67),
                (4 * BEAT_FRAMES, false, 67),
            ],
            "block size {size}"
        );
    }
}

#[test]
fn a_note_moved_off_the_keyboard_does_not_play() {
    // Spec: chord-lane R6.
    let mut rig = Rig::new(512);
    played(
        &mut rig.editor,
        &[(126, 0, 1), (60, 1, 1)],
        Scale::default(),
        true,
    );
    region(&mut rig.editor, 0, 2, Scale::default(), 2);
    rig.sync();
    let heard = heard(&mut rig, 3 * BEAT_FRAMES as usize, 512);
    assert_eq!(
        heard,
        [
            (BEAT_FRAMES, true, 62),
            (2 * BEAT_FRAMES, false, 62)
        ]
    );
}

#[test]
fn a_track_that_does_not_follow_and_a_session_clip_play_as_written() {
    // Spec: chord-lane R5, R9.
    let mut rig = Rig::new(512);
    let plain = played(&mut rig.editor, &ON_EACH_BEAT, d_dorian(), false);
    let followed = played(&mut rig.editor, &ON_EACH_BEAT, d_dorian(), true);
    region(&mut rig.editor, 0, 4, Scale::default(), 3);
    // A following track's Session clip, launched, is not moved either.
    let scene = rig.editor.document().scenes().next().unwrap().id;
    let slot = Clip::new(
        rig.editor.new_id(),
        bl_core::time::Ticks::ZERO,
        BEAT * 4,
        ClipData::Midi(MidiData::new([Note::new(62, bl_core::time::Ticks::ZERO, BEAT, 100)])),
    );
    rig.editor
        .apply(Command::CreateSlotClip {
            track: followed,
            scene,
            clip: slot,
        })
        .unwrap();
    rig.sync();
    let _ = plain;
    rig.sender.transport(TransportCommand::Play);
    rig.render(4 * BEAT_FRAMES as usize, 64);
    let heard: Vec<u8> = summary(rig.probe.recorded_notes())
        .iter()
        .filter(|n| n.1)
        .map(|n| n.2)
        .collect();
    // Eight notes: the plain track's four, as written, and the following
    // track's four, each three degrees up from D: G.
    assert_eq!(heard.iter().filter(|&&k| k == 62).count(), 4);
    assert_eq!(heard.iter().filter(|&&k| k == 67).count(), 4);

    let mut rig = Rig::new(512);
    let id = played(&mut rig.editor, &[], d_dorian(), true);
    region(&mut rig.editor, 0, 4, Scale::default(), 3);
    let scene = rig.editor.document().scenes().next().unwrap().id;
    let slot = Clip::new(
        rig.editor.new_id(),
        bl_core::time::Ticks::ZERO,
        BEAT * 4,
        ClipData::Midi(MidiData::new([Note::new(62, bl_core::time::Ticks::ZERO, BEAT, 100)])),
    );
    rig.editor
        .apply(Command::CreateSlotClip {
            track: id,
            scene,
            clip: slot,
        })
        .unwrap();
    rig.sync();
    rig.sender.session(SessionCommand::Launch { track: id, scene });
    rig.sender.transport(TransportCommand::Play);
    rig.render(2 * BEAT_FRAMES as usize, 64);
    let launched: Vec<u8> = summary(rig.probe.recorded_notes())
        .iter()
        .filter(|n| n.1)
        .map(|n| n.2)
        .collect();
    assert_eq!(launched, [62], "a Session clip plays as written");
}

#[test]
fn the_region_s_scale_is_the_following_track_s_current_scale() {
    // Spec: chord-lane R8. The clip is in C Major; a region names D Dorian and
    // another names no scale. The probe reads the scale on every frame.
    for size in SIZES {
        let mut rig = Rig::new(512);
        let id = track(&mut rig.editor, TrackKind::Midi, None);
        device(&mut rig.editor, id, testing::SCALE, &[]);
        let mut clip = Clip::new(
            rig.editor.new_id(),
            bl_core::time::Ticks::ZERO,
            BEAT * 4,
            ClipData::Midi(MidiData::new([])),
        );
        clip.scale = Scale::new(0, ScaleName::Major);
        rig.editor
            .apply(Command::CreateClip { track: id, clip })
            .unwrap();
        let other = track(&mut rig.editor, TrackKind::Midi, None);
        device(&mut rig.editor, other, testing::SCALE, &[]);
        region(&mut rig.editor, 1, 2, d_dorian(), 0);
        region(&mut rig.editor, 2, 3, Scale::default(), 4);
        rig.editor
            .apply(Command::SetFollowChordLane {
                track: id,
                follow: true,
            })
            .unwrap();
        rig.sync();
        rig.sender.transport(TransportCommand::Play);
        let out = rig.render(4 * BEAT_FRAMES as usize, size);
        let (major, dorian) = (scale_code(&Scale::new(0, ScaleName::Major)), scale_code(&d_dorian()));
        for (frame, want) in [
            (0, major),
            (BEAT_FRAMES - 1, major),
            (BEAT_FRAMES, dorian),
            (2 * BEAT_FRAMES - 1, dorian),
            (2 * BEAT_FRAMES, major),
            (4 * BEAT_FRAMES - 1, major),
        ] {
            assert_eq!(out[frame as usize], want, "block size {size}, frame {frame}");
        }
    }
}

#[test]
fn a_lane_edit_sends_data_and_no_program() {
    // Spec: chord-lane R11. The lane reaches the player as node data, and
    // takes effect in the next block.
    let mut rig = Rig::new(512);
    let id = played(&mut rig.editor, &ON_EACH_BEAT, Scale::default(), true);
    rig.sync();
    rig.sender.transport(TransportCommand::Play);
    rig.render(64, 64);
    let region_id = rig.editor.new_id();
    let synced = rig.apply(Command::AddChordRegion {
        region: ChordRegion::new(region_id, bl_core::time::Ticks::ZERO, BEAT * 4),
    });
    assert!(!synced.program, "a region makes no program");
    let synced = rig.apply(Command::SetChordRegion {
        region: region_id,
        enabled: Some(true),
        root: Some(2),
        name: Some("dorian".to_owned()),
        transpose: Some(7),
    });
    assert!(!synced.program, "a region's scale and transposition make no program");
    let synced = rig.apply(Command::MoveChordRegion {
        region: region_id,
        start: BEAT,
        end: BEAT * 3,
    });
    assert!(!synced.program, "moving a region makes no program");
    let synced = rig.apply(Command::DeleteChordRegion { region: region_id });
    assert!(!synced.program, "deleting a region makes no program");
    let _ = id;
}

#[test]
fn a_moved_note_s_expressions_go_to_the_key_its_note_on_went_to() {
    // Spec: chord-lane R7. The note's pressure runs on past the region's end,
    // through blocks after the one its note-on was in.
    for size in SIZES {
        let mut rig = Rig::new(512);
        let id = played(&mut rig.editor, &[], Scale::default(), true);
        let mut held = Note::new(62, BEAT, BEAT * 2, 100);
        held.pressure = vec![
            Breakpoint {
                at: bl_core::time::Ticks::ZERO,
                value: 20.0,
                bend: 0.0,
            },
            Breakpoint {
                at: BEAT,
                value: 100.0,
                bend: 0.0,
            },
        ];
        let clip = rig.editor.document().track(id).unwrap().clips().next().unwrap().id;
        rig.editor
            .apply(Command::SetClipData {
                clip,
                data: ClipData::Midi(MidiData::new([held])),
            })
            .unwrap();
        region(&mut rig.editor, 1, 2, Scale::default(), 3);
        rig.sync();
        rig.sender.transport(TransportCommand::Play);
        rig.render(4 * BEAT_FRAMES as usize, size);
        let notes = rig.probe.recorded_notes();
        assert!(notes.len() > 2, "the note and its pressure: {}", notes.len());
        assert!(notes.iter().all(|n| n.event.key == 65), "block size {size}");
    }
}

#[test]
fn an_export_moves_its_notes_as_playback_does() {
    // Spec: chord-lane R11, export R1. The offline engine, at another block
    // size, hears the same notes on the same frames.
    let build = |rig_editor: &mut Editor| {
        played(rig_editor, &ON_EACH_BEAT, d_dorian(), true);
        region(rig_editor, 1, 2, Scale::default(), 2);
        region(rig_editor, 2, 4, Scale::default(), -1);
    };
    let mut rig = Rig::new(512);
    build(&mut rig.editor);
    rig.sync();
    let live = heard(&mut rig, 5 * BEAT_FRAMES as usize, 64);

    let mut editor = Editor::new();
    build(&mut editor);
    let probe = std::sync::Arc::new(testing::Probe::default());
    let mut offline = Offline::new(EngineConfig::new(RATE, 480), testing::registry(&probe));
    offline.sync(editor.document());
    offline.sender().transport(TransportCommand::Play);
    offline.render(5 * BEAT_FRAMES as u64, |_| {});
    assert_eq!(offline.heap_operations(), 0);
    assert_eq!(summary(probe.recorded_notes()), live);
}

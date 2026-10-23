//! Probability groups (WP-6.2, note-editing R26 to R29): a Play All group
//! plays all its notes or none, a Play One group one of them, chosen evenly;
//! each pass rolls once, a pass entered part-way chooses among the notes
//! still to play, a relaunched Session clip rolls again, a deactivated
//! member is never chosen, and a render is the same at any block size.
//! Every block runs as a real-time section under the allocation trap.

bl_rt::install_trap!();

mod common;

use bl_core::time::{Bpm, Frames, TICKS_PER_QUARTER, TempoMap, Ticks};
use bl_engine::{NoteKind, SessionCommand, TransportCommand, testing};
use bl_model::{
    Clip, ClipData, Command, GroupKind, LaunchQuantization, LaunchSettings, MidiData, Note,
    NoteGroup, TrackKind,
};
use common::{RATE, Rig, device, track};

/// One pass of the looped clips below, and where its four notes sit.
const BEAT: Ticks = Ticks(TICKS_PER_QUARTER);
const KEYS: [u8; 4] = [60, 61, 62, 63];
const PASSES: i64 = 200;

fn tempo() -> TempoMap {
    // Fast, so two hundred passes are few frames.
    TempoMap::constant(Bpm::new(999.0).unwrap())
}

/// A looped one-beat clip holding a group of four notes, a sixteenth apart,
/// each at `chance`; the keys in `off` are deactivated.
fn grouped(kind: GroupKind, chance: f64, off: &[u8]) -> MidiData {
    let notes = KEYS.iter().enumerate().map(|(place, key)| {
        let mut note = Note::new(*key, BEAT / 4 * place as i64, BEAT / 8, 100);
        note.probability = chance;
        note.mute = off.contains(key);
        note.group = Some(1);
        note
    });
    let mut midi = MidiData::new(notes);
    midi.groups = vec![NoteGroup { id: 1, kind }];
    midi.looping = true;
    midi.loop_end = Some(BEAT);
    midi
}

fn rig_with(midi: MidiData, passes: i64, block: usize) -> Rig {
    let mut rig = Rig::new(block);
    rig.editor
        .apply(Command::SetTempo { tempo: tempo() })
        .unwrap();
    let track = track(&mut rig.editor, TrackKind::Midi, None);
    device(&mut rig.editor, track, testing::INSTRUMENT, &[]);
    let id = rig.editor.new_id();
    rig.editor
        .apply(Command::CreateClip {
            track,
            clip: Clip::new(id, Ticks::ZERO, BEAT * passes, ClipData::Midi(midi)),
        })
        .unwrap();
    rig.sync();
    rig
}

/// The pass and key of every note-on the rig has recorded.
fn played(rig: &mut Rig) -> Vec<(i64, u8)> {
    rig.probe
        .recorded_notes()
        .into_iter()
        .filter(|note| note.event.kind == NoteKind::On)
        .map(|note| {
            let at = tempo().frames_to_ticks(Frames(note.frame), RATE);
            // A frame is a little under a tick at this tempo.
            (
                (at + BEAT / 16).get().div_euclid(BEAT.get()),
                note.event.key,
            )
        })
        .collect()
}

/// Plays `midi` for `passes` passes in blocks of `block`.
fn run(midi: MidiData, passes: i64, block: usize) -> Vec<(i64, u8)> {
    let mut rig = rig_with(midi, passes, block);
    rig.sender.transport(TransportCommand::Play);
    let frames = tempo().ticks_to_frames(BEAT * passes, RATE).get() as usize;
    for done in (0..frames).step_by(block) {
        rig.block(block.min(frames - done));
    }
    played(&mut rig)
}

fn in_pass(notes: &[(i64, u8)], pass: i64) -> Vec<u8> {
    notes
        .iter()
        .filter(|(at, _)| *at == pass)
        .map(|(_, key)| *key)
        .collect()
}

const BLOCKS: [usize; 3] = [1, 64, 512];

// Spec: note-editing R26, R29
#[test]
fn a_play_all_group_at_half_plays_all_or_none_the_same_at_any_block_size() {
    let first = run(grouped(GroupKind::PlayAll, 0.5, &[]), PASSES, BLOCKS[0]);
    let mut whole = 0;
    for pass in 0..PASSES {
        let keys = in_pass(&first, pass);
        assert!(keys.is_empty() || keys == KEYS, "pass {pass}: {keys:?}");
        whole += usize::from(!keys.is_empty());
    }
    assert!((70..=130).contains(&whole), "{whole} of {PASSES} passes");
    for block in &BLOCKS[1..] {
        let other = run(grouped(GroupKind::PlayAll, 0.5, &[]), PASSES, *block);
        assert_eq!(other, first, "blocks of {block}");
    }
}

// Spec: note-editing R27
#[test]
fn a_play_one_group_at_full_chance_plays_one_note_each_pass_evenly() {
    let first = run(grouped(GroupKind::PlayOne, 1.0, &[]), PASSES, BLOCKS[0]);
    for pass in 0..PASSES {
        assert_eq!(in_pass(&first, pass).len(), 1, "pass {pass}");
    }
    for key in KEYS {
        let count = first.iter().filter(|(_, k)| *k == key).count();
        assert!((25..=75).contains(&count), "key {key} played {count} times");
    }
    for block in &BLOCKS[1..] {
        let other = run(grouped(GroupKind::PlayOne, 1.0, &[]), PASSES, *block);
        assert_eq!(other, first, "blocks of {block}");
    }
}

// Spec: note-editing R27. Chance rolls first: a Play One group at half
// plays one note in about half the passes.
#[test]
fn a_play_one_group_at_half_plays_one_or_none() {
    let notes = run(grouped(GroupKind::PlayOne, 0.5, &[]), PASSES, 64);
    let mut played = 0;
    for pass in 0..PASSES {
        let count = in_pass(&notes, pass).len();
        assert!(count <= 1, "pass {pass}");
        played += count;
    }
    assert!((70..=130).contains(&played), "{played} of {PASSES} passes");
}

// Spec: note-editing R26, R27. A pass entered after the group's first note
// chooses only among the notes still to play.
#[test]
fn a_pass_entered_part_way_chooses_among_the_notes_still_to_play() {
    for block in BLOCKS {
        let mut rig = rig_with(grouped(GroupKind::PlayOne, 1.0, &[]), 4, block);
        rig.sender.transport(TransportCommand::Play);
        // Each start lands between the second and third notes' starts, and
        // plays to the end of the beat.
        let into = BEAT / 4 + BEAT / 8;
        let remaining = tempo().ticks_to_frames(BEAT - into, RATE).get() as usize - 1;
        let mut chosen = Vec::new();
        for _ in 0..60 {
            rig.sender.transport(TransportCommand::Locate(into));
            for done in (0..remaining).step_by(block) {
                rig.block(block.min(remaining - done));
            }
            chosen.extend(played(&mut rig).into_iter().map(|(_, key)| key));
        }
        assert_eq!(
            chosen.len(),
            60,
            "one note from each start, blocks of {block}"
        );
        assert!(
            chosen.iter().all(|key| [62, 63].contains(key)),
            "{chosen:?}"
        );
        assert!(chosen.contains(&62) && chosen.contains(&63), "{chosen:?}");
    }
}

// Spec: note-editing R28
#[test]
fn a_deactivated_member_is_never_chosen_and_a_group_of_them_plays_nothing() {
    let notes = run(grouped(GroupKind::PlayOne, 1.0, &[61]), PASSES, 64);
    assert_eq!(notes.len(), PASSES as usize, "one note in every pass");
    for key in [60, 62, 63] {
        let count = notes.iter().filter(|(_, k)| *k == key).count();
        assert!(
            (35..=100).contains(&count),
            "key {key} played {count} times"
        );
    }
    assert!(notes.iter().all(|(_, key)| *key != 61));
    // Play All plays the members that are left, together.
    let notes = run(grouped(GroupKind::PlayAll, 1.0, &[60, 63]), 4, 64);
    assert_eq!(in_pass(&notes, 2), [61, 62]);
    let notes = run(grouped(GroupKind::PlayOne, 1.0, &KEYS), 4, 64);
    assert!(notes.is_empty());
}

// Spec: note-editing R26. Each launch of a Session clip is a pass of its own.
#[test]
fn a_relaunched_session_clip_rolls_again() {
    let mut rig = Rig::new(64);
    rig.editor
        .apply(Command::SetTempo { tempo: tempo() })
        .unwrap();
    let track = track(&mut rig.editor, TrackKind::Midi, None);
    device(&mut rig.editor, track, testing::INSTRUMENT, &[]);
    let scene = rig.editor.document().scenes().next().unwrap().id;
    let mut midi = grouped(GroupKind::PlayOne, 1.0, &[]);
    midi.looping = false;
    midi.loop_end = None;
    let id = rig.editor.new_id();
    rig.editor
        .apply(Command::CreateSlotClip {
            track,
            scene,
            clip: Clip::new(id, Ticks::ZERO, BEAT, ClipData::Midi(midi)),
        })
        .unwrap();
    rig.sync();
    rig.sender
        .session(SessionCommand::SetQuantization(LaunchSettings {
            quantization: LaunchQuantization::None,
            enabled: true,
            ..LaunchSettings::default()
        }));
    rig.sender.transport(TransportCommand::Play);
    let beat = tempo().ticks_to_frames(BEAT, RATE).get() as usize;
    let mut chosen = Vec::new();
    for _ in 0..60 {
        rig.sender.session(SessionCommand::Launch { track, scene });
        for _ in 0..beat.div_ceil(64) {
            rig.block(64);
        }
        chosen.extend(played(&mut rig).into_iter().map(|(_, key)| key));
    }
    assert_eq!(chosen.len(), 60, "one note from each launch: {chosen:?}");
    let distinct: std::collections::BTreeSet<_> = chosen.iter().collect();
    assert!(distinct.len() >= 3, "{chosen:?}");
}

// Spec: note-editing R26, R27. A locate while a Session clip plays doesn't
// start a pass of it: a Play One group keeps the note each launch chose.
#[test]
fn a_locate_during_a_session_clip_does_not_roll_its_groups_again() {
    let mut rig = Rig::new(64);
    rig.editor
        .apply(Command::SetTempo { tempo: tempo() })
        .unwrap();
    let track = track(&mut rig.editor, TrackKind::Midi, None);
    device(&mut rig.editor, track, testing::INSTRUMENT, &[]);
    let scene = rig.editor.document().scenes().next().unwrap().id;
    let mut midi = grouped(GroupKind::PlayOne, 1.0, &[]);
    midi.looping = false;
    midi.loop_end = None;
    let id = rig.editor.new_id();
    rig.editor
        .apply(Command::CreateSlotClip {
            track,
            scene,
            clip: Clip::new(id, Ticks::ZERO, BEAT, ClipData::Midi(midi)),
        })
        .unwrap();
    rig.sync();
    rig.sender
        .session(SessionCommand::SetQuantization(LaunchSettings {
            quantization: LaunchQuantization::None,
            enabled: true,
            ..LaunchSettings::default()
        }));
    rig.sender.transport(TransportCommand::Play);
    let beat = tempo().ticks_to_frames(BEAT, RATE).get() as usize;
    let blocks = beat.div_ceil(64);
    for launch in 0..60 {
        rig.sender.session(SessionCommand::Launch { track, scene });
        for block in 0..blocks {
            if block == blocks / 2 {
                rig.sender
                    .transport(TransportCommand::Locate(BEAT * (launch % 5) as i64));
            }
            rig.block(64);
        }
        // A locate back can play the launch's note again, but never another.
        let keys: std::collections::BTreeSet<u8> =
            played(&mut rig).into_iter().map(|(_, key)| key).collect();
        assert!(keys.len() <= 1, "launch {launch} played {keys:?}");
    }
}

// A render from the player's fixed seed is always the same.
#[test]
fn a_grouped_clip_renders_the_same_notes_from_a_fixed_seed() {
    let notes = run(grouped(GroupKind::PlayOne, 0.5, &[]), 16, 64);
    assert_eq!(
        notes, GOLDEN,
        "the first sixteen passes of a Play One group at half"
    );
}

/// Pass and key of every note the golden render plays.
const GOLDEN: [(i64, u8); 10] = [
    (0, 63),
    (1, 61),
    (2, 63),
    (4, 60),
    (5, 62),
    (8, 62),
    (9, 60),
    (10, 61),
    (13, 60),
    (14, 63),
];

//! MIDI clip playback tests (WP-2.1): timing to the sample at two tempos,
//! across loop wraps, release velocity, probability, velocity deviation,
//! note mute, and no stuck notes on stop, loop wrap, or deletion.
//! Every block runs as a real-time section under the allocation trap.

bl_rt::install_trap!();

mod common;

use bl_core::time::{Bpm, TICKS_PER_QUARTER, TempoMap, Ticks};
use bl_engine::NoteKind;
use bl_engine::TransportCommand;
use bl_engine::testing;
use bl_model::registry as model;
use bl_model::{Clip, ClipData, ClipId, Command, Editor, MidiData, Note, TrackId, TrackKind};
use common::{RATE, Rig, device, track};
use serde_json::json;

const BEAT: Ticks = Ticks(TICKS_PER_QUARTER);
const BAR: Ticks = Ticks(TICKS_PER_QUARTER * 4);

fn midi_track(editor: &mut Editor) -> TrackId {
    track(editor, TrackKind::Midi, None)
}

fn instrument(editor: &mut Editor, track: TrackId) {
    device(editor, track, testing::INSTRUMENT, &[]);
}

fn midi_clip(
    editor: &mut Editor,
    track: TrackId,
    start: Ticks,
    length: Ticks,
    midi: MidiData,
) -> ClipId {
    let id = editor.new_id();
    editor
        .apply(Command::CreateClip {
            track,
            clip: Clip::new(id, start, length, ClipData::Midi(midi)),
        })
        .unwrap();
    id
}

/// Plays `rig` for `frames` frames in blocks of `block_size`.
fn play(rig: &mut Rig, frames: usize, block_size: usize) {
    let mut remaining = frames;
    while remaining > 0 {
        let n = remaining.min(block_size);
        rig.processor.process(n);
        remaining -= n;
    }
}

// Acceptance 1: Note timing to the sample at two tempos (120 BPM and 140 BPM).
#[test]
fn note_timing_to_the_sample_at_two_tempos() {
    // 120 BPM:
    {
        let mut rig = Rig::new(256);
        let trk = midi_track(&mut rig.editor);
        instrument(&mut rig.editor, trk);

        let note = Note::new(60, Ticks::ZERO, BEAT, 100);
        midi_clip(
            &mut rig.editor,
            trk,
            Ticks::ZERO,
            BAR,
            MidiData::new([note]),
        );
        rig.sync();

        rig.sender.transport(TransportCommand::Play);
        // At 120 BPM and 48 kHz: 1 beat = 24,000 frames. 1 bar = 96,000 frames.
        play(&mut rig, 48_000, 256);

        let recorded = rig.probe.recorded_notes();
        assert_eq!(recorded.len(), 2, "Expected NoteOn and NoteOff");
        assert_eq!(recorded[0].frame, 0, "NoteOn must be at frame 0");
        assert_eq!(recorded[0].event.kind, NoteKind::On);
        assert_eq!(recorded[0].event.key, 60);

        assert_eq!(recorded[1].frame, 24_000, "NoteOff must be at frame 24000");
        assert_eq!(recorded[1].event.kind, NoteKind::Off);
        assert_eq!(recorded[1].event.key, 60);
    }

    // 140 BPM:
    {
        let mut rig = Rig::new(256);
        let tempo = TempoMap::constant(Bpm::new(140.0).unwrap());
        rig.editor
            .apply(Command::SetTempo {
                tempo: tempo.clone(),
            })
            .unwrap();

        let trk = midi_track(&mut rig.editor);
        instrument(&mut rig.editor, trk);

        let note = Note::new(64, BEAT, BEAT, 90);
        midi_clip(
            &mut rig.editor,
            trk,
            Ticks::ZERO,
            BAR,
            MidiData::new([note]),
        );
        rig.sync();

        let expected_on = tempo.ticks_to_frames(BEAT, RATE).get();
        let expected_off = tempo.ticks_to_frames(BEAT * 2, RATE).get();

        rig.sender.transport(TransportCommand::Play);
        play(&mut rig, 48_000, 256);

        let recorded = rig.probe.recorded_notes();
        assert_eq!(recorded.len(), 2);
        assert_eq!(recorded[0].frame, expected_on, "NoteOn at 140 BPM");
        assert_eq!(recorded[0].event.kind, NoteKind::On);
        assert_eq!(recorded[1].frame, expected_off, "NoteOff at 140 BPM");
        assert_eq!(recorded[1].event.kind, NoteKind::Off);
    }
}

// Acceptance 2: Note timing across a loop wrap.
// A note spanning across the loop boundary turns off at the wrap frame, and a note
// starting at the beginning of the loop triggers at the wrap frame.
#[test]
fn note_timing_across_a_loop_wrap() {
    let mut rig = Rig::new(256);
    let trk = midi_track(&mut rig.editor);
    instrument(&mut rig.editor, trk);

    // 1-bar loop inside a 2-bar clip.
    // Note 1: pitch 60, starts at BAR - BEAT / 2, length BEAT (extends past BAR loop end).
    // Note 2: pitch 67, starts at 0 ticks, length BEAT.
    let note1 = Note::new(60, BAR - BEAT / 2, BEAT, 100);
    let note2 = Note::new(67, Ticks::ZERO, BEAT, 100);

    let mut midi = MidiData::new([note1, note2]);
    midi.looping = true;
    midi.loop_start = Ticks::ZERO;
    midi.loop_end = Some(BAR);

    midi_clip(&mut rig.editor, trk, Ticks::ZERO, BAR * 2, midi);
    rig.sync();

    let tempo = TempoMap::default();
    let wrap_frame = tempo.ticks_to_frames(BAR, RATE).get();

    rig.sender.transport(TransportCommand::Play);
    // Play for 1.5 bars
    let frames_to_play = tempo.ticks_to_frames(BAR + BEAT * 2, RATE).get() as usize;
    play(&mut rig, frames_to_play, 256);

    let recorded = rig.probe.recorded_notes();
    // In rep 0:
    // - Note 2 (key 67) on at frame 0
    // - Note 2 off at tick 480
    // - Note 1 (key 60) on at tick 1500
    // At loop wrap (tick 1920):
    // - Note 1 off at wrap_frame
    // In rep 1:
    // - Note 2 (key 67) on at wrap_frame
    let wrap_events: Vec<_> = recorded.iter().filter(|n| n.frame == wrap_frame).collect();
    assert_eq!(
        wrap_events.len(),
        2,
        "Expected wrap NoteOff and new NoteOn at wrap_frame"
    );
    assert_eq!(
        wrap_events[0].event.kind,
        NoteKind::Off,
        "NoteOff must precede NoteOn at loop wrap"
    );
    assert_eq!(wrap_events[0].event.key, 60);
    assert_eq!(wrap_events[1].event.kind, NoteKind::On);
    assert_eq!(wrap_events[1].event.key, 67);
}

// Acceptance 3: Release velocity reaches the instrument on NoteOff.
#[test]
fn release_velocity_reaches_the_instrument() {
    let mut rig = Rig::new(256);
    let trk = midi_track(&mut rig.editor);
    instrument(&mut rig.editor, trk);

    let mut note = Note::new(60, Ticks::ZERO, BEAT, 110);
    note.release_velocity = 85;

    midi_clip(
        &mut rig.editor,
        trk,
        Ticks::ZERO,
        BAR,
        MidiData::new([note]),
    );
    rig.sync();

    rig.sender.transport(TransportCommand::Play);
    play(&mut rig, 48_000, 256);

    let recorded = rig.probe.recorded_notes();
    assert_eq!(recorded.len(), 2);
    assert_eq!(recorded[0].event.kind, NoteKind::On);
    assert!((recorded[0].event.velocity - 110.0 / 127.0).abs() < 1e-4);

    assert_eq!(recorded[1].event.kind, NoteKind::Off);
    assert!(
        (recorded[1].event.velocity - 85.0 / 127.0).abs() < 1e-4,
        "Release velocity must reach the instrument on NoteOff"
    );
}

// Acceptance 4: Probability 0 never sounds, 1 always sounds, and 50% plays ~half the time.
#[test]
fn probability_0_1_and_50_percent() {
    let mut rig = Rig::new(256);
    let trk = midi_track(&mut rig.editor);
    instrument(&mut rig.editor, trk);

    let mut note_prob0 = Note::new(50, Ticks::ZERO, BEAT, 100);
    note_prob0.probability = 0.0;

    let mut note_prob1 = Note::new(60, Ticks::ZERO, BEAT, 100);
    note_prob1.probability = 1.0;

    let mut note_prob50 = Note::new(70, Ticks::ZERO, BEAT, 100);
    note_prob50.probability = 0.5;

    let mut midi = MidiData::new([note_prob0, note_prob1, note_prob50]);
    midi.looping = true;
    midi.loop_start = Ticks::ZERO;
    midi.loop_end = Some(BEAT);

    // Loop 100 times
    midi_clip(&mut rig.editor, trk, Ticks::ZERO, BEAT * 100, midi);
    rig.sync();

    rig.sender.transport(TransportCommand::Play);
    let frames_per_beat = 24_000;
    play(&mut rig, frames_per_beat * 100, 256);

    let recorded = rig.probe.recorded_notes();
    let count_prob0 = recorded
        .iter()
        .filter(|n| n.event.kind == NoteKind::On && n.event.key == 50)
        .count();
    let count_prob1 = recorded
        .iter()
        .filter(|n| n.event.kind == NoteKind::On && n.event.key == 60)
        .count();
    let count_prob50 = recorded
        .iter()
        .filter(|n| n.event.kind == NoteKind::On && n.event.key == 70)
        .count();

    assert_eq!(count_prob0, 0, "Probability 0 must never play");
    assert_eq!(count_prob1, 100, "Probability 1 must always play");
    assert!(
        (35..=65).contains(&count_prob50),
        "Probability 0.5 should play roughly half the time over 100 loops (got {count_prob50})"
    );
}

// Acceptance 5: Velocity deviation stays strictly within its range.
#[test]
fn velocity_deviation_stays_in_range() {
    let mut rig = Rig::new(256);
    let trk = midi_track(&mut rig.editor);
    instrument(&mut rig.editor, trk);

    // Positive deviation: 80 with +20 (range 80..=100)
    let mut note_pos = Note::new(60, Ticks::ZERO, BEAT, 80);
    note_pos.velocity_deviation = 20;

    // Negative deviation: 80 with -20 (range 60..=80)
    let mut note_neg = Note::new(72, Ticks::ZERO, BEAT, 80);
    note_neg.velocity_deviation = -20;

    let mut midi = MidiData::new([note_pos, note_neg]);
    midi.looping = true;
    midi.loop_start = Ticks::ZERO;
    midi.loop_end = Some(BEAT);

    midi_clip(&mut rig.editor, trk, Ticks::ZERO, BEAT * 50, midi);
    rig.sync();

    rig.sender.transport(TransportCommand::Play);
    play(&mut rig, 24_000 * 50, 256);

    let recorded = rig.probe.recorded_notes();
    for n in recorded.iter().filter(|n| n.event.kind == NoteKind::On) {
        let midi_vel = (n.event.velocity * 127.0).round() as u8;
        if n.event.key == 60 {
            assert!(
                (80..=100).contains(&midi_vel),
                "Velocity deviation +20 out of range: {midi_vel}"
            );
        } else if n.event.key == 72 {
            assert!(
                (60..=80).contains(&midi_vel),
                "Velocity deviation -20 out of range: {midi_vel}"
            );
        }
    }
}

// Acceptance 6: Muted notes and notes in muted clips never sound.
#[test]
fn muted_notes_and_muted_clips_never_sound() {
    let mut rig = Rig::new(256);
    let trk = midi_track(&mut rig.editor);
    instrument(&mut rig.editor, trk);

    let mut muted_note = Note::new(60, Ticks::ZERO, BEAT, 100);
    muted_note.mute = true;
    let unmuted_note = Note::new(64, Ticks::ZERO, BEAT, 100);

    midi_clip(
        &mut rig.editor,
        trk,
        Ticks::ZERO,
        BAR,
        MidiData::new([muted_note, unmuted_note]),
    );
    rig.sync();

    rig.sender.transport(TransportCommand::Play);
    play(&mut rig, 48_000, 256);

    let recorded = rig.probe.recorded_notes();
    assert!(
        recorded.iter().all(|n| n.event.key != 60),
        "Muted note must never sound"
    );
    assert!(
        recorded.iter().any(|n| n.event.key == 64),
        "Unmuted note should sound"
    );

    // Muted clip
    let mut rig2 = Rig::new(256);
    let trk2 = midi_track(&mut rig2.editor);
    instrument(&mut rig2.editor, trk2);

    let note = Note::new(60, Ticks::ZERO, BEAT, 100);
    let id = midi_clip(
        &mut rig2.editor,
        trk2,
        Ticks::ZERO,
        BAR,
        MidiData::new([note]),
    );
    rig2.apply(Command::SetClipMute {
        clip: id,
        mute: true,
    });
    rig2.sync();

    rig2.sender.transport(TransportCommand::Play);
    play(&mut rig2, 48_000, 256);

    assert!(
        rig2.probe.recorded_notes().is_empty(),
        "Muted clip must never sound"
    );
}

// Acceptance 7: No stuck notes after stop, loop wrap, or clip deletion.
#[test]
fn no_stuck_notes_after_stop_or_clip_deletion() {
    // 1. Stop while note is sounding:
    {
        let mut rig = Rig::new(256);
        let trk = midi_track(&mut rig.editor);
        instrument(&mut rig.editor, trk);

        // Note of 4 bars
        let note = Note::new(60, Ticks::ZERO, BAR * 4, 100);
        midi_clip(
            &mut rig.editor,
            trk,
            Ticks::ZERO,
            BAR * 4,
            MidiData::new([note]),
        );
        rig.sync();

        rig.sender.transport(TransportCommand::Play);
        // Play 1 bar (note is sounding)
        play(&mut rig, 96_000, 256);

        let recorded_mid = rig.probe.recorded_notes();
        assert_eq!(recorded_mid.len(), 1, "Only NoteOn should have fired");
        assert_eq!(recorded_mid[0].event.kind, NoteKind::On);

        // Stop transport:
        rig.sender.transport(TransportCommand::Stop);
        rig.processor.process(256);

        let recorded_after = rig.probe.recorded_notes();
        assert_eq!(
            recorded_after.len(),
            1,
            "NoteOff must be emitted when transport stops"
        );
        assert_eq!(recorded_after[0].event.kind, NoteKind::Off);
        assert_eq!(recorded_after[0].event.key, 60);
    }

    // 2. Delete clip while note is sounding:
    {
        let mut rig = Rig::new(256);
        let trk = midi_track(&mut rig.editor);
        instrument(&mut rig.editor, trk);

        let note = Note::new(65, Ticks::ZERO, BAR * 4, 100);
        let clip_id = midi_clip(
            &mut rig.editor,
            trk,
            Ticks::ZERO,
            BAR * 4,
            MidiData::new([note]),
        );
        rig.sync();

        rig.sender.transport(TransportCommand::Play);
        play(&mut rig, 96_000, 256);

        // Delete clip:
        rig.apply(Command::DeleteClip { clip: clip_id });
        rig.sync();

        // Process next block:
        rig.processor.process(256);

        let recorded = rig.probe.recorded_notes();
        assert_eq!(
            recorded.len(),
            2,
            "NoteOff must be emitted on clip deletion"
        );
        assert_eq!(recorded[1].event.kind, NoteKind::Off);
        assert_eq!(recorded[1].event.key, 65);
    }
}

// Acceptance 8: Set round-trip preserving all MIDI clip properties.
#[test]
fn midi_clip_round_trip() {
    let mut editor = Editor::new();
    let trk = midi_track(&mut editor);

    let mut note = Note::new(62, Ticks(120), Ticks(480), 95);
    note.release_velocity = 75;
    note.probability = 0.8;
    note.velocity_deviation = 15;
    note.mute = false;

    let mut midi = MidiData::new([note]);
    midi.start = Ticks(60);
    midi.end = Some(Ticks(1900));
    midi.looping = true;
    midi.loop_start = Ticks(100);
    midi.loop_end = Some(Ticks(1800));

    let clip_id = midi_clip(&mut editor, trk, Ticks::ZERO, BAR * 2, midi);

    let json_val = serde_json::to_value(editor.document()).unwrap();
    let restored: bl_model::Document = serde_json::from_value(json_val).unwrap();

    let clip = restored.clip(clip_id).unwrap();
    let ClipData::Midi(m) = &clip.data else {
        panic!("Expected Midi clip data");
    };

    assert_eq!(m.start, Ticks(60));
    assert_eq!(m.end, Some(Ticks(1900)));
    assert!(m.looping);
    assert_eq!(m.loop_start, Ticks(100));
    assert_eq!(m.loop_end, Some(Ticks(1800)));
    assert_eq!(m.notes.len(), 1);

    let n = &m.notes[0];
    assert_eq!(n.key, 62);
    assert_eq!(n.start, Ticks(120));
    assert_eq!(n.length, Ticks(480));
    assert_eq!(n.velocity, 95);
    assert_eq!(n.release_velocity, 75);
    assert!((n.probability - 0.8).abs() < 1e-6);
    assert_eq!(n.velocity_deviation, 15);
    assert!(!n.mute);
}

// Acceptance 9: Agent API commands for MIDI clips.
#[test]
fn midi_clip_commands() {
    let mut editor = Editor::new();
    let trk = midi_track(&mut editor);
    let note = Note::new(60, Ticks::ZERO, BEAT, 100);
    let clip_id = midi_clip(
        &mut editor,
        trk,
        Ticks::ZERO,
        BAR * 2,
        MidiData::new([note]),
    );

    // clip.set_midi_region
    editor
        .apply(Command::SetMidiClipRegion {
            clip: clip_id,
            start: Ticks(240),
            end: Some(Ticks(1920)),
        })
        .unwrap();

    // clip.set_midi_loop
    editor
        .apply(Command::SetMidiClipLoop {
            clip: clip_id,
            start: Ticks(480),
            end: Some(Ticks(1920)),
        })
        .unwrap();

    // clip.set_loop_enabled
    editor
        .apply(Command::SetClipLoopEnabled {
            clip: clip_id,
            enabled: true,
        })
        .unwrap();

    let clip = editor.document().clip(clip_id).unwrap();
    let ClipData::Midi(m) = &clip.data else {
        panic!("Expected Midi clip");
    };
    assert_eq!(m.start, Ticks(240));
    assert_eq!(m.end, Some(Ticks(1920)));
    assert_eq!(m.loop_start, Ticks(480));
    assert_eq!(m.loop_end, Some(Ticks(1920)));
    assert!(m.looping);

    // Undo step
    editor.undo().unwrap();
    let clip = editor.document().clip(clip_id).unwrap();
    let ClipData::Midi(m) = &clip.data else {
        panic!("Expected Midi clip");
    };
    assert!(!m.looping, "Undo restored loop switch to false");
}

// Acceptance 10 (WP-2.3 part 3): loop-brace and marker edits play back as
// edited, through the same registered commands the Clip View's strip runs.
#[test]
fn clip_region_and_loop_edits_play_back_as_edited() {
    // The brace: a one-bar loop starting half a bar into a four-bar clip
    // whose notes start at 0 and at 1 bar. The moved brace wraps on its own
    // frames, so the second note sounds again one bar after each wrap.
    let mut rig = Rig::new(256);
    let trk = midi_track(&mut rig.editor);
    instrument(&mut rig.editor, trk);
    let clip = midi_clip(
        &mut rig.editor,
        trk,
        Ticks::ZERO,
        BAR * 4,
        MidiData::new([
            Note::new(60, Ticks::ZERO, BEAT, 100),
            Note::new(67, BAR, BEAT, 100),
        ]),
    );
    model::run(
        &mut rig.editor,
        "clip.set_midi_loop",
        json!({ "clip": clip, "start": BAR / 2, "end": BAR + BEAT * 2 }),
    )
    .unwrap();
    model::run(
        &mut rig.editor,
        "clip.set_loop_enabled",
        json!({ "clip": clip, "enabled": true }),
    )
    .unwrap();
    rig.sync();
    rig.sender.transport(TransportCommand::Play);
    play(&mut rig, 96_000 * 4, 256);
    let ons: Vec<(i64, u8)> = rig
        .probe
        .recorded_notes()
        .iter()
        .filter(|note| note.event.kind == NoteKind::On)
        .map(|note| (note.frame, note.event.key))
        .collect();
    assert_eq!(
        ons,
        [(0, 60), (96_000, 67), (192_000, 67), (288_000, 67)],
        "the notes follow the moved brace: into it at 1.5 bars, then back \
         from 0.5 bars one bar each time"
    );

    // The markers: with the loop off and the End marker at one bar, the
    // note past it never sounds and the first one still does.
    let mut rig = Rig::new(256);
    let trk = midi_track(&mut rig.editor);
    instrument(&mut rig.editor, trk);
    let clip = midi_clip(
        &mut rig.editor,
        trk,
        Ticks::ZERO,
        BAR * 4,
        MidiData::new([
            Note::new(60, Ticks::ZERO, BEAT, 100),
            Note::new(67, BAR + BEAT, BEAT, 100),
        ]),
    );
    model::run(
        &mut rig.editor,
        "clip.set_midi_region",
        json!({ "clip": clip, "start": 0, "end": BAR }),
    )
    .unwrap();
    rig.sync();
    rig.sender.transport(TransportCommand::Play);
    play(&mut rig, 96_000 * 2, 256);
    let ons: Vec<(i64, u8)> = rig
        .probe
        .recorded_notes()
        .iter()
        .filter(|note| note.event.kind == NoteKind::On)
        .map(|note| (note.frame, note.event.key))
        .collect();
    assert_eq!(ons, [(0, 60)], "the End marker silences the note past it");
}

/// The notes a clip with a C#4 in it plays, with a scale on the clip or
/// off.
fn played_with_a_scale(scale: bool) -> Vec<(i64, u8)> {
    let mut rig = Rig::new(256);
    let trk = midi_track(&mut rig.editor);
    instrument(&mut rig.editor, trk);
    let clip = midi_clip(
        &mut rig.editor,
        trk,
        Ticks::ZERO,
        BAR,
        MidiData::new([Note::new(61, Ticks::ZERO, BEAT, 100)]),
    );
    if scale {
        // Spec: scales R5. The clip's scale, which nothing in playback
        // reads yet.
        rig.editor
            .apply(Command::SetClipScale {
                clips: vec![clip],
                enabled: Some(true),
                root: Some(0),
                name: Some("major".to_owned()),
            })
            .unwrap();
    }
    rig.sync();
    rig.sender.transport(TransportCommand::Play);
    play(&mut rig, 48_000, 256);
    rig.probe
        .recorded_notes()
        .iter()
        .map(|note| (note.frame, note.event.key))
        .collect()
}

// Spec: scales R4.
#[test]
fn a_clip_s_scale_doesnt_change_what_it_plays() {
    let without = played_with_a_scale(false);
    assert_eq!(without, [(0, 61), (24_000, 61)], "the note plays as it is");
    let with = played_with_a_scale(true);
    assert_eq!(
        with, without,
        "the clip's scale doesn't change what it plays"
    );
}

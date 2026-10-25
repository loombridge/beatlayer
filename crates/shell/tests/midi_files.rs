//! Importing and exporting Standard MIDI Files (WP-2.21), as the behaviour
//! behind `midi.import_file` and `midi.export_clip`.

use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use bl_core::time::{Bpm, TICKS_PER_QUARTER, Ticks, TimeSignature};
use bl_interop::midi;
use bl_model::{ClipData, Command, Editor, TrackKind};
use bl_shell::midi_files::{self, MidiFileError};
use midly::num::{u4, u7, u15, u24, u28};
use midly::{
    Format, Header, MetaMessage, MidiMessage, Smf, Timing, Track, TrackEvent, TrackEventKind,
};

/// The ticks to the quarter note the generated files use.
const PPQ: u16 = 96;

/// A folder in the system's temporary folder, removed when dropped.
struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let path = std::env::temp_dir().join(format!(
            "bl-shell-midi-{name}-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("a temporary folder");
        Self(path)
    }

    fn file(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.0.join(name);
        fs::write(&path, bytes).expect("the file is written");
        path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A note in a generated file: start and length in the file's ticks, key, and
/// velocity.
struct Gen {
    start: u32,
    length: u32,
    key: u8,
    velocity: u8,
}

/// A meta event in a generated file.
enum Meta {
    /// Microseconds per quarter note.
    Tempo(u32),
    /// A time signature's numerator and denominator.
    Signature(u8, u8),
}

/// A track the generator builds: its name, its notes, and its meta events.
type GenTrack<'a> = (&'static str, &'a [Gen], &'a [(u32, Meta)]);

/// Builds a Standard MIDI File from tracks.
fn generate(format: Format, tracks: &[GenTrack<'_>]) -> Vec<u8> {
    let mut built = Vec::new();
    for (name, notes, metas) in tracks {
        let mut events: Vec<(u32, u8, TrackEventKind<'_>)> = Vec::new();
        events.push((
            0,
            0,
            TrackEventKind::Meta(MetaMessage::TrackName(name.as_bytes())),
        ));
        for (at, meta) in *metas {
            let message = match *meta {
                Meta::Tempo(micros) => MetaMessage::Tempo(u24::new(micros)),
                Meta::Signature(numerator, denominator) => {
                    let mut power = 0;
                    while (1_u16 << power) < u16::from(denominator) {
                        power += 1;
                    }
                    MetaMessage::TimeSignature(numerator, power, 24, 8)
                }
            };
            events.push((*at, 0, TrackEventKind::Meta(message)));
        }
        for note in *notes {
            events.push((
                note.start,
                2,
                TrackEventKind::Midi {
                    channel: u4::new(0),
                    message: MidiMessage::NoteOn {
                        key: u7::new(note.key),
                        vel: u7::new(note.velocity),
                    },
                },
            ));
            events.push((
                note.start + note.length,
                1,
                TrackEventKind::Midi {
                    channel: u4::new(0),
                    message: MidiMessage::NoteOff {
                        key: u7::new(note.key),
                        vel: u7::new(64),
                    },
                },
            ));
        }
        let end = notes
            .iter()
            .map(|note| note.start + note.length)
            .max()
            .unwrap_or(0);
        events.push((end, 3, TrackEventKind::Meta(MetaMessage::EndOfTrack)));
        events.sort_by_key(|(tick, order, _)| (*tick, *order));
        let mut track = Track::new();
        let mut previous = 0;
        for (tick, _, kind) in events {
            track.push(TrackEvent {
                delta: u28::new(tick - previous),
                kind,
            });
            previous = tick;
        }
        built.push(track);
    }
    let smf = Smf {
        header: Header::new(format, Timing::Metrical(u15::new(PPQ))),
        tracks: built,
    };
    let mut bytes = Vec::new();
    smf.write_std(&mut bytes).unwrap();
    bytes
}

/// A position in Beatlayer ticks, from the file's ticks at [`PPQ`].
fn at(units: u32) -> Ticks {
    Ticks(i64::from(units) * TICKS_PER_QUARTER / i64::from(PPQ))
}

fn note(start: u32, length: u32, key: u8, velocity: u8) -> Gen {
    Gen {
        start,
        length,
        key,
        velocity,
    }
}

fn midi_clip(editor: &Editor, id: bl_model::ClipId) -> &bl_model::MidiData {
    let clip = editor.document().clip(id).expect("the clip");
    let ClipData::Midi(data) = &clip.data else {
        panic!("a MIDI clip");
    };
    data
}

// Spec: scales R7. An imported MIDI file makes a clip from nothing, so it
// takes the set's current scale.
#[test]
fn an_imported_clip_takes_the_current_scale() {
    let dir = TempDir::new("import-scale");
    let bytes = generate(
        Format::SingleTrack,
        &[("Bass", &[note(0, 96, 36, 100)], &[])],
    );
    let path = dir.file("Bass.mid", &bytes);
    let mut editor = Editor::new();
    editor
        .apply(Command::SetScale {
            enabled: Some(true),
            root: Some(10),
            name: Some("minor".to_owned()),
        })
        .unwrap();
    let result = midi_files::import(&mut editor, &path, None, Ticks::ZERO, false).unwrap();
    assert_eq!(
        editor.document().clip(result.clips[0].clip).unwrap().scale,
        bl_model::Scale::new(10, bl_model::ScaleName::Minor),
        "the imported clip takes the current scale"
    );
}

// Spec: midi-clips R16 (a type 0 file becomes a new MIDI track with one clip).
#[test]
fn a_type_0_file_becomes_one_new_midi_track() {
    let dir = TempDir::new("type0");
    let bytes = generate(
        Format::SingleTrack,
        &[("Bass", &[note(0, 96, 36, 100)], &[])],
    );
    let path = dir.file("Bass.mid", &bytes);
    let mut editor = Editor::new();

    let result = midi_files::import(&mut editor, &path, None, Ticks::ZERO, false).unwrap();
    assert_eq!(result.created_tracks.len(), 1);
    assert_eq!(result.clips.len(), 1);
    let track = result.created_tracks[0];
    assert_eq!(editor.document().track(track).unwrap().name(), "1 MIDI");
    let placed = &result.clips[0];
    assert_eq!(placed.name, "Bass");
    assert_eq!(placed.start, Ticks::ZERO);
    assert_eq!(placed.length, Ticks::WHOLE, "one 4/4 bar");
    // Spec: midi-clips R17. The clip's Signature, End and loop.
    let data = midi_clip(&editor, placed.clip);
    assert_eq!(data.notes.len(), 1);
    assert_eq!(data.notes[0].key, 36);
    assert_eq!(data.signature, Some(TimeSignature::FOUR_FOUR));
    assert!(data.looping);
    assert_eq!(data.end, Some(Ticks::WHOLE));
    assert_eq!(data.loop_end, Some(Ticks::WHOLE));
    assert_eq!(editor.undo_history().next(), Some(midi_files::UNDO_LABEL));
}

// Spec: midi-clips R16 (a clip per type 1 track with notes, in order, on new
// MIDI tracks; chunks with no notes make nothing).
#[test]
fn a_type_1_file_makes_a_track_per_chunk_with_notes() {
    let dir = TempDir::new("type1");
    let bytes = generate(
        Format::Parallel,
        &[
            ("Conductor", &[], &[(0, Meta::Tempo(500_000))]),
            ("Lead", &[note(0, 96, 60, 90)], &[]),
            ("Pad", &[note(96, 96, 67, 80)], &[]),
        ],
    );
    let path = dir.file("Song.mid", &bytes);
    let mut editor = Editor::new();

    let result = midi_files::import(&mut editor, &path, None, Ticks::ZERO, false).unwrap();
    assert_eq!(result.created_tracks.len(), 2, "the conductor makes none");
    let names: Vec<String> = result
        .created_tracks
        .iter()
        .map(|t| editor.document().track(*t).unwrap().name().to_owned())
        .collect();
    assert_eq!(names, ["1 MIDI", "2 MIDI"]);
    let clips: Vec<&str> = result.clips.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(clips, ["Lead", "Pad"]);
    // A clip's start is the drop position; its contents keep the file's
    // leading silence.
    assert!(result.clips.iter().all(|c| c.start == Ticks::ZERO));
    let pad = midi_clip(&editor, result.clips[1].clip);
    assert_eq!(pad.notes[0].start, at(96), "leading silence stays");
}

// Spec: midi-clips R16 (a file with one track of notes goes on a given MIDI
// track; several can't).
#[test]
fn a_clip_goes_on_a_given_midi_track_and_a_multi_chunk_file_is_refused() {
    let dir = TempDir::new("target");
    let one = generate(
        Format::SingleTrack,
        &[("Bass", &[note(0, 96, 36, 100)], &[])],
    );
    let path = dir.file("Bass.mid", &one);
    let mut editor = Editor::new();
    let track: bl_model::TrackId = editor.new_id();
    editor
        .apply(Command::CreateTrack {
            id: track,
            kind: TrackKind::Midi,
            name: "Bass".to_owned(),
            parent: None,
            index: 0,
        })
        .unwrap();

    let result =
        midi_files::import(&mut editor, &path, Some(track), Ticks(705_600), false).unwrap();
    assert!(result.created_tracks.is_empty());
    assert_eq!(result.clips[0].track, track);
    assert_eq!(result.clips[0].start, Ticks(705_600));
    assert_eq!(editor.document().track(track).unwrap().name(), "Bass");

    let two = generate(
        Format::Parallel,
        &[
            ("A", &[note(0, 96, 60, 90)], &[]),
            ("B", &[note(0, 96, 64, 90)], &[]),
        ],
    );
    let many = dir.file("Many.mid", &two);
    let error = midi_files::import(&mut editor, &many, Some(track), Ticks::ZERO, false);
    assert!(
        matches!(&error, Err(MidiFileError::InvalidArgs(message)) if message.contains("leave `track` out")),
        "{error:?}"
    );
}

// Spec: midi-clips R18 (a file with one tempo sets the song's tempo).
#[test]
fn import_tempo_sets_the_song_tempo_for_a_single_tempo_file() {
    let dir = TempDir::new("onetempo");
    let bytes = generate(
        Format::Parallel,
        &[
            ("Conductor", &[], &[(0, Meta::Tempo(600_000))]),
            ("Keys", &[note(0, 96, 60, 90)], &[]),
        ],
    );
    let path = dir.file("Slow.mid", &bytes);
    let mut editor = Editor::new();
    editor
        .apply(Command::SetTempo {
            tempo: bl_core::time::TempoMap::constant(Bpm::new(140.0).unwrap()),
        })
        .unwrap();

    midi_files::import(&mut editor, &path, None, Ticks::ZERO, true).unwrap();
    let tempo = &editor.document().settings().tempo;
    assert_eq!(tempo.segments().count(), 1, "no automation is written");
    assert_eq!(tempo.bpm_at(Ticks::ZERO), Bpm::new(100.0).unwrap());
    assert_eq!(tempo.bpm_at(Ticks::WHOLE * 4), Bpm::new(100.0).unwrap());
}

// Spec: midi-clips R18 (a file whose tempo changes writes Song Tempo
// automation over the clip and restores the tempo at its end).
#[test]
fn import_tempo_writes_automation_for_a_changing_tempo() {
    let dir = TempDir::new("twotempo");
    // 120 bpm, 140 from the file's third quarter note, into a song at 100.
    let bytes = generate(
        Format::Parallel,
        &[
            (
                "Conductor",
                &[],
                &[(0, Meta::Tempo(500_000)), (192, Meta::Tempo(428_571))],
            ),
            ("Keys", &[note(0, 384, 60, 90)], &[]),
        ],
    );
    let path = dir.file("Change.mid", &bytes);
    let mut editor = Editor::new();
    editor
        .apply(Command::SetTempo {
            tempo: bl_core::time::TempoMap::constant(Bpm::new(100.0).unwrap()),
        })
        .unwrap();

    let result = midi_files::import(&mut editor, &path, None, Ticks::ZERO, true).unwrap();
    let end = result.clips[0].length;
    let tempo = &editor.document().settings().tempo;
    assert_eq!(tempo.bpm_at(Ticks::ZERO), Bpm::new(120.0).unwrap());
    let fast = tempo.bpm_at(at(192));
    assert!((fast.get() - 140.0).abs() < 0.05, "{}", fast.get());
    assert_eq!(
        tempo.bpm_at(end),
        Bpm::new(100.0).unwrap(),
        "the tempo before the clip is restored at its end"
    );
}

// Spec: midi-clips R18 (a time-signature marker at the clip's start and one
// restoring the previous signature at its end, and the clip's own Signature).
#[test]
fn import_tempo_writes_time_signature_markers() {
    let dir = TempDir::new("signature");
    let bytes = generate(
        Format::SingleTrack,
        &[(
            "Seven",
            &[note(0, 96, 60, 90)],
            &[(0, Meta::Signature(7, 8))],
        )],
    );
    let path = dir.file("Seven.mid", &bytes);
    let mut editor = Editor::new();

    let result = midi_files::import(&mut editor, &path, None, Ticks::ZERO, true).unwrap();
    let seven_eight = TimeSignature::new(7, 8).unwrap();
    let signatures = &editor.document().settings().time_signatures;
    assert_eq!(signatures.signature_at(Ticks::ZERO), seven_eight);
    assert_eq!(
        signatures.signature_at(result.clips[0].length),
        TimeSignature::FOUR_FOUR
    );
    assert_eq!(
        midi_clip(&editor, result.clips[0].clip).signature,
        Some(seven_eight)
    );

    // Without `import_tempo` the song is untouched, but the clip still keeps
    // the file's signature.
    let mut plain = Editor::new();
    let result = midi_files::import(&mut plain, &path, None, Ticks::ZERO, false).unwrap();
    assert_eq!(
        plain
            .document()
            .settings()
            .time_signatures
            .signature_at(Ticks::ZERO),
        TimeSignature::FOUR_FOUR
    );
    assert_eq!(
        midi_clip(&plain, result.clips[0].clip).signature,
        Some(seven_eight)
    );
}

// Spec: midi-clips R15, R19 (export, import keeps every note, and the file
// carries the clip's tempo and signature).
#[test]
fn a_clip_round_trips_through_export_and_import() {
    let dir = TempDir::new("round");
    let bytes = generate(
        Format::SingleTrack,
        &[(
            "Bass",
            &[
                note(0, 96, 36, 100),
                note(192, 48, 43, 72),
                note(288, 96, 48, 1),
            ],
            &[(0, Meta::Signature(7, 8))],
        )],
    );
    let source = dir.file("Bass.mid", &bytes);
    let mut editor = Editor::new();
    editor
        .apply(Command::SetTempo {
            tempo: bl_core::time::TempoMap::constant(Bpm::new(97.5).unwrap()),
        })
        .unwrap();
    let imported = midi_files::import(&mut editor, &source, None, Ticks::ZERO, false).unwrap();
    let original = midi_clip(&editor, imported.clips[0].clip).notes.clone();

    let out = dir.0.join("Out.mid");
    let args = bl_shell_spec::midi_files::MidiExportArgs {
        clip: imported.clips[0].clip,
        path: Some(out.to_string_lossy().into_owned()),
        replace: false,
    };
    let exported = midi_files::export(editor.document(), &args).unwrap();
    assert_eq!(exported.notes, 3);
    assert!((exported.tempo - 97.5).abs() < 0.01);
    assert_eq!(exported.signature, "7/8");

    // Read the file back: every note's pitch, start, length, and velocity is
    // kept, and so are the tempo and signature.
    let read = midi::read(&out).unwrap();
    assert_eq!(read.tracks.len(), 1);
    let mut back = read.tracks[0].notes.clone();
    back.sort_by_key(|n| (n.start, n.key));
    let mut expected: Vec<bl_model::Note> = original.iter().cloned().collect();
    expected.sort_by_key(|n| (n.start, n.key));
    assert_eq!(back, expected);
    assert!((read.tempo.bpm_at(Ticks::ZERO).get() - 97.5).abs() < 0.01);
    assert_eq!(
        read.time_signatures.signature_at(Ticks::ZERO),
        TimeSignature::new(7, 8).unwrap()
    );

    // An existing file is refused without `replace`, and an audio clip is
    // never written.
    assert!(matches!(
        midi_files::export(editor.document(), &args),
        Err(MidiFileError::Failed(_))
    ));
    let mut replace = args.clone();
    replace.replace = true;
    assert!(midi_files::export(editor.document(), &replace).is_ok());
}

// Spec: midi-clips R16 (a file that isn't a MIDI file fails without changing
// the set).
#[test]
fn a_bad_file_changes_nothing() {
    let dir = TempDir::new("bad");
    let path = dir.file("not.mid", b"this is not a MIDI file");
    let mut editor = Editor::new();
    let before = editor.document().clone();
    assert!(matches!(
        midi_files::import(&mut editor, &path, None, Ticks::ZERO, false),
        Err(MidiFileError::Failed(_))
    ));
    assert_eq!(editor.document(), &before);
    assert_eq!(editor.undo_history().count(), 0);
}

// Spec: arrangement-editing R6, midi-clips R16 (a clip placed over another
// replaces what it covers, as a drop from the browser does, so a track never
// plays two clips at once).
#[test]
fn an_import_over_a_clip_replaces_what_it_covers() {
    let dir = TempDir::new("cover");
    let bytes = generate(
        Format::SingleTrack,
        &[("Bass", &[note(0, 96, 36, 100)], &[])],
    );
    let path = dir.file("Bass.mid", &bytes);
    let mut editor = Editor::new();
    let track: bl_model::TrackId = editor.new_id();
    editor
        .apply(Command::CreateTrack {
            id: track,
            kind: TrackKind::Midi,
            name: "Bass".to_owned(),
            parent: None,
            index: 0,
        })
        .unwrap();
    // A clip on bars 1 to 4, so the import at bar 2 lands in its middle.
    let held: bl_model::ClipId = editor.new_id();
    let mut clip = bl_model::Clip::new(
        held,
        Ticks::ZERO,
        Ticks::WHOLE * 3,
        ClipData::Midi(bl_model::MidiData::new(std::iter::empty())),
    );
    clip.name = "Held".to_owned();
    editor.apply(Command::CreateClip { track, clip }).unwrap();

    let result = midi_files::import(&mut editor, &path, Some(track), Ticks::WHOLE, false).unwrap();
    assert_eq!(result.clips.len(), 1);
    let clips: Vec<(i64, i64, String)> = {
        let mut clips: Vec<_> = editor
            .document()
            .track(track)
            .unwrap()
            .clips()
            .map(|clip| (clip.start.get(), clip.end().get(), clip.name.clone()))
            .collect();
        clips.sort_unstable();
        clips
    };
    // The covered middle is gone: what's left is the part before, the
    // import, and the part after, none of them overlapping.
    assert_eq!(
        clips,
        [
            (0, Ticks::WHOLE.get(), "Held".to_owned()),
            (
                Ticks::WHOLE.get(),
                2 * Ticks::WHOLE.get(),
                "Bass".to_owned()
            ),
            (
                2 * Ticks::WHOLE.get(),
                3 * Ticks::WHOLE.get(),
                "Held".to_owned()
            ),
        ]
    );
    assert!(clips.windows(2).all(|pair| pair[0].1 <= pair[1].0));
    // The import is the clip in the middle, and the held clip's left part
    // still has its ID.
    let placed = editor.document().clip(result.clips[0].clip).unwrap();
    assert_eq!(
        (placed.start.get(), placed.end().get()),
        (Ticks::WHOLE.get(), 2 * Ticks::WHOLE.get())
    );
    assert!(editor.document().clip(held).is_some());
}

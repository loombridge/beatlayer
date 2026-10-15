//! Reading and writing Standard MIDI Files (WP-2.21).

use bl_core::time::{Bpm, TICKS_PER_QUARTER, Ticks};
use bl_interop::midi::{self, FileType};
use bl_model::Note;
use midly::num::{u4, u7, u15, u24, u28};
use midly::{Format, Header, MetaMessage, MidiMessage, Smf, Timing, Track, TrackEvent, TrackEventKind};

/// The ticks to the quarter note the generated files use: a common value,
/// not Beatlayer's, so the conversion is exercised.
const PPQ: u16 = 96;

/// A note in a generated file: start and length in the file's ticks, key,
/// and velocity.
struct Gen {
    start: u32,
    length: u32,
    key: u8,
    velocity: u8,
}

/// Builds a file of the given format: each track is a name and its notes.
fn generate(format: Format, tracks: &[(&'static str, &[Gen])]) -> Vec<u8> {
    let mut built = Vec::new();
    for (name, notes) in tracks {
        let mut events: Vec<(u32, u8, TrackEventKind<'_>)> = Vec::new();
        events.push((
            0,
            0,
            TrackEventKind::Meta(MetaMessage::TrackName(name.as_bytes())),
        ));
        if *name == "Conductor" {
            events.push((
                0,
                0,
                TrackEventKind::Meta(MetaMessage::Tempo(u24::new(500_000))),
            ));
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

// Spec: midi-clips R12 (a type 0 file makes one clip).
#[test]
fn a_type_0_file_reads_as_one_track_of_notes() {
    let bytes = generate(
        Format::SingleTrack,
        &[(
            "Bass",
            &[
                Gen {
                    start: 0,
                    length: 48,
                    key: 36,
                    velocity: 100,
                },
                Gen {
                    start: 96,
                    length: 24,
                    key: 43,
                    velocity: 72,
                },
            ],
        )],
    );
    let imported = midi::read_bytes(&bytes).unwrap();
    assert_eq!(imported.format, FileType::SingleTrack);
    assert_eq!(imported.tracks.len(), 1);
    let track = &imported.tracks[0];
    assert_eq!(track.name, "Bass");
    assert_eq!(track.notes.len(), 2);
    assert_eq!(
        (track.notes[0].key, track.notes[0].start, track.notes[0].length, track.notes[0].velocity),
        (36, at(0), at(48), 100)
    );
    assert_eq!(
        (track.notes[1].key, track.notes[1].start, track.notes[1].length, track.notes[1].velocity),
        (43, at(96), at(24), 72)
    );
    assert_eq!(track.length, at(120));
}

// Spec: midi-clips R12 (a type 1 file makes a clip per track, in order).
#[test]
fn a_type_1_file_reads_as_a_track_each() {
    let bytes = generate(
        Format::Parallel,
        &[
            ("Conductor", &[]),
            (
                "Keys",
                &[Gen {
                    start: 96,
                    length: 96,
                    key: 60,
                    velocity: 88,
                }],
            ),
        ],
    );
    let imported = midi::read_bytes(&bytes).unwrap();
    assert_eq!(imported.format, FileType::Parallel);
    assert_eq!(imported.tracks.len(), 2);
    assert!(imported.tracks[0].notes.is_empty());
    assert_eq!(imported.tracks[1].name, "Keys");
    assert_eq!(imported.tracks[1].notes.len(), 1);
    assert_eq!(imported.tracks[1].notes[0].key, 60);
    assert_eq!(imported.tracks[1].notes[0].start, at(96));
    // The file's tempo is read, but the caller decides whether to apply it.
    assert_eq!(imported.tempo, Some(Bpm::new(120.0).unwrap()));
}

// Spec: midi-clips R13 (a note-on with velocity 0 ends a note; one left open
// ends at the track's end).
#[test]
fn note_on_with_velocity_zero_and_unclosed_notes_close() {
    let track: Track<'_> = vec![
        TrackEvent {
            delta: u28::new(0),
            kind: TrackEventKind::Midi {
                channel: u4::new(0),
                message: MidiMessage::NoteOn {
                    key: u7::new(64),
                    vel: u7::new(90),
                },
            },
        },
        TrackEvent {
            delta: u28::new(48),
            kind: TrackEventKind::Midi {
                channel: u4::new(0),
                message: MidiMessage::NoteOn {
                    key: u7::new(64),
                    vel: u7::new(0),
                },
            },
        },
        TrackEvent {
            delta: u28::new(0),
            kind: TrackEventKind::Midi {
                channel: u4::new(0),
                message: MidiMessage::NoteOn {
                    key: u7::new(67),
                    vel: u7::new(70),
                },
            },
        },
        TrackEvent {
            delta: u28::new(12),
            kind: TrackEventKind::Meta(MetaMessage::EndOfTrack),
        },
    ];
    let smf = Smf {
        header: Header::new(Format::SingleTrack, Timing::Metrical(u15::new(PPQ))),
        tracks: vec![track],
    };
    let mut bytes = Vec::new();
    smf.write_std(&mut bytes).unwrap();

    let imported = midi::read_bytes(&bytes).unwrap();
    let notes = &imported.tracks[0].notes;
    assert_eq!(notes.len(), 2);
    assert_eq!((notes[0].key, notes[0].length), (64, at(48)));
    assert_eq!((notes[1].key, notes[1].start, notes[1].length), (67, at(48), at(12)));
}

// Spec: midi-clips R14 (writing a clip out and reading it back keeps every
// note's pitch, start, length, and velocity).
#[test]
fn a_clip_round_trips_through_a_written_file() {
    let notes = vec![
        Note::new(36, Ticks::ZERO, Ticks::from_quarters(1), 100),
        Note::new(38, Ticks::from_quarters(1), Ticks::from_quarters(1), 64),
        // A triplet, a 64th note, and a step just off the quarter, so the
        // grids Beatlayer edits on are all written and read back exactly.
        Note::new(43, Ticks::from_quarters(2), Ticks(235_200), 127),
        Note::new(60, Ticks::from_quarters(3), Ticks(11_025), 1),
    ];
    let bytes = midi::write_bytes("Clip", &notes, None).unwrap();
    let imported = midi::read_bytes(&bytes).unwrap();
    assert_eq!(imported.format, FileType::Parallel);
    // Writing is a type 1 file: one track, holding the clip's notes.
    assert_eq!(imported.tracks.len(), 1);
    let mut read = imported.tracks[0].notes.clone();
    read.sort_by_key(|note| (note.start, note.key));
    let mut expected = notes.clone();
    expected.sort_by_key(|note| (note.start, note.key));
    assert_eq!(read, expected);
    // With no tempo in it, the file holds none; a reader uses 120 bpm.
    assert_eq!(imported.tempo, None);
}

// Spec: midi-clips R15 (the file carries the clip's tempo when it is given).
#[test]
fn a_written_file_carries_the_tempo_it_is_given() {
    let notes = vec![Note::new(60, Ticks::ZERO, Ticks::from_quarters(1), 100)];
    let bytes = midi::write_bytes("Clip", &notes, Some(Bpm::new(90.0).unwrap())).unwrap();
    let imported = midi::read_bytes(&bytes).unwrap();
    let tempo = imported.tempo.expect("a tempo");
    assert!((tempo.get() - 90.0).abs() < 0.01, "{}", tempo.get());
}

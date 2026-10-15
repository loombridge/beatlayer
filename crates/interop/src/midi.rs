//! Standard MIDI Files (WP-2.21): reading a `.mid` into tracks of notes and
//! writing notes out as a type 1 file.
//!
//! A file's notes are read from its note-on and note-off messages: a note-on
//! with velocity 0 counts as a note-off, as the standard says, a note-off
//! whose velocity is 0 or missing takes the default release velocity (64),
//! and a note left open at the end of a track is closed there. Positions are
//! converted from the file's own ticks per quarter note to Beatlayer's, so a
//! file plays at the song's tempo like any clip: a MIDI file's own tempo is
//! read but not applied.
//!
//! Writing uses the file's whole length, not its tempo: the notes go out as a
//! type 1 file at [`DIVISION`] ticks to the quarter note.

use std::fmt;
use std::path::Path;

use bl_core::time::{Bpm, TICKS_PER_QUARTER, Ticks};
use bl_model::Note;
use midly::num::{u4, u7, u15, u24, u28};
use midly::{Format, Header, MetaMessage, MidiMessage, Smf, Timing, Track, TrackEvent, TrackEventKind};

/// The extensions of the MIDI files Beatlayer opens, lower-case.
pub const EXTENSIONS: &[&str] = &["mid", "midi"];

/// The ticks to the quarter note Beatlayer writes into a file: the largest
/// number a Standard MIDI File's header can hold (a 15-bit field) that
/// divides [`TICKS_PER_QUARTER`]. It is finer than any note grid Beatlayer
/// edits on, so a clip written and read back keeps every note's position and
/// length exactly.
pub const DIVISION: u16 = 28_224;

/// The file a format is, for reading: Standard MIDI File type 0, 1, or 2.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileType {
    /// Type 0: one track holding every channel.
    SingleTrack,
    /// Type 1: several tracks playing together.
    Parallel,
    /// Type 2: several independent tracks.
    Sequential,
}

/// One track of a file: its name, its notes from the start of the file, and
/// how long its contents run.
#[derive(Clone, Debug, PartialEq)]
pub struct ImportedTrack {
    /// The track's name, from its Track Name event, or empty.
    pub name: String,
    /// Its notes, ordered by start then key.
    pub notes: Vec<Note>,
    /// Where its contents end: the end of its last note.
    pub length: Ticks,
}

/// What a file holds.
#[derive(Clone, Debug, PartialEq)]
pub struct Imported {
    /// Which kind of file it is.
    pub format: FileType,
    /// One entry per track chunk, in the file's order, including chunks with
    /// no notes.
    pub tracks: Vec<ImportedTrack>,
    /// The file's first tempo, if it carries one and it is a tempo Beatlayer
    /// can hold (the first tempo event wins, as in the standard).
    pub tempo: Option<Bpm>,
}

/// Why a MIDI file couldn't be read or written.
#[derive(Debug)]
#[non_exhaustive]
pub enum MidiFileError {
    /// The file couldn't be read or written.
    Io(std::io::Error),
    /// The bytes aren't a Standard MIDI File Beatlayer reads.
    Parse(midly::Error),
    /// A number in the file doesn't fit Beatlayer's time.
    OutOfRange,
}

impl fmt::Display for MidiFileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => error.fmt(f),
            Self::Parse(error) => write!(f, "not a MIDI file Beatlayer reads: {error}"),
            Self::OutOfRange => f.write_str("a number in the file is out of range"),
        }
    }
}

impl std::error::Error for MidiFileError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Parse(error) => Some(error),
            Self::OutOfRange => None,
        }
    }
}

impl From<std::io::Error> for MidiFileError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

/// Reads the Standard MIDI File at `path`.
///
/// # Errors
///
/// [`MidiFileError`] if the file can't be read or isn't a Standard MIDI File
/// with a metrical (ticks per quarter note) time division. Timecode timing,
/// which no sequencer writes for note data, is refused.
pub fn read(path: &Path) -> Result<Imported, MidiFileError> {
    let bytes = std::fs::read(path)?;
    read_bytes(&bytes)
}

/// Reads a Standard MIDI File from its bytes, as [`read`] does.
///
/// # Errors
///
/// [`MidiFileError`] if the bytes aren't a Standard MIDI File with a metrical
/// time division.
pub fn read_bytes(bytes: &[u8]) -> Result<Imported, MidiFileError> {
    let smf = Smf::parse(bytes).map_err(MidiFileError::Parse)?;
    let division = match smf.header.timing {
        Timing::Metrical(division) if division.as_int() > 0 => i64::from(division.as_int()),
        // Timecode timing counts frames, not quarter notes, so no sequencer
        // writes note tracks with it; refusing is clearer than guessing.
        Timing::Metrical(_) | Timing::Timecode(..) => return Err(MidiFileError::OutOfRange),
    };
    let format = match smf.header.format {
        Format::SingleTrack => FileType::SingleTrack,
        Format::Parallel => FileType::Parallel,
        Format::Sequential => FileType::Sequential,
    };
    let mut tempo = None;
    let mut tracks = Vec::with_capacity(smf.tracks.len());
    for track in &smf.tracks {
        let (name, notes, length) = read_track(track, division);
        if tempo.is_none() {
            tempo = first_tempo(track);
        }
        tracks.push(ImportedTrack {
            name,
            notes,
            length,
        });
    }
    Ok(Imported {
        format,
        tracks,
        tempo,
    })
}

/// Writes `notes` as a type 1 Standard MIDI File, with `tempo` as its tempo
/// when it is given (a file without one plays at 120 bpm, the standard's
/// default).
///
/// # Errors
///
/// [`MidiFileError`] if a position or length can't be written (a note further
/// than the format's delta times can reach) or the file can't be written.
pub fn write(path: &Path, name: &str, notes: &[Note], tempo: Option<Bpm>) -> Result<(), MidiFileError> {
    let bytes = write_bytes(name, notes, tempo)?;
    std::fs::write(path, bytes)?;
    Ok(())
}

/// Writes `notes` as a type 1 Standard MIDI File's bytes, as [`write`] does.
///
/// # Errors
///
/// [`MidiFileError::OutOfRange`] if a note's start or length doesn't fit the
/// format's delta times.
pub fn write_bytes(
    name: &str,
    notes: &[Note],
    tempo: Option<Bpm>,
) -> Result<Vec<u8>, MidiFileError> {
    let division = i64::from(DIVISION);
    // Each event is a tick, a tie-break order (meta, note-offs, note-ons, so
    // a note-off never cuts a note that starts on the same tick), and what it
    // is.
    let mut events: Vec<(i64, u8, TrackEventKind<'_>)> = Vec::with_capacity(notes.len() * 2 + 3);
    events.push((
        0,
        0,
        TrackEventKind::Meta(MetaMessage::TrackName(name.as_bytes())),
    ));
    if let Some(bpm) = tempo {
        events.push((0, 0, TrackEventKind::Meta(MetaMessage::Tempo(per_quarter(bpm)))));
    }
    let mut end = 0_i64;
    for note in notes {
        let start = to_units(note.start, division)?;
        let stop = to_units(note.start.saturating_add(note.length), division)?.max(start + 1);
        end = end.max(stop);
        let key = u7::new(note.key.min(127));
        events.push((
            start,
            2,
            midi(MidiMessage::NoteOn {
                key,
                vel: u7::new(note.velocity.clamp(1, 127)),
            }),
        ));
        events.push((
            stop,
            1,
            midi(MidiMessage::NoteOff {
                key,
                vel: u7::new(note.release_velocity.clamp(1, 127)),
            }),
        ));
    }
    events.push((end, 3, TrackEventKind::Meta(MetaMessage::EndOfTrack)));
    events.sort_by_key(|(tick, order, _)| (*tick, *order));

    let mut track = Track::new();
    let mut previous = 0_i64;
    for (tick, _, kind) in events {
        let delta = u32::try_from((tick - previous).max(0)).map_err(|_| MidiFileError::OutOfRange)?;
        previous = tick;
        track.push(TrackEvent {
            delta: u28::new(delta),
            kind,
        });
    }

    let smf = Smf {
        header: Header::new(Format::Parallel, Timing::Metrical(u15::new(DIVISION))),
        tracks: vec![track],
    };
    let mut bytes = Vec::new();
    smf.write_std(&mut bytes)?;
    Ok(bytes)
}

/// A MIDI channel message on channel 0, which is what a clip's notes are.
fn midi(message: MidiMessage) -> TrackEventKind<'static> {
    TrackEventKind::Midi {
        channel: u4::new(0),
        message,
    }
}

/// The microseconds per quarter note a tempo is, clamped to what the meta
/// message holds.
fn per_quarter(bpm: Bpm) -> u24 {
    let micros = (60_000_000.0 / bpm.get()).round();
    let micros = micros.clamp(1.0, f64::from(u32::from(u24::max_value())));
    u24::new(micros as u32)
}

/// A track's name, notes, and the end of its contents.
fn read_track(track: &Track<'_>, division: i64) -> (String, Vec<Note>, Ticks) {
    let mut name = String::new();
    let mut named = false;
    let mut at = 0_i64;
    let mut end = 0_i64;
    // A slot per channel and key: the note-on still awaiting its note-off.
    let mut open: Vec<Option<(i64, u8)>> = vec![None; 16 * 128];
    let mut notes = Vec::new();

    for event in track {
        at = at.saturating_add(i64::from(event.delta.as_int()));
        end = end.max(at);
        match event.kind {
            TrackEventKind::Midi { channel, message } => {
                let channel = usize::from(channel.as_int());
                match message {
                    // A note-on with velocity 0 is a note-off (the standard).
                    MidiMessage::NoteOn { key, vel } if vel.as_int() == 0 => {
                        close(&mut open, &mut notes, channel, key.as_int(), at, 0, division);
                    }
                    MidiMessage::NoteOn { key, vel } => {
                        let slot = channel * 128 + usize::from(key.as_int());
                        open[slot] = Some((at, vel.as_int()));
                    }
                    MidiMessage::NoteOff { key, vel } => {
                        close(
                            &mut open,
                            &mut notes,
                            channel,
                            key.as_int(),
                            at,
                            vel.as_int(),
                            division,
                        );
                    }
                    _ => {}
                }
            }
            TrackEventKind::Meta(MetaMessage::TrackName(bytes)) if !named => {
                name = String::from_utf8_lossy(bytes).into_owned();
                named = true;
            }
            _ => {}
        }
    }
    // A note the track never released ends where the track does.
    for slot in 0..open.len() {
        if open[slot].is_some() {
            let (channel, key) = (slot / 128, (slot % 128) as u8);
            close(&mut open, &mut notes, channel, key, end, 0, division);
        }
    }
    notes.sort_by_key(|note| (note.start, note.key));
    let length = notes
        .iter()
        .map(|note| note.start.saturating_add(note.length))
        .max()
        .unwrap_or_else(|| to_ticks(end, division));
    (name, notes, length)
}

/// Closes the note on `channel` and `key`, if one is open, at tick `at`.
fn close(
    open: &mut [Option<(i64, u8)>],
    notes: &mut Vec<Note>,
    channel: usize,
    key: u8,
    at: i64,
    off_velocity: u8,
    division: i64,
) {
    let Some((start, velocity)) = open[channel * 128 + usize::from(key)].take() else {
        return;
    };
    let length = (at - start).max(1);
    let mut note = Note::new(
        key.min(127),
        to_ticks(start, division),
        to_ticks(length, division).max(Ticks(1)),
        velocity.clamp(1, 127),
    );
    note.release_velocity = if (1..=127).contains(&off_velocity) {
        off_velocity
    } else {
        64
    };
    notes.push(note);
}

/// The first tempo a track carries, if Beatlayer can hold it.
fn first_tempo(track: &Track<'_>) -> Option<Bpm> {
    track.iter().find_map(|event| match event.kind {
        TrackEventKind::Meta(MetaMessage::Tempo(micros)) => {
            let bpm = 60_000_000.0 / f64::from(micros.as_int());
            Bpm::new(bpm).ok()
        }
        _ => None,
    })
}

/// The Beatlayer ticks a count of the file's own ticks is, rounded to the
/// nearest.
fn to_ticks(units: i64, division: i64) -> Ticks {
    let scaled = i128::from(units) * i128::from(TICKS_PER_QUARTER);
    let rounded = (scaled + i128::from(division) / 2) / i128::from(division);
    Ticks(i64::try_from(rounded).unwrap_or(i64::MAX))
}

/// The file's own ticks a Beatlayer tick count is, rounded to the nearest.
fn to_units(ticks: Ticks, division: i64) -> Result<i64, MidiFileError> {
    let scaled = i128::from(ticks.get()) * i128::from(division);
    let rounded = (scaled + i128::from(TICKS_PER_QUARTER) / 2) / i128::from(TICKS_PER_QUARTER);
    i64::try_from(rounded).map_err(|_| MidiFileError::OutOfRange)
}

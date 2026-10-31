//! Standard MIDI Files (WP-2.21): reading a `.mid` into tracks of notes and
//! writing a clip's notes out as a type 1 file.
//!
//! A file's notes are read from its note-on and note-off messages: a note-on
//! with velocity 0 counts as a note-off, as the standard says, a note-off
//! whose velocity is 0 or missing takes the default release velocity (64),
//! and a note left open at the end of a track is closed there. Positions are
//! converted from the file's own ticks per quarter note to Beatlayer's, so a
//! file plays at the song's tempo like any clip.
//!
//! A file's tempo and time-signature events are read into a [`TempoMap`] and
//! a [`TimeSignatureMap`], and its track chunks keep their names and notes.
//! What the import command does with the tempo and signatures is a separate
//! question; this module only says what the file holds.

use std::collections::VecDeque;
use std::fmt;
use std::io::Read as _;
use std::path::Path;

use bl_core::time::{
    Bpm, TICKS_PER_QUARTER, TempoMap, TempoSegment, Ticks, TimeSignature, TimeSignatureChange,
    TimeSignatureMap,
};
use bl_model::Note;
use midly::num::{u4, u7, u15, u24, u28};
use midly::{
    Format, Header, MetaMessage, MidiMessage, Smf, Timing, Track, TrackEvent, TrackEventKind,
};

/// The ticks to the quarter note Beatlayer writes into a file: the largest
/// number a Standard MIDI File's header can hold (a 15-bit field) that
/// divides [`TICKS_PER_QUARTER`]. It is finer than any note grid Beatlayer
/// edits on, so a clip written and read back keeps every note's position and
/// length exactly.
pub const DIVISION: u16 = 28_224;

/// The MIDI clocks per metronome click and 32nd notes per quarter note a
/// written file announces, as the standard's defaults.
const CLOCKS_PER_CLICK: u8 = 24;
const THIRTY_SECONDS_PER_QUARTER: u8 = 8;

/// The file a format is, as the standard names it: type 0, 1, or 2.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileType {
    /// Type 0: one track holding every channel.
    SingleTrack,
    /// Type 1: several tracks playing together.
    Parallel,
    /// Type 2: several independent tracks.
    Sequential,
}

/// One track chunk of a file: its name and its notes from the file's start.
#[derive(Clone, Debug, PartialEq)]
pub struct ImportedTrack {
    /// The chunk's name, from its Track Name event, or empty.
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
    /// The file's tempo map: one constant 120 bpm segment when it carries no
    /// tempo event, the standard's default.
    pub tempo: TempoMap,
    /// Whether the file carried a tempo event.
    pub has_tempo: bool,
    /// The file's time-signature map: 4/4 throughout when it carries no
    /// time-signature event.
    pub time_signatures: TimeSignatureMap,
    /// Whether the file carried a time-signature event.
    pub has_time_signature: bool,
}

/// The largest MIDI file Beatlayer reads: 64 MiB, far past any real
/// arrangement, so a file that size or past it is refused before it's read.
pub const MAX_FILE_BYTES: u64 = 64 << 20;

/// The latest position, in Beatlayer ticks, a track of a file may reach: a
/// quarter of the range, so arithmetic on positions and lengths can't overflow.
pub const MAX_TICKS: i64 = i64::MAX / 4;

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
    /// The file is larger than [`MAX_FILE_BYTES`].
    TooLarge(u64),
}

impl fmt::Display for MidiFileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => error.fmt(f),
            Self::Parse(error) => write!(f, "not a MIDI file Beatlayer reads: {error}"),
            Self::OutOfRange => f.write_str("a number in the file is out of range"),
            Self::TooLarge(bytes) => write!(
                f,
                "the file is {bytes} bytes, more than the {MAX_FILE_BYTES} a MIDI file may be"
            ),
        }
    }
}

impl std::error::Error for MidiFileError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Parse(error) => Some(error),
            Self::OutOfRange | Self::TooLarge(_) => None,
        }
    }
}

impl From<std::io::Error> for MidiFileError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

/// The Standard MIDI File in `bytes`, from its `MThd` chunk: the bytes
/// themselves, or the `data` chunk of an RMID file, found as midly finds it.
fn smf_bytes(bytes: &[u8]) -> Option<&[u8]> {
    match bytes.get(..4)? {
        b"MThd" => Some(bytes),
        b"RIFF" if bytes.get(8..12)? == b"RMID" => {
            let riff_len = u32::from_le_bytes(bytes.get(4..8)?.try_into().ok()?) as usize;
            let mut rest = bytes.get(12..)?;
            rest = &rest[..rest.len().min(riff_len.saturating_sub(4))];
            while rest.len() >= 8 {
                let id = &rest[..4];
                let len = u32::from_le_bytes(rest[4..8].try_into().ok()?) as usize;
                rest = &rest[8..];
                let chunk = &rest[..rest.len().min(len)];
                if id == b"data" {
                    return Some(chunk);
                }
                rest = &rest[chunk.len()..];
                if len % 2 == 1 && !rest.is_empty() {
                    rest = &rest[1..];
                }
            }
            None
        }
        _ => None,
    }
}

/// Whether any `MThd` chunk, walked as midly walks the chunks, has a timing
/// field whose high byte is 0x80.
fn has_minus_128_fps(mut smf: &[u8]) -> bool {
    while smf.len() >= 8 {
        let id = &smf[..4];
        let len = u32::from_be_bytes([smf[4], smf[5], smf[6], smf[7]]) as usize;
        smf = &smf[8..];
        let chunk = &smf[..smf.len().min(len)];
        if id == b"MThd" && chunk.get(4) == Some(&0x80) {
            return true;
        }
        smf = &smf[chunk.len()..];
    }
    false
}

/// Reads the Standard MIDI File at `path`.
///
/// # Errors
///
/// [`MidiFileError`] if the file can't be read or isn't a Standard MIDI File
/// with a metrical (ticks per quarter note) time division, or is larger than
/// [`MAX_FILE_BYTES`]. Timecode timing, which no sequencer writes for note
/// data, is refused.
pub fn read(path: &Path) -> Result<Imported, MidiFileError> {
    let file = std::fs::File::open(path)?;
    let length = file.metadata()?.len();
    if length > MAX_FILE_BYTES {
        return Err(MidiFileError::TooLarge(length));
    }
    // The length can change after the check, so the read stops past the cap.
    let mut bytes = Vec::with_capacity(length as usize);
    file.take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes)?;
    read_bytes(&bytes)
}

/// Reads a Standard MIDI File from its bytes, as [`read`] does.
///
/// # Errors
///
/// [`MidiFileError`] if the bytes aren't a Standard MIDI File with a metrical
/// time division, are more than [`MAX_FILE_BYTES`], or run a track past
/// [`MAX_TICKS`].
pub fn read_bytes(bytes: &[u8]) -> Result<Imported, MidiFileError> {
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(MidiFileError::TooLarge(bytes.len() as u64));
    }
    // midly overflows negating a timecode of -128 frames per second, in any
    // header chunk it reads, so such a file is refused before midly sees it.
    // (Timecode timing is refused below anyway.)
    if smf_bytes(bytes).is_some_and(has_minus_128_fps) {
        return Err(MidiFileError::OutOfRange);
    }
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
    let mut tempos: Vec<(Ticks, Bpm)> = Vec::new();
    let mut signatures: Vec<(Ticks, TimeSignature)> = Vec::new();
    let mut tracks = Vec::with_capacity(smf.tracks.len());
    for track in &smf.tracks {
        // Every position in a track is at most the sum of its deltas.
        let units = track.iter().fold(0_i64, |sum, event| {
            sum.saturating_add(i64::from(event.delta.as_int()))
        });
        if i128::from(units) * i128::from(TICKS_PER_QUARTER) / i128::from(division)
            > i128::from(MAX_TICKS)
        {
            return Err(MidiFileError::OutOfRange);
        }
        let (name, notes, length) = read_track(track, division);
        collect_tempo(track, division, &mut tempos);
        collect_signatures(track, division, &mut signatures);
        tracks.push(ImportedTrack {
            name,
            notes,
            length,
        });
    }
    let has_tempo = !tempos.is_empty();
    let has_time_signature = !signatures.is_empty();
    Ok(Imported {
        format,
        tracks,
        tempo: tempo_map(tempos),
        has_tempo,
        time_signatures: signature_map(signatures),
        has_time_signature,
    })
}

/// Writes `notes` as a type 1 Standard MIDI File: its tempo and time
/// signature at tick 0, then the notes.
///
/// # Errors
///
/// [`MidiFileError`] if a position or length is out of range or the file
/// can't be written.
pub fn write(
    path: &Path,
    name: &str,
    notes: &[Note],
    tempo: Bpm,
    signature: TimeSignature,
) -> Result<(), MidiFileError> {
    let bytes = write_bytes(name, notes, tempo, signature)?;
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
    tempo: Bpm,
    signature: TimeSignature,
) -> Result<Vec<u8>, MidiFileError> {
    let division = i64::from(DIVISION);
    // Each event is a tick, a tie-break order (meta, note-offs, note-ons, so
    // a note-off never cuts a note that starts on the same tick), and what it
    // is.
    let mut events: Vec<(i64, u8, TrackEventKind<'_>)> = Vec::with_capacity(notes.len() * 2 + 4);
    events.push((
        0,
        0,
        TrackEventKind::Meta(MetaMessage::TrackName(name.as_bytes())),
    ));
    events.push((
        0,
        0,
        TrackEventKind::Meta(MetaMessage::Tempo(per_quarter(tempo))),
    ));
    events.push((0, 0, TrackEventKind::Meta(time_signature(signature))));
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
        // A delta is 28 bits: `u28::new` would mask a larger one, not refuse it.
        let delta = u32::try_from((tick - previous).max(0))
            .ok()
            .and_then(u28::try_from)
            .ok_or(MidiFileError::OutOfRange)?;
        previous = tick;
        track.push(TrackEvent { delta, kind });
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

/// The time-signature meta message a signature is: its denominator as the
/// power of two the standard writes.
fn time_signature(signature: TimeSignature) -> MetaMessage<'static> {
    let mut power = 0_u8;
    while (1_u16 << power) < u16::from(signature.denominator()) {
        power += 1;
    }
    MetaMessage::TimeSignature(
        signature.numerator(),
        power,
        CLOCKS_PER_CLICK,
        THIRTY_SECONDS_PER_QUARTER,
    )
}

/// The microseconds per quarter note a tempo is, clamped to what the meta
/// message holds.
fn per_quarter(bpm: Bpm) -> u24 {
    let micros = (60_000_000.0 / bpm.get()).round();
    let micros = micros.clamp(1.0, f64::from(u24::max_value().as_int()));
    u24::new(micros as u32)
}

/// A track's name, notes, and the end of its contents.
fn read_track(track: &Track<'_>, division: i64) -> (String, Vec<Note>, Ticks) {
    let mut name = String::new();
    let mut named = false;
    let mut at = 0_i64;
    let mut end = 0_i64;
    // A queue per channel and key: the note-ons still awaiting their
    // note-offs, the earliest first, so overlapping notes on one key pair
    // off in the order they began.
    let mut open: Vec<VecDeque<(i64, u8)>> = vec![VecDeque::new(); 16 * 128];
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
                        close(
                            &mut open,
                            &mut notes,
                            channel,
                            key.as_int(),
                            at,
                            0,
                            division,
                        );
                    }
                    MidiMessage::NoteOn { key, vel } => {
                        let slot = channel * 128 + usize::from(key.as_int());
                        open[slot].push_back((at, vel.as_int()));
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
        while !open[slot].is_empty() {
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

/// Closes the earliest note open on `channel` and `key`, if any, at tick `at`.
fn close(
    open: &mut [VecDeque<(i64, u8)>],
    notes: &mut Vec<Note>,
    channel: usize,
    key: u8,
    at: i64,
    off_velocity: u8,
    division: i64,
) {
    let Some((start, velocity)) = open[channel * 128 + usize::from(key)].pop_front() else {
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

/// Adds a track's tempo events to `into`.
fn collect_tempo(track: &Track<'_>, division: i64, into: &mut Vec<(Ticks, Bpm)>) {
    let mut at = 0_i64;
    for event in track {
        at = at.saturating_add(i64::from(event.delta.as_int()));
        if let TrackEventKind::Meta(MetaMessage::Tempo(micros)) = event.kind {
            let bpm = 60_000_000.0 / f64::from(micros.as_int());
            if let Ok(bpm) = Bpm::new(bpm) {
                into.push((to_ticks(at, division), bpm));
            }
        }
    }
}

/// Adds a track's time-signature events to `into`.
fn collect_signatures(track: &Track<'_>, division: i64, into: &mut Vec<(Ticks, TimeSignature)>) {
    let mut at = 0_i64;
    for event in track {
        at = at.saturating_add(i64::from(event.delta.as_int()));
        if let TrackEventKind::Meta(MetaMessage::TimeSignature(numerator, power, ..)) = event.kind {
            // The denominator is written as the power of two it is; a
            // signature Beatlayer can't hold is left out.
            let denominator = match u32::from(power) {
                0..=4 => 1_u8 << power,
                _ => continue,
            };
            if let Ok(signature) = TimeSignature::new(numerator, denominator) {
                into.push((to_ticks(at, division), signature));
            }
        }
    }
}

/// A tempo map from a file's tempo events, in order: 120 bpm until the first
/// event, and each event's tempo from its position on. Events at the same
/// position keep the last.
fn tempo_map(mut events: Vec<(Ticks, Bpm)>) -> TempoMap {
    events.sort_by_key(|(at, _)| *at);
    let mut segments: Vec<TempoSegment> = Vec::with_capacity(events.len() + 1);
    if events.first().is_none_or(|(at, _)| *at != Ticks::ZERO) {
        segments.push(TempoSegment::Constant {
            start: Ticks::ZERO,
            bpm: Bpm::DEFAULT,
        });
    }
    let mut previous: Option<Ticks> = None;
    for (at, bpm) in events {
        let segment = TempoSegment::Constant { start: at, bpm };
        if previous == Some(at) {
            *segments.last_mut().expect("a segment precedes a repeat") = segment;
        } else {
            segments.push(segment);
            previous = Some(at);
        }
    }
    TempoMap::new(segments).unwrap_or_else(|_| TempoMap::constant(Bpm::DEFAULT))
}

/// A time-signature map from a file's signature events, in order: 4/4 until
/// the first event, and each event's signature from its position on. Events
/// at the same position keep the last.
fn signature_map(mut events: Vec<(Ticks, TimeSignature)>) -> TimeSignatureMap {
    events.sort_by_key(|(at, _)| *at);
    let mut changes: Vec<TimeSignatureChange> = Vec::with_capacity(events.len() + 1);
    if events.first().is_none_or(|(at, _)| *at != Ticks::ZERO) {
        changes.push(TimeSignatureChange {
            at: Ticks::ZERO,
            signature: TimeSignature::FOUR_FOUR,
        });
    }
    let mut previous: Option<Ticks> = None;
    for (at, signature) in events {
        let change = TimeSignatureChange { at, signature };
        if previous == Some(at) {
            *changes.last_mut().expect("a change precedes a repeat") = change;
        } else {
            changes.push(change);
            previous = Some(at);
        }
    }
    TimeSignatureMap::new(changes)
        .unwrap_or_else(|_| TimeSignatureMap::constant(TimeSignature::FOUR_FOUR))
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

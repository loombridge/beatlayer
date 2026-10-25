//! Standard MIDI File import and export (WP-2.21): the behaviour behind
//! `midi.import_file` and `midi.export_clip`.
//!
//! Reading a file's track chunks, notes, tempo and time signature is
//! [`bl_interop::midi`]'s; this module turns that into clips, tracks, and,
//! when asked, the song's tempo and time-signature maps, as Live was observed
//! to on 15 October 2026 (the MIDI clips spec, WP-2.21).

use std::collections::BTreeSet;
use std::path::Path;

use bl_core::time::{
    TempoMap, TempoSegment, Ticks, TimeSignature, TimeSignatureChange, TimeSignatureMap,
};
use bl_interop::midi;
use bl_model::{Clip, ClipData, ClipId, Command, Document, Editor, MidiData, TrackId, TrackKind};
use bl_shell_spec::midi_files::{ImportedClip, MidiExportArgs, MidiExportResult, MidiImportResult};

use crate::arrangement::placing;

/// The undo step an import makes. Live's name for it isn't observed yet.
pub const UNDO_LABEL: &str = "Import MIDI File";

/// Why a MIDI file command couldn't run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MidiFileError {
    /// The arguments don't fit.
    InvalidArgs(String),
    /// It ran and failed.
    Failed(String),
}

/// Reads the Standard MIDI File at `path` into the Arrangement as one MIDI
/// clip per track of the file that has notes, as one undo step. Each clip
/// replaces what it covers of the clips already on its track, as a drop from
/// the browser does.
///
/// `at` is where every clip starts, `track` the MIDI track to put them on
/// (only a file with a single track of notes can go on one track), and
/// `import_tempo` whether to bring the file's tempo and time signature into
/// the song.
///
/// # Errors
///
/// [`MidiFileError`] if the file can't be read, holds no notes, or the
/// arguments don't fit; the set is unchanged then.
pub fn import(
    editor: &mut Editor,
    path: &Path,
    track: Option<TrackId>,
    at: Ticks,
    import_tempo: bool,
) -> Result<MidiImportResult, MidiFileError> {
    if at < Ticks::ZERO {
        return Err(MidiFileError::InvalidArgs(
            "`at` can't be before the song's start".to_owned(),
        ));
    }
    let failed =
        |e: &dyn std::fmt::Display| MidiFileError::Failed(format!("{}: {e}", path.display()));
    let file = midi::read(path).map_err(|e| failed(&e))?;
    let chunks: Vec<(usize, &midi::ImportedTrack)> = file
        .tracks
        .iter()
        .enumerate()
        .filter(|(_, chunk)| !chunk.notes.is_empty())
        .collect();
    if chunks.is_empty() {
        return Err(failed(&"it holds no notes"));
    }
    let target = match track {
        Some(track) => {
            match editor.document().track(track).map(bl_model::Track::kind) {
                Some(TrackKind::Midi) => {}
                Some(kind) => {
                    return Err(MidiFileError::InvalidArgs(format!(
                        "MIDI clips go on MIDI tracks, and track {} is a {kind} track",
                        track.0
                    )));
                }
                None => {
                    return Err(MidiFileError::InvalidArgs(format!(
                        "no track has the ID {}",
                        track.0
                    )));
                }
            }
            if chunks.len() > 1 {
                return Err(MidiFileError::InvalidArgs(
                    "this file has several tracks with notes; leave `track` out so each becomes \
                     its own MIDI track"
                        .to_owned(),
                ));
            }
            Some(track)
        }
        None => None,
    };
    // Spec: midi-clips R17. The clip's Signature is the file's, at its start.
    let signature = file.time_signatures.signature_at(Ticks::ZERO);
    let bar = signature.bar_length();

    let mut result = MidiImportResult::default();
    let applied = editor.transact(UNDO_LABEL, |editor| {
        let mut clip_end = at;
        // The clips this import places, which a later one never clears.
        let mut placed: BTreeSet<ClipId> = BTreeSet::new();
        for (index, chunk) in &chunks {
            let length = round_up(chunk.length, bar);
            let track = match target {
                Some(track) => track,
                None => {
                    let id: TrackId = editor.new_id();
                    let index = editor.document().top_level().len();
                    editor.apply(Command::CreateTrack {
                        id,
                        kind: TrackKind::Midi,
                        name: format!("{} MIDI", index + 1),
                        parent: None,
                        index,
                    })?;
                    result.created_tracks.push(id);
                    id
                }
            };
            let mut data = MidiData::new(chunk.notes.iter().cloned());
            // Spec: midi-clips R17. Looping on, with the brace the whole clip.
            data.end = Some(length);
            data.looping = true;
            data.loop_end = Some(length);
            data.signature = Some(signature);
            let name = if chunk.name.is_empty() {
                format!("Track {index}")
            } else {
                chunk.name.clone()
            };
            let id = editor.new_id();
            let mut clip = Clip::new(id, at, length, ClipData::Midi(data));
            clip.name.clone_from(&name);
            // Spec: arrangement-editing R6. Placed over other clips, it
            // replaces what it covers, as a drop from the browser does.
            placing::clear(editor, track, at, at.saturating_add(length), &placed)?;
            placed.insert(id);
            editor.apply(Command::CreateClip { track, clip })?;
            result.clips.push(ImportedClip {
                clip: id,
                track,
                start: at,
                length,
                name,
            });
            clip_end = clip_end.max(at.saturating_add(length));
        }
        if import_tempo && (file.has_tempo || file.has_time_signature) {
            // Spec: midi-clips R18. A file's tempo and signature come into the
            // song the way Live's "Import tempo and time signature data into
            // Arrangement?" answer Yes does.
            if file.has_tempo {
                let current = editor.document().settings().tempo.clone();
                if let Some(tempo) = import_tempo_map(&current, at, clip_end, &file.tempo) {
                    editor.apply(Command::SetTempo { tempo })?;
                }
            }
            let current = editor.document().settings().time_signatures.clone();
            let time_signatures = import_signature_map(&current, at, clip_end, signature);
            editor.apply(Command::SetTimeSignatures { time_signatures })?;
        }
        Ok::<_, bl_model::EditError>(())
    });
    applied.map_err(|error| MidiFileError::Failed(error.to_string()))?;
    Ok(result)
}

/// Writes a MIDI clip's notes, tempo and time signature out as a type 1
/// Standard MIDI File, as `midi.export_clip` describes.
///
/// # Errors
///
/// [`MidiFileError`] if the clip isn't a MIDI clip, a file is already at the
/// path and `replace` isn't set, or the file can't be written.
pub fn export(
    document: &Document,
    args: &MidiExportArgs,
) -> Result<MidiExportResult, MidiFileError> {
    let Some(file) = args.path.as_deref().filter(|path| !path.is_empty()) else {
        return Err(MidiFileError::InvalidArgs("`path` is empty".to_owned()));
    };
    let clip = document
        .clip(args.clip)
        .ok_or_else(|| MidiFileError::InvalidArgs(format!("no clip has the ID {}", args.clip.0)))?;
    let ClipData::Midi(data) = &clip.data else {
        return Err(MidiFileError::InvalidArgs(format!(
            "clip {} is an audio clip; `midi.export_clip` writes MIDI clips",
            args.clip.0
        )));
    };
    let path = Path::new(file);
    if path.exists() && !args.replace {
        return Err(MidiFileError::Failed(format!(
            "{file} is already there; pass `replace` to write over it"
        )));
    }
    // Spec: midi-clips R19. Live's own export wasn't observable, so these are
    // the maintainer's decision until a licensed Live can be checked.
    let settings = document.settings();
    let tempo = settings.tempo.bpm_at(clip.start);
    let signature = data
        .signature
        .unwrap_or_else(|| settings.time_signatures.signature_at(clip.start));
    let name = if clip.name.is_empty() {
        "Clip".to_owned()
    } else {
        clip.name.clone()
    };
    let notes: Vec<bl_model::Note> = data.notes.iter().cloned().collect();
    midi::write(path, &name, &notes, tempo, signature)
        .map_err(|e| MidiFileError::Failed(format!("{file}: {e}")))?;
    Ok(MidiExportResult {
        path: file.to_owned(),
        notes: notes.len(),
        start: clip.start,
        length: clip.length,
        tempo: tempo.get(),
        signature: format!("{}/{}", signature.numerator(), signature.denominator()),
    })
}

/// The length a track of contents runs, rounded up to a whole bar: a clip
/// lasts whole bars, and at least one.
fn round_up(length: Ticks, bar: Ticks) -> Ticks {
    if bar <= Ticks::ZERO {
        return length.max(Ticks(1));
    }
    let bars = (length.get() + bar.get() - 1).div_euclid(bar.get());
    Ticks(bars.max(1) * bar.get())
}

/// The song's tempo map with a file's tempo placed at `start`: a file with
/// one steady tempo becomes the whole song's tempo; a file whose tempo
/// changes is written over the clip's span, and the tempo in force before the
/// clip is restored at `end`.
fn import_tempo_map(
    current: &TempoMap,
    start: Ticks,
    end: Ticks,
    file: &TempoMap,
) -> Option<TempoMap> {
    let segments: Vec<TempoSegment> = file.segments().collect();
    if segments.len() == 1 {
        return Some(TempoMap::constant(segments[0].start_bpm()));
    }
    let before = current.bpm_at(start);
    let mut next: Vec<TempoSegment> = current
        .segments()
        .filter(|segment| segment.start() < start)
        .collect();
    for segment in segments {
        let shifted = shift(segment, start);
        if shifted.start() < end {
            next.push(shifted);
        }
    }
    next.push(TempoSegment::Constant {
        start: end,
        bpm: before,
    });
    next.extend(current.segments().filter(|segment| segment.start() > end));
    TempoMap::new(next).ok()
}

/// The song's time-signature map with the file's signature at `start` and the
/// signature in force before it restored at `end`, as Live writes both even
/// when they're the same.
fn import_signature_map(
    current: &TimeSignatureMap,
    start: Ticks,
    end: Ticks,
    signature: TimeSignature,
) -> TimeSignatureMap {
    let before = current.signature_at(start);
    let mut changes: Vec<TimeSignatureChange> = current
        .changes()
        .filter(|change| change.at < start || change.at > end)
        .collect();
    changes.push(TimeSignatureChange {
        at: start,
        signature,
    });
    changes.push(TimeSignatureChange {
        at: end,
        signature: before,
    });
    changes.sort_by_key(|change| change.at);
    TimeSignatureMap::new(changes).unwrap_or_else(|_| current.clone())
}

/// A tempo segment moved to start at `by` instead.
fn shift(segment: TempoSegment, by: Ticks) -> TempoSegment {
    match segment {
        TempoSegment::Constant { start, bpm } => TempoSegment::Constant {
            start: start.saturating_add(by),
            bpm,
        },
        TempoSegment::Ramp { start, from, to } => TempoSegment::Ramp {
            start: start.saturating_add(by),
            from,
            to,
        },
    }
}

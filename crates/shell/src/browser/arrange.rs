//! Adding audio files to the Arrangement, as dropping them from the browser
//! does (spec: browser R13, R14, R15). A clip placed over part of another
//! replaces that part, as the Arrangement's edits do
//! ([`crate::arrangement::placing`]), so a track never plays two clips at
//! once.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use bl_core::time::Ticks;
use bl_media::MediaStore;
use bl_model::{
    AudioData, Clip, ClipData, ClipId, Command, ContentHash, Document, Editor, TrackId, TrackKind,
};
use bl_shell_spec::browser::{AddResult, AddToArrangementArgs, PlacedClip};

use super::listing::stem;
use crate::arrangement::placing;

/// The undo step a drop makes. Live's name for it isn't observed yet
/// (browser, "To observe in Live").
pub const UNDO_LABEL: &str = "Insert Clip";

/// A file added to the set: what the set's media table records.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MediaFile {
    /// Its content hash, which the clips name.
    pub hash: ContentHash,
    /// Where it is.
    pub path: PathBuf,
    /// Its size in bytes.
    pub size: u64,
}

/// Why files couldn't be added.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AddError {
    /// The arguments don't fit: no files, or a track that can't hold audio.
    InvalidArgs(String),
    /// A file couldn't be read or decoded.
    Failed(String),
}

/// One file, read and measured.
struct Prepared {
    media: MediaFile,
    path: String,
    name: String,
    seconds: f64,
}

fn prepare(media: &MediaStore, path: &Path) -> Result<Prepared, AddError> {
    let failed = |e: &dyn std::fmt::Display| AddError::Failed(format!("{}: {e}", path.display()));
    let hash = media.import(path).map_err(|e| failed(&e))?;
    let entry = media
        .entry(hash)
        .ok_or_else(|| failed(&"the media store lost it"))?;
    let info = media.info(hash).map_err(|e| failed(&e))?;
    let frames = match info.frames {
        Some(frames) => frames,
        // The header doesn't say: decode it to count.
        None => media.load(hash).map_err(|e| failed(&e))?.frames() as u64,
    };
    if frames == 0 {
        return Err(failed(&"it holds no audio"));
    }
    Ok(Prepared {
        media: MediaFile {
            hash: ContentHash(*hash.as_bytes()),
            path: entry.path,
            size: entry.size,
        },
        path: path.to_string_lossy().into_owned(),
        name: stem(path),
        seconds: frames as f64 / f64::from(info.sample_rate.max(1)),
    })
}

/// How long a file lasts at the song's tempo when it starts at `start`.
fn length_at(doc: &Document, start: Ticks, seconds: f64) -> Ticks {
    let tempo = &doc.settings().tempo;
    let end = tempo.seconds_to_ticks(tempo.ticks_to_seconds(start) + seconds);
    Ticks((end.get() - start.get()).max(1))
}

/// Where a new track goes: below `track` after the `n` tracks already made
/// there, or below the last top-level track. Returns its parent and index.
fn place_below(doc: &Document, track: Option<TrackId>, n: usize) -> (Option<TrackId>, usize) {
    match track {
        Some(track) => {
            let parent = doc.parent(track);
            let siblings: Vec<TrackId> = match parent {
                Some(group) => doc
                    .track(group)
                    .map(|g| g.children().collect())
                    .unwrap_or_default(),
                None => doc.top_level().collect(),
            };
            let at = siblings.iter().position(|id| *id == track).unwrap_or(0);
            (parent, at + 1 + n)
        }
        // The tracks made so far are already at the bottom.
        None => (None, doc.top_level().len()),
    }
}

/// Puts `args.paths` into the Arrangement as one undo step, as
/// `browser.add_to_arrangement` describes, starting at the insert marker
/// unless `args.at` says. Each clip replaces what it covers of the clips
/// already on its track. Every file is read before anything changes, so a
/// file that can't be read changes nothing. Returns what was placed and the
/// files, for the set's media table.
///
/// # Errors
///
/// [`AddError`] if no file is given, the track isn't an audio track, or a
/// file can't be read; the set is unchanged.
pub fn add_files(
    editor: &mut Editor,
    media: &MediaStore,
    insert_marker: Ticks,
    args: &AddToArrangementArgs,
) -> Result<(AddResult, Vec<MediaFile>), AddError> {
    if args.paths.is_empty() {
        return Err(AddError::InvalidArgs("`paths` lists no files".to_owned()));
    }
    let start = args.at.unwrap_or(insert_marker);
    if start < Ticks::ZERO {
        return Err(AddError::InvalidArgs(
            "`at` can't be before the song's start".to_owned(),
        ));
    }
    if let Some(track) = args.track {
        match editor.document().track(track).map(bl_model::Track::kind) {
            Some(TrackKind::Audio) => {}
            Some(kind) => {
                return Err(AddError::InvalidArgs(format!(
                    "audio clips go on audio tracks, and track {} is a {kind} track",
                    track.0
                )));
            }
            None => {
                return Err(AddError::InvalidArgs(format!(
                    "no track has the ID {}",
                    track.0
                )));
            }
        }
    }
    let files = args
        .paths
        .iter()
        .map(|path| prepare(media, Path::new(path)))
        .collect::<Result<Vec<_>, _>>()?;

    let mut result = AddResult::default();
    let applied = editor.transact(UNDO_LABEL, |editor| {
        // The track each file goes on: the target (or one new track) for
        // all of them, or with `spread` a track each.
        let mut current = args.track;
        let mut made = 0;
        let mut at = start;
        // The clips this drop placed, which a later one never clears.
        let mut placed: BTreeSet<ClipId> = BTreeSet::new();
        for (index, file) in files.iter().enumerate() {
            let new_track = if args.spread {
                index > 0 || args.track.is_none()
            } else {
                index == 0 && args.track.is_none()
            };
            if new_track {
                let (parent, place) = place_below(editor.document(), args.track, made);
                let id: TrackId = editor.new_id();
                editor.apply(Command::CreateTrack {
                    id,
                    kind: TrackKind::Audio,
                    name: file.name.clone(),
                    parent,
                    index: place,
                })?;
                made += 1;
                result.created_tracks.push(id);
                current = Some(id);
            }
            if args.spread {
                at = start;
            }
            let track = current.expect("a track is chosen or made above");
            let length = length_at(editor.document(), at, file.seconds);
            let id = editor.new_id();
            let mut clip = Clip::new(
                id,
                at,
                length,
                ClipData::Audio(AudioData {
                    media: file.media.hash,
                }),
            );
            clip.name.clone_from(&file.name);
            placing::clear(editor, track, at, at.saturating_add(length), &placed)?;
            placed.insert(id);
            editor.apply(Command::CreateClip { track, clip })?;
            result.clips.push(PlacedClip {
                clip: id,
                track,
                start: at,
                length,
                name: file.name.clone(),
                path: file.path.clone(),
            });
            at = at.saturating_add(length);
        }
        Ok::<_, bl_model::EditError>(())
    });
    applied.map_err(|e| AddError::Failed(e.to_string()))?;
    Ok((result, files.into_iter().map(|f| f.media).collect()))
}

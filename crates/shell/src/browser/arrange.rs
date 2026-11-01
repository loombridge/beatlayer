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
    Clip, ClipData, ClipId, Command, ContentHash, Document, EditError, Editor, NewTakeLane,
    TakeLaneId, TrackId, TrackKind,
};
use bl_shell_spec::browser::{AddResult, AddToArrangementArgs, ItemKind, PlacedClip};

use super::listing::{self, stem};
use crate::arrangement::placing;
use crate::auto_warp::{Warping, new_clip};
use crate::clip_files::{self, Lane, Spot};

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

/// Whether `path` names an audio file, by its extension: the files a
/// Session slot takes without decoding first.
pub(crate) fn is_audio(path: &str) -> bool {
    Path::new(path)
        .file_name()
        .and_then(|name| listing::file_kind(&name.to_string_lossy()))
        == Some(ItemKind::Audio)
}

/// Whether `path` names a clip file, by its extension.
pub(crate) fn is_clip_file(path: &str) -> bool {
    listing::path_kind(Path::new(path)) == Some(ItemKind::Clip)
}

/// What the Status Bar says when placing a clip file left audio files
/// missing (clip files R12), if any.
pub(crate) fn missing_message(missing: usize) -> Option<String> {
    match missing {
        0 => None,
        1 => Some("1 audio file the clip plays is missing.".to_owned()),
        n => Some(format!("{n} audio files the clips play are missing.")),
    }
}

/// Reads one file for a Session slot: its media entry, its name, and how
/// long it lasts in seconds.
pub(crate) fn prepare_for_slot(
    media: &MediaStore,
    path: &Path,
) -> Result<(MediaFile, String, f64), AddError> {
    let prepared = prepare(media, path)?;
    Ok((prepared.media, prepared.name, prepared.seconds))
}

/// How long a file lasts at the song's tempo when it starts at `start`.
pub(crate) fn length_at(doc: &Document, start: Ticks, seconds: f64) -> Ticks {
    let tempo = &doc.settings().tempo;
    let end = tempo.seconds_to_ticks(tempo.ticks_to_seconds(start) + seconds);
    Ticks((end.get() - start.get()).max(1))
}

/// The tracks beside `track` in its group, or at the top level: its siblings,
/// itself among them, in order.
fn siblings(doc: &Document, track: TrackId) -> Vec<TrackId> {
    match doc.parent(track) {
        Some(group) => doc
            .track(group)
            .map(|g| g.children().collect())
            .unwrap_or_default(),
        None => doc.top_level().collect(),
    }
}

/// Where a new track goes: below `track` after the `n` tracks already made
/// there, or below the last top-level track. Returns its parent and index.
pub(crate) fn place_below(
    doc: &Document,
    track: Option<TrackId>,
    n: usize,
) -> (Option<TrackId>, usize) {
    match track {
        Some(track) => {
            let parent = doc.parent(track);
            let at = siblings(doc, track)
                .iter()
                .position(|id| *id == track)
                .unwrap_or(0);
            (parent, at + 1 + n)
        }
        // The tracks made so far are already at the bottom.
        None => (None, doc.top_level().len()),
    }
}

/// Where files dropped on a take lane go (the comping spec, R36): the track
/// of each file, and the lane's place among its track's take lanes.
struct LanePlan {
    /// The track each file goes on: the lane's own for all of them, or with
    /// `spread` the lane's track and those below it, one for each file.
    tracks: Vec<TrackId>,
    /// The lane's place among its track's take lanes.
    position: usize,
}

fn lane_plan(
    doc: &Document,
    args: &AddToArrangementArgs,
    lane: TakeLaneId,
) -> Result<LanePlan, AddError> {
    let invalid = |message: String| AddError::InvalidArgs(message);
    let owner = doc
        .take_lane_track(lane)
        .ok_or_else(|| invalid(format!("no take lane has the ID {}", lane.0)))?;
    if let Some(given) = args.track.filter(|track| *track != owner) {
        return Err(invalid(format!(
            "take lane {} isn't on track {}",
            lane.0, given.0
        )));
    }
    let position = doc
        .track(owner)
        .and_then(|track| track.take_lanes().position(|held| held.id == lane))
        .unwrap_or(0);
    let wanted = if args.spread { args.paths.len() } else { 1 };
    let tracks: Vec<TrackId> = siblings(doc, owner)
        .into_iter()
        .skip_while(|track| *track != owner)
        .take(wanted)
        .collect();
    if tracks.len() < wanted {
        return Err(invalid(format!(
            "{wanted} files need {wanted} tracks from track {} down, and there are {}",
            owner.0,
            tracks.len()
        )));
    }
    for track in &tracks {
        let held = doc.track(*track);
        if held.map(bl_model::Track::kind) != Some(TrackKind::Audio) {
            return Err(invalid(format!(
                "audio files go on audio tracks, and track {} isn't one",
                track.0
            )));
        }
        if held.is_some_and(bl_model::Track::is_frozen) {
            return Err(invalid(format!(
                "the track {} is frozen; unfreeze it to add takes",
                track.0
            )));
        }
    }
    Ok(LanePlan { tracks, position })
}

/// The take lane of `track` at `position` among its take lanes, adding one
/// there (after its last, when it has fewer) if it has none.
fn lane_at(
    editor: &mut Editor,
    track: TrackId,
    position: usize,
    result: &mut AddResult,
) -> Result<TakeLaneId, EditError> {
    let held = editor.document().track(track);
    if let Some(lane) = held.and_then(|t| t.take_lanes().nth(position)) {
        return Ok(lane.id);
    }
    let name = held
        .map(bl_model::Track::next_take_name)
        .unwrap_or_default();
    let index = held.map_or(0, |t| t.take_lanes().len());
    let id: TakeLaneId = editor.new_id();
    editor.apply(Command::AddTakeLane {
        track,
        lane: NewTakeLane {
            id: Some(id),
            name,
            clips: Vec::new(),
        },
        index: Some(index),
    })?;
    result.created_lanes.push(id);
    Ok(id)
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
    edge_fades: bool,
    warping: Warping<'_>,
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
    // Spec: comping R36. Dropped on a take lane, they go on take lanes.
    let lanes = args
        .lane
        .map(|lane| lane_plan(editor.document(), args, lane))
        .transpose()?;
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
    let before = editor.document().clone();
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
            if new_track && lanes.is_none() {
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
            // Spec: comping R36. On a take lane: the one dropped on for the
            // first file, and for the others with `spread` the take lane at
            // the same place of the next tracks down.
            let (track, lane) = match (&lanes, args.lane) {
                (Some(plan), Some(first)) => {
                    let track = plan.tracks[if args.spread { index } else { 0 }];
                    let lane = if index == 0 || !args.spread {
                        first
                    } else {
                        lane_at(editor, track, plan.position, &mut result)?
                    };
                    (track, Some(lane))
                }
                _ => (current.expect("a track is chosen or made above"), None),
            };
            // Spec: warping R33. The warp settings come from the file.
            let new = new_clip(&file.media, file.seconds, editor.document(), at, warping);
            let length = new.length;
            let id = editor.new_id();
            // Spec: audio-clips R7. With Create Fades on Clip Edges on, a
            // new clip's edges fade over 4 ms.
            let audio = new.audio;
            let audio = if edge_fades {
                audio.with_edge_fades()
            } else {
                audio
            };
            let mut clip = Clip::new(id, at, length, ClipData::Audio(audio));
            clip.name.clone_from(&file.name);
            placing::clear_on(editor, track, lane, at, at.saturating_add(length), &placed)?;
            placed.insert(id);
            editor.apply(match lane {
                Some(lane) => Command::CreateTakeClip { lane, clip },
                None => Command::CreateClip { track, clip },
            })?;
            result.clips.push(PlacedClip {
                clip: id,
                track,
                start: at,
                length,
                name: file.name.clone(),
                path: file.path.clone(),
                lane,
            });
            at = at.saturating_add(length);
        }
        // Spec: audio-clips R26. Clips the drop left meeting are crossfaded.
        crate::arrangement::fades::settle(editor, &before, edge_fades)?;
        Ok::<_, bl_model::EditError>(())
    });
    applied.map_err(|e| AddError::Failed(e.to_string()))?;
    Ok((result, files.into_iter().map(|f| f.media).collect()))
}

/// Puts the clips in clip files into the Arrangement (clip files R7 to R9)
/// as `browser.add_to_arrangement` describes, as one undo step: each on the
/// track of its kind the file says, or on a new track of its kind, which
/// brings the file's device chain with it. Every file is read before
/// anything changes, and every placement checked, so one that can't go
/// changes nothing. Returns what was placed, the audio files found for the
/// set's media table, and how many files the clips play that weren't.
///
/// # Errors
///
/// [`AddError`] if no file is given, the files aren't all clip files, a file
/// can't be read, or one can't go where it is asked to.
pub fn add_clip_files(
    editor: &mut Editor,
    media: &MediaStore,
    insert_marker: Ticks,
    search: &[PathBuf],
    args: &AddToArrangementArgs,
) -> Result<(AddResult, Vec<MediaFile>, usize), AddError> {
    if args.paths.is_empty() {
        return Err(AddError::InvalidArgs("`paths` lists no files".to_owned()));
    }
    if !args.paths.iter().all(|path| is_clip_file(path)) {
        return Err(AddError::InvalidArgs(
            "clip files can't be added together with other files".to_owned(),
        ));
    }
    let start = args.at.unwrap_or(insert_marker);
    if start < Ticks::ZERO {
        return Err(AddError::InvalidArgs(
            "`at` can't be before the song's start".to_owned(),
        ));
    }
    let loaded = args
        .paths
        .iter()
        .map(|path| clip_files::load(media, Path::new(path), search).map_err(AddError::Failed))
        .collect::<Result<Vec<_>, _>>()?;
    if let Some(track) = args.track {
        // The target track takes the first file with `spread`, all of them
        // without it.
        let count = if args.spread { 1 } else { loaded.len() };
        for file in loaded.iter().take(count) {
            clip_files::check(
                editor.document(),
                &file.file,
                Lane::Track(track),
                Spot::Arrangement(start),
            )
            .map_err(AddError::InvalidArgs)?;
        }
    }

    let mut result = AddResult::default();
    editor
        .transact(clip_files::UNDO_LABEL, |editor| {
            let mut current = args.track;
            let mut made = 0;
            let mut at = start;
            let mut placed: BTreeSet<ClipId> = BTreeSet::new();
            for (index, file) in loaded.iter().enumerate() {
                let kind = clip_files::track_kind(&file.file);
                let kept = current
                    .filter(|track| {
                        editor
                            .document()
                            .track(*track)
                            .is_some_and(|t| t.kind() == kind)
                    })
                    .filter(|_| !args.spread || index == 0);
                let lane = match kept {
                    Some(track) => Lane::Track(track),
                    None => {
                        let (parent, place) = place_below(editor.document(), args.track, made);
                        made += 1;
                        Lane::New {
                            parent,
                            index: place,
                        }
                    }
                };
                if args.spread {
                    at = start;
                }
                let done = clip_files::place(editor, file, lane, Spot::Arrangement(at), &placed)?;
                placed.insert(done.clip);
                if done.created {
                    result.created_tracks.push(done.track);
                }
                result.clips.push(PlacedClip {
                    clip: done.clip,
                    track: done.track,
                    start: done.start,
                    length: done.length,
                    name: editor
                        .document()
                        .clip(done.clip)
                        .map_or_else(String::new, |clip| clip.name.clone()),
                    path: args.paths[index].clone(),
                    lane: None,
                });
                current = Some(done.track);
                at = at.saturating_add(done.length);
            }
            Ok::<_, bl_model::EditError>(())
        })
        .map_err(|e| AddError::Failed(e.to_string()))?;
    let missing = loaded.iter().map(|file| file.missing).sum();
    let media = loaded.into_iter().flat_map(|file| file.media).collect();
    Ok((result, media, missing))
}

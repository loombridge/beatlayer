//! Clip files in the shell (WP-6.34; the clip files spec, R3 to R12): saving
//! a clip with the audio it plays, reading a clip file back, and placing the
//! clip it holds on a track.
//!
//! The file itself is [`bl_project::clip_file`]. Saving copies or refers to
//! the audio as Collect Files on Export says (file management R7 to R9);
//! placing registers that audio in the set's media store, finding it by its
//! content when it moved (file management R6).

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use bl_core::time::Ticks;
use bl_media::MediaStore;
use bl_model::{
    AutomationTarget, ChainId, Clip, ClipData, ClipId, Command, ContentHash, Device, DeviceId,
    Document, EditError, Editor, SceneId, TrackId, TrackKind,
};
use bl_project::clip_file::{ClipFile, ClipKind, unused_path};
use bl_project::{Media, MediaPath, MediaRef, ProjectFolder};

use crate::browser::arrange::{MediaFile, length_at};
use crate::files::free_import;

/// The undo step a placement makes (clip files R11).
pub const UNDO_LABEL: &str = "Insert Clip";

fn media_hash(hash: ContentHash) -> bl_media::ContentHash {
    bl_media::ContentHash::from_bytes(hash.0)
}

/// The project folder a clip file saved into `folder` belongs to (file
/// management R9): the nearest folder above it, itself included, that holds
/// a set, or the User Library when it comes first; `folder` itself when
/// neither does.
#[must_use]
pub fn project_of(folder: &Path, library: Option<&Path>) -> PathBuf {
    for ancestor in folder.ancestors() {
        if library == Some(ancestor) {
            return ancestor.to_owned();
        }
        if ProjectFolder::new(ancestor)
            .sets()
            .is_ok_and(|sets| !sets.is_empty())
        {
            return ancestor.to_owned();
        }
    }
    folder.to_owned()
}

/// Whether `dir` holds a file of this length and content: an earlier copy
/// to use again (file management R9).
fn same_content(dir: &Path, size: u64, hash: ContentHash) -> Option<PathBuf> {
    fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .metadata()
                .is_ok_and(|m| m.is_file() && m.len() == size)
        })
        .map(|entry| entry.path())
        .find(|path| bl_media::ContentHash::of_file(path).is_ok_and(|h| h == media_hash(hash)))
}

/// The media table for a clip file saved into `folder`: each file in `used`
/// that the store has, copied into the `Samples/Imported` folder of `home`
/// unless it is in `home` already, or left where it is when `copy` is off.
/// Returns the table and how many files were copied.
fn collect(
    store: &MediaStore,
    used: &BTreeSet<ContentHash>,
    folder: &Path,
    home: &Path,
    copy: bool,
) -> Result<(Media, usize), String> {
    let folder = std::path::absolute(folder).map_err(|e| e.to_string())?;
    let home = std::path::absolute(home).map_err(|e| e.to_string())?;
    let imported = ProjectFolder::new(home.clone()).imported();
    let mut media = Media::new();
    let mut copied = 0;
    for &hash in used {
        // A file the store doesn't know, or can't find, is left out of the
        // table: the clip file lists it missing when it is placed.
        let Some(entry) = store.entry(media_hash(hash)).filter(|e| e.path.is_file()) else {
            continue;
        };
        let source = std::path::absolute(&entry.path).map_err(|e| e.to_string())?;
        let file = if !copy || source.starts_with(&home) {
            source
        } else if let Some(existing) = same_content(&imported, entry.size, hash) {
            existing
        } else {
            let name = source
                .file_name()
                .map_or_else(|| "sample".to_owned(), |n| n.to_string_lossy().into_owned());
            fs::create_dir_all(&imported)
                .map_err(|e| format!("couldn't make {}: {e}", imported.display()))?;
            let to = free_import(&imported, &name)?;
            fs::copy(&source, &to).map_err(|e| {
                format!(
                    "couldn't copy {} to {}: {e}",
                    source.display(),
                    to.display()
                )
            })?;
            copied += 1;
            to
        };
        let path = MediaPath::between(&folder, &file).ok_or_else(|| {
            format!(
                "{} can't be named from {}; let Collect Files on Export copy it",
                file.display(),
                folder.display()
            )
        })?;
        media.insert(MediaRef {
            hash,
            path,
            size: entry.size,
            reversed_from: None,
        });
    }
    Ok((media, copied))
}

/// Whether saving `clip` into `folder` would copy any audio file: the clip or
/// its track's devices play one outside the project the file lands in. Under
/// Ask that is when the person is asked (file management R8).
#[must_use]
pub fn uses_outside(
    doc: &Document,
    store: &MediaStore,
    clip: ClipId,
    folder: &Path,
    library: Option<&Path>,
) -> bool {
    let Some(source) = doc.clip(clip) else {
        return false;
    };
    let mut used: BTreeSet<ContentHash> = doc
        .clip_track(clip)
        .and_then(|track| doc.track(track))
        .into_iter()
        .flat_map(|track| track.devices())
        .flat_map(Device::media_used)
        .collect();
    if let ClipData::Audio(audio) = &source.data {
        used.insert(audio.media);
    }
    let home = project_of(folder, library);
    let home = std::path::absolute(&home).unwrap_or(home);
    used.iter().any(|&hash| {
        store
            .entry(media_hash(hash))
            .filter(|entry| entry.path.is_file())
            .is_some_and(|entry| {
                !std::path::absolute(&entry.path)
                    .unwrap_or(entry.path)
                    .starts_with(&home)
            })
    })
}

/// How long an unwarped audio clip lasts, in seconds (clip files R9).
fn seconds_of(doc: &Document, clip: &Clip) -> Option<f64> {
    let ClipData::Audio(audio) = &clip.data else {
        return None;
    };
    let tempo = &doc.settings().tempo;
    let seconds = tempo.ticks_to_seconds(clip.end()) - tempo.ticks_to_seconds(clip.start);
    (!audio.warp && seconds > 0.0).then_some(seconds)
}

/// A clip file that was written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Saved {
    /// Its path.
    pub path: PathBuf,
    /// Its name, without the extension.
    pub name: String,
    /// What it holds.
    pub kind: ClipKind,
    /// How many audio files were copied beside it.
    pub copied: usize,
}

/// Saves `clip` as a clip file in `folder` (clip files R3 to R5): named
/// `name`, the clip's name, or its track's, with " 2" and so on for a name
/// taken, written whole, with the audio copied or referred to as `copy`
/// says. `library` is the User Library, where copies go for a file saved
/// into it.
///
/// # Errors
///
/// A message if the clip doesn't exist or a file can't be copied or
/// written; nothing is saved then, though files already copied stay.
pub fn save(
    doc: &Document,
    store: &MediaStore,
    clip: ClipId,
    folder: &Path,
    name: Option<&str>,
    copy: bool,
    library: Option<&Path>,
) -> Result<Saved, String> {
    let source = doc
        .clip(clip)
        .ok_or_else(|| format!("no clip has the ID {}", clip.0))?;
    let track = doc
        .clip_track(clip)
        .and_then(|track| doc.track(track))
        .ok_or_else(|| format!("clip {} has no track", clip.0))?;
    let name = name
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .or_else(|| Some(source.name.trim()).filter(|name| !name.is_empty()))
        .unwrap_or_else(|| track.name())
        .to_owned();
    let devices: Vec<Device> = track.devices().cloned().collect();
    let mut used: BTreeSet<ContentHash> = devices.iter().flat_map(Device::media_used).collect();
    if let ClipData::Audio(audio) = &source.data {
        used.insert(audio.media);
    }
    let (media, copied) = collect(store, &used, folder, &project_of(folder, library), copy)?;
    let file = ClipFile {
        clip: source.clone(),
        devices,
        media,
        seconds: seconds_of(doc, source),
    };
    let path = unused_path(folder, &name);
    file.save(&path).map_err(|e| e.to_string())?;
    Ok(Saved {
        name: path
            .file_stem()
            .map_or_else(String::new, |s| s.to_string_lossy().into_owned()),
        path,
        kind: ClipKind::of(&file.clip.data),
        copied,
    })
}

/// A clip file read, with the audio it plays registered.
#[derive(Clone, Debug)]
pub struct Loaded {
    /// The file's name, without the extension.
    pub stem: String,
    /// What the file holds.
    pub file: ClipFile,
    /// The audio files found and imported, for the set's media table.
    pub media: Vec<MediaFile>,
    /// How many files the clip and its devices play that weren't found.
    pub missing: usize,
}

/// Whether `path` holds the content `hash` names.
pub(crate) fn holds(path: &Path, size: u64, hash: ContentHash) -> bool {
    fs::metadata(path).is_ok_and(|m| m.is_file() && m.len() == size)
        && bl_media::ContentHash::of_file(path).is_ok_and(|h| h == media_hash(hash))
}

/// Imports the audio file at `at` into the store, as the media a clip file
/// names by `hash`.
fn register(store: &MediaStore, hash: ContentHash, at: &Path) -> Option<MediaFile> {
    let entry = store.entry(store.import(at).ok()?)?;
    Some(MediaFile {
        hash,
        path: entry.path,
        size: entry.size,
    })
}

/// Reads the clip file at `path` and registers the audio it names (clip
/// files R12): from the path the file gives, relative to its folder, or, if
/// the file isn't there or changed, from a search by content through the
/// file's folder and `search`. A file still missing is counted, and the
/// clip is placed all the same.
///
/// # Errors
///
/// A message if the file can't be read or is refused.
pub fn load(store: &MediaStore, path: &Path, search: &[PathBuf]) -> Result<Loaded, String> {
    let file = ClipFile::load(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let folder = path.parent().unwrap_or_else(|| Path::new("."));
    let mut media = Vec::new();
    let mut wanted = Vec::new();
    for hash in file.media_used() {
        if let Some(entry) = store.entry(media_hash(hash)).filter(|e| e.path.is_file()) {
            media.push(MediaFile {
                hash,
                path: entry.path,
                size: entry.size,
            });
            continue;
        }
        let Some(reference) = file.media.get(&hash) else {
            wanted.push((hash, None));
            continue;
        };
        let there = reference
            .path
            .resolve(folder)
            .filter(|p| holds(p, reference.size, hash));
        match there.and_then(|there| register(store, hash, &there)) {
            Some(file) => media.push(file),
            None => wanted.push((hash, Some(reference.size))),
        }
    }
    let sized: Vec<(bl_media::ContentHash, u64)> = wanted
        .iter()
        .filter_map(|&(hash, size)| Some((media_hash(hash), size?)))
        .collect();
    let mut folders = vec![folder.to_owned()];
    folders.extend(search.iter().cloned());
    let located: BTreeMap<_, _> = store
        .find_by_content(&sized, &folders)
        .into_iter()
        .collect();
    let mut missing = 0;
    for (hash, _) in wanted {
        match located
            .get(&media_hash(hash))
            .and_then(|at| register(store, hash, at))
        {
            Some(file) => media.push(file),
            None => missing += 1,
        }
    }
    Ok(Loaded {
        stem: path
            .file_stem()
            .map_or_else(String::new, |s| s.to_string_lossy().into_owned()),
        file,
        media,
        missing,
    })
}

/// The track a clip goes on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lane {
    /// A track the set has.
    Track(TrackId),
    /// A new track of the clip's kind, named after the clip file, in
    /// `parent` at `index`.
    New {
        /// The group it goes in, if any.
        parent: Option<TrackId>,
        /// Its place among its siblings.
        index: usize,
    },
}

/// Where on its track a clip goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Spot {
    /// The Arrangement, starting here.
    Arrangement(Ticks),
    /// The Session slot of this scene.
    Slot(SceneId),
}

/// What the track kind for a clip file is.
#[must_use]
pub fn track_kind(file: &ClipFile) -> TrackKind {
    match ClipKind::of(&file.clip.data) {
        ClipKind::Audio => TrackKind::Audio,
        ClipKind::Midi => TrackKind::Midi,
    }
}

/// Checks that `file` can go at `lane` and `spot`: a track of its kind that
/// the set has, and, in the Session, an empty slot (clip files R7). Nothing
/// is changed.
///
/// # Errors
///
/// The reason it can't.
pub fn check(doc: &Document, file: &ClipFile, lane: Lane, spot: Spot) -> Result<(), String> {
    let Lane::Track(id) = lane else {
        return Ok(());
    };
    let track = doc
        .track(id)
        .ok_or_else(|| format!("no track has the ID {}", id.0))?;
    let wanted = track_kind(file);
    if track.kind() != wanted {
        return Err(format!(
            "this clip file holds a {wanted} clip, and track {} is a {} track",
            id.0,
            track.kind()
        ));
    }
    if let Spot::Slot(scene) = spot
        && doc.slot(id, scene).is_some()
    {
        return Err(format!(
            "the slot of track {} in that scene already holds a clip",
            id.0
        ));
    }
    Ok(())
}

/// A clip that was placed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placed {
    /// The new clip.
    pub clip: ClipId,
    /// Its track.
    pub track: TrackId,
    /// Whether the track is new.
    pub created: bool,
    /// Where the clip starts.
    pub start: Ticks,
    /// How long it is.
    pub length: Ticks,
}

/// The envelope target for a clip placed with the devices in `devices` and
/// `chains` (old ID to new): a mixer control and a MIDI controller stay; a
/// device's or a chain's follows the device it belongs to, if that came with
/// the clip; the rest are left out (clip files R9).
fn retarget(
    target: AutomationTarget,
    devices: &BTreeMap<DeviceId, DeviceId>,
    chains: &BTreeMap<ChainId, ChainId>,
) -> Option<AutomationTarget> {
    use AutomationTarget as T;
    match target {
        T::Mixer { .. } | T::MidiCc { .. } | T::MidiPitchBend | T::MidiChannelPressure => {
            Some(target)
        }
        // A return track of another set means nothing here.
        T::Send { .. } => None,
        T::Device { device, param } => devices
            .get(&device)
            .map(|&device| T::Device { device, param }),
        T::Chain { chain, param } => chains.get(&chain).map(|&chain| T::Chain { chain, param }),
        T::ChainSend { chain, to } => Some(T::ChainSend {
            chain: *chains.get(&chain)?,
            to: *chains.get(&to)?,
        }),
    }
}

/// Places the clip `file` holds at `lane` and `spot`, in the caller's undo
/// step (clip files R7 to R9): on a new track, or one with no clips and no
/// devices, with its device chain first, each device and chain with new IDs.
/// A clip placed in the Arrangement replaces what it covers of the clips
/// there, but not those in `keep`. [`check`] the placement first.
///
/// # Errors
///
/// [`EditError`] if the document refuses a step.
pub fn place(
    editor: &mut Editor,
    loaded: &Loaded,
    lane: Lane,
    spot: Spot,
    keep: &BTreeSet<ClipId>,
) -> Result<Placed, EditError> {
    let file = &loaded.file;
    let (track, created) = match lane {
        Lane::Track(track) => (track, false),
        Lane::New { parent, index } => {
            let id: TrackId = editor.new_id();
            editor.apply(Command::CreateTrack {
                id,
                kind: track_kind(file),
                name: loaded.stem.clone(),
                parent,
                index,
            })?;
            (id, true)
        }
    };
    let brings_devices = editor
        .document()
        .track(track)
        .is_some_and(|t| t.devices().len() == 0 && t.clips().len() == 0 && t.slots().len() == 0);
    let mut device_map = BTreeMap::new();
    let mut chain_map = BTreeMap::new();
    if brings_devices {
        for (index, device) in file.devices.iter().enumerate() {
            let mut copy = device.clone();
            copy.reassign_ids(&mut || editor.new_id::<DeviceId>().0);
            device_map.extend(device.walk().map(|d| d.id).zip(copy.walk().map(|d| d.id)));
            chain_map.extend(
                device
                    .chains_deep()
                    .map(|c| c.id)
                    .zip(copy.chains_deep().map(|c| c.id)),
            );
            // The modulators' targets are the other set's tracks.
            copy.clear_mappings();
            editor.apply(Command::InsertDevice {
                track,
                index,
                device: copy,
            })?;
        }
    }
    let start = match spot {
        Spot::Arrangement(at) => at,
        Spot::Slot(_) => Ticks::ZERO,
    };
    let mut clip = file.clip.clone();
    clip.id = editor.new_id();
    clip.start = start;
    if clip.name.trim().is_empty() {
        clip.name.clone_from(&loaded.stem);
    }
    if let (Some(seconds), ClipData::Audio(audio)) = (file.seconds, &file.clip.data)
        && !audio.warp
    {
        clip.length = length_at(editor.document(), start, seconds);
    }
    clip.envelopes = file
        .clip
        .envelopes
        .iter()
        .filter_map(|envelope| {
            let target = retarget(envelope.target, &device_map, &chain_map)?;
            Some(bl_model::ClipEnvelope {
                target,
                ..envelope.clone()
            })
        })
        .collect();
    let (id, length) = (clip.id, clip.length);
    match spot {
        Spot::Arrangement(_) => {
            crate::arrangement::placing::clear(
                editor,
                track,
                start,
                start.saturating_add(length),
                keep,
            )?;
            editor.apply(Command::CreateClip { track, clip })?;
        }
        Spot::Slot(scene) => {
            editor.apply(Command::CreateSlotClip { track, scene, clip })?;
        }
    }
    Ok(Placed {
        clip: id,
        track,
        created,
        start,
        length,
    })
}

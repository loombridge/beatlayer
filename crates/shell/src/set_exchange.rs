//! Sets as sources and as products (WP-6.35; the set exchange spec, R1 to
//! R14): reading a set so the browser can list its tracks, Session clips and
//! chains of devices; the Session clips of a set as clip files; and building
//! and writing a new set from tracks or Session clips of the open one.
//!
//! Adding tracks to the open set is [`crate::tracks::copy_tracks`], which
//! copies whole tracks between documents; the shell runs it with what is read
//! here.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use bl_media::MediaStore;
use bl_model::{
    ClipData, ClipId, Command, ContentHash, Device, Document, Editor, Routing, SceneId, TrackId,
    TrackKind,
};
use bl_project::clip_file::{ClipFile, ClipKind};
use bl_project::{Media, Project, ProjectFolder, SetFile};
use bl_shell_spec::browser::{ClipFileKind, Item, ItemKind};

use crate::clip_files;
use crate::files::place_set;
use crate::tracks::{self, Place, TrackData};

/// The undo step name `copy_tracks` is given when the set built is a new
/// one: nothing in it is ever undone.
const BUILD_STEP: &str = "Build Set";

/// A row of an opened set: what a key in the browser names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Row {
    /// A track, a group, a return track or Main (R1).
    Track(TrackId),
    /// A track's chain of devices (R1).
    Devices(TrackId),
    /// A Session clip (R2).
    Clip(ClipId),
}

/// The key of a row of the set at `set`, which is also its path in the
/// browser: the set's path, `#`, and `t`, `d` or `c` with the ID.
#[must_use]
pub fn key(set: &Path, row: Row) -> String {
    let tail = match row {
        Row::Track(track) => format!("t{}", track.0.get()),
        Row::Devices(track) => format!("d{}", track.0.get()),
        Row::Clip(clip) => format!("c{}", clip.0.get()),
    };
    format!("{}#{tail}", set.to_string_lossy())
}

/// The set and row a key names, if it is one.
#[must_use]
pub fn parse_key(key: &str) -> Option<(PathBuf, Row)> {
    let (set, tail) = key.rsplit_once('#')?;
    if !bl_project::is_set(Path::new(set)) {
        return None;
    }
    let (kind, id) = tail.split_at(tail.chars().next()?.len_utf8());
    let id = bl_core::id::Id::new(id.parse().ok()?)?;
    let row = match kind {
        "t" => Row::Track(TrackId(id)),
        "d" => Row::Devices(TrackId(id)),
        "c" => Row::Clip(ClipId(id)),
        _ => return None,
    };
    Some((PathBuf::from(set), row))
}

/// A set read for the browser to list and the shell to take tracks from.
#[derive(Debug)]
pub struct Opened {
    /// The set file.
    pub path: PathBuf,
    /// Its project folder, where its media table's paths start.
    pub folder: PathBuf,
    /// What it holds.
    pub project: Project,
}

/// The sets read lately, so a preview, a load and a drag of the same set read
/// it once: its path, its modification time and length, and what it held.
type Cache = Vec<(PathBuf, Option<SystemTime>, u64, Arc<Opened>)>;

static RECENT: Mutex<Cache> = Mutex::new(Vec::new());

/// How many opened sets the cache keeps.
const KEPT: usize = 4;

/// Reads the set at `path` with the set loader's checks (R1), as opening it
/// does: a set from a later Beatlayer, or a damaged one, is refused with the
/// reason. A set read lately and not changed since comes from the cache.
///
/// # Errors
///
/// The reason the set can't be read.
pub fn open(path: &Path) -> Result<Arc<Opened>, String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let stamp = (meta.modified().ok(), meta.len());
    // shortcut: a set is read whole to list it; a lazy reader would help
    // sets of hundreds of megabytes, which the loader's own limit allows.
    if let Ok(recent) = RECENT.lock()
        && let Some(found) = recent
            .iter()
            .find(|(p, modified, len, _)| p == path && (*modified, *len) == stamp)
    {
        return Ok(Arc::clone(&found.3));
    }
    let (mut project, info) = bl_project::load(path).map_err(|e| e.to_string())?;
    crate::files::after_load(&mut project, &info);
    let folder = std::path::absolute(path)
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_default();
    let opened = Arc::new(Opened {
        path: path.to_path_buf(),
        folder,
        project,
    });
    if let Ok(mut recent) = RECENT.lock() {
        recent.retain(|(p, ..)| p != path);
        recent.insert(
            0,
            (path.to_path_buf(), stamp.0, stamp.1, Arc::clone(&opened)),
        );
        recent.truncate(KEPT);
    }
    Ok(opened)
}

fn clip_file_kind(data: &ClipData) -> ClipFileKind {
    match ClipKind::of(data) {
        ClipKind::Audio => ClipFileKind::Audio,
        ClipKind::Midi => ClipFileKind::Midi,
    }
}

impl Opened {
    /// The tracks a drop of the whole set adds: every track but its return
    /// tracks and Main, in its order (R3), with groups holding their tracks.
    #[must_use]
    pub fn top_tracks(&self) -> Vec<TrackId> {
        self.project.document.top_level().collect()
    }

    /// A Session clip as a clip file holds it, with its track's chain of
    /// devices and the files they play named from the set's project folder
    /// (R2).
    ///
    /// # Errors
    ///
    /// A message if the clip isn't a Session clip of the set.
    pub fn clip_file(&self, clip: ClipId) -> Result<ClipFile, String> {
        let doc = &self.project.document;
        let track = doc
            .clip_track(clip)
            .and_then(|id| doc.track(id))
            .ok_or_else(|| format!("the set has no clip {}", clip.0.get()))?;
        let source = track
            .slots()
            .map(|(_, held)| held)
            .find(|held| held.id == clip)
            .ok_or_else(|| format!("clip {} isn't a Session clip", clip.0.get()))?;
        let devices: Vec<Device> = track.devices().cloned().collect();
        let mut used: BTreeSet<ContentHash> = devices.iter().flat_map(Device::media_used).collect();
        if let ClipData::Audio(audio) = &source.data {
            used.insert(audio.media);
        }
        let media: Media = used
            .iter()
            .filter_map(|hash| self.project.media.get(hash).cloned())
            .collect();
        Ok(ClipFile {
            seconds: clip_files::seconds_of(doc, source),
            clip: source.clone(),
            devices,
            media,
        })
    }

    /// The rows of the set at `set` in the browser, below its own row at
    /// `depth - 1`: the tracks in order, a group holding its tracks, then the
    /// return tracks and Main (R1). A row is open when its key is in `open`.
    pub fn rows(&self, depth: u16, open: &BTreeSet<PathBuf>, out: &mut Vec<Item>) {
        let doc = &self.project.document;
        let tracks = doc.top_level().chain(doc.returns()).chain([doc.main_id()]);
        for track in tracks {
            self.track_rows(track, depth, open, out);
        }
    }

    fn track_rows(&self, id: TrackId, depth: u16, open: &BTreeSet<PathBuf>, out: &mut Vec<Item>) {
        let doc = &self.project.document;
        let Some(track) = doc.track(id) else {
            return;
        };
        let kind = match track.kind() {
            TrackKind::Group => ItemKind::Group,
            TrackKind::Return => ItemKind::Return,
            TrackKind::Main => ItemKind::Main,
            TrackKind::Audio | TrackKind::Midi => ItemKind::Track,
        };
        let path = key(&self.path, Row::Track(id));
        let shown = open.contains(Path::new(&path));
        out.push(Item {
            path,
            name: track.name().to_owned(),
            kind,
            depth,
            open: shown,
            folder: None,
            format: None,
            clip_kind: None,
            details: None,
            collections: Vec::new(),
            hidden: false,
            nested: Vec::new(),
        });
        if !shown {
            return;
        }
        match kind {
            ItemKind::Group => {
                for child in track.children() {
                    self.track_rows(child, depth + 1, open, out);
                }
            }
            ItemKind::Track => {
                let mut clips: Vec<(usize, &bl_model::Clip)> = track
                    .slots()
                    .filter_map(|(scene, clip)| Some((doc.scene_position(scene)?, clip)))
                    .collect();
                clips.sort_by_key(|(position, _)| *position);
                for (position, clip) in clips {
                    let name = Some(clip.name.trim())
                        .filter(|name| !name.is_empty())
                        .unwrap_or_else(|| track.name());
                    out.push(Item {
                        path: key(&self.path, Row::Clip(clip.id)),
                        name: format!("Scene {}: {name}", position + 1),
                        kind: ItemKind::SessionClip,
                        depth: depth + 1,
                        open: false,
                        folder: None,
                        format: None,
                        clip_kind: Some(clip_file_kind(&clip.data)),
                        details: None,
                        collections: Vec::new(),
                        hidden: false,
                        nested: Vec::new(),
                    });
                }
            }
            _ => {}
        }
        out.push(Item {
            path: key(&self.path, Row::Devices(id)),
            name: "Devices".to_owned(),
            kind: ItemKind::Devices,
            depth: depth + 1,
            open: false,
            folder: None,
            format: None,
            clip_kind: None,
            details: None,
            collections: Vec::new(),
            hidden: false,
            nested: Vec::new(),
        });
    }
}

/// Reads the clip file at `path`, or, when `path` is the key of a Session
/// clip of a set, that clip as a clip file (R2): what placing, previewing and
/// dragging a clip file read.
///
/// # Errors
///
/// A message if it can't be read.
pub fn read_clip_file(path: &Path) -> Result<ClipFile, String> {
    match parse_key(&path.to_string_lossy()) {
        Some((set, Row::Clip(clip))) => open(&set)?.clip_file(clip),
        Some(_) => Err("that row of the set isn't a clip".to_owned()),
        None => ClipFile::load(path).map_err(|e| format!("{}: {e}", path.display())),
    }
}

/// The name a clip file's clip takes when it has none: the file's name
/// without its extension, and for a Session clip of a set, the clip's name or
/// its track's.
#[must_use]
pub fn stem_of(path: &Path) -> String {
    if let Some((set, Row::Clip(clip))) = parse_key(&path.to_string_lossy())
        && let Ok(opened) = open(&set)
    {
        let doc = &opened.project.document;
        let named = doc
            .clip(clip)
            .map(|c| c.name.trim().to_owned())
            .filter(|name| !name.is_empty());
        if let Some(name) = named.or_else(|| {
            doc.track(doc.clip_track(clip)?)
                .map(|track| track.name().to_owned())
        }) {
            return name;
        }
    }
    path.file_stem()
        .map_or_else(String::new, |s| s.to_string_lossy().into_owned())
}

/// What the Session clip with key `path` holds, if `path` is one and its set
/// can be read.
#[must_use]
pub fn clip_kind(path: &Path) -> Option<ClipFileKind> {
    let (set, Row::Clip(clip)) = parse_key(&path.to_string_lossy())? else {
        return None;
    };
    let opened = open(&set).ok()?;
    let found = opened.project.document.clip(clip)?;
    Some(clip_file_kind(&found.data))
}

/// The folder a set's rows name their files from: the project folder of the
/// set `path` is in or a key of.
#[must_use]
pub fn folder_of(path: &Path) -> Option<PathBuf> {
    let set = match parse_key(&path.to_string_lossy()) {
        Some((set, _)) => set,
        None => path.to_path_buf(),
    };
    std::path::absolute(set)
        .ok()
        .and_then(|set| set.parent().map(Path::to_path_buf))
}

/// A new, empty set with the tempo, time signatures and scenes of `doc` and
/// no tracks (R11), or with `scenes` plain scenes when given (R12).
fn blank_like(doc: &Document, scenes: Option<usize>) -> Result<Editor, String> {
    let mut editor = Editor::new();
    let failed = |e: bl_model::EditError| e.to_string();
    editor
        .apply(Command::SetTempo {
            tempo: doc.settings().tempo.clone(),
        })
        .map_err(failed)?;
    editor
        .apply(Command::SetTimeSignatures {
            time_signatures: doc.settings().time_signatures.clone(),
        })
        .map_err(failed)?;
    let old: Vec<SceneId> = editor.document().scenes().map(|s| s.id).collect();
    for scene in old {
        editor
            .apply(Command::DeleteScene { scene })
            .map_err(failed)?;
    }
    let sources: Vec<&bl_model::Scene> = doc.scenes().collect();
    let count = scenes.unwrap_or(sources.len());
    for index in 0..count {
        let id: SceneId = editor.new_id();
        editor
            .apply(Command::CreateScene { id, index })
            .map_err(failed)?;
        // A set's own scenes keep their names, tempos and follow actions;
        // the scenes of a set made from clips are plain.
        let Some(source) = sources.get(index).filter(|_| scenes.is_none()) else {
            continue;
        };
        if !source.name.is_empty() {
            editor
                .apply(Command::RenameScene {
                    scene: id,
                    name: source.name.clone(),
                })
                .map_err(failed)?;
        }
        if source.tempo.is_some() {
            editor
                .apply(Command::SetSceneTempo {
                    scene: id,
                    tempo: source.tempo,
                })
                .map_err(failed)?;
        }
        if source.signature.is_some() {
            editor
                .apply(Command::SetSceneTimeSignature {
                    scene: id,
                    signature: source.signature,
                })
                .map_err(failed)?;
        }
        let follow = source.follow;
        if follow != bl_model::FollowActions::default() {
            editor
                .apply(Command::SetSceneFollowActions {
                    scene: id,
                    patch: bl_model::FollowPatch {
                        enabled: Some(follow.enabled),
                        action_a: Some(follow.action_a),
                        action_b: Some(follow.action_b),
                        jump_a: Some(follow.jump_a),
                        jump_b: Some(follow.jump_b),
                        chance_a: Some(follow.chance_a),
                        linked: Some(follow.linked),
                        multiplier: Some(follow.multiplier),
                        time: Some(follow.time),
                    },
                })
                .map_err(failed)?;
        }
    }
    Ok(editor)
}

/// What a new set is made of: its document, and what its tracks hold, for the
/// files they play.
#[derive(Debug)]
pub struct Built {
    /// The new set.
    pub editor: Editor,
    /// What its tracks and return tracks hold.
    pub data: Vec<TrackData>,
}

/// The tracks a set made from `tracks` holds: those given less Main, the
/// return tracks and any inside another one given, in display order (R11).
#[must_use]
pub fn chosen_tracks(doc: &Document, tracks: &[TrackId]) -> Vec<TrackId> {
    tracks::duplicated(doc, tracks)
        .into_iter()
        .filter(|&id| doc.track(id).is_some_and(|t| t.kind() != TrackKind::Return))
        .collect()
}

/// A set holding `tracks` of `doc` as R5 adds them, with the return tracks of
/// `doc` and its tempo, time signature and scenes (R11). A group's tracks come
/// with it; Main and the return tracks can't be chosen.
///
/// # Errors
///
/// A message if no track can be saved or the new set can't be built.
pub fn build_from_tracks(doc: &Document, tracks: &[TrackId]) -> Result<Built, String> {
    let chosen = chosen_tracks(doc, tracks);
    if chosen.is_empty() {
        return Err("there's no track to save: Main and return tracks can't be saved".to_owned());
    }
    let mut data: Vec<TrackData> = chosen
        .iter()
        .filter_map(|&id| tracks::snapshot(doc, id))
        .collect();
    data.extend(doc.returns().filter_map(|id| tracks::snapshot(doc, id)));
    build(doc, data, None)
}

/// A set holding the Session clips `clips` of `doc` (R12): a track for each
/// track they are on, in the order of `doc`, with its kind, name, mixer and
/// devices, and each clip in the scene as far below the first clip's scene as
/// it was.
///
/// # Errors
///
/// A message if a clip isn't a Session clip of `doc`, or the new set can't be
/// built.
pub fn build_from_clips(doc: &Document, clips: &[ClipId]) -> Result<Built, String> {
    let mut held: Vec<(TrackId, ClipId, usize)> = Vec::new();
    for &clip in clips {
        let track = doc
            .clip_track(clip)
            .ok_or_else(|| format!("no clip has the ID {}", clip.0.get()))?;
        let position = doc
            .track(track)
            .and_then(|t| t.slots().find(|(_, c)| c.id == clip))
            .and_then(|(scene, _)| doc.scene_position(scene))
            .ok_or_else(|| format!("clip {} isn't a Session clip", clip.0.get()))?;
        held.push((track, clip, position));
    }
    let first = held
        .iter()
        .map(|&(.., position)| position)
        .min()
        .ok_or_else(|| "there's no clip to save".to_owned())?;
    let last = held
        .iter()
        .map(|&(.., position)| position)
        .max()
        .unwrap_or(first);
    let mut data = Vec::new();
    for track in doc.track_order() {
        let Some(mut one) =
            tracks::snapshot(doc, track).filter(|_| held.iter().any(|h| h.0 == track))
        else {
            continue;
        };
        // Only the chosen clips, their devices and the mixer come along: not
        // the track's other clips, its automation, its routing, its sends or
        // the tracks inside it (R12).
        one.slots = held
            .iter()
            .filter(|h| h.0 == track)
            .filter_map(|&(_, clip, position)| {
                let source = doc
                    .track(track)?
                    .slots()
                    .find(|(_, c)| c.id == clip)?
                    .1
                    .clone();
                Some((position - first, source))
            })
            .collect();
        one.clips.clear();
        one.take_lanes.clear();
        one.automation.clear();
        one.removed_stop_buttons.clear();
        one.sends.clear();
        one.children.clear();
        one.frozen = None;
        one.routing = Routing::new(one.kind);
        data.push(one);
    }
    build(doc, data, Some(last - first + 1))
}

fn build(doc: &Document, data: Vec<TrackData>, scenes: Option<usize>) -> Result<Built, String> {
    let mut editor = blank_like(doc, scenes)?;
    let source_returns: Vec<TrackId> = doc.returns().collect();
    tracks::copy_tracks(
        &mut editor,
        &data,
        Place {
            parent: None,
            index: 0,
            returns: 0,
        },
        Some(source_returns),
        BUILD_STEP,
    )?;
    Ok(Built { editor, data })
}

/// A set that was written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Saved {
    /// The set file.
    pub path: PathBuf,
    /// Its name, without the extension.
    pub name: String,
    /// How many audio files were copied into its project folder.
    pub copied: usize,
}

/// Writes `built` as a set named `name` in `folder`, placed as Save As places
/// a set (R13), its files copied into the project folder's `Samples/Imported`
/// or named from where they are, as `copy` says (R14). `files` are the files
/// the open set's tracks may play: hash, path and size.
///
/// # Errors
///
/// A message if a file can't be copied or the set can't be written; nothing
/// of the set is then left, though files already copied stay.
pub fn write(
    built: &Built,
    store: &MediaStore,
    folder: &Path,
    name: &str,
    copy: bool,
) -> Result<Saved, String> {
    let name = bl_project::preset::file_safe(name);
    // A set already there under this name is kept: the new one is numbered,
    // as a clip file's name is (Save As would replace it).
    let mut placement = place_set(&folder.join(&name))?;
    for n in 2..=1_000 {
        if !placement.path.exists() {
            break;
        }
        placement = place_set(&folder.join(format!("{name} {n}")))?;
    }
    let home = ProjectFolder::of_set(&placement.path).root().to_path_buf();
    let used: BTreeSet<ContentHash> = store_hashes(store, &built.data);
    let (media, copied) = clip_files::collect(store, &used, &home, &home, copy)?;
    let project = Project::from_editor(&built.editor, media);
    let mut set = SetFile::new(placement.path.clone());
    if let Err(error) = set.save(&project) {
        if let Some(created) = &placement.new_folder {
            // Only an empty folder goes, so nothing but what this made does.
            let _ = std::fs::remove_dir(created);
        }
        return Err(error.to_string());
    }
    Ok(Saved {
        name: placement
            .path
            .file_stem()
            .map_or_else(String::new, |stem| stem.to_string_lossy().into_owned()),
        path: placement.path,
        copied,
    })
}

/// The files of `store` that `data` plays.
fn store_hashes(store: &MediaStore, data: &[TrackData]) -> BTreeSet<ContentHash> {
    let used = crate::tracks::media_hashes(data);
    store
        .entries()
        .into_iter()
        .map(|entry| ContentHash(*entry.hash.as_bytes()))
        .filter(|hash| used.contains(hash))
        .collect()
}

/// Whether writing the set would copy a file: any file the tracks play lies
/// outside the project folder the set would land in. Under Ask that is when
/// the person is asked (R14).
#[must_use]
pub fn uses_outside(data: &[TrackData], store: &MediaStore, folder: &Path, name: &str) -> bool {
    let Ok(placement) = place_set(&folder.join(bl_project::preset::file_safe(name))) else {
        return false;
    };
    let home = ProjectFolder::of_set(&placement.path).root().to_path_buf();
    let home = std::path::absolute(&home).unwrap_or(home);
    store_hashes(store, data).into_iter().any(|hash| {
        store
            .entry(bl_media::ContentHash::from_bytes(hash.0))
            .filter(|entry| entry.path.is_file())
            .is_some_and(|entry| {
                !std::path::absolute(&entry.path)
                    .unwrap_or(entry.path)
                    .starts_with(&home)
            })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Spec: set-exchange R1.
    #[test]
    fn a_row_key_names_its_set_and_row() {
        let set = Path::new("/Music/Song Project/Song.blset");
        for row in [
            Row::Track(TrackId(bl_core::id::Id::new(7).unwrap())),
            Row::Devices(TrackId(bl_core::id::Id::new(12).unwrap())),
            Row::Clip(ClipId(bl_core::id::Id::new(40).unwrap())),
        ] {
            assert_eq!(
                parse_key(&key(set, row)),
                Some((set.to_path_buf(), row)),
                "{row:?}"
            );
        }
        for not in [
            "/a/Song.blset",
            "/a/Kick#1.wav",
            "/a/Song.blset#x4",
            "/a/Song.blset#t0",
        ] {
            assert_eq!(parse_key(not), None, "{not}");
        }
    }
}

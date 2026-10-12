//! Set files, project folders, and the factory default set.
//!
//! The shell opens and saves sets through a [`SetStore`]. The app's store is
//! [`ProjectFiles`], which reads and writes Beatlayer's set files with
//! `bl-project` (WP-0.7): atomic saves, the last 10 replaced versions kept
//! in the project folder's `Backup/`, and every load checked. Save Set As
//! puts a set in a project folder as Live does ([`place_set`]: a new
//! `<name> Project` folder, or a project folder that's already there).
//! Tests can pass a store of their own.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::io;
use std::path::{Component, Path, PathBuf};

use bl_model::{ContentHash, DeviceWindows, Editor, TrackId, TrackKind, registry};
use bl_project::{
    Media, MediaPath, MediaRef, Project, ProjectFolder, SET_EXTENSION, SetFile, is_set,
};
use serde_json::json;

/// Reads and writes set files, and remembers the open set's file between
/// saves.
pub trait SetStore {
    /// Reads the set at `path`, a set file or a project folder holding one
    /// set, into an editor with an empty undo history. Returns the editor
    /// and the set file's path. The store then treats that file as the open
    /// set's.
    ///
    /// # Errors
    ///
    /// A message saying why the set couldn't be read. The store's open set
    /// is then unchanged.
    fn open(&mut self, path: &Path) -> Result<(Editor, PathBuf), String>;

    /// Writes the editor's document to `path`, replacing any file there:
    /// the open set's next revision when `path` is its file, otherwise a
    /// new file, which becomes the open set's. Save Set uses it with the
    /// set's own file. Returns warnings about things that went wrong
    /// without stopping the save.
    ///
    /// # Errors
    ///
    /// A message saying why the set couldn't be written. Nothing at `path`
    /// changes, and the store's open set is unchanged.
    fn save(&mut self, path: &Path, editor: &Editor) -> Result<Vec<String>, String>;

    /// Saves the editor's document where Save Set As puts a set the person
    /// asked to save at `chosen` (a path from a save dialog, or an agent's),
    /// which may be inside a new project folder rather than at `chosen`
    /// itself. The saved file becomes the open set's. Returns the file's
    /// path and warnings about things that went wrong without stopping the
    /// save.
    ///
    /// # Errors
    ///
    /// A message saying why the set couldn't be written. No set file
    /// changes, no new project folder is left behind, and the store's open
    /// set is unchanged.
    fn save_as(&mut self, chosen: &Path, editor: &Editor)
    -> Result<(PathBuf, Vec<String>), String>;

    /// Forgets the open set's file, as a new set has none.
    fn close(&mut self);

    /// Records a file the set's clips play (WP-1.10), by its content hash,
    /// its path on disk, and its size, so the next save lists it in the
    /// set's media table. A store that keeps no media table ignores it.
    fn add_media(&mut self, hash: ContentHash, path: &Path, size: u64) {
        let _ = (hash, path, size);
    }

    /// The open set's media files with their paths on disk, for the media
    /// store after the set is opened (WP-1.10).
    fn media_files(&self) -> Vec<(ContentHash, PathBuf, u64)> {
        Vec::new()
    }

    /// Gives the store where floating devices' windows sit (WP-2.5b), for
    /// the next save to write beside the document. A store that keeps no
    /// window frames ignores it.
    fn set_windows(&mut self, windows: DeviceWindows) {
        let _ = windows;
    }

    /// Where the open set's floating devices' windows sit, as it was saved.
    fn windows(&self) -> DeviceWindows {
        DeviceWindows::new()
    }
}

/// Beatlayer's set files, through `bl-project`. It keeps the open set's
/// file (for its revision count) and its media table, which the shell
/// doesn't edit yet, so a save writes back what the load read.
#[derive(Debug, Default)]
pub struct ProjectFiles {
    set: Option<SetFile>,
    media: Media,
    /// Files added since the set was opened (WP-1.10), by their paths on
    /// disk, for the next save's media table.
    added: BTreeMap<ContentHash, (PathBuf, u64)>,
    /// Floating devices' window frames (WP-2.5b): the open set's, then the
    /// shell's for the next save.
    windows: DeviceWindows,
}

impl ProjectFiles {
    /// A store with no set open.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The open set's file and what its last load or save said about it,
    /// if it has one.
    #[must_use]
    pub fn set_file(&self) -> Option<&SetFile> {
        self.set.as_ref()
    }
}

impl SetStore for ProjectFiles {
    fn open(&mut self, path: &Path) -> Result<(Editor, PathBuf), String> {
        let (set, project) = SetFile::open(path).map_err(|error| error.to_string())?;
        let editor = project.editor();
        let opened = set.path().to_owned();
        self.set = Some(set);
        self.media = project.media;
        self.windows = project.windows;
        self.added.clear();
        Ok((editor, opened))
    }

    fn save(&mut self, path: &Path, editor: &Editor) -> Result<Vec<String>, String> {
        self.write(path, editor, self.media.clone())
    }

    fn save_as(
        &mut self,
        chosen: &Path,
        editor: &Editor,
    ) -> Result<(PathBuf, Vec<String>), String> {
        let placement = place_set(chosen)?;
        let mut media = self.media.clone();
        let note = match &self.set {
            Some(set) => repoint_media(
                &mut media,
                set.folder().root(),
                ProjectFolder::of_set(&placement.path).root(),
            ),
            None => None,
        };
        match self.write(&placement.path, editor, media) {
            Ok(mut warnings) => {
                warnings.extend(note);
                Ok((placement.path, warnings))
            }
            Err(error) => {
                if let Some(folder) = &placement.new_folder {
                    // Spec: app-shell R20. Only an empty folder can go, so
                    // this never removes anything but what the save made.
                    let _ = std::fs::remove_dir(folder);
                }
                Err(error)
            }
        }
    }

    fn close(&mut self) {
        self.set = None;
        self.media = Media::new();
        self.windows = DeviceWindows::new();
        self.added.clear();
    }

    fn set_windows(&mut self, windows: DeviceWindows) {
        self.windows = windows;
    }

    fn windows(&self) -> DeviceWindows {
        self.windows.clone()
    }

    fn add_media(&mut self, hash: ContentHash, path: &Path, size: u64) {
        self.added.insert(hash, (path.to_path_buf(), size));
    }

    fn media_files(&self) -> Vec<(ContentHash, PathBuf, u64)> {
        let folder = self
            .set
            .as_ref()
            .and_then(|set| set.path().parent().map(Path::to_path_buf));
        let mut files: Vec<(ContentHash, PathBuf, u64)> = folder
            .map(|folder| {
                self.media
                    .iter()
                    .filter_map(|file| Some((file.hash, file.path.resolve(&folder)?, file.size)))
                    .collect()
            })
            .unwrap_or_default();
        files.extend(
            self.added
                .iter()
                .map(|(hash, (path, size))| (*hash, path.clone(), *size)),
        );
        files
    }
}

impl ProjectFiles {
    /// Saves to `path` with this media table, which becomes the open set's.
    fn write(
        &mut self,
        path: &Path,
        editor: &Editor,
        mut media: Media,
    ) -> Result<Vec<String>, String> {
        let warnings = self.add_media_to(&mut media, path);
        let project = Project::from_editor(editor, media).with_windows(&self.windows);
        let mut set = self
            .set
            .clone()
            .unwrap_or_else(|| SetFile::new(path.to_owned()));
        let report = if set.path() == path {
            set.save(&project)
        } else {
            set.save_as(path.to_owned(), &project)
        }
        .map_err(|error| error.to_string())?;
        self.set = Some(set);
        self.media = project.media;
        Ok(warnings.into_iter().chain(report.warnings).collect())
    }

    /// Lists the files the set gained (WP-1.10) in `media`, by their paths
    /// from the folder of the set file at `path`. Returns a warning for each
    /// file that can't be reached from there.
    fn add_media_to(&self, media: &mut Media, path: &Path) -> Vec<String> {
        let mut warnings = Vec::new();
        let folder = std::path::absolute(path)
            .ok()
            .and_then(|p| p.parent().map(Path::to_path_buf));
        for (hash, (file, size)) in &self.added {
            let relative = folder.as_deref().and_then(|folder| {
                std::path::absolute(file)
                    .ok()
                    .and_then(|file| MediaPath::between(folder, &file))
            });
            match relative {
                Some(relative) => {
                    media.insert(MediaRef {
                        hash: *hash,
                        path: relative,
                        size: *size,
                    });
                }
                None => warnings.push(format!(
                    "{} can't be reached from the project folder, so the set doesn't record \
                     where it is",
                    file.display()
                )),
            }
        }
        warnings
    }
}

/// Where Save Set As puts a set: see [`place_set`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Placement {
    /// The set file's path.
    pub path: PathBuf,
    /// The project folder the save makes, if the set doesn't join one
    /// that's there already.
    pub new_folder: Option<PathBuf>,
}

/// The most numbered project folders tried for one name (app-shell R20).
const MOST_PROJECT_FOLDERS: u32 = 9_999;

/// Where Save Set As puts a set the person asked to save at `chosen`
/// (app-shell R17, R18, R20). The set's name is the chosen file's name
/// without the `.blset` extension, which a name without it gets.
///
/// - If the chosen folder is a project folder, one with a set file
///   directly inside it, the set joins that project: it's saved there as
///   `<name>.blset`, replacing any set of that name (R18).
/// - Otherwise the save makes a project folder for the set in the chosen
///   folder, `<name> Project`, and saves it inside as `<name>.blset`
///   (R17). If something already has that name, the folder is
///   `<name> Project 2`, or the next number that's free (R20).
///
/// Nothing on disk changes: this only looks.
///
/// # Errors
///
/// A message saying why, if `chosen` names no file, or the chosen folder
/// or the names in it can't be read.
pub fn place_set(chosen: &Path) -> Result<Placement, String> {
    let file = chosen
        .file_name()
        .and_then(OsStr::to_str)
        .ok_or_else(|| format!("{} doesn't name a set file", chosen.display()))?;
    let (name, file) = if is_set(chosen) {
        let stem = chosen.file_stem().and_then(OsStr::to_str).unwrap_or(file);
        (stem.to_owned(), file.to_owned())
    } else {
        (file.to_owned(), format!("{file}.{SET_EXTENSION}"))
    };
    let parent = chosen.parent().unwrap_or_else(|| Path::new(""));
    if is_project_folder(parent)? {
        return Ok(Placement {
            path: parent.join(file),
            new_folder: None,
        });
    }
    let folder = free_project_folder(parent, &name)?;
    Ok(Placement {
        path: folder.join(file),
        new_folder: Some(folder),
    })
}

/// Whether `folder` is a project folder: a folder with a set file directly
/// inside it (app-shell R18). A folder that isn't there isn't one.
fn is_project_folder(folder: &Path) -> Result<bool, String> {
    let readable = if folder.as_os_str().is_empty() {
        Path::new(".")
    } else {
        folder
    };
    if !readable.is_dir() {
        return Ok(false);
    }
    ProjectFolder::new(readable)
        .sets()
        .map(|sets| !sets.is_empty())
        .map_err(|error| format!("couldn't read the folder {}: {error}", readable.display()))
}

/// The first of `<name> Project`, `<name> Project 2`, and so on that
/// nothing in `parent` is called (app-shell R20).
fn free_project_folder(parent: &Path, name: &str) -> Result<PathBuf, String> {
    for n in 1..=MOST_PROJECT_FOLDERS {
        let folder = if n == 1 {
            ProjectFolder::named(parent, name).root().to_owned()
        } else {
            parent.join(format!("{name} Project {n}"))
        };
        // A link counts as taken even if it leads nowhere.
        match std::fs::symlink_metadata(&folder) {
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(folder),
            Err(error) => {
                return Err(format!(
                    "couldn't check for the project folder {}: {error}",
                    folder.display()
                ));
            }
        }
    }
    Err(format!(
        "every name from \"{name} Project\" to \"{name} Project {MOST_PROJECT_FOLDERS}\" is taken in {}",
        parent.display()
    ))
}

/// Re-points the media table's paths, relative to the project folder
/// `from`, so they're relative to `to` and name the same files (app-shell
/// R21). Returns a note for the Status Bar if any path couldn't be.
fn repoint_media(media: &mut Media, from: &Path, to: &Path) -> Option<String> {
    let (Ok(from), Ok(to)) = (std::path::absolute(from), std::path::absolute(to)) else {
        return (!media.is_empty()).then(|| {
            "the media files' paths couldn't be re-pointed to the new project folder".to_owned()
        });
    };
    let (from, to) = (lexical(&from), lexical(&to));
    if from == to {
        return None;
    }
    let mut stuck = 0usize;
    let moved: Vec<MediaRef> = media
        .iter()
        .map(|file| {
            let path = file
                .path
                .resolve(&from)
                .and_then(|path| MediaPath::between(&to, &lexical(&path)));
            if let Some(path) = path {
                MediaRef {
                    path,
                    ..file.clone()
                }
            } else {
                stuck += 1;
                file.clone()
            }
        })
        .collect();
    for file in moved {
        media.insert(file);
    }
    match stuck {
        0 => None,
        1 => {
            Some("1 media file's path couldn't be re-pointed to the new project folder".to_owned())
        }
        n => Some(format!(
            "{n} media files' paths couldn't be re-pointed to the new project folder"
        )),
    }
}

/// `path` with `.` dropped and each `..` taking off the name before it,
/// from the path alone, as media paths are worked out.
fn lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => match out.components().next_back() {
                Some(Component::Normal(_)) => {
                    out.pop();
                }
                // `..` above the root is the root.
                Some(Component::RootDir | Component::Prefix(_)) => {}
                _ => out.push(".."),
            },
            other => out.push(other),
        }
    }
    out
}

/// The factory default set: two MIDI tracks, two audio tracks, and two
/// return tracks, as a new set in Live has (layout.md, "Default state"),
/// with an empty undo history. The shell shows it in the Arrangement View
/// (IMP-04, D20).
#[must_use]
pub fn factory_default_set() -> Editor {
    let mut editor = Editor::new();
    let tracks = [
        (TrackKind::Midi, "1 MIDI"),
        (TrackKind::Midi, "2 MIDI"),
        (TrackKind::Audio, "3 Audio"),
        (TrackKind::Audio, "4 Audio"),
        (TrackKind::Return, "A Return"),
        (TrackKind::Return, "B Return"),
    ];
    let (mut top, mut returns) = (0usize, 0usize);
    for (kind, name) in tracks {
        let id: TrackId = editor.new_id();
        let index = if kind == TrackKind::Return {
            returns += 1;
            returns - 1
        } else {
            top += 1;
            top - 1
        };
        registry::run(
            &mut editor,
            "track.create",
            json!({ "id": id, "kind": kind, "name": name, "index": index }),
        )
        .expect("the factory default set's tracks are valid");
    }
    editor
        .clear_history()
        .expect("no transaction is open while building the default set");
    editor
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_set_has_four_tracks_and_two_returns_and_no_history() {
        let editor = factory_default_set();
        let doc = editor.document();
        let names: Vec<&str> = doc
            .top_level()
            .map(|id| doc.track(id).unwrap().name())
            .collect();
        assert_eq!(names, ["1 MIDI", "2 MIDI", "3 Audio", "4 Audio"]);
        assert_eq!(doc.returns().len(), 2);
        assert!(!editor.can_undo());
    }

    #[test]
    fn lexical_paths_drop_dots_and_take_names_off_for_dot_dots() {
        let lexical = |path: &str| lexical(Path::new(path));
        assert_eq!(lexical("/a/b/../c/./d"), Path::new("/a/c/d"));
        assert_eq!(lexical("/../a"), Path::new("/a"));
        assert_eq!(lexical("../a/.."), Path::new(".."));
        assert_eq!(lexical("a/../../b"), Path::new("../b"));
    }

    #[test]
    fn a_path_that_names_no_file_has_no_place() {
        assert!(place_set(Path::new("/")).is_err());
        assert!(place_set(Path::new("")).is_err());
    }
}

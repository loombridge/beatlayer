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

use bl_core::id::IdAllocator;
use bl_model::{
    ClipFolds, ContentHash, DEFAULT_SCENE_COUNT, DeviceWindows, Document, Editor, LaunchSettings,
    MultisamplerViews, RackViewMap, SceneId, TrackId, TrackKind, registry,
};
use bl_project::midi_tools::MidiToolSettings;
use bl_project::{
    Media, MediaPath, MediaRef, Project, ProjectFolder, SET_EXTENSION, SaveOptions, SetFile,
    SetInfo, is_set,
};
use bl_shell_spec::file_manager::ExternalGroup;
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

    /// The open set's file and what its last load or save said about it,
    /// if it has one.
    fn set_file(&self) -> Option<&SetFile> {
        None
    }

    /// The set as it would be saved: the editor's document with the open
    /// set's media table and views (WP-6.62, the crash-recovery spec R21).
    /// The journal holds this, so a recovered set is the set as it was.
    fn project(&self, editor: &Editor) -> Project {
        Project::from_editor(editor, Media::new())
    }

    /// Takes over a set the app didn't open from a file: the recovery's
    /// set file, and the project the journal held (WP-6.62, the
    /// crash-recovery spec R3).
    fn adopt(&mut self, set: Option<SetFile>, project: Project) {
        let _ = (set, project);
    }

    /// Reads the set at `path` into an editor with an empty undo history,
    /// without making it the open set: the File Manager opens a backup as
    /// the set, which keeps its own file (WP-6.63, R18). A store that
    /// keeps no files can't read one.
    ///
    /// # Errors
    ///
    /// A message saying why the set couldn't be read.
    fn read(&mut self, path: &Path) -> Result<Editor, String> {
        let _ = path;
        Err("this set store can't read a set".to_owned())
    }

    /// Records a file the set's clips play (WP-1.10), by its content hash,
    /// its path on disk, and its size, so the next save lists it in the
    /// set's media table. A store that keeps no media table ignores it.
    fn add_media(&mut self, hash: ContentHash, path: &Path, size: u64) {
        let _ = (hash, path, size);
    }

    /// Records a file the set's clips play that is a reversed copy of
    /// another, and the file it was made from (the audio clip editing spec,
    /// R10), so reversing the clip again gives it back that file. A store
    /// that keeps no media table ignores it.
    fn add_reversed_media(&mut self, hash: ContentHash, path: &Path, size: u64, from: ContentHash) {
        let _ = (hash, path, size, from);
    }

    /// The file a reversed copy was made from, if the set's media table says
    /// so (R10).
    fn reversed_from(&self, _hash: &ContentHash) -> Option<ContentHash> {
        None
    }

    /// The open set's media files with their paths on disk, for the media
    /// store after the set is opened (WP-1.10).
    fn media_files(&self) -> Vec<(ContentHash, PathBuf, u64)> {
        Vec::new()
    }

    /// Tells the store where an unsaved set's processed files are written
    /// (the Temporary Folder, WP-4.20), so the first save can move them into
    /// the project. A store that keeps no media table ignores it.
    fn set_temporary_folder(&mut self, folder: PathBuf) {
        let _ = folder;
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

    /// Gives the store which views each rack shows (WP-5.2), for the next
    /// save to write beside the document. A store that keeps no rack views
    /// ignores it.
    fn set_rack_views(&mut self, views: RackViewMap) {
        let _ = views;
    }

    /// Which views each of the open set's racks shows, as it was saved.
    fn rack_views(&self) -> RackViewMap {
        RackViewMap::new()
    }

    /// Gives the store what each multisampler's views show (WP-5.13), for
    /// the next save to write beside the document. A store that keeps none
    /// ignores it.
    fn set_multisampler_views(&mut self, views: MultisamplerViews) {
        let _ = views;
    }

    /// What each of the open set's multisamplers' views showed, as it was
    /// saved.
    fn multisampler_views(&self) -> MultisamplerViews {
        MultisamplerViews::new()
    }

    /// Gives the store which clips' MIDI Note Editors are folded (WP-2.3),
    /// for the next save to write beside the document. A store that keeps
    /// no folds ignores it.
    fn set_folds(&mut self, folds: ClipFolds) {
        let _ = folds;
    }

    /// Which of the open set's clips' MIDI Note Editors are folded, as it
    /// was saved.
    fn folds(&self) -> ClipFolds {
        ClipFolds::new()
    }

    /// Gives the store the MIDI Tools' chosen tools and settings (WP-6.11),
    /// for the next save to write beside the document. A store that keeps
    /// no MIDI Tool settings ignores it.
    fn set_midi_tools(&mut self, settings: MidiToolSettings) {
        let _ = settings;
    }

    /// The open set's MIDI Tool choices and settings, as it was saved.
    fn midi_tools(&self) -> MidiToolSettings {
        MidiToolSettings::default()
    }

    /// Gives the store the global launch quantisation and its switch
    /// (WP-3.2), for the next save to write beside the document. A store
    /// that keeps no launch settings ignores it.
    fn set_launch(&mut self, launch: LaunchSettings) {
        let _ = launch;
    }

    /// The open set's global launch quantisation and its switch, as it was
    /// saved.
    fn launch(&self) -> LaunchSettings {
        LaunchSettings::default()
    }

    /// Writes `editor` to `path` as a copy without making it the open set:
    /// the default set and templates (WP-2.25). Returns warnings about
    /// things that went wrong without stopping the write. A store that
    /// keeps no file can't write one.
    ///
    /// # Errors
    ///
    /// A message saying why the copy couldn't be written.
    fn save_copy(&mut self, path: &Path, editor: &Editor) -> Result<Vec<String>, String> {
        let _ = (path, editor);
        Err(COPY_UNSUPPORTED.to_owned())
    }

    /// Copies every file the open set uses that lives outside its project
    /// folder into the project's `Samples/Imported` folder, re-pointing the
    /// media table at the copies (WP-2.25, Collect All and Save). Returns
    /// how many files were copied. A store that keeps no media table copies
    /// nothing.
    ///
    /// # Errors
    ///
    /// A message saying why a copy couldn't be made.
    fn collect(&mut self) -> Result<usize, String> {
        Ok(0)
    }

    /// Copies the files the open set uses that live outside its project
    /// folder and are in the groups given, into the project's
    /// `Samples/Imported` folder, re-pointing the media table at the copies
    /// (WP-6.63, Collect All and Save, R15). The User Library's path says
    /// which files are in it, so they can be left alone. Returns how many
    /// files were copied. A store that keeps no media table copies nothing.
    ///
    /// # Errors
    ///
    /// A message saying why a copy couldn't be made.
    fn collect_groups(
        &mut self,
        groups: &[ExternalGroup],
        library: Option<&Path>,
    ) -> Result<usize, String> {
        let _ = (groups, library);
        self.collect()
    }

    /// Drops a file from the open set's media table, so the next save
    /// doesn't list it (WP-6.63: a replaced file isn't used any more).
    fn forget_media(&mut self, hash: ContentHash) {
        let _ = hash;
    }

    /// Re-points the open set's media table at the files at `found`, by
    /// their hashes: what a missing-files search gives back (WP-2.25). A
    /// store that keeps no media table ignores it.
    fn relink(&mut self, found: &[(ContentHash, PathBuf)]) {
        let _ = found;
    }
}

/// Why [`SetStore::save_copy`] failed: the store keeps no file of its own.
const COPY_UNSUPPORTED: &str = "this set store can't write a copy of a set";

/// Beatlayer's set files, through `bl-project`. It keeps the open set's
/// file (for its revision count) and its media table, which the shell
/// doesn't edit yet, so a save writes back what the load read.
#[derive(Debug, Default)]
pub struct ProjectFiles {
    set: Option<SetFile>,
    media: Media,
    /// Files added since the set was opened (WP-1.10), by their paths on
    /// disk, for the next save's media table.
    added: BTreeMap<ContentHash, (PathBuf, u64, Option<ContentHash>)>,
    /// What each multisampler's views show (WP-5.13): the open set's, then
    /// the shell's for the next save.
    multisamplers: MultisamplerViews,
    /// The Temporary Folder, where a set never saved has its processed
    /// files (WP-4.20).
    temporary: Option<PathBuf>,
    /// Floating devices' window frames (WP-2.5b): the open set's, then the
    /// shell's for the next save.
    windows: DeviceWindows,
    /// The views racks show (WP-5.2): the open set's, then the shell's for
    /// the next save.
    rack_views: RackViewMap,
    /// Folded MIDI Note Editors (WP-2.3): the open set's, then the shell's
    /// for the next save.
    folds: ClipFolds,
    /// The MIDI Tools' chosen tools and settings (WP-6.11): the open set's,
    /// then the shell's for the next save.
    midi_tools: MidiToolSettings,
    /// The global launch quantisation and its switch (WP-3.2): the open
    /// set's, then the shell's for the next save.
    launch: LaunchSettings,
}

impl ProjectFiles {
    /// A store with no set open.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl SetStore for ProjectFiles {
    fn open(&mut self, path: &Path) -> Result<(Editor, PathBuf), String> {
        let (set, mut project) = SetFile::open(path).map_err(|error| error.to_string())?;
        if set.info().is_some_and(SetInfo::before_relative_modulation) {
            bl_devices::relative_clip_envelopes(&mut project.document);
        }
        let editor = project.editor();
        let opened = set.path().to_owned();
        self.set = Some(set);
        self.media = project.media;
        self.windows = project.windows;
        self.rack_views = project.rack_views;
        self.multisamplers = project.multisamplers;
        self.folds = project.folds;
        self.midi_tools = project.midi_tools;
        self.launch = project.launch;
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
        // Spec: audio-clip-editing R5. A set never saved has its processed
        // files in the Temporary Folder; they move into the new project.
        let moved = if self.set.is_none() {
            self.move_processed(ProjectFolder::of_set(&placement.path).root())?
        } else {
            Vec::new()
        };
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
                self.move_back(moved);
                if let Some(folder) = &placement.new_folder {
                    // Spec: app-shell R20. Only an empty folder can go, so
                    // this never removes anything but what the save made.
                    let _ = std::fs::remove_dir(folder);
                }
                Err(error)
            }
        }
    }

    fn read(&mut self, path: &Path) -> Result<Editor, String> {
        let (set, mut project) = SetFile::open(path).map_err(|error| error.to_string())?;
        if set.info().is_some_and(SetInfo::before_relative_modulation) {
            bl_devices::relative_clip_envelopes(&mut project.document);
        }
        let editor = project.editor();
        // The set's song is the backup's now, so its media table is the
        // backup's. The files the set before it gained are forgotten, so a
        // later save doesn't list them.
        self.media = project.media;
        self.added.clear();
        Ok(editor)
    }

    fn close(&mut self) {
        self.set = None;
        self.media = Media::new();
        self.windows = DeviceWindows::new();
        self.rack_views = RackViewMap::new();
        self.multisamplers = MultisamplerViews::new();
        self.folds = ClipFolds::new();
        self.midi_tools = MidiToolSettings::default();
        self.launch = LaunchSettings::default();
        self.added.clear();
    }

    fn set_file(&self) -> Option<&SetFile> {
        self.set.as_ref()
    }

    fn project(&self, editor: &Editor) -> Project {
        let mut media = self.media.clone();
        if let Some(set) = &self.set {
            self.add_media_to(&mut media, set.path());
        }
        Project::from_editor(editor, media)
            .with_windows(&self.windows)
            .with_rack_views(&self.rack_views)
            .with_multisamplers(&self.multisamplers)
            .with_folds(&self.folds)
            .with_midi_tools(&self.midi_tools)
            .with_launch(self.launch)
    }

    fn adopt(&mut self, set: Option<SetFile>, project: Project) {
        self.set = set;
        self.media = project.media;
        self.windows = project.windows;
        self.rack_views = project.rack_views;
        self.multisamplers = project.multisamplers;
        self.folds = project.folds;
        self.midi_tools = project.midi_tools;
        self.launch = project.launch;
        self.added.clear();
    }

    fn set_multisampler_views(&mut self, views: MultisamplerViews) {
        self.multisamplers = views;
    }

    fn multisampler_views(&self) -> MultisamplerViews {
        self.multisamplers.clone()
    }

    fn set_windows(&mut self, windows: DeviceWindows) {
        self.windows = windows;
    }

    fn windows(&self) -> DeviceWindows {
        self.windows.clone()
    }

    fn set_rack_views(&mut self, views: RackViewMap) {
        self.rack_views = views;
    }

    fn rack_views(&self) -> RackViewMap {
        self.rack_views.clone()
    }

    fn set_folds(&mut self, folds: ClipFolds) {
        self.folds = folds;
    }

    fn folds(&self) -> ClipFolds {
        self.folds.clone()
    }

    fn set_midi_tools(&mut self, settings: MidiToolSettings) {
        self.midi_tools = settings;
    }

    fn midi_tools(&self) -> MidiToolSettings {
        self.midi_tools.clone()
    }

    fn set_launch(&mut self, launch: LaunchSettings) {
        self.launch = launch;
    }

    fn launch(&self) -> LaunchSettings {
        self.launch
    }

    fn add_media(&mut self, hash: ContentHash, path: &Path, size: u64) {
        self.added.insert(hash, (path.to_path_buf(), size, None));
    }

    fn add_reversed_media(&mut self, hash: ContentHash, path: &Path, size: u64, from: ContentHash) {
        self.added
            .insert(hash, (path.to_path_buf(), size, Some(from)));
    }

    fn reversed_from(&self, hash: &ContentHash) -> Option<ContentHash> {
        self.added
            .get(hash)
            .and_then(|(_, _, from)| *from)
            .or_else(|| self.media.reversed_from(hash))
    }

    fn set_temporary_folder(&mut self, folder: PathBuf) {
        self.temporary = Some(folder);
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
                .map(|(hash, (path, size, _))| (*hash, path.clone(), *size)),
        );
        files
    }

    fn save_copy(&mut self, path: &Path, editor: &Editor) -> Result<Vec<String>, String> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("couldn't create {}: {e}", parent.display()))?;
        }
        let mut media = self.media.clone();
        let mut warnings = self.add_media_to(&mut media, path);
        // The copy lives in a folder of its own, so its media paths are
        // re-pointed from the open set's folder, as Save Set As does.
        if let Some(from) = self.folder()
            && let Some(to) = std::path::absolute(path)
                .ok()
                .and_then(|path| path.parent().map(Path::to_path_buf))
            && let Some(note) = repoint_media(&mut media, &from, &to)
        {
            warnings.push(note);
        }
        let project = Project::from_editor(editor, media)
            .with_windows(&self.windows)
            .with_rack_views(&self.rack_views)
            .with_multisamplers(&self.multisamplers)
            .with_folds(&self.folds)
            .with_midi_tools(&self.midi_tools)
            .with_launch(self.launch);
        let mut set = SetFile::open(path)
            .map(|(set, _)| set)
            .unwrap_or_else(|_| SetFile::new(path.to_owned()));
        // A copy has no backup history of its own.
        let report = set
            .save_with(&project, &SaveOptions { backups: 0 })
            .map_err(|error| error.to_string())?;
        Ok(warnings.into_iter().chain(report.warnings).collect())
    }

    fn collect(&mut self) -> Result<usize, String> {
        self.collect_groups(
            &[
                ExternalGroup::OtherProject,
                ExternalGroup::UserLibrary,
                ExternalGroup::Elsewhere,
            ],
            None,
        )
    }

    fn collect_groups(
        &mut self,
        groups: &[ExternalGroup],
        library: Option<&Path>,
    ) -> Result<usize, String> {
        // Spec: file-management R4, R15. The set's external files are
        // copied into `Samples/Imported` and its media table re-pointed at
        // them.
        let Some(set) = self.set.clone() else {
            return Ok(0);
        };
        let Some(folder) = self.folder() else {
            return Ok(0);
        };
        let imported = ProjectFolder::new(&folder).imported();
        let mut media = self.media.clone();
        // Files added since the set was opened join the table first, so an
        // external one among them is collected too.
        let _ = self.add_media_to(&mut media, set.path());
        let mut copied = 0usize;
        for file in media.iter().cloned().collect::<Vec<_>>() {
            // Spec: app-shell R17. A path that doesn't climb out of the
            // project folder is already inside it.
            if file.path.is_inside() {
                continue;
            }
            let Some(source) = file.path.resolve(&folder) else {
                continue;
            };
            if !source.is_file() {
                continue;
            }
            if !groups.contains(&external_group(&source, library)) {
                continue;
            }
            std::fs::create_dir_all(&imported)
                .map_err(|e| format!("couldn't create {}: {e}", imported.display()))?;
            let name = source.file_name().map_or_else(
                || file.hash.to_string(),
                |name| name.to_string_lossy().into_owned(),
            );
            let destination = free_import(&imported, &name)?;
            if destination != source {
                std::fs::copy(&source, &destination).map_err(|e| {
                    format!(
                        "couldn't copy {} into {}: {e}",
                        source.display(),
                        imported.display()
                    )
                })?;
            }
            let Some(relative) = MediaPath::between(&folder, &destination) else {
                return Err(format!(
                    "couldn't record where {} went in the project",
                    destination.display()
                ));
            };
            media.insert(MediaRef {
                path: relative,
                ..file
            });
            copied += 1;
        }
        // Files that reached the table are re-pointed; one that couldn't is
        // left in `added`, so a later save still says it can't be reached.
        self.added.retain(|hash, _| media.get(hash).is_none());
        self.media = media;
        Ok(copied)
    }

    fn forget_media(&mut self, hash: ContentHash) {
        self.media.remove(&hash);
        self.added.remove(&hash);
    }

    fn relink(&mut self, found: &[(ContentHash, PathBuf)]) {
        let Some(folder) = self.folder() else {
            return;
        };
        for (hash, path) in found {
            let Some(relative) = MediaPath::between(&folder, &lexical(path)) else {
                continue;
            };
            if let Some(file) = self.media.get(hash).cloned() {
                self.media.insert(MediaRef {
                    path: relative,
                    ..file
                });
            }
        }
    }
}

impl ProjectFiles {
    /// Moves each file the set wrote into the Temporary Folder into
    /// `Samples/Processed/<kind>/` of the project folder `project`, under a
    /// free name, and points `added` at it. A file a DAWproject import
    /// extracted goes to `Samples/Imported/` instead (the DAWproject spec,
    /// R39). Returns where each came from, to put back if the save fails.
    fn move_processed(&mut self, project: &Path) -> Result<Vec<(ContentHash, PathBuf)>, String> {
        let Some(temporary) = self.temporary.clone() else {
            return Ok(Vec::new());
        };
        let project = ProjectFolder::new(project);
        let mut moved = Vec::new();
        for (hash, (path, size, reversed_from)) in self.added.clone() {
            let Some(rest) = path.strip_prefix(&temporary).ok() else {
                continue;
            };
            // `Imported/<...>/<name>`, as a DAWproject import writes them, or
            // `<kind>/<name>`, as the shell writes the rest.
            let mut parts = rest.components();
            let folder = if rest.starts_with("Imported") {
                project.imported()
            } else if let (Some(Component::Normal(kind)), Some(Component::Normal(_)), None) =
                (parts.next(), parts.next(), parts.next())
            {
                project.processed(&kind.to_string_lossy())
            } else {
                continue;
            };
            let Some(name) = rest.file_name() else {
                continue;
            };
            let placed = std::fs::create_dir_all(&folder)
                .map_err(|e| format!("couldn't create {}: {e}", folder.display()))
                .and_then(|()| free_import(&folder, &name.to_string_lossy()))
                .and_then(|destination| move_file(&path, &destination).map(|()| destination));
            match placed {
                Ok(destination) => {
                    self.added.insert(hash, (destination, size, reversed_from));
                    moved.push((hash, path));
                }
                Err(error) => {
                    self.move_back(moved);
                    return Err(error);
                }
            }
        }
        Ok(moved)
    }

    /// Puts files [`ProjectFiles::move_processed`] moved back where they
    /// were, after a save that failed.
    fn move_back(&mut self, moved: Vec<(ContentHash, PathBuf)>) {
        for (hash, from) in moved {
            if let Some((now, size, reversed_from)) = self.added.get(&hash).cloned()
                && move_file(&now, &from).is_ok()
            {
                self.added.insert(hash, (from, size, reversed_from));
            }
        }
    }

    /// The open set's project folder, the folder its file is in, if it has
    /// a file.
    fn folder(&self) -> Option<PathBuf> {
        let set = self.set.as_ref()?;
        let path = std::path::absolute(set.path()).ok()?;
        path.parent().map(Path::to_path_buf)
    }

    /// Saves to `path` with this media table, which becomes the open set's.
    fn write(
        &mut self,
        path: &Path,
        editor: &Editor,
        mut media: Media,
    ) -> Result<Vec<String>, String> {
        let warnings = self.add_media_to(&mut media, path);
        let project = Project::from_editor(editor, media)
            .with_windows(&self.windows)
            .with_rack_views(&self.rack_views)
            .with_multisamplers(&self.multisamplers)
            .with_folds(&self.folds)
            .with_midi_tools(&self.midi_tools)
            .with_launch(self.launch);
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
        for (hash, (file, size, reversed_from)) in &self.added {
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
                        reversed_from: *reversed_from,
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

/// Where a file outside the project lives (file-management R15): the group
/// the External Files section lists it in.
pub fn external_group(path: &Path, library: Option<&Path>) -> ExternalGroup {
    if library.is_some_and(|library| path.starts_with(library)) {
        return ExternalGroup::UserLibrary;
    }
    // The path as the disk has it, so a file outside the project isn't
    // taken for one inside it.
    let path = lexical(path);
    // Another project folder: one with a set file in it.
    for folder in path.ancestors().skip(1) {
        if ProjectFolder::new(folder)
            .sets()
            .is_ok_and(|sets| !sets.is_empty())
        {
            return ExternalGroup::OtherProject;
        }
    }
    ExternalGroup::Elsewhere
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

/// The most numbered copies tried for one file name (Collect All and Save).
const MOST_IMPORT_COPIES: u32 = 1_000;

/// A free path in `folder` for a copy of a file called `name`: the name
/// itself, or `<stem> 2.<ext>` and so on when something already has it, so
/// collecting never writes over a file (WP-2.25).
pub(crate) fn free_import(folder: &Path, name: &str) -> Result<PathBuf, String> {
    let path = Path::new(name);
    let stem = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    let extension = path
        .extension()
        .map(|extension| format!(".{}", extension.to_string_lossy()))
        .unwrap_or_default();
    for n in 1..=MOST_IMPORT_COPIES {
        let candidate = if n == 1 {
            folder.join(name)
        } else {
            folder.join(format!("{stem} {n}{extension}"))
        };
        match std::fs::symlink_metadata(&candidate) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(candidate),
            Err(error) => {
                return Err(format!(
                    "couldn't check for {}: {error}",
                    candidate.display()
                ));
            }
            Ok(_) => {}
        }
    }
    Err(format!(
        "every name for {name} up to \"{stem} {MOST_IMPORT_COPIES}{extension}\" is taken in {}",
        folder.display()
    ))
}

/// Moves a file, copying it when `from` and `to` are on different volumes.
fn move_file(from: &Path, to: &Path) -> Result<(), String> {
    std::fs::rename(from, to).or_else(|_| {
        std::fs::copy(from, to)
            .and_then(|_| std::fs::remove_file(from))
            .map_err(|e| format!("couldn't move {} to {}: {e}", from.display(), to.display()))
    })
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
pub(crate) fn lexical(path: &Path) -> PathBuf {
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
///
/// The tracks come first and the default scenes after them, so the set's
/// track IDs are the ones they have always been (the first track is 2); a
/// new, empty set takes its scenes first instead ([`Editor::new`]).
#[must_use]
pub fn factory_default_set() -> Editor {
    let mut ids = IdAllocator::new();
    let main = TrackId(ids.allocate());
    let mut editor = Editor::from_document(Document::new(main, []), ids);
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
    for index in 0..DEFAULT_SCENE_COUNT {
        let id: SceneId = editor.new_id();
        registry::run(
            &mut editor,
            "scene.create",
            json!({ "id": id, "index": index }),
        )
        .expect("the factory default set's scenes are valid");
    }
    editor
        .clear_history()
        .expect("no transaction is open while building the default set");
    editor
}

/// The set a new set starts from: the saved default set at `path` when it
/// can be read, otherwise [`factory_default_set`] (WP-2.25). File ▸ Save
/// Beatlayer Set As Default Set makes the open set the one a new set starts
/// from, as Live's does.
#[must_use]
pub fn new_set_from(path: Option<&Path>) -> Editor {
    path.and_then(|path| SetFile::open(path).ok())
        .map(|(set, mut project)| {
            if set.info().is_some_and(SetInfo::before_relative_modulation) {
                bl_devices::relative_clip_envelopes(&mut project.document);
            }
            project.editor()
        })
        .unwrap_or_else(factory_default_set)
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
        assert_eq!(doc.scene_count(), DEFAULT_SCENE_COUNT);
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

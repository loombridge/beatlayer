//! The File Manager (WP-6.63): the files a set, a project or the User
//! Library uses, the missing ones, the external ones, and the unused ones;
//! and the set commands around it: the backups, and saving a copy.
//!
//! The behaviour is in the file management spec, R10 to R21.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use bl_media::ContentHash;
use bl_model::{ClipData, Command, Document};
use bl_project::ProjectFolder;
use bl_shell_spec::file_manager::{
    self as ids, Backup, Backups, DeleteUnusedArgs, DeletedFiles, ExternalFiles, FileManagerState,
    FileManagerView, FileRow, FileSection, Location, OpenBackupArgs, ReplaceFileArgs, ReplacedFile,
    SaveCopyArgs, SavedCopy, Scope, Search, ToggleArgs, UnusedFile, UnusedFiles, UnusedKind,
};
use serde_json::Value;

use super::packs::same_folder;
use super::*;

/// The File Manager's state (WP-6.63): whether it shows, which files it
/// shows, and the Automatic Search's switches.
#[derive(Clone, Debug, Default)]
pub struct FileManager {
    /// Whether it shows.
    pub open: bool,
    /// Which files it shows.
    pub scope: Scope,
    /// The project folder Manage Project works on, when it isn't the open
    /// set's.
    pub project: Option<PathBuf>,
    /// The Automatic Search's switches and folder (R14).
    pub search: Search,
    /// The file a Hot-Swap button armed: the file to replace, and the
    /// candidates it offers (R11, R14).
    pub hot_swap: Option<HotSwap>,
}

/// A file a Hot-Swap button armed (R11, R14).
#[derive(Clone, Debug)]
pub struct HotSwap {
    /// The file to replace, by its content hash.
    pub hash: ContentHash,
}

impl Shell {
    /// The items a menu row holds now, for the rows the shell fills in
    /// (WP-6.63): File ▸ Open Beatlayer Set Backups lists the open set's
    /// backups, newest first.
    pub fn menu_items(&self, row: &str) -> Vec<bl_shell_spec::menus::DynamicItem> {
        if !bl_shell_spec::menus::DYNAMIC_ROWS.contains(&row) {
            return Vec::new();
        }
        self.backups()
            .into_iter()
            .map(|backup| bl_shell_spec::menus::DynamicItem {
                label: format!("{} — {}", backup.name, backup.time),
                id: ids::SET_OPEN_BACKUP,
                args: json!({ "path": backup.path }).to_string(),
            })
            .collect()
    }

    /// Whether a folder is a project folder: one with a set file in it
    /// (WP-6.63: the browser's Manage Project).
    pub fn is_project_folder(&self, path: &Path) -> bool {
        ProjectFolder::new(path)
            .sets()
            .is_ok_and(|sets| !sets.is_empty())
    }

    /// Whether the File Manager shows (R10).
    pub fn file_manager_open(&self) -> bool {
        self.file_manager.open
    }

    /// How many of the set's files are missing or changed (R12): the Status
    /// Bar says so until none is.
    pub fn missing_files(&self) -> usize {
        let document = self.editor.document();
        // The Status Bar asks every frame, so the clips are read once and
        // the files counted from them, not the other way round.
        let mut used = std::collections::HashSet::new();
        for track in document.tracks() {
            for clip in track.clips().chain(track.slots().map(|(_, clip)| clip)) {
                if let ClipData::Audio(audio) = &clip.data {
                    used.insert(ContentHash::from_bytes(audio.media.0));
                }
            }
            for device in track.devices() {
                for played in device.media.iter().flatten() {
                    used.insert(ContentHash::from_bytes(played.0));
                }
            }
        }
        self.media
            .entries()
            .into_iter()
            .filter(|entry| used.contains(&entry.hash))
            .filter(|entry| {
                !matches!(
                    self.media.check(entry.hash),
                    Ok(bl_media::MediaStatus::Available)
                )
            })
            .count()
    }

    /// What the File Manager lists (R10 to R17).
    pub fn file_manager_state(&self) -> FileManagerState {
        let scope = self.file_manager.scope;
        let files = self.file_manager_files();
        let missing: Vec<FileRow> = files
            .iter()
            .filter(|file| file.location == Location::Missing)
            .cloned()
            .collect();
        let external = self.external_groups(&files);
        let unused = self.unused_files();
        let mut sections = Vec::new();
        match scope {
            Scope::Set => {
                sections.push(FileSection {
                    name: "Files".to_owned(),
                    count: files.len(),
                });
            }
            Scope::Project | Scope::UserLibrary => {}
        }
        sections.push(FileSection {
            name: "Missing Files".to_owned(),
            count: missing.len(),
        });
        sections.push(FileSection {
            name: "Automatic Search".to_owned(),
            count: 0,
        });
        sections.push(FileSection {
            name: "External Files".to_owned(),
            count: external.iter().map(|group| group.count).sum(),
        });
        if scope != Scope::Set {
            sections.push(FileSection {
                name: "Unused Files".to_owned(),
                count: unused.iter().map(|group| group.count).sum(),
            });
        }
        if scope == Scope::Project {
            sections.push(FileSection {
                name: "Packing".to_owned(),
                count: 0,
            });
        }
        let search = self.file_manager_search();
        FileManagerState {
            open: self.file_manager.open,
            scope,
            project: self
                .file_manager_root()
                .map(|root| root.to_string_lossy().into_owned()),
            sections,
            files,
            missing,
            external,
            unused,
            search,
            hot_swap: self.file_manager.hot_swap.as_ref().map(|hot| hex(hot.hash)),
            packing: scope == Scope::Project,
        }
    }

    /// The Automatic Search's settings and what its last search found (R14).
    fn file_manager_search(&self) -> Search {
        Search {
            folder: self.file_manager.search.folder.clone(),
            project: self.file_manager.search.project,
            library: self.file_manager.search.library,
            candidates: self.file_manager.search.candidates.clone(),
        }
    }

    /// The folder the File Manager's files are measured against: the open
    /// set's project folder, the project Manage Project was given, or the
    /// User Library.
    fn file_manager_root(&self) -> Option<PathBuf> {
        match self.file_manager.scope {
            Scope::Set => self.set_project_folder(),
            Scope::Project => self
                .file_manager
                .project
                .clone()
                .or_else(|| self.set_project_folder()),
            Scope::UserLibrary => self.user_library(),
        }
    }

    /// The open set's project folder: the folder its file is in.
    pub(super) fn set_project_folder(&self) -> Option<PathBuf> {
        self.set_path
            .as_deref()
            .and_then(Path::parent)
            .map(|folder| folder.to_path_buf())
    }

    /// The User Library's folder, if there is one.
    fn user_library(&self) -> Option<PathBuf> {
        self.browser_view
            .user_library()
            .map(|library| library.to_path_buf())
    }

    /// The files the File Manager lists, for the view it shows.
    fn file_manager_files(&self) -> Vec<FileRow> {
        match self.file_manager.scope {
            Scope::Set => self.set_file_rows(),
            Scope::Project | Scope::UserLibrary => self.folder_file_rows(),
        }
    }

    /// The open set's files, with their locations (R11).
    fn set_file_rows(&self) -> Vec<FileRow> {
        let root = self.set_project_folder();
        let library = self.user_library();
        let mut rows = Vec::new();
        for entry in self.media.entries() {
            let used_by = users_of(self.editor.document(), entry.hash);
            if used_by.is_empty() {
                // A file no clip uses isn't one of the set's files.
                continue;
            }
            let location = match self.media.check(entry.hash) {
                Ok(bl_media::MediaStatus::Available) => {
                    location_of(&entry.path, root.as_deref(), library.as_deref())
                }
                _ => Location::Missing,
            };
            rows.push(FileRow {
                hash: hex(entry.hash),
                name: file_name(&entry.path),
                path: entry.path.to_string_lossy().into_owned(),
                size: entry.size,
                location,
                used_by,
            });
        }
        rows
    }

    /// A project folder's or the User Library's files, with their
    /// locations (R16).
    fn folder_file_rows(&self) -> Vec<FileRow> {
        self.file_manager_root()
            .map_or_else(Vec::new, |root| self.project_file_rows(&root))
    }

    /// The files the sets of the project folder `root` use, with their
    /// locations (R16).
    pub(super) fn project_file_rows(&self, root: &Path) -> Vec<FileRow> {
        let library = self.user_library();
        let mut rows: Vec<FileRow> = Vec::new();
        for set in ProjectFolder::new(root).sets().unwrap_or_default() {
            let Ok((project, _)) = bl_project::load(&set) else {
                continue;
            };
            let folder = set.parent().unwrap_or(root).to_path_buf();
            for file in project.media.iter() {
                let Some(path) = file.path.resolve(&folder) else {
                    continue;
                };
                let path = without_parent_parts(&path);
                let hash = ContentHash::from_bytes(file.hash.0);
                if rows.iter().any(|row| row.hash == hex(hash)) {
                    continue;
                }
                let used_by = users_of(&project.document, hash);
                let location = location_of(&path, Some(root), library.as_deref());
                rows.push(FileRow {
                    hash: hex(hash),
                    name: file_name(&path),
                    path: path.to_string_lossy().into_owned(),
                    size: file.size,
                    location,
                    used_by,
                });
            }
        }
        rows
    }

    /// The files used from outside the project, grouped by where they are
    /// (R15).
    fn external_groups(&self, files: &[FileRow]) -> Vec<ExternalFiles> {
        let mut groups: Vec<(ids::ExternalGroup, Vec<FileRow>)> = vec![
            (ids::ExternalGroup::OtherProject, Vec::new()),
            (ids::ExternalGroup::UserLibrary, Vec::new()),
            (ids::ExternalGroup::Elsewhere, Vec::new()),
        ];
        for file in files {
            let group = match file.location {
                Location::Missing | Location::Project => continue,
                Location::UserLibrary => ids::ExternalGroup::UserLibrary,
                Location::External => crate::files::external_group(
                    Path::new(&file.path),
                    self.user_library().as_deref(),
                ),
            };
            let (_, rows) = groups
                .iter_mut()
                .find(|(kind, _)| *kind == group)
                .expect("a group");
            rows.push(file.clone());
        }
        groups
            .into_iter()
            .map(|(group, files)| ExternalFiles {
                name: group_name(group),
                group,
                count: files.len(),
                size: files.iter().map(|file| file.size).sum(),
                files,
            })
            .collect()
    }

    /// The files in the project folder that no set there uses, grouped by
    /// kind (R17).
    fn unused_files(&self) -> Vec<UnusedFiles> {
        if self.file_manager.scope == Scope::Set {
            return Vec::new();
        }
        let Some(root) = self.file_manager_root() else {
            return Vec::new();
        };
        let used = self.used_paths(&root);
        let mut groups: BTreeMap<UnusedKind, Vec<UnusedFile>> = BTreeMap::new();
        for path in audio_files(&root) {
            if used.contains(&path) {
                continue;
            }
            let kind = unused_kind(&root, &path);
            let Ok(size) = std::fs::metadata(&path).map(|meta| meta.len()) else {
                continue;
            };
            groups.entry(kind).or_default().push(UnusedFile {
                name: file_name(&path),
                path: path.to_string_lossy().into_owned(),
                size,
            });
        }
        groups
            .into_iter()
            .map(|(group, files)| UnusedFiles {
                name: unused_group_name(group),
                group,
                count: files.len(),
                size: files.iter().map(|file| file.size).sum(),
                files,
            })
            .collect()
    }

    /// The files the sets in the folder use, by their paths on disk.
    fn used_paths(&self, root: &Path) -> Vec<PathBuf> {
        let mut used = Vec::new();
        for set in ProjectFolder::new(root).sets().unwrap_or_default() {
            let Ok((project, _)) = bl_project::load(&set) else {
                continue;
            };
            let folder = set.parent().unwrap_or(root).to_path_buf();
            for file in project.media.iter() {
                if let Some(path) = file.path.resolve(&folder) {
                    used.push(path);
                }
            }
        }
        used
    }

    /// Runs the File Manager's commands (WP-6.63).
    pub(super) fn run_file_manager(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        match id {
            ids::VIEW_TOGGLE_FILE_MANAGER => self.run_toggle_file_manager(id, args),
            ids::STATE => {
                Self::args::<NoArgs>(id, args)?;
                Ok(to_value(&self.file_manager_state()))
            }
            ids::REPLACE_FILE => self.run_replace_file(id, args),
            ids::HOT_SWAP => self.run_hot_swap(id, args),
            ids::EDIT_FILE => self.run_edit_file(id, args),
            ids::SET_SEARCH_FOLDER => self.run_set_search_folder(id, args),
            ids::DELETE_UNUSED => self.run_delete_unused(id, args),
            ids::SET_BACKUPS => self.run_set_backups(id, args),
            ids::SET_OPEN_BACKUP => self.run_open_backup(id, args),
            ids::SET_SAVE_COPY => self.run_save_copy(id, args),
            other => Err(ShellError::UnknownCommand(other.to_owned())),
        }
    }

    /// Collects the open set's external files into its project folder and
    /// saves it. Returns how many files were copied.
    pub(super) fn collect_open_set(
        &mut self,
        path: &Path,
        groups: &[ids::ExternalGroup],
        library: Option<&Path>,
    ) -> Result<usize, String> {
        crate::plugins::capture_states(
            &self.plugins,
            &self.au,
            &self.vst3,
            &self.lv2,
            &self.kit,
            &mut self.editor,
        );
        self.store.set_windows(self.devices.windows().clone());
        self.store.set_rack_views(self.devices.rack_views().clone());
        self.store
            .set_multisampler_views(self.devices.multisampler_views().clone());
        self.store.set_folds(self.clip_view.folds().clone());
        self.store
            .set_scale_folds(self.clip_view.scale_folds().clone());
        self.store.set_mpe_view(self.clip_view.mpe().clone());
        self.store
            .set_lane_views(self.arrangement.lane_views().clone());
        self.store.set_column_widths(self.mixer.widths().clone());
        self.store
            .set_midi_tools(self.clip_view.midi_tools().saved());
        let files = self.store.collect_groups(groups, library)?;
        let copy = self.without_selection_arms();
        let warnings = self
            .store
            .save(path, copy.as_ref().unwrap_or(&self.editor))
            .map_err(|error| error.to_string())?;
        self.saved_as(path.to_path_buf(), &warnings);
        Ok(files)
    }

    /// Collects the files every set in a project folder uses from outside
    /// it, and saves each set, which keeps the version it replaces in its
    /// project's `Backup` folder (R16). Returns how many files were copied.
    pub(super) fn collect_project(
        &mut self,
        folder: &Path,
        groups: &[ids::ExternalGroup],
        library: Option<&Path>,
    ) -> Result<usize, String> {
        // The open set is collected through its own store, so the next save
        // keeps the new paths (R16); its file on disk isn't rewritten behind
        // the editor's back.
        let open = self
            .set_path
            .clone()
            .filter(|open| open.parent().is_some_and(|open| same_folder(open, folder)));
        if open.is_some() && self.dirty {
            return Err("the open set has unsaved changes: save it first".to_owned());
        }
        let imported = ProjectFolder::new(folder).imported();
        let mut copied = 0usize;
        for set in ProjectFolder::new(folder).sets().unwrap_or_default() {
            if open.as_deref().is_some_and(|open| same_folder(open, &set)) {
                continue;
            }
            let (mut project, _) = bl_project::load(&set).map_err(|error| error.to_string())?;
            for file in project.media.iter().cloned().collect::<Vec<_>>() {
                if file.path.is_inside() {
                    continue;
                }
                let Some(source) = file.path.resolve(folder) else {
                    continue;
                };
                if !source.is_file()
                    || !groups.contains(&crate::files::external_group(&source, library))
                {
                    continue;
                }
                std::fs::create_dir_all(&imported)
                    .map_err(|error| format!("couldn't create {}: {error}", imported.display()))?;
                let name = source.file_name().map_or_else(
                    || file.hash.to_string(),
                    |name| name.to_string_lossy().into_owned(),
                );
                let destination = crate::files::free_import(&imported, &name)?;
                if destination != source {
                    std::fs::copy(&source, &destination).map_err(|error| {
                        format!(
                            "couldn't copy {} into {}: {error}",
                            source.display(),
                            imported.display()
                        )
                    })?;
                }
                let Some(relative) = bl_project::MediaPath::between(folder, &destination) else {
                    continue;
                };
                project.media.insert(bl_project::MediaRef {
                    path: relative,
                    ..file
                });
                copied += 1;
            }
            // A save keeps the version it replaces as a backup (R16).
            let (mut file, _) =
                bl_project::SetFile::open(&set).map_err(|error| error.to_string())?;
            file.save(&project).map_err(|error| error.to_string())?;
        }
        if let Some(open) = open {
            copied += self.collect_open_set(&open, groups, library)?;
        }
        Ok(copied)
    }

    /// Runs `view.toggle_file_manager` (R10): shows or hides the File
    /// Manager, and chooses which files it lists.
    fn run_toggle_file_manager(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        let ToggleArgs {
            view,
            open,
            project,
        } = Self::args(id, args)?;
        if let Some(view) = view {
            self.file_manager.scope = view;
        }
        if let Some(project) = project {
            self.file_manager.project = Some(PathBuf::from(project));
        }
        // The menu items again hide it.
        self.file_manager.open = open.unwrap_or(!self.file_manager.open);
        if self.file_manager.open {
            // Spec: file-management R10. The File Manager and the list of
            // the set's plug-ins take the same place: showing either hides
            // the other.
            self.plugin_list.hide();
        }
        Ok(to_value(&FileManagerView {
            open: self.file_manager.open,
            scope: self.file_manager.scope,
            project: self
                .file_manager_root()
                .map(|root| root.to_string_lossy().into_owned()),
        }))
    }

    /// Runs `file_manager.replace_file` (R11, R13): every clip and device
    /// that used the file uses the new one, in one undo step.
    fn run_replace_file(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        let ReplaceFileArgs { hash, path } = Self::args(id, args)?;
        let hash = parse_hash(&hash).ok_or_else(|| ShellError::InvalidArgs {
            command: id.to_owned(),
            message: format!("{hash} isn't a media hash"),
        })?;
        let path = PathBuf::from(path);
        let media = self
            .media
            .import(&path)
            .map_err(|error| ShellError::Failed {
                command: id.to_owned(),
                message: format!("couldn't read {}: {error}", path.display()),
            })?;
        let size = std::fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
        let replaced = self.replace_media_everywhere(id, hash, media)?;
        // The set's media table names the new file, and forgets the old one.
        self.store
            .add_media(bl_model::ContentHash(*media.as_bytes()), &path, size);
        self.store
            .forget_media(bl_model::ContentHash(*hash.as_bytes()));
        self.file_manager.hot_swap = None;
        Ok(to_value(&ReplacedFile {
            hash: hex(hash),
            path: path.to_string_lossy().into_owned(),
            replaced,
        }))
    }

    /// Runs `file_manager.hot_swap` (R11, R14): the browser is armed at
    /// the file's folder, and the file the browser has chosen replaces it.
    fn run_hot_swap(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        let ids::HotSwapArgs { hash } = Self::args(id, args)?;
        let hash = parse_hash(&hash).ok_or_else(|| ShellError::InvalidArgs {
            command: id.to_owned(),
            message: format!("{hash} isn't a media hash"),
        })?;
        // The candidates the last search found for the file, when it had
        // several.
        let candidates = self
            .file_manager
            .search
            .candidates
            .iter()
            .find(|candidate| candidate.hash == hex(hash))
            .map(|candidate| candidate.paths.clone())
            .unwrap_or_default();
        let folder = candidates
            .first()
            .and_then(|path| PathBuf::from(path).parent().map(Path::to_path_buf))
            .or_else(|| {
                self.media
                    .entry(hash)
                    .map(|entry| entry.path.parent().map(Path::to_path_buf))
                    .unwrap_or(None)
            })
            .ok_or_else(|| ShellError::Unavailable {
                command: id.to_owned(),
                reason: "the file isn't there to swap".to_owned(),
            })?;
        self.file_manager.hot_swap = Some(HotSwap { hash });
        // The browser opens at the file's folder, where the file to use is
        // chosen. `open` is given, so a folder that is open already stays
        // open.
        self.run(
            crate::browser_commands::OPEN_FOLDER,
            json!({ "path": folder.to_string_lossy(), "open": true }),
        )?;
        Ok(to_value(&ReplacedFile {
            hash: hex(hash),
            path: folder.to_string_lossy().into_owned(),
            replaced: 0,
        }))
    }

    /// Runs `browser.load_selected` while a file is armed for hot-swapping
    /// (R11, R14): the file the browser has selected replaces the armed
    /// one.
    pub(super) fn hot_swap_selected(&mut self, id: &str) -> Result<Value, ShellError> {
        let Some(hot) = self.file_manager.hot_swap.clone() else {
            return Err(ShellError::Unavailable {
                command: id.to_owned(),
                reason: "no file is armed for hot-swapping".to_owned(),
            });
        };
        let selected = self.browser_view.selected_audio();
        let Some(path) = selected.first().cloned() else {
            return Err(ShellError::Unavailable {
                command: id.to_owned(),
                reason: "select a file in the browser first".to_owned(),
            });
        };
        self.run_replace_file(
            id,
            json!({
                "hash": hex(hot.hash),
                "path": path.to_string_lossy(),
            }),
        )
    }

    /// Runs `file_manager.edit_file` (R11): the file is opened in the
    /// external sample editor, and the edited file comes back for every
    /// clip that uses it.
    fn run_edit_file(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        let ids::EditFileArgs { hash } = Self::args(id, args)?;
        let hash = parse_hash(&hash).ok_or_else(|| ShellError::InvalidArgs {
            command: id.to_owned(),
            message: format!("{hash} isn't a media hash"),
        })?;
        let Some(path) = self.media.entry(hash).map(|entry| entry.path) else {
            return Err(ShellError::Failed {
                command: id.to_owned(),
                message: "the media store doesn't have the file".to_owned(),
            });
        };
        let Some(editor) = self.sample_editor() else {
            self.run(
                bl_shell_spec::commands::APP_SETTINGS,
                json!({ "page": "file-folder" }),
            )?;
            return Ok(to_value(&ids::EditedFile {
                hash: hex(hash),
                file: path.to_string_lossy().into_owned(),
                editor: "File & Folder".to_owned(),
                opened: false,
            }));
        };
        let mut parts = editor.split_whitespace();
        let Some(program) = parts.next() else {
            return Err(ShellError::InvalidArgs {
                command: id.to_owned(),
                message: "the sample editor's command is empty".to_owned(),
            });
        };
        let started = std::process::Command::new(program)
            .args(parts)
            .arg(&path)
            .spawn()
            .is_ok();
        if started {
            // The file is watched: when its content changes, every clip
            // that played it plays the new one (R11, sample-editor R16).
            self.editing.push(crate::shell::audio_panel::Editing {
                path: path.clone(),
                hash,
            });
        }
        Ok(to_value(&ids::EditedFile {
            hash: hex(hash),
            file: path.to_string_lossy().into_owned(),
            editor,
            opened: started,
        }))
    }

    /// Runs `file_manager.set_search_folder` (R14): the Automatic Search's
    /// Search Folder names a folder, and the search covers it.
    fn run_set_search_folder(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        let ids::SearchFolderArgs { folder } = Self::args(id, args)?;
        if let Some(folder) = folder {
            self.file_manager.search.folder = Some(folder);
        }
        Ok(to_value(&self.file_manager_search()))
    }

    /// Runs `file_manager.delete_unused` (R17): the files go to the
    /// system's trash.
    fn run_delete_unused(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        let DeleteUnusedArgs { paths } = Self::args(id, args)?;
        // Spec: file-management R17. Only the files the Unused Files section
        // lists go, and none goes unless every path is one of them.
        let listed: std::collections::BTreeSet<PathBuf> = self
            .unused_files()
            .into_iter()
            .flat_map(|group| group.files)
            .filter_map(|file| std::fs::canonicalize(file.path).ok())
            .collect();
        for path in &paths {
            if !std::fs::canonicalize(path).is_ok_and(|path| listed.contains(&path)) {
                return Err(ShellError::InvalidArgs {
                    command: id.to_owned(),
                    message: format!("{path} isn't in the Unused Files section"),
                });
            }
        }
        let mut deleted = Vec::new();
        for path in paths {
            let path = PathBuf::from(path);
            crate::trash::to_trash(&path).map_err(|message| ShellError::Failed {
                command: id.to_owned(),
                message,
            })?;
            deleted.push(path.to_string_lossy().into_owned());
        }
        Ok(to_value(&DeletedFiles { files: deleted }))
    }

    /// Runs `set.backups` (R18): the open set's backups, newest first.
    pub(super) fn run_set_backups(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        Self::args::<NoArgs>(id, args)?;
        Ok(to_value(&Backups {
            backups: self.backups(),
        }))
    }

    /// The open set's backups, newest first, each by the time it was made
    /// in local time (R18).
    pub fn backups(&self) -> Vec<Backup> {
        let Some(path) = self.set_path.as_deref() else {
            return Vec::new();
        };
        let Some(folder) = path.parent() else {
            return Vec::new();
        };
        let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
            return Vec::new();
        };
        let folder = ProjectFolder::new(folder);
        bl_project::backups(&folder.backup(), stem)
            .into_iter()
            .map(|backup| Backup {
                path: backup.path.to_string_lossy().into_owned(),
                name: backup_name(&backup),
                time: local_time_of(&backup),
            })
            .collect()
    }

    /// Runs `set.open_backup` (R18): the backup opens as the set itself.
    fn run_open_backup(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        let OpenBackupArgs { path } = Self::args(id, args)?;
        if self.dirty {
            return Err(ShellError::Unavailable {
                command: id.to_owned(),
                reason: "the set has unsaved changes; save them with `set.save` or \
                 `set.save_as`, or throw them away with `set.discard_changes`, first"
                    .to_owned(),
            });
        }
        let editor = self
            .store
            .read(Path::new(&path))
            .map_err(|message| ShellError::Failed {
                command: id.to_owned(),
                message,
            })?;
        // The set keeps its own file and name; its song is the backup's,
        // and it is unsaved, so `set.save` makes the backup its current
        // version.
        self.editor = editor;
        self.editor_replaced();
        self.new_set_transport();
        self.dirty = true;
        self.message = None;
        self.arrangement.reset();
        self.arrangement.restore_lanes(self.store.lane_views());
        self.clip_view.forget_selection();
        self.mixer.reset();
        self.mixer.restore_widths(self.store.column_widths());
        self.session.reset();
        for (hash, path, size) in self.store.media_files() {
            self.media
                .add_reference(bl_media::ContentHash::from_bytes(hash.0), path, size);
        }
        self.document_changed();
        Ok(self.set_result())
    }

    /// Runs `set.save_copy` (R19): a copy of the open set, which the open
    /// set doesn't notice.
    fn run_save_copy(&mut self, id: &str, args: Value) -> Result<Value, ShellError> {
        let SaveCopyArgs { path } = Self::args(id, args)?;
        let chosen = PathBuf::from(path);
        // The copy is placed as Save As places a set.
        let placement = crate::files::place_set(&chosen).map_err(|message| ShellError::Failed {
            command: id.to_owned(),
            message,
        })?;
        // Spec: push-2-notes R3. A selection arm isn't saved.
        let copy = self.without_selection_arms();
        let warnings = self
            .store
            .save_copy(&placement.path, copy.as_ref().unwrap_or(&self.editor))
            .map_err(|message| ShellError::Failed {
                command: id.to_owned(),
                message,
            })?;
        self.message = (!warnings.is_empty()).then(|| Message {
            text: format!("The copy was saved, but: {}", warnings.join("; ")),
            error: false,
            action: None,
        });
        Ok(to_value(&SavedCopy {
            path: placement.path.to_string_lossy().into_owned(),
        }))
    }

    /// Gives every clip and device that played the file `hash` the file
    /// `media` instead, in one undo step (R11, R13).
    fn replace_media_everywhere(
        &mut self,
        id: &str,
        hash: ContentHash,
        media: ContentHash,
    ) -> Result<usize, ShellError> {
        let document = self.editor.document();
        let mut clips = Vec::new();
        let mut devices = Vec::new();
        for track in document.tracks() {
            for clip in track.clips().chain(track.slots().map(|(_, clip)| clip)) {
                if let ClipData::Audio(audio) = &clip.data
                    && ContentHash::from_bytes(audio.media.0) == hash
                {
                    clips.push((clip.id, self.replaced_audio(id, clip.id, media)?));
                }
            }
            for device in track.devices() {
                for (slot, played) in device.media.iter().enumerate() {
                    if played.is_some_and(|played| ContentHash::from_bytes(played.0) == hash)
                        && let Ok(slot) = u8::try_from(slot)
                    {
                        devices.push((device.id, slot));
                    }
                }
            }
        }
        let count = clips.len() + devices.len();
        if count == 0 {
            return Ok(0);
        }
        let devices = devices.clone();
        crate::arrangement::editing::transaction(&mut self.editor, "Replace File", |editor| {
            for (clip, data) in &clips {
                editor.apply(Command::SetClipData {
                    clip: *clip,
                    data: ClipData::Audio(data.clone()),
                })?;
            }
            for (device, slot) in &devices {
                editor.apply(Command::SetDeviceSample {
                    device: *device,
                    slot: *slot,
                    media: Some(bl_model::ContentHash(*media.as_bytes())),
                })?;
            }
            Ok(())
        })
        .map_err(|error| match error {
            crate::arrangement::editing::EditFailure::Refused(reason)
            | crate::arrangement::editing::EditFailure::Unavailable(reason) => {
                ShellError::Unavailable {
                    command: id.to_owned(),
                    reason,
                }
            }
            crate::arrangement::editing::EditFailure::InvalidArgs(message) => {
                ShellError::InvalidArgs {
                    command: id.to_owned(),
                    message,
                }
            }
        })?;
        self.dirty = true;
        self.document_changed();
        Ok(count)
    }
}

/// The clips and devices that use the media `hash`, by name.
fn users_of(document: &Document, hash: ContentHash) -> Vec<String> {
    let mut users = Vec::new();
    for track in document.tracks() {
        for clip in track.clips().chain(track.slots().map(|(_, clip)| clip)) {
            if let ClipData::Audio(audio) = &clip.data
                && ContentHash::from_bytes(audio.media.0) == hash
            {
                users.push(if clip.name.is_empty() {
                    track.name().to_owned()
                } else {
                    format!("{} / {}", track.name(), clip.name)
                });
            }
        }
        for device in track.devices() {
            if device
                .media
                .iter()
                .flatten()
                .any(|played| ContentHash::from_bytes(played.0) == hash)
            {
                users.push(device.title().to_owned());
            }
        }
    }
    users
}

/// `path` with its `..` parts folded in, so `Project/../Other/a.wav`, how a
/// set's media table names a file beside its project, is `Other/a.wav` and
/// not taken for a file inside `Project`.
fn without_parent_parts(path: &Path) -> PathBuf {
    let mut clean = PathBuf::new();
    for part in path.components() {
        if part == std::path::Component::ParentDir && clean.file_name().is_some() {
            clean.pop();
        } else {
            clean.push(part);
        }
    }
    clean
}

/// Where a file is, relative to the folder the File Manager works on
/// (R11).
fn location_of(path: &Path, root: Option<&Path>, library: Option<&Path>) -> Location {
    if !path.is_file() {
        return Location::Missing;
    }
    if library.is_some_and(|library| path.starts_with(library)) {
        return Location::UserLibrary;
    }
    if root.is_some_and(|root| path.starts_with(root)) {
        return Location::Project;
    }
    Location::External
}

/// Every audio file in a folder, with its subfolders.
fn audio_files(folder: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![folder.to_path_buf()];
    while let Some(folder) = stack.pop() {
        let Ok(listing) = std::fs::read_dir(&folder) else {
            continue;
        };
        for entry in listing.filter_map(Result::ok) {
            let path = entry.path();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                stack.push(path);
                continue;
            }
            if kind.is_file() && crate::browser::arrange::is_audio(&path.to_string_lossy()) {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

/// What kind of unused file a file is (R17): the groups the Unused Files
/// section lists, after the project's own folders.
fn unused_kind(root: &Path, path: &Path) -> UnusedKind {
    let Ok(rest) = path.strip_prefix(root) else {
        return UnusedKind::Other;
    };
    let parts: Vec<_> = rest.components().collect();
    match parts.as_slice() {
        [samples, recorded, ..]
            if samples.as_os_str() == "Samples" && recorded.as_os_str() == "Recorded" =>
        {
            UnusedKind::Recorded
        }
        [samples, imported, ..]
            if samples.as_os_str() == "Samples" && imported.as_os_str() == "Imported" =>
        {
            UnusedKind::Imported
        }
        [samples, processed, ..]
            if samples.as_os_str() == "Samples" && processed.as_os_str() == "Processed" =>
        {
            UnusedKind::Processed
        }
        _ => UnusedKind::Other,
    }
}

/// The name an external group shows.
fn group_name(group: ids::ExternalGroup) -> String {
    match group {
        ids::ExternalGroup::OtherProject => "Other Projects".to_owned(),
        ids::ExternalGroup::UserLibrary => "User Library".to_owned(),
        ids::ExternalGroup::Elsewhere => "Elsewhere".to_owned(),
    }
}

/// The name an unused group shows.
fn unused_group_name(group: UnusedKind) -> String {
    match group {
        UnusedKind::Recorded => "Recorded".to_owned(),
        UnusedKind::Imported => "Imported".to_owned(),
        UnusedKind::Processed => "Processed".to_owned(),
        UnusedKind::Other => "Other".to_owned(),
    }
}

/// A file's name, or its path when it has none.
fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.to_string_lossy().into_owned(),
        |name| name.to_string_lossy().into_owned(),
    )
}

/// A media hash as the File Manager's rows name it.
fn hex(hash: ContentHash) -> String {
    hash.as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// A media hash from the text a row names it by.
fn parse_hash(text: &str) -> Option<ContentHash> {
    let text = text.trim();
    if text.len() != 64 {
        return None;
    }
    let mut bytes = [0u8; 32];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(text.get(index * 2..index * 2 + 2)?, 16).ok()?;
    }
    Some(ContentHash::from_bytes(bytes))
}

/// A backup's name, with its time in brackets.
fn backup_name(backup: &bl_project::Backup) -> String {
    let name = backup.path.file_name().map_or_else(
        || backup.path.to_string_lossy().into_owned(),
        |name| name.to_string_lossy().into_owned(),
    );
    name.strip_suffix(".blset").unwrap_or(&name).to_owned()
}

/// When a backup was made, in local time (R18).
fn local_time_of(backup: &bl_project::Backup) -> String {
    backup
        .time
        .map_or_else(|| backup.stamp.clone(), bl_project::local_stamp)
}

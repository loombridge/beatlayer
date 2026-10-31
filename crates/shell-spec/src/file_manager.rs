//! The File Manager's commands (WP-6.63): showing and hiding it, what it
//! lists, replacing a file, and moving unused files to the trash; and the
//! set commands around it: the backups, and saving a copy. The behaviour is
//! in the file management spec, R10 to R21.
//!
//! They need the disk and the media store, so they belong to the shell
//! rather than to the document. [`register`] adds their specs, which are
//! pure data, so `cargo xtask` can check them and list them in the command
//! reference; `bl-shell` runs them by ID.

use bl_commands::Context::Global;
use bl_commands::{CommandSpec, NoArgs, Permission, RegisterError, Registry};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::commands::SetResult;

/// `view.toggle_file_manager`: show or hide the File Manager.
pub const VIEW_TOGGLE_FILE_MANAGER: &str = "view.toggle_file_manager";
/// `file_manager.state`: what the File Manager lists.
pub const STATE: &str = "file_manager.state";
/// `file_manager.replace_file`: replace or relink a file.
pub const REPLACE_FILE: &str = "file_manager.replace_file";
/// `file_manager.hot_swap`: arm the browser to replace a file.
pub const HOT_SWAP: &str = "file_manager.hot_swap";
/// `file_manager.edit_file`: open a file in the external sample editor.
pub const EDIT_FILE: &str = "file_manager.edit_file";
/// `file_manager.set_search_folder`: the folder the Automatic Search looks in.
pub const SET_SEARCH_FOLDER: &str = "file_manager.set_search_folder";
/// `file_manager.delete_unused`: move unused files to the system's trash.
pub const DELETE_UNUSED: &str = "file_manager.delete_unused";
/// `set.backups`: the open set's backups.
pub const SET_BACKUPS: &str = "set.backups";
/// `set.open_backup`: open one of the open set's backups.
pub const SET_OPEN_BACKUP: &str = "set.open_backup";
/// `set.save_copy`: save a copy of the open set.
pub const SET_SAVE_COPY: &str = "set.save_copy";

/// Every command [`register`] adds, so a host knows which IDs to send to
/// `bl-shell`'s `Shell::run`.
pub const IDS: &[&str] = &[
    VIEW_TOGGLE_FILE_MANAGER,
    STATE,
    REPLACE_FILE,
    HOT_SWAP,
    EDIT_FILE,
    SET_SEARCH_FOLDER,
    DELETE_UNUSED,
    SET_BACKUPS,
    SET_OPEN_BACKUP,
    SET_SAVE_COPY,
];

/// Whether `id` is one of the File Manager's commands.
#[must_use]
pub fn handles(id: &str) -> bool {
    IDS.contains(&id)
}

/// Which files the File Manager shows (R10).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Scope {
    /// The open set's files.
    #[default]
    Set,
    /// A project folder's files.
    Project,
    /// The User Library's files.
    UserLibrary,
}

/// Where a file is (R11).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Location {
    /// The file isn't where it was, or holds other content now.
    Missing,
    /// The file is in the User Library.
    UserLibrary,
    /// The file is in the project.
    Project,
    /// The file is somewhere else.
    External,
}

/// Where a file outside the project lives (R15): the groups the External
/// Files section lists.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum ExternalGroup {
    /// In another project folder.
    OtherProject,
    /// In the User Library.
    UserLibrary,
    /// Somewhere else.
    Elsewhere,
}

/// What kind of file an unused file is (R17): the groups the Unused Files
/// section lists.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum UnusedKind {
    /// Recorded into the project.
    Recorded,
    /// Copied into the project.
    Imported,
    /// Written by an edit.
    Processed,
    /// Anything else.
    Other,
}

/// A file the File Manager lists (R11).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FileRow {
    /// The file's content hash, which names it.
    pub hash: String,
    /// Its name.
    pub name: String,
    /// Where it is, or where it was.
    pub path: String,
    /// Its length in bytes.
    pub size: u64,
    /// Where it is (R11).
    pub location: Location,
    /// The clips and devices that use it (R11).
    pub used_by: Vec<String>,
}

/// A group of files outside the project (R15).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExternalFiles {
    /// The group's name.
    pub name: String,
    /// Where the group's files are.
    pub group: ExternalGroup,
    /// How many files the group has.
    pub count: usize,
    /// How big they are together, in bytes.
    pub size: u64,
    /// The files themselves.
    pub files: Vec<FileRow>,
}

/// A file in the project that nothing uses (R17).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UnusedFile {
    /// Its name.
    pub name: String,
    /// Where it is.
    pub path: String,
    /// Its length in bytes.
    pub size: u64,
}

/// A group of unused files (R17).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UnusedFiles {
    /// The group's name.
    pub name: String,
    /// What kind of files the group holds.
    pub group: UnusedKind,
    /// How many files the group has.
    pub count: usize,
    /// How big they are together, in bytes.
    pub size: u64,
    /// The files themselves.
    pub files: Vec<UnusedFile>,
}

/// A file a missing file could be relinked to (R14).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Candidate {
    /// The missing file's content hash.
    pub hash: String,
    /// The candidate's name.
    pub name: String,
    /// Where the candidates are.
    pub paths: Vec<String>,
}

/// The Automatic Search's switches and folder (R14).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Search {
    /// The folder Search Folder names.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    /// Whether the set's project folder is searched.
    pub project: bool,
    /// Whether the User Library is searched.
    pub library: bool,
    /// The candidates the last search found, one list for each missing file.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub candidates: Vec<Candidate>,
}

/// A section of the File Manager (R21).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FileSection {
    /// The section's name.
    pub name: String,
    /// How many rows it holds.
    pub count: usize,
}

/// What the File Manager shows (R10 to R17).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FileManagerState {
    /// Whether the File Manager shows.
    pub open: bool,
    /// Which files it shows.
    pub scope: Scope,
    /// The project folder it works on, when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// The sections on screen, in order (R21).
    pub sections: Vec<FileSection>,
    /// The files the set uses (R11).
    pub files: Vec<FileRow>,
    /// The files that are missing (R13).
    pub missing: Vec<FileRow>,
    /// The files used from outside the project (R15).
    pub external: Vec<ExternalFiles>,
    /// The files in the project that nothing uses (R17).
    pub unused: Vec<UnusedFiles>,
    /// The Automatic Search (R14).
    pub search: Search,
    /// The file armed for hot-swapping, by its content hash, when a row's
    /// Hot-Swap button armed it (R11).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hot_swap: Option<String>,
    /// Whether the Packing section shows (R16).
    pub packing: bool,
}

/// Arguments of `view.toggle_file_manager`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ToggleArgs {
    /// Which files the File Manager shows: "set", "project" or "user-library".
    /// Left out, it keeps the view it shows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view: Option<Scope>,
    /// Whether it shows (`true`) or hides (`false`). Left out, it shows
    /// when it is hidden and hides when it shows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open: Option<bool>,
    /// The project folder Manage Project shows, when it isn't the open
    /// set's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
}

/// Arguments of `file_manager.replace_file`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReplaceFileArgs {
    /// The file to replace, by its content hash, as the row shows it.
    pub hash: String,
    /// The file to use instead.
    pub path: String,
}

/// Arguments of `file_manager.hot_swap`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HotSwapArgs {
    /// The file to replace, by its content hash, as the row shows it.
    pub hash: String,
}

/// Arguments of `file_manager.edit_file`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EditFileArgs {
    /// The file to edit, by its content hash, as the row shows it.
    pub hash: String,
}

/// Arguments of `file_manager.set_search_folder`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SearchFolderArgs {
    /// The folder the Automatic Search's Search Folder names. Left out, the
    /// app asks the person for one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
}

/// What `file_manager.edit_file` returns.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EditedFile {
    /// The file that was opened.
    pub hash: String,
    /// Its path.
    pub file: String,
    /// The editor it was opened in, or the Settings page that was opened
    /// instead, when the File & Folder page names no editor.
    pub editor: String,
    /// Whether the editor was found and started.
    pub opened: bool,
}

/// What `file_manager.replace_file` returns.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReplacedFile {
    /// The file that was replaced.
    pub hash: String,
    /// The file that is used now.
    pub path: String,
    /// How many clips and devices used it.
    pub replaced: usize,
}

/// Arguments of `file_manager.delete_unused`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeleteUnusedArgs {
    /// The files to move to the trash, by their paths.
    pub paths: Vec<String>,
}

/// What `file_manager.delete_unused` returns.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DeletedFiles {
    /// The files that were moved to the trash.
    pub files: Vec<String>,
}

/// One of the open set's backups (R18).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Backup {
    /// Where the backup is.
    pub path: String,
    /// Its name, with its time in brackets.
    pub name: String,
    /// When it was made, in local time.
    pub time: String,
}

/// What `set.backups` returns.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Backups {
    /// The backups, newest first.
    pub backups: Vec<Backup>,
}

/// Arguments of `set.open_backup`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OpenBackupArgs {
    /// The backup to open, by its path.
    pub path: String,
}

/// Arguments of `set.save_copy`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SaveCopyArgs {
    /// Where to save the copy, as a save dialog gives it: a folder and the
    /// set's name, such as `/Music/Tango.blset`.
    pub path: String,
}

/// What `set.save_copy` returns.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SavedCopy {
    /// Where the copy was written.
    pub path: String,
}

/// Adds the File Manager's commands to a registry.
///
/// # Errors
///
/// [`RegisterError`] if one of their IDs is already registered.
pub fn register(registry: &mut Registry) -> Result<(), RegisterError> {
    registry.register(
        CommandSpec::new(VIEW_TOGGLE_FILE_MANAGER, "File Manager")
            .description(
                "Shows the File Manager in the detail area's place, or hides it, as File ▸ Manage \
                 Files and View ▸ File Manager do. `view` chooses which files it lists: \"set\" \
                 for the open set's, \"project\" for a project folder's, \"user-library\" for the \
                 User Library's. `project` names the project folder Manage Project works on, \
                 when it isn't the open set's. The File Manager's state is view state: it isn't \
                 saved in the set and it isn't an undo step.",
            )
            .permission(Permission::Read)
            .context(Global)
            .args::<ToggleArgs>()
            .result::<FileManagerState>(),
    )?;
    registry.register(
        CommandSpec::new(STATE, "File Manager State")
            .description(
                "What the File Manager lists: the files the set uses with their locations and \
                 what uses them, the missing files, the files used from outside the project \
                 grouped by where they are, the files in the project that nothing uses, and the \
                 Automatic Search's switches with the candidates its last search found. It is \
                 `external` because it lists files outside the project.",
            )
            .permission(Permission::External)
            .context(Global)
            .args::<NoArgs>()
            .result::<FileManagerState>(),
    )?;
    registry.register(
        CommandSpec::new(REPLACE_FILE, "Replace File")
            .description(
                "Replaces the file the row with `hash` names with the file at `path`, as dropping \
                 a file from the browser on the row does: every clip and device that used the \
                 file uses the new one, in one undo step, each keeping its warp markers only if \
                 the new file is exactly as long as the old one. A missing file is relinked to \
                 the file given, whatever its content. Reads a file outside the project.",
            )
            .permission(Permission::External)
            .context(Global)
            .args::<ReplaceFileArgs>()
            .result::<ReplacedFile>(),
    )?;
    registry.register(
        CommandSpec::new(HOT_SWAP, "Hot-Swap File")
            .description(
                "Arms the browser to replace the file the row with `hash` names, as the row's \
                 Hot-Swap button does: the browser opens at the file's folder, and choosing a \
                 file there replaces the armed one, as `file_manager.replace_file` does. The \
                 browser stays armed, so another file can be chosen.",
            )
            .permission(Permission::Edit)
            .context(Global)
            .args::<HotSwapArgs>()
            .result::<ReplacedFile>(),
    )?;
    registry.register(
        CommandSpec::new(EDIT_FILE, "Edit File")
            .description(
                "Opens the file the row with `hash` names in the external sample editor, as the \
                 row's Edit button does: the editor the File & Folder page's Sample Editor row \
                 names, or that page itself when it names none. The file's content is checked \
                 when Beatlayer comes to the front again: if it has changed, every clip of the \
                 set that played the old file plays the new one, in one undo step.",
            )
            .permission(Permission::External)
            .context(Global)
            .args::<EditFileArgs>()
            .result::<EditedFile>(),
    )?;
    registry.register(
        CommandSpec::new(SET_SEARCH_FOLDER, "Search Folder")
            .description(
                "The folder the Automatic Search's Search Folder names, as the Set Folder… \
                 button does: the search covers it and its subfolders. `folder` names the \
                 folder; left out, the app asks the person for one. The folder is view \
                 state: it isn't saved with the set and it isn't an undo step.",
            )
            .permission(Permission::Read)
            .context(Global)
            .args::<SearchFolderArgs>()
            .result::<FileManagerState>(),
    )?;
    registry.register(
        CommandSpec::new(DELETE_UNUSED, "Delete Unused Files")
            .description(
                "Moves the files at `paths`, which the Unused Files section lists, to the \
                 system's trash. Nothing is erased outright, but a file another project uses \
                 breaks that project, so the files are collected first.",
            )
            .permission(Permission::Destructive)
            .context(Global)
            .args::<DeleteUnusedArgs>()
            .result::<DeletedFiles>(),
    )?;
    registry.register(
        CommandSpec::new(SET_BACKUPS, "Beatlayer Set Backups")
            .description(
                "The open set's backups, in the project folder's `Backup` folder, newest first, \
                 each with the time it was made in local time. The list is empty when the set has \
                 no backups.",
            )
            .permission(Permission::Read)
            .context(Global)
            .args::<NoArgs>()
            .result::<Backups>(),
    )?;
    registry.register(
        CommandSpec::new(SET_OPEN_BACKUP, "Open Beatlayer Set Backup")
            .description(
                "Opens the backup at `path` as the set itself, as File ▸ Open Beatlayer Set \
                 Backups does: the set keeps its own file and name, its song is the backup's, and \
                 it opens unsaved with an empty undo history, so `set.save` makes the backup the \
                 set's current version. Refuses while the set has unsaved changes, as \
                 `set.open` does.",
            )
            .permission(Permission::External)
            .context(Global)
            .args::<OpenBackupArgs>()
            .result::<SetResult>(),
    )?;
    registry.register(
        CommandSpec::new(SET_SAVE_COPY, "Save a Copy...")
            .description(
                "Saves a copy of the open set at `path`, as File ▸ Save a Copy... does: in the \
                 folder when it is a project folder, otherwise in a new \"<name> Project\" folder. \
                 The copy names the same media files. The open set keeps its file, its name, its \
                 undo history and whether it has unsaved changes.",
            )
            .permission(Permission::External)
            .context(Global)
            .args::<SaveCopyArgs>()
            .result::<SavedCopy>(),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_listed_id_is_registered_and_complete() {
        let mut registry = Registry::new();
        register(&mut registry).unwrap();
        assert_eq!(registry.len(), IDS.len());
        for id in IDS {
            assert!(registry.contains(id), "{id}");
        }
        let problems: Vec<String> = registry
            .problems()
            .iter()
            .map(ToString::to_string)
            .collect();
        assert!(problems.is_empty(), "{problems:#?}");
    }
}

//! The project folder: a set file and the folders beside it.

use std::io;
use std::path::{Path, PathBuf};

/// The file extension of a set.
pub const SET_EXTENSION: &str = "blset";

/// A project folder, as Live users know it:
///
/// ```text
/// My Song Project/
///   My Song.blset        the set: zstd-compressed JSON
///   Samples/Recorded/    audio recorded into the project
///   Samples/Imported/    files copied in by Collect All and Save
///   Backup/              the sets that the last 10 saves replaced
/// ```
///
/// A folder can hold more than one set, which share its samples. This type
/// only names paths; [`ProjectFolder::create`] is the only method that
/// touches the disk.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ProjectFolder {
    root: PathBuf,
}

impl ProjectFolder {
    /// The project folder at `root`.
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The folder for a new project called `name` inside `parent`:
    /// `parent/<name> Project`.
    #[must_use]
    pub fn named(parent: &Path, name: &str) -> Self {
        Self::new(parent.join(format!("{name} Project")))
    }

    /// The project folder that holds a set file: the file's parent folder.
    #[must_use]
    pub fn of_set(set: &Path) -> Self {
        match set.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => Self::new(parent),
            _ => Self::new("."),
        }
    }

    /// The folder itself.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The path of the set called `name` in this folder: `<name>.blset`.
    #[must_use]
    pub fn set_path(&self, name: &str) -> PathBuf {
        self.root.join(format!("{name}.{SET_EXTENSION}"))
    }

    /// `Samples/Recorded`, where recordings go.
    #[must_use]
    pub fn recorded(&self) -> PathBuf {
        self.root.join("Samples").join("Recorded")
    }

    /// `Samples/Imported`, where Collect All and Save copies files.
    #[must_use]
    pub fn imported(&self) -> PathBuf {
        self.root.join("Samples").join("Imported")
    }

    /// `Backup`, which keeps the sets that recent saves replaced.
    #[must_use]
    pub fn backup(&self) -> PathBuf {
        self.root.join("Backup")
    }

    /// Every folder in the layout, the root first.
    #[must_use]
    pub fn folders(&self) -> [PathBuf; 4] {
        [
            self.root.clone(),
            self.recorded(),
            self.imported(),
            self.backup(),
        ]
    }

    /// Creates the folder and its subfolders, keeping any that exist.
    ///
    /// # Errors
    ///
    /// If a folder couldn't be created.
    pub fn create(&self) -> io::Result<()> {
        self.folders().iter().try_for_each(std::fs::create_dir_all)
    }

    /// The sets in the folder (not in its subfolders), ordered by path.
    /// Hidden files, such as a save in progress, aren't sets.
    ///
    /// # Errors
    ///
    /// If the folder couldn't be read.
    pub fn sets(&self) -> io::Result<Vec<PathBuf>> {
        let mut sets = Vec::new();
        for entry in std::fs::read_dir(&self.root)? {
            let entry = entry?;
            let path = entry.path();
            let visible = !entry.file_name().to_string_lossy().starts_with('.');
            if visible && is_set(&path) && entry.file_type()?.is_file() {
                sets.push(path);
            }
        }
        sets.sort();
        Ok(sets)
    }
}

/// Whether a path names a set file, by its extension.
#[must_use]
pub fn is_set(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case(SET_EXTENSION))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout() {
        let folder = ProjectFolder::named(Path::new("music"), "My Song");
        let root = Path::new("music").join("My Song Project");
        assert_eq!(folder.root(), root);
        assert_eq!(folder.set_path("My Song"), root.join("My Song.blset"));
        assert_eq!(folder.recorded(), root.join("Samples").join("Recorded"));
        assert_eq!(folder.imported(), root.join("Samples").join("Imported"));
        assert_eq!(folder.backup(), root.join("Backup"));
        assert_eq!(ProjectFolder::of_set(&folder.set_path("Other")), folder);
        assert_eq!(
            ProjectFolder::of_set(Path::new("Loose.blset")).root(),
            Path::new(".")
        );
        assert!(is_set(Path::new("a/b.blset")));
        assert!(is_set(Path::new("a/b.BLSET")));
        assert!(!is_set(Path::new("a/b.blset.tmp")));
        assert!(!is_set(Path::new("a/blset")));
    }
}

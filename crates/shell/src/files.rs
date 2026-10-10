//! Set files and the factory default set.
//!
//! The shell opens and saves sets through a [`SetStore`]. The app's store is
//! [`ProjectFiles`], which reads and writes Beatlayer's set files with
//! `bl-project` (WP-0.7): atomic saves, the last 10 replaced versions kept
//! in the project folder's `Backup/`, and every load checked. Tests can
//! pass a store of their own.

use std::path::{Path, PathBuf};

use bl_model::{Editor, TrackId, TrackKind, registry};
use bl_project::{Media, Project, SetFile};
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
    /// new file, which becomes the open set's. Returns warnings about
    /// things that went wrong without stopping the save.
    ///
    /// # Errors
    ///
    /// A message saying why the set couldn't be written. Nothing at `path`
    /// changes, and the store's open set is unchanged.
    fn save(&mut self, path: &Path, editor: &Editor) -> Result<Vec<String>, String>;

    /// Forgets the open set's file, as a new set has none.
    fn close(&mut self);
}

/// Beatlayer's set files, through `bl-project`. It keeps the open set's
/// file (for its revision count) and its media table, which the shell
/// doesn't edit yet, so a save writes back what the load read.
#[derive(Debug, Default)]
pub struct ProjectFiles {
    set: Option<SetFile>,
    media: Media,
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
        Ok((editor, opened))
    }

    fn save(&mut self, path: &Path, editor: &Editor) -> Result<Vec<String>, String> {
        let project = Project::from_editor(editor, self.media.clone());
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
        Ok(report.warnings)
    }

    fn close(&mut self) {
        self.set = None;
        self.media = Media::new();
    }
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
}

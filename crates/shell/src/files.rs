//! Set files and the factory default set.
//!
//! The shell opens and saves sets through a [`SetStore`], so it doesn't
//! depend on the project file format. Project files are WP-0.7's
//! (`bl-project`); until the app wires its store in, [`NoSetFiles`]
//! answers every open and save with a message saying so.

use std::path::Path;

use bl_model::{Editor, TrackId, TrackKind, registry};
use serde_json::json;

/// Reads and writes set files.
pub trait SetStore {
    /// Reads the set at `path` into an editor with an empty undo history.
    ///
    /// # Errors
    ///
    /// A message saying why the set couldn't be read.
    fn load(&self, path: &Path) -> Result<Editor, String>;

    /// Writes the editor's document to `path`, replacing any file there.
    ///
    /// # Errors
    ///
    /// A message saying why the set couldn't be written.
    fn save(&self, path: &Path, editor: &Editor) -> Result<(), String>;
}

/// The store used until project files are wired in: every open and save
/// fails with a message that says so.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoSetFiles;

/// What [`NoSetFiles`] says.
pub const NO_SET_FILES: &str = "Opening and saving sets needs the project file format (WP-0.7), which this build doesn't \
     have yet.";

impl SetStore for NoSetFiles {
    fn load(&self, _path: &Path) -> Result<Editor, String> {
        Err(NO_SET_FILES.to_owned())
    }

    fn save(&self, _path: &Path, _editor: &Editor) -> Result<(), String> {
        Err(NO_SET_FILES.to_owned())
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

    #[test]
    fn without_project_files_open_and_save_say_why() {
        let path = Path::new("Song.blset");
        assert_eq!(NoSetFiles.load(path).err().unwrap(), NO_SET_FILES);
        assert_eq!(
            NoSetFiles.save(path, &Editor::new()).err().unwrap(),
            NO_SET_FILES
        );
    }
}

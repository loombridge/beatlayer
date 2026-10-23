//! Project files: schema versions and migrations, autosave, crash recovery,
//! and sample collection.
//!
//! A project is a folder ([`ProjectFolder`]) holding one or more sets. A set
//! file (`.blset`) is one zstd frame holding the song as JSON, with a schema
//! version, a revision count, the project's ID allocator, and the media table
//! that says where the clips' audio files are ([`format`](mod@format) has the
//! layout).
//!
//! ```no_run
//! use bl_project::{Project, ProjectFolder, SetFile};
//!
//! // Save a new song into a new project folder.
//! let folder = ProjectFolder::named("Music".as_ref(), "My Song");
//! let mut set = SetFile::new(folder.set_path("My Song"));
//! let project = Project::new();
//! set.save(&project)?;
//!
//! // Open it again, ready to edit.
//! let (mut set, project) = SetFile::open(set.path())?;
//! let editor = project.editor();
//! // ... edit ...
//! set.save(&bl_project::Project::from_editor(&editor, project.media))?;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! # Saving
//!
//! A save writes the new set to a hidden file, flushes it to disk, backs up
//! the set it replaces into `Backup/` (keeping the last 10), and renames the
//! new file into place in one step, so the set's path always holds a whole
//! set, old or new, whenever the save stops (see [`SetFile::save`]).
//!
//! # Loading
//!
//! A load checks everything: the frame's checksum, the JSON's size, the
//! schema version (migrating older files through [`migrate`]), and every
//! rule of the document model. A set that loads is one the editor can work
//! on. The loader is fuzzed (`fuzz/` at the top of the repository).
//!
//! # Crash recovery and Save Continuously
//!
//! While a set is open, [`autosave::Autosave`] journals every finished edit
//! within a second, on its own thread, in a hidden file beside the set
//! ([`journal`]). If the app stops without closing the set, the next launch
//! offers to recover it, exactly as it was. With Save Continuously on, the
//! journal's entries are save points: loading a set opens it at its newest
//! one, and they form its history since the last normal save. A normal save
//! writes the next revision and retires the journal, so the set file's
//! format is unchanged. The commands that reach these are in [`registry`].

pub mod autosave;
pub mod clip_file;
pub mod format;
pub mod groove_file;
pub mod journal;
pub mod midi_tools;
pub mod migrate;
pub mod preset;
pub mod rack_preset;
pub mod registry;

mod error;
mod folder;
mod media;
mod patch;
mod save;

pub use error::{LoadError, SaveError};
pub use folder::{ProjectFolder, SET_EXTENSION, is_set};
pub use format::{
    FORMAT, LoadOptions, Project, SAVED_BY, SetInfo, decode, decode_json, encode, encode_json,
};
pub use media::{Media, MediaPath, MediaPathError, MediaRef};
pub use migrate::{Migration, SCHEMA_VERSION};
pub use save::{
    Backup, SaveOptions, SaveReport, SetFile, backups, load, load_with, local_stamp, stamp_time,
    utc_stamp,
};

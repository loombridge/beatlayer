//! What the fuzz targets check about anything the set loader accepts.

use bl_core::id::Id;
use bl_project::{LoadOptions, Project, SetInfo, decode, encode};

/// The loader's options under fuzzing: the default migrations, and a JSON
/// limit small enough that a decompression bomb stops quickly.
pub const OPTIONS: LoadOptions = LoadOptions {
    max_json_bytes: 8 << 20,
    migrations: bl_project::migrate::MIGRATIONS,
};

/// A set the loader accepted must be one the rest of Beatlayer can work
/// with: it saves again, loads back as the same song, and gives an editor
/// that hands out an ID nothing in the song uses.
pub fn check_loaded(project: &Project, info: &SetInfo) {
    let bytes = encode(project, info.revision).expect("a loaded set encodes");
    let (again, again_info) =
        decode(&bytes, &LoadOptions::default()).expect("a re-encoded set loads");
    assert_eq!(&again, project, "a loaded set changes over a save");
    assert_eq!(again_info.revision, info.revision);
    let mut editor = project.editor();
    let id: Id = editor.new_id();
    assert!(!project.document.contains(id), "a fresh ID is in use");
    let _ = project.media.missing(&project.document);
}

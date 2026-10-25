//! Importing a DAWproject file as a new set (WP-6.36), as the behaviour
//! behind `set.import_dawproject` and `set.import_report`.

use std::path::PathBuf;

use bl_model::TrackId;
use bl_shell::dawproject;
use bl_shell::testing::rig;
use bl_shell_spec::dawproject::{
    IMPORT_DAWPROJECT, IMPORT_REPORT, ImportDawProjectArgs, ImportDawProjectResult, ImportReport,
};

/// The folder the fixtures are in.
fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/dawproject")
}

/// A folder in the system's temporary folder, removed when dropped.
struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("bl-shell-dawproject-{name}"));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("a temporary folder");
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The shell's temporary folder, where an import writes what the file holds.
fn temporary(folder: &TempDir) -> PathBuf {
    folder.0.join("Temporary")
}

/// An import opens the file as a new set, with the file's name and the
/// report (R10, R11, R12).
#[test]
fn an_import_opens_the_file_as_a_new_set() {
    let (mut shell, _control) = rig();
    let path = fixtures().join("minimal.dawproject");
    let result = shell
        .run(
            IMPORT_DAWPROJECT,
            serde_json::json!({ "path": path.to_string_lossy() }),
        )
        .expect("the file imports");
    let result: ImportDawProjectResult = serde_json::from_value(result).expect("the result");
    assert_eq!(result.name, "minimal");
    assert!(result.report.entries.is_empty(), "{:?}", result.report);
    // The set is the new one, and it is unsaved.
    assert_eq!(shell.set_name(), "minimal");
    assert!(shell.editor().document().track_count() > 1);
}

/// The import is refused while the open set has unsaved changes, and the set
/// is left as it was (R10).
#[test]
fn an_import_with_unsaved_changes_is_refused() {
    let (mut shell, _control) = rig();
    let before = shell.editor().document().clone();
    let id: TrackId = shell.new_id();
    shell
        .run(
            "track.create",
            serde_json::json!({ "id": id, "kind": "midi", "name": "Added", "index": 0 }),
        )
        .expect("a track is made");
    let path = fixtures().join("minimal.dawproject");
    let error = shell
        .run(
            IMPORT_DAWPROJECT,
            serde_json::json!({ "path": path.to_string_lossy() }),
        )
        .expect_err("the import is refused");
    assert!(error.to_string().contains("unsaved changes"), "{error}");
    // The set is as it was.
    assert_eq!(
        shell.editor().document().track_count(),
        before.track_count() + 1
    );
}

/// A file that isn't a DAWproject file leaves the open set as it was
/// (R10).
#[test]
fn a_failed_import_leaves_the_open_set_unchanged() {
    let (mut shell, _control) = rig();
    let before = shell.editor().document().clone();
    let folder = TempDir::new("failed");
    let path = folder.0.join("broken.dawproject");
    std::fs::write(&path, b"this is not a DAWproject file").expect("the file is written");
    let error = shell
        .run(
            IMPORT_DAWPROJECT,
            serde_json::json!({ "path": path.to_string_lossy() }),
        )
        .expect_err("the import is refused");
    assert!(error.to_string().contains("DAWproject"), "{error}");
    assert_eq!(shell.editor().document(), &before);
    assert_eq!(shell.set_name(), "Untitled");
}

/// The report is given again by `set.import_report` while the set the import
/// made is open (R12).
#[test]
fn the_report_is_read_back_while_the_imported_set_is_open() {
    let (mut shell, _control) = rig();
    let _folder = TempDir::new("report");
    let path = fixtures().join("clips.dawproject");
    let imported = shell
        .run(
            IMPORT_DAWPROJECT,
            serde_json::json!({ "path": path.to_string_lossy() }),
        )
        .expect("the file imports");
    let imported: ImportDawProjectResult = serde_json::from_value(imported).expect("the result");
    let report = shell
        .run(IMPORT_REPORT, serde_json::json!({}))
        .expect("the report is read");
    let report: ImportReport = serde_json::from_value(report).expect("the report");
    assert_eq!(report, imported.report);
    assert!(!report.entries.is_empty());

    // Another set opening takes the report and the file's name away (R11,
    // R12).
    shell
        .run("set.new", serde_json::json!({}))
        .expect("a new set opens");
    assert_eq!(shell.set_name(), "Untitled");
    let report = shell
        .run(IMPORT_REPORT, serde_json::json!({}))
        .expect("the report is read");
    let report: ImportReport = serde_json::from_value(report).expect("the report");
    assert!(report.entries.is_empty());
}

/// The files the file holds are written into the Temporary Folder, and the
/// first save moves them into the project's `Samples/Imported` (R39).
#[test]
fn the_files_the_file_holds_are_written_and_moved_on_the_first_save() {
    let (mut shell, _control) = rig();
    let folder = TempDir::new("media");
    let temporary = temporary(&folder);
    std::fs::create_dir_all(&temporary).expect("the temporary folder");
    shell
        .run(
            "settings.set_temporary_folder",
            serde_json::json!({ "path": temporary.to_string_lossy() }),
        )
        .expect("the temporary folder is set");
    let path = fixtures().join("media.dawproject");
    shell
        .run(
            IMPORT_DAWPROJECT,
            serde_json::json!({ "path": path.to_string_lossy() }),
        )
        .expect("the file imports");
    // The file the container held is in the Temporary Folder, under
    // `Imported/<the file's name without .dawproject>/`.
    let extracted = temporary.join("Imported/media/audio/drum.wav");
    assert!(extracted.is_file(), "{} is missing", extracted.display());

    // The first save asks where to save the set.
    let saved = folder.0.join("Saved");
    std::fs::create_dir_all(&saved).expect("a folder to save in");
    let set = saved.join("Saved.blset");
    let result = shell
        .run(
            "set.save_as",
            serde_json::json!({ "path": set.to_string_lossy() }),
        )
        .expect("the set is saved");
    let result: bl_shell_spec::commands::SetResult =
        serde_json::from_value(result).expect("the result");
    // The file has moved into the project's `Samples/Imported`.
    let project = result
        .path
        .as_ref()
        .and_then(|path| PathBuf::from(path).parent().map(PathBuf::from))
        .expect("the set's file");
    let moved = project.join("Samples/Imported/drum.wav");
    assert!(moved.is_file(), "{} is missing", moved.display());
    assert!(!extracted.exists());
}

/// The arguments of the import command are what the spec says.
#[test]
fn the_arguments_are_the_path_of_the_file() {
    let args: ImportDawProjectArgs =
        serde_json::from_value(serde_json::json!({ "path": "/a/b.dawproject" }))
            .expect("the arguments");
    assert_eq!(args.path, "/a/b.dawproject");
    assert!(serde_json::from_value::<ImportDawProjectArgs>(serde_json::json!({})).is_err());
}

/// The import's folder is under the Temporary Folder, named after the file
/// (R39).
#[test]
fn the_import_folder_is_named_after_the_file() {
    let folder = dawproject::import_folder(
        std::path::Path::new("/tmp/Temporary"),
        std::path::Path::new("/music/Song.dawproject"),
    );
    assert_eq!(folder, PathBuf::from("/tmp/Temporary/Imported/Song"));
}

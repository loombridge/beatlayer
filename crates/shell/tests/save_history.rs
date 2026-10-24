//! The save history (IMP-05, WP-6.39; the crash-recovery spec, R12 to R20):
//! the rows `set.save_history` lists, newest first, and `set.restore`
//! putting one back as one undo step. Driven through `Shell::run`, as an
//! agent drives it (D17), on real folders and bl-project's real files, and
//! a journal flushed by the test, never waited for.

use std::path::{Path, PathBuf};

use bl_media::ContentHash;
use bl_model::{ClipData, TrackKind};
use bl_project::{Project, SetFile};
use bl_shell::audio::AudioHost;
use bl_shell::files::ProjectFiles;
use bl_shell::settings::Preferences;
use bl_shell::{Shell, ShellError};
use serde_json::{Value, json};

/// A folder for one test's files, removed when the test ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "bl-save-history-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.0.join(relative)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// A shell with a recovery folder, as the app has at start-up.
fn shell(scratch: &Scratch) -> Shell {
    let mut shell = Shell::new(
        Preferences::default(),
        None,
        AudioHost::null(Default::default()),
        Box::new(ProjectFiles::new()),
    );
    shell.use_recovery(scratch.path("recovery")).unwrap();
    shell
}

/// A set saved twice, so that its file has a backup, in a project folder.
fn saved_set(scratch: &Scratch) -> PathBuf {
    let path = scratch.path("Song Project/Song.blset");
    let mut set = SetFile::new(&path);
    set.save(&Project::new()).unwrap();
    set.save(&Project::new()).unwrap();
    path
}

/// A shell with that set open and Save Continuously `continuous`.
fn open(scratch: &Scratch, continuous: bool) -> (Shell, PathBuf) {
    let set = saved_set(scratch);
    let mut shell = shell(scratch);
    shell
        .run("set.open", json!({ "path": text(&set) }))
        .unwrap();
    shell
        .run("settings.set_continuous_save", json!({ "on": continuous }))
        .unwrap();
    (shell, set)
}

/// The song, which is what a save point holds. The ID allocator stays past
/// every ID handed out, so a restore never reuses one (`Editor::replace`).
fn saved_form(shell: &Shell) -> bl_model::Document {
    shell.editor().document().clone()
}

/// Makes an edit and waits until it is on disk.
fn edit(shell: &mut Shell, name: &str) {
    shell
        .invoke("track.create", json!({ "kind": "midi", "name": name }))
        .unwrap();
    shell.flush_recovery().unwrap();
}

fn history(shell: &mut Shell) -> Value {
    shell.run("set.save_history", json!({})).unwrap()
}

fn kinds(history: &Value) -> Vec<&str> {
    history["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["kind"].as_str().unwrap())
        .collect()
}

fn id(history: &Value, row: usize) -> String {
    history["rows"][row]["id"].as_str().unwrap().to_owned()
}

fn restore(shell: &mut Shell, id: &str) -> Result<Value, ShellError> {
    shell.run("set.restore", json!({ "id": id }))
}

// Spec: crash-recovery R13, R16, R17, R20.
#[test]
fn with_save_continuously_each_edit_lists_as_a_save_point_and_the_third_restores() {
    let scratch = Scratch::new("restore");
    let (mut shell, _set) = open(&scratch, true);
    let mut songs = Vec::new();
    let mut labels = Vec::new();
    for n in 1..=4 {
        edit(&mut shell, &format!("Track {n}"));
        songs.push(saved_form(&shell));
        labels.push(shell.editor().undo_history().next().unwrap().to_owned());
    }

    let listed = history(&mut shell);
    assert_eq!(listed["continuous"], true);
    assert_eq!(
        kinds(&listed),
        [
            "save_point",
            "save_point",
            "save_point",
            "save_point",
            "saved",
            "backup"
        ],
        "the save points newest first, then the saved version and the backups: {listed}"
    );
    let rows = listed["rows"].as_array().unwrap();
    for (row, label) in rows.iter().zip(labels.iter().rev()) {
        assert_eq!(row["label"], *label, "a save point is named for its edit");
    }
    assert_eq!(rows[4]["label"], "Saved");
    assert!(rows[0]["time"].as_str().unwrap().ends_with('Z'), "{listed}");

    // The third save point gives that song exactly, as one undo step.
    let steps = shell.editor().undo_history().count();
    let restored = restore(&mut shell, &id(&listed, 1)).unwrap();
    assert_eq!(restored["undo_step"], "Restore Save Point");
    assert_eq!(saved_form(&shell), songs[2]);
    assert_eq!(shell.editor().undo_history().count(), steps + 1);
    assert_eq!(
        shell.editor().undo_history().next(),
        Some("Restore Save Point")
    );

    // The restore is the newest save point, and every older row is listed.
    shell.flush_recovery().unwrap();
    let after = history(&mut shell);
    assert_eq!(after["rows"][0]["label"], "Restore Save Point");
    let before: Vec<String> = (0..listed["rows"].as_array().unwrap().len())
        .map(|row| id(&listed, row))
        .collect();
    let now: Vec<String> = (0..after["rows"].as_array().unwrap().len())
        .map(|row| id(&after, row))
        .collect();
    assert_eq!(now[1..], before[..], "every row listed before stays listed");

    // Undo takes the restore back.
    shell.run("edit.undo", json!({})).unwrap();
    assert_eq!(saved_form(&shell), songs[3]);
}

// Spec: crash-recovery R10, R13.
#[test]
fn a_normal_save_leaves_the_saved_version_and_the_backups() {
    let scratch = Scratch::new("save");
    let (mut shell, set) = open(&scratch, true);
    edit(&mut shell, "One");
    edit(&mut shell, "Two");
    let before = history(&mut shell);
    assert_eq!(
        kinds(&before),
        ["save_point", "save_point", "saved", "backup"]
    );

    shell.run("set.save", json!({})).unwrap();
    let after = history(&mut shell);
    assert_eq!(
        kinds(&after),
        ["saved", "backup", "backup"],
        "the save points are retired and the version the save replaced is a backup: {after}"
    );
    assert_ne!(id(&after, 0), id(&before, 2), "the new save is the top row");

    // The saved version restores the set file's song.
    edit(&mut shell, "Three");
    let saved = id(&history(&mut shell), 1);
    let song = saved_form(&shell);
    restore(&mut shell, &saved).unwrap();
    assert_eq!(
        shell.editor().undo_history().next(),
        Some("Restore Saved Version")
    );
    assert_ne!(saved_form(&shell), song);
    assert!(set.is_file());
}

// Spec: crash-recovery R14, R13.
#[test]
fn with_save_continuously_off_only_saved_versions_list() {
    let scratch = Scratch::new("off");
    let (mut shell, _set) = open(&scratch, false);
    edit(&mut shell, "One");
    let listed = history(&mut shell);
    assert_eq!(listed["continuous"], false);
    assert_eq!(kinds(&listed), ["saved", "backup"], "{listed}");

    // Restoring a backup is a Saved Version, one undo step.
    let backup = id(&listed, 1);
    let restored = restore(&mut shell, &backup).unwrap();
    assert_eq!(restored["undo_step"], "Restore Saved Version");
    assert_eq!(
        shell.editor().undo_history().next(),
        Some("Restore Saved Version")
    );
}

// Spec: crash-recovery R13.
#[test]
fn a_set_never_saved_has_no_history() {
    let scratch = Scratch::new("never");
    let mut shell = shell(&scratch);
    let listed = history(&mut shell);
    assert_eq!(listed["rows"], json!([]));
    assert!(matches!(
        restore(&mut shell, "saved:1"),
        Err(ShellError::Unavailable { .. })
    ));
}

// Spec: crash-recovery R18.
#[test]
fn a_row_that_cant_be_read_restores_nothing() {
    let scratch = Scratch::new("damaged");
    let (mut shell, set) = open(&scratch, true);
    edit(&mut shell, "One");
    edit(&mut shell, "Two");
    let listed = history(&mut shell);
    let song = saved_form(&shell);
    let steps = shell.editor().undo_history().count();

    // A damaged journal.
    let journal = std::fs::read_dir(set.parent().unwrap())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.extension().is_some_and(|e| e == "bljournal"))
        .unwrap();
    std::fs::write(&journal, b"not a journal").unwrap();
    let error = restore(&mut shell, &id(&listed, 0)).unwrap_err();
    assert!(matches!(error, ShellError::Failed { .. }), "{error}");
    assert_eq!(saved_form(&shell), song);
    assert_eq!(shell.editor().undo_history().count(), steps);

    // A backup moved away.
    let backup = std::fs::read_dir(set.parent().unwrap().join("Backup"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let backup_row = listed["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["kind"] == "backup")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    std::fs::remove_file(backup).unwrap();
    assert!(restore(&mut shell, &backup_row).is_err());
    // A person's restore says why in the Status Bar.
    assert!(
        shell
            .invoke("set.restore", json!({ "id": backup_row }))
            .is_err()
    );
    assert!(
        shell
            .message()
            .is_some_and(|m| m.text.contains("couldn't read")),
        "{:?}",
        shell.message()
    );
    assert_eq!(saved_form(&shell), song);
    assert!(restore(&mut shell, "nonsense").is_err());
}

/// Writes a short 16-bit mono WAV file.
fn wav(path: &Path, frames: u32) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let data = frames * 2;
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&48_000u32.to_le_bytes());
    bytes.extend_from_slice(&96_000u32.to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data.to_le_bytes());
    for i in 0..frames {
        let phase = f64::from(i) * 440.0 / 48_000.0 * std::f64::consts::TAU;
        bytes.extend_from_slice(&((phase.sin() * 32.0) as i16).to_le_bytes());
    }
    std::fs::write(path, bytes).unwrap();
}

// Spec: crash-recovery R16; app-shell R21.
#[test]
fn restoring_a_backup_made_before_a_save_as_still_plays_every_clip() {
    let scratch = Scratch::new("media");
    let file = scratch.path("Samples/Kick.wav");
    wav(&file, 480);
    let mut shell = shell(&scratch);
    let first = scratch.path("First.blset");
    let saved = shell
        .run("set.save_as", json!({ "path": text(&first) }))
        .unwrap();
    let first_set = PathBuf::from(saved["path"].as_str().unwrap());
    let track = shell
        .editor()
        .document()
        .tracks()
        .find(|track| track.kind() == TrackKind::Audio)
        .map(bl_model::Track::id)
        .unwrap();
    shell
        .run(
            "browser.add_to_arrangement",
            json!({ "paths": [text(&file)], "track": track, "at": 0 }),
        )
        .unwrap();
    // Two saves, so the first project folder holds a backup that names the
    // sample from there.
    shell.run("set.save", json!({})).unwrap();
    shell.run("set.save", json!({})).unwrap();
    // The newest backup is the version with the clip.
    let backup = bl_project::backups(&first_set.parent().unwrap().join("Backup"), "First")
        .remove(0)
        .path;

    // Save As into another project folder re-points the set's media table;
    // the person brings the old backup along.
    let second = shell
        .run(
            "set.save_as",
            json!({ "path": text(&scratch.path("Elsewhere/Second.blset")) }),
        )
        .unwrap();
    let second_set = PathBuf::from(second["path"].as_str().unwrap());
    let backups = second_set.parent().unwrap().join("Backup");
    std::fs::create_dir_all(&backups).unwrap();
    let carried = backups.join(
        backup
            .file_name()
            .unwrap()
            .to_string_lossy()
            .replace("First", "Second"),
    );
    std::fs::copy(&backup, &carried).unwrap();

    let listed = history(&mut shell);
    let row = listed["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["kind"] == "backup")
        .unwrap_or_else(|| panic!("the carried backup lists: {listed}"))["id"]
        .as_str()
        .unwrap()
        .to_owned();
    // A song without the clip, then the backup puts it back.
    let clip = shell
        .editor()
        .document()
        .tracks()
        .flat_map(|track| track.clips().map(|clip| clip.id))
        .next()
        .unwrap();
    shell.run("clip.delete", json!({ "clip": clip })).unwrap();
    restore(&mut shell, &row).unwrap();

    let hashes: Vec<ContentHash> = shell
        .editor()
        .document()
        .tracks()
        .flat_map(|track| track.clips())
        .filter_map(|clip| match &clip.data {
            ClipData::Audio(audio) => Some(ContentHash::from_bytes(audio.media.0)),
            ClipData::Midi(_) => None,
        })
        .collect();
    assert_eq!(hashes.len(), 1, "the backup's clip is back");
    let files = shell.run("file_manager.state", json!({})).unwrap();
    let rows = files["files"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "{files}");
    assert_ne!(rows[0]["location"], "missing", "the clip finds its file");
    assert_eq!(shell.missing_files(), 0);

    // Opening the older backup, which names no file, empties the set's media
    // table; restoring the saved version brings the clip and its file back.
    let older = bl_project::backups(
        first_set.parent().unwrap().join("Backup").as_path(),
        "First",
    )
    .pop()
    .unwrap()
    .path;
    shell.run("set.save", json!({})).unwrap();
    shell
        .run("set.open_backup", json!({ "path": text(&older) }))
        .unwrap();
    assert_eq!(
        shell.run("file_manager.state", json!({})).unwrap()["files"],
        json!([])
    );
    let saved = id(&history(&mut shell), 0);
    restore(&mut shell, &saved).unwrap();
    let files = shell.run("file_manager.state", json!({})).unwrap();
    let rows = files["files"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "the file is back in the table: {files}");
    assert_ne!(rows[0]["location"], "missing");
    // Saved, the set's media table names the file again.
    shell.run("set.save", json!({})).unwrap();
    let table = bl_project::load(&second_set).unwrap().0.media;
    assert_eq!(table.iter().count(), 1, "the saved set lists its file");
}

// Spec: crash-recovery R12, R20.
#[test]
fn the_panel_toggles_as_view_state_and_shares_the_place_with_the_undo_history() {
    let scratch = Scratch::new("view");
    let (mut shell, _set) = open(&scratch, true);
    let steps = shell.editor().undo_history().count();
    let state = shell.run("view.toggle_save_history", json!({})).unwrap();
    assert_eq!(state["open"], true);
    assert_eq!(state["continuous"], true);
    assert!(shell.save_history_open());
    let view = shell.run("view.state", json!({})).unwrap();
    assert_eq!(view["save_history"], true);
    assert_eq!(shell.editor().undo_history().count(), steps, "no undo step");

    // Showing one panel hides the other.
    shell.run("view.toggle_undo_history", json!({})).unwrap();
    assert!(!shell.save_history_open());
    shell.run("view.toggle_save_history", json!({})).unwrap();
    assert!(!shell.undo_history_open());

    // The panel follows the set while it shows (R15).
    assert_eq!(shell.save_history_rows().len(), 2);
    edit(&mut shell, "One");
    shell.poll();
    assert_eq!(shell.save_history_rows().len(), 3);
    shell.run("set.save", json!({})).unwrap();
    shell.poll();
    assert_eq!(shell.save_history_rows().len(), 3, "saved and two backups");
    let hidden = shell.run("view.toggle_save_history", json!({})).unwrap();
    assert_eq!(hidden["open"], false);
}

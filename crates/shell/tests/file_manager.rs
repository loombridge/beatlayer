//! The File Manager (WP-6.63): the files a set, a project or the User
//! Library uses, the missing ones, the external ones, and the unused ones;
//! the set's backups, and saving a copy. Driven through `Shell::run`, as an
//! agent drives it (D17), on real folders and bl-project's real set files.

use std::path::{Path, PathBuf};

use bl_media::ContentHash;
use bl_model::{ClipData, TrackId, TrackKind};
use bl_project::{Project, SetFile};
use bl_shell::Shell;
use bl_shell::audio::AudioHost;
use bl_shell::files::ProjectFiles;
use bl_shell::settings::Preferences;
use serde_json::json;

/// A folder for one test's files, removed when the test ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "bl-file-manager-{name}-{}-{}",
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

/// A path for a command's arguments.
fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// Writes a short 16-bit mono WAV file that isn't silence.
fn wav(path: &Path, frames: u32) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let data = frames * 2;
    let mut bytes = Vec::with_capacity(44 + data as usize);
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
        let sample = (phase.sin() * 32.0) as i16;
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    std::fs::write(path, bytes).unwrap();
}

fn headless_with_config(config: &Path) -> Shell {
    Shell::new(
        Preferences::default(),
        Some(config.to_owned()),
        AudioHost::null(Default::default()),
        Box::new(ProjectFiles::new()),
    )
}

/// The first audio track, where a dropped file goes.
fn audio_track(shell: &Shell) -> TrackId {
    let document = shell.editor().document();
    document
        .tracks()
        .find(|track| track.kind() == TrackKind::Audio)
        .map(bl_model::Track::id)
        .expect("the default set has audio tracks")
}

/// The content hashes of the clips of the set, in order.
fn clip_media(shell: &Shell) -> Vec<ContentHash> {
    shell
        .editor()
        .document()
        .tracks()
        .flat_map(|track| track.clips())
        .filter_map(|clip| match &clip.data {
            ClipData::Audio(audio) => Some(ContentHash::from_bytes(audio.media.0)),
            ClipData::Midi(_) => None,
        })
        .collect()
}

/// The hash of a file's content, as the File Manager's rows name it.
fn hash_of(path: &Path) -> String {
    hex(ContentHash::of_file(path).unwrap())
}

/// A content hash as the File Manager's rows name it.
fn hex(hash: ContentHash) -> String {
    hash.as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// A set with the files at `paths` on its first audio track, saved in a
/// project folder of its own. Returns the shell and the set's file.
fn set_with_files(scratch: &Scratch, name: &str, paths: &[PathBuf]) -> (Shell, PathBuf) {
    let mut shell = headless_with_config(&scratch.path("config"));
    let song = PathBuf::from(
        shell
            .run(
                "set.save_as",
                json!({ "path": text(&scratch.path(&format!("{name}.blset"))) }),
            )
            .unwrap()["path"]
            .as_str()
            .unwrap(),
    );
    let track = audio_track(&shell);
    for (index, path) in paths.iter().enumerate() {
        shell
            .run(
                "browser.add_to_arrangement",
                json!({
                    "paths": [text(path)],
                    "track": track,
                    "at": index as u64 * 100_000,
                }),
            )
            .unwrap();
    }
    // The set is saved, so its media table names the files.
    shell.run("set.save", json!({})).unwrap();
    (shell, song)
}

// Spec: file-management R11, R15.
#[test]
fn the_files_list_where_they_are() {
    let scratch = Scratch::new("locations");
    let library = scratch.path("Library");
    let in_library = library.join("Samples/Pad.wav");
    let elsewhere = scratch.path("elsewhere/Tone.wav");
    let in_other_project = scratch.path("Other Project/Other.wav");
    wav(&in_library, 480);
    wav(&elsewhere, 960);
    wav(&in_other_project, 1440);
    // The other project is a project folder: it holds a set.
    let other_set = scratch.path("Other Project/Other.blset");
    std::fs::create_dir_all(other_set.parent().unwrap()).unwrap();
    SetFile::new(&other_set).save(&Project::new()).unwrap();

    let mut shell = headless_with_config(&scratch.path("config"));
    shell.set_user_library(Some(library.clone()));
    let song = PathBuf::from(
        shell
            .run(
                "set.save_as",
                json!({ "path": text(&scratch.path("Song.blset")) }),
            )
            .unwrap()["path"]
            .as_str()
            .unwrap(),
    );
    // A file in the set's own project.
    let project_file = song.parent().unwrap().join("Samples/Imported/Kick.wav");
    wav(&project_file, 1920);
    let track = audio_track(&shell);
    for (index, path) in [&project_file, &in_library, &elsewhere, &in_other_project]
        .into_iter()
        .enumerate()
    {
        shell
            .run(
                "browser.add_to_arrangement",
                json!({
                    "paths": [text(path)],
                    "track": track,
                    "at": index as u64 * 100_000,
                }),
            )
            .unwrap();
    }

    let state = shell.run("file_manager.state", json!({})).unwrap();
    let files = state["files"].as_array().unwrap();
    assert_eq!(files.len(), 4, "every file the set uses is listed");
    let location = |name: &str| {
        files
            .iter()
            .find(|file| file["name"] == name)
            .unwrap_or_else(|| panic!("{name} isn't listed"))["location"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    assert_eq!(location("Kick.wav"), "project");
    assert_eq!(location("Pad.wav"), "user-library");
    assert_eq!(location("Tone.wav"), "external");
    assert_eq!(location("Other.wav"), "external");

    // The External Files section groups them by where they are.
    let groups: Vec<(&str, u64)> = state["external"]
        .as_array()
        .unwrap()
        .iter()
        .map(|group| {
            (
                group["group"].as_str().unwrap(),
                group["count"].as_u64().unwrap(),
            )
        })
        .collect();
    assert!(groups.contains(&("elsewhere", 1)), "{groups:?}");
    assert!(groups.contains(&("other-project", 1)), "{groups:?}");
    assert!(groups.contains(&("user-library", 1)), "{groups:?}");

    // Every row says what uses it.
    let kick = files
        .iter()
        .find(|file| file["name"] == "Kick.wav")
        .unwrap();
    assert!(
        !kick["used_by"].as_array().unwrap().is_empty(),
        "the row says what uses the file: {kick}"
    );
}

// Spec: file-management R11.
#[test]
fn replacing_a_file_changes_every_clip_that_used_it_in_one_step() {
    let scratch = Scratch::new("replace");
    let first = scratch.path("first/Kick.wav");
    let second = scratch.path("second/Kick 2.wav");
    wav(&first, 480);
    wav(&second, 960);
    let (mut shell, _song) = set_with_files(&scratch, "Song", &[first.clone(), first.clone()]);
    let before = clip_media(&shell);
    assert_eq!(before.len(), 2, "two clips play the file");
    let steps = shell.editor().undo_history().count();

    let other = hash_of(&second);
    let replaced = shell
        .run(
            "file_manager.replace_file",
            json!({ "hash": hash_of(&first), "path": text(&second) }),
        )
        .unwrap();
    assert_eq!(replaced["replaced"], 2, "both clips used the file");
    let after = clip_media(&shell);
    assert!(
        after.iter().all(|hash| hex(*hash) == other),
        "both clips play the new file: {after:?}"
    );
    assert_eq!(
        shell.editor().undo_history().count(),
        steps + 1,
        "the change is one undo step"
    );

    // One undo brings the old file back.
    shell.run("edit.undo", json!({})).unwrap();
    let back = clip_media(&shell);
    assert!(
        back.iter().all(|hash| hex(*hash) == hex(before[0])),
        "the undo brings the old file back: {back:?}"
    );
}

// Spec: file-management R12, R13.
#[test]
fn a_renamed_file_shows_in_the_status_bar_and_the_missing_files() {
    let scratch = Scratch::new("missing");
    let file = scratch.path("Kick.wav");
    wav(&file, 480);
    let (mut shell, _song) = set_with_files(&scratch, "Song", std::slice::from_ref(&file));
    assert_eq!(shell.missing_files(), 0);

    // The file has moved: it isn't where the set left it.
    let moved = scratch.path("Moved/Kick.wav");
    std::fs::create_dir_all(moved.parent().unwrap()).unwrap();
    std::fs::rename(&file, &moved).unwrap();

    assert_eq!(shell.missing_files(), 1, "the Status Bar counts it");
    let state = shell.run("file_manager.state", json!({})).unwrap();
    assert_eq!(state["missing"].as_array().unwrap().len(), 1);
    assert_eq!(state["missing"][0]["location"], "missing");
}

// Spec: file-management R13.
#[test]
fn a_missing_file_is_relinked_to_other_content_in_one_step() {
    let scratch = Scratch::new("relink");
    let file = scratch.path("Kick.wav");
    let other = scratch.path("Other.wav");
    wav(&file, 480);
    wav(&other, 960);
    let hash = hash_of(&file);
    let (mut shell, _song) = set_with_files(&scratch, "Song", std::slice::from_ref(&file));
    let steps = shell.editor().undo_history().count();
    std::fs::remove_file(&file).unwrap();

    let replaced = shell
        .run(
            "file_manager.replace_file",
            json!({ "hash": hash, "path": text(&other) }),
        )
        .unwrap();
    assert_eq!(replaced["replaced"], 1);
    let media = clip_media(&shell);
    assert_eq!(
        hex(media[0]),
        hash_of(&other),
        "the clip plays the new file"
    );
    assert_eq!(
        shell.editor().undo_history().count(),
        steps + 1,
        "the relink is one undo step"
    );
    assert_eq!(shell.missing_files(), 0, "nothing is missing any more");
}

// Spec: file-management R14.
#[test]
fn a_search_with_two_candidates_relinks_nothing_until_one_is_chosen() {
    let scratch = Scratch::new("candidates");
    let file = scratch.path("Kick.wav");
    let one = scratch.path("One/Kick.wav");
    let two = scratch.path("Two/Kick.wav");
    wav(&file, 480);
    std::fs::create_dir_all(one.parent().unwrap()).unwrap();
    std::fs::create_dir_all(two.parent().unwrap()).unwrap();
    std::fs::copy(&file, &one).unwrap();
    std::fs::copy(&file, &two).unwrap();
    let hash = hash_of(&file);
    let (mut shell, _song) = set_with_files(&scratch, "Song", std::slice::from_ref(&file));
    std::fs::remove_file(&file).unwrap();

    let report = shell
        .run(
            "set.relink_missing",
            json!({ "folders": [text(one.parent().unwrap()), text(two.parent().unwrap())] }),
        )
        .unwrap();
    assert_eq!(
        report["files"], 0,
        "nothing is relinked: the person chooses"
    );
    let candidates = report["candidates"].as_array().unwrap();
    assert_eq!(candidates.len(), 1, "the file has candidates: {report}");
    assert_eq!(
        candidates[0]["paths"].as_array().unwrap().len(),
        2,
        "it has two: {report}"
    );

    // The row's Hot-Swap button arms the browser, and the file chosen
    // there replaces the missing one.
    shell
        .run("file_manager.hot_swap", json!({ "hash": hash }))
        .unwrap();
    shell
        .run(
            "browser.open_folder",
            json!({ "path": text(one.parent().unwrap()) }),
        )
        .unwrap();
    shell
        .run("browser.select", json!({ "paths": [text(&one)] }))
        .unwrap();
    shell.run("browser.load_selected", json!({})).unwrap();
    let media = clip_media(&shell);
    assert_eq!(
        hex(media[0]),
        hash_of(&one),
        "the chosen file replaces the missing one"
    );
}

// Spec: file-management R15.
#[test]
fn collect_and_save_copies_the_groups_that_are_switched_on() {
    let scratch = Scratch::new("collect");
    let elsewhere = scratch.path("elsewhere/Tone.wav");
    let other_project = scratch.path("Other Project/Other.wav");
    wav(&elsewhere, 480);
    wav(&other_project, 960);
    let other_set = scratch.path("Other Project/Other.blset");
    std::fs::create_dir_all(other_set.parent().unwrap()).unwrap();
    SetFile::new(&other_set).save(&Project::new()).unwrap();

    let (mut shell, song) = set_with_files(&scratch, "Song", &[elsewhere, other_project]);
    let report = shell
        .run(
            "set.collect_all_and_save",
            json!({ "groups": ["elsewhere"] }),
        )
        .unwrap();
    assert_eq!(report["files"], 1, "only the elsewhere file is copied");
    let imported = song.parent().unwrap().join("Samples/Imported");
    assert!(
        imported.join("Tone.wav").is_file(),
        "the file was copied in"
    );
    assert!(
        !imported.join("Other.wav").exists(),
        "the other project's file was left alone"
    );
    assert!(
        !shell.is_dirty(),
        "the set was saved: {}",
        shell.set_path().unwrap().display()
    );
}

// Spec: file-management R16.
#[test]
fn a_projects_collect_and_save_rewrites_its_sets_with_a_backup() {
    let scratch = Scratch::new("project-collect");
    let elsewhere = scratch.path("elsewhere/Tone.wav");
    wav(&elsewhere, 480);
    let (mut shell, song) = set_with_files(&scratch, "Song", std::slice::from_ref(&elsewhere));
    let project = song.parent().unwrap().to_path_buf();

    // A second set in the project, which uses the file too.
    let second = project.join("Second.blset");
    let mut data = Project::new();
    data.media.insert(bl_project::MediaRef {
        hash: bl_model::ContentHash(*ContentHash::of_file(&elsewhere).unwrap().as_bytes()),
        path: bl_project::MediaPath::new("../elsewhere/Tone.wav").unwrap(),
        size: std::fs::metadata(&elsewhere).unwrap().len(),
        reversed_from: None,
    });
    SetFile::new(&second).save(&data).unwrap();

    let report = shell
        .run(
            "set.collect_all_and_save",
            json!({ "groups": ["elsewhere"], "project": text(&project) }),
        )
        .unwrap();
    assert_eq!(report["files"], 2, "both sets are collected");
    let imported = project.join("Samples/Imported/Tone.wav");
    assert!(imported.is_file(), "the file was copied in");
    let media = bl_project::load(&second).unwrap().0.media;
    let paths: Vec<String> = media.iter().map(|file| file.path.to_string()).collect();
    assert_eq!(
        paths,
        ["Samples/Imported/Tone.wav"],
        "the set was rewritten"
    );
    let backups: Vec<String> = std::fs::read_dir(project.join("Backup"))
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        backups.iter().any(|name| name.starts_with("Second ")),
        "the second set keeps the version it replaces as a backup: {backups:?}"
    );
}

// Spec: file-management R17.
#[test]
fn an_unused_recording_is_listed_and_deleted_to_the_trash() {
    let scratch = Scratch::new("unused");
    let (mut shell, song) = set_with_files(&scratch, "Song", &[]);
    let project = song.parent().unwrap().to_path_buf();
    let recording = project.join("Samples/Recorded/Take 1.wav");
    wav(&recording, 480);
    shell
        .run(
            "view.toggle_file_manager",
            json!({ "view": "project", "open": true }),
        )
        .unwrap();

    let state = shell.run("file_manager.state", json!({})).unwrap();
    let unused = state["unused"].as_array().unwrap();
    assert_eq!(unused.len(), 1, "the recording is listed: {state}");
    assert_eq!(unused[0]["group"], "recorded");
    assert_eq!(unused[0]["count"], 1);

    let deleted = shell
        .run(
            "file_manager.delete_unused",
            json!({ "paths": [text(&recording)] }),
        )
        .unwrap();
    assert_eq!(deleted["files"].as_array().unwrap().len(), 1);
    assert!(
        !recording.exists(),
        "the file left the project: it is in the trash"
    );
}

// Spec: file-management R18.
#[test]
fn three_saves_give_three_backups_and_opening_one_makes_it_the_set() {
    let scratch = Scratch::new("backups");
    let file = scratch.path("Kick.wav");
    wav(&file, 480);
    let (mut shell, song) = set_with_files(&scratch, "Song", std::slice::from_ref(&file));
    let before = shell.run("set.backups", json!({})).unwrap()["backups"]
        .as_array()
        .unwrap()
        .len();
    let track = audio_track(&shell);
    for _ in 0..3 {
        shell
            .run(
                "browser.add_to_arrangement",
                json!({ "paths": [text(&file)], "track": track, "at": 100_000 }),
            )
            .unwrap();
        shell.run("set.save", json!({})).unwrap();
    }
    let backups = shell.run("set.backups", json!({})).unwrap();
    let list = backups["backups"].as_array().unwrap();
    assert_eq!(
        list.len(),
        before + 3,
        "three saves give three backups: {backups}"
    );
    assert!(
        list[0]["time"].as_str().unwrap().len() >= 15,
        "each names the time it was made: {list:?}"
    );

    // Opening a backup makes it the set, unsaved, with an empty undo
    // history.
    let before = clip_media(&shell).len();
    shell
        .run(
            "set.open_backup",
            json!({ "path": list.last().unwrap()["path"] }),
        )
        .unwrap();
    assert!(shell.is_dirty(), "the set is unsaved");
    assert_eq!(shell.editor().undo_history().count(), 0, "no undo history");
    assert_eq!(
        shell.set_path().unwrap(),
        song.as_path(),
        "the set keeps its own file"
    );
    assert!(
        clip_media(&shell).len() < before,
        "the backup's song is the set's now"
    );

    // Saving makes the backup the set's current version.
    shell.run("set.save", json!({})).unwrap();
    assert!(!shell.is_dirty(), "the set is saved");
}

// Spec: file-management R19.
#[test]
fn save_a_copy_leaves_the_open_set_alone() {
    let scratch = Scratch::new("copy");
    let file = scratch.path("Kick.wav");
    wav(&file, 480);
    let (mut shell, song) = set_with_files(&scratch, "Song", &[file]);
    let before = shell.editor().undo_history().count();
    let copy = scratch.path("Copy.blset");

    let saved = shell
        .run("set.save_copy", json!({ "path": text(&copy) }))
        .unwrap();
    let written = PathBuf::from(saved["path"].as_str().unwrap());
    assert!(
        written.is_file(),
        "the copy was written: {}",
        written.display()
    );
    assert_eq!(
        shell.set_path().unwrap(),
        song.as_path(),
        "the open set keeps its file"
    );
    assert_eq!(shell.set_name(), "Song", "the open set keeps its name");
    assert_eq!(
        shell.editor().undo_history().count(),
        before,
        "the open set keeps its undo history"
    );
}

// Spec: file-management R19. A file the set gained since it was saved is
// listed in the copy by a path from the copy's folder, however deep it is.
#[test]
fn a_copy_lists_files_added_since_the_last_save_from_its_own_folder() {
    let scratch = Scratch::new("copy-added");
    let file = scratch.path("Music/Kick.wav");
    wav(&file, 480);
    let (mut shell, _song) = set_with_files(&scratch, "Song", &[]);
    shell
        .run(
            "browser.add_to_arrangement",
            json!({ "paths": [text(&file)], "track": audio_track(&shell), "at": 0 }),
        )
        .unwrap();
    let copy = scratch.path("Library/Templates/Deep/Copy.blset");
    let saved = shell
        .run("set.save_copy", json!({ "path": text(&copy) }))
        .unwrap();
    let written = PathBuf::from(saved["path"].as_str().unwrap());

    let (_, project) = SetFile::open(&written).unwrap();
    let folder = written.parent().unwrap();
    let listed: Vec<PathBuf> = project
        .media
        .iter()
        .filter_map(|media| media.path.resolve(folder))
        .collect();
    assert_eq!(listed.len(), 1, "the copy lists the file");
    assert!(
        listed[0].is_file(),
        "the copy's path reaches the file: {}",
        listed[0].display()
    );
}

// Spec: file-management R15. Audio clips pasted into another set take their
// files into its media table, so the set finds them when it's opened again.
#[test]
fn pasting_audio_clips_into_a_new_set_lists_their_files() {
    let scratch = Scratch::new("paste-media");
    let file = scratch.path("Music/Kick.wav");
    wav(&file, 480);
    let (mut shell, _song) = set_with_files(&scratch, "Song", std::slice::from_ref(&file));
    let clip = shell
        .editor()
        .document()
        .tracks()
        .flat_map(|track| track.clips())
        .find(|clip| clip.data.is_audio())
        .map(|clip| clip.id)
        .unwrap();
    shell
        .run("arrangement.copy", json!({ "clips": [clip] }))
        .unwrap();
    shell.run("set.new", json!({})).unwrap();
    shell
        .run(
            "arrangement.paste",
            json!({ "at": 0, "track": audio_track(&shell) }),
        )
        .unwrap();
    let saved = shell
        .run(
            "set.save_as",
            json!({ "path": text(&scratch.path("Other/Pasted.blset")) }),
        )
        .unwrap();
    let written = PathBuf::from(saved["path"].as_str().unwrap());

    let (_, project) = SetFile::open(&written).unwrap();
    let folder = written.parent().unwrap();
    let listed: Vec<PathBuf> = project
        .media
        .iter()
        .filter_map(|media| media.path.resolve(folder))
        .collect();
    assert_eq!(listed.len(), 1, "the new set lists the pasted clip's file");
    assert!(listed[0].is_file(), "{}", listed[0].display());
}

// Spec: file-management R14.
#[test]
fn the_search_folder_is_the_one_the_person_names() {
    let scratch = Scratch::new("search-folder");
    let file = scratch.path("Kick.wav");
    let moved = scratch.path("Moved/Kick.wav");
    wav(&file, 480);
    let (mut shell, _song) = set_with_files(&scratch, "Song", std::slice::from_ref(&file));
    std::fs::create_dir_all(moved.parent().unwrap()).unwrap();
    std::fs::rename(&file, &moved).unwrap();

    // The folder is named, and the search covers it.
    shell
        .run(
            "file_manager.set_search_folder",
            json!({ "folder": text(&scratch.path("Moved")) }),
        )
        .unwrap();
    let state = shell.run("file_manager.state", json!({})).unwrap();
    assert_eq!(
        state["search"]["folder"].as_str(),
        Some(text(&scratch.path("Moved")).as_str()),
        "the state names the folder: {state}"
    );
    let report = shell
        .run(
            "set.relink_missing",
            json!({ "folder": text(&scratch.path("Moved")) }),
        )
        .unwrap();
    assert_eq!(report["files"], 1, "the file was found in it: {report}");
    assert_eq!(shell.missing_files(), 0, "nothing is missing any more");
}

// Spec: file-management R10.
#[test]
fn the_file_manager_and_the_plugin_list_hide_each_other() {
    let mut shell = Shell::headless();
    shell
        .run("view.toggle_file_manager", json!({ "open": true }))
        .unwrap();
    assert!(shell.file_manager_open(), "the File Manager shows");
    shell.run("view.toggle_plugin_list", json!({})).unwrap();
    assert!(
        !shell.file_manager_open(),
        "the plug-in list hides the File Manager"
    );
    shell
        .run("view.toggle_file_manager", json!({ "open": true }))
        .unwrap();
    assert!(
        !shell.plugin_list().visible(),
        "the File Manager hides the plug-in list"
    );
}

// Spec: file-management R20.
#[test]
fn the_file_manager_commands_are_registered_and_reach_the_agent_api() {
    use bl_commands::{Context, Permission};
    let shell = Shell::headless();
    let registry = shell.registry();
    for (id, permission) in [
        (
            bl_shell::file_manager_commands::VIEW_TOGGLE_FILE_MANAGER,
            Permission::Read,
        ),
        (
            bl_shell::file_manager_commands::SET_SAVE_COPY,
            Permission::External,
        ),
    ] {
        let spec = registry
            .get(id)
            .unwrap_or_else(|| panic!("{id} isn't registered"));
        assert_eq!(spec.permission, Some(permission));
        assert_eq!(spec.contexts, [Context::Global]);
    }
    for (id, permission) in [
        ("file_manager.state", Permission::External),
        ("file_manager.replace_file", Permission::External),
        ("file_manager.hot_swap", Permission::Edit),
        ("file_manager.edit_file", Permission::External),
        ("file_manager.set_search_folder", Permission::Read),
        ("file_manager.delete_unused", Permission::Destructive),
        ("set.backups", Permission::Read),
        ("set.open_backup", Permission::External),
    ] {
        let spec = registry
            .get(id)
            .unwrap_or_else(|| panic!("{id} isn't registered"));
        assert_eq!(spec.permission, Some(permission), "{id}");
    }

    // The commands read back where the readback table says.
    for (id, field) in [
        ("file_manager.replace_file", "data.audio.media"),
        ("file_manager.hot_swap", "file_manager.state"),
    ] {
        let readback = bl_commands::readback::READBACK
            .iter()
            .find(|(command, _)| *command == id)
            .unwrap_or_else(|| panic!("{id} has no readback"));
        let shown = match readback.1 {
            bl_commands::readback::Readback::Query { field: shows, .. } => shows.to_owned(),
            bl_commands::readback::Readback::Nothing { reason } => reason.to_owned(),
            other => panic!("{id} reads back with {other:?}"),
        };
        assert!(
            shown.contains(field),
            "{id} reads back with {shown:?}, which doesn't name {field}"
        );
    }
}

/// What the set saved at `song` lists for `hash`, resolved from its folder.
fn listed_in(song: &Path, hash: ContentHash) -> Option<PathBuf> {
    let (_, project) = SetFile::open(song).unwrap();
    project
        .media
        .get(&bl_model::ContentHash(*hash.as_bytes()))?
        .path
        .resolve(song.parent().unwrap())
}

// Spec: file-management R1. A new set starts from the saved default set
// with its files and its launch quantisation, so it plays them and a save
// lists them, after a restart and after File > New Set.
#[test]
fn a_new_set_from_a_default_set_keeps_its_files_and_launch_quantisation() {
    let scratch = Scratch::new("default-set-media");
    let file = scratch.path("samples/k.wav");
    wav(&file, 480);
    let hash = ContentHash::of_file(&file).unwrap();
    let mut shell = headless_with_config(&scratch.path("config"));
    shell
        .run(
            "browser.add_to_arrangement",
            json!({ "paths": [text(&file)], "track": audio_track(&shell), "at": 0 }),
        )
        .unwrap();
    shell
        .run(
            "session.set_quantization",
            json!({ "quantization": "quarter" }),
        )
        .unwrap();
    let default = scratch.path("Default.blset");
    shell
        .run("set.save_as_default", json!({ "path": text(&default) }))
        .unwrap();

    let check = |shell: &mut Shell, name: &str| {
        assert_eq!(
            shell.launch_settings().quantization,
            bl_model::LaunchQuantization::Quarter
        );
        let song = PathBuf::from(
            shell
                .run(
                    "set.save_as",
                    json!({ "path": text(&scratch.path(&format!("{name}/{name}.blset"))) }),
                )
                .unwrap()["path"]
                .as_str()
                .unwrap(),
        );
        let listed = listed_in(&song, hash).expect("the saved set lists the default set's file");
        assert_eq!(listed.canonicalize().unwrap(), file.canonicalize().unwrap());
    };

    // The next launch.
    let mut launched = Shell::new(
        Preferences {
            default_set: Some(default.clone()),
            ..Preferences::default()
        },
        Some(scratch.path("config2")),
        AudioHost::null(Default::default()),
        Box::new(ProjectFiles::new()),
    );
    assert_eq!(clip_media(&launched).len(), 1, "the default set's clip");
    std::fs::rename(&file, scratch.path("moved.wav")).unwrap();
    assert_eq!(
        launched.missing_files(),
        1,
        "the media store knows the file, so it reports it missing"
    );
    std::fs::rename(scratch.path("moved.wav"), &file).unwrap();
    check(&mut launched, "launched");

    // File > New Set in the session that saved it.
    shell.run("set.new", json!({})).unwrap();
    check(&mut shell, "new");
}

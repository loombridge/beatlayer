//! Templates and file management (WP-2.25): the default set a new set starts
//! from, templates in the User Library, Collect All and Save, and finding
//! and relinking missing files. Driven through `Shell::run`, as an agent
//! drives it (D17), on real folders and bl-project's real set files.

use std::path::{Path, PathBuf};

use bl_media::{ContentHash, MediaStatus};
use bl_model::{ClipData, TrackId, TrackKind};
use bl_project::{MediaPath, MediaRef, Project, ProjectFolder, SetFile};
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
            "bl-file-management-{name}-{}-{}",
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

fn track_names(shell: &Shell) -> Vec<String> {
    let document = shell.editor().document();
    document
        .top_level()
        .map(|id| document.track(id).unwrap().name().to_owned())
        .collect()
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

/// The content hash of the clip whose file is added at 1.1.1.
fn added_hash(shell: &Shell, track: TrackId) -> ContentHash {
    let document = shell.editor().document();
    let clip = document
        .track(track)
        .unwrap()
        .clips()
        .next()
        .expect("a clip was added");
    match &clip.data {
        ClipData::Audio(audio) => ContentHash::from_bytes(audio.media.0),
        ClipData::Midi(_) => unreachable!("an audio clip"),
    }
}

/// The media table of a set file, as the paths it names.
fn media_paths(set: &Path) -> Vec<String> {
    bl_project::load(set)
        .unwrap()
        .0
        .media
        .iter()
        .map(|file| file.path.to_string())
        .collect()
}

// Spec: file-management R1, R2.
#[test]
fn a_new_set_opens_from_the_saved_default() {
    let scratch = Scratch::new("default");
    let config = scratch.path("config");
    let mut shell = headless_with_config(&config);
    shell
        .invoke("track.create", json!({ "kind": "audio", "name": "Bass" }))
        .unwrap();

    let default = scratch.path("Default Set.blset");
    let saved = shell
        .run("set.save_as_default", json!({ "path": text(&default) }))
        .unwrap();
    assert_eq!(saved["path"], text(&default));
    assert_eq!(shell.set_path(), None, "the open set keeps its own file");
    assert_eq!(shell.set_name(), "Untitled");
    assert!(track_names(&shell).iter().any(|name| name == "Bass"));
    assert_eq!(
        bl_project::load(&default)
            .unwrap()
            .0
            .document
            .tracks()
            .count(),
        shell.editor().document().tracks().count()
    );

    // A new set opens from it, not from the factory default set.
    shell.run("set.new", json!({})).unwrap();
    assert!(
        track_names(&shell).iter().any(|name| name == "Bass"),
        "a new set starts from the default: {:?}",
        track_names(&shell)
    );

    // The choice is kept for the next start: a fresh shell opens from it.
    let prefs = Preferences::load(&config).unwrap();
    assert_eq!(prefs.default_set.as_deref(), Some(default.as_path()));
    let relaunched = Shell::new(
        prefs,
        Some(config),
        AudioHost::null(Default::default()),
        Box::new(ProjectFiles::new()),
    );
    assert!(track_names(&relaunched).iter().any(|name| name == "Bass"));
}

// Spec: file-management R4.
#[test]
fn collect_all_and_save_copies_external_files_into_the_project() {
    let scratch = Scratch::new("collect");
    let outside = scratch.path("outside/Tone.wav");
    std::fs::create_dir_all(outside.parent().unwrap()).unwrap();
    wav(&outside, 480);
    let before = std::fs::read(&outside).unwrap();

    let mut shell = headless_with_config(&scratch.path("config"));
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
    let track = audio_track(&shell);
    shell
        .run(
            "browser.add_to_arrangement",
            json!({ "paths": [text(&outside)], "track": track, "at": 0 }),
        )
        .unwrap();
    let hash = added_hash(&shell, track);

    let report = shell.run("set.collect_all_and_save", json!({})).unwrap();
    assert_eq!(report["files"], 1);
    let imported = song.parent().unwrap().join("Samples/Imported/Tone.wav");
    assert!(imported.is_file(), "the file was copied in");
    assert_eq!(std::fs::read(&imported).unwrap(), before, "byte for byte");
    assert_eq!(
        media_paths(&song),
        ["Samples/Imported/Tone.wav"],
        "the set points at the copy"
    );

    // The reopened set plays the same file, found inside the project.
    let mut reopened = Shell::headless();
    reopened
        .run("set.open", json!({ "path": text(&song) }))
        .unwrap();
    let store = reopened.media_store();
    assert_eq!(store.check(hash).unwrap(), MediaStatus::Available);
    let entry = store.entry(hash).unwrap();
    assert!(
        entry.path.starts_with(song.parent().unwrap()),
        "the media lives in the project: {}",
        entry.path.display()
    );
}

// Spec: file-management R6.
#[test]
fn a_moved_file_is_found_by_the_automatic_search() {
    let scratch = Scratch::new("relink");
    let project = ProjectFolder::named(&scratch.0, "Song");
    std::fs::create_dir_all(project.imported()).unwrap();
    let file = project.imported().join("Kick.wav");
    wav(&file, 480);
    let hash = ContentHash::of_file(&file).unwrap();
    let size = std::fs::metadata(&file).unwrap().len();

    // The set records the file where it was; the file has since moved into
    // a subfolder of the project.
    let mut data = Project::new();
    data.media.insert(MediaRef {
        hash: bl_model::ContentHash(*hash.as_bytes()),
        path: MediaPath::new("Samples/Imported/Kick.wav").unwrap(),
        size,
    });
    let set_path = project.set_path("Song");
    SetFile::new(&set_path).save(&data).unwrap();
    let moved = project.imported().join("Sub/Kick 2.wav");
    std::fs::create_dir_all(moved.parent().unwrap()).unwrap();
    std::fs::rename(&file, &moved).unwrap();

    // Opening the set finds it by content, whatever its name.
    let mut shell = Shell::headless();
    shell
        .run("set.open", json!({ "path": text(&set_path) }))
        .unwrap();
    assert_eq!(
        shell.media_store().check(hash).unwrap(),
        MediaStatus::Available
    );
    assert_eq!(
        shell.media_store().entry(hash).unwrap().path,
        moved,
        "relinked to where it is now"
    );
    assert!(
        shell
            .message()
            .is_some_and(|message| message.text.contains("relinked")),
        "the Status Bar says so: {:?}",
        shell.message().map(|message| message.text.clone())
    );

    // A save keeps the new path, so the next open needs no search.
    shell.run("set.save", json!({})).unwrap();
    assert_eq!(media_paths(&set_path), ["Samples/Imported/Sub/Kick 2.wav"]);

    // The command searches too, with the folders it's given.
    let mut again = Shell::headless();
    again
        .run("set.open", json!({ "path": text(&set_path) }))
        .unwrap();
    let third = project.imported().join("Elsewhere/Kick 3.wav");
    std::fs::create_dir_all(third.parent().unwrap()).unwrap();
    std::fs::rename(&moved, &third).unwrap();
    let report = again
        .run("set.relink_missing", json!({ "folders": [] }))
        .unwrap();
    assert_eq!(report["files"], 1);
    assert_eq!(again.media_store().entry(hash).unwrap().path, third);
}

// Spec: file-management R3, R5.
#[test]
fn templates_are_saved_into_the_user_library_and_listed() {
    let scratch = Scratch::new("template");
    let library = scratch.path("Music/Beatlayer/User Library");
    let mut shell = headless_with_config(&scratch.path("config"));
    shell.set_user_library(Some(library.clone()));
    shell
        .invoke("track.create", json!({ "kind": "midi", "name": "Lead" }))
        .unwrap();

    let saved = shell
        .run("set.save_as_template", json!({ "name": "My Template" }))
        .unwrap();
    let template = library.join("Templates/My Template.blset");
    assert_eq!(saved["path"], text(&template));
    assert!(template.is_file(), "the Templates folder is made");
    assert!(track_names(&shell).iter().any(|name| name == "Lead"));

    // The browser's Templates label lists it.
    let state: bl_shell_spec::browser::BrowserState = serde_json::from_value(
        shell
            .run("browser.open_place", json!({ "label": "templates" }))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        state
            .rows
            .iter()
            .map(|row| row.name.as_str())
            .collect::<Vec<_>>(),
        ["My Template.blset"]
    );
}

// Spec: file-management R2, R3, R4, R6.
#[test]
fn the_file_commands_are_registered_and_reach_the_agent_api() {
    use bl_commands::{Context, Permission};
    use bl_shell::commands;
    let shell = Shell::headless();
    let registry = shell.registry();
    for (id, label, permission) in [
        (
            commands::SET_SAVE_AS_DEFAULT,
            "Save Beatlayer Set As Default Set...",
            Permission::Destructive,
        ),
        (
            commands::SET_SAVE_AS_TEMPLATE,
            "Save Beatlayer Set As Template...",
            Permission::Destructive,
        ),
        (
            commands::SET_COLLECT_ALL,
            "Collect All and Save",
            Permission::External,
        ),
        (
            commands::SET_RELINK_MISSING,
            "Relink Missing Files",
            Permission::External,
        ),
    ] {
        let spec = registry
            .get(id)
            .unwrap_or_else(|| panic!("{id} isn't registered"));
        assert_eq!(spec.label, label);
        assert_eq!(spec.permission, Some(permission));
        assert_eq!(spec.contexts, [Context::Global]);
    }
}

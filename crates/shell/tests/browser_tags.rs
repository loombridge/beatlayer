//! The browser's database and its tags (WP-6.31; spec: browser-tags R1 to
//! R17), driven headless through `Shell::run`, as agents drive them: the
//! database on disk, the tag groups and tags, the Tag Editor's and Quick
//! Tags' commands, and searching by tag.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use bl_shell::Shell;
use bl_shell::browser_commands::{self as ids, TagsState};
use bl_shell::settings::Preferences;
use serde_json::{Value, json};

/// A folder of its own under the system's temporary folder, removed when
/// dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("bl-tags-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self, rest: &str) -> PathBuf {
        self.0.join(rest)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Writes a 16-bit mono WAV file of `frames` frames at 48 kHz: a quiet
/// ramp, so it isn't silence.
fn wav(path: &Path, frames: u32) {
    let mut bytes = Vec::new();
    let data = frames * 2;
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
        let sample = ((i % 100) as i16 - 50) * 20;
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, bytes).unwrap();
}

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// A shell on the null device, with its configuration folder.
fn shell_on(config: &Path) -> Shell {
    Shell::new(
        Preferences::default(),
        Some(config.to_path_buf()),
        bl_shell::audio::AudioHost::null(Default::default()),
        Box::new(bl_shell::files::ProjectFiles::new()),
    )
}

/// Waits for `done`, polling the shell as the host does each frame.
fn wait_until(shell: &mut Shell, what: &str, mut done: impl FnMut(&mut Shell) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        shell.poll();
        if done(shell) {
            return;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn wait_for_index(shell: &mut Shell) {
    wait_until(shell, "the index", |shell| {
        !shell.browser_view().index_state().indexing
    });
}

fn run<T: serde::de::DeserializeOwned>(shell: &mut Shell, id: &str, args: Value) -> T {
    serde_json::from_value(
        shell
            .run(id, args)
            .unwrap_or_else(|e| panic!("{id} failed: {e}")),
    )
    .unwrap()
}

/// The names of the tag groups, in order.
fn groups(tags: &TagsState) -> Vec<String> {
    tags.groups.iter().map(|g| g.name.clone()).collect()
}

/// The full names of a group's tags.
fn group_tags(tags: &TagsState, group: &str) -> Vec<String> {
    tags.groups
        .iter()
        .find(|g| g.name == group)
        .map(|g| g.tags.iter().map(|t| t.full_name.clone()).collect())
        .unwrap_or_default()
}

// Spec: browser-tags R1, R6, R8 and R13.
#[test]
fn tags_and_the_index_survive_a_restart() {
    let scratch = Scratch::new("restart");
    let config = scratch.path("config");
    let library = scratch.path("Library");
    std::fs::create_dir_all(&library).unwrap();
    wav(&library.join("Kick 10.wav"), 100);
    wav(&library.join("Snare.wav"), 100);

    let mut shell = shell_on(&config);
    shell.set_user_library(Some(library.clone()));
    wait_for_index(&mut shell);
    // The person makes a group and a tag, and puts the tag on a file.
    run::<TagsState>(&mut shell, ids::ADD_TAG_GROUP, json!({ "name": "Mine" }));
    run::<TagsState>(
        &mut shell,
        ids::ADD_TAG,
        json!({ "name": "Favourite", "group": "Mine" }),
    );
    run::<TagsState>(
        &mut shell,
        ids::TAG_ITEMS,
        json!({
            "items": [text(&library.join("Kick 10.wav"))],
            "tags": ["Mine|Favourite"],
            "attach": true,
        }),
    );
    shell.browser_view().flush_database();

    // What's in the database?
    let database_holds = || {
        let connection = rusqlite::Connection::open(config.join("browser.db")).unwrap();
        let mut rows = connection.prepare("SELECT path FROM files").unwrap();
        let all: Vec<String> = rows
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .flatten()
            .collect();
        all
    };
    // A new shell on the same configuration folder.
    let mut relaunched = shell_on(&config);
    relaunched.set_user_library(Some(library.clone()));
    let tags: TagsState = run(&mut relaunched, ids::TAGS, json!({}));
    assert_eq!(
        groups(&tags),
        ["Content", "Format", "Creator", "Devices", "Mine"]
    );
    assert_eq!(group_tags(&tags, "Mine"), ["Mine|Favourite"]);
    // The file still carries the tag.
    let tagged: TagsState = run(
        &mut relaunched,
        ids::TAGS,
        json!({ "items": [text(&library.join("Kick 10.wav"))] }),
    );
    assert_eq!(tagged.items.len(), 1);
    let names: Vec<&str> = tagged.items[0]
        .tags
        .iter()
        .map(|t| t.tag.as_str())
        .collect();
    assert!(names.contains(&"Mine|Favourite"), "{names:?}");
    // The index is there from the first poll: the search finds the file
    // before the walk has finished.
    let found: bl_shell::browser_commands::BrowserState = run(
        &mut relaunched,
        ids::SEARCH,
        json!({ "query": "kick", "label": "all" }),
    );
    assert!(
        found.rows.iter().any(|row| row.name == "Kick 10.wav"),
        "{:?} in {:?}",
        found.rows,
        database_holds()
    );
}

// Spec: browser-tags R3.
#[test]
fn a_damaged_database_is_set_aside_and_built_again() {
    let scratch = Scratch::new("damaged");
    let config = scratch.path("config");
    std::fs::create_dir_all(&config).unwrap();
    let database = config.join("browser.db");
    std::fs::write(&database, b"this is not a database").unwrap();

    let mut shell = shell_on(&config);
    shell.poll();
    let message = shell.message().expect("the Status Bar says so");
    assert!(
        message.text.contains("couldn't be read"),
        "{}",
        message.text
    );
    assert!(message.error, "it's a failure");
    assert!(database.exists(), "a new database is built");
    assert!(
        config.join("browser.db.damaged").exists(),
        "the damaged file is set aside"
    );
    // The new database works: the tags go in it.
    run::<TagsState>(&mut shell, ids::ADD_TAG_GROUP, json!({ "name": "Mine" }));
    shell.browser_view().flush_database();
    let mut again = shell_on(&config);
    let tags: TagsState = run(&mut again, ids::TAGS, json!({}));
    assert_eq!(
        groups(&tags),
        ["Content", "Format", "Creator", "Devices", "Mine"]
    );
}

// Spec: browser-tags R4.
#[test]
fn a_newer_database_is_left_alone() {
    let scratch = Scratch::new("newer");
    let config = scratch.path("config");
    std::fs::create_dir_all(&config).unwrap();
    let database = config.join("browser.db");
    {
        let mut shell = shell_on(&config);
        run::<TagsState>(&mut shell, ids::ADD_TAG_GROUP, json!({ "name": "Mine" }));
        shell.browser_view().flush_database();
    }
    // A file from a later version of Beatlayer.
    let connection = rusqlite::Connection::open(&database).unwrap();
    connection.pragma_update(None, "user_version", 99).unwrap();
    drop(connection);
    let before = std::fs::metadata(&database).unwrap().len();

    let mut shell = shell_on(&config);
    let tags: TagsState = run(&mut shell, ids::TAGS, json!({}));
    assert_eq!(
        groups(&tags),
        ["Content", "Format", "Creator", "Devices"],
        "the file's tags aren't read"
    );
    // The file isn't written to.
    shell.browser_view().flush_database();
    drop(shell);
    let connection = rusqlite::Connection::open(&database).unwrap();
    let version: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, 99, "the version is untouched");
    assert_eq!(
        std::fs::metadata(&database).unwrap().len(),
        before,
        "the file isn't written to"
    );
}

// Spec: browser-tags R3.
#[test]
fn two_shells_on_one_configuration_folder_tag_in_turn_without_damage() {
    let scratch = Scratch::new("two-shells");
    let config = scratch.path("config");
    let library = scratch.path("Library");
    std::fs::create_dir_all(&library).unwrap();
    wav(&library.join("Kick.wav"), 100);
    wav(&library.join("Snare.wav"), 100);

    let mut first = shell_on(&config);
    run::<TagsState>(&mut first, ids::ADD_TAG_GROUP, json!({ "name": "Mine" }));
    run::<TagsState>(
        &mut first,
        ids::ADD_TAG,
        json!({ "name": "Kick", "group": "Mine" }),
    );
    first.browser_view().flush_database();
    // The second shell starts on the same configuration folder, and knows
    // the group the first made.
    let mut second = shell_on(&config);
    run::<TagsState>(
        &mut second,
        ids::ADD_TAG,
        json!({ "name": "Snare", "group": "Mine" }),
    );
    run::<TagsState>(
        &mut first,
        ids::TAG_ITEMS,
        json!({
            "items": [text(&library.join("Kick.wav"))],
            "tags": ["Mine|Kick"],
            "attach": true,
        }),
    );
    run::<TagsState>(
        &mut second,
        ids::TAG_ITEMS,
        json!({
            "items": [text(&library.join("Snare.wav"))],
            "tags": ["Mine|Snare"],
            "attach": true,
        }),
    );
    first.browser_view().flush_database();
    second.browser_view().flush_database();

    // A third shell reads both: the file isn't damaged.
    let mut third = shell_on(&config);
    let tags: TagsState = run(
        &mut third,
        ids::TAGS,
        json!({
            "items": [
                text(&library.join("Kick.wav")),
                text(&library.join("Snare.wav")),
            ]
        }),
    );
    let names: Vec<&str> = tags
        .items
        .iter()
        .flat_map(|item| item.tags.iter().map(|t| t.tag.as_str()))
        .collect();
    assert!(names.contains(&"Mine|Kick"), "{names:?}");
    assert!(names.contains(&"Mine|Snare"), "{names:?}");
}

// Spec: browser-tags R5.
#[test]
fn a_tagged_file_moved_outside_beatlayer_keeps_its_tags_by_content() {
    let scratch = Scratch::new("moved");
    let config = scratch.path("config");
    let library = scratch.path("Library");
    std::fs::create_dir_all(&library).unwrap();
    let kick = library.join("Kick.wav");
    wav(&kick, 100);
    let mut shell = shell_on(&config);
    shell.set_user_library(Some(library.clone()));
    wait_for_index(&mut shell);
    run::<TagsState>(&mut shell, ids::ADD_TAG_GROUP, json!({ "name": "Mine" }));
    run::<TagsState>(
        &mut shell,
        ids::ADD_TAG,
        json!({ "name": "Favourite", "group": "Mine" }),
    );
    run::<TagsState>(
        &mut shell,
        ids::TAG_ITEMS,
        json!({
            "items": [text(&kick)],
            "tags": ["Mine|Favourite"],
            "attach": true,
        }),
    );
    // The file's content hash is known.
    wait_until(&mut shell, "the file's hash", |shell| {
        shell
            .browser_view()
            .tagged_file_hash(&text(&kick))
            .is_some()
    });

    // The file is renamed outside Beatlayer.
    let moved = library.join("Renamed.wav");
    std::fs::rename(&kick, &moved).unwrap();
    shell.browser_view().flush_database();
    // The next start indexes the place again, and the tags pass to the
    // file with the same content.
    let mut again = shell_on(&config);
    again.set_user_library(Some(library.clone()));
    wait_for_index(&mut again);
    wait_until(&mut again, "the tags to follow the file", |shell| {
        let tags: TagsState = run(shell, ids::TAGS, json!({ "items": [text(&moved)] }));
        tags.items
            .first()
            .is_some_and(|item| item.tags.iter().any(|t| t.tag == "Mine|Favourite"))
    });
}

/// The tags of group "Mine" that `path` carries, by full name.
fn tags_of(shell: &mut Shell, path: &Path) -> Vec<String> {
    let tags: TagsState = run(shell, ids::TAGS, json!({ "items": [text(path)] }));
    tags.items
        .first()
        .map(|item| {
            item.tags
                .iter()
                .filter(|t| t.tag.starts_with("Mine|"))
                .map(|t| t.tag.clone())
                .collect()
        })
        .unwrap_or_default()
}

/// Makes the group "Mine" with the tags `names`, and tags `path` with each.
fn tag_file(shell: &mut Shell, path: &Path, names: &[&str]) {
    let _ = shell.run(ids::ADD_TAG_GROUP, json!({ "name": "Mine" }));
    for name in names {
        let _ = shell.run(ids::ADD_TAG, json!({ "name": name, "group": "Mine" }));
    }
    let full: Vec<String> = names.iter().map(|name| format!("Mine|{name}")).collect();
    run::<TagsState>(
        shell,
        ids::TAG_ITEMS,
        json!({ "items": [text(path)], "tags": full, "attach": true }),
    );
    wait_until(shell, "the file's hash", |shell| {
        shell.browser_view().tagged_file_hash(&text(path)).is_some()
    });
}

/// Takes `folder` out of the places, and waits until the browser has
/// indexed again and hashed what that asked for.
fn remove_place(shell: &mut Shell, folder: &Path) {
    run::<Value>(shell, ids::REMOVE_FOLDER, json!({ "path": text(folder) }));
    wait_for_index(shell);
    wait_until(shell, "the hasher", |shell| !shell.browser_view().hashing());
}

// Spec: browser-tags R5. A file that is still on disk keeps its tags when its
// place leaves the index, whatever copies of it the other places hold.
#[test]
fn a_project_switch_keeps_tags_on_files_still_on_disk() {
    let scratch = Scratch::new("switch");
    let config = scratch.path("config");
    let library = scratch.path("Library");
    let project = scratch.path("ProjA");
    let library_kick = library.join("Samples/kick.wav");
    let project_kick = project.join("Samples/Imported/kick.wav");
    wav(&library_kick, 100);
    wav(&project_kick, 100);
    let mut shell = shell_on(&config);
    shell.set_user_library(Some(library));
    run::<Value>(
        &mut shell,
        ids::ADD_FOLDER,
        json!({ "path": text(&project) }),
    );
    wait_for_index(&mut shell);
    tag_file(&mut shell, &library_kick, &["Lo-fi"]);
    tag_file(&mut shell, &project_kick, &["Punchy"]);

    remove_place(&mut shell, &project);
    assert_eq!(tags_of(&mut shell, &project_kick), ["Mine|Punchy"]);
    assert_eq!(tags_of(&mut shell, &library_kick), ["Mine|Lo-fi"]);

    // With the library's copy untagged, the project's tags stay where they
    // are, and are there when the place comes back.
    run::<Value>(
        &mut shell,
        ids::ADD_FOLDER,
        json!({ "path": text(&project) }),
    );
    wait_for_index(&mut shell);
    run::<TagsState>(
        &mut shell,
        ids::TAG_ITEMS,
        json!({ "items": [text(&library_kick)], "tags": ["Mine|Lo-fi"], "attach": false }),
    );
    remove_place(&mut shell, &project);
    assert_eq!(tags_of(&mut shell, &project_kick), ["Mine|Punchy"]);
    run::<Value>(
        &mut shell,
        ids::ADD_FOLDER,
        json!({ "path": text(&project) }),
    );
    wait_for_index(&mut shell);
    assert_eq!(tags_of(&mut shell, &project_kick), ["Mine|Punchy"]);
    assert!(tags_of(&mut shell, &library_kick).is_empty());
}

// Spec: browser-tags R5. The hash of a tagged file outlasts its row in the
// index, so its tags follow it after more than one start.
#[test]
fn a_tagged_files_hash_survives_a_restart_after_it_leaves_the_index() {
    let scratch = Scratch::new("hash-survives");
    let config = scratch.path("config");
    let root = scratch.path("Root");
    let library = root.join("Library");
    let elsewhere = root.join("Elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    let kick = library.join("Kick.wav");
    wav(&kick, 100);
    let moved = elsewhere.join("Moved.wav");

    let mut first = shell_on(&config);
    first.set_user_library(Some(library.clone()));
    wait_for_index(&mut first);
    tag_file(&mut first, &kick, &["Favourite"]);
    first.browser_view().flush_database();
    drop(first);
    std::fs::rename(&kick, &moved).unwrap();

    // The start that drops the file's row from the index.
    let mut second = shell_on(&config);
    second.set_user_library(Some(library.clone()));
    wait_for_index(&mut second);
    second.browser_view().flush_database();
    drop(second);

    let mut third = shell_on(&config);
    third.set_user_library(Some(library));
    wait_for_index(&mut third);
    run::<Value>(
        &mut third,
        ids::ADD_FOLDER,
        json!({ "path": text(&elsewhere) }),
    );
    wait_for_index(&mut third);
    wait_until(&mut third, "the tags to follow the file", |shell| {
        tags_of(shell, &moved) == ["Mine|Favourite"]
    });
}

// Spec: browser-tags R5. A collected file that moves outside Beatlayer keeps
// its collections too.
#[test]
fn a_collected_file_moved_outside_beatlayer_keeps_its_collections_by_content() {
    let scratch = Scratch::new("moved-collected");
    let config = scratch.path("config");
    let library = scratch.path("Library");
    std::fs::create_dir_all(&library).unwrap();
    let kick = library.join("Kick.wav");
    wav(&kick, 100);
    let mut shell = shell_on(&config);
    shell.set_user_library(Some(library.clone()));
    wait_for_index(&mut shell);
    run::<Value>(
        &mut shell,
        ids::SET_COLLECTION,
        json!({ "collection": 2, "items": [text(&kick)], "member": true }),
    );
    wait_until(&mut shell, "the file's hash", |shell| {
        shell
            .browser_view()
            .tagged_file_hash(&text(&kick))
            .is_some()
    });
    let moved = library.join("Renamed.wav");
    std::fs::rename(&kick, &moved).unwrap();
    shell.browser_view().flush_database();
    let mut again = shell_on(&config);
    again.set_user_library(Some(library.clone()));
    wait_for_index(&mut again);
    wait_until(&mut again, "the collections to follow the file", |shell| {
        let state: bl_shell::browser_commands::BrowserState = run(
            shell,
            ids::OPEN_PLACE,
            json!({ "label": { "collection": 2 } }),
        );
        state.rows.iter().any(|row| row.name == "Renamed.wav")
    });
}

// Spec: browser-tags R7.
#[test]
fn beatlayers_groups_cant_be_renamed_or_deleted() {
    let scratch = Scratch::new("builtin");
    let config = scratch.path("config");
    let mut shell = shell_on(&config);
    let tags: TagsState = run(&mut shell, ids::TAGS, json!({}));
    assert_eq!(groups(&tags), ["Content", "Format", "Creator", "Devices"]);
    assert_eq!(
        group_tags(&tags, "Content"),
        [
            "Content|Sample",
            "Content|Audio Clip",
            "Content|MIDI Clip",
            "Content|Preset",
            "Content|Set",
            "Content|Backup Set",
            "Content|Device",
            "Content|Plug-In",
        ]
    );
    assert_eq!(
        group_tags(&tags, "Format"),
        ["Format|CLAP", "Format|VST3", "Format|AUv2"]
    );
    assert_eq!(
        group_tags(&tags, "Devices"),
        [
            "Devices|Instrument",
            "Devices|Audio Effect",
            "Devices|MIDI Effect"
        ]
    );
    // The groups and their tags can't be renamed or deleted.
    assert!(
        shell
            .run(
                ids::RENAME_TAG,
                json!({ "group": "Content", "name": "Stuff" })
            )
            .is_err(),
        "Beatlayer's own group can't be renamed"
    );
    assert!(
        shell
            .run(ids::DELETE_TAG, json!({ "group": "Format" }))
            .is_err(),
        "Beatlayer's own group can't be deleted"
    );
    assert!(
        shell
            .run(
                ids::RENAME_TAG,
                json!({ "tag": "Format|CLAP", "name": "Clap" })
            )
            .is_err(),
        "Beatlayer's own tag can't be renamed"
    );
    assert!(
        shell
            .run(ids::DELETE_TAG, json!({ "tag": "Content|Sample" }))
            .is_err(),
        "Beatlayer's own tag can't be deleted"
    );
}

// Spec: browser-tags R6 and R9.
#[test]
fn a_folders_tag_reaches_a_file_two_folders_down() {
    let scratch = Scratch::new("folder");
    let config = scratch.path("config");
    let library = scratch.path("Library");
    wav(&library.join("Drums").join("Deep").join("Kick.wav"), 100);
    let mut shell = shell_on(&config);
    shell.set_user_library(Some(library.clone()));
    wait_for_index(&mut shell);
    run::<TagsState>(&mut shell, ids::ADD_TAG_GROUP, json!({ "name": "Mine" }));
    run::<TagsState>(
        &mut shell,
        ids::ADD_TAG,
        json!({ "name": "Drums", "group": "Mine" }),
    );
    let folder = library.join("Drums");
    run::<TagsState>(
        &mut shell,
        ids::TAG_ITEMS,
        json!({
            "items": [text(&folder)],
            "tags": ["Mine|Drums"],
            "attach": true,
        }),
    );
    // The file two folders down carries the folder's tag.
    let file = library.join("Drums").join("Deep").join("Kick.wav");
    let tags: TagsState = run(&mut shell, ids::TAGS, json!({ "items": [text(&file)] }));
    let names: Vec<&str> = tags.items[0].tags.iter().map(|t| t.tag.as_str()).collect();
    assert!(names.contains(&"Mine|Drums"), "{names:?}");
    let sources: Vec<&str> = tags.items[0]
        .tags
        .iter()
        .map(|t| match t.source {
            bl_shell::browser_commands::TagSource::Own => "own",
            bl_shell::browser_commands::TagSource::Folder => "folder",
            bl_shell::browser_commands::TagSource::Beatlayer => "beatlayer",
        })
        .collect();
    assert!(sources.contains(&"folder"), "{sources:?}");
    // A folder's tag can't be taken off the file.
    assert!(
        shell
            .run(
                ids::TAG_ITEMS,
                json!({
                    "items": [text(&file)],
                    "tags": ["Mine|Drums"],
                    "attach": false,
                })
            )
            .is_err(),
        "a folder's tag can't be taken off"
    );
}

// Spec: browser-tags R15.
#[test]
fn a_hash_search_finds_tagged_items() {
    let scratch = Scratch::new("hash-search");
    let config = scratch.path("config");
    let library = scratch.path("Library");
    wav(&library.join("Kick.wav"), 100);
    wav(&library.join("Snare.wav"), 100);
    let mut shell = shell_on(&config);
    shell.set_user_library(Some(library.clone()));
    wait_for_index(&mut shell);
    run::<TagsState>(&mut shell, ids::ADD_TAG_GROUP, json!({ "name": "Mine" }));
    run::<TagsState>(
        &mut shell,
        ids::ADD_TAG,
        json!({ "name": "Favourite", "group": "Mine" }),
    );
    run::<TagsState>(
        &mut shell,
        ids::TAG_ITEMS,
        json!({
            "items": [text(&library.join("Kick.wav"))],
            "tags": ["Mine|Favourite"],
            "attach": true,
        }),
    );
    // The search finds the tagged file by its tag's name.
    let found: bl_shell::browser_commands::BrowserState = run(
        &mut shell,
        ids::SEARCH,
        json!({ "query": "#Favourite", "label": "all" }),
    );
    let names: Vec<&str> = found.rows.iter().map(|row| row.name.as_str()).collect();
    assert_eq!(names, ["Kick.wav"]);
    // And by the tag's full path.
    let found: bl_shell::browser_commands::BrowserState = run(
        &mut shell,
        ids::SEARCH,
        json!({ "query": "#Mine|Favourite", "label": "all" }),
    );
    assert_eq!(found.rows.len(), 1);
    // The tags searched come first among the next suggestions, most recent
    // first (spec: browser-tags R15).
    let suggestions = shell.browser_view().tag_suggestions("");
    assert_eq!(
        suggestions.first().map(String::as_str),
        Some("Mine|Favourite"),
        "{suggestions:?}"
    );
    // A `#` that names no tag is searched as text, and the Status Bar says
    // so.
    let found: bl_shell::browser_commands::BrowserState = run(
        &mut shell,
        ids::SEARCH,
        json!({ "query": "#nothing", "label": "all" }),
    );
    assert!(found.rows.is_empty(), "{:?}", found.rows);
    let message = shell.message().expect("the Status Bar says so");
    assert!(
        message.text.contains("No tag is called"),
        "{}",
        message.text
    );
}

// Spec: browser-tags R16.
#[test]
fn every_tag_command_runs_by_id_and_reads_back() {
    let scratch = Scratch::new("commands");
    let config = scratch.path("config");
    let library = scratch.path("Library");
    wav(&library.join("Kick.wav"), 100);
    let mut shell = shell_on(&config);
    shell.set_user_library(Some(library.clone()));
    wait_for_index(&mut shell);

    // The groups and tags are made, renamed, moved and deleted.
    let tags: TagsState = run(&mut shell, ids::ADD_TAG_GROUP, json!({ "name": "Mine" }));
    assert_eq!(
        groups(&tags),
        ["Content", "Format", "Creator", "Devices", "Mine"]
    );
    let tags: TagsState = run(
        &mut shell,
        ids::ADD_TAG,
        json!({ "name": "Favourite", "group": "Mine" }),
    );
    assert_eq!(group_tags(&tags, "Mine"), ["Mine|Favourite"]);
    // A subtag, one level deep.
    let tags: TagsState = run(
        &mut shell,
        ids::ADD_TAG,
        json!({ "name": "Deep", "parent": "Mine|Favourite" }),
    );
    assert_eq!(group_tags(&tags, "Mine"), ["Mine|Favourite"]);
    let subtag = tags
        .groups
        .iter()
        .find(|g| g.name == "Mine")
        .and_then(|g| g.tags.first())
        .map(|t| {
            t.subtags
                .iter()
                .map(|s| s.full_name.clone())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    assert_eq!(subtag, ["Mine|Favourite|Deep"]);
    let tags: TagsState = run(
        &mut shell,
        ids::RENAME_TAG,
        json!({ "tag": "Mine|Favourite", "name": "Faves" }),
    );
    assert_eq!(group_tags(&tags, "Mine"), ["Mine|Faves"]);
    let tags: TagsState = run(
        &mut shell,
        ids::MOVE_TAG,
        json!({ "tag": "Mine|Faves", "position": 0 }),
    );
    assert_eq!(group_tags(&tags, "Mine"), ["Mine|Faves"]);
    // The panels, which read back in the browser's state.
    run::<Value>(&mut shell, ids::SHOW_TAG_EDITOR, json!({ "show": true }));
    let panels: bl_shell::browser_commands::BrowserState = run(&mut shell, ids::STATE, json!({}));
    assert!(panels.tag_editor);
    run::<Value>(&mut shell, ids::SHOW_QUICK_TAGS, json!({ "show": false }));
    let panels: bl_shell::browser_commands::BrowserState = run(&mut shell, ids::STATE, json!({}));
    assert!(!panels.quick_tags);
    // The tags go on an item, and come off again.
    let tags: TagsState = run(
        &mut shell,
        ids::TAG_ITEMS,
        json!({
            "items": [text(&library.join("Kick.wav"))],
            "tags": ["Mine|Faves"],
            "attach": true,
        }),
    );
    assert_eq!(tags.items[0].tags.len(), 2, "{:?}", tags.items[0].tags);
    let tags: TagsState = run(
        &mut shell,
        ids::TAG_ITEMS,
        json!({
            "items": [text(&library.join("Kick.wav"))],
            "tags": ["Mine|Faves"],
            "attach": false,
        }),
    );
    assert!(
        tags.items[0].tags.iter().all(|t| t.tag != "Mine|Faves"),
        "{:?}",
        tags.items[0].tags
    );
    // The order goes back to its first order.
    let tags: TagsState = run(&mut shell, ids::RESET_TAG_ORDER, json!({}));
    assert_eq!(
        groups(&tags),
        ["Content", "Format", "Creator", "Devices", "Mine"]
    );
    // A tag is deleted.
    let tags: TagsState = run(&mut shell, ids::DELETE_TAG, json!({ "tag": "Mine|Faves" }));
    assert!(
        group_tags(&tags, "Mine").is_empty(),
        "{:?}",
        group_tags(&tags, "Mine")
    );
}

// Spec: browser-tags R15. Several `#` terms must all match.
#[test]
fn several_hash_terms_must_all_match() {
    let scratch = Scratch::new("hash-terms");
    let config = scratch.path("config");
    let library = scratch.path("Library");
    wav(&library.join("Kick.wav"), 100);
    wav(&library.join("Snare.wav"), 100);
    let mut shell = shell_on(&config);
    shell.set_user_library(Some(library.clone()));
    wait_for_index(&mut shell);
    run::<TagsState>(&mut shell, ids::ADD_TAG_GROUP, json!({ "name": "Mine" }));
    run::<TagsState>(
        &mut shell,
        ids::ADD_TAG,
        json!({ "name": "One", "group": "Mine" }),
    );
    run::<TagsState>(
        &mut shell,
        ids::ADD_TAG,
        json!({ "name": "Two", "group": "Mine" }),
    );
    // One file carries both tags, the other only one.
    run::<TagsState>(
        &mut shell,
        ids::TAG_ITEMS,
        json!({
            "items": [text(&library.join("Kick.wav"))],
            "tags": ["Mine|One", "Mine|Two"],
            "attach": true,
        }),
    );
    run::<TagsState>(
        &mut shell,
        ids::TAG_ITEMS,
        json!({
            "items": [text(&library.join("Snare.wav"))],
            "tags": ["Mine|One"],
            "attach": true,
        }),
    );
    // Both terms must match, so only the file carrying both is listed.
    let found: bl_shell::browser_commands::BrowserState = run(
        &mut shell,
        ids::SEARCH,
        json!({ "query": "#One #Two", "label": "all" }),
    );
    let names: Vec<&str> = found.rows.iter().map(|row| row.name.as_str()).collect();
    assert_eq!(names, ["Kick.wav"]);
}

// Spec: browser-tags R12 and R1. The order the person made is kept.
#[test]
fn the_order_of_the_groups_survives_a_restart() {
    let scratch = Scratch::new("order");
    let config = scratch.path("config");
    let mut shell = shell_on(&config);
    run::<TagsState>(&mut shell, ids::ADD_TAG_GROUP, json!({ "name": "Mine" }));
    run::<TagsState>(
        &mut shell,
        ids::ADD_TAG,
        json!({ "name": "Favourite", "group": "Mine" }),
    );
    // The person's group goes to the front of the list.
    let tags: TagsState = run(
        &mut shell,
        ids::MOVE_TAG,
        json!({ "group": "Mine", "position": 0 }),
    );
    assert_eq!(
        groups(&tags),
        ["Mine", "Content", "Format", "Creator", "Devices"]
    );
    shell.browser_view().flush_database();
    // A new shell keeps that order.
    let mut again = shell_on(&config);
    let tags: TagsState = run(&mut again, ids::TAGS, json!({}));
    assert_eq!(
        groups(&tags),
        ["Mine", "Content", "Format", "Creator", "Devices"],
        "the order is kept between launches"
    );
}

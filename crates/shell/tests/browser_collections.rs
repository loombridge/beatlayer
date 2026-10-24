//! The browser's collections and its item's context menu (WP-6.32b; spec:
//! browser-tags R34 to R40, browser R25), driven headless through
//! `Shell::run`, as agents drive them.

use std::path::{Path, PathBuf};
use std::time::Duration;

use bl_commands::Permission;
use bl_model::PluginFormat;
use bl_shell::browser_commands::{self as ids, BrowserState, Item, ItemKind, Label, TagsState};
use bl_shell::plugin_commands as plugin_ids;
use bl_shell::settings::Preferences;
use bl_shell::{Effect, Shell};
use serde_json::{Value, json};

/// A folder of its own under the system's temporary folder, removed when
/// dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("bl-collections-{}-{name}", std::process::id()));
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

/// Writes a 16-bit mono WAV file of `frames` frames at 48 kHz.
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

/// A shell on the null device with its configuration folder, loading the
/// preferences kept there.
fn shell_on(config: &Path) -> Shell {
    Shell::new(
        Preferences::load(config).unwrap_or_default(),
        Some(config.to_path_buf()),
        bl_shell::audio::AudioHost::null(Default::default()),
        Box::new(bl_shell::files::ProjectFiles::new()),
    )
}

fn run<T: serde::de::DeserializeOwned>(shell: &mut Shell, id: &str, args: Value) -> T {
    serde_json::from_value(
        shell
            .run(id, args)
            .unwrap_or_else(|e| panic!("{id} failed: {e}")),
    )
    .unwrap()
}

fn state(shell: &mut Shell) -> BrowserState {
    serde_json::from_value(shell.run(ids::STATE, json!({ "limit": 1000 })).unwrap()).unwrap()
}

/// The names of the rows, in order.
fn names(state: &BrowserState) -> Vec<String> {
    state.rows.iter().map(|row| row.name.clone()).collect()
}

/// Chooses a collection and returns what it lists.
fn open_collection(shell: &mut Shell, number: u8) -> BrowserState {
    run(
        shell,
        ids::OPEN_PLACE,
        json!({ "label": { "collection": number } }),
    )
}

/// The collections a row is in, by the row's name, in the label chosen.
fn collections_of(state: &BrowserState, name: &str) -> Vec<u8> {
    state
        .rows
        .iter()
        .find(|row| row.name == name)
        .map(|row| row.collections.clone())
        .unwrap_or_default()
}

fn select(shell: &mut Shell, paths: &[String]) {
    shell
        .run(ids::SELECT, json!({ "paths": paths }))
        .unwrap_or_else(|e| panic!("select failed: {e}"));
}

/// A library with a `Drums` folder holding two kicks, and two loose files.
fn library(scratch: &Scratch) -> (Shell, PathBuf, PathBuf) {
    let config = scratch.path("config");
    let lib = scratch.path("Library");
    wav(&lib.join("Drums/Kick 10.wav"), 100);
    wav(&lib.join("Drums/Kick 2.wav"), 100);
    wav(&lib.join("Pad.wav"), 2_000);
    wav(&lib.join("Snare.wav"), 400);
    let mut shell = shell_on(&config);
    shell.set_user_library(Some(lib.clone()));
    assert!(shell.wait_for_browser_index(Duration::from_secs(20)));
    (shell, config, lib)
}

// Spec: browser-tags R34, browser R2. The sidebar lists the seven
// collections first, in order, each usable, and their names are Live's.
#[test]
fn the_sidebar_lists_the_seven_collections_in_order() {
    let scratch = Scratch::new("sidebar");
    let (mut shell, _, _) = library(&scratch);
    let labels = state(&mut shell).labels;
    let collections: Vec<_> = labels
        .iter()
        .filter(|l| l.section == ids::Section::Collections)
        .collect();
    assert_eq!(
        collections
            .iter()
            .map(|l| l.name.as_str())
            .collect::<Vec<_>>(),
        [
            "Favorites",
            "Orange",
            "Yellow",
            "Green",
            "Blue",
            "Purple",
            "Gray"
        ]
    );
    assert!(collections.iter().all(|l| l.available && l.shown));
    assert_eq!(
        labels
            .iter()
            .take(7)
            .map(|l| l.label.clone())
            .collect::<Vec<_>>(),
        (1..=7).map(Label::Collection).collect::<Vec<_>>(),
        "they come before the Library labels"
    );
    // Choosing an empty one says what to do.
    let empty = open_collection(&mut shell, 3);
    assert_eq!(empty.label, Label::Collection(3));
    assert!(empty.rows.is_empty());
    assert!(
        empty.note.as_deref().is_some_and(|n| n.contains("Yellow")),
        "{:?}",
        empty.note
    );
}

// Spec: browser-tags R35. 3 puts three selected items of different kinds in
// Yellow, 3 again takes them out, 0 clears every collection.
#[test]
fn three_items_of_different_kinds_go_in_yellow_and_come_out_again() {
    let scratch = Scratch::new("toggle");
    let (mut shell, _, lib) = library(&scratch);
    // A file, a folder and one of Beatlayer's own devices.
    let items = [
        text(&lib.join("Pad.wav")),
        text(&lib.join("Drums")),
        "rack:audio_effect".to_owned(),
    ];
    select(&mut shell, &items);
    let put: BrowserState = run(&mut shell, ids::SET_COLLECTION, json!({ "collection": 3 }));
    assert_eq!(put.selected.len(), 3);
    let yellow = open_collection(&mut shell, 3);
    assert_eq!(
        names(&yellow),
        ["Drums", "Pad.wav", "Audio Effect Rack"],
        "{yellow:?}"
    );
    assert_eq!(
        yellow.rows.iter().map(|r| r.kind).collect::<Vec<_>>(),
        [ItemKind::Folder, ItemKind::Audio, ItemKind::Device]
    );
    assert!(yellow.rows.iter().all(|r| r.collections == [3]));

    // Pressing the key again, with all three still selected, takes them out.
    let out: BrowserState = run(&mut shell, ids::SET_COLLECTION, json!({ "collection": 3 }));
    assert!(out.rows.is_empty(), "{out:?}");
    assert!(open_collection(&mut shell, 3).rows.is_empty());

    // An item can be in several collections; a partly collected selection
    // all goes in. 0 takes the selection out of every one.
    select(&mut shell, &[text(&lib.join("Pad.wav"))]);
    run::<BrowserState>(&mut shell, ids::SET_COLLECTION, json!({ "collection": 1 }));
    run::<BrowserState>(&mut shell, ids::SET_COLLECTION, json!({ "collection": 6 }));
    select(&mut shell, &items);
    run::<BrowserState>(&mut shell, ids::SET_COLLECTION, json!({ "collection": 1 }));
    let favourites = open_collection(&mut shell, 1);
    assert_eq!(
        names(&favourites),
        ["Drums", "Pad.wav", "Audio Effect Rack"]
    );
    assert_eq!(
        collections_of(&favourites, "Pad.wav"),
        [1, 6],
        "in two at once"
    );
    let purple = open_collection(&mut shell, 6);
    assert_eq!(names(&purple), ["Pad.wav"]);

    select(
        &mut shell,
        &[text(&lib.join("Pad.wav")), text(&lib.join("Drums"))],
    );
    run::<BrowserState>(&mut shell, ids::CLEAR_COLLECTIONS, json!({}));
    assert!(open_collection(&mut shell, 1).rows.len() == 1);
    assert!(open_collection(&mut shell, 6).rows.is_empty());
    assert_eq!(
        names(&open_collection(&mut shell, 1)),
        ["Audio Effect Rack"]
    );
}

// Spec: browser-tags R35, R40. The commands put items in and take them out
// by name, with `member` choosing the direction, and refuse what doesn't fit.
#[test]
fn set_collection_names_its_items_and_refuses_what_does_not_fit() {
    let scratch = Scratch::new("args");
    let (mut shell, _, lib) = library(&scratch);
    let pad = text(&lib.join("Pad.wav"));
    run::<BrowserState>(
        &mut shell,
        ids::SET_COLLECTION,
        json!({ "collection": 2, "items": [pad], "member": true }),
    );
    run::<BrowserState>(
        &mut shell,
        ids::SET_COLLECTION,
        json!({ "collection": 2, "items": [pad], "member": true }),
    );
    assert_eq!(names(&open_collection(&mut shell, 2)), ["Pad.wav"], "once");
    run::<BrowserState>(
        &mut shell,
        ids::SET_COLLECTION,
        json!({ "collection": 2, "items": [pad], "member": false }),
    );
    assert!(open_collection(&mut shell, 2).rows.is_empty());
    for collection in [0, 8] {
        let error = shell
            .run(
                ids::SET_COLLECTION,
                json!({ "collection": collection, "items": [pad] }),
            )
            .unwrap_err()
            .to_string();
        assert!(error.contains("isn't a collection"), "{error}");
    }
    let error = shell
        .run(ids::OPEN_PLACE, json!({ "label": { "collection": 9 } }))
        .unwrap_err()
        .to_string();
    assert!(error.contains("isn't a collection"), "{error}");
    // With nothing selected there is nothing to act on.
    select(&mut shell, &[]);
    assert!(!shell.can_invoke(ids::SET_COLLECTION, &json!({ "collection": 2 })));
    assert!(!shell.can_invoke(ids::CLEAR_COLLECTIONS, &json!({})));
    assert!(shell.can_invoke(
        ids::SET_COLLECTION,
        &json!({ "collection": 2, "items": [pad] })
    ));
    assert!(!shell.can_invoke(ids::REVEAL, &json!({})));
}

// Spec: browser-tags R36, browser R6. A collection lists its items in the
// browser's order, folders first and numbers in names in numeric order;
// search, filters and opening a folder work there.
#[test]
fn a_collection_lists_in_the_browsers_order_and_searches_filters_and_opens() {
    let scratch = Scratch::new("listing");
    let (mut shell, _, lib) = library(&scratch);
    let item = |name: &str| text(&lib.join(name));
    run::<BrowserState>(
        &mut shell,
        ids::SET_COLLECTION,
        json!({
            "collection": 4,
            "items": [item("Snare.wav"), item("Drums/Kick 10.wav"), item("Drums"),
                      item("Drums/Kick 2.wav"), item("Pad.wav")],
            "member": true,
        }),
    );
    let green = open_collection(&mut shell, 4);
    assert_eq!(
        names(&green),
        ["Drums", "Kick 2.wav", "Kick 10.wav", "Pad.wav", "Snare.wav"],
        "{green:?}"
    );
    assert_eq!(green.rows[0].kind, ItemKind::Folder);
    assert!(!green.rows[0].open && green.rows[0].depth == 0);

    // The folder opens, and what is inside it is listed beneath it.
    let drums = item("Drums");
    run::<BrowserState>(&mut shell, ids::OPEN_FOLDER, json!({ "path": drums }));
    let opened = state(&mut shell);
    assert_eq!(
        names(&opened),
        [
            "Drums",
            "Kick 2.wav",
            "Kick 10.wav",
            "Kick 2.wav",
            "Kick 10.wav",
            "Pad.wav",
            "Snare.wav"
        ],
        "{opened:?}"
    );
    assert!(opened.rows[0].open);
    assert_eq!(opened.rows[1].depth, 1);
    run::<BrowserState>(
        &mut shell,
        ids::OPEN_FOLDER,
        json!({ "path": drums, "open": false }),
    );

    // Search matches names inside the collection only.
    let found: BrowserState = run(&mut shell, ids::SEARCH, json!({ "query": "kick 1" }));
    assert_eq!(names(&found), ["Kick 10.wav"], "{found:?}");
    let none: BrowserState = run(&mut shell, ids::SEARCH, json!({ "query": "zzz" }));
    assert!(none.rows.is_empty() && none.note.as_deref().is_some_and(|n| n.contains("zzz")));
    run::<BrowserState>(&mut shell, ids::SEARCH, json!({ "query": "" }));

    // The Filter View's tags narrow a collection too.
    run::<TagsState>(&mut shell, ids::ADD_TAG_GROUP, json!({ "name": "Mine" }));
    run::<TagsState>(
        &mut shell,
        ids::ADD_TAG,
        json!({ "name": "Loud", "group": "Mine" }),
    );
    run::<TagsState>(
        &mut shell,
        ids::TAG_ITEMS,
        json!({ "items": [item("Snare.wav")], "tags": ["Mine|Loud"] }),
    );
    let filtered: BrowserState = run(&mut shell, ids::FILTER, json!({ "tags": ["Mine|Loud"] }));
    assert_eq!(names(&filtered), ["Snare.wav"], "{filtered:?}");
    run::<BrowserState>(&mut shell, ids::CLEAR_FILTERS, json!({}));

    // Items that are gone from disk aren't listed, and are still collected.
    std::fs::remove_file(lib.join("Pad.wav")).unwrap();
    run::<BrowserState>(&mut shell, ids::SEARCH, json!({ "query": "" }));
    assert!(!names(&open_collection(&mut shell, 4)).contains(&"Pad.wav".to_owned()));
}

// Spec: browser-tags R36, plugin-scanning R11. A plug-in in a collection
// loads from there as it does from the Plug-Ins label.
#[test]
fn a_plug_in_in_a_collection_loads_onto_a_track() {
    if bl_plugin_host::scan::scan_program().is_none() {
        eprintln!("beatlayer-scan wasn't built; skipping (build the workspace's bins)");
        return;
    }
    let mut shell = Shell::headless();
    let bundle = bl_test_clap::bundle().to_string_lossy().into_owned();
    shell
        .run(plugin_ids::SCAN, json!({ "paths": [bundle.clone()] }))
        .unwrap();
    let row = format!("{bundle}#{}", bl_test_clap::PLUGIN_ID);
    select(&mut shell, std::slice::from_ref(&row));
    run::<BrowserState>(&mut shell, ids::SET_COLLECTION, json!({ "collection": 5 }));
    let blue = open_collection(&mut shell, 5);
    assert_eq!(blue.rows.len(), 1, "{blue:?}");
    let plugin: &Item = &blue.rows[0];
    assert_eq!(plugin.kind, ItemKind::Plugin);
    assert_eq!(plugin.format, Some(PluginFormat::Clap));
    assert_eq!(plugin.path, row);
    assert_eq!(plugin.collections, [5]);

    select(&mut shell, std::slice::from_ref(&plugin.path));
    let track = shell
        .editor()
        .document()
        .tracks()
        .find(|t| t.kind() == bl_model::TrackKind::Midi)
        .map(bl_model::Track::id)
        .expect("the default set has a MIDI track");
    shell
        .run(
            bl_shell::arrangement_commands::SELECT_TRACKS,
            json!({ "tracks": [track] }),
        )
        .unwrap();
    let loaded: plugin_ids::PluginLoaded =
        serde_json::from_value(shell.run(ids::LOAD_SELECTED, json!({})).unwrap()).unwrap();
    assert_eq!(loaded.plugin, bl_test_clap::PLUGIN_ID);
    assert_eq!(loaded.track, track);
}

// Spec: browser-tags R5, R35, R38, R39, browser-tags R1. Collections, a
// collection's name, and which are hidden are kept between launches, and a
// hidden collection comes back into the sidebar when an item goes in.
#[test]
fn collections_names_and_hiding_survive_a_restart() {
    let scratch = Scratch::new("restart");
    let (mut shell, config, lib) = library(&scratch);
    let pad = text(&lib.join("Pad.wav"));
    // Hiding is kept, and a rename.
    run::<BrowserState>(
        &mut shell,
        ids::SET_LABEL_SHOWN,
        json!({ "label": { "collection": 5 }, "shown": false }),
    );
    run::<BrowserState>(
        &mut shell,
        ids::SET_LABEL_SHOWN,
        json!({ "label": { "collection": 6 }, "shown": false }),
    );
    run::<BrowserState>(
        &mut shell,
        ids::RENAME_LABEL,
        json!({ "label": { "collection": 3 }, "name": "Keepers" }),
    );
    // An item goes into a hidden collection: it comes back; the other stays
    // hidden.
    let after: BrowserState = run(
        &mut shell,
        ids::SET_COLLECTION,
        json!({ "collection": 5, "items": [pad], "member": true }),
    );
    let shown = |state: &BrowserState, number: u8| {
        state
            .labels
            .iter()
            .find(|l| l.label == Label::Collection(number))
            .map(|l| l.shown)
    };
    assert_eq!(shown(&after, 5), Some(true), "it comes back");
    assert_eq!(shown(&after, 6), Some(false), "the other stays hidden");
    // A shown one stays when it empties.
    run::<BrowserState>(
        &mut shell,
        ids::SET_COLLECTION,
        json!({ "collection": 5, "items": [pad], "member": false }),
    );
    assert_eq!(shown(&state(&mut shell), 5), Some(true));
    run::<BrowserState>(
        &mut shell,
        ids::SET_COLLECTION,
        json!({ "collection": 2, "items": [pad, text(&lib.join("Drums"))], "member": true }),
    );
    shell.browser_view().flush_database();

    let mut again = shell_on(&config);
    again.set_user_library(Some(lib.clone()));
    assert!(again.wait_for_browser_index(Duration::from_secs(20)));
    let restarted = state(&mut again);
    let label = |number: u8| {
        restarted
            .labels
            .iter()
            .find(|l| l.label == Label::Collection(number))
            .unwrap()
    };
    assert_eq!(label(3).name, "Keepers");
    assert!(label(5).shown && !label(6).shown);
    let orange = open_collection(&mut again, 2);
    assert_eq!(names(&orange), ["Drums", "Pad.wav"], "{orange:?}");
    assert!(open_collection(&mut again, 5).rows.is_empty());
    // Giving a collection its own name back is an empty name.
    let renamed: BrowserState = run(
        &mut again,
        ids::RENAME_LABEL,
        json!({ "label": { "collection": 3 }, "name": "" }),
    );
    assert_eq!(
        renamed
            .labels
            .iter()
            .find(|l| l.label == Label::Collection(3))
            .unwrap()
            .name,
        "Yellow"
    );
}

// Spec: browser-tags R38, R39. Rename opens a collection's name for
// editing; hiding the chosen collection chooses All; a collection has a
// colour, not an icon, and keeps its place.
#[test]
fn a_collection_is_renamed_hidden_and_keeps_its_place() {
    let scratch = Scratch::new("labels");
    let (mut shell, _, _) = library(&scratch);
    open_collection(&mut shell, 4);
    assert!(
        shell.can_invoke_in(
            ids::RENAME_LABEL,
            &json!({}),
            &[bl_commands::Context::BrowserSidebar]
        ),
        "Cmd-R on a collection"
    );
    let editing: BrowserState = run(&mut shell, ids::RENAME_LABEL, json!({}));
    assert_eq!(editing.renaming_label, Some(Label::Collection(4)));
    let done: BrowserState = run(&mut shell, ids::RENAME_LABEL, json!({ "name": "Greens" }));
    assert!(done.renaming_label.is_none());
    assert_eq!(done.labels[3].name, "Greens");
    // Hiding the chosen one chooses All.
    let hidden: BrowserState = run(
        &mut shell,
        ids::SET_LABEL_SHOWN,
        json!({ "label": { "collection": 4 }, "shown": false }),
    );
    assert_eq!(hidden.label, Label::All);
    assert!(!hidden.labels[3].shown);
    for (id, args) in [
        (
            ids::SET_LABEL_ICON,
            json!({ "label": { "collection": 4 }, "icon": "star" }),
        ),
        (
            ids::MOVE_LABEL,
            json!({ "label": { "collection": 4 }, "position": 2 }),
        ),
        (ids::REMOVE_LABEL, json!({ "label": { "collection": 4 } })),
    ] {
        assert!(shell.run(id, args).is_err(), "{id}");
    }
}

// Spec: browser R25, sample-editor R2. Show in Places shows a collected file
// in the place it lies in, opened down to it, and does the same for a folder.
#[test]
fn show_in_places_shows_a_collected_file_in_its_place() {
    let scratch = Scratch::new("places");
    let (mut shell, _, lib) = library(&scratch);
    let kick = text(&lib.join("Drums").join("Kick 2.wav"));
    run::<BrowserState>(
        &mut shell,
        ids::SET_COLLECTION,
        json!({ "collection": 7, "items": [kick, text(&lib.join("Drums"))], "member": true }),
    );
    let gray = open_collection(&mut shell, 7);
    assert_eq!(gray.label, Label::Collection(7));
    let shown: BrowserState = run(&mut shell, ids::SHOW_FILE, json!({ "path": kick }));
    assert_eq!(shown.label, Label::UserLibrary);
    assert_eq!(shown.selected, std::slice::from_ref(&kick));
    let row = shown
        .rows
        .iter()
        .find(|r| r.path == kick)
        .expect("the file");
    assert_eq!(row.depth, 1, "inside its opened folder");
    assert_eq!(row.collections, [7], "still in Gray");
    // A folder is shown in its place too.
    let folder = text(&lib.join("Drums"));
    let shown: BrowserState = run(&mut shell, ids::SHOW_FILE, json!({ "path": folder }));
    assert_eq!(shown.label, Label::UserLibrary);
    assert_eq!(shown.selected, [folder]);
}

// Spec: browser R25. Show in Finder asks the host to show the files and
// folders in the file manager, and refuses what isn't there.
#[test]
fn reveal_asks_the_host_to_show_the_selection_in_the_file_manager() {
    let scratch = Scratch::new("reveal");
    let (mut shell, _, lib) = library(&scratch);
    let pad = lib.join("Pad.wav");
    let drums = lib.join("Drums");
    shell.take_effects();
    select(&mut shell, &[text(&pad), text(&drums)]);
    run::<BrowserState>(&mut shell, ids::REVEAL, json!({}));
    assert_eq!(
        shell.take_effects(),
        [Effect::ShowInFileManager(vec![pad.clone(), drums])]
    );
    run::<BrowserState>(&mut shell, ids::REVEAL, json!({ "paths": [text(&pad)] }));
    assert_eq!(shell.take_effects(), [Effect::ShowInFileManager(vec![pad])]);
    let error = shell
        .run(
            ids::REVEAL,
            json!({ "paths": [text(&lib.join("Gone.wav"))] }),
        )
        .unwrap_err()
        .to_string();
    assert!(error.contains("isn't a file or folder"), "{error}");
    assert!(shell.take_effects().is_empty());
}

// Spec: browser-tags R40. The new commands are registered with their
// permission classes.
#[test]
fn the_new_commands_are_registered_with_their_classes() {
    let shell = Shell::headless();
    let registry = shell.registry();
    let class = |id: &str| registry.get(id).unwrap().permission.unwrap();
    assert_eq!(class(ids::SET_COLLECTION), Permission::Edit);
    assert_eq!(class(ids::CLEAR_COLLECTIONS), Permission::Edit);
    assert_eq!(class(ids::REVEAL), Permission::External);
}

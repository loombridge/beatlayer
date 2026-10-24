//! The browser's Filter View, saved labels, columns and history (WP-6.32;
//! spec: browser-tags R18 to R33, browser R26), driven headless through
//! `Shell::run`, as agents drive them.

use std::path::{Path, PathBuf};
use std::time::Duration;

use bl_commands::Permission;
use bl_model::TrackId;
use bl_shell::Shell;
use bl_shell::browser_commands::{
    self as ids, BrowserState, Column, FilterViewState, Label, TagsState,
};
use bl_shell::settings::Preferences;
use serde_json::{Value, json};

/// A folder of its own under the system's temporary folder, removed when
/// dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("bl-filters-{}-{name}", std::process::id()));
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

/// A library of three samples, with its configuration folder, indexed.
fn library(scratch: &Scratch) -> (Shell, PathBuf, PathBuf) {
    let config = scratch.path("config");
    let lib = scratch.path("Library");
    wav(&lib.join("Kick.wav"), 100);
    wav(&lib.join("Snare.wav"), 2_000);
    wav(&lib.join("Pad.wav"), 40_000);
    let mut shell = shell_on(&config);
    shell.set_user_library(Some(lib.clone()));
    assert!(shell.wait_for_browser_index(Duration::from_secs(20)));
    (shell, config, lib)
}

/// The tags Mine|A, Mine|B and Feel|Hard, on Kick (A, Hard), Snare (B,
/// Hard) and Pad (A).
fn tag_the_library(shell: &mut Shell, lib: &Path) {
    run::<TagsState>(shell, ids::ADD_TAG_GROUP, json!({ "name": "Mine" }));
    run::<TagsState>(shell, ids::ADD_TAG_GROUP, json!({ "name": "Feel" }));
    for (name, group) in [("A", "Mine"), ("B", "Mine"), ("Hard", "Feel")] {
        run::<TagsState>(shell, ids::ADD_TAG, json!({ "name": name, "group": group }));
    }
    let item = |name: &str| text(&lib.join(name));
    for (items, tag) in [
        (vec![item("Kick.wav"), item("Pad.wav")], "Mine|A"),
        (vec![item("Snare.wav")], "Mine|B"),
        (vec![item("Kick.wav"), item("Snare.wav")], "Feel|Hard"),
    ] {
        run::<TagsState>(
            shell,
            ids::TAG_ITEMS,
            json!({ "items": items, "tags": [tag] }),
        );
    }
}

fn filter(shell: &mut Shell, tags: &[&str], add: bool) -> BrowserState {
    run(shell, ids::FILTER, json!({ "tags": tags, "add": add }))
}

// Spec: browser-tags R20.
#[test]
fn tags_in_one_group_widen_the_list_and_a_tag_in_another_group_narrows_it() {
    let scratch = Scratch::new("widen");
    let (mut shell, _, lib) = library(&scratch);
    tag_the_library(&mut shell, &lib);

    let one = filter(&mut shell, &["Mine|A"], false);
    assert_eq!(names(&one), ["Kick.wav", "Pad.wav"]);
    // Cmd-click in the same group widens.
    let both = filter(&mut shell, &["Mine|B"], true);
    assert_eq!(names(&both), ["Kick.wav", "Pad.wav", "Snare.wav"]);
    // A tag in another group narrows.
    let narrowed = filter(&mut shell, &["Feel|Hard"], false);
    assert_eq!(names(&narrowed), ["Kick.wav", "Snare.wav"]);
    assert_eq!(narrowed.filter_view.selected.len(), 3);
    // Clicking the only selected tag of a group clears it.
    let cleared = filter(&mut shell, &["Feel|Hard"], false);
    assert_eq!(names(&cleared), ["Kick.wav", "Pad.wav", "Snare.wav"]);
    // Cmd-click on a selected tag takes it out.
    let taken_out = filter(&mut shell, &["Mine|B"], true);
    assert_eq!(names(&taken_out), ["Kick.wav", "Pad.wav"]);
    // Search words narrow it too.
    let searched: BrowserState = run(&mut shell, ids::SEARCH, json!({ "query": "pad" }));
    assert_eq!(names(&searched), ["Pad.wav"]);
}

// Spec: browser-tags R19, R20.
#[test]
fn the_filter_view_lists_the_groups_a_label_shows_and_their_tags_in_use() {
    let scratch = Scratch::new("groups");
    let (mut shell, _, lib) = library(&scratch);
    tag_the_library(&mut shell, &lib);

    let all = state(&mut shell).filter_view;
    assert!(all.shown, "the Filter View shows in a new install");
    let group = |view: &FilterViewState, name: &str| {
        view.groups.iter().find(|g| g.name == name).map(|g| {
            g.tags
                .iter()
                .map(|t| t.full_name.clone())
                .collect::<Vec<_>>()
        })
    };
    assert_eq!(
        group(&all, "Mine"),
        Some(vec!["Mine|A".into(), "Mine|B".into()])
    );
    assert_eq!(group(&all, "Feel"), Some(vec!["Feel|Hard".into()]));
    assert_eq!(group(&all, "Content"), Some(vec!["Content|Sample".into()]));
    assert_eq!(group(&all, "Format"), None, "no item carries a Format tag");
    assert!(
        all.menu.iter().any(|g| g.name == "Format" && g.shown),
        "but All can show it"
    );

    // Under Samples only Content and the person's own groups show.
    let samples: BrowserState = run(&mut shell, ids::OPEN_PLACE, json!({ "label": "samples" }));
    let menu: Vec<(&str, bool)> = samples
        .filter_view
        .menu
        .iter()
        .map(|g| (g.name.as_str(), g.shown))
        .collect();
    assert!(menu.contains(&("Content", true)));
    assert!(menu.contains(&("Mine", true)));
    assert!(menu.contains(&("Format", false)));
    assert!(menu.contains(&("Devices", false)));
}

// Spec: browser-tags R20.
#[test]
fn selected_tags_survive_a_change_of_library_label() {
    let scratch = Scratch::new("kept");
    let (mut shell, _, lib) = library(&scratch);
    tag_the_library(&mut shell, &lib);
    // A Format tag on Kick, so a group Samples doesn't show is selected.
    run::<TagsState>(
        &mut shell,
        ids::TAG_ITEMS,
        json!({ "items": [text(&lib.join("Kick.wav"))], "tags": ["Format|CLAP"] }),
    );
    let under_all = filter(&mut shell, &["Mine|A"], false);
    assert_eq!(names(&under_all), ["Kick.wav", "Pad.wav"]);
    let samples: BrowserState = run(&mut shell, ids::OPEN_PLACE, json!({ "label": "samples" }));
    assert_eq!(names(&samples), ["Kick.wav", "Pad.wav"]);
    assert_eq!(samples.filter_view.selected, ["Mine|A"]);
    let place: BrowserState = run(
        &mut shell,
        ids::OPEN_PLACE,
        json!({ "label": "user_library" }),
    );
    assert_eq!(names(&place), ["Kick.wav", "Pad.wav"], "under a place too");

    // A selected tag in a group the label doesn't show isn't applied there.
    let with_format = filter(&mut shell, &["Format|CLAP"], true);
    assert_eq!(names(&with_format), ["Kick.wav", "Pad.wav"]);
    let all: BrowserState = run(&mut shell, ids::OPEN_PLACE, json!({ "label": "all" }));
    assert_eq!(names(&all), ["Kick.wav"], "Format shows under All");
    assert_eq!(all.filter_view.selected.len(), 2);
}

// Spec: browser-tags R21.
#[test]
fn the_groups_a_label_shows_are_chosen_and_kept_between_launches() {
    let scratch = Scratch::new("menu");
    let (mut shell, config, _) = library(&scratch);
    run::<TagsState>(&mut shell, ids::ADD_TAG_GROUP, json!({ "name": "Mine" }));
    let _: BrowserState = run(&mut shell, ids::OPEN_PLACE, json!({ "label": "samples" }));
    let shown = |shell: &mut Shell, name: &str| {
        state(shell)
            .filter_view
            .menu
            .iter()
            .find(|g| g.name == name)
            .map(|g| g.shown)
    };
    assert_eq!(shown(&mut shell, "Format"), Some(false));
    run::<FilterViewState>(
        &mut shell,
        ids::SET_FILTER_GROUPS,
        json!({ "group": "Format" }),
    );
    run::<FilterViewState>(
        &mut shell,
        ids::SET_FILTER_GROUPS,
        json!({ "group": "Content", "shown": false }),
    );
    assert_eq!(shown(&mut shell, "Format"), Some(true));
    assert_eq!(shown(&mut shell, "Content"), Some(false));
    // Not under another label.
    let _: BrowserState = run(&mut shell, ids::OPEN_PLACE, json!({ "label": "clips" }));
    assert_eq!(shown(&mut shell, "Format"), Some(false));

    let mut relaunched = shell_on(&config);
    let _: BrowserState = run(
        &mut relaunched,
        ids::OPEN_PLACE,
        json!({ "label": "samples" }),
    );
    assert_eq!(shown(&mut relaunched, "Format"), Some(true));
    assert_eq!(shown(&mut relaunched, "Content"), Some(false));
    // Reset puts them back, and Show All shows every group.
    run::<FilterViewState>(
        &mut relaunched,
        ids::SET_FILTER_GROUPS,
        json!({ "reset": true }),
    );
    assert_eq!(shown(&mut relaunched, "Format"), Some(false));
    assert_eq!(shown(&mut relaunched, "Content"), Some(true));
    run::<FilterViewState>(
        &mut relaunched,
        ids::SET_FILTER_GROUPS,
        json!({ "show_all": true }),
    );
    assert_eq!(shown(&mut relaunched, "Devices"), Some(true));
    // The command wants one thing at a time.
    assert!(
        relaunched
            .run(
                ids::SET_FILTER_GROUPS,
                json!({ "reset": true, "show_all": true })
            )
            .is_err()
    );
}

// Spec: browser-tags R18.
#[test]
fn the_filter_view_is_shown_hidden_and_kept_between_launches() {
    let scratch = Scratch::new("toggle");
    let (mut shell, config, _) = library(&scratch);
    assert!(state(&mut shell).filter_view.shown);
    let hidden: FilterViewState = run(&mut shell, ids::SET_FILTER_VIEW, json!({}));
    assert!(!hidden.shown, "left out, it toggles");
    let mut relaunched = shell_on(&config);
    assert!(!state(&mut relaunched).filter_view.shown);
    let shown: FilterViewState = run(
        &mut relaunched,
        ids::SET_FILTER_VIEW,
        json!({ "show": true }),
    );
    assert!(shown.shown);
}

// Spec: browser-tags R22.
#[test]
fn clear_and_cmd_f_clear_what_the_results_bar_says() {
    let scratch = Scratch::new("clear");
    let (mut shell, _, lib) = library(&scratch);
    tag_the_library(&mut shell, &lib);
    filter(&mut shell, &["Mine|A"], false);
    let searched: BrowserState = run(&mut shell, ids::SEARCH, json!({ "query": "pad" }));
    assert_eq!(names(&searched), ["Pad.wav"]);
    assert!(shell.browser_view().results_bar());

    // The search field's own clear button clears only the words.
    let words: BrowserState = run(&mut shell, ids::SEARCH, json!({ "query": "" }));
    assert_eq!(words.filter_view.selected, ["Mine|A"]);
    assert_eq!(names(&words), ["Kick.wav", "Pad.wav"]);
    // Esc and Clear clear the words and the tags.
    run::<BrowserState>(&mut shell, ids::SEARCH, json!({ "query": "pad" }));
    let cleared: BrowserState = run(&mut shell, ids::CLEAR_FILTERS, json!({}));
    assert_eq!(cleared.search, "");
    assert!(cleared.filter_view.selected.is_empty());
    assert!(!shell.browser_view().results_bar());
    // Either can be kept.
    filter(&mut shell, &["Mine|A"], false);
    run::<BrowserState>(&mut shell, ids::SEARCH, json!({ "query": "pad" }));
    let words_only: BrowserState = run(&mut shell, ids::CLEAR_FILTERS, json!({ "tags": false }));
    assert_eq!(words_only.search, "");
    assert_eq!(words_only.filter_view.selected, ["Mine|A"]);
    // Cmd-F chooses All and clears the selected tags, leaving the words.
    run::<BrowserState>(&mut shell, ids::SEARCH, json!({ "query": "pad" }));
    shell.run(ids::FOCUS_SEARCH, json!({})).unwrap();
    let after = state(&mut shell);
    assert_eq!(after.label, Label::All);
    assert!(after.filter_view.selected.is_empty());
    assert_eq!(
        after.search, "pad",
        "the field's text is selected, not cleared"
    );
}

// Spec: browser-tags R24, R25.
#[test]
fn a_saved_label_lists_a_file_tagged_after_it_was_made_and_survives_a_restart() {
    let scratch = Scratch::new("saved");
    let (mut shell, config, lib) = library(&scratch);
    tag_the_library(&mut shell, &lib);
    // Nothing to keep yet.
    assert!(shell.run(ids::ADD_LABEL, json!({})).is_err());
    filter(&mut shell, &["Mine|A"], false);
    let added: BrowserState = run(&mut shell, ids::ADD_LABEL, json!({}));
    let label = Label::Saved(1);
    assert_eq!(added.label, label);
    assert_eq!(added.renaming_label, Some(label.clone()));
    let saved = &added.saved_labels[0];
    assert_eq!(saved.name, "A", "named after its words and tags");
    assert_eq!(saved.base, Label::All);
    assert_eq!(saved.tags, ["Mine|A"]);
    // It goes at the end of the Library section.
    let library_labels: Vec<_> = added
        .labels
        .iter()
        .filter(|l| l.section == ids::Section::Library)
        .collect();
    assert_eq!(library_labels.last().unwrap().label, label);
    // Rename finishes the edit.
    let renamed: BrowserState = run(
        &mut shell,
        ids::RENAME_LABEL,
        json!({ "label": label, "name": "Aye" }),
    );
    assert_eq!(renamed.renaming_label, None);
    assert_eq!(renamed.saved_labels[0].name, "Aye");

    // Choosing it lists what its tags find now.
    run::<BrowserState>(&mut shell, ids::CLEAR_FILTERS, json!({}));
    let chosen: BrowserState = run(&mut shell, ids::OPEN_PLACE, json!({ "label": label }));
    assert_eq!(names(&chosen), ["Kick.wav", "Pad.wav"]);
    assert_eq!(chosen.filter_view.selected, ["Mine|A"]);
    // A file tagged after the label was made shows up in it.
    run::<TagsState>(
        &mut shell,
        ids::TAG_ITEMS,
        json!({ "items": [text(&lib.join("Snare.wav"))], "tags": ["Mine|A"] }),
    );
    run::<BrowserState>(&mut shell, ids::OPEN_PLACE, json!({ "label": "all" }));
    run::<BrowserState>(&mut shell, ids::CLEAR_FILTERS, json!({}));
    let again: BrowserState = run(&mut shell, ids::OPEN_PLACE, json!({ "label": label }));
    assert_eq!(names(&again), ["Kick.wav", "Pad.wav", "Snare.wav"]);

    // Kept between launches.
    shell.browser_view().flush_database();
    let mut relaunched = shell_on(&config);
    relaunched.set_user_library(Some(lib.clone()));
    assert!(relaunched.wait_for_browser_index(Duration::from_secs(20)));
    let back = state(&mut relaunched);
    assert_eq!(back.saved_labels.len(), 1);
    assert_eq!(back.saved_labels[0].name, "Aye");
    let listed: BrowserState = run(&mut relaunched, ids::OPEN_PLACE, json!({ "label": label }));
    assert_eq!(names(&listed), ["Kick.wav", "Pad.wav", "Snare.wav"]);

    // Remove deletes the label and nothing else.
    let removed: BrowserState = run(
        &mut relaunched,
        ids::REMOVE_LABEL,
        json!({ "label": label }),
    );
    assert!(removed.saved_labels.is_empty());
    assert_eq!(removed.label, Label::All);
    assert!(lib.join("Kick.wav").is_file());
    let tags: TagsState = run(&mut relaunched, ids::TAGS, json!({}));
    assert!(tags.groups.iter().any(|g| g.name == "Mine"));
    assert!(
        relaunched
            .run(ids::REMOVE_LABEL, json!({ "label": label }))
            .is_err()
    );
}

// Spec: browser-tags R8, R24.
#[test]
fn deleting_a_tag_a_saved_label_uses_asks_first_and_the_label_leaves_it_out() {
    let scratch = Scratch::new("delete");
    let (mut shell, _, lib) = library(&scratch);
    tag_the_library(&mut shell, &lib);
    filter(&mut shell, &["Mine|A", "Feel|Hard"], false);
    run::<BrowserState>(&mut shell, ids::ADD_LABEL, json!({}));
    run::<BrowserState>(
        &mut shell,
        ids::RENAME_LABEL,
        json!({ "label": { "saved": 1 }, "name": "Hard A" }),
    );
    // Asks, naming the label; nothing changed.
    let error = shell
        .run(ids::DELETE_TAG, json!({ "tag": "Mine|A" }))
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("Hard A") && error.contains("confirm"),
        "{error}"
    );
    let tags: TagsState = run(&mut shell, ids::TAGS, json!({}));
    assert!(
        tags.groups
            .iter()
            .any(|g| g.tags.iter().any(|t| t.full_name == "Mine|A"))
    );
    // Confirmed, the label leaves it out.
    run::<TagsState>(
        &mut shell,
        ids::DELETE_TAG,
        json!({ "tag": "Mine|A", "confirm": true }),
    );
    let after = state(&mut shell);
    assert_eq!(after.saved_labels[0].tags, ["Feel|Hard"]);
    assert_eq!(after.filter_view.selected, ["Feel|Hard"]);
    // A group the label uses is asked about the same way.
    assert!(
        shell
            .run(ids::DELETE_TAG, json!({ "group": "Feel" }))
            .is_err()
    );
    run::<TagsState>(
        &mut shell,
        ids::DELETE_TAG,
        json!({ "group": "Feel", "confirm": true }),
    );
    assert!(state(&mut shell).saved_labels[0].tags.is_empty());
    // A tag no label uses is deleted without asking.
    run::<TagsState>(&mut shell, ids::DELETE_TAG, json!({ "tag": "Mine|B" }));
}

// Spec: browser-tags R25.
#[test]
fn labels_are_renamed_given_icons_and_kept_between_launches() {
    let scratch = Scratch::new("names");
    let (mut shell, config, lib) = library(&scratch);
    let renamed: BrowserState = run(
        &mut shell,
        ids::RENAME_LABEL,
        json!({ "label": "drums", "name": "Beats" }),
    );
    let info = |state: &BrowserState, label: &Label| {
        state
            .labels
            .iter()
            .find(|l| &l.label == label)
            .cloned()
            .unwrap()
    };
    assert_eq!(info(&renamed, &Label::Drums).name, "Beats");
    // Without a name the name opens for editing, and again it ends.
    let editing: BrowserState = run(&mut shell, ids::RENAME_LABEL, json!({ "label": "drums" }));
    assert_eq!(editing.renaming_label, Some(Label::Drums));
    let ended: BrowserState = run(&mut shell, ids::RENAME_LABEL, json!({ "label": "drums" }));
    assert_eq!(ended.renaming_label, None);
    assert_eq!(info(&ended, &Label::Drums).name, "Beats");
    let icon: BrowserState = run(
        &mut shell,
        ids::SET_LABEL_ICON,
        json!({ "label": "drums", "icon": "bolt" }),
    );
    assert_eq!(info(&icon, &Label::Drums).icon.as_deref(), Some("bolt"));
    // A folder in Places takes an icon; All and the User Library keep theirs.
    let folder = scratch.path("Loops");
    wav(&folder.join("Loop.wav"), 100);
    run::<BrowserState>(
        &mut shell,
        ids::ADD_FOLDER,
        json!({ "path": text(&folder) }),
    );
    let folder_label = Label::Folder(text(&folder));
    let with_folder: BrowserState = run(
        &mut shell,
        ids::SET_LABEL_ICON,
        json!({ "label": folder_label, "icon": "star" }),
    );
    assert_eq!(
        info(&with_folder, &folder_label).icon.as_deref(),
        Some("star")
    );
    assert!(
        shell
            .run(
                ids::SET_LABEL_ICON,
                json!({ "label": "all", "icon": "star" })
            )
            .is_err()
    );
    assert!(
        shell
            .run(
                ids::SET_LABEL_ICON,
                json!({ "label": "user_library", "icon": "star" })
            )
            .is_err()
    );
    assert!(
        shell
            .run(
                ids::SET_LABEL_ICON,
                json!({ "label": "drums", "icon": "teapot" })
            )
            .is_err()
    );
    assert!(
        shell
            .run(
                ids::RENAME_LABEL,
                json!({ "label": folder_label, "name": "x" })
            )
            .is_err()
    );

    let mut relaunched = shell_on(&config);
    relaunched.set_user_library(Some(lib));
    let back = state(&mut relaunched);
    assert_eq!(info(&back, &Label::Drums).name, "Beats");
    assert_eq!(info(&back, &Label::Drums).icon.as_deref(), Some("bolt"));
    // Reset Icon, and the name given back.
    let reset: BrowserState = run(
        &mut relaunched,
        ids::SET_LABEL_ICON,
        json!({ "label": "drums" }),
    );
    assert_eq!(info(&reset, &Label::Drums).icon, None);
    let own: BrowserState = run(
        &mut relaunched,
        ids::RENAME_LABEL,
        json!({ "label": "drums", "name": "" }),
    );
    assert_eq!(info(&own, &Label::Drums).name, "Drums");
}

// Spec: browser-tags R26.
#[test]
fn library_labels_are_hidden_moved_and_reset() {
    let scratch = Scratch::new("order");
    let (mut shell, config, _) = library(&scratch);
    let library_names = |state: &BrowserState| -> Vec<String> {
        state
            .labels
            .iter()
            .filter(|l| l.section == ids::Section::Library)
            .map(|l| l.name.clone())
            .collect()
    };
    let first = state(&mut shell);
    assert_eq!(library_names(&first)[0], "All");
    assert_eq!(library_names(&first)[1], "Sounds");
    // Dragging Samples up puts it second; All stays first.
    let moved: BrowserState = run(
        &mut shell,
        ids::MOVE_LABEL,
        json!({ "label": "samples", "position": 0 }),
    );
    assert_eq!(&library_names(&moved)[..3], ["All", "Samples", "Sounds"]);
    assert!(
        shell
            .run(ids::MOVE_LABEL, json!({ "label": "all", "position": 4 }))
            .is_err()
    );
    // Hiding a label leaves it out of the sidebar, listed with its flag.
    let hidden: BrowserState = run(
        &mut shell,
        ids::SET_LABEL_SHOWN,
        json!({ "label": "tunings", "shown": false }),
    );
    let tunings = hidden
        .labels
        .iter()
        .find(|l| l.label == Label::Tunings)
        .unwrap();
    assert!(!tunings.shown);
    assert!(
        shell
            .run(
                ids::SET_LABEL_SHOWN,
                json!({ "label": "all", "shown": false })
            )
            .is_err()
    );
    // Hiding the chosen label chooses All.
    run::<BrowserState>(&mut shell, ids::OPEN_PLACE, json!({ "label": "clips" }));
    let chosen: BrowserState = run(
        &mut shell,
        ids::SET_LABEL_SHOWN,
        json!({ "label": "clips", "shown": false }),
    );
    assert_eq!(chosen.label, Label::All);

    // Kept between launches.
    let mut relaunched = shell_on(&config);
    let back = state(&mut relaunched);
    assert_eq!(&library_names(&back)[..3], ["All", "Samples", "Sounds"]);
    assert!(
        !back
            .labels
            .iter()
            .find(|l| l.label == Label::Tunings)
            .unwrap()
            .shown
    );
    // Reset Label Order puts the order back; hidden labels stay hidden.
    let reset: BrowserState = run(&mut relaunched, ids::RESET_LABEL_ORDER, json!({}));
    assert_eq!(&library_names(&reset)[..3], ["All", "Sounds", "Drums"]);
    assert!(
        !reset
            .labels
            .iter()
            .find(|l| l.label == Label::Tunings)
            .unwrap()
            .shown
    );
}

// Spec: browser-tags R27, R28.
#[test]
fn columns_sort_and_file_extensions_survive_a_restart() {
    let scratch = Scratch::new("columns");
    let (mut shell, config, lib) = library(&scratch);
    wav(&lib.join("Drums").join("Hat.wav"), 20);
    shell.browser_view().flush_database();
    run::<BrowserState>(
        &mut shell,
        ids::OPEN_PLACE,
        json!({ "label": "user_library" }),
    );
    // By default: Name, folders first.
    let first = state(&mut shell);
    assert_eq!(names(&first), ["Drums", "Kick.wav", "Pad.wav", "Snare.wav"]);
    assert_eq!(first.columns.sort, Column::Name);
    assert!(first.columns.shown.is_empty());
    assert!(first.rows.iter().all(|row| row.details.is_none()));

    let set: BrowserState = run(
        &mut shell,
        ids::SET_COLUMNS,
        json!({
            "shown": ["size", "kind", "length", "sample_rate", "channels"],
            "sort": "size",
            "descending": true,
            "file_extensions": false,
        }),
    );
    // Folders stay first, then the files by size, largest first.
    assert_eq!(names(&set), ["Drums", "Pad.wav", "Snare.wav", "Kick.wav"]);
    let pad = set.rows.iter().find(|r| r.name == "Pad.wav").unwrap();
    let details = pad.details.clone().unwrap();
    assert_eq!(details.kind.as_deref(), Some("Audio File"));
    assert_eq!(details.size, Some(80_044));
    assert_eq!(details.sample_rate, Some(48_000));
    assert_eq!(details.channels, Some(1));
    assert!((details.length_seconds.unwrap() - 40_000.0 / 48_000.0).abs() < 1e-6);
    let folder = set.rows.iter().find(|r| r.name == "Drums").unwrap();
    assert_eq!(
        folder.details.clone().unwrap().kind.as_deref(),
        Some("Folder")
    );
    assert_eq!(folder.details.clone().unwrap().length_seconds, None);
    // Clicking the column again reverses.
    let ascending: BrowserState = run(&mut shell, ids::SET_COLUMNS, json!({ "descending": false }));
    assert_eq!(
        names(&ascending),
        ["Drums", "Kick.wav", "Snare.wav", "Pad.wav"]
    );
    // Sorting by a column that isn't shown is refused.
    assert!(
        shell
            .run(ids::SET_COLUMNS, json!({ "sort": "date_modified" }))
            .is_err()
    );
    assert!(
        shell
            .run(ids::SET_COLUMNS, json!({ "shown": ["name"] }))
            .is_err()
    );
    run::<BrowserState>(&mut shell, ids::SET_COLUMNS, json!({ "descending": true }));

    // Kept between launches.
    let mut relaunched = shell_on(&config);
    relaunched.set_user_library(Some(lib.clone()));
    assert!(relaunched.wait_for_browser_index(Duration::from_secs(20)));
    run::<BrowserState>(
        &mut relaunched,
        ids::OPEN_PLACE,
        json!({ "label": "user_library" }),
    );
    let back = state(&mut relaunched);
    assert_eq!(back.columns, set.columns);
    assert_eq!(names(&back), names(&set));

    // Hiding the sorted column goes back to sorting by name.
    let hidden: BrowserState = run(
        &mut relaunched,
        ids::SET_COLUMNS,
        json!({ "shown": ["kind"] }),
    );
    assert_eq!(hidden.columns.sort, Column::Name);
    assert!(!hidden.columns.descending);
    // Search results sort by the column too, folders first.
    run::<BrowserState>(
        &mut relaunched,
        ids::SET_COLUMNS,
        json!({ "shown": ["size"], "sort": "size" }),
    );
    let found: BrowserState = run(
        &mut relaunched,
        ids::SEARCH,
        json!({ "query": ".wav", "label": "all" }),
    );
    assert_eq!(
        names(&found),
        ["Hat.wav", "Kick.wav", "Snare.wav", "Pad.wav"]
    );
}

// Spec: browser-tags R29, R30.
#[test]
fn history_steps_back_through_label_search_tags_and_a_new_step_drops_those_ahead() {
    let scratch = Scratch::new("history");
    let (mut shell, _, lib) = library(&scratch);
    tag_the_library(&mut shell, &lib);
    let start = state(&mut shell);
    assert!(
        !start.can_back && !start.can_forward,
        "disabled in a new set"
    );
    assert!(shell.run(ids::BACK, json!({})).is_err());

    run::<BrowserState>(&mut shell, ids::OPEN_PLACE, json!({ "label": "samples" }));
    // Typing is one step.
    for typed in ["p", "pa", "pad"] {
        run::<BrowserState>(&mut shell, ids::SEARCH, json!({ "query": typed }));
    }
    filter(&mut shell, &["Mine|A"], false);
    let now = state(&mut shell);
    assert!(now.can_back && !now.can_forward);

    let back: BrowserState = run(&mut shell, ids::BACK, json!({}));
    assert_eq!(
        (back.label.clone(), back.search.as_str()),
        (Label::Samples, "pad")
    );
    assert!(back.filter_view.selected.is_empty(), "the tags step");
    let back: BrowserState = run(&mut shell, ids::BACK, json!({}));
    assert_eq!(
        (back.label.clone(), back.search.as_str()),
        (Label::Samples, "")
    );
    let back: BrowserState = run(&mut shell, ids::BACK, json!({}));
    assert_eq!(back.label, Label::All);
    assert!(!back.can_back && back.can_forward);

    let forward: BrowserState = run(&mut shell, ids::FORWARD, json!({}));
    assert_eq!(forward.label, Label::Samples);
    // A new step drops those ahead.
    run::<BrowserState>(&mut shell, ids::OPEN_PLACE, json!({ "label": "clips" }));
    let end = state(&mut shell);
    assert!(end.can_back && !end.can_forward);
    assert!(shell.run(ids::FORWARD, json!({})).is_err());
}

// Spec: browser-tags R29.
#[test]
fn history_brings_back_open_folders_and_the_selection() {
    let scratch = Scratch::new("history-open");
    let (mut shell, _, lib) = library(&scratch);
    wav(&lib.join("Drums").join("Hat.wav"), 20);
    run::<BrowserState>(
        &mut shell,
        ids::OPEN_PLACE,
        json!({ "label": "user_library" }),
    );
    run::<BrowserState>(
        &mut shell,
        ids::OPEN_FOLDER,
        json!({ "path": text(&lib.join("Drums")), "open": true }),
    );
    run::<BrowserState>(
        &mut shell,
        ids::SELECT,
        json!({ "paths": [text(&lib.join("Drums").join("Hat.wav"))] }),
    );
    run::<BrowserState>(&mut shell, ids::OPEN_PLACE, json!({ "label": "samples" }));
    let back: BrowserState = run(&mut shell, ids::BACK, json!({}));
    assert_eq!(back.label, Label::UserLibrary);
    assert!(
        names(&back).contains(&"Hat.wav".to_owned()),
        "the folder is open again"
    );
    assert_eq!(back.selected, [text(&lib.join("Drums").join("Hat.wav"))]);
}

// Spec: browser-tags R31.
#[test]
fn starting_hot_swap_clears_the_search_and_the_tags_as_a_step() {
    use bl_engine::testing;
    let scratch = Scratch::new("hot-swap");
    let (mut shell, _, lib) = library(&scratch);
    tag_the_library(&mut shell, &lib);
    let top: Vec<TrackId> = shell.editor().document().top_level().collect();
    shell
        .invoke("arrangement.select_tracks", json!({ "tracks": [top[2]] }))
        .unwrap();
    let t = testing::gain_type();
    shell
        .invoke(
            "device.add",
            json!({ "device": { "kind": t.kind, "name": t.name, "role": t.role } }),
        )
        .unwrap();
    filter(&mut shell, &["Mine|A"], false);
    run::<BrowserState>(&mut shell, ids::SEARCH, json!({ "query": "pad" }));

    shell
        .invoke("device.hot_swap", json!({ "on": true }))
        .unwrap();
    let swapping = state(&mut shell);
    assert!(swapping.search.is_empty());
    assert!(swapping.filter_view.selected.is_empty());
    // Browse Back brings them back, label by label.
    let mut brought_back = None;
    for _ in 0..4 {
        let back: BrowserState = run(&mut shell, ids::BACK, json!({}));
        if back.search == "pad" {
            brought_back = Some(back);
            break;
        }
    }
    let back = brought_back.expect("a step holds the search");
    assert_eq!(back.filter_view.selected, ["Mine|A"]);
}

// Spec: browser-tags R32.
#[test]
fn the_new_commands_are_registered_with_their_classes() {
    let shell = Shell::headless();
    let registry = shell.registry();
    let class = |id: &str| registry.get(id).unwrap().permission.unwrap();
    for id in [ids::FILTER, ids::CLEAR_FILTERS, ids::BACK, ids::FORWARD] {
        assert_eq!(class(id), Permission::External, "{id}");
    }
    assert_eq!(class(ids::REMOVE_LABEL), Permission::Destructive);
    for id in [
        ids::SET_FILTER_VIEW,
        ids::SET_FILTER_GROUPS,
        ids::ADD_LABEL,
        ids::RENAME_LABEL,
        ids::SET_LABEL_ICON,
        ids::SET_LABEL_SHOWN,
        ids::MOVE_LABEL,
        ids::RESET_LABEL_ORDER,
        ids::SET_COLUMNS,
    ] {
        assert_eq!(class(id), Permission::Edit, "{id}");
    }
}

// Spec: browser R26.
#[test]
fn the_full_height_browser_switches_reads_back_and_survives_a_restart() {
    let scratch = Scratch::new("full-height");
    let config = scratch.path("config");
    let mut shell = shell_on(&config);
    let view = |shell: &mut Shell| -> Value { shell.run("view.state", json!({})).unwrap() };
    assert_eq!(view(&mut shell)["window"]["full_height_browser"], false);
    assert_eq!(
        shell.switch_state("view.toggle_full_height_browser"),
        Some(false)
    );
    let on = shell
        .run("view.toggle_full_height_browser", json!({}))
        .unwrap();
    assert_eq!(on["full_height_browser"], true);
    assert_eq!(view(&mut shell)["window"]["full_height_browser"], true);
    assert_eq!(
        shell.switch_state("view.toggle_full_height_browser"),
        Some(true)
    );
    assert!(shell.browser_view().full_height());

    let mut relaunched = shell_on(&config);
    assert_eq!(view(&mut relaunched)["window"]["full_height_browser"], true);
    // Calling it again puts the column back.
    let off = relaunched
        .run("view.toggle_full_height_browser", json!({}))
        .unwrap();
    assert_eq!(off["full_height_browser"], false);
    assert_eq!(
        view(&mut relaunched)["window"]["full_height_browser"],
        false
    );
}

// Spec: browser R29. `view.set_browser_width` sets the browser's width,
// brought within 70 to 4000 points, reads it back in `view.state`, changes
// nothing in the set, and keeps it for the next start.
#[test]
fn the_browser_width_is_set_within_its_limits_read_back_and_kept() {
    let scratch = Scratch::new("browser-width");
    let config = scratch.path("config");
    let mut shell = shell_on(&config);
    let width = |shell: &mut Shell| -> Value {
        shell.run("view.state", json!({})).unwrap()["window"]["browser_width"].clone()
    };
    assert_eq!(width(&mut shell), 302.0);
    let steps = shell.editor().undo_history().len();
    let set = shell
        .run("view.set_browser_width", json!({ "points": 420.0 }))
        .unwrap();
    assert_eq!(set["browser_width"], 420.0);
    assert_eq!(width(&mut shell), 420.0);
    shell
        .run("view.set_browser_width", json!({ "points": 12.0 }))
        .unwrap();
    assert_eq!(width(&mut shell), 70.0, "never narrower than its narrowest");
    shell
        .run("view.set_browser_width", json!({ "points": 9000.0 }))
        .unwrap();
    assert_eq!(width(&mut shell), 4000.0);
    shell
        .run("view.set_browser_width", json!({ "points": 455.0 }))
        .unwrap();
    assert!(shell.run("view.set_browser_width", json!({})).is_err());
    assert_eq!(shell.editor().undo_history().len(), steps);
    assert!(!shell.is_dirty());

    let mut relaunched = shell_on(&config);
    assert_eq!(width(&mut relaunched), 455.0);
}

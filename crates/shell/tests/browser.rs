//! The browser, driven headless through `Shell::run` and `Shell::invoke`, as
//! agents, menus, and keys drive it (WP-1.10; D17): places, listing, search,
//! the index, preview through the null device, and adding files to the
//! Arrangement. The behaviour is in the browser spec.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use bl_audio_io::NullControl;
use bl_commands::{Context, Permission};
use bl_core::time::Ticks;
use bl_model::{ClipData, TrackId, TrackKind};
use bl_shell::browser::places::{USER_LIBRARY_FOLDERS, create_user_library};
use bl_shell::browser::{BrowserPrefs, Part};
use bl_shell::browser_commands::{
    self as ids, AddResult, BrowserState, ItemKind, Label, PreviewState, Section,
};
use bl_shell::settings::Preferences;
use bl_shell::testing;
use bl_shell::{Area, Effect, Shell, ShellError, commands};
use serde_json::{Value, json};

const BAR: i64 = Ticks::WHOLE.get();

/// A folder of its own under the system's temporary folder, removed when
/// dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("bl-browser-{}-{name}", std::process::id()));
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

/// A library like a person's: folders, samples, a set, and files the
/// browser leaves out.
fn library(scratch: &Scratch) -> PathBuf {
    let root = scratch.path("User Library");
    create_user_library(&root).unwrap();
    wav(
        &root.join("Samples").join("Drums").join("Kick 10.wav"),
        4_800,
    );
    wav(
        &root.join("Samples").join("Drums").join("Kick 2.wav"),
        4_800,
    );
    wav(
        &root.join("Samples").join("Drums").join("Snare.aif.wav"),
        4_800,
    );
    wav(
        &root.join("Samples").join("Pads").join("Warm Pad.wav"),
        48_000,
    );
    wav(&root.join("Loop 120.wav"), 96_000);
    std::fs::write(root.join("Song.blset"), b"not really a set").unwrap();
    std::fs::write(root.join("notes.txt"), b"left out").unwrap();
    std::fs::write(root.join(".hidden.wav"), b"left out").unwrap();
    root
}

fn shell_with_library(scratch: &Scratch) -> (Shell, PathBuf) {
    let mut shell = Shell::headless();
    let root = library(scratch);
    shell.set_user_library(Some(root.clone()));
    (shell, root)
}

fn state(shell: &mut Shell) -> BrowserState {
    serde_json::from_value(shell.run(ids::STATE, json!({ "limit": 1000 })).unwrap()).unwrap()
}

fn run<T: serde::de::DeserializeOwned>(shell: &mut Shell, id: &str, args: Value) -> T {
    serde_json::from_value(
        shell
            .run(id, args)
            .unwrap_or_else(|e| panic!("{id} failed: {e}")),
    )
    .unwrap()
}

fn names(state: &BrowserState) -> Vec<String> {
    state
        .rows
        .iter()
        .map(|row| format!("{}{}", "  ".repeat(usize::from(row.depth)), row.name))
        .collect()
}

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
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

/// A shell on the null device paced by hand (T199), with the test clock
/// driving it, a library to browse, and the control that runs its blocks.
fn rig_with_library(scratch: &Scratch) -> (Shell, NullControl, PathBuf) {
    let (mut shell, control) = testing::rig();
    let root = library(scratch);
    shell.set_user_library(Some(root.clone()));
    (shell, control, root)
}

/// Advances the set and polls the shell (which reads the preview and the
/// index) until `done` holds, up to about ten seconds of test time.
fn advance_until(
    control: &NullControl,
    shell: &mut Shell,
    what: &str,
    mut done: impl FnMut(&mut Shell) -> bool,
) {
    for _ in 0..2_000 {
        shell.poll();
        if done(shell) {
            return;
        }
        testing::advance(control, shell, testing::STEP_MS);
    }
    panic!("timed out waiting for {what}");
}

fn wait_for_index(shell: &mut Shell) {
    wait_until(shell, "the index", |shell| {
        !shell.browser_view().index_state().indexing
    });
}

// Spec: browser R2.
#[test]
fn the_sidebar_lists_live_s_labels_without_ableton_s_services() {
    let mut shell = Shell::headless();
    let state = state(&mut shell);
    assert_eq!(state.label, Label::All, "a new set shows All");
    let library: Vec<&str> = state
        .labels
        .iter()
        .filter(|l| l.section == Section::Library)
        .map(|l| l.name.as_str())
        .collect();
    assert_eq!(
        library,
        [
            "All",
            "Sounds",
            "Drums",
            "Instruments",
            "Audio Effects",
            "MIDI Effects",
            "Modulators",
            "Plug-Ins",
            "Clips",
            "Samples",
            "Grooves",
            "Tunings",
            "Templates"
        ]
    );
    let places: Vec<&str> = state
        .labels
        .iter()
        .filter(|l| l.section == Section::Places)
        .map(|l| l.name.as_str())
        .collect();
    assert_eq!(places, ["User Library", "Current Project"]);
    for left_out in ["Max for Live", "Packs", "Splice", "Cloud", "Push"] {
        assert!(
            !state.labels.iter().any(|l| l.name == left_out),
            "{left_out} (D21)"
        );
    }
    // A Library label without content says what fills it (Instruments lists
    // the instrument rack since WP-5.8, so Sounds stands for it).
    let sounds: BrowserState = run(&mut shell, ids::OPEN_PLACE, json!({ "label": "sounds" }));
    assert!(sounds.rows.is_empty());
    assert!(sounds.note.unwrap().contains("M2"));
}

// Spec: modulation R12.
#[test]
fn the_modulators_label_lists_the_six_modulators() {
    let mut shell = Shell::headless();
    let state: BrowserState = run(
        &mut shell,
        ids::OPEN_PLACE,
        json!({ "label": "modulators" }),
    );
    let names: Vec<&str> = state.rows.iter().map(|row| row.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "LFO",
            "Shaper",
            "Envelope Follower",
            "Envelope MIDI",
            "Shaper MIDI",
            "Expression Control"
        ]
    );
    assert!(
        state
            .rows
            .iter()
            .all(|row| row.kind == bl_shell_spec::browser::ItemKind::Device)
    );
    assert!(state.note.is_none());
    // The search narrows the list by name.
    let found: BrowserState = run(&mut shell, ids::SEARCH, json!({ "query": "follower" }));
    assert_eq!(found.rows.len(), 1);
    // Loading the selected device adds it to the selected track's chain.
    let path = found.rows[0].path.clone();
    run::<Value>(&mut shell, ids::SELECT, json!({ "paths": [path] }));
    let track = shell.editor().document().top_level().next().unwrap();
    shell
        .invoke("arrangement.select_tracks", json!({ "tracks": [track] }))
        .unwrap();
    let before = shell.editor().document().device_count();
    shell.invoke(ids::LOAD_SELECTED, json!({})).unwrap();
    assert_eq!(shell.editor().document().device_count(), before + 1);
}

// Spec: browser R3.
#[test]
fn the_user_library_is_made_once_with_live_s_folders() {
    let scratch = Scratch::new("user-library");
    let root = scratch.path("Music/Beatlayer/User Library");
    assert!(create_user_library(&root).unwrap(), "made the first time");
    for folder in USER_LIBRARY_FOLDERS {
        assert!(root.join(folder).is_dir(), "{folder}");
    }
    std::fs::remove_dir(root.join("Defaults")).unwrap();
    wav(&root.join("Samples").join("mine.wav"), 10);
    assert!(!create_user_library(&root).unwrap(), "left alone after");
    assert!(!root.join("Defaults").exists(), "nothing is put back");
    assert!(root.join("Samples").join("mine.wav").is_file());

    let mut shell = Shell::headless();
    shell.set_user_library(Some(root.clone()));
    let state: BrowserState = run(
        &mut shell,
        ids::OPEN_PLACE,
        json!({ "label": "user_library" }),
    );
    assert_eq!(names(&state), ["Clips", "Grooves", "Presets", "Samples"]);
    let place = state
        .labels
        .iter()
        .find(|l| l.label == Label::UserLibrary)
        .unwrap();
    assert!(place.available);
    assert_eq!(place.path.as_deref(), Some(text(&root).as_str()));
}

// Spec: browser R4.
#[test]
fn current_project_follows_the_set_file() {
    let scratch = Scratch::new("project");
    let mut shell = Shell::headless();
    let state: BrowserState = run(
        &mut shell,
        ids::OPEN_PLACE,
        json!({ "label": "current_project" }),
    );
    assert!(state.rows.is_empty());
    assert!(state.note.unwrap().contains("hasn't been saved"));

    // Save Set As puts the set in a new "Song Project" folder (app-shell
    // R17), which becomes Current Project.
    let saved = shell
        .run(
            commands::SET_SAVE_AS,
            json!({ "path": text(&scratch.path("Song.blset")) }),
        )
        .unwrap();
    let project = Path::new(saved["path"].as_str().unwrap())
        .parent()
        .unwrap()
        .to_path_buf();
    assert_eq!(project, scratch.path("Song Project"));
    assert_eq!(shell.browser_view().project(), Some(project.as_path()));
    wav(
        &project.join("Samples").join("Imported").join("Vox.wav"),
        100,
    );
    let state: BrowserState = run(
        &mut shell,
        ids::OPEN_PLACE,
        json!({ "label": "current_project" }),
    );
    // The set file itself, and its Backup folder once it's saved twice.
    assert_eq!(names(&state), ["Samples", "Song.blset"]);
    shell.run(commands::SET_NEW, json!({})).unwrap();
    assert_eq!(shell.browser_view().project(), None);
}

// Spec: browser R5.
#[test]
fn added_folders_are_kept_and_removed() {
    let scratch = Scratch::new("folders");
    let config = scratch.path("config");
    let loops = scratch.path("Loops");
    wav(&loops.join("Break.wav"), 100);
    let mut shell = Shell::new(
        Preferences::default(),
        Some(config.clone()),
        bl_shell::audio::AudioHost::null(Default::default()),
        Box::new(bl_shell::files::ProjectFiles::new()),
    );
    // A person picks the folder in a dialog.
    assert!(shell.can_invoke(ids::ADD_FOLDER, &json!({})));
    shell.invoke(ids::ADD_FOLDER, json!({})).unwrap();
    assert_eq!(shell.take_effects(), [Effect::PickFolder]);

    let added: BrowserState = run(&mut shell, ids::ADD_FOLDER, json!({ "path": text(&loops) }));
    assert_eq!(added.label, Label::Folder(text(&loops)));
    assert_eq!(names(&added), ["Break.wav"]);
    let again: BrowserState = run(&mut shell, ids::ADD_FOLDER, json!({ "path": text(&loops) }));
    let places = |s: &BrowserState| {
        s.labels
            .iter()
            .filter(|l| l.section == Section::Places)
            .map(|l| l.name.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(places(&again), ["User Library", "Current Project", "Loops"]);
    assert!(loops.join("Break.wav").is_file(), "nothing moved");

    // Kept between launches.
    let prefs = Preferences::load(&config).unwrap();
    assert_eq!(prefs.browser.folders, std::slice::from_ref(&loops));
    let mut relaunched = Shell::new(
        prefs,
        None,
        bl_shell::audio::AudioHost::null(Default::default()),
        Box::new(bl_shell::files::ProjectFiles::new()),
    );
    assert_eq!(places(&state(&mut relaunched)), places(&again));

    // Remove from Sidebar, which leaves the folder alone.
    assert!(!shell.can_invoke(ids::REMOVE_FOLDER, &json!({ "path": "/nowhere" })));
    shell.set_focused_area(Area::Browser);
    shell
        .run(ids::FOCUS_BROWSER, json!({ "part": "sidebar" }))
        .unwrap();
    assert!(shell.can_invoke(ids::REMOVE_FOLDER, &json!({ "path": text(&loops) })));
    let removed: BrowserState = run(
        &mut shell,
        ids::REMOVE_FOLDER,
        json!({ "path": text(&loops) }),
    );
    assert_eq!(removed.label, Label::All);
    assert_eq!(places(&removed), ["User Library", "Current Project"]);
    assert!(loops.is_dir());
    assert!(
        Preferences::load(&config)
            .unwrap()
            .browser
            .folders
            .is_empty()
    );
    let error = shell
        .run(
            ids::ADD_FOLDER,
            json!({ "path": text(&loops.join("Break.wav")) }),
        )
        .unwrap_err();
    assert!(matches!(error, ShellError::InvalidArgs { .. }), "{error}");
}

// Spec: browser R6.
#[test]
fn folders_list_what_beatlayer_opens_folders_first() {
    let scratch = Scratch::new("listing");
    let (mut shell, root) = shell_with_library(&scratch);
    let state: BrowserState = run(
        &mut shell,
        ids::OPEN_PLACE,
        json!({ "label": "user_library" }),
    );
    assert_eq!(
        names(&state),
        [
            "Clips",
            "Defaults",
            "Grooves",
            "Presets",
            "Samples",
            "Loop 120.wav",
            "Song.blset"
        ]
    );
    assert_eq!(state.rows[5].kind, ItemKind::Audio);
    assert_eq!(state.rows[6].kind, ItemKind::Set);
    let drums = root.join("Samples").join("Drums");
    run::<BrowserState>(
        &mut shell,
        ids::OPEN_FOLDER,
        json!({ "path": text(&root.join("Samples")) }),
    );
    let state: BrowserState = run(
        &mut shell,
        ids::OPEN_FOLDER,
        json!({ "path": text(&drums) }),
    );
    assert_eq!(
        names(&state)[4..10],
        [
            "Samples",
            "  Drums",
            "    Kick 2.wav",
            "    Kick 10.wav",
            "    Snare.aif.wav",
            "  Pads"
        ]
    );
    assert!(state.rows[4].open && state.rows[5].open && !state.rows[9].open);

    // All lists the places themselves.
    let all: BrowserState = run(&mut shell, ids::OPEN_PLACE, json!({ "label": "all" }));
    assert_eq!(names(&all)[0], "User Library");
    assert_eq!(all.rows[0].kind, ItemKind::Folder);
}

// Spec: browser R6.
#[test]
fn opening_a_folder_closes_the_others_unless_kept() {
    let scratch = Scratch::new("open");
    let (mut shell, root) = shell_with_library(&scratch);
    run::<BrowserState>(
        &mut shell,
        ids::OPEN_PLACE,
        json!({ "label": "user_library" }),
    );
    let samples = root.join("Samples");
    let open = |shell: &mut Shell| -> Vec<String> {
        state(shell)
            .rows
            .iter()
            .filter(|r| r.open)
            .map(|r| r.name.clone())
            .collect()
    };
    for path in [&samples, &samples.join("Drums"), &samples.join("Pads")] {
        run::<BrowserState>(&mut shell, ids::OPEN_FOLDER, json!({ "path": text(path) }));
    }
    // Opening Pads closed Drums but kept Samples, which it's inside.
    assert_eq!(open(&mut shell), ["Samples", "Pads"]);
    run::<BrowserState>(
        &mut shell,
        ids::OPEN_FOLDER,
        json!({ "path": text(&samples.join("Drums")), "keep_others": true }),
    );
    assert_eq!(open(&mut shell), ["Samples", "Drums", "Pads"]);
    run::<BrowserState>(
        &mut shell,
        ids::OPEN_FOLDER,
        json!({ "path": text(&root.join("Clips")) }),
    );
    assert_eq!(open(&mut shell), ["Clips"]);
    // Closing a folder closes what's open inside it.
    run::<BrowserState>(
        &mut shell,
        ids::OPEN_FOLDER,
        json!({ "path": text(&samples.join("Drums")), "keep_others": true }),
    );
    run::<BrowserState>(
        &mut shell,
        ids::OPEN_FOLDER,
        json!({ "path": text(&samples), "open": false }),
    );
    assert_eq!(open(&mut shell), ["Clips"]);
}

// Spec: browser R7.
#[test]
fn selection_moves_extends_and_crosses_folders() {
    let scratch = Scratch::new("select");
    let (mut shell, root) = shell_with_library(&scratch);
    run::<PreviewState>(&mut shell, ids::SET_PREVIEW, json!({ "on": false }));
    run::<BrowserState>(
        &mut shell,
        ids::OPEN_PLACE,
        json!({ "label": "user_library" }),
    );
    let loop_file = root.join("Loop 120.wav");
    let state: BrowserState = run(
        &mut shell,
        ids::SELECT,
        json!({ "paths": [text(&loop_file)] }),
    );
    assert_eq!(state.selected, [text(&loop_file)]);
    assert_eq!(shell.browser_view().cursor(), Some(5));
    let set = root.join("Song.blset");
    let state: BrowserState = run(
        &mut shell,
        ids::SELECT,
        json!({ "paths": [text(&set)], "add": true }),
    );
    assert_eq!(state.selected.len(), 2);
    assert_eq!(shell.browser_view().cursor(), Some(6));
    // The selection survives opening a folder above it; the cursor follows.
    run::<BrowserState>(
        &mut shell,
        ids::OPEN_FOLDER,
        json!({ "path": text(&root.join("Samples")) }),
    );
    assert_eq!(shell.browser_view().cursor(), Some(8));
    let error = shell
        .run(
            ids::SELECT,
            json!({ "paths": [text(&root.join("gone.wav"))] }),
        )
        .unwrap_err();
    assert!(matches!(error, ShellError::InvalidArgs { .. }));
    assert_eq!(shell.browser_view().selected().len(), 2, "unchanged");
}

// Spec: browser R9.
#[test]
fn search_matches_every_word_in_the_label_s_places() {
    let scratch = Scratch::new("search");
    let (mut shell, root) = shell_with_library(&scratch);
    let loops = scratch.path("Loops");
    wav(&loops.join("Kick Loop.wav"), 100);
    run::<BrowserState>(&mut shell, ids::ADD_FOLDER, json!({ "path": text(&loops) }));
    wait_for_index(&mut shell);

    let found = |state: &BrowserState| -> Vec<String> {
        state
            .rows
            .iter()
            .map(|r| format!("{} in {}", r.name, r.folder.clone().unwrap_or_default()))
            .collect()
    };
    let all: BrowserState = run(
        &mut shell,
        ids::SEARCH,
        json!({ "query": "KICK", "label": "all" }),
    );
    assert_eq!(
        found(&all),
        [
            "Kick 2.wav in User Library/Samples/Drums",
            "Kick 10.wav in User Library/Samples/Drums",
            "Kick Loop.wav in Loops"
        ]
    );
    let both: BrowserState = run(&mut shell, ids::SEARCH, json!({ "query": "loop kick" }));
    assert_eq!(found(&both), ["Kick Loop.wav in Loops"]);
    let place: BrowserState = run(
        &mut shell,
        ids::SEARCH,
        json!({ "query": "kick", "label": "user_library" }),
    );
    assert_eq!(place.total_rows, 2);
    let samples: BrowserState = run(
        &mut shell,
        ids::SEARCH,
        json!({ "query": "", "label": "samples" }),
    );
    assert_eq!(
        samples.total_rows, 6,
        "every audio file, no folders or sets"
    );
    let folders: BrowserState = run(
        &mut shell,
        ids::SEARCH,
        json!({ "query": "drums", "label": "all" }),
    );
    assert_eq!(folders.rows[0].kind, ItemKind::Folder);
    let none: BrowserState = run(&mut shell, ids::SEARCH, json!({ "query": "zzz" }));
    assert!(none.note.unwrap().contains("Nothing matches"));
    // An empty query shows the label's contents again.
    let back: BrowserState = run(
        &mut shell,
        ids::SEARCH,
        json!({ "query": "", "label": "user_library" }),
    );
    assert_eq!(back.rows[0].name, "Clips");
    let _ = root;
}

// Spec: browser R10.
#[test]
fn the_index_follows_places_in_the_background() {
    let scratch = Scratch::new("index");
    let (mut shell, root) = shell_with_library(&scratch);
    wait_for_index(&mut shell);
    let indexed = shell.browser_view().index_state();
    // Five audio files and the set; the five starting folders, and Drums
    // and Pads in Samples.
    assert_eq!((indexed.files, indexed.folders), (6, 7));

    let extra = scratch.path("Extra");
    for i in 0..300 {
        wav(&extra.join(format!("Folder {}/Hit {i}.wav", i % 7)), 10);
    }
    run::<BrowserState>(&mut shell, ids::ADD_FOLDER, json!({ "path": text(&extra) }));
    wait_for_index(&mut shell);
    assert_eq!(shell.browser_view().index_state().files, 306);
    let hits: BrowserState = run(
        &mut shell,
        ids::SEARCH,
        json!({ "query": "hit 29", "label": "all" }),
    );
    // Every word somewhere in the name: Hit 29, 129, 229, and 290 to 299.
    assert_eq!(hits.total_rows, 13);
    run::<BrowserState>(
        &mut shell,
        ids::REMOVE_FOLDER,
        json!({ "path": text(&extra) }),
    );
    wait_for_index(&mut shell);
    assert_eq!(shell.browser_view().index_state().files, 6);
    let _ = root;
}

fn preview(shell: &mut Shell) -> PreviewState {
    shell.browser_preview()
}

// Spec: browser R11.
#[test]
fn selecting_a_file_previews_it_while_the_switch_is_on() {
    let scratch = Scratch::new("preview");
    let (mut shell, control, root) = rig_with_library(&scratch);
    shell.start_audio();
    assert!(shell.audio().is_running(), "the null device runs");
    assert!(preview(&mut shell).on, "on in a new install");
    let pad = root.join("Samples").join("Pads").join("Warm Pad.wav");
    let before = shell.editor().document().clone();

    run::<BrowserState>(&mut shell, ids::SELECT, json!({ "paths": [text(&pad)] }));
    assert_eq!(preview(&mut shell).loading, Some(text(&pad)));
    advance_until(&control, &mut shell, "the preview to play", |shell| {
        shell.browser_preview().position_seconds > 0.0
    });
    let playing = preview(&mut shell);
    assert_eq!(playing.playing, Some(text(&pad)));
    assert!((playing.length_seconds - 1.0).abs() < 0.01, "{playing:?}");
    // The set never changes.
    assert_eq!(*shell.editor().document(), before);
    assert!(!shell.is_dirty());

    // Selecting a folder stops it.
    run::<BrowserState>(
        &mut shell,
        ids::SELECT,
        json!({ "paths": [text(&root.join("Samples"))] }),
    );
    assert_eq!(preview(&mut shell).playing, None);

    // With the switch off, selecting doesn't preview; Shift-Enter does.
    let state: PreviewState = run(&mut shell, ids::SET_PREVIEW, json!({}));
    assert!(!state.on);
    assert_eq!(shell.switch_state(ids::SET_PREVIEW), Some(false));
    let kick = root.join("Samples").join("Drums").join("Kick 2.wav");
    run::<BrowserState>(&mut shell, ids::SELECT, json!({ "paths": [text(&kick)] }));
    assert_eq!(preview(&mut shell).loading, None);
    shell.set_focused_area(Area::Browser);
    assert!(shell.can_invoke(ids::PREVIEW, &json!({})));
    run::<PreviewState>(&mut shell, ids::PREVIEW, json!({}));
    advance_until(&control, &mut shell, "the short preview to end", |shell| {
        let preview = shell.browser_preview();
        preview.loading.is_none() && preview.playing.is_none()
    });

    // A file that isn't audio can't be previewed.
    let error = shell
        .run(
            ids::PREVIEW,
            json!({ "path": text(&root.join("Song.blset")) }),
        )
        .unwrap_err();
    assert!(matches!(error, ShellError::InvalidArgs { .. }));
    // Nor without a running device.
    shell.audio_mut().stop();
    let error = shell
        .run(ids::PREVIEW, json!({ "path": text(&pad) }))
        .unwrap_err();
    assert!(matches!(error, ShellError::Unavailable { .. }), "{error}");
}

// Spec: browser R11.
#[test]
fn preview_volume_reaches_silence() {
    let scratch = Scratch::new("volume");
    let config = scratch.path("config");
    let mut shell = Shell::new(
        Preferences::default(),
        Some(config.clone()),
        bl_shell::audio::AudioHost::null(Default::default()),
        Box::new(bl_shell::files::ProjectFiles::new()),
    );
    let state: PreviewState = run(&mut shell, ids::SET_PREVIEW, json!({ "volume": 0.0 }));
    assert_eq!(state.volume, 0.0);
    assert!(state.on, "setting the volume leaves the switch");
    let state: PreviewState = run(&mut shell, ids::SET_PREVIEW, json!({ "volume": 9.0 }));
    assert!((state.volume - ids::MAX_PREVIEW_VOLUME).abs() < 1e-9);
    assert_eq!(
        Preferences::load(&config).unwrap().browser,
        BrowserPrefs {
            preview_volume: ids::MAX_PREVIEW_VOLUME,
            ..BrowserPrefs::default()
        }
    );
    assert!(
        shell
            .run(ids::SET_PREVIEW, json!({ "volume": "loud" }))
            .is_err()
    );
}

fn audio_tracks(shell: &Shell) -> Vec<TrackId> {
    let doc = shell.editor().document();
    doc.top_level()
        .filter(|id| doc.track(*id).unwrap().kind() == TrackKind::Audio)
        .collect()
}

// Spec: browser R13. The acceptance criterion of WP-1.10.
#[test]
fn dropping_below_the_last_track_creates_a_track() {
    let scratch = Scratch::new("drop-new");
    let (mut shell, root) = shell_with_library(&scratch);
    let tracks_before: Vec<TrackId> = shell.editor().document().top_level().collect();
    let loop_file = root.join("Loop 120.wav");
    let result: AddResult = run(
        &mut shell,
        ids::ADD_TO_ARRANGEMENT,
        json!({ "paths": [text(&loop_file)], "at": 4 * BAR }),
    );
    let doc = shell.editor().document();
    let tracks: Vec<TrackId> = doc.top_level().collect();
    assert_eq!(tracks.len(), tracks_before.len() + 1);
    let new = *tracks.last().unwrap();
    assert_eq!(result.created_tracks, [new], "below the last track");
    let track = doc.track(new).unwrap();
    assert_eq!(track.kind(), TrackKind::Audio);
    assert_eq!(track.name(), "Loop 120");
    let clip = track.clips().next().unwrap();
    assert_eq!(clip.start, Ticks(4 * BAR));
    assert_eq!(clip.name, "Loop 120");
    // Two seconds at 120 BPM is a bar of 4/4.
    assert_eq!(clip.length, Ticks(BAR));
    assert!(matches!(clip.data, ClipData::Audio(_)));
    assert!(shell.is_dirty());
    // One undo step takes the track and the clip away together.
    assert_eq!(
        shell.editor().undo_history().collect::<Vec<_>>(),
        ["Insert Clip"]
    );
    shell.run("edit.undo", json!({})).unwrap();
    let after: Vec<TrackId> = shell.editor().document().top_level().collect();
    assert_eq!(after, tracks_before);
    assert_eq!(shell.editor().document().clip_count(), 0);
    // The file is in the media store for its waveform.
    assert_eq!(shell.media_store().entries().len(), 1);
}

// Spec: browser R13.
#[test]
fn dropping_on_an_audio_track_places_a_clip_snapped() {
    let scratch = Scratch::new("drop-track");
    let (mut shell, root) = shell_with_library(&scratch);
    let track = audio_tracks(&shell)[0];
    // Where a drop 2.3 bars in lands with the default grid (snapping on).
    let at = shell
        .arrangement()
        .snap(shell.editor().document(), Ticks(2 * BAR + BAR * 3 / 10));
    assert_eq!(at, Ticks(2 * BAR), "the bar grid at the default zoom");
    let tracks_before = shell.editor().document().top_level().len();
    let result: AddResult = run(
        &mut shell,
        ids::ADD_TO_ARRANGEMENT,
        json!({ "paths": [text(&root.join("Loop 120.wav"))], "track": track, "at": at }),
    );
    assert!(result.created_tracks.is_empty());
    assert_eq!(shell.editor().document().top_level().len(), tracks_before);
    assert_eq!(result.clips[0].track, track);
    assert_eq!(result.clips[0].start, at);
    assert_eq!(
        shell
            .editor()
            .document()
            .track(track)
            .unwrap()
            .clips()
            .count(),
        1
    );
    assert_eq!(shell.editor().undo_history().len(), 1);
}

// Spec: browser R13. A drop over part of a clip replaces that part, as the
// Arrangement's edits do: the clip is split around it, shortened, or has
// its start trimmed (playing on from where it was), all in the drop's one
// undo step.
#[test]
fn a_drop_replaces_what_it_covers_in_one_step() {
    let scratch = Scratch::new("drop-over");
    let (mut shell, root) = shell_with_library(&scratch);
    let track = audio_tracks(&shell)[0];
    let lane = |shell: &Shell| {
        let mut clips: Vec<(i64, i64, i64)> = shell
            .editor()
            .document()
            .track(track)
            .unwrap()
            .clips()
            .map(|c| {
                // Where its contents begin: an audio clip's Start marker
                // moves as it's trimmed.
                let origin = c.origin(&shell.editor().document().settings().tempo);
                (c.start.get(), c.end().get(), (c.start - origin).get())
            })
            .collect();
        clips.sort_unstable();
        clips
    };
    let (looped, pad) = (
        text(&root.join("Loop 120.wav")),
        text(&root.join("Samples").join("Pads").join("Warm Pad.wav")),
    );
    // A four-bar clip from the one-bar loop.
    let placed: AddResult = run(
        &mut shell,
        ids::ADD_TO_ARRANGEMENT,
        json!({ "paths": [looped], "track": track, "at": 0 }),
    );
    let long = placed.clips[0].clip;
    shell
        .run(
            "clip.set_length",
            json!({ "clip": long, "length": 4 * BAR }),
        )
        .unwrap();
    let before = shell.editor().document().clone();
    let steps = shell.editor().undo_history().len();

    // The half-bar pad inside it splits it around the pad.
    let _: AddResult = run(
        &mut shell,
        ids::ADD_TO_ARRANGEMENT,
        json!({ "paths": [pad], "track": track, "at": BAR }),
    );
    assert_eq!(
        lane(&shell),
        [
            (0, BAR, 0),
            (BAR, 3 * BAR / 2, 0),
            (3 * BAR / 2, 4 * BAR, 3 * BAR / 2),
        ]
    );
    assert_eq!(shell.editor().undo_history().len(), steps + 1, "one step");
    shell.run("edit.undo", json!({})).unwrap();
    assert_eq!(
        *shell.editor().document(),
        before,
        "undo restores it exactly"
    );
    shell.run("edit.redo", json!({})).unwrap();

    // The loop over the pad's end and the second part's start: the pad is
    // shortened, and the part starts later, playing on from where it was.
    let _: AddResult = run(
        &mut shell,
        ids::ADD_TO_ARRANGEMENT,
        json!({ "paths": [looped], "track": track, "at": 5 * BAR / 4 }),
    );
    assert_eq!(
        lane(&shell),
        [
            (0, BAR, 0),
            (BAR, 5 * BAR / 4, 0),
            (5 * BAR / 4, 9 * BAR / 4, 0),
            (9 * BAR / 4, 4 * BAR, 9 * BAR / 4),
        ]
    );
    // One that covers a clip whole removes it (the pad), and trims the
    // start of the one it covers part of.
    let _: AddResult = run(
        &mut shell,
        ids::ADD_TO_ARRANGEMENT,
        json!({ "paths": [looped], "track": track, "at": BAR }),
    );
    assert_eq!(
        lane(&shell),
        [
            (0, BAR, 0),
            (BAR, 2 * BAR, 0),
            (2 * BAR, 9 * BAR / 4, 3 * BAR / 4),
            (9 * BAR / 4, 4 * BAR, 9 * BAR / 4),
        ]
    );
}

// Spec: browser R13.
#[test]
fn several_files_go_in_sequence_or_spread() {
    let scratch = Scratch::new("drop-several");
    let (mut shell, root) = shell_with_library(&scratch);
    let files = [
        root.join("Loop 120.wav"),
        root.join("Samples").join("Pads").join("Warm Pad.wav"),
    ]
    .map(|p| text(&p));
    let track = audio_tracks(&shell)[1];
    let result: AddResult = run(
        &mut shell,
        ids::ADD_TO_ARRANGEMENT,
        json!({ "paths": files, "track": track, "at": 0 }),
    );
    // One after another: the loop is a bar, the pad half a bar.
    let starts: Vec<(TrackId, Ticks)> = result.clips.iter().map(|c| (c.track, c.start)).collect();
    assert_eq!(starts, [(track, Ticks(0)), (track, Ticks(BAR))]);
    assert_eq!(result.clips[1].length, Ticks(BAR / 2));

    // Spread (Cmd held): a track each, new ones below the target.
    let tracks_before: Vec<TrackId> = shell.editor().document().top_level().collect();
    let result: AddResult = run(
        &mut shell,
        ids::ADD_TO_ARRANGEMENT,
        json!({ "paths": files, "track": track, "at": 8 * BAR, "spread": true }),
    );
    assert_eq!(result.created_tracks.len(), 1);
    let tracks: Vec<TrackId> = shell.editor().document().top_level().collect();
    let at = tracks.iter().position(|t| *t == track).unwrap();
    assert_eq!(tracks[at + 1], result.created_tracks[0], "just below");
    assert_eq!(result.clips[0].start, result.clips[1].start);
    assert_eq!(tracks.len(), tracks_before.len() + 1);
    // Into the drop area, spread: two new tracks at the bottom, named after
    // their files.
    let result: AddResult = run(
        &mut shell,
        ids::ADD_TO_ARRANGEMENT,
        json!({ "paths": files, "spread": true }),
    );
    let doc = shell.editor().document();
    let names: Vec<&str> = result
        .created_tracks
        .iter()
        .map(|t| doc.track(*t).unwrap().name())
        .collect();
    assert_eq!(names, ["Loop 120", "Warm Pad"]);
    let tracks: Vec<TrackId> = doc.top_level().collect();
    assert_eq!(tracks[tracks.len() - 2..], result.created_tracks[..]);
    assert_eq!(shell.editor().undo_history().len(), 3, "a step each");
}

// Spec: browser R13.
#[test]
fn drops_on_other_tracks_are_refused() {
    let scratch = Scratch::new("drop-refused");
    let (mut shell, root) = shell_with_library(&scratch);
    let doc = shell.editor().document();
    let midi = doc.top_level().next().unwrap();
    let ret = doc.returns().next().unwrap();
    let main = doc.main_id();
    let loop_file = text(&root.join("Loop 120.wav"));
    for track in [midi, ret, main] {
        let error = shell
            .run(
                ids::ADD_TO_ARRANGEMENT,
                json!({ "paths": [loop_file], "track": track }),
            )
            .unwrap_err();
        assert!(matches!(error, ShellError::InvalidArgs { .. }), "{error}");
    }
    let error = shell
        .run(
            ids::ADD_TO_ARRANGEMENT,
            json!({ "paths": [text(&root.join("notes.txt"))] }),
        )
        .unwrap_err();
    assert!(matches!(error, ShellError::Failed { .. }), "{error}");
    assert!(
        shell
            .run(ids::ADD_TO_ARRANGEMENT, json!({ "paths": [] }))
            .is_err()
    );
    assert!(!shell.editor().can_undo(), "nothing changed");
    assert!(!shell.is_dirty());
}

// Spec: browser R14.
#[test]
fn enter_loads_onto_the_selected_track_or_a_new_one() {
    let scratch = Scratch::new("enter");
    let (mut shell, root) = shell_with_library(&scratch);
    run::<PreviewState>(&mut shell, ids::SET_PREVIEW, json!({ "on": false }));
    shell.set_focused_area(Area::Browser);
    assert!(
        !shell.can_invoke(ids::LOAD_SELECTED, &json!({})),
        "nothing selected"
    );
    run::<BrowserState>(
        &mut shell,
        ids::SELECT,
        json!({ "paths": [text(&root.join("Loop 120.wav"))] }),
    );
    assert!(shell.active_contexts().contains(&Context::Browser));
    assert!(shell.can_invoke(ids::LOAD_SELECTED, &json!({})));
    // An audio track selected, the insert marker at bar 3.
    let track = audio_tracks(&shell)[0];
    shell
        .run(
            "arrangement.set_insert_marker",
            json!({ "at": 2 * BAR, "track": track }),
        )
        .unwrap();
    let result: AddResult = run(&mut shell, ids::LOAD_SELECTED, json!({}));
    assert!(result.created_tracks.is_empty());
    assert_eq!(
        (result.clips[0].track, result.clips[0].start),
        (track, Ticks(2 * BAR))
    );
    // A MIDI track selected: a new audio track.
    let midi = shell.editor().document().top_level().next().unwrap();
    shell
        .run(
            "arrangement.set_insert_marker",
            json!({ "at": 0, "track": midi }),
        )
        .unwrap();
    let result: AddResult = run(&mut shell, ids::LOAD_SELECTED, json!({}));
    assert_eq!(result.created_tracks.len(), 1);
    assert_eq!(result.clips[0].start, Ticks(0));
}

// Spec: browser R15.
#[test]
fn added_media_is_saved_and_registered_again() {
    let scratch = Scratch::new("media");
    let (mut shell, root) = shell_with_library(&scratch);
    let pad = root.join("Samples").join("Pads").join("Warm Pad.wav");
    let result: AddResult = run(
        &mut shell,
        ids::ADD_TO_ARRANGEMENT,
        json!({ "paths": [text(&pad)] }),
    );
    let saved = shell
        .run(
            commands::SET_SAVE_AS,
            json!({ "path": text(&scratch.path("Song.blset")) }),
        )
        .unwrap();
    // In the "Song Project" folder Save Set As makes (app-shell R17).
    let set = PathBuf::from(saved["path"].as_str().unwrap());
    assert_eq!(set, scratch.path("Song Project/Song.blset"));
    let (_, project) = bl_project::SetFile::open(&set).unwrap();
    let file = project.media.iter().next().expect("the pad is listed");
    assert_eq!(
        file.path.as_str(),
        "../User Library/Samples/Pads/Warm Pad.wav"
    );
    let ClipData::Audio(audio) = &shell
        .editor()
        .document()
        .clip(result.clips[0].clip)
        .unwrap()
        .data
    else {
        panic!("an audio clip");
    };
    assert_eq!(file.hash, audio.media);

    let mut reopened = Shell::headless();
    reopened
        .run(commands::SET_OPEN, json!({ "path": text(&set) }))
        .unwrap();
    let store = reopened.media_store();
    let hash = bl_media::ContentHash::from_bytes(audio.media.0);
    assert_eq!(store.check(hash).unwrap(), bl_media::MediaStatus::Available);
    assert_eq!(
        std::fs::canonicalize(store.entry(hash).unwrap().path).unwrap(),
        std::fs::canonicalize(&pad).unwrap()
    );
}

// Spec: browser R16.
#[test]
fn every_browser_command_is_registered_with_its_class() {
    let shell = Shell::headless();
    for id in ids::IDS {
        let spec = shell.registry().get(id).unwrap_or_else(|| panic!("{id}"));
        assert!(spec.args.is_some() && spec.result.is_some(), "{id}");
        assert!(!spec.description.is_empty(), "{id}");
    }
    let class = |id| shell.registry().get(id).unwrap().permission.unwrap();
    assert_eq!(class(ids::ADD_TO_ARRANGEMENT), Permission::External);
    assert_eq!(class(ids::ADD_FOLDER), Permission::External);
    assert_eq!(class(ids::SET_PREVIEW), Permission::Edit);
    assert_eq!(class(ids::FOCUS_BROWSER), Permission::Read);
    // The keys: Option-5, Cmd-F, and Enter in the list.
    let bound = |id: &str| -> Vec<String> {
        shell
            .registry()
            .get(id)
            .unwrap()
            .bindings
            .iter()
            .map(|b| b.shortcut.to_string())
            .collect()
    };
    assert_eq!(bound(ids::FOCUS_BROWSER), ["S-FOCUS-006"]);
    assert_eq!(bound(ids::FOCUS_SEARCH), ["S-BRW-006"]);
    assert_eq!(bound(ids::LOAD_SELECTED), ["S-BRW-004"]);
}

// Spec: browser R8.
#[test]
fn focus_moves_to_the_browser_and_its_parts() {
    let mut shell = Shell::headless();
    shell.run(commands::VIEW_TOGGLE_BROWSER, json!({})).unwrap();
    assert!(!shell.browser_visible());
    shell.run(ids::FOCUS_BROWSER, json!({})).unwrap();
    assert!(shell.browser_visible(), "Option-5 shows the browser");
    assert_eq!(shell.focused_area(), Area::Browser);
    assert_eq!(shell.browser_view().focus(), Part::Content);
    assert_eq!(shell.active_contexts()[0], Context::Browser);
    shell
        .run(ids::FOCUS_BROWSER, json!({ "part": "sidebar" }))
        .unwrap();
    assert_eq!(shell.active_contexts()[0], Context::BrowserSidebar);
    run::<BrowserState>(&mut shell, ids::OPEN_PLACE, json!({ "label": "samples" }));
    shell.run(ids::FOCUS_SEARCH, json!({})).unwrap();
    assert_eq!(
        shell.browser_view().label(),
        &Label::All,
        "Cmd-F chooses All"
    );
    assert_eq!(shell.browser_view().focus(), Part::Search);
    assert!(
        shell
            .take_effects()
            .contains(&Effect::FocusArea(Area::Browser))
    );
}

// Spec: browser R13. Where a drop lands, headless: the rows the Arrangement
// shows at its default zoom of 30 points a bar.
#[test]
fn drop_targets_follow_the_rows_and_the_grid() {
    use bl_shell::browser::drop::drop_target;

    let shell = Shell::headless();
    let doc = shell.editor().document();
    let view = shell.arrangement();
    let tracks: Vec<TrackId> = doc.top_level().collect();
    let at = |x: f32, y: f32, ignore_snap: bool| drop_target(view, doc, 569.0, x, y, ignore_snap);
    // The MIDI tracks (rows 0 and 1) take no audio.
    assert_eq!(at(10.0, 40.0, false), None);
    // "3 Audio" is the third 88-point row; 2.3 bars in snaps to the bar.
    let target = at(69.0, 200.0, false).unwrap();
    assert_eq!(target.track, Some(tracks[2]));
    assert_eq!(target.at, Ticks(2 * BAR));
    assert_eq!((target.top, target.height), (176.0, 88.0));
    // Cmd (Alt) held ignores the grid.
    assert_eq!(
        at(69.0, 200.0, true).unwrap().at,
        Ticks(2 * BAR + BAR * 3 / 10)
    );
    // Below the last track: the drop area, for a new track.
    let area = at(69.0, 400.0, false).unwrap();
    assert_eq!(area.track, None);
    assert_eq!(area.top, 352.0);
    // Outside the lanes.
    assert_eq!(at(-1.0, 200.0, false), None);
    assert_eq!(at(10.0, 600.0, false), None);
}

// Spec: multisampler R47. The browser lists `.sfz` files with the samples,
// and Load Selected makes a multisampler of one: on the selected MIDI track,
// otherwise on a new one.
#[test]
fn the_browser_lists_sfz_files_and_loads_one_as_a_multisampler() {
    let scratch = Scratch::new("sfz");
    let kits = scratch.path("Banks");
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/sfz");
    std::fs::create_dir_all(kits.join("samples")).unwrap();
    for entry in std::fs::read_dir(fixtures.join("samples")).unwrap() {
        let file = entry.unwrap().path();
        std::fs::copy(&file, kits.join("samples").join(file.file_name().unwrap())).unwrap();
    }
    std::fs::copy(fixtures.join("Sine Bank.sfz"), kits.join("Sine Bank.sfz")).unwrap();
    let mut shell = Shell::headless();
    let listed: BrowserState = run(&mut shell, ids::ADD_FOLDER, json!({ "path": text(&kits) }));
    let sfz = kits.join("Sine Bank.sfz");
    let row = listed
        .rows
        .iter()
        .find(|row| row.name == "Sine Bank.sfz")
        .expect("the SFZ file is listed");
    assert_eq!(row.kind, ItemKind::Sfz);
    shell.set_focused_area(Area::Browser);
    run::<BrowserState>(&mut shell, ids::SELECT, json!({ "paths": [text(&sfz)] }));
    assert!(shell.can_invoke(ids::LOAD_SELECTED, &json!({})));
    // A MIDI track is selected in a new set: the multisampler goes on it.
    let midi = shell.editor().document().top_level().next().unwrap();
    shell
        .run("arrangement.select_tracks", json!({ "tracks": [midi] }))
        .unwrap();
    let loaded = shell.run(ids::LOAD_SELECTED, json!({})).unwrap();
    assert_eq!(loaded["track"], json!(midi));
    let device = serde_json::from_value(loaded["device"].clone()).unwrap();
    let held = shell.editor().document().device(device).unwrap();
    assert_eq!(held.layers.list.len(), 5);
    // An audio track selected: a new MIDI track.
    let audio = audio_tracks(&shell)[0];
    shell
        .run("arrangement.select_tracks", json!({ "tracks": [audio] }))
        .unwrap();
    let tracks = shell.editor().document().top_level().count();
    let again = shell.run(ids::LOAD_SELECTED, json!({})).unwrap();
    assert_ne!(again["track"], json!(audio));
    assert_eq!(shell.editor().document().top_level().count(), tracks + 1);
}

/// A click track file, and where each click starts.
fn clicks(
    scratch: &Scratch,
    name: &str,
    bpm: f64,
    offset: f64,
    seconds: f64,
) -> (PathBuf, Vec<f64>) {
    let path = scratch.path(&format!("{name}.wav"));
    let (samples, starts) = bl_analysis::signals::click_track(bpm, offset, seconds, 48_000);
    let mut bytes = Vec::new();
    let frames = samples.len() as u32;
    let byte_len = frames * 2;
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + byte_len).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&48_000u32.to_le_bytes());
    bytes.extend_from_slice(&96_000u32.to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&byte_len.to_le_bytes());
    for sample in &samples {
        bytes.extend_from_slice(&((sample * 32_000.0) as i16).to_le_bytes());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&path, bytes).unwrap();
    (path, starts)
}

// Spec: browser R18.
#[test]
fn the_raw_switch_is_a_kept_preference_and_raw_previews_play_once() {
    let scratch = Scratch::new("raw-pref");
    let config = scratch.path("config");
    let mut shell = Shell::new(
        Preferences::default(),
        Some(config.clone()),
        bl_shell::audio::AudioHost::null(Default::default()),
        Box::new(bl_shell::files::ProjectFiles::new()),
    );
    shell.start_audio();
    let preview = shell.browser_preview();
    assert!(!preview.raw, "raw is off by default");

    let state: PreviewState = run(&mut shell, ids::SET_RAW, json!({}));
    assert!(state.raw, "toggling raw switches it on");

    let state: PreviewState = run(&mut shell, ids::SET_RAW, json!({ "raw": false }));
    assert!(!state.raw, "setting raw false turns it off");

    let _ = run::<PreviewState>(&mut shell, ids::SET_RAW, json!({ "raw": true }));
    assert!(
        Preferences::load(&config).unwrap().browser.raw,
        "raw is kept in preferences"
    );

    let (loop_file, _) = clicks(&scratch, "loop", 100.0, 0.0, 1.2);
    run::<BrowserState>(
        &mut shell,
        ids::SELECT,
        json!({ "paths": [text(&loop_file)] }),
    );
    wait_until(&mut shell, "the preview to load and start", |shell| {
        shell.browser_preview().playing.is_some()
    });
    // With Raw on, a preview plays unwarped at its own speed and stops.
    wait_until(&mut shell, "raw preview to finish", |shell| {
        let p = shell.browser_preview();
        p.loading.is_none() && p.playing.is_none()
    });
}

// Spec: browser R19.
#[test]
fn a_100_bpm_loop_previews_at_the_sets_120_and_loops() {
    let scratch = Scratch::new("bpm-loop");
    let (mut shell, _) = shell_with_library(&scratch);
    shell.start_audio();
    // 2-bar loop at 100 BPM (4.8 seconds).
    let (loop_file, _) = clicks(&scratch, "loop100", 100.0, 0.0, 4.8);
    run::<BrowserState>(
        &mut shell,
        ids::SELECT,
        json!({ "paths": [text(&loop_file)] }),
    );
    // Warped to 120 BPM the two bars last 4 seconds and start again; an
    // unwarped preview would play its 4.8 seconds once and stop.
    wait_until(&mut shell, "the preview to pass 3 seconds", |shell| {
        shell.browser_preview().position_seconds > 3.0
    });
    wait_until(&mut shell, "the preview to loop", |shell| {
        let preview = shell.browser_preview();
        preview.playing.is_some() && preview.position_seconds < 1.0
    });
}

// Spec: browser R19.
#[test]
fn a_one_shot_previews_once_at_its_own_speed() {
    let scratch = Scratch::new("one-shot");
    let (mut shell, _) = shell_with_library(&scratch);
    shell.start_audio();
    let (hit_file, _) = clicks(&scratch, "hit", 60.0, 0.0, 0.2);
    run::<BrowserState>(
        &mut shell,
        ids::SELECT,
        json!({ "paths": [text(&hit_file)] }),
    );
    wait_until(&mut shell, "one-shot to start playing", |shell| {
        shell.browser_preview().playing.is_some()
    });
    wait_until(&mut shell, "one-shot to finish playing", |shell| {
        let p = shell.browser_preview();
        p.loading.is_none() && p.playing.is_none()
    });
}

// Spec: browser R21.
#[test]
fn a_warped_preview_waits_for_its_analysis_and_a_raw_one_does_not() {
    let scratch = Scratch::new("wait-analysis");
    let (mut shell, _) = shell_with_library(&scratch);
    shell.start_audio();
    let (loop_file, _) = clicks(&scratch, "newloop", 100.0, 0.0, 4.8);
    // Raw preview: does not wait for analysis, starts immediately.
    run::<PreviewState>(&mut shell, ids::SET_RAW, json!({ "raw": true }));
    run::<BrowserState>(
        &mut shell,
        ids::SELECT,
        json!({ "paths": [text(&loop_file)] }),
    );
    wait_until(&mut shell, "raw preview to start", |shell| {
        shell.browser_preview().playing.is_some()
    });
    run::<PreviewState>(&mut shell, ids::STOP_PREVIEW, json!({}));

    // With Raw off, warped preview waits for analysis if pending.
    run::<PreviewState>(&mut shell, ids::SET_RAW, json!({ "raw": false }));
    let (loop_file2, _) = clicks(&scratch, "newloop2", 100.0, 0.0, 4.8);
    run::<BrowserState>(
        &mut shell,
        ids::SELECT,
        json!({ "paths": [text(&loop_file2)] }),
    );
    wait_until(
        &mut shell,
        "warped preview to start once analyzed",
        |shell| shell.browser_preview().playing.is_some(),
    );
}

// Spec: browser R22.
#[test]
fn clicking_the_strip_moves_a_warped_preview_on_the_quantisation_step() {
    let scratch = Scratch::new("scrub-preview");
    let (mut shell, _) = shell_with_library(&scratch);
    shell.start_audio();
    let (loop_file, _) = clicks(&scratch, "scrubloop", 100.0, 0.0, 4.8);
    run::<BrowserState>(
        &mut shell,
        ids::SELECT,
        json!({ "paths": [text(&loop_file)] }),
    );
    wait_until(&mut shell, "warped preview to play", |shell| {
        shell.browser_preview().playing.is_some()
    });
    // Scrubbing to 50% moves the preview.
    run::<PreviewState>(&mut shell, ids::SCRUB_PREVIEW, json!({ "fraction": 0.5 }));
    wait_until(&mut shell, "scrubbed position to update", |shell| {
        (shell.browser_preview().position_seconds - 2.4).abs() < 0.5
    });

    // With Raw on, scrubbing does nothing.
    run::<PreviewState>(&mut shell, ids::SET_RAW, json!({ "raw": true }));
    let res = shell.run(ids::SCRUB_PREVIEW, json!({ "fraction": 0.2 }));
    assert!(res.is_ok());
}

// Spec: warping R51.
#[test]
fn a_warped_preview_plays_through_the_map_a_new_clip_would_get() {
    let scratch = Scratch::new("warp-map-preview");
    let (mut shell, _) = shell_with_library(&scratch);
    shell.start_audio();
    let (loop_file, _) = clicks(&scratch, "defaults_loop", 100.0, 0.0, 4.8);
    run::<BrowserState>(
        &mut shell,
        ids::SELECT,
        json!({ "paths": [text(&loop_file)] }),
    );
    wait_until(&mut shell, "warped preview to play", |shell| {
        shell.browser_preview().playing.is_some()
    });
    let preview = shell.browser_preview();
    assert!(preview.playing.is_some());
}

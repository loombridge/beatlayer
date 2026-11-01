//! Show Similar Files (WP-9.9; spec: sound-similarity R11 to R16, R19),
//! driven headless through `Shell::run`, as agents drive it: the neighbours
//! of a generated family are listed most similar first with the reference
//! first, kinds stay apart, the search field's token narrows and ends, and a
//! saved label keeps the reference first across a restart. Analysis is paced
//! by hand, so no test waits on the clock.

use std::path::{Path, PathBuf};
use std::time::Duration;

use bl_shell::Shell;
use bl_shell::browser_commands::{self as ids, BrowserState, Label, SimilarList};
use bl_shell::settings::Preferences;
use serde_json::json;

/// A folder of its own under the system's temporary folder, removed when
/// dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("bl-similar-{}-{name}", std::process::id()));
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

/// Writes a 16-bit mono WAV of a decaying tone of `hz`, `seconds` long.
fn wav(path: &Path, hz: f64, seconds: f64, rate: u32) {
    let frames = (seconds * f64::from(rate)) as u32;
    let data = frames * 2;
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&rate.to_le_bytes());
    bytes.extend_from_slice(&(rate * 2).to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data.to_le_bytes());
    for i in 0..frames {
        let t = f64::from(i) / f64::from(rate);
        let sample = (t * hz * std::f64::consts::TAU).sin() * (-t * 6.0).exp() * 20_000.0;
        bytes.extend_from_slice(&(sample as i16).to_le_bytes());
    }
    std::fs::write(path, bytes).unwrap();
}

/// Saves a Polysynth's preset, which plays by itself, at `path`.
fn preset(path: &Path) {
    let synth = bl_devices::polysynth_type();
    let id = bl_core::id::Id::new(1).unwrap();
    let device = bl_model::Device::new(bl_model::DeviceId(id), synth.name.to_string(), synth.kind)
        .with_role(synth.role);
    bl_project::preset::Preset::of(&device).save(path).unwrap();
}

/// A shell on the null device with `library` as its User Library, indexed,
/// analysis paced by hand.
fn shell_on(config: &Path, library: &Path) -> Shell {
    let mut shell = Shell::new(
        Preferences::default(),
        Some(config.to_path_buf()),
        bl_shell::audio::AudioHost::null(Default::default()),
        Box::new(bl_shell::files::ProjectFiles::new()),
    );
    shell.pace_analysis_by_hand();
    shell.set_user_library(Some(library.to_path_buf()));
    assert!(shell.wait_for_browser_index(Duration::from_secs(60)));
    shell
}

/// Three kicks, three hi-hats, a sample over 60 seconds and two presets.
fn place(scratch: &Scratch) -> PathBuf {
    let library = scratch.path("Library");
    std::fs::create_dir_all(&library).unwrap();
    for (name, hz) in [("Kick 50", 50.0), ("Kick 55", 55.0), ("Kick 60", 60.0)] {
        wav(&library.join(format!("{name}.wav")), hz, 0.4, 22_050);
    }
    for (name, hz) in [
        ("Hat 6k", 6_000.0),
        ("Hat 7k", 7_000.0),
        ("Hat 8k", 8_000.0),
    ] {
        wav(&library.join(format!("{name}.wav")), hz, 0.12, 22_050);
    }
    wav(&library.join("Long.wav"), 110.0, 61.0, 8_000);
    preset(&library.join("Lead.blpreset"));
    preset(&library.join("Pluck.blpreset"));
    library
}

fn run<T: serde::de::DeserializeOwned>(shell: &mut Shell, id: &str, args: serde_json::Value) -> T {
    serde_json::from_value(
        shell
            .run(id, args)
            .unwrap_or_else(|e| panic!("{id} failed: {e}")),
    )
    .unwrap()
}

fn state(shell: &mut Shell) -> BrowserState {
    run(shell, ids::STATE, json!({ "limit": 1000 }))
}

fn names(state: &BrowserState) -> Vec<String> {
    state.rows.iter().map(|row| row.name.clone()).collect()
}

fn path(library: &Path, name: &str) -> String {
    library.join(name).to_string_lossy().into_owned()
}

fn select(shell: &mut Shell, path: &str) {
    run::<BrowserState>(shell, ids::SELECT, json!({ "paths": [path] }));
}

/// A shell whose nine files are all analysed.
fn analysed(scratch: &Scratch) -> (Shell, PathBuf) {
    let library = place(scratch);
    let mut shell = shell_on(&scratch.path("config"), &library);
    shell.release_analysis(9);
    (shell, library)
}

// Spec: sound-similarity R12, R13, R16, R19. The list is the reference,
// then its neighbours most similar first; a kick's neighbours are kicks.
#[test]
fn the_neighbours_of_a_kick_are_the_kicks_most_similar_first() {
    let scratch = Scratch::new("order");
    let (mut shell, library) = analysed(&scratch);
    let list: SimilarList = run(
        &mut shell,
        ids::SIMILAR,
        json!({ "path": path(&library, "Kick 55.wav") }),
    );
    assert_eq!(list.reference.path, path(&library, "Kick 55.wav"));
    assert_eq!(list.items[0], list.reference, "the reference is first");
    assert_eq!(list.reference.distance, 0.0);
    let order: Vec<f32> = list.items.iter().map(|item| item.distance).collect();
    assert!(order.windows(2).all(|w| w[0] <= w[1]), "{order:?}");
    assert!(order.iter().skip(1).all(|d| *d < 0.35), "{order:?}");
    let names: Vec<&str> = list
        .items
        .iter()
        .map(|item| Path::new(&item.path).file_stem().unwrap().to_str().unwrap())
        .collect();
    assert_eq!(names[0], "Kick 55");
    assert!(
        names[1..3].contains(&"Kick 50") && names[1..3].contains(&"Kick 60"),
        "the other kicks come before any hi-hat: {names:?}"
    );
    assert!(
        list.items
            .iter()
            .all(|item| item.path.ends_with(".wav") && !item.path.contains("Long")),
        "samples with samples, and not the long one: {names:?}"
    );
}

// Spec: sound-similarity R12. Presets list presets, never samples.
#[test]
fn a_preset_lists_presets_and_no_samples() {
    let scratch = Scratch::new("presets");
    let (mut shell, library) = analysed(&scratch);
    let list: SimilarList = run(
        &mut shell,
        ids::SIMILAR,
        json!({ "path": path(&library, "Lead.blpreset") }),
    );
    assert!(
        list.items.iter().all(|i| i.path.ends_with(".blpreset")),
        "{list:?}"
    );
    assert_eq!(list.items[0].path, path(&library, "Lead.blpreset"));
}

// Spec: sound-similarity R11. A file not analysed yet, a sample over 60
// seconds and a file that isn't a sound offer no search.
#[test]
fn files_not_analysed_or_over_sixty_seconds_offer_no_search() {
    let scratch = Scratch::new("none");
    let library = place(&scratch);
    let mut shell = shell_on(&scratch.path("config"), &library);
    let kick = path(&library, "Kick 55.wav");
    select(&mut shell, &kick);
    assert!(
        shell
            .browser_view()
            .selected_reference()
            .unwrap_err()
            .contains("hasn't been analysed")
    );
    assert!(shell.run(ids::SIMILAR, json!({})).is_err());
    assert!(shell.run(ids::SHOW_SIMILAR, json!({})).is_err());

    shell.release_analysis(9);
    assert_eq!(shell.browser_view().selected_reference(), Ok(kick.into()));
    select(&mut shell, &path(&library, "Long.wav"));
    assert!(shell.browser_view().selected_reference().is_err());
    assert!(
        shell
            .run(ids::SHOW_SIMILAR, json!({}))
            .is_err_and(|e| format!("{e:?}").contains("60 seconds"))
    );
    assert_eq!(state(&mut shell).similar, None, "nothing changed");
}

// Spec: sound-similarity R12, R14, R16. Showing similar files chooses All,
// puts the reference in the field, and lists what `browser.similar` does;
// typing narrows, and ending goes back.
#[test]
fn showing_similar_files_lists_the_neighbours_under_the_token_and_ends_back() {
    let scratch = Scratch::new("show");
    let (mut shell, library) = analysed(&scratch);
    run::<BrowserState>(&mut shell, ids::OPEN_PLACE, json!({ "label": "samples" }));
    let kick = path(&library, "Kick 55.wav");
    select(&mut shell, &kick);

    let shown: BrowserState = run(&mut shell, ids::SHOW_SIMILAR, json!({}));
    assert_eq!(shown.label, Label::All);
    assert_eq!(shown.similar.as_deref(), Some(kick.as_str()));
    assert_eq!(shown.search, "", "the token isn't text");
    let list: SimilarList = run(&mut shell, ids::SIMILAR, json!({}));
    let rows: Vec<String> = shown.rows.iter().map(|r| r.path.clone()).collect();
    let items: Vec<String> = list.items.iter().map(|i| i.path.clone()).collect();
    assert_eq!(rows, items, "headless gives the same list");
    assert_eq!(rows[0], kick);

    // The columns' sort doesn't reorder it, and typing narrows by name.
    let narrowed: BrowserState = run(&mut shell, ids::SEARCH, json!({ "query": "kick" }));
    assert_eq!(narrowed.similar.as_deref(), Some(kick.as_str()));
    assert_eq!(names(&narrowed)[0], "Kick 55.wav");
    assert!(names(&narrowed).iter().all(|n| n.starts_with("Kick")));

    let ended: BrowserState = run(&mut shell, ids::SHOW_SIMILAR, json!({ "end": true }));
    assert_eq!((ended.label, ended.similar), (Label::Samples, None));
    assert_eq!(ended.search, "");

    // Clearing the words ends it too.
    run::<BrowserState>(&mut shell, ids::SHOW_SIMILAR, json!({}));
    let cleared: BrowserState = run(&mut shell, ids::CLEAR_FILTERS, json!({}));
    assert_eq!(cleared.similar, None);
}

// Spec: sound-similarity R15. A saved label made from the results lists the
// reference first, after a restart too.
#[test]
fn a_saved_label_from_the_results_lists_the_reference_first_after_a_restart() {
    let scratch = Scratch::new("label");
    let config = scratch.path("config");
    let library = place(&scratch);
    let kick = path(&library, "Kick 55.wav");
    let before: Vec<String> = {
        let mut shell = shell_on(&config, &library);
        shell.release_analysis(9);
        select(&mut shell, &kick);
        run::<BrowserState>(&mut shell, ids::SHOW_SIMILAR, json!({}));
        let saved: BrowserState = run(&mut shell, ids::ADD_LABEL, json!({}));
        assert_eq!(
            saved.saved_labels[0].reference.as_deref(),
            Some(kick.as_str())
        );
        assert_eq!(saved.rows[0].path, kick);
        shell.browser_view().flush_database();
        saved.rows.iter().map(|r| r.path.clone()).collect()
    };
    let mut shell = shell_on(&config, &library);
    let label = bl_shell::browser_commands::Label::Saved(1);
    let reopened: BrowserState = run(&mut shell, ids::OPEN_PLACE, json!({ "label": label }));
    let rows: Vec<String> = reopened.rows.iter().map(|r| r.path.clone()).collect();
    assert_eq!(rows[0], kick, "the reference first, not by name");
    assert_eq!(rows, before);
    assert_eq!(reopened.similar.as_deref(), Some(kick.as_str()));
}

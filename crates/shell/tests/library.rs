//! Beatlayer's library (WP-9.46; the core library spec, R5 to R14, and the
//! browser spec, R31), headless: a package built from the test manifest is
//! installed from a `file://` URL through the Packages manager.

use std::path::{Path, PathBuf};

use bl_library::{Manifest as LibraryManifest, build};
use bl_shell::Shell;
use bl_shell::audio::AudioHost;
use bl_shell::browser_commands::{self as browser, BrowserState, Label};
use bl_shell::files::ProjectFiles;
use bl_shell::packages::Manifest;
use bl_shell::settings::Preferences;
use serde_json::{Value, json};

/// A folder for one test's files, removed when the test ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("bl-library-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../library/tests/fixtures")
}

/// A shell whose manifest lists the library's test package, built from the
/// test manifest and served from a file.
fn shell(scratch: &Scratch) -> Shell {
    let manifest = LibraryManifest::load(&fixtures()).unwrap();
    let built = build(&manifest, &fixtures(), "core", &scratch.0, &|url, _| {
        Err(format!("nothing is fetched here: {url}"))
    })
    .unwrap();
    let entry = json!({ "packages": [{
        "id": "library-core", "name": "Test Library", "version": "1",
        "project": "https://example.com/", "purpose": "Two generated tones",
        "platform": "any", "role": "library-core",
        "url": format!("file://{}", built.archive.display()),
        "size": built.size, "sha256": built.sha256, "unpack": "tar_gz", "keep_all": true,
        "folder": "library-core-1", "items": 2,
        "files": [{ "name": "library-package.json" }],
    }]});
    let mut shell = Shell::new(
        Preferences::default(),
        Some(scratch.0.join("config")),
        AudioHost::null(Default::default()),
        Box::new(ProjectFiles::new()),
    );
    shell.use_manifest(Manifest::parse(&entry.to_string()).unwrap());
    shell
}

fn run(shell: &mut Shell, id: &str, args: Value) -> Value {
    shell
        .run(id, args)
        .unwrap_or_else(|e| panic!("{id} failed: {e}"))
}

fn browser_state(shell: &mut Shell) -> BrowserState {
    serde_json::from_value(run(shell, browser::STATE, json!({ "limit": 100 }))).unwrap()
}

fn names(shell: &mut Shell) -> Vec<String> {
    browser_state(shell)
        .rows
        .into_iter()
        .map(|r| r.name)
        .collect()
}

fn open(shell: &mut Shell, label: Label) {
    run(shell, browser::OPEN_PLACE, json!({ "label": label }));
}

fn search(shell: &mut Shell, words: &str) -> Vec<String> {
    run(shell, browser::SEARCH, json!({ "query": words }));
    assert!(shell.wait_for_browser_index(std::time::Duration::from_secs(20)));
    names(shell)
}

fn install(shell: &mut Shell) {
    run(shell, "library.install", Value::Null);
    shell.wait_packages();
}

// Spec: core-library R5, R6, R13; browser R31. Before it's installed the
// library's packages are listed with their sizes and counts, the label has no
// rows, and Cancel on its page goes back to All.
#[test]
fn the_library_is_offered_before_it_is_installed() {
    let scratch = Scratch::new("offered");
    let mut shell = shell(&scratch);
    let state = run(&mut shell, "library.state", Value::Null);
    assert_eq!(state["installed"], false);
    assert_eq!(state["packages"][0]["id"], "library-core");
    assert_eq!(state["packages"][0]["state"], "not_installed");
    assert_eq!(state["packages"][0]["items"], 2);
    assert!(state["folder"].as_str().unwrap().ends_with("Library"));

    // The label is first after All, and opens with no rows: the page shows.
    let labels: Vec<_> = browser_state(&mut shell)
        .labels
        .into_iter()
        .filter(|l| l.section == bl_shell::browser_commands::Section::Library)
        .collect();
    assert_eq!(labels[0].label, Label::All);
    assert_eq!(labels[1].label, Label::CoreLibrary);
    assert_eq!(labels[1].name, "Core Library");
    open(&mut shell, Label::CoreLibrary);
    assert!(names(&mut shell).is_empty());

    run(&mut shell, "library.cancel", Value::Null);
    assert_eq!(browser_state(&mut shell).label, Label::All);
}

// Spec: core-library R5 to R10, R13; browser R31. The package installs from a
// file URL on first use, read-only; its items list under Core Library and the
// label they belong to, with their tags, and search finds them.
#[test]
fn the_package_installs_and_its_items_list_with_their_tags() {
    let scratch = Scratch::new("installs");
    let mut shell = shell(&scratch);
    install(&mut shell);

    let state = run(&mut shell, "library.state", Value::Null);
    assert_eq!(state["installed"], true);
    assert_eq!(state["packages"][0]["state"], "installed");
    assert_eq!(state["packages"][0]["items"], 2);
    // R7: in Library, not Packages, and not to be written to.
    let dir = shell.library_folder().join("library-core-1");
    let file = dir.join("tone-a3/tone-a3.wav");
    assert!(file.is_file());
    assert!(std::fs::metadata(&file).unwrap().permissions().readonly());
    assert!(!shell.packages_folder().join("library-core-1").exists());

    open(&mut shell, Label::CoreLibrary);
    assert_eq!(names(&mut shell), ["Tone A3", "Tone A4"]);

    // R9: the manifest's tags, and the Source tag, are in the database.
    let key = file.to_string_lossy().into_owned();
    let tags: Value = run(&mut shell, browser::TAGS, json!({ "items": [key] }));
    let carried: Vec<&str> = tags["items"][0]["tags"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["tag"].as_str().unwrap())
        .collect();
    for tag in ["Genre|Test", "Character|Pure", "Source|Generated"] {
        assert!(carried.contains(&tag), "{tag} in {carried:?}");
    }

    // R10: search by name and by tag, under the library's label and All.
    assert_eq!(search(&mut shell, "a4"), ["Tone A4"]);
    assert_eq!(search(&mut shell, "#Character|Bright"), ["Tone A4"]);
    assert_eq!(search(&mut shell, "#Genre|Test"), ["Tone A3", "Tone A4"]);
    run(&mut shell, browser::SEARCH, json!({ "query": "" }));
    open(&mut shell, Label::All);
    assert!(search(&mut shell, "tone-a3").contains(&"tone-a3.wav".to_owned()));
    // Samples lists the library's audio beside the person's.
    run(&mut shell, browser::SEARCH, json!({ "query": "" }));
    open(&mut shell, Label::Samples);
    assert!(shell.wait_for_browser_index(std::time::Duration::from_secs(20)));
    assert_eq!(names(&mut shell), ["tone-a3.wav", "tone-a4.wav"]);
}

// Spec: core-library R11, R12, R13. Each item's credit names its creator and
// licence; the credits list exports as a text file.
#[test]
fn credits_list_and_export() {
    let scratch = Scratch::new("credits");
    let mut shell = shell(&scratch);
    // Nothing is installed, so there is nothing to export.
    let none = shell.run(
        "library.export_credits",
        json!({ "path": scratch.0.join("none.txt").to_string_lossy() }),
    );
    assert!(none.is_err_and(|e| e.to_string().contains("no library items")));

    install(&mut shell);
    let credits = run(&mut shell, "library.credits", json!({}));
    assert_eq!(credits["packages"][0]["name"], "Test Library");
    let a3 = &credits["packages"][0]["items"][0];
    assert_eq!(a3["id"], "tone-a3");
    let credit = a3["credit"].as_str().unwrap();
    assert!(credit.contains("by Beatlayer"), "{credit}");
    assert!(credit.contains("CC0 1.0 (https://creativecommons.org/publicdomain/zero/1.0/)"));

    let path = scratch.0.join("credits.txt");
    let exported = run(
        &mut shell,
        "library.export_credits",
        json!({ "path": path.to_string_lossy() }),
    );
    assert_eq!(exported["items"], 2);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), credits["text"]);

    // The window opens and closes (Help ▸ Library Credits...).
    assert_eq!(
        run(&mut shell, "library.show_credits", Value::Null)["credits_open"],
        true
    );
    assert_eq!(
        run(&mut shell, "library.close_credits", Value::Null)["credits_open"],
        false
    );

    // The set uses none of it yet.
    let set = run(&mut shell, "library.credits", json!({ "set": true }));
    assert_eq!(set["packages"], json!([]));
}

// Spec: core-library R5, R7, R13. Removing the package takes its items out of
// the browser and the credits.
#[test]
fn removing_the_package_takes_its_items_away() {
    let scratch = Scratch::new("removes");
    let mut shell = shell(&scratch);
    install(&mut shell);
    open(&mut shell, Label::CoreLibrary);
    assert_eq!(names(&mut shell).len(), 2);

    run(
        &mut shell,
        "packages.remove",
        json!({ "ids": ["library-core"] }),
    );
    shell.wait_packages();
    assert!(!shell.library_folder().join("library-core-1").exists());
    assert_eq!(
        run(&mut shell, "library.state", Value::Null)["installed"],
        false
    );
    assert!(names(&mut shell).is_empty());
    assert_eq!(
        run(&mut shell, "library.credits", json!({}))["packages"],
        json!([])
    );
}

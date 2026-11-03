//! Beatlayer's library in the windows (WP-9.46; the browser spec, R31; the
//! core library spec, R6, R11 and R12; the Info View spec, R13): the
//! download page in the browser, the Library Credits window, and a library
//! item's credit in the Info View.

use std::path::{Path, PathBuf};

use crate::common::{Harness, Window};
use bl_library::{Manifest as LibraryManifest, build};
use bl_shell::packages::Manifest;
use egui::accesskit::{Action, Role};
use serde_json::{Value, json};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../library/tests/fixtures")
}

/// A harness whose manifest lists the library's test package, built from the
/// test manifest, in a folder of its own, and the package's ID.
fn harness(name: &str) -> (Harness, String, PathBuf) {
    let dir = std::env::temp_dir().join(format!("bl-ui-library-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let manifest = LibraryManifest::load(&fixtures()).unwrap();
    let built = build(&manifest, &fixtures(), "core", &dir, &|url, _| {
        Err(format!("nothing is fetched here: {url}"))
    })
    .unwrap();
    // Tests run in parallel and share the system's temporary folder: each
    // package has an ID, a role and a folder of its own.
    let id = format!("library-core-{name}-{}", std::process::id());
    let folder = format!("{id}-1");
    let entry = json!({ "packages": [{
        "id": id, "name": "Test Library", "version": "1",
        "project": "https://example.com/", "purpose": "Two generated tones",
        "platform": "any", "role": id,
        "url": format!("file://{}", built.archive.display()),
        "size": built.size, "sha256": built.sha256, "unpack": "tar_gz", "keep_all": true,
        "folder": folder, "items": 2,
        "files": [{ "name": "library-package.json" }],
    }]});
    let mut h = Harness::new(Window::Main);
    h.shell
        .use_manifest(Manifest::parse(&entry.to_string()).unwrap());
    let installed = h.shell.library_folder().join(folder);
    (h, id, installed)
}

/// Removes what a test installed, so the next run starts clean.
fn clean(h: &mut Harness, id: &str) {
    let _ = h.shell.run("packages.remove", json!({ "ids": [id] }));
}

fn press(h: &mut Harness, label: &str) {
    let nodes = Harness::nodes(&h.frame(vec![]));
    let id = Harness::node(&nodes, label).id;
    h.frame(vec![egui::Event::AccessKitActionRequest(
        egui::accesskit::ActionRequest {
            action: Action::Click,
            target_tree: egui::accesskit::TreeId::ROOT,
            target_node: id,
            data: None,
        },
    )]);
}

// Spec: browser R31, core-library R6. Core Library, before it's installed,
// shows the packages, their sizes and the folder, with Download and Cancel;
// Download installs, and the items list in its place.
#[test]
fn the_label_shows_the_download_page_until_the_library_is_installed() {
    let (mut h, id, _) = harness("page");
    h.shell
        .run("browser.open_place", json!({ "label": "core_library" }))
        .unwrap();
    h.settle();
    let nodes = Harness::nodes(&h.frame(vec![]));
    let labels: Vec<&str> = nodes.iter().map(|n| n.label.as_str()).collect();
    assert!(
        labels.contains(&"Download Required Packages Now"),
        "{labels:?}"
    );
    assert!(labels.contains(&"Cancel"));
    assert!(nodes.iter().any(|n| n.label.contains("Test Library")));

    press(&mut h, "Download Required Packages Now");
    h.wait("the library to install", |h| {
        h.shell.library_state().installed
    });
    h.settle();
    let labels: Vec<String> = Harness::nodes(&h.frame(vec![]))
        .into_iter()
        .map(|n| n.label)
        .collect();
    assert!(!labels.contains(&"Download Required Packages Now".to_owned()));
    assert!(labels.iter().any(|l| l == "Tone A3"), "{labels:?}");
    clean(&mut h, &id);
}

// Spec: core-library R12. Help ▸ Library Credits... opens the window, which
// lists each package's credits as a list; Close and Esc close it.
#[test]
fn the_credits_window_lists_each_items_credit() {
    let (mut h, id, _) = harness("credits");
    h.shell.run("library.install", Value::Null).unwrap();
    h.shell.wait_packages();
    h.shell.run("library.show_credits", Value::Null).unwrap();
    h.settle();
    let nodes = Harness::nodes(&h.frame(vec![]));
    assert!(
        nodes
            .iter()
            .any(|n| n.role == Role::List && n.label == "Test Library")
    );
    let credits: Vec<&str> = nodes
        .iter()
        .filter(|n| n.role == Role::ListItem)
        .map(|n| n.label.as_str())
        .collect();
    assert!(
        credits
            .iter()
            .any(|c| c.contains("“Tone A4” by Beatlayer, CC0 1.0")),
        "{credits:?}"
    );
    press(&mut h, "Close");
    h.settle();
    assert!(!h.shell.library_credits_open());
    clean(&mut h, &id);
}

// Spec: info-view R13, core-library R11. A library item selected in the
// browser shows its credit in the Info View.
#[test]
fn a_selected_library_item_shows_its_credit_in_the_info_view() {
    let (mut h, id, folder) = harness("info");
    h.shell.run("library.install", Value::Null).unwrap();
    h.shell.wait_packages();
    let file = folder.join("tone-a3/tone-a3.wav");
    h.shell
        .run("browser.open_place", json!({ "label": "core_library" }))
        .unwrap();
    h.shell
        .run(
            "browser.select",
            json!({ "paths": [file.to_string_lossy()] }),
        )
        .unwrap();
    h.settle();
    h.frame(vec![]);
    let content = h.main.info_view_content();
    assert_eq!(content.title, "Tone A3");
    assert!(content.body.contains("by Beatlayer"), "{}", content.body);
    clean(&mut h, &id);
}

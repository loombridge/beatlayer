//! The File Manager (WP-6.63), headless: it opens in the detail area's
//! place from every entry point, and AccessKit reaches its tabs, its
//! sections and its rows.
//!
//! Spec: file-management R10, R21.

use crate::common;
use common::{Harness, Window};
use egui::accesskit::Role;
use serde_json::json;

/// The File Manager's region in the tree, if it is there.
fn file_manager(nodes: &[common::Node]) -> bool {
    nodes
        .iter()
        .any(|node| node.role == Role::Group && node.label == "File Manager")
}

#[test]
fn the_file_manager_opens_in_the_detail_areas_place_from_each_entry_point() {
    // Spec: file-management R10. File ▸ Manage Files, View ▸ File Manager,
    // the Status Bar's missing-files message, and a project folder's
    // Manage Project all show it in the detail area's place.
    let mut h = Harness::new(Window::Main);
    h.settle();
    assert!(!h.shell.file_manager_open(), "it is hidden to begin with");

    // File ▸ Manage Files.
    h.shell
        .run("view.toggle_file_manager", json!({ "open": true }))
        .unwrap();
    let output = h.settle();
    assert!(h.shell.file_manager_open());
    assert!(
        file_manager(&Harness::nodes(&output)),
        "the File Manager is in the tree"
    );

    // The menu items again hide it.
    h.shell.run("view.toggle_file_manager", json!({})).unwrap();
    h.settle();
    assert!(!h.shell.file_manager_open(), "the menu items hide it");

    // View ▸ File Manager shows it again.
    h.shell
        .run("view.toggle_file_manager", json!({ "open": true }))
        .unwrap();
    let output = h.settle();
    assert!(file_manager(&Harness::nodes(&output)));

    // The Status Bar's missing-files message opens it at its Manage Set
    // view.
    h.shell
        .run("view.toggle_file_manager", json!({ "open": false }))
        .unwrap();
    h.settle();
    assert_eq!(h.shell.missing_files(), 0, "nothing is missing");
    h.shell
        .run(
            "view.toggle_file_manager",
            json!({ "view": "set", "open": true }),
        )
        .unwrap();
    h.settle();
    assert!(h.shell.file_manager_open());
}

#[test]
fn accesskit_reaches_the_file_managers_tabs_sections_and_rows() {
    // Spec: file-management R21. Its view buttons are a tab list, each
    // section is a group with its name and count, and each row is a list
    // item naming its file, its location and what uses it.
    let mut h = Harness::new(Window::Main);
    h.shell
        .run("view.toggle_file_manager", json!({ "open": true }))
        .unwrap();
    let output = h.settle();
    let nodes = Harness::nodes(&output);

    let tabs = nodes
        .iter()
        .find(|node| node.role == Role::TabList)
        .expect("the view buttons are a tab list");
    assert_eq!(tabs.label, "File Manager views");

    let sections: Vec<&str> = nodes
        .iter()
        .filter(|node| node.role == Role::Group)
        .map(|node| node.label.as_str())
        .filter(|label| label.starts_with("Missing Files"))
        .collect();
    assert!(
        !sections.is_empty(),
        "the sections are groups with their names and counts: {sections:?}"
    );

    let rows = nodes
        .iter()
        .filter(|node| node.role == Role::ListItem)
        .count();
    assert!(rows > 0, "the rows are list items");
}

#[test]
fn the_file_manager_shows_the_set_s_files() {
    // Spec: file-management R11, R16. The state lists the files with
    // their locations and what uses them.
    let mut h = Harness::new(Window::Main);
    h.shell
        .run("view.toggle_file_manager", json!({ "open": true }))
        .unwrap();
    h.settle();
    let state = h.shell.file_manager_state();
    assert!(state.open, "the File Manager shows");
    assert_eq!(state.scope, bl_shell::file_manager_commands::Scope::Set);
    assert!(
        state
            .sections
            .iter()
            .any(|section| section.name == "Missing Files"),
        "the sections are named: {:?}",
        state.sections
    );
}

/// A folder for one test's files, removed when the test ends.
struct Scratch(std::path::PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "bl-ui-packs-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A file dropped into the window, as the app's drag and drop gives egui.
#[derive(Debug)]
struct Dropped(std::path::PathBuf);

impl egui::DroppedFile for Dropped {
    fn path(&self) -> &std::path::Path {
        &self.0
    }

    fn bytes(&self) -> Result<Vec<u8>, String> {
        std::fs::read(&self.0).map_err(|error| error.to_string())
    }
}

#[test]
fn the_packing_section_holds_create_pack() {
    // Spec: set-exchange R16. Manage Project's Packing section has a Create
    // Pack button, which asks where to save the pack of the project folder.
    let scratch = Scratch::new("create");
    let mut h = Harness::new(Window::Main);
    h.shell
        .run(
            "set.save_as",
            json!({ "path": scratch.0.join("Song.blset").to_string_lossy() }),
        )
        .unwrap();
    h.shell
        .run(
            "view.toggle_file_manager",
            json!({ "view": "project", "open": true }),
        )
        .unwrap();
    let nodes = Harness::nodes(&h.settle());
    let button = Harness::node(&nodes, "Create Pack");
    assert!(!button.disabled);
    let id = button.id;
    h.take_events();
    h.frame(vec![egui::Event::AccessKitActionRequest(
        egui::accesskit::ActionRequest {
            action: egui::accesskit::Action::Click,
            target_tree: egui::accesskit::TreeId::ROOT,
            target_node: id,
            data: None,
        },
    )]);
    h.settle();
    assert!(
        h.events.iter().any(|e| matches!(
            e,
            bl_ui::main_window::UiEvent::Invoke(i) if i.id == "set.create_pack"
        )),
        "the button runs the command"
    );
    assert!(
        matches!(h.effects.as_slice(), [.., bl_shell::Effect::PickPackPath { file_name, .. }]
            if file_name == "Song Project.blpack"),
        "{:?}",
        h.effects
    );
}

#[test]
fn a_pack_dropped_on_the_window_is_installed() {
    // Spec: set-exchange R20 (I-DROP-023). A pack dropped on the main window
    // asks where to unpack it; any other file dropped there is left alone.
    let scratch = Scratch::new("drop");
    let pack = scratch.0.join("Song.blpack");
    std::fs::write(&pack, b"not read until a folder is chosen").unwrap();
    let other = scratch.0.join("notes.txt");
    std::fs::write(&other, b"text").unwrap();
    let mut h = Harness::new(Window::Main);
    h.settle();
    h.dropped.push(std::sync::Arc::new(Dropped(other)));
    h.frame(vec![]);
    assert!(h.effects.is_empty(), "{:?}", h.effects);

    h.dropped.push(std::sync::Arc::new(Dropped(pack.clone())));
    h.frame(vec![]);
    assert_eq!(
        h.effects,
        [bl_shell::Effect::PickInstallFolder {
            pack: pack.to_string_lossy().into_owned()
        }]
    );
}

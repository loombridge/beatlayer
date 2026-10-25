//! DAWproject in the main window (WP-6.37b; the DAWproject spec, R85 and
//! R87): a file dropped on the window, and the report window.

use crate::common::{Harness, Window};
use bl_shell::Effect;
use bl_shell::dawproject_commands::{IMPORT_DAWPROJECT, ImportReport};
use egui::accesskit::Role;
use egui::output::OutputCommand;
use serde_json::json;
use std::path::{Path, PathBuf};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/dawproject")
        .join(name)
}

/// A file dropped into the window, as the app's drag and drop gives egui.
#[derive(Debug)]
struct Dropped(PathBuf);

impl egui::DroppedFile for Dropped {
    fn path(&self) -> &Path {
        &self.0
    }

    fn bytes(&self) -> Result<Vec<u8>, String> {
        std::fs::read(&self.0).map_err(|error| error.to_string())
    }
}

fn drop_file(h: &mut Harness, name: &str) {
    h.dropped.push(std::sync::Arc::new(Dropped(fixture(name))));
    h.frame(Vec::new());
    h.settle();
}

fn import(h: &mut Harness, name: &str) {
    let path = fixture(name);
    h.shell
        .invoke(IMPORT_DAWPROJECT, json!({ "path": path.to_string_lossy() }))
        .unwrap();
    h.settle();
}

// Spec: dawproject R85. A `.dawproject` file dropped on the window imports
// as a new set, and no view places it as a clip.
#[test]
fn a_dropped_dawproject_imports_and_no_view_places_it() {
    let mut h = Harness::with_title_bar(Window::Main);
    drop_file(&mut h, "minimal.dawproject");
    assert_eq!(h.shell.set_name(), "minimal");
    let imported = h.shell.editor().document().clone();
    assert!(!h.shell.unsaved_changes(), "nothing else was added to it");
    assert_eq!(h.shell.editor().document(), &imported);
}

// Spec: dawproject R85, R83. With unsaved changes the drop asks first.
#[test]
fn a_dropped_dawproject_asks_about_unsaved_changes() {
    let mut h = Harness::with_title_bar(Window::Main);
    let id: bl_model::TrackId = h.shell.new_id();
    h.shell
        .run(
            "track.create",
            json!({ "id": id, "kind": "midi", "name": "Added", "index": 0 }),
        )
        .unwrap();
    drop_file(&mut h, "minimal.dawproject");
    assert_ne!(h.shell.set_name(), "minimal");
    let then = h.effects.iter().find_map(|effect| match effect {
        Effect::ConfirmDiscard { then } => Some(then.clone()),
        _ => None,
    });
    let then = then.expect("the person is asked about the unsaved changes");
    assert_eq!(then.id, IMPORT_DAWPROJECT);
}

/// Whether the window's title is drawn.
fn shows_title(nodes: &[crate::common::Node]) -> bool {
    nodes
        .iter()
        .any(|n| n.label == "DAWproject Import" || n.value == "DAWproject Import")
}

/// Presses the button labelled `label`, and returns what the clipboard got.
fn press(h: &mut Harness, label: &str) -> Vec<String> {
    let nodes = Harness::nodes(&h.frame(vec![]));
    let id = Harness::node(&nodes, label).id;
    let copied = |output: &egui::FullOutput| -> Vec<String> {
        output
            .platform_output
            .commands
            .iter()
            .filter_map(|command| match command {
                OutputCommand::CopyText(text) => Some(text.clone()),
                _ => None,
            })
            .collect()
    };
    let request = |h: &mut Harness| {
        h.frame(vec![egui::Event::AccessKitActionRequest(
            egui::accesskit::ActionRequest {
                action: egui::accesskit::Action::Click,
                target_tree: egui::accesskit::TreeId::ROOT,
                target_node: id,
                data: None,
            },
        )])
    };
    let mut out = copied(&request(h));
    for _ in 0..3 {
        if !out.is_empty() {
            break;
        }
        out = copied(&h.frame(vec![]));
    }
    out
}

// Spec: dawproject R87. The window lists a fixture's entries under Left Out
// and Changed as lists, with Copy and Close; Copy puts the report on the
// clipboard as text, and Close closes the window.
#[test]
fn the_report_window_lists_the_entries_and_copy_and_close_work() {
    let mut h = Harness::with_title_bar(Window::Main);
    import(&mut h, "bitwig-shaped.dawproject");
    let report: ImportReport =
        serde_json::from_str(&std::fs::read_to_string(fixture("bitwig-shaped.json")).unwrap())
            .unwrap();
    let nodes = Harness::nodes(&h.frame(vec![]));
    assert!(shows_title(&nodes));
    let lists: Vec<&str> = nodes
        .iter()
        .filter(|n| n.role == Role::List)
        .map(|n| n.label.as_str())
        .collect();
    assert!(lists.contains(&"Changed"), "{lists:?}");
    let items: Vec<&str> = nodes
        .iter()
        .filter(|n| n.role == Role::ListItem)
        .map(|n| n.label.as_str())
        .collect();
    for entry in &report.entries {
        assert!(items.contains(&entry.line().as_str()), "{items:?}");
    }
    for label in ["Copy", "Close"] {
        assert_eq!(Harness::node(&nodes, label).role, Role::Button);
    }
    assert_eq!(press(&mut h, "Copy"), [report.text()]);
    press(&mut h, "Close");
    h.settle();
    assert!(h.shell.dawproject_report_window().is_none());
    let nodes = Harness::nodes(&h.frame(vec![]));
    assert!(!shows_title(&nodes));
}

// Spec: dawproject R87. Esc closes the window.
#[test]
fn escape_closes_the_report_window() {
    let mut h = Harness::with_title_bar(Window::Main);
    import(&mut h, "bitwig-shaped.dawproject");
    assert!(h.shell.dawproject_report_window().is_some());
    h.key(egui::Key::Escape, egui::Modifiers::NONE);
    h.settle();
    assert!(h.shell.dawproject_report_window().is_none());
}

// Spec: dawproject R87. An empty report opens no window.
#[test]
fn an_empty_report_opens_no_window() {
    let mut h = Harness::with_title_bar(Window::Main);
    import(&mut h, "minimal.dawproject");
    let nodes = Harness::nodes(&h.frame(vec![]));
    assert!(!shows_title(&nodes));
    assert!(h.shell.dawproject_report_window().is_none());
}

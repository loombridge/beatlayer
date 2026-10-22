//! The recovery prompt and the Save Continuously switch (WP-6.62; the
//! crash-recovery spec, R22 to R25): the prompt lists the sets a crash
//! left, newest first, and Recover opens one while Later keeps the rest for
//! the next launch; the switch on File & Folder turns Save Continuously on
//! and `settings.state` reads it.

mod common;

use std::path::PathBuf;

use bl_project::Project;
use bl_project::autosave::Autosave;
use common::{Harness, Window};
use egui::accesskit::{Action, Role};
use egui::{Id, Key, Modifiers};
use serde_json::json;

/// A folder for one test's files, removed when the test ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "bl-ui-recovery-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> PathBuf {
        self.0.join("Recovery")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A session that stopped without closing its set, so the prompt offers it.
/// The set is saved under `name`, so the prompt can name it.
fn crashed(dir: &PathBuf, name: &str, label: &str) {
    let path = dir
        .join(format!("{name} Project"))
        .join(format!("{name}.blset"));
    let mut file = bl_project::SetFile::new(&path);
    file.save(&Project::new()).unwrap();
    let (file, _) = bl_project::SetFile::open(&path).unwrap();
    let mut autosave = Autosave::start(dir).unwrap();
    autosave.begin(Some(&file)).unwrap();
    autosave.edited(&Project::new(), label);
    autosave.flush().unwrap();
    // The session ends as a crash would: its lock goes with it.
    drop(autosave);
}

/// The labels of the nodes in a frame's output, in the order they come.
fn labels(h: &mut Harness) -> Vec<String> {
    Harness::nodes(&h.settle())
        .iter()
        .map(|node| node.label.clone())
        .collect()
}

// Spec: crash-recovery R22, R23.
#[test]
fn the_recovery_prompt_lists_the_crashed_sets_newest_first() {
    let scratch = Scratch::new("prompt");
    let recovery = scratch.path();
    crashed(&recovery, "First", "First Edit");
    crashed(&recovery, "Second", "Second Edit");
    let mut h = Harness::with_recovery(Window::Main, recovery.clone());
    let shown = labels(&mut h);
    assert!(
        shown.iter().any(|label| label == "Recover Unsaved Sets"),
        "the prompt is there: {shown:?}"
    );
    // The newest set is first: it is drawn above the older one.
    let nodes = Harness::nodes(&h.settle());
    let at = |name: &str| {
        nodes
            .iter()
            .find(|node| node.label == name)
            .unwrap_or_else(|| panic!("{name} is listed"))
            .bounds
            .expect("the set is drawn")
            .y0
    };
    assert!(
        at("Second") < at("First"),
        "the newest set comes first: {:?} {:?}",
        at("Second"),
        at("First")
    );
    // Each set has Recover and Discard, and the prompt has Later.
    for button in ["Recover", "Discard", "Later"] {
        assert!(
            shown.iter().any(|label| label == button),
            "the prompt has {button}: {shown:?}"
        );
    }
    // The dialog reaches assistive technology as a dialog.
    let nodes = Harness::nodes(&h.settle());
    assert!(
        nodes.iter().any(|node| node.role == Role::Dialog),
        "the prompt is a dialog: {nodes:?}"
    );
}

// Spec: crash-recovery R22, R23.
#[test]
fn recovering_from_the_prompt_opens_the_set() {
    let scratch = Scratch::new("recover");
    let recovery = scratch.path();
    crashed(&recovery, "First", "First Edit");
    crashed(&recovery, "Second", "Second Edit");
    let mut h = Harness::with_recovery(Window::Main, recovery.clone());
    h.settle();
    // Recover the newest set: the first Recover button is its.
    let recover = Harness::nodes(&h.settle())
        .into_iter()
        .find(|node| node.label == "Recover")
        .expect("the prompt has a Recover button");
    let bounds = recover.bounds.expect("the button is drawn");
    h.click(egui::pos2(
        ((bounds.x0 + bounds.x1) / 2.0) as f32,
        ((bounds.y0 + bounds.y1) / 2.0) as f32,
    ));
    h.settle();
    // The set is open, and the prompt has the other one left.
    assert_eq!(h.shell.recovery_offers().len(), 1);
    assert!(h.shell.is_dirty(), "the recovered set is unsaved");
}

// Spec: crash-recovery R22, R23.
#[test]
fn esc_is_later_and_keeps_the_set_for_the_next_launch() {
    let scratch = Scratch::new("later");
    let recovery = scratch.path();
    crashed(&recovery, "First", "First Edit");
    crashed(&recovery, "Second", "Second Edit");
    let mut h = Harness::with_recovery(Window::Main, recovery.clone());
    h.settle();
    assert!(h.shell.recovery_prompt_open());
    // Esc does what Later does.
    h.key(Key::Escape, Modifiers::NONE);
    h.settle();
    assert!(!h.shell.recovery_prompt_open(), "the prompt closed");
    // Every set is still offered, for the next launch.
    assert_eq!(h.shell.recovery_offers().len(), 2);
    drop(h);
    let next = Harness::with_recovery(Window::Main, recovery);
    assert_eq!(next.shell.recovery_offers().len(), 2);
    assert!(next.shell.recovery_prompt_open());
}

// Spec: crash-recovery R24, R25.
#[test]
fn the_file_and_folder_switch_turns_save_continuously_on() {
    let scratch = Scratch::new("switch");
    let recovery = scratch.path();
    let mut h = Harness::with_recovery(Window::Settings, recovery);
    h.shell
        .invoke("app.settings", json!({ "page": "file-folder" }))
        .unwrap();
    h.shell.take_effects();
    h.settings.refresh();
    h.settle();
    // The switch is off in a new install.
    let state = h
        .shell
        .run("settings.state", json!({}))
        .expect("the settings are read");
    assert_eq!(state["file_folder"]["save_continuously"], false);
    // The switch turns it on.
    h.accesskit(Id::new(("switch", "save-continuously")), Action::Click);
    h.settle();
    let state = h
        .shell
        .run("settings.state", json!({}))
        .expect("the settings are read");
    assert_eq!(state["file_folder"]["save_continuously"], true);
    assert!(h.shell.save_continuously());
    // The switch is read by assistive technology.
    let nodes = Harness::nodes(&h.settle());
    let switch = nodes
        .iter()
        .find(|node| node.label == "Save Continuously")
        .expect("the switch is named");
    assert_eq!(switch.toggled, Some(true));
}

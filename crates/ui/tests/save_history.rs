//! The Save History panel (IMP-05, WP-6.39; the crash-recovery spec, R12 to
//! R19), headless: File ▸ Save History opens it in the Undo History's place,
//! its rows reach assistive technology, and Restore, a double-click and
//! Return restore a row.

mod common;

use std::path::PathBuf;

use bl_project::{Project, SetFile};
use bl_shell::menus::{Action, MenuPlatform, menu_bar};
use common::{Harness, Window};
use egui::accesskit::Role;
use egui::{Key, Modifiers, Pos2};
use serde_json::json;

/// A folder for one test's files, removed when the test ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "bl-ui-save-history-{name}-{}-{}",
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

/// A harness with a saved set open, Save Continuously `continuous`, and
/// three edits made, each on disk, and the song after each.
fn harness(scratch: &Scratch, continuous: bool) -> (Harness, Vec<bl_model::Document>) {
    let set = scratch.0.join("Song Project/Song.blset");
    let mut file = SetFile::new(&set);
    file.save(&Project::new()).unwrap();
    file.save(&Project::new()).unwrap();
    let mut h = Harness::with_recovery(Window::Main, scratch.0.join("Recovery"));
    h.shell
        .run("set.open", json!({ "path": set.to_string_lossy() }))
        .unwrap();
    h.shell
        .run("settings.set_continuous_save", json!({ "on": continuous }))
        .unwrap();
    let mut songs = Vec::new();
    for name in ["One", "Two", "Three"] {
        h.shell
            .invoke("track.create", json!({ "kind": "midi", "name": name }))
            .unwrap();
        h.shell.flush_recovery().unwrap();
        songs.push(h.shell.editor().document().clone());
    }
    h.settle();
    (h, songs)
}

/// What the app does between frames: the journal reaches the disk, and the
/// shell takes the news.
fn refresh(h: &mut Harness) {
    h.shell.flush_recovery().unwrap();
    h.shell.poll();
    h.settle();
}

fn open(h: &mut Harness) {
    h.shell
        .invoke("view.toggle_save_history", json!({}))
        .unwrap();
    refresh(h);
}

/// The panel's rows, from the top of the list.
fn rows(h: &mut Harness) -> Vec<common::Node> {
    let nodes = Harness::nodes(&h.frame(vec![]));
    let list = nodes
        .iter()
        .find(|n| n.role == Role::ListBox)
        .expect("a list box");
    let mut rows: Vec<common::Node> = nodes
        .iter()
        .filter(|n| n.role == Role::ListItem && Harness::descends_from(&nodes, n.id, list.id))
        .cloned()
        .collect();
    rows.sort_by(|a, b| a.bounds.unwrap().y0.total_cmp(&b.bounds.unwrap().y0));
    rows
}

fn middle(node: &common::Node) -> Pos2 {
    let b = node.bounds.unwrap();
    Pos2::new(((b.x0 + b.x1) / 2.0) as f32, ((b.y0 + b.y1) / 2.0) as f32)
}

fn undo_step(h: &Harness) -> Option<String> {
    h.shell.editor().undo_history().next().map(str::to_owned)
}

// Spec: crash-recovery R12.
#[test]
fn the_item_follows_open_set_backups_and_the_panel_opens_and_closes() {
    let mut rows_in_order = Vec::new();
    for menu in menu_bar(MenuPlatform::current()) {
        for item in menu.walk() {
            rows_in_order.push((item.row, item.action));
        }
    }
    let at = rows_in_order
        .iter()
        .position(|(row, _)| *row == "M-FILE-008")
        .unwrap();
    assert_eq!(rows_in_order[at + 1].0, "IMP-05");
    let Action::Command(id) = &rows_in_order[at + 1].1 else {
        panic!("File ▸ Save History runs a command")
    };
    assert_eq!(*id, "view.toggle_save_history");

    let scratch = Scratch::new("open");
    let (mut h, _) = harness(&scratch, true);
    assert_eq!(h.main.drawn_layout().unwrap().undo_history, None);
    h.shell.invoke(id, json!({})).unwrap();
    h.settle();
    assert!(h.shell.save_history_open());
    let panel = h.main.drawn_layout().unwrap().undo_history;
    assert!(panel.is_some(), "it opens in the Undo History's place");
    assert_eq!(panel.unwrap().width(), 295.0);

    // The close button hides it again.
    let nodes = Harness::nodes(&h.frame(vec![]));
    let close = Harness::node(&nodes, "Close Save History").id;
    h.frame(vec![egui::Event::AccessKitActionRequest(
        egui::accesskit::ActionRequest {
            action: egui::accesskit::Action::Click,
            target_tree: egui::accesskit::TreeId::ROOT,
            target_node: close,
            data: None,
        },
    )]);
    h.settle();
    assert!(!h.shell.save_history_open());
    assert_eq!(h.main.drawn_layout().unwrap().undo_history, None);
}

// Spec: crash-recovery R13, R19.
#[test]
fn the_rows_reach_assistive_technology_newest_first_with_their_times() {
    let scratch = Scratch::new("tree");
    let (mut h, _) = harness(&scratch, true);
    open(&mut h);
    let nodes = Harness::nodes(&h.frame(vec![]));
    let region = Harness::node(&nodes, "Save History");
    assert_eq!(region.role, Role::Group);
    let list = nodes.iter().find(|n| n.role == Role::ListBox).unwrap();
    assert!(Harness::descends_from(&nodes, list.id, region.id));
    let button = Harness::node(&nodes, "Restore");
    assert_eq!(button.role, Role::Button);
    assert!(button.disabled, "nothing is selected yet");

    let rows = rows(&mut h);
    let labels: Vec<&str> = rows.iter().map(|row| row.label.as_str()).collect();
    assert_eq!(
        labels,
        [
            "Create track",
            "Create track",
            "Create track",
            "Saved",
            "Saved"
        ],
        "three save points, the saved version and a backup"
    );
    // A save point's time has seconds; a saved version's does not.
    assert_eq!(rows[0].description.matches(':').count(), 2, "{:?}", rows[0]);
    assert_eq!(rows[3].description.matches(':').count(), 1, "{:?}", rows[3]);
}

// Spec: crash-recovery R16.
#[test]
fn restore_and_a_double_click_each_restore_the_row() {
    let scratch = Scratch::new("restore");
    let (mut h, songs) = harness(&scratch, true);
    open(&mut h);

    // Selecting the third row, the first save point, and pressing Restore.
    let target = middle(&rows(&mut h)[2]);
    h.click(target);
    h.settle();
    let nodes = Harness::nodes(&h.frame(vec![]));
    let restore = Harness::node(&nodes, "Restore");
    assert!(!restore.disabled, "a row is selected");
    h.click(middle(restore));
    h.settle();
    assert_eq!(*h.shell.editor().document(), songs[0]);
    assert_eq!(undo_step(&h).as_deref(), Some("Restore Save Point"));

    // Double-clicking a row restores it. The restore is the top row now,
    // then the third save point, so the second is the third row.
    refresh(&mut h);
    let target = middle(&rows(&mut h)[2]);
    // Long enough after the clicks above that this isn't a triple click.
    h.time += 1.0;
    h.frame(vec![egui::Event::PointerMoved(target)]);
    for _ in 0..2 {
        for pressed in [true, false] {
            h.frame(vec![egui::Event::PointerButton {
                pos: target,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Modifiers::NONE,
            }]);
        }
    }
    h.settle();
    assert_eq!(*h.shell.editor().document(), songs[1]);
}

// Spec: crash-recovery R14.
#[test]
fn with_save_continuously_off_a_line_and_a_button_lead_to_settings() {
    let scratch = Scratch::new("off");
    let (mut h, _) = harness(&scratch, false);
    open(&mut h);
    let rows = rows(&mut h);
    assert_eq!(rows.len(), 2, "only the saved versions");
    let nodes = Harness::nodes(&h.frame(vec![]));
    let settings = Harness::node(&nodes, "Settings");
    h.click(middle(settings));
    h.settle();
    assert!(h.shell.settings_open(), "the button opens Settings");
    assert_eq!(
        h.shell.preferences().settings_page,
        bl_shell::settings::SettingsPage::FileFolder
    );
}

// Spec: crash-recovery R19.
#[test]
fn return_restores_the_selected_row_from_the_keyboard() {
    let scratch = Scratch::new("keys");
    let (mut h, _) = harness(&scratch, true);
    open(&mut h);
    // Down selects the first row, Down again the second; Return restores it.
    h.key(Key::ArrowDown, Modifiers::NONE);
    h.key(Key::ArrowDown, Modifiers::NONE);
    h.settle();
    let steps = h.shell.editor().undo_history().count();
    h.key(Key::Enter, Modifiers::NONE);
    h.settle();
    assert_eq!(h.shell.editor().undo_history().count(), steps + 1);
    assert_eq!(undo_step(&h).as_deref(), Some("Restore Save Point"));
}

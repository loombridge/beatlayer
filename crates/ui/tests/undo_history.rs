//! The Undo History panel (WP-6.28), headless: it opens and closes by
//! key, menu, and close button, it sits in the right-hand column beside
//! the assistant's pane, clicking a row returns the set to that point,
//! the arrow keys move a highlight and Enter jumps to it, and the
//! accessibility tree holds the list and its rows.

mod common;

use bl_shell::Shell;
use bl_shell::menus::{Action, MenuPlatform, menu_bar};
use bl_ui::layout::ShellLayout;
use common::{Harness, Window};
use egui::accesskit::Role;
use egui::{Key, Modifiers, Pos2};
use serde_json::json;

/// The Undo History's key: Cmd+Option+Z on macOS, Ctrl+Alt+Z on Windows
/// and Linux (S-VIEW-021).
fn history_key() -> Modifiers {
    if cfg!(target_os = "macos") {
        Modifiers::COMMAND | Modifiers::MAC_CMD | Modifiers::ALT
    } else {
        Modifiers::CTRL | Modifiers::ALT
    }
}

/// A shell with a MIDI track and two edits, so the history has rows.
fn shell() -> Shell {
    let mut shell = Shell::headless();
    let track: bl_model::TrackId = shell.new_id();
    shell
        .run(
            "track.create",
            json!({ "id": track, "kind": "midi", "name": "Keys", "index": 0 }),
        )
        .unwrap();
    shell
        .run("mixer.set_volume", json!({ "track": track, "volume": 0.5 }))
        .unwrap();
    shell
}

/// The panel's rectangle, as the last frame drew it.
fn drawn(h: &Harness) -> Option<egui::Rect> {
    h.main.drawn_layout().expect("a drawn layout").undo_history
}

#[test]
fn the_panel_opens_by_its_key_and_closes_by_it_again() {
    // Spec: undo-history R1. Cmd+Option+Z shows the panel and hides it.
    let mut h = Harness::new(Window::Main);
    h.settle();
    assert_eq!(drawn(&h), None, "a new session starts hidden");

    h.key(Key::Z, history_key());
    h.settle();
    assert!(drawn(&h).is_some(), "the key shows the panel");
    assert!(h.shell.undo_history_open());

    h.key(Key::Z, history_key());
    h.settle();
    assert_eq!(drawn(&h), None, "the key hides it again");
    assert!(!h.shell.undo_history_open());
}

#[test]
fn the_menu_item_shows_the_panel_and_the_close_button_hides_it() {
    // Spec: undo-history R1. View ▸ Undo History runs the same command,
    // and the close button at the left of the header hides the panel.
    let mut action = None;
    for menu in menu_bar(MenuPlatform::current()) {
        for item in menu.walk() {
            if item.row == "M-VIEW-008" {
                action = Some(item.action);
            }
        }
    }
    let Some(Action::Command(id)) = action else {
        panic!("View ▸ Undo History runs a command")
    };
    assert_eq!(id, "view.toggle_undo_history");

    let mut h = Harness::new(Window::Main);
    h.settle();
    h.shell.invoke(id, json!({})).unwrap();
    h.settle();
    assert!(h.shell.undo_history_open());
    assert!(drawn(&h).is_some());

    // The close button at the left of the header.
    let nodes = Harness::nodes(&h.frame(vec![]));
    let close = Harness::node(&nodes, "Close Undo History").id;
    h.frame(vec![egui::Event::AccessKitActionRequest(
        egui::accesskit::ActionRequest {
            action: egui::accesskit::Action::Click,
            target_tree: egui::accesskit::TreeId::ROOT,
            target_node: close,
            data: None,
        },
    )]);
    h.settle();
    assert!(!h.shell.undo_history_open());
    assert_eq!(drawn(&h), None);
}

#[test]
fn the_panel_sits_in_the_right_hand_column() {
    // Spec: undo-history R1, R2. The panel is the Learn View's column,
    // 295 points wide, and sits immediately left of the assistant's pane.
    let mut h = Harness::new(Window::Main);
    h.shell = shell();
    h.settle();
    h.shell
        .invoke("view.toggle_undo_history", json!({}))
        .unwrap();
    h.settle();

    let layout = h.main.drawn_layout().unwrap();
    let panel = layout.undo_history.expect("the panel shows");
    // The Learn View's column: 1296 to 1591 at 1600 points wide.
    assert_eq!(panel.left(), 1296.0);
    assert_eq!(panel.right(), 1591.0);
    assert_eq!(panel.width(), 295.0);
    assert_eq!(panel.top(), layout.main_view.top());
    assert_eq!(panel.bottom(), layout.content.bottom());
    // The main view, the detail area, and the Status Bar stop 9 points
    // left of it.
    assert_eq!(layout.main_view.right(), panel.left() - 9.0);
    assert_eq!(layout.detail.right(), panel.left() - 9.0);
    assert_eq!(layout.status_bar.right(), panel.left());

    // With the assistant's pane docked, the panel is immediately left of
    // it, 9 points apart, and the pane keeps the window's right edge.
    h.shell.invoke("view.toggle_assistant", json!({})).unwrap();
    h.settle();
    let layout = h.main.drawn_layout().unwrap();
    let panel = layout.undo_history.expect("the panel shows");
    let pane = layout.assistant.expect("the pane shows");
    assert_eq!(pane.right(), 1591.0);
    assert_eq!(panel.right(), pane.left() - 9.0);
    assert_eq!(layout.main_view.right(), panel.left() - 9.0);
    assert_eq!(layout.status_bar.right(), panel.left());
}

#[test]
fn the_close_button_takes_a_click() {
    let mut h = Harness::new(Window::Main);
    h.shell = shell();
    h.settle();
    h.shell
        .invoke("view.toggle_undo_history", json!({}))
        .unwrap();
    h.settle();
    let nodes = Harness::nodes(&h.frame(vec![]));
    let close = Harness::node(&nodes, "Close Undo History").bounds.unwrap();
    h.click(Pos2::new(
        ((close.x0 + close.x1) / 2.0) as f32,
        ((close.y0 + close.y1) / 2.0) as f32,
    ));
    h.settle();
    assert!(!h.shell.undo_history_open(), "the click closed the panel");
}

#[test]
fn clicking_a_row_returns_the_set_to_that_point() {
    // Spec: undo-history R6. Clicking a row undoes the steps above it.
    let mut h = Harness::new(Window::Main);
    h.shell = shell();
    h.settle();
    h.shell
        .invoke("view.toggle_undo_history", json!({}))
        .unwrap();
    h.settle();
    assert_eq!(h.shell.editor().current_index(), 2);

    // The rows from the top: the newest step, the one before it, and the
    // set's start. Clicking the second row returns the set to the step
    // before the newest one.
    let nodes = Harness::nodes(&h.frame(vec![]));
    let row = Harness::node(&nodes, "Create track").bounds.unwrap();
    let at = Pos2::new(
        ((row.x0 + row.x1) / 2.0) as f32,
        ((row.y0 + row.y1) / 2.0) as f32,
    );
    h.click(at);
    h.settle();

    // The newest step is undone, and the set is one step back.
    assert_eq!(h.shell.editor().current_index(), 1);
    assert!(h.shell.editor().can_redo());
}

#[test]
fn the_keys_move_the_highlight_and_enter_jumps_to_it() {
    // Spec: undo-history R8. Up and Down move a highlight, and Enter
    // jumps to the highlighted row as a click does.
    let mut h = Harness::new(Window::Main);
    h.shell = shell();
    h.settle();
    h.shell
        .invoke("view.toggle_undo_history", json!({}))
        .unwrap();
    h.settle();
    assert_eq!(h.shell.editor().current_index(), 2);

    // The panel has the focus when it opens, with the current row
    // highlighted. The rows run newest first, so the current row is at
    // the top: Down moves the highlight to the row below it.
    h.key(Key::ArrowDown, Modifiers::NONE);
    h.settle();
    assert_eq!(
        h.shell.editor().current_index(),
        2,
        "the set hasn't changed"
    );

    // Enter jumps to the highlighted row.
    h.key(Key::Enter, Modifiers::NONE);
    h.settle();
    assert_eq!(h.shell.editor().current_index(), 1);

    // And Up moves the highlight back, and Enter jumps forward again.
    h.key(Key::ArrowUp, Modifiers::NONE);
    h.settle();
    h.key(Key::Enter, Modifiers::NONE);
    h.settle();
    assert_eq!(h.shell.editor().current_index(), 2);
}

#[test]
fn the_accessibility_tree_holds_the_list_and_its_rows() {
    // Spec: undo-history R9, R12. The panel is a region named "Undo
    // History" with a list box in it; each row is a list item named by
    // its label, greyed rows are reported as undone, and an agent's row
    // says who made it.
    let mut h = Harness::new(Window::Main);
    let mut shell = shell();
    let track = shell.editor().document().top_level().next().unwrap();
    shell
        .run_labelled(
            bl_model::Maker::Agent("editor".to_owned()),
            "editor: Mute track",
            "mixer.set_mute",
            json!({ "track": track, "mute": true }),
        )
        .unwrap();
    h.shell = shell;
    h.settle();
    h.shell
        .invoke("view.toggle_undo_history", json!({}))
        .unwrap();
    h.settle();

    let nodes = Harness::nodes(&h.frame(vec![]));
    let region = Harness::node(&nodes, "Undo History");
    assert_eq!(region.role, Role::Group);
    let list = nodes
        .iter()
        .find(|n| n.role == Role::ListBox)
        .expect("a list box");
    assert!(Harness::descends_from(&nodes, list.id, region.id));

    // The rows, from the top: the agent's step, the person's steps, and
    // the set's start. The agent's step is the newest, so it is the
    // current row.
    let rows = panel_rows(&nodes, list.id);
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[0].label, "editor: Mute track");
    assert_eq!(rows[0].description, "Made by editor");
    assert_eq!(rows[0].selected, Some(true), "the current row is selected");
    assert_eq!(rows[1].label, "Change volume");
    assert_eq!(rows[1].selected, Some(false));
    assert_eq!(rows[1].description, "");
    assert_eq!(rows[3].label, "New Beatlayer Set");

    // A greyed row is reported as undone: the jump to index 1 undoes the
    // two steps above it.
    h.shell
        .run("edit.go_to_step", json!({ "index": 1 }))
        .unwrap();
    h.settle();
    let nodes = Harness::nodes(&h.frame(vec![]));
    let list = nodes
        .iter()
        .find(|n| n.role == Role::ListBox)
        .expect("a list box");
    let rows = panel_rows(&nodes, list.id);
    assert_eq!(rows[0].state, "undone");
    assert_eq!(rows[0].description, "Made by editor");
    assert_eq!(rows[1].state, "undone");
    assert_eq!(rows[2].state, "");
    assert_eq!(rows[2].selected, Some(true), "the current row moved");
}

/// The panel's rows, from the top of the list.
fn panel_rows(nodes: &[common::Node], list: egui::accesskit::NodeId) -> Vec<&common::Node> {
    let mut rows: Vec<&common::Node> = nodes
        .iter()
        .filter(|n| n.role == Role::ListItem && Harness::descends_from(nodes, n.id, list))
        .collect();
    rows.sort_by(|a, b| {
        let top = |row: &common::Node| row.bounds.map_or(0.0, |b| b.y0);
        top(a).total_cmp(&top(b))
    });
    rows
}

#[test]
fn the_layout_places_the_panel_where_the_reference_says() {
    // The layout layer places the panel as a row of the UI parity
    // reference (L-OTH-001).
    let layout =
        ShellLayout::compute_with_panes(egui::vec2(1600.0, 977.0), false, false, None, Some(295.0));
    let panel = layout.undo_history.expect("the panel shows");
    assert_eq!(panel.left(), 1296.0);
    assert_eq!(panel.right(), 1591.0);
    assert!(layout.rows().iter().any(|row| row.id == "L-OTH-001"));
}

#[test]
fn the_panel_scales_with_the_zoom() {
    // Spec: undo-history R11. The panel is laid out in layout points, which
    // the zoom draws as that many window points times the zoom: at 150% the
    // tree is scaled by 1.5 and the panel and its rows keep their layout
    // size.
    let mut h = Harness::new(Window::Main);
    h.shell = shell();
    h.shell
        .run("view.set_zoom", json!({ "percent": 150 }))
        .unwrap();
    h.size = egui::vec2(2400.0, 1500.0);
    h.shell
        .invoke("view.toggle_undo_history", json!({}))
        .unwrap();
    let output = h.settle();
    assert!((output.pixels_per_point - 1.5).abs() < 1e-6);

    let nodes = Harness::nodes(&output);
    let region = Harness::node(&nodes, "Undo History").bounds.unwrap();
    assert_eq!(region.x1 - region.x0, 295.0);
    let list = nodes
        .iter()
        .find(|n| n.role == Role::ListBox)
        .expect("a list box");
    let rows = panel_rows(&nodes, list.id);
    let row = rows[0].bounds.unwrap();
    assert_eq!(row.y1 - row.y0, 18.0);
}

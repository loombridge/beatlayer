//! The small view options (WP-6.49), headless: the mark on empty tracks in
//! the Arrangement, the Session View and the mixers, and the Settings
//! switch for the wheel. Note colours and the wheel on controls are tested
//! with the editor and the controls they change.
//!
//! Spec: view-options R6 to R10, R16.

use crate::common;
use bl_model::TrackId;
use bl_shell::settings::zoom_factor;
use bl_shell::{Shell, ZOOM_STEPS};
use bl_ui::layout::{REFERENCE, TITLE_BAR};
use common::{Harness, Node, Window};
use egui::accesskit::Role;
use egui::{pos2, vec2};
use serde_json::json;

fn harness() -> Harness {
    let mut h = Harness::with_title_bar(Window::Main);
    h.settle();
    h
}

fn tracks(shell: &Shell) -> Vec<TrackId> {
    shell.editor().document().top_level().collect()
}

fn nodes(h: &mut Harness) -> Vec<Node> {
    let output = h.settle();
    Harness::nodes(&output)
}

fn set_mark(h: &mut Harness, on: bool) {
    h.shell
        .run("view.set_mark_empty_tracks", json!({ "on": on }))
        .unwrap();
}

/// The Arrangement's track headers that read as empty.
fn empty_headers(nodes: &[Node]) -> Vec<String> {
    let mut found: Vec<String> = nodes
        .iter()
        .filter(|n| n.role == Role::RowHeader && n.description.ends_with(", empty"))
        .map(|n| n.label.clone())
        .collect();
    found.sort();
    found
}

/// How many title bars read as empty with nothing else to say.
fn empty_title_bars(nodes: &[Node]) -> usize {
    nodes.iter().filter(|n| n.description == "empty").count()
}

// Spec: view-options R6, R8, R9.
#[test]
fn an_empty_track_wears_the_ring_in_the_arrangement_until_a_clip_or_a_device_is_added() {
    let mut h = harness();
    let t = tracks(&h.shell);
    assert!(
        empty_headers(&nodes(&mut h)).is_empty(),
        "the mark is off in a new install"
    );
    let before = common::stroked_circles(&h.settle()).len();
    set_mark(&mut h, true);
    let output = h.settle();
    let found = empty_headers(&Harness::nodes(&output));
    assert_eq!(
        found,
        [
            "1 MIDI", "2 MIDI", "3 Audio", "4 Audio", "A Return", "B Return"
        ]
    );
    assert!(
        common::stroked_circles(&output).len() >= before + found.len(),
        "a ring for each of them"
    );

    // A clip takes the mark away on the frame it is added.
    let scene = h.shell.editor().document().scenes().next().unwrap().id;
    h.shell
        .run(
            "session.insert_empty_midi_clip",
            json!({ "track": t[0], "scene": scene }),
        )
        .unwrap();
    let found = empty_headers(&Harness::nodes(&h.frame(vec![])));
    assert_eq!(
        found,
        ["2 MIDI", "3 Audio", "4 Audio", "A Return", "B Return"]
    );

    // So does a device.
    let eq = bl_devices::eq_type();
    h.shell
        .run(
            "device.add",
            json!({ "track": t[1], "device": {
                "kind": eq.kind, "name": eq.name, "role": eq.role, "sidechain": eq.sidechain } }),
        )
        .unwrap();
    let found = empty_headers(&Harness::nodes(&h.frame(vec![])));
    assert_eq!(found, ["3 Audio", "4 Audio", "A Return", "B Return"]);

    // Switched off again, no ring.
    set_mark(&mut h, false);
    assert!(empty_headers(&nodes(&mut h)).is_empty());
}

// Spec: view-options R8, R9.
#[test]
fn the_session_view_and_both_mixers_mark_empty_tracks_too() {
    let mut h = harness();
    set_mark(&mut h, true);
    // The Arrangement's mixer has a row of title bars of its own.
    h.shell.run("mixer_view.toggle", json!({})).unwrap();
    let nodes_in_arrangement = nodes(&mut h);
    assert_eq!(
        empty_title_bars(&nodes_in_arrangement),
        6,
        "four tracks and two returns, never Main"
    );
    // The Session View's title bars.
    h.shell
        .run("view.toggle_session_arrangement", json!({}))
        .unwrap();
    let in_session = nodes(&mut h);
    assert_eq!(empty_title_bars(&in_session), 6);
    // A clip in the first track's slot and the mark goes, in every view.
    let t = tracks(&h.shell);
    let scene = h.shell.editor().document().scenes().next().unwrap().id;
    h.shell
        .run(
            "session.insert_empty_midi_clip",
            json!({ "track": t[0], "scene": scene }),
        )
        .unwrap();
    assert_eq!(empty_title_bars(&Harness::nodes(&h.frame(vec![]))), 5);
    h.shell
        .run("view.toggle_session_arrangement", json!({}))
        .unwrap();
    assert_eq!(empty_title_bars(&nodes(&mut h)), 5);
}

// Spec: view-options R16 (interface-zoom R1).
#[test]
fn the_ring_keeps_its_place_in_its_title_bar_at_every_zoom() {
    for percent in ZOOM_STEPS {
        let zoom = zoom_factor(percent);
        let mut h = Harness::with_title_bar(Window::Main);
        h.shell
            .run("view.set_zoom", json!({ "percent": percent }))
            .unwrap();
        set_mark(&mut h, true);
        h.size = vec2(
            REFERENCE.x * zoom,
            (REFERENCE.y - TITLE_BAR) * zoom + TITLE_BAR,
        );
        let output = h.settle();
        let all = Harness::nodes(&output);
        let title = all
            .iter()
            .find(|n| n.role == Role::RowHeader && n.label == "3 Audio")
            .and_then(|n| n.bounds)
            .expect("the title bar");
        let rect = egui::Rect::from_min_max(
            pos2(title.x0 as f32, title.y0 as f32),
            pos2(title.x1 as f32, title.y1 as f32),
        );
        let ring = common::stroked_circles(&output)
            .into_iter()
            .find(|(centre, _)| rect.contains(*centre))
            .unwrap_or_else(|| panic!("{percent}%: a ring in the title bar"));
        let right = rect.right() - ring.0.x;
        assert!(
            (3.0..=8.0).contains(&right) && (2.5..=4.0).contains(&ring.1),
            "{percent}%: {ring:?} in {rect:?}"
        );
    }
}

// Spec: view-options R10.
#[test]
fn the_wheel_switch_is_the_last_row_of_navigation_and_keyboard() {
    let mut h = Harness::new(Window::Settings);
    h.shell
        .invoke("app.settings", json!({ "page": "display-input" }))
        .unwrap();
    h.shell.take_effects();
    h.settings.refresh();
    let all = nodes(&mut h);
    let switch = all
        .iter()
        .find(|n| n.label == "Adjust Values with Mouse Wheel")
        .expect("the row's switch");
    assert_eq!(switch.toggled, Some(false), "off in a new install");
    let wrap = all
        .iter()
        .find(|n| n.label == "Wrap Tab Navigation")
        .expect("the Tab settings");
    assert!(
        switch.bounds.unwrap().y0 > wrap.bounds.unwrap().y0,
        "after the Tab settings"
    );
    h.shell
        .run("settings.set_wheel_adjusts_values", json!({ "on": true }))
        .unwrap();
    let all = nodes(&mut h);
    let switch = all
        .iter()
        .find(|n| n.label == "Adjust Values with Mouse Wheel")
        .unwrap();
    assert_eq!(switch.toggled, Some(true));
}

/// The View menu's item `row`, as the menu shows it now.
fn menu_item(h: &Harness, row: &str) -> bl_ui::menus::ItemState {
    let menus = bl_shell::menus::menu_bar(bl_shell::menus::MenuPlatform::current());
    let item = menus
        .iter()
        .flat_map(bl_shell::menus::Menu::walk)
        .find(|item| item.row == row)
        .unwrap_or_else(|| panic!("the menu has {row}"));
    bl_ui::menus::item_state(
        item,
        &h.shell,
        &h.keymap,
        bl_ui::keymap::Platform::current(),
    )
}

// Spec: view-options R2, R6.
#[test]
fn the_view_menu_offers_both_options_after_the_items_they_follow() {
    let mut h = harness();
    // Their places: Mark Empty Tracks straight after Show Selected Grouped
    // Track, and the Note Colors submenu straight after Fold to Scale.
    let menus = bl_shell::menus::menu_bar(bl_shell::menus::MenuPlatform::current());
    let rows: Vec<String> = menus
        .iter()
        .flat_map(bl_shell::menus::Menu::walk)
        .map(|item| item.row.to_owned())
        .collect();
    let at = |row: &str| rows.iter().position(|r| r == row).unwrap();
    assert_eq!(at("IMP-34"), at("M-VIEW-038") + 1);
    assert_eq!(at("IMP-35"), at("M-VIEW-040") + 1);

    // Mark Empty Tracks is ticked while it's on.
    assert_eq!(menu_item(&h, "IMP-34").checked, Some(false));
    let invocation = menu_item(&h, "IMP-34")
        .invocation
        .expect("it runs a command");
    h.shell.run(&invocation.id, invocation.args).unwrap();
    assert_eq!(menu_item(&h, "IMP-34").checked, Some(true));

    // Note Colors ticks the one in use, By Velocity first.
    let ticked = |h: &Harness| -> Vec<Option<bool>> {
        ["IMP-35a", "IMP-35b", "IMP-35c"]
            .map(|row| menu_item(h, row).checked)
            .to_vec()
    };
    assert_eq!(ticked(&h), [Some(true), Some(false), Some(false)]);
    let invocation = menu_item(&h, "IMP-35c")
        .invocation
        .expect("it runs a command");
    h.shell.run(&invocation.id, invocation.args).unwrap();
    assert_eq!(ticked(&h), [Some(false), Some(false), Some(true)]);
}

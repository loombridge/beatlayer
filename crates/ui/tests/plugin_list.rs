//! The list of the set's plug-ins, in the main window (IMP-29, WP-6.41; the
//! plug-in list spec): the menu item opens it in the detail area's place
//! and closes it again, its rows reach assistive technology, Show Only
//! Missing hides the plug-ins this computer has, and a click on a track's
//! name in a row selects the device.

mod common;

use bl_model::{DeviceId, TrackId};
use bl_ui::main_window::UiEvent;
use common::{Harness, Node, Window};
use egui::Pos2;
use egui::accesskit::Role;
use serde_json::json;

/// The scanner's child process is built beside these tests; a run that
/// didn't build it can't scan, and the plug-in is blocklisted instead.
fn ready(h: &mut Harness) -> bool {
    let scan: bl_shell::plugin_commands::PluginScan = serde_json::from_value(
        h.shell
            .run(
                "plugin.scan",
                json!({ "paths": [bl_test_clap::bundle().to_string_lossy()] }),
            )
            .expect("the scan runs"),
    )
    .expect("the scan's report");
    !scan.plugins.is_empty()
}

/// A harness with the test CLAP plug-in on a track, and the list of the
/// set's plug-ins open in the detail area.
fn with_the_list() -> (Harness, TrackId, DeviceId) {
    let mut h = Harness::with_title_bar(Window::Main);
    let track = h.shell.editor().document().top_level().next().unwrap();
    let loaded = h
        .shell
        .run(
            "plugin.load",
            json!({ "path": bl_test_clap::bundle(), "track": track }),
        )
        .unwrap();
    let device: DeviceId = serde_json::from_value(loaded["device"].clone()).unwrap();
    h.shell.run("view.toggle_plugin_list", json!({})).unwrap();
    h.settle();
    (h, track, device)
}

// Spec: plugin-list R1, R2, R8.
#[test]
fn the_list_opens_in_the_detail_area_and_its_rows_reach_assistive_technology() {
    let (mut h, track, _device) = with_the_list();
    let output = h.settle();
    let nodes = Harness::nodes(&output);
    let table = Harness::node(&nodes, "Plug-Ins");
    assert_eq!(table.role, Role::Table, "the rows are a table");
    // The row for the plug-in, with its cells.
    let row = nodes
        .iter()
        .find(|node| node.role == Role::Row && node.label == bl_test_clap::PLUGIN_NAME)
        .expect("the plug-in's row");
    assert!(
        Harness::descends_from(&nodes, row.id, table.id),
        "the row hangs from the table"
    );
    // The row's cells, in the order they are read.
    let cells: Vec<String> = under(&nodes, row)
        .into_iter()
        .filter(|node| node.role == Role::Cell)
        .map(|node| node.label.clone())
        .collect();
    assert_eq!(
        cells,
        [
            bl_test_clap::PLUGIN_NAME.to_owned(),
            String::new(),
            String::new(),
            String::new(),
            track_name(&h, track),
            "Missing".to_owned(),
        ],
        "the row's cells read in order"
    );
    // The switch and the Export button are buttons.
    let switch = Harness::node(&nodes, "Show Only Missing");
    assert_eq!(switch.role, Role::Switch);
    let export = Harness::node(&nodes, "Export…");
    assert_eq!(export.role, Role::Button);
    // The track's name in the row is a button.
    let name = nodes
        .iter()
        .find(|node| node.role == Role::Button && node.label == track_name(&h, track))
        .expect("the track's name in the row");
    assert!(name.bounds.is_some(), "the name's place");

    // The item closes the view again.
    h.shell.run("view.toggle_plugin_list", json!({})).unwrap();
    let output = h.settle();
    let nodes = Harness::nodes(&output);
    assert!(
        !nodes
            .iter()
            .any(|node| node.role == Role::Table && node.label == "Plug-Ins"),
        "the list is hidden"
    );
}

// Spec: plugin-list R3.
#[test]
fn show_only_missing_hides_the_plug_ins_this_computer_has() {
    let (mut h, _track, _device) = with_the_list();
    // The scanner knows the plug-in, so it isn't missing.
    if !ready(&mut h) {
        eprintln!("beatlayer-scan wasn't built; skipping (build the workspace's bins)");
        return;
    }
    h.settle();
    h.shell
        .run(
            "view.set_plugin_list_only_missing",
            json!({ "only_missing": true }),
        )
        .unwrap();
    let output = h.settle();
    let nodes = Harness::nodes(&output);
    assert!(
        !nodes
            .iter()
            .any(|node| node.label == bl_test_clap::PLUGIN_NAME),
        "the plug-in this computer has is hidden"
    );
    // Turning it off shows the row again.
    h.shell
        .run(
            "view.set_plugin_list_only_missing",
            json!({ "only_missing": false }),
        )
        .unwrap();
    let output = h.settle();
    let nodes = Harness::nodes(&output);
    assert!(
        nodes
            .iter()
            .any(|node| node.label == bl_test_clap::PLUGIN_NAME),
        "the row is back"
    );
}

// Spec: plugin-list R6.
#[test]
fn a_click_on_a_track_name_selects_the_device() {
    let (mut h, track, device) = with_the_list();
    // Nothing is selected to begin with.
    h.shell
        .run("device.select", json!({ "device": null }))
        .unwrap();
    let output = h.settle();
    let nodes = Harness::nodes(&output);
    let name = nodes
        .iter()
        .find(|node| node.role == Role::Button && node.label == track_name(&h, track))
        .expect("the track's name in the row");
    let bounds = name.bounds.expect("the name's place");
    let at = Pos2::new(
        ((bounds.x0 + bounds.x1) / 2.0) as f32,
        ((bounds.y0 + bounds.y1) / 2.0) as f32,
    );
    h.click(at);
    h.settle();
    let state: bl_shell::device_commands::DeviceViewState =
        serde_json::from_value(h.shell.run("device_view.state", json!({})).unwrap()).unwrap();
    assert_eq!(state.selected, Some(device), "the device is selected");
}

// Spec: plugin-list R4.
#[test]
fn the_list_follows_the_set() {
    let (mut h, track, _device) = with_the_list();
    // A track renamed: the row shows the new name, without a click.
    h.shell
        .run("track.rename", json!({ "track": track, "name": "Renamed" }))
        .unwrap();
    let output = h.settle();
    let nodes = Harness::nodes(&output);
    let row = nodes
        .iter()
        .find(|node| node.role == Role::Row && node.label == bl_test_clap::PLUGIN_NAME)
        .expect("the plug-in's row");
    let cells: Vec<String> = under(&nodes, row)
        .into_iter()
        .filter(|node| node.role == Role::Cell)
        .map(|node| node.label.clone())
        .collect();
    assert!(
        cells.contains(&"Renamed".to_owned()),
        "the row shows the new name: {cells:?}"
    );
}

// Spec: plugin-list R9.
#[test]
fn the_list_scales_with_the_interface_zoom() {
    // The list is drawn in layout points, so egui scales it with the zoom,
    // and it keeps its place in the detail area at every zoom.
    let mut first = None;
    for percent in [50, 100, 150, 200] {
        let (mut h, _track, _device) = with_the_list();
        h.shell
            .run("view.set_zoom", json!({ "percent": percent }))
            .unwrap();
        let output = h.settle();
        let zoom = percent as f32 / 100.0;
        assert!(
            (output.pixels_per_point - zoom).abs() < 1e-6,
            "{percent}%: the window scales by the zoom"
        );
        let nodes = Harness::nodes(&output);
        let table = nodes
            .iter()
            .find(|node| node.role == Role::Table && node.label == "Plug-Ins")
            .expect("the list's table");
        let bounds = table.bounds.expect("the table's place");
        let place = (bounds.x0, bounds.y1 - bounds.y0);
        if let Some(first) = first {
            assert_eq!(place, first, "{percent}%: the list keeps its place");
        } else {
            first = Some(place);
        }
    }
}

/// The nodes under `node`, in the order they are read.
fn under<'a>(nodes: &'a [Node], node: &Node) -> Vec<&'a Node> {
    let mut out = Vec::new();
    for child in &node.children {
        if let Some(child) = nodes.iter().find(|node| node.id == *child) {
            out.push(child);
            out.extend(under(nodes, child));
        }
    }
    out
}

/// The track's name, as the list shows it.
fn track_name(h: &Harness, track: TrackId) -> String {
    h.shell
        .editor()
        .document()
        .track(track)
        .unwrap()
        .name()
        .to_owned()
}

/// Whether the harness's last events ran a command.
#[allow(dead_code)]
fn ran(h: &mut Harness, id: &str) -> bool {
    h.take_events()
        .iter()
        .any(|event| matches!(event, UiEvent::Invoke(i) if i.id == id))
}

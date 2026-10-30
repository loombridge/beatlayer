//! The Control Surfaces table in the Settings window, headless (WP-7.5; the
//! control surfaces spec, R1, R2, R11 and R14): six numbered rows with their
//! choosers and Dump, a missing port by name, and Dump enabled only for a
//! surface with a setup and an output.

use crate::common::{Harness, Window};
use bl_control::testing::SETUP;
use bl_midi_io::{Direction, TestBackend};
use bl_shell::settings::SettingsPage;
use egui::Id;
use egui::accesskit::{Action, ActionRequest, Role, TreeId};
use serde_json::json;

fn open() -> (Harness, TestBackend) {
    let mut h = Harness::new(Window::Settings);
    let test = TestBackend::new();
    test.add_port(Direction::Input, "Pads");
    test.add_port(Direction::Output, "Pads Out");
    h.shell.use_midi_backend(Box::new(test.clone()));
    h.shell
        .run("app.settings", json!({ "page": SettingsPage::TempoMidi }))
        .unwrap();
    h.settle();
    (h, test)
}

/// Opens a chooser and picks the choice called `label`.
fn choose(h: &mut Harness, chooser: &str, label: &str) {
    h.accesskit(Id::new(("chooser", chooser)), Action::Click);
    let nodes = Harness::nodes(&h.settle());
    let id = nodes
        .iter()
        .find(|n| n.label == label && n.role == Role::Button)
        .unwrap_or_else(|| panic!("{chooser} offers no {label:?}"))
        .id;
    h.frame(vec![egui::Event::AccessKitActionRequest(ActionRequest {
        action: Action::Click,
        target_tree: TreeId::ROOT,
        target_node: id,
        data: None,
    })]);
    h.settle();
}

// Spec: control-surfaces R1. Six numbered rows, each a Control Surface
// chooser, an Input chooser, an Output chooser and a Dump button, left to
// right, above the MIDI ports table; None in a new install.
#[test]
fn the_table_has_six_rows_of_choosers_and_a_dump_button_above_the_ports() {
    let (mut h, _) = open();
    let nodes = Harness::nodes(&h.settle());
    for n in 1..=6 {
        let bounds = |label: String| Harness::node(&nodes, &label).bounds.unwrap();
        let (surface, input, output, dump) = (
            bounds(format!("Control Surface {n}")),
            bounds(format!("Input {n}")),
            bounds(format!("Output {n}")),
            bounds(format!("Dump {n}")),
        );
        assert!(surface.x1 <= input.x0 && input.x1 <= output.x0 && output.x1 <= dump.x0);
        assert_eq!(
            Harness::node(&nodes, &format!("Control Surface {n}")).value,
            "None"
        );
        assert_eq!(Harness::node(&nodes, &format!("Input {n}")).value, "None");
        assert!(Harness::node(&nodes, &format!("Dump {n}")).disabled);
    }
    let right = Harness::node(&nodes, "Dump 1").bounds.unwrap().x1;
    assert!(
        right <= f64::from(bl_ui::layout::SETTINGS_WINDOW.x),
        "the row fits the window"
    );
    let table_bottom = Harness::node(&nodes, "Dump 6").bounds.unwrap().y1;
    let ports_top = Harness::node(&nodes, "In: Pads Track").bounds.unwrap().y0;
    assert!(
        table_bottom < ports_top,
        "the table comes before the MIDI ports"
    );
}

// Spec: control-surfaces R1, R11, R14. The choosers list None, the surfaces
// and the ports; choosing runs `control_surface.set`, and Dump runs
// `control_surface.dump`, enabled only for the test surface with an output.
#[test]
fn choosing_runs_set_and_dump_runs_dump_enabled_only_with_a_setup_and_an_output() {
    let (mut h, test) = open();
    choose(&mut h, "Control Surface 1", "Test Surface");
    assert_eq!(
        h.shell.control_surface_rows()[0]
            .surface
            .as_ref()
            .unwrap()
            .id,
        "test"
    );
    assert!(
        h.events
            .iter()
            .any(|e| format!("{e:?}").contains("control_surface.set"))
    );
    let nodes = Harness::nodes(&h.settle());
    assert!(
        Harness::node(&nodes, "Dump 1").disabled,
        "it has no output yet"
    );

    choose(&mut h, "Input 1", "Pads");
    choose(&mut h, "Output 1", "Pads Out");
    let nodes = Harness::nodes(&h.settle());
    assert_eq!(Harness::node(&nodes, "Input 1").value, "Pads");
    assert_eq!(Harness::node(&nodes, "Output 1").value, "Pads Out");
    assert!(!Harness::node(&nodes, "Dump 1").disabled);
    assert!(Harness::node(&nodes, "Dump 2").disabled);
    assert_eq!(
        h.shell.control_surface_rows()[0].state,
        bl_shell::control_surface_commands::RowRun::Running
    );

    let dump = Harness::node(&nodes, "Dump 1").bounds.unwrap();
    h.click(egui::pos2(
        ((dump.x0 + dump.x1) / 2.0) as f32,
        ((dump.y0 + dump.y1) / 2.0) as f32,
    ));
    h.settle();
    assert_eq!(test.sent("Pads Out").last().unwrap(), SETUP);
    assert!(
        h.events
            .iter()
            .any(|e| format!("{e:?}").contains("control_surface.dump"))
    );
}

// Spec: control-surfaces R2. A row whose port isn't there keeps its name and
// reads as missing.
#[test]
fn a_missing_port_shows_by_name() {
    let (mut h, test) = open();
    h.shell
        .run(
            "control_surface.set",
            json!({ "row": 2, "surface": "test", "input": "Gone", "output": "Pads Out" }),
        )
        .unwrap();
    let nodes = Harness::nodes(&h.settle());
    assert_eq!(Harness::node(&nodes, "Input 2").value, "Gone (missing)");
    assert_eq!(Harness::node(&nodes, "Output 2").value, "Pads Out");
    // The surface starts when a port of that name appears.
    test.add_port(Direction::Input, "Gone");
    h.shell.refresh_midi();
    let nodes = Harness::nodes(&h.settle());
    assert_eq!(Harness::node(&nodes, "Input 2").value, "Gone");
}

// Spec: supported-controllers R1. The Control Surface chooser lists None,
// then the supported controllers by their product names in alphabetical
// order (the Launchpads so far), and choosing one runs `control_surface.set`
// with its ID.
#[test]
fn the_chooser_lists_the_launchpads_by_their_product_names() {
    let (mut h, _) = open();
    h.accesskit(Id::new(("chooser", "Control Surface 1")), Action::Click);
    let nodes = Harness::nodes(&h.settle());
    let top = |label: &str| {
        nodes
            .iter()
            .find(|n| n.label == label && n.role == Role::Button)
            .unwrap_or_else(|| panic!("the chooser offers no {label:?}"))
            .bounds
            .unwrap()
            .y0
    };
    let order = [
        "None",
        "Launchpad Mini MK3",
        "Launchpad Pro MK3",
        "Launchpad X",
    ]
    .map(top);
    assert!(order.windows(2).all(|w| w[0] < w[1]), "{order:?}");
    let id = nodes
        .iter()
        .find(|n| n.label == "Launchpad X" && n.role == Role::Button)
        .unwrap()
        .id;
    h.frame(vec![egui::Event::AccessKitActionRequest(ActionRequest {
        action: Action::Click,
        target_tree: TreeId::ROOT,
        target_node: id,
        data: None,
    })]);
    h.settle();
    assert_eq!(
        h.shell.control_surface_rows()[0]
            .surface
            .as_ref()
            .unwrap()
            .id,
        "launchpad-x"
    );
}

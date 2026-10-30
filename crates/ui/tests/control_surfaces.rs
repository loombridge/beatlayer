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
// order (the APCs, Launchkeys and Launchpads so far), and choosing one runs
// `control_surface.set` with its ID.
#[test]
fn the_chooser_lists_the_supported_controllers_by_their_product_names() {
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
        "APC mini mk2",
        "APC40 mkII",
        "Launchkey MK3",
        "Launchkey MK4",
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

/// A User Library with two templates, `Zed Pads` naming the port `Pads` and
/// `Beta` naming none, and the Settings opened on Tempo & MIDI.
fn open_with_templates() -> Harness {
    let library = std::env::temp_dir().join(format!("bl-ui-templates-{}", std::process::id()));
    let folder = library.join("Controller Templates");
    std::fs::create_dir_all(&folder).unwrap();
    let write = |file: &str, text: &str| std::fs::write(folder.join(file), text).unwrap();
    write(
        "zed.bltemplate.toml",
        "format = 1\n[surface]\nname = \"Zed Pads\"\nports = [\"Pads\"]\n",
    );
    write(
        "beta.bltemplate.toml",
        "format = 1\n[surface]\nname = \"Beta\"\n",
    );
    let mut h = Harness::new(Window::Settings);
    h.shell.set_user_library(Some(library.clone()));
    let test = TestBackend::new();
    for (direction, name) in [
        (Direction::Input, "Pads"),
        (Direction::Output, "Pads Out"),
        (Direction::Input, "Other"),
    ] {
        test.add_port(direction, name);
    }
    h.shell.use_midi_backend(Box::new(test));
    h.shell
        .run("app.settings", json!({ "page": SettingsPage::TempoMidi }))
        .unwrap();
    h.settle();
    std::fs::remove_dir_all(&library).unwrap();
    h
}

// Spec: control-surfaces R1, R43, R45. The Control Surface chooser lists None,
// the native surfaces, then the templates by name with the one that names the
// ports present first; the ports it names carry a "Matches" note.
#[test]
fn the_chooser_lists_templates_after_the_native_surfaces_and_notes_a_match() {
    let mut h = open_with_templates();
    h.accesskit(Id::new(("chooser", "Control Surface 1")), Action::Click);
    let nodes = Harness::nodes(&h.settle());
    let mut offered: Vec<_> = nodes
        .iter()
        .filter(|n| {
            n.role == Role::Button
                && [
                    "None",
                    "Test Surface",
                    "Test Instant Surface",
                    "Zed Pads",
                    "Beta",
                    "Generic 8 + 8",
                ]
                .contains(&n.label.as_str())
        })
        .filter_map(|n| Some((n.bounds?.y0, n.label.clone())))
        .collect();
    offered.sort_by(|a, b| a.0.total_cmp(&b.0));
    let labels: Vec<_> = offered.into_iter().map(|(_, label)| label).collect();
    assert_eq!(
        labels,
        [
            "None",
            "Test Surface",
            "Test Instant Surface",
            "Zed Pads",
            "Beta",
            "Generic 8 + 8"
        ]
    );

    // The note sits under each port the template names, and no other.
    let notes: Vec<_> = nodes
        .iter()
        .filter(|n| n.label == "Matches Zed Pads")
        .collect();
    assert_eq!(notes.len(), 2, "the input Pads and the output Pads Out");
    let y = |label: &str| Harness::node(&nodes, label).bounds.unwrap().y0;
    let (pads, other) = (y("In: Pads Track"), y("In: Other Track"));
    assert!(notes.iter().all(|n| n.bounds.unwrap().y0 != other));
    assert!(notes.iter().any(|n| n.bounds.unwrap().y0 > pads));
}

// Spec: push-2 R2. A Push 2 that is plugged in fills the first empty row: the
// chooser reads Push 2 with its Live port as input and output; choosing None
// stops it, and it doesn't come back into a row on its own.
#[test]
fn a_push_2_fills_the_first_row_and_none_keeps_it_out() {
    use bl_push::sim::{LIVE, Simulated};

    let mut h = Harness::new(Window::Settings);
    let test = TestBackend::new();
    let mut sim = Simulated::new(&test, &bl_core::clock::TestClock::new());
    h.shell.use_midi_backend(Box::new(test.clone()));
    h.shell.use_push_display(sim.frames().claim());
    h.shell
        .run("app.settings", json!({ "page": SettingsPage::TempoMidi }))
        .unwrap();
    h.settle();

    sim.plug_in();
    h.shell.refresh_midi();
    let nodes = Harness::nodes(&h.settle());
    assert_eq!(Harness::node(&nodes, "Control Surface 1").value, "Push 2");
    assert_eq!(Harness::node(&nodes, "Input 1").value, LIVE);
    assert_eq!(Harness::node(&nodes, "Output 1").value, LIVE);
    assert_eq!(Harness::node(&nodes, "Control Surface 2").value, "None");

    choose(&mut h, "Control Surface 1", "None");
    assert!(
        h.events
            .iter()
            .any(|e| format!("{e:?}").contains("control_surface.set"))
    );
    h.shell.refresh_midi();
    let nodes = Harness::nodes(&h.settle());
    assert_eq!(Harness::node(&nodes, "Control Surface 1").value, "None");
    assert_eq!(Harness::node(&nodes, "Control Surface 2").value, "None");
}

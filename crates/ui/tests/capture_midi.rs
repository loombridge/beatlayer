//! Capture MIDI in the UI (WP-3.14): the Control Bar's button is live and,
//! like Create ▸ Capture MIDI, disabled until notes have been played.
//!
//! The behaviour is in the capture MIDI spec.

mod common;

use bl_ui::layout::Element;
use bl_ui::main_window;
use common::Harness;
use egui::accesskit::Action;
use serde_json::json;

// Spec: capture-midi R1. In a new set there is nothing to capture: the
// button reads as disabled, pressing it runs nothing, and the command is
// refused.
#[test]
fn the_button_and_the_command_are_disabled_with_nothing_played() {
    let mut h = Harness::new(common::Window::Main);
    let output = h.settle();
    assert!(main_window::is_live(Element::CaptureMidi), "it is built");
    let nodes = Harness::nodes(&output);
    let node = Harness::node(&nodes, "Capture MIDI");
    assert!(node.disabled, "nothing to capture");
    assert_eq!(node.value, "nothing to capture");
    assert!(!h.shell.can_invoke("midi.capture", &json!({})));

    let undo = h.shell.editor().undo_history().len();
    h.accesskit(main_window::element_id(Element::CaptureMidi), Action::Click);
    h.settle();
    assert_eq!(h.shell.editor().undo_history().len(), undo, "nothing ran");
}

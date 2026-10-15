//! Automation's Control Bar indicator (WP-2.12): the Re-Enable Automation
//! button lights while a control's automation is overridden, and pressing it
//! puts every control back under its envelope.

mod common;

use common::Harness;
use egui::accesskit::Action;
use serde_json::json;

use bl_ui::layout::Element;
use bl_ui::{control_bar, main_window};

/// The accesskit ID an element's node has.
fn element_id(element: Element) -> egui::Id {
    main_window::element_id(element)
}

// Spec: automation R5
#[test]
fn the_re_enable_automation_button_lights_and_re_enables() {
    let mut h = Harness::new(common::Window::Main);
    let mut output = h.settle();
    assert!(
        !control_bar::automation_overridden(&h.shell),
        "a new set overrides nothing"
    );
    let nodes = Harness::nodes(&output);
    let node = Harness::node(&nodes, "Re-Enable Automation");
    assert!(!node.disabled, "it can be pressed");
    assert!(node.value.contains("every envelope"));

    // A control with an envelope, moved by hand: its automation is off and
    // the button lights.
    let track = h.shell.new_id();
    h.shell
        .run(
            "track.create",
            json!({ "id": track, "kind": "audio", "name": "Tone", "index": 0 }),
        )
        .unwrap();
    let volume = json!({ "type": "mixer", "param": "volume" });
    h.shell
        .run(
            "automation.set_breakpoint",
            json!({ "track": track, "target": volume, "at": 0, "value": 1.0 }),
        )
        .unwrap();
    h.shell
        .run("mixer.set_volume", json!({ "track": track, "volume": 0.5 }))
        .unwrap();
    assert!(control_bar::automation_overridden(&h.shell));
    output = h.settle();
    let nodes = Harness::nodes(&output);
    let node = Harness::node(&nodes, "Re-Enable Automation");
    assert!(node.value.contains("some automation is off"));

    // Pressing it, as a screen reader does, re-enables every envelope.
    h.accesskit(element_id(Element::ReEnableAutomation), Action::Click);
    h.settle();
    assert!(!control_bar::automation_overridden(&h.shell));
    assert!(
        h.shell
            .editor()
            .document()
            .track(track)
            .unwrap()
            .automation_for(&bl_model::AutomationTarget::Mixer {
                param: bl_model::MixerParam::Volume
            })
            .unwrap()
            .enabled,
        "the envelope drives the control again"
    );
}

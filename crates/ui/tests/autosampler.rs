//! The autosampler's dialog (WP-5.16, IMP-27; the multisampler spec, R64,
//! R65): the settings it holds, how many samples they make, and how long
//! the render takes.

mod common;

use bl_model::TrackId;
use common::{Harness, Window};
use serde_json::json;

/// A window with the Device View open on a MIDI track.
fn harness() -> Harness {
    let mut h = Harness::with_title_bar(Window::Main);
    h.settle();
    let tracks: Vec<TrackId> = h.shell.editor().document().top_level().collect();
    h.shell
        .invoke(
            "arrangement.select_tracks",
            json!({ "tracks": [tracks[0]] }),
        )
        .unwrap();
    h.shell.set_device_types(bl_devices::device_types());
    h.settle();
    h
}

/// The polysynth on the selected track.
fn polysynth(h: &mut Harness) -> bl_model::DeviceId {
    let t = bl_devices::polysynth_type();
    let added: bl_shell::device_commands::DeviceAdded = serde_json::from_value(
        h.shell
            .run(
                "device.add",
                json!({ "device": { "kind": t.kind, "name": t.name, "role": t.role } }),
            )
            .unwrap(),
    )
    .unwrap();
    added.device
}

// Spec: multisampler R64, R65
#[test]
fn the_dialog_holds_its_settings_and_counts_the_samples() {
    let mut h = harness();
    let device = polysynth(&mut h);
    h.shell
        .invoke("device.autosample_open", json!({ "device": device }))
        .unwrap();
    h.settle();
    assert!(h.shell.autosampler_dialog().is_some(), "the dialog is open");
    let nodes = Harness::nodes(&h.settle());
    // A widget's text is its label or its value.
    let texts: Vec<&str> = nodes
        .iter()
        .flat_map(|node| [node.label.as_str(), node.value.as_str()])
        .filter(|text| !text.is_empty())
        .collect();
    let label = |text: &str| texts.contains(&text);
    // The settings the dialog holds.
    for setting in [
        "Notes",
        "Step",
        "Velocity Layers",
        "Note Length",
        "Release Tail",
        "Round Robins",
        "Detect Loops",
        "Name",
    ] {
        assert!(label(setting), "the dialog holds {setting}: {texts:?}");
    }
    // The settings are numbers: the notes, the step, the velocity layers,
    // the note length, the tail and the round robins.
    let numbers: Vec<f64> = nodes.iter().filter_map(|node| node.numeric).collect();
    assert!(
        numbers.contains(&36.0) && numbers.contains(&84.0),
        "the notes: {numbers:?}"
    );
    // The samples the settings make, and how long the render takes.
    assert!(
        texts.iter().any(|text| text.contains("samples")),
        "the count and the estimate: {texts:?}"
    );
    // The buttons.
    assert!(label("Autosample"));
    assert!(label("Cancel"));
}

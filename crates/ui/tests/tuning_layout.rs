//! Tunings on plug-ins and MIDI controllers (WP-6.9b; the tunings spec, R20,
//! R22 to R24; C-DEVICE-015, L-MIX-010, L-OTH-009), headless: a plug-in's
//! Apply Tuning to Per-Note Pitch Bend item, the MIDI Controller Layout
//! chooser in a MIDI track's In/Out section, and the Configure MIDI Layout
//! dialog.

use crate::common;
use crate::mpe_settings::{click_labelled, harness, last_step, open_menu};

use bl_model::{DeviceId, LayoutKind, TrackId, Tuning};
use common::Harness;
use serde_json::json;

fn nineteen() -> Tuning {
    let cents = (1..=19).map(|i| 1200.0 * f64::from(i) / 19.0).collect();
    Tuning::new("19-edo".to_owned(), String::new(), cents).unwrap()
}

fn load_tuning(h: &mut Harness) {
    h.shell
        .run(
            "tuning.set",
            json!({ "tuning": nineteen(), "retune": false }),
        )
        .unwrap();
}

fn tuning_item(nodes: &[common::Node]) -> Option<&common::Node> {
    nodes
        .iter()
        .find(|n| n.label.ends_with("Apply Tuning to Per-Note Pitch Bend"))
}

fn switch(h: &Harness, device: DeviceId) -> bool {
    h.shell
        .editor()
        .document()
        .device(device)
        .unwrap()
        .mpe
        .tuning
}

// Spec: tunings R20 (C-DEVICE-015). With no tuning loaded the menu ends at
// MPE Settings….
#[test]
fn a_plugins_menu_has_no_tuning_item_without_a_tuning() {
    let (mut h, device, _) = harness();
    let nodes = open_menu(&mut h, device);
    assert!(nodes.iter().any(|n| n.label == "MPE Settings…"));
    assert!(tuning_item(&nodes).is_none(), "no tuning, no item");
}

// Spec: tunings R20 (C-DEVICE-015). The item shows while the set has a
// tuning, for every plug-in; it is ticked, on by default, and can't be
// chosen while MPE Mode is off.
#[test]
fn a_plugins_menu_ends_with_the_tuning_item_while_a_tuning_is_loaded() {
    let (mut h, device, _) = harness();
    load_tuning(&mut h);
    let nodes = open_menu(&mut h, device);
    let item = tuning_item(&nodes).expect("the item shows under a tuning");
    assert_eq!(
        item.label, "✔ Apply Tuning to Per-Note Pitch Bend",
        "ticked"
    );
    assert!(item.disabled, "MPE Mode is off");
    // It follows MPE Settings… in the menu.
    let at = |label: &str| nodes.iter().position(|n| n.label == label).unwrap();
    assert!(at("MPE Settings…") < at("✔ Apply Tuning to Per-Note Pitch Bend"));

    // With MPE Mode on it can be chosen: one undo step, and the tick goes.
    click_labelled(&mut h, "MPE Mode");
    open_menu(&mut h, device);
    let steps = h.shell.editor().undo_history().len();
    click_labelled(&mut h, "✔ Apply Tuning to Per-Note Pitch Bend");
    assert!(!switch(&h, device));
    assert_eq!(last_step(&h), "Apply Tuning to Per-Note Pitch Bend");
    assert_eq!(h.shell.editor().undo_history().len(), steps + 1);
    let nodes = open_menu(&mut h, device);
    assert_eq!(
        tuning_item(&nodes).unwrap().label,
        "Apply Tuning to Per-Note Pitch Bend"
    );
    click_labelled(&mut h, "Apply Tuning to Per-Note Pitch Bend");
    assert!(switch(&h, device));
}

fn midi_track(h: &Harness) -> TrackId {
    h.shell.editor().document().top_level().next().unwrap()
}

// Spec: tunings R22 (L-MIX-010). The section lists the layouts while a
// tuning is loaded, the chosen one shown; choosing one is one undo step,
// and the chooser is disabled while Bypass Tuning is on.
#[test]
fn the_in_out_section_offers_the_layouts_and_disables_them_while_bypassing() {
    let (mut h, _, _) = harness();
    let track = midi_track(&h);
    let section = |h: &Harness| {
        bl_shell::routing::in_out(h.shell.editor().document(), track, &h.shell.interface())
    };
    let row = |h: &Harness| match section(h) {
        Some(bl_shell::routing::InOut::Clips { tuning_layout, .. }) => tuning_layout,
        other => panic!("{other:?}"),
    };
    assert!(row(&h).is_none(), "no tuning, no chooser");
    load_tuning(&mut h);
    let layouts = row(&h).expect("shown under a tuning");
    assert_eq!(
        layouts.chooser.labels(),
        [
            "All Keys",
            "Black Keys Only",
            "White Keys Only",
            "Closest in Pitch to Keyboard",
            "Custom Controller Layout"
        ]
    );
    assert_eq!(layouts.chooser.text, "All Keys");
    assert!(layouts.configure.is_none(), "… needs Custom");

    let run = layouts.chooser.run_for("Black Keys Only").unwrap().clone();
    h.shell.run(&run.id, run.args).unwrap();
    assert_eq!(last_step(&h), "Change MIDI Controller Layout");
    assert_eq!(
        h.shell
            .editor()
            .document()
            .track(track)
            .unwrap()
            .tuning_layout()
            .layout,
        LayoutKind::BlackKeysOnly
    );
    let layouts = row(&h).unwrap();
    assert_eq!(layouts.chooser.text, "Black Keys Only");
    h.shell
        .run(
            "track.set_tuning_layout",
            json!({ "track": track, "layout": "custom" }),
        )
        .unwrap();
    assert!(row(&h).unwrap().configure.is_some(), "… opens the dialog");

    h.shell
        .run(
            "track.set_tuning_bypass",
            json!({ "track": track, "bypass": true }),
        )
        .unwrap();
    let layouts = row(&h).unwrap();
    assert!(!layouts.chooser.enabled(), "disabled while bypassing");
    assert!(layouts.configure.is_none());
}

/// The nodes of the main window, once it has settled.
fn nodes(h: &mut Harness) -> Vec<common::Node> {
    Harness::nodes(&h.settle())
}

// Spec: tunings R22 (L-MIX-010). The Arrangement's header draws the chooser
// where it is tall enough, and choosing in it runs the command.
#[test]
fn the_headers_chooser_runs_the_command() {
    let (mut h, _, _) = harness();
    let track = midi_track(&h);
    load_tuning(&mut h);
    h.shell
        .run("arrangement.select_tracks", json!({ "tracks": [track] }))
        .unwrap();
    h.shell
        .run(
            "arrangement.set_track_height",
            json!({ "tracks": [track], "height": 220.0 }),
        )
        .unwrap();
    let all = nodes(&mut h);
    let chooser = all
        .iter()
        .find(|n| n.label == "MIDI Controller Layout")
        .expect("the header draws the chooser");
    assert_eq!(chooser.value, "All Keys");
    click_labelled(&mut h, "MIDI Controller Layout");
    click_labelled(&mut h, "White Keys Only");
    assert_eq!(
        h.shell
            .editor()
            .document()
            .track(track)
            .unwrap()
            .tuning_layout()
            .layout,
        LayoutKind::WhiteKeysOnly
    );
}

// Spec: tunings R23 (L-OTH-009). The dialog sets, for each incoming key, the
// key of the tuning it plays, or none, one undo step each.
#[test]
fn the_configure_dialog_sets_each_keys_value_one_undo_step_each() {
    let (mut h, _, _) = harness();
    let track = midi_track(&h);
    load_tuning(&mut h);
    // It opens only for a track that plays the tuning.
    h.shell
        .run("tuning.show_layout", json!({ "track": track }))
        .unwrap();
    let all = nodes(&mut h);
    assert!(all.iter().any(|n| n.label == "Increase the key C3 plays"));
    assert!(all.iter().any(|n| n.label == "OK"));
    let steps = h.shell.editor().undo_history().len();
    click_labelled(&mut h, "Increase the key C-2 plays");
    let layout = h
        .shell
        .editor()
        .document()
        .track(track)
        .unwrap()
        .tuning_layout()
        .clone();
    assert_eq!(layout.custom(0), Some(1));
    assert_eq!(layout.custom(1), Some(1));
    assert_eq!(h.shell.editor().undo_history().len(), steps + 1);
    assert_eq!(last_step(&h), "Change MIDI Controller Layout");
    click_labelled(&mut h, "Play nothing on C-2");
    let layout = h
        .shell
        .editor()
        .document()
        .track(track)
        .unwrap()
        .tuning_layout()
        .clone();
    assert_eq!(layout.custom(0), None);
    assert_eq!(h.shell.editor().undo_history().len(), steps + 2);
    // A key that plays nothing gets its own key back.
    click_labelled(&mut h, "Increase the key C-2 plays");
    let layout = h
        .shell
        .editor()
        .document()
        .track(track)
        .unwrap()
        .tuning_layout()
        .clone();
    assert_eq!(layout.custom(0), Some(0));
    click_labelled(&mut h, "OK");
    assert!(!h.shell.tuning_layout_dialog().open);

    // Bypass Tuning closes it, and it won't open.
    h.shell
        .run("tuning.show_layout", json!({ "track": track }))
        .unwrap();
    h.shell
        .run(
            "track.set_tuning_bypass",
            json!({ "track": track, "bypass": true }),
        )
        .unwrap();
    assert!(!h.shell.tuning_layout_dialog().open);
    assert!(
        h.shell
            .run("tuning.show_layout", json!({ "track": track }))
            .is_err()
    );
}

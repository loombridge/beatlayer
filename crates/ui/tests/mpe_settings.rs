//! A plug-in's MPE Mode and the MPE/Multi-channel Settings dialog (WP-6.17b;
//! the MPE spec, R14, R18, R19; C-DEVICE-015, L-OTH-008), headless: the
//! plug-in's title bar menu lists MPE Mode, ticked while it's on, and MPE
//! Settings…; the dialog's controls run the commands that save the settings,
//! one undo step each, for a plug-in, a track's MIDI To and an External
//! Instrument. The plug-in is the test plug-in, built from source.

use crate::common;
use std::time::{Duration, Instant};

use bl_model::{DeviceId, MidiTo, MpeMode, MpeZone, TrackId};
use bl_shell::plugin_commands::{self as ids, PluginLoaded};
use bl_ui::device_view::{layout, title_rect};
use common::{Harness, Window};
use egui::{Event, Modifiers, PointerButton, Pos2, Rect, pos2};
use serde_json::json;

/// The main window with the test plug-in on the third track.
fn harness() -> (Harness, DeviceId, TrackId) {
    let mut h = Harness::with_title_bar(Window::Main);
    h.settle();
    let track: TrackId = h.shell.editor().document().top_level().nth(2).unwrap();
    h.shell
        .run("arrangement.select_tracks", json!({ "tracks": [track] }))
        .unwrap();
    let loaded: PluginLoaded = serde_json::from_value(
        h.shell
            .run(
                ids::LOAD,
                json!({ "path": bl_test_clap::bundle(), "track": track }),
            )
            .unwrap(),
    )
    .unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    while !h
        .shell
        .plugin_info(loaded.device)
        .is_some_and(|i| i.has_window)
    {
        assert!(
            Instant::now() < until,
            "the plug-in never offered its editor"
        );
        h.shell.poll();
        std::thread::sleep(Duration::from_millis(5));
    }
    h.settle();
    (h, loaded.device, track)
}

fn frame_of(h: &mut Harness, device: DeviceId) -> Rect {
    let nodes = Harness::nodes(&h.settle());
    let b = Harness::node(&nodes, "Device View").bounds.unwrap();
    let view = Rect::from_min_max(
        pos2(b.x0 as f32, b.y0 as f32),
        pos2(b.x1 as f32, b.y1 as f32),
    );
    layout(&h.shell, view)
        .into_iter()
        .find(|(id, _)| *id == device)
        .unwrap()
        .1
}

fn press(at: Pos2, pressed: bool) -> Event {
    Event::PointerButton {
        pos: at,
        button: PointerButton::Secondary,
        pressed,
        modifiers: Modifiers::NONE,
    }
}

/// Opens the plug-in's title bar menu with a right-click.
fn open_menu(h: &mut Harness, device: DeviceId) -> Vec<common::Node> {
    let frame = frame_of(h, device);
    let at = title_rect(frame, false).center();
    h.frame(vec![Event::PointerMoved(at)]);
    h.frame(vec![press(at, true)]);
    h.frame(vec![press(at, false)]);
    Harness::nodes(&h.settle())
}

/// Clicks the node labelled `label` among those drawn now.
fn click_labelled(h: &mut Harness, label: &str) {
    let nodes = Harness::nodes(&h.settle());
    let node = nodes.iter().find(|n| n.label == label).unwrap_or_else(|| {
        panic!(
            "no `{label}` among {:?}",
            nodes.iter().map(|n| &n.label).collect::<Vec<_>>()
        )
    });
    assert!(!node.disabled, "{label} is disabled");
    let b = node.bounds.expect("the control's bounds");
    h.click(pos2(
        ((b.x0 + b.x1) / 2.0) as f32,
        ((b.y0 + b.y1) / 2.0) as f32,
    ));
    h.settle();
}

fn last_step(h: &Harness) -> String {
    h.shell
        .editor()
        .undo_history()
        .next()
        .unwrap_or_default()
        .to_owned()
}

fn mpe(h: &Harness, device: DeviceId) -> bl_model::PluginMpe {
    h.shell.editor().document().device(device).unwrap().mpe
}

// Spec: mpe R14, R18 (C-DEVICE-015)
#[test]
fn a_plugins_menu_lists_mpe_mode_ticked_while_on_and_mpe_settings() {
    let (mut h, device, _) = harness();
    let nodes = open_menu(&mut h, device);
    assert!(nodes.iter().any(|n| n.label == "MPE Mode"));
    assert!(nodes.iter().any(|n| n.label == "MPE Settings…"));
    assert!(!nodes.iter().any(|n| n.label == "✔ MPE Mode"));

    // Choosing MPE Mode switches it on in one undo step, and the menu then
    // ticks it.
    let steps = h.shell.editor().undo_history().len();
    click_labelled(&mut h, "MPE Mode");
    assert!(mpe(&h, device).on);
    assert_eq!(last_step(&h), "Change MPE Mode");
    assert_eq!(h.shell.editor().undo_history().len(), steps + 1);
    let nodes = open_menu(&mut h, device);
    assert!(nodes.iter().any(|n| n.label == "✔ MPE Mode"), "ticked");
    // Choosing it again switches it off.
    click_labelled(&mut h, "✔ MPE Mode");
    assert!(!mpe(&h, device).on);
    assert_eq!(h.shell.editor().undo_history().len(), steps + 2);

    // MPE Settings… opens the dialog on this plug-in.
    open_menu(&mut h, device);
    click_labelled(&mut h, "MPE Settings…");
    let dialog = h.shell.mpe_settings_dialog();
    assert!(dialog.open);
    assert_eq!(dialog.device, Some(device));
    assert_eq!(
        h.shell.editor().undo_history().len(),
        steps + 2,
        "opening it isn't an undo step"
    );
}

// Spec: mpe R18 (L-OTH-008)
#[test]
fn the_dialogs_controls_save_the_zone_and_the_channels_one_undo_step_each() {
    let (mut h, device, _) = harness();
    h.shell
        .run(ids::SHOW_MPE_SETTINGS, json!({ "device": device }))
        .unwrap();
    let nodes = Harness::nodes(&h.settle());
    for label in [
        "Zone",
        "Multi-channel",
        "Lower",
        "Upper",
        "Decrease Note Channels",
        "Increase Note Channels",
        "OK",
    ] {
        assert!(
            nodes.iter().any(|n| n.label == label),
            "the dialog has `{label}`"
        );
    }
    let steps = h.shell.editor().undo_history().len();
    click_labelled(&mut h, "Upper");
    assert_eq!(mpe(&h, device).settings.zone, MpeZone::Upper);
    assert_eq!(last_step(&h), "Change MPE Settings");
    click_labelled(&mut h, "Decrease Note Channels");
    click_labelled(&mut h, "Decrease Note Channels");
    assert_eq!(mpe(&h, device).settings.channels, 13);
    assert_eq!(h.shell.editor().undo_history().len(), steps + 3);

    // Multi-channel mode has its own two channels, the last never below the
    // first.
    click_labelled(&mut h, "Multi-channel");
    assert_eq!(mpe(&h, device).settings.mode, MpeMode::MultiChannel);
    for _ in 0..5 {
        click_labelled(&mut h, "Increase First Channel");
    }
    let settings = mpe(&h, device).settings;
    assert_eq!((settings.first, settings.last), (6, 16));
    for _ in 0..10 {
        click_labelled(&mut h, "Decrease Last Channel");
    }
    let settings = mpe(&h, device).settings;
    assert_eq!((settings.first, settings.last), (6, 6));

    // OK closes it.
    click_labelled(&mut h, "OK");
    assert!(!h.shell.mpe_settings_dialog().open);
}

// Spec: mpe R18, R19
#[test]
fn the_dialog_edits_a_tracks_midi_to_and_an_external_instruments() {
    let (mut h, _, _) = harness();
    let midi: TrackId = h.shell.editor().document().top_level().next().unwrap();
    h.shell
        .run(
            "routing.set_midi_output",
            json!({ "track": midi, "to": { "name": "Synth", "channel": 4, "mpe": {} } }),
        )
        .unwrap();
    h.shell
        .run(ids::SHOW_MPE_SETTINGS, json!({ "track": midi }))
        .unwrap();
    click_labelled(&mut h, "Upper");
    let settings = match h
        .shell
        .editor()
        .document()
        .track(midi)
        .unwrap()
        .routing()
        .midi_output
        .clone()
    {
        Some(MidiTo::Port {
            channel,
            mpe: Some(mpe),
            ..
        }) => {
            assert_eq!(channel, 4, "the port and channel stay");
            mpe
        }
        other => panic!("{other:?}"),
    };
    assert_eq!(settings.zone, MpeZone::Upper);
    click_labelled(&mut h, "OK");

    // An External Instrument's.
    let t = bl_devices::hardware_instrument_type();
    h.shell
        .run("arrangement.select_tracks", json!({ "tracks": [midi] }))
        .unwrap();
    let added: bl_shell::device_commands::DeviceAdded = serde_json::from_value(
        h.shell
            .run(
                "device.add",
                json!({ "device": { "kind": t.kind, "name": t.name, "role": t.role } }),
            )
            .unwrap(),
    )
    .unwrap();
    h.shell
        .run(
            "device.set_external",
            json!({ "device": added.device, "mpe": {} }),
        )
        .unwrap();
    h.shell
        .run(ids::SHOW_MPE_SETTINGS, json!({ "device": added.device }))
        .unwrap();
    click_labelled(&mut h, "Decrease Note Channels");
    let external = h
        .shell
        .editor()
        .document()
        .device(added.device)
        .unwrap()
        .external
        .clone()
        .unwrap();
    assert_eq!(external.mpe.map(|m| m.channels), Some(14));
}

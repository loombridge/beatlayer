//! The plug-in device panel (WP-2.10; L-DEV-010, D19, IMP-03), headless:
//! a loaded plug-in's frame has the window button, a wrench named
//! Show/Hide Plug-In Window, after the activator and before the name;
//! clicking it opens and closes the plug-in's editor window as
//! `plugin.show_window` does, and the button shows whether the window is
//! open. The panel shows the plug-in's first parameters as knobs. The
//! plug-in is the test plug-in, built from source.
//!
//! Spec: clap-hosting R13, R16.

mod common;

use std::time::{Duration, Instant};

use bl_model::{DeviceId, TrackId};
use bl_shell::plugin_commands::{self as ids, PluginInfo, PluginLoaded};
use bl_ui::device_view::{PLUGIN_WINDOW_BUTTON, activator_rect, layout, plugin_window_rect};
use common::{Harness, Window};
use egui::{Rect, pos2};
use serde_json::json;

/// The main window with the test plug-in on the third track, the selected
/// track, once the plug-in reports an editor that can open.
fn harness() -> (Harness, DeviceId) {
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
    (h, loaded.device)
}

fn info(h: &mut Harness, device: DeviceId) -> PluginInfo {
    serde_json::from_value(h.shell.run(ids::INFO, json!({ "device": device })).unwrap()).unwrap()
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

fn button_value(h: &mut Harness) -> String {
    let nodes = Harness::nodes(&h.settle());
    Harness::node(&nodes, PLUGIN_WINDOW_BUTTON).value.clone()
}

// Spec: clap-hosting R13. L-DEV-010.
#[test]
fn clicking_the_wrench_opens_and_closes_the_plugins_window() {
    let (mut h, device) = harness();
    let frame = frame_of(&mut h, device);
    let expandable = h
        .shell
        .device_view_state()
        .devices
        .iter()
        .find(|d| d.id == device)
        .unwrap()
        .expandable;
    let wrench = plugin_window_rect(frame, expandable);
    // After the activator, before the name.
    assert!(wrench.left() > activator_rect(frame, false).right());
    let nodes = Harness::nodes(&h.settle());
    let button = Harness::node(&nodes, PLUGIN_WINDOW_BUTTON);
    let b = button.bounds.unwrap();
    assert!(
        (b.x0 as f32 - wrench.left()).abs() < 3.0,
        "drawn where its rect says"
    );
    assert!(!button.disabled);
    assert_eq!(button.value, "off");
    let title = Harness::node(&nodes, bl_test_clap::PLUGIN_NAME);
    assert!(
        title.bounds.unwrap().x0 as f32 >= wrench.right(),
        "the name follows it"
    );
    assert!(!info(&mut h, device).window_open);

    h.click(wrench.center());
    assert!(
        info(&mut h, device).window_open,
        "the click opened the editor"
    );
    assert_eq!(button_value(&mut h), "on", "the button shows it open");
    assert_eq!(
        h.shell.editor().undo_history().next(),
        Some("Insert Device"),
        "opening the window isn't an undo step"
    );

    h.click(wrench.center());
    assert!(
        !info(&mut h, device).window_open,
        "a second click closes it"
    );
    assert_eq!(button_value(&mut h), "off");
}

// Spec: clap-hosting R5; D19, IMP-03.
#[test]
fn the_panel_shows_the_plugins_parameters_as_knobs() {
    let (mut h, device) = harness();
    let state = h.shell.device_view_state();
    let panel = state
        .devices
        .iter()
        .find(|d| d.id == device)
        .and_then(|d| d.panel.clone())
        .expect("a plug-in device has a panel");
    assert!(panel.plugin);
    assert_eq!(
        panel.description.params.len(),
        bl_shell::plugin_commands::PANEL_KNOBS
    );
    let nodes = Harness::nodes(&h.settle());
    let gain = Harness::node(&nodes, "Gain");
    assert!(gain.numeric.is_some(), "the first parameter is a knob");
    let knob = gain.bounds.unwrap();
    h.click(pos2(
        ((knob.x0 + knob.x1) / 2.0) as f32,
        ((knob.y0 + knob.y1) / 2.0) as f32,
    ));
    assert!(
        !info(&mut h, device).window_open,
        "only the wrench opens the editor"
    );
}

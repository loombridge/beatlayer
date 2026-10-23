//! The plug-in device panel (WP-2.10; L-DEV-010, D19, IMP-03), headless:
//! a loaded plug-in's frame has the window button, a wrench named
//! Show/Hide Plug-In Window, after the activator and before the name;
//! clicking it opens and closes the plug-in's editor window as
//! `plugin.show_window` does, and the button shows whether the window is
//! open. The panel shows the plug-in's first parameters as knobs. The
//! plug-in is the test plug-in, built from source.
//!
//! Spec: clap-hosting R13, R16.

use crate::common;
use std::time::{Duration, Instant};

use bl_model::{DeviceId, TrackId};
use bl_shell::plugin_commands::{self as ids, PluginInfo, PluginLoaded};
use bl_ui::device_view::{
    PLUGIN_CONFIGURE, PLUGIN_WINDOW_BUTTON, activator_rect, layout, plugin_configure_rect,
    plugin_window_rect,
};
use common::{Harness, Window};
use egui::{Event, Modifiers, PointerButton, Pos2, Rect, pos2};
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
    // The name is on the device's group and on its title bar's button; the
    // button is what sits after the wrench.
    let title = nodes
        .iter()
        .find(|n| n.role == egui::accesskit::Role::Button && n.label == bl_test_clap::PLUGIN_NAME)
        .expect("the title bar's button");
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

/// A right-click at a point: the context menu's opening gesture.
fn right_click(h: &mut Harness, at: Pos2) {
    h.frame(vec![Event::PointerMoved(at)]);
    for pressed in [true, false] {
        h.frame(vec![Event::PointerButton {
            pos: at,
            button: PointerButton::Secondary,
            pressed,
            modifiers: Modifiers::NONE,
        }]);
    }
}

/// Whether the plug-in device is in Configure Mode.
fn configuring(h: &mut Harness, device: DeviceId) -> bool {
    h.shell
        .device_view_state()
        .devices
        .iter()
        .find(|d| d.id == device)
        .unwrap()
        .configure
}

// Spec: clap-hosting R5 (WP-5.40, manual 24.1.2). L-DEV-010. The Configure
// switch sits in the plug-in device's title bar, after the window button;
// clicking it opens and closes Plug-In Configure Mode, and neither is an
// undo step.
#[test]
fn clicking_the_configure_switch_opens_and_closes_configure_mode() {
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
    let configure = plugin_configure_rect(frame, expandable);
    let wrench = plugin_window_rect(frame, expandable);
    assert!(configure.left() > wrench.right(), "after the window button");
    let nodes = Harness::nodes(&h.settle());
    let button = Harness::node(&nodes, PLUGIN_CONFIGURE);
    let b = button.bounds.unwrap();
    assert!(
        (b.x0 as f32 - configure.left()).abs() < 3.0,
        "drawn where its rect says"
    );
    assert_eq!(button.value, "off");
    assert!(!configuring(&mut h, device));

    let steps = h.shell.editor().undo_history().len();
    h.click(configure.center());
    assert!(configuring(&mut h, device), "the click opened it");
    let nodes = Harness::nodes(&h.settle());
    assert_eq!(Harness::node(&nodes, PLUGIN_CONFIGURE).value, "on");
    assert_eq!(
        h.shell.editor().undo_history().len(),
        steps,
        "configure mode is view state"
    );

    h.click(configure.center());
    assert!(!configuring(&mut h, device), "a second click closes it");
    assert_eq!(
        Harness::node(&Harness::nodes(&h.settle()), PLUGIN_CONFIGURE).value,
        "off"
    );
}

// Spec: clap-hosting R5 (WP-5.40, manual 24.1.2). Right-clicking a panel
// knob offers the device's configured parameters and a way back to
// following the list, which remaps that knob through `plugin.set_panel_param`.
#[test]
fn right_clicking_a_knob_offers_the_configured_parameters() {
    let (mut h, device) = harness();
    // Take Last Note off the panel, then remap the first knob to it, so the
    // knob reads a name no other panel knob does and its menu offers the way
    // back to the list.
    h.shell
        .run(
            ids::UNCONFIGURE_PARAM,
            json!({ "device": device, "param": bl_test_clap::LAST_NOTE }),
        )
        .unwrap();
    h.shell
        .run(
            ids::SET_PANEL_PARAM,
            json!({ "device": device, "index": 0, "param": bl_test_clap::LAST_NOTE }),
        )
        .unwrap();
    let knob = {
        let nodes = Harness::nodes(&h.settle());
        Harness::node(&nodes, "Last Note").bounds.unwrap()
    };
    let at = pos2(
        ((knob.x0 + knob.x1) / 2.0) as f32,
        ((knob.y0 + knob.y1) / 2.0) as f32,
    );
    right_click(&mut h, at);
    assert!(
        Harness::nodes(&h.settle())
            .iter()
            .any(|n| n.label == "Follow the List"),
        "a remapped knob offers the way back to the list"
    );
    let follow = Harness::node(&Harness::nodes(&h.settle()), "Follow the List")
        .bounds
        .unwrap();
    h.click(pos2(
        ((follow.x0 + follow.x1) / 2.0) as f32,
        ((follow.y0 + follow.y1) / 2.0) as f32,
    ));
    let panel = h
        .shell
        .device_view_state()
        .devices
        .iter()
        .find(|d| d.id == device)
        .unwrap()
        .panel
        .clone()
        .unwrap();
    assert_eq!(
        panel.description.params[0].name, "Gain",
        "the knob follows the list again"
    );
}

/// The device's frame as `device_view.state` reports it.
fn frame_state(h: &mut Harness, device: DeviceId) -> bl_shell::device_commands::DeviceFrame {
    h.shell
        .device_view_state()
        .devices
        .into_iter()
        .find(|d| d.id == device)
        .unwrap()
}

/// The device's panel as `device_view.state` reports it.
fn panel_of(h: &mut Harness, device: DeviceId) -> bl_shell::device_commands::DevicePanel {
    frame_state(h, device).panel.unwrap()
}

/// The panel's knob parameter names, in the panel's own order.
fn panel_names(h: &mut Harness, device: DeviceId) -> Vec<String> {
    panel_of(h, device)
        .description
        .params
        .iter()
        .map(|p| p.name.clone())
        .collect()
}

// Spec: clap-hosting R5 (WP-5.40, manual 24.1.2). A knob whose remap sits
// past the end of the configured list is drawn after the list's knobs, but
// its menu still acts on its own knob: Follow the List clears that knob's
// remap and not a neighbour's.
#[test]
fn a_remapped_knob_past_the_configured_list_follows_its_own_list() {
    let (mut h, device) = harness();
    // Leave the first three parameters configured, so knobs 3 to 7 have
    // nothing of the list to show.
    for param in [
        bl_test_clap::EDITOR_SCALE,
        bl_test_clap::EDITOR_WIDTH,
        bl_test_clap::EDITOR_HEIGHT,
        bl_test_clap::DEPTH,
        bl_test_clap::DEPTH_MOD,
    ] {
        h.shell
            .run(
                ids::UNCONFIGURE_PARAM,
                json!({ "device": device, "param": param }),
            )
            .unwrap();
    }
    assert_eq!(
        panel_names(&mut h, device),
        ["Gain", "Latency", "Last Note"],
        "three configured parameters, three knobs"
    );
    // Remap knob 5, which the list has no entry for; it is drawn after the
    // list's three knobs.
    h.shell
        .run(
            ids::SET_PANEL_PARAM,
            json!({ "device": device, "index": 5, "param": bl_test_clap::EDITOR_SCALE }),
        )
        .unwrap();
    assert_eq!(
        panel_names(&mut h, device),
        ["Gain", "Latency", "Last Note", "Editor Scale"],
        "the remapped knob is drawn after them"
    );

    // Right-click the knob the remap is drawn on — the panel's fourth knob,
    // after the list's three — and take it back to the list.
    let panel = panel_of(&mut h, device);
    let expanded = frame_state(&mut h, device).expanded;
    let frame = frame_of(&mut h, device);
    let body = bl_ui::device_panel::body_rect(frame, bl_shell::device_commands::TITLE_BAR_HEIGHT);
    let controls = bl_ui::device_panel::place_shown(&panel, expanded, body).0;
    right_click(&mut h, controls[3].2.center());
    let follow = Harness::node(&Harness::nodes(&h.settle()), "Follow the List")
        .bounds
        .unwrap();
    h.click(pos2(
        ((follow.x0 + follow.x1) / 2.0) as f32,
        ((follow.y0 + follow.y1) / 2.0) as f32,
    ));
    assert_eq!(
        panel_names(&mut h, device),
        ["Gain", "Latency", "Last Note"],
        "the remap is cleared, so the knob follows the list again"
    );
}

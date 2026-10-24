//! Editor host windows (WP-2.10): where the platform builds one, a test
//! CLAP editor opens in a window Beatlayer owns, with the plug-in's view
//! embedded into it, the display's scale reaches the plug-in, the editor
//! resizes — as the person drags it, and as the plug-in asks the host
//! itself — and it closes again. Windows and X11 only: on macOS a window
//! opens only on the main thread, which `tests/mac_editor.rs` runs on. On
//! Linux these need an X display; CI runs them under Xvfb.

#![cfg(any(target_os = "windows", all(unix, not(target_os = "macos"))))]

use crate::common;
use bl_engine::NodeFactory as _;
use bl_model::{DeviceId, ParamId};
use bl_plugin_host::clap::ClapHost;
use bl_plugin_host::window::GuiSize;
use common::{
    ASK_RESIZE, ASK_RESIZE_HEIGHT, ASK_RESIZE_WIDTH, EDITOR_HEIGHT, EDITOR_SCALE, EDITOR_WIDTH,
    bundle, config, device,
};

/// A host with the test plug-in loaded and a running instance for it: a
/// node for its device, which the main thread gives its plug-in at once.
fn running() -> (ClapHost, DeviceId) {
    let host = ClapHost::new();
    host.load(bundle()).expect("the test plug-in loads");
    let device = device(1, &[]);
    let _node = host
        .factory()
        .create(&device, &config())
        .expect("a node for a CLAP device");
    (host, device.id)
}

/// The value the plug-in reports for `param`.
fn reported(host: &ClapHost, device: DeviceId, param: u32) -> f64 {
    host.status(device)
        .expect("a status")
        .values
        .iter()
        .find(|(id, _)| *id == ParamId(param))
        .map(|(_, value)| *value)
        .unwrap_or_else(|| panic!("no parameter {param}"))
}

// Spec: clap-hosting R13; WP-2.10. A plug-in whose editor supports
// embedding opens in a host window of Beatlayer's own, and the display's
// scale reaches the plug-in (`set_scale`).
#[test]
fn a_test_editor_opens_in_a_host_window_and_the_scale_reaches_it() {
    let (host, device) = running();
    assert!(host.status(device).unwrap().has_editor);
    host.open_editor(device, "Test Gain Echo/1-Audio")
        .expect("it opens");
    let status = host.status(device).unwrap();
    assert!(status.editor_open);
    let scale = status.editor_scale.expect("a host window's scale");
    assert!(scale > 0.0, "a scale from the display");
    assert_eq!(
        reported(&host, device, EDITOR_SCALE),
        scale,
        "the scale the plug-in was given"
    );

    host.close_editor(device).unwrap();
    assert!(!host.status(device).unwrap().editor_open);
}

// Spec: clap-hosting R13; WP-2.10. An embedded editor resizes: the size
// the plug-in takes is told to it (`set_size`), and closing the editor
// closes its window.
#[test]
fn the_editor_resizes_to_the_size_the_plugin_takes_and_closes() {
    let (host, device) = running();
    host.open_editor(device, "Test Gain Echo/1-Audio")
        .expect("it opens");
    host.resize_editor(
        device,
        GuiSize {
            width: 320,
            height: 200,
        },
    )
    .expect("it resizes");
    assert_eq!(reported(&host, device, EDITOR_WIDTH), 320.0);
    assert_eq!(reported(&host, device, EDITOR_HEIGHT), 200.0);

    host.close_editor(device).unwrap();
    let closed = host.status(device).unwrap();
    assert!(!closed.editor_open, "closed");
    assert_eq!(closed.editor_scale, None, "its window is gone");
}

// Spec: clap-hosting R13; WP-2.10. A plug-in that resizes its own view
// asks the host (`request_resize`), and the host window follows it.
#[test]
fn a_resize_the_plugin_asks_for_reaches_the_host_window() {
    let (host, device) = running();
    host.open_editor(device, "Test Gain Echo/1-Audio")
        .expect("it opens");
    host.load_state(device, ASK_RESIZE).expect("it asks");
    host.idle();
    assert_eq!(
        host.status(device).unwrap().editor_size,
        Some(GuiSize {
            width: ASK_RESIZE_WIDTH,
            height: ASK_RESIZE_HEIGHT,
        }),
        "the window the plug-in asked for"
    );

    host.close_editor(device).unwrap();
    assert_eq!(
        host.status(device).unwrap().editor_size,
        None,
        "its window is gone"
    );
}

/// An LV2 host with a running node for the test plug-in's device, or `None`
/// without lilv.
#[cfg(all(unix, not(target_os = "macos")))]
fn lv2_running() -> Option<(bl_plugin_host::lv2::Lv2Host, DeviceId, Box<dyn bl_engine::Node>)> {
    let host = common::lv2_host()?;
    let device = common::lv2_device(1, &[]);
    let node = host.factory().create(&device, &config()).unwrap();
    // The node runs a block, as the engine's thread would, so the plug-in
    // has its values.
    let mut node = node;
    common::run(node.as_mut(), &[0.0; common::FRAMES], &[], &[]);
    Some((host, device.id, node))
}

// Spec: lv2-hosting R13. The plug-in's X11 UI opens in a host window of
// Beatlayer's own, which takes the size of the UI's view; the display's scale
// is the window's.
#[cfg(all(unix, not(target_os = "macos")))]
#[test]
fn an_lv2_editor_opens_in_a_host_window_sized_to_its_view() {
    let Some((host, device, _node)) = lv2_running() else {
        return;
    };
    assert!(host.status(device).unwrap().has_editor);
    host.open_editor(device, "Test Gain Echo/1-Audio")
        .expect("it opens");
    let status = host.status(device).unwrap();
    assert!(status.editor_open && !status.editor_hidden);
    assert_eq!(
        status.editor_size,
        Some(GuiSize {
            width: bl_test_lv2::UI_WIDTH,
            height: bl_test_lv2::UI_HEIGHT
        }),
        "the window the view asked for"
    );

    host.set_editor_visible(device, false).unwrap();
    assert!(host.status(device).unwrap().editor_hidden);
    host.set_editor_visible(device, true).unwrap();

    host.close_editor(device).unwrap();
    let closed = host.status(device).unwrap();
    assert!(!closed.editor_open);
    assert_eq!(closed.editor_size, None, "its window is gone");
}

// Spec: lv2-hosting R13. A window the person resizes tells the UI, which here
// writes the new width to the plug-in's cutoff: one change of the set.
#[cfg(all(unix, not(target_os = "macos")))]
#[test]
fn an_lv2_editor_is_told_when_its_window_is_resized() {
    let Some((host, device, _node)) = lv2_running() else {
        return;
    };
    host.open_editor(device, "Test Gain Echo/1-Audio")
        .expect("it opens");
    let size = GuiSize {
        width: 320,
        height: 200,
    };
    host.resize_editor(device, size).expect("it resizes");
    assert_eq!(host.status(device).unwrap().editor_size, Some(size));
    host.idle();
    let changes = host.take_param_changes();
    assert!(
        changes
            .iter()
            .any(|c| c.param == ParamId(bl_test_lv2::CUTOFF) && c.value == 320.0),
        "{changes:?}"
    );
    host.close_editor(device).unwrap();
}

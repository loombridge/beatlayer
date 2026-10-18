//! Editor host windows on macOS (WP-2.10): a CLAP editor that only embeds,
//! as most JUCE CLAPs' editors on macOS do, opens in a utility panel of
//! Beatlayer's own with the plug-in's view added to it (CLAP's `cocoa`
//! API), hides and shows without closing, resizes, and closes.
//!
//! AppKit makes windows only on the main thread, so this runs as a binary
//! of its own with the test harness off: `fn main` is the process's main
//! thread. With Surge XT installed where macOS keeps CLAP plug-ins, it is
//! opened the same way; without it, that part is skipped. On Windows and
//! Linux `tests/editor.rs` covers the host windows; this binary passes.

#[cfg(target_os = "macos")]
mod common;

fn main() {
    // `cargo nextest` and `cargo test -- --list` ask a harness to list its
    // tests: this binary is one, and none of them is ignored.
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|arg| arg == "--list") {
        if !args.iter().any(|arg| arg == "--ignored") {
            println!("mac_editor: test");
        }
        return;
    }
    #[cfg(target_os = "macos")]
    macos::run();
    #[cfg(not(target_os = "macos"))]
    println!("mac_editor: tests/editor.rs covers Windows and Linux");
    println!("test result: ok. 1 passed; 0 failed; 0 ignored");
}

#[cfg(target_os = "macos")]
mod macos {
    use std::path::PathBuf;

    use bl_core::id::Id;
    use bl_engine::NodeFactory as _;
    use bl_model::{Device, DeviceId, DeviceKind, PluginFormat};
    use bl_plugin_host::clap::ClapHost;
    use bl_plugin_host::window::GuiSize;
    use bl_test_clap::EMBED_ONLY;

    use super::common::{bundle, config, device};

    pub fn run() {
        // The app's own NSApplication, as the app has one.
        let mtm = objc2::MainThreadMarker::new().expect("main runs on the main thread");
        let _app = objc2_app_kit::NSApplication::sharedApplication(mtm);
        an_editor_that_only_embeds_opens_in_a_host_window();
        surge_xt_opens_where_it_is_installed();
    }

    // Spec: clap-hosting R13; WP-2.10.
    fn an_editor_that_only_embeds_opens_in_a_host_window() {
        let host = ClapHost::new();
        host.load(bundle()).expect("the test plug-in loads");
        let device = device(1, &[]);
        let _node = host
            .factory()
            .create(&device, &config())
            .expect("a node for a CLAP device");
        host.load_state(device.id, EMBED_ONLY)
            .expect("it embeds only");
        let status = host.status(device.id).unwrap();
        assert!(status.has_editor, "an editor that only embeds can open");

        host.open_editor(device.id, "Test Gain Echo/1-Audio")
            .expect("it opens in a host window");
        let status = host.status(device.id).unwrap();
        assert!(status.editor_open);
        assert_eq!(
            status.editor_size,
            Some(GuiSize {
                width: 240,
                height: 120
            }),
            "the window is the size the editor asked for, in points"
        );
        assert!(status.editor_scale.is_some_and(|s| s > 0.0));

        host.set_editor_visible(device.id, false).unwrap();
        host.idle();
        let status = host.status(device.id).unwrap();
        assert!(
            status.editor_open && status.editor_hidden,
            "hidden, not closed"
        );
        host.set_editor_visible(device.id, true).unwrap();
        host.idle();
        assert!(!host.status(device.id).unwrap().editor_hidden);

        let bigger = GuiSize {
            width: 320,
            height: 200,
        };
        host.resize_editor(device.id, bigger).expect("it resizes");
        host.idle();
        assert_eq!(host.status(device.id).unwrap().editor_size, Some(bigger));

        host.close_editor(device.id).unwrap();
        let status = host.status(device.id).unwrap();
        assert!(!status.editor_open);
        assert_eq!(status.editor_size, None, "its window is gone");
        println!("test an_editor_that_only_embeds_opens_in_a_host_window ... ok");
    }

    fn surge_xt_opens_where_it_is_installed() {
        let Some(home) = std::env::var_os("HOME") else {
            return;
        };
        let path = PathBuf::from(home).join("Library/Audio/Plug-Ins/CLAP/Surge XT.clap");
        if !path.exists() {
            println!("test surge_xt_opens_where_it_is_installed ... skipped, not installed");
            return;
        }
        let host = ClapHost::new();
        let types = host.load(&path).expect("Surge XT loads");
        let surge = types
            .iter()
            .find(|t| t.device_type.name == "Surge XT")
            .expect("Surge XT is in its bundle");
        let DeviceKind::Plugin { plugin_id, .. } = &surge.device_type.kind else {
            panic!("a plug-in's device type");
        };
        let device = Device::new(
            DeviceId(Id::new(1).expect("an ID")),
            "Surge XT",
            DeviceKind::Plugin {
                format: PluginFormat::Clap,
                plugin_id: plugin_id.clone(),
            },
        );
        let _node = host
            .factory()
            .create(&device, &config())
            .expect("a node for Surge XT");
        let status = host.status(device.id).unwrap();
        assert!(status.has_editor, "Surge XT's editor can open");
        host.open_editor(device.id, "Surge XT/1-MIDI")
            .expect("Surge XT's editor opens");
        let status = host.status(device.id).unwrap();
        assert!(status.editor_open);
        assert!(
            status.editor_size.is_some(),
            "embedded in a host window of Beatlayer's own"
        );
        host.close_editor(device.id).unwrap();
        assert!(!host.status(device.id).unwrap().editor_open);
        println!("test surge_xt_opens_where_it_is_installed ... ok");
    }
}

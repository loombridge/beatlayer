//! An Audio Unit's own editor (WP-2.8's second part; the CLAP hosting spec's
//! R13, which the AU spec mirrors).
//!
//! Apple makes a unit's view controller on the main thread and delivers the
//! completion on the main thread's run loop, so this runs as a binary of its
//! own with the test harness off: `fn main` is the process's main thread,
//! where the completion arrives. `cargo xtask check` runs it beside the
//! other tests.
//!
//! On Windows and Linux there is no editor to host; the binary passes.

#[cfg(target_os = "macos")]
fn main() {
    // `cargo nextest` and `cargo test -- --list` ask a harness to list its
    // tests: this binary is one, and none of them is ignored.
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|arg| arg == "--list") {
        if !args.iter().any(|arg| arg == "--ignored") {
            println!("au_editor: test");
        }
        return;
    }
    macos::run();
    println!("test result: ok. 1 passed; 0 failed; 0 ignored");
}

#[cfg(not(target_os = "macos"))]
fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|arg| arg == "--list") {
        if !args.iter().any(|arg| arg == "--ignored") {
            println!("au_editor: test");
        }
        return;
    }
    println!("au_editor: Windows and Linux host no Audio Unit editors");
    println!("test result: ok. 1 passed; 0 failed; 0 ignored");
}

#[cfg(target_os = "macos")]
mod macos {
    use std::time::{Duration, Instant};

    use bl_core::id::Id;
    use bl_core::time::SampleRate;
    use bl_engine::{NodeConfig, NodeFactory as _};
    use bl_model::{Device, DeviceId, DeviceKind, PluginFormat, TrackId, TrackKind};
    use bl_plugin_host::au::AuHost;
    use bl_shell::Shell;
    use bl_shell::plugin_commands as ids;
    use bl_shell::plugin_commands::{PluginInfo, PluginLoaded};
    use objc2::rc::Retained;
    use objc2_app_kit::{NSWindow, NSWindowStyleMask};
    use serde_json::json;

    fn audio_track(shell: &Shell) -> TrackId {
        shell
            .editor()
            .document()
            .tracks()
            .find(|t| t.kind() == TrackKind::Audio)
            .map(bl_model::Track::id)
            .expect("the default set has an audio track")
    }

    /// Polls the shell, as the app's frames do, until `ready`.
    fn wait(shell: &mut Shell, device: bl_model::DeviceId, ready: impl Fn(&PluginInfo) -> bool) {
        let until = Instant::now() + Duration::from_secs(10);
        loop {
            shell.poll();
            if let Ok(value) = shell.run(ids::INFO, json!({ "device": device }))
                && let Ok(info) = serde_json::from_value::<PluginInfo>(value)
                && ready(&info)
            {
                return;
            }
            assert!(Instant::now() < until, "the unit never got ready");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn info(shell: &mut Shell, device: bl_model::DeviceId) -> PluginInfo {
        serde_json::from_value(shell.run(ids::INFO, json!({ "device": device })).unwrap()).unwrap()
    }

    pub fn run() {
        let mut shell = Shell::headless();
        shell.start_audio();
        assert!(shell.audio().engine_running(), "{:?}", shell.audio());
        let track = audio_track(&shell);
        // Auto-Hide Plug-In Windows hides an editor off the selected track.
        shell
            .invoke("arrangement.select_tracks", json!({ "tracks": [track] }))
            .unwrap();
        let device = load_unit(&mut shell, track);

        // Spec: au-hosting's second part, clap-hosting R13. Its own editor
        // opens and closes by `plugin.show_window`; neither is an undo step.
        // The unit's view controller arrives on the main thread's run loop,
        // which `poll` turns, so the window opens a frame or so later.
        let history = shell.editor().undo_history().len();
        let before = info(&mut shell, device);
        assert!(!before.window_open, "{before:?}");
        shell
            .run(ids::SHOW_WINDOW, json!({ "device": device }))
            .unwrap();
        wait(&mut shell, device, |i| i.window_open);
        let opened = info(&mut shell, device);
        assert!(opened.window_open, "{opened:?}");
        assert!(opened.has_window, "{opened:?}");

        shell
            .run(ids::SHOW_WINDOW, json!({ "device": device }))
            .unwrap();
        wait(&mut shell, device, |i| !i.window_open);
        let closed = info(&mut shell, device);
        assert!(!closed.window_open, "{closed:?}");
        assert_eq!(shell.editor().undo_history().len(), history);

        println!("au_editor: opened and closed AUDelay's editor");

        // A unit hands out its view controller once, so each case opens a
        // unit of its own.
        let au = shell
            .plugin_hosts()
            .au
            .expect("the Audio Unit host")
            .clone();
        the_person_closes_the_editor(&mut shell, &au, track);
        the_editor_window_cannot_be_minimised(&mut shell, &au, track);
        a_deleted_device_closes_its_editor(&mut shell, &au, track);
        a_unit_let_go_of_closes_its_editor();
        a_scheduled_change_is_not_the_persons_edit();
    }

    /// Loads AUDelay onto `track` and waits until it is running.
    fn load_unit(shell: &mut Shell, track: TrackId) -> DeviceId {
        let loaded: PluginLoaded = serde_json::from_value(
            shell
                .run(
                    ids::LOAD_AU,
                    json!({
                        "type": "aufx",
                        "subtype": "dely",
                        "manufacturer": "appl",
                        "track": track,
                    }),
                )
                .unwrap(),
        )
        .unwrap();
        wait(shell, loaded.device, |i| i.active);
        loaded.device
    }

    /// Opens a new unit's editor and returns its device and window.
    fn open_editor(
        shell: &mut Shell,
        au: &AuHost,
        track: TrackId,
    ) -> (DeviceId, Retained<NSWindow>) {
        let device = load_unit(shell, track);
        shell
            .run(ids::SHOW_WINDOW, json!({ "device": device }))
            .unwrap();
        wait(shell, device, |i| i.window_open);
        (
            device,
            au.editor_windows().pop().expect("the editor's window"),
        )
    }

    // Spec: au-hosting R13. The title bar's close button closes the editor:
    // the next frames neither bring the window back nor report it open.
    fn the_person_closes_the_editor(shell: &mut Shell, au: &AuHost, track: TrackId) {
        let (device, window) = open_editor(shell, au, track);
        window.close();
        for _ in 0..3 {
            shell.poll();
        }
        assert!(!info(shell, device).window_open, "the editor is closed");
        assert!(!window.isVisible(), "the window stayed closed");
        println!("au_editor: the close button closed the editor");
    }

    // Spec: au-hosting R13. The editor window has no minimise button: a
    // minimised window is not on screen, which is how a closed one is told.
    fn the_editor_window_cannot_be_minimised(shell: &mut Shell, au: &AuHost, track: TrackId) {
        let (device, window) = open_editor(shell, au, track);
        assert!(
            !window
                .styleMask()
                .contains(NSWindowStyleMask::Miniaturizable)
        );
        shell
            .run(ids::SHOW_WINDOW, json!({ "device": device }))
            .unwrap();
        println!("au_editor: the editor window has no minimise button");
    }

    // Spec: au-hosting R13. Deleting the device closes its editor; undo
    // brings the unit back with its editor closed.
    fn a_deleted_device_closes_its_editor(shell: &mut Shell, au: &AuHost, track: TrackId) {
        let (device, _) = open_editor(shell, au, track);
        shell
            .invoke("device.remove", json!({ "device": device }))
            .unwrap();
        shell.poll();
        assert!(
            au.open_editors().is_empty(),
            "no editor for a deleted device"
        );
        shell.run("edit.undo", json!({})).unwrap();
        wait(shell, device, |i| i.active && !i.window_open);
        println!("au_editor: a deleted device's editor closed");
    }

    // Spec: au-hosting R13. Letting go of a unit closes its editor window,
    // whether its node still holds the processor (the unit waits in
    // `closing`) or not.
    fn a_unit_let_go_of_closes_its_editor() {
        for keep_node in [true, false] {
            let au = AuHost::new();
            au.load("aufx", "dely", "appl")
                .expect("AUDelay is installed");
            let device = Device::new(
                DeviceId(Id::new(9).expect("an ID")),
                "Test AU",
                DeviceKind::Plugin {
                    format: PluginFormat::AudioUnit,
                    plugin_id: "aufx/dely/appl".to_owned(),
                },
            );
            let config = NodeConfig {
                round_trip_latency: 0,
                sample_rate: SampleRate::HZ_48000,
                max_frames: 512,
                channels: 2,
            };
            let node = au.factory().create(&device, &config).expect("a node");
            let node = keep_node.then_some(node);
            au.idle();
            au.open_editor(device.id, "Test AU").expect("it opens");
            while au.open_editors() != [device.id] {
                au.idle();
            }
            let windows = au.editor_windows();
            assert_eq!(windows.len(), 1);
            au.clear();
            assert!(
                windows.iter().all(|w| !w.isVisible()),
                "no window stays up (node kept: {keep_node})"
            );
            assert!(au.editor_windows().is_empty());
            drop(node);
        }
        println!("au_editor: a unit let go of closed its editor");
    }

    // Spec: au-hosting R6. The host's own automation, scheduled on the unit,
    // is not reported back as the person's edit through the open editor.
    fn a_scheduled_change_is_not_the_persons_edit() {
        use bl_engine::{Block, NodeEvent, ParamValue, TransportInfo};
        use bl_model::ParamId;
        let au = AuHost::new();
        au.load("aufx", "dely", "appl")
            .expect("AUDelay is installed");
        let device = Device::new(
            DeviceId(Id::new(9).expect("an ID")),
            "Test AU",
            DeviceKind::Plugin {
                format: PluginFormat::AudioUnit,
                plugin_id: "aufx/dely/appl".to_owned(),
            },
        );
        let config = NodeConfig {
            round_trip_latency: 0,
            sample_rate: SampleRate::HZ_48000,
            max_frames: 512,
            channels: 2,
        };
        let mut node = au.factory().create(&device, &config).expect("a node");
        au.idle();
        au.open_editor(device.id, "Test AU").expect("it opens");
        while au.open_editors() != [device.id] {
            au.idle();
        }
        let events = [NodeEvent {
            offset: 100,
            param: ParamId(0),
            value: ParamValue::Set(50.0),
        }];
        let mut data = vec![0.0f32; 2 * 512];
        for _ in 0..3 {
            let mut block = Block::new(
                &mut data,
                2,
                512,
                512,
                &events,
                TransportInfo::default(),
                SampleRate::HZ_48000,
            );
            node.process(&mut block);
            au.idle();
            std::thread::sleep(Duration::from_millis(20));
        }
        let changes = au.take_param_changes();
        assert_eq!(changes, [], "the editor's observer saw the host's own change");
        println!("au_editor: a scheduled change is not the person's edit");
        au.clear();
    }
}

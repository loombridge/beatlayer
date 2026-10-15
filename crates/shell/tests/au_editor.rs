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

    use bl_model::{TrackId, TrackKind};
    use bl_shell::Shell;
    use bl_shell::plugin_commands as ids;
    use bl_shell::plugin_commands::{PluginInfo, PluginLoaded};
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
        let device = loaded.device;
        wait(&mut shell, device, |i| i.active);

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
    }
}

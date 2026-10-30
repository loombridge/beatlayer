//! Link in the app (WP-7.14, `docs/specs/link.md` R1 to R3, R8, R14, R15):
//! its switches are kept between starts, a peer's tempo becomes the set's
//! with no undo step, and an export ignores Link. A second Link instance in
//! the test plays the peer over the loopback network, so these tests wait
//! for discovery with a deadline, and run one at a time with `bl-link`'s
//! (`.config/nextest.toml`).

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use bl_core::time::Ticks;
use bl_link::Link;
use bl_shell::audio::AudioHost;
use bl_shell::files::ProjectFiles;
use bl_shell::settings::Preferences;
use bl_shell::{Shell, browser_commands, testing};
use serde_json::{Value, json};

const BAR: i64 = Ticks::WHOLE.get();

/// Waits until `done` holds, up to ten seconds.
fn wait(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn state(shell: &mut Shell) -> Value {
    shell.run("link.state", json!({})).unwrap()
}

/// A shell on the preferences in `dir`.
fn start(dir: &Path) -> Shell {
    let prefs = Preferences::load(dir).unwrap();
    Shell::new(
        prefs.clone(),
        Some(dir.to_owned()),
        AudioHost::null(prefs.audio),
        Box::new(ProjectFiles::new()),
    )
}

/// A peer that has been in its session for well over the half second
/// within which Link breaks a tie between sessions by their identifiers, so
/// the shell that switches Link on joins it.
fn older_peer(bpm: f64) -> Link {
    let peer = Link::new(bpm);
    peer.enable(true, bpm);
    std::thread::sleep(Duration::from_millis(1500));
    peer
}

// Spec: link R1, R8, R15. Link and Start Stop Sync are off in a new install,
// and kept across a relaunch.
#[test]
fn link_and_start_stop_sync_are_off_at_first_and_kept_across_a_relaunch() {
    let dir = std::env::temp_dir().join(format!("bl-shell-link-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut shell = start(&dir);
    let at_first = state(&mut shell);
    assert_eq!(at_first["enabled"], false);
    assert_eq!(at_first["start_stop_sync"], false);
    assert_eq!(at_first["show_toggle"], true, "the toggle shows at first");
    assert_eq!(at_first["peers"], 0);
    assert!(!shell.can_invoke("link.set_enabled", &json!({})) || shell.link_peers() == 0);

    let on = shell.run("link.set_enabled", json!({})).unwrap();
    assert_eq!(on["on"], true, "left out, it switches to the other state");
    let sync = shell
        .run("link.set_start_stop_sync", json!({ "on": true }))
        .unwrap();
    assert_eq!(sync["on"], true);
    shell
        .run("link.set_show_toggle", json!({ "on": false }))
        .unwrap();
    let now = state(&mut shell);
    assert_eq!(now["enabled"], true);
    assert_eq!(now["start_stop_sync"], true);
    drop(shell);

    let mut shell = start(&dir);
    let kept = state(&mut shell);
    assert_eq!(kept["enabled"], true, "Link is kept on");
    assert_eq!(kept["start_stop_sync"], true, "and Start Stop Sync");
    assert_eq!(kept["show_toggle"], false, "and Show Link Toggle");
    shell
        .run("link.set_enabled", json!({ "on": false }))
        .unwrap();
    drop(shell);
    let mut shell = start(&dir);
    assert_eq!(state(&mut shell)["enabled"], false);
    let _ = std::fs::remove_dir_all(&dir);
}

// Spec: link R2, R3. Switching Link on where a session exists takes its
// tempo into the set, and a peer's change reaches the set after it, with no
// undo step of its own.
#[test]
fn a_peers_tempo_becomes_the_sets_with_no_undo_step() {
    let peer = older_peer(100.0);
    let (mut shell, control) = testing::rig();
    shell.start_audio();
    testing::settle(&control, &mut shell);
    let steps = shell.editor().undo_history().len();
    assert!((tempo(&mut shell) - 120.0).abs() < 1e-9);
    shell
        .run("link.set_enabled", json!({ "on": true }))
        .unwrap();
    wait("the shell's tempo to become the session's", || {
        testing::advance(&control, &mut shell, 20);
        shell.poll();
        (tempo(&mut shell) - 100.0).abs() < 0.01
    });
    assert_eq!(state(&mut shell)["peers"], 1);

    // A peer's change reaches the set, whatever the pace of the changes.
    for bpm in [111.0, 123.0, 135.0] {
        peer.set_tempo(bpm);
    }
    wait("the peer's tempo", || {
        testing::advance(&control, &mut shell, 20);
        shell.poll();
        (tempo(&mut shell) - 135.0).abs() < 0.01
    });
    assert_eq!(shell.editor().undo_history().len(), steps, "no undo step");
    assert!(shell.is_dirty(), "though the set is marked as changed");
    let session = state(&mut shell);
    assert!((session["tempo"].as_f64().unwrap() - 135.0).abs() < 0.01);

    // Spec: link R2. A new set made in the session keeps the session's tempo.
    shell.run("set.new", json!({})).unwrap();
    assert!(
        (tempo(&mut shell) - 135.0).abs() < 0.01,
        "{}",
        tempo(&mut shell)
    );
}

fn tempo(shell: &mut Shell) -> f64 {
    shell.run("transport.state", json!({})).unwrap()["tempo"]
        .as_f64()
        .unwrap()
}

/// A folder of its own under the system's temporary folder.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("bl-link-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Writes a 16-bit mono WAV file of one second at 48 kHz.
fn wav(path: &Path) {
    let frames = 48_000u32;
    let data = frames * 2;
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    for field in [
        16u32.to_le_bytes(),
        [1, 0, 1, 0],
        48_000u32.to_le_bytes(),
        96_000u32.to_le_bytes(),
    ] {
        bytes.extend_from_slice(&field);
    }
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data.to_le_bytes());
    for i in 0..frames {
        bytes.extend_from_slice(&(((i % 100) as i16 - 50) * 20).to_le_bytes());
    }
    std::fs::write(path, bytes).unwrap();
}

// Spec: link R14. An export plays the set's own tempo map: the same file
// with Link on, in a session at another tempo, as with it off.
#[test]
fn an_export_ignores_link() {
    let scratch = Scratch::new("export");
    let file = scratch.0.join("Tone.wav");
    wav(&file);
    let mut shell = Shell::headless();
    shell
        .run(
            browser_commands::ADD_TO_ARRANGEMENT,
            json!({ "paths": [file.to_string_lossy()], "at": 4 * BAR }),
        )
        .unwrap();
    let render = |shell: &mut Shell, name: &str| -> (u64, Vec<u8>) {
        let path = scratch.0.join(name);
        let result = shell
            .run(
                "export.audio",
                json!({ "path": path.to_string_lossy(), "bit_depth": 24, "sample_rate": 48000 }),
            )
            .unwrap();
        (
            result["frames"].as_u64().unwrap(),
            std::fs::read(path).unwrap(),
        )
    };
    let off = render(&mut shell, "off.wav");

    let _peer = older_peer(200.0);
    shell
        .run("link.set_enabled", json!({ "on": true }))
        .unwrap();
    wait("a peer", || shell.link_peers() == 1);
    // The session is at 200 BPM; the set, which the shell hasn't polled, is
    // at its own 120.
    wait("the session's tempo", || {
        (state(&mut shell)["tempo"].as_f64().unwrap() - 200.0).abs() < 0.01
    });
    let on = render(&mut shell, "on.wav");
    assert_eq!(on.0, 9 * 48_000, "the set's tempo map, not the session's");
    assert_eq!(on, off, "the same file as with Link off");
}

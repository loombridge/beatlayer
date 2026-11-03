//! Tempo Follower in the app (WP-9.13, `docs/specs/tempo-follower.md` R1 to
//! R5, R19, R21): its two settings are kept in real preference files across
//! a relaunch, Follow starts off, it is refused with its reason while the
//! toggle is hidden or the input can't be reached, it switches EXT off and
//! keeps it off, and Tap Tempo is refused while it is on. A null device
//! paced by hand plays the stream, with silence on its input.

use std::path::Path;

use bl_audio_io::NullControl;
use bl_midi_io::{Direction, TestBackend};
use bl_shell::{Shell, ShellError, testing};
use serde_json::{Value, json};

const PORT: &str = "Sequencer";

fn dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "bl-shell-tempo-follower-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// A shell on the preferences in `dir`, its stream running with the null
/// device's input on.
fn start(dir: &Path) -> (Shell, NullControl) {
    let (mut shell, control) = testing::rig_in(dir);
    shell.start_audio();
    shell
        .run("audio.configure", json!({ "input_device": "null:null" }))
        .unwrap();
    testing::settle(&control, &mut shell);
    (shell, control)
}

fn state(shell: &mut Shell) -> Value {
    shell.run("tempo_follower.state", json!({})).unwrap()
}

fn show(shell: &mut Shell, on: bool) {
    shell
        .run(
            "settings.set_show_tempo_follower_toggle",
            json!({ "on": on }),
        )
        .unwrap();
}

fn follow(shell: &mut Shell, on: bool) -> Result<Value, ShellError> {
    shell.run("tempo_follower.set_enabled", json!({ "on": on }))
}

// Spec: tempo-follower R1, R3. The toggle is hidden and the input is the
// first mono channel at first, Follow is off, and the two settings are kept
// across a relaunch, which starts with Follow off again.
#[test]
fn the_settings_are_kept_across_a_relaunch_and_follow_starts_off() {
    let dir = dir("kept");
    let (mut shell, _control) = start(&dir);
    let at_first = state(&mut shell);
    assert_eq!(at_first["on"], false);
    assert_eq!(at_first["show_toggle"], false);
    assert_eq!(at_first["input"], json!({ "type": "mono", "channel": 1 }));
    assert_eq!(at_first["input_label"], "1");
    assert_eq!(at_first["can_run"], true);
    assert_eq!(at_first["tempo"], Value::Null);

    show(&mut shell, true);
    let set = shell
        .run(
            "settings.set_tempo_follower_input",
            json!({ "channel": { "type": "pair", "channel": 1 } }),
        )
        .unwrap();
    assert_eq!(set["label"], "1/2");
    follow(&mut shell, true).unwrap();
    assert_eq!(state(&mut shell)["on"], true);
    drop(shell);

    let (mut shell, _control) = start(&dir);
    let kept = state(&mut shell);
    assert_eq!(kept["show_toggle"], true, "kept");
    assert_eq!(
        kept["input"],
        json!({ "type": "pair", "channel": 1 }),
        "kept"
    );
    assert_eq!(kept["on"], false, "Follow isn't kept");
    let _ = std::fs::remove_dir_all(&dir);
}

// Spec: tempo-follower R2. Follow is refused while the toggle is hidden, and
// hiding the toggle switches Follow off.
#[test]
fn follow_never_runs_while_the_toggle_is_hidden() {
    let (mut shell, _control) = start(&dir("hidden"));
    let error = follow(&mut shell, true).unwrap_err();
    assert!(
        matches!(&error, ShellError::Unavailable { reason, .. } if reason.contains("hidden")),
        "{error}"
    );
    show(&mut shell, true);
    follow(&mut shell, true).unwrap();
    assert_eq!(state(&mut shell)["on"], true);
    show(&mut shell, false);
    assert_eq!(
        state(&mut shell)["on"],
        false,
        "hiding it switches Follow off"
    );
    assert!(shell.tempo_follower_input() == bl_model::Channel::Mono(1));
}

// Spec: tempo-follower R4. With the device's inputs off, as they are at
// first, Follow is refused with the reason.
#[test]
fn follow_is_refused_while_the_devices_inputs_are_off() {
    let (mut shell, control) = testing::rig_in(&dir("inputs-off"));
    shell.start_audio();
    testing::settle(&control, &mut shell);
    show(&mut shell, true);
    let shown = state(&mut shell);
    assert_eq!(shown["can_run"], false);
    assert!(
        shown["unavailable"]
            .as_str()
            .unwrap()
            .contains("inputs are off"),
        "{shown}"
    );
    let error = follow(&mut shell, true).unwrap_err();
    assert!(error.to_string().contains("inputs are off"), "{error}");
}

// Spec: tempo-follower R4. With the input channel not on the device, or the
// engine off, Follow is refused with the reason, can't run in the state, and
// Follow already on goes off.
#[test]
fn follow_is_refused_with_its_reason_while_the_input_cant_be_reached() {
    let (mut shell, _control) = start(&dir("unreachable"));
    show(&mut shell, true);
    shell
        .run(
            "settings.set_tempo_follower_input",
            json!({ "channel": { "type": "mono", "channel": 9 } }),
        )
        .unwrap();
    let shown = state(&mut shell);
    assert_eq!(shown["can_run"], false);
    assert!(
        shown["unavailable"]
            .as_str()
            .unwrap()
            .contains("input channel 9"),
        "{shown}"
    );
    let error = follow(&mut shell, true).unwrap_err();
    assert!(
        matches!(&error, ShellError::Unavailable { reason, .. } if reason.contains("input channel 9")),
        "{error}"
    );
    assert_eq!(state(&mut shell)["on"], false);

    // Follow on, then the input moves to a channel the device hasn't.
    shell
        .run(
            "settings.set_tempo_follower_input",
            json!({ "channel": { "type": "mono", "channel": 2 } }),
        )
        .unwrap();
    follow(&mut shell, true).unwrap();
    shell
        .run(
            "settings.set_tempo_follower_input",
            json!({ "channel": { "type": "pair", "channel": 2 } }),
        )
        .unwrap();
    assert_eq!(
        state(&mut shell)["on"],
        false,
        "a pair 2/3 needs a third channel"
    );

    // Follow on, then the engine goes off.
    shell
        .run(
            "settings.set_tempo_follower_input",
            json!({ "channel": { "type": "mono", "channel": 1 } }),
        )
        .unwrap();
    follow(&mut shell, true).unwrap();
    shell.run("audio.toggle_engine", json!({})).unwrap();
    shell.poll();
    assert_eq!(state(&mut shell)["on"], false, "the engine is off");
    let error = follow(&mut shell, true).unwrap_err();
    assert!(error.to_string().contains("audio engine is off"), "{error}");
}

// Spec: tempo-follower R5; midi-clock R28. Follow switches EXT off and
// disables it, and leaves it off when Follow goes off.
#[test]
fn follow_switches_ext_off_and_leaves_it_off() {
    let (mut shell, control) = testing::rig();
    let test = TestBackend::new();
    test.add_port(Direction::Input, PORT);
    shell.use_midi_backend(Box::new(test));
    shell.start_audio();
    shell
        .run("audio.configure", json!({ "input_device": "null:null" }))
        .unwrap();
    shell
        .run(
            "midi.set_port",
            json!({ "direction": "input", "name": PORT, "sync": true }),
        )
        .unwrap();
    shell
        .run("transport.set_external_sync", json!({ "on": true }))
        .unwrap();
    testing::settle(&control, &mut shell);
    let ext = |shell: &mut Shell| {
        shell.run("transport.state", json!({})).unwrap()["external_sync"] == true
    };
    assert!(ext(&mut shell));
    assert!(shell.can_invoke("transport.set_external_sync", &json!({})));

    show(&mut shell, true);
    follow(&mut shell, true).unwrap();
    assert!(!ext(&mut shell), "EXT is off while Follow is on");
    assert!(
        !shell.can_invoke("transport.set_external_sync", &json!({})),
        "Options ▸ External Sync is disabled"
    );
    let error = shell
        .run("transport.set_external_sync", json!({ "on": true }))
        .unwrap_err();
    assert!(
        matches!(&error, ShellError::Unavailable { reason, .. } if reason.contains("Tempo Follower")),
        "{error}"
    );

    follow(&mut shell, false).unwrap();
    assert!(!ext(&mut shell), "switching Follow off leaves EXT off");
    assert!(shell.can_invoke("transport.set_external_sync", &json!({})));
    shell
        .run("transport.set_external_sync", json!({ "on": true }))
        .unwrap();
    assert!(ext(&mut shell));
}

// Spec: tempo-follower R6, R7, R21; R14. With nothing rhythmic on the input
// the tracker is never sure, so the song keeps its own tempo, and Tap Tempo
// is refused while Follow is on.
#[test]
fn follow_on_silence_keeps_the_sets_tempo_and_refuses_tap_tempo() {
    let (mut shell, control) = start(&dir("silence"));
    show(&mut shell, true);
    follow(&mut shell, true).unwrap();
    testing::advance(&control, &mut shell, 3_000);
    let shown = state(&mut shell);
    assert_eq!(shown["on"], true);
    assert_eq!(shown["tempo"], Value::Null, "the tracker has not been sure");
    let tempo = shell.run("transport.state", json!({})).unwrap()["tempo"].clone();
    assert!((tempo.as_f64().unwrap() - 120.0).abs() < 1e-9, "{tempo}");

    let error = shell.run("transport.tap_tempo", json!({})).unwrap_err();
    assert!(
        matches!(&error, ShellError::Unavailable { reason, .. } if reason.contains("Tempo Follower")),
        "{error}"
    );
    follow(&mut shell, false).unwrap();
    assert_eq!(state(&mut shell)["on"], false);
}

// Spec: tempo-follower R6. The tempo the engine played reaches the main
// thread as it does for a Link peer's: the first at once, then at most once
// every 100 ms while it moves, the last one after it stops.
#[test]
fn the_tempo_the_engine_played_is_taken_at_most_every_100_ms() {
    use bl_engine::Tracker;
    use bl_shell::tempo_follower::Tracking;

    let (mut shell, control) = start(&dir("written"));
    let clock = control.clock().unwrap();
    let (mut tracking, mut engine) = Tracking::start(48_000, 120.0, bl_model::Channel::Mono(1));
    assert_eq!(tracking.followed(), None, "nothing played yet");
    assert_eq!(tracking.hear(clock.now()), None);
    engine.given(140.0);
    assert_eq!(tracking.followed(), Some(140.0));
    assert_eq!(tracking.hear(clock.now()), Some(140.0));
    engine.given(141.0);
    assert_eq!(tracking.hear(clock.now()), None, "not again within 100 ms");
    testing::advance(&control, &mut shell, 150);
    assert_eq!(tracking.hear(clock.now()), Some(141.0));
    assert_eq!(tracking.hear(clock.now()), None, "taken once");
}

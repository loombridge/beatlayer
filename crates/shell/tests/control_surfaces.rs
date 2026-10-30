//! The Control Surfaces table and its host in the shell (WP-7.5;
//! `docs/specs/control-surfaces.md`, R2, R7, R11, R13 to R15): rows are set,
//! listed and dumped by command, kept in the preferences, and a surface's
//! gesture is one undo step. Everything runs against the in-process test
//! backend and the test surface.

use std::path::Path;

use bl_audio_io::NullControl;
use bl_control::testing::{GOODBYE, HELLO, Journal, SETUP};
use bl_core::clock::TestClock;
use bl_midi_io::{Direction, TestBackend};
use bl_model::TrackId;
use bl_shell::audio::AudioHost;
use bl_shell::files::ProjectFiles;
use bl_shell::settings::Preferences;
use bl_shell::{Shell, ShellError, testing};
use serde_json::{Value, json};

struct Rig {
    shell: Shell,
    control: NullControl,
    test: TestBackend,
    clock: TestClock,
}

impl Rig {
    fn new() -> Self {
        let (mut shell, control) = testing::rig();
        let test = TestBackend::new();
        for name in ["Pads", "Other"] {
            test.add_port(Direction::Input, name);
            test.add_port(Direction::Output, &format!("{name} Out"));
        }
        shell.use_midi_backend(Box::new(test.clone()));
        let clock = control.clock().expect("manual pacing");
        shell.use_test_clock(clock.clone());
        shell.start_audio();
        Self {
            shell,
            control,
            test,
            clock,
        }
    }

    fn run(&mut self, id: &str, args: Value) -> Value {
        self.shell
            .run(id, args)
            .unwrap_or_else(|error| panic!("{id}: {error}"))
    }

    fn set(&mut self, row: u8, surface: &str, input: &str, output: &str) -> Value {
        self.run(
            "control_surface.set",
            json!({ "row": row, "surface": surface, "input": input, "output": output }),
        )
    }

    fn rows(&mut self) -> Vec<Value> {
        self.run("control_surface.list", json!({}))["rows"]
            .as_array()
            .unwrap()
            .clone()
    }

    fn journal(&self, row: usize) -> Journal {
        self.shell
            .midi()
            .surfaces()
            .probe(row)
            .expect("the row runs a test surface")
            .downcast::<Journal>()
            .map(|journal| (*journal).clone())
            .unwrap_or_else(|_| panic!("the probe is a journal"))
    }

    /// Sends a message from a port, lets the host take it and runs what it
    /// asked for.
    fn send(&mut self, port: &str, bytes: &[u8]) {
        assert!(self.test.send(port, self.clock.now(), bytes));
        self.shell.midi().surfaces().settle();
        self.shell.poll();
    }

    fn pass(&mut self, millis: u64) {
        testing::advance(&self.control, &mut self.shell, millis);
        self.shell.poll();
    }

    fn track(&mut self) -> TrackId {
        self.shell
            .invoke("track.create", json!({ "kind": "audio", "name": "T" }))
            .unwrap();
        self.shell.editor().document().top_level().last().unwrap()
    }

    fn volume(&self, track: TrackId) -> f64 {
        self.shell
            .editor()
            .document()
            .track(track)
            .unwrap()
            .mixer()
            .volume
    }

    fn steps(&self) -> usize {
        self.shell.editor().undo_history().count()
    }
}

// Spec: control-surfaces R1, R3, R9, R11, R13, R14. `set`, `list`, `dump` and
// `surfaces` through the shell's own command path, read back.
#[test]
fn rows_are_set_listed_dumped_and_stopped_by_command() {
    let mut rig = Rig::new();
    let surfaces = rig.run("control_surface.surfaces", json!({}));
    // The native surfaces come first in alphabetical order (the APCs, the
    // Launchpads and Push 2), then the test surfaces, which only test builds
    // offer.
    let listed: Vec<Value> = surfaces["surfaces"].as_array().unwrap().clone();
    let ids: Vec<&str> = listed.iter().map(|s| s["id"].as_str().unwrap()).collect();
    assert_eq!(
        ids,
        [
            "apc-mini-mk2",
            "apc40-mkii",
            "launchpad-mini-mk3",
            "launchpad-pro-mk3",
            "launchpad-x",
            "push-2",
            "test",
            "test-instant",
            "template:Generic 8 + 8"
        ]
    );
    assert_eq!(
        listed[6..],
        [
            json!({
                "id": "test", "name": "Test Surface", "needs_output": true, "can_lock": false,
                "ring": { "tracks": 8, "scenes": 2 }, "has_setup": true
            }),
            json!({
                "id": "test-instant", "name": "Test Instant Surface", "needs_output": true,
                "can_lock": true, "ring": { "tracks": 8, "scenes": 2 }, "has_setup": false
            }),
            // The bundled controller template comes after the native surfaces.
            json!({
                "id": "template:Generic 8 + 8", "name": "Generic 8 + 8", "needs_output": true,
                "can_lock": true, "ring": { "tracks": 8, "scenes": 2 }, "has_setup": false
            })
        ]
    );
    let rows = rig.rows();
    assert_eq!(rows.len(), 6);
    assert!(
        rows.iter()
            .all(|r| r["state"] == "empty" && r["surface"].is_null())
    );
    assert_eq!(rows[5]["row"], 6);

    // Spec: R12. Rows are no part of the set and no undo step.
    let (steps, doc) = (rig.steps(), rig.shell.editor().document().clone());
    let set = rig.set(1, "test", "Pads", "Pads Out");
    assert_eq!(set["state"], "running");
    assert_eq!(rig.steps(), steps);
    assert!(*rig.shell.editor().document() == doc);
    let rows = rig.rows();
    assert_eq!(rows[0], set, "list reads back what set did");
    assert_eq!(
        rows[0]["surface"],
        json!({ "id": "test", "name": "Test Surface" })
    );
    assert_eq!(rows[0]["input"], json!({ "name": "Pads", "present": true }));
    assert_eq!(rows[0]["can_dump"], true);
    let ring = &rows[0]["surface_state"]["ring"];
    assert_eq!((&ring["tracks"], &ring["scenes"]), (&json!(8), &json!(2)));
    assert_eq!(rig.test.sent("Pads Out"), [HELLO]);

    // A port serves one row, and the refusal names it.
    let refused = rig
        .shell
        .run(
            "control_surface.set",
            json!({ "row": 2, "surface": "test", "input": "Pads" }),
        )
        .unwrap_err();
    assert!(refused.to_string().contains("row 1"), "{refused}");
    assert!(matches!(
        rig.shell.run("control_surface.set", json!({ "row": 7 })),
        Err(ShellError::InvalidArgs { .. })
    ));
    assert!(matches!(
        rig.shell.run("control_surface.dump", json!({ "row": 2 })),
        Err(ShellError::Failed { .. })
    ));

    // Dump sends the setup and changes nothing in the set.
    let doc = rig.shell.editor().document().clone();
    assert_eq!(
        rig.run("control_surface.dump", json!({ "row": 1 })),
        json!({ "sent": 1 })
    );
    assert_eq!(rig.test.sent("Pads Out").last().unwrap(), SETUP);
    assert!(*rig.shell.editor().document() == doc);

    // None stops the surface, which says goodbye.
    let cleared = rig.run("control_surface.set", json!({ "row": 1, "surface": null }));
    assert_eq!(cleared["state"], "empty");
    assert_eq!(cleared["input"]["name"], "Pads", "the ports stay");
    assert_eq!(rig.test.sent("Pads Out").last().unwrap(), GOODBYE);
}

// Spec: control-surfaces R2, R9. The rows are kept in the preferences by the
// surface's ID and the ports' names: after a relaunch they apply again, a
// missing port reads as missing by name, and the surface starts when a port
// of that name appears.
#[test]
fn rows_survive_a_relaunch_and_wait_for_missing_ports() {
    let dir = std::env::temp_dir().join(format!("bl-shell-surfaces-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let start = |dir: &Path, ports: &[(Direction, &str)]| {
        let prefs = Preferences::load(dir).unwrap();
        let test = TestBackend::new();
        for (direction, name) in ports {
            test.add_port(*direction, name);
        }
        let mut shell = Shell::new(
            prefs.clone(),
            Some(dir.to_owned()),
            AudioHost::null(prefs.audio),
            Box::new(ProjectFiles::new()),
        );
        shell.use_midi_backend(Box::new(test.clone()));
        (shell, test)
    };
    let ports = [(Direction::Input, "Pads"), (Direction::Output, "Pads Out")];
    let (mut shell, _) = start(&dir, &ports);
    let fresh = shell.run("control_surface.list", json!({})).unwrap();
    assert!(
        fresh["rows"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["state"] == "empty")
    );
    shell
        .run(
            "control_surface.set",
            json!({ "row": 3, "surface": "test", "input": "Pads", "output": "Pads Out" }),
        )
        .unwrap();
    drop(shell);

    // Relaunched with the ports gone.
    let (mut shell, test) = start(&dir, &[]);
    let rows = shell.run("control_surface.list", json!({})).unwrap();
    let row = &rows["rows"][2];
    assert_eq!(row["surface"]["id"], "test");
    assert_eq!(row["input"], json!({ "name": "Pads", "present": false }));
    assert_eq!(row["state"], "waiting");
    // A port of that name appears: the surface starts.
    test.add_port(Direction::Input, "Pads");
    test.add_port(Direction::Output, "Pads Out");
    assert!(shell.refresh_midi());
    let rows = shell.run("control_surface.list", json!({})).unwrap();
    assert_eq!(rows["rows"][2]["state"], "running");
    assert_eq!(test.sent("Pads Out"), [HELLO]);
    drop(shell);

    // Relaunched with the ports there: running at once, and the hello is
    // sent again.
    let (mut shell, test) = start(&dir, &ports);
    let rows = shell.run("control_surface.list", json!({})).unwrap();
    assert_eq!(rows["rows"][2]["state"], "running");
    assert_eq!(test.sent("Pads Out"), [HELLO]);
    drop(shell);
    assert_eq!(test.sent("Pads Out"), [HELLO, GOODBYE], "quitting stops it");
    let _ = std::fs::remove_dir_all(&dir);
}

// Spec: control-surfaces R7. A knob turned three times and released is one
// undo step, named as the window names the command's, and the person's
// gesture lasts until then.
#[test]
fn a_knob_turned_three_times_and_released_is_one_undo_step() {
    let mut rig = Rig::new();
    let track = rig.track();
    rig.set(1, "test", "Pads", "Pads Out");
    rig.journal(0)
        .knob("mixer.set_volume", json!({ "track": track, "volume": "$" }));
    let steps = rig.steps();
    assert_eq!(rig.volume(track), 1.0);

    for value in [20, 40, 60] {
        rig.send("Pads", &[0xB0, 7, value]);
    }
    assert!(
        rig.shell.gesture_under_way(),
        "agents' calls wait while the knob is held"
    );
    assert!((rig.volume(track) - 60.0 / 127.0).abs() < 1e-9);
    rig.send("Pads", &[0x80, 7, 0]);
    assert!(!rig.shell.gesture_under_way());
    assert_eq!(
        rig.steps(),
        steps + 1,
        "three turns and a release are one step"
    );
    assert_eq!(
        rig.shell.editor().undo_history().next().unwrap(),
        "Set Track Volume"
    );

    rig.run("edit.undo", json!({}));
    assert_eq!(rig.volume(track), 1.0);
    assert_eq!(rig.shell.surface_counts(), (4, 0));
}

// Spec: control-surfaces R7. A control with no release ends its step after
// 500 ms of rest, and another edit ends it too.
#[test]
fn a_control_with_no_release_rests_for_half_a_second() {
    let mut rig = Rig::new();
    let track = rig.track();
    rig.set(1, "test", "Pads", "Pads Out");
    rig.journal(0)
        .knob("mixer.set_volume", json!({ "track": track, "volume": "$" }));
    let steps = rig.steps();

    rig.send("Pads", &[0xB0, 7, 30]);
    rig.pass(300);
    rig.send("Pads", &[0xB0, 7, 50]);
    rig.pass(400);
    assert!(
        rig.shell.gesture_under_way(),
        "300 ms and 400 ms are not 500 ms of rest"
    );
    rig.pass(200);
    assert!(!rig.shell.gesture_under_way());
    assert_eq!(rig.steps(), steps + 1);

    // A turn after the rest is a step of its own.
    rig.send("Pads", &[0xB0, 7, 70]);
    rig.pass(600);
    assert_eq!(rig.steps(), steps + 2);

    // Another edit ends a held gesture first.
    rig.send("Pads", &[0xB0, 7, 90]);
    rig.track();
    assert!(!rig.shell.gesture_under_way());
    assert_eq!(rig.steps(), steps + 4);
}

// Spec: control-surfaces R7. A command a surface asks for is refused like
// the same command from a window; the others still run.
#[test]
fn a_refused_command_is_counted_and_the_next_one_runs() {
    let mut rig = Rig::new();
    let track = rig.track();
    rig.set(1, "test", "Pads", "Pads Out");
    rig.journal(0).knob(
        "mixer.set_volume",
        json!({ "track": 99_999, "volume": 0.5 }),
    );
    rig.send("Pads", &[0xB0, 7, 1]);
    assert_eq!(
        rig.shell.surface_counts(),
        (0, 1),
        "the set has no such track"
    );
    rig.journal(0).knob(
        "mixer.set_volume",
        json!({ "track": track, "volume": 0.25 }),
    );
    rig.send("Pads", &[0xB0, 7, 1]);
    assert_eq!(rig.shell.surface_counts(), (1, 1));
    assert!((rig.volume(track) - 0.25).abs() < 1e-9);
}

// Spec: control-surfaces R15. A message a surface takes lights the remote
// input indicator, and one it sends lights the remote output indicator.
#[test]
fn a_surfaces_traffic_lights_the_remote_indicators() {
    let mut rig = Rig::new();
    rig.set(1, "test", "Pads", "Pads Out");
    rig.shell.poll();
    assert_eq!(
        rig.shell.remote_indicators(),
        (false, true),
        "its hello left"
    );
    rig.send("Pads", &[0x90, 36, 100]);
    assert_eq!(rig.shell.remote_indicators(), (true, true));
}

// Spec: control-surfaces R7. A surface's command joins a drag of the
// person's that is under way, and the surface's release doesn't end it.
#[test]
fn a_surface_does_not_end_a_drag_of_the_persons() {
    let mut rig = Rig::new();
    let track = rig.track();
    rig.set(1, "test", "Pads", "Pads Out");
    rig.journal(0)
        .knob("mixer.set_volume", json!({ "track": track, "volume": "$" }));
    rig.shell.begin_gesture("Drag");
    rig.send("Pads", &[0xB0, 7, 64]);
    rig.send("Pads", &[0x80, 7, 0]);
    assert!(
        rig.shell.gesture_under_way(),
        "the drag is still the person's"
    );
    rig.shell.end_gesture();
    assert!(!rig.shell.gesture_under_way());
}

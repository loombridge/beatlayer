//! Push 2 in the shell (`docs/specs/push-2.md`, R1, R2, R9, R10, R27, R31 and
//! R34): plugging the simulated device in fills the first empty row, None
//! keeps it out across a relaunch, and the controls run the app's own
//! commands as one undo step each.

use std::path::Path;
use std::time::Duration;

use bl_audio_io::NullControl;
use bl_core::clock::TestClock;
use bl_midi_io::{Direction, TestBackend};
use bl_push::controls;
use bl_push::sim::{LIVE, Simulated};
use bl_push::sysex::{self, MidiMode};
use bl_shell::audio::AudioHost;
use bl_shell::files::ProjectFiles;
use bl_shell::settings::Preferences;
use bl_shell::{Shell, testing};
use serde_json::{Value, json};

struct Rig {
    shell: Shell,
    _control: NullControl,
    test: TestBackend,
    clock: TestClock,
    sim: Simulated,
}

impl Rig {
    fn new() -> Self {
        let (mut shell, control) = testing::rig();
        let test = TestBackend::new();
        shell.use_midi_backend(Box::new(test.clone()));
        let clock = control.clock().expect("manual pacing");
        shell.use_test_clock(clock.clone());
        let sim = Simulated::new(&test, &clock);
        shell.use_push_display(sim.frames().claim());
        shell.start_audio();
        Self {
            shell,
            _control: control,
            test,
            clock,
            sim,
        }
    }

    fn run(&mut self, id: &str, args: Value) -> Value {
        self.shell
            .run(id, args)
            .unwrap_or_else(|error| panic!("{id}: {error}"))
    }

    /// Lets the host and the device talk until both are quiet.
    fn settle(&mut self) {
        loop {
            self.shell.midi().surfaces().settle();
            if self.sim.pump() == 0 {
                break;
            }
        }
    }

    /// Lists the ports again, then settles.
    fn refresh(&mut self) {
        self.shell.refresh_midi();
        self.settle();
    }

    /// One poll, one tick of the surfaces, and the commands they asked for.
    fn step(&mut self) {
        self.shell.poll();
        self.shell.midi().surfaces().tick();
        self.settle();
        self.shell.poll();
    }

    fn pass(&mut self, millis: u64) {
        self.clock.advance(Duration::from_millis(millis));
        self.step();
    }

    fn rows(&mut self) -> Vec<Value> {
        self.run("control_surface.list", json!({}))["rows"]
            .as_array()
            .unwrap()
            .clone()
    }

    fn state(&mut self) -> Value {
        self.run("push.state", json!({}))["pushes"][0].clone()
    }

    fn tap(&mut self, name: &str) {
        assert!(self.sim.press(name));
        self.pass(50);
        assert!(self.sim.let_go(name));
        self.pass(1);
    }

    fn steps(&self) -> usize {
        self.shell.editor().undo_history().count()
    }
}

// Spec: push-2 R1, R2. Plugging a Push 2 in takes the first empty row with
// its Live port as input and output, and starts it with R4's sequence;
// the chooser offers it by name.
#[test]
fn a_push_2_that_appears_takes_the_first_empty_row_and_starts() {
    let mut rig = Rig::new();
    let surfaces = rig.run("control_surface.surfaces", json!({}));
    let push = surfaces["surfaces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"] == "push-2")
        .expect("the chooser offers it");
    assert_eq!(push["name"], "Push 2");
    assert_eq!(push["ring"], json!({ "tracks": 8, "scenes": 8 }));

    // Row 1 is taken by another surface; the Push 2 goes to row 2.
    rig.test.add_port(Direction::Input, "Other");
    rig.test.add_port(Direction::Output, "Other Out");
    rig.shell.refresh_midi();
    rig.run(
        "control_surface.set",
        json!({ "row": 1, "surface": "test", "input": "Other", "output": "Other Out" }),
    );
    rig.sim.plug_in();
    rig.refresh();
    let rows = rig.rows();
    assert_eq!(rows[0]["surface"]["id"], "test");
    assert_eq!(
        rows[1]["surface"],
        json!({ "id": "push-2", "name": "Push 2" })
    );
    assert_eq!(rows[1]["input"]["name"], LIVE);
    assert_eq!(rows[1]["output"]["name"], LIVE);
    assert_eq!(rows[1]["state"], "running");
    assert_eq!(rows[2]["state"], "empty");
    let sysex = rig.sim.sysex();
    assert_eq!(sysex[0], sysex::DEVICE_INQUIRY);
    assert_eq!(sysex[1], sysex::midi_mode(MidiMode::Live));
    assert_eq!(sysex[2], sysex::request_statistics());

    // A relist doesn't take another row for the same Push 2.
    rig.refresh();
    let rows = rig.rows();
    assert_eq!(
        rows.iter()
            .filter(|r| r["surface"]["id"] == "push-2")
            .count(),
        1
    );
}

// Spec: push-2 R2. Setting the row to None stops the surface (R4's stop) and
// keeps it from taking a row again, across a relaunch with real preference
// files, until the user chooses it.
#[test]
fn none_in_the_row_stops_it_and_keeps_it_stopped_across_a_relaunch() {
    let dir = std::env::temp_dir().join(format!("bl-shell-push-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let start = |dir: &Path| {
        let prefs = Preferences::load(dir).unwrap();
        let test = TestBackend::new();
        let clock = TestClock::new();
        let sim = Simulated::new(&test, &clock);
        let mut shell = Shell::new(
            prefs.clone(),
            Some(dir.to_owned()),
            AudioHost::null(prefs.audio),
            Box::new(ProjectFiles::new()),
        );
        shell.use_push_display(sim.frames().claim());
        shell.use_midi_backend(Box::new(test.clone()));
        (shell, sim)
    };
    let settle = |shell: &Shell, sim: &mut Simulated| loop {
        shell.midi().surfaces().settle();
        if sim.pump() == 0 {
            break;
        }
    };

    let (mut shell, mut sim) = start(&dir);
    sim.plug_in();
    shell.refresh_midi();
    settle(&shell, &mut sim);
    let rows = shell.run("control_surface.list", json!({})).unwrap();
    assert_eq!(rows["rows"][0]["state"], "running");

    sim.forget_sent();
    shell
        .run("control_surface.set", json!({ "row": 1, "surface": null }))
        .unwrap();
    settle(&shell, &mut sim);
    let rows = shell.run("control_surface.list", json!({})).unwrap();
    assert_eq!(rows["rows"][0]["state"], "empty");
    let sent = sim.sent();
    assert_eq!(sent.len(), 64 + controls::BUTTONS.len() + 2, "R4's stop");
    assert_eq!(sent[sent.len() - 2], sysex::display_brightness(0));
    assert_eq!(sent[sent.len() - 1], sysex::midi_mode(MidiMode::Live));
    shell.refresh_midi();
    let rows = shell.run("control_surface.list", json!({})).unwrap();
    assert_eq!(rows["rows"][0]["state"], "empty", "not taken again");
    drop(shell);

    // Relaunched with the Push 2 plugged in: still no row.
    let (mut shell, mut sim) = start(&dir);
    sim.plug_in();
    shell.refresh_midi();
    settle(&shell, &mut sim);
    let rows = shell.run("control_surface.list", json!({})).unwrap();
    assert!(
        rows["rows"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["state"] == "empty")
    );
    assert!(sim.sysex().is_empty(), "nothing was sent to it");

    // Choosing it again starts it, and a later launch takes it again.
    shell
        .run(
            "control_surface.set",
            json!({ "row": 1, "surface": "push-2", "input": LIVE, "output": LIVE }),
        )
        .unwrap();
    settle(&shell, &mut sim);
    let rows = shell.run("control_surface.list", json!({})).unwrap();
    assert_eq!(rows["rows"][0]["state"], "running");
    drop(shell);
    let (mut shell, mut sim) = start(&dir);
    sim.plug_in();
    shell.refresh_midi();
    settle(&shell, &mut sim);
    let rows = shell.run("control_surface.list", json!({})).unwrap();
    assert_eq!(rows["rows"][0]["state"], "running");
    assert!(
        rows["rows"].as_array().unwrap()[1..]
            .iter()
            .all(|r| r["state"] == "empty")
    );
    drop(shell);
    let _ = std::fs::remove_dir_all(&dir);
}

// Spec: push-2 R2. With every row taken the surface doesn't start and the
// Status Bar says why.
#[test]
fn with_every_row_taken_it_doesnt_start_and_the_status_bar_says_why() {
    let mut rig = Rig::new();
    for n in 0..6 {
        rig.test.add_port(Direction::Input, &format!("In {n}"));
        rig.test.add_port(Direction::Output, &format!("Out {n}"));
    }
    rig.shell.refresh_midi();
    for n in 0..6u8 {
        rig.run(
            "control_surface.set",
            json!({ "row": n + 1, "surface": "test", "input": format!("In {n}"), "output": format!("Out {n}") }),
        );
    }
    rig.sim.plug_in();
    rig.refresh();
    rig.shell.poll();
    assert!(rig.rows().iter().all(|r| r["surface"]["id"] == "test"));
    assert!(rig.sim.sysex().is_empty());
    let message = rig.shell.message().expect("the Status Bar says why");
    assert!(message.text.contains("all six rows"), "{}", message.text);
}

// Spec: push-2 R34. `push.state` reads the modes, the LEDs, the display's
// text and the Setup values; `push.set_mode` and `push.set_setup` change them
// and are read back.
#[test]
fn push_state_reads_back_the_modes_the_leds_the_display_and_the_setup() {
    let mut rig = Rig::new();
    rig.sim.plug_in();
    rig.refresh();
    rig.step();
    let state = rig.state();
    assert_eq!(state["row"], 1);
    assert_eq!(state["firmware"], "1.0.47");
    assert_eq!(state["external_power"], true);
    assert_eq!(
        (state["pad_mode"].as_str(), state["display_mode"].as_str()),
        (Some("note"), Some("device"))
    );
    assert_eq!(state["midi_mode"], "live");
    assert_eq!(state["setup"]["pad_gain"], 5);
    assert_eq!(state["touch_strip"]["configuration"], "pitch_bend");
    assert_eq!(state["touch_strip"]["flags"], 0x68);
    assert_eq!(state["leds"]["note"]["colour"], 127);
    assert_eq!(state["leds"]["session"]["colour"], 16);
    assert_eq!(state["leds"]["pad_0_0"]["colour"], 0);
    assert_eq!(
        state["leds"].as_object().unwrap().len(),
        64 + controls::BUTTONS.len()
    );
    let text: Vec<&str> = state["display"]["text"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l.as_str().unwrap())
        .collect();
    assert!(text.contains(&"title: Device"), "{text:?}");

    // push.set_mode, as the buttons do.
    let set = rig.run(
        "push.set_mode",
        json!({ "pad": "session", "display": "mix", "mix": "global" }),
    );
    assert_eq!(set["rows"], json!([1]));
    rig.step();
    let state = rig.state();
    assert_eq!(
        (
            state["pad_mode"].as_str(),
            state["display_mode"].as_str(),
            state["mix"].as_str()
        ),
        (Some("session"), Some("mix"), Some("global"))
    );
    assert!(rig.shell.run("push.set_mode", json!({ "row": 9 })).is_err());

    // push.set_setup: a preference, read back, and sent to the device.
    let steps = rig.steps();
    let setup = rig.run(
        "push.set_setup",
        json!({ "pad_sensitivity": 8, "led_brightness": 50, "workflow": "clip" }),
    );
    assert_eq!(setup["pad_sensitivity"], 8);
    rig.step();
    let state = rig.state();
    assert_eq!(state["setup"]["pad_sensitivity"], 8);
    assert_eq!(state["setup"]["workflow"], "clip");
    assert_eq!(rig.sim.device.led_brightness, 64);
    assert_eq!(rig.sim.device.curve, bl_push::curve::curve(8, 5, 5));
    assert_eq!(rig.steps(), steps, "no part of the set, no undo step");

    // push.display_frame: a PNG of the display.
    let frame = rig.run("push.display_frame", json!({}));
    assert_eq!(
        (frame["width"].as_u64(), frame["height"].as_u64()),
        (Some(960), Some(160))
    );
    assert!(
        frame["png"].as_str().unwrap().starts_with("iVBOR"),
        "a PNG in base64"
    );
}

// Spec: push-2 R31. The Setup values are preferences: they come back after a
// relaunch.
#[test]
fn the_setup_values_and_the_swing_amount_survive_a_relaunch() {
    let dir = std::env::temp_dir().join(format!("bl-shell-push-setup-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let start = |dir: &Path| {
        let prefs = Preferences::load(dir).unwrap();
        let mut shell = Shell::new(
            prefs.clone(),
            Some(dir.to_owned()),
            AudioHost::null(prefs.audio),
            Box::new(ProjectFiles::new()),
        );
        shell.use_midi_backend(Box::new(TestBackend::new()));
        shell
    };
    let mut shell = start(&dir);
    shell
        .run(
            "push.set_setup",
            json!({ "pad_dynamics": 2, "display_light": 40 }),
        )
        .unwrap();
    shell.midi().push().set_swing(0.25);
    shell.poll();
    drop(shell);
    let shell = start(&dir);
    let setup = shell.midi().push().setup();
    assert_eq!((setup.dynamics, setup.display_light), (2, 40));
    assert!((shell.midi().push().swing() - 0.25).abs() < 1e-9);
    let _ = std::fs::remove_dir_all(&dir);
}

// Spec: push-2 R9, R10. The master encoder sets Main's volume, and a touch is
// one undo step however many steps it turned.
#[test]
fn a_touch_of_the_master_encoder_is_one_undo_step() {
    let mut rig = Rig::new();
    rig.sim.plug_in();
    rig.refresh();
    rig.step();
    let steps = rig.steps();
    let main = rig.shell.editor().document().main_id();
    assert_eq!(
        rig.shell
            .editor()
            .document()
            .track(main)
            .unwrap()
            .mixer()
            .volume,
        1.0
    );

    assert!(rig.sim.touch(controls::MASTER));
    for _ in 0..3 {
        assert!(rig.sim.turn(controls::MASTER, -6));
        rig.pass(10);
    }
    assert!(
        rig.shell.gesture_under_way(),
        "agents' calls wait while the encoder is touched"
    );
    assert!(rig.sim.release(controls::MASTER));
    rig.pass(10);
    assert!(!rig.shell.gesture_under_way());
    assert_eq!(rig.steps(), steps + 1, "three turns, one undo step");
    let volume = rig
        .shell
        .editor()
        .document()
        .track(main)
        .unwrap()
        .mixer()
        .volume;
    assert!(volume < 1.0, "Main's volume went down: {volume}");
    rig.run("edit.undo", json!({}));
    assert_eq!(
        rig.shell
            .editor()
            .document()
            .track(main)
            .unwrap()
            .mixer()
            .volume,
        1.0
    );
}

// Spec: push-2 R12, R26, R27. Transport buttons run the app's commands, a
// lower display button selects the ring's track, and the colour picker
// colours a track as one undo step.
#[test]
fn the_buttons_run_the_apps_commands_and_the_picker_colours_a_track() {
    let mut rig = Rig::new();
    rig.shell
        .invoke("track.create", json!({ "kind": "midi", "name": "Keys" }))
        .unwrap();
    rig.sim.plug_in();
    rig.refresh();
    rig.step();
    let track = rig.shell.editor().document().top_level().last().unwrap();

    // Play/Stop starts the transport; Metronome switches the metronome.
    rig.tap("play");
    rig.tap("metronome");
    assert!(rig.shell.editor().document().settings().metronome.enabled);
    rig.tap("undo");
    assert!(
        !rig.shell.editor().document().settings().metronome.enabled,
        "Undo undoes the last action"
    );

    // The track's name and colour label its lower button.
    let text = rig.state()["display"]["text"].clone();
    assert!(
        text.as_array()
            .unwrap()
            .iter()
            .any(|l| l.as_str().unwrap().starts_with("lower 5: Keys")),
        "{text}"
    );

    // A lower button selects it: the new track is the fifth of the ring.
    rig.tap("lower_5");
    assert_eq!(rig.shell.arrangement().selected_tracks(), [track]);

    // Shift + lower_5, then swatch 9: one undo step, and the colour is the swatch.
    let steps = rig.steps();
    assert!(rig.sim.press("shift"));
    rig.pass(10);
    rig.tap("lower_5");
    assert!(rig.sim.let_go("shift"));
    rig.pass(10);
    assert_eq!(rig.steps(), steps, "opening the picker changes nothing");
    assert_eq!(rig.state()["colour_picker"], true);
    assert!(rig.sim.press_pad(7, 2, 100));
    assert!(rig.sim.release_pad(7, 2));
    rig.pass(10);
    assert_eq!(
        rig.shell.editor().document().track(track).unwrap().colour(),
        9
    );
    assert_eq!(rig.steps(), steps + 1);
    assert_eq!(rig.state()["colour_picker"], false);
}

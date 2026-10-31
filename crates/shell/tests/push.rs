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
    control: NullControl,
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
            control,
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

impl Rig {
    /// A Push 2 running, with a new MIDI track selected.
    fn browsing() -> Self {
        let mut rig = Self::new();
        rig.shell
            .invoke("track.create", json!({ "kind": "midi", "name": "Keys" }))
            .unwrap();
        rig.sim.plug_in();
        rig.refresh();
        rig.step();
        rig
    }

    /// The columns of Browse Mode, as `push.state` reads them.
    fn columns(&mut self) -> Vec<Value> {
        self.state()["browse"]["columns"]
            .as_array()
            .unwrap()
            .clone()
    }

    fn selected_in(&mut self, column: usize) -> Value {
        self.columns()[column]["entries"]
            [self.columns()[column]["selected"].as_u64().unwrap() as usize]
            .clone()
    }

    /// The devices on `track`, by name.
    fn devices(&self, track: bl_model::TrackId) -> Vec<String> {
        let doc = self.shell.editor().document();
        doc.track(track)
            .unwrap()
            .devices()
            .map(|d| d.title().to_owned())
            .collect()
    }

    /// Moves the column in use, which is `column`, to `label` and opens it.
    fn open_label(&mut self, column: usize, label: &str) {
        for _ in 0..13 {
            if self.selected_in(column) == label {
                break;
            }
            self.tap("down");
        }
        assert_eq!(self.selected_in(column), label);
        self.tap("upper_8");
    }
}

// Spec: push-2-tracks R5, R9, R11, R14, R2. Browse on an empty MIDI track
// lists everything a MIDI track takes and changes nothing; Load adds the
// chosen device as one undo step, and a second Load replaces what the first
// added, each one undo step, with Push 2 staying in Browse Mode.
#[test]
fn load_adds_to_an_empty_midi_track_and_the_next_load_replaces() {
    let mut rig = Rig::browsing();
    let track = rig.shell.arrangement().selected_tracks()[0];
    let steps = rig.steps();
    rig.tap("browse");
    let browse = rig.state()["browse"].clone();
    assert_eq!(browse["load"], "add");
    let labels: Vec<_> = browse["columns"][0]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l.as_str().unwrap().to_owned())
        .collect();
    assert_eq!(
        &labels[..6],
        [
            "Sounds",
            "Drums",
            "Instruments",
            "Audio Effects",
            "MIDI Effects",
            "Plug-Ins"
        ]
    );
    assert_eq!(labels[6], "Favorites", "the collections follow");
    assert_eq!(rig.steps(), steps, "Browse Mode adds no undo step");

    rig.open_label(0, "Instruments");
    let first = rig.selected_in(1);
    rig.tap("lower_8");
    rig.step();
    assert_eq!(rig.steps(), steps + 1);
    assert_eq!(rig.devices(track).len(), 1);
    assert_eq!(
        rig.state()["browse"]["load"],
        "replace",
        "the loaded device is next"
    );
    assert_eq!(rig.state()["display_mode"], "browse");

    // The next entry replaces it: still one device, one more undo step.
    rig.tap("down");
    let second = rig.selected_in(1);
    assert_ne!(first, second);
    rig.tap("lower_8");
    rig.step();
    assert_eq!(rig.steps(), steps + 2);
    assert_eq!(rig.devices(track).len(), 1);
    rig.run("edit.undo", json!({}));
    assert_eq!(rig.devices(track).len(), 1, "undo brings the first back");
    rig.run("edit.undo", json!({}));
    assert!(rig.devices(track).is_empty());
}

// Spec: push-2-tracks R5. An audio track lists audio effects, and a Load
// with one selected replaces it.
#[test]
fn an_audio_track_takes_audio_effects_and_load_replaces_the_selected_one() {
    let mut rig = Rig::browsing();
    rig.shell
        .invoke("track.create", json!({ "kind": "audio" }))
        .unwrap();
    let track = rig.shell.arrangement().selected_tracks()[0];
    rig.step();
    rig.tap("browse");
    assert_eq!(rig.columns()[0]["entries"][0], "Audio Effects");
    assert_eq!(rig.columns()[0]["entries"][1], "Plug-Ins");
    rig.open_label(0, "Audio Effects");
    rig.tap("upper_8");
    let steps = rig.steps();
    rig.tap("lower_8");
    rig.step();
    assert_eq!(rig.devices(track).len(), 1);
    assert_eq!(rig.steps(), steps + 1);
    rig.tap("down");
    rig.tap("lower_8");
    rig.step();
    assert_eq!(rig.devices(track).len(), 1, "replaced, not added");
    assert_eq!(rig.steps(), steps + 2);
}

// Spec: push-2-tracks R12, R1. Add Track inserts a MIDI track with an
// instrument after the selected track, as one undo step; selected in a group
// it goes inside the group.
#[test]
fn add_track_inserts_a_track_with_a_device_after_the_selected_one_in_its_group() {
    let mut rig = Rig::browsing();
    let keys = rig.shell.arrangement().selected_tracks()[0];
    let before = rig.shell.editor().document().top_level().len();
    let steps = rig.steps();
    rig.tap("add_track");
    assert_eq!(rig.state()["browse"]["load"], "add_track");
    assert_eq!(
        rig.columns()[0]["entries"],
        json!(["MIDI", "Audio", "Return"])
    );
    rig.tap("upper_8");
    rig.open_label(1, "Instruments");
    rig.tap("lower_8");
    rig.step();
    let doc = rig.shell.editor().document();
    let top: Vec<_> = doc.top_level().collect();
    assert_eq!(top.len(), before + 1);
    let new = top[top.iter().position(|t| *t == keys).unwrap() + 1];
    assert_eq!(doc.track(new).unwrap().kind(), bl_model::TrackKind::Midi);
    assert_eq!(rig.shell.arrangement().selected_tracks(), [new]);
    assert_eq!(rig.devices(new).len(), 1);
    assert_eq!(
        rig.steps(),
        steps + 1,
        "the track and its device are one step"
    );
    assert_eq!(rig.state()["browse"]["load"], "replace");
    rig.run("edit.undo", json!({}));
    assert_eq!(rig.shell.editor().document().top_level().len(), before);

    // In a group: the new track goes inside it, after the selected one.
    rig.run("arrangement.select_tracks", json!({ "tracks": [keys] }));
    rig.run("track.group", json!({}));
    let group = rig.shell.editor().document().parent(keys).expect("grouped");
    rig.run("arrangement.select_tracks", json!({ "tracks": [keys] }));
    rig.step();
    rig.tap("add_track");
    rig.tap("lower_8");
    rig.step();
    let doc = rig.shell.editor().document();
    let children: Vec<_> = doc.track(group).unwrap().children().collect();
    assert_eq!(children.len(), 2);
    assert_eq!(children[0], keys);
    assert_eq!(doc.parent(children[1]), Some(group));
    assert_eq!(rig.shell.arrangement().selected_tracks(), [children[1]]);
}

// Spec: push-2-tracks R10. The Preview button switches the browser's own
// Preview switch, which the window reads too, and Shift with the Master
// encoder sets the preview volume `browser.state` reads.
#[test]
fn preview_is_the_browsers_switch_and_shift_master_sets_its_volume() {
    let mut rig = Rig::browsing();
    rig.tap("browse");
    let preview = |rig: &mut Rig| rig.run("browser.state", json!({}))["preview"].clone();
    let was = preview(&mut rig)["on"].as_bool().unwrap();
    rig.tap("lower_7");
    assert_eq!(preview(&mut rig)["on"].as_bool().unwrap(), !was);
    let lit = rig.state()["display"]["text"]
        .as_array()
        .unwrap()
        .iter()
        .any(|l| l.as_str().unwrap().starts_with("lower 7: Preview"));
    assert!(lit);
    let volume = preview(&mut rig)["volume"].as_f64().unwrap();
    assert!(rig.sim.press("shift"));
    rig.pass(10);
    assert!(rig.sim.touch(controls::MASTER));
    assert!(rig.sim.turn(controls::MASTER, -30));
    assert!(rig.sim.release(controls::MASTER));
    rig.pass(10);
    assert!(rig.sim.let_go("shift"));
    rig.pass(10);
    assert!(preview(&mut rig)["volume"].as_f64().unwrap() < volume);
}

// Spec: push-2-tracks R13. The columns read what the browser has indexed and
// fill in as indexing goes on, never waiting for it.
#[test]
fn the_columns_fill_in_as_the_index_does() {
    let mut rig = Rig::browsing();
    let library = std::env::temp_dir().join(format!("bl-push-browse-{}", std::process::id()));
    let folder = library.join("Presets/Audio Effects/Reverb");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("Big Hall.blpreset"), "{}").unwrap();
    rig.shell.set_user_library(Some(library.clone()));
    rig.shell
        .invoke("track.create", json!({ "kind": "audio" }))
        .unwrap();
    rig.step();
    rig.tap("browse");
    rig.open_label(0, "Audio Effects");
    assert!(rig.shell.wait_for_browser_index(Duration::from_secs(20)));
    rig.step();
    rig.step();
    let names: Vec<_> = rig.columns()[1]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n.as_str().unwrap().to_owned())
        .collect();
    assert_eq!(names[0], "Reverb", "the indexed preset folder, first");
    std::fs::remove_dir_all(&library).ok();
}

impl Rig {
    /// A Push 2 running in Session Mode, over a set of `tracks` MIDI tracks
    /// and its eight scenes.
    fn sessions(tracks: usize) -> Self {
        let mut rig = Self::new();
        // An empty set, so a track is the column its test puts it in.
        let doc = rig.shell.editor().document();
        let existing: Vec<_> = doc.top_level().chain(doc.returns()).collect();
        for track in existing {
            rig.run("track.delete", json!({ "track": track }));
        }
        for n in 0..tracks {
            rig.shell
                .invoke(
                    "track.create",
                    json!({ "kind": "midi", "name": format!("T{n}") }),
                )
                .unwrap();
        }
        rig.sim.plug_in();
        rig.refresh();
        rig.step();
        rig.tap("session");
        rig
    }

    fn tracks(&self) -> Vec<bl_model::TrackId> {
        self.shell.editor().document().top_level().collect()
    }

    fn scenes(&self) -> Vec<bl_model::SceneId> {
        self.shell
            .editor()
            .document()
            .scenes()
            .map(|s| s.id)
            .collect()
    }

    /// A new empty MIDI clip in a slot.
    fn clip(&mut self, track: usize, scene: usize) -> bl_model::ClipId {
        let (track, scene) = (self.tracks()[track], self.scenes()[scene]);
        self.run(
            "session.insert_empty_midi_clip",
            json!({ "track": track, "scene": scene }),
        );
        self.shell
            .editor()
            .document()
            .slot(track, scene)
            .expect("the clip is there")
            .id
    }

    /// Presses and releases the pad in row `row` from the top.
    fn pad(&mut self, row: u8, column: u8) {
        self.pad_down(row, column);
        self.pad_up(row, column);
    }

    fn pad_down(&mut self, row: u8, column: u8) {
        assert!(self.sim.press_pad(7 - row, column, 100));
        self.pass(1);
    }

    fn pad_up(&mut self, row: u8, column: u8) {
        assert!(self.sim.release_pad(7 - row, column));
        self.pass(1);
    }

    /// Runs the engine for `millis` of test time, then the surfaces.
    fn engine(&mut self, millis: u64) {
        testing::advance(&self.control, &mut self.shell, millis);
        self.step();
    }

    /// Runs the engine until `done`, then the surfaces twice.
    fn wait(&mut self, what: &str, done: impl FnMut(&mut Shell) -> bool) {
        testing::wait_for(&self.control, &mut self.shell, what, done);
        self.step();
        self.step();
    }

    fn slot_is(&self, track: usize, scene: usize, state: bl_engine::SlotState) -> bool {
        let (track, scene) = (self.tracks()[track], self.scenes()[scene]);
        self.shell
            .session_states()
            .iter()
            .any(|s| s.track == track && s.scene == scene && s.state == state)
    }

    /// Waits until the slot is in `state`.
    fn wait_slot(&mut self, track: usize, scene: usize, state: bl_engine::SlotState) {
        let (track, scene) = (self.tracks()[track], self.scenes()[scene]);
        self.wait("the slot's state", |shell| {
            shell
                .session_states()
                .iter()
                .any(|s| s.track == track && s.scene == scene && s.state == state)
        });
    }

    fn wait_stopped(&mut self) {
        self.wait("every clip to stop", |shell| {
            shell.session_states().is_empty()
        });
    }

    fn pad_led(&mut self, row: u8, column: u8) -> Value {
        self.state()["leds"][format!("pad_{}_{column}", 7 - row)].clone()
    }

    fn clip_colour(&self, track: usize, scene: usize) -> Option<u8> {
        let (track, scene) = (self.tracks()[track], self.scenes()[scene]);
        self.shell
            .editor()
            .document()
            .slot(track, scene)
            .and_then(|c| c.colour)
    }
}

// Spec: push-2-tracks R61, R63, R72. A pad launches its clip on the launch
// grid as `session.state` reads it, and its pad pulses while it plays; a Gate
// clip stops on the release. It works as well with the Arrangement shown as
// with the Session View.
#[test]
fn a_pad_launches_its_clip_on_the_grid_and_a_gate_clip_stops_on_release() {
    let mut rig = Rig::sessions(2);
    let first = rig.shell.main_view();
    rig.clip(0, 0);
    let gate = rig.clip(1, 1);
    rig.run(
        "clip.set_launch_mode",
        json!({ "clip": gate, "mode": "gate" }),
    );
    rig.run(
        "session.set_quantization",
        json!({ "quantization": "1_bar" }),
    );
    rig.run("transport.play", json!({}));
    rig.step();

    for view in 0..2 {
        // The pad is lit in its clip's colour, and still.
        rig.step();
        assert_eq!(rig.pad_led(0, 0)["animation"], Value::Null);
        rig.pad(0, 0);
        rig.engine(20);
        assert!(
            rig.slot_is(0, 0, bl_engine::SlotState::Queued),
            "view {view}"
        );
        let state = rig.run("session.state", json!({}));
        assert_eq!(state["launches"][0]["state"], "queued", "{state}");
        rig.wait_slot(0, 0, bl_engine::SlotState::Playing);
        assert_ne!(rig.pad_led(0, 0)["animation"], Value::Null, "pulses");
        rig.run("session.stop_all", json!({}));
        rig.wait_stopped();

        // A Gate clip plays while the pad is down and stops after it.
        rig.run(
            "session.set_quantization",
            json!({ "quantization": "none" }),
        );
        rig.pad_down(1, 1);
        rig.wait_slot(1, 1, bl_engine::SlotState::Playing);
        rig.pad_up(1, 1);
        rig.wait_stopped();
        rig.run(
            "session.set_quantization",
            json!({ "quantization": "1_bar" }),
        );
        rig.run("view.toggle_session_arrangement", json!({}));
        rig.step();
    }
    assert_eq!(rig.shell.main_view(), first, "both views were tried");
}

// Spec: push-2-tracks R61, R62, R65. An empty pad on an armed track records into
// its slot, and on an unarmed one stops its track; a scene button launches its
// scene, and with Select selects it.
#[test]
fn an_empty_pad_records_and_the_scene_buttons_launch_and_select() {
    let mut rig = Rig::sessions(2);
    rig.run(
        "session.set_quantization",
        json!({ "quantization": "none" }),
    );
    rig.clip(1, 0);
    rig.run("transport.play", json!({}));
    rig.step();
    rig.pad(0, 1);
    rig.wait_slot(1, 0, bl_engine::SlotState::Playing);
    // An unarmed track's empty slot stops its track.
    rig.pad(1, 1);
    rig.wait_stopped();
    // An armed track's empty slot records.
    let tracks = rig.tracks();
    rig.run("mixer.set_arm", json!({ "track": tracks[0], "arm": true }));
    rig.step();
    rig.pad(2, 0);
    rig.wait_slot(0, 2, bl_engine::SlotState::Recording);
    assert_eq!(rig.pad_led(2, 0)["animation"]["colour"], 127, "pulses red");
    rig.run("session.stop_all", json!({}));
    rig.wait("the recording to end", |shell| {
        shell.session_states().is_empty()
    });

    // A scene button launches the scene; with Select it only selects it.
    rig.tap("scene_1");
    rig.wait_slot(1, 0, bl_engine::SlotState::Playing);
    rig.run("session.stop_all", json!({}));
    rig.wait_stopped();
    let scenes = rig.scenes();
    assert!(rig.sim.press("select"));
    rig.pass(10);
    rig.tap("scene_4");
    assert!(rig.sim.let_go("select"));
    rig.pass(10);
    assert!(rig.shell.session_states().is_empty(), "nothing launched");
    let state = rig.run("session.state", json!({}));
    assert_eq!(state["scenes"], json!([scenes[3]]), "{state}");
}

// Spec: push-2-tracks R64, R66, R1. Shift and a pad open the colour picker and a
// ring pad colours the clip; Duplicate copies a clip to two slots; Delete and
// a pad deletes a clip. Each is one undo step.
#[test]
fn colouring_duplicating_and_deleting_clips_are_one_undo_step_each() {
    let mut rig = Rig::sessions(2);
    rig.clip(0, 0);
    let (tracks, scenes) = (rig.tracks(), rig.scenes());
    rig.step();

    let steps = rig.steps();
    assert!(rig.sim.press("shift"));
    rig.pass(10);
    rig.pad(0, 0);
    assert!(rig.sim.let_go("shift"));
    rig.pass(10);
    assert_eq!(rig.state()["colour_picker"], true);
    assert_eq!(rig.steps(), steps, "opening it changes nothing");
    rig.pad(5, 0);
    assert_eq!(rig.clip_colour(0, 0), Some(2));
    assert_eq!(rig.steps(), steps + 1);

    let steps = rig.steps();
    assert!(rig.sim.press("duplicate"));
    rig.pass(10);
    rig.pad(0, 0);
    rig.pad(1, 0);
    rig.pad(3, 1);
    assert!(rig.sim.let_go("duplicate"));
    rig.pass(10);
    let doc = rig.shell.editor().document();
    assert!(doc.slot(tracks[0], scenes[1]).is_some());
    assert!(doc.slot(tracks[1], scenes[3]).is_some());
    assert_eq!(rig.steps(), steps + 2, "one undo step for each copy");

    let steps = rig.steps();
    assert!(rig.sim.press("delete"));
    rig.pass(10);
    rig.pad(1, 0);
    assert!(rig.sim.let_go("delete"));
    rig.pass(10);
    let doc = rig.shell.editor().document();
    assert!(doc.slot(tracks[0], scenes[1]).is_none());
    assert!(doc.slot(tracks[0], scenes[0]).is_some());
    assert_eq!(rig.steps(), steps + 1);
}

// Spec: push-2-tracks R67, R73, session-view R37. The arrows, Octave and Page
// move the ring the window's border shows and `push.state` reads, as far as
// the set reaches.
#[test]
fn the_arrows_move_the_ring_and_the_state_and_the_window_follow() {
    let mut rig = Rig::sessions(12);
    let tracks = rig.tracks();
    let ring = |rig: &mut Rig| rig.run("session.state", json!({}))["rings"][0].clone();
    assert_eq!(ring(&mut rig)["tracks"][0], json!(tracks[0]));
    rig.tap("right");
    assert_eq!(ring(&mut rig)["tracks"][0], json!(tracks[1]));
    assert_eq!(rig.state()["session"]["first_track"], json!(tracks[1]));
    rig.tap("page_right");
    assert_eq!(
        ring(&mut rig)["tracks"][0],
        json!(tracks[4]),
        "as far as the set reaches"
    );
    assert_eq!(rig.state()["leds"]["right"]["colour"], 0, "nothing further");
    rig.tap("page_left");
    rig.tap("left");
    assert_eq!(ring(&mut rig)["tracks"][0], json!(tracks[0]));
    assert_eq!(rig.state()["leds"]["left"]["colour"], 0);
    let scenes = rig.scenes();
    assert_eq!(rig.state()["session"]["first_scene"], json!(scenes[0]));
    assert_eq!(rig.state()["session"]["pads"].as_array().unwrap().len(), 64);
}

// Spec: push-2-tracks R68. Stop Clip stops the selected track's clip; with a
// lower button, that track; with Shift, every clip; locked, the lower
// buttons stop their tracks.
#[test]
fn stop_clip_stops_the_selected_track_a_lower_buttons_track_or_every_clip() {
    let mut rig = Rig::sessions(3);
    rig.run(
        "session.set_quantization",
        json!({ "quantization": "none" }),
    );
    for track in 0..3 {
        rig.clip(track, 0);
    }
    rig.run("transport.play", json!({}));
    let tracks = rig.tracks();
    let launch = |rig: &mut Rig| {
        rig.run("session.launch_scene", json!({ "scene": rig.scenes()[0] }));
        for track in 0..3 {
            rig.wait_slot(track, 0, bl_engine::SlotState::Playing);
        }
    };

    launch(&mut rig);
    rig.run(
        "arrangement.select_tracks",
        json!({ "tracks": [tracks[1]] }),
    );
    rig.step();
    rig.tap("stop_clip");
    rig.wait("the selected track to stop", |shell| {
        shell.session_states().len() == 2
    });
    assert!(!rig.slot_is(1, 0, bl_engine::SlotState::Playing));

    assert!(rig.sim.press("stop_clip"));
    rig.pass(10);
    assert_ne!(rig.state()["leds"]["lower_1"]["animation"], Value::Null);
    assert_eq!(rig.state()["leds"]["lower_2"]["animation"], Value::Null);
    rig.tap("lower_1");
    assert!(rig.sim.let_go("stop_clip"));
    rig.pass(10);
    rig.wait("that track to stop", |shell| {
        shell.session_states().len() == 1
    });

    launch(&mut rig);
    assert!(rig.sim.press("shift"));
    rig.pass(10);
    rig.tap("stop_clip");
    assert!(rig.sim.let_go("shift"));
    rig.pass(10);
    rig.wait_stopped();

    // Locked: the lower buttons stop their tracks.
    launch(&mut rig);
    assert!(rig.sim.press("stop_clip"));
    rig.pass(1000);
    assert!(rig.sim.let_go("stop_clip"));
    rig.pass(10);
    assert_eq!(rig.state()["session"]["stop_clip_locked"], true);
    rig.tap("lower_3");
    rig.wait("the third to stop", |shell| {
        shell.session_states().len() == 2
    });
    rig.tap("lower_2");
    rig.tap("lower_1");
    rig.wait_stopped();
}

// Spec: push-2-tracks R70, R73. Session Overview, held with Layout, lights the
// ring's block white and a block with a clip playing green, and a pad moves
// the ring to its block.
#[test]
fn session_overview_lights_blocks_and_a_pad_moves_the_ring() {
    let mut rig = Rig::sessions(12);
    rig.run(
        "session.set_quantization",
        json!({ "quantization": "none" }),
    );
    rig.clip(10, 0);
    rig.run("transport.play", json!({}));
    rig.run(
        "session.launch",
        json!({ "track": rig.tracks()[10], "scene": rig.scenes()[0] }),
    );
    rig.wait_slot(10, 0, bl_engine::SlotState::Playing);
    assert!(rig.sim.press("layout"));
    rig.pass(300);
    let state = rig.state();
    assert_eq!(state["session"]["overview"], true);
    assert_eq!(state["leds"]["pad_7_0"]["colour"], 122, "the ring's block");
    assert_eq!(state["leds"]["pad_7_1"]["colour"], 126, "a clip plays");
    assert_eq!(state["leds"]["pad_7_2"]["colour"], 0);
    rig.pad(0, 1);
    let tracks = rig.tracks();
    assert_eq!(
        rig.run("session.state", json!({}))["rings"][0]["tracks"][0],
        json!(tracks[4]),
        "the ring goes as far as the set reaches"
    );
    assert!(rig.sim.let_go("layout"));
    rig.pass(10);
    assert_eq!(rig.state()["session"]["overview"], false);
}

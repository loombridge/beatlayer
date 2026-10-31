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
    // Device Mode is built: with no track selected it says so.
    assert!(text.contains(&"title: No track is selected"), "{text:?}");

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

const UTILITY: &str = "beatlayer.utility";

impl Rig {
    /// Adds a Utility to the selected track, which the window selects.
    fn utility(&mut self) -> Value {
        let added = self
            .shell
            .invoke(
                "device.add",
                json!({ "device": {
                    "kind": { "native": { "device_type": UTILITY } },
                    "name": "Utility",
                    "role": "audio_effect",
                } }),
            )
            .unwrap();
        self.step();
        added["device"].clone()
    }

    fn rack(&mut self, kind: &str, chains: usize) -> (Value, Vec<Value>) {
        let rack = self
            .shell
            .invoke(
                "device.add",
                json!({ "device": {
                    "kind": { "rack": { "kind": kind } },
                    "name": "Rack",
                    "role": "audio_effect",
                } }),
            )
            .unwrap()["device"]
            .clone();
        let ids = (0..chains)
            .map(|n| {
                let id: bl_model::ChainId = self.shell.new_id();
                self.shell
                    .invoke(
                        "chain.add",
                        json!({ "rack": rack, "index": n, "chain": { "id": id, "name": "", "devices": [] } }),
                    )
                    .unwrap();
                json!(id)
            })
            .collect();
        self.step();
        (rack, ids)
    }

    /// What Device Mode shows, as `push.state` reads it.
    fn device_mode(&mut self) -> Value {
        self.state()["device"].clone()
    }

    /// The devices on the upper buttons, by ID.
    fn on_buttons(&mut self) -> Vec<Value> {
        self.device_mode()["devices"]
            .as_array()
            .unwrap()
            .iter()
            .map(|d| d["device"].clone())
            .collect()
    }

    fn selected_device(&mut self) -> Value {
        self.device_mode()["devices"]
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["selected"] == true)
            .map_or(Value::Null, |d| d["device"].clone())
    }

    /// The devices on the first selected track, in chain order.
    fn chain(&self) -> Vec<bl_model::DeviceId> {
        let track = self.shell.arrangement().selected_tracks()[0];
        self.shell
            .editor()
            .document()
            .track(track)
            .unwrap()
            .devices()
            .map(|d| d.id)
            .collect()
    }

    fn hold(&mut self, name: &str) {
        assert!(self.sim.press(name));
        self.pass(350);
    }

    fn release(&mut self, name: &str) {
        assert!(self.sim.let_go(name));
        self.pass(10);
    }
}

// Spec: push-2-tracks R15, R18, R1, R27. Device Mode puts the selected
// track's devices on the upper buttons and the selected one's first bank on
// the encoders; a touch of an encoder, however many turns, is one undo step,
// and `device.parameters` reads the change.
#[test]
fn turning_a_device_encoder_is_one_undo_step_and_reads_back() {
    let mut rig = Rig::browsing();
    let (a, b, c) = (rig.utility(), rig.utility(), rig.utility());
    assert_eq!(rig.on_buttons(), [a, b, c.clone()]);
    assert_eq!(
        rig.selected_device(),
        c,
        "the window selected the last added"
    );
    let mode = rig.device_mode();
    assert_eq!(mode["banks"], json!(["Utility", "Bass"]));
    assert_eq!(mode["bank"], 0);
    assert_eq!(mode["params"].as_array().unwrap().len(), 8);
    let name = mode["params"][0]["name"].as_str().unwrap().to_owned();
    let text = rig.state()["display"]["text"].clone();
    assert!(
        text.as_array().unwrap().iter().any(|l| l
            .as_str()
            .unwrap()
            .starts_with("upper 3: Utility (selected)")),
        "{text}"
    );
    assert!(
        text.as_array().unwrap().iter().any(|l| l
            .as_str()
            .unwrap()
            .starts_with(&format!("column 1: {name} = "))),
        "{text}"
    );

    // The first parameter of the first bank, as `control_surface.banks` reads it.
    let first = |rig: &mut Rig| {
        rig.run("control_surface.banks", json!({ "device": c }))["banks"][0]["params"][0].clone()
    };
    let before = first(&mut rig)["value"].as_f64().unwrap();
    let steps = rig.steps();
    assert!(rig.sim.touch(0));
    for _ in 0..3 {
        assert!(rig.sim.turn(0, 12));
        rig.pass(10);
    }
    assert!(rig.shell.gesture_under_way());
    assert!(rig.sim.release(0));
    rig.pass(10);
    assert!(!rig.shell.gesture_under_way());
    assert_eq!(rig.steps(), steps + 1, "three turns, one undo step");
    let after = first(&mut rig)["value"].as_f64().unwrap();
    assert!(after > before, "{before} to {after}");
    rig.step();
    assert_eq!(
        rig.device_mode()["params"][0]["value"],
        first(&mut rig)["text"],
        "the value on the display is the one the device shows"
    );
    rig.run("edit.undo", json!({}));
    rig.step();
    let undone = first(&mut rig)["value"].as_f64().unwrap();
    assert!((undone - before).abs() < 1e-9);
}

// Spec: push-2-tracks R3, R16, R26. Selecting a device on Push 2 selects it
// in the window and the other way round; the second tap enters Edit Mode, and
// its lower buttons choose the bank the shell remembers for the device.
#[test]
fn push_2_and_the_window_select_the_same_device_and_edit_mode_chooses_the_bank() {
    let mut rig = Rig::browsing();
    let (a, b) = (rig.utility(), rig.utility());
    assert_eq!(rig.selected_device(), b);
    // The window selects the first: Push 2 follows at the next tick.
    rig.run("device.select", json!({ "device": a }));
    rig.step();
    assert_eq!(rig.selected_device(), a);
    // Push 2 selects the second: the window follows.
    rig.tap("upper_2");
    assert_eq!(rig.run("device_view.state", json!({}))["selected"], b);
    rig.step();
    assert_eq!(rig.selected_device(), b);

    let steps = rig.steps();
    rig.tap("upper_2");
    let mode = rig.device_mode();
    assert_eq!(mode["edit"], true);
    let text = rig.state()["display"]["text"].clone();
    let has = |start: &str| {
        text.as_array()
            .unwrap()
            .iter()
            .any(|l| l.as_str().unwrap().starts_with(start))
    };
    assert!(has("lower 1: Utility (selected)"), "{text}");
    assert!(has("lower 2: Bass"), "{text}");
    rig.tap("lower_2");
    rig.step();
    let mode = rig.device_mode();
    assert_eq!(mode["bank"], 1);
    assert_ne!(mode["params"][0]["name"], "");
    assert_eq!(rig.steps(), steps, "banks are no undo step");
    // The device on the row reads back the same bank.
    let list = rig.run("control_surface.list", json!({}));
    let row = list["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["surface"]["id"] == "push-2")
        .unwrap();
    assert_eq!(row["surface_state"]["device"]["bank"], 1);
    rig.tap("upper_1");
    assert_eq!(rig.device_mode()["edit"], false, "back to the top level");
}

// Spec: push-2-tracks R19, R20, R21, R1. Delete with a device's button
// deletes it as one undo step and selects the next; Mute switches it; a held
// effect turned moves through its chain as one undo step for the hold.
#[test]
fn delete_mute_and_moves_run_the_device_commands_as_one_undo_step_each() {
    let mut rig = Rig::browsing();
    let (a, b, c) = (rig.utility(), rig.utility(), rig.utility());
    let ids = |rig: &Rig| {
        rig.chain()
            .into_iter()
            .map(|d| json!(d))
            .collect::<Vec<_>>()
    };

    // Mute: held, then pressed with the device's button.
    let steps = rig.steps();
    assert!(rig.sim.press("mute"));
    rig.pass(10);
    rig.tap("upper_2");
    assert!(rig.sim.let_go("mute"));
    rig.pass(10);
    assert_eq!(rig.steps(), steps + 1);
    let enabled = |rig: &Rig, device: &Value| {
        let id: bl_model::DeviceId = serde_json::from_value(device.clone()).unwrap();
        rig.shell.editor().document().device(id).unwrap().enabled
    };
    assert!(!enabled(&rig, &b));
    assert!(enabled(&rig, &a));
    assert_eq!(rig.device_mode()["devices"][1]["enabled"], false);

    // A held effect, turned right, goes past its neighbour.
    let steps = rig.steps();
    rig.hold("upper_1");
    assert!(rig.sim.turn(0, 10));
    rig.pass(10);
    assert!(rig.sim.turn(1, 10));
    rig.pass(10);
    rig.release("upper_1");
    assert_eq!(ids(&rig), [b.clone(), c.clone(), a.clone()], "two places");
    assert_eq!(rig.steps(), steps + 1, "two moves, one undo step");
    rig.run("edit.undo", json!({}));
    assert_eq!(ids(&rig), [a.clone(), b.clone(), c.clone()]);

    // Delete: the device goes, the one after it is selected.
    let steps = rig.steps();
    assert!(rig.sim.press("delete"));
    rig.pass(10);
    rig.tap("upper_2");
    assert!(rig.sim.let_go("delete"));
    rig.pass(10);
    assert_eq!(ids(&rig), [a.clone(), c.clone()]);
    assert_eq!(rig.steps(), steps + 1);
    assert_eq!(rig.selected_device(), c);
    // The last: the one before.
    assert!(rig.sim.press("delete"));
    rig.pass(10);
    rig.tap("upper_2");
    assert!(rig.sim.let_go("delete"));
    rig.pass(10);
    assert_eq!(rig.selected_device(), a);
}

// Spec: push-2-tracks R22, R23, R24. A rack unfolds and folds with the
// window's own devices view; its macros are on the encoders; a device inside
// it is selectable; holding it selects a chain the window shows.
#[test]
fn a_rack_folds_with_the_devices_view_and_holding_it_selects_a_chain() {
    let mut rig = Rig::browsing();
    let (rack, chains) = rig.rack("audio_effect", 2);
    let inner = rig
        .shell
        .invoke(
            "device.add",
            json!({
                "device": {
                    "kind": { "native": { "device_type": UTILITY } },
                    "name": "Utility",
                    "role": "audio_effect",
                },
                "chain": chains[1],
            }),
        )
        .unwrap()["device"]
        .clone();
    rig.run("device.select", json!({ "device": rack }));
    rig.step();
    let mode = rig.device_mode();
    assert_eq!(
        mode["banks"][0], "Macros 1–8",
        "a rack's encoders play its macros"
    );
    assert_eq!(mode["devices"][0]["kind"], "audio_effect_rack");
    assert_eq!(mode["devices"][0]["unfolded"], true);
    assert_eq!(
        rig.on_buttons(),
        std::slice::from_ref(&rack),
        "the first chain holds nothing"
    );

    // Holding the rack's button shows its chains; a press selects the second,
    // which the window's chain list shows, and its device follows the rack.
    rig.hold("upper_1");
    let held = rig.device_mode();
    assert_eq!(held["chains"].as_array().unwrap().len(), 2);
    assert_eq!(held["chains"][0]["selected"], true);
    rig.tap("lower_2");
    rig.release("upper_1");
    rig.step();
    assert_eq!(rig.on_buttons(), [rack.clone(), inner.clone()]);
    let window = rig.run("device_view.state", json!({}));
    let frame = window["devices"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["id"] == rack)
        .unwrap()["rack"]
        .clone();
    assert_eq!(frame["selected_chains"], json!([chains[1]]));
    assert_eq!(rig.device_mode()["devices"][1]["depth"], 1);
    let text = rig.state()["display"]["text"].clone();
    assert!(
        text.as_array().unwrap().iter().any(|l| l == "fold: 1 to 2"),
        "{text}"
    );

    // A contained device is selectable, and the encoders play its bank.
    rig.tap("upper_2");
    rig.step();
    assert_eq!(rig.selected_device(), inner);
    assert_eq!(rig.device_mode()["banks"][0], "Utility");

    // The rack's second tap folds it through `rack.set_view`, one the window
    // shows too, and not an undo step.
    rig.tap("upper_1");
    rig.step();
    let steps = rig.steps();
    rig.tap("upper_1");
    rig.step();
    let views = |rig: &mut Rig| {
        rig.run("device_view.state", json!({}))["devices"]
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["id"] == rack)
            .unwrap()["rack"]["views"]["devices"]
            .clone()
    };
    assert_eq!(views(&mut rig), false);
    assert_eq!(rig.on_buttons(), std::slice::from_ref(&rack));
    assert_eq!(rig.steps(), steps, "folding is no undo step");
    // Showing it in the window shows it on Push 2.
    rig.run(
        "rack.set_view",
        json!({ "rack": rack, "view": "devices", "shown": true }),
    );
    rig.step();
    assert_eq!(rig.on_buttons(), [rack, inner]);
}

// Spec: push-2-tracks R25, R1. Add Device lists everything the track takes
// and Load adds after the selected device; an instrument replaces the
// track's instrument.
#[test]
fn add_device_loads_after_the_selected_device_and_an_instrument_replaces_the_instrument() {
    let mut rig = Rig::browsing();
    let a = rig.utility();
    let b = rig.utility();
    rig.run("device.select", json!({ "device": a }));
    rig.step();
    rig.tap("add_device");
    assert_eq!(rig.state()["display_mode"], "browse");
    let labels: Vec<_> = rig.columns()[0]["entries"]
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
        ],
        "an effect is selected, and Add Device still lists instruments"
    );

    // An audio effect goes right after the selected device, not at the end.
    rig.open_label(0, "Audio Effects");
    let steps = rig.steps();
    rig.tap("lower_8");
    rig.step();
    assert_eq!(rig.steps(), steps + 1);
    let chain = rig.chain();
    assert_eq!(chain.len(), 3);
    assert_eq!(json!(chain[0]), a);
    assert_eq!(json!(chain[2]), b, "after the selected one");
    assert_eq!(rig.state()["browse"]["load"], "add");

    // An instrument goes where instruments go, and a second one replaces it.
    rig.tap("upper_7");
    rig.tap("up");
    assert_eq!(rig.selected_in(0), "Instruments");
    rig.tap("upper_8");
    rig.tap("lower_8");
    rig.step();
    assert_eq!(rig.chain().len(), 4);
    assert_eq!(rig.state()["browse"]["load"], "replace");
    rig.tap("down");
    rig.tap("lower_8");
    rig.step();
    assert_eq!(rig.chain().len(), 4, "the instrument was replaced");
}

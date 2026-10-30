//! Controller templates in the shell (WP-7.7; `docs/specs/control-surfaces.md`,
//! R39 to R48): a template surface on the in-process test backend plays the
//! ring, the mixer, the banks and the transport, lights its lamps, and the
//! User Library's `Controller Templates` folder fills the chooser.

use std::path::PathBuf;

use bl_audio_io::NullControl;
use bl_core::clock::TestClock;
use bl_midi_io::{Direction, TestBackend};
use bl_model::{DeviceId, RelativeEncoding, SceneId, TrackId};
use bl_shell::device_commands::DeviceAdded;
use bl_shell::{Area, Shell, testing};
use serde_json::{Value, json};

const UTILITY: &str = "beatlayer.utility";

/// A template of four tracks by two scenes, written for these tests.
const PADS: &str = r#"
format = 1

[surface]
name = "Test Pads"
ring_width = 4
ring_height = 2
lock = true
ports = ["Pads"]

[[control]]
id = "pad 1"
message = "note"
channel = 1
number = 36
kind = "button"
role = "slot 1 1"
feedback = { off = 0, on = 127, flash = 10 }

[[control]]
id = "pad 2"
message = "note"
channel = 1
number = 37
kind = "button"
role = "slot 2 1"
feedback = { off = 0, on = 127, flash = 10 }

[[control]]
id = "scene 2"
message = "cc"
channel = 1
number = 51
kind = "button"
role = "scene 2"

[[control]]
id = "stop 1"
message = "cc"
channel = 1
number = 52
kind = "button"
role = "track-stop 1"

[[control]]
id = "fader 1"
message = "cc"
channel = 1
number = 7
kind = "absolute"
role = "volume 1"
feedback = { value = true }

[[control]]
id = "signed bit"
message = "cc"
channel = 1
number = 20
kind = "relative"
encoding = "signed-bit"
role = "device 1"

[[control]]
id = "signed bit 2"
message = "cc"
channel = 1
number = 21
kind = "relative"
encoding = "signed-bit-2"
accelerated = true
role = "device 2"

[[control]]
id = "bin offset"
message = "cc"
channel = 1
number = 22
kind = "relative"
encoding = "bin-offset"
role = "device 3"

[[control]]
id = "twos complement"
message = "cc"
channel = 1
number = 23
kind = "relative"
encoding = "twos-complement"
role = "device 4"

[[control]]
id = "knob"
message = "cc"
channel = 1
number = 24
kind = "absolute"
role = "device 5"

[[control]]
id = "ring right"
message = "cc"
channel = 1
number = 60
kind = "button"
role = "ring-right"

[[control]]
id = "next bank"
message = "cc"
channel = 1
number = 61
kind = "button"
role = "next-bank"

[[control]]
id = "play"
message = "cc"
channel = 1
number = 62
kind = "button"
role = "play"
feedback = { off = 0, on = 127 }

[[control]]
id = "tempo"
message = "cc"
channel = 1
number = 70
kind = "absolute"
role = "tempo"

[[control]]
id = "next device"
message = "cc"
channel = 1
number = 71
kind = "button"
role = "next-device"

[[control]]
id = "main"
message = "cc"
channel = 1
number = 72
kind = "absolute"
role = "main-volume"

[[control]]
id = "undo"
message = "cc"
channel = 1
number = 73
kind = "button"
role = "undo"

[[setup]]
bytes = "F0 7D 10 F7"

[[goodbye]]
bytes = "F0 7D 11 F7"

[[dump]]
bytes = "F0 7D 12 F7"
"#;

const SETUP: [u8; 4] = [0xF0, 0x7D, 0x10, 0xF7];
const GOODBYE: [u8; 4] = [0xF0, 0x7D, 0x11, 0xF7];
const DUMP: [u8; 4] = [0xF0, 0x7D, 0x12, 0xF7];

/// A folder for one test's files, removed when the test ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "bl-templates-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    /// The User Library's template folder, made.
    fn templates(&self) -> PathBuf {
        let folder = self.0.join("Controller Templates");
        std::fs::create_dir_all(&folder).unwrap();
        folder
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Rig {
    shell: Shell,
    control: NullControl,
    test: TestBackend,
    clock: TestClock,
    scratch: Scratch,
    /// How much of the output's log earlier checks have taken.
    seen: usize,
}

impl Rig {
    /// A set with the Test Pads template, from the User Library, on row 1
    /// and port `Pads`.
    fn new(name: &str) -> Self {
        let (mut shell, control) = testing::rig();
        let scratch = Scratch::new(name);
        std::fs::write(scratch.templates().join("pads.bltemplate.toml"), PADS).unwrap();
        shell.set_user_library(Some(scratch.0.clone()));
        let test = TestBackend::new();
        test.add_port(Direction::Input, "Pads");
        test.add_port(Direction::Output, "Pads Out");
        shell.use_midi_backend(Box::new(test.clone()));
        let clock = control.clock().expect("manual pacing");
        shell.use_test_clock(clock.clone());
        shell.start_audio();
        let mut rig = Self {
            shell,
            control,
            test,
            clock,
            scratch,
            seen: 0,
        };
        rig.run(
            "session.set_quantization",
            json!({ "quantization": "none" }),
        );
        let found = rig.run("control_surface.templates", json!({}));
        assert!(
            found["templates"]
                .as_array()
                .unwrap()
                .iter()
                .any(|t| t["name"] == "Test Pads" && t["listed"] == true),
            "{found}"
        );
        rig.run(
            "control_surface.set",
            json!({ "row": 1, "surface": "template:Test Pads", "input": "Pads", "output": "Pads Out" }),
        );
        rig
    }

    fn run(&mut self, id: &str, args: Value) -> Value {
        self.shell
            .run(id, args)
            .unwrap_or_else(|error| panic!("{id}: {error}"))
    }

    fn invoke(&mut self, id: &str, args: Value) -> Value {
        self.shell
            .invoke(id, args)
            .unwrap_or_else(|error| panic!("{id}: {error}"))
    }

    fn send(&mut self, bytes: &[u8]) {
        assert!(self.test.send("Pads", self.clock.now(), bytes));
        self.shell.midi().surfaces().settle();
        self.shell.poll();
    }

    fn cc(&mut self, controller: u8, value: u8) {
        self.send(&[0xB0, controller, value]);
    }

    /// A button pressed and let go: a note, or a controller sending 127 and 0.
    fn tap_note(&mut self, key: u8) {
        self.send(&[0x90, key, 100]);
        self.send(&[0x80, key, 0]);
    }

    fn tap_cc(&mut self, controller: u8) {
        self.cc(controller, 127);
        self.cc(controller, 0);
    }

    fn pass(&mut self, millis: u64) {
        testing::advance(&self.control, &mut self.shell, millis);
        self.shell.poll();
    }

    /// What the output was sent since the last call.
    fn fed(&mut self) -> Vec<Vec<u8>> {
        self.shell.poll();
        self.shell.midi().surfaces().tick();
        let all = self.test.sent("Pads Out");
        let new = all[self.seen..].to_vec();
        self.seen = all.len();
        new
    }

    fn new_track(&mut self, kind: &str) -> TrackId {
        self.invoke("track.create", json!({ "kind": kind, "name": "T" }));
        self.shell.editor().document().top_level().last().unwrap()
    }

    fn scenes(&self) -> Vec<SceneId> {
        self.shell
            .editor()
            .document()
            .scenes()
            .map(|s| s.id)
            .collect()
    }

    /// Two new MIDI tracks with a clip in each of the first two scenes,
    /// under the ring's first and second column.
    fn clips(&mut self) -> (TrackId, TrackId, Vec<SceneId>) {
        let (a, b) = (self.new_track("midi"), self.new_track("midi"));
        let scenes = self.scenes();
        assert!(scenes.len() >= 2, "the set has two scenes");
        for track in [a, b] {
            for scene in &scenes[..2] {
                self.run(
                    "session.insert_empty_midi_clip",
                    json!({ "track": track, "scene": scene }),
                );
            }
        }
        self.run("control_surface.move_ring", json!({ "row": 1, "track": a }));
        (a, b, scenes)
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

    fn playing(&self, track: TrackId, scene: SceneId) -> bool {
        self.shell
            .session_states()
            .iter()
            .any(|s| s.track == track && s.scene == scene)
    }

    fn select(&mut self, track: TrackId) {
        self.invoke("arrangement.select_tracks", json!({ "tracks": [track] }));
        self.shell.set_focused_area(Area::Detail);
    }

    fn add_utility(&mut self) -> DeviceId {
        let added: DeviceAdded = serde_json::from_value(self.invoke(
            "device.add",
            json!({ "device": {
                "kind": { "native": { "device_type": UTILITY } },
                "name": UTILITY,
                "role": "audio_effect",
            } }),
        ))
        .unwrap();
        added.device
    }

    fn param(&self, device: DeviceId, param: u32) -> f64 {
        self.shell
            .editor()
            .document()
            .device(device)
            .and_then(|d| d.params.get(&bl_model::ParamId(param)).copied())
            .unwrap()
    }

    fn surfaces(&mut self) -> Vec<String> {
        self.run("control_surface.surfaces", json!({}))["surfaces"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["id"].as_str().unwrap().to_owned())
            .collect()
    }
}

// Spec: control-surfaces R40, R42. A template surface launches a slot and a
// scene and stops a track, as the ring's roles do.
#[test]
fn a_template_surface_launches_a_slot_a_scene_and_a_stop() {
    let mut rig = Rig::new("slots");
    let (a, b, scenes) = rig.clips();

    rig.tap_note(36);
    rig.pass(100);
    assert!(rig.playing(a, scenes[0]), "pad 1 launches column 1, row 1");
    assert!(!rig.playing(b, scenes[0]));

    rig.tap_note(37);
    rig.pass(100);
    assert!(rig.playing(b, scenes[0]), "pad 2 is column 2");

    rig.tap_cc(51);
    rig.pass(100);
    assert!(
        rig.playing(a, scenes[1]) && rig.playing(b, scenes[1]),
        "the second scene's button launches its row"
    );

    rig.tap_cc(52);
    rig.pass(100);
    assert!(!rig.playing(a, scenes[1]), "track-stop 1 stops column 1");
    assert!(rig.playing(b, scenes[1]), "and only column 1");
}

// Spec: control-surfaces R42, remote-mapping R28. An absolute control moves
// a track's volume through Takeover Mode.
#[test]
fn a_template_fader_moves_a_volume_through_takeover_mode() {
    let mut rig = Rig::new("takeover");
    let (a, _, _) = rig.clips();
    rig.run("settings.set_takeover_mode", json!({ "mode": "pick_up" }));
    let before = rig.volume(a);
    rig.cc(7, 0);
    assert_eq!(rig.volume(a), before, "the fader has to meet the volume");
    rig.cc(7, 127);
    assert_ne!(rig.volume(a), before, "it crossed the volume");
}

// Spec: control-surfaces R40, R42. A relative encoder of each of the four
// types steps the parameter its role reaches, up and down.
#[test]
fn a_relative_encoder_of_each_type_steps_its_parameter() {
    use bl_control::size;
    let mut rig = Rig::new("relative");
    let audio = rig.new_track("audio");
    rig.select(audio);
    let utility = rig.add_utility();
    let banks = rig.run("control_surface.banks", json!({ "device": utility }));
    let params: Vec<u32> = banks["banks"][0]["params"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| u32::try_from(p["param"].as_u64().unwrap()).unwrap())
        .collect();
    // The controller, an increment of each type and a decrement.
    let encoders = [
        (20, RelativeEncoding::SignedBit, 65, 1),
        (21, RelativeEncoding::SignedBit2, 1, 65),
        (22, RelativeEncoding::BinOffset, 65, 63),
        (23, RelativeEncoding::TwosComplement, 1, 127),
    ];
    for (n, (controller, encoding, up, down)) in encoders.into_iter().enumerate() {
        assert!(size(encoding, up) > 0 && size(encoding, down) < 0);
        let param = params[n];
        rig.run(
            "device.set_param",
            json!({ "device": utility, "param": param, "value": 0.5 }),
        );
        let start = rig.param(utility, param);
        rig.cc(controller, up);
        let raised = rig.param(utility, param);
        assert!(raised > start, "{encoding:?} up: {start} to {raised}");
        rig.cc(controller, down);
        rig.cc(controller, down);
        let lowered = rig.param(utility, param);
        assert!(lowered < start, "{encoding:?} down: {start} to {lowered}");
    }
}

// Spec: control-surfaces R40, R42. The ring's and the bank's buttons move
// them.
#[test]
fn a_template_surface_moves_the_ring_and_the_bank() {
    let mut rig = Rig::new("ring");
    rig.clips();
    for _ in 0..8 {
        rig.new_track("audio");
    }
    let state = |rig: &mut Rig| {
        rig.run("control_surface.list", json!({}))["rows"][0]["surface_state"].clone()
    };
    let before = state(&mut rig)["ring"]["track_index"].as_u64().unwrap();
    rig.tap_cc(60);
    assert_eq!(
        state(&mut rig)["ring"]["track_index"].as_u64().unwrap(),
        before + 1,
        "ring-right moves the ring a track"
    );
    // A device with more than one bank, in focus.
    let audio = rig.new_track("audio");
    rig.select(audio);
    rig.add_utility();
    let bank = |rig: &mut Rig| state(rig)["device"]["bank"].as_u64().unwrap();
    assert_eq!(bank(&mut rig), 0);
    rig.tap_cc(61);
    assert_eq!(bank(&mut rig), 1, "next-bank steps to the next bank");
}

// Spec: control-surfaces R41, R9, R11. The setup is sent when the surface
// starts, the goodbye when it stops and the dump by Dump.
#[test]
fn a_template_sends_its_setup_its_goodbye_and_its_dump() {
    let mut rig = Rig::new("messages");
    assert_eq!(rig.test.sent("Pads Out").first().unwrap(), &SETUP);
    let row = rig.run("control_surface.dump", json!({ "row": 1 }));
    assert_eq!(row["sent"], 1);
    assert_eq!(rig.test.sent("Pads Out").last().unwrap(), &DUMP);
    rig.run("control_surface.set", json!({ "row": 1, "surface": null }));
    assert_eq!(rig.test.sent("Pads Out").last().unwrap(), &GOODBYE);
}

// Spec: control-surfaces R41, R42. A playing slot's button is lit and a
// queued one flashes.
#[test]
fn a_playing_slots_button_is_lit_and_a_queued_one_flashes() {
    let mut rig = Rig::new("lamps");
    let (a, _, scenes) = rig.clips();
    rig.pass(40);
    rig.fed();
    rig.tap_note(36);
    rig.pass(100);
    assert!(rig.playing(a, scenes[0]));
    assert!(
        rig.fed().contains(&vec![0x90, 36, 127]),
        "a playing slot is on"
    );

    // With a bar to wait for, the next launch is queued and flashes.
    rig.run(
        "session.set_quantization",
        json!({ "quantization": "1_bar" }),
    );
    rig.run("transport.play", json!({}));
    rig.pass(50);
    rig.tap_note(37);
    rig.pass(20);
    let sent = rig.fed();
    assert!(
        sent.contains(&vec![0x90, 37, 10]),
        "a queued slot flashes: {sent:?}"
    );
    rig.pass(4000);
    assert!(
        rig.fed().contains(&vec![0x90, 37, 127]),
        "and is on once it plays"
    );
}

// Spec: control-surfaces R41. An absolute control carries its value to a
// fader, and a transport button's lamp follows the transport.
#[test]
fn a_fader_and_a_transport_lamp_follow_the_set() {
    let mut rig = Rig::new("faders");
    let (a, _, _) = rig.clips();
    rig.run("mixer.set_volume", json!({ "track": a, "volume": 1.0 }));
    rig.pass(40);
    let sent = rig.fed();
    let fader = sent.iter().rev().find(|m| m[..2] == [0xB0, 7]).unwrap();
    // Unity sits where the fader's law puts it, as a move is read.
    let unity = (bl_model::fader::fader_position(1.0) * 127.0).round() as u8;
    assert_eq!(
        fader[2], unity,
        "unity is where the fader puts it: {fader:?}"
    );
    rig.tap_cc(62);
    rig.pass(100);
    assert!(rig.fed().contains(&vec![0xB0, 62, 127]), "play lights");
}

/// Writes a template file, in a subfolder if `folder` says.
fn write(scratch: &Scratch, folder: &str, file: &str, text: &str) {
    let dir = scratch.templates().join(folder);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(file), text).unwrap();
}

fn named(name: &str) -> String {
    format!("format = 1\n[surface]\nname = \"{name}\"\n")
}

// Spec: control-surfaces R43. The chooser lists the native surfaces, then
// the templates by name, those in the User Library's subfolders too, and a
// User Library template named like the bundled one replaces it.
#[test]
fn the_chooser_lists_the_user_librarys_templates_after_the_native_surfaces() {
    let mut rig = Rig::new("chooser");
    write(&rig.scratch, "", "zeta.bltemplate.toml", &named("Zeta"));
    write(
        &rig.scratch,
        "Vendors",
        "alpha.bltemplate.toml",
        &named("Alpha"),
    );
    rig.run("control_surface.templates", json!({}));
    let ids = rig.surfaces();
    let natives = ids
        .iter()
        .take_while(|id| !id.starts_with("template:"))
        .count();
    assert!(natives >= 2, "the native surfaces come first: {ids:?}");
    assert_eq!(
        &ids[natives..],
        [
            "template:Alpha",
            "template:Generic 8 + 8",
            "template:Test Pads",
            "template:Zeta"
        ],
        "bundled, one folder down and a subfolder, by name"
    );

    // One of the bundled template's name replaces it.
    write(
        &rig.scratch,
        "",
        "mine.bltemplate.toml",
        "format = 1\n[surface]\nname = \"Generic 8 + 8\"\nring_width = 2\nring_height = 1\n",
    );
    let found = rig.run("control_surface.templates", json!({}));
    let generic: Vec<&Value> = found["templates"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|t| t["name"] == "Generic 8 + 8")
        .collect();
    assert_eq!(generic.len(), 2);
    assert!(
        generic
            .iter()
            .any(|t| t["bundled"] == true && t["listed"] == false)
    );
    assert!(
        generic
            .iter()
            .any(|t| t["bundled"] == false && t["listed"] == true)
    );
    let surfaces = rig.run("control_surface.surfaces", json!({}));
    let own = surfaces["surfaces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"] == "template:Generic 8 + 8")
        .unwrap();
    assert_eq!(own["ring"], json!({ "tracks": 2, "scenes": 1 }));
}

// Spec: control-surfaces R44. A row whose template's file is deleted keeps
// the template's name, and stops, as a row does for a missing port.
#[test]
fn deleting_a_chosen_templates_file_leaves_its_row_naming_it() {
    let mut rig = Rig::new("deleted");
    let row = rig.run("control_surface.list", json!({}))["rows"][0].clone();
    assert_eq!(row["state"], "running");
    std::fs::remove_file(rig.scratch.templates().join("pads.bltemplate.toml")).unwrap();
    rig.run("control_surface.templates", json!({}));
    let row = rig.run("control_surface.list", json!({}))["rows"][0].clone();
    assert_eq!(row["surface"]["id"], "template:Test Pads");
    assert_eq!(row["state"], "stopped");
    assert!(!rig.surfaces().contains(&"template:Test Pads".to_owned()));

    // The file comes back, and the row starts again.
    std::fs::write(rig.scratch.templates().join("pads.bltemplate.toml"), PADS).unwrap();
    rig.run("control_surface.templates", json!({}));
    rig.shell.refresh_midi();
    assert_eq!(
        rig.run("control_surface.list", json!({}))["rows"][0]["state"],
        "running"
    );
}

// Spec: control-surfaces R44. `control_surface.templates` lists a broken
// file with each error and its line, and doesn't list it for the chooser.
#[test]
fn a_broken_file_is_reported_with_its_line_and_not_listed() {
    let mut rig = Rig::new("broken");
    write(
        &rig.scratch,
        "",
        "broken.bltemplate.toml",
        "format = 1\n[surface]\nname = \"Broken\"\n\n[[control]]\nid = \"a\"\nmessage = \"note\"\nchannel = 1\nnumber = 1\nkind = \"button\"\nrole = \"fly\"\n",
    );
    let found = rig.run("control_surface.templates", json!({}));
    let broken = found["templates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| {
            t["path"]
                .as_str()
                .is_some_and(|p| p.ends_with("broken.bltemplate.toml"))
        })
        .unwrap();
    assert_eq!(broken["listed"], false);
    assert_eq!(broken["errors"][0]["line"], 11);
    assert!(
        broken["errors"][0]["message"]
            .as_str()
            .unwrap()
            .contains("fly")
    );
    assert!(!rig.surfaces().contains(&"template:Broken".to_owned()));
}

// Spec: control-surfaces R45. A template that names a port is matched to it
// while no row uses the port and a row is free; nothing fills a row.
#[test]
fn a_template_that_names_a_port_matches_it_until_a_row_uses_it() {
    let mut rig = Rig::new("matches");
    rig.run(
        "control_surface.set",
        json!({ "row": 1, "surface": null, "input": null, "output": null }),
    );
    assert_eq!(
        rig.shell.template_matches(),
        [
            ("Pads".to_owned(), "Test Pads".to_owned()),
            ("Pads Out".to_owned(), "Test Pads".to_owned())
        ],
        "the template names the controller's input and its output"
    );
    let found = rig.run("control_surface.templates", json!({}));
    let pads = found["templates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "Test Pads")
        .unwrap();
    assert_eq!(pads["matches"], json!(["Pads", "Pads Out"]));
    // The matching template comes first among the templates.
    let ids = rig.surfaces();
    let first = ids
        .iter()
        .position(|id| id.starts_with("template:"))
        .unwrap();
    assert_eq!(ids[first], "template:Test Pads");
    assert_eq!(
        rig.run("control_surface.list", json!({}))["rows"][0]["state"],
        "empty",
        "nothing fills a row"
    );

    rig.run(
        "control_surface.set",
        json!({ "row": 2, "surface": "template:Test Pads", "input": "Pads", "output": "Pads Out" }),
    );
    assert!(
        rig.shell.template_matches().is_empty(),
        "a row uses the port"
    );
}

// Spec: control-surfaces R40. The tempo, Main volume, device and undo roles
// run their commands.
#[test]
fn the_tempo_main_volume_device_and_undo_roles_run_their_commands() {
    let mut rig = Rig::new("commands");
    let tempo = |rig: &Rig| rig.shell.editor().document().settings().tempo.clone();
    let before = tempo(&rig);
    rig.cc(70, 127);
    assert_ne!(tempo(&rig), before, "an absolute control sets the tempo");

    let main = rig.shell.editor().document().main_id();
    let gain = rig.volume(main);
    rig.cc(72, 0);
    assert_ne!(rig.volume(main), gain, "main-volume is the Main track's");

    let audio = rig.new_track("audio");
    rig.select(audio);
    let (first, second) = (rig.add_utility(), rig.add_utility());
    let focus = |rig: &mut Rig| {
        let rows = rig.run("control_surface.list", json!({}));
        rows["rows"][0]["surface_state"]["device"]["device"].clone()
    };
    rig.select(audio);
    rig.run("device.select", json!({ "device": first }));
    assert_eq!(focus(&mut rig), json!(first));
    // The surface learns where the focus is on its tick.
    rig.fed();
    rig.tap_cc(71);
    assert_eq!(
        focus(&mut rig),
        json!(second),
        "next-device selects the next"
    );

    let steps = rig.shell.editor().undo_history().count();
    rig.cc(73, 127);
    assert!(
        rig.shell.editor().undo_history().count() < steps,
        "undo undoes a step"
    );
}

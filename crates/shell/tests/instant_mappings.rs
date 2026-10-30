//! Instant mappings in the shell (WP-7.6; `docs/specs/control-surfaces.md`,
//! R17 to R38): the device a surface's knobs follow, its banks, the lock to
//! a device, the session ring and the mixer controls over it. The instant
//! test surface runs on the in-process test backend, so every rule runs
//! headless.

use bl_audio_io::NullControl;
use bl_core::clock::TestClock;
use bl_midi_io::{Direction, TestBackend};
use bl_model::{DeviceId, TrackId, TrackKind};
use bl_shell::device_commands::DeviceAdded;
use bl_shell::{Area, Shell, testing};
use serde_json::{Value, json};

const UTILITY: &str = "beatlayer.utility";

struct Rig {
    shell: Shell,
    control: NullControl,
    test: TestBackend,
    clock: TestClock,
}

impl Rig {
    /// A set with the instant test surface on row 1, port `Pads`.
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
        let mut rig = Self {
            shell,
            control,
            test,
            clock,
        };
        rig.set(1, "test-instant", "Pads");
        rig
    }

    fn run(&mut self, id: &str, args: Value) -> Value {
        self.shell
            .run(id, args)
            .unwrap_or_else(|error| panic!("{id}: {error}"))
    }

    /// As a person does: the shell issues the IDs.
    fn invoke(&mut self, id: &str, args: Value) -> Value {
        self.shell
            .invoke(id, args)
            .unwrap_or_else(|error| panic!("{id}: {error}"))
    }

    fn set(&mut self, row: u8, surface: &str, port: &str) {
        self.run(
            "control_surface.set",
            json!({ "row": row, "surface": surface, "input": port, "output": format!("{port} Out") }),
        );
    }

    /// The row's state as `control_surface.list` reports it.
    fn state(&mut self, row: usize) -> Value {
        self.run("control_surface.list", json!({}))["rows"][row - 1]["surface_state"].clone()
    }

    /// Sends a message from the pads, lets the host take it and runs what
    /// it asked for.
    fn send(&mut self, bytes: &[u8]) {
        assert!(self.test.send("Pads", self.clock.now(), bytes));
        self.shell.midi().surfaces().settle();
        self.shell.poll();
    }

    fn cc(&mut self, controller: u8, value: u8) {
        self.send(&[0xB0, controller, value]);
    }

    /// A pad pressed and let go.
    fn tap(&mut self, key: u8) {
        self.send(&[0x90, key, 100]);
        self.send(&[0x80, key, 0]);
    }

    fn select(&mut self, track: TrackId) {
        self.invoke("arrangement.select_tracks", json!({ "tracks": [track] }));
        self.shell.set_focused_area(Area::Detail);
    }

    fn track(&self, kind: TrackKind) -> TrackId {
        let doc = self.shell.editor().document();
        doc.top_level()
            .find(|t| doc.track(*t).is_some_and(|t| t.kind() == kind))
            .expect("the set has one")
    }

    fn new_track(&mut self, kind: &str, name: &str) -> TrackId {
        self.invoke("track.create", json!({ "kind": kind, "name": name }));
        self.shell.editor().document().top_level().last().unwrap()
    }

    /// Adds `device_type` to the selected track's chain.
    fn add(&mut self, device_type: &str, role: &str) -> DeviceId {
        let added: DeviceAdded = serde_json::from_value(self.invoke(
            "device.add",
            json!({ "device": {
                "kind": { "native": { "device_type": device_type } },
                "name": device_type,
                "role": role,
            } }),
        ))
        .unwrap();
        added.device
    }

    fn param(&self, device: DeviceId, param: u32) -> Option<f64> {
        self.shell
            .editor()
            .document()
            .device(device)
            .and_then(|d| d.params.get(&bl_model::ParamId(param)).copied())
    }

    /// The device a row's device controls reach, and its bank.
    fn focus(&mut self, row: usize) -> Value {
        self.state(row)["device"].clone()
    }

    /// A device's banks by name, each with its parameter IDs.
    fn banks(&mut self, device: DeviceId) -> Vec<(String, Vec<u32>)> {
        let banks = self.run("control_surface.banks", json!({ "device": device }));
        banks["banks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| {
                (
                    b["name"].as_str().unwrap().to_owned(),
                    b["params"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|p| u32::try_from(p["param"].as_u64().unwrap()).unwrap())
                        .collect(),
                )
            })
            .collect()
    }

    fn steps(&self) -> usize {
        self.shell.editor().undo_history().count()
    }

    fn pass(&mut self, millis: u64) {
        testing::advance(&self.control, &mut self.shell, millis);
        self.shell.poll();
    }

    /// A rack with one chain holding a Utility, on the selected track.
    fn rack_with_utility(&mut self) -> (DeviceId, DeviceId, bl_model::ChainId) {
        let rack: DeviceAdded = serde_json::from_value(self.invoke(
            "device.add",
            json!({ "device": {
                "kind": { "rack": { "kind": "audio_effect" } },
                "name": "Rack",
                "role": "audio_effect",
            } }),
        ))
        .unwrap();
        let chain = self.shell.new_id::<bl_model::ChainId>();
        self.invoke(
            "chain.add",
            json!({ "rack": rack.device, "index": 0, "chain": { "id": chain, "name": "Chain", "devices": [] } }),
        );
        let utility: DeviceAdded = serde_json::from_value(self.invoke(
            "device.add",
            json!({ "chain": chain, "device": {
                "kind": { "native": { "device_type": UTILITY } },
                "name": "Utility",
                "role": "audio_effect",
            } }),
        ))
        .unwrap();
        (rack.device, utility.device, chain)
    }

    fn volume(&self, track: TrackId) -> f64 {
        self.shell.editor().document().track(track).unwrap().mixer().volume
    }

    /// The tracks the ring runs along, as `session.state` reports the ring
    /// of row 1.
    fn ring_tracks(&mut self) -> Vec<TrackId> {
        let rings = self.run("session.state", json!({}))["rings"].clone();
        serde_json::from_value(rings[0]["tracks"].clone()).unwrap_or_default()
    }
}

// Spec: control-surfaces R17, R18, R22. The knobs reach the selected device's
// first bank, follow a new selection at once, and step through banks, which
// stop at the first and last and are remembered per device.
#[test]
fn knobs_follow_the_selected_device_and_step_through_banks() {
    let mut rig = Rig::new();
    let (first, second) = (rig.track(TrackKind::Audio), rig.new_track("audio", "Second"));
    rig.select(first);
    let on_first = rig.add(UTILITY, "audio_effect");
    rig.select(second);
    let on_second = rig.add(UTILITY, "audio_effect");

    // The device in focus is the selected track's selected device.
    assert_eq!(rig.focus(1)["device"], json!(on_second));
    let banks = rig.banks(on_second);
    let names: Vec<_> = banks.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(names, ["Utility", "Bass"]);
    assert_eq!(banks[0].1.len(), 8, "main controls first, eight to a bank");
    assert_eq!(rig.focus(1)["bank_name"], "Utility");

    // Knob 1 reaches the first parameter of the first bank, over its range.
    let (gain, width) = (banks[0].1[0], banks[0].1[1]);
    rig.cc(0, 0);
    let low = rig.param(on_second, gain).unwrap();
    rig.cc(0, 127);
    let high = rig.param(on_second, gain).unwrap();
    assert!(high > low, "{low} to {high}");
    assert!(
        rig.param(on_first, gain).is_none_or(|v| (v - high).abs() > 1e-9),
        "the other device is untouched"
    );
    rig.cc(1, 127);
    assert!(rig.param(on_second, width).unwrap() > 1.0 - 1e-9);

    // Selecting another track moves the knobs at once.
    rig.select(first);
    assert_eq!(rig.focus(1)["device"], json!(on_first));
    rig.cc(0, 64);
    assert!(rig.param(on_first, gain).is_some());

    // Banks: next stops at the last, previous at the first, and a device
    // shows the bank it was last on.
    rig.select(second);
    rig.tap(1);
    assert_eq!(rig.focus(1)["bank_name"], "Bass");
    rig.tap(1);
    assert_eq!(rig.focus(1)["bank"], 1, "stops at the last");
    let bass = banks[1].1[0];
    rig.cc(0, 127);
    assert!(rig.param(on_second, bass).is_some(), "the bank's first parameter");
    rig.select(first);
    assert_eq!(rig.focus(1)["bank"], 0, "the other device is on its own bank");
    rig.select(second);
    assert_eq!(rig.focus(1)["bank"], 1, "remembered");
    rig.tap(0);
    rig.tap(0);
    assert_eq!(rig.focus(1)["bank"], 0, "stops at the first");

    // control_surface.set_bank does what the buttons do.
    let set = rig.run("control_surface.set_bank", json!({ "row": 1, "bank": 1 }));
    assert_eq!(set["surface_state"]["device"]["bank_name"], "Bass");
    assert!(
        rig.shell
            .run("control_surface.set_bank", json!({ "row": 1, "bank": 9 }))
            .is_err()
    );
}

// Spec: control-surfaces R17. On a track with no device selected the knobs
// reach its first device, and a track with none gives them nothing.
#[test]
fn a_track_with_no_device_selected_gives_its_first_device() {
    let mut rig = Rig::new();
    let (first, empty) = (rig.track(TrackKind::Audio), rig.new_track("audio", "Empty"));
    rig.select(first);
    let device = rig.add(UTILITY, "audio_effect");
    rig.select(empty);
    assert!(rig.focus(1).is_null(), "no device, no focus");
    rig.select(first);
    rig.run("device.select", json!({ "device": device }));
    assert_eq!(rig.focus(1)["device"], json!(device));
}

// Spec: control-surfaces R20, R22. A rack's banks are its macros, 9 to 16
// only with more than eight shown; a parameter a macro holds isn't moved, and
// the surface locks into a rack's chain.
#[test]
fn a_racks_banks_are_its_macros_and_a_held_parameter_is_not_moved() {
    let mut rig = Rig::new();
    let audio = rig.track(TrackKind::Audio);
    rig.select(audio);
    let (rack, utility, _) = rig.rack_with_utility();
    rig.run("device.select", json!({ "device": rack }));
    let focus = rig.focus(1);
    assert_eq!(focus["device"], json!(rack));
    assert_eq!(focus["banks"], json!(["Macros 1–8"]));
    let banks = rig.banks(rack);
    assert_eq!(banks.len(), 1);
    assert_eq!(
        banks[0].1,
        (1..=8).map(|n| bl_model::macro_param(n).0).collect::<Vec<_>>()
    );

    rig.cc(0, 127);
    assert!((rig.param(rack, bl_model::macro_param(1).0).unwrap() - 1.0).abs() < 1e-9);

    rig.run("rack.set_macro_count", json!({ "rack": rack, "count": 16 }));
    assert_eq!(rig.focus(1)["banks"], json!(["Macros 1–8", "Macros 9–16"]));
    rig.tap(1);
    rig.cc(0, 127);
    assert!((rig.param(rack, bl_model::macro_param(9).0).unwrap() - 1.0).abs() < 1e-9);

    // Macro 1 holds Width: the surface, locked to the Utility in the rack's
    // chain, doesn't move it and reads it as held.
    rig.run(
        "control_surface.lock",
        json!({ "row": 1, "device": utility }),
    );
    assert_eq!(rig.focus(1)["device"], json!(utility));
    rig.invoke(
        "rack.map_macro",
        json!({
            "rack": rack,
            "macro_number": 1,
            "target": { "type": "device", "device": utility, "param": 6 },
        }),
    );
    let banks = rig.banks(utility);
    let width = banks[0].1.iter().position(|p| *p == 6).unwrap();
    let held = rig.run("control_surface.banks", json!({ "device": utility }));
    assert_eq!(held["banks"][0]["params"][width]["held"], true);
    let (before, refused) = (rig.param(utility, 6), rig.shell.surface_counts().1);
    rig.cc(u8::try_from(width).unwrap(), 127);
    assert_eq!(rig.param(utility, 6), before, "not moved");
    assert_eq!(rig.shell.surface_counts().1, refused + 1);
}

// Spec: control-surfaces R21. A plug-in's banks are its configured list in
// eights, and change when it is reordered or shortened.
#[test]
fn a_plug_ins_banks_follow_its_configured_list() {
    let mut rig = Rig::new();
    let audio = rig.track(TrackKind::Audio);
    rig.select(audio);
    let loaded: Value = rig.run(
        "plugin.load",
        json!({ "path": bl_test_clap::bundle(), "track": audio }),
    );
    let device: DeviceId = serde_json::from_value(loaded["device"].clone()).unwrap();
    rig.run("device.select", json!({ "device": device }));
    let banks = rig.banks(device);
    assert_eq!(banks.len(), 1, "eight parameters, one bank");
    assert_eq!(banks[0].0, "1–8");
    let list = banks[0].1.clone();
    assert_eq!(list.len(), 8);
    assert_eq!(rig.focus(1)["bank_name"], "1–8");

    rig.run(
        "plugin.move_configured_param",
        json!({ "device": device, "from": 7, "to": 0 }),
    );
    let moved = rig.banks(device)[0].1.clone();
    assert_eq!(moved[0], list[7]);
    assert_eq!(moved[1..], list[..7]);

    rig.run(
        "plugin.unconfigure_param",
        json!({ "device": device, "param": list[0] }),
    );
    assert_eq!(rig.banks(device)[0].1.len(), 7);
}

// Spec: control-surfaces R22. With Takeover Mode Pick-Up, a knob after a bank
// change moves nothing until it crosses the value.
#[test]
fn with_pick_up_a_knob_after_a_bank_change_moves_nothing_until_it_crosses_the_value() {
    let mut rig = Rig::new();
    rig.run("settings.set_takeover_mode", json!({ "mode": "pick_up" }));
    let audio = rig.track(TrackKind::Audio);
    rig.select(audio);
    let utility = rig.add(UTILITY, "audio_effect");
    rig.run(
        "device.set_param",
        json!({ "device": utility, "param": 6, "value": 100.0 }),
    );
    let width = rig.param(utility, 6);

    // The knob has to meet the control before it moves it.
    rig.cc(1, 0);
    assert_eq!(rig.param(utility, 6), width, "nothing at first");
    rig.cc(1, 127);
    assert_ne!(rig.param(utility, 6), width, "it crossed the value");

    // The control moves away, and a bank change forgets where the knob was.
    rig.run(
        "device.set_param",
        json!({ "device": utility, "param": 6, "value": 100.0 }),
    );
    rig.tap(1);
    rig.tap(0);
    rig.cc(1, 127);
    assert_eq!(rig.param(utility, 6), Some(100.0), "moves nothing until it crosses");
    rig.cc(1, 0);
    assert_ne!(rig.param(utility, 6), Some(100.0), "crossed");
}

// Spec: control-surfaces R23, R24, R25. Locking keeps the knobs on the device
// across selection changes, a move to another track and into a rack;
// deleting it ends the lock and undo doesn't restore it.
#[test]
fn locking_keeps_the_knobs_on_the_device_until_it_is_deleted() {
    let mut rig = Rig::new();
    let (first, second) = (rig.track(TrackKind::Audio), rig.new_track("audio", "Second"));
    rig.select(first);
    let locked = rig.add(UTILITY, "audio_effect");
    rig.select(second);
    let other = rig.add(UTILITY, "audio_effect");
    assert_eq!(rig.focus(1)["device"], json!(other));

    // Lock to is no undo step; the knobs now reach the locked device.
    let steps = rig.steps();
    let reply = rig.run(
        "control_surface.lock",
        json!({ "row": 1, "device": locked }),
    );
    assert_eq!(rig.steps(), steps, "no undo step");
    assert_eq!(reply["surface_state"]["device"]["device"], json!(locked));
    assert_eq!(reply["surface_state"]["device"]["locked"], "hand");
    let gain = rig.banks(locked)[0].1[0];
    rig.cc(0, 127);
    assert!(rig.param(locked, gain).is_some(), "the locked device moved");
    assert_eq!(rig.param(other, gain), None, "the selected device isn't touched");

    // device_view.state names the surface each device is locked to.
    rig.select(first);
    let state = rig.run("device_view.state", json!({}));
    assert_eq!(
        state["devices"][0]["locks"],
        json!([{ "row": 1, "surface": "Test Instant Surface", "by": "hand" }])
    );
    rig.select(second);
    let state = rig.run("device_view.state", json!({}));
    assert!(state["devices"][0].get("locks").is_none());

    // The lock stays when the device moves to another track and into a rack.
    rig.run(
        "device.move",
        json!({ "device": locked, "track": second, "index": 0 }),
    );
    assert_eq!(rig.focus(1)["device"], json!(locked));
    let (_rack, _inner, chain) = rig.rack_with_utility();
    rig.run(
        "device.move_to_chain",
        json!({ "device": locked, "chain": chain, "index": 0 }),
    );
    assert_eq!(rig.focus(1)["device"], json!(locked));
    assert_eq!(rig.focus(1)["locked"], "hand");

    // Locking again gives it back; locking another moves the lock.
    rig.run("control_surface.lock", json!({ "row": 1, "device": locked }));
    assert!(rig.focus(1)["locked"].is_null());
    rig.run("control_surface.lock", json!({ "row": 1, "device": locked }));
    rig.run("control_surface.lock", json!({ "row": 1, "device": other }));
    assert_eq!(rig.focus(1)["device"], json!(other));
    rig.run("control_surface.unlock", json!({ "row": 1 }));
    assert!(rig.focus(1)["locked"].is_null());

    // Deleting the device ends its lock, and undo doesn't bring it back.
    rig.run("control_surface.lock", json!({ "row": 1, "device": locked }));
    rig.run("device.remove", json!({ "device": locked }));
    assert!(rig.focus(1)["locked"].is_null(), "the lock ended");
    assert_ne!(rig.focus(1)["device"], json!(locked));
    rig.run("edit.undo", json!({}));
    assert!(rig.shell.editor().document().device(locked).is_some());
    assert!(rig.focus(1)["locked"].is_null(), "undo doesn't restore it");
}

// Spec: control-surfaces R26, R27. Arming a MIDI track locks its instrument,
// a second armed track takes over, disarming falls back, and a lock made by
// hand survives arming. A surface that can't lock isn't locked.
#[test]
fn arming_locks_the_instrument_and_a_hand_lock_survives_it() {
    let mut rig = Rig::new();
    rig.set(2, "test", "Other");
    let (first, second) = (rig.track(TrackKind::Midi), rig.new_track("midi", "Second"));
    rig.select(first);
    let one = rig.add(bl_devices::POLYSYNTH, "instrument");
    rig.select(second);
    let two = rig.add(bl_devices::POLYSYNTH, "instrument");
    let audio = rig.track(TrackKind::Audio);
    rig.select(audio);
    let fx = rig.add(UTILITY, "audio_effect");
    let arm = |rig: &mut Rig, track, on| {
        rig.run("mixer.set_arm", json!({ "track": track, "arm": on }));
    };

    arm(&mut rig, first, true);
    assert_eq!(rig.focus(1)["device"], json!(one));
    assert_eq!(rig.focus(1)["locked"], "arming");
    assert!(
        rig.focus(2)["locked"].is_null(),
        "a surface that can't lock isn't locked"
    );
    assert_ne!(rig.focus(2)["device"], json!(one));
    arm(&mut rig, second, true);
    assert_eq!(rig.focus(1)["device"], json!(two), "the latest wins");
    arm(&mut rig, second, false);
    assert_eq!(rig.focus(1)["device"], json!(one), "disarming falls back");
    arm(&mut rig, first, false);
    assert_ne!(rig.focus(1)["locked"], "arming");
    assert_eq!(rig.focus(1)["device"], json!(fx), "back to the device in focus");

    // A lock by hand isn't replaced by arming.
    rig.run("control_surface.lock", json!({ "row": 1, "device": fx }));
    arm(&mut rig, first, true);
    assert_eq!(rig.focus(1)["device"], json!(fx));
    assert_eq!(rig.focus(1)["locked"], "hand");
    rig.run("control_surface.unlock", json!({ "row": 1 }));
    arm(&mut rig, first, false);

    // An instrument deleted ends the lock arming made.
    arm(&mut rig, second, true);
    assert_eq!(rig.focus(1)["device"], json!(two));
    rig.run("device.remove", json!({ "device": two }));
    assert_ne!(rig.focus(1)["device"], json!(two));
    assert!(
        rig.shell
            .run("control_surface.lock", json!({ "row": 2, "device": fx }))
            .is_err(),
        "a surface that can't lock refuses"
    );
}

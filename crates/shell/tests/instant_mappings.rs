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
        self.shell
            .editor()
            .document()
            .track(track)
            .unwrap()
            .mixer()
            .volume
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
    let (first, second) = (
        rig.track(TrackKind::Audio),
        rig.new_track("audio", "Second"),
    );
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
        rig.param(on_first, gain)
            .is_none_or(|v| (v - high).abs() > 1e-9),
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
    assert!(
        rig.param(on_second, bass).is_some(),
        "the bank's first parameter"
    );
    rig.select(first);
    assert_eq!(
        rig.focus(1)["bank"],
        0,
        "the other device is on its own bank"
    );
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
        (1..=8)
            .map(|n| bl_model::macro_param(n).0)
            .collect::<Vec<_>>()
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
    assert_eq!(
        rig.param(utility, 6),
        Some(100.0),
        "moves nothing until it crosses"
    );
    rig.cc(1, 0);
    assert_ne!(rig.param(utility, 6), Some(100.0), "crossed");
}

// Spec: control-surfaces R23, R24, R25. Locking keeps the knobs on the device
// across selection changes, a move to another track and into a rack;
// deleting it ends the lock and undo doesn't restore it.
#[test]
fn locking_keeps_the_knobs_on_the_device_until_it_is_deleted() {
    let mut rig = Rig::new();
    let (first, second) = (
        rig.track(TrackKind::Audio),
        rig.new_track("audio", "Second"),
    );
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
    assert_eq!(
        rig.param(other, gain),
        None,
        "the selected device isn't touched"
    );

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
    rig.run(
        "control_surface.lock",
        json!({ "row": 1, "device": locked }),
    );
    assert!(rig.focus(1)["locked"].is_null());
    rig.run(
        "control_surface.lock",
        json!({ "row": 1, "device": locked }),
    );
    rig.run("control_surface.lock", json!({ "row": 1, "device": other }));
    assert_eq!(rig.focus(1)["device"], json!(other));
    rig.run("control_surface.unlock", json!({ "row": 1 }));
    assert!(rig.focus(1)["locked"].is_null());

    // Deleting the device ends its lock, and undo doesn't bring it back.
    rig.run(
        "control_surface.lock",
        json!({ "row": 1, "device": locked }),
    );
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
    assert_eq!(
        rig.focus(1)["device"],
        json!(fx),
        "back to the device in focus"
    );

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

/// The ring of row 1 as `control_surface.list` reports it.
fn ring(rig: &mut Rig) -> Value {
    rig.state(1)["ring"].clone()
}

fn first_track(rig: &mut Rig) -> TrackId {
    serde_json::from_value(ring(rig)["first_track"].clone()).unwrap()
}

/// A set of twenty-odd audio tracks and the default returns, so an 8 wide ring
/// has room to move.
fn long_set(rig: &mut Rig) -> Vec<TrackId> {
    for n in 0..20 {
        rig.new_track("audio", &format!("T{n}"));
    }
    let columns = rig.shell.mixer_columns();
    assert!(columns.len() > 9, "{}", columns.len());
    columns
}

// Spec: control-surfaces R28, R29, R31. The ring moves a track or a page, stops
// with the last track in its last column, and changes no selection and no
// undo step.
#[test]
fn the_ring_moves_by_a_track_or_a_page_and_stops_at_the_last_track() {
    let mut rig = Rig::new();
    let columns = long_set(&mut rig);
    let returns: Vec<TrackId> = rig.shell.editor().document().returns().collect();
    let all: Vec<TrackId> = columns.iter().chain(&returns).copied().collect();
    let last = all.len() - 8;

    let start = ring(&mut rig);
    assert_eq!(start["track_index"], 0);
    assert_eq!(start["scene_index"], 0);
    assert_eq!(
        (start["tracks"].as_u64(), start["scenes"].as_u64()),
        (Some(8), Some(2))
    );
    assert_eq!(
        start["can_move"],
        json!({ "left": false, "right": true, "up": false, "down": true })
    );

    let (steps, selected) = (
        rig.steps(),
        rig.shell.arrangement().selected_tracks().to_vec(),
    );
    rig.tap(3);
    assert_eq!(first_track(&mut rig), all[1], "one track");
    rig.tap(7);
    assert_eq!(
        ring(&mut rig)["track_index"],
        9,
        "a page is the ring's width"
    );
    rig.tap(7);
    rig.tap(7);
    assert_eq!(
        ring(&mut rig)["track_index"],
        last,
        "a page stops at the last"
    );
    assert_eq!(first_track(&mut rig), all[last]);
    assert_eq!(ring(&mut rig)["can_move"]["right"], false);
    rig.tap(3);
    assert_eq!(ring(&mut rig)["track_index"], last, "stays");
    rig.tap(6);
    assert_eq!(ring(&mut rig)["track_index"], last - 8);
    rig.tap(2);
    assert_eq!(ring(&mut rig)["track_index"], last - 9);
    // Moving the ring selects nothing and adds no undo step.
    assert_eq!(rig.steps(), steps);
    assert_eq!(rig.shell.arrangement().selected_tracks(), selected);

    // Scenes move the same way, down to the last row.
    let scenes = rig.shell.editor().document().scenes().len();
    rig.tap(5);
    assert_eq!(ring(&mut rig)["scene_index"], 1);
    rig.tap(9);
    rig.tap(9);
    rig.tap(9);
    rig.tap(9);
    assert_eq!(ring(&mut rig)["scene_index"], scenes - 2);
    assert_eq!(ring(&mut rig)["can_move"]["down"], false);
    rig.tap(8);
    assert_eq!(ring(&mut rig)["scene_index"], scenes - 4);

    // control_surface.move_ring names a place or a step, and reads back.
    let moved = rig.run(
        "control_surface.move_ring",
        json!({ "row": 1, "track": all[2], "scene": null }),
    );
    assert_eq!(moved["surface_state"]["ring"]["first_track"], json!(all[2]));
    let stepped = rig.run(
        "control_surface.move_ring",
        json!({ "row": 1, "step": "page_right" }),
    );
    assert_eq!(
        stepped["surface_state"]["ring"]["track_index"],
        10.min(last)
    );
}

// Spec: control-surfaces R28. With fewer tracks than its width the ring stays
// at the first track.
#[test]
fn a_ring_wider_than_the_set_stays_at_the_first_track() {
    let mut rig = Rig::new();
    let columns = rig.shell.mixer_columns().len();
    let returns = rig.shell.editor().document().returns().len();
    assert!(columns + returns < 8, "the default set is short");
    rig.tap(3);
    rig.tap(7);
    assert_eq!(ring(&mut rig)["track_index"], 0);
    assert_eq!(ring(&mut rig)["can_move"]["right"], false);
    // The ring's tracks are the columns, then the returns.
    let tracks = rig.ring_tracks();
    assert_eq!(tracks.len(), columns + returns);
}

// Spec: control-surfaces R30. A ring keeps its first track and scene by ID
// across insertions before it, and across the deletion of it.
#[test]
fn the_ring_keeps_its_first_track_by_id() {
    let mut rig = Rig::new();
    let columns = long_set(&mut rig);
    rig.run(
        "control_surface.move_ring",
        json!({ "row": 1, "track": columns[3] }),
    );
    // A track moved in front of it moves the ring with it.
    let moved = *columns.last().unwrap();
    rig.run("track.move", json!({ "track": moved, "index": 0 }));
    assert_eq!(first_track(&mut rig), columns[3]);
    assert_eq!(ring(&mut rig)["track_index"], 4);

    // Deleting it leaves the ring at that place, on the track there now.
    rig.run("track.delete", json!({ "track": columns[3] }));
    let at = ring(&mut rig)["track_index"].as_u64().unwrap();
    assert_eq!(at, 4);
    let now = rig.shell.mixer_columns();
    assert_eq!(first_track(&mut rig), now[4]);

    // The newly opened set starts the ring again.
    rig.run("set.new", json!({}));
    assert_eq!(ring(&mut rig)["track_index"], 0);
}

// Spec: control-surfaces R30. Folding a group that holds the ring's first
// track moves the ring to the group.
#[test]
fn folding_a_group_moves_a_ring_that_starts_inside_it_to_the_group() {
    let mut rig = Rig::new();
    long_set(&mut rig);
    let columns = rig.shell.mixer_columns();
    let (a, b) = (columns[2], columns[3]);
    rig.invoke("arrangement.select_tracks", json!({ "tracks": [a, b] }));
    rig.invoke("track.group", json!({ "tracks": [a, b] }));
    let columns = rig.shell.mixer_columns();
    let group = rig.shell.editor().document().parent(a).unwrap();
    assert!(columns.contains(&a) && columns.contains(&group));
    rig.run("control_surface.move_ring", json!({ "row": 1, "track": b }));
    assert_eq!(first_track(&mut rig), b);
    rig.invoke("arrangement.select_tracks", json!({ "tracks": [group] }));
    rig.invoke("arrangement.fold_groups", json!({ "fold": true }));
    assert!(!rig.shell.mixer_columns().contains(&b), "folded away");
    assert_eq!(first_track(&mut rig), group);
}

// Spec: control-surfaces R32, R33, R34. The mixer controls cover the ring's
// tracks, and a control past the last track does nothing; moving the ring
// moves no selection and selecting a track moves no ring.
#[test]
fn mixer_controls_cover_the_rings_tracks() {
    let mut rig = Rig::new();
    let columns = long_set(&mut rig);
    let before: Vec<f64> = columns.iter().map(|t| rig.volume(*t)).collect();
    rig.cc(16, 20);
    assert_ne!(
        rig.volume(columns[0]),
        before[0],
        "column 1 is the ring's first track"
    );
    assert_eq!(rig.volume(columns[1]), before[1]);

    // Moved one track along, the same control reaches the next track.
    rig.tap(3);
    rig.cc(16, 20);
    assert_ne!(rig.volume(columns[1]), before[1]);
    assert_eq!(rig.volume(columns[2]), before[2]);

    // Mute, Solo and Arm are toggles over the ring's tracks.
    let doc = |rig: &Rig, track: TrackId| {
        rig.shell
            .editor()
            .document()
            .track(track)
            .unwrap()
            .mixer()
            .clone()
    };
    assert!(!doc(&rig, columns[1]).solo);
    rig.tap(48);
    assert!(doc(&rig, columns[1]).solo);
    rig.tap(48);
    assert!(!doc(&rig, columns[1]).solo);
    rig.tap(40);
    assert!(doc(&rig, columns[1]).mute);

    // Selecting is the track's own, and doesn't move the ring.
    let ring_at = ring(&mut rig)["track_index"].clone();
    rig.tap(66);
    assert!(rig.shell.session().state().tracks.contains(&columns[3]));
    assert_eq!(
        ring(&mut rig)["track_index"],
        ring_at,
        "selecting moves no ring"
    );

    // Past the last track a control does nothing, and isn't refused.
    rig.run(
        "control_surface.move_ring",
        json!({ "row": 1, "step": "page_right" }),
    );
    rig.run(
        "control_surface.move_ring",
        json!({ "row": 1, "step": "page_right" }),
    );
    let counts = rig.shell.surface_counts();
    let last = ring(&mut rig)["track_index"].as_u64().unwrap();
    assert!(last + 8 >= rig.shell.mixer_columns().len() as u64);
    rig.cc(16 + 7, 5);
    assert!(rig.shell.surface_counts().1 == counts.1);
}

// Spec: control-surfaces R35, session-view R38. `session.state` reports each
// ring's tracks and scenes by ID.
#[test]
fn session_state_reports_each_rings_tracks_and_scenes() {
    let mut rig = Rig::new();
    let columns = long_set(&mut rig);
    rig.set(2, "test", "Other");
    let state = rig.run("session.state", json!({}));
    let rings = state["rings"].as_array().unwrap();
    assert_eq!(rings.len(), 2);
    assert_eq!(rings[0]["row"], 1);
    assert_eq!(rings[0]["surface"], "Test Instant Surface");
    assert_eq!(rings[0]["tracks"].as_array().unwrap().len(), 8);
    assert_eq!(rings[0]["tracks"][0], json!(columns[0]));
    assert_eq!(rings[0]["scenes"].as_array().unwrap().len(), 2);
    rig.tap(3);
    let state = rig.run("session.state", json!({}));
    assert_eq!(state["rings"][0]["tracks"][0], json!(columns[1]));
    assert_eq!(state["rings"][1]["tracks"][0], json!(columns[0]), "its own");
}

// Spec: control-surfaces R22. A relative knob steps the parameter as a
// relative mapping does, and instant mappings are never listed as mappings
// or saved with the set.
#[test]
fn a_relative_knob_steps_the_parameter_and_leaves_no_mapping() {
    let mut rig = Rig::new();
    let audio = rig.track(TrackKind::Audio);
    rig.select(audio);
    let utility = rig.add(UTILITY, "audio_effect");
    let gain = rig.banks(utility)[0].1[0];
    rig.run(
        "device.set_param",
        json!({ "device": utility, "param": gain, "value": 0.5 }),
    );
    let start = rig.param(utility, gain).unwrap();
    // Two's complement: 1 is one step up, 127 one step down.
    rig.cc(8, 1);
    rig.cc(8, 1);
    let up = rig.param(utility, gain).unwrap();
    assert!(up > start, "{start} to {up}");
    rig.cc(8, 127);
    let down = rig.param(utility, gain).unwrap();
    assert!(down < up && down > start, "{up} to {down}");
    assert_eq!(
        rig.run("remote.mappings", json!({}))["mappings"],
        json!([]),
        "an instant mapping is no mapping"
    );
}

// Spec: control-surfaces R7. A knob turned several times is one undo step,
// which ends when it has rested half a second.
#[test]
fn a_knob_turned_several_times_is_one_undo_step() {
    let mut rig = Rig::new();
    let audio = rig.track(TrackKind::Audio);
    rig.select(audio);
    let utility = rig.add(UTILITY, "audio_effect");
    let steps = rig.steps();
    for value in [10, 40, 90] {
        rig.cc(0, value);
    }
    assert!(rig.shell.gesture_under_way(), "the step stays open");
    rig.pass(600);
    assert!(!rig.shell.gesture_under_way(), "it rested");
    assert_eq!(rig.steps(), steps + 1, "one undo step");
    let gain = rig.banks(utility)[0].1[0];
    assert!(rig.param(utility, gain).is_some());
}

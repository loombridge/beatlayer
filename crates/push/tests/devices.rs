//! Device Mode (`docs/specs/push-2-tracks.md`, R15 to R27) on the simulated
//! Push 2, with a read model the test builds by hand.

use std::collections::BTreeSet;
use std::sync::Arc;

use bl_control::{Ask, DeviceViews, Focus, ParamView};
use bl_model::{
    Chain, ChainId, Command, Device, DeviceId, DeviceKind, DeviceRole, ParamDescriptor, ParamId,
    ParamUnit, RackKind, RackView, RackViews,
};
use serde_json::{Value, json};

use crate::rig::Rig;

/// What the surface asked for: a command with its gesture, or the end of one.
#[derive(Debug, PartialEq)]
enum Asked {
    Run {
        id: String,
        args: Value,
        control: Option<u32>,
        released: bool,
    },
    End(u32),
}

struct Devs {
    rig: Rig,
    /// The device in focus, as the window selects it.
    focus: Option<DeviceId>,
    bank: usize,
    banks: usize,
    /// Parameters a macro holds, by number.
    held: BTreeSet<u32>,
    racks: bl_model::RackViewMap,
    chains: std::collections::BTreeMap<DeviceId, ChainId>,
}

fn effect(id: DeviceId, name: &str, role: DeviceRole) -> Device {
    Device::new(
        id,
        name,
        DeviceKind::Native {
            device_type: "test".to_owned(),
        },
    )
    .with_role(role)
}

impl Devs {
    fn new() -> Self {
        let mut rig = Rig::devices();
        rig.model.selection.tracks = vec![rig.tracks[0]];
        rig.start();
        rig.tick();
        Self {
            rig,
            focus: None,
            bank: 0,
            banks: 3,
            held: BTreeSet::new(),
            racks: bl_model::RackViewMap::new(),
            chains: std::collections::BTreeMap::new(),
        }
    }

    fn track(&self) -> bl_model::TrackId {
        self.rig.tracks[0]
    }

    /// Adds a device at the end of the track's chain.
    fn add(&mut self, name: &str, role: DeviceRole) -> DeviceId {
        let id = self.rig.editor.new_id();
        self.insert(effect(id, name, role))
    }

    fn insert(&mut self, device: Device) -> DeviceId {
        let id = device.id;
        let track = self.track();
        let index = self
            .rig
            .editor
            .document()
            .track(track)
            .unwrap()
            .devices()
            .len();
        self.rig.apply(Command::InsertDevice {
            track,
            index,
            device,
        });
        id
    }

    fn device_id(&mut self) -> DeviceId {
        self.rig.editor.new_id()
    }

    fn chain_id(&mut self) -> ChainId {
        self.rig.editor.new_id()
    }

    /// The window selects `device`, or none: Push 2 follows at the next tick.
    fn select(&mut self, device: Option<DeviceId>) {
        self.focus = device;
        self.refresh();
    }

    /// Builds what the shell would give: the device in focus and its bank.
    fn refresh(&mut self) {
        let doc = Arc::clone(&self.rig.model.document);
        self.rig.model.selected_device = self.focus;
        self.rig.model.devices = DeviceViews {
            racks: Arc::new(self.racks.clone()),
            chains: Arc::new(self.chains.clone()),
        };
        let first = || {
            let track = doc.track(self.track())?;
            track.devices().next().map(|d| d.id)
        };
        let device = self.focus.or_else(first);
        self.rig.model.rows[0].focus = device.and_then(|device| {
            let banks: Vec<String> = match doc.device(device)?.rack_kind() {
                Some(_) => vec!["Macros 1–8".to_owned(), "Macros 9–16".to_owned()],
                None => (0..self.banks).map(|n| format!("Bank {}", n + 1)).collect(),
            };
            let bank = self.bank.min(banks.len() - 1);
            let params = (0..8u32)
                .map(|k| {
                    let number = bank as u32 * 8 + k;
                    let descriptor = if k == 7 {
                        ParamDescriptor::choice(ParamId(number), "Mode", &["A", "B", "C"], 0)
                    } else {
                        ParamDescriptor::continuous(
                            ParamId(number),
                            &format!("P{number}"),
                            (0.0, 1.0),
                            0.5,
                            ParamUnit::None,
                        )
                    };
                    let value = doc
                        .device(device)
                        .and_then(|d| d.params.get(&ParamId(number)).copied())
                        .unwrap_or(descriptor.default);
                    ParamView {
                        param: ParamId(number),
                        name: descriptor.name.clone(),
                        value,
                        text: descriptor.format(value),
                        held: self.held.contains(&number),
                        descriptor,
                    }
                })
                .collect();
            Some(Focus {
                device,
                lock: None,
                bank,
                banks,
                params,
            })
        });
        self.rig.tick();
    }

    fn tap(&mut self, name: &str) {
        self.rig.tap(name);
    }

    /// Holds `name` past the hold time, a tick later.
    fn hold(&mut self, name: &str) {
        assert!(self.rig.sim.press(name));
        self.rig.settle();
        self.rig.advance(350);
    }

    fn let_go(&mut self, name: &str) {
        assert!(self.rig.sim.let_go(name));
        self.rig.advance(1);
    }

    fn text(&self) -> Vec<String> {
        self.rig.snapshot().screen.text()
    }

    fn shows(&self, line: &str) -> bool {
        self.text().iter().any(|l| l == line)
    }

    fn asked(&mut self) -> Vec<Asked> {
        self.rig
            .requests()
            .into_iter()
            .filter_map(|r| match r.ask {
                Ask::Command(c) => Some(Asked::Run {
                    id: c.id,
                    args: c.args,
                    control: c.gesture.map(|g| g.control),
                    released: c.gesture.is_some_and(|g| g.released),
                }),
                Ask::EndGesture(control) => Some(Asked::End(control)),
                _ => None,
            })
            .collect()
    }

    /// The commands asked for, by ID and arguments.
    fn commands(&mut self) -> Vec<(String, Value)> {
        self.asked()
            .into_iter()
            .filter_map(|a| match a {
                Asked::Run { id, args, .. } => Some((id, args)),
                Asked::End(_) => None,
            })
            .collect()
    }

    fn state(&self) -> bl_push::device::State {
        self.rig.snapshot().device.expect("Device Mode is shown")
    }
}

fn run(id: &str, args: Value) -> (String, Value) {
    (id.to_owned(), args)
}

// Spec: push-2-tracks R15, R18, R27. Device Mode on a track of three devices
// puts them on the upper buttons with the selected one highlighted, and its
// first bank on the encoders with names and values; it changes nothing.
#[test]
fn device_mode_shows_the_devices_and_the_first_bank_of_the_selected_one() {
    let mut d = Devs::new();
    let a = d.add("Reverb", DeviceRole::AudioEffect);
    let b = d.add("Delay", DeviceRole::AudioEffect);
    let c = d.add("Echo", DeviceRole::AudioEffect);
    d.select(Some(b));
    assert!(d.shows("upper 1: Reverb"), "{:?}", d.text());
    assert!(d.shows("upper 2: Delay (selected)"));
    assert!(d.shows("upper 3: Echo"));
    assert!(d.shows("column 1: P0 = 0.50"));
    assert!(d.shows("column 8: Mode = A"));
    let state = d.state();
    assert_eq!(
        state
            .devices
            .iter()
            .map(|s| (s.button, s.device, s.depth, s.selected))
            .collect::<Vec<_>>(),
        [(1, a, 0, false), (2, b, 0, true), (3, c, 0, false)]
    );
    assert!(!state.edit);
    assert_eq!((state.bank, state.banks.len()), (0, 3));
    assert_eq!(state.params.len(), 8);
    assert_eq!(state.params[0].name, "P0");
    // The lower buttons stand for the ring's tracks at the top level, and
    // Device Mode changes nothing.
    assert!(d.shows("lower 1: Track 1 (selected)"));
    assert!(d.asked().is_empty());
}

// Spec: push-2-tracks R16, R3. Tapping a device's button selects it; the
// second tap enters Edit Mode, whose lower buttons name the banks and choose
// one, and whose leftmost upper button goes back.
#[test]
fn a_second_tap_enters_edit_mode_and_its_lower_buttons_choose_the_bank() {
    let mut d = Devs::new();
    let a = d.add("Reverb", DeviceRole::AudioEffect);
    let b = d.add("Delay", DeviceRole::AudioEffect);
    d.select(Some(a));
    d.asked();
    d.tap("upper_2");
    assert_eq!(
        d.commands(),
        [run("device.select", json!({ "device": b }))],
        "a tap on another device selects it"
    );
    d.select(Some(b));
    d.tap("upper_2");
    assert!(d.state().edit, "the second tap enters Edit Mode");
    assert!(d.asked().is_empty(), "and runs no command");
    assert!(d.shows("lower 1: Bank 1 (selected)"), "{:?}", d.text());
    assert!(d.shows("lower 2: Bank 2"));
    assert!(d.shows("lower 3: Bank 3"));
    assert!(d.shows("upper 1: Back<left arrow>"), "{:?}", d.text());
    assert!(!d.text().iter().any(|l| l.starts_with("upper 2")));

    d.tap("lower_2");
    assert_eq!(
        d.commands(),
        [run(
            "control_surface.set_bank",
            json!({ "row": 1, "bank": 1 })
        )]
    );
    d.bank = 1;
    d.refresh();
    assert!(d.shows("lower 2: Bank 2 (selected)"));
    assert!(
        d.shows("column 1: P8 = 0.50"),
        "the second bank is on the encoders"
    );

    d.tap("upper_1");
    assert!(
        !d.state().edit,
        "the leftmost button returns to the top level"
    );
    assert!(d.shows("upper 2: Delay (selected)"));
}

// Spec: push-2-tracks R17. A device of more than eight banks pages: seven on
// the first page with an arrow on the rightmost button, a back arrow on the
// leftmost button of the next.
#[test]
fn edit_mode_pages_through_more_than_eight_banks_and_back() {
    let mut d = Devs::new();
    let a = d.add("Operator", DeviceRole::Instrument);
    d.banks = 10;
    d.select(Some(a));
    d.tap("upper_1");
    assert!(d.state().edit);
    assert!(d.shows("lower 1: Bank 1 (selected)"));
    assert!(d.shows("lower 7: Bank 7"));
    assert!(d.shows("lower 8: <right arrow>"), "{:?}", d.text());
    assert_eq!((d.state().page, d.state().pages), (0, 2));

    d.tap("lower_8");
    assert_eq!(d.state().page, 1);
    assert!(d.asked().is_empty(), "paging changes no bank");
    assert!(d.shows("lower 1: <left arrow>"));
    assert!(d.shows("lower 2: Bank 8"));
    assert!(d.shows("lower 4: Bank 10"));
    assert!(!d.text().iter().any(|l| l.starts_with("lower 5")));
    d.tap("lower_4");
    assert_eq!(
        d.commands(),
        [run(
            "control_surface.set_bank",
            json!({ "row": 1, "bank": 9 })
        )]
    );

    d.tap("lower_1");
    assert_eq!(d.state().page, 0);
    assert!(d.shows("lower 7: Bank 7"));
}

// Spec: push-2-tracks R15, R3. A track of ten devices has arrows on its first
// and last upper buttons that move the six between them by one, so the ninth
// is reached; selecting a device in the window brings it into view.
#[test]
fn a_track_of_ten_devices_reaches_the_ninth_by_the_arrows() {
    let mut d = Devs::new();
    let ids: Vec<DeviceId> = (1..=10)
        .map(|n| d.add(&format!("Fx {n}"), DeviceRole::AudioEffect))
        .collect();
    d.select(Some(ids[0]));
    assert!(
        !d.text().iter().any(|l| l.starts_with("upper 1:")),
        "nothing to the left"
    );
    assert!(d.shows("upper 2: Fx 1 (selected)"), "{:?}", d.text());
    assert!(d.shows("upper 7: Fx 6"));
    assert!(d.shows("upper 8: <right arrow>"));
    assert_eq!(d.state().arrows, (false, true));

    for _ in 0..3 {
        d.tap("upper_8");
    }
    assert!(d.shows("upper 1: <left arrow>"));
    assert!(d.shows("upper 2: Fx 4"));
    assert!(d.shows("upper 7: Fx 9"), "the ninth, by the arrows");
    assert!(d.asked().is_empty(), "the arrows change nothing in the set");
    d.tap("upper_7");
    assert_eq!(
        d.commands(),
        [run("device.select", json!({ "device": ids[8] }))]
    );
    d.tap("upper_1");
    assert!(d.shows("upper 2: Fx 3"), "back by one");

    // The window selecting the tenth shows it at once, with the arrows.
    d.select(Some(ids[9]));
    assert!(d.shows("upper 7: Fx 10 (selected)"), "{:?}", d.text());
    d.select(Some(ids[0]));
    assert!(d.shows("upper 2: Fx 1 (selected)"));
}

// Spec: push-2-tracks R18, R1, R4. Turning an encoder sets its parameter by
// a two hundredth of the range a step (a two thousandth with Shift), a list
// by one value for ten steps, and from touch to release is one gesture.
#[test]
fn an_encoder_sets_its_parameter_and_a_touch_is_one_gesture() {
    let mut d = Devs::new();
    let a = d.add("Reverb", DeviceRole::AudioEffect);
    d.select(Some(a));
    d.asked();
    assert!(d.rig.sim.touch(0));
    assert!(d.rig.sim.turn(0, 10));
    d.rig.advance(1);
    let asked = d.asked();
    let Asked::Run {
        id,
        args,
        control,
        released,
    } = &asked[0]
    else {
        panic!("{asked:?}");
    };
    assert_eq!(id, "device.set_param");
    assert_eq!(args["device"], json!(a));
    assert_eq!(args["param"], json!(0));
    assert!((args["value"].as_f64().unwrap() - 0.55).abs() < 1e-9);
    assert!(!released);
    let key = control.expect("part of a gesture");
    // A second turn before the set shows the first adds to it.
    assert!(d.rig.sim.turn(0, 10));
    d.rig.advance(1);
    let Asked::Run { args, .. } = &d.asked()[0] else {
        panic!("a command");
    };
    assert!((args["value"].as_f64().unwrap() - 0.60).abs() < 1e-9);
    assert!(d.rig.sim.release(0));
    d.rig.advance(1);
    assert_eq!(d.asked(), [Asked::End(key)], "the release ends the step");

    // Shift: finer. The ends stop it.
    assert!(d.rig.sim.press("shift"));
    d.rig.settle();
    assert!(d.rig.sim.touch(1));
    assert!(d.rig.sim.turn(1, 20));
    d.rig.advance(1);
    let Asked::Run { args, .. } = &d.asked()[0] else {
        panic!("a command");
    };
    assert!((args["value"].as_f64().unwrap() - 0.51).abs() < 1e-9);
    assert!(d.rig.sim.release(1));
    assert!(d.rig.sim.let_go("shift"));
    d.rig.advance(1);
    d.asked();
    assert!(d.rig.sim.touch(2));
    for _ in 0..4 {
        assert!(d.rig.sim.turn(2, 63));
    }
    d.rig.advance(1);
    let last = d.asked().into_iter().rev().find_map(|a| match a {
        Asked::Run { args, .. } => Some(args),
        Asked::End(_) => None,
    });
    assert_eq!(last.unwrap()["value"].as_f64().unwrap(), 1.0);

    // A list moves one value for ten steps.
    d.asked();
    assert!(d.rig.sim.turn(7, 9));
    d.rig.advance(1);
    assert!(d.asked().is_empty(), "nine steps are not a list step");
    assert!(d.rig.sim.turn(7, 1));
    d.rig.advance(1);
    let Asked::Run { args, .. } = &d.asked()[0] else {
        panic!("a command");
    };
    assert_eq!(args["value"], json!(1.0));
}

// Spec: push-2-tracks R18. A parameter a macro holds doesn't move and is
// drawn dimmed; a device that is switched off is grey with all its
// parameters.
#[test]
fn a_held_parameter_is_dimmed_and_unmoved_and_a_device_that_is_off_is_grey() {
    let mut d = Devs::new();
    let a = d.add("Reverb", DeviceRole::AudioEffect);
    d.held.insert(1);
    d.select(Some(a));
    d.asked();
    assert!(d.shows("column 2: P1 = 0.50 (dimmed)"), "{:?}", d.text());
    assert!(d.shows("column 1: P0 = 0.50"));
    assert!(d.rig.sim.touch(1));
    assert!(d.rig.sim.turn(1, 10));
    d.rig.advance(1);
    assert!(d.commands().is_empty(), "a held parameter doesn't move");

    d.rig.apply(Command::SetDeviceEnabled {
        device: a,
        enabled: false,
    });
    d.refresh();
    assert!(d.shows("column 1: P0 = 0.50 (grey)"), "{:?}", d.text());
    assert!(d.shows("column 3: P2 = 0.50 (grey)"));
    assert!(!d.state().devices[0].enabled);
    // The frame draws without it breaking.
    assert!(
        d.rig
            .snapshot()
            .screen
            .shades
            .contains(&bl_push::screen::Shade::Grey)
    );
}

// Spec: push-2-tracks R19. Holding Delete and pressing a device's button
// deletes it and selects the next device, or the one before when it was the
// last.
#[test]
fn delete_with_a_devices_button_deletes_it_and_selects_the_next() {
    let mut d = Devs::new();
    let a = d.add("A", DeviceRole::AudioEffect);
    let b = d.add("B", DeviceRole::AudioEffect);
    let c = d.add("C", DeviceRole::AudioEffect);
    d.select(Some(b));
    d.asked();
    assert!(d.rig.sim.press("delete"));
    d.rig.settle();
    d.tap("upper_2");
    assert_eq!(
        d.commands(),
        [
            run("device.remove", json!({ "device": b })),
            run("device.select", json!({ "device": c })),
        ]
    );
    assert!(d.rig.sim.let_go("delete"));
    d.rig.advance(1);
    assert!(
        d.asked().is_empty(),
        "Delete used with a button does nothing alone"
    );

    // The last: the one before.
    assert!(d.rig.sim.press("delete"));
    d.rig.settle();
    d.tap("upper_3");
    assert_eq!(
        d.commands(),
        [
            run("device.remove", json!({ "device": c })),
            run("device.select", json!({ "device": b })),
        ]
    );
    assert!(d.rig.sim.let_go("delete"));
    d.rig.advance(1);
    let _ = a;
}

// Spec: push-2-tracks R20. Holding Mute and pressing a device's button
// switches it off, or on when it is off; with Mute locked each button
// switches its device until Mute is pressed again.
#[test]
fn mute_held_or_locked_switches_a_device() {
    let mut d = Devs::new();
    let a = d.add("A", DeviceRole::AudioEffect);
    let b = d.add("B", DeviceRole::AudioEffect);
    d.select(Some(a));
    d.asked();
    assert!(d.rig.sim.press("mute"));
    d.rig.settle();
    d.tap("upper_2");
    assert_eq!(
        d.commands(),
        [run(
            "device.set_enabled",
            json!({ "device": b, "enabled": false })
        )]
    );
    assert!(d.rig.sim.let_go("mute"));
    d.rig.advance(1);
    assert!(d.asked().is_empty());

    // Locked: held alone for a second.
    d.rig.apply(Command::SetDeviceEnabled {
        device: b,
        enabled: false,
    });
    d.refresh();
    assert!(d.rig.sim.press("mute"));
    d.rig.settle();
    d.rig.advance(1100);
    assert!(d.rig.sim.let_go("mute"));
    d.rig.advance(1);
    assert!(d.rig.snapshot().locked.contains(&"mute"));
    d.asked();
    d.tap("upper_2");
    d.tap("upper_1");
    assert_eq!(
        d.commands(),
        [
            run(
                "device.set_enabled",
                json!({ "device": b, "enabled": true })
            ),
            run(
                "device.set_enabled",
                json!({ "device": a, "enabled": false })
            ),
        ],
        "each button switches its device, and a tap selects nothing"
    );
    d.tap("mute");
    assert!(d.rig.snapshot().locked.is_empty());
    d.asked();
    d.tap("upper_2");
    assert_eq!(
        d.commands(),
        [run("device.select", json!({ "device": b }))],
        "unlocked, a tap selects again"
    );
}

// Spec: push-2-tracks R21, R1. Holding an effect's button and turning an
// encoder moves it one place for each list step, only among the neighbours
// that keep the chain's order, as one undo step for the hold; an instrument
// doesn't move.
#[test]
fn a_held_effect_moves_where_the_chains_order_allows() {
    let mut d = Devs::new();
    let midi = d.add("Arp", DeviceRole::MidiEffect);
    let synth = d.add("Synth", DeviceRole::Instrument);
    let b = d.add("B", DeviceRole::AudioEffect);
    let c = d.add("C", DeviceRole::AudioEffect);
    let e = d.add("E", DeviceRole::AudioEffect);
    d.select(Some(b));
    d.asked();
    d.hold("upper_3");
    assert!(d.rig.sim.turn(0, 10));
    d.rig.advance(1);
    assert!(d.rig.sim.turn(1, 10));
    d.rig.advance(1);
    assert!(d.rig.sim.turn(2, 30));
    d.rig.advance(1);
    let asked = d.asked();
    let moves: Vec<_> = asked
        .iter()
        .filter_map(|a| match a {
            Asked::Run {
                id, args, control, ..
            } => Some((id.as_str(), args["index"].as_u64()?, *control)),
            Asked::End(_) => None,
        })
        .collect();
    // Two places right, then the end stops it.
    assert_eq!(
        moves.iter().map(|m| (m.0, m.1)).collect::<Vec<_>>(),
        [("device.move", 3), ("device.move", 4)]
    );
    let key = moves[0].2.expect("a gesture");
    assert!(moves.iter().all(|m| m.2 == Some(key)), "one gesture");
    d.let_go("upper_3");
    assert_eq!(d.asked(), [Asked::End(key)], "the release ends the step");

    // Left: an audio effect doesn't go before the instrument.
    d.hold("upper_3");
    assert!(d.rig.sim.turn(0, -40));
    d.rig.advance(1);
    d.let_go("upper_3");
    assert!(d.commands().is_empty(), "B can't pass the instrument");

    // An instrument and a MIDI effect among others of another kind stay.
    d.hold("upper_2");
    assert!(d.rig.sim.turn(0, 30));
    d.rig.advance(1);
    d.let_go("upper_2");
    assert!(d.commands().is_empty(), "an instrument doesn't move");
    d.hold("upper_1");
    assert!(d.rig.sim.turn(0, 30));
    d.rig.advance(1);
    d.let_go("upper_1");
    assert!(
        d.commands().is_empty(),
        "a lone MIDI effect has nowhere to go"
    );
    let _ = (midi, synth, c, e);
}

// Spec: push-2-tracks R22, R23. A rack carries its kind's icon and, while its
// devices view shows, the devices of its selected chain follow it with a line
// under them; the selected rack's second tap folds it through the window's own
// devices view; a drum rack doesn't fold from Push 2; Shift with a tap enters
// the rack's Edit Mode with its macros; a contained device is selectable.
#[test]
fn a_rack_unfolds_and_folds_with_its_devices_view() {
    let mut d = Devs::new();
    let inner1 = d.device_id();
    let inner2 = d.device_id();
    let chain_id = d.chain_id();
    let rack_id = d.device_id();
    let rack = Device::rack(rack_id, RackKind::AudioEffect).with_chain(
        Chain::new(chain_id, "Chain 1").with_devices(vec![
            effect(inner1, "Comp", DeviceRole::AudioEffect),
            effect(inner2, "EQ", DeviceRole::AudioEffect),
        ]),
    );
    d.add("Synth", DeviceRole::Instrument);
    d.insert(rack);
    d.chains.insert(rack_id, chain_id);
    d.select(Some(rack_id));
    assert!(
        d.shows("upper 2: Audio Effect Rack [audio effect rack] (selected)"),
        "{:?}",
        d.text()
    );
    assert!(d.shows("upper 3: Comp"));
    assert!(d.shows("upper 4: EQ"));
    assert!(d.shows("fold: 2 to 4"), "{:?}", d.text());
    let state = d.state();
    assert_eq!(state.devices[1].kind, "audio_effect_rack");
    assert!(state.devices[1].unfolded);
    assert_eq!(
        state.devices[2].depth, 1,
        "the rack's devices are one deeper"
    );
    assert_eq!(
        state.banks,
        ["Macros 1–8", "Macros 9–16"],
        "a rack's encoders play its macros"
    );

    // A contained device is selectable, and the encoders play its parameters.
    d.asked();
    d.tap("upper_3");
    assert_eq!(
        d.commands(),
        [run("device.select", json!({ "device": inner1 }))]
    );

    // The second tap folds it through the devices view.
    d.tap("upper_2");
    d.asked();
    d.tap("upper_2");
    assert_eq!(
        d.commands(),
        [run(
            "rack.set_view",
            json!({ "rack": rack_id, "view": "devices", "shown": false })
        )]
    );
    d.racks
        .insert(rack_id, RackViews::default().with(RackView::Devices, false));
    d.refresh();
    assert!(!d.text().iter().any(|l| l.starts_with("fold:")));
    assert!(!d.shows("upper 3: Comp"), "folded: its devices are gone");
    d.select(Some(rack_id));
    d.tap("upper_2");
    assert_eq!(
        d.commands(),
        [run(
            "rack.set_view",
            json!({ "rack": rack_id, "view": "devices", "shown": true })
        )],
        "and the second tap unfolds it again"
    );

    // Shift with a tap enters the rack's Edit Mode: Macros 1–8 and 9–16.
    assert!(d.rig.sim.press("shift"));
    d.rig.settle();
    d.tap("upper_2");
    assert!(d.rig.sim.let_go("shift"));
    d.rig.advance(1);
    assert!(d.state().edit);
    assert!(d.shows("lower 1: Macros 1–8 (selected)"), "{:?}", d.text());
    assert!(d.shows("lower 2: Macros 9–16"));
    assert!(d.commands().is_empty());
}

// Spec: push-2-tracks R22. A drum rack doesn't fold or unfold from Push 2:
// its second tap runs nothing, and it shows as the window has it.
#[test]
fn a_drum_rack_does_not_fold_from_push_2() {
    let mut d = Devs::new();
    let chain_id = d.chain_id();
    let rack_id = d.device_id();
    let inner = d.device_id();
    let rack = Device::rack(rack_id, RackKind::Drum).with_chain(
        Chain::new(chain_id, "Kick").with_devices(vec![effect(
            inner,
            "Kick Synth",
            DeviceRole::Instrument,
        )]),
    );
    d.insert(rack);
    d.chains.insert(rack_id, chain_id);
    d.select(Some(rack_id));
    assert!(
        d.shows("upper 1: Drum Rack [drum rack] (selected)"),
        "{:?}",
        d.text()
    );
    assert!(d.shows("upper 2: Kick Synth"));
    d.asked();
    d.tap("upper_1");
    assert!(d.commands().is_empty(), "a drum rack doesn't fold");
    assert!(!d.state().edit, "and doesn't enter Edit Mode either");

    // As the window has it: folded there, folded here.
    d.racks
        .insert(rack_id, RackViews::default().with(RackView::Devices, false));
    d.refresh();
    assert!(!d.shows("upper 2: Kick Synth"));
}

// Spec: push-2-tracks R24. Holding a rack's button shows its chains along
// the lower buttons, the selected one lit, and pressing one selects it;
// releasing gives the lower buttons back to the tracks.
#[test]
fn holding_a_racks_button_shows_its_chains_and_selects_one() {
    let mut d = Devs::new();
    let rack_id = d.device_id();
    let chains: Vec<ChainId> = (0..3).map(|_| d.chain_id()).collect();
    let mut rack = Device::rack(rack_id, RackKind::Instrument);
    for (n, id) in chains.iter().enumerate() {
        rack = rack.with_chain(Chain::new(*id, format!("Layer {}", n + 1)));
    }
    d.insert(rack);
    d.chains.insert(rack_id, chains[0]);
    d.select(Some(rack_id));
    d.asked();
    d.hold("upper_1");
    assert!(d.shows("lower 1: Layer 1 (selected)"), "{:?}", d.text());
    assert!(d.shows("lower 3: Layer 3"));
    assert_eq!(d.state().chains.len(), 3);
    d.tap("lower_3");
    assert_eq!(
        d.commands(),
        [run("chain.select", json!({ "chain": chains[2] }))]
    );
    d.chains.insert(rack_id, chains[2]);
    d.refresh();
    assert!(d.shows("lower 3: Layer 3 (selected)"));
    d.let_go("upper_1");
    assert!(
        d.shows("lower 1: Track 1 (selected)"),
        "the tracks have the lower buttons back"
    );
    assert!(d.state().chains.is_empty());
    assert!(
        d.asked().is_empty(),
        "holding and releasing a rack's button selects and unfolds nothing"
    );
}

// Spec: push-2-tracks R26, R3. A track with no devices says so and offers
// Add Device; selecting a device in the window redraws Push 2 at once.
#[test]
fn an_empty_track_says_so_and_the_window_redraws_push_2() {
    let mut d = Devs::new();
    assert!(
        d.shows("title: This track has no devices: press Add Device"),
        "{:?}",
        d.text()
    );
    assert!(d.state().empty);
    assert_ne!(
        d.rig.snapshot().led("add_device").unwrap().colour,
        0,
        "Add Device is lit"
    );
    let a = d.add("A", DeviceRole::AudioEffect);
    let b = d.add("B", DeviceRole::AudioEffect);
    d.select(Some(a));
    assert!(!d.text().iter().any(|l| l.starts_with("title:")));
    assert!(d.shows("upper 1: A (selected)"));
    d.select(Some(b));
    assert!(
        d.shows("upper 2: B (selected)"),
        "the window's selection shows"
    );
    assert!(d.shows("upper 1: A"));
    assert!(d.asked().is_empty(), "Push 2 only follows");
    d.rig.apply(Command::RemoveDevice { device: b });
    d.select(Some(a));
    assert!(
        !d.text().iter().any(|l| l.starts_with("upper 2")),
        "a deleted device goes"
    );
}

// Spec: push-2-tracks R21. An effect inside a rack's chain moves within that
// chain by `device.move_to_chain`, as one gesture.
#[test]
fn an_effect_inside_a_rack_moves_within_its_chain() {
    let mut d = Devs::new();
    let (comp, eq) = (d.device_id(), d.device_id());
    let chain_id = d.chain_id();
    let rack_id = d.device_id();
    let rack = Device::rack(rack_id, RackKind::AudioEffect).with_chain(
        Chain::new(chain_id, "Chain 1").with_devices(vec![
            effect(comp, "Comp", DeviceRole::AudioEffect),
            effect(eq, "EQ", DeviceRole::AudioEffect),
        ]),
    );
    d.insert(rack);
    d.chains.insert(rack_id, chain_id);
    d.select(Some(eq));
    d.asked();
    d.hold("upper_3");
    assert!(d.rig.sim.turn(0, -10));
    d.rig.advance(1);
    d.let_go("upper_3");
    let asked = d.asked();
    assert!(
        matches!(
            &asked[0],
            Asked::Run { id, args, .. }
                if id == "device.move_to_chain"
                    && *args == json!({ "device": eq, "chain": chain_id, "index": 0 })
        ),
        "{asked:?}"
    );
    assert!(matches!(asked[1], Asked::End(_)));
}

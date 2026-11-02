//! Collision in the shell (WP-8.21; the collision spec, R10): Copy sends every
//! setting of one resonator to the other as one undo step. Driven through
//! `Shell::run`, as an agent drives it.

use bl_devices::collision::{COLLISION, R_DECAY, R_PAN, R_TUNE, R_TYPE, RES_COUNT, res};
use bl_model::{DeviceId, DeviceKind, DeviceRole, ParamId, TrackKind};
use bl_shell::Shell;
use bl_shell::device_commands::{DeviceAdded, DeviceParameters};
use serde_json::json;

fn add(shell: &mut Shell, device_type: &str) -> DeviceId {
    let track = shell
        .editor()
        .document()
        .tracks()
        .find(|t| t.kind() == TrackKind::Midi)
        .map(bl_model::Track::id)
        .unwrap();
    shell
        .invoke("arrangement.select_tracks", json!({ "tracks": [track] }))
        .unwrap();
    shell.set_focused_area(bl_shell::Area::Detail);
    let added: DeviceAdded = serde_json::from_value(
        shell
            .run(
                "device.add",
                json!({ "device": {
                    "kind": DeviceKind::Native { device_type: device_type.to_owned() },
                    "name": "Device",
                    "role": DeviceRole::Instrument,
                } }),
            )
            .unwrap(),
    )
    .unwrap();
    added.device
}

fn set(shell: &mut Shell, device: DeviceId, id: ParamId, value: f64) {
    shell
        .run(
            "device.set_param",
            json!({ "device": device, "param": id, "value": value }),
        )
        .unwrap();
}

/// A resonator's settings, in the order of its parameters.
fn resonator(shell: &mut Shell, device: DeviceId, r: usize) -> Vec<f64> {
    let read: DeviceParameters = serde_json::from_value(
        shell
            .run("device.parameters", json!({ "device": device }))
            .unwrap(),
    )
    .unwrap();
    (0..RES_COUNT)
        .map(|offset| {
            read.params
                .iter()
                .find(|p| p.descriptor.id == res(r, offset))
                .unwrap()
                .value
        })
        .collect()
}

// Spec: collision R10
#[test]
fn copy_sends_every_setting_of_a_resonator_to_the_other_in_one_undo_step() {
    let mut shell = Shell::headless();
    let device = add(&mut shell, COLLISION);
    for (offset, value) in [
        (R_TYPE, 3.0),
        (R_DECAY, 7.5),
        (R_TUNE, -5.0),
        (R_PAN, -30.0),
    ] {
        set(&mut shell, device, res(0, offset), value);
    }
    let first = resonator(&mut shell, device, 0);
    let before = resonator(&mut shell, device, 1);
    assert_ne!(first, before, "the resonators start unlike");
    let copied = shell
        .run(
            "collision.copy_resonator",
            json!({ "device": device, "from": 1 }),
        )
        .unwrap();
    assert_eq!(copied, json!({ "device": device, "from": 1, "to": 2 }));
    assert_eq!(
        resonator(&mut shell, device, 1),
        first,
        "Resonator 2 is Resonator 1's"
    );
    assert_eq!(
        resonator(&mut shell, device, 0),
        first,
        "and Resonator 1 is as it was"
    );
    // One undo step puts Resonator 2 back.
    shell.run("edit.undo", json!({})).unwrap();
    assert_eq!(resonator(&mut shell, device, 1), before);
    assert_eq!(resonator(&mut shell, device, 0), first);
    // The other way round.
    set(&mut shell, device, res(1, R_DECAY), 0.3);
    let second = resonator(&mut shell, device, 1);
    shell
        .run(
            "collision.copy_resonator",
            json!({ "device": device, "from": 2 }),
        )
        .unwrap();
    assert_eq!(resonator(&mut shell, device, 0), second);
}

// Spec: collision R10
#[test]
fn copy_refuses_a_resonator_that_isnt_there_and_a_device_that_isnt_a_collision() {
    let mut shell = Shell::headless();
    let device = add(&mut shell, COLLISION);
    let before = resonator(&mut shell, device, 1);
    for from in [0, 3] {
        assert!(
            shell
                .run(
                    "collision.copy_resonator",
                    json!({ "device": device, "from": from })
                )
                .is_err(),
            "from {from}"
        );
    }
    assert_eq!(resonator(&mut shell, device, 1), before);
    let mut other = Shell::headless();
    let meld = add(&mut other, bl_devices::meld::MELD);
    assert!(
        other
            .run(
                "collision.copy_resonator",
                json!({ "device": meld, "from": 1 })
            )
            .is_err(),
        "a Meld has no resonators"
    );
}

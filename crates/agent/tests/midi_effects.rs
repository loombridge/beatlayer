//! The MIDI effects through the agent API (WP-5.30): an agent adds each to a
//! MIDI track, sets and reads back every parameter, undoes an edit in one
//! step, switches it off, and saves its preset under MIDI Effects.

use bl_agent::Agent;
use bl_agent::client::{Client, InProcess};
use bl_devices::{
    arpeggiator_type, cc_control_type, dice_type, hold_type, lift_type, midi_monitor_type,
    mpe_control_type, note_echo_type, remap_type, stack_type, touch_type,
};
use bl_model::{DeviceId, TrackId, TrackKind};
use bl_shell::Shell;
use serde_json::{Value, json};

fn client() -> Client<InProcess> {
    let mut client = Client::new(InProcess::new(Agent::headless(true), Shell::headless()));
    client.initialize("test-agent").unwrap();
    client
}

fn run(client: &mut Client<InProcess>, command: &str, args: Value) -> Value {
    client
        .call(
            "run",
            json!({ "commands": [{ "command": command, "args": args }] }),
        )
        .unwrap()
        .ok()
        .unwrap_or_else(|error| panic!("{command} failed: {error}"))["results"][0]["result"]
        .clone()
}

fn midi_track(shell: &Shell) -> TrackId {
    shell
        .editor()
        .document()
        .tracks()
        .find(|track| track.kind() == TrackKind::Midi)
        .map(bl_model::Track::id)
        .expect("the default set has a MIDI track")
}

fn held(client: &mut Client<InProcess>, device: DeviceId, param: u32) -> Option<f64> {
    client
        .transport()
        .shell
        .editor()
        .document()
        .device(device)
        .and_then(|device| device.params.get(&bl_model::ParamId(param)).copied())
}

// Spec: midi-transpose R9, midi-velocity R8, midi-random-pitch R8,
// midi-scale-map R8, midi-chord R14, midi-note-length R9, arpeggiator R13,
// midi-echo R8
#[test]
fn an_agent_reaches_every_parameter_of_every_midi_effect() {
    let library = std::env::temp_dir().join(format!("bl-midi-effects-{}", std::process::id()));
    std::fs::remove_dir_all(&library).ok();
    std::fs::create_dir_all(&library).unwrap();
    let mut client = client();
    client
        .transport()
        .shell
        .set_user_library(Some(library.clone()));
    let track = midi_track(&client.transport().shell);
    for device_type in [
        lift_type(),
        touch_type(),
        dice_type(),
        remap_type(),
        stack_type(),
        hold_type(),
        arpeggiator_type(),
        note_echo_type(),
        cc_control_type(),
        mpe_control_type(),
        midi_monitor_type(),
    ] {
        let name = &device_type.name;
        assert_eq!(device_type.role, bl_model::DeviceRole::MidiEffect, "{name}");
        let added = run(
            &mut client,
            "device.add",
            json!({
                "device": {
                    "kind": device_type.kind,
                    "name": device_type.name,
                    "role": device_type.role,
                },
                "track": track,
            }),
        );
        let device: DeviceId = serde_json::from_value(added["device"].clone()).unwrap();
        let chain = client
            .transport()
            .shell
            .editor()
            .document()
            .track(track)
            .unwrap()
            .devices()
            .map(|d| d.id)
            .collect::<Vec<_>>();
        assert_eq!(chain.last(), Some(&device), "{name}: in the chain");

        // Every parameter: set away from its default, read back.
        for param in &device_type.description.params {
            let to = if param.default == param.max() {
                param.min()
            } else {
                param.max()
            };
            run(
                &mut client,
                "device.set_param",
                json!({ "device": device, "param": param.id, "value": to }),
            );
            assert_eq!(
                held(&mut client, device, param.id.0),
                Some(to),
                "{name}: {}",
                param.name
            );
        }
        let described = run(
            &mut client,
            "device.parameters",
            json!({ "device": device }),
        );
        let params = described["params"].as_array().unwrap();
        assert_eq!(params.len(), device_type.description.params.len(), "{name}");
        assert!(
            params
                .iter()
                .all(|p| p["descriptor"]["automatable"] != json!(false)),
            "{name}"
        );

        // One undo step undoes one edit.
        let last = device_type.description.params.last().unwrap();
        run(&mut client, "edit.undo", json!({}));
        assert_eq!(held(&mut client, device, last.id.0), None, "{name}: undone");
        run(&mut client, "edit.redo", json!({}));
        assert!(held(&mut client, device, last.id.0).is_some());

        // Off and on again.
        run(
            &mut client,
            "device.set_enabled",
            json!({ "device": device, "enabled": false }),
        );
        run(
            &mut client,
            "device.set_enabled",
            json!({ "device": device, "enabled": true }),
        );

        // Its preset is saved under MIDI Effects.
        let saved = run(
            &mut client,
            "device.save_preset",
            json!({ "device": device }),
        );
        let path = saved["path"].as_str().unwrap();
        assert!(path.contains("MIDI Effects"), "{name}: {path}");
        assert!(std::path::Path::new(path).is_file());
    }
    std::fs::remove_dir_all(&library).ok();
}

// Spec: scales R16, midi-scale-map R10, arpeggiator R15. An agent turns Use
// Current Scale on by ID on each of the five scale-aware devices and reads it
// back as the device's title bar switch, as one undo step, and sets Scale
// Name, Root and Scale by ID.
#[test]
fn an_agent_sets_the_use_current_scale_switches_and_the_new_choosers_by_id() {
    use bl_devices::{arpeggiator, dice, lift, remap, stack};

    let mut client = client();
    let track = midi_track(&client.transport().shell);
    for (device_type, switch) in [
        (arpeggiator_type(), arpeggiator::USE_SCALE),
        (stack_type(), stack::USE_SCALE),
        (lift_type(), lift::USE_SCALE),
        (dice_type(), dice::USE_SCALE),
        (remap_type(), remap::USE_SCALE),
    ] {
        let name = &device_type.name;
        let added = run(
            &mut client,
            "device.add",
            json!({
                "device": {
                    "kind": device_type.kind,
                    "name": device_type.name,
                    "role": device_type.role,
                },
                "track": track,
            }),
        );
        let device: DeviceId = serde_json::from_value(added["device"].clone()).unwrap();
        let read = |client: &mut Client<InProcess>| {
            let described = run(client, "device.parameters", json!({ "device": device }));
            described["params"]
                .as_array()
                .unwrap()
                .iter()
                .find(|p| p["id"] == json!(switch.0))
                .cloned()
                .unwrap_or_else(|| panic!("{name}: no switch"))
        };
        let before = read(&mut client);
        assert_eq!(before["value"], json!(0.0), "{name}: off in a new device");
        assert_eq!(before["title_switch"], json!(true), "{name}");
        run(
            &mut client,
            "device.set_param",
            json!({ "device": device, "param": switch.0, "value": 1.0 }),
        );
        assert_eq!(read(&mut client)["value"], json!(1.0), "{name}");
        run(&mut client, "edit.undo", json!({}));
        assert_eq!(
            held(&mut client, device, switch.0),
            None,
            "{name}: one step"
        );
    }
    // The new choosers: Scale's Scale Name, and Arpeggiator's Root and Scale.
    let added = run(
        &mut client,
        "device.add",
        json!({
            "device": {
                "kind": remap_type().kind,
                "name": remap_type().name,
                "role": remap_type().role,
            },
            "track": track,
        }),
    );
    let device: DeviceId = serde_json::from_value(added["device"].clone()).unwrap();
    run(
        &mut client,
        "device.set_param",
        json!({ "device": device, "param": remap::SCALE_NAME.0, "value": 3.0 }),
    );
    let described = run(
        &mut client,
        "device.parameters",
        json!({ "device": device }),
    );
    let chooser = described["params"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == json!(remap::SCALE_NAME.0))
        .unwrap();
    assert_eq!(chooser["text"], json!("Dorian"));
}

// Spec: arpeggiator R18. An agent chooses a groove by ID with
// `device.set_param` and reads it back from `device.parameters` as its name.
#[test]
fn an_agent_chooses_the_arpeggiators_groove_by_id() {
    use bl_devices::arpeggiator;

    let mut client = client();
    let track = midi_track(&client.transport().shell);
    let added = run(
        &mut client,
        "device.add",
        json!({
            "device": {
                "kind": arpeggiator_type().kind,
                "name": arpeggiator_type().name,
                "role": arpeggiator_type().role,
            },
            "track": track,
        }),
    );
    let device: DeviceId = serde_json::from_value(added["device"].clone()).unwrap();
    let read = |client: &mut Client<InProcess>| {
        let described = run(client, "device.parameters", json!({ "device": device }));
        described["params"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["id"] == json!(arpeggiator::GROOVE.0))
            .cloned()
            .expect("the Groove chooser")
    };
    assert_eq!(read(&mut client)["text"], json!("None"));
    run(
        &mut client,
        "device.set_param",
        json!({ "device": device, "param": arpeggiator::GROOVE.0, "value": 7.0 }),
    );
    let groove = read(&mut client);
    assert_eq!(groove["value"], json!(7.0));
    assert_eq!(groove["text"], json!("Swing 16ths 60"));
}

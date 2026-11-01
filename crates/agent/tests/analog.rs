//! Analog's controls through the agent API (WP-8.14, part 1): an agent adds
//! the instrument, sets and reads back every parameter, switches the shell's
//! sections by name, and finds the pages in the device view's state.

use bl_agent::Agent;
use bl_agent::client::{Client, InProcess};
use bl_devices::analog::{ANALOG, PAGES, SECTIONS, analog_type};
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
        .unwrap_or_else(|error| panic!("{command} failed: {error}"))
}

fn audio_track(shell: &Shell) -> TrackId {
    shell
        .editor()
        .document()
        .tracks()
        .find(|track| track.kind() == TrackKind::Audio)
        .map(bl_model::Track::id)
        .expect("the default set has an audio track")
}

// Spec: analog R16, R17 (every control as a command, the sections as pages)
#[test]
fn every_parameter_is_set_and_read_back_and_the_sections_switch_by_name() {
    let mut client = client();
    let track = audio_track(&client.transport().shell);
    let added = run(
        &mut client,
        "device.add",
        json!({
            "device": {
                "kind": { "native": { "device_type": ANALOG } },
                "name": "Analog",
                "role": "instrument",
            },
            "track": track,
        }),
    );
    let device: DeviceId = serde_json::from_value(added["results"][0]["result"]["device"].clone())
        .expect("the new device");
    let description = analog_type().description;
    for param in &description.params {
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
        let held = client
            .transport()
            .shell
            .editor()
            .document()
            .device(device)
            .and_then(|device| device.params.get(&param.id).copied());
        assert_eq!(held, Some(to), "{}", param.name);
    }

    // The instrument describes every control to an agent.
    let described = run(
        &mut client,
        "device.parameters",
        json!({ "device": device }),
    );
    let names: Vec<String> = described["results"][0]["result"]["params"]
        .as_array()
        .expect("the params")
        .iter()
        .filter_map(|entry| entry["name"].as_str().map(str::to_owned))
        .collect();
    for param in &description.params {
        assert!(names.contains(&param.name), "{} is described", param.name);
    }

    // The shell's sections: the first shows in a new device, each can be
    // chosen by name, and the device view reads the choice back.
    let shown = |client: &mut Client<InProcess>| {
        let state = run(client, "device_view.state", json!({}));
        let device = state["results"][0]["result"]["devices"]
            .as_array()
            .and_then(|devices| {
                devices
                    .iter()
                    .find(|d| d["panel"]["pages"][0]["node"] == PAGES)
            })
            .cloned()
            .expect("Analog's pages are in the device view");
        device["panel"]["pages"][0]["shown"].clone()
    };
    assert_eq!(shown(&mut client), SECTIONS[0]);
    for section in SECTIONS {
        run(
            &mut client,
            "device.set_page",
            json!({ "device": device, "node": PAGES, "page": section }),
        );
        assert_eq!(shown(&mut client), section);
    }
}

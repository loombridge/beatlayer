//! Tension through the agent API (WP-8.22; the Tension spec, R18): an agent
//! adds the instrument to a track, sets every parameter and reads each back,
//! and switches the tab it shows.

use bl_agent::Agent;
use bl_agent::client::{Client, InProcess};
use bl_devices::tension::{TENSION, tension_type};
use bl_model::{DeviceId, ParamId, TrackKind};
use bl_shell::Shell;
use serde_json::{Value, json};

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

fn held(shell: &Shell, device: DeviceId, param: ParamId) -> Option<f64> {
    shell
        .editor()
        .document()
        .device(device)
        .and_then(|device| device.params.get(&param).copied())
}

// Spec: tension R18
#[test]
fn an_agent_sets_every_tension_parameter_and_reads_it_back() {
    let mut client = Client::new(InProcess::new(Agent::headless(true), Shell::headless()));
    client.initialize("test-agent").unwrap();
    let track = client
        .transport()
        .shell
        .editor()
        .document()
        .tracks()
        .find(|track| track.kind() == TrackKind::Audio)
        .map(bl_model::Track::id)
        .expect("the default set has an audio track");
    let added = run(
        &mut client,
        "device.add",
        json!({
            "device": {
                "kind": { "native": { "device_type": TENSION } },
                "name": "Tension",
                "role": "instrument",
            },
            "track": track,
        }),
    );
    let device: DeviceId = serde_json::from_value(added["results"][0]["result"]["device"].clone())
        .expect("the new device");
    let params = tension_type().description.params;
    for param in &params {
        // Every parameter's top, or its bottom where the default is the top.
        let value = if param.default == param.max() {
            param.min()
        } else {
            param.max()
        };
        run(
            &mut client,
            "device.set_param",
            json!({ "device": device, "param": param.id, "value": value }),
        );
        assert_eq!(
            held(&client.transport().shell, device, param.id),
            Some(value),
            "{}",
            param.name
        );
    }
    // The instrument describes every control to an agent.
    let described = run(
        &mut client,
        "device.parameters",
        json!({ "device": device }),
    );
    let names: Vec<&str> = described["results"][0]["result"]["params"]
        .as_array()
        .expect("the params")
        .iter()
        .filter_map(|entry| entry["name"].as_str())
        .collect();
    for param in &params {
        assert!(
            names.contains(&param.name.as_str()),
            "{} is described",
            param.name
        );
    }
    // And the tab it shows is a command's too.
    let shown = run(
        &mut client,
        "device.set_page",
        json!({ "device": device, "node": "Tabs", "page": "Filter/Global" }),
    );
    assert_eq!(
        shown["results"][0]["result"]["devices"][0]["panel"]["pages"][0]["shown"],
        "Filter/Global"
    );
}

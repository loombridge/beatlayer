//! Modular through the agent API (WP-9.36; the modular device spec, R9): an
//! agent adds the device, builds a patch with the `modular.` commands and
//! reads it back with `modular.patch` and the query tool.

use bl_agent::Agent;
use bl_agent::client::{Client, InProcess};
use bl_model::modular::INSTRUMENT;
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

// Spec: modular-device R9
#[test]
fn an_agent_builds_a_patch_by_command_and_reads_it_back() {
    let mut client = client();
    let track = audio_track(&client.transport().shell);
    let added = run(
        &mut client,
        "device.add",
        json!({
            "device": {
                "kind": { "native": { "device_type": INSTRUMENT } },
                "name": "Modular Instrument",
                "role": "instrument",
            },
            "track": track,
        }),
    );
    let device: DeviceId = serde_json::from_value(added["results"][0]["result"]["device"].clone())
        .expect("the new device");

    run(&mut client, "modular.add_block", json!({ "device": device, "block_type": "filter", "id": 20, "x": 1.0, "y": 2.0 }));
    run(&mut client, "modular.disconnect", json!({ "device": device, "from": 2, "from_port": "out", "to": 3, "to_port": "in" }));
    run(&mut client, "modular.connect", json!({ "device": device, "from": 2, "from_port": "out", "to": 20, "to_port": "in" }));
    run(&mut client, "modular.connect", json!({ "device": device, "from": 20, "from_port": "out", "to": 3, "to_port": "in" }));
    run(&mut client, "modular.set", json!({ "device": device, "block": 20, "setting": "cutoff", "value": 2000.0 }));
    run(&mut client, "modular.expose", json!({ "device": device, "block": 20, "setting": "cutoff", "exposed": true }));

    let read = run(&mut client, "modular.patch", json!({ "device": device }));
    let state = &read["results"][0]["result"];
    let filter = state["blocks"].as_array().unwrap().iter().find(|b| b["id"] == 20).unwrap();
    assert_eq!(filter["type"], "filter");
    let cutoff = filter["settings"].as_array().unwrap().iter().find(|s| s["id"] == "cutoff").unwrap();
    assert_eq!((cutoff["value"].as_f64(), cutoff["slot"].as_u64()), (Some(2000.0), Some(1)));
    assert_eq!(state["cables"].as_array().unwrap().len(), 7);
    assert_eq!(state["params"][0]["name"], "Filter Cutoff");

    // The query tool's device shows the patch too.
    let queried = client
        .call("query", json!({ "id": device, "detail": "full" }))
        .unwrap()
        .ok()
        .unwrap();
    assert_eq!(queried["patch"]["blocks"].as_array().unwrap().len(), 6);

    // A cable that makes a loop with no Delay is refused; nothing changes.
    let refused = client
        .call(
            "run",
            json!({ "commands": [{ "command": "modular.connect", "args": { "device": device, "from": 20, "from_port": "out", "to": 20, "to_port": "in" } }] }),
        )
        .unwrap();
    assert!(refused.ok().is_err());
    let again = run(&mut client, "modular.patch", json!({ "device": device }));
    assert_eq!(again["results"][0]["result"]["cables"].as_array().unwrap().len(), 7);
}

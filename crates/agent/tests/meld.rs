//! Meld through the agent API (WP-8.17; the meld spec, R2 to R5, R14 and
//! R20): an agent adds the instrument to a track, sets and reads back every
//! parameter by ID, reads the macros under the type's names, and shows an
//! engine's page.

use bl_agent::Agent;
use bl_agent::client::{Client, InProcess};
use bl_devices::meld::{MACRO_1, MELD, TYPE, meld_type, param};
use bl_model::{DeviceId, TrackKind};
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

// Spec: meld R2 to R5, R20
#[test]
fn an_agent_sets_and_reads_back_every_parameter_and_reads_the_macros_by_type() {
    let mut client = client();
    let track = client
        .transport()
        .shell
        .editor()
        .document()
        .tracks()
        .find(|track| track.kind() == TrackKind::Midi)
        .map(bl_model::Track::id)
        .expect("the default set has a MIDI track");
    let added = run(
        &mut client,
        "device.add",
        json!({
            "device": {
                "kind": { "native": { "device_type": MELD } },
                "name": "Meld",
                "role": "instrument",
            },
            "track": track,
        }),
    );
    let device: DeviceId = serde_json::from_value(added["results"][0]["result"]["device"].clone())
        .expect("the new device");
    // Every parameter, moved to the far end of its range and read back.
    let mut wanted = Vec::new();
    for p in &meld_type().description.params {
        let to = if p.default == p.max() {
            p.min()
        } else {
            p.max()
        };
        run(
            &mut client,
            "device.set_param",
            json!({ "device": device, "param": p.id, "value": to }),
        );
        wanted.push((p.id, to));
    }
    let read = run(
        &mut client,
        "device.parameters",
        json!({ "device": device }),
    );
    let params = read["results"][0]["result"]["params"].as_array().unwrap();
    for (id, to) in wanted {
        let state = params
            .iter()
            .find(|p| p["id"] == json!(id))
            .unwrap_or_else(|| panic!("{id:?} is read back"));
        assert_eq!(state["value"], json!(to), "{}", state["name"]);
    }
    // The type picks the macros' names; the IDs stay.
    run(
        &mut client,
        "device.set_param",
        json!({ "device": device, "param": param(0, TYPE), "value": 5.0 }),
    );
    let read = run(
        &mut client,
        "device.parameters",
        json!({ "device": device }),
    );
    let params = read["results"][0]["result"]["params"].as_array().unwrap();
    let macro_1 = params
        .iter()
        .find(|p| p["id"] == json!(param(0, MACRO_1)))
        .unwrap();
    assert_eq!(macro_1["name"], "Tone", "Sub's first macro");
    // The engine selector is a page the agent shows.
    run(
        &mut client,
        "arrangement.select_tracks",
        json!({ "tracks": [track] }),
    );
    let shown = run(
        &mut client,
        "device.set_page",
        json!({ "device": device, "node": "Engine", "page": "B" }),
    );
    assert_eq!(
        shown["results"][0]["result"]["devices"][0]["panel"]["pages"][0]["shown"],
        "B"
    );
}

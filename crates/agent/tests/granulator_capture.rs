//! Granulator III's capture through the agent API (WP-8.27; the Granulator
//! III spec R19, R23): an agent adds the device, reads its `capture` in the
//! query tool's `device` result, and `granulator.capture` is refused with
//! No Input chosen.

use bl_agent::Agent;
use bl_agent::client::{Client, InProcess};
use bl_devices::granulator_iii::GRANULATOR_III;
use bl_shell::Shell;
use serde_json::{Value, json};

fn client() -> Client<InProcess> {
    let mut client = Client::new(InProcess::new(Agent::headless(true), Shell::headless()));
    client.initialize("test-agent").unwrap();
    client
}

fn call(client: &mut Client<InProcess>, tool: &str, args: Value) -> Result<Value, String> {
    client
        .call(tool, args)
        .unwrap()
        .ok()
        .map_err(|e| e.to_string())
}

// Spec: granulator iii R19, R23
#[test]
fn an_agent_reads_a_granulators_capture_and_is_refused_with_no_input() {
    let mut client = client();
    let track = client
        .transport()
        .shell
        .editor()
        .document()
        .tracks()
        .find(|track| track.kind() == bl_model::TrackKind::Midi)
        .map(bl_model::Track::id)
        .expect("the default set has a MIDI track");
    let added = call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "device.add", "args": {
            "device": {
                "kind": { "native": { "device_type": GRANULATOR_III } },
                "name": "Granulator III",
                "role": "instrument",
            },
            "track": track,
        } }] }),
    )
    .unwrap();
    let device = added["results"][0]["result"]["device"].as_u64().unwrap();

    let found = call(
        &mut client,
        "query",
        json!({ "path": format!("device {device}") }),
    )
    .unwrap();
    assert_eq!(
        found["capture"]["source"],
        json!({ "type": "none" }),
        "{found}"
    );
    assert_eq!(found["capture"]["length"], 10.0);
    assert_eq!(found["capture"]["capturing"], false);
    assert_eq!(found["capture"]["frames"], 0);
    assert!(found["capture"]["file"].is_null());

    let refused = call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "granulator.capture",
            "args": { "device": device, "action": "start" } }] }),
    );
    let message = format!("{refused:?}");
    assert!(message.contains("No Input"), "{message}");
}

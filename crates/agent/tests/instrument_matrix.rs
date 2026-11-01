//! An instrument's modulation matrix through the agent API (WP-8.4; the
//! instrument modulation spec, R8 to R11): an agent sets a route by ID,
//! reads the matrix back in the `device` result, and is refused for an
//! unknown name or a full matrix. The device is the framework's test
//! instrument.

use bl_agent::Agent;
use bl_agent::client::{Client, InProcess};
use bl_devices::testing::{self, INSTRUMENT_FREE_AMOUNTS};
use bl_model::{DeviceId, TrackKind};
use bl_shell::Shell;
use serde_json::{Value, json};

fn client() -> Client<InProcess> {
    let mut shell = Shell::headless();
    shell.set_device_types(testing::device_types());
    let mut client = Client::new(InProcess::new(Agent::headless(true), shell));
    client.initialize("test-agent").unwrap();
    client
}

fn try_run(client: &mut Client<InProcess>, command: &str, args: Value) -> Result<Value, String> {
    client
        .call(
            "run",
            json!({ "commands": [{ "command": command, "args": args }] }),
        )
        .unwrap()
        .ok()
        .map_err(|error| error.to_string())
}

fn run(client: &mut Client<InProcess>, command: &str, args: Value) -> Value {
    try_run(client, command, args).unwrap_or_else(|error| panic!("{command} failed: {error}"))
}

fn add(client: &mut Client<InProcess>) -> DeviceId {
    let track = client
        .transport()
        .shell
        .editor()
        .document()
        .tracks()
        .find(|track| track.kind() == TrackKind::Audio)
        .map(bl_model::Track::id)
        .expect("the default set has an audio track");
    let t = testing::instrument_type();
    let added = run(
        client,
        "device.add",
        json!({
            "device": { "kind": t.kind, "name": t.name, "role": "instrument" },
            "track": track,
        }),
    );
    serde_json::from_value(added["results"][0]["result"]["device"].clone()).expect("the device")
}

fn matrix(client: &mut Client<InProcess>, device: DeviceId) -> Value {
    client
        .call("query", json!({ "id": device }))
        .unwrap()
        .ok()
        .unwrap_or_else(|error| panic!("query failed: {error}"))["matrix"]
        .clone()
}

// Spec: instrument modulation R8, R9, R11
#[test]
fn an_agent_sets_a_route_by_name_and_reads_the_matrix_back() {
    let mut client = client();
    let device = add(&mut client);
    let before = matrix(&mut client, device);
    assert_eq!(before["slots"].as_array().unwrap().len(), 6);
    assert_eq!(before["slots"][0]["source"], Value::Null);
    assert_eq!(before["sources"][0]["name"], "Velocity");
    assert_eq!(before["destinations"][2]["name"], "Master");

    let set = run(
        &mut client,
        "device.set_route",
        json!({ "device": device, "source": "Velocity", "destination": "Note Level", "amount": -35.0 }),
    );
    assert_eq!(set["results"][0]["result"]["slot"], 0);
    let after = matrix(&mut client, device);
    let slot = &after["slots"][0];
    assert_eq!(slot["kind"], "free");
    assert_eq!(slot["source"], "Velocity");
    assert_eq!(slot["destination"], "Note Level");
    assert_eq!(slot["amount"], -35.0);
    assert_eq!(slot["amount_param"], json!(INSTRUMENT_FREE_AMOUNTS[0]));

    // The amount is an ordinary parameter: set by command, read in the slot.
    run(
        &mut client,
        "device.set_param",
        json!({ "device": device, "param": INSTRUMENT_FREE_AMOUNTS[0], "value": 60.0 }),
    );
    assert_eq!(matrix(&mut client, device)["slots"][0]["amount"], 60.0);

    run(
        &mut client,
        "device.clear_route",
        json!({ "device": device, "slot": 0 }),
    );
    let cleared = matrix(&mut client, device);
    assert_eq!(cleared["slots"][0]["source"], Value::Null);
    assert_eq!(cleared["slots"][0]["amount"], 0.0);
}

// Spec: instrument modulation R8
#[test]
fn an_agent_is_refused_for_an_unknown_name_and_a_full_matrix() {
    let mut client = client();
    let device = add(&mut client);
    let error = try_run(
        &mut client,
        "device.set_route",
        json!({ "device": device, "source": "Nope", "destination": "Note Level" }),
    )
    .unwrap_err();
    assert!(error.contains("no modulation source"), "{error}");
    for (source, destination) in [
        ("Velocity", "Note Level"),
        ("Velocity", "Fine Tune"),
        ("Key", "Note Level"),
        ("Pressure", "Master"),
    ] {
        run(
            &mut client,
            "device.set_route",
            json!({ "device": device, "source": source, "destination": destination }),
        );
    }
    let error = try_run(
        &mut client,
        "device.set_route",
        json!({ "device": device, "source": "Mod Wheel", "destination": "Master" }),
    )
    .unwrap_err();
    assert!(error.contains("every free slot"), "{error}");
}

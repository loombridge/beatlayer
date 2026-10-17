//! A device's A and B states through the agent API (WP-5.12; the device
//! compare spec, R11): a scripted MCP client switches and copies by command
//! ID and reads both states back with `device.parameters` and the query
//! tool's `compare` field.

use std::process::Command;

use bl_agent::client::{Client, Stdio, Transport};
use serde_json::{Value, json};

fn bl_cli_mcp() -> Client<Stdio> {
    let mut command = Command::new(env!("CARGO_BIN_EXE_bl-cli"));
    command.arg("mcp");
    let mut client = Client::new(Stdio::spawn(&mut command).expect("bl-cli mcp starts"));
    client
        .initialize("scripted-client")
        .expect("it initialises");
    client
}

fn call(client: &mut Client<impl Transport>, tool: &str, args: Value) -> Value {
    client
        .call(tool, args)
        .expect("the call reaches the server")
        .ok()
        .unwrap_or_else(|error| panic!("{tool} failed: {error}"))
}

/// Runs one command and returns its result.
fn run(client: &mut Client<impl Transport>, command: &str, args: Value) -> Value {
    let reply = call(
        client,
        "run",
        json!({ "commands": [{ "command": command, "args": args }] }),
    );
    reply["results"][0]["result"].clone()
}

/// Both states of the device's Width, as the query tool's `compare` field
/// gives them, and which is active.
fn compare(client: &mut Client<impl Transport>) -> (String, Value, Value) {
    let device = call(
        client,
        "query",
        json!({ "path": "track \"Pad\" / device #1" }),
    );
    let compare = &device["compare"];
    (
        compare["active"].as_str().unwrap().to_owned(),
        compare["a"].clone(),
        compare["b"].clone(),
    )
}

// Spec: device-compare R11
#[test]
fn an_agent_switches_copies_and_reads_back_both_states() {
    let mut client = bl_cli_mcp();
    let reply = call(
        &mut client,
        "run",
        json!({ "commands": [
            { "command": "track.create",
              "args": { "id": {"$new": "pad"}, "kind": "audio", "name": "Pad",
                        "parent": null, "index": 0 } },
            { "command": "device.insert",
              "args": { "track": {"$ref": "pad"}, "index": 0,
                        "device": { "id": {"$new": "utility"}, "name": "Utility",
                                    "kind": { "native": { "device_type": "beatlayer.utility" } },
                                    "enabled": true, "params": [] } } },
        ] }),
    );
    assert!(reply["results"].as_array().is_some_and(|r| r.len() == 2));
    let device = call(
        &mut client,
        "query",
        json!({ "path": "track \"Pad\" / device #1" }),
    )["id"]
        .clone();

    // A holds Width 50 %, B the default 100 %.
    run(
        &mut client,
        "device.set_param",
        json!({ "device": device, "param": 6, "value": 50.0 }),
    );
    let (active, a, b) = compare(&mut client);
    assert_eq!(active, "A");
    assert_eq!(a, json!([{ "param": 6, "value": 50.0 }]));
    assert_eq!(b, json!([]), "B keeps the default");

    // A switch makes B active, and `device.parameters` reads the other
    // state's value beside the active one.
    run(
        &mut client,
        "device.compare_switch",
        json!({ "device": device }),
    );
    let (active, a, b) = compare(&mut client);
    assert_eq!(
        (active.as_str(), a, b),
        ("B", json!([{ "param": 6, "value": 50.0 }]), json!([]))
    );
    let parameters = run(
        &mut client,
        "device.parameters",
        json!({ "device": device }),
    );
    assert_eq!(parameters["active"], "B");
    let width = parameters["params"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == 6)
        .expect("Width");
    assert_eq!(
        (&width["value"], &width["other"]),
        (&json!(100.0), &json!(50.0))
    );

    // A copy takes the active state, B's, over A's: both read the default.
    run(
        &mut client,
        "device.compare_copy",
        json!({ "device": device }),
    );
    let (active, a, b) = compare(&mut client);
    assert_eq!((active.as_str(), a, b), ("B", json!([]), json!([])));
    let history = call(&mut client, "query", json!({ "path": "history" }));
    // An agent's steps carry its name and the command's label.
    assert_eq!(
        history["undo"]["items"][0],
        "scripted-client: Compare: Copy"
    );

    // Undo brings back both states, and the switch before it.
    run(&mut client, "edit.undo", json!({}));
    let (active, a, _) = compare(&mut client);
    assert_eq!(
        (active.as_str(), a),
        ("B", json!([{ "param": 6, "value": 50.0 }]))
    );
    assert!(client.into_transport().finish().unwrap().success());
}

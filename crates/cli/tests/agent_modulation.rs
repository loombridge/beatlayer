//! Relative modulation through the agent API (WP-5.9; the modulation spec,
//! R11): a scripted MCP client gives a clip a volume modulation envelope by
//! command ID and reads the parameter's base, its played value and the
//! envelope modulating it back with `modulation.state`.

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

// Spec: modulation R11
#[test]
fn an_agent_reads_a_parameters_base_played_value_and_sources() {
    let mut client = bl_cli_mcp();
    let volume = json!({ "type": "mixer", "param": "volume" });
    let made = call(
        &mut client,
        "run",
        json!({ "commands": [
            { "command": "track.create",
              "args": { "id": {"$new": "Lead"}, "kind": "midi", "name": "Lead", "index": 0 } },
            { "command": "clip.create",
              "args": { "track": {"$ref": "Lead"},
                        "clip": { "id": {"$new": "Clip"}, "name": "", "start": 0,
                                  "length": 7680, "mute": false,
                                  "data": { "midi": { "notes": [] } } } } },
            { "command": "mixer.set_volume",
              "args": { "track": {"$ref": "Lead"}, "volume": 0.5 } },
            { "command": "clip.set_envelope_breakpoint",
              "args": { "clip": {"$ref": "Clip"}, "target": volume, "at": 0, "value": 0.5 } },
        ] }),
    );
    assert_eq!(made["results"].as_array().unwrap().len(), 4);
    let state = &call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "modulation.state", "args": {} }] }),
    )["results"][0]["result"];
    let parameters = state["parameters"].as_array().unwrap();
    assert_eq!(parameters.len(), 1, "{state}");
    let parameter = &parameters[0];
    assert_eq!(parameter["target"], volume);
    // The song is stopped: the base is the fader's, nothing modulates it
    // yet, and the envelope is listed with the share it holds at the
    // play position.
    assert_eq!(parameter["base"], 0.5);
    assert_eq!(parameter["played"], 0.5);
    let source = &parameter["sources"][0];
    assert_eq!(source["share"], 0.5);
    assert!(source["contribution"].as_f64().unwrap() < 0.0, "a volume only goes down");

    // The same query for a track that has none reports nothing.
    call(
        &mut client,
        "run",
        json!({ "commands": [
            { "command": "track.create",
              "args": { "id": {"$new": "Pad"}, "kind": "midi", "name": "Pad", "index": 1 } },
        ] }),
    );
    let tracks = call(&mut client, "query", json!({ "path": "tracks" }));
    let pad = tracks["tracks"]["items"][1]["id"].clone();
    let other = call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "modulation.state", "args": { "track": pad } }] }),
    );
    assert_eq!(other["results"][0]["result"]["parameters"], json!([]));
    assert!(client.into_transport().finish().unwrap().success());
}

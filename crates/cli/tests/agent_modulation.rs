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
    assert!(
        source["contribution"].as_f64().unwrap() < 0.0,
        "a volume only goes down"
    );

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

// Spec: modulation R18
#[test]
fn an_agent_maps_a_modulator_and_reads_its_mappings_back() {
    let mut client = bl_cli_mcp();
    let device = |track: &str, name: &str, device_type: &str| {
        json!({ "command": "device.insert",
                "args": { "track": {"$ref": track}, "index": 0,
                          "device": { "id": {"$new": name}, "name": name,
                                      "kind": { "native": { "device_type": device_type } },
                                      "enabled": true, "params": [] } } })
    };
    let made = call(
        &mut client,
        "run",
        json!({ "commands": [
            { "command": "track.create",
              "args": { "id": {"$new": "Mod"}, "kind": "audio", "name": "Mod", "index": 0 } },
            { "command": "track.create",
              "args": { "id": {"$new": "Voice"}, "kind": "audio", "name": "Voice", "index": 1 } },
            device("Mod", "Wave", "beatlayer.wave_mod"),
            device("Voice", "Utility", "beatlayer.utility"),
            { "command": "modulator.map",
              "args": { "device": {"$ref": "Wave"}, "slot": 0,
                        "target": { "type": "device", "device": {"$ref": "Utility"}, "param": 9 } } },
            { "command": "modulator.map",
              "args": { "device": {"$ref": "Wave"}, "slot": 1,
                        "target": { "type": "mixer", "track": {"$ref": "Voice"}, "param": "volume" } } },
        ] }),
    );
    let results = made["results"].as_array().unwrap();
    assert_eq!(results.len(), 6, "{made}");
    let wave = call(&mut client, "query", json!({ "path": "device \"Wave\"" }));
    let mappings = wave["mappings"].as_array().unwrap();
    assert_eq!(mappings.len(), 2, "{wave}");
    assert_eq!(mappings[0]["slot"], 0);
    assert_eq!(mappings[0]["target"]["type"], "device");
    assert_eq!(mappings[0]["target"]["param"], 9);
    assert_eq!(mappings[1]["target"]["type"], "mixer");
    // The state query names the slot among a parameter's modulators.
    let state = &call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "modulation.state", "args": {} }] }),
    )["results"][0]["result"];
    let parameters = state["parameters"].as_array().unwrap();
    assert_eq!(parameters.len(), 2, "{state}");
    assert!(
        parameters
            .iter()
            .all(|p| p["modulators"][0]["remote_control"] == false),
        "{state}"
    );
    // Unmap takes one away, and the Mod switch holds the other.
    call(
        &mut client,
        "run",
        json!({ "commands": [
            { "command": "modulator.unmap", "args": { "device": {"$ref": "Wave"}, "slot": 1 } },
            { "command": "device.set_param",
              "args": { "device": {"$ref": "Wave"}, "param": 64, "value": 1.0 } },
        ] }),
    );
    let wave = call(&mut client, "query", json!({ "path": "device \"Wave\"" }));
    assert_eq!(wave["mappings"].as_array().unwrap().len(), 1);
    let state = &call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "modulation.state", "args": {} }] }),
    )["results"][0]["result"];
    let held = &state["parameters"][0];
    assert_eq!(held["modulators"][0]["remote_control"], true, "{state}");
    assert!(held["held_by"].is_number(), "{state}");
    assert!(client.into_transport().finish().unwrap().success());
}

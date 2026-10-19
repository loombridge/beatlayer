//! Macros through the agent API (WP-5.4; the macros spec, R5 to R19): a
//! scripted MCP client adds a rack and a Utility, maps a macro, moves and
//! randomises it with a seed, and reads the result back through
//! `rack.macros` and the query tool.

use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use bl_agent::client::{Client, Stdio, Transport};
use serde_json::{Value, json};

fn bl_cli_mcp(args: &[&str]) -> Client<Stdio> {
    let mut command = Command::new(env!("CARGO_BIN_EXE_bl-cli"));
    command.arg("mcp").args(args);
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

fn run(client: &mut Client<impl Transport>, command: &str, args: Value) -> Value {
    let outcome = call(
        client,
        "run",
        json!({ "commands": [{ "command": command, "args": args }] }),
    );
    outcome["results"][0]["result"].clone()
}

// Spec: macros R5, R7, R15, R19.
#[test]
fn an_agent_maps_moves_randomises_and_reads_back_macros() {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64);
    let mut client = bl_cli_mcp(&["--autonomous"]);

    let tracks = call(&mut client, "query", json!({ "path": "tracks" }));
    let audio = tracks["tracks"]
        .as_array()
        .expect("the set's tracks")
        .iter()
        .find(|track| track["kind"] == "audio")
        .expect("an audio track")["id"]
        .clone();

    let rack = run(
        &mut client,
        "device.add",
        json!({ "track": audio, "device": {
            "kind": { "rack": { "kind": "audio_effect" } },
            "name": "Rack",
            "role": "audio_effect",
        } }),
    );
    let rack = rack["device"].clone();
    let chain = 9_000_000 + nanos % 1_000_000;
    run(
        &mut client,
        "chain.add",
        json!({ "rack": rack, "index": 0, "chain": {
            "id": chain, "name": "Range", "devices": [],
        } }),
    );
    let utility = run(
        &mut client,
        "device.add",
        json!({ "chain": chain, "device": {
            "kind": { "native": { "device_type": "beatlayer.utility" } },
            "name": "Utility",
            "role": "audio_effect",
        } }),
    );
    let utility = utility["device"].clone();

    run(
        &mut client,
        "rack.map_macro",
        json!({ "rack": rack, "macro_number": 1, "target": {
            "type": "device", "device": utility, "param": 6,
        } }),
    );
    run(
        &mut client,
        "device.set_param",
        json!({ "device": rack, "param": 100, "value": 0.25 }),
    );
    let first = run(
        &mut client,
        "rack.randomize_macros",
        json!({ "rack": rack, "seed": 11 }),
    );
    assert!(first.is_null() || first.is_object(), "{first}");
    let read = run(&mut client, "rack.macros", json!({ "rack": rack }));
    let macro_one = &read["macros"][0];
    assert_eq!(macro_one["name"], "Width");
    assert_eq!(macro_one["mappings"][0]["param"], "Width");
    let value = macro_one["value"].as_f64().expect("a value");
    assert!((0.0..=1.0).contains(&value), "{value}");

    // The query tool gives the same in the rack device's `macros` field.
    let queried = call(&mut client, "query", json!({ "id": rack }));
    assert_eq!(
        queried["content"]["macros"]["macros"][0]["name"],
        json!("Width")
    );
    assert_eq!(
        queried["content"]["macros"]["macros"][0]["mappings"][0]["param"],
        json!("Width")
    );
}

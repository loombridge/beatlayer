//! Wavetables through the agent API (WP-8.8; the wavetable spec, R8): a
//! scripted MCP client drives `bl-cli mcp`, loads a factory table and a
//! file into a device's oscillators by command ID, and reads each back
//! from the query tool's `device` result (D17).

use std::path::Path;
use std::process::Command;

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

/// Runs one command and returns its whole outcome.
fn outcome(client: &mut Client<impl Transport>, command: &str, args: Value) -> Value {
    let done = call(
        client,
        "run",
        json!({ "commands": [{ "command": command, "args": args }] }),
    );
    done["results"][0].clone()
}

// Spec: wavetable R8
#[test]
fn an_agent_loads_wavetables_by_command_id_and_reads_them_back() {
    let file = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/wavetables/clm-4-frames.wav");
    let mut client = bl_cli_mcp(&["--autonomous"]);

    let tracks = call(&mut client, "query", json!({ "path": "tracks" }));
    let midi = tracks["tracks"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["kind"] == "midi")
        .expect("a MIDI track")["id"]
        .clone();
    outcome(
        &mut client,
        "arrangement.select_tracks",
        json!({ "tracks": [midi] }),
    );
    let added = outcome(
        &mut client,
        "device.add",
        json!({ "device": {
            "kind": { "native": { "device_type": "test.framework.wavetable" } },
            "name": "Wavetable bench",
            "role": "instrument",
        } }),
    )["result"]
        .clone();
    let device = added["device"].clone();
    let wavetables = |client: &mut Client<Stdio>| {
        call(client, "query", json!({ "id": device }))
            .get("wavetables")
            .cloned()
            .unwrap_or(Value::Null)
    };
    assert_eq!(wavetables(&mut client), Value::Null, "none yet");

    let factory = outcome(
        &mut client,
        "device.load_wavetable",
        json!({ "device": device, "oscillator": 1, "category": "Voices", "name": "Vowel Walk" }),
    );
    assert_eq!(factory["result"]["frames"], 10, "{factory}");
    let user = outcome(
        &mut client,
        "device.load_wavetable",
        json!({ "device": device, "oscillator": 2, "file": file.to_string_lossy(), "raw": true }),
    );
    assert_eq!(user["result"]["frames"], 4, "{user}");
    let read = wavetables(&mut client);
    assert_eq!(
        read[0],
        json!({ "oscillator": 1, "category": "Voices", "name": "Vowel Walk", "frames": 10 })
    );
    assert_eq!(read[1]["oscillator"], 2);
    assert_eq!(read[1]["raw"], true);
    assert_eq!(read[1]["frames"], 4);
    assert_eq!(read[1]["file"], file.to_string_lossy().as_ref());

    // The same query reads the table `device.set_wavetable` sets by hash.
    let media = call(
        &mut client,
        "query",
        json!({ "id": device, "detail": "full" }),
    )["saved"]["media"]["items"][1]
        .clone();
    outcome(
        &mut client,
        "device.set_wavetable",
        json!({ "device": device, "oscillator": 1,
                "choice": { "kind": "user", "raw": false, "frames": 4 }, "media": media }),
    );
    assert_eq!(wavetables(&mut client)[0]["raw"], false);

    // One undo step each: the last load goes, the first stays.
    outcome(&mut client, "edit.undo", json!({}));
    assert_eq!(wavetables(&mut client)[0]["category"], "Voices");
    outcome(&mut client, "edit.undo", json!({}));
    let after = wavetables(&mut client);
    assert_eq!(after.as_array().map(Vec::len), Some(1), "{after}");

    // A table that doesn't exist is refused and changes nothing.
    let refused = client
        .call(
            "run",
            json!({ "commands": [{ "command": "device.load_wavetable", "args": {
                "device": device, "oscillator": 1, "category": "Voices", "name": "No Such Table"
            } }] }),
        )
        .expect("the call reaches the server")
        .ok();
    assert!(refused.is_err(), "{refused:?}");
    assert_eq!(wavetables(&mut client), after);
}

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
    // The media store keeps an absolute path, which on Windows has no `..`.
    let file = std::path::absolute(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/wavetables/clm-4-frames.wav"),
    )
    .unwrap();
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

// Spec: wavetable R8, R11, R13
#[test]
fn an_agent_loads_a_table_into_wavetable_and_sets_position() {
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
    let device = outcome(
        &mut client,
        "device.add",
        json!({ "device": {
            "kind": { "native": { "device_type": "beatlayer.wavetable" } },
            "name": "Wavetable",
            "role": "instrument",
        } }),
    )["result"]["device"]
        .clone();
    outcome(
        &mut client,
        "device.load_wavetable",
        json!({ "device": device, "oscillator": 1, "category": "Shape Morphs", "name": "Sine Folds" }),
    );
    // Osc 1 Position is parameter 1.
    outcome(
        &mut client,
        "device.set_param",
        json!({ "device": device, "param": 1, "value": 40.0 }),
    );
    let parameters = outcome(
        &mut client,
        "device.parameters",
        json!({ "device": device }),
    );
    let position = parameters["result"]["params"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "Osc 1 Position")
        .expect("Wavetable has Osc 1 Position")["value"]
        .clone();
    assert_eq!(position, json!(40.0));
    let read = call(&mut client, "query", json!({ "id": device }));
    assert_eq!(read["wavetables"][0]["name"], "Sine Folds", "{read}");
    assert_eq!(read["wavetables"][0]["frames"], 48, "{read}");
}

// Spec: wavetable R25, R32, R34
#[test]
fn an_agent_routes_wavetables_matrix_and_reads_the_routes_and_settings_back() {
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
    let device = outcome(
        &mut client,
        "device.add",
        json!({ "device": {
            "kind": { "native": { "device_type": "beatlayer.wavetable" } },
            "name": "Wavetable",
            "role": "instrument",
        } }),
    )["result"]["device"]
        .clone();
    let matrix = |client: &mut Client<Stdio>| {
        call(client, "query", json!({ "id": device }))["matrix"].clone()
    };

    let before = matrix(&mut client);
    assert_eq!(
        before["sources"].as_array().map(Vec::len),
        Some(13),
        "{before}"
    );
    assert_eq!(before["destinations"].as_array().map(Vec::len), Some(69));
    assert_eq!(before["slots"].as_array().map(Vec::len), Some(64));

    // Routes from the three kinds of source, into the first free slots.
    for (source, destination, amount) in [
        ("LFO 1", "Osc 1 Position", 40.0),
        ("Env 2", "Filter 1 Freq", -25.0),
        ("Pressure", "Osc 2 Gain", 100.0),
    ] {
        outcome(
            &mut client,
            "device.set_route",
            json!({ "device": device, "source": source, "destination": destination,
                    "amount": amount }),
        );
    }
    let after = matrix(&mut client);
    for (slot, source, destination, amount) in [
        (0, "LFO 1", "Osc 1 Position", 40.0),
        (1, "Env 2", "Filter 1 Freq", -25.0),
        (2, "Pressure", "Osc 2 Gain", 100.0),
    ] {
        let read = &after["slots"][slot];
        assert_eq!(read["kind"], "free");
        assert_eq!(read["source"], source);
        assert_eq!(read["destination"], destination);
        assert_eq!(read["amount"], amount);
    }

    // The settings of part 2 are parameters, read back by name.
    let settings = [
        ("LFO 1 Wave", 3.0),
        ("LFO 1 Retrigger", 1.0),
        ("Env 2 Loop Mode", 2.0),
        ("Unison Mode", 6.0),
        ("Hi-Quality", 1.0),
        ("Matrix Time", -50.0),
    ];
    let parameters = outcome(
        &mut client,
        "device.parameters",
        json!({ "device": device }),
    );
    let params = parameters["result"]["params"].as_array().unwrap().clone();
    for (name, value) in settings {
        let id = params
            .iter()
            .find(|p| p["name"] == name)
            .unwrap_or_else(|| panic!("Wavetable has {name}"))["id"]
            .clone();
        outcome(
            &mut client,
            "device.set_param",
            json!({ "device": device, "param": id, "value": value }),
        );
    }
    let parameters = outcome(
        &mut client,
        "device.parameters",
        json!({ "device": device }),
    );
    for (name, value) in settings {
        let read = parameters["result"]["params"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["name"] == name)
            .expect("read back")["value"]
            .clone();
        assert_eq!(read, json!(value), "{name}");
    }
}

// Spec: wavetable R35, R38, R39
#[test]
fn an_agent_reads_wavetables_displays_and_adds_and_removes_a_destination() {
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
    let device = outcome(
        &mut client,
        "device.add",
        json!({ "device": {
            "kind": { "native": { "device_type": "beatlayer.wavetable" } },
            "name": "Wavetable",
            "role": "instrument",
        } }),
    )["result"]["device"]
        .clone();
    outcome(
        &mut client,
        "device.load_wavetable",
        json!({ "device": device, "oscillator": 1, "category": "Shape Morphs", "name": "Sine Folds" }),
    );
    outcome(
        &mut client,
        "device.set_param",
        json!({ "device": device, "param": 1, "value": 50.0 }),
    );
    let shown =
        outcome(&mut client, "device.display", json!({ "device": device }))["result"].clone();
    let series = shown["series"].as_array().unwrap();
    let names = |prefix: &str| {
        series
            .iter()
            .filter(|s| s["name"].as_str().unwrap().starts_with(prefix))
            .count()
    };
    assert_eq!(names("Osc 1 Table "), 32, "32 of the table's 48 frames");
    assert_eq!(
        names("Osc 2 Table "),
        32,
        "the default table's 64 frames, decimated"
    );
    let frame = series.iter().find(|s| s["name"] == "Osc 1 Frame").unwrap();
    assert_eq!(frame["values"].as_array().unwrap().len(), 128);
    let readings = shown["readings"].as_array().unwrap();
    let position = readings
        .iter()
        .find(|r| r["name"] == "Osc 1 Position")
        .unwrap();
    assert_eq!(position["text"], "50.0 %, frame 25 of 48");
    assert!(series.iter().any(|s| s["name"] == "Filter"));
    assert!(
        shown["markers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["name"] == "Amp Envelope Attack")
    );
    assert_eq!(shown["view"], "Perspective");

    // A destination is shown, routed into and removed, each by command.
    outcome(
        &mut client,
        "device.add_destination",
        json!({ "device": device, "destination": "Osc 1 Position" }),
    );
    outcome(
        &mut client,
        "device.set_route",
        json!({ "device": device, "source": "LFO 1", "destination": "Osc 1 Position", "amount": 30.0 }),
    );
    let removed = outcome(
        &mut client,
        "device.remove_destination",
        json!({ "device": device, "destination": "Osc 1 Position" }),
    );
    assert_eq!(removed["result"]["cleared"], json!([0]), "{removed}");
    let read = call(&mut client, "query", json!({ "id": device }));
    let routed = read["matrix"]["slots"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|slot| slot["source"].is_string())
        .count();
    assert_eq!(routed, 0, "{read}");
    let refused = client
        .call(
            "run",
            json!({ "commands": [{ "command": "device.add_destination",
                "args": { "device": device, "destination": "No Such Thing" } }] }),
        )
        .expect("the call reaches the server")
        .ok();
    assert!(refused.is_err(), "an unknown destination is refused");
}

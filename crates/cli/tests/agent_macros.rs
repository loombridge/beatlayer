//! Macros through the agent API (WP-5.4; the macros spec, R5 to R19): a
//! scripted MCP client adds a rack and a Utility, maps a macro, moves and
//! randomises it with a seed, and reads the result back through
//! `rack.macros` and the query tool.

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

fn run(client: &mut Client<impl Transport>, command: &str, args: Value) -> Value {
    let outcome = call(
        client,
        "run",
        json!({ "commands": [{ "command": command, "args": args }] }),
    );
    outcome["results"][0]["result"].clone()
}

/// Builds an audio effect rack holding one chain with a Utility, in one batch
/// so every new ID comes from the project and reads back in `ids` (D17), and
/// returns the rack and the Utility.
fn build_rack(client: &mut Client<impl Transport>) -> (Value, Value) {
    let tracks = call(client, "query", json!({ "path": "tracks" }));
    let audio = tracks["tracks"]["items"]
        .as_array()
        .expect("the set's tracks")
        .iter()
        .find(|track| track["kind"] == "audio")
        .expect("an audio track")["id"]
        .clone();
    let built = call(
        client,
        "run",
        json!({ "label": "Build a rack", "commands": [
            { "command": "device.insert", "args": { "track": audio, "index": 0, "device": {
                "id": {"$new": "rack"}, "name": "Rack", "enabled": true, "params": [],
                "role": "audio_effect",
                "kind": { "rack": { "kind": "audio_effect" } },
                "chains": [
                    { "id": {"$new": "range"}, "name": "Range", "devices": [
                        { "id": {"$new": "utility"}, "name": "Utility", "enabled": true,
                          "params": [], "role": "audio_effect",
                          "kind": { "native": { "device_type": "beatlayer.utility" } } },
                    ] },
                ],
            } } },
        ] }),
    );
    let ids = &built["ids"];
    let id = |name: &str| {
        ids[name]
            .as_u64()
            .unwrap_or_else(|| panic!("no `{name}`: {ids}"))
    };
    (json!(id("rack")), json!(id("utility")))
}

// Spec: macros R5, R7, R15, R19.
#[test]
fn an_agent_maps_moves_randomises_and_reads_back_macros() {
    let mut client = bl_cli_mcp(&["--autonomous"]);
    let (rack, utility) = build_rack(&mut client);

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
    // Spec: macros R7, R13. Moving a macro through the agent API moves the
    // parameter it maps, so the document keeps up.
    let width = |client: &mut Client<_>| {
        call(client, "query", json!({ "id": utility }))["params"]["items"]
            .as_array()
            .and_then(|items| items.iter().find(|item| item["param"] == 6).cloned())
    };
    let mapped = width(&mut client).expect("the mapped parameter takes the macro's value");
    assert!(mapped["value"].is_number(), "{mapped}");

    // Spec: macros R11. A mapped parameter is held by its macro, so setting
    // it directly is refused, naming the macro.
    let refused = client
        .call(
            "run",
            json!({ "commands": [{ "command": "device.set_param",
                "args": { "device": utility, "param": 6, "value": 0.5 } }] }),
        )
        .expect("the call reaches the server");
    assert!(refused.is_error, "{}", refused.text);
    assert!(refused.text.contains("Width"), "{}", refused.text);

    // Spec: macros R15. Randomising with a seed through the agent API moves
    // the macro, and the same seed gives the same value.
    let seeded = |client: &mut Client<_>| {
        run(
            client,
            "rack.randomize_macros",
            json!({ "rack": rack, "seed": 11 }),
        );
        run(client, "rack.macros", json!({ "rack": rack }))["macros"][0]["value"].clone()
    };
    let once = seeded(&mut client);
    let twice = seeded(&mut client);
    assert_eq!(once, twice, "the same seed gives the same value");
    let read = run(&mut client, "rack.macros", json!({ "rack": rack }));
    let macro_one = &read["macros"][0];
    assert_eq!(macro_one["name"], "Width");
    assert_eq!(macro_one["mappings"][0]["param"], "Width");
    let value = macro_one["value"].as_f64().expect("a value");
    assert!((0.0..=1.0).contains(&value), "{value}");

    // The query tool gives the same in the rack device's `macros` field.
    let queried = call(&mut client, "query", json!({ "id": rack }));
    assert_eq!(queried["macros"]["macros"][0]["name"], json!("Width"));
    assert_eq!(
        queried["macros"]["macros"][0]["mappings"][0]["param"],
        json!("Width")
    );
}

// Spec: macros R26, R27, R32.
#[test]
fn an_agent_stores_launches_and_reads_back_variations() {
    let mut client = bl_cli_mcp(&["--autonomous"]);
    let (rack, utility) = build_rack(&mut client);
    run(
        &mut client,
        "rack.map_macro",
        json!({ "rack": rack, "macro_number": 1, "target": {
            "type": "device", "device": utility, "param": 6,
        } }),
    );

    // The macro sits at the mapped parameter's value; New stores it.
    run(&mut client, "rack.store_variation", json!({ "rack": rack }));
    run(
        &mut client,
        "device.set_param",
        json!({ "device": rack, "param": 100, "value": 0.9 }),
    );
    let moved = run(&mut client, "rack.macros", json!({ "rack": rack }));
    assert_eq!(moved["macros"][0]["value"], 0.9);

    // Launch brings the stored value back, and reads back as the variation.
    run(
        &mut client,
        "rack.launch_variation",
        json!({ "rack": rack, "index": 0 }),
    );
    let read = run(&mut client, "rack.macros", json!({ "rack": rack }));
    assert_eq!(read["variations"][0]["name"], "Variation 1");
    let stored = read["variations"][0]["values"][0]
        .as_f64()
        .expect("a value");
    assert!((stored - 0.5).abs() < 1e-9, "{stored}");
    assert_eq!(read["macros"][0]["value"], stored);

    // Excluding a macro leaves it out of the next launch.
    run(
        &mut client,
        "rack.set_macro_variation_excluded",
        json!({ "rack": rack, "macro_number": 1, "excluded": true }),
    );
    run(
        &mut client,
        "device.set_param",
        json!({ "device": rack, "param": 100, "value": 0.9 }),
    );
    run(
        &mut client,
        "rack.launch_variation",
        json!({ "rack": rack, "index": 0 }),
    );
    let excluded = run(&mut client, "rack.macros", json!({ "rack": rack }));
    assert_eq!(excluded["macros"][0]["value"], 0.9);
    assert_eq!(excluded["macros"][0]["variation_excluded"], true);

    // The query tool reports the variations in the rack's `macros` field.
    let queried = call(&mut client, "query", json!({ "id": rack }));
    assert_eq!(
        queried["macros"]["variations"][0]["name"],
        json!("Variation 1")
    );
}

//! MIDI and key mappings through the agent API (WP-7.1; the remote mapping
//! spec, R14): an agent maps, changes and deletes mappings by command ID,
//! reads them back through `remote.mappings`, and moves a mapped fader with
//! `remote.input`, with no controller.

use bl_agent::Agent;
use bl_agent::client::{Client, InProcess, Transport};
use bl_shell::Shell;
use serde_json::{Value, json};

fn run(client: &mut Client<impl Transport>, command: &str, args: Value) -> Value {
    let reply = client
        .call(
            "run",
            json!({ "commands": [{ "command": command, "args": args }] }),
        )
        .expect("the call reaches the server")
        .ok()
        .unwrap_or_else(|error| panic!("{command} failed: {error}"));
    reply["results"][0]["result"].clone()
}

/// Spec: remote-mapping R14.
#[test]
fn an_agent_maps_reads_back_and_tries_a_mapping_by_id() {
    let mut client = Client::new(InProcess::new(Agent::headless(true), Shell::headless()));
    run(
        &mut client,
        "track.create",
        json!({ "id": { "$new": "Keys" }, "kind": "audio", "name": "Keys", "index": 0 }),
    );
    let track = {
        let document = client.transport().shell.editor().document();
        document.tracks().find(|t| t.name() == "Keys").unwrap().id()
    };
    let volume = |client: &mut Client<InProcess>| {
        client
            .transport()
            .shell
            .editor()
            .document()
            .track(track)
            .unwrap()
            .mixer()
            .volume
    };
    let source = json!({ "type": "cc", "channel": 1, "controller": 20 });
    let listed = |client: &mut Client<InProcess>| -> Value {
        run(client, "remote.mappings", json!({}))["mappings"][0].clone()
    };

    run(
        &mut client,
        "remote.map",
        json!({ "source": source,
                "target": { "type": "mixer", "track": track, "param": "volume" } }),
    );
    let mapping = listed(&mut client);
    assert_eq!(mapping["source"], source);
    assert_eq!(mapping["kind"], json!({ "type": "continuous" }));
    assert_eq!(
        (mapping["path"].as_str(), mapping["name"].as_str()),
        (Some("Keys"), Some("Volume"))
    );
    assert_eq!(mapping["mode"], json!({ "type": "absolute" }));

    run(
        &mut client,
        "remote.set_range",
        json!({ "source": source, "min": 0.0, "max": 0.5 }),
    );
    assert_eq!(listed(&mut client)["max"], 0.5);
    run(
        &mut client,
        "remote.invert_range",
        json!({ "source": source }),
    );
    let mapping = listed(&mut client);
    assert_eq!(
        (mapping["min"].as_f64(), mapping["max"].as_f64()),
        (Some(0.5), Some(0.0))
    );
    run(
        &mut client,
        "remote.invert_range",
        json!({ "source": source }),
    );
    run(
        &mut client,
        "remote.set_mode",
        json!({ "source": source,
                "mode": { "type": "relative", "encoding": "twos_complement", "accelerated": true } }),
    );
    assert_eq!(listed(&mut client)["mode"]["encoding"], "twos_complement");
    run(
        &mut client,
        "remote.set_mode",
        json!({ "source": source, "mode": { "type": "absolute" } }),
    );

    // The agent moves the mapped fader without a controller.
    let before = volume(&mut client);
    let result = run(
        &mut client,
        "remote.input",
        json!({ "bytes": [176, 20, 127] }),
    );
    assert_eq!(result["taken"], true);
    let after = volume(&mut client);
    assert!(
        (after - 0.5).abs() < 1e-9 && after != before,
        "{after} after {before}"
    );
    let result = run(
        &mut client,
        "remote.input",
        json!({ "bytes": [176, 21, 127] }),
    );
    assert_eq!(result["taken"], false, "nothing maps controller 21");

    let removed = run(&mut client, "remote.unmap", json!({ "source": source }));
    assert_eq!(removed["removed"], 1);
    let left = run(&mut client, "remote.mappings", json!({}));
    assert_eq!(left["mappings"], json!([]));
}

/// Spec: map-modes R2, R4, R10. An agent turns a map mode on, selects a
/// control by ID, maps a key to it, and reads the mode, the selection and the
/// Mapping Browser's rows back through `remote.state`; `remote.input` takes a
/// message for no mapping while a map mode is on.
#[test]
fn an_agent_turns_a_map_mode_on_selects_a_control_and_reads_the_state() {
    let mut client = Client::new(InProcess::new(Agent::headless(true), Shell::headless()));
    run(
        &mut client,
        "track.create",
        json!({ "id": { "$new": "Keys" }, "kind": "audio", "name": "Keys", "index": 0 }),
    );
    let track = {
        let document = client.transport().shell.editor().document();
        document.tracks().find(|t| t.name() == "Keys").unwrap().id()
    };
    let solo = json!({ "type": "solo", "track": track });
    let fader = json!({ "type": "mixer", "track": track, "param": "volume" });

    let state = run(&mut client, "remote.state", json!({}));
    assert_eq!(state["mode"], "off");
    assert!(state["rows"].as_array().unwrap().is_empty());

    let state = run(&mut client, "remote.map_mode", json!({ "mode": "key" }));
    assert_eq!(state["mode"], "key");
    let state = run(&mut client, "remote.select", json!({ "target": solo }));
    assert_eq!(state["selected"], solo);

    // The next key is mapped by `remote.map`, and the row reads back.
    run(
        &mut client,
        "remote.map",
        json!({ "source": { "type": "key", "key": "S" }, "target": solo }),
    );
    let state = run(&mut client, "remote.state", json!({}));
    assert_eq!(
        state["rows"][0]["source"],
        json!({ "type": "key", "key": "S" })
    );
    assert_eq!(state["rows"][0]["name"], "Solo");
    assert_eq!(state["rows"][0]["range"], json!([0.0, 1.0]));

    // No mapping acts while the mode is on.
    let result = run(&mut client, "remote.input", json!({ "key": "S" }));
    assert_eq!(result["taken"], false);

    // MIDI Map Mode lists the MIDI mappings, and takes a continuous control.
    let state = run(&mut client, "remote.map_mode", json!({ "mode": "midi" }));
    assert_eq!(state["mode"], "midi");
    assert!(state["selected"].is_null(), "a new mode starts with none");
    assert!(state["rows"].as_array().unwrap().is_empty());
    let state = run(&mut client, "remote.select", json!({ "target": fader }));
    assert_eq!(state["selected"], fader);
    let state = run(&mut client, "remote.select", json!({}));
    assert!(state["selected"].is_null());

    let state = run(
        &mut client,
        "remote.map_mode",
        json!({ "mode": "midi", "toggle": true }),
    );
    assert_eq!(state["mode"], "off");
    let result = run(&mut client, "remote.input", json!({ "key": "S" }));
    assert_eq!(result["taken"], true, "leaving the mode lets mappings act");
}

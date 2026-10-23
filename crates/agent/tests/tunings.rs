//! Tuning systems through the agent API (WP-6.9; the tunings spec, R19): the
//! tuning commands run by ID and read back through `tuning.state` and the
//! query tool, as an agent sets and reads them.

use std::path::Path;

use bl_agent::Agent;
use bl_agent::client::{Client, InProcess};
use bl_shell::Shell;
use serde_json::{Value, json};

fn client() -> Client<InProcess> {
    let mut client = Client::new(InProcess::new(Agent::headless(true), Shell::headless()));
    client.initialize("test-agent").unwrap();
    client
}

fn run(client: &mut Client<InProcess>, command: &str, args: Value) -> Value {
    let result = client
        .call(
            "run",
            json!({ "commands": [{ "command": command, "args": args }] }),
        )
        .unwrap()
        .ok()
        .unwrap_or_else(|error| panic!("{command} failed: {error}"));
    result["results"][0]["result"].clone()
}

fn query(client: &mut Client<InProcess>, args: Value) -> Value {
    client
        .call("query", args.clone())
        .unwrap()
        .ok()
        .unwrap_or_else(|error| panic!("query {args}: {error}"))
}

fn fixture(name: &str) -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/tunings")
        .join(name)
        .to_string_lossy()
        .into_owned()
}

// Spec: tunings R2, R7, R8, R9, R14, R19
#[test]
fn the_tuning_commands_run_by_id_and_read_back_through_tuning_state_and_the_query_tool() {
    let mut client = client();
    assert_eq!(
        run(&mut client, "tuning.state", json!({}))["tuning"],
        Value::Null
    );
    let song = query(&mut client, json!({ "path": "song" }));
    assert_eq!(song["tuning"], Value::Null, "twelve equal steps");

    let loaded = run(
        &mut client,
        "tuning.load",
        json!({ "path": fixture("equal-19.scl") }),
    );
    let tuning = &loaded["tuning"];
    assert_eq!(tuning["name"], "equal-19");
    assert_eq!(tuning["notes_per_octave"], 19);
    assert_eq!(tuning["pseudo_octave"], 1200.0);
    assert_eq!(
        tuning["reference"],
        json!({ "octave": 3, "degree": 0, "frequency": 261.626 })
    );
    assert_eq!(tuning["key_count"], 128);
    assert_eq!(
        run(&mut client, "tuning.state", json!({}))["tuning"],
        *tuning
    );
    assert_eq!(
        query(&mut client, json!({ "path": "song" }))["tuning"],
        *tuning
    );

    // The reference pitch, then the range.
    run(
        &mut client,
        "tuning.set_reference",
        json!({ "frequency": 270.0 }),
    );
    let state = run(&mut client, "tuning.state", json!({}));
    assert_eq!(state["tuning"]["reference"]["frequency"], 270.0);
    assert!((state["tuning"]["frequencies"][60].as_f64().unwrap() - 270.0).abs() < 1e-9);
    run(
        &mut client,
        "tuning.set_range",
        json!({ "highest": { "octave": 3, "degree": 5 } }),
    );
    let state = run(&mut client, "tuning.state", json!({}));
    assert_eq!(
        state["tuning"]["range"]["highest"],
        json!({ "octave": 3, "degree": 5 })
    );
    assert_eq!(
        state["tuning"]["key_count"], 128,
        "the other end moved with it"
    );

    // Bypass Tuning reads back from the track query.
    run(
        &mut client,
        "track.create",
        json!({ "id": { "$new": "keys" }, "kind": "midi", "name": "Keys", "index": 0 }),
    );
    let tracks = query(&mut client, json!({ "path": "tracks" }));
    let id = tracks["tracks"]["items"][0]["id"].clone();
    let track = query(&mut client, json!({ "id": id }));
    assert_eq!(track["tuning_bypass"], false);
    run(
        &mut client,
        "track.set_tuning_bypass",
        json!({ "track": id, "bypass": true }),
    );
    assert_eq!(
        query(&mut client, json!({ "id": id }))["tuning_bypass"],
        true
    );

    // Removing it puts the set back to twelve equal steps; undo brings it
    // back.
    run(&mut client, "tuning.remove", json!({}));
    assert_eq!(
        run(&mut client, "tuning.state", json!({}))["tuning"],
        Value::Null
    );
    run(&mut client, "edit.undo", json!({}));
    assert_eq!(
        run(&mut client, "tuning.state", json!({}))["tuning"]["name"],
        "equal-19"
    );
}

// Spec: tunings R15, R19. The retune setting reads back from settings.state.
#[test]
fn the_retune_setting_reads_back_from_settings_state() {
    let mut client = client();
    let state = run(&mut client, "settings.state", json!({}));
    assert_eq!(state["retune_on_tuning_load"], false);
    let result = run(
        &mut client,
        "settings.set_retune_on_tuning_load",
        json!({ "on": true }),
    );
    assert_eq!(result["on"], true);
    let state = run(&mut client, "settings.state", json!({}));
    assert_eq!(state["retune_on_tuning_load"], true);
}

// Spec: tunings R3. A bad file is refused with its reason, through the API.
#[test]
fn a_bad_file_is_refused_through_the_agent_api() {
    let mut client = client();
    let result = client
        .call(
            "run",
            json!({ "commands": [{
                "command": "tuning.load",
                "args": { "path": fixture("falling.scl") },
            }] }),
        )
        .unwrap();
    let text = format!("{result:?}");
    assert!(text.contains("pseudo-octave"), "{text}");
    assert_eq!(
        run(&mut client, "tuning.state", json!({}))["tuning"],
        Value::Null
    );
}

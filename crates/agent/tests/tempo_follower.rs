//! Tempo Follower through the agent API (WP-9.13,
//! `docs/specs/tempo-follower.md` R4, R19): the two settings are kept
//! preferences that `tempo_follower.state` reads back, and Follow is refused
//! with its reason while the audio input can't be reached, as it is on a
//! headless shell with no device. Each is run by ID.

use bl_agent::Agent;
use bl_agent::client::{Client, InProcess};
use bl_shell::Shell;
use serde_json::{Value, json};

fn client() -> Client<InProcess> {
    let mut client = Client::new(InProcess::new(Agent::headless(false), Shell::headless()));
    client.initialize("test-agent").unwrap();
    client
}

/// Runs one command and returns its result, or the error it was refused with.
fn run(client: &mut Client<InProcess>, command: &str, args: Value) -> Result<Value, String> {
    let result = client
        .call(
            "run",
            json!({ "commands": [{ "command": command, "args": args }] }),
        )
        .unwrap()
        .ok()
        .map_err(|error| error.to_string())?;
    let first = &result["results"][0];
    match first.get("error") {
        Some(error) => Err(error.to_string()),
        None => Ok(first["result"].clone()),
    }
}

#[test]
fn the_settings_are_set_by_id_and_read_back_in_tempo_follower_state() {
    let mut client = client();
    let state = run(&mut client, "tempo_follower.state", json!({})).unwrap();
    assert_eq!(state["on"], false);
    assert_eq!(state["show_toggle"], false);
    assert_eq!(state["input"], json!({ "type": "mono", "channel": 1 }));
    assert_eq!(state["tempo"], Value::Null);
    assert_eq!(state["blocks_dropped"], 0);

    let shown = run(
        &mut client,
        "settings.set_show_tempo_follower_toggle",
        json!({ "on": true }),
    )
    .unwrap();
    assert_eq!(shown["on"], true);
    let set = run(
        &mut client,
        "settings.set_tempo_follower_input",
        json!({ "channel": { "type": "pair", "channel": 3 } }),
    )
    .unwrap();
    assert_eq!(set["label"], "3/4");
    let state = run(&mut client, "tempo_follower.state", json!({})).unwrap();
    assert_eq!(state["show_toggle"], true, "read back");
    assert_eq!(state["input"], json!({ "type": "pair", "channel": 3 }));
    assert_eq!(state["input_label"], "3/4");
    let hidden = run(
        &mut client,
        "settings.set_show_tempo_follower_toggle",
        json!({}),
    )
    .unwrap();
    assert_eq!(
        hidden["on"], false,
        "left out, it switches to the other state"
    );
}

#[test]
fn follow_is_refused_with_its_reason_while_the_input_cant_be_reached() {
    let mut client = client();
    run(
        &mut client,
        "settings.set_show_tempo_follower_toggle",
        json!({ "on": true }),
    )
    .unwrap();
    let state = run(&mut client, "tempo_follower.state", json!({})).unwrap();
    assert_eq!(
        state["can_run"], false,
        "a headless shell has no audio device"
    );
    let reason = state["unavailable"].as_str().unwrap().to_owned();
    assert!(reason.contains("no audio device"), "{reason}");

    let refused = run(
        &mut client,
        "tempo_follower.set_enabled",
        json!({ "on": true }),
    )
    .expect_err("refused while the input can't be reached");
    assert!(refused.contains("no audio device"), "{refused}");
    let state = run(&mut client, "tempo_follower.state", json!({})).unwrap();
    assert_eq!(state["on"], false, "nothing changed");
}

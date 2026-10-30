//! Link through the agent API (WP-7.14, WP-7.15, `docs/specs/link.md` R15,
//! R24): `link.set_enabled` talks on the network, so a headless agent is
//! refused it unless it is autonomous; `link.set_start_stop_sync` and
//! `link.set_show_toggle` are kept preferences. Each is run by ID, and
//! `link.state` and `transport.state` read their effects back. Link runs on
//! its own here, so no peer joins the session, and these tests run one at a
//! time with `bl-link`'s (`.config/nextest.toml`).

use bl_agent::Agent;
use bl_agent::client::{Client, InProcess};
use bl_shell::Shell;
use serde_json::{Value, json};

fn client(autonomous: bool) -> Client<InProcess> {
    let mut client = Client::new(InProcess::new(
        Agent::headless(autonomous),
        Shell::headless(),
    ));
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
fn link_set_enabled_is_refused_headless_without_autonomous_and_runs_with_it() {
    let mut client = client(false);
    let refused = run(&mut client, "link.set_enabled", json!({ "on": true }))
        .expect_err("an external command, refused");
    assert!(refused.contains("autonomous"), "{refused}");
    let state = run(&mut client, "link.state", json!({})).unwrap();
    assert_eq!(state["enabled"], false, "nothing changed");

    let mut client = self::client(true);
    let on = run(&mut client, "link.set_enabled", json!({ "on": true })).unwrap();
    assert_eq!(on["on"], true);
    let state = run(&mut client, "link.state", json!({})).unwrap();
    assert_eq!(state["enabled"], true, "link.state reads it back");
    assert_eq!(state["peers"], 0);
    assert_eq!(state["quantum"], 4.0);
    assert_eq!(state["waiting_for_link"], false);
    assert!((state["tempo"].as_f64().unwrap() - 120.0).abs() < 0.01);
    let off = run(&mut client, "link.set_enabled", json!({})).unwrap();
    assert_eq!(off["on"], false, "left out, it switches to the other state");
    let state = run(&mut client, "link.state", json!({})).unwrap();
    assert_eq!(state["enabled"], false);
}

#[test]
fn link_set_start_stop_sync_is_set_by_id_and_read_back() {
    // Spec: link R8, R15. A kept preference, so no autonomy is needed.
    let mut client = client(false);
    let state = run(&mut client, "link.state", json!({})).unwrap();
    assert_eq!(state["start_stop_sync"], false);
    let on = run(
        &mut client,
        "link.set_start_stop_sync",
        json!({ "on": true }),
    )
    .unwrap();
    assert_eq!(on["on"], true);
    let state = run(&mut client, "link.state", json!({})).unwrap();
    assert_eq!(state["start_stop_sync"], true);
    let transport = run(&mut client, "transport.state", json!({})).unwrap();
    assert_eq!(transport["waiting_for_link"], false);
    assert_eq!(transport["waiting_start"], false);
    run(
        &mut client,
        "link.set_start_stop_sync",
        json!({ "on": false }),
    )
    .unwrap();
    let state = run(&mut client, "link.state", json!({})).unwrap();
    assert_eq!(state["start_stop_sync"], false);
}

#[test]
fn link_set_show_toggle_is_set_by_id_and_read_back() {
    // Spec: link R17, R24. A kept preference, so no autonomy is needed, and
    // the Control Bar's toggle shows in a new install.
    let mut client = client(false);
    let state = run(&mut client, "link.state", json!({})).unwrap();
    assert_eq!(state["show_toggle"], true);
    let off = run(&mut client, "link.set_show_toggle", json!({ "on": false })).unwrap();
    assert_eq!(off["on"], false);
    let state = run(&mut client, "link.state", json!({})).unwrap();
    assert_eq!(state["show_toggle"], false, "link.state reads it back");
    assert_eq!(state["enabled"], false, "hiding it leaves Link as it is");
    let on = run(&mut client, "link.set_show_toggle", json!({})).unwrap();
    assert_eq!(on["on"], true, "left out, it switches to the other state");
    let state = run(&mut client, "link.state", json!({})).unwrap();
    assert_eq!(state["show_toggle"], true);
}

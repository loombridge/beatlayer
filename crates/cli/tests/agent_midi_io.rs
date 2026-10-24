//! MIDI ports and the computer MIDI keyboard through the agent API (WP-2.2):
//! a scripted MCP client drives `bl-cli mcp` and runs each `midi.*` command
//! by ID. The headless shell has no MIDI system, so no ports show; the
//! keyboard commands and the refusals are the same as in the app.

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

/// Spec: midi-io R6, R17, R19, R20, R21, R24.
#[test]
fn an_agent_lists_the_ports_and_plays_the_computer_keyboard() {
    let mut client = bl_cli_mcp(&["--autonomous"]);

    let ports = run(&mut client, "midi.ports", json!({}));
    assert_eq!(ports["ports"], json!([]));
    assert_eq!(ports["keyboard"]["on"], false);
    assert_eq!(ports["keyboard"]["lowest_note"], 60);
    assert_eq!(ports["keyboard"]["velocity"], 100);

    let keyboard = run(&mut client, "midi.toggle_computer_keyboard", json!({}));
    assert_eq!(keyboard["on"], true);
    let keyboard = run(
        &mut client,
        "midi.computer_keyboard_octave",
        json!({ "steps": 1 }),
    );
    assert_eq!(keyboard["lowest_note"], 72);
    assert_eq!(keyboard["range"], "C4 to D#5");
    let keyboard = run(
        &mut client,
        "midi.computer_keyboard_velocity",
        json!({ "steps": -1 }),
    );
    assert_eq!(keyboard["velocity"], 80);

    let down = run(
        &mut client,
        "midi.computer_keyboard_key",
        json!({ "key": "A", "pressed": true }),
    );
    assert_eq!(down, json!({ "sent": true, "note": 72 }));
    let up = run(
        &mut client,
        "midi.computer_keyboard_key",
        json!({ "key": "A", "pressed": false }),
    );
    assert_eq!(up, json!({ "sent": true, "note": 72 }));

    let keyboard = run(
        &mut client,
        "midi.toggle_computer_keyboard",
        json!({ "on": false }),
    );
    assert_eq!(keyboard["on"], false);

    // A port the computer doesn't offer is refused.
    let refused = client
        .call(
            "run",
            json!({ "commands": [{ "command": "midi.set_port",
                                   "args": { "direction": "input", "name": "Nothing",
                                             "track": true } }] }),
        )
        .unwrap();
    assert!(refused.is_error);
    assert!(refused.text.contains("Nothing"), "{}", refused.text);
    assert!(client.into_transport().finish().unwrap().success());
}

/// Spec: midi-io R6. Changing a port's switches opens a device outside
/// Beatlayer, so it is external.
#[test]
fn without_autonomous_changing_a_port_is_refused() {
    let mut client = bl_cli_mcp(&[]);
    let refused = client
        .call(
            "run",
            json!({ "commands": [{ "command": "midi.set_port",
                                   "args": { "direction": "input", "name": "Keys",
                                             "track": false } }] }),
        )
        .unwrap();
    assert!(refused.is_error);
    assert!(
        refused.text.contains("`midi.set_port` is external"),
        "{}",
        refused.text
    );
    // Listing the ports is a read.
    let ports = run(&mut client, "midi.ports", json!({}));
    assert_eq!(ports["ports"], json!([]));
}

/// Spec: midi-io R27. The MPE switch is `midi.set_port`'s `mpe`, which is
/// external like the other switches, and is accepted as an argument when
/// autonomous; a port the computer doesn't offer is still refused.
#[test]
fn the_mpe_switch_is_external_like_the_others() {
    let mut client = bl_cli_mcp(&[]);
    let refused = client
        .call(
            "run",
            json!({ "commands": [{ "command": "midi.set_port",
                                   "args": { "direction": "input", "name": "Roli",
                                             "mpe": true } }] }),
        )
        .unwrap();
    assert!(refused.is_error);
    assert!(
        refused.text.contains("`midi.set_port` is external"),
        "{}",
        refused.text
    );
    assert!(client.into_transport().finish().unwrap().success());

    let mut client = bl_cli_mcp(&["--autonomous"]);
    let refused = client
        .call(
            "run",
            json!({ "commands": [{ "command": "midi.set_port",
                                   "args": { "direction": "input", "name": "Roli",
                                             "mpe": true } }] }),
        )
        .unwrap();
    assert!(refused.is_error);
    assert!(
        refused.text.contains("Roli") && !refused.text.contains("unknown field"),
        "the argument is known; the port isn't offered: {}",
        refused.text
    );
    assert!(client.into_transport().finish().unwrap().success());
}

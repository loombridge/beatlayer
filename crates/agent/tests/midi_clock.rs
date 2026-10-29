//! MIDI clock out through the agent API (WP-7.11, `docs/specs/midi-clock.md`
//! R25): an output's sync options and Show Resync Button are set by ID and
//! read back from `midi.ports`, the resync runs by ID, and `sync.state`
//! reports the sending half. The shell plays through the null device paced
//! by hand (T199), as the app's host does.

use bl_agent::Agent;
use bl_agent::client::{Client, InProcess};
use bl_audio_io::NullControl;
use bl_midi_io::{Direction, TestBackend};
use bl_shell::testing;
use serde_json::{Value, json};

fn host() -> (Client<InProcess>, NullControl) {
    let test = TestBackend::new();
    test.add_port(Direction::Input, "Keys");
    test.add_port(Direction::Output, "Drum machine");
    let (mut shell, control) = testing::rig();
    shell.use_midi_backend(Box::new(test));
    shell.start_audio();
    assert!(shell.audio().engine_running(), "{:?}", shell.audio());
    let mut client = Client::new(InProcess::new(Agent::for_app(), shell));
    // The person allows the connection and the external command.
    client.transport().answer = Box::new(|_| true);
    client.initialize("conductor").unwrap();
    (client, control)
}

/// Runs one command, then what the app's event loop does: the engine hears
/// it, and `millis` of test time pass.
fn run(
    control: &NullControl,
    client: &mut Client<InProcess>,
    command: &str,
    args: Value,
    millis: u64,
) -> Value {
    let result = client
        .call(
            "run",
            json!({ "commands": [{ "command": command, "args": args }] }),
        )
        .unwrap()
        .ok()
        .unwrap_or_else(|error| panic!("{command} failed: {error}"));
    let host = client.transport();
    host.agent.sync_engine(&mut host.shell);
    testing::advance(control, &mut host.shell, millis);
    result["results"][0]["result"].clone()
}

fn drums(ports: &Value) -> &Value {
    ports["ports"]
        .as_array()
        .unwrap()
        .iter()
        .find(|port| port["name"] == "Drum machine")
        .unwrap()
}

#[test]
fn the_sync_options_and_the_resync_button_are_set_by_id_and_read_back() {
    let (mut client, control) = host();
    let mut run =
        |command: &str, args: Value, millis: u64| run(&control, &mut client, command, args, millis);
    let before = run("midi.ports", json!({}), 5);
    assert_eq!(drums(&before)["sync_delay_ms"], 0);
    assert_eq!(drums(&before)["clock_type"], "song");
    assert_eq!(drums(&before)["hardware_resync"], "stop_and_start");
    assert_eq!(before["show_resync_button"], false);
    let state = run("sync.state", json!({}), 5);
    assert_eq!(state["output_ports"], 0);
    assert_eq!(state["sending"], false);

    run(
        "midi.set_port",
        json!({ "direction": "output", "name": "Drum machine", "sync": true,
                "sync_delay_ms": 25, "clock_type": "pattern",
                "hardware_resync": "start_only" }),
        5,
    );
    run("midi.set_show_resync_button", json!({ "on": true }), 5);
    let after = run("midi.ports", json!({}), 5);
    let port = drums(&after);
    assert_eq!(port["sync"], true);
    assert_eq!(port["sync_delay_ms"], 25);
    assert_eq!(port["clock_type"], "pattern");
    assert_eq!(port["hardware_resync"], "start_only");
    assert_eq!(after["show_resync_button"], true);

    // Stopped, the resync does nothing and nothing is sent.
    run("transport.resync_external_hardware", json!({}), 50);
    let state = run("sync.state", json!({}), 5);
    assert_eq!(
        (state["output_ports"].clone(), state["sending"].clone()),
        (json!(1), json!(false))
    );
    assert_eq!(state["clocks_sent"], 0);

    // Playing, a quarter note of clock goes out, and the resync runs by ID.
    run("transport.play", json!({}), 600);
    run("transport.resync_external_hardware", json!({}), 600);
    let state = run("sync.state", json!({}), 5);
    assert_eq!(state["sending"], true);
    assert!(state["clocks_sent"].as_u64().unwrap() >= 24, "{state}");
    assert_eq!(state["clocks_dropped"], 0);
}

#[test]
fn a_delay_past_the_range_and_an_inputs_clock_type_are_refused() {
    let (mut client, _control) = host();
    for args in [
        json!({ "direction": "output", "name": "Drum machine", "sync_delay_ms": 101 }),
        json!({ "direction": "input", "name": "Keys", "clock_type": "song" }),
    ] {
        let refused = client
            .call(
                "run",
                json!({ "commands": [{ "command": "midi.set_port", "args": args }] }),
            )
            .unwrap();
        assert!(
            refused.is_error,
            "{args} should be refused: {}",
            refused.text
        );
    }
}

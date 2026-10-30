//! MIDI clock through the agent API (WP-7.11 and WP-7.12,
//! `docs/specs/midi-clock.md` R25): an output's sync options and Show Resync
//! Button are set by ID and read back from `midi.ports`, the resync runs by
//! ID, and `sync.state` reports the sending half; EXT is switched by ID and
//! `sync.state` and `transport.state` read the receiving half back. The shell plays through the null device paced
//! by hand (T199), as the app's host does.

use bl_agent::Agent;
use bl_agent::client::{Client, InProcess};
use bl_audio_io::NullControl;
use bl_midi_io::{Direction, TestBackend};
use bl_shell::testing;
use serde_json::{Value, json};

fn host() -> (Client<InProcess>, NullControl) {
    let (client, control, _) = host_with_ports();
    (client, control)
}

fn host_with_ports() -> (Client<InProcess>, NullControl, TestBackend) {
    let test = TestBackend::new();
    test.add_port(Direction::Input, "Keys");
    test.add_port(Direction::Output, "Drum machine");
    let (mut shell, control) = testing::rig();
    shell.use_midi_backend(Box::new(test.clone()));
    shell.start_audio();
    assert!(shell.audio().engine_running(), "{:?}", shell.audio());
    let mut client = Client::new(InProcess::new(Agent::for_app(), shell));
    // The person allows the connection and the external command.
    client.transport().answer = Box::new(|_| true);
    client.initialize("conductor").unwrap();
    (client, control, test)
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

#[test]
fn ext_is_switched_by_id_and_the_clock_it_follows_is_read_back() {
    let (mut client, control, test) = host_with_ports();
    let mut run =
        |command: &str, args: Value, millis: u64| run(&control, &mut client, command, args, millis);
    let state = run("sync.state", json!({}), 5);
    assert_eq!(state["external_sync_available"], false);
    assert_eq!(state["external_sync"], false);
    assert_eq!(run("transport.state", json!({}), 5)["external_sync"], false);

    run(
        "midi.set_port",
        json!({ "direction": "input", "name": "Keys", "sync": true, "sync_delay_ms": 3 }),
        5,
    );
    let state = run("sync.state", json!({}), 5);
    assert_eq!(state["external_sync_available"], true);
    assert_eq!(state["input_ports"], 1);
    let on = run("transport.set_external_sync", json!({ "on": true }), 5);
    assert_eq!(on["on"], true);
    assert_eq!(run("transport.state", json!({}), 5)["external_sync"], true);

    // A 140 BPM clock, 160 clocks of it, and the song plays along.
    let now = control.clock().unwrap().now();
    let period = std::time::Duration::from_secs_f64(60.0 / (140.0 * 24.0));
    for k in 0..160 {
        assert!(test.send("Keys", now + period * k, &[0xF8]));
    }
    run("transport.play", json!({}), 2_500);
    let state = run("sync.state", json!({}), 5);
    assert_eq!(state["receiving"], true);
    assert_eq!(state["leading_input"], "Keys");
    assert!(
        (state["tempo"].as_f64().unwrap() - 140.0).abs() < 0.2,
        "{state}"
    );
    assert!(state["clocks_received"].as_u64().unwrap() > 80);
    assert_eq!(state["clocks_set_aside"], 0);
    let transport = run("transport.state", json!({}), 5);
    assert!((transport["tempo"].as_f64().unwrap() - 140.0).abs() < 0.2);
    assert_eq!(transport["tempo_editable"], false);

    // Left out, it switches to the other state.
    let off = run("transport.set_external_sync", json!({}), 50);
    assert_eq!(off["on"], false);
    let transport = run("transport.state", json!({}), 5);
    assert_eq!(transport["external_sync"], false);
    assert_eq!(transport["tempo"], 120.0);
}

#[test]
fn ext_is_refused_while_no_input_has_sync_on() {
    let (mut client, _control) = host();
    let refused = client
        .call(
            "run",
            json!({ "commands": [{ "command": "transport.set_external_sync",
                                   "args": { "on": true } }] }),
        )
        .unwrap();
    assert!(refused.is_error, "{}", refused.text);
}

// Spec: midi-timecode R1, R12. An input's sync type, rate and offset are set
// by ID and read back from `midi.ports`; `sync.state` reports the last whole
// time, its rate, whether it runs and the song position it gives.
#[test]
fn timecode_options_are_set_by_id_and_the_time_it_follows_is_read_back() {
    let (mut client, control, test) = host_with_ports();
    let mut run =
        |command: &str, args: Value, millis: u64| run(&control, &mut client, command, args, millis);
    let set = run(
        "midi.set_port",
        json!({ "direction": "input", "name": "Keys", "sync": true,
                "sync_type": "midi_timecode", "timecode_rate": "smpte_all",
                "timecode_offset": "00:00:10:00" }),
        5,
    );
    assert_eq!(set["sync_type"], "midi_timecode");
    assert_eq!(set["timecode_offset"], "00:00:10:00");
    let ports = run("midi.ports", json!({}), 5);
    let keys = &ports["ports"][0];
    assert_eq!(
        (
            &keys["sync_type"],
            &keys["timecode_rate"],
            &keys["timecode_offset"]
        ),
        (
            &json!("midi_timecode"),
            &json!("smpte_all"),
            &json!("00:00:10:00")
        )
    );
    run("transport.set_external_sync", json!({ "on": true }), 5);
    assert_eq!(run("sync.state", json!({}), 5)["timecode"], Value::Null);

    // Two whole times of 25 fps timecode from 00:00:12:00: the song starts at
    // the offset's two seconds on.
    let now = control.clock().unwrap().now();
    let frame = std::time::Duration::from_millis(40);
    for pair in 0..2u32 {
        let frames = 300 + pair * 2;
        let (s, f) = ((frames / 25 % 60) as u8, (frames % 25) as u8);
        let nibbles = [f & 15, f >> 4, s & 15, s >> 4, 0, 0, 0, 1 << 1];
        for (k, nibble) in nibbles.into_iter().enumerate() {
            let at = now + frame * (pair * 2) + frame * k as u32 / 4;
            assert!(test.send("Keys", at, &[0xF1, ((k as u8) << 4) | nibble]));
        }
    }
    run("transport.state", json!({}), 200);
    let state = run("sync.state", json!({}), 5);
    let report = &state["timecode"];
    assert_eq!(report["rate"], "fps_25");
    assert_eq!(report["time"], "00:00:12:02");
    assert_eq!(report["running"], true);
    // 12:02 and the eighth piece's two frames, less the offset: 2.16 s, 4.32
    // quarter notes at 120 BPM.
    let ticks = report["position_ticks"].as_i64().unwrap() as f64;
    assert!((ticks - 2.16 * 2.0 * 705_600.0).abs() < 2.0, "{ticks}");
    assert_eq!(run("transport.state", json!({}), 5)["playing"], true);
}

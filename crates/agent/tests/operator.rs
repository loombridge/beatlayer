//! Operator's parameters through the agent API (WP-8.12; the Operator spec,
//! R11, R19, R20): an agent adds the instrument to a track, sets every
//! parameter by its ID, and reads each back, with the per-note pitch bend
//! marked as a menu option.

use bl_agent::Agent;
use bl_agent::client::{Client, InProcess};
use bl_devices::operator::{NOTE_PB, OPERATOR, operator_type};
use bl_model::{DeviceId, ParamId, TrackId, TrackKind};
use bl_shell::Shell;
use serde_json::{Value, json};

fn client() -> Client<InProcess> {
    let mut client = Client::new(InProcess::new(Agent::headless(true), Shell::headless()));
    client.initialize("test-agent").unwrap();
    client
}

fn run(client: &mut Client<InProcess>, command: &str, args: Value) -> Value {
    client
        .call(
            "run",
            json!({ "commands": [{ "command": command, "args": args }] }),
        )
        .unwrap()
        .ok()
        .unwrap_or_else(|error| panic!("{command} failed: {error}"))
}

fn audio_track(shell: &Shell) -> TrackId {
    shell
        .editor()
        .document()
        .tracks()
        .find(|track| track.kind() == TrackKind::Audio)
        .map(bl_model::Track::id)
        .expect("the default set has an audio track")
}

// Spec: operator R11, R19, R20
#[test]
fn an_agent_sets_every_operator_parameter_by_id_and_reads_it_back() {
    let mut client = client();
    let track = audio_track(&client.transport().shell);
    let added = run(
        &mut client,
        "device.add",
        json!({
            "device": {
                "kind": { "native": { "device_type": OPERATOR } },
                "name": "Operator",
                "role": "instrument",
            },
            "track": track,
        }),
    );
    let device: DeviceId = serde_json::from_value(added["results"][0]["result"]["device"].clone())
        .expect("the new device");

    // Every parameter to the other end of its range.
    let mut wanted: Vec<(ParamId, f64)> = Vec::new();
    for param in &operator_type().description.params {
        let to = if param.default == param.max() {
            param.min()
        } else {
            param.max()
        };
        run(
            &mut client,
            "device.set_param",
            json!({ "device": device, "param": param.id, "value": to }),
        );
        wanted.push((param.id, to));
    }
    let described = run(
        &mut client,
        "device.parameters",
        json!({ "device": device }),
    );
    let described = &described["results"][0]["result"]["params"];
    let entries = described.as_array().expect("the params");
    assert_eq!(entries.len(), wanted.len(), "every parameter is described");
    for (id, value) in wanted {
        let entry = entries
            .iter()
            .find(|e| e["id"] == json!(id.0))
            .unwrap_or_else(|| panic!("{id:?} is described"));
        assert_eq!(entry["value"], json!(value), "{id:?}");
        assert_eq!(
            entry["menu_option"],
            json!(id == NOTE_PB),
            "only the per-note pitch bend is a menu option"
        );
    }
}

/// The query tool's `device` result for `device`.
fn device_result(client: &mut Client<InProcess>, device: DeviceId) -> Value {
    client
        .call("query", json!({ "path": "device", "id": device }))
        .unwrap()
        .ok()
        .unwrap_or_else(|error| panic!("query failed: {error}"))
}

// Spec: operator R27, R28, R29
#[test]
fn an_agent_draws_partials_runs_the_menus_commands_and_reads_the_displays() {
    let mut client = client();
    let track = audio_track(&client.transport().shell);
    let added = run(
        &mut client,
        "device.add",
        json!({
            "device": {
                "kind": { "native": { "device_type": OPERATOR } },
                "name": "Operator",
                "role": "instrument",
            },
            "track": track,
        }),
    );
    let device: DeviceId = serde_json::from_value(added["results"][0]["result"]["device"].clone())
        .expect("the new device");
    // No wave is drawn in a new device, so none is reported.
    assert!(device_result(&mut client, device).get("partials").is_none());
    // device.set_partials draws osc B's wave, and the query reads it back.
    let mut levels = vec![0.0f32; 64];
    levels[0] = 1.0;
    levels[4] = 0.25;
    run(
        &mut client,
        "device.set_partials",
        json!({ "device": device, "osc": 1, "levels": levels, "repeat": 30.0 }),
    );
    let result = device_result(&mut client, device);
    assert_eq!(result["partials"], json!([{ "osc": 1, "levels": levels }]));
    // Operator's menu commands are agent commands too, each read back from
    // the device's parameters.
    run(
        &mut client,
        "operator.copy_oscillator",
        json!({ "device": device, "from": 1, "into": 3 }),
    );
    let result = device_result(&mut client, device);
    let partials = result["partials"].as_array().expect("partials");
    assert!(
        partials
            .iter()
            .any(|p| p["osc"] == json!(3) && p["levels"] == json!(levels))
    );
    run(
        &mut client,
        "operator.set_envelope_levels",
        json!({ "device": device, "envelope": "LFO", "to": "minimum" }),
    );
    run(
        &mut client,
        "operator.play_by_key",
        json!({ "device": device }),
    );
    let described = run(
        &mut client,
        "device.parameters",
        json!({ "device": device }),
    );
    let value_of = |name: &str| -> Value {
        described["results"][0]["result"]["params"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["name"] == json!(name))
            .unwrap_or_else(|| panic!("{name}"))["value"]
            .clone()
    };
    assert_eq!(value_of("LFO Sustain"), json!(-100.0));
    assert_eq!(value_of("Filter Freq"), json!(466.0));
    assert_eq!(value_of("Freq < Key"), json!(100.0));
    // device.display reports the view chosen, from the parameters.
    run(
        &mut client,
        "device.set_display_view",
        json!({ "device": device, "view": "Filter Response" }),
    );
    let shown = run(&mut client, "device.display", json!({ "device": device }));
    let display = &shown["results"][0]["result"];
    assert_eq!(display["view"], json!("Filter Response"));
    assert_eq!(display["series"][0]["name"], json!("Filter Response"));
}

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

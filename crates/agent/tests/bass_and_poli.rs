//! Bass and Poli through the agent API (WP-8.16; the bass and Poli spec, R1,
//! R2 and R17): an agent adds each to a track, sets every parameter to a
//! value other than its default and reads each back, as the set holds it
//! and as `device.parameters` reports it.

use bl_agent::Agent;
use bl_agent::client::{Client, InProcess};
use bl_devices::bass::{BASS, bass_type};
use bl_devices::poli::{POLI, poli_type};
use bl_engine::DeviceType;
use bl_model::{DeviceId, ParamId, ParamKind, TrackId, TrackKind};
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

fn held(shell: &Shell, device: DeviceId, param: ParamId) -> Option<f64> {
    shell
        .editor()
        .document()
        .device(device)
        .and_then(|device| device.params.get(&param).copied())
}

/// A value of `param` other than its default, inside its range.
fn other_than_default(param: &bl_model::ParamDescriptor) -> f64 {
    match &param.kind {
        ParamKind::Toggle => 1.0 - param.default,
        ParamKind::Choice { choices } => (choices.len() - 1) as f64 - param.default,
        ParamKind::Integer { .. } => {
            let middle = ((param.min() + param.max()) / 2.0).round();
            if middle == param.default {
                middle + 1.0
            } else {
                middle
            }
        }
        _ => {
            let middle = param.denormalize(0.5);
            if (middle - param.default).abs() > 1e-9 {
                middle
            } else {
                param.denormalize(0.75)
            }
        }
    }
}

fn sets_and_reads_every_parameter(device_type: &str, name: &str, kind: &DeviceType) {
    let mut client = client();
    let track = audio_track(&client.transport().shell);
    let added = run(
        &mut client,
        "device.add",
        json!({
            "device": {
                "kind": { "native": { "device_type": device_type } },
                "name": name,
                "role": "instrument",
            },
            "track": track,
        }),
    );
    let device: DeviceId = serde_json::from_value(added["results"][0]["result"]["device"].clone())
        .expect("the new device");

    for param in &kind.description.params {
        let value = other_than_default(param);
        assert_ne!(value, param.default, "{} has a value to set", param.name);
        run(
            &mut client,
            "device.set_param",
            json!({ "device": device, "param": param.id, "value": value }),
        );
        assert_eq!(
            held(&client.transport().shell, device, param.id),
            Some(value),
            "{} is held",
            param.name
        );
    }
    let described = run(
        &mut client,
        "device.parameters",
        json!({ "device": device }),
    );
    let entries = described["results"][0]["result"]["params"]
        .as_array()
        .expect("the params");
    assert_eq!(entries.len(), kind.description.params.len());
    for param in &kind.description.params {
        let entry = entries
            .iter()
            .find(|entry| entry["name"] == param.name)
            .unwrap_or_else(|| panic!("{} is described", param.name));
        assert_eq!(
            entry["value"].as_f64(),
            Some(other_than_default(param)),
            "{} reads back",
            param.name
        );
        assert!(
            entry["text"].as_str().is_some_and(|text| !text.is_empty()),
            "{} reads with its unit",
            param.name
        );
    }
}

// Spec: bass and Poli R1, R2, R17 (agent API)
#[test]
fn an_agent_sets_and_reads_every_parameter_of_bass() {
    sets_and_reads_every_parameter(BASS, "Bass", &bass_type());
}

// Spec: bass and Poli R1, R2, R17 (agent API)
#[test]
fn an_agent_sets_and_reads_every_parameter_of_poli() {
    sets_and_reads_every_parameter(POLI, "Poli", &poli_type());
}

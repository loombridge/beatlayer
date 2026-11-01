//! The instruments' filter through the agent API (WP-8.3; the instrument
//! filter spec, R14 and R16): an agent adds Simpler and Sampler to a track,
//! sets each filter parameter the circuits brought and reads it back.

use bl_agent::Agent;
use bl_agent::client::{Client, InProcess};
use bl_devices::instrument_filter::CIRCUITS;
use bl_devices::multisampler::{
    FILTER_CIRCUIT as SAMPLER_CIRCUIT, FILTER_DRIVE as SAMPLER_DRIVE,
    FILTER_MORPH as SAMPLER_MORPH, FILTER_TYPE as SAMPLER_TYPE, MULTISAMPLER,
};
use bl_devices::sampler::{
    FILTER_CIRCUIT, FILTER_DRIVE, FILTER_MORPH, FILTER_TYPE, FILTER_TYPE_CHOICES, SAMPLER,
};
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

fn held(shell: &Shell, device: DeviceId, param: ParamId) -> Option<f64> {
    shell
        .editor()
        .document()
        .device(device)
        .and_then(|device| device.params.get(&param).copied())
}

fn add(client: &mut Client<InProcess>, device_type: &str, name: &str) -> DeviceId {
    let track = audio_track(&client.transport().shell);
    let added = run(
        client,
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
    serde_json::from_value(added["results"][0]["result"]["device"].clone()).expect("the new device")
}

/// Sets each `(param, value)` by command and reads it back from the set and
/// from `device.parameters`.
fn set_and_read(
    client: &mut Client<InProcess>,
    device: DeviceId,
    settings: &[(ParamId, &str, f64)],
) {
    for (id, name, value) in settings {
        run(
            client,
            "device.set_param",
            json!({ "device": device, "param": id, "value": value }),
        );
        assert_eq!(
            held(&client.transport().shell, device, *id),
            Some(*value),
            "{name}"
        );
        let described = run(client, "device.parameters", json!({ "device": device }));
        let entry = described["results"][0]["result"]["params"]
            .as_array()
            .expect("the params")
            .iter()
            .find(|entry| entry["name"] == *name)
            .unwrap_or_else(|| panic!("{name} is described"))
            .clone();
        assert_eq!(entry["value"].as_f64(), Some(*value), "{name} reads back");
    }
}

// Spec: instrument filter R14, R16 (Simpler's Circuit, Drive and Morph)
#[test]
fn an_agent_sets_and_reads_simplers_circuit_drive_morph_and_types() {
    let mut client = client();
    let device = add(&mut client, SAMPLER, "Simpler");
    let mut settings = vec![(FILTER_DRIVE, "Drive", 18.0), (FILTER_MORPH, "Morph", 40.0)];
    for index in 0..CIRCUITS.len() {
        settings.push((FILTER_CIRCUIT, "Circuit", index as f64));
    }
    for index in 0..FILTER_TYPE_CHOICES.len() {
        settings.push((FILTER_TYPE, "Type", index as f64));
    }
    set_and_read(&mut client, device, &settings);
    // PRD stays chosen when the type goes to one that doesn't offer it.
    set_and_read(
        &mut client,
        device,
        &[(FILTER_CIRCUIT, "Circuit", 4.0), (FILTER_TYPE, "Type", 4.0)],
    );
    assert_eq!(
        held(&client.transport().shell, device, FILTER_CIRCUIT),
        Some(4.0)
    );
}

// Spec: instrument filter R14, R16 (Sampler's Circuit)
#[test]
fn an_agent_sets_and_reads_samplers_circuit_and_its_other_filter_values() {
    let mut client = client();
    let device = add(&mut client, MULTISAMPLER, "Sampler");
    let mut settings = vec![
        (SAMPLER_DRIVE, "Filter Drive", 12.0),
        (SAMPLER_MORPH, "Filter Morph", 60.0),
        (SAMPLER_TYPE, "Filter Type", 4.0),
    ];
    for index in 0..CIRCUITS.len() {
        settings.push((SAMPLER_CIRCUIT, "Filter Circuit", index as f64));
    }
    set_and_read(&mut client, device, &settings);
}

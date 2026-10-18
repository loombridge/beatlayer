//! Drift's LFO, modulation routes and MPE settings through the agent API
//! (WP-2.17, part 2): an agent adds the instrument to a track and reaches
//! every LFO control, every modulation source, destination and amount, and
//! the MPE settings by command.

use bl_agent::Agent;
use bl_agent::client::{Client, InProcess};
use bl_devices::polysynth::{
    FREQ, HP, LFO_AMOUNT, LFO_DIVISIONS, LFO_MODES, LFO_RATIO, LFO_SHAPES, LFO_SYNC, LFO_TIME,
    MOD_DESTINATIONS, MOD_SOURCES, MOD1_AMOUNT, MOD1_DEST, MOD1_SOURCE, NOISE_GAIN, NOTE_PB,
    OSC1_GAIN, OSC1_SHAPE, OSC2_DETUNE, OSC2_GAIN, PB_RANGE, PITCH_MOD1_AMOUNT, PITCH_MOD1_SOURCE,
    POLYSYNTH, RES, SHAPE_MOD_AMOUNT, SHAPE_MOD_SOURCE, VOLUME,
};
use bl_model::{DeviceId, ParamId, TrackId, TrackKind};
use bl_shell::Shell;
use serde_json::{Value, json};

fn client() -> Client<InProcess> {
    let mut client = Client::new(InProcess::new(Agent::headless(true), Shell::headless()));
    client.initialize("test-agent").unwrap();
    client
}

fn call(client: &mut Client<InProcess>, tool: &str, args: Value) -> Value {
    client
        .call(tool, args)
        .unwrap()
        .ok()
        .unwrap_or_else(|error| panic!("{tool} failed: {error}"))
}

fn run(client: &mut Client<InProcess>, command: &str, args: Value) -> Value {
    call(
        client,
        "run",
        json!({ "commands": [{ "command": command, "args": args }] }),
    )
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

// Spec: polysynth R19 to R21 (the LFO, the routes and MPE as commands)
#[test]
fn every_lfo_route_and_mpe_setting_is_reachable_by_command() {
    let mut client = client();
    let track = audio_track(&client.transport().shell);
    let added = run(
        &mut client,
        "device.add",
        json!({
            "device": {
                "kind": { "native": { "device_type": POLYSYNTH } },
                "name": "Drift",
                "role": "instrument",
            },
            "track": track,
        }),
    );
    let device: DeviceId = serde_json::from_value(added["results"][0]["result"]["device"].clone())
        .expect("the new device");

    // Set and read back one parameter per route, by command.
    let mut settings: Vec<(ParamId, f64)> = Vec::new();
    for (device_param, value) in [
        (LFO_SYNC, 2.0),
        (LFO_RATIO, 2.0),
        (LFO_TIME, 250.0),
        (LFO_AMOUNT, 40.0),
        (MOD1_SOURCE, 0.0),
        (MOD1_DEST, 0.0),
        (MOD1_AMOUNT, 0.0),
        (SHAPE_MOD_SOURCE, 1.0),
        (SHAPE_MOD_AMOUNT, -30.0),
        (PITCH_MOD1_SOURCE, 1.0),
        (PITCH_MOD1_AMOUNT, 20.0),
        (NOTE_PB, 0.0),
        (PB_RANGE, 24.0),
    ] {
        settings.push((device_param, value));
    }
    // Every modulation source and destination the manual lists.
    for index in 1..MOD_SOURCES.len() {
        settings.push((MOD1_SOURCE, index as f64));
    }
    for index in 1..MOD_DESTINATIONS.len() {
        settings.push((MOD1_DEST, index as f64));
    }
    // Every LFO shape, time mode and Sync division.
    for index in 0..LFO_SHAPES.len() {
        settings.push((bl_devices::polysynth::LFO_SHAPE, index as f64));
    }
    for index in 0..LFO_MODES.len() {
        settings.push((bl_devices::polysynth::LFO_MODE, index as f64));
    }
    for index in 0..LFO_DIVISIONS.len() {
        settings.push((LFO_SYNC, index as f64));
    }
    for (id, value) in settings {
        run(
            &mut client,
            "device.set_param",
            json!({ "device": device, "param": id, "value": value }),
        );
        assert_eq!(
            held(&client.transport().shell, device, id),
            Some(value),
            "parameter {id:?}"
        );
    }

    // And every Mod destination's own parameter, by command.
    for (id, value) in [
        (OSC1_GAIN, 1.0),
        (OSC1_SHAPE, 50.0),
        (OSC2_GAIN, 0.5),
        (OSC2_DETUNE, -5.0),
        (NOISE_GAIN, 0.25),
        (FREQ, 2_000.0),
        (RES, 30.0),
        (HP, 100.0),
        (bl_devices::polysynth::LFO_RATE, 4.0),
        (VOLUME, 0.5),
    ] {
        run(
            &mut client,
            "device.set_param",
            json!({ "device": device, "param": id, "value": value }),
        );
        assert_eq!(
            held(&client.transport().shell, device, id),
            Some(value),
            "destination {id:?}"
        );
    }

    // The instrument describes the new controls to an agent.
    let described = run(
        &mut client,
        "device.parameters",
        json!({ "device": device }),
    );
    let described = &described["results"][0]["result"];
    let names: Vec<String> = described["params"]
        .as_array()
        .expect("the params")
        .iter()
        .filter_map(|entry| entry["name"].as_str().map(str::to_owned))
        .collect();
    for name in [
        "LFO Mode",
        "LFO Shape",
        "LFO Sync",
        "Mod 1 Source",
        "Mod 1 Dest",
        "Shape Mod",
        "Pitch Mod 1",
        "Freq Mod 1",
        "Note PB",
        "PB Range",
    ] {
        assert!(
            names.iter().any(|held| held == name),
            "{name} is described: {names:?}"
        );
    }
}

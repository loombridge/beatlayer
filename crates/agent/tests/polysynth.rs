//! Drift's LFO, modulation routes and MPE settings through the agent API
//! (WP-2.17, part 2): an agent adds the instrument to a track and reaches
//! every LFO control, every modulation source, destination and amount, and
//! the MPE settings by command.

use bl_agent::Agent;
use bl_agent::client::{Client, InProcess};
use bl_devices::polysynth::{
    CYC_HOLD, CYC_MODE, CYC_RATE, CYC_RATIO, CYC_SYNC, CYC_TILT, CYC_TIME, DRIFT, ENV2_MODE, FREQ,
    GLIDE, HI_QUALITY, HP, LEGATO, LFO_AMOUNT, LFO_DIVISIONS, LFO_MODES, LFO_RATIO, LFO_SHAPES,
    LFO_SYNC, LFO_TIME, MOD_DESTINATIONS, MOD_SOURCES, MOD1_AMOUNT, MOD1_DEST, MOD1_SOURCE,
    NOISE_GAIN, NOTE_PB, OSC1_GAIN, OSC1_SHAPE, OSC2_DETUNE, OSC2_GAIN, PB_RANGE,
    PITCH_MOD1_AMOUNT, PITCH_MOD1_SOURCE, POLYSYNTH, RES, SHAPE_MOD_AMOUNT, SHAPE_MOD_SOURCE,
    SPREAD, STRENGTH, THICKNESS, VOICE_MODE, VOLUME,
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
        (OSC1_SHAPE, 0.5),
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

// Spec: polysynth R35 (every control of Drift completed, by command)
#[test]
fn every_voice_mode_drift_glide_cycle_and_hi_quality_setting_is_reachable_by_command() {
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
    let parameters = |client: &mut Client<InProcess>| -> Value {
        run(client, "device.parameters", json!({ "device": device }))["results"][0]["result"]
            .clone()
    };
    let read = |parameters: &Value, name: &str| -> (f64, bool) {
        let entry = parameters["params"]
            .as_array()
            .expect("the params")
            .iter()
            .find(|entry| entry["name"] == name)
            .unwrap_or_else(|| panic!("{name} is described"));
        (
            entry["value"].as_f64().unwrap(),
            entry["menu_option"].as_bool().unwrap(),
        )
    };

    // A new device plays Poly, with Drift at Live's 7.2 %, Legato off, Glide
    // at 0, Envelope 2 an ADSR and Hi-Quality off, which is a menu option.
    let before = parameters(&mut client);
    assert_eq!(read(&before, "Drift").0, 7.2, "Drift in a new device");
    for name in [
        "Voice Mode",
        "Legato",
        "Glide Time",
        "Env 2 Mode",
        "Hi-Quality",
    ] {
        assert_eq!(read(&before, name).0, 0.0, "{name} in a new device");
    }
    assert!(read(&before, "Hi-Quality").1, "Hi-Quality is a menu option");
    assert_eq!(read(&before, "Spread").0, 50.0);
    assert_eq!(read(&before, "Strength").0, 50.0);
    assert_eq!(read(&before, "Cyc Env Tilt").0, 50.0);
    assert_eq!(read(&before, "Cyc Env Hold").0, 0.0);

    let settings = [
        (VOICE_MODE, 3.0, "Voice Mode"),
        (THICKNESS, 40.0, "Thickness"),
        (SPREAD, 80.0, "Spread"),
        (STRENGTH, 30.0, "Strength"),
        (DRIFT, 65.0, "Drift"),
        (LEGATO, 1.0, "Legato"),
        (GLIDE, 750.0, "Glide Time"),
        (ENV2_MODE, 1.0, "Env 2 Mode"),
        (CYC_TILT, 20.0, "Cyc Env Tilt"),
        (CYC_HOLD, 35.0, "Cyc Env Hold"),
        (CYC_MODE, 2.0, "Cyc Env Mode"),
        (CYC_RATE, 8.0, "Cyc Env Rate"),
        (CYC_RATIO, 2.0, "Cyc Env Ratio"),
        (CYC_TIME, 125.0, "Cyc Env Time"),
        (CYC_SYNC, 4.0, "Cyc Env Sync"),
        (HI_QUALITY, 1.0, "Hi-Quality"),
    ];
    for (id, value, _) in settings {
        run(
            &mut client,
            "device.set_param",
            json!({ "device": device, "param": id, "value": value }),
        );
        assert_eq!(
            held(&client.transport().shell, device, id),
            Some(value),
            "{id:?}"
        );
    }
    // Read back by name, and kept by the set: the device's parameters
    // survive being written out and read in.
    let after = parameters(&mut client);
    for (_, value, name) in settings {
        assert_eq!(read(&after, name).0, value, "{name} read back");
    }
    let saved = client
        .transport()
        .shell
        .editor()
        .document()
        .device(device)
        .cloned()
        .expect("the device");
    let reopened: bl_model::Device =
        serde_json::from_value(serde_json::to_value(&saved).unwrap()).unwrap();
    for (id, value, name) in settings {
        assert_eq!(reopened.params.get(&id), Some(&value), "{name} saved");
    }
    // Cyc Env Rate is a destination of the Mod section, after Main Volume.
    run(
        &mut client,
        "device.set_param",
        json!({
            "device": device,
            "param": MOD1_DEST,
            "value": (MOD_DESTINATIONS.len() - 1) as f64,
        }),
    );
    assert_eq!(MOD_DESTINATIONS[MOD_DESTINATIONS.len() - 1], "Cyc Env Rate");
    // Hi-Quality is one undo step.
    let steps = client.transport().shell.editor().undo_history().len();
    run(
        &mut client,
        "device.set_param",
        json!({ "device": device, "param": HI_QUALITY, "value": 0.0 }),
    );
    assert_eq!(
        client.transport().shell.editor().undo_history().len(),
        steps + 1
    );
    run(&mut client, "edit.undo", json!({}));
    assert_eq!(
        held(&client.transport().shell, device, HI_QUALITY),
        Some(1.0)
    );
}

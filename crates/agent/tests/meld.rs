//! Meld through the agent API (WP-8.17; the meld spec, R2 to R5, R14 and
//! R20): an agent adds the instrument to a track, sets and reads back every
//! parameter by ID, reads the macros under the type's names, and shows an
//! engine's page.

use bl_agent::Agent;
use bl_agent::client::{Client, InProcess};
use bl_devices::meld::{MACRO_1, MELD, TYPE, meld_type, param};
use bl_model::{DeviceId, TrackKind};
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

// Spec: meld R2 to R5, R20
#[test]
fn an_agent_sets_and_reads_back_every_parameter_and_reads_the_macros_by_type() {
    let mut client = client();
    let track = client
        .transport()
        .shell
        .editor()
        .document()
        .tracks()
        .find(|track| track.kind() == TrackKind::Midi)
        .map(bl_model::Track::id)
        .expect("the default set has a MIDI track");
    let added = run(
        &mut client,
        "device.add",
        json!({
            "device": {
                "kind": { "native": { "device_type": MELD } },
                "name": "Meld",
                "role": "instrument",
            },
            "track": track,
        }),
    );
    let device: DeviceId = serde_json::from_value(added["results"][0]["result"]["device"].clone())
        .expect("the new device");
    // Every parameter, moved to the far end of its range and read back.
    let mut wanted = Vec::new();
    for p in &meld_type().description.params {
        let to = if p.default == p.max() {
            p.min()
        } else {
            p.max()
        };
        run(
            &mut client,
            "device.set_param",
            json!({ "device": device, "param": p.id, "value": to }),
        );
        wanted.push((p.id, to));
    }
    let read = run(
        &mut client,
        "device.parameters",
        json!({ "device": device }),
    );
    let params = read["results"][0]["result"]["params"].as_array().unwrap();
    for (id, to) in wanted {
        let state = params
            .iter()
            .find(|p| p["id"] == json!(id))
            .unwrap_or_else(|| panic!("{id:?} is read back"));
        assert_eq!(state["value"], json!(to), "{}", state["name"]);
    }
    // The type picks the macros' names; the IDs stay.
    run(
        &mut client,
        "device.set_param",
        json!({ "device": device, "param": param(0, TYPE), "value": 5.0 }),
    );
    let read = run(
        &mut client,
        "device.parameters",
        json!({ "device": device }),
    );
    let params = read["results"][0]["result"]["params"].as_array().unwrap();
    let macro_1 = params
        .iter()
        .find(|p| p["id"] == json!(param(0, MACRO_1)))
        .unwrap();
    assert_eq!(macro_1["name"], "Tone", "Sub's first macro");
    // The engine selector is a page the agent shows.
    run(
        &mut client,
        "arrangement.select_tracks",
        json!({ "tracks": [track] }),
    );
    let shown = run(
        &mut client,
        "device.set_page",
        json!({ "device": device, "node": "Engine", "page": "B" }),
    );
    assert_eq!(
        shown["results"][0]["result"]["devices"][0]["panel"]["pages"][0]["shown"],
        "B"
    );
}

fn matrix(client: &mut Client<InProcess>, device: DeviceId) -> Value {
    client
        .call("query", json!({ "id": device }))
        .unwrap()
        .ok()
        .unwrap_or_else(|error| panic!("query failed: {error}"))["matrix"]
        .clone()
}

// Spec: meld R62 to R64, R67, R73
#[test]
fn an_agent_routes_by_engine_names_copies_and_clears_reads_the_matrix_and_the_displays() {
    let mut client = client();
    let track = client
        .transport()
        .shell
        .editor()
        .document()
        .tracks()
        .find(|track| track.kind() == TrackKind::Midi)
        .map(bl_model::Track::id)
        .expect("the default set has a MIDI track");
    let added = run(
        &mut client,
        "device.add",
        json!({
            "device": {
                "kind": { "native": { "device_type": MELD } },
                "name": "Meld",
                "role": "instrument",
            },
            "track": track,
        }),
    );
    let device: DeviceId = serde_json::from_value(added["results"][0]["result"]["device"].clone())
        .expect("the new device");
    let before = matrix(&mut client, device);
    assert_eq!(before["slots"].as_array().unwrap().len(), 64);
    let names = |key: &str| -> Vec<String> {
        before[key]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["name"].as_str().unwrap().to_owned())
            .collect()
    };
    let (sources, destinations) = (names("sources"), names("destinations"));
    for name in ["A LFO 1 FX", "B Amp Env", "MPE Slide", "Spread"] {
        assert!(sources.contains(&name.to_owned()), "{name}");
    }
    for name in [
        "A Filter Frequency",
        "B Filter Frequency",
        "B Pitch Quant",
        "Mixer Drive",
    ] {
        assert!(destinations.contains(&name.to_owned()), "{name}");
    }
    // Routes by name, read back, and refused across the engines.
    for (source, destination, amount) in [
        ("A LFO 1", "A Filter Frequency", 40.0),
        ("A Mod Env", "A Pitch Quant", -20.0),
    ] {
        run(
            &mut client,
            "device.set_route",
            json!({ "device": device, "source": source, "destination": destination, "amount": amount }),
        );
    }
    let refused = client
        .call(
            "run",
            json!({ "commands": [{ "command": "device.set_route", "args": {
                "device": device, "source": "A LFO 1", "destination": "B Filter Frequency",
            } }] }),
        )
        .unwrap()
        .ok();
    assert!(
        refused.is_err(),
        "an engine's modulator doesn't reach the other engine"
    );
    let copied = run(
        &mut client,
        "meld.copy_modulation",
        json!({ "device": device, "engine": "A" }),
    );
    assert_eq!(copied["results"][0]["result"]["routes"], 2);
    let slots = matrix(&mut client, device)["slots"].clone();
    let route = |source: &str, destination: &str| {
        slots
            .as_array()
            .unwrap()
            .iter()
            .find(|slot| slot["source"] == source && slot["destination"] == destination)
            .cloned()
            .unwrap_or_else(|| panic!("{source} to {destination} is a slot"))
    };
    assert_eq!(route("B LFO 1", "B Filter Frequency")["amount"], 40.0);
    assert_eq!(route("B Mod Env", "B Pitch Quant")["amount"], -20.0);
    let cleared = run(
        &mut client,
        "meld.clear_modulation",
        json!({ "device": device, "engine": "B" }),
    );
    assert_eq!(cleared["results"][0]["result"]["routes"], 2);
    // The displays an agent reads headless: every series, and a reading each.
    let shown = run(&mut client, "device.display", json!({ "device": device }));
    let display = &shown["results"][0]["result"];
    let series: Vec<&str> = display["series"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    for name in [
        "A Oscillator",
        "B Filter",
        "A Amp Envelope",
        "B Mod Envelope",
        "A LFO 1 FX",
    ] {
        assert!(series.contains(&name), "{name}");
    }
    assert!(
        display["readings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["name"] == "A Oscillator"
                && r["text"].as_str().unwrap().starts_with("Basic Shapes"))
    );
}

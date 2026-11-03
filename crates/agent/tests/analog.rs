//! Analog's controls through the agent API (WP-8.14, part 1): an agent adds
//! the instrument, sets and reads back every parameter, switches the shell's
//! sections by name, and finds the pages in the device view's state.

use bl_agent::Agent;
use bl_agent::client::{Client, InProcess};
use bl_devices::analog::{ANALOG, PAGES, SECTIONS, analog_type};
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

// Spec: analog R16, R17 (every control as a command, the sections as pages)
#[test]
fn every_parameter_is_set_and_read_back_and_the_sections_switch_by_name() {
    let mut client = client();
    let track = audio_track(&client.transport().shell);
    let added = run(
        &mut client,
        "device.add",
        json!({
            "device": {
                "kind": { "native": { "device_type": ANALOG } },
                "name": "Analog",
                "role": "instrument",
            },
            "track": track,
        }),
    );
    let device: DeviceId = serde_json::from_value(added["results"][0]["result"]["device"].clone())
        .expect("the new device");
    let description = analog_type().description;
    for param in &description.params {
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
        let held = client
            .transport()
            .shell
            .editor()
            .document()
            .device(device)
            .and_then(|device| device.params.get(&param.id).copied());
        assert_eq!(held, Some(to), "{}", param.name);
    }

    // The instrument describes every control to an agent.
    let described = run(
        &mut client,
        "device.parameters",
        json!({ "device": device }),
    );
    let names: Vec<String> = described["results"][0]["result"]["params"]
        .as_array()
        .expect("the params")
        .iter()
        .filter_map(|entry| entry["name"].as_str().map(str::to_owned))
        .collect();
    for param in &description.params {
        assert!(names.contains(&param.name), "{} is described", param.name);
    }

    // The shell's sections: the first shows in a new device, each can be
    // chosen by name, and the device view reads the choice back.
    let shown = |client: &mut Client<InProcess>| {
        let state = run(client, "device_view.state", json!({}));
        let device = state["results"][0]["result"]["devices"]
            .as_array()
            .and_then(|devices| {
                devices
                    .iter()
                    .find(|d| d["panel"]["pages"][0]["node"] == PAGES)
            })
            .cloned()
            .expect("Analog's pages are in the device view");
        device["panel"]["pages"][0]["shown"].clone()
    };
    assert_eq!(shown(&mut client), SECTIONS[0]);
    for section in SECTIONS {
        run(
            &mut client,
            "device.set_page",
            json!({ "device": device, "node": PAGES, "page": section }),
        );
        assert_eq!(shown(&mut client), section);
    }
}

fn added(client: &mut Client<InProcess>) -> DeviceId {
    let track = audio_track(&client.transport().shell);
    let added = run(
        client,
        "device.add",
        json!({
            "device": {
                "kind": { "native": { "device_type": ANALOG } },
                "name": "Analog",
                "role": "instrument",
            },
            "track": track,
        }),
    );
    serde_json::from_value(added["results"][0]["result"]["device"].clone()).expect("the device")
}

fn value(client: &mut Client<InProcess>, device: DeviceId, param: ParamId) -> f64 {
    client
        .transport()
        .shell
        .editor()
        .document()
        .device(device)
        .and_then(|d| d.params.get(&param).copied())
        .expect("the parameter has a value")
}

// Spec: analog R22 (Quick Routing, with a readback)
#[test]
fn an_agent_sets_quick_routing_and_reads_the_parameters_back() {
    use bl_devices::analog::{amp, filter};
    let mut client = client();
    let device = added(&mut client);
    let reply = run(
        &mut client,
        "analog.quick_routing",
        json!({ "device": device, "routing": "serial" }),
    );
    assert_eq!(reply["results"][0]["result"]["routing"], "serial");
    assert_eq!(
        value(&mut client, device, filter::id(0, filter::EXTRA)),
        100.0
    );
    assert_eq!(value(&mut client, device, amp::id(0, amp::ON)), 0.0);
    assert_eq!(value(&mut client, device, amp::id(1, amp::ON)), 1.0);
    // The readback the command names: the device's parameters.
    let described = run(
        &mut client,
        "device.parameters",
        json!({ "device": device }),
    );
    let to_f2 = described["results"][0]["result"]["params"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "Filter 1 To F2")
        .unwrap()
        .clone();
    assert_eq!(to_f2["value"], 100.0);
    // A routing nobody knows is refused.
    let refused = client
        .call(
            "run",
            json!({ "commands": [{ "command": "analog.quick_routing", "args": { "device": device, "routing": "diagonal" } }] }),
        )
        .unwrap();
    assert!(refused.ok().is_err(), "an unknown routing is refused");
    assert_eq!(
        value(&mut client, device, filter::id(0, filter::EXTRA)),
        100.0
    );
}

// Spec: analog R24, R25 (the MPE routings are chosen slots of the matrix)
#[test]
fn an_agent_routes_pressure_and_slide_and_reads_the_matrix_back() {
    use bl_devices::analog::{DESTINATIONS, MPE};
    let mut client = client();
    let device = added(&mut client);
    let matrix = |client: &mut Client<InProcess>| -> Value {
        client
            .call("query", json!({ "id": device }))
            .unwrap()
            .ok()
            .unwrap_or_else(|error| panic!("query failed: {error}"))["matrix"]
            .clone()
    };
    let before = matrix(&mut client);
    assert_eq!(before["slots"].as_array().unwrap().len(), 4);
    assert_eq!(before["sources"][0]["name"], "Pressure");
    assert_eq!(before["sources"][1]["name"], "Slide");
    assert_eq!(
        before["destinations"].as_array().unwrap().len(),
        DESTINATIONS.len()
    );
    assert_eq!(before["slots"][0]["kind"], "chosen");
    assert_eq!(before["slots"][0]["destination"], Value::Null, "Off");
    // Slide's second slot to Amp 2 Level at -40 %, by its choosers.
    let dest = 1.0
        + DESTINATIONS
            .iter()
            .position(|d| *d == "Amp 2 Level")
            .unwrap() as f64;
    for (param, to) in [(MPE[3].0, dest), (MPE[3].1, -40.0)] {
        run(
            &mut client,
            "device.set_param",
            json!({ "device": device, "param": param, "value": to }),
        );
    }
    let after = matrix(&mut client);
    let slot = &after["slots"][3];
    assert_eq!(slot["source"], "Slide");
    assert_eq!(slot["destination"], "Amp 2 Level");
    assert_eq!(slot["amount"], -40.0);
    // A chosen slot isn't set by the route commands.
    let refused = client
        .call(
            "run",
            json!({ "commands": [{ "command": "device.set_route", "args": { "device": device, "slot": 0, "source": "Pressure", "destination": "F1 Freq" } }] }),
        )
        .unwrap();
    assert!(refused.ok().is_err(), "a chosen slot is refused");
}

// Spec: analog R26 (the display's data, headless)
#[test]
fn an_agent_reads_the_sections_displays() {
    let mut client = client();
    let device = added(&mut client);
    let shown = run(&mut client, "device.display", json!({ "device": device }));
    let series: Vec<String> = shown["results"][0]["result"]["series"]
        .as_array()
        .expect("the series")
        .iter()
        .map(|s| s["name"].as_str().unwrap().to_owned())
        .collect();
    for name in [
        "Filter 1 Envelope",
        "Amp 2 Envelope",
        "Filter 2 Response",
        "LFO 1 Wave",
    ] {
        assert!(series.iter().any(|s| s == name), "{name} in {series:?}");
    }
}

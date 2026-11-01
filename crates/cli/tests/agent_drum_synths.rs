//! The Drum Synths through the agent API (WP-8.24; the drum synths spec, R6,
//! R7): an agent adds each device, sets its parameters by ID, reads them
//! back with `device.parameters`, reads its display with `device.display`
//! and plays a hit with `device.preview_hit`, which changes nothing in the
//! set and is no undo step.

use bl_agent::Agent;
use bl_agent::client::{Client, InProcess, Transport};
use bl_shell::Shell;
use serde_json::{Value, json};

fn call(client: &mut Client<impl Transport>, tool: &str, args: Value) -> Value {
    client
        .call(tool, args)
        .expect("the call reaches the server")
        .ok()
        .unwrap_or_else(|error| panic!("{tool} failed: {error}"))
}

/// Runs one command and returns its result.
fn run(client: &mut Client<impl Transport>, command: &str, args: Value) -> Value {
    let done = call(
        client,
        "run",
        json!({ "commands": [{ "command": command, "args": args }] }),
    );
    done["results"][0]["result"].clone()
}

/// The ID of the one track a `track.create` result lists.
fn new_track(made: &Value) -> Value {
    let (id, _) = made["tracks"].as_object().unwrap().iter().next().unwrap();
    json!(id.parse::<u64>().unwrap())
}

/// Whether the command is refused.
fn refused(client: &mut Client<impl Transport>, command: &str, args: Value) -> bool {
    client
        .call(
            "run",
            json!({ "commands": [{ "command": command, "args": args }] }),
        )
        .expect("the call reaches the server")
        .ok()
        .is_err()
}

// Spec: drum-synths R6, R7
#[test]
fn an_agent_sets_every_drum_synth_parameter_reads_its_display_and_previews_a_hit() {
    let devices = [
        bl_devices::ds_kick::ds_kick_type(),
        bl_devices::ds_snare::ds_snare_type(),
        bl_devices::ds_tom::ds_tom_type(),
        bl_devices::ds_clap::ds_clap_type(),
    ];
    let mut shell = Shell::headless();
    shell.set_device_types(bl_devices::device_types());
    let mut client = Client::new(InProcess::new(Agent::headless(true), shell));
    client.initialize("drum-synths-agent").unwrap();
    for device_type in devices {
        let name = device_type.name.clone();
        // An instrument each needs a MIDI track of its own.
        let made = run(
            &mut client,
            "track.create",
            json!({ "id": { "$new": format!("{name} track") }, "kind": "midi", "name": name, "index": 0 }),
        );
        let track = new_track(&made);
        let bl_model::DeviceKind::Native { device_type: id } = &device_type.kind else {
            panic!("{name} is native");
        };
        let added = run(
            &mut client,
            "device.add",
            json!({ "track": track,
                    "device": { "kind": { "native": { "device_type": id } },
                                "name": name, "role": "instrument" } }),
        );
        let device = added["device"].clone();
        // Every parameter, moved off its default and read back.
        let description = &device_type.description;
        let wanted: Vec<(bl_model::ParamId, f64)> = description
            .params
            .iter()
            .map(|p| {
                let (lo, hi) = (p.min(), p.max());
                let value = if (p.default - hi).abs() > (p.default - lo).abs() {
                    lo + 0.75 * (hi - lo)
                } else {
                    lo + 0.25 * (hi - lo)
                };
                (p.id, p.clamp(value))
            })
            .collect();
        for (param, value) in &wanted {
            run(
                &mut client,
                "device.set_param",
                json!({ "device": device, "param": param, "value": value }),
            );
        }
        let read = run(
            &mut client,
            "device.parameters",
            json!({ "device": device }),
        );
        let params = read["params"].as_array().unwrap();
        assert_eq!(params.len(), description.params.len(), "{name}");
        for (param, value) in &wanted {
            let found = params
                .iter()
                .find(|p| p["id"] == json!(param))
                .unwrap_or_else(|| panic!("{name}: parameter {param:?} is listed"));
            let got = found["value"].as_f64().unwrap();
            assert!(
                (got - value).abs() < 1e-6 * value.abs().max(1.0),
                "{name}: {got} for {value}"
            );
        }

        // The display draws the envelope, from the parameters.
        let display = run(&mut client, "device.display", json!({ "device": device }));
        let series = display["series"].as_array().unwrap();
        assert_eq!(series[0]["name"], "Envelope", "{name}");
        assert!(series[0]["values"].as_array().unwrap().len() > 100);

        // A preview is a read: it changes nothing and is no undo step. With
        // no engine running nothing plays, and the result says so.
        let before = client.transport().shell.editor().history().len();
        let document = client.transport().shell.editor().document().clone();
        let sent = run(
            &mut client,
            "device.preview_hit",
            json!({ "device": device }),
        );
        assert_eq!(sent["sent"], false, "{name}: no engine runs");
        assert_eq!(client.transport().shell.editor().history().len(), before);
        assert_eq!(*client.transport().shell.editor().document(), document);
    }

    // A device that isn't a Drum Synth is refused.
    let made = run(
        &mut client,
        "track.create",
        json!({ "id": { "$new": "sampler track" }, "kind": "midi", "name": "Sampler", "index": 0 }),
    );
    let track = new_track(&made);
    let added = run(
        &mut client,
        "device.add",
        json!({ "track": track,
                "device": { "kind": { "native": { "device_type": bl_devices::DRUM_SAMPLER } },
                            "name": "Drum Sampler", "role": "instrument" } }),
    );
    assert!(refused(
        &mut client,
        "device.preview_hit",
        json!({ "device": added["device"] })
    ));
}

//! Collision through the agent API (WP-8.21; the collision spec, R10, R14 and
//! R17): an agent adds the instrument to a track, sets and reads back every
//! parameter by ID, copies a resonator, and reads the matrix's routes.

use bl_agent::Agent;
use bl_agent::client::{Client, InProcess};
use bl_devices::collision::{
    COLLISION, L_AMOUNT_A, L_DEST_A, L_ON, R_DECAY, R_TUNE, collision_type, destination, lfo, res,
};
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

fn parameters(client: &mut Client<InProcess>, device: DeviceId) -> Vec<Value> {
    let read = run(client, "device.parameters", json!({ "device": device }));
    read["results"][0]["result"]["params"]
        .as_array()
        .unwrap()
        .clone()
}

// Spec: collision R10, R14, R17
#[test]
fn an_agent_sets_and_reads_back_every_parameter_copies_a_resonator_and_reads_the_routes() {
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
                "kind": { "native": { "device_type": COLLISION } },
                "name": "Collision",
                "role": "instrument",
            },
            "track": track,
        }),
    );
    let device: DeviceId = serde_json::from_value(added["results"][0]["result"]["device"].clone())
        .expect("the new device");
    // Every parameter, moved to the far end of its range and read back.
    let mut wanted = Vec::new();
    for p in &collision_type().description.params {
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
    let params = parameters(&mut client, device);
    for (id, to) in wanted {
        let state = params
            .iter()
            .find(|p| p["id"] == json!(id))
            .unwrap_or_else(|| panic!("{id:?} is read back"));
        assert_eq!(state["value"], json!(to), "{}", state["name"]);
    }
    // Copy: Resonator 2 takes Resonator 1's settings, and they read back.
    run(
        &mut client,
        "device.set_param",
        json!({ "device": device, "param": res(0, R_TUNE), "value": 7.0 }),
    );
    run(
        &mut client,
        "collision.copy_resonator",
        json!({ "device": device, "from": 1 }),
    );
    let params = parameters(&mut client, device);
    let value = |id| params.iter().find(|p| p["id"] == json!(id)).unwrap()["value"].clone();
    assert_eq!(value(res(1, R_TUNE)), json!(7.0));
    assert_eq!(value(res(1, R_DECAY)), value(res(0, R_DECAY)));
    // A chosen slot's destination and amount are parameters, and the query
    // tool's `matrix` reads the route they make.
    run(
        &mut client,
        "device.set_param",
        json!({ "device": device, "param": lfo(0, L_ON), "value": 1.0 }),
    );
    run(
        &mut client,
        "device.set_param",
        json!({ "device": device, "param": lfo(0, L_DEST_A), "value": destination(res(0, R_TUNE)) }),
    );
    run(
        &mut client,
        "device.set_param",
        json!({ "device": device, "param": lfo(0, L_AMOUNT_A), "value": 40.0 }),
    );
    let params = parameters(&mut client, device);
    let state = params
        .iter()
        .find(|p| p["id"] == json!(lfo(0, L_DEST_A)))
        .unwrap();
    assert_eq!(
        state["text"], "Res 1 Tune",
        "the chooser reads as the destination"
    );
}

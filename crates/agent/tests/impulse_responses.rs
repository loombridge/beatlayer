//! Impulse-response slots through the agent API (WP-8.30; the impulse
//! responses spec, R10 and R12): an agent loads a response into the
//! convolution test device by command ID, chooses another from the
//! chooser, clears the slot, and reads every effect back from
//! `device.impulse_responses` and the query tool's `device` result.

use bl_agent::Agent;
use bl_agent::client::{Client, InProcess};
use bl_shell::Shell;
use serde_json::{Value, json};

fn client() -> Client<InProcess> {
    let mut shell = Shell::headless();
    shell.set_device_types(vec![bl_devices::testing::convolution_type()]);
    let mut client = Client::new(InProcess::new(Agent::headless(true), shell));
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
        .unwrap_or_else(|error| panic!("{command} failed: {error}"))["results"][0]["result"]
        .clone()
}

fn query(client: &mut Client<InProcess>, id: &Value) -> Value {
    client
        .call("query", json!({ "id": id }))
        .unwrap()
        .ok()
        .unwrap_or_else(|error| panic!("query failed: {error}"))
}

/// Writes a short stereo WAV of a decaying tone.
fn write(path: &std::path::Path, decay: f32) {
    let samples: Vec<f32> = (0..4800 * 2)
        .map(|i| ((i / 2) as f32 * 0.1).sin() * (-((i / 2) as f32) / decay).exp())
        .collect();
    let bytes = bl_media::wav_bytes(
        &samples,
        2,
        48_000,
        bl_media::BitDepth::Int16,
        bl_media::Dither::None,
    )
    .unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

// Spec: impulse-responses R10, R12
#[test]
fn an_agent_chooses_a_response_clears_it_and_reads_both_back() {
    let dir = std::env::temp_dir().join(format!("bl-agent-ir-{}", std::process::id()));
    let (room, plate) = (dir.join("Room.wav"), dir.join("Plate.wav"));
    write(&room, 800.0);
    write(&plate, 1200.0);
    let mut client = client();
    let tracks = client
        .call("query", json!({ "path": "tracks" }))
        .unwrap()
        .ok()
        .unwrap();
    let audio = tracks["tracks"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["kind"] == "audio")
        .expect("an audio track")["id"]
        .clone();
    run(
        &mut client,
        "arrangement.select_tracks",
        json!({ "tracks": [audio] }),
    );
    let convolution = bl_devices::testing::convolution_type();
    let added = run(
        &mut client,
        "device.add",
        json!({ "device": {
            "kind": convolution.kind, "name": convolution.name, "role": convolution.role,
        } }),
    );
    let device = added["device"].clone();
    assert!(
        query(&mut client, &device)
            .get("impulse_responses")
            .is_some()
    );
    assert_eq!(
        query(&mut client, &device)["impulse_responses"][0]["state"],
        "empty"
    );

    let loaded = run(
        &mut client,
        "device.load_sample",
        json!({ "device": device, "path": room.to_string_lossy() }),
    );
    assert_eq!(loaded["impulse_response"]["layout"], "stereo");
    let read = query(&mut client, &device);
    let slot = &read["impulse_responses"][0];
    assert_eq!(slot["state"], "ready");
    assert_eq!(slot["file"], "Room.wav");
    assert_eq!(slot["category"], "User");
    assert_eq!(slot["file_frames"], 4800);

    run(
        &mut client,
        "device.load_sample",
        json!({ "device": device, "path": plate.to_string_lossy(), "slot": 0 }),
    );
    let chooser = run(
        &mut client,
        "device.impulse_responses",
        json!({ "device": device }),
    );
    assert_eq!(chooser["slots"][0]["file"], "Plate.wav");
    assert_eq!(
        chooser["categories"].as_array().unwrap().last().unwrap()["name"],
        "User"
    );

    run(
        &mut client,
        "device.set_sample",
        json!({ "device": device, "media": null }),
    );
    assert_eq!(
        query(&mut client, &device)["impulse_responses"][0]["state"],
        "empty"
    );
    let _ = std::fs::remove_dir_all(dir);
}

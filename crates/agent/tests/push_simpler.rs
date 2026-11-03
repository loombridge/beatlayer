//! Simpler on Push 2 through the agent API (WP-7.24; `docs/specs/push-2-notes.md`,
//! R67 to R76): `push.state` reads Simpler's bank, Zoom, Edit Mode and the
//! selected slice, and `push.convert` makes what Convert does and reads back
//! in the song's tracks.

use bl_agent::Agent;
use bl_agent::client::{Client, InProcess};
use bl_core::clock::TestClock;
use bl_midi_io::TestBackend;
use bl_model::{DeviceId, TrackKind};
use bl_push::sim::Simulated;
use bl_shell::Shell;
use serde_json::{Value, json};

fn run(client: &mut Client<InProcess>, command: &str, args: Value) -> Value {
    let reply = client
        .call(
            "run",
            json!({ "commands": [{ "command": command, "args": args }] }),
        )
        .unwrap();
    assert!(!reply.is_error, "{command}: {}", reply.text);
    reply.ok().unwrap()["results"][0]["result"].clone()
}

// Spec: push-2-notes R67, R70, R76. An agent reads Simpler's main bank and its
// Edit Mode from `push.state`, then converts it and reads the new track back.
#[test]
fn an_agent_reads_simplers_bank_and_edit_mode_and_converts_it() {
    let dir = std::env::temp_dir().join(format!("bl-agent-push-simpler-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let samples: Vec<f32> = (0..48_000).map(|n| (n as f32 * 0.05).sin() * 0.5).collect();
    let bytes = bl_media::wav_bytes(
        &samples,
        1,
        48_000,
        bl_media::BitDepth::Int16,
        bl_media::Dither::None,
    )
    .unwrap();
    let file = dir.join("tone.wav");
    std::fs::write(&file, bytes).unwrap();

    let test = TestBackend::new();
    let clock = TestClock::new();
    let mut sim = Simulated::new(&test, &clock);
    let mut shell = Shell::headless();
    shell.use_midi_backend(Box::new(test.clone()));
    shell.use_test_clock(clock.clone());
    shell.use_push_display(sim.frames().claim());
    let track = shell
        .editor()
        .document()
        .tracks()
        .find(|t| t.kind() == TrackKind::Midi)
        .map(bl_model::Track::id)
        .unwrap();
    shell
        .invoke("arrangement.select_tracks", json!({ "tracks": [track] }))
        .unwrap();
    let added = shell
        .invoke(
            "device.add",
            json!({ "device": {
                "kind": { "native": { "device_type": bl_devices::sampler::SAMPLER } },
                "name": "Simpler",
                "role": "instrument",
            } }),
        )
        .unwrap();
    let simpler: DeviceId = serde_json::from_value(added["device"].clone()).unwrap();
    shell
        .run(
            "device.load_sample",
            json!({ "device": simpler, "path": file.to_string_lossy() }),
        )
        .unwrap();
    sim.plug_in();
    shell.refresh_midi();
    let mut client = Client::new(InProcess::new(Agent::headless(true), shell));
    client.initialize("test-agent").unwrap();
    let settle = |client: &mut Client<InProcess>, sim: &mut Simulated| {
        for _ in 0..6 {
            client.transport().shell.poll();
            client.transport().shell.midi().surfaces().tick();
            loop {
                run(client, "control_surface.list", json!({}));
                if sim.pump() == 0 {
                    break;
                }
            }
            clock.advance(std::time::Duration::from_millis(30));
        }
    };
    settle(&mut client, &mut sim);

    let state = run(&mut client, "push.state", json!({}));
    let device = &state["pushes"][0]["device"];
    assert_eq!(device["bank"], 0, "{state}");
    assert_eq!(device["banks"][0], "Simpler");
    assert_eq!(device["banks"][1], "Global");
    let simpler_state = &device["simpler"];
    assert_eq!(simpler_state["mode"], "classic");
    assert_eq!(simpler_state["main_bank"], true);
    assert_eq!(simpler_state["zoom"], "×1.0");
    assert_eq!(simpler_state["zoom_around"], "start");
    assert_eq!(simpler_state["region"], json!([0, 48_000]));
    assert_eq!(simpler_state["edit_buttons"], json!([]));

    // Edit Mode, by the second tap on the device's button.
    assert!(sim.press("upper_1"));
    settle(&mut client, &mut sim);
    assert!(sim.let_go("upper_1"));
    settle(&mut client, &mut sim);
    let state = run(&mut client, "push.state", json!({}));
    let simpler_state = &state["pushes"][0]["device"]["simpler"];
    assert_eq!(state["pushes"][0]["device"]["edit"], true, "{state}");
    assert_eq!(simpler_state["edit_buttons"][0]["name"], "loop");
    assert_eq!(simpler_state["edit_buttons"][1]["name"], "warp_as");

    // Convert, by ID, and the song reads the new track back.
    let tracks_before = client.call("query", json!({})).unwrap().ok().unwrap();
    let converted = run(&mut client, "push.convert", json!({}));
    assert_eq!(converted["from"], "simpler");
    let tracks_after = client.call("query", json!({})).unwrap().ok().unwrap();
    assert_ne!(tracks_before, tracks_after, "a track was made");
    let new = converted["track"].clone();
    let read = client
        .call("query", json!({ "id": new }))
        .unwrap()
        .ok()
        .unwrap();
    assert_eq!(
        read["devices"][0]["kind"],
        json!({ "rack": { "kind": "drum" } }),
        "{read}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

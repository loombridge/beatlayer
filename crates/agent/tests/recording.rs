//! Take lanes and Warp recorded audio through the agent API (WP-2.13): an
//! agent adds a take lane to an audio track and reads it back, a MIDI track
//! refuses one, and it switches Warp recorded audio as the Settings window's
//! switch does. The rules are in the recording spec.

use bl_agent::Agent;
use bl_agent::client::{Client, InProcess};
use bl_shell::Shell;
use serde_json::{Value, json};

fn call(client: &mut Client<InProcess>, tool: &str, args: Value) -> Value {
    client
        .call(tool, args)
        .unwrap()
        .ok()
        .unwrap_or_else(|error| panic!("{tool} failed: {error}"))
}

fn headless() -> Client<InProcess> {
    let mut client = Client::new(InProcess::new(Agent::headless(false), Shell::headless()));
    client.initialize("test-agent").unwrap();
    client
}

// Spec: recording R12
#[test]
fn an_agent_adds_a_take_lane_and_reads_it_back() {
    let mut client = headless();
    let result = call(
        &mut client,
        "run",
        json!({ "commands": [
            { "command": "track.create",
              "args": { "id": {"$new": "audio"}, "kind": "audio", "name": "Guitar", "index": 0 } },
            { "command": "track.create",
              "args": { "id": {"$new": "midi"}, "kind": "midi", "name": "Keys", "index": 1 } },
            { "command": "track.add_take_lane",
              "args": { "track": {"$ref": "audio"},
                        "lane": { "name": "Take 1", "clips": [
                            { "name": "Guitar 0001", "start": 705_600, "length": 705_600,
                              "data": { "media": "ab".repeat(32), "start": 0.5, "end": 1.0 } },
                        ] } } },
        ] }),
    );
    assert_eq!(result["ran"], 3);
    let audio = result["ids"]["audio"].as_u64().unwrap();

    let track = call(
        &mut client,
        "query",
        json!({ "path": format!("track {audio}") }),
    );
    let lane = &track["take_lanes"][0];
    assert_eq!(lane["name"], "Take 1");
    assert_eq!(lane["clips"][0]["name"], "Guitar 0001");
    assert_eq!(lane["clips"][0]["length"]["ticks"], 705_600);
    assert_eq!(track["clips"]["total"], 0, "the main lane is untouched");

    // The step is one undo, and a MIDI track has no take lanes to add to.
    let midi = result["ids"]["midi"].as_u64().unwrap();
    let refused = client
        .call(
            "run",
            json!({ "commands": [{ "command": "track.add_take_lane",
                "args": { "track": midi, "lane": { "name": "Take 1", "clips": [] } } }] }),
        )
        .unwrap();
    assert!(refused.is_error, "{refused:?}");
}

// Spec: recording R13
#[test]
fn an_agent_switches_warp_recorded_audio() {
    let mut client = headless();
    let described = call(
        &mut client,
        "describe_command",
        json!({ "command": "settings.set_warp_recorded_audio" }),
    );
    assert_eq!(described["label"], "Warp Recorded Audio");
    assert_eq!(described["permission"], "edit");
    assert_eq!(described["access"], "runs");

    // On, as Live warps its recordings.
    assert!(client.transport().shell.warp_recorded_audio());
    let result = call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "settings.set_warp_recorded_audio",
                               "args": { "on": false } }] }),
    );
    assert_eq!(result["ran"], 1);
    assert!(!client.transport().shell.warp_recorded_audio());
    call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "settings.set_warp_recorded_audio",
                               "args": { "on": true } }] }),
    );
    assert!(client.transport().shell.warp_recorded_audio());
}

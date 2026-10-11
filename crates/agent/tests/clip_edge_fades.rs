//! Create Fades on Clip Edges through the agent API (WP-1.4b): an agent
//! reads its description, switches it with `run`, as the Settings window's
//! switch does, and the clips it adds from the browser from then on get
//! 4 ms fades at their edges. The rules are in the audio clips spec.

use std::path::PathBuf;

use bl_agent::Agent;
use bl_agent::client::{Client, InProcess};
use bl_model::{ClipData, EDGE_FADE_SECONDS, TrackKind};
use bl_shell::Shell;
use serde_json::{Value, json};

fn call(client: &mut Client<InProcess>, tool: &str, args: Value) -> Value {
    client
        .call(tool, args)
        .unwrap()
        .ok()
        .unwrap_or_else(|error| panic!("{tool} failed: {error}"))
}

/// A tenth of a second of a quiet 16-bit mono WAV at 48 kHz, in a file of
/// its own under the system's temporary folder.
fn wav() -> PathBuf {
    let path = std::env::temp_dir().join(format!("bl-agent-fades-{}.wav", std::process::id()));
    let frames: u32 = 4_800;
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + frames * 2).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    for half in [1u16, 1] {
        bytes.extend_from_slice(&half.to_le_bytes());
    }
    bytes.extend_from_slice(&48_000u32.to_le_bytes());
    bytes.extend_from_slice(&96_000u32.to_le_bytes());
    for half in [2u16, 16] {
        bytes.extend_from_slice(&half.to_le_bytes());
    }
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&(frames * 2).to_le_bytes());
    for i in 0..frames {
        bytes.extend_from_slice(&(((i % 48) as i16 - 24) * 2).to_le_bytes());
    }
    std::fs::write(&path, bytes).unwrap();
    path
}

/// Adds the file at 1.1.1 on the first audio track, through the agent API,
/// and returns the new clip's fades.
fn add(client: &mut Client<InProcess>, file: &PathBuf) -> (f64, f64) {
    let track = client
        .transport()
        .shell
        .editor()
        .document()
        .tracks()
        .find(|t| t.kind() == TrackKind::Audio)
        .unwrap()
        .id();
    let result = call(
        client,
        "run",
        json!({ "commands": [{ "command": "browser.add_to_arrangement",
                               "args": { "paths": [file], "track": track, "at": 0 } }] }),
    );
    assert_eq!(result["ran"], 1);
    let document = client.transport().shell.editor().document();
    let clip = document
        .track(track)
        .unwrap()
        .clips()
        .find(|c| c.start.get() == 0)
        .unwrap();
    let ClipData::Audio(audio) = clip.data else {
        unreachable!("an audio clip");
    };
    (audio.fade_in, audio.fade_out)
}

// Spec: audio-clips R7, R22
#[test]
fn an_agent_switches_create_fades_on_clip_edges() {
    let file = wav();
    // Autonomous, so adding a file from outside the project runs.
    let mut client = Client::new(InProcess::new(Agent::headless(true), Shell::headless()));
    client.initialize("test-agent").unwrap();

    let described = call(
        &mut client,
        "describe_command",
        json!({ "command": "settings.set_create_fades_on_clip_edges" }),
    );
    assert_eq!(described["label"], "Create Fades on Clip Edges");
    assert_eq!(described["permission"], "edit");
    assert_eq!(described["access"], "runs");
    assert!(described["description"].as_str().unwrap().contains("4 ms"));

    assert_eq!(add(&mut client, &file), (0.0, 0.0), "off until switched on");
    let result = call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "settings.set_create_fades_on_clip_edges",
                               "args": { "on": true } }] }),
    );
    assert_eq!(result["ran"], 1);
    assert!(client.transport().shell.create_fades_on_clip_edges());
    assert_eq!(
        add(&mut client, &file),
        (EDGE_FADE_SECONDS, EDGE_FADE_SECONDS)
    );

    call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "settings.set_create_fades_on_clip_edges",
                               "args": { "on": false } }] }),
    );
    assert!(!client.transport().shell.create_fades_on_clip_edges());
    assert_eq!(add(&mut client, &file), (0.0, 0.0));
    let _ = std::fs::remove_file(&file);
}

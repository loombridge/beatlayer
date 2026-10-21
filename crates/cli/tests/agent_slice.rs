//! Slice to New MIDI Track through the agent API (WP-4.16): a scripted MCP
//! client drives `bl-cli mcp`, slices an audio clip into a Drum Rack by
//! command ID, and reads the new track, its rack and its clip back through
//! the query tool. The behaviour is in the slice-to-midi spec, R7 to R10 and
//! R13.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use bl_agent::client::{Client, Stdio, Transport};
use serde_json::{Value, json};

/// A folder in the system's temporary folder, removed when dropped.
struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let path = std::env::temp_dir().join(format!(
            "bl-cli-agent-slice-{name}-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("a temporary folder");
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn bl_cli_mcp(args: &[&str]) -> Client<Stdio> {
    let mut command = Command::new(env!("CARGO_BIN_EXE_bl-cli"));
    command.arg("mcp").args(args);
    let mut client = Client::new(Stdio::spawn(&mut command).expect("bl-cli mcp starts"));
    client
        .initialize("scripted-client")
        .expect("it initialises");
    client
}

fn call(client: &mut Client<impl Transport>, tool: &str, args: Value) -> Value {
    client
        .call(tool, args)
        .expect("the call reaches the server")
        .ok()
        .unwrap_or_else(|error| panic!("{tool} failed: {error}"))
}

/// One command by ID, and its result.
fn run(client: &mut Client<impl Transport>, command: &str, args: Value) -> Value {
    let outcome = call(
        client,
        "run",
        json!({ "commands": [{ "command": command, "args": args }] }),
    );
    outcome["results"][0]["result"].clone()
}

// Spec: slice-to-midi R7 to R10, R13. An agent slices a drum loop at the
// quarter note and reads the new track back.
#[test]
fn an_agent_slices_a_drum_loop_and_reads_the_track_back() {
    let dir = TempDir::new("slice");
    let loop_path = dir.0.join("Drum Loop.wav");
    fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/drums/drum-loop.wav"),
        &loop_path,
    )
    .unwrap();

    let mut client = bl_cli_mcp(&["--autonomous"]);
    call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "set.save_as",
                               "args": { "path": format!("{}/Song.blset", dir.0.display()) } }] }),
    );
    let placed = run(
        &mut client,
        "browser.add_to_arrangement",
        json!({ "paths": [loop_path.to_string_lossy()], "at": 0 }),
    );
    let clip = placed["clips"][0]["clip"].clone();
    let audio_track = placed["clips"][0]["track"].clone();

    // The loop is two seconds at 120 BPM: four quarter notes, so four slices
    // (R4).
    let sliced = run(
        &mut client,
        "clip.slice_to_midi_track",
        json!({ "clip": clip, "by": "quarter" }),
    );
    assert_eq!(sliced["slices"], json!(4), "{sliced}");
    let track = sliced["track"].clone();
    let midi = sliced["clip"].clone();

    // R7, R8: the new MIDI track is directly after the audio track and holds
    // a Drum Rack of one chain per slice, each receiving its slice's note
    // from C1 up and playing C3.
    let tracks = call(&mut client, "query", json!({ "path": "tracks" }))["tracks"]["items"]
        .as_array()
        .unwrap()
        .clone();
    let ids: Vec<Value> = tracks.iter().map(|track| track["id"].clone()).collect();
    let at = ids
        .iter()
        .position(|id| *id == audio_track)
        .expect("the audio track is there");
    assert_eq!(ids[at + 1], track, "directly after it: {ids:?}");
    let read = call(&mut client, "query", json!({ "id": track }));
    assert_eq!(read["kind"], json!("midi"), "{read}");
    let rack = read["devices"]
        .as_array()
        .unwrap()
        .iter()
        .find(|device| device["kind"]["rack"]["kind"] == json!("drum"))
        .unwrap_or_else(|| panic!("a Drum Rack: {read}"))
        .clone();
    let chains = rack["chains"].as_array().unwrap();
    assert_eq!(chains.len(), 4, "one chain per slice: {rack}");
    for (at, chain) in chains.iter().enumerate() {
        assert_eq!(chain["receive"], json!({ "note": 36 + at }), "{chain}");
        assert_eq!(chain["play"], json!(60), "{chain}");
    }

    // R9: one note per slice, rising from C1, at velocity 127.
    let read = call(
        &mut client,
        "query",
        json!({ "id": midi, "detail": "full" }),
    );
    assert_eq!(read["kind"], json!("midi"), "{read}");
    let notes = read["content"]["notes"]["items"].as_array().unwrap();
    assert_eq!(notes.len(), 4, "one note per slice: {read}");
    for (at, note) in notes.iter().enumerate() {
        assert_eq!(note["key"], json!(36 + at), "{note}");
        assert_eq!(note["velocity"], json!(127), "{note}");
    }
    // The readback R13 names.
    assert_eq!(
        read["saved"]["data"]["midi"]["notes"]["items"]
            .as_array()
            .unwrap()
            .len(),
        4,
        "{read}"
    );

    // R12: the audio clip is where it was.
    let before = call(&mut client, "query", json!({ "id": clip }));
    assert_eq!(before["kind"], json!("audio"), "{before}");
    assert_eq!(before["track"], audio_track, "{before}");

    assert!(client.into_transport().finish().unwrap().success());
}

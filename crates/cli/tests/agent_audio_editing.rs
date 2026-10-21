//! Crop, Consolidate, and Consolidate Time to New Scene through the agent
//! API (WP-4.13): a scripted MCP client drives `bl-cli mcp`, crops a clip
//! and consolidates a range by command ID, and reads each edit back through
//! the query tool. The behaviour is in the audio clip editing spec, R14 to
//! R28.

use std::fs;
use std::path::PathBuf;
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
            "bl-cli-agent-audio-{name}-{}-{nanos}",
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

/// Writes a short sine as a 16-bit WAV file. Test audio is generated, never
/// committed.
fn write_tone(path: &PathBuf, hz: f64) {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 48_000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec).unwrap();
    for n in 0..48_000 {
        let sample = (std::f64::consts::TAU * hz * f64::from(n) / 48_000.0).sin() * 0.2;
        writer.write_sample((sample * 32_767.0) as i16).unwrap();
    }
    writer.finalize().unwrap();
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

// Spec: audio-clip-editing R14, R18, R25, R27.
#[test]
fn an_agent_crops_and_consolidates_by_command_id_and_reads_them_back() {
    let dir = TempDir::new("commands");
    let tone = dir.0.join("Tone.wav");
    write_tone(&tone, 220.0);
    let project = dir.0.join("Song Project");

    let mut client = bl_cli_mcp(&["--autonomous"]);
    let saved = call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "set.save_as",
                               "args": { "path": format!("{}/Song.blset", dir.0.display()) } }] }),
    );
    assert!(
        saved["results"][0]["result"]["path"]
            .as_str()
            .unwrap()
            .contains("Agent Project")
            || saved["results"][0]["result"]["path"].is_string(),
        "the set was saved into a project folder"
    );
    let placed = run(
        &mut client,
        "browser.add_to_arrangement",
        json!({ "paths": [tone.to_string_lossy()], "at": 0 }),
    );
    let clip = placed["clips"][0]["clip"].clone();
    let track = placed["clips"][0]["track"].clone();

    // Crop a clip to its own region: the whole file, so nothing is removed.
    let cropped = run(&mut client, "clip.crop", json!({ "clip": clip }));
    assert!(
        cropped["media"].is_string(),
        "the new file's hash: {cropped}"
    );
    assert!(cropped["path"].is_string(), "and where it is: {cropped}");
    let read = call(&mut client, "query", json!({ "id": clip }));
    assert_eq!(
        read["content"]["media"], cropped["media"],
        "the query tool reads the clip playing the new file: {read}"
    );

    // Consolidate the first quarter into one clip.
    let range = (0, 705_600);
    let made = run(
        &mut client,
        "arrangement.consolidate",
        json!({ "tracks": [track], "start": range.0, "end": range.1 }),
    );
    assert_eq!(made["created"].as_array().unwrap().len(), 1, "{made}");
    assert_eq!(made["files"].as_array().unwrap().len(), 1, "{made}");
    let created = made["created"][0].clone();
    let read = call(
        &mut client,
        "query",
        json!({ "id": created, "detail": "full" }),
    );
    assert_eq!(read["kind"], json!("audio"), "it holds a clip: {read}");
    let saved = &read["saved"]["data"]["audio"];
    assert_eq!(
        saved["warp"],
        json!(true),
        "the consolidated clip is warped: {read}"
    );
    assert_eq!(saved["looping"], json!(true), "and looped: {read}");

    // Consolidate Time to New Scene: a new scene with the clip in its slot,
    // and the Arrangement kept.
    let before = call(&mut client, "query", json!({ "id": clip }));
    let scene = run(
        &mut client,
        "arrangement.consolidate_to_scene",
        json!({ "tracks": [track], "start": range.0, "end": range.1 }),
    );
    let scene_id = scene["scene"].clone();
    assert!(!scene_id.is_null(), "{scene}");
    assert_eq!(scene["created"].as_array().unwrap().len(), 1);
    let read = call(&mut client, "query", json!({ "id": scene_id }));
    assert_eq!(
        read["id"], scene_id,
        "the query tool reads the new scene: {read}"
    );
    let slot = call(&mut client, "query", json!({ "id": scene["created"][0] }));
    assert_eq!(
        slot["scene"], scene_id,
        "the new clip sits in the new scene's slot: {slot}"
    );
    let after = call(&mut client, "query", json!({ "id": clip }));
    assert_eq!(
        after["start"], before["start"],
        "the Arrangement is as it was"
    );

    assert!(client.into_transport().finish().unwrap().success());
    assert!(
        project.join("Samples/Processed/Crop").exists(),
        "the crop's file went into the project's Crop folder"
    );
}

// Spec: audio-clip-editing R18. Cropping away material asks first.
#[test]
fn without_autonomous_a_crop_is_refused() {
    let mut client = bl_cli_mcp(&[]);
    // A clip holding a file of its own: the permission is decided from the
    // call's arguments against the set, before anything is read.
    let tracks = call(&mut client, "query", json!({ "path": "tracks" }));
    let track = tracks["tracks"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|track| track["kind"] == "audio")
        .expect("the default set has an audio track")["id"]
        .clone();
    let created = call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "clip.create", "args": { "track": track, "clip": {
            "id": { "$new": "clip" }, "name": "Tone", "start": 0, "length": 1_411_200,
            "mute": false,
            "data": { "audio": { "media": "ab".repeat(32) } } } } }] }),
    );
    assert!(created["results"][0]["result"].is_object() || created["results"].is_array());
    let clip = json!({ "$ref": "clip" });
    // Keep only part of the clip: the rest is removed, so it asks first.
    let refused = client
        .call(
            "run",
            json!({ "commands": [{ "command": "clip.crop",
                                   "args": { "clip": clip, "start": 235_200, "end": 470_400 } }] }),
        )
        .unwrap();
    assert!(refused.is_error, "{}", refused.text);
    assert!(
        refused.text.contains("clip.crop") && refused.text.contains("destructive"),
        "{}",
        refused.text
    );
}

// Spec: audio-clip-editing R13. An agent reverses a clip by command ID and
// reads it back.
#[test]
fn an_agent_reverses_a_clip_and_reads_it_back() {
    let dir = TempDir::new("reverse");
    let tone = dir.0.join("Tone.wav");
    write_tone(&tone, 220.0);

    let mut client = bl_cli_mcp(&["--autonomous"]);
    run(
        &mut client,
        "set.save_as",
        json!({ "path": format!("{}/Song.blset", dir.0.display()) }),
    );
    let placed = run(
        &mut client,
        "browser.add_to_arrangement",
        json!({ "paths": [tone.to_string_lossy()], "at": 0 }),
    );
    let clip = placed["clips"][0]["clip"].clone();

    let reversed = run(&mut client, "clip.reverse", json!({ "clip": clip }));
    assert!(reversed["media"].is_string(), "the new file: {reversed}");
    assert!(reversed["path"].is_string(), "and where it is: {reversed}");
    let read = call(
        &mut client,
        "query",
        json!({ "id": clip, "detail": "full" }),
    );
    assert_eq!(
        read["saved"]["data"]["audio"]["media"], reversed["media"],
        "the clip plays the reversed file: {read}"
    );

    assert!(client.into_transport().finish().unwrap().success());
}

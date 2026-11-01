//! Stem separation through the agent API (WP-9.2): an agent separates a clip
//! in either view, a file and a time selection by ID, and reads the tracks
//! and clips back. A generated stand-in does the network's work. Spec:
//! stem-separation R22 to R30.

use std::path::{Path, PathBuf};

use bl_agent::Agent;
use bl_agent::client::{Client, InProcess};
use bl_shell::Shell;
use serde_json::{Value, json};

const BAR: i64 = 2_822_400;

/// A folder for one test's file, removed when the test ends.
struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn client() -> Client<InProcess> {
    let mut shell = Shell::headless();
    shell.use_stem_fixture();
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

fn query(client: &mut Client<InProcess>, args: Value) -> Value {
    client
        .call("query", args)
        .unwrap()
        .ok()
        .unwrap_or_else(|error| panic!("query failed: {error}"))
}

/// A second of a tone, as a WAV file.
fn tone(path: &Path) {
    let samples: Vec<f32> = (0..96_000)
        .map(|i| (i as f32 * 110.0 / 48_000.0 * std::f32::consts::TAU).sin() * 0.25)
        .collect();
    let bytes = bl_media::wav_bytes(
        &samples,
        2,
        48_000,
        bl_media::BitDepth::Float32,
        bl_media::Dither::None,
    )
    .unwrap();
    std::fs::write(path, bytes).unwrap();
}

/// Separates `source` and polls `stems.status` until it lands; the status.
fn separate(client: &mut Client<InProcess>, source: Value) -> Value {
    run(client, "stems.separate", json!({ "source": source }));
    for _ in 0..200_000 {
        // The window or `bl-cli mcp`'s transport polls the shell; this
        // client has neither (R14).
        client.transport().shell.poll();
        let status = run(client, "stems.status", json!({}));
        match status["separation"]["state"].as_str() {
            Some("running") => std::thread::yield_now(),
            Some("done") => return status,
            other => panic!("the separation ended {other:?}: {status}"),
        }
    }
    panic!("the separation never landed");
}

fn names(client: &mut Client<InProcess>, ids: &Value) -> Vec<String> {
    ids.as_array()
        .unwrap()
        .iter()
        .map(|id| {
            query(client, json!({ "id": id }))["name"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect()
}

/// The clip's `start` and `length` in ticks, and whether it's muted.
fn clip_at(client: &mut Client<InProcess>, clip: &Value) -> (i64, i64, bool) {
    let held = query(client, json!({ "id": clip }));
    (
        held["start"]["ticks"].as_i64().unwrap(),
        held["length"]["ticks"].as_i64().unwrap(),
        held["mute"].as_bool().unwrap(),
    )
}

// Spec: stem-separation R22 to R30. Each source is separated by ID, and the
// result names the group, the tracks and the clips, which read back through
// the query tool.
#[test]
fn an_agent_separates_each_source_by_id_and_reads_the_tracks_back() {
    let scratch =
        Scratch(std::env::temp_dir().join(format!("bl-agent-stems-{}", std::process::id())));
    std::fs::create_dir_all(&scratch.0).unwrap();
    let file = scratch.0.join("Pad.wav");
    tone(&file);
    let mut client = client();
    let song = query(&mut client, json!({ "path": "song" }));
    let audio = song["tracks"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["kind"] == "audio")
        .unwrap()["id"]
        .clone();
    let parts = ["Vocals", "Drums", "Bass", "Others"];

    // An Arrangement clip: copies on tracks in a group after its track.
    let placed = run(
        &mut client,
        "browser.add_to_arrangement",
        json!({ "paths": [file.to_string_lossy()], "track": audio, "at": BAR }),
    );
    let arrangement_clip = placed["clips"][0]["clip"].clone();
    let status = separate(
        &mut client,
        json!({ "kind": "clip", "clip": arrangement_clip }),
    );
    assert_eq!(
        names(&mut client, &status["created_tracks"]),
        ["Pad", "Vocals", "Drums", "Bass", "Others"]
    );
    assert_eq!(names(&mut client, &status["created_clips"]), parts);
    assert_eq!(
        clip_at(&mut client, &arrangement_clip),
        (BAR, BAR / 2, true)
    );
    assert_eq!(
        clip_at(&mut client, &status["created_clips"][0]),
        (BAR, BAR / 2, false)
    );

    // A Session clip: its stems are in the slots of its scene.
    let scene = query(&mut client, json!({ "path": "scene 1" }))["id"].clone();
    let dropped = run(
        &mut client,
        "session.drop_files",
        json!({ "paths": [file.to_string_lossy()], "track": audio, "scene": scene }),
    );
    let session_clip: Value = json!(
        dropped["clips"]
            .as_object()
            .unwrap()
            .keys()
            .next()
            .unwrap()
            .parse::<u64>()
            .unwrap()
    );
    let status = separate(&mut client, json!({ "kind": "clip", "clip": session_clip }));
    assert_eq!(names(&mut client, &status["created_clips"]), parts);
    let slots = query(&mut client, json!({ "path": "scene 1" }));
    let in_scene: Vec<&Value> = slots["slots"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|slot| &slot["clip"]["id"])
        .collect();
    for clip in status["created_clips"].as_array().unwrap() {
        assert!(in_scene.contains(&clip), "{clip} is in the scene's slots");
    }
    assert_eq!(
        query(&mut client, json!({ "id": session_clip }))["mute"],
        true
    );

    // A file: new tracks, its clips at the insert marker.
    let status = separate(
        &mut client,
        json!({ "kind": "file", "path": file.to_string_lossy() }),
    );
    assert_eq!(
        names(&mut client, &status["created_tracks"]),
        ["Pad", "Vocals", "Drums", "Bass", "Others"]
    );
    assert_eq!(clip_at(&mut client, &status["created_clips"][0]).0, 0);

    // A time selection: clips that span exactly the range. The clips
    // above are deactivated, so it covers a new one.
    run(
        &mut client,
        "browser.add_to_arrangement",
        json!({ "paths": [file.to_string_lossy()], "track": audio, "at": BAR * 4 }),
    );
    let status = separate(
        &mut client,
        json!({ "kind": "time_selection", "tracks": [audio], "start": BAR * 4, "end": BAR * 5 }),
    );
    assert_eq!(names(&mut client, &status["created_clips"]), parts);
    for clip in status["created_clips"].as_array().unwrap() {
        assert_eq!(clip_at(&mut client, clip), (BAR * 4, BAR, false));
    }
}

//! Convert Harmony, Melody and Drums to New MIDI Track through the agent API
//! (WP-9.7): an agent converts a clip by ID and reads the track and its
//! notes back. A stand-in gives the transcription's notes. Spec:
//! audio-to-midi R11, R26, R27.

use std::path::{Path, PathBuf};

use bl_agent::Agent;
use bl_agent::client::{Client, InProcess};
use bl_analysis::transcribe::Note as Found;
use bl_shell::Shell;
use serde_json::{Value, json};

const BAR: i64 = 2_822_400;
const QUARTER: i64 = BAR / 4;

/// A folder for one test's file, removed when the test ends.
struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn found(pitch: u8, start: f64, end: f64) -> Found {
    Found {
        start,
        end,
        pitch,
        velocity: 90,
        confidence: 0.5,
    }
}

fn client(notes: Vec<Found>) -> Client<InProcess> {
    let mut shell = Shell::headless();
    shell.use_transcription_fixture(notes);
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

/// Two seconds of a tone, as a WAV file.
fn tone(path: &Path) {
    let samples: Vec<f32> = (0..96_000)
        .map(|i| (i as f32 * 110.0 / 48_000.0 * std::f32::consts::TAU).sin() * 0.25)
        .collect();
    let bytes = bl_media::wav_bytes(
        &samples,
        1,
        48_000,
        bl_media::BitDepth::Float32,
        bl_media::Dither::None,
    )
    .unwrap();
    std::fs::write(path, bytes).unwrap();
}

/// Converts and polls `audio_to_midi.status` until it lands; the status.
fn convert(client: &mut Client<InProcess>, clip: &Value, kind: &str) -> Value {
    run(
        client,
        "audio_to_midi.convert",
        json!({ "clip": clip, "kind": kind }),
    );
    for _ in 0..200_000 {
        // The window or `bl-cli mcp`'s transport polls the shell; this
        // client has neither (stem separation R14).
        client.transport().shell.poll();
        let status = run(client, "audio_to_midi.status", json!({}));
        match status["conversion"]["state"].as_str() {
            Some("running") => std::thread::yield_now(),
            Some("done") => return status,
            other => panic!("the conversion ended {other:?}: {status}"),
        }
    }
    panic!("the conversion never landed");
}

// Spec: audio-to-midi R11. An agent converts a clip by ID with each kind; the
// status names the new track and clip, and `song` `tracks` and `clip` read
// them and the notes back.
#[test]
fn an_agent_converts_a_clip_by_id_and_reads_the_track_and_notes_back() {
    let scratch =
        Scratch(std::env::temp_dir().join(format!("bl-agent-a2m-{}", std::process::id())));
    std::fs::create_dir_all(&scratch.0).unwrap();
    let file = scratch.0.join("Pad.wav");
    tone(&file);
    let mut client = client(vec![found(60, 0.5, 1.0), found(64, 1.0, 1.5)]);
    let song = query(&mut client, json!({ "path": "song" }));
    let audio = song["tracks"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["kind"] == "audio")
        .unwrap()["id"]
        .clone();
    let placed = run(
        &mut client,
        "browser.add_to_arrangement",
        json!({ "paths": [file.to_string_lossy()], "track": audio, "at": BAR }),
    );
    let clip = placed["clips"][0]["clip"].clone();

    // Nothing is running, and nothing was made yet.
    let idle = run(&mut client, "audio_to_midi.status", json!({}));
    assert_eq!(idle["conversion"]["state"], "idle");
    assert!(idle["dialog"].is_null());

    for (kind, name) in [("melody", "Pad Melody"), ("drums", "Pad Drums")] {
        let status = convert(&mut client, &clip, kind);
        assert_eq!(status["kind"], kind);
        assert_eq!(status["clip"], clip);
        assert_eq!(status["notes"], 2);
        let track = query(&mut client, json!({ "id": status["created_track"] }));
        assert_eq!(track["name"], name);
        assert_eq!(track["kind"], "midi");
        // The track reads back in the song's list, after the clip's.
        let song = query(&mut client, json!({ "path": "song" }));
        let order: Vec<&Value> = song["tracks"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| &t["id"])
            .collect();
        let at = order.iter().position(|id| **id == audio).unwrap();
        assert_eq!(*order[at + 1], status["created_track"], "{kind}");
        // The clip is where the source is, with the notes at their beats.
        let made = query(
            &mut client,
            json!({ "id": status["created_clip"], "detail": "full" }),
        );
        assert_eq!(made["start"]["ticks"], BAR);
        let mut starts: Vec<u64> = made["saved"]["data"]["midi"]["notes"]["items"]
            .as_array()
            .unwrap_or_else(|| panic!("the clip's notes: {made}"))
            .iter()
            .map(|n| n["start"].as_u64().unwrap())
            .collect();
        starts.sort_unstable();
        assert_eq!(starts, [QUARTER as u64, 2 * QUARTER as u64], "{kind}");
        // The track's instrument reads back.
        let full = query(
            &mut client,
            json!({ "id": status["created_track"], "detail": "full" }),
        );
        let text = full.to_string();
        let marker = if kind == "melody" {
            "Synth to Piano"
        } else {
            "Closed Hi-Hat"
        };
        assert!(text.contains(marker), "{marker} in {text}");
    }
}

// Spec: audio-to-midi R26. A MIDI clip and an unknown clip are refused, with
// the reason, and a refusal changes nothing.
#[test]
fn an_agent_is_refused_what_cannot_be_converted() {
    let scratch =
        Scratch(std::env::temp_dir().join(format!("bl-agent-a2m-r-{}", std::process::id())));
    std::fs::create_dir_all(&scratch.0).unwrap();
    let file = scratch.0.join("Pad.wav");
    tone(&file);
    let mut client = client(vec![found(60, 0.5, 1.0)]);
    let song = query(&mut client, json!({ "path": "song" }));
    let audio = song["tracks"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["kind"] == "audio")
        .unwrap()["id"]
        .clone();
    let placed = run(
        &mut client,
        "browser.add_to_arrangement",
        json!({ "paths": [file.to_string_lossy()], "track": audio, "at": 0 }),
    );
    let clip = placed["clips"][0]["clip"].clone();
    let status = convert(&mut client, &clip, "harmony");
    let before = query(&mut client, json!({ "path": "song" }));
    for args in [
        json!({ "clip": status["created_clip"], "kind": "melody" }),
        json!({ "clip": 999_999, "kind": "melody" }),
    ] {
        let refused = client
            .call(
                "run",
                json!({ "commands": [{ "command": "audio_to_midi.convert", "args": args }] }),
            )
            .unwrap();
        let error = refused.ok().unwrap_err().to_string();
        assert!(!error.is_empty());
    }
    assert_eq!(query(&mut client, json!({ "path": "song" })), before);
}

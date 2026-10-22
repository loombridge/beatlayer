//! Fades and crossfades through the agent API (the audio clips spec, R30): an
//! agent describes the commands, runs them by ID, and reads each clip's fades,
//! curves and crossfade back from the query tool's `clip`, field `data`.

use std::path::PathBuf;

use bl_agent::Agent;
use bl_agent::client::{Client, InProcess};
use bl_model::{ClipId, TrackKind};
use bl_shell::Shell;
use serde_json::{Value, json};

fn call(client: &mut Client<InProcess>, tool: &str, args: Value) -> Value {
    client
        .call(tool, args)
        .unwrap()
        .ok()
        .unwrap_or_else(|error| panic!("{tool} failed: {error}"))
}

fn run(client: &mut Client<InProcess>, command: &str, args: Value) -> Value {
    let result = call(
        client,
        "run",
        json!({ "commands": [{ "command": command, "args": args }] }),
    );
    assert_eq!(result["ran"], 1, "{command}: {result}");
    result
}

/// Half a second of a quiet 16-bit mono WAV at 48 kHz, in a file of its own
/// under the system's temporary folder.
fn wav(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("bl-agent-{name}-{}.wav", std::process::id()));
    let frames: u32 = 24_000;
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
        bytes.extend_from_slice(&(((i % 48) as i16 - 24) * 2 + i16::from(name == "b")).to_le_bytes());
    }
    std::fs::write(&path, bytes).unwrap();
    path
}

/// A clip's audio data as the query tool reads it back, where a field a new
/// clip has is left out.
fn audio(client: &mut Client<InProcess>, clip: ClipId) -> Value {
    let got = call(client, "query", json!({ "id": clip, "detail": "full" }));
    got["saved"]["data"]["audio"].clone()
}

// Spec: audio-clips R30
#[test]
fn an_agent_makes_changes_and_deletes_fades_and_crossfades_and_reads_them_back() {
    let (first, second) = (wav("a"), wav("b"));
    let mut client = Client::new(InProcess::new(Agent::headless(true), Shell::headless()));
    client.initialize("test-agent").unwrap();

    for (command, permission) in [
        ("arrangement.set_crossfade", "edit"),
        ("arrangement.delete_crossfade", "edit"),
        ("arrangement.create_fade", "edit"),
        ("arrangement.delete_fades", "edit"),
        ("clip.set_fades", "edit"),
    ] {
        let described = call(&mut client, "describe_command", json!({ "command": command }));
        assert_eq!(described["permission"], permission, "{command}");
        assert_eq!(described["access"], "runs", "{command}");
    }

    let track = client
        .transport()
        .shell
        .editor()
        .document()
        .tracks()
        .find(|t| t.kind() == TrackKind::Audio)
        .unwrap()
        .id();
    // Two half-second clips, the second where the first ends; Create Fades
    // on Clip Edges is on, so they meet in a 4 ms crossfade.
    run(
        &mut client,
        "browser.add_to_arrangement",
        json!({ "paths": [first], "track": track, "at": 0 }),
    );
    let (one, end) = {
        let document = client.transport().shell.editor().document();
        let clip = document.track(track).unwrap().clips().next().unwrap();
        (clip.id, clip.end())
    };
    run(
        &mut client,
        "browser.add_to_arrangement",
        json!({ "paths": [second], "track": track, "at": end }),
    );
    let two = {
        let document = client.transport().shell.editor().document();
        document
            .track(track)
            .unwrap()
            .clips()
            .find(|c| c.id != one)
            .unwrap()
            .id
    };
    let (a, b) = (audio(&mut client, one), audio(&mut client, two));
    assert_eq!(a["fade_out_after"], 0.004, "{a}");
    assert_eq!(b["fade_in"], 0.004, "{b}");

    // A longer one with a curve, by hand; the later clip's file starts at its
    // start, so the crossfade can only run after the boundary (R25).
    run(
        &mut client,
        "arrangement.set_crossfade",
        json!({ "track": track, "boundary": end, "before": 0.0, "after": 0.1, "curve": 0.5 }),
    );
    let (a, b) = (audio(&mut client, one), audio(&mut client, two));
    assert_eq!((&a["fade_out_after"], &a["fade_out_curve"]), (&json!(0.1), &json!(0.5)));
    assert_eq!((&b["fade_in"], &b["fade_in_curve"]), (&json!(0.1), &json!(0.5)));
    // Past the later clip's file, it is refused and changes nothing.
    let refused = client
        .call(
            "run",
            json!({ "commands": [{ "command": "arrangement.set_crossfade",
                                   "args": { "track": track, "boundary": end,
                                             "before": 0.05, "after": 0.1 } }] }),
        )
        .unwrap()
        .ok();
    assert!(refused.is_err(), "{refused:?}");
    assert_eq!(audio(&mut client, two)["fade_in_before"], Value::Null);

    // Fades with curves.
    run(
        &mut client,
        "clip.set_fades",
        json!({ "clip": one, "fade_in": 0.05, "fade_out": 0.0, "fade_in_curve": -0.5 }),
    );
    let a = audio(&mut client, one);
    assert_eq!((&a["fade_in"], &a["fade_in_curve"]), (&json!(0.05), &json!(-0.5)));

    // Delete Fades puts the edges back and the crossfade to 4 ms; ending a
    // crossfade by ID leaves none.
    run(&mut client, "arrangement.delete_fades", json!({ "clips": [one, two] }));
    assert_eq!(audio(&mut client, one)["fade_in"], 0.004);
    assert_eq!(audio(&mut client, two)["fade_in"], 0.004);
    run(
        &mut client,
        "arrangement.delete_crossfade",
        json!({ "track": track, "boundary": end }),
    );
    let (a, b) = (audio(&mut client, one), audio(&mut client, two));
    assert_eq!((&a["fade_out"], &a["fade_out_after"]), (&Value::Null, &Value::Null));
    assert_eq!((&b["fade_in"], &b["fade_in_before"]), (&Value::Null, &Value::Null));
    // Create Fade on the two clips crossfades them again.
    run(&mut client, "arrangement.create_fade", json!({ "clips": [one, two] }));
    assert_eq!(audio(&mut client, one)["fade_out_after"], 0.004);
    let _ = std::fs::remove_file(&first);
    let _ = std::fs::remove_file(&second);
}

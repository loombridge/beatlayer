//! A clip's sound settings as automation and clip-envelope targets through
//! the agent API (WP-4.8, the warp modes spec R32): an agent automates
//! Formants on the track and draws a Transposition envelope on the clip by
//! command ID, with `{"type": "clip", "param": …}` as the target, and reads
//! both back.

use bl_agent::Agent;
use bl_agent::client::{Client, InProcess};
use bl_core::time::Ticks;
use bl_model::{AudioData, Clip, ClipData, ClipId, Command, ContentHash, TrackId, TrackKind};
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
    call(
        client,
        "run",
        json!({ "commands": [{ "command": command, "args": args }] }),
    )
}

// Spec: warp-modes R32
#[test]
fn an_agent_automates_formants() {
    let mut client = Client::new(InProcess::new(Agent::headless(true), Shell::headless()));
    client.initialize("test-agent").unwrap();
    let shell = &mut client.transport().shell;
    let track: TrackId = shell
        .editor()
        .document()
        .tracks()
        .find(|t| t.kind() == TrackKind::Audio)
        .map(bl_model::Track::id)
        .expect("the default set has an audio track");
    let clip: ClipId = shell.edit(|editor| {
        let id: ClipId = editor.new_id();
        editor
            .apply(Command::CreateClip {
                track,
                clip: Clip::new(
                    id,
                    Ticks::ZERO,
                    Ticks::WHOLE,
                    ClipData::Audio(AudioData::new(ContentHash([5; 32]))),
                ),
            })
            .unwrap();
        id
    });

    let name = client
        .transport()
        .shell
        .editor()
        .document()
        .track(track)
        .unwrap()
        .name()
        .to_owned();
    let formants = json!({ "type": "clip", "param": "formants" });
    for (at, value) in [(0, 100.0), (960, 25.0)] {
        let result = run(
            &mut client,
            "automation.set_breakpoint",
            json!({ "track": track, "target": formants, "at": at, "value": value }),
        );
        assert_eq!(result["ran"], 1);
    }
    let result = run(
        &mut client,
        "clip.set_envelope_breakpoint",
        json!({ "clip": clip,
                "target": { "type": "clip", "param": "transpose" },
                "at": 0, "value": 7.0 }),
    );
    assert_eq!(result["ran"], 1);

    // The automation reads back on the track, the envelope on the clip.
    let lane = call(
        &mut client,
        "query",
        json!({ "path": format!("track \"{name}\" / automation \"clip formants\""),
                "detail": "full" }),
    );
    assert_eq!(lane["target"], formants);
    assert_eq!(lane["enabled"], true);
    assert_eq!(lane["points"]["total"], 2);
    assert_eq!(lane["points"]["items"][1]["value"], 25.0);
    let saved = call(
        &mut client,
        "query",
        json!({ "id": clip, "detail": "full" }),
    );
    let envelope = &saved["saved"]["envelopes"]["items"][0];
    assert_eq!(
        envelope["target"],
        json!({ "type": "clip", "param": "transpose" })
    );
    assert_eq!(envelope["envelope"]["points"]["total"], 1);

    // A setting a clip doesn't have, and a value out of its range, are
    // refused.
    for (target, value) in [
        (json!({ "type": "clip", "param": "no_such_setting" }), 1.0),
        (formants.clone(), 101.0),
    ] {
        let refused = client
            .call(
                "run",
                json!({ "commands": [{ "command": "automation.set_breakpoint",
                        "args": { "track": track, "target": target, "at": 0, "value": value } }] }),
            )
            .unwrap();
        assert!(refused.is_error, "{target} {value}: {}", refused.text);
    }
}

//! Bouncing through the agent API (WP-4.15). The behaviour is in the
//! freeze and bounce spec, R26: the bounce commands are registered, they
//! ask first when they are destructive, and an agent can undo them.

use bl_agent::Agent;
use bl_agent::client::{Client, InProcess, Transport};
use bl_shell::Shell;
use serde_json::{Value, json};

/// A bar, in ticks.
const BAR: i64 = 4 * 705_600;

fn call(client: &mut Client<impl Transport>, tool: &str, args: Value) -> Value {
    client
        .call(tool, args)
        .expect("the call reaches the server")
        .ok()
        .unwrap_or_else(|error| panic!("{tool} failed: {error}"))
}

/// A client with a track holding a clip, and the track's and the clip's
/// IDs.
fn client_with_a_track(autonomous: bool) -> (Client<InProcess>, u64, u64) {
    let mut client = Client::new(InProcess::new(
        Agent::headless(autonomous),
        Shell::headless(),
    ));
    client.initialize("bounce").unwrap();
    let ran = call(
        &mut client,
        "run",
        json!({ "commands": [
            { "command": "track.create",
              "args": { "id": {"$new": "Bounce"}, "kind": "audio", "name": "Bounce", "index": 0 } },
            { "command": "clip.create",
              "args": { "track": {"$ref": "Bounce"}, "clip": {
                  "id": {"$new": "Bounce clip"}, "name": "Bounce clip", "start": 0,
                  "length": 4 * BAR, "mute": false,
                  "data": { "audio": { "media": "ab".repeat(32) } } } } },
        ] }),
    );
    assert_eq!(ran["ran"], 2, "{ran}");
    let tracks = call(
        &mut client,
        "query",
        json!({ "path": "song", "detail": "full" }),
    );
    let items = tracks["tracks"]["items"]
        .as_array()
        .expect("the song's tracks");
    let track = items
        .iter()
        .find(|track| track["name"] == "Bounce")
        .expect("the track")["id"]
        .as_u64()
        .expect("the track's ID");
    let clip = call(&mut client, "query", json!({ "id": track }));
    let clip = clip["clips"]["items"][0]["id"]
        .as_u64()
        .expect("the clip's ID");
    (client, track, clip)
}

// Spec: freeze-and-bounce R26. An agent reaches the bounces by command ID,
// and Bounce Track in Place asks first, since it deletes the track's
// devices and clips.
#[test]
fn an_agent_bounces_a_track_in_place_and_undoes_it() {
    let (mut client, track, _clip) = client_with_a_track(true);
    let bounced = call(
        &mut client,
        "run",
        json!({ "commands": [
            { "command": "track.bounce_in_place", "args": { "tracks": [track] } },
        ] }),
    );
    assert!(
        !bounced["results"].as_array().unwrap().is_empty(),
        "{bounced}"
    );
    let after = call(&mut client, "query", json!({ "id": track }));
    assert_eq!(after["name"], "Bounce (Bounce)", "{after}");
    assert_eq!(
        after["clips"]["items"].as_array().unwrap().len(),
        1,
        "{after}"
    );

    // Undoing the bounce gives the source track back.
    call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "edit.undo", "args": {} }] }),
    );
    let after = call(&mut client, "query", json!({ "id": track }));
    assert_eq!(after["name"], "Bounce", "{after}");
}

// Spec: freeze-and-bounce R26, the agent permissions spec R2. A bounce in
// place is destructive, so an agent that isn't autonomous is refused and
// the track is as it was.
#[test]
fn a_bounce_in_place_asks_first() {
    let (mut client, track, _clip) = client_with_a_track(false);
    let refused = client
        .call(
            "run",
            json!({ "commands": [
                { "command": "track.bounce_in_place", "args": { "tracks": [track] } },
            ] }),
        )
        .unwrap();
    assert!(refused.is_error, "{}", refused.text);
    assert!(refused.text.contains("destructive"), "{}", refused.text);
    let after = call(&mut client, "query", json!({ "id": track }));
    assert_eq!(after["name"], "Bounce", "{after}");
}

// Spec: freeze-and-bounce R26, the agent permissions spec R1. A paste of
// bounced audio that would replace a clip is destructive, so an agent that
// isn't autonomous is refused.
#[test]
fn a_paste_of_bounced_audio_over_clips_asks_first() {
    let (mut client, track, clip) = client_with_a_track(false);
    call(
        &mut client,
        "run",
        json!({ "commands": [
            { "command": "arrangement.copy", "args": { "clips": [clip] } },
        ] }),
    );
    let refused = client
        .call(
            "run",
            json!({ "commands": [
                { "command": "track.paste_bounced_audio", "args": { "track": track } },
            ] }),
        )
        .unwrap();
    assert!(refused.is_error, "{}", refused.text);
    assert!(refused.text.contains("destructive"), "{}", refused.text);
}

//! Linked tracks through the agent API (WP-6.23; the linked-track editing
//! spec, R15 and R16): an agent links tracks by ID, reads the group back in
//! the query tool's `track`, field `linked`, edits one clip with and without
//! `"linked": false`, and reads both edits back.

use bl_agent::client::{Client, InProcess};
use bl_agent::{Agent, QuestionKind};
use bl_shell::Shell;
use serde_json::{Value, json};

/// A bar of 4/4, in ticks.
const BAR: u64 = 2_822_400;

fn app() -> Client<InProcess> {
    let mut client = Client::new(InProcess::new(Agent::for_app(), Shell::headless()));
    client.transport().answer = Box::new(|q| matches!(q.kind, QuestionKind::Connect { .. }));
    client.initialize("editor").unwrap();
    client
}

fn call(client: &mut Client<InProcess>, tool: &str, args: Value) -> Value {
    client
        .call(tool, args)
        .unwrap()
        .ok()
        .unwrap_or_else(|e| panic!("{tool}: {e}"))
}

fn run(client: &mut Client<InProcess>, command: &str, args: Value) -> Value {
    let result = call(
        client,
        "run",
        json!({ "commands": [{ "command": command, "args": args }] }),
    );
    result["results"][0]["result"].clone()
}

fn track(client: &mut Client<InProcess>, name: &str) -> u64 {
    call(
        client,
        "query",
        json!({ "path": format!("track \"{name}\"") }),
    )["id"]
        .as_u64()
        .unwrap()
}

fn linked(client: &mut Client<InProcess>, id: u64) -> Value {
    call(client, "query", json!({ "id": id }))["linked"].clone()
}

/// A clip from bar 0, two bars long, on `track`: its ID.
fn clip(client: &mut Client<InProcess>, track: u64, name: &str) -> u64 {
    let result = call(
        client,
        "run",
        json!({ "commands": [{ "command": "clip.create", "args": {
            "track": track,
            "clip": { "id": { "$new": name }, "name": name, "start": 0,
                      "length": 2 * BAR, "mute": false, "data": { "audio": { "media": "ab".repeat(32) } } } } }] }),
    );
    result["ids"][name].as_u64().unwrap()
}

fn start(client: &mut Client<InProcess>, clip: u64) -> u64 {
    call(client, "query", json!({ "id": clip }))["start"]["ticks"]
        .as_u64()
        .unwrap()
}

// Spec: linked-track editing R15, R16.
#[test]
fn an_agent_links_tracks_by_id_edits_with_and_without_linked_and_reads_both_back() {
    let mut client = app();
    let (a, b, midi) = (
        track(&mut client, "3 Audio"),
        track(&mut client, "4 Audio"),
        track(&mut client, "1 MIDI"),
    );
    assert_eq!(
        linked(&mut client, a),
        json!([]),
        "empty for an unlinked track"
    );

    let result = run(&mut client, "track.link", json!({ "tracks": [a, b] }));
    assert_eq!(result["tracks"], json!([a, b]));
    assert_eq!(linked(&mut client, a), json!([a, b]));
    assert_eq!(linked(&mut client, b), json!([a, b]));
    assert_eq!(linked(&mut client, midi), json!([]));

    let (on_a, on_b) = (clip(&mut client, a, "on a"), clip(&mut client, b, "on b"));
    // One clip named: both move, and the result lists both.
    let moved = run(
        &mut client,
        "clip.move",
        json!({ "clip": on_a, "track": a, "start": BAR }),
    );
    assert_eq!(start(&mut client, on_a), BAR);
    assert_eq!(start(&mut client, on_b), BAR);
    assert_eq!(moved["clips"].as_object().unwrap().len(), 2, "{moved}");

    // `"linked": false` limits it to the clip named.
    run(
        &mut client,
        "clip.move",
        json!({ "clip": on_a, "track": a, "start": 2 * BAR, "linked": false }),
    );
    assert_eq!(start(&mut client, on_a), 2 * BAR);
    assert_eq!(start(&mut client, on_b), BAR);

    // Arming reaches the group; the group reads back from the query.
    run(
        &mut client,
        "mixer.set_arm",
        json!({ "track": a, "arm": true }),
    );
    let armed = |client: &mut Client<InProcess>, id| {
        call(client, "query", json!({ "id": id }))["mixer"]["arm"].clone()
    };
    assert_eq!(armed(&mut client, b), json!(true));

    let unlinked = run(&mut client, "track.unlink", json!({ "tracks": [b] }));
    assert_eq!(unlinked["tracks"], json!([b]));
    assert_eq!(linked(&mut client, a), json!([]), "a group of one ends");
}

//! The chord lane through the agent API (WP-6.48): every command runs by ID,
//! the permission classes are the spec's, and what an agent sets reads back
//! from the query tool's `song` (`chord_lane`) and `track`
//! (`follow_chord_lane`) and from `arrangement.state`. Spec: chord-lane R12.

use bl_agent::Agent;
use bl_agent::client::{Client, InProcess};
use bl_shell::Shell;
use serde_json::{Value, json};

const BAR: i64 = 2_822_400;

fn client() -> Client<InProcess> {
    let mut client = Client::new(InProcess::new(Agent::headless(true), Shell::headless()));
    client.initialize("test-agent").unwrap();
    client
}

/// Runs a batch; a `{"$new": name}` issues an ID that later calls refer to
/// as `{"$ref": name}`.
fn batch(client: &mut Client<InProcess>, commands: Value) -> Value {
    client
        .call("run", json!({ "commands": commands }))
        .unwrap()
        .ok()
        .unwrap_or_else(|error| panic!("{commands} failed: {error}"))
}

fn run(client: &mut Client<InProcess>, command: &str, args: Value) -> Value {
    batch(client, json!([{ "command": command, "args": args }]))["results"][0]["result"].clone()
}

fn query(client: &mut Client<InProcess>, args: Value) -> Value {
    client
        .call("query", args)
        .unwrap()
        .ok()
        .unwrap_or_else(|error| panic!("query failed: {error}"))
}

#[test]
fn every_chord_lane_command_runs_by_id_and_what_it_sets_reads_back() {
    let mut client = client();
    let made = batch(
        &mut client,
        json!([
            { "command": "track.create",
              "args": { "id": {"$new": "keys"}, "kind": "midi", "name": "Keys", "index": 0 } },
            { "command": "chord_lane.add_region",
              "args": { "region": { "id": {"$new": "verse"}, "start": BAR, "end": 3 * BAR } } },
        ]),
    );
    let (keys, verse) = (made["ids"]["keys"].clone(), made["ids"]["verse"].clone());
    // A new set's lane is hidden and a new track doesn't follow it.
    assert_eq!(
        run(&mut client, "arrangement.state", json!({}))["chord_lane"],
        false
    );
    assert_eq!(
        query(&mut client, json!({ "id": keys }))["follow_chord_lane"],
        false
    );
    // The region reads back from the song.
    let song = query(&mut client, json!({ "path": "song" }));
    assert_eq!(
        song["chord_lane"],
        json!([{ "id": verse, "start": BAR, "end": 3 * BAR,
                 "scale": { "enabled": false, "root": 0, "name": "major" }, "transpose": 0 }])
    );
    run(
        &mut client,
        "chord_lane.move_region",
        json!({ "region": verse, "start": 2 * BAR, "end": 4 * BAR }),
    );
    run(
        &mut client,
        "chord_lane.set_region",
        json!({ "region": verse, "enabled": true, "root": 2, "name": "dorian", "transpose": 2 }),
    );
    run(
        &mut client,
        "track.set_follow_chord_lane",
        json!({ "track": keys, "follow": true }),
    );
    run(&mut client, "view.chord_lane", json!({ "on": true }));
    run(
        &mut client,
        "chord_lane.select_region",
        json!({ "region": verse }),
    );
    let song = query(&mut client, json!({ "path": "song" }));
    assert_eq!(
        song["chord_lane"][0],
        json!({ "id": verse, "start": 2 * BAR, "end": 4 * BAR,
                "scale": { "enabled": true, "root": 2, "name": "dorian" }, "transpose": 2 })
    );
    assert_eq!(
        query(&mut client, json!({ "id": keys }))["follow_chord_lane"],
        true
    );
    let state = run(&mut client, "arrangement.state", json!({}));
    assert_eq!(state["chord_lane"], true);
    assert_eq!(state["selected_chord_region"], verse);
    // The saved form holds them too.
    let full = query(&mut client, json!({ "path": "song", "detail": "full" }));
    assert_eq!(
        full["saved"]["settings"]["chord_lane"]["items"][0]["transpose"],
        2
    );
    // A region that would overlap is refused, and nothing changes.
    let _ = client.call(
        "run",
        json!({ "commands": [{ "command": "chord_lane.add_region",
            "args": { "region": { "id": {"$new": "clash"}, "start": 3 * BAR,
                                  "end": 5 * BAR } } }] }),
    );
    assert_eq!(
        query(&mut client, json!({ "path": "song" }))["chord_lane"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    // Delete, and undo brings it back: one undo step each.
    run(
        &mut client,
        "chord_lane.delete_region",
        json!({ "region": verse }),
    );
    assert_eq!(
        query(&mut client, json!({ "path": "song" }))["chord_lane"],
        json!([])
    );
    run(&mut client, "edit.undo", json!({}));
    assert_eq!(
        query(&mut client, json!({ "path": "song" }))["chord_lane"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn the_chord_lane_commands_have_the_classes_the_spec_gives() {
    // Spec: chord-lane R12.
    let mut registry = bl_commands::Registry::new();
    bl_model::registry::register(&mut registry).unwrap();
    bl_shell::register(&mut registry).unwrap();
    let class = |id: &str| registry.get(id).unwrap().permission.unwrap();
    use bl_commands::Permission::{Destructive, Edit, Read};
    for (id, want) in [
        ("view.chord_lane", Read),
        ("chord_lane.select_region", Read),
        ("chord_lane.add_region", Edit),
        ("chord_lane.move_region", Edit),
        ("chord_lane.set_region", Edit),
        ("chord_lane.delete_region", Destructive),
        ("track.set_follow_chord_lane", Edit),
    ] {
        assert_eq!(class(id), want, "{id}");
    }
}

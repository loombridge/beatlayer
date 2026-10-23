//! Comping through the agent API (WP-6.21): every take-lane and comp
//! command runs by ID and reads back in the query tool's `take_lanes` and
//! `clips` or in `take_lane.comp_sources`; a lane is addressed by its ID or
//! by `track "X" / lane N`; and the commands that delete, or that replace
//! clips on the main lane, ask first. The rules are in the comping spec, R37.

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

fn query(client: &mut Client<InProcess>, path: &str) -> Value {
    call(client, "query", json!({ "path": path }))
}

/// A take lane `name` on the audio track with `clips` bars-long clips from
/// bar 0 of file `file`, and its ID.
fn lane(client: &mut Client<InProcess>, track: u64, name: &str, file: &str, bars: u64) -> u64 {
    let result = call(
        client,
        "run",
        json!({ "commands": [{ "command": "track.add_take_lane", "args": {
            "track": track,
            "lane": { "id": { "$new": name }, "name": name, "clips": [
                { "name": name, "start": 0, "length": bars * BAR,
                  "data": { "media": file.repeat(32) } }
            ] } } }] }),
    );
    result["ids"][name].as_u64().unwrap()
}

/// Runs `command`, with the person declining any question, and says whether
/// it asked.
fn asks(client: &mut Client<InProcess>, command: &str, args: &Value) -> bool {
    let asked_before = client.transport().asked.len();
    let result = client
        .call(
            "run",
            json!({ "commands": [{ "command": command, "args": args }] }),
        )
        .unwrap();
    let asked = client.transport().asked.len() > asked_before;
    if asked {
        assert!(
            result.is_error && result.text.contains("declined"),
            "{result:?}"
        );
    } else {
        assert!(!result.is_error, "{command}: {}", result.text);
    }
    asked
}

// Spec: comping R4 to R8, R14, R37.
#[test]
fn every_take_lane_command_runs_by_id_and_reads_back() {
    let mut client = app();
    let audio = query(&mut client, "track \"3 Audio\"")["id"]
        .as_u64()
        .unwrap();
    let verse = lane(&mut client, audio, "Verse", "ab", 4);

    // A lane reads back by its ID and by its place, with its clips' IDs.
    let by_id = call(&mut client, "query", json!({ "id": verse }));
    let by_place = query(&mut client, "track \"3 Audio\" / lane 1");
    assert_eq!(by_id, by_place);
    assert_eq!(by_id["name"], "Verse");
    assert_eq!(by_id["audition"], false);
    assert_eq!(by_id["clips"]["total"], 1);

    // Insert and Duplicate make lanes with IDs they report.
    let inserted = run(
        &mut client,
        "take_lane.insert",
        json!({ "tracks": [audio], "after": [verse] }),
    );
    let new = inserted["lanes"][0].as_u64().unwrap();
    let track = query(&mut client, "track \"3 Audio\"");
    let lanes: Vec<(u64, String)> = track["take_lanes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| {
            (
                l["id"].as_u64().unwrap(),
                l["name"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    assert_eq!(
        lanes,
        [(verse, "Verse".to_owned()), (new, "Take 1".to_owned())]
    );
    let copied = run(
        &mut client,
        "take_lane.duplicate",
        json!({ "lanes": [verse] }),
    );
    let copy = copied["lanes"][0].as_u64().unwrap();
    let track = query(&mut client, "track \"3 Audio\"");
    assert_eq!(track["take_lanes"][1]["id"], copy);
    assert_eq!(track["take_lanes"][1]["name"], "Verse");
    assert_eq!(track["take_lanes"][1]["clips"].as_array().unwrap().len(), 1);

    // Rename, move and audition read back from the track.
    run(
        &mut client,
        "take_lane.rename",
        json!({ "lane": copy, "name": "Chorus" }),
    );
    run(
        &mut client,
        "take_lane.move",
        json!({ "lanes": [copy], "index": 0 }),
    );
    run(
        &mut client,
        "take_lane.set_audition",
        json!({ "lane": new, "on": true }),
    );
    let track = query(&mut client, "track \"3 Audio\"");
    let summary: Vec<(String, bool)> = track["take_lanes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| {
            (
                l["name"].as_str().unwrap().to_owned(),
                l["audition"].as_bool().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        summary,
        [
            ("Chorus".to_owned(), false),
            ("Verse".to_owned(), false),
            ("Take 1".to_owned(), true)
        ]
    );
    let full = call(
        &mut client,
        "query",
        json!({ "id": audio, "detail": "full" }),
    );
    assert_eq!(full["saved"]["audition"], new);

    // Delete, Delete Unused and Delete All are destructive and ask first.
    for (command, args) in [
        ("take_lane.delete", json!({ "lanes": [copy] })),
        ("take_lane.delete_unused", json!({ "tracks": [audio] })),
        ("take_lane.delete_all", json!({ "tracks": [audio] })),
    ] {
        assert!(asks(&mut client, command, &args), "{command} asks first");
    }
    client.transport().answer = Box::new(|_| true);
    run(&mut client, "take_lane.delete", json!({ "lanes": [copy] }));
    assert_eq!(
        query(&mut client, "track \"3 Audio\"")["take_lanes"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    run(
        &mut client,
        "take_lane.delete_unused",
        json!({ "tracks": [audio] }),
    );
    run(
        &mut client,
        "take_lane.delete_all",
        json!({ "tracks": [audio] }),
    );
    assert!(
        query(&mut client, "track \"3 Audio\"")
            .get("take_lanes")
            .is_none()
    );
}

// Spec: comping R17, R20, R21, R37. Copying to the main lane over a clip asks
// first, into empty space it doesn't; the comp reads back.
#[test]
fn the_comp_commands_read_back_and_ask_when_they_replace_a_clip() {
    let mut client = app();
    let audio = query(&mut client, "track \"3 Audio\"")["id"]
        .as_u64()
        .unwrap();
    let take = lane(&mut client, audio, "Take A", "ab", 8);

    // Into empty space: an edit, no question.
    let copy = json!({ "lanes": [take], "start": 0, "end": 2 * BAR });
    assert!(!asks(&mut client, "take_lane.copy_to_main_lane", &copy));
    let main = query(&mut client, "track \"3 Audio\"");
    assert_eq!(main["clips"]["total"], 1);
    let sources = run(
        &mut client,
        "take_lane.comp_sources",
        json!({ "track": audio }),
    );
    assert_eq!(sources["stretches"].as_array().unwrap().len(), 1);
    assert_eq!(sources["stretches"][0]["lane"], take);

    // Over the copy it would replace it: destructive.
    let over = json!({ "lanes": [take], "start": BAR, "end": 3 * BAR });
    assert!(asks(&mut client, "take_lane.copy_to_main_lane", &over));
    let dry = call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "take_lane.copy_to_main_lane", "args": over }],
                "dry_run": true }),
    );
    assert_eq!(dry["permission"], "edit and destructive");

    // Moving the comp edge only resizes the two parts it joins: an edit.
    client.transport().answer = Box::new(|_| true);
    let second = lane(&mut client, audio, "Take B", "cd", 8);
    run(
        &mut client,
        "take_lane.copy_to_main_lane",
        json!({ "lanes": [second], "start": 2 * BAR, "end": 4 * BAR }),
    );
    client.transport().answer = Box::new(|_| false);
    let edge = json!({ "track": audio, "from": 2 * BAR, "to": 3 * BAR });
    assert!(!asks(&mut client, "take_lane.move_comp_edge", &edge));
    let main = query(&mut client, "track \"3 Audio\"");
    let spans: Vec<(u64, u64)> = main["clips"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            (
                c["start"]["ticks"].as_u64().unwrap() / BAR,
                c["end"]["ticks"].as_u64().unwrap() / BAR,
            )
        })
        .collect();
    assert_eq!(spans, [(0, 3), (3, 4)]);

    // Replace with the next take, from the lane: destructive over the clips.
    let replace =
        json!({ "track": audio, "start": 0, "end": BAR, "direction": "next", "lane": take });
    assert!(asks(&mut client, "take_lane.replace_with_take", &replace));
}

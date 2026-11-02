//! Linked clips through the agent API (WP-6.44): a scripted MCP client drives
//! `bl-cli mcp`, copies a clip as a linked clip by command ID, edits one clip
//! of the link by ID and reads every clip of it back through the query
//! tool's `clip.link`. The behaviour is in the linked clips spec, R14 and
//! R15.

use std::process::Command;

use bl_agent::client::{Client, Stdio, Transport};
use bl_core::time::Ticks;
use bl_model::Note;
use serde_json::{Value, json};

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

/// A clip's notes' keys, sorted, as the query tool reads them.
fn keys(client: &mut Client<impl Transport>, clip: &Value) -> Vec<u64> {
    let read = call(client, "query", json!({ "id": clip, "detail": "full" }));
    let mut keys: Vec<u64> = read["content"]["notes"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|note| note["key"].as_u64().unwrap())
        .collect();
    keys.sort_unstable();
    keys
}

fn midi(keys: &[u8]) -> Value {
    let notes: Vec<Value> = keys
        .iter()
        .enumerate()
        .map(|(at, key)| {
            serde_json::to_value(Note::new(*key, Ticks(at as i64 * 240), Ticks(240), 100)).unwrap()
        })
        .collect();
    json!({ "midi": { "notes": notes } })
}

// Spec: linked-clips R14, R15
#[test]
fn an_agent_links_edits_and_unlinks_clips_and_reads_the_link_back() {
    let mut client = bl_cli_mcp(&[]);
    call(
        &mut client,
        "run",
        json!({ "commands": [
            { "command": "track.create",
              "args": { "id": {"$new": "Keys"}, "kind": "midi", "name": "Keys", "index": 0 } },
            { "command": "clip.create",
              "args": { "track": {"$ref": "Keys"},
                        "clip": { "id": {"$new": "Clip"}, "name": "", "start": 7680,
                                  "length": 15360, "mute": false, "data": midi(&[60, 64]) } } },
        ] }),
    );
    let tracks = call(&mut client, "query", json!({ "path": "tracks" }))["tracks"]["items"].clone();
    let track = tracks
        .as_array()
        .unwrap()
        .iter()
        .find(|track| track["name"] == "Keys")
        .unwrap()["id"]
        .clone();
    let held = call(
        &mut client,
        "query",
        json!({ "id": track, "detail": "full" }),
    );
    let clip = held["clips"]["items"][0]["id"].clone();
    assert!(clip.is_u64(), "{held}");

    // An independent clip reads back a null link.
    let read = call(&mut client, "query", json!({ "id": clip }));
    assert_eq!(read["link"], Value::Null, "{read}");

    // R14: the copy's ID, and the link on both clips.
    let made = run(
        &mut client,
        "clip.duplicate_linked",
        json!({ "clips": [clip] }),
    );
    let copy = made["created"][0].clone();
    assert!(copy.is_u64(), "{made}");
    let read = call(&mut client, "query", json!({ "id": clip }));
    let other = call(&mut client, "query", json!({ "id": copy }));
    assert_eq!(read["link"]["others"], json!([copy]), "{read}");
    assert_eq!(other["link"]["others"], json!([clip]), "{other}");
    assert_eq!(read["link"]["id"], other["link"]["id"]);

    // R15: a clip.set_data on the copy reaches the clip, and says so.
    let outcome = run(
        &mut client,
        "clip.set_data",
        json!({ "clip": copy, "data": midi(&[60, 64, 67]) }),
    );
    let changed = outcome.to_string();
    assert!(
        changed.contains(&clip.to_string()) && changed.contains(&copy.to_string()),
        "the result names both clips: {outcome}"
    );
    assert_eq!(keys(&mut client, &clip), [60, 64, 67]);
    assert_eq!(keys(&mut client, &copy), [60, 64, 67]);

    // Dropping notes from the link asks first: without --autonomous it's
    // refused, and nothing changes.
    let refused = client
        .call(
            "run",
            json!({ "commands": [{ "command": "clip.set_data",
                                   "args": { "clip": copy, "data": midi(&[60]) } }] }),
        )
        .unwrap();
    assert!(refused.is_error, "{}", refused.text);
    assert!(refused.text.contains("destructive"), "{}", refused.text);
    assert_eq!(keys(&mut client, &clip), [60, 64, 67]);

    // R14: clip.unlink makes both independent, with the contents they have.
    run(&mut client, "clip.unlink", json!({ "clips": [copy] }));
    let read = call(&mut client, "query", json!({ "id": clip }));
    let other = call(&mut client, "query", json!({ "id": copy }));
    assert_eq!(read["link"], Value::Null, "{read}");
    assert_eq!(other["link"], Value::Null, "{other}");
    assert_eq!(keys(&mut client, &copy), [60, 64, 67]);
    assert!(client.into_transport().finish().unwrap().success());
}

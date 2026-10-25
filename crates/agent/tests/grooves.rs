//! Grooves through the agent API (WP-6.19): every groove command runs by ID,
//! the permission classes are the spec's, and what an agent sets reads back
//! from `groove.pool` and the query tool's `song` and `clip`. Spec: grooves
//! R5 and R23.

use bl_agent::Agent;
use bl_agent::client::{Client, InProcess};
use bl_shell::Shell;
use serde_json::{Value, json};

const BAR: i64 = 2_822_400;
const STEP: i64 = BAR / 16;

fn client() -> Client<InProcess> {
    let mut client = Client::new(InProcess::new(Agent::headless(true), Shell::headless()));
    client.initialize("test-agent").unwrap();
    client
}

/// Runs a batch through the agent API and returns the call's result. A
/// `{"$new": name}` in a command's arguments issues an ID that later calls
/// refer to as `{"$ref": name}`.
fn batch(client: &mut Client<InProcess>, commands: Value) -> Value {
    client
        .call("run", json!({ "commands": commands }))
        .unwrap()
        .ok()
        .unwrap_or_else(|error| panic!("{commands} failed: {error}"))
}

/// Runs one command and returns its result.
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
fn every_groove_command_runs_by_id_and_what_it_sets_reads_back() {
    let mut client = client();
    // The factory default set holds a groove (R38); this test builds its own
    // pool.
    let factory = run(&mut client, "groove.pool", json!({}))["grooves"][0]["id"].clone();
    run(&mut client, "groove.delete", json!({ "groove": factory }));
    let made = batch(
        &mut client,
        json!([
            { "command": "track.create",
              "args": { "id": {"$new": "keys"}, "kind": "midi", "name": "Keys", "index": 0 } },
            { "command": "clip.create",
              "args": { "track": {"$ref": "keys"},
                        "clip": { "id": {"$new": "riff"}, "name": "Riff", "start": 0,
                                  "length": BAR, "mute": false,
                                  "data": { "midi": { "notes": [
                                      { "key": 60, "velocity": 100, "start": 0, "length": STEP },
                                      { "key": 62, "velocity": 80, "start": STEP, "length": STEP }
                                  ] } } } } },
            { "command": "groove.add",
              "args": { "groove": { "id": {"$new": "swing"}, "name": "Swing", "length": BAR,
                                    "notes": [{ "position": 0, "velocity": 127 },
                                              { "position": STEP + 1000, "velocity": 70 }] } } },
        ]),
    );
    let (keys, riff, swing) = (
        made["ids"]["keys"].clone(),
        made["ids"]["riff"].clone(),
        made["ids"]["swing"].clone(),
    );
    run(
        &mut client,
        "groove.rename",
        json!({ "groove": swing, "name": "Swing 60" }),
    );
    run(
        &mut client,
        "groove.set",
        json!({ "groove": swing, "base": "1/8T", "quantize": 30.0, "timing": 70.0 }),
    );
    run(
        &mut client,
        "groove.set_global_amount",
        json!({ "amount": 90.0 }),
    );
    run(
        &mut client,
        "clip.set_groove",
        json!({ "clips": [riff], "groove": swing }),
    );
    run(
        &mut client,
        "groove.set_auto_load",
        json!({ "enabled": true, "groove": swing }),
    );

    // The pool reads back what was set.
    let pool = run(&mut client, "groove.pool", json!({}));
    let groove = &pool["grooves"][0];
    assert_eq!(groove["name"], "Swing 60");
    assert_eq!(groove["base"], "1/8T");
    assert_eq!(groove["quantize"], json!(30.0));
    assert_eq!(groove["timing"], json!(70.0));
    assert_eq!(groove["clips"], json!([riff]));
    assert_eq!(pool["global_amount"], json!(90.0));
    assert_eq!(pool["auto_load"], true);
    assert_eq!(pool["auto_load_groove"], swing);

    // The query tool's song holds the pool under its saved settings, and its
    // clip the clip's groove.
    let song = query(&mut client, json!({ "path": "song", "detail": "full" }));
    let saved = &song["saved"]["settings"]["grooves"];
    assert_eq!(saved["pool"]["items"][0]["name"], "Swing 60");
    assert_eq!(saved["global_amount"], json!(90.0));
    assert_eq!(saved["auto_load_groove"], swing);
    assert_eq!(query(&mut client, json!({ "id": riff }))["groove"], swing);
    let full = query(&mut client, json!({ "id": riff, "detail": "full" }));
    assert_eq!(full["saved"]["groove"], swing);

    // Extract, commit, make a clip, and delete.
    let extracted = run(&mut client, "groove.extract", json!({ "clips": [riff] }));
    assert_eq!(extracted["grooves"].as_array().unwrap().len(), 1);
    run(
        &mut client,
        "groove.commit",
        json!({ "clips": [riff], "seed": 4 }),
    );
    assert_eq!(
        query(&mut client, json!({ "id": riff }))["groove"],
        Value::Null
    );
    run(
        &mut client,
        "groove.to_clip",
        json!({ "groove": swing, "track": keys, "at": BAR * 2 }),
    );
    run(&mut client, "groove.delete", json!({ "groove": swing }));
    let pool = run(&mut client, "groove.pool", json!({}));
    assert_eq!(
        pool["grooves"].as_array().unwrap().len(),
        1,
        "only the extracted one is left"
    );
    assert_eq!(pool["auto_load_groove"], Value::Null);
}

#[test]
fn the_groove_commands_have_the_classes_the_spec_gives() {
    let mut registry = bl_commands::Registry::new();
    bl_model::registry::register(&mut registry).unwrap();
    bl_shell::register(&mut registry).unwrap();
    let class = |id: &str| registry.get(id).unwrap().permission.unwrap();
    use bl_commands::Permission::{Destructive, Edit, External, Read};
    for (id, want) in [
        ("groove.load", External),
        ("groove.save", External),
        ("groove.extract", Edit),
        ("groove.commit", Edit),
        ("groove.to_clip", Edit),
        ("groove.set", Edit),
        ("groove.rename", Edit),
        ("groove.set_global_amount", Edit),
        ("groove.set_auto_load", Edit),
        ("clip.set_groove", Edit),
        ("groove.delete", Destructive),
        ("groove.pool", Read),
        // Spec: grooves R24, R32. The pool's view is view state.
        ("view.toggle_groove_pool", Read),
        ("focus.groove_pool", Read),
        ("groove.hot_swap", Read),
    ] {
        assert_eq!(class(id), want, "{id}");
    }
}

// Spec: grooves R24, R32. The pool's view and its hot-swap are set and read
// through the agent API, as view state: no undo step, and `groove.pool`
// reads them back.
#[test]
fn the_pools_view_and_its_hot_swap_are_set_and_read_back() {
    let mut client = client();
    let pool = run(&mut client, "groove.pool", json!({}));
    assert_eq!(pool["view"], json!({ "shown": false }));
    let groove = pool["grooves"][0]["id"].clone();
    assert_eq!(
        run(&mut client, "view.toggle_groove_pool", json!({})),
        json!({ "shown": true })
    );
    let swapping = run(&mut client, "groove.hot_swap", json!({ "groove": groove }));
    assert_eq!(
        swapping,
        json!({ "shown": true, "hot_swap": { "groove": groove } })
    );
    assert_eq!(run(&mut client, "groove.pool", json!({}))["view"], swapping);
    let auto = run(&mut client, "groove.hot_swap", json!({ "auto_load": true }));
    assert_eq!(auto["hot_swap"], json!("auto_load"));
    let ended = run(&mut client, "groove.hot_swap", json!({ "on": false }));
    assert_eq!(ended, json!({ "shown": true }));
    assert_eq!(
        run(&mut client, "focus.groove_pool", json!({})),
        json!({ "shown": true })
    );
    // Loading a file with `replace` runs by ID too, and `groove.to_clip` takes
    // a file and makes a new MIDI track.
    let file = bl_shell::resources::grooves()
        .unwrap()
        .join("Swing 8ths 55.blgroove")
        .to_string_lossy()
        .into_owned();
    let loaded = run(
        &mut client,
        "groove.load",
        json!({ "path": file, "replace": { "groove": groove } }),
    );
    let pool = run(&mut client, "groove.pool", json!({}));
    assert_eq!(pool["grooves"].as_array().unwrap().len(), 1);
    assert_eq!(pool["grooves"][0]["id"], loaded["groove"]);
    assert_eq!(
        pool["auto_load_groove"], loaded["groove"],
        "the auto-load groove followed"
    );
    let made = run(
        &mut client,
        "groove.to_clip",
        json!({ "path": file, "at": 0 }),
    );
    assert!(made["track"].is_u64());
    let pool = run(&mut client, "groove.pool", json!({}));
    assert_eq!(
        pool["grooves"].as_array().unwrap().len(),
        2,
        "the file joined the pool"
    );
}

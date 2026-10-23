//! The agent API's tools, driven in-process by the scripted client as an
//! agent drives them: discovery, run (batches, dry runs, IDs), query,
//! the transport, checkpoints, permissions, and the protocol around them.

use bl_agent::client::{Client, InProcess};
use bl_agent::{Agent, EngineMessage, QuestionKind, guide};
use bl_core::time::Ticks;
use bl_engine::{Action, ScheduledCommand, TransportCommand};
use bl_model::{CountIn, MetronomeRhythm, MetronomeSound};
use bl_shell::Shell;
use serde_json::{Value, json};

fn headless(autonomous: bool) -> Client<InProcess> {
    let mut client = Client::new(InProcess::new(
        Agent::headless(autonomous),
        Shell::headless(),
    ));
    client.initialize("test-agent").unwrap();
    client
}

fn app() -> Client<InProcess> {
    let mut client = Client::new(InProcess::new(Agent::for_app(), Shell::headless()));
    client.transport().answer = Box::new(|_| true);
    client.initialize("test-agent").unwrap();
    client
}

fn call(client: &mut Client<impl bl_agent::client::Transport>, tool: &str, args: Value) -> Value {
    client
        .call(tool, args)
        .unwrap()
        .ok()
        .unwrap_or_else(|error| panic!("{tool} failed: {error}"))
}

fn call_err(
    client: &mut Client<impl bl_agent::client::Transport>,
    tool: &str,
    args: Value,
) -> String {
    let result = client.call(tool, args).unwrap();
    assert!(result.is_error, "{tool} should fail: {}", result.text);
    result.text
}

fn track_names(client: &mut Client<InProcess>) -> Vec<String> {
    let shell = &client.transport().shell;
    let document = shell.editor().document();
    document
        .top_level()
        .map(|id| document.track(id).unwrap().name().to_owned())
        .collect()
}

#[test]
fn the_server_says_what_it_is_and_offers_a_few_tools_and_the_guide() {
    let mut client = Client::new(InProcess::new(Agent::headless(false), Shell::headless()));
    let init = client.initialize("test-agent").unwrap();
    assert_eq!(init["protocolVersion"], "2025-11-25");
    assert_eq!(init["serverInfo"]["name"], "beatlayer");
    assert!(init["capabilities"]["tools"].is_object());
    assert!(init["capabilities"]["resources"].is_object());
    assert!(init["instructions"].as_str().unwrap().contains(guide::URI));

    let names: Vec<String> = client
        .tools()
        .unwrap()
        .iter()
        .map(|tool| {
            assert_eq!(tool["inputSchema"]["type"], "object", "{tool}");
            assert!(!tool["description"].as_str().unwrap().is_empty());
            tool["name"].as_str().unwrap().to_owned()
        })
        .collect();
    assert_eq!(
        names,
        [
            "list_commands",
            "describe_command",
            "run",
            "query",
            "transport",
            "schedule",
            "checkpoint",
            "preview",
            "analyze",
            "capture_view"
        ]
    );

    let resources = client.request("resources/list", json!({})).unwrap();
    assert_eq!(resources["resources"][0]["uri"], guide::URI);
    let text = client.read_resource(guide::URI).unwrap();
    assert_eq!(text, guide::TEXT);
    assert!(text.starts_with("# Beatlayer agent guide"));
    assert!(
        client
            .read_resource("beatlayer://nothing")
            .unwrap_err()
            .contains("-32002")
    );
    assert!(client.request("nothing/here", json!({})).is_err());
    assert_eq!(client.request("ping", json!({})).unwrap(), json!({}));
    // An unknown tool is a protocol error, not a tool failure.
    assert!(client.call("make_hit", json!({})).is_err());
}

#[test]
fn older_protocol_versions_are_spoken_and_unknown_ones_get_the_newest() {
    for (asked, agreed) in [
        ("2025-03-26", "2025-03-26"),
        ("2025-06-18", "2025-06-18"),
        ("2099-01-01", "2025-11-25"),
    ] {
        let mut client = Client::new(InProcess::new(Agent::headless(false), Shell::headless()));
        let result = client
            .request(
                "initialize",
                json!({ "protocolVersion": asked, "capabilities": {},
                        "clientInfo": { "name": "old" } }),
            )
            .unwrap();
        assert_eq!(result["protocolVersion"], agreed);
        let tool = client.call("query", json!({})).unwrap();
        // Structured results came in 2025-06-18; older clients read text.
        assert_eq!(tool.structured.is_some(), agreed != "2025-03-26", "{asked}");
        assert_eq!(tool.value()["counts"]["tracks"], 7);
    }
}

#[test]
fn discovery_lists_areas_searches_and_describes_with_schemas() {
    let mut client = headless(false);
    let areas = call(&mut client, "list_commands", json!({}));
    for area in [
        "track",
        "clip",
        "mixer",
        "device",
        "song",
        "set",
        "edit",
        "agent",
        "transport",
    ] {
        assert!(areas["areas"][area].as_u64().unwrap() > 0, "{area}");
    }
    assert_eq!(
        areas["commands"],
        areas["areas"]
            .as_object()
            .unwrap()
            .values()
            .map(|n| n.as_u64().unwrap())
            .sum::<u64>()
    );

    let tracks = call(&mut client, "list_commands", json!({ "area": "track" }));
    let ids: Vec<&str> = tracks["commands"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["command"].as_str().unwrap())
        .collect();
    assert_eq!(
        ids,
        [
            "track.add_take_lane",
            "track.bounce_in_place",
            "track.bounce_to_new_track",
            "track.clear_frozen",
            "track.copy",
            "track.create",
            "track.create_bounced_track",
            "track.cut",
            "track.delete",
            "track.duplicate",
            "track.duplicate_without_clips",
            "track.freeze",
            "track.group",
            "track.move",
            "track.paste",
            "track.paste_bounced_audio",
            "track.rename",
            "track.set_bounced",
            "track.set_devices_enabled",
            "track.set_frozen",
            "track.set_tuning_bypass",
            "track.unfreeze",
            "track.ungroup"
        ]
    );
    let delete = tracks["commands"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["command"] == "track.delete")
        .unwrap();
    assert_eq!(delete["permission"], "destructive");
    assert_eq!(delete["access"], "refused to agents");

    let found = call(&mut client, "list_commands", json!({ "search": "volume" }));
    let ids: Vec<&Value> = found["commands"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| &c["command"])
        .collect();
    assert!(ids.contains(&&json!("mixer.set_volume")), "{ids:?}");
    assert!(
        call_err(&mut client, "list_commands", json!({ "area": "nowhere" }))
            .contains("no commands")
    );

    let clip = call(
        &mut client,
        "describe_command",
        json!({ "command": "clip.create" }),
    );
    assert_eq!(clip["id"], "clip.create");
    assert_eq!(clip["permission"], "edit");
    assert_eq!(clip["access"], "runs");
    assert_eq!(clip["args"]["type"], "object");
    assert!(clip["args"]["properties"]["clip"].is_object());
    assert!(clip.get("bindings").is_none());
    assert!(
        call_err(
            &mut client,
            "describe_command",
            json!({ "command": "clip.explode" })
        )
        .contains("list_commands")
    );

    // The transport's playback commands are the engine's, and listed with
    // the loop's, which are the document's, and the shell's.
    let transport = call(&mut client, "list_commands", json!({ "area": "transport" }));
    let ids: Vec<&str> = transport["commands"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["command"].as_str().unwrap())
        .collect();
    assert_eq!(ids, TRANSPORT_COMMANDS);
    let play = call(
        &mut client,
        "describe_command",
        json!({ "command": "transport.play" }),
    );
    assert_eq!(play["permission"], "read");
    assert_eq!(play["access"], "runs");

    // Agents may never change their own permissions.
    let autonomy = call(
        &mut client,
        "describe_command",
        json!({ "command": "agent.set_autonomous" }),
    );
    assert_eq!(autonomy["access"], "refused to agents");
}

#[test]
fn a_batch_is_one_undo_step_labelled_with_the_agent_and_ids_carry_across_calls() {
    let mut client = headless(false);
    let result = call(
        &mut client,
        "run",
        json!({
            "label": "Add a bass track with a clip",
            "commands": [
                { "command": "track.create",
                  "args": { "id": {"$new": "bass"}, "kind": "midi", "name": "Bass", "index": 0 } },
                { "command": "clip.create",
                  "args": { "track": {"$ref": "bass"},
                            "clip": { "id": {"$new": "riff"}, "name": "Riff", "start": 0,
                                      "length": 2_822_400, "mute": false,
                                      "data": { "midi": { "notes": [] } } } } },
                { "command": "mixer.set_volume", "args": { "track": {"$ref": "bass"}, "volume": 0.5 } },
            ],
        }),
    );
    assert_eq!(result["ran"], 3);
    assert_eq!(
        result["undo_step"],
        "test-agent: Add a bass track with a clip"
    );
    let bass = result["ids"]["bass"].as_u64().unwrap();
    assert!(result["ids"]["riff"].as_u64().unwrap() > bass);

    let shell = &client.transport().shell;
    assert_eq!(
        shell.editor().undo_history().collect::<Vec<_>>(),
        ["test-agent: Add a bass track with a clip"]
    );
    assert!(shell.is_dirty());

    // A later call refers to the ID by its name, and an unnamed batch is
    // named after its commands.
    let renamed = call(
        &mut client,
        "run",
        json!({ "commands": [
            { "command": "track.rename", "args": { "track": {"$ref": "bass"}, "name": "Sub" } },
            { "command": "clip.rename", "args": { "clip": {"$ref": "riff"}, "name": "Drop" } },
        ] }),
    );
    assert_eq!(
        renamed["undo_step"],
        "test-agent: Rename Track and Rename Clip"
    );
    assert_eq!(track_names(&mut client)[0], "Sub");

    // Undo through the agent API takes the whole step back.
    call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "edit.undo" }] }),
    );
    assert_eq!(track_names(&mut client)[0], "Bass");
    call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "edit.undo" }] }),
    );
    assert!(!track_names(&mut client).contains(&"Bass".to_owned()));
}

#[test]
fn a_failing_batch_changes_nothing_and_names_the_command() {
    let mut client = headless(false);
    let before = client.transport().shell.editor().document().clone();
    let message = call_err(
        &mut client,
        "run",
        json!({ "commands": [
            { "command": "track.create",
              "args": { "id": {"$new": "a"}, "kind": "audio", "name": "A", "index": 0 } },
            { "command": "mixer.set_pan", "args": { "track": {"$ref": "a"}, "pan": 3.0 } },
        ] }),
    );
    assert!(
        message.starts_with("command 2 (`mixer.set_pan`)"),
        "{message}"
    );
    assert!(message.contains("maximum"), "{message}");
    let message = call_err(
        &mut client,
        "run",
        json!({ "commands": [
            { "command": "track.create",
              "args": { "id": {"$new": "b"}, "kind": "audio", "name": "B", "index": 0 } },
            { "command": "mixer.set_arm",
              "args": { "track": 1, "arm": true } },
        ] }),
    );
    assert!(message.contains("Nothing changed"), "{message}");
    let shell = &client.transport().shell;
    assert_eq!(shell.editor().document(), &before);
    assert!(!shell.editor().can_undo());
    assert!(!shell.is_dirty());

    // Batches hold document edits only, and every command must exist.
    let message = call_err(
        &mut client,
        "run",
        json!({ "commands": [
            { "command": "track.rename", "args": { "track": 2, "name": "X" } },
            { "command": "set.save", "args": {} },
        ] }),
    );
    assert!(message.contains("only document edits"), "{message}");
    assert!(call_err(&mut client, "run", json!({ "commands": [] })).contains("empty"));
    assert!(
        call_err(
            &mut client,
            "run",
            json!({ "commands": [{ "command": "track.fly" }] })
        )
        .contains("no command")
    );
    assert!(
        call_err(
            &mut client,
            "run",
            json!({ "commands": [{ "command": "track.rename", "args": { "track": {"$ref": "nope"}, "name": "" } }] })
        )
        .contains("names no ID")
    );
}

#[test]
fn a_batch_that_leaves_the_song_as_it_was_records_no_undo_step() {
    let mut client = headless(false);
    let result = call(
        &mut client,
        "run",
        json!({ "commands": [
            { "command": "mixer.set_volume", "args": { "track": 2, "volume": 0.5 } },
            { "command": "mixer.set_volume", "args": { "track": 2, "volume": 1.0 } },
        ] }),
    );
    assert_eq!(result["ran"], 2);
    assert_eq!(result["undo_step"], Value::Null);
    let shell = &client.transport().shell;
    assert!(!shell.editor().can_undo());
    assert!(!shell.is_dirty());
}

#[test]
fn a_dry_run_checks_the_batch_on_the_song_and_changes_nothing() {
    let mut client = headless(false);
    let before = client.transport().shell.editor().document().clone();
    let result = call(
        &mut client,
        "run",
        json!({
            "dry_run": true,
            "commands": [
                { "command": "track.create",
                  "args": { "id": {"$new": "a"}, "kind": "audio", "name": "A", "index": 0 } },
                { "command": "mixer.set_volume", "args": { "track": {"$ref": "a"}, "volume": 0.25 } },
            ],
        }),
    );
    assert_eq!(result["dry_run"], true);
    assert_eq!(result["valid"], true);
    assert_eq!(result["would"], "run");
    assert_eq!(result["results"].as_array().unwrap().len(), 2);
    assert_eq!(result["results"][0]["changes"]["layout"], true);

    let invalid = call(
        &mut client,
        "run",
        json!({ "dry_run": true, "commands": [
            { "command": "mixer.set_arm", "args": { "track": 1, "arm": true } },
        ] }),
    );
    assert_eq!(invalid["valid"], false);
    assert!(invalid["error"].as_str().unwrap().contains("command 1"));

    // A destructive command is checked too, and says it would be refused.
    let refused = call(
        &mut client,
        "run",
        json!({ "dry_run": true, "commands": [{ "command": "track.delete", "args": { "track": 2 } }] }),
    );
    assert_eq!(refused["valid"], true);
    assert_eq!(refused["would"], "be refused");

    let shell = &client.transport().shell;
    assert_eq!(shell.editor().document(), &before);
    assert!(!shell.editor().can_undo());
    assert!(!shell.is_dirty());
    // A dry run's $new names aren't kept for later calls.
    assert!(
        call_err(
            &mut client,
            "run",
            json!({ "commands": [{ "command": "track.rename", "args": { "track": {"$ref": "a"}, "name": "" } }] })
        )
        .contains("names no ID")
    );
}

#[test]
fn headless_refuses_what_would_ask_unless_autonomous_and_never_runs_agent_commands() {
    let mut client = headless(false);
    let message = call_err(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "track.delete", "args": { "track": 2 } }] }),
    );
    assert!(message.contains("--autonomous"), "{message}");
    assert!(message.contains("destructive"), "{message}");
    assert_eq!(track_names(&mut client).len(), 4);
    let message = call_err(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "set.save_as", "args": { "path": "/tmp/x.blset" } }] }),
    );
    assert!(message.contains("external"), "{message}");

    let mut free = headless(true);
    let result = call(
        &mut free,
        "run",
        json!({ "commands": [{ "command": "track.delete", "args": { "track": 2 } }] }),
    );
    assert_eq!(result["ran"], 1);
    assert_eq!(track_names(&mut free).len(), 3);

    for client in [&mut client, &mut free] {
        let message = call_err(
            client,
            "run",
            json!({ "commands": [{ "command": "agent.set_autonomous",
                                   "args": { "client": "test-agent", "on": true } }] }),
        );
        assert!(message.contains("only a person"), "{message}");
    }
}

#[test]
fn in_the_app_a_destructive_command_asks_the_person_and_runs_only_if_allowed() {
    let mut client = Client::new(InProcess::new(Agent::for_app(), Shell::headless()));
    // The person allows the agent to connect...
    client.transport().answer = Box::new(|q| matches!(q.kind, QuestionKind::Connect { .. }));
    client.initialize("test-agent").unwrap();
    assert_eq!(client.transport().asked.len(), 1);
    assert!(client.transport().asked[0].contains("wants to control Beatlayer"));
    assert!(
        client
            .transport()
            .shell
            .agent_settings()
            .client("test-agent")
            .is_some()
    );

    // ...and declines the delete.
    let result = client
        .call(
            "run",
            json!({ "commands": [{ "command": "track.delete", "args": { "track": 2 } }] }),
        )
        .unwrap();
    assert!(result.is_error);
    assert!(result.text.contains("declined"), "{}", result.text);
    let asked = client.transport().asked.last().unwrap().clone();
    assert!(asked.contains("Delete Track (destructive)"), "{asked}");
    assert!(asked.contains("track \u{201c}1 MIDI\u{201d}"), "{asked}");
    assert_eq!(track_names(&mut client).len(), 4);

    // Allowed, it runs.
    client.transport().answer = Box::new(|_| true);
    let result = call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "track.delete", "args": { "track": 2 } }] }),
    );
    assert_eq!(result["asked_first"], true);
    assert_eq!(track_names(&mut client).len(), 3);

    // Edits don't ask.
    let asked = client.transport().asked.len();
    call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "track.rename", "args": { "track": 3, "name": "Keys" } }] }),
    );
    assert_eq!(client.transport().asked.len(), asked);

    // Autonomous, it doesn't ask.
    client
        .transport()
        .shell
        .run(
            "agent.set_autonomous",
            json!({ "client": "test-agent", "on": true }),
        )
        .unwrap();
    call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "track.delete", "args": { "track": 3 } }] }),
    );
    assert_eq!(client.transport().asked.len(), asked);

    // A forgotten agent's session ends.
    client
        .transport()
        .shell
        .run("agent.forget_client", json!({ "client": "test-agent" }))
        .unwrap();
    let error = client.call("query", json!({})).unwrap_err();
    assert!(error.contains("withdrew"), "{error}");
}

#[test]
fn an_agent_the_person_refuses_cannot_connect() {
    let mut client = Client::new(InProcess::new(Agent::for_app(), Shell::headless()));
    let error = client.initialize("stranger").unwrap_err();
    assert!(error.contains("didn't allow"), "{error}");
    assert!(client.transport().shell.agent_settings().clients.is_empty());
}

#[test]
fn query_reads_by_id_and_path_with_pages_for_large_projects() {
    let mut client = headless(false);
    let overview = call(&mut client, "query", json!({}));
    assert_eq!(overview["set"]["name"], "Untitled");
    assert_eq!(overview["tempo"]["bpm_at_start"], 120.0);
    assert_eq!(overview["time_signature_at_start"], "4/4");
    assert_eq!(overview["counts"]["tracks"], 7);
    assert_eq!(overview["tracks"]["total"], 7);
    assert_eq!(overview["tracks"]["items"][0]["name"], "1 MIDI");

    // 300 more tracks with a clip each: summaries come a page at a time.
    let commands: Vec<Value> = (0..300)
        .flat_map(|i| {
            let name = format!("t{i}");
            [
                json!({ "command": "track.create",
                        "args": { "id": {"$new": name}, "kind": "midi",
                                  "name": format!("Track {i}"), "index": 4 + i } }),
                json!({ "command": "clip.create",
                        "args": { "track": {"$ref": name},
                                  "clip": { "id": {"$new": format!("c{i}")}, "name": "Loop",
                                            "start": 2_822_400 * i, "length": 705_600, "mute": false,
                                            "data": { "midi": { "notes": [
                                                { "key": 60, "velocity": 100, "start": 0, "length": 176_400 }
                                            ] } } } } }),
            ]
        })
        .collect();
    call(
        &mut client,
        "run",
        json!({ "label": "300 tracks", "commands": commands }),
    );
    let overview = call(&mut client, "query", json!({}));
    assert_eq!(overview["counts"]["tracks"], 307);
    assert_eq!(overview["tracks"]["items"].as_array().unwrap().len(), 50);
    assert_eq!(overview["tracks"]["next_offset"], 50);
    let page = call(
        &mut client,
        "query",
        json!({ "path": "tracks", "offset": 300, "limit": 10 }),
    );
    assert_eq!(page["tracks"]["items"].as_array().unwrap().len(), 7);
    assert_eq!(page["tracks"]["next_offset"], Value::Null);

    let track = call(&mut client, "query", json!({ "path": "track \"Track 7\"" }));
    assert_eq!(track["kind"], "midi");
    assert_eq!(track["clips"]["total"], 1);
    let clip = &track["clips"]["items"][0];
    assert_eq!(clip["start"]["bbs"], "8.1.1");
    assert_eq!(clip["length"]["quarters"], 1.0);
    let clip_id = clip["id"].as_u64().unwrap();

    let by_path = call(
        &mut client,
        "query",
        json!({ "path": "track \"Track 7\" / clip #1", "detail": "full" }),
    );
    assert_eq!(by_path["id"], clip_id);
    assert_eq!(by_path["content"]["notes"]["items"][0]["key"], 60);
    let by_id = call(&mut client, "query", json!({ "id": clip_id }));
    assert_eq!(by_id["name"], "Loop");
    assert_eq!(by_id["content"]["notes"], 1);

    // Names that repeat ask for an ID; missing things say so.
    let message = call_err(&mut client, "query", json!({ "path": "clip \"Loop\"" }));
    assert!(message.contains("300 clips match"), "{message}");
    assert!(call_err(&mut client, "query", json!({ "id": 999_999 })).contains("999999"));
    assert!(call_err(&mut client, "query", json!({ "path": "track Bass" })).contains("quotes"));
    let main = call(&mut client, "query", json!({ "path": "track main" }));
    assert_eq!(main["kind"], "main");
    let tempo = call(&mut client, "query", json!({ "path": "tempo" }));
    assert!(tempo["tempo"].is_array() || tempo["tempo"].is_object());
    let history = call(&mut client, "query", json!({ "path": "history" }));
    assert_eq!(history["undo"]["items"][0], "test-agent: 300 tracks");
}

#[test]
fn checkpoints_save_restore_as_one_undo_step_and_ask_before_removing() {
    let mut client = app();
    call(
        &mut client,
        "checkpoint",
        json!({ "action": "save", "name": "start" }),
    );
    call(
        &mut client,
        "run",
        json!({ "commands": [
            { "command": "track.create",
              "args": { "id": {"$new": "x"}, "kind": "audio", "name": "Extra", "index": 4 } },
        ] }),
    );
    call(
        &mut client,
        "checkpoint",
        json!({ "action": "save", "name": "with extra" }),
    );
    let list = call(&mut client, "checkpoint", json!({ "action": "list" }));
    assert_eq!(list["checkpoints"].as_array().unwrap().len(), 2);
    assert_eq!(list["checkpoints"][1]["tracks"], 8);
    assert_eq!(list["checkpoints"][1]["by"], "test-agent");

    // Going back removes the track, so the person is asked.
    let asked = client.transport().asked.len();
    let restored = call(
        &mut client,
        "checkpoint",
        json!({ "action": "restore", "name": "start" }),
    );
    assert_eq!(client.transport().asked.len(), asked + 1);
    assert!(
        client
            .transport()
            .asked
            .last()
            .unwrap()
            .contains("Restore checkpoint"),
        "{:?}",
        client.transport().asked
    );
    assert_eq!(restored["changes"]["removed"], 1);
    assert_eq!(track_names(&mut client).len(), 4);
    assert_eq!(
        client.transport().shell.editor().undo_history().next(),
        Some("test-agent: Restore checkpoint \u{201c}start\u{201d}")
    );

    // Forward again only adds, so it's an edit and doesn't ask.
    call(
        &mut client,
        "checkpoint",
        json!({ "action": "restore", "name": "with extra" }),
    );
    assert_eq!(client.transport().asked.len(), asked + 1);
    assert_eq!(track_names(&mut client).len(), 5);

    // The person can undo a restore.
    call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "edit.undo" }] }),
    );
    assert_eq!(track_names(&mut client).len(), 4);

    call(
        &mut client,
        "checkpoint",
        json!({ "action": "delete", "name": "start" }),
    );
    assert!(
        call_err(
            &mut client,
            "checkpoint",
            json!({ "action": "restore", "name": "start" })
        )
        .contains("no checkpoint")
    );
    assert!(call_err(&mut client, "checkpoint", json!({ "action": "save" })).contains("name"));
}

/// Every transport command, the engine's and the loop's, by ID.
/// The transport's commands: the engine's playback commands, the loop's
/// (the document's), and the shell's Return to Start and state (WP-1.18).
const TRANSPORT_COMMANDS: [&str; 21] = [
    "transport.continue",
    "transport.insert_marker_to_playhead",
    "transport.locate",
    "transport.play",
    "transport.play_from_insert_marker",
    "transport.play_stop",
    "transport.return_to_start",
    "transport.set_loop",
    "transport.set_loop_enabled",
    "transport.set_metronome_count_in",
    "transport.set_metronome_enabled",
    "transport.set_metronome_only_while_recording",
    "transport.set_metronome_rhythm",
    "transport.set_metronome_sound",
    "transport.set_midi_overdub",
    "transport.set_punch_in",
    "transport.set_punch_out",
    "transport.set_recording",
    "transport.state",
    "transport.stop",
    "transport.stop_recording",
];

const BAR: i64 = 2_822_400;

#[test]
fn without_an_engine_the_transport_says_so_and_the_loop_still_changes_the_set() {
    let mut client = headless(false);
    let status = call(&mut client, "transport", json!({ "action": "status" }));
    assert_eq!(status["engine_plays_here"], false);
    assert_eq!(status["playing"], Value::Null);
    assert_eq!(status["position"], Value::Null);
    assert_eq!(status["transport_commands"], json!(TRANSPORT_COMMANDS));
    assert_eq!(
        status["loop"],
        json!({
            "enabled": false,
            "start": { "bbs": "1.1.1", "ticks": 0 },
            "end": { "bbs": "5.1.1", "ticks": 4 * BAR },
            "length": 4 * BAR,
        })
    );

    // Nothing plays the song here, so playback commands are refused,
    // through the tool or `run`, and nothing goes to an engine.
    let message = call_err(&mut client, "transport", json!({ "action": "play" }));
    assert!(message.contains("no engine is attached"), "{message}");
    assert!(message.contains("Nothing changed"), "{message}");
    let message = call_err(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "transport.stop" }] }),
    );
    assert!(message.contains("`transport.stop`"), "{message}");
    assert!(client.transport().agent.take_engine_messages().is_empty());

    // The loop belongs to the set, engine or not: each change is an undo
    // step with the agent's name.
    call(
        &mut client,
        "transport",
        json!({ "action": "set_loop", "args": { "start": BAR, "length": 2 * BAR } }),
    );
    call(
        &mut client,
        "transport",
        json!({ "action": "set_loop_enabled", "args": { "enabled": true } }),
    );
    let status = call(&mut client, "transport", json!({ "action": "status" }));
    assert_eq!(status["loop"]["enabled"], true);
    assert_eq!(status["loop"]["start"]["bbs"], "2.1.1");
    assert_eq!(status["loop"]["end"]["bbs"], "4.1.1");
    let history = call(&mut client, "query", json!({ "path": "history" }));
    assert_eq!(
        history["undo"]["items"],
        json!([
            "test-agent: Arrangement Loop On/Off",
            "test-agent: Set Loop Brace"
        ])
    );
    // A brace that can't be is refused and changes nothing.
    let message = call_err(
        &mut client,
        "transport",
        json!({ "action": "set_loop", "args": { "start": BAR, "length": 0 } }),
    );
    assert!(message.contains("Nothing changed"), "{message}");
    // The loop's commands are document edits, so they batch with others.
    let batch = call(
        &mut client,
        "run",
        json!({ "label": "Loop the verse", "commands": [
            { "command": "transport.set_loop", "args": { "start": 0, "length": BAR } },
            { "command": "track.rename", "args": { "track": 2, "name": "Verse" } },
        ] }),
    );
    assert_eq!(batch["undo_step"], "test-agent: Loop the verse");
    assert!(client.transport().agent.take_engine_messages().is_empty());
}

#[test]
fn with_an_engine_transport_commands_go_to_its_queue_in_order() {
    let mut client = headless(false);
    client.transport().agent.set_engine_attached(true);
    let status = call(&mut client, "transport", json!({ "action": "status" }));
    assert_eq!(status["engine_plays_here"], true);
    // The engine hasn't said where the playhead is yet.
    assert_eq!(status["playing"], Value::Null);

    // Playback changes nothing in the set, so it's no undo step, and it
    // runs without asking (the `read` class) even headless.
    let played = call(&mut client, "transport", json!({ "action": "play" }));
    assert_eq!(played["ran"], 1);
    assert_eq!(played["undo_step"], Value::Null);
    call(
        &mut client,
        "transport",
        json!({ "action": "locate", "at": "3.2.1" }),
    );
    let scheduled = call(
        &mut client,
        "schedule",
        json!({ "action": "add", "at": "5.1.1", "command": "transport.stop" }),
    );
    assert_eq!(scheduled["sample_accurate"], true);
    call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "transport.play_stop" }] }),
    );
    call(
        &mut client,
        "transport",
        json!({ "action": "locate", "args": { "position": 0 } }),
    );
    call(&mut client, "transport", json!({ "action": "continue" }));
    call(
        &mut client,
        "transport",
        json!({ "action": "insert_marker_to_playhead" }),
    );
    call(&mut client, "transport", json!({ "action": "stop" }));
    // A dry run checks and sends nothing.
    let dry = call(
        &mut client,
        "run",
        json!({ "dry_run": true, "commands": [{ "command": "transport.play" }] }),
    );
    assert_eq!(dry["valid"], true);

    assert_eq!(
        client.transport().agent.take_engine_messages(),
        [
            EngineMessage::Transport(TransportCommand::Play),
            EngineMessage::Transport(TransportCommand::Locate(Ticks(2 * BAR + 705_600))),
            EngineMessage::Schedule(ScheduledCommand {
                id: 1,
                at: Ticks(4 * BAR),
                action: Action::Transport(TransportCommand::Stop),
            }),
            EngineMessage::Transport(TransportCommand::PlayStop),
            EngineMessage::Transport(TransportCommand::Locate(Ticks::ZERO)),
            EngineMessage::Transport(TransportCommand::Continue),
            EngineMessage::Transport(TransportCommand::InsertMarkerToPlayhead),
            EngineMessage::Transport(TransportCommand::Stop),
        ]
    );

    // Mistakes show now, and send nothing.
    for (args, expected) in [
        (
            json!({ "action": "play", "at": "2.1.1" }),
            "`at` is for `locate`",
        ),
        (
            json!({ "action": "locate", "at": "2.1.1", "args": { "position": 0 } }),
            "not both",
        ),
        (
            json!({ "action": "locate", "at": "2.9.1" }),
            "isn't a position",
        ),
        (
            json!({ "action": "locate", "args": { "position": "2.1.1" } }),
            "invalid arguments",
        ),
        (json!({ "action": "rewind" }), "invalid arguments"),
    ] {
        let message = call_err(&mut client, "transport", args.clone());
        assert!(message.contains(expected), "{args}: {message}");
    }
    let message = call_err(
        &mut client,
        "schedule",
        json!({ "action": "add", "at": "5.1.1", "command": "transport.play" }),
    );
    assert!(message.contains("would do nothing"), "{message}");
    assert!(client.transport().agent.take_engine_messages().is_empty());
    // Nothing the transport did is in the undo history.
    let history = call(&mut client, "query", json!({ "path": "history" }));
    assert_eq!(history["undo"]["items"], json!([]));
}

#[test]
fn the_metronome_settings_run_through_the_agent_api() {
    // Spec: metronome R8
    let mut client = headless(true);
    call(
        &mut client,
        "run",
        json!({ "commands": [
            { "command": "transport.set_metronome_enabled", "args": { "enabled": true } },
            { "command": "transport.set_metronome_count_in", "args": { "count_in": "2_bars" } },
            { "command": "transport.set_metronome_sound", "args": { "sound": "wood" } },
            { "command": "transport.set_metronome_rhythm", "args": { "rhythm": "1/8" } },
            { "command": "transport.set_metronome_only_while_recording",
              "args": { "only_while_recording": true } },
        ] }),
    );
    let settings = client
        .transport()
        .shell
        .editor()
        .document()
        .settings()
        .metronome;
    assert_eq!(
        settings,
        bl_model::MetronomeSettings {
            enabled: true,
            count_in: CountIn::Bars2,
            sound: MetronomeSound::Wood,
            rhythm: MetronomeRhythm::Eighth,
            only_while_recording: true,
        }
    );
}

#[test]
fn the_punch_switches_and_the_compensation_run_through_the_agent_api() {
    // Spec: recording R6, R7; WP-2.13
    let mut client = headless(true);
    call(
        &mut client,
        "run",
        json!({ "commands": [
            { "command": "transport.set_punch_in", "args": { "enabled": true } },
            { "command": "transport.set_punch_out", "args": { "enabled": true } },
        ] }),
    );
    call(
        &mut client,
        "run",
        json!({ "commands": [
            { "command": "audio.configure",
              "args": { "driver_error_compensation_ms": 12.345 } },
        ] }),
    );
    let transport = client.transport();
    let settings = transport.shell.editor().document().settings();
    assert!(settings.punch_in && settings.punch_out);
    assert!(
        !settings.arrangement_loop.enabled,
        "punch needs no Loop switch"
    );
    assert_eq!(
        transport
            .shell
            .audio()
            .settings()
            .driver_error_compensation_ms,
        12.35
    );
    // The switches are saved with the set and undo switches them back.
    call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "edit.undo" }] }),
    );
    assert!(
        !client
            .transport()
            .shell
            .editor()
            .document()
            .settings()
            .punch_out
    );
}

#[test]
fn scheduled_commands_are_checked_when_scheduled_and_wait_for_the_song() {
    let mut client = headless(false);
    let result = call(
        &mut client,
        "schedule",
        json!({ "action": "add", "at": "2.1.1", "command": "mixer.set_mute",
                "args": { "track": 2, "mute": true } }),
    );
    assert_eq!(result["at"], "2.1.1");
    assert_eq!(result["ticks"], 2_822_400);
    assert_eq!(result["sample_accurate"], true);
    let renamed = call(
        &mut client,
        "schedule",
        json!({ "action": "add", "at": "3.2", "command": "track.rename",
                "args": { "track": 2, "name": "Later" } }),
    );
    assert_eq!(renamed["at"], "3.2.1");
    assert_eq!(renamed["sample_accurate"], false);

    // Mistakes show now, not when the song gets there.
    assert!(
        call_err(
            &mut client,
            "schedule",
            json!({ "action": "add", "at": "2.5.1", "command": "mixer.set_mute",
                    "args": { "track": 2, "mute": true } })
        )
        .contains("isn't a position")
    );
    assert!(
        call_err(
            &mut client,
            "schedule",
            json!({ "action": "add", "at": "2.1.1", "command": "mixer.set_arm",
                    "args": { "track": 1, "arm": true } })
        )
        .contains("fails on the song as it is now")
    );
    assert!(
        call_err(
            &mut client,
            "schedule",
            json!({ "action": "add", "at": "2.1.1", "command": "track.delete",
                    "args": { "track": 2 } })
        )
        .contains("--autonomous")
    );

    let list = call(&mut client, "schedule", json!({ "action": "list" }));
    assert_eq!(list["scheduled"].as_array().unwrap().len(), 2);
    assert_eq!(list["scheduled"][0]["state"], "waiting");
    let id = list["scheduled"][1]["id"].as_u64().unwrap();
    call(
        &mut client,
        "schedule",
        json!({ "action": "cancel", "id": id }),
    );
    let messages = client.transport().agent.take_engine_messages();
    assert_eq!(
        messages.len(),
        3,
        "two schedules and a cancel: {messages:?}"
    );
    assert!(matches!(messages[2], bl_agent::EngineMessage::Cancel(i) if i == id));
    // Nothing fired, so the document is as it was.
    assert_eq!(track_names(&mut client)[0], "1 MIDI");
}

#[test]
fn the_guide_s_example_runs_as_written() {
    // From the line after the fence, so a checkout with CRLF line endings
    // (Git for Windows' default) reads it too.
    let fence = guide::TEXT.find("```json").unwrap();
    let start = fence + guide::TEXT[fence..].find('\n').unwrap() + 1;
    let end = start + guide::TEXT[start..].find("```").unwrap();
    let example: Value = serde_json::from_str(&guide::TEXT[start..end]).unwrap();
    let mut client = headless(false);
    let result = call(&mut client, "run", example);
    assert_eq!(result["ran"], 2);
    assert_eq!(
        result["undo_step"],
        "test-agent: Add a bass track with a clip"
    );
    let bass = call(
        &mut client,
        "query",
        json!({ "path": "track \"Bass\" / clip #1", "detail": "full" }),
    );
    assert_eq!(bass["content"]["notes"]["items"][0]["key"], 36);
}

#[test]
fn an_agent_quantises_and_transposes_notes_by_command() {
    // Spec: piano-roll (WP-2.3 part 3). Quantise and transpose reach the
    // Clip View's notes through the same commands the mouse runs, and each
    // call is one undo step labelled with the agent's name (agent-permissions
    // R6).
    let mut client = headless(false);
    let created = call(
        &mut client,
        "run",
        json!({ "label": "A clip of notes", "commands": [
            { "command": "track.create",
              "args": { "id": {"$new": "notes"}, "kind": "midi", "name": "Notes",
                        "index": 0 } },
            { "command": "clip.create",
              "args": { "track": {"$ref": "notes"},
                        "clip": { "id": {"$new": "clip"}, "name": "Notes", "start": 0,
                                  "length": 705_600, "mute": false,
                                  "data": { "midi": { "notes": [
                                      { "key": 60, "velocity": 100, "start": 130,
                                        "length": 480 },
                                      { "key": 120, "velocity": 100, "start": 500,
                                        "length": 480 }
                                  ] } } } } },
        ] }),
    );
    let clip = created["ids"]["clip"].as_u64().unwrap();

    // Quantise the first note to 1/16 lines at 100 percent.
    let quantised = call(
        &mut client,
        "run",
        json!({ "label": "Quantise the notes", "commands": [
            { "command": "note.quantize",
              "args": { "clip": clip, "notes": [0], "grid": 120, "amount": 100 } },
        ] }),
    );
    assert_eq!(quantised["ran"], 1);
    assert_eq!(quantised["undo_step"], "test-agent: Quantise the notes");
    let notes = agent_notes(&mut client);
    assert_eq!(notes[0], (60, 120), "the start landed on the line");

    // Transposing past key 127 is refused and changes nothing; down an
    // octave is one labelled undo step.
    let refused = client
        .call(
            "run",
            json!({ "commands": [
                { "command": "note.move",
                  "args": { "clip": clip, "notes": [1], "semitones": 12 } },
            ] }),
        )
        .unwrap();
    assert!(refused.is_error, "{}", refused.text);
    assert!(
        refused.text.contains("0 to 127"),
        "the refusal names the keys: {}",
        refused.text
    );
    assert_eq!(agent_notes(&mut client)[1].0, 120, "and changes nothing");
    let moved = call(
        &mut client,
        "run",
        json!({ "label": "Transpose down", "commands": [
            { "command": "note.move",
              "args": { "clip": clip, "notes": [1], "semitones": -12 } },
        ] }),
    );
    assert_eq!(moved["undo_step"], "test-agent: Transpose down");
    assert_eq!(agent_notes(&mut client)[1].0, 108, "an octave down");
}

#[test]
fn an_agent_copies_pastes_and_duplicates_notes_by_command() {
    // Spec: piano-roll R28, R29, R31. The note clipboard's commands run by
    // ID through the agent API, and the query tool reads the notes back.
    // The agent is autonomous, so it may run the destructive Cut.
    let mut client = headless(true);
    let created = call(
        &mut client,
        "run",
        json!({ "label": "A clip of notes", "commands": [
            { "command": "track.create",
              "args": { "id": {"$new": "notes"}, "kind": "midi", "name": "Notes",
                        "index": 0 } },
            { "command": "clip.create",
              "args": { "track": {"$ref": "notes"},
                        "clip": { "id": {"$new": "clip"}, "name": "Notes", "start": 0,
                                  "length": 705_600, "mute": false,
                                  "data": { "midi": { "notes": [
                                      { "key": 60, "velocity": 100, "start": 0,
                                        "length": 480 },
                                      { "key": 120, "velocity": 100, "start": 500,
                                        "length": 480 }
                                  ] } } } } },
        ] }),
    );
    let clip = created["ids"]["clip"].as_u64().unwrap();
    // The Clip View shows the clip, so its notes can be selected.
    call(
        &mut client,
        "run",
        json!({ "commands": [
            { "command": "arrangement.select_clips", "args": { "clips": [clip] } },
        ] }),
    );
    // Select the notes, and copy them to the note clipboard. The view's
    // commands run in a call of their own.
    call(
        &mut client,
        "run",
        json!({ "commands": [
            { "command": "clip_view.select_notes",
              "args": { "clip": clip, "notes": [0, 1] } },
        ] }),
    );
    let copied = call(
        &mut client,
        "run",
        json!({ "label": "Copy the notes", "commands": [
            { "command": "note.copy", "args": { "clip": clip } },
        ] }),
    );
    assert_eq!(copied["ran"], 1);
    assert!(
        copied["undo_step"].is_null(),
        "copying changes nothing, so it records no step"
    );
    // The insert marker, and a paste at it.
    call(
        &mut client,
        "run",
        json!({ "commands": [
            { "command": "clip_view.set_insert_marker",
              "args": { "clip": clip, "beat": 600 } },
        ] }),
    );
    // `view.state` reads the insert marker and the time selection back:
    // placing the marker clears the selection.
    let state = call(
        &mut client,
        "run",
        json!({ "commands": [ { "command": "view.state" } ] }),
    );
    let view = &state["results"][0]["result"]["clip_view"];
    assert_eq!(view["insert_marker"], 600);
    assert!(
        view["time_selection"].is_null(),
        "placing the marker clears the time selection"
    );
    let pasted = call(
        &mut client,
        "run",
        json!({ "label": "Paste the notes", "commands": [
            { "command": "note.paste", "args": { "clip": clip } },
        ] }),
    );
    assert_eq!(pasted["undo_step"], "test-agent: Paste the notes");
    // The pasted notes become the time selection, as selecting notes does.
    let state = call(
        &mut client,
        "run",
        json!({ "commands": [ { "command": "view.state" } ] }),
    );
    assert_eq!(
        state["results"][0]["result"]["clip_view"]["time_selection"],
        json!([600, 1580])
    );
    // The query tool reads the notes back: the copies keep their pitches and
    // their spacing.
    let queried = call(
        &mut client,
        "query",
        json!({ "id": clip, "detail": "full" }),
    );
    let notes = queried["content"]["notes"]["items"].as_array().unwrap();
    assert_eq!(notes.len(), 4, "the pasted notes are there: {notes:?}");
    assert_eq!(notes[2]["key"], 60, "the first copy keeps its pitch");
    assert_eq!(notes[2]["start"], 600, "and starts at the insert marker");
    assert_eq!(notes[3]["key"], 120, "the second copy keeps its pitch");
    // A duplicate lays another copy straight after the time selection.
    call(
        &mut client,
        "run",
        json!({ "commands": [
            { "command": "clip_view.select_notes",
              "args": { "clip": clip, "notes": [0, 1] } },
        ] }),
    );
    call(
        &mut client,
        "run",
        json!({ "label": "Duplicate the notes", "commands": [
            { "command": "note.duplicate", "args": { "clip": clip } },
        ] }),
    );
    let queried = call(
        &mut client,
        "query",
        json!({ "id": clip, "detail": "full" }),
    );
    let notes = queried["content"]["notes"]["items"].as_array().unwrap();
    assert_eq!(notes.len(), 6, "and another one: {notes:?}");
    // Cut copies the notes and deletes them.
    call(
        &mut client,
        "run",
        json!({ "commands": [
            { "command": "clip_view.select_notes",
              "args": { "clip": clip, "notes": [0, 1] } },
        ] }),
    );
    call(
        &mut client,
        "run",
        json!({ "label": "Cut the notes", "commands": [
            { "command": "note.cut", "args": { "clip": clip } },
        ] }),
    );
    let queried = call(
        &mut client,
        "query",
        json!({ "id": clip, "detail": "full" }),
    );
    let notes = queried["content"]["notes"]["items"].as_array().unwrap();
    assert_eq!(notes.len(), 4, "the cut notes are gone: {notes:?}");
}

#[test]
fn an_agent_sets_the_note_filters_and_inverts_the_selection_by_command() {
    // Spec: find-and-select-notes R14. Both commands run by ID, and
    // `view.state` reads the bar and the filters back.
    let mut client = headless(false);
    let created = call(
        &mut client,
        "run",
        json!({ "label": "A clip of notes", "commands": [
            { "command": "track.create",
              "args": { "id": {"$new": "notes"}, "kind": "midi", "name": "Notes",
                        "index": 0 } },
            { "command": "clip.create",
              "args": { "track": {"$ref": "notes"},
                        "clip": { "id": {"$new": "clip"}, "name": "Notes", "start": 0,
                                  "length": 2_822_400, "mute": false,
                                  "data": { "midi": { "notes": [
                                      { "key": 60, "velocity": 100, "start": 0,
                                        "length": 705_600 },
                                      { "key": 61, "velocity": 100, "start": 705_600,
                                        "length": 705_600 },
                                      { "key": 72, "velocity": 100, "start": 1_411_200,
                                        "length": 705_600 }
                                  ] } } } } },
        ] }),
    );
    let clip = created["ids"]["clip"].as_u64().unwrap();
    call(
        &mut client,
        "run",
        json!({ "commands": [
            { "command": "arrangement.select_clips", "args": { "clips": [clip] } },
        ] }),
    );
    let ran = call(
        &mut client,
        "run",
        json!({ "commands": [
            { "command": "clip_view.note_filters",
              "args": { "visible": true, "shown": "pitch", "pitch": { "classes": [0] } } },
        ] }),
    );
    let result = &ran["results"][0]["result"];
    assert_eq!(result["selected"], json!([0, 2]), "C in both octaves");
    assert_eq!(result["filters"]["pitch"]["on"], true);
    assert!(
        ran["undo_step"].is_null(),
        "selecting is never an undo step"
    );
    let state = call(
        &mut client,
        "run",
        json!({ "commands": [ { "command": "view.state" } ] }),
    );
    let view = &state["results"][0]["result"]["clip_view"];
    assert_eq!(view["filters"]["visible"], true);
    assert_eq!(view["filters"]["pitch"]["classes"], json!([0]));
    assert_eq!(view["selected"], json!([0, 2]));
    let inverted = call(
        &mut client,
        "run",
        json!({ "commands": [ { "command": "clip_view.invert_selection" } ] }),
    );
    assert_eq!(inverted["results"][0]["result"]["selected"], json!([1]));
    assert!(inverted["undo_step"].is_null());
}

/// The MIDI clips' notes, as (key, start) pairs by start then key.
fn agent_notes(client: &mut Client<InProcess>) -> Vec<(u8, i64)> {
    let document = client.transport().shell.editor().document();
    let mut notes = Vec::new();
    for track in document.tracks() {
        for clip in track.clips() {
            if let bl_model::ClipData::Midi(midi) = &clip.data {
                notes.extend(midi.notes.iter().map(|note| (note.key, note.start.get())));
            }
        }
    }
    notes.sort_by_key(|note| (note.1, note.0));
    notes
}

#[test]
fn malformed_messages_get_json_rpc_errors_and_notifications_get_nothing() {
    let mut transport = InProcess::new(Agent::headless(false), Shell::headless());
    use bl_agent::client::Transport as _;
    let reply = transport
        .send(&json!({ "id": 1, "method": "ping" }))
        .unwrap()
        .unwrap();
    assert_eq!(reply["error"]["code"], -32_600);
    let reply = transport.send(&json!([1, 2])).unwrap().unwrap();
    assert!(
        reply["error"]["message"]
            .as_str()
            .unwrap()
            .contains("batches")
    );
    assert_eq!(
        transport
            .send(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }))
            .unwrap(),
        None
    );
    let reply = transport
        .send(&json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {} }))
        .unwrap()
        .unwrap();
    assert_eq!(reply["error"]["code"], -32_602);
}

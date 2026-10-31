//! Agent permissions per call (WP-1.15b), headless through the agent API:
//! an edit that would delete or shorten clips it doesn't act on (moving,
//! copying, resizing, nudging, duplicating, or pasting clips over others,
//! or dropping files on them) asks first as destructive, decided from its
//! arguments against the set as it is, while the same edit into empty space
//! doesn't; an agent's calls wait for the person's gesture to end and land
//! after it, each its own undo step; and the rights that let an agent skip
//! asking go with its own token. The rules are in the agent permissions
//! spec.

use bl_agent::client::{Client, InProcess};
use bl_agent::{Agent, Credential, EngineMessage, Outcome, QuestionKind};
use bl_commands::Permission;
use bl_core::time::Frames;
use bl_engine::{Report, TransportCommand};
use bl_shell::{GesturePhase, Invocation, Shell};
use serde_json::{Value, json};

/// A bar of 4/4, in ticks.
const BAR: u64 = 2_822_400;

/// The app's agent API on a headless shell: the person allows the agent
/// called `editor` to connect, which gives it its own approval, and
/// declines everything else unless a test says otherwise.
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

fn track(client: &mut Client<InProcess>, name: &str) -> u64 {
    call(
        client,
        "query",
        json!({ "path": format!("track \"{name}\"") }),
    )["id"]
        .as_u64()
        .unwrap()
}

/// Puts a clip called `name` on `track` (MIDI or audio, as the track is)
/// from `bar` (from 0) for `bars`, and returns its ID.
fn clip(client: &mut Client<InProcess>, track: u64, name: &str, bar: u64, bars: u64) -> u64 {
    let audio = call(client, "query", json!({ "id": track }))["kind"] == "audio";
    let data = if audio {
        json!({ "audio": { "media": "11".repeat(32) } })
    } else {
        json!({ "midi": { "notes": [] } })
    };
    let result = call(
        client,
        "run",
        json!({ "commands": [{ "command": "clip.create", "args": {
            "track": track,
            "clip": { "id": { "$new": name }, "name": name, "start": bar * BAR,
                      "length": bars * BAR, "mute": false, "data": data } } }] }),
    );
    result["ids"][name].as_u64().unwrap()
}

fn exists(client: &mut Client<InProcess>, clip: u64) -> bool {
    client
        .call("query", json!({ "id": clip }))
        .unwrap()
        .ok()
        .is_ok()
}

/// How an edit went: whether it asked the person (who declined), and the
/// classes a dry run gave it first.
#[derive(Debug, PartialEq)]
struct Went {
    asked: bool,
    permission: String,
}

/// Dry-runs `command`, then runs it with the person declining any
/// question; checks that a declined call changed nothing.
fn attempt(client: &mut Client<InProcess>, command: &str, args: &Value) -> Went {
    let dry = call(
        client,
        "run",
        json!({ "commands": [{ "command": command, "args": args }], "dry_run": true }),
    );
    let asked_before = client.transport().asked.len();
    let before = client.transport().shell.editor().document().clone();
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
        assert_eq!(
            *client.transport().shell.editor().document(),
            before,
            "{command}: a declined call changes nothing"
        );
        assert_eq!(dry["would"], "ask the person first", "{dry}");
    } else {
        assert!(!result.is_error, "{command}: {}", result.text);
        assert_eq!(dry["would"], "run", "{dry}");
    }
    Went {
        asked,
        permission: dry["permission"].as_str().unwrap_or_default().to_owned(),
    }
}

/// A set with clip A at bar 1 on the first MIDI track and clip B at bar
/// `b` (from 0), each a bar long.
fn two_clips(b: u64) -> (Client<InProcess>, u64, u64, u64) {
    let mut client = app();
    let midi = track(&mut client, "1 MIDI");
    let a = clip(&mut client, midi, "A", 0, 1);
    let b = clip(&mut client, midi, "B", b, 1);
    (client, midi, a, b)
}

#[test]
fn an_agent_s_clearing_of_a_track_s_envelopes_asks_first() {
    // Spec: automation R28. Clear All Envelopes and Clear Envelopes are
    // destructive, so an agent asks before it deletes a track's automation.
    let mut client = app();
    let midi = track(&mut client, "1 MIDI");
    call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "automation.set_breakpoint",
                               "args": { "track": midi,
                                         "target": { "type": "mixer", "param": "volume" },
                                         "at": 0, "value": 0.5 } }] }),
    );
    for (command, args) in [
        ("automation.clear_track", json!({ "track": midi })),
        ("automation.clear_device", json!({ "track": midi })),
    ] {
        let went = attempt(&mut client, command, &args);
        assert_eq!(
            went,
            Went {
                asked: true,
                permission: "destructive".to_owned()
            },
            "{command}"
        );
    }
    // Declined, the envelope is still there; allowed, the query tool's track
    // no longer lists it.
    let envelopes = |client: &mut Client<InProcess>| {
        call(
            client,
            "query",
            json!({ "path": "track \"1 MIDI\" / automation" }),
        )["automation"]["total"]
            .clone()
    };
    assert_eq!(envelopes(&mut client), 1);
    client.transport().answer = Box::new(|_| true);
    call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "automation.clear_track",
                               "args": { "track": midi } }] }),
    );
    assert_eq!(envelopes(&mut client), 0);
}

#[test]
fn an_agent_s_clip_move_over_another_clip_asks_and_one_into_empty_space_doesn_t() {
    // Spec: agent-permissions R1, R2.
    let (mut client, _, a, b) = two_clips(4);
    let over = json!({ "clips": [a], "to": "5.1.1" });
    let went = attempt(&mut client, "arrangement.move_clips", &over);
    assert_eq!(
        went,
        Went {
            asked: true,
            permission: "edit and destructive".to_owned()
        }
    );
    let asked = client.transport().asked.last().unwrap().clone();
    assert!(
        asked.contains("wants to run destructive command:"),
        "{asked}"
    );
    assert!(asked.contains("Move Clips (destructive): clips"), "{asked}");
    // The dry run names what it would cover.
    let dry = call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "arrangement.move_clips", "args": over }],
                "dry_run": true }),
    );
    assert_eq!(dry["results"][0]["replaces"], json!([b]));

    // Into empty space it's an edit: it runs without asking.
    let empty = json!({ "clips": [a], "to": "3.1.1" });
    let went = attempt(&mut client, "arrangement.move_clips", &empty);
    assert!(!went.asked);
    assert_eq!(went.permission, "edit");

    // Allowed, the move over B replaces it, as the person's own move does,
    // as the agent's undo step.
    client.transport().answer = Box::new(|_| true);
    let result = call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "arrangement.move_clips", "args": over }] }),
    );
    assert_eq!(result["asked_first"], true);
    assert_eq!(result["undo_step"], "editor: Move Clips");
    assert!(!exists(&mut client, b));
    assert!(exists(&mut client, a));
}

#[test]
fn every_edit_that_places_clips_over_others_is_destructive_decided_from_its_arguments() {
    // Spec: agent-permissions R1, R2. Each case: the edit over B, then the
    // same edit where nothing is in its way.
    type Case = (&'static str, u64, fn(u64, u64) -> Value, u64);
    let cases: [Case; 6] = [
        // Copying A onto B.
        (
            "arrangement.move_clips",
            4,
            |a, _| json!({ "clips": [a], "to": "5.1.1", "copy": true }),
            8,
        ),
        // Lengthening A over B.
        (
            "arrangement.resize_clips",
            2,
            |a, _| json!({ "clips": [a], "edge": "end", "by": 2 * BAR }),
            8,
        ),
        // Nudging A into B, right beside it.
        (
            "arrangement.nudge",
            1,
            |a, _| json!({ "clips": [a], "direction": "right" }),
            8,
        ),
        // Duplicating A right after itself, where B is.
        (
            "arrangement.duplicate",
            1,
            |a, _| json!({ "clips": [a] }),
            8,
        ),
        // Moving B back over A: A is replaced, and B, which it acts on,
        // isn't counted.
        (
            "arrangement.move_clips",
            4,
            |_, b| json!({ "clips": [b], "by": -((4 * BAR) as i64) }),
            8,
        ),
        // Resizing A's start later only shortens A itself.
        (
            "arrangement.resize_clips",
            8,
            |a, _| json!({ "clips": [a], "edge": "start", "by": BAR / 4 }),
            8,
        ),
    ];
    for (index, (command, b_bar, args, empty_bar)) in cases.into_iter().enumerate() {
        let (mut client, _, a, b) = two_clips(b_bar);
        let went = attempt(&mut client, command, &args(a, b));
        let shortens_itself = index == 5;
        assert_eq!(
            went.asked, !shortens_itself,
            "case {index} ({command}) over B: {went:?}"
        );
        if !shortens_itself {
            assert!(went.permission.contains("destructive"), "{went:?}");
        }
        // The same edit with B out of the way.
        let (mut client, _, a, b) = two_clips(empty_bar);
        let went = attempt(&mut client, command, &args(a, b));
        assert!(!went.asked, "case {index} ({command}) into empty space");
        assert_eq!(went.permission, "edit", "case {index}");
    }
}

#[test]
fn pasting_over_clips_asks_and_into_empty_space_doesn_t() {
    // Spec: agent-permissions R1, R2.
    let (mut client, midi, a, _) = two_clips(4);
    call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "arrangement.copy", "args": { "clips": [a] } }] }),
    );
    let over = json!({ "at": "5.1.1", "track": midi });
    assert!(attempt(&mut client, "arrangement.paste", &over).asked);
    let empty = json!({ "at": "9.1.1", "track": midi });
    assert!(!attempt(&mut client, "arrangement.paste", &empty).asked);
}

#[test]
fn copies_between_the_views_over_clips_ask_and_into_empty_space_do_not() {
    // Spec: second-window R23; agent-permissions R1, R2. A copy into the
    // Arrangement over another clip, or into a full slot, is destructive as
    // well as an edit, decided from where it lands; the same copy into empty
    // space isn't. Cut always asks.
    let (mut client, midi, _, b) = two_clips(4);
    let scene = |client: &mut Client<InProcess>, number: u64| {
        call(
            client,
            "query",
            json!({ "path": format!("scene {number}") }),
        )["id"]
            .as_u64()
            .unwrap()
    };
    let (first, third) = (scene(&mut client, 1), scene(&mut client, 3));
    let made = call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "clip.create_in_slot", "args": {
            "track": midi,
            "scene": first,
            "clip": { "id": { "$new": "S" }, "name": "S", "start": 0, "length": BAR,
                      "mute": false, "data": { "midi": { "notes": [] } } } } }] }),
    );
    let slot_clip = made["ids"]["S"].as_u64().unwrap();
    // Into the Arrangement: over B at bar 5 (from 1), and into empty space.
    let over = json!({ "clips": [slot_clip], "track": midi, "at": 4 * BAR });
    let went = attempt(&mut client, "session.copy_to_arrangement", &over);
    assert!(went.asked, "{went:?}");
    assert!(went.permission.contains("destructive"), "{went:?}");
    let empty = json!({ "clips": [slot_clip], "track": midi, "at": 9 * BAR });
    let went = attempt(&mut client, "session.copy_to_arrangement", &empty);
    assert!(!went.asked, "{went:?}");
    assert_eq!(went.permission, "edit");
    // Into the Session View: onto the full slot, and onto an empty one.
    let over = json!({ "clips": [b], "track": midi, "scene": first });
    let went = attempt(&mut client, "session.copy_to_session", &over);
    assert!(went.asked, "{went:?}");
    assert!(went.permission.contains("destructive"), "{went:?}");
    let empty = json!({ "clips": [b], "track": midi, "scene": third });
    let went = attempt(&mut client, "session.copy_to_session", &empty);
    assert!(!went.asked, "{went:?}");
    assert_eq!(went.permission, "edit");
    // Cut removes the person's work.
    call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "session.select",
                               "args": { "track": midi, "scene": first } }] }),
    );
    assert!(attempt(&mut client, "session.cut", &json!({})).asked);
}

#[test]
fn an_agent_s_delete_time_asks_first_and_copy_time_and_insert_silence_don_t() {
    // Spec: agent-permissions R1; time-editing R15, R17, R18. Removing time
    // from every track removes the person's work whatever it holds; copying
    // it changes nothing, and opening silence only moves.
    let (mut client, ..) = two_clips(4);
    let span = json!({ "start": "2.1.1", "end": "4.1.1" });
    let went = attempt(&mut client, "arrangement.delete_time", &span);
    assert_eq!(
        went,
        Went {
            asked: true,
            permission: "destructive".to_owned()
        }
    );
    let went = attempt(&mut client, "arrangement.cut_time", &span);
    assert!(went.asked, "{went:?}");
    let went = attempt(&mut client, "arrangement.copy_time", &span);
    assert_eq!(
        went,
        Went {
            asked: false,
            permission: "read".to_owned()
        }
    );
    let went = attempt(&mut client, "arrangement.insert_silence", &span);
    assert_eq!(
        went,
        Went {
            asked: false,
            permission: "edit".to_owned()
        }
    );
}

#[test]
fn a_drop_from_the_browser_over_clips_is_destructive_as_well_as_external() {
    // Spec: agent-permissions R3. Its files aren't read before the person
    // allows it, so a drop is destructive when a clip on its track ends
    // after where it lands.
    let mut client = app();
    let audio = track(&mut client, "3 Audio");
    let held = clip(&mut client, audio, "Held", 4, 2);
    let path = std::env::temp_dir().join("bl-agent-permissions-never-read.wav");
    let drop = |at: u64, track: Option<u64>| {
        let mut args = json!({ "paths": [path.to_string_lossy()], "at": at * BAR });
        if let Some(track) = track {
            args["track"] = json!(track);
        }
        args
    };
    let went = attempt(
        &mut client,
        "browser.add_to_arrangement",
        &drop(5, Some(audio)),
    );
    assert_eq!(
        went,
        Went {
            asked: true,
            permission: "destructive and external".to_owned()
        }
    );
    let asked = client.transport().asked.last().unwrap().clone();
    assert!(
        asked.contains("wants to run destructive and external command:"),
        "{asked}"
    );
    assert!(
        asked.contains("Add Files to Arrangement (destructive and external)"),
        "{asked}"
    );
    let dry = call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "browser.add_to_arrangement",
                               "args": drop(5, Some(audio)) }], "dry_run": true }),
    );
    assert_eq!(dry["results"][0]["replaces"], json!([held]));

    // After every clip on the track, or on a new track, it's external only.
    for args in [drop(6, Some(audio)), drop(0, None)] {
        let went = attempt(&mut client, "browser.add_to_arrangement", &args);
        assert!(went.asked, "external still asks");
        assert_eq!(went.permission, "external", "{args}");
    }
}

#[test]
fn a_midi_import_over_clips_is_destructive_as_well_as_external() {
    // Spec: agent-permissions R1, R3. The file isn't read before the person
    // allows it, so an import is destructive when a clip on its track ends
    // after where it lands.
    let mut client = app();
    let midi = track(&mut client, "1 MIDI");
    let held = clip(&mut client, midi, "Held", 4, 2);
    let path = std::env::temp_dir().join("bl-agent-permissions-never-read.mid");
    let import = |at: u64, track: Option<u64>| {
        let mut args = json!({ "path": path.to_string_lossy(), "at": at * BAR });
        if let Some(track) = track {
            args["track"] = json!(track);
        }
        args
    };
    let went = attempt(&mut client, "midi.import_file", &import(5, Some(midi)));
    assert_eq!(
        went,
        Went {
            asked: true,
            permission: "destructive and external".to_owned()
        }
    );
    let asked = client.transport().asked.last().unwrap().clone();
    assert!(
        asked.contains("Import MIDI File (destructive and external)"),
        "{asked}"
    );
    let dry = call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "midi.import_file",
                               "args": import(5, Some(midi)) }], "dry_run": true }),
    );
    assert_eq!(dry["results"][0]["replaces"], json!([held]));

    // After every clip on the track, or with no track (new tracks), it's
    // external only.
    for args in [import(7, Some(midi)), import(0, None)] {
        let went = attempt(&mut client, "midi.import_file", &args);
        assert!(went.asked, "external still asks");
        assert_eq!(went.permission, "external", "{args}");
    }
}

#[test]
fn an_autonomous_agent_s_move_over_clips_runs_without_asking() {
    // Spec: agent-permissions R2, R8.
    let (mut client, _, a, b) = two_clips(4);
    client
        .transport()
        .shell
        .run(
            "agent.set_autonomous",
            json!({ "client": "editor", "on": true }),
        )
        .unwrap();
    let asked = client.transport().asked.len();
    let result = call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "arrangement.move_clips",
                               "args": { "clips": [a], "to": "5.1.1" } }] }),
    );
    assert_eq!(client.transport().asked.len(), asked);
    assert_eq!(result["undo_step"], "editor: Move Clips");
    assert!(!exists(&mut client, b));
}

#[test]
fn headless_a_move_over_clips_is_refused_without_autonomous() {
    // Spec: agent-permissions R2. `bl-cli mcp` has no person to ask.
    for autonomous in [false, true] {
        let mut client = Client::new(InProcess::new(
            Agent::headless(autonomous),
            Shell::headless(),
        ));
        client.initialize("script").unwrap();
        let midi = track(&mut client, "1 MIDI");
        let a = clip(&mut client, midi, "A", 0, 1);
        let b = clip(&mut client, midi, "B", 4, 1);
        let result = client
            .call(
                "run",
                json!({ "commands": [{ "command": "arrangement.move_clips",
                                       "args": { "clips": [a], "to": "5.1.1" } }] }),
            )
            .unwrap();
        assert_eq!(result.is_error, !autonomous, "{}", result.text);
        if !autonomous {
            assert!(result.text.contains("--autonomous"), "{}", result.text);
        }
        assert_eq!(exists(&mut client, b), !autonomous);
    }
}

#[test]
fn a_scheduled_move_over_clips_asks_when_it_s_scheduled() {
    // Spec: agent-permissions R1.
    let (mut client, _, a, _) = two_clips(4);
    let result = client
        .call(
            "schedule",
            json!({ "action": "add", "at": "2.1.1", "command": "arrangement.move_clips",
                    "args": { "clips": [a], "to": "5.1.1" } }),
        )
        .unwrap();
    assert!(result.is_error && result.text.contains("declined"));
    let asked = client.transport().asked.last().unwrap().clone();
    assert!(
        asked.contains("At 2.1.1: Move Clips (destructive)"),
        "{asked}"
    );
}

#[test]
fn starting_a_recording_asks_first_and_stopping_never_does() {
    // Recording captures an input device, so it is `external`.
    let start = json!({ "commands": [{ "command": "transport.set_recording",
                                       "args": { "recording": true } }] });
    let stop = json!({ "commands": [{ "command": "transport.stop_recording" }] });

    // The person declines: nothing reaches the engine.
    let mut client = app();
    client.transport().agent.set_engine_attached(true);
    let connected = client.transport().asked.len();
    let result = client.call("run", start.clone()).unwrap();
    assert!(
        result.is_error && result.text.contains("declined"),
        "{result:?}"
    );
    assert_eq!(client.transport().asked.len(), connected + 1);
    assert!(client.transport().agent.take_engine_messages().is_empty());

    // Stopping runs without asking.
    call(&mut client, "run", stop.clone());
    assert_eq!(client.transport().asked.len(), connected + 1);
    assert_eq!(
        client.transport().agent.take_engine_messages(),
        [EngineMessage::Transport(TransportCommand::SetRecording(
            false
        ))]
    );

    // Autonomous, it records.
    client
        .transport()
        .shell
        .run(
            "agent.set_autonomous",
            json!({ "client": "editor", "on": true }),
        )
        .unwrap();
    call(&mut client, "run", start.clone());
    assert_eq!(client.transport().asked.len(), connected + 1);
    assert_eq!(
        client.transport().agent.take_engine_messages(),
        [EngineMessage::Transport(TransportCommand::SetRecording(
            true
        ))]
    );

    // Headless, it is refused unless the server was started autonomous.
    for autonomous in [false, true] {
        let mut client = Client::new(InProcess::new(
            Agent::headless(autonomous),
            Shell::headless(),
        ));
        client.initialize("script").unwrap();
        client.transport().agent.set_engine_attached(true);
        let result = client.call("run", start.clone()).unwrap();
        assert_eq!(result.is_error, !autonomous, "{}", result.text);
        assert_eq!(
            client.transport().agent.take_engine_messages().len(),
            usize::from(autonomous)
        );
        call(&mut client, "run", stop.clone());
    }
}

#[test]
fn describe_says_which_commands_rise_with_their_arguments() {
    let mut client = app();
    let move_clips = call(
        &mut client,
        "describe_command",
        json!({ "command": "arrangement.move_clips" }),
    );
    assert!(
        move_clips["raised_by_arguments"]
            .as_str()
            .unwrap()
            .contains("destructive"),
        "{move_clips}"
    );
    let split = call(
        &mut client,
        "describe_command",
        json!({ "command": "arrangement.split" }),
    );
    assert!(split.get("raised_by_arguments").is_none());
}

#[test]
fn a_confirmation_names_the_clips_a_call_would_replace_or_shorten() {
    // Spec: agent-permissions R4. A on bars 1 and 2; B on bar 5; C on
    // bars 6 and 7. A moved to bar 5 covers B and the start of C.
    let mut client = app();
    let midi = track(&mut client, "1 MIDI");
    let a = clip(&mut client, midi, "A", 0, 2);
    let b = clip(&mut client, midi, "B", 4, 1);
    let c = clip(&mut client, midi, "C", 5, 2);
    let over = json!({ "clips": [a], "to": "5.1.1" });
    assert!(attempt(&mut client, "arrangement.move_clips", &over).asked);
    let asked = client.transport().asked.last().unwrap().clone();
    assert!(
        asked.contains(
            "Move Clips (destructive): clips (…), to \u{201c}5.1.1\u{201d}. It replaces or \
             shortens clip \u{201c}B\u{201d} and clip \u{201c}C\u{201d}."
        ),
        "{asked}"
    );
    // The same clips the dry run lists.
    let dry = call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "arrangement.move_clips", "args": over }],
                "dry_run": true }),
    );
    assert_eq!(dry["results"][0]["replaces"], json!([b, c]));

    // Scheduled, it names them too.
    let result = client
        .call(
            "schedule",
            json!({ "action": "add", "at": "2.1.1", "command": "arrangement.move_clips",
                    "args": over }),
        )
        .unwrap();
    assert!(result.is_error && result.text.contains("declined"));
    let asked = client.transport().asked.last().unwrap().clone();
    assert!(
        asked.starts_with("\u{201c}editor\u{201d} wants to run destructive command:"),
        "{asked}"
    );
    assert!(
        asked.contains("At 2.1.1: Move Clips (destructive): ")
            && asked.contains(
                "It replaces or shortens clip \u{201c}B\u{201d} and clip \u{201c}C\u{201d}."
            ),
        "{asked}"
    );

    // A drop may replace them: its files' lengths aren't known yet.
    let audio = track(&mut client, "3 Audio");
    clip(&mut client, audio, "Held", 4, 2);
    let path = std::env::temp_dir().join("bl-agent-permissions-never-read.wav");
    let drop = json!({ "paths": [path.to_string_lossy()], "at": 5 * BAR, "track": audio });
    assert!(attempt(&mut client, "browser.add_to_arrangement", &drop).asked);
    let asked = client.transport().asked.last().unwrap().clone();
    assert!(
        asked.contains("It may replace or shorten clip \u{201c}Held\u{201d}."),
        "{asked}"
    );

    // A call that replaces nothing says nothing of the kind.
    let went = attempt(&mut client, "track.delete", &json!({ "track": midi }));
    assert!(went.asked);
    let asked = client.transport().asked.last().unwrap().clone();
    assert!(!asked.contains("replace"), "{asked}");
}

/// Notes for a MIDI clip: `count` of them, a beat apart.
fn notes(count: u64) -> Value {
    let beat = BAR / 4;
    let notes: Vec<Value> = (0..count)
        .map(|n| json!({ "key": 60 + n, "velocity": 100, "start": n * beat, "length": beat }))
        .collect();
    json!({ "midi": { "notes": notes } })
}

#[test]
fn an_agent_s_clip_set_data_that_drops_notes_asks_first_and_one_that_adds_notes_doesn_t() {
    // Spec: agent-permissions R11. Decided against the clip as it is.
    let mut client = app();
    let midi = track(&mut client, "1 MIDI");
    let bass = clip(&mut client, midi, "Bass", 0, 1);
    let set = |count: u64| json!({ "clip": bass, "data": notes(count) });

    // From none to three: it adds notes, so it's an edit.
    let went = attempt(&mut client, "clip.set_data", &set(3));
    assert_eq!(
        went,
        Went {
            asked: false,
            permission: "edit".to_owned()
        }
    );
    // Three to four adds; four different notes keep the count.
    assert!(!attempt(&mut client, "clip.set_data", &set(4)).asked);
    let moved: Vec<Value> = (0..4u64)
        .map(|n| json!({ "key": 48 + n, "velocity": 90, "start": n * BAR / 8, "length": BAR / 8 }))
        .collect();
    let same_count = json!({ "clip": bass, "data": { "midi": { "notes": moved } } });
    assert!(!attempt(&mut client, "clip.set_data", &same_count).asked);

    // Four to two drops notes: destructive, and the person is asked.
    let went = attempt(&mut client, "clip.set_data", &set(2));
    assert_eq!(
        went,
        Went {
            asked: true,
            permission: "edit and destructive".to_owned()
        }
    );
    let asked = client.transport().asked.last().unwrap().clone();
    assert!(
        asked.contains("wants to run destructive command:")
            && asked.contains("Replace Clip Contents (destructive): clip \u{201c}Bass\u{201d}"),
        "{asked}"
    );
    let dry = call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "clip.set_data", "args": set(2) }],
                "dry_run": true }),
    );
    assert_eq!(dry["results"][0]["permission"], "edit and destructive");
    assert_eq!(
        dry["results"][0]["fewer_notes"],
        json!({ "clip": bass, "has": 4, "leaves": 2 })
    );
    // In a batch, the batch asks.
    let batch = client
        .call(
            "run",
            json!({ "commands": [
                { "command": "clip.rename", "args": { "clip": bass, "name": "Bass 2" } },
                { "command": "clip.set_data", "args": set(0) }] }),
        )
        .unwrap();
    assert!(
        batch.is_error && batch.text.contains("declined"),
        "{}",
        batch.text
    );

    // Allowed, it replaces the notes as the agent's undo step.
    client.transport().answer = Box::new(|_| true);
    let result = call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "clip.set_data", "args": set(2) }] }),
    );
    assert_eq!(result["asked_first"], true);
    let doc = client.transport().shell.editor().document().clone();
    let Some(bl_model::ClipData::Midi(data)) = doc.clip(clip_id(bass)).map(|c| c.data.clone())
    else {
        panic!("Bass is a MIDI clip")
    };
    assert_eq!(data.notes.len(), 2);

    // Tempo and time signatures keep their class.
    for command in [
        "song.set_tempo",
        "song.set_time_signatures",
        "song.set_tempo_breakpoint",
        "song.move_tempo_breakpoint",
        "song.delete_tempo_breakpoint",
        "song.set_time_signature_change",
        "song.move_time_signature_change",
        "song.delete_time_signature_change",
    ] {
        let described = call(
            &mut client,
            "describe_command",
            json!({ "command": command }),
        );
        assert!(described.get("raised_by_arguments").is_none(), "{command}");
        assert_eq!(described["permission"], "edit", "{command}");
    }
    let described = call(
        &mut client,
        "describe_command",
        json!({ "command": "clip.set_data" }),
    );
    assert!(
        described["raised_by_arguments"]
            .as_str()
            .unwrap()
            .contains("fewer notes"),
        "{described}"
    );
}

/// The agent API driven as the app's host drives it, without a client:
/// the session's messages go to `Agent::handle`, and calls that wait are
/// resumed with `Agent::resume`.
struct Host {
    agent: Agent,
    shell: Shell,
    session: String,
    next: u64,
}

impl Host {
    fn new() -> Self {
        let mut agent = Agent::for_app();
        let session = agent.open_session_as(None, Credential::Server);
        let mut host = Self {
            agent,
            shell: Shell::headless(),
            session,
            next: 0,
        };
        let outcome = host.send(
            "initialize",
            json!({ "protocolVersion": "2025-11-25", "capabilities": {},
                    "clientInfo": { "name": "editor" } }),
        );
        let Outcome::Ask(question) = outcome else {
            panic!("{outcome:?}")
        };
        host.agent
            .resolve(&mut host.shell, question.id, true)
            .unwrap();
        host
    }

    fn send(&mut self, method: &str, params: Value) -> Outcome {
        self.next += 1;
        let message = json!({ "jsonrpc": "2.0", "id": self.next, "method": method,
                              "params": params });
        self.agent.handle(&mut self.shell, &self.session, message)
    }

    fn tool(&mut self, tool: &str, args: Value) -> Outcome {
        self.send("tools/call", json!({ "name": tool, "arguments": args }))
    }

    fn history(&self) -> Vec<String> {
        self.shell
            .editor()
            .undo_history()
            .map(str::to_owned)
            .collect()
    }
}

fn result(outcome: Outcome) -> Value {
    let Outcome::Reply(Some(reply)) = outcome else {
        panic!("{outcome:?}")
    };
    reply["result"]["structuredContent"].clone()
}

#[test]
fn agent_edits_wait_for_a_value_drag_and_land_after_it_in_order() {
    // Spec: agent-permissions R5, R6. A drag on a value (a volume fader)
    // holds a transaction open; agents' edits, and their reads, wait for it
    // and then run in the order they came, each its own undo step.
    let mut host = Host::new();
    let steps = host.history().len();
    host.shell.begin_gesture("Change volume");
    host.shell
        .run("mixer.set_volume", json!({ "track": 4, "volume": 0.5 }))
        .unwrap();
    let rename = |name: &str| {
        json!({ "commands": [{ "command": "track.rename", "args": { "track": 3, "name": name } }],
                "label": format!("Call it {name}") })
    };
    let first = host.tool("run", rename("Keys"));
    let second = host.tool("run", rename("Bass"));
    let read = host.tool("query", json!({ "id": 3 }));
    let (Outcome::Wait(first), Outcome::Wait(second), Outcome::Wait(read)) = (first, second, read)
    else {
        panic!("every call waits while the drag goes on")
    };
    assert!(host.agent.resume(&mut host.shell).is_none());
    host.shell
        .run("mixer.set_volume", json!({ "track": 4, "volume": 0.25 }))
        .unwrap();
    host.shell.end_gesture();

    let mut done = Vec::new();
    while let Some((ticket, outcome)) = host.agent.resume(&mut host.shell) {
        done.push((ticket, result(outcome)));
    }
    assert_eq!(
        done.iter().map(|(t, _)| *t).collect::<Vec<_>>(),
        [first, second, read]
    );
    assert_eq!(done[0].1["undo_step"], "editor: Call it Keys");
    assert_eq!(done[1].1["undo_step"], "editor: Call it Bass");
    // The read came after both edits, so it sees them.
    assert_eq!(done[2].1["name"], "Bass");
    let history = host.history();
    assert_eq!(history.len(), steps + 3);
    assert_eq!(
        history[..3],
        [
            "editor: Call it Bass",
            "editor: Call it Keys",
            "Change volume"
        ]
    );
    // Nothing waits now: the next call runs straight away.
    assert!(matches!(
        host.tool("query", json!({})),
        Outcome::Reply(Some(_))
    ));
}

#[test]
fn a_call_sent_mid_drag_that_asks_first_asks_after_the_drop() {
    // Spec: agent-permissions R1, R5. Its class is decided against the set
    // as the drop left it: the drag moved B into A's way.
    let mut host = Host::new();
    let midi: Vec<_> = host.shell.editor().document().top_level().collect();
    let clip = |host: &mut Host, name: &str, bar: u64| {
        let outcome = host.tool(
            "run",
            json!({ "commands": [{ "command": "clip.create", "args": {
                "track": midi[0].0.get(),
                "clip": { "id": { "$new": name }, "name": name, "start": bar * BAR,
                          "length": BAR, "mute": false, "data": { "midi": { "notes": [] } } } } }] }),
        );
        result(outcome)["ids"][name].as_u64().unwrap()
    };
    let a = clip(&mut host, "A", 0);
    let b = clip(&mut host, "B", 8);
    let drag = |by: u64| {
        Invocation::new(
            "arrangement.move_clips",
            json!({ "clips": [b], "by": -((by * BAR) as i64) }),
        )
    };
    host.shell
        .gesture(GesturePhase::Update, Some(&drag(2)))
        .unwrap();
    // Sent while B is out of A's way, at bar 7.
    let moved = host.tool(
        "run",
        json!({ "commands": [{ "command": "arrangement.move_clips",
                               "args": { "clips": [a], "to": "5.1.1" } }] }),
    );
    let Outcome::Wait(ticket) = moved else {
        panic!("{moved:?}")
    };
    // Dropped at bar 5, where A is going.
    host.shell
        .gesture(GesturePhase::Commit, Some(&drag(4)))
        .unwrap();
    let Some((resumed, Outcome::Ask(question))) = host.agent.resume(&mut host.shell) else {
        panic!("the move asks after the drop")
    };
    assert_eq!(resumed, ticket);
    let QuestionKind::Confirm { classes, .. } = &question.kind else {
        panic!("{question:?}")
    };
    assert_eq!(classes, &[Permission::Destructive]);
    // Nothing else runs until the person answers.
    assert!(host.agent.resume(&mut host.shell).is_none());
    host.agent.resolve(&mut host.shell, question.id, false);
    assert!(host.shell.editor().document().clip(clip_id(b)).is_some());
}

fn clip_id(id: u64) -> bl_model::ClipId {
    bl_model::ClipId(bl_core::id::Id::new(id).unwrap())
}

#[test]
fn agent_server_commands_stay_the_person_s_own() {
    // Spec: agent-permissions R9. Whatever token it carries, an agent can't
    // change its own rights or anyone's token.
    let mut client = Client::new(InProcess::with_credential(
        Agent::for_app(),
        Shell::headless(),
        Credential::Agent("editor".to_owned()),
    ));
    client
        .transport()
        .shell
        .run("agent.approve_client", json!({ "client": "editor" }))
        .unwrap();
    client.initialize("editor").unwrap();
    for (command, args) in [
        (
            "agent.set_autonomous",
            json!({ "client": "editor", "on": true }),
        ),
        ("agent.new_client_token", json!({ "client": "editor" })),
        ("agent.new_token", json!({})),
        ("agent.approve_client", json!({ "client": "other" })),
    ] {
        let result = client
            .call(
                "run",
                json!({ "commands": [{ "command": command, "args": args }] }),
            )
            .unwrap();
        assert!(result.is_error, "{command}");
        assert!(result.text.contains("only a person"), "{}", result.text);
    }
    assert!(
        !client
            .transport()
            .shell
            .agent_settings()
            .is_autonomous("editor")
    );
}

#[test]
fn a_scheduled_paste_is_judged_again_when_it_fires_and_not_run_if_it_would_replace_clips() {
    // Spec: agent-permissions R1. With the clipboard empty at 17.1.1 the
    // paste would fail, so scheduling it asks nothing; by the time it fires
    // the person has copied a clip and put the insert marker over another.
    let (mut client, midi, a, b) = two_clips(4);
    let asked = client.transport().asked.len();
    call(
        &mut client,
        "schedule",
        json!({ "action": "add", "at": "17.1.1", "command": "arrangement.paste", "args": {} }),
    );
    assert_eq!(client.transport().asked.len(), asked);
    let person = &mut client.transport().shell;
    person
        .invoke("arrangement.copy", json!({ "clips": [a] }))
        .unwrap();
    person
        .invoke(
            "arrangement.set_insert_marker",
            json!({ "at": 4 * BAR, "track": midi }),
        )
        .unwrap();
    let steps = person.editor().undo_history().len();

    let host = client.transport();
    host.agent.on_report(
        &mut host.shell,
        Report::Fired {
            id: 1,
            position: Frames(0),
            clock: 0,
        },
    );

    assert!(exists(&mut client, b), "the person's clip is still there");
    assert_eq!(
        client.transport().shell.editor().undo_history().len(),
        steps
    );
    let list = call(&mut client, "schedule", json!({ "action": "list" }));
    let entry = &list["scheduled"][0];
    assert_eq!(entry["state"], "failed", "{entry}");
    assert!(
        entry["detail"]["error"]
            .as_str()
            .is_some_and(|error| error.starts_with("not run")),
        "{entry}"
    );
}

#[test]
fn binding_keys_to_a_destructive_command_asks_first() {
    // Spec: agent-permissions R17.
    let mut c = app();
    let bind = json!({ "commands": [{ "command": "keymap.bind", "args": {
        "command": "set.discard_changes", "keys": ["Space"], "platform": "mac",
        "context": "global", "replace": true } }] });
    let mut dry = bind.clone();
    dry["dry_run"] = json!(true);
    let dry = call(&mut c, "run", dry);
    assert_eq!(dry["permission"], "destructive", "{dry}");
    assert_eq!(dry["would"], "ask the person first", "{dry}");
    // The person declines, so no key is taken.
    let result = c.call("run", bind).unwrap();
    assert!(
        result.is_error && result.text.contains("declined"),
        "{result:?}"
    );
    assert!(c.transport().shell.user_keymap().binds.is_empty());
}

#[test]
fn an_agent_cannot_bind_keys_to_the_agent_servers_commands() {
    // Spec: agent-permissions R17.
    let mut c = app();
    let result = c
        .call(
            "run",
            json!({ "commands": [{ "command": "keymap.bind", "args": {
                "command": "agent.set_autonomous", "args": { "client": "editor", "on": true },
                "keys": ["A"], "platform": "mac", "context": "settings" } }] }),
        )
        .unwrap();
    assert!(
        result.is_error
            && result.text.contains("agent.set_autonomous")
            && result.text.contains("Settings window"),
        "{result:?}"
    );
    assert!(c.transport().shell.user_keymap().binds.is_empty());
}

//! WP-3.11 through the agent API: the Session recording commands are
//! described, starting a recording asks the person first and ending one
//! never does, and a recording an agent starts shows in `transport.state`
//! and makes its clip in the slot.

use std::time::{Duration, Instant};

use bl_agent::client::{Client, InProcess};
use bl_agent::{Agent, QuestionKind};
use bl_audio_io::{BackendKind, NullBackend, NullOptions};
use bl_shell::Shell;
use bl_shell::audio::{AudioHost, AudioSettings};
use bl_shell::files::ProjectFiles;
use bl_shell::settings::Preferences;
use serde_json::{Value, json};

fn client() -> Client<InProcess> {
    let (backend, _control) = NullBackend::with_devices(
        NullOptions {
            dropout_after: None,
            channels: 4,
            ..NullOptions::default()
        },
        &[("null", "Null")],
    );
    let audio = AudioHost::with_backends(
        vec![BackendKind::Null],
        Box::new(move |_| Ok(Box::new(backend.clone()))),
        AudioSettings::default(),
    );
    let mut shell = Shell::new(
        Preferences::default(),
        None,
        audio,
        Box::new(ProjectFiles::new()),
    );
    shell
        .run("audio.configure", json!({ "input_device": "null:null" }))
        .unwrap();
    shell.start_audio();
    let mut client = Client::new(InProcess::new(Agent::for_app(), shell));
    // The person lets the agent connect, and nothing more.
    client.transport().answer = Box::new(|q| matches!(q.kind, QuestionKind::Connect { .. }));
    client.initialize("session-recording").unwrap();
    client
}

fn call(client: &mut Client<InProcess>, tool: &str, args: Value) -> Value {
    client
        .call(tool, args)
        .unwrap()
        .ok()
        .unwrap_or_else(|error| panic!("{tool} failed: {error}"))
}

fn wait(client: &mut Client<InProcess>, what: &str, done: impl Fn(&mut Client<InProcess>) -> bool) {
    let until = Instant::now() + Duration::from_secs(10);
    while Instant::now() < until {
        client.transport().shell.poll_engine();
        if done(client) {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("timed out waiting for {what}");
}

// Spec: session-recording R9, R12. The commands are described with their
// permission classes: three that start a recording are external, the one
// that ends it is read.
#[test]
fn the_session_recording_commands_are_described() {
    let mut client = client();
    for (command, permission) in [
        ("session.record", "external"),
        ("session.record_slot", "external"),
        ("session.overdub", "external"),
        ("session.stop_recording", "read"),
    ] {
        let described = call(
            &mut client,
            "describe_command",
            json!({ "command": command }),
        );
        assert_eq!(
            described["permission"], permission,
            "{command}: {described}"
        );
        assert!(
            !described["description"].as_str().unwrap().is_empty(),
            "{command}"
        );
        assert!(described["args"].is_object(), "{command}: {described}");
    }
}

// Spec: session-recording R9, R12. An agent that the person has not allowed
// to record is refused; once allowed it records into the slot, reads the
// recording back, and ends it without being asked.
#[test]
fn an_agent_records_into_a_slot_and_ends_it_without_asking() {
    let mut client = client();
    call(
        &mut client,
        "run",
        json!({ "commands": [
            { "command": "session.set_quantization", "args": { "quantization": "none" } },
        ] }),
    );
    call(
        &mut client,
        "run",
        json!({ "commands": [
            { "command": "track.create",
              "args": { "id": {"$new": "keys"}, "kind": "midi", "name": "Keys", "index": 0 } },
        ] }),
    );
    let keys = client
        .transport()
        .shell
        .editor()
        .document()
        .tracks()
        .find(|track| track.name() == "Keys")
        .unwrap()
        .id();
    call(
        &mut client,
        "run",
        json!({ "commands": [
            { "command": "mixer.set_arm", "args": { "track": keys, "arm": true } },
        ] }),
    );

    // The person has not allowed it: no recording starts.
    let refused = client
        .call(
            "run",
            json!({ "commands": [{ "command": "session.record", "args": {} }] }),
        )
        .unwrap();
    assert!(refused.is_error, "{refused:?}");
    assert!(client.transport().shell.running_recording().is_none());

    client.transport().answer = Box::new(|_| true);
    let started = call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "session.record", "args": {} }] }),
    );
    assert_eq!(
        started["results"][0]["result"]["slots"][0]["track"],
        json!(keys)
    );
    let state = call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "transport.state", "args": {} }] }),
    );
    assert_eq!(
        state["results"][0]["result"]["recording"]["session_slots"][0]["overdub"],
        json!(false)
    );
    std::thread::sleep(Duration::from_millis(150));
    client.transport().shell.poll_engine();

    // Ending a recording is read class: it runs with every question denied.
    client.transport().answer = Box::new(|_| false);
    call(
        &mut client,
        "run",
        json!({ "commands": [{ "command": "session.stop_recording", "args": {} }] }),
    );
    wait(&mut client, "the recording to end", |client| {
        let shell = &mut client.transport().shell;
        shell.running_recording().is_none() && !shell.editor().in_transaction()
    });
    let scene = client
        .transport()
        .shell
        .editor()
        .document()
        .scenes()
        .next()
        .unwrap()
        .id;
    let number = serde_json::to_value(keys).unwrap();
    let slot = call(
        &mut client,
        "query",
        json!({ "path": format!("track {number} / slot 1") }),
    );
    assert_eq!(slot["clip"]["kind"], "midi", "{slot}");
    assert!(slot["clip"]["length"]["ticks"].as_i64().unwrap() > 0);
    assert!(
        client
            .transport()
            .shell
            .editor()
            .document()
            .slot(keys, scene)
            .is_some()
    );
}

//! The app's host wiring the agent API to the engine it plays the set
//! through (WP-1.15b), headless: the shell and its engine on the null
//! device, paced like a sound card, with no window. After every tool call,
//! and between them, the test does what the app's event loop does
//! (`Agent::sync_engine`): agents' transport and scheduled commands go on
//! the queue the person's transport commands take, the engine's reports
//! come back, and fired scheduled commands run as the agent's undo steps,
//! on the frames their bars and beats fall on, as `bl-cli`'s engine
//! harness plays them offline.

use std::time::{Duration, Instant};

use bl_agent::client::{Client, InProcess};
use bl_agent::{Agent, QuestionKind};
use bl_core::time::{SampleRate, Ticks};
use bl_shell::Shell;
use serde_json::{Value, json};

const BAR: i64 = Ticks::WHOLE.get();

/// The app's agent API on a shell playing through the null device, the
/// agent `conductor` allowed.
fn host() -> Client<InProcess> {
    let mut shell = Shell::headless();
    shell.start_audio();
    assert!(shell.audio().engine_running(), "{:?}", shell.audio());
    let mut client = Client::new(InProcess::new(Agent::for_app(), shell));
    client.transport().answer = Box::new(|q| matches!(q.kind, QuestionKind::Connect { .. }));
    client.initialize("conductor").unwrap();
    pump(&mut client);
    client
}

/// What the app's event loop does after every event.
fn pump(client: &mut Client<InProcess>) {
    let host = client.transport();
    host.agent.sync_engine(&mut host.shell);
    while host.agent.resume(&mut host.shell).is_some() {}
}

/// A tool call, as the app serves it: the engine hears what it asked at
/// once.
fn call(client: &mut Client<InProcess>, tool: &str, args: Value) -> Value {
    let result = client
        .call(tool, args)
        .unwrap()
        .ok()
        .unwrap_or_else(|e| panic!("{tool}: {e}"));
    pump(client);
    result
}

/// Pumps until `done` holds, for up to ten seconds.
fn wait(client: &mut Client<InProcess>, what: &str, done: impl Fn(&mut Client<InProcess>) -> bool) {
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        pump(client);
        if done(client) {
            return;
        }
        assert!(Instant::now() < until, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn status(client: &mut Client<InProcess>) -> Value {
    call(client, "transport", json!({ "action": "status" }))
}

fn scheduled(client: &mut Client<InProcess>) -> Vec<Value> {
    call(client, "schedule", json!({ "action": "list" }))["scheduled"]
        .as_array()
        .unwrap()
        .clone()
}

fn all_fired(client: &mut Client<InProcess>) -> bool {
    scheduled(client)
        .iter()
        .all(|entry| entry["state"] == "fired")
}

/// The song frame a bar.beat.sixteenth position falls on in this set, at
/// the device's rate.
fn frame_of(client: &mut Client<InProcess>, at: &str) -> i64 {
    let shell = &client.transport().shell;
    let rate = SampleRate::new(shell.audio().sample_rate().unwrap()).unwrap();
    let settings = shell.editor().document().settings();
    let ticks = settings.time_signatures.parse_position(at).unwrap();
    settings.tempo.ticks_to_frames(ticks, rate).get()
}

fn audio_track(client: &mut Client<InProcess>) -> u64 {
    call(client, "query", json!({ "path": "track \"3 Audio\"" }))["id"]
        .as_u64()
        .unwrap()
}

#[test]
fn scheduled_commands_fire_in_the_app_s_engine_on_their_frames_as_the_agent_s_undo_steps() {
    // Spec: agent-permissions R10.
    let mut client = host();
    let before = status(&mut client);
    assert_eq!(before["engine_plays_here"], true, "{before}");
    assert_eq!(before["playing"], false);
    let track = audio_track(&mut client);
    for (at, command, args) in [
        (
            "1.2.1",
            "mixer.set_mute",
            json!({ "track": track, "mute": true }),
        ),
        (
            "1.3.1",
            "mixer.set_volume",
            json!({ "track": track, "volume": 0.5 }),
        ),
    ] {
        let added = call(
            &mut client,
            "schedule",
            json!({ "action": "add", "at": at, "command": command, "args": args }),
        );
        assert_eq!(added["sample_accurate"], true);
        assert!(
            added["note"]
                .as_str()
                .unwrap()
                .contains("fires when the playing song"),
            "{added}"
        );
    }
    call(&mut client, "transport", json!({ "action": "play" }));
    wait(&mut client, "both to fire", all_fired);

    // Each fired on the frame its bar and beat falls on.
    let entries = scheduled(&mut client);
    for entry in &entries {
        let at = entry["at"].as_str().unwrap();
        assert_eq!(
            entry["detail"]["song_frame"].as_i64().unwrap(),
            frame_of(&mut client, at),
            "{entry}"
        );
    }
    // And the set followed, each change the agent's own undo step.
    let shell = &client.transport().shell;
    let history: Vec<&str> = shell.editor().undo_history().take(2).collect();
    assert!(
        history[0].starts_with("conductor: ") && history[0].ends_with(" at 1.3.1"),
        "{history:?}"
    );
    assert_eq!(history[1], "conductor: Activate/Deactivate Track at 1.2.1");
    let document = shell.editor().document();
    let mixer = document
        .track(bl_model::TrackId(bl_core::id::Id::new(track).unwrap()))
        .unwrap()
        .mixer();
    assert!(mixer.mute);
    assert_eq!(mixer.volume, 0.5);
    // The agent sees the engine's transport.
    let now = status(&mut client);
    assert_eq!(now["playing"], true, "{now}");
    assert!(
        now["position"]["ticks"].as_i64().unwrap() >= BAR / 2,
        "{now}"
    );
    call(&mut client, "transport", json!({ "action": "stop" }));
    wait(&mut client, "the stop", |c| status(c)["playing"] == false);
}

#[test]
fn the_agent_s_and_the_person_s_transport_commands_share_one_queue_in_order() {
    // Spec: agent-permissions R10; control-bar-transport R12.
    let mut client = host();
    // The person plays, from the Control Bar.
    client
        .transport()
        .shell
        .invoke("transport.play", json!({}))
        .unwrap();
    wait(&mut client, "the person's play", |c| {
        status(c)["playing"] == true
    });
    // The agent jumps to bar 9, and straight after the person stops: the
    // engine takes them in that order, so the song stops at bar 9, not
    // where it was.
    call(
        &mut client,
        "transport",
        json!({ "action": "locate", "at": "9.1.1" }),
    );
    client
        .transport()
        .shell
        .invoke("transport.stop", json!({}))
        .unwrap();
    wait(&mut client, "the stop", |c| status(c)["playing"] == false);
    let stopped = status(&mut client);
    let position = stopped["position"]["ticks"].as_i64().unwrap();
    assert!(
        (8 * BAR..9 * BAR).contains(&position),
        "stopped at {stopped}"
    );
    assert!(
        stopped["position"]["bbs"]
            .as_str()
            .unwrap()
            .starts_with("9.1."),
        "{stopped}"
    );
}

#[test]
fn a_new_engine_gets_the_commands_still_scheduled_and_none_plays_without_one() {
    // Spec: agent-permissions R10.
    let mut client = host();
    let track = audio_track(&mut client);
    call(
        &mut client,
        "schedule",
        json!({ "action": "add", "at": "1.2.1", "command": "mixer.set_mute",
                "args": { "track": track, "mute": true } }),
    );
    // The person switches the audio engine off: nothing plays the song.
    client
        .transport()
        .shell
        .invoke("audio.toggle_engine", json!({}))
        .unwrap();
    pump(&mut client);
    assert_eq!(status(&mut client)["engine_plays_here"], false);
    let refused = client
        .call("transport", json!({ "action": "play" }))
        .unwrap();
    assert!(refused.is_error, "{}", refused.text);
    // On again, a new engine: the command still waits, on it.
    client
        .transport()
        .shell
        .invoke("audio.toggle_engine", json!({}))
        .unwrap();
    pump(&mut client);
    assert_eq!(status(&mut client)["engine_plays_here"], true);
    call(&mut client, "transport", json!({ "action": "play" }));
    wait(&mut client, "the mute", all_fired);
    let entries = scheduled(&mut client);
    assert_eq!(entries.len(), 1);
    assert_eq!(
        entries[0]["detail"]["song_frame"].as_i64().unwrap(),
        frame_of(&mut client, "1.2.1")
    );
    let shell = &client.transport().shell;
    let fired = shell
        .editor()
        .undo_history()
        .filter(|label| label.ends_with(" at 1.2.1"))
        .count();
    assert_eq!(fired, 1, "it fired once");
}

#[test]
fn a_scheduled_command_that_fires_mid_drag_lands_after_the_drop() {
    // Spec: agent-permissions R5, R10. The engine changes the sound on its
    // frame; the set follows once the person lets go.
    let mut client = host();
    let track = audio_track(&mut client);
    call(
        &mut client,
        "schedule",
        json!({ "action": "add", "at": "1.2.1", "command": "mixer.set_mute",
                "args": { "track": track, "mute": true } }),
    );
    call(&mut client, "transport", json!({ "action": "play" }));
    // The person drags a fader on another track meanwhile.
    let shell = &mut client.transport().shell;
    shell.begin_gesture("Change volume");
    shell
        .run("mixer.set_volume", json!({ "track": 4, "volume": 0.25 }))
        .unwrap();
    wait(&mut client, "the engine to fire", |c| {
        c.transport().agent.holds_calls()
    });
    let muted = |c: &mut Client<InProcess>| {
        c.transport()
            .shell
            .editor()
            .document()
            .track(bl_model::TrackId(bl_core::id::Id::new(track).unwrap()))
            .unwrap()
            .mixer()
            .mute
    };
    assert!(!muted(&mut client), "the set waits for the drop");
    client.transport().shell.end_gesture();
    pump(&mut client);
    assert!(muted(&mut client));
    let history: Vec<String> = client
        .transport()
        .shell
        .editor()
        .undo_history()
        .take(2)
        .map(str::to_owned)
        .collect();
    assert_eq!(
        history,
        [
            "conductor: Activate/Deactivate Track at 1.2.1",
            "Change volume"
        ]
    );
    call(&mut client, "transport", json!({ "action": "stop" }));
}

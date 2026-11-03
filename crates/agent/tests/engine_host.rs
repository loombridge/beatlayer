//! The app's host wiring the agent API to the engine it plays the set
//! through (WP-1.15b), headless: the shell and its engine on the null
//! device, paced like a sound card, with no window. After every tool call,
//! and between them, the test does what the app's event loop does
//! (`Agent::sync_engine`): agents' transport and scheduled commands go on
//! the queue the person's transport commands take, the engine's reports
//! come back, and fired scheduled commands run as the agent's undo steps,
//! on the frames their bars and beats fall on, as `bl-cli`'s engine
//! harness plays them offline.

use bl_agent::client::{Client, InProcess};
use bl_agent::{Agent, QuestionKind};
use bl_audio_io::NullControl;
use bl_core::time::{SampleRate, Ticks};
use bl_shell::testing;
use serde_json::{Value, json};

const BAR: i64 = Ticks::WHOLE.get();

/// The app's agent API on a shell playing through the null device paced by
/// hand (T199), the agent `conductor` allowed, and the control that runs its
/// blocks.
fn host() -> (Client<InProcess>, NullControl) {
    let (mut shell, control) = testing::rig();
    shell.start_audio();
    assert!(shell.audio().engine_running(), "{:?}", shell.audio());
    let mut client = Client::new(InProcess::new(Agent::for_app(), shell));
    client.transport().answer = Box::new(|q| matches!(q.kind, QuestionKind::Connect { .. }));
    client.initialize("conductor").unwrap();
    pump(&control, &mut client);
    (client, control)
}

/// What the app's event loop does after every event, and the block the
/// hand-paced null device runs for it.
fn pump(control: &NullControl, client: &mut Client<InProcess>) {
    let host = client.transport();
    host.agent.sync_engine(&mut host.shell);
    while host.agent.resume(&mut host.shell).is_some() {}
    // With the audio engine switched off there is no stream to run.
    if host.shell.audio().is_running() {
        testing::advance(control, &mut host.shell, testing::STEP_MS);
    }
}

/// A tool call, as the app serves it: the engine hears what it asked at
/// once.
fn call(control: &NullControl, client: &mut Client<InProcess>, tool: &str, args: Value) -> Value {
    let result = client
        .call(tool, args)
        .unwrap()
        .ok()
        .unwrap_or_else(|e| panic!("{tool}: {e}"));
    pump(control, client);
    result
}

/// Pumps until `done` holds, for up to about ten seconds of test time.
fn wait(
    control: &NullControl,
    client: &mut Client<InProcess>,
    what: &str,
    done: impl Fn(&mut Client<InProcess>) -> bool,
) {
    for _ in 0..2_000 {
        pump(control, client);
        if done(client) {
            return;
        }
    }
    panic!("timed out waiting for {what}");
}

fn status(control: &NullControl, client: &mut Client<InProcess>) -> Value {
    call(control, client, "transport", json!({ "action": "status" }))
}

fn scheduled(control: &NullControl, client: &mut Client<InProcess>) -> Vec<Value> {
    call(control, client, "schedule", json!({ "action": "list" }))["scheduled"]
        .as_array()
        .unwrap()
        .clone()
}

fn all_fired(control: &NullControl, client: &mut Client<InProcess>) -> bool {
    scheduled(control, client)
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

fn audio_track(control: &NullControl, client: &mut Client<InProcess>) -> u64 {
    call(
        control,
        client,
        "query",
        json!({ "path": "track \"3 Audio\"" }),
    )["id"]
        .as_u64()
        .unwrap()
}

#[test]
fn scheduled_commands_fire_in_the_app_s_engine_on_their_frames_as_the_agent_s_undo_steps() {
    // Spec: agent-permissions R10.
    let (mut client, control) = host();
    let before = status(&control, &mut client);
    assert_eq!(before["engine_plays_here"], true, "{before}");
    assert_eq!(before["playing"], false);
    let track = audio_track(&control, &mut client);
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
            &control,
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
    call(
        &control,
        &mut client,
        "transport",
        json!({ "action": "play" }),
    );
    wait(&control, &mut client, "both to fire", |c| {
        all_fired(&control, c)
    });

    // Each fired on the frame its bar and beat falls on.
    let entries = scheduled(&control, &mut client);
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
    let now = status(&control, &mut client);
    assert_eq!(now["playing"], true, "{now}");
    assert!(
        now["position"]["ticks"].as_i64().unwrap() >= BAR / 2,
        "{now}"
    );
    call(
        &control,
        &mut client,
        "transport",
        json!({ "action": "stop" }),
    );
    wait(&control, &mut client, "the stop", |c| {
        status(&control, c)["playing"] == false
    });
}

#[test]
fn the_agent_s_and_the_person_s_transport_commands_share_one_queue_in_order() {
    // Spec: agent-permissions R10; control-bar-transport R12.
    let (mut client, control) = host();
    // The person plays, from the Control Bar.
    client
        .transport()
        .shell
        .invoke("transport.play", json!({}))
        .unwrap();
    wait(&control, &mut client, "the person's play", |c| {
        status(&control, c)["playing"] == true
    });
    // The agent jumps to bar 9, and straight after the person stops: the
    // engine takes them in that order, so the song stops at bar 9, not
    // where it was.
    call(
        &control,
        &mut client,
        "transport",
        json!({ "action": "locate", "at": "9.1.1" }),
    );
    client
        .transport()
        .shell
        .invoke("transport.stop", json!({}))
        .unwrap();
    wait(&control, &mut client, "the stop", |c| {
        status(&control, c)["playing"] == false
    });
    let stopped = status(&control, &mut client);
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
    let (mut client, control) = host();
    let track = audio_track(&control, &mut client);
    call(
        &control,
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
    pump(&control, &mut client);
    assert_eq!(status(&control, &mut client)["engine_plays_here"], false);
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
    pump(&control, &mut client);
    assert_eq!(status(&control, &mut client)["engine_plays_here"], true);
    call(
        &control,
        &mut client,
        "transport",
        json!({ "action": "play" }),
    );
    wait(&control, &mut client, "the mute", |c| {
        all_fired(&control, c)
    });
    let entries = scheduled(&control, &mut client);
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
    let (mut client, control) = host();
    let track = audio_track(&control, &mut client);
    call(
        &control,
        &mut client,
        "schedule",
        json!({ "action": "add", "at": "1.2.1", "command": "mixer.set_mute",
                "args": { "track": track, "mute": true } }),
    );
    call(
        &control,
        &mut client,
        "transport",
        json!({ "action": "play" }),
    );
    // The person drags a fader on another track meanwhile.
    let shell = &mut client.transport().shell;
    shell.begin_gesture("Change volume");
    shell
        .run("mixer.set_volume", json!({ "track": 4, "volume": 0.25 }))
        .unwrap();
    wait(&control, &mut client, "the engine to fire", |c| {
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
    pump(&control, &mut client);
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
    call(
        &control,
        &mut client,
        "transport",
        json!({ "action": "stop" }),
    );
}

#[test]
fn an_agent_s_arrangement_record_sets_the_recording_up_as_the_ui_s_does() {
    // Spec: recording R1, R2. `transport.set_recording` through the agent API
    // gives the engine the take's rings first, as the Control Bar does: a set
    // that has never been saved can't hold an armed audio track's file, and
    // the shell says so. Starting a recording asks first; the person allows
    // it.
    let (mut client, control) = host();
    client.transport().answer = Box::new(|_| true);
    let track = audio_track(&control, &mut client);
    for (command, args) in [
        (
            "routing.set_input",
            json!({ "track": track,
                    "input": { "type": "external", "channel": { "type": "mono", "channel": 1 } } }),
        ),
        ("mixer.set_arm", json!({ "track": track, "arm": true })),
        ("transport.set_recording", json!({ "recording": true })),
    ] {
        call(
            &control,
            &mut client,
            "run",
            json!({ "commands": [{ "command": command, "args": args }] }),
        );
    }
    wait(&control, &mut client, "the engine to record", |c| {
        c.transport().shell.poll_engine().recording
    });
    let message = client
        .transport()
        .shell
        .message()
        .map(|message| message.text.clone());
    assert!(
        message
            .as_deref()
            .is_some_and(|text| text.starts_with("the set has never been saved")),
        "{message:?}"
    );
    call(
        &control,
        &mut client,
        "transport",
        json!({ "action": "stop" }),
    );
}

/// What `session.state` lists for the slot at `scene`: its `launched` and
/// `clip_time`, in ticks, if the engine publishes it.
fn slot_state(
    control: &NullControl,
    client: &mut Client<InProcess>,
    scene: &Value,
) -> Option<(i64, i64)> {
    let state = call(
        control,
        client,
        "run",
        json!({ "commands": [{ "command": "session.state", "args": {} }] }),
    );
    let launches = state["results"][0]["result"]["launches"]
        .as_array()?
        .clone();
    let slot = launches.iter().find(|slot| slot["scene"] == *scene)?;
    Some((slot["launched"].as_i64()?, slot["clip_time"].as_i64()?))
}

// Spec: launch-quantisation R27, R30, R34. An agent launches, swaps by Legato
// and nudges by command ID, and reads each back from `session.state`'s
// `clip_time`; a nudge of 0 or 65 steps is refused.
#[test]
fn an_agent_launches_legato_swaps_and_nudges_by_command_id_and_reads_each_back() {
    let (mut client, control) = host();
    let (track, scenes) = {
        let document = client.transport().shell.editor().document();
        let track = document.top_level().next().unwrap();
        let scenes: Vec<Value> = document
            .scenes()
            .take(2)
            .map(|scene| json!(scene.id))
            .collect();
        (track, scenes)
    };
    // A batch holds only document edits, so each Session command runs in a
    // call of its own.
    let run = |client: &mut Client<InProcess>, command: &str, args: Value| {
        call(
            &control,
            client,
            "run",
            json!({ "commands": [{ "command": command, "args": args }] }),
        )
    };
    for scene in &scenes {
        run(
            &mut client,
            "session.insert_empty_midi_clip",
            json!({ "track": track, "scene": scene }),
        );
    }
    run(
        &mut client,
        "session.set_quantization",
        json!({ "quantization": "none" }),
    );
    let clips: Vec<Value> = {
        let document = client.transport().shell.editor().document();
        let track = document.track(track).unwrap();
        track.slots().map(|(_, clip)| json!(clip.id)).collect()
    };
    for clip in &clips {
        run(
            &mut client,
            "clip.set_legato",
            json!({ "clip": clip, "legato": true }),
        );
        run(
            &mut client,
            "clip.set_launch_quantization",
            json!({ "clip": clip, "quantization": "none" }),
        );
    }

    // Launch the first clip, let it play, then launch the second: it takes
    // over the first's clip time instead of starting from zero.
    run(
        &mut client,
        "session.launch",
        json!({ "track": track, "scene": scenes[0] }),
    );
    // Three beats in, so a clip that started from zero a moment ago would
    // read far less.
    wait(&control, &mut client, "the first clip", |c| {
        slot_state(&control, c, &scenes[0])
            .is_some_and(|(_, time)| time >= 3 * Ticks::QUARTER.get())
    });
    let (_, first) = slot_state(&control, &mut client, &scenes[0]).unwrap();
    run(
        &mut client,
        "session.launch",
        json!({ "track": track, "scene": scenes[1] }),
    );
    wait(&control, &mut client, "the swap", |c| {
        slot_state(&control, c, &scenes[1]).is_some()
    });
    // The transport stands still from here, so clip times stand still.
    call(
        &control,
        &mut client,
        "transport",
        json!({ "action": "stop" }),
    );
    wait(&control, &mut client, "the stop", |c| {
        !c.transport().shell.is_playing()
    });
    let (_, second) = slot_state(&control, &mut client, &scenes[1]).unwrap();
    assert!(
        second >= first,
        "Legato took over {first}, the clip reads {second}"
    );

    // With the global value None, a nudge of two steps is two thirty-seconds.
    run(
        &mut client,
        "session.nudge",
        json!({ "track": track, "scene": scenes[1], "steps": 2 }),
    );
    let thirty_second = Ticks::SIXTEENTH.get() / 2;
    wait(&control, &mut client, "the nudge", |c| {
        slot_state(&control, c, &scenes[1])
            .is_some_and(|(_, time)| time == second + 2 * thirty_second)
    });

    // 0 and 65 steps are refused.
    for steps in [0, 65] {
        let refused = client
            .call(
                "run",
                json!({ "commands": [{ "command": "session.nudge",
                    "args": { "track": track, "scene": scenes[1], "steps": steps } }] }),
            )
            .unwrap();
        assert!(refused.ok().is_err(), "{steps} steps is refused");
    }
}

/// What `transport.state` says of the Phase Nudge buttons, through the tool
/// an agent reads it with.
fn phase_nudge(control: &NullControl, client: &mut Client<InProcess>) -> Value {
    call(
        control,
        client,
        "run",
        json!({ "commands": [{ "command": "transport.state", "args": {} }] }),
    )["results"][0]["result"]["phase_nudge"]
        .clone()
}

fn nudge_args(direction: &str, held: bool) -> Value {
    json!({ "commands": [{
        "command": "transport.phase_nudge",
        "args": { "direction": direction, "held": held },
    }] })
}

#[test]
fn an_agent_nudges_by_id_reads_it_back_and_its_nudge_ends_when_it_disconnects() {
    // Spec: transport R27, R28. Only the nudges the agent started end with it.
    let (mut client, control) = host();
    call(
        &control,
        &mut client,
        "transport",
        json!({ "action": "play" }),
    );
    wait(&control, &mut client, "playback", |c| {
        status(&control, c)["playing"] == true
    });
    assert_eq!(phase_nudge(&control, &mut client), "none");
    let sent = call(&control, &mut client, "run", nudge_args("up", true));
    assert_eq!(sent["ran"], 1, "{sent}");
    wait(&control, &mut client, "the nudge", |c| {
        phase_nudge(&control, c) == "up"
    });
    call(&control, &mut client, "run", nudge_args("up", false));
    wait(&control, &mut client, "the release", |c| {
        phase_nudge(&control, c) == "none"
    });
    // The person holds Down; the agent holds Up and disconnects.
    client
        .transport()
        .shell
        .invoke(
            "transport.phase_nudge",
            json!({ "direction": "down", "held": true }),
        )
        .unwrap();
    wait(&control, &mut client, "the person's nudge", |c| {
        phase_nudge(&control, c) == "down"
    });
    call(&control, &mut client, "run", nudge_args("up", true));
    wait(&control, &mut client, "the agent's nudge", |c| {
        phase_nudge(&control, c) == "up"
    });
    let session = client
        .transport()
        .agent
        .sessions()
        .next()
        .map(|(id, _)| id.to_owned())
        .unwrap();
    client.transport().agent.close_session(&session);
    wait(&control, &mut client, "the agent's nudge to end", |c| {
        let host = c.transport();
        host.shell.poll_engine().phase_nudge == Some(bl_engine::NudgeDirection::Down)
    });
    // The person's nudge goes on.
    let state = client
        .transport()
        .shell
        .run("transport.state", json!({}))
        .unwrap();
    assert_eq!(state["phase_nudge"], "down");
    client
        .transport()
        .shell
        .invoke(
            "transport.phase_nudge",
            json!({ "direction": "down", "held": false }),
        )
        .unwrap();
    client.transport().shell.audio_mut().stop();
}

#[test]
fn an_agent_s_phase_nudge_is_refused_while_an_outside_tempo_leads_but_its_release_is_taken() {
    // Spec: transport R29.
    let (mut client, control) = host();
    call(
        &control,
        &mut client,
        "transport",
        json!({ "action": "play" }),
    );
    wait(&control, &mut client, "playback", |c| {
        status(&control, c)["playing"] == true
    });
    call(&control, &mut client, "run", nudge_args("down", true));
    wait(&control, &mut client, "the nudge", |c| {
        phase_nudge(&control, c) == "down"
    });
    client
        .transport()
        .shell
        .audio()
        .set_follower(Some(Box::new(bl_engine::testing::following(140.0))));
    wait(&control, &mut client, "the follower", |c| {
        c.transport().shell.poll_engine().following
    });
    // The host hands the agent API the engine's transport after every event.
    pump(&control, &mut client);
    let refused = client
        .call("run", nudge_args("up", true))
        .unwrap()
        .ok()
        .unwrap_err();
    assert!(refused.contains("leads the tempo"), "{refused}");
    call(&control, &mut client, "run", nudge_args("down", false));
    wait(&control, &mut client, "the release", |c| {
        phase_nudge(&control, c) == "none"
    });
    client.transport().shell.audio_mut().stop();
}

#[test]
fn an_agent_s_scene_launch_moves_the_selection_as_the_person_s_does() {
    // Spec: agent-permissions R10; launch-quantisation R9, R16. Select Next
    // Scene on Launch is on by default.
    let (mut client, control) = host();
    let scenes: Vec<_> = client
        .transport()
        .shell
        .editor()
        .document()
        .scenes()
        .map(|scene| scene.id)
        .collect();
    let (first, second) = (scenes[0], scenes[1]);
    call(
        &control,
        &mut client,
        "run",
        json!({ "commands": [{ "command": "session.launch_scene", "args": { "scene": first } }] }),
    );
    let state = client
        .transport()
        .shell
        .run("session.state", json!({}))
        .unwrap();
    assert_eq!(state["scenes"], json!([second]), "{state}");
}

fn run_one(
    control: &NullControl,
    client: &mut Client<InProcess>,
    command: &str,
    args: Value,
) -> Value {
    call(
        control,
        client,
        "run",
        json!({ "commands": [{ "command": command, "args": args }] }),
    )
}

#[test]
fn a_set_change_cancels_scheduled_commands_and_forgets_ref_names() {
    // Spec: agent-permissions R1, R10. What an agent scheduled and named in
    // one set must not act on the next one, which numbers its objects alike.
    let (mut client, control) = host();
    client.transport().answer = Box::new(|_| true);
    let made = run_one(
        &control,
        &mut client,
        "track.create",
        json!({ "id": { "$new": "bass" }, "kind": "midi", "name": "Bass", "index": 0 }),
    );
    let bass = made["ids"]["bass"].as_u64().unwrap();
    call(
        &control,
        &mut client,
        "schedule",
        json!({ "action": "add", "at": "2.1.1", "command": "mixer.set_mute",
                "args": { "track": { "$ref": "bass" }, "mute": true } }),
    );
    run_one(&control, &mut client, "set.new", json!({}));
    let made = run_one(
        &control,
        &mut client,
        "track.create",
        json!({ "id": { "$new": "again" }, "kind": "midi", "name": "Again", "index": 0 }),
    );
    assert_eq!(made["ids"]["again"].as_u64().unwrap(), bass);
    call(
        &control,
        &mut client,
        "transport",
        json!({ "action": "play" }),
    );
    wait(&control, &mut client, "the playhead to pass 3.1.1", |c| {
        status(&control, c)["position"]["ticks"].as_i64().unwrap() > 2 * BAR
    });
    let host = client.transport();
    let track = host
        .shell
        .editor()
        .document()
        .track(bl_model::TrackId(bl_core::id::Id::new(bass).unwrap()))
        .unwrap();
    assert!(!track.mixer().mute, "the new set's track was muted");
    assert!(
        host.shell
            .editor()
            .undo_history()
            .all(|label| !label.contains("Set Mute")),
        "a step for the old set's command was recorded"
    );
    let list = scheduled(&control, &mut client);
    assert_eq!(list[0]["state"], "cancelled", "{list:?}");
    let refused = client
        .call(
            "run",
            json!({ "commands": [{ "command": "mixer.set_mute",
                    "args": { "track": { "$ref": "bass" }, "mute": true } }] }),
        )
        .unwrap();
    assert!(refused.is_error, "{}", refused.text);
    assert!(refused.text.contains("bass"), "{}", refused.text);
    client.transport().shell.audio_mut().stop();
}

#[test]
fn a_scheduled_phase_nudge_that_would_be_held_is_refused_and_its_release_is_taken() {
    // Spec: transport R28. A scheduled press has no session to end with.
    let (mut client, control) = host();
    let refused = client
        .call(
            "schedule",
            json!({ "action": "add", "at": "2.1.1", "command": "transport.phase_nudge",
                    "args": { "direction": "up", "held": true } }),
        )
        .unwrap();
    assert!(refused.is_error, "{}", refused.text);
    assert!(refused.text.contains("Phase Nudge"), "{}", refused.text);
    assert!(scheduled(&control, &mut client).is_empty());
    call(
        &control,
        &mut client,
        "schedule",
        json!({ "action": "add", "at": "2.1.1", "command": "transport.phase_nudge",
                "args": { "direction": "up", "held": false } }),
    );
}

#[test]
fn a_transport_refusal_reaches_the_agent_that_asked() {
    // Spec: recording R1, session-recording R11. The shell's refusal of a
    // second take comes back as the call's error, not "sent".
    let (mut client, control) = host();
    client.transport().answer = Box::new(|_| true);
    let track = audio_track(&control, &mut client);
    for (command, args) in [
        (
            "routing.set_input",
            json!({ "track": track,
                    "input": { "type": "external", "channel": { "type": "mono", "channel": 1 } } }),
        ),
        ("mixer.set_arm", json!({ "track": track, "arm": true })),
        ("transport.set_recording", json!({ "recording": true })),
    ] {
        run_one(&control, &mut client, command, args);
    }
    wait(&control, &mut client, "the engine to record", |c| {
        c.transport().shell.poll_engine().recording
    });
    client
        .call("transport", json!({ "action": "stop" }))
        .unwrap()
        .ok()
        .unwrap();
    // Run blocks until the engine has stopped recording, without polling the
    // shell: whether the take's writer has finished by then is a thread's
    // timing, so the shell must not get to see it.
    let host = client.transport();
    host.agent.sync_engine(&mut host.shell);
    for _ in 0..2_000 {
        if !host.shell.audio_mut().transport_state().recording {
            break;
        }
        control.run_frames(256);
    }
    assert!(!host.shell.audio_mut().transport_state().recording);
    // The shell hasn't polled, so it still holds the take.
    let refused = client
        .call(
            "run",
            json!({ "commands": [{ "command": "transport.set_recording",
                    "args": { "recording": true } }] }),
        )
        .unwrap();
    assert!(refused.is_error, "{}", refused.text);
    assert!(
        refused.text.contains("still being finished"),
        "{}",
        refused.text
    );
}

#[test]
fn an_arrangement_record_is_refused_while_a_session_recording_runs() {
    // Spec: session-recording R11.
    let (mut client, control) = host();
    client.transport().answer = Box::new(|_| true);
    let midi = call(
        &control,
        &mut client,
        "query",
        json!({ "path": "track \"1 MIDI\"" }),
    )["id"]
        .as_u64()
        .unwrap();
    run_one(
        &control,
        &mut client,
        "mixer.set_arm",
        json!({ "track": midi, "arm": true }),
    );
    run_one(&control, &mut client, "session.record", json!({}));
    let refused = client
        .call(
            "run",
            json!({ "commands": [{ "command": "transport.set_recording",
                    "args": { "recording": true } }] }),
        )
        .unwrap();
    assert!(refused.is_error, "{}", refused.text);
    assert!(
        refused.text.contains("Session recording"),
        "{}",
        refused.text
    );
    client.transport().shell.audio_mut().stop();
}

#[test]
fn a_scheduled_macro_move_writes_the_mapped_values_as_a_run_does() {
    // Spec: macros R13. A fired command goes through the shell, so the macro
    // law's values reach the document, not only the macro knob.
    const WIDTH: u32 = 6;
    let (mut client, control) = host();
    let (rack, utility) = {
        let shell = &mut client.transport().shell;
        let audio = shell.editor().document().top_level().nth(2).unwrap();
        shell
            .invoke("arrangement.select_tracks", json!({ "tracks": [audio] }))
            .unwrap();
        shell.set_focused_area(bl_shell::Area::Detail);
        let rack = shell
            .invoke(
                "device.add",
                json!({ "device": { "kind": { "rack": { "kind": "audio_effect" } },
                                    "name": "Rack", "role": "audio_effect" } }),
            )
            .unwrap()["device"]
            .as_u64()
            .unwrap();
        let chain = shell.new_id::<bl_model::ChainId>();
        shell
            .invoke(
                "chain.add",
                json!({ "rack": rack, "index": 0,
                        "chain": { "id": chain, "name": "Range", "devices": [] } }),
            )
            .unwrap();
        let utility = shell
            .invoke(
                "device.add",
                json!({ "chain": chain,
                        "device": { "kind": { "native": { "device_type": "beatlayer.utility" } },
                                    "name": "Utility", "role": "audio_effect" } }),
            )
            .unwrap()["device"]
            .as_u64()
            .unwrap();
        (rack, utility)
    };
    run_one(
        &control,
        &mut client,
        "rack.map_macro",
        json!({ "rack": rack, "macro_number": 1,
                "target": { "type": "device", "device": utility, "param": WIDTH } }),
    );
    let width = |client: &mut Client<InProcess>| {
        client
            .transport()
            .shell
            .editor()
            .document()
            .device(bl_model::DeviceId(bl_core::id::Id::new(utility).unwrap()))
            .and_then(|device| device.params.get(&bl_model::ParamId(WIDTH)).copied())
    };
    let knob =
        |value: f64| json!({ "device": rack, "param": bl_model::macro_param(1), "value": value });
    // What a run of the same move leaves in the Utility.
    run_one(&control, &mut client, "device.set_param", knob(0.25));
    let by_run = width(&mut client);
    run_one(&control, &mut client, "device.set_param", knob(0.5));
    assert_ne!(width(&mut client), by_run);
    call(
        &control,
        &mut client,
        "schedule",
        json!({ "action": "add", "at": "2.1.1", "command": "device.set_param",
                "args": knob(0.25) }),
    );
    call(
        &control,
        &mut client,
        "transport",
        json!({ "action": "play" }),
    );
    wait(&control, &mut client, "the command to fire", |c| {
        all_fired(&control, c)
    });
    assert_eq!(width(&mut client), by_run);
    client.transport().shell.audio_mut().stop();
}

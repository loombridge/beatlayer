//! Clip Mode on Push 2 through the agent API (WP-7.27;
//! `docs/specs/push-2-tracks.md`, R43 to R58): `push.state` reads the clip,
//! the controls on the encoders with their values, the zoom and the view,
//! and what the encoders set reads back in the query tool's clip.

use bl_agent::Agent;
use bl_agent::client::{Client, InProcess};
use bl_core::clock::TestClock;
use bl_midi_io::TestBackend;
use bl_model::TrackKind;
use bl_push::sim::Simulated;
use bl_shell::Shell;
use serde_json::{Value, json};

fn run(client: &mut Client<InProcess>, command: &str, args: Value) -> Value {
    let reply = client
        .call(
            "run",
            json!({ "commands": [{ "command": command, "args": args }] }),
        )
        .unwrap();
    assert!(!reply.is_error, "{command}: {}", reply.text);
    reply.ok().unwrap()["results"][0]["result"].clone()
}

fn query(client: &mut Client<InProcess>, args: Value) -> Value {
    let reply = client.call("query", args).unwrap();
    assert!(!reply.is_error, "{}", reply.text);
    reply.ok().unwrap()
}

// Spec: push-2-tracks R45, R49, R55, R58. An agent reads Clip Mode's state;
// what the simulated Push 2's encoders and buttons set reads back in the
// query tool's clip.
#[test]
fn an_agent_reads_clip_mode_and_reads_back_what_its_controls_set() {
    let test = TestBackend::new();
    let clock = TestClock::new();
    let mut sim = Simulated::new(&test, &clock);
    let mut shell = Shell::headless();
    shell.use_midi_backend(Box::new(test.clone()));
    shell.use_test_clock(clock.clone());
    shell.use_push_display(sim.frames().claim());
    let track = shell
        .editor()
        .document()
        .tracks()
        .find(|t| t.kind() == TrackKind::Midi)
        .map(bl_model::Track::id)
        .unwrap();
    let scene = shell.editor().document().scenes().next().unwrap().id;
    shell
        .invoke("arrangement.select_tracks", json!({ "tracks": [track] }))
        .unwrap();
    shell
        .invoke(
            "session.insert_empty_midi_clip",
            json!({ "track": track, "scene": scene }),
        )
        .unwrap();
    let clip = shell.editor().document().slot(track, scene).unwrap().id;
    sim.plug_in();
    shell.refresh_midi();
    let mut client = Client::new(InProcess::new(Agent::headless(true), shell));
    client.initialize("test-agent").unwrap();
    run(&mut client, "view.toggle_session_arrangement", json!({}));
    let settle = |client: &mut Client<InProcess>, sim: &mut Simulated| {
        for _ in 0..6 {
            client.transport().shell.poll();
            client.transport().shell.midi().surfaces().tick();
            // What the tick made the surfaces ask for, before the agent calls.
            client.transport().shell.poll();
            loop {
                run(client, "control_surface.list", json!({}));
                if sim.pump() == 0 {
                    break;
                }
            }
            clock.advance(std::time::Duration::from_millis(30));
        }
    };
    settle(&mut client, &mut sim);
    run(&mut client, "push.set_mode", json!({ "display": "clip" }));
    settle(&mut client, &mut sim);

    let state = run(&mut client, "push.state", json!({}));
    let push = &state["pushes"][0];
    assert_eq!(push["display_mode"], "clip", "{state}");
    let shown = &push["clip"];
    assert_eq!(shown["clip"], json!(clip), "{shown}");
    assert_eq!(shown["shows"], "midi");
    assert_eq!(shown["looping"], false);
    assert_eq!(shown["zoom"], 1.0);
    assert_eq!(shown["view"], json!([0.0, 4.0]));
    let names: Vec<&str> = shown["controls"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["Zoom", "Start", "End"]);
    assert_eq!(shown["held_steps"], json!([]));
    assert_eq!(shown["select_pad"], Value::Null);

    // The third encoder sets End, and the second button switches Loop: both
    // read back in the query tool's clip.
    // A gesture parks agent calls until it ends, so it ends before the
    // agent looks.
    assert!(sim.touch(2));
    assert!(sim.turn(2, -10));
    assert!(sim.release(2));
    settle(&mut client, &mut sim);
    assert!(sim.press("upper_2"));
    settle(&mut client, &mut sim);
    assert!(sim.let_go("upper_2"));
    settle(&mut client, &mut sim);
    let read = query(&mut client, json!({ "id": clip.0, "detail": "full" }));
    assert_eq!(read["saved"]["data"]["midi"]["end"], 3 * 705_600, "{read}");
    assert_eq!(read["saved"]["data"]["midi"]["looping"], true);
    let state = run(&mut client, "push.state", json!({}));
    assert_eq!(state["pushes"][0]["clip"]["looping"], true);
    let names: Vec<&str> = state["pushes"][0]["clip"]["controls"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        ["Zoom", "Loop Position", "Loop Length", "Start Offset"]
    );
}

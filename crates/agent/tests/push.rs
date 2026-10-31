//! Push 2 through the agent API (WP-7.19; `docs/specs/push-2.md`, R34):
//! `push.state` reads the modes, the LEDs, the display's text and the Setup
//! values of a simulated Push 2, `push.set_mode` and `push.set_setup` change
//! them and read back, and `push.display_frame` returns a PNG.

use bl_agent::Agent;
use bl_agent::client::{Client, InProcess};
use bl_core::clock::TestClock;
use bl_midi_io::TestBackend;
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

// Spec: push-2 R34. The state, the modes, the setup and the frame, by agent.
#[test]
fn an_agent_reads_and_sets_a_simulated_push_2() {
    let test = TestBackend::new();
    let clock = TestClock::new();
    let mut sim = Simulated::new(&test, &clock);
    let mut shell = Shell::headless();
    shell.use_midi_backend(Box::new(test.clone()));
    shell.use_test_clock(clock.clone());
    shell.use_push_display(sim.frames().claim());
    sim.plug_in();
    shell.refresh_midi();
    let mut client = Client::new(InProcess::new(Agent::headless(true), shell));
    client.initialize("test-agent").unwrap();

    // Let the device answer the start, then read.
    let settle = |client: &mut Client<InProcess>, sim: &mut Simulated| {
        loop {
            run(client, "control_surface.list", json!({}));
            if sim.pump() == 0 {
                break;
            }
        }
    };
    settle(&mut client, &mut sim);
    let state = run(&mut client, "push.state", json!({}));
    let push = &state["pushes"][0];
    assert_eq!(push["row"], 1);
    assert_eq!(push["pad_mode"], "note");
    assert_eq!(push["display_mode"], "device");
    assert_eq!(push["setup"]["pad_sensitivity"], 5);
    assert!(push["leds"]["play"].is_object());
    assert!(push["display"]["text"].is_array());

    // push.set_setup reads back in push.state.
    run(
        &mut client,
        "push.set_setup",
        json!({ "pad_dynamics": 9, "workflow": "clip" }),
    );
    let state = run(&mut client, "push.state", json!({}));
    assert_eq!(state["pushes"][0]["setup"]["pad_dynamics"], 9);
    assert_eq!(state["pushes"][0]["setup"]["workflow"], "clip");

    // An out-of-range value is refused by the schema.
    let reply = client
        .call(
            "run",
            json!({ "commands": [{ "command": "push.set_setup", "args": { "pad_gain": 11 } }] }),
        )
        .unwrap();
    assert!(reply.is_error || reply.ok().unwrap()["results"][0].get("error").is_some());

    // Spec: push-2-tracks R14. Browse Mode's fields are part of the state,
    // empty while another mode is shown.
    assert!(push["browse"].is_null());
    assert!(push["session"].is_null(), "Note Mode");

    let frame = run(&mut client, "push.display_frame", json!({}));
    assert_eq!(frame["width"], 960);
    assert!(frame["png"].as_str().unwrap().starts_with("iVBOR"));
}

// Spec: push-2-tracks R73. Session Mode's fields are part of the state: the
// ring, what each pad and Scene/Grid button shows, Session Overview and the
// Stop Clip lock.
#[test]
fn an_agent_reads_what_session_mode_shows() {
    let test = TestBackend::new();
    let clock = TestClock::new();
    let mut sim = Simulated::new(&test, &clock);
    let mut shell = Shell::headless();
    shell.use_midi_backend(Box::new(test.clone()));
    shell.use_test_clock(clock.clone());
    shell.use_push_display(sim.frames().claim());
    shell
        .invoke("track.create", json!({ "kind": "midi", "name": "Keys" }))
        .unwrap();
    sim.plug_in();
    shell.refresh_midi();
    // The surface paints on its tick, so the test ticks it before the agent
    // reads.
    let step = |shell: &mut Shell, sim: &mut Simulated| {
        for _ in 0..3 {
            shell.poll();
            shell.midi().surfaces().tick();
            while sim.pump() > 0 {
                shell.midi().surfaces().settle();
            }
            shell.midi().surfaces().settle();
        }
    };
    step(&mut shell, &mut sim);
    let doc = shell.editor().document();
    let first = doc.top_level().next().unwrap();
    let (track, scene) = (
        doc.top_level()
            .find(|t| doc.track(*t).unwrap().kind() == bl_model::TrackKind::Midi)
            .unwrap(),
        doc.scenes().next().unwrap().id,
    );
    shell
        .run(
            "session.insert_empty_midi_clip",
            json!({ "track": track, "scene": scene }),
        )
        .unwrap();
    shell
        .run("push.set_mode", json!({ "pad": "session" }))
        .unwrap();
    step(&mut shell, &mut sim);
    let mut client = Client::new(InProcess::new(Agent::headless(true), shell));
    client.initialize("test-agent").unwrap();

    let state = run(&mut client, "push.state", json!({}));
    let session = &state["pushes"][0]["session"];
    assert_eq!(session["first_track"], json!(first));
    assert_eq!(session["first_scene"], json!(scene));
    assert_eq!(session["scenes"], 8);
    assert_eq!(session["overview"], false);
    assert_eq!(session["stop_clip_locked"], false);
    assert_eq!(session["pads"].as_array().unwrap().len(), 64);
    let pad = |row: usize, column: usize| session["pads"][row * 8 + column].clone();
    assert_eq!(pad(0, 0)["shows"], "stopped");
    assert!(pad(0, 0)["clip"].is_number());
    assert_eq!(pad(1, 0)["shows"], "empty");
    assert_eq!(pad(0, 7)["shows"], "none", "past the last track");
    assert_eq!(session["scene_buttons"].as_array().unwrap().len(), 8);
}

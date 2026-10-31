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

    // Spec: push-2-tracks R42. Mix Mode's fields are part of the state too
    // (the shell's tests read them while Mix is shown), empty in other modes.
    assert!(push["mix_mode"].is_null());

    let frame = run(&mut client, "push.display_frame", json!({}));
    assert_eq!(frame["width"], 960);
    assert!(frame["png"].as_str().unwrap().starts_with("iVBOR"));
}

// Spec: push-2-tracks R27. In Device Mode `push.state` reads the devices on
// the upper buttons with their depth and selection, whether Edit Mode is on,
// the bank and its page, the parameters on the encoders with their names and
// values, the chains shown and whether Mute is locked.
#[test]
fn an_agent_reads_device_mode_through_push_state() {
    let test = TestBackend::new();
    let clock = TestClock::new();
    let mut sim = Simulated::new(&test, &clock);
    let mut shell = Shell::headless();
    shell.use_midi_backend(Box::new(test.clone()));
    shell.use_test_clock(clock.clone());
    shell.use_push_display(sim.frames().claim());
    shell.start_audio();
    sim.plug_in();
    shell.refresh_midi();
    // A MIDI track, selected as a new track is, with two effects.
    shell
        .invoke("track.create", json!({ "kind": "midi", "name": "Keys" }))
        .unwrap();
    let added: Vec<Value> = (0..2)
        .map(|_| {
            shell
                .invoke(
                    "device.add",
                    json!({ "device": {
                        "kind": { "native": { "device_type": "beatlayer.utility" } },
                        "name": "Utility",
                        "role": "audio_effect",
                    } }),
                )
                .unwrap()["device"]
                .clone()
        })
        .collect();
    let mut client = Client::new(InProcess::new(Agent::headless(true), shell));
    client.initialize("test-agent").unwrap();
    // The surface ticks with the set as it stands, then the device and the
    // host talk until both are quiet.
    let settle = |client: &mut Client<InProcess>, sim: &mut Simulated| {
        client.transport().shell.poll();
        client.transport().shell.midi().surfaces().tick();
        loop {
            run(client, "control_surface.list", json!({}));
            if sim.pump() == 0 {
                break;
            }
        }
        client.transport().shell.poll();
    };
    for _ in 0..4 {
        settle(&mut client, &mut sim);
    }
    let state = run(&mut client, "push.state", json!({}));
    let device = &state["pushes"][0]["device"];
    assert_eq!(device["empty"], false, "{state}");
    assert_eq!(device["edit"], false);
    assert_eq!(device["mute_locked"], false);
    assert_eq!(device["bank"], 0);
    assert_eq!(device["banks"], json!(["Utility", "Bass"]));
    assert_eq!(device["bank_page"], 0);
    assert_eq!(device["bank_pages"], 1);
    assert_eq!(device["params"].as_array().unwrap().len(), 8);
    assert_eq!(device["chains"], json!([]));
    let on_buttons: Vec<_> = device["devices"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| (d["button"].clone(), d["device"].clone(), d["depth"].clone()))
        .collect();
    assert_eq!(
        on_buttons,
        [
            (json!(1), added[0].clone(), json!(0)),
            (json!(2), added[1].clone(), json!(0))
        ]
    );
    assert_eq!(device["devices"][1]["selected"], true);
    assert_eq!(device["devices"][1]["kind"], "audio_effect");

    // Another display mode: no device fields.
    run(&mut client, "push.set_mode", json!({ "display": "mix" }));
    settle(&mut client, &mut sim);
    settle(&mut client, &mut sim);
    let state = run(&mut client, "push.state", json!({}));
    assert!(state["pushes"][0]["device"].is_null());
}

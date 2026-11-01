//! Beats on Push 2 through the agent API (WP-7.21; `docs/specs/push-2-notes.md`,
//! R1 to R31): `push.state` reads the layout, the bank, the pages and the held
//! steps of Note Mode, `pad.copy` and `pad.set_push_colour` read back in the
//! query tool's chains, and the track reads whether its arm is by selection.

use bl_agent::Agent;
use bl_agent::client::{Client, InProcess};
use bl_core::clock::TestClock;
use bl_midi_io::TestBackend;
use bl_model::{DeviceId, TrackKind};
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

// Spec: push-2-notes R1, R3, R5, R17, R19. An agent reads Note Mode's state,
// copies a pad and colours it, and reads both back.
#[test]
fn an_agent_reads_note_mode_and_reads_back_pad_copy_and_pad_colours() {
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
    shell
        .invoke("arrangement.select_tracks", json!({ "tracks": [track] }))
        .unwrap();
    let added = shell
        .invoke(
            "device.add",
            json!({ "device": {
                "kind": { "rack": { "kind": "drum" } },
                "name": "Drum Rack",
                "role": "instrument",
            } }),
        )
        .unwrap();
    let rack: DeviceId = serde_json::from_value(added["device"].clone()).unwrap();
    let gain = bl_engine::testing::gain_type();
    for note in [36, 37] {
        shell
            .invoke(
                "pad.drop",
                json!({ "rack": rack, "note": note,
                        "device": { "kind": gain.kind, "name": "Gain", "role": gain.role } }),
            )
            .unwrap();
    }
    sim.plug_in();
    shell.refresh_midi();
    // Let the device answer the start and the surface tick.
    for _ in 0..10 {
        shell.poll();
        shell.midi().surfaces().tick();
        loop {
            shell.midi().surfaces().settle();
            if sim.pump() == 0 {
                break;
            }
        }
        clock.advance(std::time::Duration::from_millis(30));
    }
    let mut client = Client::new(InProcess::new(Agent::headless(true), shell));
    client.initialize("test-agent").unwrap();

    let state = run(&mut client, "push.state", json!({}));
    let note = &state["pushes"][0]["note"];
    assert_eq!(note["layout"], "Loop Selector", "{state}");
    assert_eq!(note["bank"], 36);
    assert_eq!(note["step_size"], "1/16");
    assert_eq!(note["page"], 1);
    assert_eq!(note["following"], true);
    assert_eq!(note["held_steps"], json!([]));
    assert_eq!(note["track"], json!(track));

    // The track reads that its arm is by selection.
    let read = query(&mut client, json!({ "id": track }));
    assert_eq!(read["mixer"]["arm"], true, "{read}");
    assert_eq!(read["mixer"]["arm_by_selection"], true);

    // pad.set_push_colour and pad.copy read back in the rack's chains.
    run(
        &mut client,
        "pad.set_push_colour",
        json!({ "rack": rack, "note": 36, "colour": 11 }),
    );
    run(
        &mut client,
        "pad.copy",
        json!({ "rack": rack, "from": 36, "to": 40 }),
    );
    let read = query(&mut client, json!({ "id": rack.0 }));
    let chains = read["chains"].as_array().unwrap();
    let on = |n: u64| -> Vec<&Value> {
        chains
            .iter()
            .filter(|c| c["receive"] == json!({ "note": n }))
            .collect()
    };
    assert_eq!(on(36)[0]["push_colour"], 11, "{read}");
    assert_eq!(on(40).len(), 1, "the copy");
    assert_eq!(on(40)[0]["push_colour"], 11, "the copy keeps the colour");
    assert_eq!(on(37)[0]["push_colour"], Value::Null);
}

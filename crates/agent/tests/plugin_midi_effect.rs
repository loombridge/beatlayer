//! A plug-in as a MIDI effect through the agent API (WP-5.33; IMP-24): an
//! agent loads a plug-in, places it in the MIDI-effect position with
//! `device.set_role`, reads the placement back, undoes it in one step, and
//! is refused one without a note output.

use bl_agent::Agent;
use bl_agent::client::{Client, InProcess};
use bl_model::{DeviceId, DeviceRole, TrackId, TrackKind};
use bl_shell::Shell;
use serde_json::{Value, json};

fn client() -> Client<InProcess> {
    let mut client = Client::new(InProcess::new(Agent::headless(true), Shell::headless()));
    client.initialize("test-agent").unwrap();
    client
}

fn run(client: &mut Client<InProcess>, command: &str, args: Value) -> Value {
    client
        .call(
            "run",
            json!({ "commands": [{ "command": command, "args": args }] }),
        )
        .unwrap()
        .ok()
        .unwrap_or_else(|error| panic!("{command} failed: {error}"))["results"][0]["result"]
        .clone()
}

fn query(client: &mut Client<InProcess>, args: Value) -> Value {
    client
        .call("query", args.clone())
        .unwrap()
        .ok()
        .unwrap_or_else(|error| panic!("query {args}: {error}"))
}

fn midi_track(shell: &Shell) -> TrackId {
    shell
        .editor()
        .document()
        .tracks()
        .find(|track| track.kind() == TrackKind::Midi)
        .map(bl_model::Track::id)
        .expect("the default set has a MIDI track")
}

/// The role the set gives `device`.
fn role(client: &mut Client<InProcess>, device: DeviceId) -> DeviceRole {
    client
        .transport()
        .shell
        .editor()
        .document()
        .device(device)
        .expect("the device is in the set")
        .role
}

/// Loads the test plug-in onto the default set's MIDI track.
fn load(client: &mut Client<InProcess>) -> DeviceId {
    let track = midi_track(&client.transport().shell);
    let loaded = run(
        client,
        "plugin.load",
        json!({ "path": bl_test_clap::bundle(), "track": track }),
    );
    serde_json::from_value(loaded["device"].clone()).unwrap()
}

// Spec: device-chains R2, R3; clap-hosting R1; IMP-24
#[test]
fn an_agent_places_a_plugin_as_a_midi_effect_and_reads_it_back() {
    let mut client = client();
    let device = load(&mut client);
    assert_eq!(role(&mut client, device), DeviceRole::AudioEffect);

    let steps = client.transport().shell.editor().undo_history().len();
    run(
        &mut client,
        "device.set_role",
        json!({ "device": device, "role": "midi_effect" }),
    );
    assert_eq!(role(&mut client, device), DeviceRole::MidiEffect);
    assert_eq!(
        client.transport().shell.editor().undo_history().len(),
        steps + 1,
        "one undo step"
    );

    // The placement reads back through the query tool.
    let found = query(
        &mut client,
        json!({ "path": format!("device {}", device.0.get()) }),
    );
    assert_eq!(found["role"], "midi_effect", "{found}");

    // One undo step puts it back where its type goes.
    run(&mut client, "edit.undo", json!({}));
    assert_eq!(role(&mut client, device), DeviceRole::AudioEffect);
}

// Spec: device-chains R2; clap-hosting R1; IMP-24
#[test]
fn an_agent_is_refused_a_plugin_without_a_note_output() {
    // SAFETY: the plug-in reads this when it is asked for its note ports,
    // and this test runs in a process of its own.
    unsafe { std::env::set_var(bl_test_clap::NOTE_PORTS_VAR, "none") };
    let mut client = client();
    let device = load(&mut client);
    let refused = client
        .call(
            "run",
            json!({ "commands": [{ "command": "device.set_role",
                                   "args": { "device": device, "role": "midi_effect" } }] }),
        )
        .unwrap()
        .ok()
        .unwrap_err();
    assert!(
        refused.to_string().contains("note output"),
        "refused: {refused}"
    );
    assert_eq!(role(&mut client, device), DeviceRole::AudioEffect);
}

// Spec: agent-permissions R6; clap-hosting R1
#[test]
fn an_agent_s_plugin_load_is_its_undo_step_under_its_label() {
    let mut client = client();
    let track = midi_track(&client.transport().shell);
    let ran = client
        .call(
            "run",
            json!({ "commands": [{ "command": "plugin.load",
                                   "args": { "path": bl_test_clap::bundle(), "track": track } }],
                    "label": "Load the synth" }),
        )
        .unwrap()
        .ok()
        .unwrap_or_else(|error| panic!("plugin.load failed: {error}"));
    assert_eq!(ran["undo_step"], "test-agent: Load the synth");
    assert_eq!(
        client.transport().shell.editor().undo_history().next(),
        Some("test-agent: Load the synth")
    );
}

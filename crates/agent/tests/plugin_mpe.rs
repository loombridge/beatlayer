//! A plug-in's MPE Mode and a track's MPE MIDI To through the agent API
//! (WP-6.17b; the MPE spec, R14, R18, R19): the commands run by ID, and
//! `plugin.info` and the track query read them back. The plug-in is the
//! test plug-in, built from source.

use bl_agent::Agent;
use bl_agent::client::{Client, InProcess};
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

fn call(client: &mut Client<impl bl_agent::client::Transport>, tool: &str, args: Value) -> Value {
    client
        .call(tool, args)
        .unwrap()
        .ok()
        .unwrap_or_else(|error| panic!("{tool} failed: {error}"))
}

#[test]
fn an_agent_sets_mpe_for_a_plugin_and_a_tracks_midi_to_and_reads_both_back() {
    // Spec: mpe R14, R18, R19. `plugin.set_mpe`, `plugin.set_mpe_settings`
    // and `routing.set_midi_output` run by ID; `plugin.info` and the track
    // query read them back.
    // Loading a plug-in is external: the agent is autonomous.
    let mut client = headless(true);
    let made = call(
        &mut client,
        "run",
        json!({ "commands": [
            { "command": "track.create",
              "args": { "id": {"$new": "keys"}, "kind": "midi", "name": "Keys", "index": 0 } },
        ] }),
    );
    let keys = made["ids"]["keys"].as_u64().unwrap();
    // Loading a plug-in is a call of its own.
    let loaded = call(
        &mut client,
        "run",
        json!({ "commands": [
            { "command": "plugin.load",
              "args": { "path": bl_test_clap::bundle(), "track": keys } },
        ] }),
    );
    let device = loaded["results"][0]["result"]["device"].as_u64().unwrap();
    // The engine starts it, as the app's does; `plugin.info` answers once it
    // has.
    client.transport().shell.start_audio();
    let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        client.transport().shell.poll();
        let ran = client
            .call(
                "run",
                json!({ "commands": [{ "command": "plugin.info", "args": { "device": device } }] }),
            )
            .unwrap();
        if !ran.is_error {
            break;
        }
        assert!(std::time::Instant::now() < until, "{}", ran.text);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let info = |client: &mut Client<InProcess>| {
        let ran = call(
            client,
            "run",
            json!({ "commands": [{ "command": "plugin.info", "args": { "device": device } }] }),
        );
        ran["results"][0]["result"]["mpe"].clone()
    };
    assert_eq!(info(&mut client), json!({ "on": false }));

    let steps = client.transport().shell.editor().undo_history().len();
    for (command, args) in [
        ("plugin.set_mpe", json!({ "device": device, "on": true })),
        (
            "plugin.set_mpe_settings",
            json!({ "device": device, "settings": { "zone": "upper", "channels": 7 } }),
        ),
    ] {
        let ran = call(
            &mut client,
            "run",
            json!({ "commands": [{ "command": command, "args": args }] }),
        );
        assert_eq!(ran["ran"], 1, "{command}");
    }
    assert_eq!(
        client.transport().shell.editor().undo_history().len(),
        steps + 2,
        "one undo step each"
    );
    assert_eq!(
        info(&mut client),
        json!({ "on": true, "settings": { "zone": "upper", "channels": 7 } })
    );

    // A track's MIDI To: a port with MPE and its settings, read back in the
    // track query's routing.
    call(
        &mut client,
        "run",
        json!({ "commands": [
            { "command": "routing.set_midi_output",
              "args": { "track": keys,
                        "to": { "name": "Synth", "channel": 2,
                                "mpe": { "mode": "multi_channel", "first": 3, "last": 8 } } } },
        ] }),
    );
    let queried = call(
        &mut client,
        "query",
        json!({ "id": keys, "detail": "full" }),
    );
    assert_eq!(
        queried["saved"]["routing"]["midi_output"],
        json!({ "name": "Synth", "channel": 2,
                "mpe": { "mode": "multi_channel", "first": 3, "last": 8 } })
    );
    // A zone of 16 note channels is refused, and the track keeps its MIDI To.
    let refused = client
        .call(
            "run",
            json!({ "commands": [
                { "command": "routing.set_midi_output",
                  "args": { "track": keys,
                            "to": { "name": "Synth", "channel": 2,
                                    "mpe": { "channels": 16 } } } },
            ] }),
        )
        .unwrap();
    assert!(refused.is_error, "{}", refused.text);
}

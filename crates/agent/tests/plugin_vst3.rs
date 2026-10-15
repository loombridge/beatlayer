//! VST3 hosting, part 1 (WP-2.7; the VST3 hosting spec, R1 and R6):
//! `plugin.load` loads a `.vst3` bundle by path through the agent API, the
//! set holds it as a VST3 device, an engine starts its plug-in, and
//! `plugin.info` reports it.

use std::time::{Duration, Instant};

use bl_agent::Agent;
use bl_agent::client::{Client, InProcess};
use bl_model::{DeviceId, DeviceKind, PluginFormat, TrackKind};
use bl_shell::Shell;
use serde_json::{Value, json};

fn client(autonomous: bool) -> Client<InProcess> {
    let mut client = Client::new(InProcess::new(
        Agent::headless(autonomous),
        Shell::headless(),
    ));
    client.initialize("test-agent").unwrap();
    client
}

fn call(client: &mut Client<InProcess>, tool: &str, args: Value) -> Value {
    client
        .call(tool, args)
        .unwrap()
        .ok()
        .unwrap_or_else(|error| panic!("{tool} failed: {error}"))
}

fn run(client: &mut Client<InProcess>, command: &str, args: Value) -> Value {
    call(
        client,
        "run",
        json!({ "commands": [{ "command": command, "args": args }] }),
    )
}

// Spec: vst3-hosting R1, R6
#[test]
fn a_vst3_plugin_loads_by_path_through_the_agent_api() {
    // Loading a plug-in reaches outside the set, so this agent is
    // autonomous.
    let mut client = client(true);
    let track = client
        .transport()
        .shell
        .editor()
        .document()
        .tracks()
        .find(|t| t.kind() == TrackKind::Audio)
        .map(bl_model::Track::id)
        .expect("the default set has an audio track");
    let loaded = run(
        &mut client,
        "plugin.load",
        json!({ "path": bl_test_vst3::bundle(), "track": track }),
    );
    let result = &loaded["results"][0]["result"];
    let device: DeviceId = serde_json::from_value(result["device"].clone()).unwrap();
    assert_eq!(result["plugin"], bl_test_vst3::plugin_id());
    assert_eq!(result["name"], bl_test_vst3::PLUGIN_NAME);

    // The set holds it as a VST3 device of that class.
    let kind = client
        .transport()
        .shell
        .editor()
        .document()
        .device(device)
        .expect("the device is in the set")
        .kind
        .clone();
    assert_eq!(
        kind,
        DeviceKind::Plugin {
            format: PluginFormat::Vst3,
            plugin_id: bl_test_vst3::plugin_id()
        }
    );

    // An engine plays the set: the plug-in starts, and `plugin.info`
    // reports it, its file, and its Gain parameter.
    client.transport().shell.start_audio();
    let until = Instant::now() + Duration::from_secs(10);
    let info = loop {
        client.transport().shell.poll();
        let attempt = client
            .call(
                "run",
                json!({ "commands": [{ "command": "plugin.info", "args": { "device": device } }] }),
            )
            .unwrap()
            .ok();
        if let Ok(value) = &attempt
            && value["results"][0]["result"]["active"] == true
        {
            break value["results"][0]["result"].clone();
        }
        assert!(
            Instant::now() < until,
            "the plug-in never started: {attempt:?}"
        );
        std::thread::sleep(Duration::from_millis(5));
    };
    assert_eq!(info["plugin"], bl_test_vst3::plugin_id());
    assert_eq!(
        info["path"],
        bl_test_vst3::bundle().to_string_lossy().as_ref()
    );
    let values = info["values"].as_array().expect("its values");
    assert!(
        values.iter().any(|value| value[0] == 0 && value[1] == 1.0),
        "the Gain is 1 by default: {values:?}"
    );
}

//! LV2 hosting (WP-5.39; the LV2 hosting spec, R1, R3, R5 and R17):
//! `plugin.load` loads an LV2 bundle through the agent API, the set holds it
//! as an LV2 device, an engine starts its plug-in, `device.set_param` and
//! `plugin.info` reach it, and on Linux the scanner finds the bundle. They run
//! where lilv is: on Linux always, elsewhere when `BL_TEST_LILV` names a
//! lilv library.

use bl_agent::Agent;
use bl_agent::client::{Client, InProcess};
use bl_model::{DeviceId, DeviceKind, PluginFormat, TrackKind};
use bl_shell::Shell;
use serde_json::{Value, json};

fn client(autonomous: bool) -> Option<Client<InProcess>> {
    let shell = Shell::headless();
    if let Some(library) = std::env::var_os("BL_TEST_LILV") {
        shell.lv2().use_library(library);
    }
    shell.lv2().world().ok()?;
    let mut client = Client::new(InProcess::new(Agent::headless(autonomous), shell));
    client.initialize("test-agent").unwrap();
    Some(client)
}

fn run(client: &mut Client<InProcess>, command: &str, args: Value) -> Value {
    client
        .call(
            "run",
            json!({ "commands": [{ "command": command, "args": args }] }),
        )
        .unwrap()
        .ok()
        .unwrap_or_else(|error| panic!("{command} failed: {error}"))
}

// Spec: lv2-hosting R1, R3, R5, R17
#[test]
fn an_lv2_plugin_loads_by_bundle_and_takes_a_parameter_through_the_agent_api() {
    // Loading a plug-in reaches outside the set, so this agent is
    // autonomous.
    let Some(mut client) = client(true) else {
        return;
    };
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
        json!({ "path": bl_test_lv2::bundle(), "track": track }),
    );
    let result = &loaded["results"][0]["result"];
    let device: DeviceId = serde_json::from_value(result["device"].clone()).unwrap();
    assert_eq!(result["plugin"], bl_test_lv2::PLUGIN_URI);
    assert_eq!(result["name"], bl_test_lv2::PLUGIN_NAME);

    // The set holds it as an LV2 device of that URI.
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
            format: PluginFormat::Lv2,
            plugin_id: bl_test_lv2::PLUGIN_URI.to_owned()
        }
    );

    // An engine plays the set: the plug-in starts, and a parameter an agent
    // sets reaches it; `plugin.info` reports both.
    client.transport().shell.start_audio();
    run(
        &mut client,
        "device.set_param",
        json!({ "device": device, "param": bl_test_lv2::GAIN, "value": 0.25 }),
    );
    let info = loop {
        client.transport().shell.poll();
        let attempt = client
            .call(
                "run",
                json!({ "commands": [{ "command": "plugin.info", "args": { "device": device } }] }),
            )
            .unwrap()
            .ok();
        if let Ok(value) = &attempt {
            let info = &value["results"][0]["result"];
            let gain = info["values"]
                .as_array()
                .and_then(|v| v.iter().find(|v| v[0] == bl_test_lv2::GAIN))
                .map(|v| v[1].clone());
            if info["active"] == true && gain == Some(json!(0.25)) {
                break info.clone();
            }
        }
        std::thread::yield_now();
    };
    assert_eq!(info["plugin"], bl_test_lv2::PLUGIN_URI);
    assert_eq!(info["failure"], Value::Null);
    assert!(
        info["path"]
            .as_str()
            .is_some_and(|p| p.ends_with("test-gain-echo.lv2")),
        "{info}"
    );
}

// Spec: plugin-scanning R1, R9. On Linux the scanner describes LV2 plug-ins
// through the agent API.
#[cfg(target_os = "linux")]
#[test]
fn a_scan_lists_lv2_plug_ins_through_the_agent_api() {
    if bl_plugin_host::scan::scan_program().is_none() {
        return;
    }
    // Scanning reads plug-in files outside the project, so this agent is
    // autonomous.
    let Some(mut client) = client(true) else {
        return;
    };
    let scanned = run(
        &mut client,
        "plugin.scan",
        json!({ "paths": [bl_test_lv2::bundle()] }),
    );
    let plugins = scanned["results"][0]["result"]["plugins"]
        .as_array()
        .expect("the plug-ins");
    assert_eq!(plugins.len(), 1, "{plugins:?}");
    assert_eq!(plugins[0]["format"], "lv2");
    assert_eq!(plugins[0]["plugin_id"], bl_test_lv2::PLUGIN_URI);
}

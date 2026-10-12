//! Plug-ins built on the plug-in kit, driven by a scripted MCP client
//! through both of their routes into Beatlayer (the device framework spec,
//! R23 to R25; the CLAP hosting spec, R18 and R19): compiled in as a native
//! device, and loaded as a CLAP plug-in that offers the kit's
//! agent-commands extension. Either way the plug-in's own commands are
//! device commands an agent lists, describes, queries, runs and schedules;
//! a run is one undo step labelled with the agent's name; the parameter
//! change reaches the engine on its frame; and the state survives saving
//! and reopening the set.

use std::sync::Arc;

use bl_agent::Agent;
use bl_agent::client::{Client, InProcess};
use bl_engine::testing::{self, Probe};
use bl_engine::{EngineConfig, Offline};
use bl_model::{Command, Device, DeviceId, DeviceKind, ParamId, TrackId, TrackKind};
use bl_plugin_host::clap::EntryDescriptor;
use bl_shell::Shell;
use bl_shell::commands::{SET_NEW, SET_OPEN, SET_SAVE_AS};
use lspk_example_gain::{GAIN, Gain};
use serde_json::{Value, json};

const RATE: bl_core::time::SampleRate = bl_core::time::SampleRate::HZ_48000;
const FRAMES: usize = 64;

/// Example Gain as a CLAP plug-in, with the agent interface its own build
/// exports, compiled into this test binary: Beatlayer hosts it through
/// CLAP's ABI, its agent-commands extension included.
static GAIN_CLAP: EntryDescriptor = lspk_plugin::clap_entry!(lspk_plugin_agent::WithAgent<Gain>);

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

fn audio_track(shell: &Shell) -> TrackId {
    shell
        .editor()
        .document()
        .tracks()
        .find(|t| t.kind() == TrackKind::Audio)
        .map(bl_model::Track::id)
        .expect("the default set has an audio track")
}

/// Puts a constant 1.0 at the head of `track`, so the level after the
/// plug-in is its gain.
fn dc_source(shell: &mut Shell, track: TrackId) {
    shell.edit(|editor| {
        let id: DeviceId = editor.new_id();
        let mut dc = Device::new(
            id,
            "",
            DeviceKind::Native {
                device_type: testing::DC.to_owned(),
            },
        );
        dc.params.insert(testing::VALUE, 1.0);
        editor
            .apply(Command::InsertDevice {
                track,
                index: 0,
                device: dc,
            })
            .unwrap();
        editor.clear_history().unwrap();
    });
}

/// An engine playing the shell's set, its compiler on this thread, so the
/// plug-ins' processors arrive at once.
fn engine(shell: &Shell) -> Offline {
    let mut registry = testing::registry(&Arc::new(Probe::default()));
    registry.add(shell.kit().factory());
    registry.add(shell.plugins().factory());
    let mut offline = Offline::new(EngineConfig::new(RATE, FRAMES), registry);
    offline.sync(shell.editor().document());
    offline
}

/// One block of the Main output's left channel.
fn block(offline: &mut Offline) -> Vec<f32> {
    let mut last = Vec::new();
    offline.render(FRAMES as u64, |out| last = out.channel(0).to_vec());
    last
}

/// Renders past a 20 ms glide and returns the level it settled on.
fn settled(offline: &mut Offline) -> f32 {
    let mut last = Vec::new();
    for _ in 0..20 {
        last = block(offline);
    }
    assert!(last.iter().all(|s| *s == last[0]), "{last:?}");
    last[0]
}

fn listed(client: &mut Client<InProcess>, prefix: &str) -> Vec<Value> {
    let listed = call(client, "list_commands", json!({ "area": "device" }));
    listed["commands"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["command"].as_str().unwrap().starts_with(prefix))
        .cloned()
        .collect()
}

fn param(shell: &Shell, device: DeviceId, param: ParamId) -> Option<f64> {
    shell
        .editor()
        .document()
        .device(device)
        .and_then(|d| d.params.get(&param).copied())
}

/// Checks the next block glides from `from` towards `to` from its first
/// frame, and settles there with no allocation on the audio side.
fn glides_from_the_first_frame(offline: &mut Offline, from: f32, to: f32) {
    let out = block(offline);
    let rising = to > from;
    assert!(
        if rising { out[0] > from } else { out[0] < from },
        "the glide starts on the block's first frame: {}",
        out[0]
    );
    assert!(
        out.windows(2)
            .all(|w| if rising { w[1] > w[0] } else { w[1] < w[0] }),
        "and moves every frame"
    );
    assert!((settled(offline) - to).abs() < 1e-6);
    assert_eq!(offline.heap_operations(), 0);
}

/// Checks a change a command made to the set reaches the engine on the
/// first frame after the set changes, and not before.
fn lands_on_its_frame(offline: &mut Offline, shell: &Shell, from: f32, to: f32) {
    assert_eq!(
        block(offline),
        vec![from; FRAMES],
        "unchanged until it lands"
    );
    offline.sync(shell.editor().document());
    glides_from_the_first_frame(offline, from, to);
}

// Spec: device-framework R23, R24, R25
#[test]
fn a_kit_plugin_compiled_in_serves_its_commands_as_device_commands() {
    let scratch = std::env::temp_dir().join(format!("bl-agent-kit-native-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch).unwrap();
    let mut client = client(false);
    let track = audio_track(&client.transport().shell);

    // The kit's examples are in the device list, and an agent adds one.
    let types = client.transport().shell.kit().device_types();
    assert_eq!(types.len(), 2, "Example Gain and Example Sine");
    let added = run(
        &mut client,
        "device.add",
        json!({
            "device": {
                "kind": { "native": { "device_type": lspk_example_gain::ID } },
                "name": "Example Gain",
                "role": "audio_effect",
            },
            "track": track,
        }),
    );
    let device: DeviceId = serde_json::from_value(added["results"][0]["result"]["device"].clone())
        .expect("the new device");
    let reset = format!("device.d{}.gain.reset", device.0.get());

    // Listed, described and queried like any other device command.
    let commands = listed(&mut client, &format!("device.d{}.", device.0.get()));
    assert_eq!(commands.len(), 1, "{commands:?}");
    assert_eq!(commands[0]["command"], reset.as_str());
    assert_eq!(commands[0]["label"], "Example Gain: Reset Gain");
    assert_eq!(commands[0]["permission"], "edit");
    assert_eq!(commands[0]["access"], "runs");
    let described = call(&mut client, "describe_command", json!({ "command": reset }));
    assert!(
        described["description"]
            .as_str()
            .unwrap()
            .starts_with("Puts the gain back to 0 dB")
    );
    assert_eq!(described["args"]["type"], "object");
    let queried = call(&mut client, "query", json!({ "id": device }));
    assert_eq!(queried["commands"], json!([reset]));

    // The engine plays the set: a constant through the plug-in's gain.
    dc_source(&mut client.transport().shell, track);
    let mut offline = engine(&client.transport().shell);
    run(
        &mut client,
        "device.set_param",
        json!({ "device": device, "param": GAIN.0, "value": 0.25 }),
    );
    offline.sync(client.transport().shell.editor().document());
    assert!((settled(&mut offline) - 0.25).abs() < 1e-6);

    // The agent's run: one undo step with its name, the set changed in it.
    let history = client.transport().shell.editor().undo_history().len();
    let ran = run(&mut client, &reset, json!({}));
    assert_eq!(ran["undo_step"], "test-agent: Example Gain: Reset Gain");
    assert_eq!(ran["results"][0]["result"], json!(null));
    let shell = &client.transport().shell;
    assert_eq!(shell.editor().undo_history().len(), history + 1);
    assert_eq!(param(shell, device, GAIN), Some(1.0));
    // On its frame in the engine, gliding up from 0.25.
    lands_on_its_frame(&mut offline, &client.transport().shell, 0.25, 1.0);
    // Undo takes the whole run back.
    run(&mut client, "edit.undo", json!({}));
    assert_eq!(param(&client.transport().shell, device, GAIN), Some(0.25));

    // Scheduled, it waits for its bar and runs then as the agent's step.
    let scheduled = call(
        &mut client,
        "schedule",
        json!({ "action": "add", "at": "3.1.1", "command": reset }),
    );
    assert_eq!(scheduled["command"], reset.as_str());
    assert_eq!(scheduled["sample_accurate"], false);

    // Saved and opened again, the device keeps its value and its plug-in
    // state, and its commands come back.
    run(
        &mut client,
        "device.set_param",
        json!({ "device": device, "param": GAIN.0, "value": 0.5 }),
    );
    let shell = &mut client.transport().shell;
    let saved = shell
        .run(SET_SAVE_AS, json!({ "path": scratch.join("Native.blset") }))
        .unwrap();
    let state = shell
        .editor()
        .document()
        .device(device)
        .and_then(|d| d.plugin.clone())
        .expect("its state, taken in before the save");
    assert_eq!(&state.data[..4], b"LSPK");
    shell.run(SET_NEW, json!({})).unwrap();
    assert!(shell.kit().save_state(device).is_none(), "let go of");
    shell
        .run(SET_OPEN, json!({ "path": saved["path"] }))
        .unwrap();
    assert_eq!(param(shell, device, GAIN), Some(0.5));
    let reopened = shell.kit().save_state(device).expect("a new instance");
    let decoded = lspk_plugin::state::decode(&reopened.data).unwrap();
    assert_eq!(decoded.params, vec![(GAIN, 0.5)]);
    let ran = run(&mut client, &reset, json!({}));
    assert_eq!(ran["undo_step"], "test-agent: Example Gain: Reset Gain");
    let _ = std::fs::remove_dir_all(&scratch);
}

// Spec: clap-hosting R18, R19
#[test]
fn a_kit_plugin_as_clap_serves_its_commands_through_the_extension() {
    let scratch = std::env::temp_dir().join(format!("bl-agent-kit-clap-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch).unwrap();
    // Loading a plug-in reaches outside the set, so this agent is
    // autonomous.
    let mut client = client(true);
    let path = scratch.join("Example Gain.clap");
    // SAFETY: the kit's CLAP export follows the CLAP specification.
    unsafe {
        client
            .transport()
            .shell
            .plugins()
            .add_entry(&path, &GAIN_CLAP)
    }
    .unwrap();
    let track = audio_track(&client.transport().shell);
    let loaded = run(
        &mut client,
        "plugin.load",
        json!({ "path": path, "track": track }),
    );
    let device: DeviceId =
        serde_json::from_value(loaded["results"][0]["result"]["device"].clone()).unwrap();
    let prefix = format!("device.d{}.", device.0.get());

    // Its plug-in starts when an engine plays the set; then its commands
    // are device commands, without the ones for its own agent server.
    dc_source(&mut client.transport().shell, track);
    let mut offline = engine(&client.transport().shell);
    client.transport().shell.poll();
    let commands = listed(&mut client, &prefix);
    let ids: Vec<&str> = commands
        .iter()
        .map(|c| c["command"].as_str().unwrap())
        .collect();
    let reset = format!("{prefix}gain.reset");
    for id in ["gain.reset", "params.get", "params.set"] {
        assert!(
            ids.contains(&format!("{prefix}{id}").as_str()),
            "{id} in {ids:?}"
        );
    }
    assert!(!ids.iter().any(|id| id.contains(".agent.")), "{ids:?}");
    let permission = |id: &str| {
        commands
            .iter()
            .find(|c| c["command"] == format!("{prefix}{id}").as_str())
            .map(|c| c["permission"].clone())
    };
    assert_eq!(permission("params.get"), Some(json!("read")));
    assert_eq!(permission("gain.reset"), Some(json!("edit")));

    // A command that sets a value, then one that resets it: each one undo
    // step with the agent's name, the set holding what the plug-in reports.
    let set = call(
        &mut client,
        "run",
        json!({
            "commands": [{
                "command": format!("{prefix}params.set"),
                "args": { "values": [{ "param": "Gain", "text": "-12.0412 dB" }] },
            }],
            "label": "Quieter",
        }),
    );
    assert_eq!(set["undo_step"], "test-agent: Quieter");
    let quiet = param(&client.transport().shell, device, GAIN).expect("set");
    offline.sync(client.transport().shell.editor().document());
    assert!((settled(&mut offline) - 0.25).abs() < 1e-4);
    let history = client.transport().shell.editor().undo_history().len();
    let ran = run(&mut client, &reset, json!({}));
    assert_eq!(ran["undo_step"], "test-agent: Example Gain: Reset Gain");
    let shell = &mut client.transport().shell;
    assert_eq!(shell.editor().undo_history().len(), history + 1);
    let unity = param(shell, device, GAIN).expect("reset");
    assert!(unity > quiet);
    let info = shell
        .run("plugin.info", json!({ "device": device }))
        .unwrap();
    assert_eq!(info["values"], json!([[GAIN.0, unity]]));
    // The plug-in applies its own change from the first frame of its next
    // block, as in any CLAP host, and sends it as a gesture, which comes
    // back to the host already in the set: no second undo step.
    glides_from_the_first_frame(&mut offline, 0.25, 1.0);
    shell
        .run("plugin.info", json!({ "device": device }))
        .unwrap();
    shell.poll();
    let steps: Vec<&str> = shell.editor().undo_history().collect();
    assert_eq!(steps.len(), history + 1, "{steps:?}");
    offline.sync(shell.editor().document());
    assert!((settled(&mut offline) - 1.0).abs() < 1e-6);

    // Saved and opened again, a new instance starts from the saved state.
    run(
        &mut client,
        &format!("{prefix}params.set"),
        json!({ "values": [{ "param": 1, "value": 0.5 }] }),
    );
    let half = param(&client.transport().shell, device, GAIN).expect("half");
    let shell = &mut client.transport().shell;
    let saved = shell
        .run(SET_SAVE_AS, json!({ "path": scratch.join("Clap.blset") }))
        .unwrap();
    shell.run(SET_NEW, json!({})).unwrap();
    shell
        .run(SET_OPEN, json!({ "path": saved["path"] }))
        .unwrap();
    drop(offline);
    let _offline = engine(shell);
    shell.poll();
    let info = shell
        .run("plugin.info", json!({ "device": device }))
        .unwrap();
    assert_eq!(info["values"], json!([[GAIN.0, half]]));
    let got = run(&mut client, &format!("{prefix}params.get"), json!({}));
    assert_eq!(got["results"][0]["result"]["params"][0]["text"], "-6.0 dB");
    let _ = std::fs::remove_dir_all(&scratch);
}

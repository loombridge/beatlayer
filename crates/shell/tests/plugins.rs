//! Plug-in devices through the shell (WP-2.6; the CLAP hosting spec, R1,
//! R5, R8, R11, R13, R16): the test plug-in, built from source, loads by
//! path as a device the Device View describes, takes parameter changes,
//! opens its window, and keeps its state when the set is saved and opened
//! again, all through registered commands as an agent runs them.

use std::time::{Duration, Instant};

use bl_model::{DeviceId, ParamId, TrackKind};
use bl_shell::device_commands::{DeviceParameters, DeviceViewState};
use bl_shell::plugin_commands::{self as ids, PluginInfo, PluginLoaded};
use bl_shell::{Shell, commands};
use bl_test_clap::{GAIN, PLUGIN_ID, PLUGIN_NAME};
use serde_json::{Value, json};

fn audio_track(shell: &Shell) -> bl_model::TrackId {
    shell
        .editor()
        .document()
        .tracks()
        .find(|t| t.kind() == TrackKind::Audio)
        .map(bl_model::Track::id)
        .expect("the default set has an audio track")
}

/// Polls the shell, as the app's frames do, until the device's plug-in
/// reports `ready`.
fn wait(shell: &mut Shell, device: DeviceId, ready: impl Fn(&PluginInfo) -> bool) -> PluginInfo {
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        shell.poll();
        if let Ok(value) = shell.run(ids::INFO, json!({ "device": device }))
            && let Ok(info) = serde_json::from_value::<PluginInfo>(value)
            && ready(&info)
        {
            return info;
        }
        assert!(Instant::now() < until, "the plug-in never got ready");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn value(info: &PluginInfo, param: u32) -> Option<f64> {
    info.values
        .iter()
        .find(|(id, _)| *id == ParamId(param))
        .map(|(_, v)| *v)
}

// Spec: clap-hosting R1, R5, R8, R11, R13, R16
#[test]
fn a_plugin_loads_runs_and_keeps_its_state_with_the_set() {
    let scratch = std::env::temp_dir().join(format!("bl-shell-plugins-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch).unwrap();

    let mut shell = Shell::headless();
    shell.start_audio();
    assert!(shell.audio().engine_running(), "{:?}", shell.audio());
    let track = audio_track(&shell);
    let loaded: PluginLoaded = serde_json::from_value(
        shell
            .run(
                ids::LOAD,
                json!({ "path": bl_test_clap::bundle(), "track": track }),
            )
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        (loaded.plugin.as_str(), loaded.name.as_str()),
        (PLUGIN_ID, PLUGIN_NAME)
    );
    assert_eq!(shell.editor().undo_history().last(), Some("Insert Device"));
    let device = loaded.device;

    // The Device View describes it, and agents read its parameters.
    shell
        .run("device.select", json!({ "device": device }))
        .unwrap();
    let view: DeviceViewState =
        serde_json::from_value(shell.run("device_view.state", json!({})).unwrap()).unwrap();
    let frame = view.devices.iter().find(|d| d.id == device).unwrap();
    assert!(frame.selected);
    assert_eq!(frame.panel.as_ref().unwrap().description.params.len(), 3);
    let params: DeviceParameters = serde_json::from_value(
        shell
            .run("device.parameters", json!({ "device": device }))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(params.params[0].descriptor.name, "Gain");

    // The engine runs it, and a parameter change reaches it.
    let info = wait(&mut shell, device, |i| i.active);
    assert_eq!(info.latency, 0);
    assert_eq!(info.failure, None);
    shell
        .run(
            "device.set_param",
            json!({ "device": device, "param": GAIN, "value": 0.5 }),
        )
        .unwrap();
    wait(&mut shell, device, |i| value(i, GAIN) == Some(0.5));

    // Its window opens and closes, changing nothing in the set.
    let history = shell.editor().undo_history().len();
    let shown: PluginInfo = serde_json::from_value(
        shell
            .run(ids::SHOW_WINDOW, json!({ "device": device }))
            .unwrap(),
    )
    .unwrap();
    assert!(shown.has_window && shown.window_open);
    let hidden: PluginInfo = serde_json::from_value(
        shell
            .run(ids::SHOW_WINDOW, json!({ "device": device }))
            .unwrap(),
    )
    .unwrap();
    assert!(!hidden.window_open);
    assert_eq!(shell.editor().undo_history().len(), history);

    // Saved, the set holds its state, taken in without an undo step.
    let history = shell.editor().undo_history().len();
    let saved = shell
        .run(
            commands::SET_SAVE_AS,
            json!({ "path": scratch.join("Plug.blset") }),
        )
        .unwrap();
    let path = saved["path"].as_str().unwrap().to_owned();
    let state = shell
        .editor()
        .document()
        .device(device)
        .and_then(|d| d.plugin.clone())
        .expect("the state, taken in before the save");
    assert_eq!(&state.data[..4], b"BLTC");
    assert_eq!(shell.editor().undo_history().len(), history);

    // Opened again, a new plug-in instance starts from it.
    shell.run(commands::SET_NEW, json!({})).unwrap();
    shell
        .run(commands::SET_OPEN, json!({ "path": path }))
        .unwrap();
    let info = wait(&mut shell, device, |i| i.active);
    assert_eq!(value(&info, GAIN), Some(0.5));
    assert_eq!(
        shell.editor().document().device(device).unwrap().plugin,
        Some(state)
    );

    // A device that isn't a plug-in has no plug-in to report.
    let not_a_plugin: Value = json!({ "device": DeviceId(bl_core::id::Id::new(999_999).unwrap()) });
    assert!(shell.run(ids::INFO, not_a_plugin).is_err());
    let _ = std::fs::remove_dir_all(&scratch);
}

// Spec: clap-hosting R12
#[test]
fn a_file_that_isnt_a_plugin_is_refused() {
    let mut shell = Shell::headless();
    let track = audio_track(&shell);
    let before = shell.editor().undo_history().len();
    let refused = shell.run(
        ids::LOAD,
        json!({ "path": "/nowhere/Missing.clap", "track": track }),
    );
    assert!(refused.is_err());
    assert_eq!(shell.editor().undo_history().len(), before);
}

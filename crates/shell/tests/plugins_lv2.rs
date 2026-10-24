//! LV2 plug-in devices through the shell (WP-5.39; the LV2 hosting spec, R1,
//! R2, R3, R5, R8, R10, R11, R17): the test plug-in, built from source, loads by
//! its bundle as a device the Device View describes, takes parameter changes,
//! reports its latency, keeps its state when the set is saved and opened
//! again, is found by the scanner on Linux, and plays in an offline render,
//! all through registered commands as an agent runs them. They run where
//! lilv is: on Linux always, elsewhere when `BL_TEST_LILV` names a lilv
//! library.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use bl_core::time::Ticks;
use bl_model::{DeviceId, ParamId, TrackId};
use bl_shell::device_commands::{DeviceParameters, DeviceViewState};
use bl_shell::export::{self, Length, Render};
use bl_shell::plugin_commands::{self as ids, PluginInfo, PluginLoaded};
use bl_shell::{Shell, browser_commands, commands};
use bl_test_lv2::{GAIN, LATENCY, PLUGIN_NAME, PLUGIN_URI};
use serde_json::json;

/// A headless shell whose LV2 host has lilv, or `None` where there is none.
fn shell() -> Option<Shell> {
    let shell = Shell::headless();
    if let Some(library) = std::env::var_os("BL_TEST_LILV") {
        shell.lv2().use_library(library);
    }
    shell.lv2().world().ok().map(|_| shell)
}

fn value(info: &PluginInfo, param: u32) -> Option<f64> {
    info.values
        .iter()
        .find(|(id, _)| *id == ParamId(param))
        .map(|(_, v)| *v)
}

/// Polls the shell, as the app's frames do, until the device's plug-in
/// reports `ready`: a condition, not a deadline.
fn until(shell: &mut Shell, device: DeviceId, ready: impl Fn(&PluginInfo) -> bool) -> PluginInfo {
    loop {
        shell.poll();
        if let Ok(value) = shell.run(ids::INFO, json!({ "device": device }))
            && let Ok(info) = serde_json::from_value::<PluginInfo>(value)
            && ready(&info)
        {
            return info;
        }
        std::thread::yield_now();
    }
}

fn track(shell: &Shell) -> TrackId {
    shell
        .editor()
        .document()
        .tracks()
        .find(|t| t.kind() == bl_model::TrackKind::Audio)
        .map(bl_model::Track::id)
        .expect("the default set has an audio track")
}

/// Loads the test plug-in onto `track` by its bundle.
fn load(shell: &mut Shell, track: TrackId) -> PluginLoaded {
    serde_json::from_value(
        shell
            .run(
                ids::LOAD,
                json!({ "path": bl_test_lv2::bundle(), "track": track }),
            )
            .unwrap(),
    )
    .unwrap()
}

// Spec: lv2-hosting R1, R5, R17. `plugin.load` takes a bundle folder and
// offers its plug-in as a device the Device View describes.
#[test]
fn an_lv2_plugin_loads_by_bundle_and_the_device_view_describes_it() {
    let Some(mut shell) = shell() else { return };
    let track = track(&shell);
    let loaded = load(&mut shell, track);
    assert_eq!(
        (loaded.plugin.as_str(), loaded.name.as_str()),
        (PLUGIN_URI, PLUGIN_NAME)
    );
    assert_eq!(shell.editor().undo_history().next(), Some("Insert Device"));
    let device = loaded.device;
    shell
        .run("device.select", json!({ "device": device }))
        .unwrap();
    let view: DeviceViewState =
        serde_json::from_value(shell.run("device_view.state", json!({})).unwrap()).unwrap();
    let frame = view.devices.iter().find(|d| d.id == device).unwrap();
    assert!(frame.selected);
    assert_eq!(frame.panel.as_ref().unwrap().description.params.len(), 5);
    let params: DeviceParameters = serde_json::from_value(
        shell
            .run("device.parameters", json!({ "device": device }))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(params.params[0].descriptor.name, "Gain");

    // A bundle that isn't one is refused, and nothing changes.
    let history = shell.editor().undo_history().len();
    let refused = shell.run(
        ids::LOAD,
        json!({ "path": std::env::temp_dir(), "track": track }),
    );
    assert!(refused.is_err());
    assert_eq!(shell.editor().undo_history().len(), history);
}

// Spec: lv2-hosting R2, R3, R8, R10, R11, R17. The engine runs it, a
// parameter change reaches the plug-in, its latency is reported, and its
// state round-trips a saved set with the bundle it came from.
#[test]
fn an_lv2_plugin_runs_reports_its_latency_and_keeps_its_state_with_the_set() {
    let Some(mut shell) = shell() else { return };
    let scratch = std::env::temp_dir().join(format!("bl-shell-lv2-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch).unwrap();

    shell.start_audio();
    assert!(shell.audio().engine_running(), "{:?}", shell.audio());
    let track = track(&shell);
    let device = load(&mut shell, track).device;

    let info = until(&mut shell, device, |i| i.active);
    assert_eq!(info.failure, None);
    shell
        .run(
            "device.set_param",
            json!({ "device": device, "param": GAIN, "value": 0.5 }),
        )
        .unwrap();
    // Its first blocks tell the engine the latency, and `plugin.info` says
    // it.
    let info = until(&mut shell, device, |i| {
        value(i, GAIN) == Some(0.5) && i.latency == LATENCY
    });
    assert_eq!(info.failure, None);

    // Saved, the set holds the plug-in's state with its bundle, taken in
    // without an undo step.
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
    assert!(state.path.ends_with("test-gain-echo.lv2"), "{}", state.path);
    let text = String::from_utf8(state.data.clone()).expect("lilv writes text");
    assert!(text.contains("urn:beatlayer:state"), "{text}");
    assert_eq!(shell.editor().undo_history().len(), history);

    // Opened again, a new plug-in instance starts from it.
    shell.run(commands::SET_NEW, json!({})).unwrap();
    shell
        .run(commands::SET_OPEN, json!({ "path": path }))
        .unwrap();
    let info = until(&mut shell, device, |i| i.active);
    assert_eq!(value(&info, GAIN), Some(0.5));
    assert_eq!(
        shell.editor().document().device(device).unwrap().plugin,
        Some(state)
    );
    let _ = std::fs::remove_dir_all(&scratch);
}

// Spec: plugin-scanning R1, R2, R7, R8, R10. On Linux `plugin.scan` finds the
// bundle and `plugin.list` names it for the set that uses it.
#[cfg(target_os = "linux")]
#[test]
fn the_scanner_finds_an_lv2_bundle_and_the_set_list_names_it() {
    use bl_model::PluginFormat;
    use bl_shell::plugin_commands::{PluginScan, SetPluginList};

    if bl_plugin_host::scan::scan_program().is_none() {
        return;
    }
    let mut shell = Shell::headless();
    let bundle = bl_test_lv2::bundle().to_string_lossy().into_owned();
    let scan: PluginScan = serde_json::from_value(
        shell
            .run(ids::SCAN, json!({ "paths": [bundle.clone()] }))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(scan.plugins.len(), 1, "{:?}", scan.blocked);
    assert_eq!(scan.plugins[0].format, PluginFormat::Lv2);
    assert_eq!(scan.plugins[0].plugin_id, PLUGIN_URI);
    assert!(scan.plugins[0].note_output);
    let track = track(&shell);
    load(&mut shell, track);
    let list: SetPluginList =
        serde_json::from_value(shell.run(ids::LIST, json!({})).unwrap()).unwrap();
    assert_eq!(list.plugins.len(), 1);
    assert_eq!(list.plugins[0].plugin_id, PLUGIN_URI);
    assert!(!list.plugins[0].missing, "the scanner knows it");
}

/// Writes a 16-bit mono WAV file of one second at 48 kHz: a ramp.
fn wav(path: &Path) {
    let frames = 48_000u32;
    let mut bytes = Vec::new();
    let data = frames * 2;
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&48_000u32.to_le_bytes());
    bytes.extend_from_slice(&96_000u32.to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data.to_le_bytes());
    for i in 0..frames {
        let sample = ((i % 100) as i16 - 50) * 200;
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    std::fs::write(path, bytes).unwrap();
}

/// The set rendered as exports do, borrowing the shell's plug-ins, with
/// blocks of `block` frames.
fn render(shell: &Shell, block: usize) -> Vec<f32> {
    let spec = Render {
        start: Ticks::ZERO,
        length: Length::Frames(24_000),
        sample_rate: 48_000,
        engine_rate: 48_000,
        block,
    };
    let mut samples = Vec::new();
    let rendered = export::render(
        shell.editor().document(),
        shell.media_store(),
        shell.plugin_hosts(),
        &spec,
        60.0,
        &AtomicBool::new(false),
        &mut |_| {},
        &mut |out| {
            samples.extend_from_slice(out);
            Ok(())
        },
        None,
    )
    .expect("the render");
    assert_eq!(rendered.heap_operations, 0, "the real-time rules");
    samples
}

// Spec: offline-plugin-renders R1, R3, R4, R5, R11. An export plays the
// plug-in, with the set's values, the same at any block size, and leaves the
// engine's plug-in as it was.
#[test]
fn an_export_renders_the_lv2_plugin_and_the_engines_one_is_untouched() {
    let Some(mut shell) = shell() else { return };
    let scratch: PathBuf =
        std::env::temp_dir().join(format!("bl-shell-lv2-ex-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch).unwrap();
    let file = scratch.join("Ramp.wav");
    wav(&file);
    shell
        .run(
            browser_commands::ADD_TO_ARRANGEMENT,
            json!({ "paths": [file.to_string_lossy()], "at": 0 }),
        )
        .unwrap();
    let track = shell
        .editor()
        .document()
        .tracks()
        .find(|t| t.clips().next().is_some())
        .map(bl_model::Track::id)
        .expect("the clip's track");
    let device = load(&mut shell, track).device;
    let set_gain = |shell: &mut Shell, gain: f64| {
        shell
            .run(
                "device.set_param",
                json!({ "device": device, "param": GAIN, "value": gain }),
            )
            .unwrap();
    };

    set_gain(&mut shell, 1.0);
    let whole = render(&shell, 512);
    assert!(whole.iter().any(|s| *s != 0.0), "the clip plays");
    set_gain(&mut shell, 0.5);
    let half = render(&shell, 512);
    // The plug-in is played, at the rendered set's gain: it halves the ramp.
    assert_eq!(half.len(), whole.len());
    for (half, whole) in half.iter().zip(&whole) {
        assert_eq!(*half, whole * 0.5);
    }
    // The same at any block size, run after run.
    for block in [64, 4096] {
        assert_eq!(render(&shell, block), half, "blocks of {block}");
    }
    assert!(shell.lv2().lend().is_ok(), "every loan ended");
    let _ = std::fs::remove_dir_all(&scratch);
}

//! A plug-in's presets under its row in the browser (WP-6.66; the plug-in
//! scanning spec, R15 to R20): Save Preset on a plug-in device keeps its own
//! state, the preset lists under the plug-in's row, and loading it puts the
//! state in a new device, in the device being hot-swapped, or in place of
//! one of another plug-in, all through registered commands as an agent runs
//! them.

use std::path::PathBuf;
use std::time::Duration;

use bl_model::{DeviceId, TrackId};
use bl_shell::browser_commands::{self as browser_ids, BrowserState, Item, ItemKind};
use bl_shell::plugin_commands::{self as ids, PluginInfo, PluginLoaded};
use bl_shell::{Area, Shell};
use serde_json::{Value, json};

/// The plug-in scanner's child process is built beside these tests; a run
/// that didn't build it can't scan.
fn ready() -> bool {
    bl_plugin_host::scan::scan_program().is_some()
}

/// A scratch folder for a User Library, removed when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("bl-plugin-presets-{}-{name}", std::process::id()));
        std::fs::remove_dir_all(&path).ok();
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

/// One format of the test plug-in: where it is, and how a test reads and
/// gives the state only the running plug-in holds.
struct Format {
    bundle: String,
    key: String,
    held: fn(&Shell, DeviceId) -> Vec<u8>,
    give: fn(&Shell, DeviceId, u8),
}

fn clap() -> Format {
    let bundle = bl_test_clap::bundle().to_string_lossy().into_owned();
    Format {
        key: format!("{bundle}#{}", bl_test_clap::PLUGIN_ID),
        bundle,
        held: |shell, device| shell.plugins().save_state(device).unwrap().data,
        give: |shell, device, step| {
            let mut state = shell.plugins().save_state(device).unwrap().data;
            state[16..24].copy_from_slice(&f64::from(step).to_le_bytes());
            shell.plugins().load_state(device, &state).unwrap();
        },
    }
}

fn vst3() -> Format {
    let bundle = bl_test_vst3::bundle().to_string_lossy().into_owned();
    Format {
        key: format!("{bundle}#{}", bl_test_vst3::plugin_id()),
        bundle,
        held: |shell, device| shell.vst3().save_state(device).unwrap().data,
        give: |shell, device, step| {
            let state = bl_plugin_host::vst3::plugin_state(
                &[],
                &bl_test_vst3::gesture_state(step, step.wrapping_add(16)),
            );
            shell.vst3().load_state(device, &state).unwrap();
        },
    }
}

/// A headless shell with a scratch User Library, the engine running, and the
/// test plug-ins scanned and listed.
fn shell(library: &Scratch) -> Shell {
    let mut shell = Shell::headless();
    shell.set_user_library(Some(library.0.clone()));
    shell.start_audio();
    assert!(shell.audio().engine_running(), "{:?}", shell.audio());
    let paths = [clap().bundle, vst3().bundle];
    shell.run(ids::SCAN, json!({ "paths": paths })).unwrap();
    shell
        .run(ids::SETTINGS, json!({ "vst3_system_folders": true }))
        .unwrap();
    shell
        .run(browser_ids::OPEN_PLACE, json!({ "label": "plug_ins" }))
        .unwrap();
    shell
        .run(
            browser_ids::OPEN_FOLDER,
            json!({ "path": "plug-ins:/Beatlayer tests", "open": true }),
        )
        .unwrap();
    shell
}

fn browser(shell: &mut Shell) -> BrowserState {
    serde_json::from_value(shell.run(browser_ids::STATE, json!({})).unwrap()).unwrap()
}

fn row(shell: &mut Shell, key: &str) -> Option<Item> {
    browser(shell).rows.into_iter().find(|row| row.path == key)
}

/// The new audio track the test's devices go on.
fn new_track(shell: &mut Shell) -> TrackId {
    let before: Vec<TrackId> = shell
        .editor()
        .document()
        .tracks()
        .map(bl_model::Track::id)
        .collect();
    shell
        .invoke("track.create", json!({ "kind": "audio" }))
        .unwrap();
    shell
        .editor()
        .document()
        .tracks()
        .map(bl_model::Track::id)
        .find(|t| !before.contains(t))
        .unwrap()
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

/// Selects `track` and the browser's `row`, and loads it as Enter does.
fn load_row(shell: &mut Shell, row: &str, track: TrackId) -> PluginLoaded {
    shell
        .run(
            bl_shell::arrangement_commands::SELECT_TRACKS,
            json!({ "tracks": [track] }),
        )
        .unwrap();
    shell
        .run(browser_ids::SELECT, json!({ "paths": [row] }))
        .unwrap();
    serde_json::from_value(shell.run(browser_ids::LOAD_SELECTED, json!({})).unwrap()).unwrap()
}

fn last_step(shell: &Shell) -> String {
    shell
        .editor()
        .undo_history()
        .next()
        .unwrap_or_default()
        .to_owned()
}

/// Gives the running plug-in a state, and tells the set the parameter values
/// the plug-in now reports, as the plug-in's own gestures would, so that the
/// set's values and the plug-in's state agree.
fn give_state(shell: &mut Shell, format: &Format, device: DeviceId, step: u8) {
    (format.give)(shell, device, step);
    let values = until(shell, device, |_| true).values;
    for (param, value) in values {
        let held = shell
            .editor()
            .document()
            .device(device)
            .unwrap()
            .params
            .get(&param)
            .copied();
        if held != Some(value) {
            shell
                .invoke(
                    "device.set_param",
                    json!({ "device": device, "param": param, "value": value }),
                )
                .unwrap();
        }
    }
}

/// Loads the plug-in from its row onto a new track, running, with a state
/// only it holds, and saves a preset from it: the device, that state, and
/// the preset's file.
fn saved_preset(shell: &mut Shell, format: &Format, step: u8) -> (DeviceId, Vec<u8>, String) {
    let track = new_track(shell);
    let device = load_row(shell, &format.key, track).device;
    until(shell, device, |i| i.active);
    give_state(shell, format, device, step);
    let held = (format.held)(shell, device);
    let saved: Value = shell
        .run("device.save_preset", json!({ "device": device }))
        .unwrap();
    let path = saved["path"].as_str().unwrap().to_owned();
    assert!(shell.wait_for_browser_index(Duration::from_secs(30)));
    (device, held, path)
}

// Spec: plugin-scanning R15, R16, R17, R20. Save Preset writes the plug-in's
// state, the preset lists under the plug-in's row, and loading it by its row
// puts the state in a new device in one undo step.
fn a_preset_keeps_a_plug_ins_state_lists_under_its_row_and_loads_into_a_new_device(
    format: &Format,
) {
    let library = Scratch::new("round-trip");
    let mut shell = shell(&library);
    let plain = row(&mut shell, &format.key).expect("the plug-in's row");
    assert!(!plain.presets, "no presets yet, no fold triangle");

    let (_, held, path) = saved_preset(&mut shell, format, 60);
    let preset = bl_project::preset::Preset::load(std::path::Path::new(&path)).unwrap();
    assert_eq!(preset.state.as_deref(), Some(held.as_slice()));

    // The row has a fold triangle, closed at first; opening it lists the
    // preset by name beneath it.
    let closed = row(&mut shell, &format.key).unwrap();
    assert!(closed.presets && !closed.open, "{closed:?}");
    shell
        .run(browser_ids::OPEN_FOLDER, json!({ "path": format.key }))
        .unwrap();
    let key = format!("{}#preset/{path}", format.key);
    let listed = row(&mut shell, &key).expect("the preset's row");
    assert_eq!(listed.kind, ItemKind::Preset);
    assert_eq!(
        listed.name,
        bl_project::preset::preset_name(std::path::Path::new(&path))
    );
    assert_eq!(listed.depth, closed.depth + 1);
    assert!(row(&mut shell, &format.key).unwrap().open);

    // Enter on it loads the plug-in with the state, as one undo step.
    let track = new_track(&mut shell);
    let steps = shell.editor().undo_history().len();
    let loaded = load_row(&mut shell, &key, track);
    assert_eq!(shell.editor().undo_history().len(), steps + 1);
    assert_eq!(last_step(&shell), "Load Preset");
    let device = shell.editor().document().device(loaded.device).unwrap();
    assert_eq!(device.plugin.as_ref().unwrap().data, held);
    assert_eq!(device.preset_name.as_deref(), Some(listed.name.as_str()));
    until(&mut shell, loaded.device, |i| i.active);
    assert_eq!((format.held)(&shell, loaded.device), held);
    shell.run("edit.undo", json!({})).unwrap();
    assert!(shell.editor().document().device(loaded.device).is_none());
}

#[test]
fn a_clap_plug_ins_preset_lists_under_its_row_and_loads_its_state() {
    if !ready() {
        eprintln!("beatlayer-scan wasn't built; skipping (build the workspace's bins)");
        return;
    }
    a_preset_keeps_a_plug_ins_state_lists_under_its_row_and_loads_into_a_new_device(&clap());
}

#[test]
fn a_vst3_plug_ins_preset_lists_under_its_row_and_loads_its_state() {
    if !ready() {
        eprintln!("beatlayer-scan wasn't built; skipping (build the workspace's bins)");
        return;
    }
    a_preset_keeps_a_plug_ins_state_lists_under_its_row_and_loads_into_a_new_device(&vst3());
}

// Spec: plugin-scanning R15, browser-tags R20. The presets open under a
// plug-in's row leave the list with it when a filter hides it.
#[test]
fn a_filtered_out_plug_ins_presets_leave_with_its_row() {
    if !ready() {
        eprintln!("beatlayer-scan wasn't built; skipping (build the workspace's bins)");
        return;
    }
    let library = Scratch::new("filtered");
    let mut shell = shell(&library);
    let format = clap();
    let (_, _, path) = saved_preset(&mut shell, &format, 30);
    shell
        .run(browser_ids::OPEN_FOLDER, json!({ "path": format.key }))
        .unwrap();
    let key = format!("{}#preset/{path}", format.key);
    assert!(row(&mut shell, &key).is_some(), "the preset's row");
    shell
        .run(
            browser_ids::FILTER,
            json!({ "tags": ["Format|VST3"], "add": false }),
        )
        .unwrap();
    assert!(
        row(&mut shell, &format.key).is_none(),
        "the CLAP row is filtered out"
    );
    assert!(row(&mut shell, &key).is_none(), "and so is its preset");
}

// Spec: plugin-scanning R16. A plug-in device's preset saved before the state
// field has no state, and loads its parameter values only.
#[test]
fn a_preset_saved_before_loads_its_values_only() {
    if !ready() {
        eprintln!("beatlayer-scan wasn't built; skipping (build the workspace's bins)");
        return;
    }
    let format = clap();
    let library = Scratch::new("older");
    let mut shell = shell(&library);
    let (_, _, path) = saved_preset(&mut shell, &format, 60);
    let mut file: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    file.as_object_mut().unwrap().remove("state");
    file["params"] = json!([[bl_test_clap::GAIN, 0.5]]);
    std::fs::write(&path, serde_json::to_vec(&file).unwrap()).unwrap();
    shell
        .run(browser_ids::OPEN_FOLDER, json!({ "path": format.key }))
        .unwrap();
    let key = format!("{}#preset/{path}", format.key);
    let track = new_track(&mut shell);
    let loaded = load_row(&mut shell, &key, track);
    let device = shell.editor().document().device(loaded.device).unwrap();
    assert_eq!(
        device.params.get(&bl_model::ParamId(bl_test_clap::GAIN)),
        Some(&0.5)
    );
    assert!(
        device.plugin.as_ref().is_none_or(|p| p.data.is_empty()),
        "no state came with it"
    );
}

// Spec: plugin-scanning R17; racks R45. A preset of the plug-in being
// hot-swapped loads into its device in place; a preset of another plug-in of
// its role replaces the device, and hot-swap stays on, now on the new one.
#[test]
fn a_preset_loads_into_the_device_being_hot_swapped_or_replaces_it() {
    if !ready() {
        eprintln!("beatlayer-scan wasn't built; skipping (build the workspace's bins)");
        return;
    }
    let (clap, vst3) = (clap(), vst3());
    let library = Scratch::new("hot-swap");
    let mut shell = shell(&library);
    let (clap_device, held, path) = saved_preset(&mut shell, &clap, 60);
    let key = format!("{}#preset/{path}", clap.key);
    shell
        .run(browser_ids::OPEN_FOLDER, json!({ "path": clap.key }))
        .unwrap();

    // Another state in the device, then the same plug-in's preset puts the
    // saved one back in the same device, as one step.
    give_state(&mut shell, &clap, clap_device, 7);
    assert_ne!((clap.held)(&shell, clap_device), held);
    shell
        .run("device.hot_swap", json!({ "device": clap_device }))
        .unwrap();
    shell.set_focused_area(Area::Browser);
    let steps = shell.editor().undo_history().len();
    shell
        .run(browser_ids::SELECT, json!({ "paths": [key] }))
        .unwrap();
    shell.run(browser_ids::LOAD_SELECTED, json!({})).unwrap();
    assert_eq!(shell.editor().undo_history().len(), steps + 1);
    assert_eq!(last_step(&shell), "Load Preset");
    assert_eq!((clap.held)(&shell, clap_device), held);
    assert_eq!(
        shell.device_view_state().hot_swap.map(|h| h.device),
        Some(clap_device),
        "hot-swap stays on the device"
    );

    // A VST3 plug-in's preset replaces it, and hot-swap moves to the new
    // device.
    shell
        .run("device.hot_swap", json!({ "on": false }))
        .unwrap();
    let (_, vst3_held, vst3_path) = saved_preset(&mut shell, &vst3, 48);
    shell
        .run("device.hot_swap", json!({ "device": clap_device }))
        .unwrap();
    shell
        .run(browser_ids::OPEN_FOLDER, json!({ "path": vst3.key }))
        .unwrap();
    let vst3_key = format!("{}#preset/{vst3_path}", vst3.key);
    shell
        .run(browser_ids::SELECT, json!({ "paths": [vst3_key] }))
        .unwrap();
    let loaded: PluginLoaded =
        serde_json::from_value(shell.run(browser_ids::LOAD_SELECTED, json!({})).unwrap()).unwrap();
    assert!(shell.editor().document().device(clap_device).is_none());
    assert_eq!(
        shell.device_view_state().hot_swap.map(|h| h.device),
        Some(loaded.device)
    );
    until(&mut shell, loaded.device, |i| i.active);
    assert_eq!((vst3.held)(&shell, loaded.device), vst3_held);
}

// Spec: plugin-scanning R18. Arming a plug-in device opens the Plug-Ins label
// at its row with its presets listed, and selects the one it last loaded.
#[test]
fn arming_a_plug_in_device_opens_its_row_with_its_last_preset_selected() {
    if !ready() {
        eprintln!("beatlayer-scan wasn't built; skipping (build the workspace's bins)");
        return;
    }
    let format = clap();
    let library = Scratch::new("arming");
    let mut shell = shell(&library);
    let (device, _, path) = saved_preset(&mut shell, &format, 60);
    // Somewhere else in the browser, with the vendor's folder closed.
    shell
        .run(browser_ids::OPEN_PLACE, json!({ "label": "user_library" }))
        .unwrap();
    shell
        .run(
            browser_ids::OPEN_FOLDER,
            json!({ "path": "plug-ins:/Beatlayer tests", "open": false }),
        )
        .ok();
    shell
        .run("device.hot_swap", json!({ "device": device }))
        .unwrap();
    let state = browser(&mut shell);
    assert_eq!(state.label, browser_ids::Label::PlugIns);
    let key = format!("{}#preset/{path}", format.key);
    assert!(state.rows.iter().any(|row| row.path == key), "{state:?}");
    assert_eq!(state.selected, vec![key]);
}

// Spec: plugin-scanning R15, R19. An Audio Unit's preset files list under its
// row by manufacturer and plug-in, and one loads into the unit as its state.
#[cfg(target_os = "macos")]
#[test]
fn an_audio_units_preset_file_lists_and_loads_into_the_unit() {
    if !ready() {
        eprintln!("beatlayer-scan wasn't built; skipping (build the workspace's bins)");
        return;
    }
    let library = Scratch::new("au");
    let folders = Scratch::new("au-presets");
    let mut shell = Shell::headless();
    shell.set_user_library(Some(library.0.clone()));
    shell.set_audio_unit_preset_folders(vec![folders.0.clone()]);
    shell.start_audio();
    assert!(shell.audio().engine_running(), "{:?}", shell.audio());
    let scan: bl_shell::plugin_commands::PluginScan = serde_json::from_value(
        shell
            .run(
                ids::SCAN,
                json!({ "paths": ["/System/Library/Components"] }),
            )
            .unwrap(),
    )
    .unwrap();
    let Some(delay) = scan
        .plugins
        .iter()
        .find(|p| p.plugin_id == "aufx/dely/appl")
    else {
        eprintln!("AUDelay isn't installed; skipping");
        return;
    };
    let (vendor, name) = (delay.vendor.clone().unwrap(), delay.name.clone());
    shell
        .run(ids::SETTINGS, json!({ "use_audio_units_v2": true }))
        .unwrap();
    shell
        .run(browser_ids::OPEN_PLACE, json!({ "label": "plug_ins" }))
        .unwrap();
    shell
        .run(
            browser_ids::OPEN_FOLDER,
            json!({ "path": format!("plug-ins:/{vendor}"), "open": true }),
        )
        .unwrap();
    let key = browser(&mut shell)
        .rows
        .into_iter()
        .find(|row| row.name == name && row.format == Some(bl_model::PluginFormat::AudioUnit))
        .map(|row| row.path)
        .expect("the unit's row");
    assert!(!row(&mut shell, &key).unwrap().presets);

    // The unit's own state, written where Audio Units keep presets.
    let track = shell
        .editor()
        .document()
        .tracks()
        .find(|t| t.kind() == bl_model::TrackKind::Audio)
        .map(bl_model::Track::id)
        .unwrap();
    let device = load_row(&mut shell, &key, track).device;
    until(&mut shell, device, |i| i.active);
    let state = shell
        .plugin_hosts()
        .au
        .unwrap()
        .save_state(device)
        .unwrap()
        .data;
    let folder = folders.0.join(&vendor).join(&name);
    std::fs::create_dir_all(&folder).unwrap();
    let file = folder.join("Mine.aupreset");
    std::fs::write(&file, &state).unwrap();
    shell.set_audio_unit_preset_folders(vec![folders.0.clone()]);
    assert!(row(&mut shell, &key).unwrap().presets);

    shell
        .run(browser_ids::OPEN_FOLDER, json!({ "path": key }))
        .unwrap();
    let preset_key = format!("{key}#preset/{}", file.display());
    let listed = row(&mut shell, &preset_key).expect("the preset file's row");
    assert_eq!(
        (listed.kind, listed.name.as_str()),
        (ItemKind::Preset, "Mine")
    );
    let other = new_track(&mut shell);
    let steps = shell.editor().undo_history().len();
    let loaded = load_row(&mut shell, &preset_key, other);
    assert_eq!(shell.editor().undo_history().len(), steps + 1);
    let loaded = shell.editor().document().device(loaded.device).unwrap();
    assert_eq!(loaded.preset_name.as_deref(), Some("Mine"));
    assert_eq!(loaded.plugin.as_ref().unwrap().data, state);
}
